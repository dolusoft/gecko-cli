//! The development hub as a black box: it starts through the testkit's [`Hub`] and the
//! tests speak to it over HTTP and WebSocket only. Attach codes, the CLI and fake tabs:
//! each row of the attach spec's §3.4, the reload of §4, the held `send`, the grant that
//! ends with its binding, CORS, the checks of what a tab publishes and sends, the
//! isolation of tabs, the stamp, and the one live CLI call per grant.
//! `crates/gecko/tests/e2e.rs` runs the real CLI against this hub.
//!
//! These tests live in the testkit's package, not in `gecko-dev-hub`'s: the testkit
//! depends on the hub, so a dev-dependency of the hub on the testkit would close a
//! cycle in the package graph.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gecko_testkit::{Hub, Options, Timings, User};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

/// The longest a test waits for one frame or one HTTP answer.
const WAIT: Duration = gecko_testkit::WAIT;

/// A hub with the testkit's options (a free port, a rate limit no test reaches) and the
/// default heartbeat, changed by `change`.
fn hub(change: impl FnOnce(&mut Options)) -> Hub {
    Hub::with(|options| {
        options.timings.heartbeat_ms = Timings::default().heartbeat_ms;
        change(options);
    })
}

/// An HTTP answer: status, headers (names in lower case) and the JSON body (`null` when
/// the body is not JSON).
#[derive(Debug)]
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// One HTTP request with extra header lines; the whole answer.
async fn request(
    hub: &Hub,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> Answer {
    let exchange = async {
        let mut stream = TcpStream::connect(("127.0.0.1", hub.port)).await.unwrap();
        let body = if body.is_null() {
            String::new()
        } else {
            body.to_string()
        };
        let extra: String = headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\r\n"))
            .collect();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: x\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        answer
    };
    let answer = tokio::time::timeout(WAIT, exchange)
        .await
        .unwrap_or_else(|_| panic!("{method} {path}: no answer within {WAIT:?}"));
    let (head, body) = answer.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Answer {
        status,
        headers,
        body: serde_json::from_str(body).unwrap_or(Value::Null),
    }
}

/// One HTTP request; the status and the JSON answer.
async fn http(
    hub: &Hub,
    method: &str,
    path: &str,
    auth: Option<&str>,
    body: Value,
) -> (u16, Value) {
    let bearer = auth.map(|a| format!("Bearer {a}"));
    let headers: Vec<(&str, &str)> = bearer
        .as_deref()
        .map(|b| vec![("Authorization", b)])
        .unwrap_or_default();
    let answer = request(hub, method, path, &headers, body).await;
    (answer.status, answer.body)
}

/// base64url without padding, decoded.
fn unbase64url(text: &str) -> Vec<u8> {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for byte in text.bytes() {
        let value = TABLE.iter().position(|t| *t == byte).unwrap() as u32;
        bits = (bits << 6) | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    out
}

/// The JSON inside a `gk1_` code.
fn opened(code: &str) -> Value {
    let body = code.strip_prefix("gk1_").expect("the gk1_ prefix");
    serde_json::from_slice(&unbase64url(body)).unwrap()
}

/// The secret of a code.
fn secret_of(code: &Value) -> String {
    opened(code["code"].as_str().unwrap())["s"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn attach(hub: &Hub, secret: &str) -> (u16, Value) {
    http(
        hub,
        "POST",
        "/cli/attach",
        None,
        json!({ "secret": secret }),
    )
    .await
}

type Socket = WebSocketStream<TcpStream>;

struct Client {
    socket: Socket,
    next: u64,
    events: Vec<(String, Value)>,
    results: Vec<(u64, Result<Value, Value>)>,
}

impl Client {
    /// Connects to `/hub`; `Err` carries the close code of a refusal.
    async fn connect(hub: &Hub, bearer: Option<&str>, query: &str) -> Result<Client, u16> {
        let mut request = format!("ws://{}/hub{query}", hub.env)
            .into_client_request()
            .unwrap();
        if let Some(bearer) = bearer {
            request
                .headers_mut()
                .insert("authorization", format!("Bearer {bearer}").parse().unwrap());
        }
        let deadline = Instant::now() + WAIT;
        let opening = async {
            let stream = TcpStream::connect(("127.0.0.1", hub.port)).await.unwrap();
            client_async(request, stream).await.unwrap().0
        };
        let socket = tokio::time::timeout_at(deadline, opening)
            .await
            .unwrap_or_else(|_| panic!("no WebSocket within {WAIT:?}"));
        let mut client = Client {
            socket,
            next: 0,
            events: Vec::new(),
            results: Vec::new(),
        };
        match client.frame(deadline).await {
            Ok(frame) => {
                assert_eq!(frame["name"], "ready", "{frame}");
                Ok(client)
            }
            Err(code) => Err(code),
        }
    }

    /// A tab of user `u1`, said hello as `page`.
    async fn tab(hub: &Hub, page: &str) -> Client {
        let mut tab = Client::connect(hub, None, "?access_token=dev:u1")
            .await
            .unwrap();
        let hello = tab.call("hello", json!({ "pageId": page })).await.unwrap();
        assert_eq!(hello, json!({ "binding": null }));
        tab
    }

    /// The next JSON frame before `deadline`, or the close code. The deadline holds
    /// across the pings in between: a heartbeat never stretches the wait.
    async fn frame(&mut self, deadline: Instant) -> Result<Value, u16> {
        loop {
            let message = tokio::time::timeout_at(deadline, self.socket.next())
                .await
                .unwrap_or_else(|_| panic!("no frame within {WAIT:?}; events: {:?}", self.events));
            match message {
                Some(Ok(Message::Text(text))) => {
                    return Ok(serde_json::from_str(text.as_str()).unwrap());
                }
                Some(Ok(Message::Close(frame))) => {
                    return Err(frame.map(|f| u16::from(f.code)).unwrap_or(0));
                }
                Some(Ok(_)) => {}
                other => panic!("{other:?}"),
            }
        }
    }

    /// Sends an `invoke` frame; the answer is read by `result`.
    async fn invoke(&mut self, method: &str, args: Value) -> u64 {
        self.next += 1;
        let id = self.next;
        let frame = json!({ "t": "invoke", "id": id, "method": method, "args": args });
        self.socket
            .send(Message::text(frame.to_string()))
            .await
            .unwrap();
        id
    }

    async fn result(&mut self, id: u64) -> Result<Value, Value> {
        if let Some(index) = self.results.iter().position(|(i, _)| *i == id) {
            return self.results.remove(index).1;
        }
        let deadline = Instant::now() + WAIT;
        loop {
            let frame = self.frame(deadline).await.unwrap();
            if frame["t"] == "result" {
                let answer = if frame["ok"] == true {
                    Ok(frame["value"].clone())
                } else {
                    Err(frame["error"].clone())
                };
                if frame["id"] == id {
                    return answer;
                }
                self.results.push((frame["id"].as_u64().unwrap(), answer));
                continue;
            }
            self.events.push((
                frame["name"].as_str().unwrap_or("").to_owned(),
                frame["payload"].clone(),
            ));
        }
    }

    async fn call(&mut self, method: &str, args: Value) -> Result<Value, Value> {
        let id = self.invoke(method, args).await;
        self.result(id).await
    }

    /// The next event of this name (events seen during calls count).
    async fn event(&mut self, name: &str) -> Value {
        if let Some(index) = self.events.iter().position(|(n, _)| n == name) {
            return self.events.remove(index).1;
        }
        let deadline = Instant::now() + WAIT;
        loop {
            let frame = self.frame(deadline).await.unwrap();
            if frame["t"] == "event" && frame["name"] == name {
                return frame["payload"].clone();
            }
            if frame["t"] == "event" {
                self.events.push((
                    frame["name"].as_str().unwrap_or("").to_owned(),
                    frame["payload"].clone(),
                ));
            }
        }
    }

    /// The close code the hub ends this connection with.
    async fn closed(&mut self) -> u16 {
        let deadline = Instant::now() + WAIT;
        loop {
            match self.frame(deadline).await {
                Ok(_) => continue,
                Err(code) => return code,
            }
        }
    }

    async fn close(mut self) {
        let closing = async {
            self.socket.close(None).await.unwrap();
            while self.socket.next().await.is_some() {}
        };
        tokio::time::timeout(WAIT, closing)
            .await
            .unwrap_or_else(|_| panic!("the connection did not close within {WAIT:?}"));
    }
}

fn send_args(id: &str) -> Value {
    json!({ "envelope": { "v": 1, "kind": "cli-request", "id": id, "argv": ["tasks.list"] } })
}

/// A tab with a fresh code, used by the CLI: the tab, the attach answer, the CLI.
async fn bound(hub: &Hub, page: &str, level: &str) -> (Client, Value, Client) {
    let mut tab = Client::tab(hub, page).await;
    let code = tab
        .call("attachCode", json!({ "pageId": page, "level": level }))
        .await
        .unwrap();
    let (status, tokens) = attach(hub, &secret_of(&code)).await;
    assert_eq!(status, 200, "{tokens}");
    let event = tab.event("binding").await;
    assert_eq!(event["state"], "active");
    assert_eq!(event["bindingId"], tokens["bindingId"]);
    let cli = Client::connect(hub, tokens["access_token"].as_str(), "")
        .await
        .unwrap();
    (tab, tokens, cli)
}

async fn refresh(hub: &Hub, tokens: &Value) -> (u16, Value) {
    http(
        hub,
        "POST",
        "/oauth/token",
        None,
        json!({ "grant_type": "refresh_token", "refresh_token": tokens["refresh_token"] }),
    )
    .await
}

#[tokio::test]
async fn a_code_carries_the_hub_and_binds_its_page() {
    let hub = hub(|_| {});
    let (status, health) = http(&hub, "GET", "/health", None, Value::Null).await;
    assert_eq!(
        (status, health["env"].as_str()),
        (200, Some(hub.env.as_str()))
    );

    let mut tab = Client::tab(&hub, "p1").await;
    let code = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "never-ask" }),
        )
        .await
        .unwrap();
    let text = code["code"].as_str().unwrap();
    assert!(text.starts_with("gk1_"), "{text}");
    assert!(!text.contains('='), "no padding: {text}");
    let inside = opened(text);
    assert_eq!(inside["v"], 1);
    assert_eq!(inside["hub"], hub.url.as_str());
    // At least 128 bits: 22 base64url characters.
    assert!(inside["s"].as_str().unwrap().len() >= 22);
    let left = code["expiresAt"].as_f64().unwrap() - now_ms();
    assert!(left > 110_000.0 && left <= 120_000.0, "{left}");

    let (status, tokens) = attach(&hub, inside["s"].as_str().unwrap()).await;
    assert_eq!(status, 200, "{tokens}");
    assert!(tokens["access_token"].as_str().unwrap().starts_with("cli_"));
    assert!(tokens["refresh_token"].is_string());
    assert_eq!(tokens["expires_in"], 3600);
    assert_eq!(tokens["level"], "never-ask");
    assert_eq!(tokens["user"], json!({ "id": "u1", "name": "zahid" }));
    let event = tab.event("binding").await;
    assert_eq!(
        event,
        json!({ "bindingId": tokens["bindingId"], "state": "active", "grantId": tokens["grantId"], "level": "never-ask" })
    );

    let mut cli = Client::connect(&hub, tokens["access_token"].as_str(), "")
        .await
        .unwrap();
    let status = cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["grantId"], tokens["grantId"]);
    assert_eq!(status["binding"]["state"], "active");
    assert_eq!(status["binding"]["level"], "never-ask");
    assert_eq!(status["binding"]["pageId"], "p1");

    // The token renews while the binding lives; the grant stays.
    let (status, again) = refresh(&hub, &tokens).await;
    assert_eq!(status, 200);
    assert_eq!(again["grant_id"], tokens["grantId"]);

    // `hello` again on the same connection names the binding.
    let hello = tab.call("hello", json!({ "pageId": "p1" })).await.unwrap();
    assert_eq!(
        hello,
        json!({ "binding": {
            "bindingId": tokens["bindingId"], "grantId": tokens["grantId"],
            "level": "never-ask", "approvedAt": status_approved(&mut cli).await
        } })
    );
    hub.stop();
}

