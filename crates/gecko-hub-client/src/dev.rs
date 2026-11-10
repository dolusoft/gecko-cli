//! The development transport: the development hub's JSON frames over one WebSocket
//! (Node: src/transport/dev.ts, src/shared/rpc.ts, src/shared/wire.ts).
//!
//! Frames: `{ t: "invoke", id, method, args? }` from the CLI, answered by
//! `{ t: "result", id, ok: true, value }` or `{ t: "result", id, ok: false, error }`;
//! `{ t: "event", name, payload }` from the hub. The connection is usable once the hub
//! sent the `ready` event. The hub ends a connection with a close code: 4401
//! (unauthorized), 4403 (token in the query string), 4409 (busy), 4410 (revoked).
//! Localhost only, plain `ws://`, no proxy.

use std::collections::HashMap;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{self, Message};

use crate::select::is_local_host;
use crate::{CallError, Closed, HubError, HubEvent, Transport, net};

#[derive(Clone, Debug)]
pub struct DevOptions {
    /// The environment's `host[:port]`; must be localhost.
    pub host: String,
    /// Sent only in the `Authorization` header, never in the address.
    pub token: String,
    /// How long the hub may take to say `ready` (Node: 10 s).
    pub connect_timeout: Duration,
    /// How long one hub method may take to answer.
    pub invoke_timeout: Duration,
    pub user_agent: String,
}

impl DevOptions {
    pub fn new(host: impl Into<String>, token: impl Into<String>) -> DevOptions {
        DevOptions {
            host: host.into(),
            token: token.into(),
            connect_timeout: Duration::from_secs(10),
            invoke_timeout: Duration::from_secs(10),
            user_agent: concat!("gecko/", env!("CARGO_PKG_VERSION")).to_owned(),
        }
    }
}

type Socket = WebSocketStream<TcpStream>;
type Answer = Result<Box<RawValue>, HubError>;

#[derive(Debug)]
enum Command {
    Invoke {
        frame: String,
        id: u64,
        reply: oneshot::Sender<Answer>,
    },
    Close,
}

/// An open connection to the development hub.
#[derive(Debug)]
pub struct DevTransport {
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::UnboundedReceiver<HubEvent>,
    closed: watch::Receiver<Option<Closed>>,
    invoke_timeout: Duration,
    next_id: u64,
}

/// One frame from the hub; anything else is skipped, as the Node client does.
#[derive(Debug)]
enum Incoming {
    Result {
        id: u64,
        ok: bool,
        value: Option<Box<RawValue>>,
        error: Option<HubError>,
    },
    Event {
        name: String,
        payload: Option<Box<RawValue>>,
    },
}

fn parse_frame(text: &str) -> Option<Incoming> {
    // A flat struct, not a `#[serde(tag)]` enum: a tagged enum buffers its fields, and
    // `RawValue` cannot be read from that buffer.
    #[derive(Deserialize)]
    struct Raw {
        t: String,
        id: Option<u64>,
        ok: Option<bool>,
        value: Option<Box<RawValue>>,
        error: Option<HubError>,
        name: Option<String>,
        payload: Option<Box<RawValue>>,
    }
    let raw: Raw = serde_json::from_str(text).ok()?;
    match raw.t.as_str() {
        "result" => Some(Incoming::Result {
            id: raw.id?,
            ok: raw.ok?,
            value: raw.value,
            error: raw.error,
        }),
        "event" => Some(Incoming::Event {
            name: raw.name?,
            payload: raw.payload,
        }),
        _ => None,
    }
}

fn null() -> Box<RawValue> {
    RawValue::from_string("null".to_owned()).expect("null is JSON")
}

fn transport(error: impl std::fmt::Display) -> Closed {
    Closed::Transport(error.to_string())
}

/// `{ t: "invoke", id, method, args? }`.
fn invoke_frame(id: u64, method: &str, args: Option<&RawValue>) -> String {
    let mut frame = format!(
        "{{\"t\":\"invoke\",\"id\":{id},\"method\":{}",
        serde_json::to_string(method).expect("a string serializes")
    );
    if let Some(args) = args {
        frame.push_str(",\"args\":");
        frame.push_str(args.get());
    }
    frame.push('}');
    frame
}

