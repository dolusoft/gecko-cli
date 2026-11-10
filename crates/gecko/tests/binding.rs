//! `gecko attach`, `detach` and `status` as processes, against a scripted development
//! hub. The same listener answers the token refresh (`POST /oauth/token`), as the
//! environment's host serves both.
// Test helpers outside #[test] functions; the upgrade callback's error type is
// tungstenite's own, large by design.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// How the scripted hub behaves.
#[derive(Clone)]
struct Plan {
    /// The `binding` field of the `status` answer.
    binding: Value,
    /// Tokens the hub refuses at the upgrade (401).
    refused_tokens: Vec<&'static str>,
    /// Closes right after the upgrade with this code (4409 busy, ...).
    close_at_once: Option<u16>,
    /// The pairing code's lifetime.
    pair_expires_in_ms: u64,
    /// Sent after the `pair` answer: events by name, or a close code.
    after_pair: Vec<Step>,
}

#[derive(Clone)]
enum Step {
    Event(&'static str, Value),
    Close(u16),
}

impl Default for Plan {
    fn default() -> Plan {
        Plan {
            binding: Value::Null,
            refused_tokens: Vec::new(),
            close_at_once: None,
            pair_expires_in_ms: 120_000,
            after_pair: Vec::new(),
        }
    }
}

/// What the hub saw: upgrade tokens, hub methods, refresh requests.
#[derive(Default, Debug, Clone)]
struct Seen {
    tokens: Vec<String>,
    methods: Vec<String>,
    refreshes: usize,
}

fn active_binding() -> Value {
    json!({
        "bindingId": "b1", "state": "active", "pageId": "p1",
        "approvedAt": now_ms() - 7 * 60_000,
        "where": { "title": "Tasks", "path": "/tasks" }
    })
}

/// Starts the hub on its own thread; returns `host:port` and the log.
fn hub(plan: Plan) -> (String, Arc<Mutex<Seen>>) {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std_listener.set_nonblocking(true).unwrap();
    let host = std_listener.local_addr().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let log = seen.clone();
    let address = host.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = TcpListener::from_std(std_listener).unwrap();
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                tokio::spawn(serve(tcp, plan.clone(), log.clone(), address.clone()));
            }
        });
    });
    (host, seen)
}

async fn serve(tcp: TcpStream, plan: Plan, log: Arc<Mutex<Seen>>, host: String) {
    let mut head = [0u8; 4];
    if tcp.peek(&mut head).await.is_err() {
        return;
    }
    if &head == b"POST" {
        refresh(tcp, &log).await;
        return;
    }
    let refused = plan.refused_tokens.clone();
    let record = log.clone();
    let callback = move |request: &Request, response: Response| {
        let token = request
            .headers()
            .get("authorization")
            .map(|value| {
                value
                    .to_str()
                    .unwrap()
                    .trim_start_matches("Bearer ")
                    .to_owned()
            })
            .unwrap_or_default();
        record.lock().unwrap().tokens.push(token.clone());
        if refused.iter().any(|r| *r == token) {
            let mut error = ErrorResponse::new(Some("unauthorized".to_owned()));
            *error.status_mut() = StatusCode::UNAUTHORIZED;
            return Err(error);
        }
        Ok(response)
    };
    let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(tcp, callback).await else {
        return;
    };
    if let Some(code) = plan.close_at_once {
        close(&mut socket, code).await;
        return;
    }
    say(
        &mut socket,
        json!({ "t": "event", "name": "ready", "payload": {} }),
    )
    .await;
    while let Some(Ok(message)) = socket.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let frame: Value = serde_json::from_str(text.as_str()).unwrap();
        let method = frame["method"].as_str().unwrap().to_owned();
        log.lock().unwrap().methods.push(method.clone());
        let id = frame["id"].clone();
        let value = match method.as_str() {
            "status" => json!({
                "user": { "id": "u1", "name": "Zahit" }, "env": host, "grantId": "g_1",
                "binding": plan.binding
            }),
            "pair" => json!({
                "code": "ABCD-EFGH",
                "expiresAt": now_ms() + plan.pair_expires_in_ms,
                "url": format!("http://{host}/cli-pair#ABCD-EFGH")
            }),
            _ => json!({}),
        };
        say(
            &mut socket,
            json!({ "t": "result", "id": id, "ok": true, "value": value }),
        )
        .await;
        if method == "pair" {
            for step in &plan.after_pair {
                match step {
                    Step::Event(name, payload) => {
                        say(
                            &mut socket,
                            json!({ "t": "event", "name": name, "payload": payload }),
                        )
                        .await;
                    }
                    Step::Close(code) => {
                        close(&mut socket, *code).await;
                        return;
                    }
                }
            }
        }
    }
}

type Socket = tokio_tungstenite::WebSocketStream<TcpStream>;

async fn say(socket: &mut Socket, frame: Value) {
    socket.send(Message::text(frame.to_string())).await.unwrap();
}

