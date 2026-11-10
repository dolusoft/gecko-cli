//! The byte stream under a WebSocket: plain TCP or TLS (rustls with the operating
//! system's verifier), directly or through an HTTP `CONNECT` tunnel. The WebSocket library
//! gets a ready stream, so HTTP and WebSocket share one TLS setup and one proxy rule.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use percent_encoding::percent_decode_str;
use rustls::pki_types::ServerName;
use rustls_platform_verifier::ConfigVerifierExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use url::{Host, Url};

use crate::proxy::ProxyRule;

/// An open connection to a server, ready for the WebSocket handshake.
#[derive(Debug)]
pub enum Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

/// Installs ring as the process crypto provider once; reqwest and the WebSocket use it.
pub fn install_crypto_provider() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // Fails only if another provider is installed already, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// `gecko/<version> (<platform>)`: how the CLI names itself to every server (§5.7).
pub fn user_agent() -> String {
    format!(
        "gecko/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// An HTTP client for requests to `target`: rustls with ring and the system verifier,
/// and the proxy the CLI's rule picks for that address (reqwest's own lookup is off, so
/// HTTP and WebSocket never disagree).
pub fn http_client(target: &Url, proxy: &ProxyRule) -> Result<reqwest::Client, String> {
    install_crypto_provider();
    let mut builder = reqwest::Client::builder()
        .user_agent(user_agent())
        .no_proxy();
    if let Some(via) = proxy.proxy_for(target)? {
        builder = builder.proxy(reqwest::Proxy::all(via.as_str()).map_err(|e| e.to_string())?);
    }
    builder.build().map_err(|error| error.to_string())
}

fn tls_config() -> io::Result<Arc<rustls::ClientConfig>> {
    install_crypto_provider();
    let mut config = rustls::ClientConfig::with_platform_verifier()
        .map_err(|e| io::Error::other(format!("TLS setup failed: {e}")))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// The host of an address as a socket and TLS name: an IPv6 address without brackets.
pub fn host_name(url: &Url) -> io::Result<String> {
    match url.host() {
        Some(Host::Domain(name)) => Ok(name.to_owned()),
        Some(Host::Ipv4(address)) => Ok(address.to_string()),
        Some(Host::Ipv6(address)) => Ok(address.to_string()),
        None => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the address has no host",
        )),
    }
}

/// Opens a stream to the host and port of `target` (`ws`, `wss`, `http` or `https`),
/// through the proxy the rule picks for it.
pub async fn open(target: &Url, proxy: &ProxyRule) -> io::Result<Stream> {
    let tls = matches!(target.scheme(), "https" | "wss");
    let host = host_name(target)?;
    let port = target
        .port_or_known_default()
        .unwrap_or(if tls { 443 } else { 80 });
    let via = proxy
        .proxy_for(target)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let tcp = match via {
        Some(proxy) => tunnel(&proxy, &host, port).await?,
        None => TcpStream::connect((host.as_str(), port)).await?,
    };
    tcp.set_nodelay(true)?;
    if !tls {
        return Ok(Stream::Plain(tcp));
    }
    let name =
        ServerName::try_from(host).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let stream = TlsConnector::from(tls_config()?).connect(name, tcp).await?;
    Ok(Stream::Tls(Box::new(stream)))
}

/// The longest proxy answer read before giving up.
const MAX_PROXY_ANSWER: usize = 8 * 1024;

/// Asks an `http://` proxy for a tunnel to `host:port` and returns the open socket.
pub async fn tunnel(proxy: &Url, host: &str, port: u16) -> io::Result<TcpStream> {
    let proxy_host = host_name(proxy)?;
    let proxy_port = proxy.port_or_known_default().unwrap_or(80);
    let mut socket = TcpStream::connect((proxy_host.as_str(), proxy_port))
        .await
        .map_err(|error| io::Error::new(error.kind(), format!("proxy {proxy_host}: {error}")))?;

    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if !proxy.username().is_empty() {
        let decode = |text: &str| percent_decode_str(text).collect::<Vec<u8>>();
        let mut credentials = decode(proxy.username());
        credentials.push(b':');
        credentials.extend(decode(proxy.password().unwrap_or("")));
        request.push_str("Proxy-Authorization: Basic ");
        request.push_str(&base64(&credentials));
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    socket.write_all(request.as_bytes()).await?;

    // The server behind the tunnel speaks only after the client, so the answer is all
    // the proxy sends; reading past its end would mean the proxy is not tunnelling.
    let mut answer = Vec::new();
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        let read = socket.read(&mut chunk).await?;
        if read == 0 {
            return Err(proxy_error("the proxy closed the connection"));
        }
        answer.extend_from_slice(&chunk[..read]);
        if let Some(end) = answer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        if answer.len() > MAX_PROXY_ANSWER {
            return Err(proxy_error("the proxy answer is too long"));
        }
    };
    let head = String::from_utf8_lossy(&answer[..head_end]);
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| proxy_error("the proxy answer is not HTTP"))?;
    match status {
        200..=299 if head_end + 4 == answer.len() => Ok(socket),
        200..=299 => Err(proxy_error("the proxy sent data after its answer")),
        407 => Err(proxy_error(
            "the proxy requires authentication (put user:password in the proxy address)",
        )),
        other => Err(proxy_error(&format!(
            "the proxy refused the tunnel to {authority} (HTTP {other})"
        ))),
    }
}