async fn status_approved(cli: &mut Client) -> Value {
    cli.call("status", json!({})).await.unwrap()["binding"]["approvedAt"].clone()
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64
}

#[tokio::test]
async fn attach_codes_are_checked() {
    let hub = hub(|_| {});
    let mut tab = Client::connect(&hub, None, "?access_token=dev:u1")
        .await
        .unwrap();
    let before_hello = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "ask-server" }),
        )
        .await
        .unwrap_err();
    assert_eq!(before_hello["code"], "invalid");
    tab.call("hello", json!({ "pageId": "p1" })).await.unwrap();
    for args in [
        json!({ "pageId": "p1", "level": "always" }),
        json!({ "pageId": "p1" }),
        json!({ "pageId": "other", "level": "ask-server" }),
    ] {
        let refused = tab.call("attachCode", args.clone()).await.unwrap_err();
        assert_eq!(refused["code"], "invalid", "{args}");
    }
    hub.stop();
}

/// §3.4: a code used a second time, or expired, is refused with one message.
#[tokio::test]
async fn a_used_or_expired_code_is_invalid() {
    let hub = hub(|options| options.timings.attach_ttl_ms = 300.0);
    let (mut tab, tokens, _cli) = bound(&hub, "p1", "ask-server").await;
    let _ = tokens;
    let code = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "ask-server" }),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let (status, expired) = attach(&hub, &secret_of(&code)).await;
    assert_eq!((status, &expired["error"]), (400, &json!("invalid-code")));

    let code = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "ask-server" }),
        )
        .await
        .unwrap();
    let (status, _) = attach(&hub, &secret_of(&code)).await;
    assert_eq!(status, 200);
    let (status, used) = attach(&hub, &secret_of(&code)).await;
    assert_eq!((status, &used["error"]), (400, &json!("invalid-code")));
    let (_, unknown) = attach(&hub, "nope").await;
    assert_eq!(unknown, expired);
    assert_eq!(used, expired);
    hub.stop();
}

