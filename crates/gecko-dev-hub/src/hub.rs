//! The hub's records and rules: device login (RFC 8628), CLI tokens, the binding of a
//! CLI grant to one tab page, routing with the hub's stamp, and the browser broadcast
//! that CLI connections never join. The server (`server.rs`) only moves bytes.
//!
//! Times are milliseconds since the epoch as `f64`: the windows come from JSON and may
//! be fractional.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

use crate::random::{code, normalize_code, secret, uuid};

/// WebSocket close codes the hub ends or refuses a connection with.
pub mod close_codes {
    /// No token, a wrong or expired token, or a revoked grant.
    pub const UNAUTHORIZED: u16 = 4401;
    /// A CLI token in the query string: the CLI sends it as a header.
    pub const FORBIDDEN: u16 = 4403;
    /// Another live CLI connection holds the grant (§3).
    pub const BUSY: u16 = 4409;
    /// The grant was revoked while connected.
    pub const REVOKED: u16 = 4410;
}

/// §5.8: one part at most 16 KB, at most 3 parts.
const MAX_PART_BYTES: usize = 16 * 1024;
const MAX_PARTS: usize = 3;
const MAX_REVISION_LENGTH: usize = 256;
/// §5.9: generous bounds; the hub stores the text, it does not read it.
const MAX_TITLE_LENGTH: usize = 512;
const MAX_PATH_LENGTH: usize = 2048;

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
    /// Device codes live this long (§8: 10 min).
    pub device_ttl_ms: f64,
    /// The device poll interval (RFC 8628: 5 s).
    pub interval_ms: f64,
    /// Pairing codes live this long (§5.2: 2 min).
    pub pair_ttl_ms: f64,
    /// A suspended binding waits this long for its page (§5.3: 60 s).
    pub suspend_ms: f64,
    /// `expiresAt - stampedAt` of a CLI message (§4: 30 s).
    pub request_ttl_ms: f64,
    /// Access token life (§8: 1 h).
    pub access_ttl_ms: f64,
    /// Refresh token life (§8: 7 days).
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
            device_ttl_ms: 10.0 * 60_000.0,
            interval_ms: 5_000.0,
            pair_ttl_ms: 2.0 * 60_000.0,
            suspend_ms: 60_000.0,
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
            ("deviceTtlMs", &mut self.device_ttl_ms),
            ("intervalMs", &mut self.interval_ms),
            ("pairTtlMs", &mut self.pair_ttl_ms),
            ("suspendMs", &mut self.suspend_ms),
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
    /// Wrong codes per user per minute before `too-many-attempts`.
    pub max_code_attempts: usize,
    /// Approve every device code for the first user on its first poll.
    pub auto_approve: bool,
    /// The origin of the web app for the login and pairing addresses. Given, it is the
    /// one origin the browser may call the hub from; left out, any loopback origin may.
    pub web_origin: Option<String>,
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
            .field("auto_approve", &self.auto_approve)
            .field("web_origin", &self.web_origin)
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
            auto_approve: false,
            web_origin: None,
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

#[derive(Debug, PartialEq, Eq)]
enum DeviceStatus {
    Pending,
    Approved,
    Denied,
    Used,
}

