//! The development transport against a scripted hub in the same process. The real Node
//! hub is exercised by the contract tests (step 4b-6).
// Test helpers outside #[test] functions; the upgrade callback's error type is
// tungstenite's own, large by design.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gecko_hub_client::dev::{DevOptions, DevTransport};
use gecko_hub_client::{CallError, Closed, HubEvent, Transport};
use serde_json::Value;
use serde_json::value::RawValue;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

type Hub = WebSocketStream<TcpStream>;

/// What the hub saw in the upgrade request.
#[derive(Debug, Default, Clone)]
struct Seen {
    uri: String,
    authorization: Option<String>,
    user_agent: Option<String>,
}

/// Starts a hub that accepts one connection and runs `script` on it. With `refuse`, the
/// upgrade is answered with that HTTP status instead.
async fn hub<F, Fut>(refuse: Option<StatusCode>, script: F) -> (String, Arc<Mutex<Seen>>)
where
    F: FnOnce(Hub) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let record = seen.clone();
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let callback = move |request: &Request, response: Response| {
            let header = |name: &str| {
                request
                    .headers()
                    .get(name)
                    .map(|value| value.to_str().unwrap().to_owned())
            };
            *record.lock().unwrap() = Seen {
                uri: request.uri().to_string(),
                authorization: header("authorization"),
                user_agent: header("user-agent"),
            };
            match refuse {
                Some(status) => {
                    let mut error = ErrorResponse::new(Some("refused".to_owned()));
                    *error.status_mut() = status;
                    Err(error)
                }
                None => Ok(response),
            }
        };
        if let Ok(socket) = tokio_tungstenite::accept_hdr_async(tcp, callback).await {
            script(socket).await;
        }
    });
    (host, seen)
}

fn options(host: &str) -> DevOptions {
    let mut options = DevOptions::new(host, "secret-token");
    options.connect_timeout = Duration::from_secs(2);
    options.invoke_timeout = Duration::from_secs(2);
    options
}

async fn say(hub: &mut Hub, frame: Value) {
    hub.send(Message::text(frame.to_string())).await.unwrap();
}

async fn ready(hub: &mut Hub) {
    say(
        hub,
        serde_json::json!({ "t": "event", "name": "ready", "payload": {} }),
    )
    .await;
}

/// The next invoke frame from the CLI.
async fn heard(hub: &mut Hub) -> Value {
    loop {
        match hub.next().await.unwrap().unwrap() {
            Message::Text(text) => return serde_json::from_str(text.as_str()).unwrap(),
            Message::Close(_) => panic!("closed"),
            _ => {}
        }
    }
}

async fn close(hub: &mut Hub, code: u16, reason: &str) {
    hub.close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: reason.to_owned().into(),
    }))
    .await
    .unwrap();
}

fn raw(json: &str) -> Box<RawValue> {
    RawValue::from_string(json.to_owned()).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn calls_events_and_close() {
    let (host, seen) = hub(None, |mut hub| async move {
        // An event before `ready` is kept and delivered first.
        say(
            &mut hub,
            serde_json::json!({ "t": "event", "name": "binding", "payload": { "state": "active" } }),
        )
        .await;
        ready(&mut hub).await;

        let status = heard(&mut hub).await;
        assert_eq!(status["t"], "invoke");
        assert_eq!(status["method"], "status");
        assert!(status.get("args").is_none());
        say(&mut hub, serde_json::json!({ "t": "result", "id": status["id"], "ok": true, "value": { "grantId": "g1" } })).await;

        let pair = heard(&mut hub).await;
        assert_eq!(pair["method"], "pair");
        say(&mut hub, serde_json::json!({ "t": "result", "id": pair["id"], "ok": true, "value": { "code": "ABCD" } })).await;
        say(&mut hub, serde_json::json!({ "t": "event", "name": "paired", "payload": { "bindingId": "b1" } })).await;
        say(&mut hub, serde_json::json!({ "t": "event", "name": "pair-refused", "payload": { "code": "expired" } })).await;

        let send = heard(&mut hub).await;
        assert_eq!(send["method"], "send");
        assert_eq!(send["args"], serde_json::json!({ "envelope": { "v": 1, "kind": "cli-request" } }));
        say(&mut hub, serde_json::json!({ "t": "result", "id": send["id"], "ok": true, "value": { "stampedAt": 1, "expiresAt": 2 } })).await;
        say(&mut hub, serde_json::json!({ "t": "event", "name": "reply", "payload": { "kind": "received" } })).await;

        let detach = heard(&mut hub).await;
        assert_eq!(detach["method"], "detach");
        say(&mut hub, serde_json::json!({ "t": "result", "id": detach["id"], "ok": true, "value": null })).await;

        // The CLI's own close.
        match hub.next().await {
            Some(Ok(Message::Close(Some(frame)))) => assert_eq!(u16::from(frame.code), 1000),
            other => panic!("unexpected {other:?}"),
        }
    })
    .await;

    let mut transport = DevTransport::connect(&options(&host)).await.unwrap();
    {
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.uri, "/hub");
        assert_eq!(seen.authorization.as_deref(), Some("Bearer secret-token"));
        assert!(seen.user_agent.unwrap().starts_with("gecko/"));
    }
    assert!(matches!(
        transport.next_event().await.unwrap(),
        HubEvent::Binding(_)
    ));
    assert_eq!(
        transport.status().await.unwrap().get(),
        "{\"grantId\":\"g1\"}"
    );
    assert_eq!(transport.pair().await.unwrap().get(), "{\"code\":\"ABCD\"}");
    assert!(matches!(
        transport.next_event().await.unwrap(),
        HubEvent::Paired(_)
    ));
    assert!(matches!(
        transport.next_event().await.unwrap(),
        HubEvent::PairRefused(code) if code == "expired"
    ));
    let receipt = transport
        .send(&raw("{\"v\":1,\"kind\":\"cli-request\"}"))
        .await
        .unwrap();
    assert_eq!(receipt.get(), "{\"stampedAt\":1,\"expiresAt\":2}");
    assert!(matches!(
        transport.next_event().await.unwrap(),
        HubEvent::Reply(payload) if payload.get() == "{\"kind\":\"received\"}"
    ));
    transport.detach().await.unwrap();
    assert_eq!(transport.close().await, Closed::Local);
}