/// §3.4: without terminal access the hub makes no code.
#[tokio::test]
async fn no_terminal_access_makes_no_code() {
    let hub = hub(|options| {
        options.users = vec![User {
            id: "u1".into(),
            name: "Ada".into(),
            terminal: Some(false),
        }];
    });
    let mut tab = Client::tab(&hub, "p1").await;
    let refused = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "ask-server" }),
        )
        .await
        .unwrap_err();
    assert_eq!(refused["code"], "no-terminal-access");
    hub.stop();
}

/// §3.4: a used code replaces the tab's binding; an unused one leaves it alone.
#[tokio::test]
async fn a_new_code_replaces_the_tabs_binding() {
    let hub = hub(|_| {});
    let (mut tab, old, mut old_cli) = bound(&hub, "p1", "ask-server").await;
    let code = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "never-ask" }),
        )
        .await
        .unwrap();
    // Unused, it changes nothing.
    let status = old_cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"]["state"], "active");

    let (status, new) = attach(&hub, &secret_of(&code)).await;
    assert_eq!(status, 200);
    let gone = tab.event("binding").await;
    assert_eq!(
        gone,
        json!({ "bindingId": old["bindingId"], "state": "gone", "reason": "replaced" })
    );
    let active = tab.event("binding").await;
    assert_eq!(active["bindingId"], new["bindingId"]);
    assert_eq!(active["level"], "never-ask");
    // The old grant ends with its binding.
    let announced = old_cli.event("binding").await;
    assert_eq!(
        (announced["state"].as_str(), announced["reason"].as_str()),
        (Some("gone"), Some("replaced"))
    );
    assert_eq!(old_cli.closed().await, 4410);
    let (status, refused) = refresh(&hub, &old).await;
    assert_eq!((status, &refused["error"]), (400, &json!("invalid_grant")));
    assert_eq!(
        Client::connect(&hub, old["access_token"].as_str(), "")
            .await
            .err(),
        Some(4401)
    );
    hub.stop();
}

/// §3.4: a second code of the same page voids the first.
#[tokio::test]
async fn a_second_code_voids_the_first() {
    let hub = hub(|_| {});
    let mut tab = Client::tab(&hub, "p1").await;
    let first = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "ask-server" }),
        )
        .await
        .unwrap();
    let second = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "ask-server" }),
        )
        .await
        .unwrap();
    let (status, _) = attach(&hub, &secret_of(&first)).await;
    assert_eq!(status, 400);
    let (status, _) = attach(&hub, &secret_of(&second)).await;
    assert_eq!(status, 200);
    hub.stop();
}

/// §3.4: a code dies with its tab, closed or unloaded.
#[tokio::test]
async fn a_closed_tab_voids_its_code() {
    let hub = hub(|_| {});
    let mut tab = Client::tab(&hub, "p1").await;
    let code = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "ask-server" }),
        )
        .await
        .unwrap();
    tab.close().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (status, refused) = attach(&hub, &secret_of(&code)).await;
    assert_eq!((status, &refused["error"]), (400, &json!("invalid-code")));

    let mut tab = Client::tab(&hub, "p2").await;
    let code = tab
        .call(
            "attachCode",
            json!({ "pageId": "p2", "level": "ask-server" }),
        )
        .await
        .unwrap();
    tab.call("unload", json!({})).await.unwrap();
    let (status, _) = attach(&hub, &secret_of(&code)).await;
    assert_eq!(status, 400);
    hub.stop();
}

/// §3.4: five wrong secrets a minute per address; then even a right one waits.
#[tokio::test]
async fn too_many_wrong_secrets_are_refused() {
    let hub = hub(|_| {});
    let mut tab = Client::tab(&hub, "p1").await;
    let code = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "ask-server" }),
        )
        .await
        .unwrap();
    for _ in 0..5 {
        let (status, _) = attach(&hub, "wrong").await;
        assert_eq!(status, 400);
    }
    let (status, limited) = attach(&hub, &secret_of(&code)).await;
    assert_eq!(
        (status, limited["error"].as_str()),
        (429, Some("too-many-attempts"))
    );
    hub.stop();
}

