//! The hub's records and rules: attach codes, CLI tokens that live with their binding,
//! the binding of a CLI grant to one tab page, routing with the hub's stamp, and the
//! browser broadcast that CLI connections never join. The server (`server.rs`) only
//! moves bytes.
//!
//! Times are milliseconds since the epoch as `f64`: the windows come from JSON and may
//! be fractional.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::random::{base64url, secret, uuid};

/// WebSocket close codes the hub ends or refuses a connection with.
pub mod close_codes {
    /// No token, a wrong or expired token, or a revoked grant.
    pub const UNAUTHORIZED: u16 = 4401;
    /// A CLI token in the query string: the CLI sends it as a header.
    pub const FORBIDDEN: u16 = 4403;
    /// Another live CLI connection holds the grant (§3).
    pub const BUSY: u16 = 4409;
    /// The grant ended while connected (its binding is gone).
    pub const REVOKED: u16 = 4410;
}

/// §5.8: one part at most 16 KB, at most 3 parts.
const MAX_PART_BYTES: usize = 16 * 1024;
const MAX_PARTS: usize = 3;
const MAX_REVISION_LENGTH: usize = 256;
/// §5.9: generous bounds; the hub stores the text, it does not read it.
const MAX_TITLE_LENGTH: usize = 512;
const MAX_PATH_LENGTH: usize = 2048;
/// The prefix of an attach code (attach spec §3.2).
const CODE_PREFIX: &str = "gk1_";
/// The one message of every refused secret: unknown, used and expired look alike.
const INVALID_CODE: &str = "the code is invalid or expired";

/// A fake user of the development hub.
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    /// Whether the user may use the terminal. Default `true`.
    #[serde(default)]
    pub terminal: Option<bool>,
}

/// The hub's windows, in milliseconds.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// Attach codes live this long (attach spec §3.1: 2 min).
    pub attach_ttl_ms: f64,
    /// A binding whose connection dropped waits this long for its page (§5.3: 60 s).
    pub suspend_ms: f64,
    /// A binding whose page said `unload` waits this long for `resume` (attach spec §4:
    /// 10 s).
    pub unload_ms: f64,
    /// A `send` to a suspended binding waits this long for it (attach spec §4: 5 s).
    pub hold_ms: f64,
    /// `expiresAt - stampedAt` of a CLI message (§4: 30 s).
    pub request_ttl_ms: f64,
    /// Access token life (§8: 1 h).
    pub access_ttl_ms: f64,
    /// Refresh token life (§8: 7 days); the binding ends it sooner.
    pub refresh_ttl_ms: f64,
    /// The hub pings each browser connection this often (4 s). With two misses a
    /// silent tab is found 8 to 12 s after its last pong.
    pub heartbeat_ms: f64,
    /// Unanswered pings before a browser connection counts as lost (2).
    pub heartbeat_misses: f64,
}

impl Default for Timings {
    fn default() -> Self {
        Timings {
            attach_ttl_ms: 2.0 * 60_000.0,
            suspend_ms: 60_000.0,
            unload_ms: 10_000.0,
            hold_ms: 5_000.0,
            request_ttl_ms: 30_000.0,
            access_ttl_ms: 60.0 * 60_000.0,
            refresh_ttl_ms: 7.0 * 24.0 * 60.0 * 60_000.0,
            heartbeat_ms: 4_000.0,
            heartbeat_misses: 2.0,
        }
    }
}

impl Timings {
    /// Overrides the windows a JSON object names (`DevHubTimings`, camelCase).
    pub fn merge(&mut self, json: &Map<String, Value>) {
        let fields: [(&str, &mut f64); 9] = [
            ("attachTtlMs", &mut self.attach_ttl_ms),
            ("suspendMs", &mut self.suspend_ms),
            ("unloadMs", &mut self.unload_ms),
            ("holdMs", &mut self.hold_ms),
            ("requestTtlMs", &mut self.request_ttl_ms),
            ("accessTtlMs", &mut self.access_ttl_ms),
            ("refreshTtlMs", &mut self.refresh_ttl_ms),
            ("heartbeatMs", &mut self.heartbeat_ms),
            ("heartbeatMisses", &mut self.heartbeat_misses),
        ];
        for (name, field) in fields {
            if let Some(value) = json.get(name).and_then(Value::as_f64) {
                *field = value;
            }
        }
    }
}

/// Where the hub writes its log lines (never a command line).
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// How the hub is started.
#[derive(Clone)]
pub struct Options {
    /// Default 4400; 0 picks a free port.
    pub port: u16,
    /// Default one user, `u1`.
    pub users: Vec<User>,
    pub timings: Timings,
    /// CLI requests per grant per second on average (§5.1: 5).
    pub rate_per_second: f64,
    /// Requests a grant may spend at once (a token bucket). Default 20 or the rate,
    /// whichever is more: a CLI loop runs about 16 calls a second.
    pub rate_burst: Option<f64>,
    /// Wrong secrets per address per minute before `too-many-attempts`.
    pub max_code_attempts: usize,
    /// The origin of the web app. Given, it is the one origin the browser may call the
    /// hub from; left out, any loopback origin may.
    pub web_origin: Option<String>,
    /// The hub address written into attach codes; left out, `http://127.0.0.1:<port>`.
    pub public_url: Option<String>,
    pub log: Option<Log>,
}

impl std::fmt::Debug for Options {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Options")
            .field("port", &self.port)
            .field("users", &self.users)
            .field("timings", &self.timings)
            .field("rate_per_second", &self.rate_per_second)
            .field("rate_burst", &self.rate_burst)
            .field("web_origin", &self.web_origin)
            .field("public_url", &self.public_url)
            .finish_non_exhaustive()
    }
}

impl Default for Options {
    fn default() -> Self {
        Options {
            port: 4400,
            users: vec![User {
                id: "u1".into(),
                name: "zahid".into(),
                terminal: None,
            }],
            timings: Timings::default(),
            rate_per_second: 5.0,
            rate_burst: None,
            max_code_attempts: 5,
            web_origin: None,
            public_url: None,
            log: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cli,
    Browser,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Cli => "cli",
            Kind::Browser => "browser",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    Suspended,
    Gone,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            State::Active => "active",
            State::Suspended => "suspended",
            State::Gone => "gone",
        }
    }
}

/// The approval level the user chose when making the code (attach spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    AskServer,
    NeverAsk,
}

impl Level {
    fn parse(text: &str) -> Option<Level> {
        match text {
            "ask-server" => Some(Level::AskServer),
            "never-ask" => Some(Level::NeverAsk),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Level::AskServer => "ask-server",
            Level::NeverAsk => "never-ask",
        }
    }
}