async fn close(socket: &mut Socket, code: u16) {
    let _ = socket
        .close(Some(CloseFrame {
            code: CloseCode::from(code),
            reason: "".into(),
        }))
        .await;
}

/// Answers a refresh with a new access token.
async fn refresh(mut tcp: TcpStream, log: &Arc<Mutex<Seen>>) {
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = tcp.read(&mut buffer).await.unwrap();
        request.extend_from_slice(&buffer[..read]);
        let text = String::from_utf8_lossy(&request);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if request.len() >= end + 4 + length || read == 0 {
                break;
            }
        }
        if read == 0 {
            break;
        }
    }
    log.lock().unwrap().refreshes += 1;
    let body = json!({
        "access_token": "at-2", "token_type": "Bearer", "expires_in": 900,
        "refresh_token": "rt-2", "grant_id": "g_1",
        "user": { "id": "u1", "name": "Zahit" }
    })
    .to_string();
    let answer = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    tcp.write_all(answer.as_bytes()).await.unwrap();
}

/// A config directory signed in to `host` with access token `at-1`.
fn signed_in(name: &str, host: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("binding")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), json!({ "env": host }).to_string()).unwrap();
    let token = json!({
        "env": host, "accessToken": "at-1", "accessExpiresAt": now_ms() + 3_600_000,
        "refreshToken": "rt-1", "grantId": "g_1", "user": { "id": "u1", "name": "Zahit" }
    });
    std::fs::write(
        dir.join(format!("token-{}.json", host.replace(':', "_"))),
        token.to_string(),
    )
    .unwrap();
    dir
}

fn gecko(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gecko"));
    command
        .args(args)
        .env("GECKO_CONFIG_DIR", dir)
        .env("GECKO_BROWSER", "none")
        .env_remove("GECKO_TRANSPORT")
        .env_remove("GECKO_TEST_INTERRUPT_STDIN")
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .stdin(Stdio::null());
    command
}

