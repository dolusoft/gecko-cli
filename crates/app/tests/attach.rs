//! `gecko attach` end to end: the real CLI binary against the real development hub,
//! with a fake Gecko tab on the hub's WebSocket. The tab makes an attach code, the CLI
//! spends it, and lines travel to the tab and back (attach spec §3, §4, §5).
#![cfg(feature = "dev-hub")]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use gecko_dev_hub::{Options, Running, start};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{WebSocketStream, client_async};

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

async fn hub(change: impl FnOnce(&mut Options)) -> Running {
    let mut options = Options {
        port: 0,
        rate_per_second: 1000.0,
        ..Options::default()
    };
    options.timings.heartbeat_ms = 600_000.0;
    change(&mut options);
    start(options).await.expect("hub")
}

/// An empty config directory.
fn config(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("attach")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("config");
    dir
}

fn gecko(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gecko"));
    command
        .args(args)
        .env("GECKO_CONFIG_DIR", dir)
        .env_remove("GECKO_TRANSPORT")
        .env_remove("GECKO_TEST_INTERRUPT_STDIN")
        .env_remove("GECKO_TEST_TTY")
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Runs the CLI with `stdin` on a blocking thread, so the tab can answer meanwhile.
fn spawn_cli(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    stdin: Option<&str>,
) -> tokio::task::JoinHandle<Output> {
    let mut command = gecko(dir, args);
    for (key, value) in env {
        command.env(key, value);
    }
    let input = stdin.map(str::to_owned);
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    tokio::task::spawn_blocking(move || {
        let mut child = command.spawn().expect("spawn_cli");
        if let Some(input) = input {
            child
                .stdin
                .take()
                .expect("spawn_cli")
                .write_all(input.as_bytes())
                .expect("spawn_cli");
        }
        child.wait_with_output().expect("spawn_cli")
    })
}

async fn cli(dir: &Path, args: &[&str]) -> Output {
    spawn_cli(dir, args, &[], None).await.expect("cli")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("text")
}

fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..=chunk.len() {
            out.push(char::from(TABLE[((n >> (18 - 6 * i)) & 63) as usize]));
        }
    }
    out
}

/// A code as the tab would write it, for a given hub and secret.
fn code_for(hub: &str, secret: &str) -> String {
    let inside = json!({ "v": 1, "hub": hub, "s": secret }).to_string();
    format!("gk1_{}", base64url(inside.as_bytes()))
}

