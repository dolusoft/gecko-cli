//! The Rust development hub in process: device login, the CLI and a fake tab over
//! WebSocket. The CLI's tests (`crates/gecko/tests/`) use a scripted hub, not this one.

#![allow(clippy::unwrap_used)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gecko_dev_hub::{Options, Running, Timings, start};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

async fn hub(change: impl FnOnce(&mut Options)) -> Running {
    let mut options = Options {
        port: 0,
        auto_approve: true,
        rate_per_second: 1000.0,
        ..Options::default()
    };
    options.timings = Timings {
        interval_ms: 0.0,
        ..Timings::default()
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

async fn login(hub: &Running) -> Value {
    let (_, device) = http(hub, "POST", "/oauth/device_authorization", None, json!({})).await;
    let (status, tokens) = http(
        hub,
        "POST",
        "/oauth/token",
        None,
        json!({
            "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            "device_code": device["device_code"],
        }),
    )
    .await;
    assert_eq!(status, 200, "{tokens}");
    tokens
}

type Socket = WebSocketStream<TcpStream>;

struct Client {
    socket: Socket,
    next: u64,
    events: Vec<(String, Value)>,
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
        };
        match client.frame().await {
            Ok(frame) => {
                assert_eq!(frame["name"], "ready", "{frame}");
                Ok(client)
            }
            Err(code) => Err(code),
        }
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

    async fn call(&mut self, method: &str, args: Value) -> Result<Value, Value> {
        self.next += 1;
        let id = self.next;
        let frame = json!({ "t": "invoke", "id": id, "method": method, "args": args });
        self.socket
            .send(Message::text(frame.to_string()))
            .await
            .unwrap();
        loop {
            let frame = self.frame().await.unwrap();
            if frame["t"] == "result" && frame["id"] == id {
                return if frame["ok"] == true {
                    Ok(frame["value"].clone())
                } else {
                    Err(frame["error"].clone())
                };
            }
            self.events.push((
                frame["name"].as_str().unwrap_or("").to_owned(),
                frame["payload"].clone(),
            ));
        }
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
        }
    }
}

