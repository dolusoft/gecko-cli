//! One short SignalR connection: connect, invoke hub methods, receive server events, close.
//! A connection that ends is never reopened here; a command is never sent twice.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{sleep_until, timeout, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::WebSocketStream;
use url::Url;

use crate::protocol::{self, Closed, Incoming, RecordReader};
use crate::stream::{self, Stream};

#[derive(Clone, Debug)]
pub struct Options {
    /// The hub address, `http(s)://host[:port]/path`.
    pub url: String,
    /// Sent only in the `Authorization` header, never in the address.
    pub token: String,
    pub skip_negotiation: bool,
    /// Send a Ping when nothing else was sent for this long (SignalR default 15 s).
    pub keep_alive: Duration,
    /// Close when nothing arrived from the server for this long (SignalR default 30 s).
    pub server_timeout: Duration,
    pub handshake_timeout: Duration,
    pub user_agent: String,
    /// The framed handshake request; only tests change it.
    #[doc(hidden)]
    pub handshake: String,
}

impl Options {
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            token: token.into(),
            skip_negotiation: true,
            keep_alive: Duration::from_secs(15),
            server_timeout: Duration::from_secs(30),
            handshake_timeout: Duration::from_secs(15),
            user_agent: concat!("gecko-signalr-trial/", env!("CARGO_PKG_VERSION")).to_owned(),
            handshake: protocol::HANDSHAKE_REQUEST.to_owned(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InvokeError {
    /// The connection ended before the Completion arrived.
    Closed(Closed),
    /// A Completion with `error`.
    Failed(String),
}

/// A server-to-client invocation that needs no answer (`Reply`, `binding`, ...).
#[derive(Debug)]
pub struct Event {
    pub target: String,
    pub arguments: Vec<Box<RawValue>>,
}

enum Command {
    Invoke {
        frame: String,
        id: String,
        reply: oneshot::Sender<Result<Option<Box<RawValue>>, String>>,
    },
    Close,
}

pub struct Connection {
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::UnboundedReceiver<Event>,
    closed: watch::Receiver<Option<Closed>>,
    next_id: u64,
}

type Socket = WebSocketStream<Stream>;

impl Connection {
    pub async fn connect(options: &Options) -> Result<Connection, Closed> {
        let (ws_url, token) = if options.skip_negotiation {
            (
                Url::parse(&options.url).map_err(transport)?,
                options.token.clone(),
            )
        } else {
            negotiate(options).await?
        };
        let mut socket = open_socket(&ws_url, &token, &options.user_agent).await?;

        socket
            .send(Message::text(options.handshake.clone()))
            .await
            .map_err(transport)?;
        let mut reader = RecordReader::default();
        let handshake = timeout(
            options.handshake_timeout,
            first_record(&mut socket, &mut reader),
        )
        .await
        .map_err(|_| Closed::Transport("handshake timed out".to_owned()))??;
        protocol::parse_handshake(&handshake).map_err(Closed::Handshake)?;

        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        let (closed_tx, closed) = watch::channel(None);
        tokio::spawn(drive(
            socket,
            reader,
            options.clone(),
            command_rx,
            event_tx,
            closed_tx,
        ));
        Ok(Connection {
            commands,
            events,
            closed,
            next_id: 0,
        })
    }

    /// Calls a hub method and waits for its Completion. The result is the raw `result`.
    pub async fn invoke(
        &mut self,
        target: &str,
        arguments: &[&RawValue],
    ) -> Result<Box<RawValue>, InvokeError> {
        self.next_id += 1;
        let id = self.next_id.to_string();
        let (reply, answer) = oneshot::channel();
        let frame = protocol::invocation(&id, target, arguments);
        if self
            .commands
            .send(Command::Invoke { frame, id, reply })
            .is_err()
        {
            return Err(InvokeError::Closed(self.reason().await));
        }
        match answer.await {
            Ok(Ok(result)) => {
                Ok(result
                    .unwrap_or_else(|| RawValue::from_string("null".to_owned()).expect("null")))
            }
            Ok(Err(error)) => Err(InvokeError::Failed(error)),
            Err(_) => Err(InvokeError::Closed(self.reason().await)),
        }
    }

    /// The next server event, or why the connection ended.
    pub async fn next_event(&mut self) -> Result<Event, Closed> {
        match self.events.recv().await {
            Some(event) => Ok(event),
            None => Err(self.reason().await),
        }
    }

    /// Resolves when the connection has ended, with the reason.
    pub async fn reason(&mut self) -> Closed {
        loop {
            if let Some(reason) = self.closed.borrow().clone() {
                return reason;
            }
            if self.closed.changed().await.is_err() {
                return self.closed.borrow().clone().unwrap_or(Closed::Local);
            }
        }
    }

    pub async fn close(mut self) -> Closed {
        let _ = self.commands.send(Command::Close);
        self.reason().await
    }
}

fn transport(error: impl std::fmt::Display) -> Closed {
    Closed::Transport(error.to_string())
}

fn status_reason(status: u16, body: &str) -> Closed {
    match status {
        401 | 403 => Closed::Unauthorized,
        _ => Closed::Transport(format!("HTTP {status}: {}", body.trim())),
    }
}

/// POST `<url>/negotiate`, following Azure-style redirects (`url` + `accessToken`).
async fn negotiate(options: &Options) -> Result<(Url, String), Closed> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Response {
        url: Option<String>,
        access_token: Option<String>,
        error: Option<String>,
        connection_token: Option<String>,
        connection_id: Option<String>,
        #[serde(default)]
        available_transports: Vec<Transport>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Transport {
        transport: String,
        #[serde(default)]
        transfer_formats: Vec<String>,
    }

    stream::install_crypto_provider();
    let client = reqwest::Client::builder()
        .user_agent(options.user_agent.clone())
        .build()
        .map_err(transport)?;
    let mut url = Url::parse(&options.url).map_err(transport)?;
    let mut token = options.token.clone();

    for _ in 0..5 {
        let mut negotiate_url = url.clone();
        negotiate_url.set_path(&format!("{}/negotiate", url.path().trim_end_matches('/')));
        negotiate_url
            .query_pairs_mut()
            .append_pair("negotiateVersion", "1");
        let response = client
            .post(negotiate_url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(transport)?;
        let status = response.status().as_u16();
        let body = response.text().await.map_err(transport)?;
        if !(200..300).contains(&status) {
            return Err(status_reason(status, &body));
        }
        let parsed: Response = serde_json::from_str(&body).map_err(transport)?;
        if let Some(error) = parsed.error {
            return Err(Closed::Server(Some(error)));
        }
        if let Some(next) = parsed.url {
            url = Url::parse(&next).map_err(transport)?;
            if let Some(next_token) = parsed.access_token {
                token = next_token;
            }
            continue;
        }
        let websockets = parsed
            .available_transports
            .iter()
            .any(|t| t.transport == "WebSockets" && t.transfer_formats.iter().any(|f| f == "Text"));
        if !websockets {
            return Err(Closed::Transport(
                "the server offers no WebSockets text transport".to_owned(),
            ));
        }
        let id = parsed
            .connection_token
            .or(parsed.connection_id)
            .ok_or_else(|| {
                Closed::Transport("negotiate answer without a connection id".to_owned())
            })?;
        url.query_pairs_mut().append_pair("id", &id);
        return Ok((url, token));
    }
    Err(Closed::Transport("too many negotiate redirects".to_owned()))
}

async fn open_socket(url: &Url, token: &str, user_agent: &str) -> Result<Socket, Closed> {
    let tls = match url.scheme() {
        "https" | "wss" => true,
        "http" | "ws" => false,
        other => return Err(Closed::Transport(format!("unsupported scheme {other}"))),
    };
    let mut ws_url = url.clone();
    ws_url
        .set_scheme(if tls { "wss" } else { "ws" })
        .map_err(|_| Closed::Transport("cannot build the WebSocket address".to_owned()))?;
    let host = ws_url
        .host_str()
        .ok_or_else(|| Closed::Transport("address without host".to_owned()))?;
    let port = ws_url
        .port_or_known_default()
        .unwrap_or(if tls { 443 } else { 80 });

    let stream = stream::open(host, port, tls).await.map_err(transport)?;
    let mut request = ws_url.as_str().into_client_request().map_err(transport)?;
    let headers = request.headers_mut();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(transport)?,
    );
    headers.insert(
        "User-Agent",
        HeaderValue::from_str(user_agent).map_err(transport)?,
    );

    match tokio_tungstenite::client_async(request, stream).await {
        Ok((socket, _)) => Ok(socket),
        Err(tungstenite::Error::Http(response)) => {
            let status = response.status();
            let body = response
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default();
            Err(status_reason(status.as_u16(), &body))
        }
        Err(error) => Err(transport(error)),
    }
}

async fn first_record(socket: &mut Socket, reader: &mut RecordReader) -> Result<String, Closed> {
    loop {
        if let Some(record) = reader.next_record() {
            return Ok(record);
        }
        match socket.next().await {
            Some(Ok(Message::Text(text))) => reader.push(text.as_str()),
            Some(Ok(Message::Close(_))) | None => {
                return Err(Closed::Transport("closed during the handshake".to_owned()))
            }
            Some(Ok(_)) => {}
            Some(Err(error)) => return Err(transport(error)),
        }
    }
}

type Pending = HashMap<String, oneshot::Sender<Result<Option<Box<RawValue>>, String>>>;

async fn drive(
    mut socket: Socket,
    mut reader: RecordReader,
    options: Options,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<Event>,
    closed: watch::Sender<Option<Closed>>,
) {
    let mut pending = Pending::new();
    let mut ping_at = Instant::now() + options.keep_alive;
    let mut server_deadline = Instant::now() + options.server_timeout;

    // Records that arrived in the same frame as the handshake answer.
    let mut reason = handle_records(&mut socket, &mut reader, &mut pending, &events).await;

    while reason.is_none() {
        tokio::select! {
            message = socket.next() => {
                server_deadline = Instant::now() + options.server_timeout;
                reason = match message {
                    Some(Ok(Message::Text(text))) => {
                        reader.push(text.as_str());
                        handle_records(&mut socket, &mut reader, &mut pending, &events).await
                    }
                    Some(Ok(Message::Close(frame))) => Some(Closed::Transport(format!(
                        "closed without a Close message ({})",
                        frame.map(|f| f.code.to_string()).unwrap_or_else(|| "no code".to_owned())
                    ))),
                    Some(Ok(_)) => None,
                    Some(Err(error)) => Some(transport(error)),
                    None => Some(Closed::Transport("connection ended".to_owned())),
                };
            }
            command = commands.recv() => {
                reason = match command {
                    Some(Command::Invoke { frame, id, reply }) => {
                        pending.insert(id, reply);
                        ping_at = Instant::now() + options.keep_alive;
                        socket.send(Message::text(frame)).await.err().map(transport)
                    }
                    Some(Command::Close) | None => {
                        let _ = socket.send(Message::text(protocol::CLOSE)).await;
                        let _ = socket.close(None).await;
                        Some(Closed::Local)
                    }
                };
            }
            _ = sleep_until(ping_at) => {
                ping_at = Instant::now() + options.keep_alive;
                reason = socket.send(Message::text(protocol::PING)).await.err().map(transport);
            }
            _ = sleep_until(server_deadline) => {
                reason = Some(Closed::ServerTimeout);
            }
        }
    }

    // Dropping `pending` makes every waiting invoke return the reason.
    let _ = closed.send(reason);
    drop(pending);
}

async fn handle_records(
    socket: &mut Socket,
    reader: &mut RecordReader,
    pending: &mut Pending,
    events: &mpsc::UnboundedSender<Event>,
) -> Option<Closed> {
    while let Some(record) = reader.next_record() {
        let message = match protocol::parse_message(&record) {
            Ok(message) => message,
            Err(error) => return Some(Closed::Transport(error)),
        };
        match message {
            Incoming::Completion {
                invocation_id,
                result,
                error,
            } => {
                if let Some(reply) = pending.remove(&invocation_id) {
                    let _ = reply.send(match error {
                        Some(error) => Err(error),
                        None => Ok(result),
                    });
                }
            }
            Incoming::Invocation {
                invocation_id: Some(id),
                ..
            } => {
                // The server waits for a client result; the CLI offers none.
                if let Err(error) = socket
                    .send(Message::text(protocol::no_result_completion(&id)))
                    .await
                {
                    return Some(transport(error));
                }
            }
            Incoming::Invocation {
                invocation_id: None,
                target,
                arguments,
            } => {
                let _ = events.send(Event { target, arguments });
            }
            Incoming::Ping | Incoming::Ignored(_) => {}
            Incoming::Close { error, .. } => {
                // allowReconnect is read and ignored: the CLI never reconnects by itself.
                let _ = socket.close(None).await;
                return Some(protocol::close_reason(error.as_deref()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_status_reasons() {
        assert_eq!(status_reason(401, ""), Closed::Unauthorized);
        assert_eq!(status_reason(403, ""), Closed::Unauthorized);
        assert!(matches!(status_reason(500, "x"), Closed::Transport(_)));
    }
}