/// One HTTP request with a JSON body; the status and the JSON answer.
async fn http(hub: &Running, path: &str, body: Value) -> (u16, Value) {
    let mut stream = TcpStream::connect(("127.0.0.1", hub.port))
        .await
        .expect("http");
    let body = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.expect("http");
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.expect("http");
    let (head, body) = answer.split_once("\r\n\r\n").expect("http");
    let status = head.split(' ').nth(1).expect("http").parse().expect("http");
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn capabilities() -> Value {
    let list = json!({
        "envelopes": [1],
        "features": ["argv", "received-v2", "suggest-argv", "help-v2", "error-class", "capabilities-v2"],
        "revision": "r1",
        "sensitive": [],
        "commands": [
            { "name": "tasks.list", "aliases": [], "danger": "read" },
            { "name": "tasks.stop", "aliases": [], "danger": "change", "effect": "server" }
        ]
    });
    json!({ "revision": "r1", "parts": [list.to_string()] })
}

/// A fake Gecko tab of user `u1`.
struct Tab {
    socket: WebSocketStream<TcpStream>,
    next: u64,
    events: Vec<(String, Value)>,
}

impl Tab {
    async fn open(hub: &Running, page: &str, resume: Option<&str>) -> Tab {
        let request = format!("ws://{}/hub?access_token=dev:u1", hub.env)
            .into_client_request()
            .expect("open");
        let stream = TcpStream::connect(("127.0.0.1", hub.port))
            .await
            .expect("open");
        let (socket, _) = client_async(request, stream).await.expect("open");
        let mut tab = Tab {
            socket,
            next: 0,
            events: Vec::new(),
        };
        assert_eq!(tab.frame().await["name"], "ready");
        let mut hello = json!({ "pageId": page });
        if let Some(binding) = resume {
            hello["resume"] = json!(binding);
        }
        tab.call("hello", hello).await;
        if resume.is_some() {
            tab.publish().await;
        }
        tab
    }

    /// Publishes the command list and the place: a bound tab does so.
    async fn publish(&mut self) {
        self.call("publishCapabilities", capabilities()).await;
        self.call(
            "publishWhere",
            json!({ "title": "Tasks", "path": "/tasks" }),
        )
        .await;
    }

    /// Waits for the tab's new binding and publishes to it.
    async fn bound(&mut self) -> Value {
        loop {
            // An ended previous binding is told first.
            let event = self.event("binding").await;
            if event["state"] == "active" {
                self.publish().await;
                return event;
            }
        }
    }

    async fn frame(&mut self) -> Value {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(20), self.socket.next())
                .await
                .expect("a frame within 20 s");
            match message {
                Some(Ok(Message::Text(text))) => {
                    return serde_json::from_str(text.as_str()).expect("frame");
                }
                Some(Ok(_)) => {}
                other => panic!("{other:?}"),
            }
        }
    }

    async fn call(&mut self, method: &str, args: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let frame = json!({ "t": "invoke", "id": id, "method": method, "args": args });
        self.socket
            .send(Message::text(frame.to_string()))
            .await
            .expect("call");
        loop {
            let frame = self.frame().await;
            if frame["t"] == "result" && frame["id"] == id {
                assert_eq!(frame["ok"], true, "{method}: {frame}");
                return frame["value"].clone();
            }
            if frame["t"] == "event" {
                self.events.push((
                    frame["name"].as_str().unwrap_or("").to_owned(),
                    frame["payload"].clone(),
                ));
            }
        }
    }

    async fn event(&mut self, name: &str) -> Value {
        if let Some(index) = self.events.iter().position(|(n, _)| n == name) {
            return self.events.remove(index).1;
        }
        loop {
            let frame = self.frame().await;
            if frame["t"] == "event" && frame["name"] == name {
                return frame["payload"].clone();
            }
            if frame["t"] == "event" {
                self.events.push((
                    frame["name"].as_str().unwrap_or("").to_owned(),
                    frame["payload"].clone(),
                ));
            }
        }
    }

    async fn code(&mut self, page: &str, level: &str) -> String {
        let made = self
            .call("attachCode", json!({ "pageId": page, "level": level }))
            .await;
        made["code"].as_str().expect("code").to_owned()
    }

    /// Answers the next CLI line with `received` and an ok `finished`; returns its argv.
    async fn answer(&mut self, change: bool, approval: &str) -> Value {
        let message = self.event("cli-message").await;
        let id = message["id"].clone();
        let received = json!({ "v": 1, "kind": "received", "id": id, "hidden": false,
            "dataAgeMs": null, "change": change, "viewRelevant": false });
        let finished = json!({ "v": 1, "kind": "finished", "id": id,
            "outcome": { "status": "ok", "summary": "done" },
            "chunks": 0, "bytes": 0, "approval": approval });
        for reply in [received, finished] {
            self.call("sendToCli", json!({ "reply": reply })).await;
        }
        message["argv"].clone()
    }

    async fn close(mut self) {
        let _ = self.socket.close(None).await;
        while self.socket.next().await.is_some() {}
    }
}

fn token_of(dir: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("token.json")).expect("token_of"))
        .expect("token_of")
}