/// A CLI connection and a tab bound to it.
async fn bound(hub: &Running) -> (Client, Client) {
    let tokens = login(hub).await;
    let mut cli = Client::connect(hub, tokens["access_token"].as_str(), "")
        .await
        .unwrap();
    let pair = cli.call("pair", json!({})).await.unwrap();
    let mut tab = Client::connect(hub, None, "?access_token=dev:u1")
        .await
        .unwrap();
    tab.call("hello", json!({ "pageId": "p1" })).await.unwrap();
    let code = pair["code"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .replace('-', " ");
    tab.call("pair", json!({ "code": code })).await.unwrap();
    assert_eq!(cli.event("paired").await["state"], "active");
    (cli, tab)
}

#[tokio::test]
async fn device_login_refresh_and_revoke() {
    let hub = hub(|_| {}).await;
    let (status, health) = http(&hub, "GET", "/health", None, Value::Null).await;
    assert_eq!(
        (status, health["env"].as_str()),
        (200, Some(hub.env.as_str()))
    );

    let tokens = login(&hub).await;
    assert!(tokens["access_token"].as_str().unwrap().starts_with("cli_"));
    assert_eq!(tokens["expires_in"], 3600);
    assert_eq!(tokens["user"]["id"], "u1");

    let refresh =
        json!({ "grant_type": "refresh_token", "refresh_token": tokens["refresh_token"] });
    let (status, again) = http(&hub, "POST", "/oauth/token", None, refresh.clone()).await;
    assert_eq!(status, 200);
    assert_eq!(again["grant_id"], tokens["grant_id"]);
    assert_ne!(again["access_token"], tokens["access_token"]);

    let (_, grants) = http(&hub, "GET", "/cli/grants", Some("dev:u1"), Value::Null).await;
    assert_eq!(grants.as_array().unwrap().len(), 1);
    let (status, _) = http(&hub, "GET", "/cli/grants", None, Value::Null).await;
    assert_eq!(status, 401);

    let (status, _) = http(
        &hub,
        "POST",
        "/oauth/revoke",
        None,
        json!({ "token": tokens["refresh_token"] }),
    )
    .await;
    assert_eq!(status, 200);
    let (status, refused) = http(&hub, "POST", "/oauth/token", None, refresh).await;
    assert_eq!(
        (status, refused["error"].as_str()),
        (400, Some("invalid_grant"))
    );
    hub.close();
}

#[tokio::test]
async fn a_pending_code_is_approved_in_the_browser() {
    let hub = hub(|options| options.auto_approve = false).await;
    let (_, device) = http(&hub, "POST", "/oauth/device_authorization", None, json!({})).await;
    let poll = json!({
        "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
        "device_code": device["device_code"],
    });
    let (_, pending) = http(&hub, "POST", "/oauth/token", None, poll.clone()).await;
    assert_eq!(pending["error"], "authorization_pending");
    let (status, _) = http(
        &hub,
        "POST",
        "/oauth/device/verify",
        Some("dev:u1"),
        json!({ "user_code": device["user_code"], "approve": true }),
    )
    .await;
    assert_eq!(status, 200);
    let (status, tokens) = http(&hub, "POST", "/oauth/token", None, poll).await;
    assert_eq!(
        (status, tokens["user"]["name"].as_str()),
        (200, Some("zahid"))
    );
    hub.close();
}

#[tokio::test]
async fn connections_are_refused_with_close_codes() {
    let hub = hub(|_| {}).await;
    let tokens = login(&hub).await;
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
    let mut first = Client::connect(&hub, Some(token), "").await.unwrap();
    assert_eq!(
        Client::connect(&hub, Some(token), "").await.err(),
        Some(4409)
    );
    let status = first.call("status", json!({})).await.unwrap();
    assert_eq!(status["binding"], Value::Null);
    assert_eq!(status["user"]["name"], "zahid");
    // A closed call frees the grant for the next one.
    first.socket.close(None).await.unwrap();
    while first.socket.next().await.is_some() {}
    assert!(Client::connect(&hub, Some(token), "").await.is_ok());
    hub.close();
}

#[tokio::test]
async fn requests_and_replies_travel_between_the_cli_and_the_tab() {
    let hub = hub(|_| {}).await;
    let (mut cli, mut tab) = bound(&hub).await;

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

    cli.call("detach", json!({})).await.unwrap();
    assert_eq!(tab.event("binding").await["reason"], "detached");
    let gone = cli
        .call("send", json!({ "envelope": envelope }))
        .await
        .unwrap_err();
    assert_eq!(
        (gone["code"].as_str(), gone["reason"].as_str()),
        (Some("gone"), Some("detached"))
    );
    hub.close();
}

#[tokio::test]
async fn the_rate_limit_says_when_to_retry() {
    let hub = hub(|options| {
        options.rate_per_second = 1.0;
        options.rate_burst = Some(1.0);
    })
    .await;
    let (mut cli, _tab) = bound(&hub).await;
    let envelope = |id: &str| json!({ "envelope": { "v": 1, "kind": "cli-request", "id": id } });
    cli.call("send", envelope("a")).await.unwrap();
    let limited = cli.call("send", envelope("b")).await.unwrap_err();
    assert_eq!(limited["code"], "rate-limited");
    let wait = limited["retryAfterMs"].as_f64().unwrap();
    assert!(wait > 0.0 && wait <= 1000.0, "{wait}");
    // A cancel is not counted.
    cli.call(
        "send",
        json!({ "envelope": { "v": 1, "kind": "cancel", "id": "a" } }),
    )
    .await
    .unwrap();
    hub.close();
}

#[tokio::test]
async fn a_closed_tab_suspends_then_ends_the_binding() {
    let hub = hub(|options| options.timings.suspend_ms = 200.0).await;
    let (mut cli, mut tab) = bound(&hub).await;
    tab.socket.close(None).await.unwrap();
    assert_eq!(cli.event("binding").await["state"], "suspended");
    let unreachable = cli
        .call(
            "send",
            json!({ "envelope": { "v": 1, "kind": "cli-request", "id": "x" } }),
        )
        .await
        .unwrap_err();
    assert_eq!(unreachable["code"], "tab-unreachable");
    let gone = cli.event("binding").await;
    assert_eq!(
        (gone["state"].as_str(), gone["reason"].as_str()),
        (Some("gone"), Some("suspended-timeout"))
    );
    hub.close();
}

#[tokio::test]
async fn a_tab_that_comes_back_in_time_keeps_its_binding() {
    let hub = hub(|options| options.timings.suspend_ms = 2_000.0).await;
    let (mut cli, mut tab) = bound(&hub).await;
    tab.socket.close(None).await.unwrap();
    assert_eq!(cli.event("binding").await["state"], "suspended");
    let mut again = Client::connect(&hub, None, "?access_token=dev:u1")
        .await
        .unwrap();
    let hello = again
        .call("hello", json!({ "pageId": "p1" }))
        .await
        .unwrap();
    assert!(hello["binding"]["bindingId"].is_string());
    assert_eq!(cli.event("binding").await["state"], "active");
    hub.close();
}