#[derive(Debug)]
struct Device {
    device_code: String,
    user_code: String,
    expires_at: f64,
    interval_ms: f64,
    last_poll_at: Option<f64>,
    status: DeviceStatus,
    user_id: Option<String>,
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

#[derive(Debug)]
struct PairCode {
    code: String,
    grant_id: String,
    user_id: String,
    expires_at: f64,
    used: bool,
}

#[derive(Debug)]
struct Binding {
    binding_id: String,
    grant_id: String,
    user_id: String,
    page_id: String,
    browser: Option<u64>,
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

/// A JSON number as JavaScript writes it: whole numbers without a fraction.
pub fn num(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        Value::from(value as i64)
    } else {
        Value::from(value)
    }
}

/// `String(value ?? '')` of JavaScript, for the few fields read that way.
fn text_of(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
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

/// What an HTTP route answers.
#[derive(Debug)]
pub enum Body {
    Json(Value),
    Html(String),
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
    first_user: Option<String>,
    pub env: String,
    pub web_origin: String,
    devices: Vec<Device>,
    grants: Vec<Grant>,
    access_tokens: HashMap<String, (String, f64)>,
    pair_codes: HashMap<String, PairCode>,
    bindings: Vec<Binding>,
    conns: Vec<Conn>,
    cli_by_grant: HashMap<String, u64>,
    /// A token bucket per grant; only work that goes to the tab spends it.
    rate: HashMap<String, (f64, f64)>,
    attempts: HashMap<String, Vec<f64>>,
    /// The last binding of a grant, kept after it is gone for its reason.
    last_binding: HashMap<String, String>,
    next_conn: u64,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("env", &self.env)
            .finish_non_exhaustive()
    }
}

impl Hub {
    pub fn new(options: Options, env: String, web_origin: String) -> Shared {
        Arc::new_cyclic(|me| {
            let rate_burst = options
                .rate_burst
                .unwrap_or_else(|| options.rate_per_second.max(20.0));
            let first_user = options.users.first().map(|user| user.id.clone());
            Mutex::new(Hub {
                me: me.clone(),
                options,
                rate_burst,
                first_user,
                env,
                web_origin,
                devices: Vec::new(),
                grants: Vec::new(),
                access_tokens: HashMap::new(),
                pair_codes: HashMap::new(),
                bindings: Vec::new(),
                conns: Vec::new(),
                cli_by_grant: HashMap::new(),
                rate: HashMap::new(),
                attempts: HashMap::new(),
                last_binding: HashMap::new(),
                next_conn: 0,
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

    fn too_many_attempts(&mut self, user_id: &str) -> bool {
        let at = now();
        let window: Vec<f64> = self
            .attempts
            .get(user_id)
            .map(|times| {
                times
                    .iter()
                    .copied()
                    .filter(|t| at - t < 60_000.0)
                    .collect()
            })
            .unwrap_or_default();
        let count = window.len();
        self.attempts.insert(user_id.to_owned(), window);
        count >= self.options.max_code_attempts
    }

    fn failed_attempt(&mut self, user_id: &str) {
        self.attempts
            .entry(user_id.to_owned())
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

    fn binding_view(&self, index: usize) -> Map<String, Value> {
        let binding = &self.bindings[index];
        let mut view = Map::new();
        view.insert("bindingId".into(), binding.binding_id.clone().into());
        view.insert("state".into(), binding.state.as_str().into());
        if let Some(reason) = binding.reason {
            view.insert("reason".into(), reason.into());
        }
        view.insert("pageId".into(), binding.page_id.clone().into());
        view.insert("approvedAt".into(), num(binding.approved_at));
        if let Some(location) = &binding.location
            && binding.state != State::Gone
        {
            view.insert("where".into(), location.clone());
        }
        view
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

    fn end(&mut self, index: usize, reason: &'static str) {
        let binding = &mut self.bindings[index];
        if binding.state == State::Gone {
            return;
        }
        binding.suspend_generation += 1;
        binding.state = State::Gone;
        binding.reason = Some(reason);
        let browser = binding.browser.take();
        let id = binding.binding_id.clone();
        if let Some(browser) = browser {
            self.send(
                browser,
                "binding",
                Some(json!({ "bindingId": id, "state": "gone", "reason": reason })),
            );
        }
        self.announce(index);
        self.log(&format!("binding {id} gone: {reason}"));
    }

    fn suspend(&mut self, index: usize) {
        let binding = &mut self.bindings[index];
        if binding.state != State::Active {
            return;
        }
        binding.state = State::Suspended;
        binding.browser = None;
        binding.suspend_generation += 1;
        let generation = binding.suspend_generation;
        let id = binding.binding_id.clone();
        let me = self.me.clone();
        let wait = Duration::from_secs_f64(self.options.timings.suspend_ms.max(0.0) / 1000.0);
        tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            if let Some(hub) = me.upgrade() {
                lock(&hub).suspension_ended(&id, generation);
            }
        });
        self.announce(index);
    }

    fn suspension_ended(&mut self, binding_id: &str, generation: u64) {
        if let Some(index) = self.binding_index(binding_id)
            && self.bindings[index].suspend_generation == generation
            && self.bindings[index].state == State::Suspended
        {
            self.end(index, "suspended-timeout");
        }
    }

    fn revoke(&mut self, grant_id: &str, reason: &'static str) {
        if let Some(grant) = self.grants.iter_mut().find(|g| g.grant_id == grant_id) {
            grant.revoked = true;
        }
        self.access_tokens
            .retain(|_, (owner, _)| owner.as_str() != grant_id);
        for index in 0..self.bindings.len() {
            if self.bindings[index].grant_id == grant_id {
                self.end(index, reason);
            }
        }
        if let Some(cli) = self.cli_by_grant.get(grant_id).copied() {
            self.close_conn(cli, close_codes::REVOKED, reason);
        }
    }

    fn issue_tokens(&mut self, grant_id: &str) -> Value {
        let access_token = format!("cli_{}", secret());
        let at = now();
        let ttl = self.options.timings.access_ttl_ms;
        self.access_tokens
            .insert(access_token.clone(), (grant_id.to_owned(), at + ttl));
        let Some(grant) = self.grant(grant_id) else {
            return json!({});
        };
        json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "expires_in": num((ttl / 1000.0).round()),
            "refresh_token": grant.refresh_token,
            "refresh_expires_in": num(((grant.refresh_expires_at - at) / 1000.0).round()),
            "grant_id": grant.grant_id,
            "user": { "id": grant.user_id, "name": self.user_name(&grant.user_id) },
        })
    }

    /// The user of a browser token: `dev:<userId>`.
    fn browser_user(&self, token: Option<&str>) -> Option<&User> {
        self.user(token?.strip_prefix("dev:")?)
    }

    // --- HTTP: device login, refresh, revoke, grants ------------------------------

    /// Answers an HTTP request. `body` is the parsed request body (JSON or form).
    pub fn http(
        &mut self,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: &Map<String, Value>,
    ) -> (u16, Body) {
        let found = |status: u16, value: Value| (status, Body::Json(value));
        if method == "GET" && path == "/health" {
            return found(200, json!({ "ok": true, "env": self.env }));
        }
        if method == "GET" && path == "/cli-login" {
            let user = self.first_user.clone().unwrap_or_else(|| "u1".into());
            return (200, Body::Html(crate::page::login_page(&user)));
        }
        if method != "POST" && !(method == "GET" && path == "/cli/grants") {
            return found(404, json!({ "error": "not_found" }));
        }

        if path == "/oauth/device_authorization" {
            let timings = self.options.timings;
            let device = Device {
                device_code: secret(),
                user_code: code(),
                expires_at: now() + timings.device_ttl_ms,
                interval_ms: timings.interval_ms,
                last_poll_at: None,
                status: DeviceStatus::Pending,
                user_id: None,
            };
            let answer = json!({
                "device_code": device.device_code,
                "user_code": device.user_code,
                "verification_uri": format!("{}/cli-login", self.web_origin),
                "verification_uri_complete": format!("{}/cli-login#{}", self.web_origin, device.user_code),
                "expires_in": num((timings.device_ttl_ms / 1000.0).round()),
                "interval": num(timings.interval_ms / 1000.0),
            });
            self.devices.push(device);
            return found(200, answer);
        }

        if path == "/oauth/token" {
            return match body.get("grant_type").and_then(Value::as_str) {
                Some("urn:ietf:params:oauth:grant-type:device_code") => {
                    let (status, value) = self.device_token(&text_of(body.get("device_code")));
                    found(status, value)
                }
                Some("refresh_token") => {
                    let wanted = body.get("refresh_token").and_then(Value::as_str);
                    let at = now();
                    let grant = self
                        .grants
                        .iter()
                        .find(|g| Some(g.refresh_token.as_str()) == wanted)
                        .filter(|g| !g.revoked && at < g.refresh_expires_at)
                        .map(|g| g.grant_id.clone());
                    match grant {
                        // The grant id stays; the binding does not drop (§3).
                        Some(id) => found(200, self.issue_tokens(&id)),
                        None => found(400, json!({ "error": "invalid_grant" })),
                    }
                }
                _ => found(400, json!({ "error": "unsupported_grant_type" })),
            };
        }

        if path == "/oauth/revoke" {
            let wanted = body.get("token").and_then(Value::as_str);
            let grant = self
                .grants
                .iter()
                .find(|g| Some(g.refresh_token.as_str()) == wanted && !g.revoked)
                .map(|g| g.grant_id.clone());
            if let Some(id) = grant {
                self.revoke(&id, "logged-out");
            }
            // RFC 7009: 200 whether or not the token was known.
            return found(200, json!({}));
        }

        // The browser side: the fake user's session is `Bearer dev:<userId>`.
        let Some(user) = self.browser_user(bearer).cloned() else {
            return found(401, json!({ "error": "unauthorized" }));
        };

        if path == "/oauth/device/verify" {
            if self.too_many_attempts(&user.id) {
                return found(429, json!({ "error": "too-many-attempts" }));
            }
            let user_code =
                normalize_code(body.get("user_code").and_then(Value::as_str).unwrap_or(""));
            let at = now();
            let device = self
                .devices
                .iter_mut()
                .find(|d| d.user_code == user_code && d.status == DeviceStatus::Pending)
                .filter(|d| at < d.expires_at);
            let Some(device) = device else {
                self.failed_attempt(&user.id);
                return found(400, json!({ "error": "invalid-code" }));
            };
            let approve = !matches!(body.get("approve"), Some(Value::Bool(false)))
                && body.get("approve").and_then(Value::as_str) != Some("false");
            device.status = if approve {
                DeviceStatus::Approved
            } else {
                DeviceStatus::Denied
            };
            device.user_id = Some(user.id.clone());
            return found(200, json!({ "ok": true }));
        }

        if path == "/cli/grants" && method == "GET" {
            let list: Vec<Value> = self
                .grants
                .iter()
                .filter(|g| g.user_id == user.id && !g.revoked)
                .map(|g| json!({ "grantId": g.grant_id, "createdAt": num(g.created_at), "env": g.env }))
                .collect();
            return found(200, Value::Array(list));
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
                return found(404, json!({ "error": "not_found" }));
            }
            self.revoke(&id, "revoked");
            return found(200, json!({ "ok": true }));
        }

        found(404, json!({ "error": "not_found" }))
    }

    /// One poll of the device grant (RFC 8628 §3.4).
    fn device_token(&mut self, device_code: &str) -> (u16, Value) {
        let at = now();
        let timings = self.options.timings;
        let auto = self.options.auto_approve;
        let first_user = self.first_user.clone();
        let Some(index) = self
            .devices
            .iter()
            .position(|d| d.device_code == device_code && d.status != DeviceStatus::Used)
        else {
            return (400, json!({ "error": "invalid_grant" }));
        };
        let device = &mut self.devices[index];
        if at >= device.expires_at {
            self.devices.remove(index);
            return (400, json!({ "error": "expired_token" }));
        }
        if let Some(last) = device.last_poll_at
            && at - last < device.interval_ms
        {
            // RFC 8628 §3.5: the interval grows by 5 s on every slow_down.
            device.interval_ms += timings.interval_ms.max(1.0);
            device.last_poll_at = Some(at);
            return (
                400,
                json!({ "error": "slow_down", "interval": num(device.interval_ms / 1000.0) }),
            );
        }
        device.last_poll_at = Some(at);
        if device.status == DeviceStatus::Pending
            && auto
            && let Some(first) = first_user
        {
            device.status = DeviceStatus::Approved;
            device.user_id = Some(first);
        }
        match device.status {
            DeviceStatus::Pending => return (400, json!({ "error": "authorization_pending" })),
            DeviceStatus::Denied => {
                self.devices.remove(index);
                return (400, json!({ "error": "access_denied" }));
            }
            _ => {}
        }
        device.status = DeviceStatus::Used;
        let user_id = device.user_id.clone().unwrap_or_default();
        let grant = Grant {
            grant_id: format!("g_{}", uuid()),
            user_id,
            env: self.env.clone(),
            created_at: at,
            revoked: false,
            refresh_token: format!("cr_{}", secret()),
            refresh_expires_at: at + timings.refresh_ttl_ms,
        };
        let id = grant.grant_id.clone();
        self.log(&format!("grant {id} for {}", grant.user_id));
        self.grants.push(grant);
        (200, self.issue_tokens(&id))
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
        if conn.lost {
            conn.lost = false;
            self.log(&format!("browser connection {id} answers again"));
            self.resume(id);
        }
    }

    /// One heartbeat tick; false once the connection is gone.
    pub fn heartbeat(&mut self, id: u64) -> bool {
        let misses = self.options.timings.heartbeat_misses;
        let Some(conn) = self.conn_mut(id) else {
            return false;
        };
        if conn.missed >= misses && !conn.lost {
            conn.lost = true;
            self.log(&format!("browser connection {id} lost: no pong"));
            for index in 0..self.bindings.len() {
                if self.bindings[index].browser == Some(id) {
                    self.suspend(index);
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
            return;
        }
        for index in 0..self.bindings.len() {
            if self.bindings[index].browser == Some(id) {
                self.suspend(index);
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
            (Kind::Cli, "pair") => self.cli_pair(conn),
            (Kind::Cli, "send") => self.cli_send(conn, args),
            (Kind::Cli, "detach") => self.cli_detach(conn),
            (Kind::Browser, "hello") => self.hello(conn, args),
            (Kind::Browser, "pairInfo") => self.pair_info(conn, args),
            (Kind::Browser, "pair") => self.browser_pair(conn, args),
            (Kind::Browser, "sendToCli") => self.send_to_cli(conn, args),
            (Kind::Browser, "publishCapabilities") => self.publish_capabilities(conn, args),
            (Kind::Browser, "publishWhere") => self.publish_where(conn, args),
            (Kind::Browser, "detach") => self.browser_detach(conn),
            (Kind::Browser, "unload") => self.unload(conn),
            (Kind::Browser, "logout") => self.browser_logout(conn),
            (Kind::Browser, "broadcast") => self.broadcast(conn, args),
            _ => Err(refuse("unknown-method", Some(method), None)),
        };
        let answer = match result {
            Ok(value) => json!({ "t": "result", "id": id, "ok": true, "value": value }),
            Err(error) => json!({ "t": "result", "id": id, "ok": false, "error": error }),
        };
        self.send_text(conn, answer.to_string());
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

    fn cli_pair(&mut self, conn: u64) -> Result<Value, Refusal> {
        let grant_id = self.grant_of(conn)?;
        let user_id = self
            .conn(conn)
            .map(|c| c.user_id.clone())
            .unwrap_or_default();
        // An older code of the grant stops working.
        for entry in self.pair_codes.values_mut() {
            if entry.grant_id == grant_id {
                entry.used = true;
            }
        }
        let entry = PairCode {
            code: code(),
            grant_id,
            user_id,
            expires_at: now() + self.options.timings.pair_ttl_ms,
            used: false,
        };
        let answer = json!({
            "code": entry.code,
            "expiresAt": num(entry.expires_at),
            "url": format!("{}/cli-pair#{}", self.web_origin, entry.code),
        });
        self.pair_codes.insert(entry.code.clone(), entry);
        Ok(answer)
    }

    fn cli_send(&mut self, conn: u64, args: &Map<String, Value>) -> Result<Value, Refusal> {
        let grant_id = self.grant_of(conn)?;
        let Some(Value::Object(envelope)) = args.get("envelope") else {
            return Err(refuse("invalid", None, None));
        };
        // The hub reads `kind` and the request id only (§2).
        let kind = envelope.get("kind").and_then(Value::as_str);
        if !matches!(kind, Some("cli-request" | "cancel" | "help-request"))
            || !envelope.get("id").is_some_and(Value::is_string)
        {
            return Err(refuse("invalid", None, None));
        }
        let Some(index) = self.binding_of_grant(&grant_id) else {
            return Err(refuse(
                "no-binding",
                Some("no binding for this grant"),
                None,
            ));
        };
        let binding = &self.bindings[index];
        if binding.state == State::Gone {
            return Err(refuse("gone", Some("the binding ended"), binding.reason));
        }
        let Some(browser) = binding.browser.filter(|_| binding.state == State::Active) else {
            return Err(refuse(
                "tab-unreachable",
                Some("the bound tab is not connected"),
                None,
            ));
        };
        let binding_id = binding.binding_id.clone();
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
                return Err(Value::Object(error));
            }
            bucket.0 -= 1.0;
        }
        let stamped_at = now();
        let expires_at = stamped_at + self.options.timings.request_ttl_ms;
        // The client's own copy of the stamp is dropped, then written anew.
        let mut stamped = envelope.clone();
        stamped.remove("origin");
        stamped.remove("stampedAt");
        stamped.remove("expiresAt");
        stamped.insert(
            "origin".into(),
            json!({ "kind": "cli", "grantId": grant_id, "bindingId": binding_id }),
        );
        stamped.insert("stampedAt".into(), num(stamped_at));
        stamped.insert("expiresAt".into(), num(expires_at));
        self.send(browser, "cli-message", Some(Value::Object(stamped)));
        Ok(json!({ "stampedAt": num(stamped_at), "expiresAt": num(expires_at) }))
    }

    fn cli_detach(&mut self, conn: u64) -> Result<Value, Refusal> {
        let grant_id = self.grant_of(conn)?;
        if let Some(index) = self.binding_of_grant(&grant_id) {
            self.end(index, "detached");
        }
        Ok(json!({ "ok": true }))
    }

    // --- browser methods ----------------------------------------------------------

    /// The page of `conn` came back within the suspension time (§5.3).
    fn resume(&mut self, conn: u64) {
        let Some((page_id, user_id)) = self
            .conn(conn)
            .and_then(|c| Some((c.page_id.clone()?, c.user_id.clone())))
        else {
            return;
        };
        let found = self.bindings.iter().position(|b| {
            b.page_id == page_id
                && b.user_id == user_id
                && (b.state == State::Suspended
                    || (b.state == State::Active && b.browser != Some(conn)))
        });
        if let Some(index) = found {
            let binding = &mut self.bindings[index];
            binding.suspend_generation += 1;
            binding.browser = Some(conn);
            binding.state = State::Active;
            self.announce(index);
        }
    }

    fn pair_check(&mut self, conn: u64, args: &Map<String, Value>) -> Result<String, Refusal> {
        let user_id = self
            .conn(conn)
            .map(|c| c.user_id.clone())
            .unwrap_or_default();
        if self.too_many_attempts(&user_id) {
            return Err(refuse(
                "too-many-attempts",
                Some("too many wrong codes; wait a minute"),
                None,
            ));
        }
        let wanted = normalize_code(args.get("code").and_then(Value::as_str).unwrap_or(""));
        let at = now();
        let Some(entry) = self
            .pair_codes
            .get(&wanted)
            .filter(|entry| !entry.used && at < entry.expires_at)
        else {
            self.failed_attempt(&user_id);
            return Err(refuse(
                "invalid-code",
                Some("the code is wrong, used or expired"),
                None,
            ));
        };
        let (grant_id, owner) = (entry.grant_id.clone(), entry.user_id.clone());
        if owner != user_id {
            self.failed_attempt(&user_id);
            // The CLI keeps waiting and says another account is signed in (§5.5).
            if let Some(cli) = self.cli_by_grant.get(&grant_id) {
                self.send(
                    *cli,
                    "pair-refused",
                    Some(json!({ "code": "other-account" })),
                );
            }
            return Err(refuse(
                "other-account",
                Some("the code belongs to another account"),
                None,
            ));
        }
        if self.user(&user_id).and_then(|u| u.terminal) == Some(false) {
            // Not consumed: the CLI hears it and ends with code 3 (§5.2).
            if let Some(cli) = self.cli_by_grant.get(&grant_id) {
                self.send(
                    *cli,
                    "pair-refused",
                    Some(json!({ "code": "no-terminal-access" })),
                );
            }
            return Err(refuse(
                "no-terminal-access",
                Some("this account has no terminal access"),
                None,
            ));
        }
        if self.grant(&grant_id).is_none_or(|g| g.revoked) {
            return Err(refuse("invalid-code", Some("the grant was revoked"), None));
        }
        Ok(wanted)
    }

    fn hello(&mut self, conn: u64, args: &Map<String, Value>) -> Result<Value, Refusal> {
        let Some(page_id) = args
            .get("pageId")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
        else {
            return Err(refuse("invalid", Some("pageId"), None));
        };
        let current = self.conn(conn).and_then(|c| c.page_id.clone());
        if current.is_some_and(|current| current != page_id) {
            // A new page on the same connection: the old page is gone.
            if let Some(old) = self.binding_of_browser(conn) {
                self.end(old, "closed");
            }
        }
        if let Some(c) = self.conn_mut(conn) {
            c.page_id = Some(page_id);
        }
        self.resume(conn);
        let live = self.binding_of_browser(conn).map(|index| {
            let b = &self.bindings[index];
            json!({ "bindingId": b.binding_id, "grantId": b.grant_id, "approvedAt": num(b.approved_at) })
        });
        Ok(json!({ "binding": live }))
    }

    fn pair_info(&mut self, conn: u64, args: &Map<String, Value>) -> Result<Value, Refusal> {
        let code = self.pair_check(conn, args)?;
        let grant = self
            .pair_codes
            .get(&code)
            .and_then(|entry| self.grant(&entry.grant_id))
            .ok_or_else(|| refuse("invalid-code", None, None))?;
        // Server facts only; never a name the CLI declares (§5.2).
        Ok(json!({ "grantCreatedAt": num(grant.created_at), "env": grant.env }))
    }

    fn browser_pair(&mut self, conn: u64, args: &Map<String, Value>) -> Result<Value, Refusal> {
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
        let code = self.pair_check(conn, args)?;
        // Atomic: the check and the use happen under one lock.
        let Some(entry) = self.pair_codes.get_mut(&code) else {
            return Err(refuse("invalid-code", None, None));
        };
        entry.used = true;
        let grant_id = entry.grant_id.clone();
        if let Some(old) = self.binding_of_browser(conn) {
            self.end(old, "replaced");
        }
        if let Some(previous) = self.binding_of_grant(&grant_id) {
            self.end(previous, "replaced");
        }
        let binding = Binding {
            binding_id: format!("b_{}", uuid()),
            grant_id: grant_id.clone(),
            user_id,
            page_id,
            browser: Some(conn),
            state: State::Active,
            reason: None,
            approved_at: now(),
            suspend_generation: 0,
            capabilities: None,
            published: None,
            location: None,
        };
        let answer = json!({
            "bindingId": binding.binding_id,
            "grantId": binding.grant_id,
            "approvedAt": num(binding.approved_at),
        });
        let binding_id = binding.binding_id.clone();
        self.bindings.push(binding);
        self.last_binding
            .insert(grant_id.clone(), binding_id.clone());
        if let Some(cli) = self.cli_by_grant.get(&grant_id) {
            let view = self.binding_view(self.bindings.len() - 1);
            self.send(*cli, "paired", Some(Value::Object(view)));
        }
        self.log(&format!("binding {binding_id} active"));
        Ok(answer)
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

    fn unload(&mut self, conn: u64) -> Result<Value, Refusal> {
        // The page is closing or reloading.
        if let Some(index) = self.binding_of_browser(conn) {
            self.end(index, "closed");
        }
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
        let json = json!({ "suspendMs": 500, "intervalMs": 0.5, "other": 1, "pairTtlMs": "x" });
        timings.merge(json.as_object().unwrap());
        assert_eq!(timings.suspend_ms, 500.0);
        assert_eq!(timings.interval_ms, 0.5);
        assert_eq!(timings.pair_ttl_ms, 120_000.0);
    }
}
