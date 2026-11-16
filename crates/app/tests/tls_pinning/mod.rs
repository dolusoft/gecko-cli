//! The process-level approval and migration flow, over real HTTPS and WSS.

use gecko_testkit::{Cli, TlsHub};
use serde_json::{Value, json};

#[tokio::test]
async fn tls_signalr_negotiation_and_origin_scope_use_the_same_pin() {
    use gecko_hub_client::{
        ProxyRule, SignalrOptions, SignalrTransport, TlsTrust, Transport, certificate_fingerprint,
        http_client_with_trust,
    };
    let hub = TlsHub::start("first").await;
    let url = url::Url::parse(&hub.url).expect("URL");
    let pin = certificate_fingerprint(&url, &ProxyRule::default())
        .await
        .expect("discovery");
    let trust = TlsTrust::new(&hub.url, pin.clone()).expect("trust");
    let mut options = SignalrOptions::new(&hub.url, "test-token");
    options.skip_negotiation = false;
    options.proxy = ProxyRule::default();
    options.tls_trust = Some(trust);
    let mut ws = SignalrTransport::connect(&options)
        .await
        .expect("pinned negotiate and WS");
    ws.status().await.expect("status");
    ws.close().await;
    assert_eq!(hub.requests().len(), 2);
    let other_origin = TlsTrust::new("https://other.invalid", pin).expect("trust");
    assert!(
        http_client_with_trust(&url, &ProxyRule::default(), &other_origin)
            .expect("client")
            .post(url)
            .send()
            .await
            .is_err()
    );
    options.tls_trust = Some(other_origin);
    assert!(SignalrTransport::connect(&options).await.is_err());
    assert_eq!(
        hub.requests().len(),
        2,
        "a pin must never transfer to another origin"
    );
}

fn settings(cli: &Cli) -> Value {
    serde_json::from_str(&std::fs::read_to_string(cli.dir().join("config.json")).expect("settings"))
        .expect("JSON")
}

async fn status(cli: &Cli) -> gecko_testkit::Output {
    cli.call(["status"])
        .env("NO_PROXY", "*")
        .env("no_proxy", "*")
        .output()
        .await
}

#[tokio::test]
async fn tls_first_approval_http_ws_rotation_and_reapproval() {
    let hub = TlsHub::start("first").await;
    let cli = Cli::new(super::GECKO);
    let code = hub.code();
    let refused = cli.run(["attach", &code]).await;
    refused.expect_code(3);
    assert!(refused.stderr.contains("untrusted-hub"), "{refused}");
    assert!(refused.stderr.contains("Certificate SHA-256:"), "{refused}");
    assert!(
        hub.requests().is_empty(),
        "discovery must send no HTTP or credentials"
    );
    let declined = cli
        .call(["attach", &code])
        .tty()
        .stdin("no\n")
        .output()
        .await;
    declined.expect_code(3);
    assert!(!cli.dir().join("config.json").exists());
    let attached = cli
        .call(["attach", &code])
        .tty()
        .stdin("yes\n")
        .output()
        .await;
    attached.expect_code(0);
    assert!(attached.stderr.contains(&hub.url), "{attached}");
    assert!(
        attached.stderr.contains("Certificate SHA-256:"),
        "{attached}"
    );
    let saved = settings(&cli);
    assert_eq!(saved["trustedHubs"][0]["hub"], hub.url);
    let old_pin = saved["trustedHubs"][0]["fingerprint"]
        .as_str()
        .expect("fingerprint");
    assert_eq!(old_pin.len(), 95);
    status(&cli).await.expect_code(0);
    assert!(hub.requests().iter().any(|r| r == "WebSocket"));

    hub.rotate("rotated");
    let before = hub.requests().len();
    let refused = status(&cli).await;
    refused.expect_code(3);
    assert!(refused.stderr.contains("untrusted-hub"), "{refused}");
    assert!(refused.stderr.contains("SHA-256:"), "{refused}");
    assert!(
        !refused.stderr.contains(old_pin),
        "new fingerprint must be shown"
    );
    assert_eq!(
        hub.requests().len(),
        before,
        "no bearer token after rotation"
    );
    assert_eq!(settings(&cli), saved, "a refusal cannot rewrite trust");

    // A refresh uses HTTP too, and must reject before sending the refresh token.
    let mut token = cli.token().expect("token");
    token["accessExpiresAt"] = json!(0);
    std::fs::write(cli.dir().join("token.json"), token.to_string()).expect("expired token");
    let refused = status(&cli).await;
    refused.expect_code(3);
    assert!(refused.stderr.contains("untrusted-hub"), "{refused}");
    assert_eq!(hub.requests().len(), before);
    let refused = cli.run(["attach", &code]).await;
    refused.expect_code(3);
    assert!(refused.stderr.contains("Certificate SHA-256:"), "{refused}");
    cli.call(["attach", &code])
        .tty()
        .stdin("y\n")
        .output()
        .await
        .expect_code(0);
    assert_ne!(settings(&cli)["trustedHubs"][0]["fingerprint"], old_pin);
    status(&cli).await.expect_code(0);
}

#[tokio::test]
async fn tls_legacy_record_is_read_but_requires_certificate_approval() {
    let hub = TlsHub::start("first").await;
    let cli = Cli::new(super::GECKO);
    let code = hub.code();
    cli.run(["attach", "--trust-hub", &code])
        .await
        .expect_code(0);
    std::fs::write(
        cli.dir().join("config.json"),
        json!({"trustedHubs":[hub.url, "http://127.0.0.1:4400"]}).to_string(),
    )
    .expect("legacy settings");
    let before = hub.requests().len();
    let refused = status(&cli).await;
    refused.expect_code(3);
    assert!(
        refused.stderr.contains("no approved certificate"),
        "{refused}"
    );
    assert!(refused.stderr.contains("SHA-256:"), "{refused}");
    assert_eq!(hub.requests().len(), before);
    cli.run(["attach", &code]).await.expect_code(3);
    let approved = cli
        .call(["attach", &code])
        .tty()
        .stdin("yes\n")
        .output()
        .await;
    approved.expect_code(0);
    assert!(
        approved.stderr.contains("Certificate SHA-256:"),
        "{approved}"
    );
    let saved = settings(&cli);
    assert_eq!(saved["trustedHubs"].as_array().expect("records").len(), 2);
    assert!(
        saved["trustedHubs"]
            .as_array()
            .expect("records")
            .iter()
            .any(|r| r == "http://127.0.0.1:4400")
    );
    status(&cli).await.expect_code(0);
}