/// The binding's level: in `status`, in the tab's `binding` event, in `hello`.
#[tokio::test]
async fn the_level_is_kept_with_the_binding() {
    let hub = hub(|_| {});
    let (mut tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    assert_eq!(tokens["level"], "ask-server");
    let status = cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"]["level"], "ask-server");
    let hello = tab.call("hello", json!({ "pageId": "p1" })).await.unwrap();
    assert_eq!(hello["binding"]["level"], "ask-server");
    hub.stop();
}

/// §4: unload, then the reloaded page resumes with the binding id; the CLI hears both.
#[tokio::test]
async fn a_reloaded_page_resumes_its_binding() {
    let hub = hub(|options| options.timings.unload_ms = 5_000.0);
    let (mut tab, tokens, mut cli) = bound(&hub, "p1", "never-ask").await;
    tab.call("unload", json!({})).await.unwrap();
    assert_eq!(cli.event("binding").await["state"], "suspended");
    tab.close().await;

    let mut again = Client::connect(&hub, None, "?access_token=dev:u1")
        .await
        .unwrap();
    let hello = again
        .call(
            "hello",
            json!({ "pageId": "p2", "resume": tokens["bindingId"] }),
        )
        .await
        .unwrap();
    assert_eq!(hello["binding"]["bindingId"], tokens["bindingId"]);
    assert_eq!(hello["binding"]["level"], "never-ask");
    let active = cli.event("binding").await;
    assert_eq!(
        (active["state"].as_str(), active["pageId"].as_str()),
        (Some("active"), Some("p2"))
    );
    hub.stop();
}

/// §4: without `resume` within `unloadMs`, the binding ends as `closed`.
#[tokio::test]
async fn an_unloaded_page_that_does_not_come_back_is_closed() {
    let hub = hub(|options| options.timings.unload_ms = 200.0);
    let (mut tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    tab.call("unload", json!({})).await.unwrap();
    assert_eq!(cli.event("binding").await["state"], "suspended");
    let gone = cli.event("binding").await;
    assert_eq!(
        (gone["state"].as_str(), gone["reason"].as_str()),
        (Some("gone"), Some("closed"))
    );
    assert_eq!(cli.closed().await, 4410);
    let (status, _) = refresh(&hub, &tokens).await;
    assert_eq!(status, 400);
    hub.stop();
}

/// §4: a copied tab holds the same id in `sessionStorage`; one live browser connection
/// per binding, so it gets none.
#[tokio::test]
async fn a_copied_tab_cannot_take_the_binding() {
    let hub = hub(|_| {});
    let (_tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    let mut copy = Client::connect(&hub, None, "?access_token=dev:u1")
        .await
        .unwrap();
    let hello = copy
        .call(
            "hello",
            json!({ "pageId": "p9", "resume": tokens["bindingId"] }),
        )
        .await
        .unwrap();
    assert_eq!(hello, json!({ "binding": null }));
    let status = cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"]["pageId"], "p1");

    // Another user cannot take a suspended one either.
    let hub2 = self::hub(|options| {
        options.users.push(User {
            id: "u2".into(),
            name: "Bob".into(),
            terminal: None,
        });
    });
    let (mut tab, tokens, _cli) = bound(&hub2, "p1", "ask-server").await;
    tab.call("unload", json!({})).await.unwrap();
    let mut other = Client::connect(&hub2, None, "?access_token=dev:u2")
        .await
        .unwrap();
    let hello = other
        .call(
            "hello",
            json!({ "pageId": "p2", "resume": tokens["bindingId"] }),
        )
        .await
        .unwrap();
    assert_eq!(hello, json!({ "binding": null }));
    hub.stop();
    hub2.stop();
}

/// §4: a send while the binding is suspended waits; the page's return delivers it and
/// answers the send then.
#[tokio::test]
async fn a_held_send_goes_out_when_the_page_returns() {
    let hub = hub(|_| {});
    let (mut tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    tab.call("unload", json!({})).await.unwrap();
    assert_eq!(cli.event("binding").await["state"], "suspended");
    let sent_at = now_ms();
    let send = cli.invoke("send", send_args("r1")).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut again = Client::connect(&hub, None, "?access_token=dev:u1")
        .await
        .unwrap();
    again
        .call(
            "hello",
            json!({ "pageId": "p2", "resume": tokens["bindingId"] }),
        )
        .await
        .unwrap();
    // The page has the answer of `hello`, and so its binding, before the held send:
    // nothing arrived ahead of that answer.
    assert!(
        again.events.iter().all(|(name, _)| name != "cli-message"),
        "a held send went out before the answer of hello"
    );
    let stamp = cli.result(send).await.unwrap();
    // Stamped when it went out, not when it was sent.
    assert!(stamp["stampedAt"].as_f64().unwrap() >= sent_at + 250.0);
    let message = again.event("cli-message").await;
    assert_eq!(message["id"], "r1");
    assert_eq!(message["stampedAt"], stamp["stampedAt"]);
    hub.stop();
}

/// §4: a held send whose page does not return in `holdMs` is `tab-unreachable`; one
/// whose binding ends meanwhile is `gone`.
#[tokio::test]
async fn a_held_send_ends_unreachable_or_gone() {
    let hub = hub(|options| {
        options.timings.hold_ms = 200.0;
        options.timings.unload_ms = 5_000.0;
    });
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    tab.call("unload", json!({})).await.unwrap();
    let started = std::time::Instant::now();
    let refused = cli.call("send", send_args("r1")).await.unwrap_err();
    assert_eq!(refused["code"], "tab-unreachable");
    assert!(started.elapsed() >= Duration::from_millis(180));
    hub.stop();

    let hub = self::hub(|options| {
        options.timings.hold_ms = 5_000.0;
        options.timings.unload_ms = 200.0;
    });
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    tab.call("unload", json!({})).await.unwrap();
    let refused = cli.call("send", send_args("r1")).await.unwrap_err();
    assert_eq!(
        (refused["code"].as_str(), refused["reason"].as_str()),
        (Some("gone"), Some("closed"))
    );
    hub.stop();
}

/// The rate limit counts before a send waits.
#[tokio::test]
async fn the_rate_limit_comes_before_the_hold() {
    let hub = hub(|options| {
        options.rate_per_second = 1.0;
        options.rate_burst = Some(1.0);
    });
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    cli.call("send", send_args("a")).await.unwrap();
    tab.call("unload", json!({})).await.unwrap();
    let started = std::time::Instant::now();
    let limited = cli.call("send", send_args("b")).await.unwrap_err();
    assert_eq!(limited["code"], "rate-limited");
    assert!(started.elapsed() < Duration::from_millis(1000));
    let wait = limited["retryAfterMs"].as_f64().unwrap();
    assert!(wait > 0.0 && wait <= 1000.0, "{wait}");
    hub.stop();
}

/// The grant ends with the binding: the tab's detach closes the CLI with 4410 and the
/// refresh is `invalid_grant`; the CLI's own detach answers first.
#[tokio::test]
async fn the_grant_ends_with_the_binding() {
    let hub = hub(|_| {});
    let (mut tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    tab.call("detach", json!({})).await.unwrap();
    assert_eq!(cli.event("binding").await["reason"], "detached");
    assert_eq!(cli.closed().await, 4410);
    let (status, refused) = refresh(&hub, &tokens).await;
    assert_eq!((status, &refused["error"]), (400, &json!("invalid_grant")));

    let (mut tab, tokens, mut cli) = bound(&hub, "p2", "ask-server").await;
    let detached = cli.call("detach", json!({})).await.unwrap();
    assert_eq!(detached["ok"], true);
    assert_eq!(cli.closed().await, 4410);
    assert_eq!(tab.event("binding").await["reason"], "detached");
    let (status, _) = refresh(&hub, &tokens).await;
    assert_eq!(status, 400);
    hub.stop();
}

#[tokio::test]
async fn connections_are_refused_with_close_codes() {
    let hub = hub(|_| {});
    let (_tab, tokens, first) = bound(&hub, "p1", "ask-server").await;
    let token = tokens["access_token"].as_str().unwrap();
    assert_eq!(
        Client::connect(&hub, Some("cli_nope"), "").await.err(),
        Some(4401)
    );
    assert_eq!(
        Client::connect(&hub, None, &format!("?access_token={token}"))
            .await
            .err(),
        Some(4403)
    );
    assert_eq!(
        Client::connect(&hub, None, "?access_token=dev:nobody")
            .await
            .err(),
        Some(4401)
    );
    assert_eq!(
        Client::connect(&hub, Some(token), "").await.err(),
        Some(4409)
    );
    // A closed call frees the grant for the next one.
    first.close().await;
    let mut next = Client::connect(&hub, Some(token), "").await.unwrap();
    let status = next.call("status", json!({})).await.unwrap();
    assert_eq!(status["user"]["name"], "zahid");
    let unknown = next.call("pair", json!({})).await.unwrap_err();
    assert_eq!(unknown["code"], "unknown-method");
    hub.stop();
}

#[tokio::test]
async fn removed_routes_are_gone() {
    let hub = hub(|_| {});
    for path in ["/oauth/device_authorization", "/oauth/device/verify"] {
        let (status, _) = http(&hub, "POST", path, Some("dev:u1"), json!({})).await;
        assert_eq!(status, 404, "{path}");
    }
    let (status, _) = http(&hub, "GET", "/cli-login", None, Value::Null).await;
    assert_eq!(status, 404);
    let (status, answer) = http(
        &hub,
        "POST",
        "/oauth/token",
        None,
        json!({ "grant_type": "urn:ietf:params:oauth:grant-type:device_code", "device_code": "x" }),
    )
    .await;
    assert_eq!(
        (status, answer["error"].as_str()),
        (400, Some("unsupported_grant_type"))
    );
    let mut tab = Client::tab(&hub, "p1").await;
    for method in ["pairInfo", "pair"] {
        let refused = tab
            .call(method, json!({ "code": "ABCD-EFGH" }))
            .await
            .unwrap_err();
        assert_eq!(refused["code"], "unknown-method", "{method}");
    }
    hub.stop();
}

#[tokio::test]
async fn requests_and_replies_travel_between_the_cli_and_the_tab() {
    let hub = hub(|_| {});
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;

    let envelope = json!({ "v": 1, "kind": "cli-request", "id": "r1", "argv": ["tasks.list"], "stampedAt": 1 });
    let stamp = cli
        .call("send", json!({ "envelope": envelope }))
        .await
        .unwrap();
    let message = tab.event("cli-message").await;
    assert_eq!(message["id"], "r1");
    assert_eq!(message["argv"], json!(["tasks.list"]));
    assert_eq!(message["origin"]["kind"], "cli");
    assert_eq!(message["stampedAt"], stamp["stampedAt"]);
    assert_eq!(
        stamp["expiresAt"].as_f64().unwrap() - stamp["stampedAt"].as_f64().unwrap(),
        30_000.0
    );

    let reply = json!({ "v": 1, "kind": "received", "id": "r1" });
    let sent = tab
        .call("sendToCli", json!({ "reply": reply }))
        .await
        .unwrap();
    assert_eq!(sent["delivered"], true);
    assert_eq!(cli.event("reply").await, reply);

    tab.call(
        "publishCapabilities",
        json!({ "revision": "r", "parts": ["{}"] }),
    )
    .await
    .unwrap();
    tab.call(
        "publishWhere",
        json!({ "title": "Tasks", "path": "/tasks" }),
    )
    .await
    .unwrap();
    let status = cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"]["state"], "active");
    assert_eq!(status["binding"]["capabilities"]["revision"], "r");
    assert_eq!(status["binding"]["where"]["title"], "Tasks");

    let invalid = cli
        .call(
            "send",
            json!({ "envelope": { "kind": "other", "id": "x" } }),
        )
        .await;
    assert_eq!(invalid.unwrap_err()["code"], "invalid");
    let unknown = cli.call("hello", json!({})).await;
    assert_eq!(unknown.unwrap_err()["code"], "unknown-method");
    hub.stop();
}

#[tokio::test]
async fn a_dropped_tab_suspends_then_ends_the_binding() {
    let hub = hub(|options| {
        options.timings.suspend_ms = 200.0;
        options.timings.hold_ms = 50.0;
    });
    let (tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    tab.close().await;
    assert_eq!(cli.event("binding").await["state"], "suspended");
    let unreachable = cli.call("send", send_args("x")).await.unwrap_err();
    assert_eq!(unreachable["code"], "tab-unreachable");
    let gone = cli.event("binding").await;
    assert_eq!(
        (gone["state"].as_str(), gone["reason"].as_str()),
        (Some("gone"), Some("suspended-timeout"))
    );
    hub.stop();
}

#[tokio::test]
async fn a_tab_whose_connection_drops_resumes_with_its_binding_id() {
    let hub = hub(|options| options.timings.suspend_ms = 2_000.0);
    let (tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    tab.close().await;
    assert_eq!(cli.event("binding").await["state"], "suspended");
    let mut again = Client::connect(&hub, None, "?access_token=dev:u1")
        .await
        .unwrap();
    let hello = again
        .call(
            "hello",
            json!({ "pageId": "p1", "resume": tokens["bindingId"] }),
        )
        .await
        .unwrap();
    assert_eq!(hello["binding"]["bindingId"], tokens["bindingId"]);
    assert_eq!(cli.event("binding").await["state"], "active");
    hub.stop();
}

/// A tab that stops answering pings is suspended; a pong on the same connection brings
/// the binding back.
#[tokio::test]
async fn a_silent_tab_comes_back_on_the_same_connection() {
    let hub = hub(|options| {
        options.timings = Timings {
            heartbeat_ms: 100.0,
            heartbeat_misses: 1.0,
            ..Timings::default()
        };
    });
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    // Read nothing: tungstenite answers pings only while the socket is read.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(cli.event("binding").await["state"], "suspended");
    // Reading answers the pings.
    let _ = tab.call("publishWhere", json!({})).await;
    assert_eq!(cli.event("binding").await["state"], "active");
    hub.stop();
}

/// A second user, `u2`, next to the default `u1`.
fn with_second_user(options: &mut Options) {
    options.users.push(User {
        id: "u2".into(),
        name: "Bob".into(),
        terminal: None,
    });
}

/// Row 2: the user revokes a grant from the profile (`/cli/grants/{id}/revoke`): the
/// tab hears its binding end as `revoked`, the CLI is closed with 4410, and the refresh
/// token is dead. Only the grant's own user may revoke it.
#[tokio::test]
async fn a_grant_revoked_from_the_profile_ends_its_binding() {
    let hub = hub(with_second_user);
    let (mut tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    let grant = tokens["grantId"].as_str().unwrap();
    let path = format!("/cli/grants/{grant}/revoke");

    let (status, _) = http(&hub, "POST", &path, None, json!({})).await;
    assert_eq!(status, 401);
    let (status, _) = http(&hub, "POST", &path, Some("dev:u2"), json!({})).await;
    assert_eq!(status, 404, "another user's grant");
    let (status, list) = http(&hub, "GET", "/cli/grants", Some("dev:u1"), Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(list[0]["grantId"], grant, "{list}");

    let (status, answer) = http(&hub, "POST", &path, Some("dev:u1"), json!({})).await;
    assert_eq!((status, answer), (200, json!({ "ok": true })));
    assert_eq!(
        tab.event("binding").await,
        json!({ "bindingId": tokens["bindingId"], "state": "gone", "reason": "revoked" })
    );
    let announced = cli.event("binding").await;
    assert_eq!(
        (announced["state"].as_str(), announced["reason"].as_str()),
        (Some("gone"), Some("revoked")),
        "{announced}"
    );
    assert_eq!(cli.closed().await, 4410);
    let (status, refused) = refresh(&hub, &tokens).await;
    assert_eq!((status, &refused["error"]), (400, &json!("invalid_grant")));
    assert_eq!(
        Client::connect(&hub, tokens["access_token"].as_str(), "")
            .await
            .err(),
        Some(4401)
    );
    let (_, list) = http(&hub, "GET", "/cli/grants", Some("dev:u1"), Value::Null).await;
    assert_eq!(list, json!([]));
    hub.stop();
}

/// Row 2: the CLI's own `/oauth/revoke` with its refresh token ends the binding the
/// same way; an unknown token is answered 200 too (RFC 7009) and ends nothing.
#[tokio::test]
async fn oauth_revoke_ends_the_binding() {
    let hub = hub(|_| {});
    let (mut tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;

    let unknown = json!({ "token": "cr_unknown" });
    let (status, answer) = http(&hub, "POST", "/oauth/revoke", None, unknown).await;
    assert_eq!((status, answer), (200, json!({})));
    let alive = cli.call("status", json!({})).await.unwrap();
    assert_eq!(alive["binding"]["state"], "active");

    let known = json!({ "token": tokens["refresh_token"] });
    let (status, answer) = http(&hub, "POST", "/oauth/revoke", None, known).await;
    assert_eq!((status, answer), (200, json!({})));
    assert_eq!(
        tab.event("binding").await,
        json!({ "bindingId": tokens["bindingId"], "state": "gone", "reason": "revoked" })
    );
    assert_eq!(cli.event("binding").await["reason"], "revoked");
    assert_eq!(cli.closed().await, 4410);
    let (status, refused) = refresh(&hub, &tokens).await;
    assert_eq!((status, &refused["error"]), (400, &json!("invalid_grant")));
    // The page is free: `hello` names no binding.
    let hello = tab.call("hello", json!({ "pageId": "p1" })).await.unwrap();
    assert_eq!(hello, json!({ "binding": null }));
    hub.stop();
}

/// A preflight from `origin`.
async fn preflight(hub: &Hub, path: &str, origin: &str) -> Answer {
    let headers = [
        ("Origin", origin),
        ("Access-Control-Request-Method", "POST"),
        (
            "Access-Control-Request-Headers",
            "authorization, content-type",
        ),
    ];
    request(hub, "OPTIONS", path, &headers, Value::Null).await
}

/// Row 3: a loopback origin gets 204 to its preflight and `Access-Control-Allow-Origin`
/// on the answer; another origin gets no CORS header.
#[tokio::test]
async fn loopback_origins_may_call_the_hub() {
    let hub = hub(|_| {});
    for origin in [
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://[::1]:8080",
    ] {
        let answer = preflight(&hub, "/cli/grants", origin).await;
        assert_eq!(answer.status, 204, "{origin}: {answer:?}");
        assert_eq!(
            answer.header("access-control-allow-origin"),
            Some(origin),
            "{answer:?}"
        );
        let methods = answer.header("access-control-allow-methods").unwrap();
        assert!(
            methods.contains("GET") && methods.contains("POST"),
            "{methods}"
        );
        let allowed = answer
            .header("access-control-allow-headers")
            .unwrap()
            .to_ascii_lowercase();
        assert!(
            allowed.contains("authorization") && allowed.contains("content-type"),
            "{allowed}"
        );
        assert_eq!(answer.header("vary"), Some("Origin"));

        let headers = [("Origin", origin), ("Authorization", "Bearer dev:u1")];
        let answer = request(&hub, "GET", "/cli/grants", &headers, Value::Null).await;
        assert_eq!(answer.status, 200, "{answer:?}");
        assert_eq!(answer.header("access-control-allow-origin"), Some(origin));
    }
    for origin in [
        "http://example.com",
        "https://localhost:5173",
        "http://localhost:5173/",
        "null",
    ] {
        let answer = preflight(&hub, "/cli/grants", origin).await;
        assert_eq!(answer.status, 204, "{origin}");
        assert_eq!(
            answer.header("access-control-allow-origin"),
            None,
            "{origin}: {answer:?}"
        );
        let headers = [("Origin", origin), ("Authorization", "Bearer dev:u1")];
        let answer = request(&hub, "GET", "/cli/grants", &headers, Value::Null).await;
        assert_eq!(
            answer.header("access-control-allow-origin"),
            None,
            "{origin}"
        );
    }
    hub.stop();
}

/// Row 3: with `--web <origin>` that origin is the only one; other loopback origins get
/// no CORS header.
#[tokio::test]
async fn the_web_origin_is_the_only_one() {
    let web = "http://localhost:5173";
    let hub = hub(|options| options.web_origin = Some(web.to_owned()));
    let answer = preflight(&hub, "/oauth/token", web).await;
    assert_eq!(answer.status, 204);
    assert_eq!(answer.header("access-control-allow-origin"), Some(web));
    for origin in ["http://localhost:5174", "http://127.0.0.1:5173"] {
        let answer = preflight(&hub, "/oauth/token", origin).await;
        assert_eq!(answer.status, 204, "{origin}");
        assert_eq!(
            answer.header("access-control-allow-origin"),
            None,
            "{origin}: {answer:?}"
        );
    }
    hub.stop();
}

/// Row 4: the same code used twice at once makes one binding; the other use is
/// `invalid-code`.
#[tokio::test]
async fn a_code_used_twice_at_once_binds_once() {
    let hub = hub(|_| {});
    // Each round fails one attempt; an address may fail five a minute.
    for round in 0..3 {
        let page = format!("p{round}");
        let mut tab = Client::tab(&hub, &page).await;
        let code = tab
            .call(
                "attachCode",
                json!({ "pageId": page, "level": "ask-server" }),
            )
            .await
            .unwrap();
        let secret = secret_of(&code);
        let ((first, a), (second, b)) = tokio::join!(attach(&hub, &secret), attach(&hub, &secret));
        let mut statuses = [first, second];
        statuses.sort_unstable();
        assert_eq!(statuses, [200, 400], "{a} {b}");
        let (won, lost) = if first == 200 { (a, b) } else { (b, a) };
        assert_eq!(lost["error"], "invalid-code", "{lost}");

        // The answer of `hello` comes after every event the two uses made.
        let hello = tab.call("hello", json!({ "pageId": page })).await.unwrap();
        assert_eq!(hello["binding"]["bindingId"], won["bindingId"], "{hello}");
        let bindings: Vec<&Value> = tab
            .events
            .iter()
            .filter(|(name, _)| name == "binding")
            .map(|(_, payload)| payload)
            .collect();
        assert_eq!(bindings.len(), 1, "{bindings:?}");
        assert_eq!(bindings[0]["state"], "active");
        assert_eq!(bindings[0]["bindingId"], won["bindingId"]);
        let (_, list) = http(&hub, "GET", "/cli/grants", Some("dev:u1"), Value::Null).await;
        assert_eq!(list.as_array().unwrap().len(), round + 1, "{list}");
    }
    hub.stop();
}

/// Row 6: a tab without a binding publishes nothing (`not-bound`); a malformed list is
/// `invalid`; a fourth part or a part over 16 KiB is `too-large`, and the last good
/// list stays.
#[tokio::test]
async fn published_lists_are_checked() {
    let hub = hub(|_| {});
    let mut loose = Client::tab(&hub, "p0").await;
    let list = json!({ "revision": "r", "parts": ["{}"] });
    let refused = loose.call("publishCapabilities", list).await.unwrap_err();
    assert_eq!(refused["code"], "not-bound", "{refused}");
    let place = json!({ "title": "Tasks" });
    let refused = loose.call("publishWhere", place).await.unwrap_err();
    assert_eq!(refused["code"], "not-bound", "{refused}");

    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    for list in [
        json!({ "parts": ["{}"] }),
        json!({ "revision": "", "parts": ["{}"] }),
        json!({ "revision": 7, "parts": ["{}"] }),
        json!({ "revision": "x".repeat(257), "parts": ["{}"] }),
        json!({ "revision": "r" }),
        json!({ "revision": "r", "parts": [] }),
        json!({ "revision": "r", "parts": "{}" }),
        json!({ "revision": "r", "parts": ["{}", 1] }),
    ] {
        let refused = tab
            .call("publishCapabilities", list.clone())
            .await
            .unwrap_err();
        assert_eq!(refused["code"], "invalid", "{list}");
    }
    let refused = tab
        .call("publishWhere", json!({ "title": 1 }))
        .await
        .unwrap_err();
    assert_eq!(refused["code"], "invalid", "{refused}");

    // Three parts, one of them 16 KiB exactly, is the most a list may be.
    let full = "x".repeat(16 * 1024);
    let over = "x".repeat(16 * 1024 + 1);
    let three = json!({ "revision": "r3", "parts": [full, "b", "c"] });
    tab.call("publishCapabilities", three.clone())
        .await
        .unwrap();
    for list in [
        json!({ "revision": "r4", "parts": ["a", "b", "c", "d"] }),
        json!({ "revision": "r5", "parts": [over] }),
    ] {
        let refused = tab.call("publishCapabilities", list).await.unwrap_err();
        assert_eq!(refused["code"], "too-large", "{refused}");
    }
    let status = cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"]["capabilities"], three);
    hub.stop();
}

/// Row 6: a new list replaces the old one whole; no part of the old one stays. The
/// place is replaced whole too.
#[tokio::test]
async fn a_new_list_replaces_the_old_one_whole() {
    let hub = hub(|_| {});
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    let older = json!({ "revision": "a", "parts": ["{\"a\":1}", "{\"b\":2}", "{\"c\":3}"] });
    tab.call("publishCapabilities", older.clone())
        .await
        .unwrap();
    let status = cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"]["capabilities"], older);
    let newer = json!({ "revision": "b", "parts": ["{\"d\":4}"] });
    tab.call("publishCapabilities", newer.clone())
        .await
        .unwrap();
    let status = cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"]["capabilities"], newer);

    let tasks = json!({ "title": "Tasks", "path": "/tasks" });
    tab.call("publishWhere", tasks).await.unwrap();
    let view = json!({ "title": "Data View" });
    tab.call("publishWhere", view.clone()).await.unwrap();
    let status = cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"]["where"], view);
    hub.stop();
}

/// Row 7: `broadcast` reaches the user's other tabs, never a CLI connection and never
/// another user's tab; a tab without a binding cannot `sendToCli`.
#[tokio::test]
async fn tabs_are_isolated_from_the_cli() {
    let hub = hub(with_second_user);
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    let mut sibling = Client::tab(&hub, "p2").await;
    let mut stranger = Client::connect(&hub, None, "?access_token=dev:u2")
        .await
        .unwrap();
    stranger
        .call("hello", json!({ "pageId": "p3" }))
        .await
        .unwrap();

    let envelope = json!({ "v": 1, "kind": "theme", "value": "dark" });
    let sent = tab
        .call("broadcast", json!({ "envelope": envelope }))
        .await
        .unwrap();
    assert_eq!(sent, json!({ "delivered": 1 }));
    assert_eq!(sibling.event("broadcast").await, envelope);
    // A round trip on each other connection: whatever the hub sent it before is in.
    cli.call("status", json!({})).await.unwrap();
    assert!(
        cli.events.iter().all(|(name, _)| name != "broadcast"),
        "the CLI got a broadcast: {:?}",
        cli.events
    );
    let _ = stranger.call("publishWhere", json!({})).await;
    assert!(
        stranger.events.iter().all(|(name, _)| name != "broadcast"),
        "another user's tab got a broadcast: {:?}",
        stranger.events
    );

    // The sibling has no binding: it cannot speak to the CLI.
    let reply = json!({ "v": 1, "kind": "received", "id": "r1" });
    let refused = sibling
        .call("sendToCli", json!({ "reply": reply }))
        .await
        .unwrap_err();
    assert_eq!(refused["code"], "not-bound", "{refused}");
    cli.call("status", json!({})).await.unwrap();
    assert!(
        cli.events.iter().all(|(name, _)| name != "reply"),
        "the CLI got a reply from an unbound tab: {:?}",
        cli.events
    );
    // The bound tab still can.
    let sent = tab
        .call("sendToCli", json!({ "reply": reply }))
        .await
        .unwrap();
    assert_eq!(sent["delivered"], true);
    assert_eq!(cli.event("reply").await, reply);
    hub.stop();
}

/// Row 9: the CLI's own `origin`, `stampedAt` and `expiresAt` are dropped; the tab gets
/// the hub's.
#[tokio::test]
async fn the_hub_writes_the_stamp() {
    let hub = hub(|_| {});
    let (mut tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    let forged = json!({
        "v": 1, "kind": "cli-request", "id": "r1", "argv": ["tasks.list"],
        "origin": { "kind": "browser", "grantId": "g_forged", "bindingId": "b_forged" },
        "stampedAt": 1, "expiresAt": 9_999_999_999_999u64,
    });
    let before = now_ms().floor();
    let stamp = cli
        .call("send", json!({ "envelope": forged }))
        .await
        .unwrap();
    let message = tab.event("cli-message").await;
    assert_eq!(
        message["origin"],
        json!({ "kind": "cli", "grantId": tokens["grantId"], "bindingId": tokens["bindingId"] })
    );
    assert_eq!(message["stampedAt"], stamp["stampedAt"]);
    assert_eq!(message["expiresAt"], stamp["expiresAt"]);
    let stamped_at = stamp["stampedAt"].as_f64().unwrap();
    assert!(stamped_at >= before, "{stamp}");
    assert_eq!(stamp["expiresAt"].as_f64().unwrap() - stamped_at, 30_000.0);
    assert_eq!(message["argv"], json!(["tasks.list"]));
    hub.stop();
}

/// Row 9: a second live CLI connection of the grant is closed with 4409, and the tab
/// hears `cli-busy`.
#[tokio::test]
async fn a_second_cli_call_is_busy_and_the_tab_hears_it() {
    let hub = hub(|_| {});
    let (mut tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    let before = now_ms().floor();
    assert_eq!(
        Client::connect(&hub, tokens["access_token"].as_str(), "")
            .await
            .err(),
        Some(4409)
    );
    let busy = tab.event("cli-busy").await;
    assert_eq!(busy["grantId"], tokens["grantId"], "{busy}");
    assert!(busy["at"].as_f64().unwrap() >= before, "{busy}");
    // The first call is untouched.
    let status = cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"]["state"], "active");
    hub.stop();
}

/// Row 9: call after call, each closed before the next opens, never leaves the grant
/// busy.
#[tokio::test]
async fn calls_one_after_another_never_find_the_grant_busy() {
    let hub = hub(|_| {});
    let (mut tab, tokens, first) = bound(&hub, "p1", "ask-server").await;
    first.close().await;
    for call in 0..20 {
        let mut cli = Client::connect(&hub, tokens["access_token"].as_str(), "")
            .await
            .unwrap_or_else(|code| panic!("call {call}: closed with {code}"));
        let status = cli.call("status", json!({})).await.unwrap();
        assert_eq!(status["binding"]["state"], "active", "call {call}");
        cli.close().await;
    }
    let _ = tab.call("publishWhere", json!({})).await;
    assert!(
        tab.events.iter().all(|(name, _)| name != "cli-busy"),
        "{:?}",
        tab.events
    );
    hub.stop();
}

/// Row 9: with the default rate (5 a second) a grant may spend 20 requests at once;
/// the next one is `rate-limited` with the wait until a request is free again.
#[tokio::test]
async fn the_default_rate_allows_a_burst_of_20() {
    let hub = Hub::start_with(Options {
        port: 0,
        ..Options::default()
    });
    let (_tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    // All sent at once, then the answers read. The bucket refills 5 a second
    // meanwhile, so at most that many more than the burst may pass.
    let started = std::time::Instant::now();
    let mut sent = Vec::new();
    for n in 0..25 {
        sent.push(cli.invoke("send", send_args(&format!("r{n}"))).await);
    }
    let mut answers = Vec::new();
    for id in sent {
        answers.push(cli.result(id).await);
    }
    let elapsed = started.elapsed().as_secs_f64();
    for (n, answer) in answers.iter().take(20).enumerate() {
        assert!(answer.is_ok(), "request {n}: {answer:?}");
    }
    let passed = answers.iter().filter(|a| a.is_ok()).count();
    let refill = (elapsed * 5.0).floor() as usize;
    assert!(
        passed <= 20 + refill,
        "{passed} passed in {elapsed} s: {answers:?}"
    );
    let limited = answers
        .iter()
        .find_map(|a| a.as_ref().err())
        .unwrap_or_else(|| panic!("nothing was rate-limited in {elapsed} s"));
    assert_eq!(limited["code"], "rate-limited", "{limited}");
    let wait = limited["retryAfterMs"].as_f64().unwrap();
    assert!(wait > 0.0 && wait <= 200.0, "{wait}");
    hub.stop();
}
