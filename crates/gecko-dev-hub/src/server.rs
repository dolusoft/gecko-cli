//! The hub's listener: plain HTTP/1.1 for the device login routes, and the WebSocket
//! upgrade on `/hub`. One request per connection (`Connection: close`); the hub serves
//! a handful of local clients, never the internet.

use std::io;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};

use crate::hub::{Body, Hub, Kind, Options, Shared, lock};

const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// The largest WebSocket message the hub reads (as the Node hub's `maxPayload`).
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// A running hub.
#[derive(Debug)]
pub struct Running {
    /// `http://127.0.0.1:<port>`
    pub url: String,
    pub port: u16,
    /// The environment a CLI signs in with: `127.0.0.1:<port>`.
    pub env: String,
    accept: JoinHandle<()>,
}

impl Running {
    /// Stops taking connections. Open connections end with the runtime.
    pub fn close(self) {
        self.accept.abort();
    }
}

/// Starts the hub on `127.0.0.1`. Returns once it listens.
pub async fn start(options: Options) -> io::Result<Running> {
    let listener = TcpListener::bind(("127.0.0.1", options.port)).await?;
    let port = listener.local_addr()?.port();
    let env = format!("127.0.0.1:{port}");
    let url = format!("http://{env}");
    let web_origin = options.web_origin.clone().unwrap_or_else(|| url.clone());
    let cors = match &options.web_origin {
        Some(origin) => format!("cors: {origin} only"),
        None => {
            "cors: loopback origins only (http://127.0.0.1:*, http://localhost:*, http://[::1]:*)"
                .to_owned()
        }
    };
    let hub = Hub::new(options, env.clone(), web_origin);
    {
        let hub = lock(&hub);
        hub.log(&format!("dev hub listening on {url}"));
        hub.log(&cors);
    }
    let accept = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let _ = stream.set_nodelay(true);
            let hub = hub.clone();
            tokio::spawn(async move {
                if let Err(error) = connection(&hub, stream).await {
                    lock(&hub).log(&format!("connection error: {error}"));
                }
            });
        }
    });
    Ok(Running {
        url,
        port,
        env,
        accept,
    })
}

/// The head of an HTTP request.
#[derive(Debug)]
struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn bearer(&self) -> Option<&str> {
        self.header("authorization")?.strip_prefix("Bearer ")
    }
}

fn parse_head(head: &str) -> Option<Request> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        })
        .collect();
    Some(Request {
        method,
        target,
        headers,
    })
}

async fn connection(hub: &Shared, mut stream: TcpStream) -> io::Result<()> {
    let mut buffer = Vec::with_capacity(1024);
    let head_end = loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        if buffer.len() > MAX_HEAD_BYTES {
            return Ok(());
        }
        let mut chunk = [0u8; 4096];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let Some(request) = parse_head(&String::from_utf8_lossy(&buffer[..head_end])) else {
        return Ok(());
    };
    let mut body = buffer.split_off(head_end + 4);
    let address = url::Url::parse("http://localhost")
        .and_then(|base| base.join(&request.target))
        .ok();
    let Some(address) = address else {
        return respond(&mut stream, 400, &[], b"", "text/plain").await;
    };

    if request
        .header("upgrade")
        .is_some_and(|u| u.eq_ignore_ascii_case("websocket"))
    {
        if address.path() != "/hub" {
            return Ok(());
        }
        return websocket(hub, stream, &request, &address).await;
    }

    // The browser pages call the hub from the web origin (CORS).
    let mut cors = Vec::new();
    if let Some(origin) = request.header("origin")
        && lock(hub).allows_origin(origin)
    {
        cors.push(("Access-Control-Allow-Origin", origin.to_owned()));
        cors.push((
            "Access-Control-Allow-Headers",
            "Authorization, Content-Type".to_owned(),
        ));
        cors.push((
            "Access-Control-Allow-Methods",
            "GET, POST, OPTIONS".to_owned(),
        ));
        cors.push(("Vary", "Origin".to_owned()));
    }
    if request.method == "OPTIONS" {
        return respond(&mut stream, 204, &cors, b"", "").await;
    }

    let length: usize = request
        .header("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    if length > MAX_BODY_BYTES {
        return respond(&mut stream, 413, &cors, b"", "").await;
    }
    while body.len() < length {
        let mut chunk = vec![0u8; length - body.len()];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);
    let fields = if request.method == "POST" {
        read_body(&body, request.header("content-type").unwrap_or(""))
    } else {
        Map::new()
    };
    let (status, answer) =
        lock(hub).http(&request.method, address.path(), request.bearer(), &fields);
    match answer {
        Body::Json(value) => {
            let mut headers = cors;
            headers.push(("Cache-Control", "no-store".to_owned()));
            respond(
                &mut stream,
                status,
                &headers,
                value.to_string().as_bytes(),
                "application/json; charset=utf-8",
            )
            .await
        }
        Body::Html(page) => {
            respond(
                &mut stream,
                status,
                &cors,
                page.as_bytes(),
                "text/html; charset=utf-8",
            )
            .await
        }
    }
}

/// A request body as fields: JSON when the type says so, a form otherwise.
fn read_body(body: &[u8], content_type: &str) -> Map<String, Value> {
    if body.is_empty() {
        return Map::new();
    }
    if content_type.contains("json") {
        return match serde_json::from_slice::<Value>(body) {
            Ok(Value::Object(fields)) => fields,
            _ => Map::new(),
        };
    }
    url::form_urlencoded::parse(body)
        .map(|(key, value)| (key.into_owned(), Value::String(value.into_owned())))
        .collect()
}