#[derive(Debug)]
struct Grant {
    grant_id: String,
    user_id: String,
    env: String,
    created_at: f64,
    revoked: bool,
    refresh_token: String,
    refresh_expires_at: f64,
}

/// An unused attach code. Only the SHA-256 of its secret is kept.
#[derive(Debug)]
struct AttachCode {
    hash: [u8; 32],
    user_id: String,
    /// The browser connection and the page that made it.
    conn: u64,
    page_id: String,
    level: Level,
    expires_at: f64,
}

#[derive(Debug)]
struct Binding {
    binding_id: String,
    grant_id: String,
    user_id: String,
    page_id: String,
    level: Level,
    browser: Option<u64>,
    /// The browser connection that held it before it was suspended: a pong on that
    /// same connection brings it back.
    last_browser: Option<u64>,
    state: State,
    reason: Option<&'static str>,
    approved_at: f64,
    /// Bumped whenever a pending suspension timer must no longer fire.
    suspend_generation: u64,
    /// The tab's last `capabilities` reply, kept whole and unread.
    capabilities: Option<Value>,
    /// The list from `publishCapabilities` (§5.8), replaced whole; unread.
    published: Option<Value>,
    /// Where the tab is, from `publishWhere` (§5.9); unread.
    location: Option<Value>,
}

/// A CLI `send` waiting for its suspended binding (attach spec §4).
#[derive(Debug)]
struct Held {
    hold_id: u64,
    cli: u64,
    /// The `invoke` frame's id: the answer goes out when the hold ends.
    invoke_id: Value,
    binding_id: String,
    envelope: Map<String, Value>,
}

#[derive(Debug)]
struct Conn {
    id: u64,
    kind: Kind,
    user_id: String,
    grant_id: Option<String>,
    page_id: Option<String>,
    /// Pings sent since the last pong (browser connections only).
    missed: f64,
    /// The heartbeat counted this connection as lost; a pong brings it back.
    lost: bool,
    /// False once the hub started closing it.
    open: bool,
    tx: UnboundedSender<Message>,
}

/// A refusal: `{ code, message?, reason?, retryAfterMs? }`.
type Refusal = Value;

fn refuse(code: &str, message: Option<&str>, reason: Option<&str>) -> Refusal {
    let mut body = Map::new();
    body.insert("code".into(), code.into());
    if let Some(message) = message {
        body.insert("message".into(), message.into());
    }
    if let Some(reason) = reason {
        body.insert("reason".into(), reason.into());
    }
    Value::Object(body)
}

/// What a method answers: now, or later (a held `send`).
enum Answer {
    Now(Result<Value, Refusal>),
    Later,
}

