//! The Rust development hub in process: attach codes, the CLI and fake tabs over
//! WebSocket. Each row of the attach spec's §3.4, the reload of §4, the held `send`, and
//! the grant that ends with its binding. `crates/gecko/tests/attach.rs` runs the real
//! CLI against this hub.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gecko_dev_hub::{Options, Running, Timings, User, start};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

async fn hub(change: impl FnOnce(&mut Options)) -> Running {
    let mut options = Options {
        port: 0,
        rate_per_second: 1000.0,
        ..Options::default()
    };
    change(&mut options);
    start(options).await.unwrap()
}

/// One HTTP request; the status and the JSON answer.
async fn http(
    hub: &Running,
    method: &str,
    path: &str,
    auth: Option<&str>,
    body: Value,
) -> (u16, Value) {
    let mut stream = TcpStream::connect(("127.0.0.1", hub.port)).await.unwrap();
    let body = body.to_string();
    let auth = auth
        .map(|a| format!("Authorization: Bearer {a}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
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

async fn attach(hub: &Running, secret: &str) -> (u16, Value) {
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
    async fn connect(hub: &Running, bearer: Option<&str>, query: &str) -> Result<Client, u16> {
        let mut request = format!("ws://{}/hub{query}", hub.env)
            .into_client_request()
            .unwrap();
        if let Some(bearer) = bearer {
            request
                .headers_mut()
                .insert("authorization", format!("Bearer {bearer}").parse().unwrap());
        }
        let stream = TcpStream::connect(("127.0.0.1", hub.port)).await.unwrap();
        let (socket, _) = client_async(request, stream).await.unwrap();
        let mut client = Client {
            socket,
            next: 0,
            events: Vec::new(),
            results: Vec::new(),
        };
        match client.frame().await {
            Ok(frame) => {
                assert_eq!(frame["name"], "ready", "{frame}");
                Ok(client)
            }
            Err(code) => Err(code),
        }
    }

    /// A tab of user `u1`, said hello as `page`.
    async fn tab(hub: &Running, page: &str) -> Client {
        let mut tab = Client::connect(hub, None, "?access_token=dev:u1")
            .await
            .unwrap();
        let hello = tab.call("hello", json!({ "pageId": page })).await.unwrap();
        assert_eq!(hello, json!({ "binding": null }));
        tab
    }

    /// The next JSON frame, or the close code.
    async fn frame(&mut self) -> Result<Value, u16> {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), self.socket.next())
                .await
                .expect("a frame within 5 s");
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
        loop {
            let frame = self.frame().await.unwrap();
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
        loop {
            let frame = self.frame().await.unwrap();
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
        loop {
            match self.frame().await {
                Ok(_) => continue,
                Err(code) => return code,
            }
        }
    }

    async fn close(mut self) {
        self.socket.close(None).await.unwrap();
        while self.socket.next().await.is_some() {}
    }
}

fn send_args(id: &str) -> Value {
    json!({ "envelope": { "v": 1, "kind": "cli-request", "id": id, "argv": ["tasks.list"] } })
}

/// A tab with a fresh code, used by the CLI: the tab, the attach answer, the CLI.
async fn bound(hub: &Running, page: &str, level: &str) -> (Client, Value, Client) {
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

async fn refresh(hub: &Running, tokens: &Value) -> (u16, Value) {
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
    let hub = hub(|_| {}).await;
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
    hub.close();
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
    let hub = hub(|_| {}).await;
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
    hub.close();
}

/// §3.4: a code used a second time, or expired, is refused with one message.
#[tokio::test]
async fn a_used_or_expired_code_is_invalid() {
    let hub = hub(|options| options.timings.attach_ttl_ms = 300.0).await;
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
    hub.close();
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
    })
    .await;
    let mut tab = Client::tab(&hub, "p1").await;
    let refused = tab
        .call(
            "attachCode",
            json!({ "pageId": "p1", "level": "ask-server" }),
        )
        .await
        .unwrap_err();
    assert_eq!(refused["code"], "no-terminal-access");
    hub.close();
}

/// §3.4: a used code replaces the tab's binding; an unused one leaves it alone.
#[tokio::test]
async fn a_new_code_replaces_the_tabs_binding() {
    let hub = hub(|_| {}).await;
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
    hub.close();
}

/// §3.4: a second code of the same page voids the first.
#[tokio::test]
async fn a_second_code_voids_the_first() {
    let hub = hub(|_| {}).await;
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
    hub.close();
}

/// §3.4: a code dies with its tab, closed or unloaded.
#[tokio::test]
async fn a_closed_tab_voids_its_code() {
    let hub = hub(|_| {}).await;
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
    hub.close();
}

/// §3.4: five wrong secrets a minute per address; then even a right one waits.
#[tokio::test]
async fn too_many_wrong_secrets_are_refused() {
    let hub = hub(|_| {}).await;
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
    hub.close();
}

/// The binding's level: in `status`, in the tab's `binding` event, in `hello`.
#[tokio::test]
async fn the_level_is_kept_with_the_binding() {
    let hub = hub(|_| {}).await;
    let (mut tab, tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    assert_eq!(tokens["level"], "ask-server");
    let status = cli.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"]["level"], "ask-server");
    let hello = tab.call("hello", json!({ "pageId": "p1" })).await.unwrap();
    assert_eq!(hello["binding"]["level"], "ask-server");
    hub.close();
}

/// §4: unload, then the reloaded page resumes with the binding id; the CLI hears both.
#[tokio::test]
async fn a_reloaded_page_resumes_its_binding() {
    let hub = hub(|options| options.timings.unload_ms = 5_000.0).await;
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
    hub.close();
}

/// §4: without `resume` within `unloadMs`, the binding ends as `closed`.
#[tokio::test]
async fn an_unloaded_page_that_does_not_come_back_is_closed() {
    let hub = hub(|options| options.timings.unload_ms = 200.0).await;
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
    hub.close();
}

/// §4: a copied tab holds the same id in `sessionStorage`; one live browser connection
/// per binding, so it gets none.
#[tokio::test]
async fn a_copied_tab_cannot_take_the_binding() {
    let hub = hub(|_| {}).await;
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
    })
    .await;
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
    hub.close();
    hub2.close();
}

/// §4: a send while the binding is suspended waits; the page's return delivers it and
/// answers the send then.
#[tokio::test]
async fn a_held_send_goes_out_when_the_page_returns() {
    let hub = hub(|_| {}).await;
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
    hub.close();
}

/// §4: a held send whose page does not return in `holdMs` is `tab-unreachable`; one
/// whose binding ends meanwhile is `gone`.
#[tokio::test]
async fn a_held_send_ends_unreachable_or_gone() {
    let hub = hub(|options| {
        options.timings.hold_ms = 200.0;
        options.timings.unload_ms = 5_000.0;
    })
    .await;
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    tab.call("unload", json!({})).await.unwrap();
    let started = std::time::Instant::now();
    let refused = cli.call("send", send_args("r1")).await.unwrap_err();
    assert_eq!(refused["code"], "tab-unreachable");
    assert!(started.elapsed() >= Duration::from_millis(180));
    hub.close();

    let hub = self::hub(|options| {
        options.timings.hold_ms = 5_000.0;
        options.timings.unload_ms = 200.0;
    })
    .await;
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    tab.call("unload", json!({})).await.unwrap();
    let refused = cli.call("send", send_args("r1")).await.unwrap_err();
    assert_eq!(
        (refused["code"].as_str(), refused["reason"].as_str()),
        (Some("gone"), Some("closed"))
    );
    hub.close();
}

/// The rate limit counts before a send waits.
#[tokio::test]
async fn the_rate_limit_comes_before_the_hold() {
    let hub = hub(|options| {
        options.rate_per_second = 1.0;
        options.rate_burst = Some(1.0);
    })
    .await;
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    cli.call("send", send_args("a")).await.unwrap();
    tab.call("unload", json!({})).await.unwrap();
    let started = std::time::Instant::now();
    let limited = cli.call("send", send_args("b")).await.unwrap_err();
    assert_eq!(limited["code"], "rate-limited");
    assert!(started.elapsed() < Duration::from_millis(1000));
    let wait = limited["retryAfterMs"].as_f64().unwrap();
    assert!(wait > 0.0 && wait <= 1000.0, "{wait}");
    hub.close();
}

/// The grant ends with the binding: the tab's detach closes the CLI with 4410 and the
/// refresh is `invalid_grant`; the CLI's own detach answers first.
#[tokio::test]
async fn the_grant_ends_with_the_binding() {
    let hub = hub(|_| {}).await;
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
    hub.close();
}

#[tokio::test]
async fn connections_are_refused_with_close_codes() {
    let hub = hub(|_| {}).await;
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
    hub.close();
}

#[tokio::test]
async fn removed_routes_are_gone() {
    let hub = hub(|_| {}).await;
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
    hub.close();
}

#[tokio::test]
async fn requests_and_replies_travel_between_the_cli_and_the_tab() {
    let hub = hub(|_| {}).await;
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
    hub.close();
}

#[tokio::test]
async fn a_dropped_tab_suspends_then_ends_the_binding() {
    let hub = hub(|options| {
        options.timings.suspend_ms = 200.0;
        options.timings.hold_ms = 50.0;
    })
    .await;
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
    hub.close();
}

#[tokio::test]
async fn a_tab_whose_connection_drops_resumes_with_its_binding_id() {
    let hub = hub(|options| options.timings.suspend_ms = 2_000.0).await;
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
    hub.close();
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
    })
    .await;
    let (mut tab, _tokens, mut cli) = bound(&hub, "p1", "ask-server").await;
    // Read nothing: tungstenite answers pings only while the socket is read.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(cli.event("binding").await["state"], "suspended");
    // Reading answers the pings.
    let _ = tab.call("publishWhere", json!({})).await;
    assert_eq!(cli.event("binding").await["state"], "active");
    hub.close();
}