impl DevTransport {
    /// Opens the connection and waits for the hub's `ready`.
    pub async fn connect(options: &DevOptions) -> Result<DevTransport, Closed> {
        if !is_local_host(&options.host) {
            return Err(Closed::Transport(format!(
                "the development transport connects to localhost only, not {}",
                options.host
            )));
        }
        let url = url::Url::parse(&format!("ws://{}/hub", options.host)).map_err(transport)?;
        let host = net::host_name(&url).map_err(transport)?;
        let port = url.port_or_known_default().unwrap_or(80);
        let opening = async {
            let tcp = TcpStream::connect((host.as_str(), port))
                .await
                .map_err(|error| Closed::Transport(format!("{}: {error}", options.host)))?;
            tcp.set_nodelay(true).map_err(transport)?;
            let mut request = url.as_str().into_client_request().map_err(transport)?;
            let headers = request.headers_mut();
            headers.insert(
                "Authorization",
                HeaderValue::from_str(&format!("Bearer {}", options.token)).map_err(transport)?,
            );
            headers.insert(
                "User-Agent",
                HeaderValue::from_str(&options.user_agent).map_err(transport)?,
            );
            let mut socket = match tokio_tungstenite::client_async(request, tcp).await {
                Ok((socket, _)) => socket,
                Err(tungstenite::Error::Http(response)) => {
                    return Err(match response.status().as_u16() {
                        401 | 403 => Closed::Unauthorized,
                        status => Closed::Transport(format!("HTTP {status}")),
                    });
                }
                Err(error) => return Err(transport(error)),
            };
            let early = wait_for_ready(&mut socket).await?;
            Ok((socket, early))
        };
        let (socket, early) = timeout(options.connect_timeout, opening)
            .await
            .map_err(|_| Closed::Transport("the hub did not answer in time".to_owned()))??;

        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::unbounded_channel();
        for event in early {
            let _ = event_tx.send(event);
        }
        let (closed_tx, closed) = watch::channel(None);
        tokio::spawn(drive(socket, command_rx, event_tx, closed_tx));
        Ok(DevTransport {
            commands,
            events,
            closed,
            invoke_timeout: options.invoke_timeout,
            next_id: 0,
        })
    }

    /// Calls a hub method; the answer is its raw `value`.
    pub async fn invoke(
        &mut self,
        method: &str,
        args: Option<&RawValue>,
    ) -> Result<Box<RawValue>, CallError> {
        self.next_id += 1;
        let id = self.next_id;
        let (reply, answer) = oneshot::channel();
        let frame = invoke_frame(id, method, args);
        if self
            .commands
            .send(Command::Invoke { frame, id, reply })
            .is_err()
        {
            return Err(CallError::Closed(self.reason().await));
        }
        match timeout(self.invoke_timeout, answer).await {
            Err(_) => Err(CallError::TimedOut),
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => Err(CallError::Hub(error)),
            Ok(Err(_)) => Err(CallError::Closed(self.reason().await)),
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
}

impl Transport for DevTransport {
    async fn status(&mut self) -> Result<Box<RawValue>, CallError> {
        self.invoke("status", None).await
    }

    async fn pair(&mut self) -> Result<Box<RawValue>, CallError> {
        self.invoke("pair", None).await
    }

    async fn send(&mut self, envelope: &RawValue) -> Result<Box<RawValue>, CallError> {
        let args = format!("{{\"envelope\":{}}}", envelope.get());
        let args = RawValue::from_string(args).map_err(|error| {
            CallError::Closed(Closed::Transport(format!("bad envelope: {error}")))
        })?;
        self.invoke("send", Some(&args)).await
    }

    async fn detach(&mut self) -> Result<(), CallError> {
        self.invoke("detach", None).await.map(|_| ())
    }

    async fn next_event(&mut self) -> Result<HubEvent, Closed> {
        match self.events.recv().await {
            Some(event) => Ok(event),
            None => Err(self.reason().await),
        }
    }

    async fn close(mut self) -> Closed {
        let _ = self.commands.send(Command::Close);
        self.reason().await
    }
}

/// Reads frames until `ready`; events before it are kept in order.
async fn wait_for_ready(socket: &mut Socket) -> Result<Vec<HubEvent>, Closed> {
    let mut early = Vec::new();
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => match parse_frame(text.as_str()) {
                Some(Incoming::Event { name, .. }) if name == "ready" => return Ok(early),
                Some(Incoming::Event { name, payload }) => {
                    early.push(HubEvent::from_parts(&name, payload.unwrap_or_else(null)));
                }
                _ => {}
            },
            Some(Ok(Message::Close(frame))) => return Err(close_reason(frame)),
            Some(Ok(_)) => {}
            Some(Err(error)) => return Err(transport(error)),
            None => return Err(Closed::Transport("connection ended".to_owned())),
        }
    }
}