#[test]
fn a_code_binds_the_cli_and_lines_run_in_the_tab() {
    runtime().block_on(async {
        let hub = hub(|_| {}).await;
        let mut tab = Tab::open(&hub, "p1", None).await;
        let dir = config("bind");

        // A hub this computer has not used: no prompt without a terminal.
        let code = tab.code("p1", "ask-server").await;
        let output = cli(&dir, &["attach", &code]).await;
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(3), "{stderr}");
        assert!(stderr.contains(&format!("Connecting to {}", hub.url)), "{stderr}");
        assert!(stderr.contains("error (untrusted-hub)"), "{stderr}");
        assert!(stderr.contains("--trust-hub"), "{stderr}");
        assert!(!dir.join("token.json").exists());

        // The code was not spent: `--trust-hub` binds with it.
        let output = cli(&dir, &["attach", "--trust-hub", &code]).await;
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert_eq!(
            text(&output.stdout),
            "Attached: zahid · level ask-server (changes on the server ask for approval in the tab)\n"
        );
        let bound = tab.bound().await;
        let token = token_of(&dir);
        assert_eq!(token["hub"], hub.url.as_str());
        assert_eq!(token["bindingId"], bound["bindingId"]);
        assert_eq!(token["level"], "ask-server");

        // The used code is spent.
        let output = cli(&dir, &["attach", &code]).await;
        assert_eq!(output.status.code(), Some(6));
        assert!(
            text(&output.stderr)
                .contains("error (invalid-code): the code is invalid, expired or already used; if you did not use it, check the binding in the tab; then copy a new code"),
            "{}",
            text(&output.stderr)
        );

        // A line runs in the tab; `--yes` and `--dry-run` stay in its argv.
        let run = spawn_cli(&dir, &["tasks.stop", "--id", "7", "--yes"], &[], None);
        let argv = tab.answer(true, "card").await;
        assert_eq!(argv, json!(["tasks.stop", "--id", "7", "--yes"]));
        let output = run.await.unwrap();
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        assert_eq!(text(&output.stdout), "done\n");
        assert!(!text(&output.stderr).contains("without approval"));

        let run = spawn_cli(&dir, &["tasks.stop", "--dry-run"], &[], None);
        assert_eq!(
            tab.answer(false, "none").await,
            json!(["tasks.stop", "--dry-run"])
        );
        assert_eq!(run.await.unwrap().status.code(), Some(0));

        // The status names the level.
        let output = cli(&dir, &["status"]).await;
        let stdout = text(&output.stdout);
        assert_eq!(output.status.code(), Some(0), "{stdout}");
        assert!(stdout.contains("user    zahid\n"), "{stdout}");
        assert!(stdout.contains("tab     bound (attached just now) · Tasks · /tasks"), "{stdout}");
        assert!(stdout.contains("level   ask-server"), "{stdout}");
        assert!(stdout.contains(&format!("hub     {} · reachable", hub.url)), "{stdout}");

        // `detach` ends the binding: the token goes and its refresh is refused.
        let output = cli(&dir, &["detach"]).await;
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(text(&output.stdout), "Detached\n");
        assert!(!dir.join("token.json").exists());
        let (status, refused) = http(
            &hub,
            "/api/v0/connect/token",
            json!({ "grant_type": "refresh_token", "refresh_token": token["refreshToken"] }),
        )
        .await;
        assert_eq!((status, refused["error"].as_str()), (400, Some("invalid_grant")));
        let ended = tab.event("binding").await;
        assert_eq!(ended["state"], "gone");

        tab.close().await;
        hub.close();
    });
}