fn run(dir: &Path, args: &[&str]) -> Output {
    gecko(dir, args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn seen(log: &Arc<Mutex<Seen>>) -> Seen {
    log.lock().unwrap().clone()
}

#[test]
fn status_shows_env_tab_and_hub() {
    let (host, log) = hub(Plan {
        binding: active_binding(),
        ..Plan::default()
    });
    let dir = signed_in("status", &host);
    let output = run(&dir, &["status"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], format!("env     {host} · Zahit"));
    assert_eq!(
        lines[1],
        "tab     bound (approved 7 min ago) · Tasks · /tasks"
    );
    assert!(lines[2].starts_with("hub     reachable · "), "{stdout}");
    assert!(lines[2].ends_with(" ms"), "{stdout}");
    let seen = seen(&log);
    assert_eq!(seen.tokens, ["at-1"]);
    assert_eq!(seen.methods, ["status"]);
}

#[test]
fn status_json_has_data_and_marks_the_tab_text() {
    let (host, _) = hub(Plan {
        binding: active_binding(),
        ..Plan::default()
    });
    let dir = signed_in("status-json", &host);
    let output = run(&dir, &["--json", "status"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = text(&output.stdout);
    assert!(stdout.is_ascii());
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        value["summary"],
        "tab     bound (approved 7 min ago) · Tasks · /tasks"
    );
    assert_eq!(value["data"]["env"], host.as_str());
    assert_eq!(value["data"]["user"]["name"], "Zahit");
    assert_eq!(value["data"]["hub"]["reachable"], true);
    assert_eq!(value["data"]["binding"]["state"], "active");
    assert_eq!(value["data"]["binding"]["title"], "Tasks");
    assert_eq!(value["data"]["binding"]["tab"], "ok");
    assert_eq!(
        value["untrusted"],
        json!(["data.binding.title", "data.binding.path"])
    );
}

#[test]
fn status_when_busy_is_still_an_answer() {
    let (host, _) = hub(Plan {
        close_at_once: Some(4409),
        ..Plan::default()
    });
    let dir = signed_in("status-busy", &host);
    let output = run(&dir, &["status"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    assert!(
        stdout.contains("tab     unknown · another gecko call is running"),
        "{stdout}"
    );
    assert!(stdout.contains("hub     reachable\n"), "{stdout}");
}

#[test]
fn status_without_sign_in_or_hub() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("binding/none");
    let _ = std::fs::remove_dir_all(&dir);
    let output = run(&dir, &["status", "--env", "127.0.0.1:9"]);
    assert_eq!(output.status.code(), Some(6));
    assert!(text(&output.stderr).contains("error (not-logged-in)"));

    // A port nobody listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let dir = signed_in("unreachable", &port);
    let output = run(&dir, &["status"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(text(&output.stderr).contains("error (unreachable)"));
}

#[test]
fn a_refused_token_is_refreshed_once() {
    let (host, log) = hub(Plan {
        refused_tokens: vec!["at-1"],
        ..Plan::default()
    });
    let dir = signed_in("refresh", &host);
    let output = run(&dir, &["status"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let seen = seen(&log);
    assert_eq!(seen.tokens, ["at-1", "at-2"]);
    assert_eq!(seen.refreshes, 1);
    let token = std::fs::read_to_string(dir.join(format!("token-{}.json", host.replace(':', "_"))))
        .unwrap();
    assert!(token.contains("\"at-2\""), "{token}");

    let (host, _) = hub(Plan {
        refused_tokens: vec!["at-1", "at-2"],
        ..Plan::default()
    });
    let dir = signed_in("refresh-refused", &host);
    let output = run(&dir, &["status"]);
    assert_eq!(output.status.code(), Some(6));
    assert!(text(&output.stderr).contains("error (unauthorized)"));
}

#[test]
fn attach_when_bound_does_not_pair() {
    let (host, log) = hub(Plan {
        binding: active_binding(),
        ..Plan::default()
    });
    let dir = signed_in("attach-bound", &host);
    let output = run(&dir, &["attach"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        text(&output.stdout),
        "Already bound: Tasks · /tasks. For a new tab: gecko attach --new\n"
    );
    assert_eq!(seen(&log).methods, ["status"]);
}

fn paired() -> Step {
    Step::Event(
        "paired",
        json!({
            "bindingId": "b2", "state": "active", "pageId": "p2", "approvedAt": now_ms(),
            "where": { "title": "Assets", "path": "/assets" }
        }),
    )
}

#[test]
fn attach_pairs_and_names_the_tab() {
    let (host, log) = hub(Plan {
        after_pair: vec![
            Step::Event("pair-refused", json!({ "code": "other-account" })),
            paired(),
        ],
        ..Plan::default()
    });
    let dir = signed_in("attach", &host);
    let output = run(&dir, &["attach"]);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(text(&output.stdout), "Attached: Assets · /assets\n");
    assert!(
        stderr.contains(&format!(
            "Code: ABCD-EFGH - open http://{host}/cli-pair#ABCD-EFGH"
        )),
        "{stderr}"
    );
    assert!(stderr.contains("cli.attach ABCD-EFGH"), "{stderr}");
    assert!(stderr.contains("another account"), "{stderr}");
    assert_eq!(seen(&log).methods, ["status", "pair"]);
}

#[test]
fn attach_new_pairs_even_when_bound() {
    let (host, log) = hub(Plan {
        binding: active_binding(),
        after_pair: vec![paired()],
        ..Plan::default()
    });
    let dir = signed_in("attach-new", &host);
    let output = run(&dir, &["attach", "--new"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(text(&output.stdout), "Attached: Assets · /assets\n");
    assert_eq!(seen(&log).methods, ["status", "pair"]);
}

#[test]
fn attach_failures_have_their_exit_codes() {
    for (name, plan, code, error) in [
        (
            "refused",
            Plan {
                after_pair: vec![Step::Event(
                    "pair-refused",
                    json!({ "code": "no-terminal-access" }),
                )],
                ..Plan::default()
            },
            3,
            "error (no-terminal-access): this account has no terminal access",
        ),
        (
            "expired",
            Plan {
                after_pair: vec![Step::Event("pair-refused", json!({ "code": "expired" }))],
                ..Plan::default()
            },
            4,
            "error (expired)",
        ),
        (
            "revoked",
            Plan {
                after_pair: vec![Step::Close(4410)],
                ..Plan::default()
            },
            6,
            "error (revoked)",
        ),
        (
            "lost",
            Plan {
                after_pair: vec![Step::Close(1011)],
                ..Plan::default()
            },
            4,
            "error (connection-lost)",
        ),
        (
            "timeout",
            Plan {
                pair_expires_in_ms: 0,
                ..Plan::default()
            },
            3,
            "error (pair-timeout)",
        ),
    ] {
        let (host, _) = hub(plan);
        let dir = signed_in(&format!("attach-{name}"), &host);
        let output = run(&dir, &["attach"]);
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(code), "{name}: {stderr}");
        assert!(stderr.contains(error), "{name}: {stderr}");
    }
}

#[test]
fn attach_can_be_interrupted() {
    let (host, _) = hub(Plan::default());
    let dir = signed_in("attach-interrupt", &host);
    let mut child = gecko(&dir, &["attach"])
        .env("GECKO_TEST_INTERRUPT_STDIN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert!(text(&output.stderr).contains("error (cancelled)"));
}

#[test]
fn detach_says_what_was_there() {
    let (host, log) = hub(Plan {
        binding: active_binding(),
        ..Plan::default()
    });
    let dir = signed_in("detach", &host);
    let output = run(&dir, &["detach"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(text(&output.stdout), "Detached\n");
    assert_eq!(seen(&log).methods, ["status", "detach"]);

    let (host, log) = hub(Plan::default());
    let dir = signed_in("detach-none", &host);
    let output = run(&dir, &["detach"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(text(&output.stdout), "There was no binding\n");
    assert_eq!(seen(&log).methods, ["status"]);
}
