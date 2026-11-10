//! The trial scenarios (docs/rust-migration.md §7.4) against tests/signalr-hub.
//!
//! Needs a running hub with short keep-alive settings:
//!   TEST_HUB_KEEPALIVE_MS=500 TEST_HUB_CLIENT_TIMEOUT_MS=2000 SignalRTestHub --urls http://127.0.0.1:5199
//! and GECKO_SIGNALR_TEST_HUB=http://127.0.0.1:5199. Without the variable the tests are
//! skipped, except in CI, where a missing hub fails them.

use std::time::Duration;

use serde_json::value::RawValue;
use signalr_manual_trial::{hub_result, Closed, Connection, InvokeError, Options};

fn base() -> Option<String> {
    match std::env::var("GECKO_SIGNALR_TEST_HUB") {
        Ok(url) => Some(url.trim_end_matches('/').to_owned()),
        Err(_) if std::env::var_os("CI").is_some() => {
            panic!("GECKO_SIGNALR_TEST_HUB is not set in CI")
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
    options
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
    assert!(hub_result(&status)
        .unwrap()
        .get()
        .contains("\"bound\":true"));

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
