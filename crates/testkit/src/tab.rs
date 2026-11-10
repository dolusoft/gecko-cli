//! A fake Gecko tab on the hub's WebSocket (`/hub`, the browser side).
//!
//! The tab reads its socket only inside its own calls. Between calls it is silent: the
//! hub's heartbeat pings stay unread and unanswered (tungstenite answers a ping only
//! while the socket is read), so a test cuts the heartbeat by not calling the tab, and
//! [`Tab::heartbeat`] answers the pings that came meanwhile.

use std::collections::VecDeque;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

use crate::envelope::Envelope;
use crate::{Hub, WAIT, capabilities};

/// A request the CLI sent, as the tab got it (`cli-message`).
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: String,
    /// `cli-request`, `help-request` or `cancel`.
    pub kind: String,
    pub argv: Option<Vec<String>>,
    /// The whole message, with the hub's stamp (`origin`, `stampedAt`, `expiresAt`).
    pub message: Value,
}

/// A fake tab of one user, on one page.
#[derive(Debug)]
pub struct Tab {
    socket: WebSocketStream<TcpStream>,
    next: u64,
    /// Events read while waiting for something else, oldest first.
    events: VecDeque<(String, Value)>,
    page: String,
    place: Value,
}

impl Tab {
    /// A tab of user `u1` on page `page`, not bound.
    pub async fn open(hub: &Hub, page: &str) -> Tab {
        Tab::open_as(hub, "u1", page, None).await
    }

    /// A reloaded page that takes its binding back: `hello` with `resume`, then the
    /// command list and the place.
    pub async fn resume(hub: &Hub, page: &str, binding_id: &str) -> Tab {
        Tab::open_as(hub, "u1", page, Some(binding_id)).await
    }

    /// A tab of `user` (its session is `dev:<user>`).
    pub async fn open_as(hub: &Hub, user: &str, page: &str, resume: Option<&str>) -> Tab {
        let opening = async {
            let request = format!("ws://{}/hub?access_token=dev:{user}", hub.env)
                .into_client_request()
                .expect("a WebSocket request");
            let stream = TcpStream::connect(("127.0.0.1", hub.port))
                .await
                .expect("the hub accepts a connection");
            client_async(request, stream)
                .await
                .expect("the hub upgrades the tab's connection")
                .0
        };
        let socket = tokio::time::timeout(WAIT, opening)
            .await
            .unwrap_or_else(|_| panic!("the tab's connection did not open within {WAIT:?}"));
        let mut tab = Tab {
            socket,
            next: 0,
            events: VecDeque::new(),
            page: page.to_owned(),
            place: json!({ "title": "Tasks", "path": "/tasks" }),
        };
        let ready = tab.saw("ready").await;
        assert_eq!(ready["kind"], "browser", "{ready}");
        let mut hello = json!({ "pageId": page });
        if let Some(binding) = resume {
            hello["resume"] = json!(binding);
        }
        tab.call("hello", hello)
            .await
            .unwrap_or_else(|error| panic!("hello: {error}"));
        if resume.is_some() {
            tab.publish().await;
        }
        tab
    }

