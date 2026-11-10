//! The SignalR client (spec §7.4) against tests/signalr-hub.
//!
//! Needs a running hub with short keep-alive settings:
//!   TEST_HUB_KEEPALIVE_MS=500 TEST_HUB_CLIENT_TIMEOUT_MS=2000 SignalRTestHub --urls http://127.0.0.1:5199
//! and GECKO_SIGNALR_TEST_HUB=http://127.0.0.1:5199. Without the variable the tests are
//! skipped; the CI job that starts the hub also sets GECKO_SIGNALR_TEST_HUB_REQUIRED=1,
//! so a missing hub fails there instead of passing silently.
// Test helpers outside #[test] functions: clippy's test allowances do not reach them.
#![allow(clippy::print_stderr, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gecko_hub_client::Closed;
use gecko_hub_client::proxy::ProxyRule;
use gecko_hub_client::signalr::{Connection, InvokeError, Options, hub_result};
use serde_json::value::RawValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn base() -> Option<String> {
    match std::env::var("GECKO_SIGNALR_TEST_HUB") {
        Ok(url) => Some(url.trim_end_matches('/').to_owned()),
        Err(_) if std::env::var_os("GECKO_SIGNALR_TEST_HUB_REQUIRED").is_some() => {
            panic!("GECKO_SIGNALR_TEST_HUB is not set although the hub is required")
        }
        Err(_) => {
            eprintln!("skipped: GECKO_SIGNALR_TEST_HUB is not set");
            None
        }
    }
}

fn options(base: &str, token: &str) -> Options {
    let mut options = Options::new(format!("{base}/hub"), token);
    options.keep_alive = Duration::from_millis(1000);
    options.server_timeout = Duration::from_secs(5);
    options.proxy = ProxyRule::none();
    options
}

/// A CONNECT proxy on a random local port; counts the tunnels it opened.
async fn connect_proxy() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let tunnels = Arc::new(AtomicUsize::new(0));
    let counter = tunnels.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else {
                return;
            };
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if client.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).into_owned();
                let target = head.split(' ').nth(1).unwrap_or("").to_owned();
                let Ok(mut upstream) = TcpStream::connect(target).await else {
                    let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                    return;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                client
                    .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .await
                    .unwrap();
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            });
        }
    });
    (format!("http://{address}"), tunnels)
}

fn proxy_rule(pairs: &[(&str, &str)]) -> ProxyRule {
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    ProxyRule::from_lookup(move |name| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    })
}

fn raw(json: &str) -> Box<RawValue> {
    RawValue::from_string(json.to_owned()).unwrap()
}

async fn send(connection: &mut Connection, envelope: &str) -> Result<Box<RawValue>, InvokeError> {
    connection.invoke("Send", &[&raw(envelope)]).await
}

#[tokio::test(flavor = "current_thread")]
async fn skip_negotiation_status_send_and_reply() {
    let Some(base) = base() else { return };
    let mut connection = Connection::connect(&options(&base, "good")).await.unwrap();
    let status = connection.invoke("Status", &[]).await.unwrap();
    assert!(
        hub_result(&status)
            .unwrap()
            .get()
            .contains("\"bound\":true")
    );

    let accepted = send(&mut connection, "{\"kind\":\"echo\",\"n\":1}")
        .await
        .unwrap();
    assert_eq!(hub_result(&accepted).unwrap().get(), "{\"accepted\":true}");
    let event = connection.next_event().await.unwrap();
    assert_eq!(event.target, "Reply");
    assert_eq!(event.arguments[0].get(), "{\"kind\":\"echo\",\"n\":1}");
    assert_eq!(connection.close().await, Closed::Local);
}

