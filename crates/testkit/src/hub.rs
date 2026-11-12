//! The development hub on its own thread and its own tokio runtime. Stopping it drops
//! the runtime, and with it every open connection: the CLI and the tab see the hub go.

use std::sync::mpsc;
use std::thread::JoinHandle;

pub use gecko_dev_hub::{Options, Timings, User};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

use crate::WAIT;

/// A running development hub.
#[derive(Debug)]
pub struct Hub {
    /// `http://127.0.0.1:<port>`
    pub url: String,
    pub port: u16,
    /// `127.0.0.1:<port>`
    pub env: String,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Hub {
    /// The tests' options: a free port, a rate limit no test reaches, and a heartbeat
    /// that does not fire while a test runs (a test that cuts the heartbeat shortens it).
    pub fn options() -> Options {
        let mut options = Options {
            port: 0,
            rate_per_second: 1000.0,
            ..Options::default()
        };
        options.timings.heartbeat_ms = 600_000.0;
        options
    }

    /// A hub with the tests' options.
    pub fn start() -> Hub {
        Hub::with(|_| {})
    }

    /// A hub with the tests' options, changed by `change` (timings, users, rate).
    pub fn with(change: impl FnOnce(&mut Options)) -> Hub {
        let mut options = Hub::options();
        change(&mut options);
        Hub::start_with(options)
    }

    /// A hub with exactly these options; `Options { port: 0, ..Options::default() }`
    /// keeps the default rate limit and heartbeat.
    pub fn start_with(options: Options) -> Hub {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("gecko-testkit-hub".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("the hub's runtime starts");
                runtime.block_on(async move {
                    match gecko_dev_hub::start(options).await {
                        Ok(running) => {
                            let address = (running.url.clone(), running.port, running.env.clone());
                            let _ = ready_tx.send(Ok(address));
                            let _ = stop_rx.await;
                            running.close();
                        }
                        Err(error) => {
                            let _ = ready_tx.send(Err(error.to_string()));
                        }
                    }
                });
                // The runtime goes with its tasks: every connection closes here.
                drop(runtime);
            })
            .expect("the hub's thread starts");
        let (url, port, env) = ready_rx
            .recv_timeout(WAIT)
            .expect("the hub answers within the wait")
            .expect("the hub listens");
        Hub {
            url,
            port,
            env,
            stop: Some(stop_tx),
            thread: Some(thread),
        }
    }

    /// Stops the hub: every open connection ends without a close frame, as when the
    /// hub's process dies.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// One `POST` with a JSON body: the status and the JSON answer (`null` when the
    /// body is not JSON).
    pub async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        let exchange = async {
            let mut stream = TcpStream::connect(("127.0.0.1", self.port))
                .await
                .expect("the hub accepts a connection");
            let body = body.to_string();
            let request = format!(
                "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                self.env,
                body.len()
            );
            stream
                .write_all(request.as_bytes())
                .await
                .expect("the request is written");
            let mut answer = String::new();
            stream
                .read_to_string(&mut answer)
                .await
                .expect("the answer is read");
            answer
        };
        let answer = tokio::time::timeout(WAIT, exchange)
            .await
            .unwrap_or_else(|_| panic!("POST {path}: no answer within {WAIT:?}"));
        let (head, body) = answer
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("POST {path}: not HTTP: {answer:?}"));
        let status = head
            .split(' ')
            .nth(1)
            .and_then(|status| status.parse().ok())
            .unwrap_or_else(|| panic!("POST {path}: no status: {head:?}"));
        (status, serde_json::from_str(body).unwrap_or(Value::Null))
    }

    /// A refresh of `refresh_token` at the token endpoint.
    pub async fn refresh(&self, refresh_token: &str) -> (u16, Value) {
        let body =
            serde_json::json!({ "grant_type": "refresh_token", "refresh_token": refresh_token });
        self.post("/api/v0/connect/token", &body).await
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.shutdown();
    }
}