/// A JSON number as JavaScript writes it: whole numbers without a fraction.
pub fn num(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

/// The length JavaScript gives a string (UTF-16 units).
fn js_length(text: &str) -> usize {
    text.encode_utf16().count()
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

fn hash(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

fn millis(ms: f64) -> Duration {
    Duration::from_secs_f64(ms.max(0.0) / 1000.0)
}

pub type Shared = Arc<Mutex<Hub>>;

pub fn lock(hub: &Shared) -> MutexGuard<'_, Hub> {
    hub.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The hub's records. One lock guards them; no call holds it across an await.
pub struct Hub {
    me: Weak<Mutex<Hub>>,
    options: Options,
    rate_burst: f64,
    pub env: String,
    /// The hub address written into attach codes.
    pub url: String,
    grants: Vec<Grant>,
    access_tokens: HashMap<String, (String, f64)>,
    attach_codes: Vec<AttachCode>,
    bindings: Vec<Binding>,
    conns: Vec<Conn>,
    cli_by_grant: HashMap<String, u64>,
    /// A token bucket per grant; only work that goes to the tab spends it.
    rate: HashMap<String, (f64, f64)>,
    /// Wrong secrets per peer address, as times.
    attempts: HashMap<String, Vec<f64>>,
    /// The last binding of a grant, kept after it is gone for its reason.
    last_binding: HashMap<String, String>,
    held: Vec<Held>,
    next_conn: u64,
    next_hold: u64,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("env", &self.env)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl Hub {
    pub fn new(options: Options, env: String, url: String) -> Shared {
        Arc::new_cyclic(|me| {
            let rate_burst = options
                .rate_burst
                .unwrap_or_else(|| options.rate_per_second.max(20.0));
            Mutex::new(Hub {
                me: me.clone(),
                options,
                rate_burst,
                env,
                url,
                grants: Vec::new(),
                access_tokens: HashMap::new(),
                attach_codes: Vec::new(),
                bindings: Vec::new(),
                conns: Vec::new(),
                cli_by_grant: HashMap::new(),
                rate: HashMap::new(),
                attempts: HashMap::new(),
                last_binding: HashMap::new(),
                held: Vec::new(),
                next_conn: 0,
                next_hold: 0,
            })
        })
    }

    pub fn log(&self, line: &str) {
        if let Some(log) = &self.options.log {
            log(line);
        }
    }

    pub fn timings(&self) -> Timings {
        self.options.timings
    }

    /// The browser origins whose calls the hub answers (CORS).
    pub fn allows_origin(&self, origin: &str) -> bool {
        match &self.options.web_origin {
            Some(web) => origin == web,
            None => is_loopback_origin(origin),
        }
    }

    fn user(&self, id: &str) -> Option<&User> {
        self.options.users.iter().find(|user| user.id == id)
    }

    fn user_name(&self, id: &str) -> String {
        self.user(id)
            .map(|user| user.name.clone())
            .unwrap_or_default()
    }

    fn has_terminal(&self, id: &str) -> bool {
        self.user(id).and_then(|u| u.terminal) != Some(false)
    }

    fn conn(&self, id: u64) -> Option<&Conn> {
        self.conns.iter().find(|conn| conn.id == id)
    }

    fn conn_mut(&mut self, id: u64) -> Option<&mut Conn> {
        self.conns.iter_mut().find(|conn| conn.id == id)
    }

    fn grant(&self, id: &str) -> Option<&Grant> {
        self.grants.iter().find(|grant| grant.grant_id == id)
    }

    fn binding_index(&self, id: &str) -> Option<usize> {
        self.bindings.iter().position(|b| b.binding_id == id)
    }

    // --- helpers -------------------------------------------------------------

    /// Sends a text frame to a connection that is still open.
    fn send_text(&self, conn: u64, text: String) {
        if let Some(conn) = self.conn(conn)
            && conn.open
        {
            let _ = conn.tx.send(Message::text(text));
        }
    }

    fn send(&self, conn: u64, name: &str, payload: Option<Value>) {
        let mut frame = Map::new();
        frame.insert("t".into(), "event".into());
        frame.insert("name".into(), name.into());
        if let Some(payload) = payload {
            frame.insert("payload".into(), payload);
        }
        self.send_text(conn, Value::Object(frame).to_string());
    }

    /// The answer to one `invoke` frame.
    fn answer(&self, conn: u64, id: &Value, result: Result<Value, Refusal>) {
        let frame = match result {
            Ok(value) => json!({ "t": "result", "id": id, "ok": true, "value": value }),
            Err(error) => json!({ "t": "result", "id": id, "ok": false, "error": error }),
        };
        self.send_text(conn, frame.to_string());
    }

    /// Starts closing a connection with a close code; the reader ends it.
    fn close_conn(&mut self, id: u64, code: u16, reason: &str) {
        if let Some(conn) = self.conn_mut(id) {
            conn.open = false;
            let _ = conn.tx.send(Message::Close(Some(CloseFrame {
                code: CloseCode::from(code),
                reason: reason.to_owned().into(),
            })));
        }
    }

    /// Whether `peer` has used up its wrong secrets for this minute.
    fn too_many_attempts(&mut self, peer: &str) -> bool {
        let at = now();
        let window: Vec<f64> = self
            .attempts
            .get(peer)
            .map(|times| {
                times
                    .iter()
                    .copied()
                    .filter(|t| at - t < 60_000.0)
                    .collect()
            })
            .unwrap_or_default();
        let count = window.len();
        self.attempts.insert(peer.to_owned(), window);
        count >= self.options.max_code_attempts
    }

    fn failed_attempt(&mut self, peer: &str) {
        self.attempts
            .entry(peer.to_owned())
            .or_default()
            .push(now());
    }

    fn binding_of_grant(&self, grant_id: &str) -> Option<usize> {
        self.last_binding
            .get(grant_id)
            .and_then(|id| self.binding_index(id))
    }

    fn binding_of_browser(&self, conn: u64) -> Option<usize> {
        self.bindings
            .iter()
            .position(|b| b.browser == Some(conn) && b.state == State::Active)
    }

    /// The binding as the CLI sees it (`status`, the `binding` event).
    fn binding_view(&self, index: usize) -> Map<String, Value> {
        let binding = &self.bindings[index];
        let mut view = Map::new();
        view.insert("bindingId".into(), binding.binding_id.clone().into());
        view.insert("state".into(), binding.state.as_str().into());
        if let Some(reason) = binding.reason {
            view.insert("reason".into(), reason.into());
        }
        view.insert("pageId".into(), binding.page_id.clone().into());
        view.insert("level".into(), binding.level.as_str().into());
        view.insert("approvedAt".into(), num(binding.approved_at));
        if let Some(location) = &binding.location
            && binding.state != State::Gone
        {
            view.insert("where".into(), location.clone());
        }
        view
    }

    /// The binding as the tab's `hello` returns it.
    fn hello_view(&self, index: usize) -> Value {
        let b = &self.bindings[index];
        json!({
            "bindingId": b.binding_id,
            "grantId": b.grant_id,
            "level": b.level.as_str(),
            "approvedAt": num(b.approved_at),
        })
    }

    fn announce(&self, index: usize) {
        if let Some(cli) = self.cli_by_grant.get(&self.bindings[index].grant_id) {
            self.send(
                *cli,
                "binding",
                Some(Value::Object(self.binding_view(index))),
            );
        }
    }

    /// Ends a binding, and with it its grant (attach spec §6): the tab and the CLI
    /// hear it, held sends are refused, the tokens go and an open CLI connection closes
    /// with 4410.
    fn end(&mut self, index: usize, reason: &'static str) {
        let binding = &mut self.bindings[index];
        if binding.state == State::Gone {
            return;
        }
        binding.suspend_generation += 1;
        binding.state = State::Gone;
        binding.reason = Some(reason);
        binding.last_browser = None;
        let browser = binding.browser.take();
        let id = binding.binding_id.clone();
        let grant_id = binding.grant_id.clone();
        if let Some(browser) = browser {
            self.send(
                browser,
                "binding",
                Some(json!({ "bindingId": id, "state": "gone", "reason": reason })),
            );
        }
        self.announce(index);
        self.refuse_held(&id, refuse("gone", Some("the binding ended"), Some(reason)));
        self.log(&format!("binding {id} gone: {reason}"));
        self.drop_grant(&grant_id, reason);
    }

    /// The grant's end: no binding is touched here, so `end` and `revoke` never call
    /// each other in a loop.
    fn drop_grant(&mut self, grant_id: &str, reason: &str) {
        if let Some(grant) = self.grants.iter_mut().find(|g| g.grant_id == grant_id) {
            grant.revoked = true;
        }
        self.access_tokens
            .retain(|_, (owner, _)| owner.as_str() != grant_id);
        if let Some(cli) = self.cli_by_grant.get(grant_id).copied() {
            self.close_conn(cli, close_codes::REVOKED, reason);
        }
    }

    /// Revokes a grant from outside (the profile, `/oauth/revoke`): its binding ends.
    fn revoke(&mut self, grant_id: &str, reason: &'static str) {
        for index in 0..self.bindings.len() {
            if self.bindings[index].grant_id == grant_id {
                self.end(index, reason);
            }
        }
        self.drop_grant(grant_id, reason);
    }

    /// Suspends an active binding; unless its page comes back within `wait_ms`, it
    /// ends with `reason`.
    fn suspend(&mut self, index: usize, wait_ms: f64, reason: &'static str) {
        let binding = &mut self.bindings[index];
        if binding.state != State::Active {
            return;
        }
        binding.state = State::Suspended;
        binding.last_browser = binding.browser.take();
        binding.suspend_generation += 1;
        let generation = binding.suspend_generation;
        let id = binding.binding_id.clone();
        let me = self.me.clone();
        self.log(&format!(
            "binding {id} suspended; ends as {reason} in {wait_ms} ms"
        ));
        tokio::spawn(async move {
            tokio::time::sleep(millis(wait_ms)).await;
            if let Some(hub) = me.upgrade() {
                lock(&hub).suspension_ended(&id, generation, reason);
            }
        });
        self.announce(index);
    }

    fn suspension_ended(&mut self, binding_id: &str, generation: u64, reason: &'static str) {
        if let Some(index) = self.binding_index(binding_id)
            && self.bindings[index].suspend_generation == generation
            && self.bindings[index].state == State::Suspended
        {
            self.end(index, reason);
        }
    }

    /// A suspended binding is the page's again: it is active, the CLI hears it, and the
    /// sends that waited for it go out.
    fn reactivate(&mut self, index: usize, conn: u64, page_id: String) {
        let binding = &mut self.bindings[index];
        binding.suspend_generation += 1;
        binding.browser = Some(conn);
        binding.last_browser = None;
        binding.page_id = page_id;
        binding.state = State::Active;
        let id = binding.binding_id.clone();
        self.announce(index);
        self.log(&format!("binding {id} active again"));
        // After the answer of the call that brought the page back (`hello`), which goes
        // out under this lock: the page adopts the binding before its held sends.
        let me = self.me.clone();
        tokio::spawn(async move {
            if let Some(hub) = me.upgrade() {
                lock(&hub).release_held(&id);
            }
        });
    }

    fn issue_tokens(&mut self, grant_id: &str) -> (String, f64) {
        let access_token = format!("cli_{}", secret());
        let ttl = self.options.timings.access_ttl_ms;
        self.access_tokens
            .insert(access_token.clone(), (grant_id.to_owned(), now() + ttl));
        (access_token, ttl)
    }

    /// The user of a browser token: `dev:<userId>`.
    fn browser_user(&self, token: Option<&str>) -> Option<&User> {
        self.user(token?.strip_prefix("dev:")?)
    }

    /// Drops the unused codes that ran out.
    fn purge_codes(&mut self) {
        let at = now();
        self.attach_codes.retain(|code| at < code.expires_at);
    }

    // --- HTTP: attach, refresh, revoke, grants -------------------------------------

    /// Answers an HTTP request. `body` is the parsed request body (JSON or form);
    /// `peer` is the caller's address, the key of the attempt limit.
    pub fn http(
        &mut self,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: &Map<String, Value>,
        peer: &str,
    ) -> (u16, Value) {
        if method == "GET" && path == "/health" {
            return (200, json!({ "ok": true, "env": self.env }));
        }
        if method != "POST" && !(method == "GET" && path == "/cli/grants") {
            return (404, json!({ "error": "not_found" }));
        }

        if path == "/cli/attach" {
            return self.cli_attach(body, peer);
        }

        if path == "/oauth/token" {
            if body.get("grant_type").and_then(Value::as_str) != Some("refresh_token") {
                return (400, json!({ "error": "unsupported_grant_type" }));
            }
            let wanted = body.get("refresh_token").and_then(Value::as_str);
            let at = now();
            let grant = self
                .grants
                .iter()
                .find(|g| Some(g.refresh_token.as_str()) == wanted)
                .filter(|g| !g.revoked && at < g.refresh_expires_at)
                .map(|g| g.grant_id.clone());
            let Some(id) = grant else {
                // The grant ends with its binding (attach spec §6).
                return (400, json!({ "error": "invalid_grant" }));
            };
            let (access_token, ttl) = self.issue_tokens(&id);
            let Some(grant) = self.grant(&id) else {
                return (400, json!({ "error": "invalid_grant" }));
            };
            return (
                200,
                json!({
                    "access_token": access_token,
                    "token_type": "Bearer",
                    "expires_in": num((ttl / 1000.0).round()),
                    "refresh_token": grant.refresh_token,
                    "refresh_expires_in": num(((grant.refresh_expires_at - at) / 1000.0).round()),
                    "grant_id": grant.grant_id,
                    "user": { "id": grant.user_id, "name": self.user_name(&grant.user_id) },
                }),
            );
        }

        if path == "/oauth/revoke" {
            let wanted = body.get("token").and_then(Value::as_str);
            let grant = self
                .grants
                .iter()
                .find(|g| Some(g.refresh_token.as_str()) == wanted && !g.revoked)
                .map(|g| g.grant_id.clone());
            if let Some(id) = grant {
                self.revoke(&id, "revoked");
            }
            // RFC 7009: 200 whether or not the token was known.
            return (200, json!({}));
        }

        // The browser side: the fake user's session is `Bearer dev:<userId>`.
        let Some(user) = self.browser_user(bearer).cloned() else {
            return (401, json!({ "error": "unauthorized" }));
        };

        if path == "/cli/grants" && method == "GET" {
            let list: Vec<Value> = self
                .grants
                .iter()
                .filter(|g| g.user_id == user.id && !g.revoked)
                .map(|g| json!({ "grantId": g.grant_id, "createdAt": num(g.created_at), "env": g.env }))
                .collect();
            return (200, Value::Array(list));
        }

        if let Some(rest) = path.strip_prefix("/cli/grants/")
            && let Some(encoded) = rest.strip_suffix("/revoke")
            && !encoded.is_empty()
            && !encoded.contains('/')
        {
            let id = percent_encoding::percent_decode_str(encoded)
                .decode_utf8_lossy()
                .into_owned();
            let owned = self.grant(&id).is_some_and(|g| g.user_id == user.id);
            if !owned {
                return (404, json!({ "error": "not_found" }));
            }
            self.revoke(&id, "revoked");
            return (200, json!({ "ok": true }));
        }

        (404, json!({ "error": "not_found" }))
    }

    /// `POST /cli/attach { secret }` (attach spec §10.2): the code is used up at once,
    /// a grant is born with the binding on the page that made the code, and the CLI
    /// gets its tokens.
    fn cli_attach(&mut self, body: &Map<String, Value>, peer: &str) -> (u16, Value) {
        let refused = |status: u16, code: &str, message: &str| {
            (status, json!({ "error": code, "message": message }))
        };
        if self.too_many_attempts(peer) {
            return refused(
                429,
                "too-many-attempts",
                "too many wrong codes; wait a minute",
            );
        }
        self.purge_codes();
        let wanted = body.get("secret").and_then(Value::as_str).unwrap_or("");
        let digest = hash(wanted);
        let found = (!wanted.is_empty())
            .then(|| self.attach_codes.iter().position(|c| c.hash == digest))
            .flatten();
        let Some(position) = found else {
            self.failed_attempt(peer);
            return refused(400, "invalid-code", INVALID_CODE);
        };
        // Atomic: found and used up under one lock.
        let code = self.attach_codes.remove(position);
        if !self.has_terminal(&code.user_id) {
            return refused(
                403,
                "no-terminal-access",
                "this account has no terminal access",
            );
        }
        let page_alive = self.conn(code.conn).is_some_and(|c| {
            c.open && c.kind == Kind::Browser && c.page_id.as_deref() == Some(&code.page_id)
        });
        if !page_alive {
            return refused(400, "invalid-code", INVALID_CODE);
        }
        if let Some(old) = self.binding_of_browser(code.conn) {
            self.end(old, "replaced");
        }
        let at = now();
        let grant = Grant {
            grant_id: format!("g_{}", uuid()),
            user_id: code.user_id.clone(),
            env: self.env.clone(),
            created_at: at,
            revoked: false,
            refresh_token: format!("cr_{}", secret()),
            refresh_expires_at: at + self.options.timings.refresh_ttl_ms,
        };
        let grant_id = grant.grant_id.clone();
        let refresh_token = grant.refresh_token.clone();
        self.grants.push(grant);
        let binding = Binding {
            binding_id: format!("b_{}", uuid()),
            grant_id: grant_id.clone(),
            user_id: code.user_id.clone(),
            page_id: code.page_id.clone(),
            level: code.level,
            browser: Some(code.conn),
            last_browser: None,
            state: State::Active,
            reason: None,
            approved_at: at,
            suspend_generation: 0,
            capabilities: None,
            published: None,
            location: None,
        };
        let binding_id = binding.binding_id.clone();
        self.bindings.push(binding);
        self.last_binding
            .insert(grant_id.clone(), binding_id.clone());
        self.send(
            code.conn,
            "binding",
            Some(json!({
                "bindingId": binding_id,
                "state": "active",
                "grantId": grant_id,
                "level": code.level.as_str(),
            })),
        );
        self.log(&format!(
            "binding {binding_id} active ({}, grant {grant_id})",
            code.level.as_str()
        ));
        let (access_token, ttl) = self.issue_tokens(&grant_id);
        (
            200,
            json!({
                "access_token": access_token,
                "refresh_token": refresh_token,
                "expires_in": num((ttl / 1000.0).round()),
                "grantId": grant_id,
                "bindingId": binding_id,
                "level": code.level.as_str(),
                "user": { "id": code.user_id, "name": self.user_name(&code.user_id) },
            }),
        )
    }

    // --- WebSocket: connections ---------------------------------------------------

    /// Admits a hub connection, or says which close code refuses it.
    pub fn accept(
        &mut self,
        bearer: Option<&str>,
        query_token: Option<&str>,
        tx: UnboundedSender<Message>,
    ) -> Result<(u64, Kind), (u16, &'static str)> {
        if query_token.is_some_and(|t| t.starts_with("cli_")) {
            // §8: the query string path is closed for a CLI token.
            return Err((close_codes::FORBIDDEN, "cli token in the query string"));
        }
        self.next_conn += 1;
        let id = self.next_conn;
        let conn = if let Some(token) = bearer.filter(|t| t.starts_with("cli_")) {
            let at = now();
            let grant = self
                .access_tokens
                .get(token)
                .filter(|(_, expires_at)| at < *expires_at)
                .and_then(|(grant_id, _)| self.grant(grant_id))
                .filter(|grant| !grant.revoked)
                .map(|grant| (grant.grant_id.clone(), grant.user_id.clone()));
            let Some((grant_id, user_id)) = grant else {
                return Err((close_codes::UNAUTHORIZED, "token"));
            };
            if let Some(held) = self.cli_by_grant.get(&grant_id).copied()
                && !self.conn(held).is_some_and(|conn| conn.open)
            {
                // The previous call has already closed: its close frame is in, but the
                // reader may not have run yet. A closing connection no longer holds
                // the grant.
                self.cli_by_grant.remove(&grant_id);
                self.conns.retain(|conn| conn.id != held);
                self.held.retain(|h| h.cli != held);
            }
            if self.cli_by_grant.contains_key(&grant_id) {
                // One live CLI connection per grant (§3); the tab sees the attempt.
                if let Some(index) = self.binding_of_grant(&grant_id)
                    && let Some(browser) = self.bindings[index].browser
                {
                    self.send(
                        browser,
                        "cli-busy",
                        Some(json!({ "grantId": grant_id, "at": num(at) })),
                    );
                }
                return Err((close_codes::BUSY, "another CLI call is running"));
            }
            self.cli_by_grant.insert(grant_id.clone(), id);
            Conn {
                id,
                kind: Kind::Cli,
                user_id,
                grant_id: Some(grant_id),
                page_id: None,
                missed: 0.0,
                lost: false,
                open: true,
                tx,
            }
        } else {
            let Some(user) = self.browser_user(query_token.or(bearer)) else {
                return Err((close_codes::UNAUTHORIZED, "session"));
            };
            Conn {
                id,
                kind: Kind::Browser,
                user_id: user.id.clone(),
                grant_id: None,
                page_id: None,
                missed: 0.0,
                lost: false,
                open: true,
                tx,
            }
        };
        let kind = conn.kind;
        self.conns.push(conn);
        self.send(id, "ready", Some(json!({ "kind": kind.as_str() })));
        Ok((id, kind))
    }

    /// A frame or a pong from a browser connection: the page is there.
    pub fn heard(&mut self, id: u64) {
        let Some(conn) = self.conn_mut(id) else {
            return;
        };
        if conn.kind != Kind::Browser {
            return;
        }
        conn.missed = 0.0;
        if !conn.lost {
            return;
        }
        conn.lost = false;
        let page_id = conn.page_id.clone();
        self.log(&format!("browser connection {id} answers again"));
        // The same connection and the same page: the binding is the page's again.
        let Some(page_id) = page_id else {
            return;
        };
        let found = self.bindings.iter().position(|b| {
            b.state == State::Suspended && b.last_browser == Some(id) && b.page_id == page_id
        });
        if let Some(index) = found {
            self.reactivate(index, id, page_id);
        }
    }

    /// One heartbeat tick; false once the connection is gone.
    pub fn heartbeat(&mut self, id: u64) -> bool {
        let misses = self.options.timings.heartbeat_misses;
        let suspend_ms = self.options.timings.suspend_ms;
        let Some(conn) = self.conn_mut(id) else {
            return false;
        };
        if conn.missed >= misses && !conn.lost {
            conn.lost = true;
            self.log(&format!("browser connection {id} lost: no pong"));
            for index in 0..self.bindings.len() {
                if self.bindings[index].browser == Some(id) {
                    self.suspend(index, suspend_ms, "suspended-timeout");
                }
            }
        }
        if let Some(conn) = self.conn_mut(id) {
            conn.missed += 1.0;
            if conn.open {
                let _ = conn.tx.send(Message::Ping(Default::default()));
            }
        }
        true
    }

    /// The connection ended (a close frame, an error, or the end of the stream).
    pub fn closed(&mut self, id: u64) {
        let Some(position) = self.conns.iter().position(|conn| conn.id == id) else {
            return;
        };
        let conn = self.conns.remove(position);
        if conn.kind == Kind::Cli {
            if let Some(grant) = &conn.grant_id
                && self.cli_by_grant.get(grant) == Some(&id)
            {
                self.cli_by_grant.remove(grant);
            }
            self.held.retain(|h| h.cli != id);
            return;
        }
        // A code dies with the page that made it (attach spec §3.4).
        self.attach_codes.retain(|code| code.conn != id);
        let suspend_ms = self.options.timings.suspend_ms;
        for index in 0..self.bindings.len() {
            if self.bindings[index].browser == Some(id) {
                self.suspend(index, suspend_ms, "suspended-timeout");
            }
        }
    }

    /// Answers one `invoke` frame of a connection.
    pub fn invoke(&mut self, conn: u64, id: &Value, method: &str, args: &Map<String, Value>) {
        let Some(kind) = self.conn(conn).map(|c| c.kind) else {
            return;
        };
        let result = match (kind, method) {
            (Kind::Cli, "status") => self.cli_status(conn),
            (Kind::Cli, "send") => match self.cli_send(conn, id, args) {
                Answer::Now(result) => result,
                Answer::Later => return,
            },
            (Kind::Cli, "detach") => {
                // The answer goes out first: the end of the binding closes this
                // connection with 4410.
                self.answer(conn, id, Ok(json!({ "ok": true })));
                self.cli_detach(conn);
                return;
            }
            (Kind::Browser, "hello") => self.hello(conn, args),
            (Kind::Browser, "attachCode") => self.attach_code(conn, args),
            (Kind::Browser, "sendToCli") => self.send_to_cli(conn, args),
            (Kind::Browser, "publishCapabilities") => self.publish_capabilities(conn, args),
            (Kind::Browser, "publishWhere") => self.publish_where(conn, args),
            (Kind::Browser, "detach") => self.browser_detach(conn),
            (Kind::Browser, "unload") => self.unload(conn),
            (Kind::Browser, "logout") => self.browser_logout(conn),
            (Kind::Browser, "broadcast") => self.broadcast(conn, args),
            _ => Err(refuse("unknown-method", Some(method), None)),
        };
        self.answer(conn, id, result);
    }

    // --- CLI methods --------------------------------------------------------------

    fn grant_of(&self, conn: u64) -> Result<String, Refusal> {
        self.conn(conn)
            .and_then(|c| c.grant_id.clone())
            .ok_or_else(|| refuse("failed", Some("no grant"), None))
    }

    fn cli_status(&mut self, conn: u64) -> Result<Value, Refusal> {
        let grant_id = self.grant_of(conn)?;
        let grant = self
            .grant(&grant_id)
            .ok_or_else(|| refuse("failed", Some("no grant"), None))?;
        let binding = self.binding_of_grant(&grant_id).map(|index| {
            let mut view = self.binding_view(index);
            let binding = &self.bindings[index];
            // The published list (§5.8) wins over a `capabilities` reply.
            if let Some(list) = binding.published.as_ref().or(binding.capabilities.as_ref())
                && binding.state != State::Gone
            {
                view.insert("capabilities".into(), list.clone());
            }
            Value::Object(view)
        });
        Ok(json!({
            "user": { "id": grant.user_id, "name": self.user_name(&grant.user_id) },
            "env": self.env,
            "grantId": grant.grant_id,
            "binding": binding,
        }))
    }

    fn cli_send(&mut self, conn: u64, invoke_id: &Value, args: &Map<String, Value>) -> Answer {
        let now_answer = |result| Answer::Now(result);
        let grant_id = match self.grant_of(conn) {
            Ok(id) => id,
            Err(error) => return now_answer(Err(error)),
        };
        let Some(Value::Object(envelope)) = args.get("envelope") else {
            return now_answer(Err(refuse("invalid", None, None)));
        };
        // The hub reads `kind` and the request id only (§2).
        let kind = envelope.get("kind").and_then(Value::as_str);
        if !matches!(kind, Some("cli-request" | "cancel" | "help-request"))
            || !envelope.get("id").is_some_and(Value::is_string)
        {
            return now_answer(Err(refuse("invalid", None, None)));
        }
        let Some(index) = self.binding_of_grant(&grant_id) else {
            return now_answer(Err(refuse(
                "no-binding",
                Some("no binding for this grant"),
                None,
            )));
        };
        let binding = &self.bindings[index];
        if binding.state == State::Gone {
            return now_answer(Err(refuse(
                "gone",
                Some("the binding ended"),
                binding.reason,
            )));
        }
        // The rate limit counts before any wait (attach spec §10.2).
        if matches!(kind, Some("cli-request" | "help-request")) {
            let at = now();
            let (rate, burst) = (self.options.rate_per_second, self.rate_burst);
            let bucket = self.rate.entry(grant_id.clone()).or_insert((burst, at));
            bucket.0 = burst.min(bucket.0 + ((at - bucket.1) / 1000.0) * rate);
            bucket.1 = at;
            if bucket.0 < 1.0 {
                // Not `busy` (one live call): the CLI waits this long and retries.
                let mut error = Map::new();
                error.insert("code".into(), "rate-limited".into());
                error.insert(
                    "message".into(),
                    format!("rate limit: {} requests a second on average", num(rate)).into(),
                );
                error.insert(
                    "retryAfterMs".into(),
                    num((((1.0 - bucket.0) / rate) * 1000.0).ceil()),
                );
                return now_answer(Err(Value::Object(error)));
            }
            bucket.0 -= 1.0;
        }
        let binding = &self.bindings[index];
        match (binding.state, binding.browser) {
            (State::Active, Some(browser)) => {
                let binding_id = binding.binding_id.clone();
                now_answer(Ok(self.deliver(
                    browser,
                    &grant_id,
                    &binding_id,
                    envelope.clone(),
                )))
            }
            (State::Suspended, _) => {
                // The one exception to "no queue": while the page comes back, the send
                // waits up to `holdMs` (attach spec §4).
                self.next_hold += 1;
                let hold_id = self.next_hold;
                self.held.push(Held {
                    hold_id,
                    cli: conn,
                    invoke_id: invoke_id.clone(),
                    binding_id: binding.binding_id.clone(),
                    envelope: envelope.clone(),
                });
                let me = self.me.clone();
                let wait = millis(self.options.timings.hold_ms);
                tokio::spawn(async move {
                    tokio::time::sleep(wait).await;
                    if let Some(hub) = me.upgrade() {
                        lock(&hub).hold_ended(hold_id);
                    }
                });
                Answer::Later
            }
            _ => now_answer(Err(unreachable_tab())),
        }
    }

    /// Stamps an envelope and gives it to the tab; the answer is the stamp.
    fn deliver(
        &self,
        browser: u64,
        grant_id: &str,
        binding_id: &str,
        envelope: Map<String, Value>,
    ) -> Value {
        let stamped_at = now();
        let expires_at = stamped_at + self.options.timings.request_ttl_ms;
        // The client's own copy of the stamp is dropped, then written anew.
        let mut stamped = envelope;
        stamped.remove("origin");
        stamped.remove("stampedAt");
        stamped.remove("expiresAt");
        stamped.insert(
            "origin".into(),
            json!({ "kind": "cli", "grantId": grant_id, "bindingId": binding_id }),
        );
        stamped.insert("stampedAt".into(), num(stamped_at));
        stamped.insert("expiresAt".into(), num(expires_at));
        // The hub reads `kind` and the request id only (§2); never the command line.
        let field = |name: &str| {
            stamped
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned()
        };
        self.log(&format!(
            "{} {} to browser connection {browser}",
            field("kind"),
            field("id")
        ));
        self.send(browser, "cli-message", Some(Value::Object(stamped)));
        json!({ "stampedAt": num(stamped_at), "expiresAt": num(expires_at) })
    }

    /// The held sends of a binding that is active again go out now.
    fn release_held(&mut self, binding_id: &str) {
        let Some(index) = self.binding_index(binding_id) else {
            return;
        };
        let (Some(browser), grant_id) = (
            self.bindings[index].browser,
            self.bindings[index].grant_id.clone(),
        ) else {
            return;
        };
        let (ready, waiting): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|h| h.binding_id == binding_id);
        self.held = waiting;
        if !ready.is_empty() {
            self.log(&format!(
                "binding {binding_id}: {} held send(s) go out",
                ready.len()
            ));
        }
        for held in ready {
            let stamp = self.deliver(browser, &grant_id, binding_id, held.envelope);
            self.answer(held.cli, &held.invoke_id, Ok(stamp));
        }
    }

    /// The held sends of a binding are refused with `refusal`.
    fn refuse_held(&mut self, binding_id: &str, refusal: Refusal) {
        let (ended, waiting): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.held)
            .into_iter()
            .partition(|h| h.binding_id == binding_id);
        self.held = waiting;
        for held in ended {
            self.answer(held.cli, &held.invoke_id, Err(refusal.clone()));
        }
    }

    fn hold_ended(&mut self, hold_id: u64) {
        let Some(position) = self.held.iter().position(|h| h.hold_id == hold_id) else {
            return;
        };
        let held = self.held.remove(position);
        self.log(&format!(
            "binding {}: a held send ends tab-unreachable",
            held.binding_id
        ));
        self.answer(held.cli, &held.invoke_id, Err(unreachable_tab()));
    }

    fn cli_detach(&mut self, conn: u64) {
        let Ok(grant_id) = self.grant_of(conn) else {
            return;
        };
        match self.binding_of_grant(&grant_id) {
            Some(index) => self.end(index, "detached"),
            None => self.drop_grant(&grant_id, "detached"),
        }
    }

    // --- browser methods ----------------------------------------------------------

    fn hello(&mut self, conn: u64, args: &Map<String, Value>) -> Result<Value, Refusal> {
        let Some(page_id) = args
            .get("pageId")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
        else {
            return Err(refuse("invalid", Some("pageId"), None));
        };
        let resume = match args.get("resume") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) if !id.is_empty() => Some(id.clone()),
            Some(_) => return Err(refuse("invalid", Some("resume: a bindingId"), None)),
        };
        let current = self.conn(conn).and_then(|c| c.page_id.clone());
        if current.as_ref().is_some_and(|current| *current != page_id)
            && let Some(old) = self.binding_of_browser(conn)
        {
            if resume.as_deref() == Some(self.bindings[old].binding_id.as_str()) {
                self.bindings[old].page_id = page_id.clone();
            } else {
                // A new page on the same connection: the old page is gone.
                self.end(old, "closed");
            }
        }
        let user_id = match self.conn_mut(conn) {
            Some(c) => {
                c.page_id = Some(page_id.clone());
                c.user_id.clone()
            }
            None => return Err(refuse("failed", None, None)),
        };
        if let Some(wanted) = resume
            && let Some(index) = self.binding_index(&wanted)
        {
            let binding = &self.bindings[index];
            // One live browser connection per binding: a copied tab, whose
            // `sessionStorage` holds the same id, finds it active and gets nothing.
            if binding.state == State::Suspended && binding.user_id == user_id {
                self.reactivate(index, conn, page_id);
            }
        }
        let live = self
            .binding_of_browser(conn)
            .map(|index| self.hello_view(index));
        Ok(json!({ "binding": live }))
    }

    /// `attachCode { pageId, level }`: a one-time code for `gecko attach` (attach spec
    /// §3.1). The page's previous unused code stops working.
    fn attach_code(&mut self, conn: u64, args: &Map<String, Value>) -> Result<Value, Refusal> {
        let Some((page_id, user_id)) = self
            .conn(conn)
            .and_then(|c| Some((c.page_id.clone()?, c.user_id.clone())))
        else {
            return Err(refuse(
                "invalid",
                Some("say hello with the pageId first"),
                None,
            ));
        };
        if args.get("pageId").and_then(Value::as_str) != Some(page_id.as_str()) {
            return Err(refuse(
                "invalid",
                Some("pageId: the page of this connection"),
                None,
            ));
        }
        let Some(level) = args
            .get("level")
            .and_then(Value::as_str)
            .and_then(Level::parse)
        else {
            return Err(refuse(
                "invalid",
                Some("level: ask-server or never-ask"),
                None,
            ));
        };
        if !self.has_terminal(&user_id) {
            return Err(refuse(
                "no-terminal-access",
                Some("this account has no terminal access"),
                None,
            ));
        }
        self.purge_codes();
        self.attach_codes
            .retain(|code| !(code.conn == conn && code.page_id == page_id));
        let secret = secret();
        let payload = json!({ "v": 1, "hub": self.url, "s": secret });
        let code = format!("{CODE_PREFIX}{}", base64url(payload.to_string().as_bytes()));
        let expires_at = now() + self.options.timings.attach_ttl_ms;
        self.attach_codes.push(AttachCode {
            hash: hash(&secret),
            user_id,
            conn,
            page_id,
            level,
            expires_at,
        });
        Ok(json!({ "code": code, "expiresAt": num(expires_at) }))
    }

    fn bound(&self, conn: u64) -> Result<usize, Refusal> {
        self.binding_of_browser(conn)
            .ok_or_else(|| refuse("not-bound", Some("this connection has no binding"), None))
    }

    fn send_to_cli(&mut self, conn: u64, args: &Map<String, Value>) -> Result<Value, Refusal> {
        let index = self.bound(conn)?;
        let reply = match args.get("reply") {
            Some(reply @ (Value::Object(_) | Value::Array(_))) => reply.clone(),
            _ => return Err(refuse("invalid", None, None)),
        };
        // The hub reads `kind` only; the capabilities are kept whole for the next CLI
        // connection, as each `gecko` call is a new connection.
        if reply.get("kind").and_then(Value::as_str) == Some("capabilities") {
            self.bindings[index].capabilities = Some(reply.clone());
        }
        match self.cli_by_grant.get(&self.bindings[index].grant_id) {
            Some(cli) => {
                self.send(*cli, "reply", Some(reply));
                Ok(json!({ "delivered": true }))
            }
            None => Ok(json!({ "delivered": false })),
        }
    }

    fn publish_capabilities(
        &mut self,
        conn: u64,
        args: &Map<String, Value>,
    ) -> Result<Value, Refusal> {
        let index = self.bound(conn)?;
        let revision = args
            .get("revision")
            .and_then(Value::as_str)
            .filter(|r| !r.is_empty() && js_length(r) <= MAX_REVISION_LENGTH);
        let parts: Option<Vec<&str>> = args
            .get("parts")
            .and_then(Value::as_array)
            .filter(|parts| !parts.is_empty())
            .and_then(|parts| parts.iter().map(Value::as_str).collect());
        let (Some(revision), Some(parts)) = (revision, parts) else {
            return Err(refuse(
                "invalid",
                Some("revision: string, parts: string[]"),
                None,
            ));
        };
        if parts.len() > MAX_PARTS || parts.iter().any(|part| part.len() > MAX_PART_BYTES) {
            return Err(refuse(
                "too-large",
                Some(&format!(
                    "at most {MAX_PARTS} parts of {MAX_PART_BYTES} bytes"
                )),
                None,
            ));
        }
        // Replaced whole: old and new parts never mix (§5.8).
        self.bindings[index].published = Some(json!({ "revision": revision, "parts": parts }));
        Ok(json!({ "ok": true }))
    }

    fn publish_where(&mut self, conn: u64, args: &Map<String, Value>) -> Result<Value, Refusal> {
        let index = self.bound(conn)?;
        let optional = |name: &str, max: usize| match args.get(name) {
            None => true,
            Some(Value::String(text)) => js_length(text) <= max,
            Some(_) => false,
        };
        if !optional("title", MAX_TITLE_LENGTH) || !optional("path", MAX_PATH_LENGTH) {
            return Err(refuse(
                "invalid",
                Some("title?: string, path?: string"),
                None,
            ));
        }
        let mut location = Map::new();
        for name in ["title", "path"] {
            if let Some(value) = args.get(name) {
                location.insert(name.into(), value.clone());
            }
        }
        self.bindings[index].location = Some(Value::Object(location));
        Ok(json!({ "ok": true }))
    }

    fn browser_detach(&mut self, conn: u64) -> Result<Value, Refusal> {
        if let Some(index) = self.binding_of_browser(conn) {
            self.end(index, "detached");
        }
        Ok(json!({ "ok": true }))
    }

    /// The page is closing or reloading; it cannot tell which. The binding waits
    /// `unloadMs` for a `hello` with `resume` (attach spec §4).
    fn unload(&mut self, conn: u64) -> Result<Value, Refusal> {
        let unload_ms = self.options.timings.unload_ms;
        if let Some(index) = self.binding_of_browser(conn) {
            self.suspend(index, unload_ms, "closed");
        }
        self.attach_codes.retain(|code| code.conn != conn);
        if let Some(c) = self.conn_mut(conn) {
            c.page_id = None;
        }
        Ok(json!({ "ok": true }))
    }

    fn browser_logout(&mut self, conn: u64) -> Result<Value, Refusal> {
        let user_id = self
            .conn(conn)
            .map(|c| c.user_id.clone())
            .unwrap_or_default();
        for index in 0..self.bindings.len() {
            if self.bindings[index].user_id == user_id {
                self.end(index, "logged-out");
            }
        }
        Ok(json!({ "ok": true }))
    }

    fn broadcast(&mut self, conn: u64, args: &Map<String, Value>) -> Result<Value, Refusal> {
        // Today's `Send`: every other browser connection of the user. A CLI connection
        // never joins (§5.1).
        let user_id = self
            .conn(conn)
            .map(|c| c.user_id.clone())
            .unwrap_or_default();
        let others: Vec<u64> = self
            .conns
            .iter()
            .filter(|c| c.id != conn && c.kind == Kind::Browser && c.user_id == user_id)
            .map(|c| c.id)
            .collect();
        for other in &others {
            self.send(*other, "broadcast", args.get("envelope").cloned());
        }
        Ok(json!({ "delivered": others.len() }))
    }
}