#[tokio::test(flavor = "current_thread")]
async fn refused_call_is_a_hub_error() {
    let (host, _) = hub(None, |mut hub| async move {
        ready(&mut hub).await;
        let call = heard(&mut hub).await;
        say(&mut hub, serde_json::json!({ "t": "result", "id": call["id"], "ok": false, "error": { "code": "rate-limited", "retryAfterMs": 250 } })).await;
        let call = heard(&mut hub).await;
        say(&mut hub, serde_json::json!({ "t": "result", "id": call["id"], "ok": true, "value": 1 })).await;
        let _ = hub.next().await;
    })
    .await;
    let mut transport = DevTransport::connect(&options(&host)).await.unwrap();
    match transport.send(&raw("{}")).await {
        Err(CallError::Hub(error)) => {
            assert_eq!(error.code, "rate-limited");
            assert_eq!(error.retry_after_ms, Some(250));
        }
        other => panic!("unexpected {other:?}"),
    }
    // The connection survives a refused call.
    assert_eq!(transport.status().await.unwrap().get(), "1");
}

#[tokio::test(flavor = "current_thread")]
async fn close_codes_before_ready() {
    for (code, expected) in [
        (4401, Closed::Unauthorized),
        (4403, Closed::Unauthorized),
        (4409, Closed::Busy),
        (4410, Closed::Revoked),
    ] {
        let (host, _) = hub(None, move |mut hub| async move {
            close(&mut hub, code, "no").await;
        })
        .await;
        assert_eq!(
            DevTransport::connect(&options(&host)).await.err(),
            Some(expected),
            "{code}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn close_during_a_call_ends_it_with_the_reason() {
    let (host, _) = hub(None, |mut hub| async move {
        ready(&mut hub).await;
        let _ = heard(&mut hub).await;
        close(&mut hub, 4410, "revoked").await;
    })
    .await;
    let mut transport = DevTransport::connect(&options(&host)).await.unwrap();
    assert_eq!(
        transport.status().await.err(),
        Some(CallError::Closed(Closed::Revoked))
    );
    assert_eq!(transport.next_event().await.err(), Some(Closed::Revoked));
    assert_eq!(
        transport.detach().await.err(),
        Some(CallError::Closed(Closed::Revoked))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn upgrade_refusal_is_unauthorized() {
    let (host, _) = hub(Some(StatusCode::UNAUTHORIZED), |_| async {}).await;
    assert_eq!(
        DevTransport::connect(&options(&host)).await.err(),
        Some(Closed::Unauthorized)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn silent_hub_times_out() {
    let (host, _) = hub(None, |mut hub| async move {
        // Never says ready; holds the socket open until the client leaves.
        let _ = hub.next().await;
    })
    .await;
    let mut opts = options(&host);
    opts.connect_timeout = Duration::from_millis(200);
    match DevTransport::connect(&opts).await.err() {
        Some(Closed::Transport(text)) => assert!(text.contains("in time"), "{text}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn late_answer_times_out_and_the_connection_stays() {
    let (host, _) = hub(None, |mut hub| async move {
        ready(&mut hub).await;
        let late = heard(&mut hub).await;
        let next = heard(&mut hub).await;
        // The late answer first, then the second call's.
        say(
            &mut hub,
            serde_json::json!({ "t": "result", "id": late["id"], "ok": true, "value": "late" }),
        )
        .await;
        say(
            &mut hub,
            serde_json::json!({ "t": "result", "id": next["id"], "ok": true, "value": "next" }),
        )
        .await;
        let _ = hub.next().await;
    })
    .await;
    let mut opts = options(&host);
    opts.invoke_timeout = Duration::from_millis(200);
    let mut transport = DevTransport::connect(&opts).await.unwrap();
    assert_eq!(transport.status().await.err(), Some(CallError::TimedOut));
    // The hub answers both now; the late answer is dropped, the second call gets its own.
    assert_eq!(transport.status().await.unwrap().get(), "\"next\"");
}

#[tokio::test(flavor = "current_thread")]
async fn nothing_listening_is_a_transport_error() {
    let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = free.local_addr().unwrap().to_string();
    drop(free);
    assert!(matches!(
        DevTransport::connect(&options(&host)).await.err(),
        Some(Closed::Transport(_))
    ));
}