fn close_reason(frame: Option<CloseFrame>) -> Closed {
    match frame {
        Some(frame) => Closed::from_close_code(u16::from(frame.code), frame.reason.as_str()),
        None => Closed::Transport("closed without a code".to_owned()),
    }
}

async fn drive(
    mut socket: Socket,
    mut commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<HubEvent>,
    closed: watch::Sender<Option<Closed>>,
) {
    let mut pending: HashMap<u64, oneshot::Sender<Answer>> = HashMap::new();
    let reason = loop {
        tokio::select! {
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => match parse_frame(text.as_str()) {
                    Some(Incoming::Result { id, ok, value, error }) => {
                        if let Some(reply) = pending.remove(&id) {
                            let _ = reply.send(if ok {
                                Ok(value.unwrap_or_else(null))
                            } else {
                                Err(error.unwrap_or_else(|| HubError {
                                    code: "malformed-result".to_owned(),
                                    message: Some("ok is false without error".to_owned()),
                                    reason: None,
                                    retry_after_ms: None,
                                }))
                            });
                        }
                    }
                    Some(Incoming::Event { name, payload }) => {
                        let _ = events.send(HubEvent::from_parts(&name, payload.unwrap_or_else(null)));
                    }
                    None => {}
                },
                Some(Ok(Message::Close(frame))) => break close_reason(frame),
                Some(Ok(_)) => {}
                Some(Err(error)) => break transport(error),
                None => break Closed::Transport("connection ended".to_owned()),
            },
            command = commands.recv() => match command {
                Some(Command::Invoke { frame, id, reply }) => {
                    pending.retain(|_, waiting| !waiting.is_closed());
                    pending.insert(id, reply);
                    if let Err(error) = socket.send(Message::text(frame)).await {
                        break transport(error);
                    }
                }
                Some(Command::Close) | None => {
                    let _ = socket
                        .close(Some(CloseFrame { code: CloseCode::Normal, reason: "bye".into() }))
                        .await;
                    break Closed::Local;
                }
            },
        }
    };
    // Dropping `pending` makes every waiting call return the reason.
    let _ = closed.send(Some(reason));
    drop(pending);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoke_frames() {
        assert_eq!(
            invoke_frame(1, "status", None),
            "{\"t\":\"invoke\",\"id\":1,\"method\":\"status\"}"
        );
        let args = RawValue::from_string("{\"envelope\":{\"v\":1}}".to_owned()).unwrap();
        assert_eq!(
            invoke_frame(7, "send", Some(&args)),
            "{\"t\":\"invoke\",\"id\":7,\"method\":\"send\",\"args\":{\"envelope\":{\"v\":1}}}"
        );
    }

    #[test]
    fn frames_with_raw_values_parse() {
        assert!(matches!(
            parse_frame("{\"t\":\"event\",\"name\":\"reply\",\"payload\":{\"a\":[1,2]}}"),
            Some(Incoming::Event { payload: Some(payload), .. }) if payload.get() == "{\"a\":[1,2]}"
        ));
        assert!(matches!(
            parse_frame("{\"t\":\"result\",\"id\":1,\"ok\":true,\"value\":{\"b\":true}}"),
            Some(Incoming::Result { value: Some(value), .. }) if value.get() == "{\"b\":true}"
        ));
    }

    #[test]
    fn frames_that_are_not_ours_are_skipped() {
        assert!(parse_frame("not json").is_none());
        assert!(parse_frame("{\"t\":\"invoke\",\"id\":1,\"method\":\"x\"}").is_none());
        assert!(parse_frame("{\"t\":\"result\"}").is_none());
        assert!(matches!(
            parse_frame("{\"t\":\"result\",\"id\":3,\"ok\":false,\"error\":{\"code\":\"gone\",\"reason\":\"revoked\"}}"),
            Some(Incoming::Result { id: 3, ok: false, error: Some(HubError { code, .. }), .. }) if code == "gone"
        ));
        assert!(matches!(
            parse_frame("{\"t\":\"event\",\"name\":\"ready\"}"),
            Some(Incoming::Event { name, payload: None }) if name == "ready"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn non_local_host_is_refused_before_connecting() {
        let error = DevTransport::connect(&DevOptions::new("develop.example", "t"))
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Closed::Transport(text) if text.contains("localhost only")),
            "{error:?}"
        );
    }
}
