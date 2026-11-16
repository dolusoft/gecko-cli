use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// Shared data, not source: the TLS fixtures also drive the process tests.
fn certificate(expired: bool) -> CertificateDer<'static> {
    CertificateDer::from(if expired {
        include_bytes!("../../../../testkit/fixtures/tls/expired.der").as_slice()
    } else {
        include_bytes!("../../../../testkit/fixtures/tls/first.der").as_slice()
    })
}

fn trusted_system() -> Arc<dyn ServerCertVerifier> {
    Arc::new(
        rustls_platform_verifier::Verifier::new_with_extra_roots(
            [certificate(false)],
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .expect("test root"),
    )
}

#[test]
fn tls_pin_explicitly_replaces_name_and_time_validation_only_for_exact_leaf() {
    crate::net::install_crypto_provider();
    let cert = certificate(false);
    let trust = TlsTrust::new("https://wrong.invalid", Some(fingerprint(&cert))).expect("trust");
    let system = trusted_system();
    let verifier = PinnedVerifier {
        system: system.clone(),
        trust,
    };
    let wrong_name = ServerName::try_from("wrong.invalid").expect("name");
    let now = UnixTime::now();
    assert!(
        system
            .verify_server_cert(&cert, &[], &wrong_name, &[], now)
            .is_err()
    );
    assert!(
        verifier
            .verify_server_cert(&cert, &[], &wrong_name, &[], now)
            .is_ok()
    );
    let expired = certificate(true);
    let verifier = PinnedVerifier {
        system: Arc::new(
            rustls_platform_verifier::Verifier::new_with_extra_roots(
                [expired.clone()],
                Arc::new(rustls::crypto::ring::default_provider()),
            )
            .expect("expired test root"),
        ),
        trust: TlsTrust::new("https://localhost", Some(fingerprint(&expired))).expect("trust"),
    };
    let localhost = ServerName::try_from("localhost").expect("name");
    assert!(
        verifier
            .system
            .verify_server_cert(&expired, &[], &localhost, &[], now)
            .is_err()
    );
    assert!(
        verifier
            .verify_server_cert(&expired, &[], &localhost, &[], now)
            .is_ok()
    );
    assert!(
        verifier
            .verify_server_cert(&cert, &[], &wrong_name, &[], now)
            .is_err()
    );
}

async fn local_server(ws: bool) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let url = format!("https://{}", listener.local_addr().expect("address"));
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(
        include_bytes!("../../../../testkit/fixtures/tls/first-key.der").to_vec(),
    );
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![certificate(false)], key.into())
        .expect("server config");
    let task = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let (tcp, _) = listener.accept().await.expect("accept");
            let mut tls = tokio_rustls::TlsAcceptor::from(Arc::new(config))
                .accept(tcp)
                .await
                .expect("TLS");
            if ws {
                let mut socket = tokio_tungstenite::accept_async(tls).await.expect("WS");
                socket.close(None).await.expect("close");
            } else {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    tls.read_exact(&mut byte).await.expect("HTTP");
                    head.push(byte[0]);
                }
                tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await
                    .expect("response");
            }
        })
        .await
        .expect("bounded server");
    });
    (url, task)
}

#[tokio::test]
async fn tls_system_trusted_certificate_still_passes_http_and_ws_with_a_stale_pin() {
    crate::net::install_crypto_provider();
    for ws in [false, true] {
        let (url, task) = local_server(ws).await;
        let trust = TlsTrust::new(&url, Some("stale pin".to_owned())).expect("trust");
        let verifier = Arc::new(PinnedVerifier {
            system: trusted_system(),
            trust,
        });
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        if ws {
            let target = Url::parse(&url).expect("URL");
            let tcp = tokio::net::TcpStream::connect(format!(
                "127.0.0.1:{}",
                target.port().expect("port")
            ))
            .await
            .expect("TCP");
            let tls = tokio_rustls::TlsConnector::from(Arc::new(config))
                .connect(ServerName::try_from("127.0.0.1").expect("name"), tcp)
                .await
                .expect("system TLS");
            let (mut socket, _) =
                tokio_tungstenite::client_async(url.replace("https://", "wss://"), tls)
                    .await
                    .expect("system WS");
            socket.close(None).await.expect("close");
        } else {
            reqwest::Client::builder()
                .no_proxy()
                .tls_backend_preconfigured(config)
                .build()
                .expect("client")
                .post(url)
                .send()
                .await
                .expect("system HTTP");
        }
        task.await.expect("server task");
    }
}