    /// The next text frame, by `deadline`: the one bound of the whole wait that
    /// called it. Control frames (the hub's pings) and other frames read on the way do
    /// not start the wait again, so a test that breaks fails instead of hanging.
    async fn frame(&mut self, deadline: Instant) -> Value {
        loop {
            let message = tokio::time::timeout_at(deadline, self.socket.next())
                .await
                .unwrap_or_else(|_| panic!("the tab waited {WAIT:?} in vain"));
            match message {
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str(text.as_str())
                        .unwrap_or_else(|error| panic!("a frame that is not JSON: {error}"));
                }
                Some(Ok(_)) => {}
                other => panic!("the tab's connection ended: {other:?}"),
            }
        }
    }

    fn keep(&mut self, frame: Value) {
        if frame["t"] == "event" {
            let name = frame["name"].as_str().unwrap_or_default().to_owned();
            self.events.push_back((name, frame["payload"].clone()));
        }
    }

    /// Invokes a hub method; `Err` carries the hub's error object.
    pub async fn call(&mut self, method: &str, args: Value) -> Result<Value, Value> {
        self.next += 1;
        let id = self.next;
        let frame = json!({ "t": "invoke", "id": id, "method": method, "args": args });
        let deadline = Instant::now() + WAIT;
        tokio::time::timeout_at(deadline, self.socket.send(Message::text(frame.to_string())))
            .await
            .unwrap_or_else(|_| panic!("{method}: not sent within {WAIT:?}"))
            .unwrap_or_else(|error| panic!("{method}: {error}"));
        loop {
            let frame = self.frame(deadline).await;
            if frame["t"] == "result" && frame["id"] == id {
                return if frame["ok"] == true {
                    Ok(frame["value"].clone())
                } else {
                    Err(frame["error"].clone())
                };
            }
            self.keep(frame);
        }
    }

    async fn must(&mut self, method: &str, args: Value) -> Value {
        self.call(method, args)
            .await
            .unwrap_or_else(|error| panic!("{method}: {error}"))
    }

    /// Waits for the hub's event `name` (`binding`, `cli-message`, `broadcast`, ...)
    /// and returns its payload; earlier events of other names are kept.
    pub async fn saw(&mut self, name: &str) -> Value {
        self.saw_by(name, Instant::now() + WAIT).await
    }

    async fn saw_by(&mut self, name: &str, deadline: Instant) -> Value {
        if let Some(index) = self.events.iter().position(|(n, _)| n == name)
            && let Some((_, payload)) = self.events.remove(index)
        {
            return payload;
        }
        loop {
            let frame = self.frame(deadline).await;
            if frame["t"] == "event" && frame["name"] == name {
                return frame["payload"].clone();
            }
            self.keep(frame);
        }
    }

    /// Waits for a `binding` event in `state` (`active`, `suspended`, `gone`); the
    /// binding events before it are passed over.
    pub async fn saw_binding(&mut self, state: &str) -> Value {
        // One bound for the whole wait, however many other states pass by.
        let deadline = Instant::now() + WAIT;
        loop {
            let event = self.saw_by("binding", deadline).await;
            if event["state"] == state {
                return event;
            }
        }
    }

    /// Waits for the next request from the CLI.
    pub async fn request(&mut self) -> Request {
        let message = self.saw("cli-message").await;
        let id = message["id"].as_str().unwrap_or_default().to_owned();
        let kind = message["kind"].as_str().unwrap_or_default().to_owned();
        let argv = message["argv"].as_array().map(|argv| {
            argv.iter()
                .map(|word| word.as_str().unwrap_or_default().to_owned())
                .collect()
        });
        Request {
            id,
            kind,
            argv,
            message,
        }
    }

    /// Sends a reply to the bound CLI; the CLI's connection must be there to take it.
    pub async fn reply(&mut self, envelope: &Envelope) {
        let sent = self.send_to_cli(envelope.value().clone()).await;
        let sent = sent.unwrap_or_else(|error| panic!("sendToCli {}: {error}", envelope.kind()));
        assert_eq!(
            sent["delivered"],
            true,
            "sendToCli {}: no CLI connection took it",
            envelope.kind()
        );
    }

    /// `sendToCli` with any value, for tests of the hub's checks.
    pub async fn send_to_cli(&mut self, reply: Value) -> Result<Value, Value> {
        self.call("sendToCli", json!({ "reply": reply })).await
    }

    /// A one-time attach code for this page at `level` (`ask-server`, `never-ask`).
    pub async fn attach_code(&mut self, level: &str) -> String {
        let page = self.page.clone();
        let made = self
            .must("attachCode", json!({ "pageId": page, "level": level }))
            .await;
        made["code"]
            .as_str()
            .unwrap_or_else(|| panic!("attachCode gave no code: {made}"))
            .to_owned()
    }

    /// Waits for this page's new binding and publishes to it, as a bound tab does.
    pub async fn bound(&mut self) -> Value {
        let binding = self.saw_binding("active").await;
        self.publish().await;
        binding
    }

    /// Publishes the 1.8.2 fixtures' command list ([`capabilities`]) and the place.
    pub async fn publish(&mut self) {
        self.must("publishCapabilities", capabilities()).await;
        let place = self.place.clone();
        self.must("publishWhere", place).await;
    }

    pub async fn publish_capabilities(&mut self, published: Value) -> Result<Value, Value> {
        self.call("publishCapabilities", published).await
    }

    /// The place the tab publishes from now on.
    pub async fn publish_where(&mut self, place: Value) -> Result<Value, Value> {
        self.place = place.clone();
        self.call("publishWhere", place).await
    }

    /// The page is closing or reloading (`unload`).
    pub async fn unload(&mut self) {
        self.must("unload", json!({})).await;
    }

    /// The user ends the binding in the tab.
    pub async fn detach(&mut self) {
        self.must("detach", json!({})).await;
    }

    pub async fn broadcast(&mut self, envelope: Value) -> Result<Value, Value> {
        self.call("broadcast", json!({ "envelope": envelope }))
            .await
    }

    /// Reads the socket once, round trip: the pings that came while the tab was
    /// silent are answered.
    pub async fn heartbeat(&mut self) {
        let place = self.place.clone();
        // A tab without a binding is refused; the round trip is what counts.
        let _ = self.call("publishWhere", place).await;
    }

    /// Closes the connection, as a closed page does.
    pub async fn close(mut self) {
        let _ = tokio::time::timeout(WAIT, async {
            let _ = self.socket.close(None).await;
            while self.socket.next().await.is_some() {}
        })
        .await;
    }
}