fn proxy_error(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionRefused, text.to_owned())
}

/// Standard base64 with padding, for one header.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let b = [
            group[0],
            group.get(1).copied().unwrap_or(0),
            group.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= group.len() {
                out.push(char::from(TABLE[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The longest server text that goes into an error message.
const MAX_ERROR_TEXT: usize = 200;

/// A server's answer body made fit for one error line: whitespace and control characters
/// collapse to single spaces, and the text is cut at 200 characters.
pub fn short_text(body: &str) -> String {
    let mut out = String::new();
    let mut count = 0;
    let mut space = false;
    for c in body.trim().chars() {
        if c.is_whitespace() || c.is_control() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
            count += 1;
            space = false;
        }
        if count == MAX_ERROR_TEXT {
            out.push_str("...");
            return out;
        }
        out.push(c);
        count += 1;
    }
    out
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_flush(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Stream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Stream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"user:p@ss"), "dXNlcjpwQHNz");
    }

    #[test]
    fn short_text_is_one_bounded_line() {
        assert_eq!(short_text("  a\r\n\tb \u{7}c  "), "a b c");
        let long = "x".repeat(500);
        let cut = short_text(&long);
        assert_eq!(cut.len(), MAX_ERROR_TEXT + 3);
        assert!(cut.ends_with("..."));
        assert_eq!(short_text(&"y".repeat(MAX_ERROR_TEXT)), "y".repeat(200));
    }

    /// A one-shot proxy that answers the CONNECT with `answer` and returns what it read.
    async fn fake_proxy(answer: &'static str) -> (Url, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 256];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let read = socket.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..read]);
            }
            socket.write_all(answer.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (
            Url::parse(&format!("http://user:p%40ss@{address}")).unwrap(),
            task,
        )
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tunnel_sends_connect_with_credentials() {
        let (proxy, task) = fake_proxy("HTTP/1.1 200 Connection established\r\n\r\n").await;
        tunnel(&proxy, "hub.example", 443).await.unwrap();
        let request = task.await.unwrap();
        assert!(request.starts_with("CONNECT hub.example:443 HTTP/1.1\r\n"));
        assert!(request.contains("Host: hub.example:443\r\n"));
        assert!(request.contains("Proxy-Authorization: Basic dXNlcjpwQHNz\r\n"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tunnel_brackets_ipv6_targets() {
        let (proxy, task) = fake_proxy("HTTP/1.0 200 OK\r\n\r\n").await;
        tunnel(&proxy, "::1", 5199).await.unwrap();
        assert!(task.await.unwrap().starts_with("CONNECT [::1]:5199 "));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tunnel_refusals_are_explained() {
        let (proxy, _task) = fake_proxy("HTTP/1.1 407 Proxy Authentication Required\r\n\r\n").await;
        let error = tunnel(&proxy, "hub.example", 443).await.unwrap_err();
        assert!(
            error.to_string().contains("requires authentication"),
            "{error}"
        );

        let (proxy, _task) = fake_proxy("HTTP/1.1 403 Forbidden\r\n\r\n").await;
        let error = tunnel(&proxy, "hub.example", 443).await.unwrap_err();
        assert!(error.to_string().contains("HTTP 403"), "{error}");

        let (proxy, _task) = fake_proxy("HTTP/1.1 200 OK\r\n\r\nextra").await;
        let error = tunnel(&proxy, "hub.example", 443).await.unwrap_err();
        assert!(error.to_string().contains("after its answer"), "{error}");
    }
}