#[tokio::test]
async fn tls12_pin_keeps_handshake_signature_verification() {
    crate::net::install_crypto_provider();
    let (url, task) = local_server(false).await;
    let trust = TlsTrust::new(&url, Some(fingerprint(&certificate(false)))).expect("trust");
    let system = Arc::new(
        rustls_platform_verifier::Verifier::new(Arc::new(rustls::crypto::ring::default_provider()))
            .expect("system"),
    );
    let config = rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier { system, trust }))
        .with_no_client_auth();
    reqwest::Client::builder()
        .no_proxy()
        .tls_backend_preconfigured(config)
        .build()
        .expect("client")
        .post(url)
        .send()
        .await
        .expect("pinned TLS 1.2");
    task.await.expect("server task");
}

#[test]
fn tls_pin_is_scoped_to_origin_including_port_and_scheme() {
    let trust = TlsTrust::new("https://hub.example:8443", Some("pin".to_owned())).expect("trust");
    for (url, expected) in [
        ("https://hub.example:8443/api", true),
        ("wss://hub.example:8443/relay", true),
        ("https://hub.example", false),
        ("https://other.example:8443", false),
        ("http://hub.example:8443", false),
    ] {
        assert_eq!(
            trust.matches(&Url::parse(url).expect("URL")),
            expected,
            "{url}"
        );
    }
}

#[tokio::test]
async fn tls_pin_rejects_a_peer_without_the_certificates_private_key_in_both_versions() {
    #[derive(Debug)]
    struct WrongKey(Arc<rustls::sign::CertifiedKey>);
    impl rustls::server::ResolvesServerCert for WrongKey {
        fn resolve(
            &self,
            _: rustls::server::ClientHello<'_>,
        ) -> Option<Arc<rustls::sign::CertifiedKey>> {
            Some(self.0.clone())
        }
    }

    crate::net::install_crypto_provider();
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(
            include_bytes!("../../../../testkit/fixtures/tls/rotated-key.der").to_vec(),
        );
        let signing =
            rustls::crypto::ring::sign::any_supported_type(&key.into()).expect("signing key");
        // A malicious server sends the pinned certificate but signs with another key.
        let identity = rustls::sign::CertifiedKey::new(vec![certificate(false)], signing);
        let server = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(WrongKey(Arc::new(identity))));
        let task = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                let (tcp, _) = listener.accept().await.expect("accept");
                assert!(
                    tokio_rustls::TlsAcceptor::from(Arc::new(server))
                        .accept(tcp)
                        .await
                        .is_err()
                );
            })
            .await
            .expect("bounded handshake");
        });
        let trust = TlsTrust::new(
            &format!("https://{address}"),
            Some(fingerprint(&certificate(false))),
        )
        .expect("trust");
        let system = Arc::new(
            rustls_platform_verifier::Verifier::new(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .expect("system"),
        );
        let client = rustls::ClientConfig::builder_with_protocol_versions(&[version])
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedVerifier {
                system,
                trust: trust.clone(),
            }))
            .with_no_client_auth();
        let tcp = tokio::net::TcpStream::connect(address).await.expect("TCP");
        let result = tokio_rustls::TlsConnector::from(Arc::new(client))
            .connect(ServerName::try_from("127.0.0.1").expect("name"), tcp)
            .await;
        let error = result.expect_err("a pin never replaces proof of key possession");
        assert!(
            error.to_string().contains("BadSignature"),
            "{version:?}: {error}"
        );
        assert!(
            trust.rejection().is_none(),
            "the leaf itself matched the pin"
        );
        task.await.expect("server task");
    }
}