async fn respond(
    stream: &mut TcpStream,
    status: u16,
    headers: &[(&str, String)],
    body: &[u8],
    content_type: &str,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        _ => "Internal Server Error",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
    if !content_type.is_empty() {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    if status != 204 {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    let _ = stream.shutdown().await;
    Ok(())
}

async fn websocket(
    hub: &Shared,
    mut stream: TcpStream,
    request: &Request,
    address: &url::Url,
) -> io::Result<()> {
    let key = request.header("sec-websocket-key");
    let version_ok = request.header("sec-websocket-version") == Some("13");
    let (Some(key), true, "GET") = (key, version_ok, request.method.as_str()) else {
        return respond(&mut stream, 400, &[], b"", "").await;
    };
    let answer = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        derive_accept_key(key.as_bytes())
    );
    stream.write_all(answer.as_bytes()).await?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let socket = WebSocketStream::from_raw_socket(stream, Role::Server, Some(config)).await;
    let (mut sink, mut source) = socket.split();

    let query_token = address
        .query_pairs()
        .find(|(name, _)| name == "access_token")
        .map(|(_, value)| value.into_owned());
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let admitted = lock(hub).accept(request.bearer(), query_token.as_deref(), tx);
    let (id, kind) = match admitted {
        Ok(admitted) => admitted,
        Err((code, reason)) => {
            let frame = CloseFrame {
                code: CloseCode::from(code),
                reason: reason.into(),
            };
            let _ = sink.send(Message::Close(Some(frame))).await;
            // Wait for the client's close frame so it reads the code.
            let _ = tokio::time::timeout(Duration::from_secs(1), async {
                while let Some(Ok(message)) = source.next().await {
                    if message.is_close() {
                        break;
                    }
                }
            })
            .await;
            return Ok(());
        }
    };

    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let close = message.is_close();
            if sink.send(message).await.is_err() || close {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // A browser connection can stay open after its network is gone; a heartbeat tells
    // the hub, and the binding is suspended at once instead of every CLI call waiting
    // for a delivery that never comes.
    let heartbeat = (kind == Kind::Browser).then(|| {
        let hub = hub.clone();
        let millis = lock(&hub).timings().heartbeat_ms.max(1.0);
        let period = Duration::from_secs_f64(millis / 1000.0);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            loop {
                ticker.tick().await;
                if !lock(&hub).heartbeat(id) {
                    break;
                }
            }
        })
    });

    while let Some(message) = source.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let mut hub = lock(hub);
                hub.heard(id);
                if let Some((frame_id, method, args)) = invoke_frame(text.as_str()) {
                    hub.invoke(id, &frame_id, &method, &args);
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => lock(hub).heard(id),
        }
    }
    // The close frame is in: the connection no longer holds its grant.
    lock(hub).closed(id);
    if let Some(heartbeat) = heartbeat {
        heartbeat.abort();
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
    Ok(())
}

/// `{ t: 'invoke', id: number, method: string, args? }`, or nothing.
fn invoke_frame(text: &str) -> Option<(Value, String, Map<String, Value>)> {
    let Value::Object(mut frame) = serde_json::from_str::<Value>(text).ok()? else {
        return None;
    };
    if frame.get("t").and_then(Value::as_str) != Some("invoke") {
        return None;
    }
    let id = frame.remove("id").filter(Value::is_number)?;
    let method = frame.get("method").and_then(Value::as_str)?.to_owned();
    let args = match frame.remove("args") {
        Some(Value::Object(args)) => args,
        _ => Map::new(),
    };
    Some((id, method, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_are_parsed_with_lowercase_names() {
        let request =
            parse_head("POST /oauth/token HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer dev:u1")
                .unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/oauth/token");
        assert_eq!(request.bearer(), Some("dev:u1"));
        assert_eq!(request.header("host"), Some("x"));
    }

    #[test]
    fn bodies_are_json_or_forms() {
        let json = read_body(br#"{"a":1}"#, "application/json");
        assert_eq!(json.get("a"), Some(&Value::from(1)));
        let form = read_body(
            b"grant_type=refresh_token&x=a+b",
            "application/x-www-form-urlencoded",
        );
        assert_eq!(form.get("x"), Some(&Value::from("a b")));
        assert!(read_body(b"[1]", "application/json").is_empty());
        assert!(read_body(b"{", "application/json").is_empty());
    }

    #[test]
    fn invoke_frames_need_a_number_id_and_a_method() {
        let (id, method, args) =
            invoke_frame(r#"{"t":"invoke","id":3,"method":"status","args":{"a":1}}"#).unwrap();
        assert_eq!(
            (id, method.as_str(), args.len()),
            (Value::from(3), "status", 1)
        );
        assert!(invoke_frame(r#"{"t":"invoke","id":"3","method":"status"}"#).is_none());
        assert!(invoke_frame(r#"{"t":"result","id":3,"method":"status"}"#).is_none());
        let (_, _, args) =
            invoke_frame(r#"{"t":"invoke","id":1,"method":"m","args":[1]}"#).unwrap();
        assert!(args.is_empty());
    }
}