fn unreachable_tab() -> Refusal {
    refuse(
        "tab-unreachable",
        Some("the bound tab is not connected"),
        None,
    )
}

/// `http://127.0.0.1:<port>`, `http://localhost:<port>` or `http://[::1]:<port>`
/// exactly: an origin with a path, credentials or another host is not one.
pub fn is_loopback_origin(origin: &str) -> bool {
    let Ok(parsed) = url::Url::parse(origin) else {
        return false;
    };
    parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        && parsed.origin().ascii_serialization() == origin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_origins_are_exact() {
        assert!(is_loopback_origin("http://127.0.0.1:5173"));
        assert!(is_loopback_origin("http://localhost:5173"));
        assert!(is_loopback_origin("http://[::1]:5173"));
        assert!(!is_loopback_origin("https://localhost:5173"));
        assert!(!is_loopback_origin("http://localhost:5173/"));
        assert!(!is_loopback_origin("http://user@localhost:5173"));
        assert!(!is_loopback_origin("http://example.com"));
        assert!(!is_loopback_origin("null"));
    }

    #[test]
    fn numbers_are_written_as_javascript_does() {
        assert_eq!(num(5.0).to_string(), "5");
        assert_eq!(num(0.1).to_string(), "0.1");
        assert_eq!(num(1_731_250_000_000.0).to_string(), "1731250000000");
    }

    #[test]
    fn timings_merge_known_fields_only() {
        let mut timings = Timings::default();
        let json = json!({ "suspendMs": 500, "holdMs": 0.5, "other": 1, "attachTtlMs": "x" });
        timings.merge(json.as_object().unwrap());
        assert_eq!(timings.suspend_ms, 500.0);
        assert_eq!(timings.hold_ms, 0.5);
        assert_eq!(timings.attach_ttl_ms, 120_000.0);
        assert_eq!(timings.unload_ms, 10_000.0);
    }

    #[test]
    fn levels_are_two() {
        assert_eq!(Level::parse("ask-server"), Some(Level::AskServer));
        assert_eq!(Level::parse("never-ask"), Some(Level::NeverAsk));
        assert_eq!(Level::parse("always"), None);
        assert_eq!(Level::NeverAsk.as_str(), "never-ask");
    }

    #[test]
    fn secrets_are_kept_as_sha256() {
        // SHA-256("abc"), FIPS 180-2.
        let digest = hash("abc");
        assert_eq!(digest[..4], [0xba, 0x78, 0x16, 0xbf]);
        assert_eq!(digest[28..], [0xf2, 0x00, 0x15, 0xad]);
    }
}