#[test]
fn a_trusted_hub_binds_from_stdin_and_never_ask_says_so() {
    runtime().block_on(async {
        let hub = hub(|_| {}).await;
        let mut tab = Tab::open(&hub, "p1", None).await;
        let dir = config("stdin");

        // The prompt on a terminal: `y` trusts the hub.
        let code = tab.code("p1", "ask-server").await;
        let output = spawn_cli(
            &dir,
            &["attach", &code],
            &[("GECKO_TEST_TTY", "1")],
            Some("y\n"),
        )
        .await
        .unwrap();
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert!(
            stderr.contains(&format!(
                "{} has not been used from this computer. Connect to it? [y/N]",
                hub.url
            )),
            "{stderr}"
        );
        tab.bound().await;

        // A trusted hub asks no more; `attach -` reads the code from stdin, and the
        // previous binding ends first.
        let code = tab.code("p1", "never-ask").await;
        let output = spawn_cli(
            &dir,
            &["--json", "attach", "-"],
            &[],
            Some(&format!("{code}\n")),
        )
        .await
        .unwrap();
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{stderr}");
        assert!(stderr.contains("The previous binding ended"), "{stderr}");
        tab.bound().await;
        let answer: Value = serde_json::from_str(&text(&output.stdout)).unwrap();
        assert_eq!(answer["data"]["level"], "never-ask");
        assert_eq!(answer["data"]["hub"], hub.url.as_str());

        // A change that ran without approval says so: a note in text, the field in JSON.
        let run = spawn_cli(&dir, &["tasks.stop", "--id", "7"], &[], None);
        tab.answer(true, "none").await;
        let output = run.await.unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(
            text(&output.stderr).contains("(ran without approval: binding level never-ask)"),
            "{}",
            text(&output.stderr)
        );
        let run = spawn_cli(&dir, &["--json", "tasks.stop", "--id", "7"], &[], None);
        tab.answer(true, "none").await;
        let output = run.await.unwrap();
        let result: Value = serde_json::from_str(&text(&output.stdout)).unwrap();
        assert_eq!(result["approval"], "none");
        assert!(!text(&output.stderr).contains("without approval"));

        tab.close().await;
        hub.close();
    });
}

#[test]
fn bad_codes_and_hubs_are_refused() {
    runtime().block_on(async {
        let hub = hub(|_| {}).await;
        let dir = config("bad");
        for (code, exit, error) in [
            ("gk1_not-json".to_owned(), 6, "error (invalid-code)"),
            ("ABCD-EFGH".to_owned(), 6, "error (invalid-code)"),
            (
                code_for("http://192.168.1.5:4400", "s"),
                3,
                "error (insecure-hub)",
            ),
            (
                code_for(&format!("{}/path", hub.url), "s"),
                6,
                "error (invalid-code)",
            ),
            (
                code_for(&hub.url, "a-wrong-secret-of-enough-length"),
                6,
                "error (invalid-code)",
            ),
        ] {
            let output = cli(&dir, &["attach", "--trust-hub", &code]).await;
            let stderr = text(&output.stderr);
            assert_eq!(output.status.code(), Some(exit), "{code}: {stderr}");
            assert!(stderr.contains(error), "{code}: {stderr}");
        }
        assert!(!dir.join("token.json").exists());
        hub.close();
    });
}

#[test]
fn a_new_cli_takes_the_tab_and_a_reload_keeps_the_binding() {
    runtime().block_on(async {
        let hub = hub(|_| {}).await;
        let mut tab = Tab::open(&hub, "p1", None).await;

        let first = config("first");
        let code = tab.code("p1", "ask-server").await;
        let output = cli(&first, &["attach", "--trust-hub", &code]).await;
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        tab.bound().await;

        // A second CLI binds the same tab: the first one's binding is replaced.
        let second = config("second");
        let code = tab.code("p1", "ask-server").await;
        let output = cli(&second, &["attach", "--trust-hub", &code]).await;
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        tab.bound().await;
        let output = cli(&first, &["tasks.list"]).await;
        assert_eq!(output.status.code(), Some(4));
        assert!(
            text(&output.stderr).contains("error (gone)"),
            "{}",
            text(&output.stderr)
        );
        assert!(!first.join("token.json").exists());

        // The tab reloads: it says `unload`, and the page that comes back resumes. A
        // line sent meanwhile waits for it.
        let binding = token_of(&second)["bindingId"].as_str().unwrap().to_owned();
        tab.call("unload", json!({})).await;
        tab.close().await;
        let run = spawn_cli(&second, &["tasks.list"], &[], None);
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut again = Tab::open(&hub, "p1", Some(&binding)).await;
        assert_eq!(again.answer(false, "none").await, json!(["tasks.list"]));
        let output = run.await.unwrap();
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        assert_eq!(text(&output.stdout), "done\n");

        again.close().await;
        hub.close();
    });
}