#[tokio::test(flavor = "current_thread")]
async fn negotiate_then_connect() {
    let Some(base) = base() else { return };
    let mut opts = options(&base, "good");
    opts.skip_negotiation = false;
    let mut connection = Connection::connect(&opts).await.unwrap();
    assert!(connection.invoke("Status", &[]).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn negotiate_redirect_switches_url_and_token() {
    let Some(base) = base() else { return };
    let mut opts = Options::new(format!("{base}/redirect"), "outer");
    opts.proxy = ProxyRule::none();
    opts.skip_negotiation = false;
    let mut connection = Connection::connect(&opts).await.unwrap();
    assert!(connection.invoke("Status", &[]).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn unknown_token_is_unauthorized_on_both_paths() {
    let Some(base) = base() else { return };
    let mut opts = options(&base, "nope");
    assert_eq!(
        Connection::connect(&opts).await.err(),
        Some(Closed::Unauthorized)
    );
    opts.skip_negotiation = false;
    assert_eq!(
        Connection::connect(&opts).await.err(),
        Some(Closed::Unauthorized)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn handshake_error_is_reported() {
    let Some(base) = base() else { return };
    let mut opts = options(&base, "good");
    opts.handshake = "{\"protocol\":\"nope\",\"version\":1}\u{1e}".to_owned();
    match Connection::connect(&opts).await.err() {
        Some(Closed::Handshake(text)) => assert!(text.contains("nope"), "{text}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn server_close_after_handshake_is_busy() {
    let Some(base) = base() else { return };
    let mut connection = Connection::connect(&options(&base, "busy")).await.unwrap();
    assert_eq!(connection.reason().await, Closed::Busy);
    assert_eq!(
        connection.invoke("Status", &[]).await.err(),
        Some(InvokeError::Closed(Closed::Busy))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn server_close_during_a_call_is_revoked() {
    let Some(base) = base() else { return };
    let mut connection = Connection::connect(&options(&base, "good")).await.unwrap();
    let result = send(
        &mut connection,
        "{\"kind\":\"close\",\"reason\":\"4410 revoked\"}",
    )
    .await;
    // The Close may arrive before or after the Completion; either way the end is `Revoked`.
    if let Err(error) = result {
        assert_eq!(error, InvokeError::Closed(Closed::Revoked));
    }
    assert_eq!(connection.reason().await, Closed::Revoked);
}

#[tokio::test(flavor = "current_thread")]
async fn hub_error_value_and_completion_error() {
    let Some(base) = base() else { return };
    let mut connection = Connection::connect(&options(&base, "good")).await.unwrap();
    let refused = send(&mut connection, "{\"kind\":\"refuse\"}")
        .await
        .unwrap();
    let error = hub_result(&refused).unwrap_err();
    assert_eq!(error.code, "rate-limited");
    assert_eq!(error.retry_after_ms, Some(1000));

    match send(&mut connection, "{\"kind\":\"throw\"}").await {
        Err(InvokeError::Failed(text)) => assert!(text.contains("boom"), "{text}"),
        other => panic!("unexpected {other:?}"),
    }
    // The connection survives a failed call.
    assert!(connection.invoke("Status", &[]).await.is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn server_call_waiting_for_a_result_gets_an_error() {
    let Some(base) = base() else { return };
    let mut connection = Connection::connect(&options(&base, "good")).await.unwrap();
    let answer = send(&mut connection, "{\"kind\":\"ask\"}").await.unwrap();
    let value = hub_result(&answer).unwrap();
    assert!(value.get().contains("client error"), "{}", value.get());
}

#[tokio::test(flavor = "current_thread")]
async fn several_records_arrive_in_order() {
    let Some(base) = base() else { return };
    let mut connection = Connection::connect(&options(&base, "good")).await.unwrap();
    send(&mut connection, "{\"kind\":\"burst\"}").await.unwrap();
    for index in 0..3 {
        let event = connection.next_event().await.unwrap();
        assert_eq!(
            event.arguments[0].get(),
            format!("{{\"kind\":\"burst\",\"index\":{index}}}")
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn client_pings_keep_an_idle_connection() {
    let Some(base) = base() else { return };
    // The hub drops a client that is silent for 2 s once it has sent its first Ping.
    let mut connection = Connection::connect(&options(&base, "good")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(connection.invoke("Status", &[]).await.is_ok());

    let mut slow = options(&base, "good");
    slow.keep_alive = Duration::from_millis(3000);
    let mut connection = Connection::connect(&slow).await.unwrap();
    tokio::time::sleep(Duration::from_secs(7)).await;
    assert!(matches!(
        connection.invoke("Status", &[]).await,
        Err(InvokeError::Closed(_))
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn silent_server_times_out() {
    let Some(base) = base() else { return };
    // The hub pings every 500 ms; a 200 ms limit cannot be met.
    let mut opts = options(&base, "good");
    opts.server_timeout = Duration::from_millis(200);
    let mut connection = Connection::connect(&opts).await.unwrap();
    assert_eq!(connection.reason().await, Closed::ServerTimeout);
}

#[tokio::test(flavor = "current_thread")]
async fn late_answer_times_out_and_the_connection_stays() {
    let Some(base) = base() else { return };
    let mut opts = options(&base, "good");
    opts.invoke_timeout = Duration::from_millis(300);
    let mut connection = Connection::connect(&opts).await.unwrap();
    assert_eq!(
        send(&mut connection, "{\"kind\":\"slow\",\"ms\":1500}")
            .await
            .err(),
        Some(InvokeError::TimedOut)
    );
    // The late Completion is dropped; the next call gets its own answer.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let status = connection.invoke("Status", &[]).await.unwrap();
    assert!(hub_result(&status).unwrap().get().contains("\"bound\""));
}

#[tokio::test(flavor = "current_thread")]
async fn websocket_goes_through_the_connect_tunnel() {
    let Some(base) = base() else { return };
    let (proxy, tunnels) = connect_proxy().await;
    let mut opts = options(&base, "good");
    opts.proxy = proxy_rule(&[("HTTP_PROXY", &proxy)]);
    let mut connection = Connection::connect(&opts).await.unwrap();
    assert!(connection.invoke("Status", &[]).await.is_ok());
    assert_eq!(tunnels.load(Ordering::SeqCst), 1);
    assert_eq!(connection.close().await, Closed::Local);
}

#[tokio::test(flavor = "current_thread")]
async fn no_proxy_connects_directly() {
    let Some(base) = base() else { return };
    let (proxy, tunnels) = connect_proxy().await;
    let mut opts = options(&base, "good");
    opts.proxy = proxy_rule(&[("HTTP_PROXY", &proxy), ("NO_PROXY", "127.0.0.1,localhost")]);
    let mut connection = Connection::connect(&opts).await.unwrap();
    assert!(connection.invoke("Status", &[]).await.is_ok());
    assert_eq!(tunnels.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn unreachable_proxy_is_a_transport_error() {
    let Some(base) = base() else { return };
    // A port nothing listens on: bind, read the port, release it.
    let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = free.local_addr().unwrap();
    drop(free);
    let mut opts = options(&base, "good");
    opts.proxy = proxy_rule(&[("HTTP_PROXY", &format!("http://{address}"))]);
    match Connection::connect(&opts).await.err() {
        Some(Closed::Transport(text)) => assert!(text.contains("proxy"), "{text}"),
        other => panic!("unexpected {other:?}"),
    }
}
