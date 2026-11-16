//! A local TLS token endpoint and small SignalR peer. Fixture keys are public test
//! material, never credentials. Every connection and task is owned by the test.

use std::sync::{Arc, Mutex, RwLock};

use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufStream};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::Message;

/// The public DER fixture for a named local test identity.
pub fn test_certificate(name: &str) -> CertificateDer<'static> {
    CertificateDer::from(match name {
        "first" => include_bytes!("../fixtures/tls/first.der").as_slice(),
        "rotated" => include_bytes!("../fixtures/tls/rotated.der").as_slice(),
        "expired" => include_bytes!("../fixtures/tls/expired.der").as_slice(),
        _ => panic!("unknown test certificate"),
    })
}

fn server_config(name: &str) -> Arc<rustls::ServerConfig> {
    let key = match name {
        "first" => include_bytes!("../fixtures/tls/first-key.der").as_slice(),
        "rotated" => include_bytes!("../fixtures/tls/rotated-key.der").as_slice(),
        "expired" => include_bytes!("../fixtures/tls/expired-key.der").as_slice(),
        _ => panic!("unknown test key"),
    };
    Arc::new(
        rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![test_certificate(name)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.to_vec())),
        )
        .expect("test identity"),
    )
}

#[derive(Debug)]
pub struct TlsHub {
    pub url: String,
    config: Arc<RwLock<Arc<rustls::ServerConfig>>>,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl TlsHub {
    pub async fn start(name: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("TLS listener");
        let url = format!("https://{}", listener.local_addr().expect("local address"));
        let config = Arc::new(RwLock::new(server_config(name)));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let configs = config.clone();
        let seen = requests.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    incoming = listener.accept() => {
                        let (tcp, _) = incoming.expect("accept");
                        let acceptor = TlsAcceptor::from(configs.read().expect("config lock").clone());
                        let seen = seen.clone();
                        connections.spawn(async move {
                            let _ = tokio::time::timeout(crate::WAIT, async {
                                let Ok(stream) = acceptor.accept(tcp).await else { return; };
                                let mut stream = BufStream::new(stream);
                                let Ok(first) = stream.fill_buf().await else { return; };
                                if first.is_empty() { return; }
                                if first.starts_with(b"POST") {
                                    let mut request = Vec::new();
                                    let mut byte = [0u8; 1];
                                    while !request.ends_with(b"\r\n\r\n") {
                                        if stream.read_exact(&mut byte).await.is_err() { return; }
                                        request.push(byte[0]);
                                        assert!(request.len() < 16_384, "bounded request headers");
                                    }
                                    let head = String::from_utf8(request).expect("HTTP headers");
                                    seen.lock().expect("request lock").push(head.clone());
                                    let length = head.lines().find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().expect("length"))).unwrap_or(0);
                                    assert!(length < 16_384);
                                    let mut body = vec![0; length];
                                    if stream.read_exact(&mut body).await.is_err() { return; }
                                    let body = if head.lines().next().expect("request line").contains("/negotiate?") {
                                        json!({"connectionToken":"test-connection","availableTransports":[{"transport":"WebSockets","transferFormats":["Text"]}]}).to_string()
                                    } else { json!({
                                        "access_token":"test-access", "refresh_token":"test-refresh", "expires_in":3600,
                                        "grantId":"g", "bindingId":"b", "level":"ask-server",
                                        "user":{"id":"u","name":"test"}
                                    }).to_string() };
                                    let answer = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                                    let _ = stream.write_all(answer.as_bytes()).await;
                                    let _ = stream.flush().await;
                                } else {
                                    let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return; };
                                    seen.lock().expect("request lock").push("WebSocket".to_owned());
                                    if ws.next().await.is_none() { return; }
                                    if ws.send(Message::text("{}\u{1e}")).await.is_err() { return; }
                                    while let Some(Ok(Message::Text(text))) = ws.next().await {
                                        for record in text.split('\u{1e}').filter(|s| !s.is_empty()) {
                                            let message: Value = serde_json::from_str(record).expect("SignalR record");
                                            if let Some(id) = message.get("invocationId") {
                                                let answer = json!({"type":3,"invocationId":id,"result":{"ok":true,"value":{"user":{"id":"u","name":"test"},"grantId":"g","binding":null}}});
                                                if ws.send(Message::text(format!("{answer}\u{1e}"))).await.is_err() { return; }
                                            }
                                        }
                                    }
                                }
                            }).await;
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        Self {
            url,
            config,
            requests,
            task,
        }
    }

    /// Replaces the certificate on the same socket address, without changing the pin.
    pub fn rotate(&self, name: &str) {
        *self.config.write().expect("config lock") = server_config(name);
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("request lock").clone()
    }

    pub fn code(&self) -> String {
        let body = json!({"v":1,"hub":self.url,"s":"test-code"}).to_string();
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut encoded = String::from("gk1_");
        let mut bits = 0u32;
        let mut count = 0;
        for byte in body.bytes() {
            bits = (bits << 8) | u32::from(byte);
            count += 8;
            while count >= 6 {
                count -= 6;
                encoded.push(char::from(TABLE[((bits >> count) & 63) as usize]));
            }
        }
        if count > 0 {
            encoded.push(char::from(TABLE[((bits << (6 - count)) & 63) as usize]));
        }
        encoded
    }
}

impl Drop for TlsHub {
    fn drop(&mut self) {
        // Aborting the listener drops its JoinSet, aborting all open connections too.
        self.task.abort();
    }
}
