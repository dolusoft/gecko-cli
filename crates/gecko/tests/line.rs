//! A line for the bound tab as a process, against a scripted development hub that
//! plays the tab too: it publishes a capability list in `status`, stamps `send`, and
//! pushes the replies of the plan back as `reply` events.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// What the tab answers: replies by request kind; `close` ends the socket after them.
#[derive(Clone, Default)]
struct Plan {
    /// The `binding.capabilities` field of `status`; `None` leaves it out.
    capabilities: Option<Value>,
    no_binding: bool,
    /// Replies to a `cli-request`, without `v`/`id` (they are filled in).
    replies: Vec<Value>,
    /// Closes the socket after the replies.
    close_after: bool,
    /// The reply to a `help-request`.
    help: Option<Value>,
    /// How many first sends the hub refuses as `rate-limited`.
    rate_limited: usize,
    /// The tab's wait between its first reply and the rest.
    delay_ms: u64,
}

#[derive(Default, Debug, Clone)]
struct Seen {
    methods: Vec<String>,
    envelopes: Vec<Value>,
}

fn caps(commands: Value) -> Value {
    let list = json!({
        "envelopes": [1],
        "features": ["argv", "received-v2", "suggest-argv", "help-v2", "error-class", "capabilities-v2"],
        "revision": "r1",
        "sensitive": ["signin"],
        "commands": commands
    });
    json!({ "revision": "r1", "parts": [list.to_string()] })
}

fn standard_caps() -> Option<Value> {
    Some(caps(json!([
        { "name": "tasks.list", "aliases": ["tl"], "danger": "read" },
        { "name": "tasks.stop", "aliases": [], "danger": "change" },
        { "name": "dataview.open", "aliases": [], "danger": "read" }
    ])))
}

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

type Socket = tokio_tungstenite::WebSocketStream<TcpStream>;

async fn say(socket: &mut Socket, frame: Value) {
    socket.send(Message::text(frame.to_string())).await.unwrap();
}

async fn serve(tcp: TcpStream, plan: Plan, log: Arc<Mutex<Seen>>, host: String) {
    let Ok(mut socket) = tokio_tungstenite::accept_async(tcp).await else {
        return;
    };
    say(
        &mut socket,
        json!({ "t": "event", "name": "ready", "payload": {} }),
    )
    .await;
    let mut refused = 0;
    while let Some(Ok(message)) = socket.next().await {
        let Message::Text(text) = message else {
            continue;
        };
        let frame: Value = serde_json::from_str(text.as_str()).unwrap();
        let method = frame["method"].as_str().unwrap().to_owned();
        let id = frame["id"].clone();
        log.lock().unwrap().methods.push(method.clone());
        match method.as_str() {
            "status" => {
                let mut binding = json!({
                    "bindingId": "b1", "state": "active", "pageId": "p1",
                    "approvedAt": now_ms() - 60_000
                });
                if let Some(caps) = &plan.capabilities {
                    binding["capabilities"] = caps.clone();
                }
                let value = json!({
                    "user": { "id": "u1", "name": "Zahit" }, "env": host, "grantId": "g_1",
                    "binding": if plan.no_binding { Value::Null } else { binding }
                });
                say(
                    &mut socket,
                    json!({ "t": "result", "id": id, "ok": true, "value": value }),
                )
                .await;
            }
            "send" => {
                if refused < plan.rate_limited {
                    refused += 1;
                    say(
                        &mut socket,
                        json!({ "t": "result", "id": id, "ok": false,
                                "error": { "code": "rate-limited", "retryAfterMs": 50 } }),
                    )
                    .await;
                    continue;
                }
                let envelope = frame["args"]["envelope"].clone();
                log.lock().unwrap().envelopes.push(envelope.clone());
                let now = now_ms();
                say(
                    &mut socket,
                    json!({ "t": "result", "id": id, "ok": true,
                            "value": { "stampedAt": now, "expiresAt": now + 30_000 } }),
                )
                .await;
                let request_id = envelope["id"].clone();
                let replies = match envelope["kind"].as_str() {
                    Some("cli-request") => plan.replies.clone(),
                    Some("help-request") => plan.help.clone().into_iter().collect(),
                    _ => Vec::new(),
                };
                for (n, mut reply) in replies.into_iter().enumerate() {
                    if n == 1 && plan.delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(plan.delay_ms)).await;
                    }
                    reply["v"] = json!(1);
                    reply["id"] = request_id.clone();
                    say(
                        &mut socket,
                        json!({ "t": "event", "name": "reply", "payload": reply }),
                    )
                    .await;
                }
                if plan.close_after && envelope["kind"] == "cli-request" {
                    let _ = socket.close(None).await;
                    return;
                }
            }
            _ => {
                say(
                    &mut socket,
                    json!({ "t": "result", "id": id, "ok": true, "value": {} }),
                )
                .await;
            }
        }
    }
}

fn signed_in(name: &str, host: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("line")
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

fn run(name: &str, plan: Plan, args: &[&str]) -> (Output, Seen) {
    let (host, log) = hub(plan);
    let dir = signed_in(name, &host);
    let output = gecko(&dir, args).output().unwrap();
    let seen = log.lock().unwrap().clone();
    (output, seen)
}

fn gecko(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gecko"));
    command
        .args(args)
        .env("GECKO_CONFIG_DIR", dir)
        .env("GECKO_BROWSER", "none")
        .env_remove("GECKO_TRANSPORT")
        .env_remove("GECKO_TEST_INTERRUPT_STDIN")
        .env_remove("GECKO_TEST_TTY")
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .stdin(Stdio::null());
    command
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn json_of(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn kinds(seen: &Seen) -> Vec<String> {
    seen.envelopes
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect()
}

fn read_ok() -> Vec<Value> {
    vec![
        json!({ "kind": "received", "hidden": false, "dataAgeMs": null, "change": false, "viewRelevant": false }),
        json!({ "kind": "chunk", "index": 0, "total": 1, "bytes": 7, "data": "{\"a\":1}" }),
        json!({ "kind": "finished", "outcome": { "status": "ok", "summary": "1 row" }, "chunks": 1, "bytes": 7,
                "next": ["tasks.list", "--cursor", "40"] }),
    ]
}

#[test]
fn a_read_command_runs_and_prints() {
    let plan = Plan {
        capabilities: standard_caps(),
        replies: read_ok(),
        ..Plan::default()
    };
    let (output, seen) = run("read-plain", plan, &["tasks.list", "--limit", "5"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "1 row\na: 1\nMore: gecko tasks.list --cursor 40\n"
    );
    assert_eq!(kinds(&seen), ["cli-request"]);
    assert_eq!(
        seen.envelopes[0]["argv"],
        json!(["tasks.list", "--limit", "5"])
    );
}

#[test]
fn json_result_carries_data_and_untrusted() {
    let plan = Plan {
        capabilities: standard_caps(),
        replies: read_ok(),
        ..Plan::default()
    };
    let (output, _) = run("read-json", plan, &["--json", "tl"]);
    assert_eq!(output.status.code(), Some(0));
    let result = json_of(&output);
    assert_eq!(result["status"], "ok");
    assert_eq!(result["data"], json!({ "a": 1 }));
    assert_eq!(result["next"], json!(["tasks.list", "--cursor", "40"]));
    assert_eq!(result["untrusted"], json!(["summary", "data"]));
    assert_eq!(result["hidden"], false);
    assert_eq!(result["dataAgeMs"], Value::Null);
}

#[test]
fn line_option_sends_the_line_and_the_view() {
    let plan = Plan {
        capabilities: standard_caps(),
        replies: read_ok(),
        ..Plan::default()
    };
    let (output, seen) = run(
        "line-view",
        plan,
        &["--json", "--view", "v-1", "--line", "tasks.list --q 'a b'"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(seen.envelopes[0]["line"], "tasks.list --q 'a b'");
    assert_eq!(seen.envelopes[0]["view"], "v-1");
}

#[test]
fn a_sensitive_command_never_leaves() {
    let plan = Plan {
        capabilities: standard_caps(),
        ..Plan::default()
    };
    let (output, seen) = run("sensitive", plan, &["--json", "signin", "--password", "x"]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(json_of(&output)["code"], "not-allowed");
    assert!(seen.envelopes.is_empty());
}

#[test]
fn an_unknown_name_asks_only_the_name() {
    let plan = Plan {
        capabilities: standard_caps(),
        help: Some(json!({
            "kind": "help-result", "status": "error", "code": "unknown-command", "class": "usage",
            "message": "unknown command: tsks", "suggest": ["tasks.list"]
        })),
        ..Plan::default()
    };
    let (output, seen) = run("unknown", plan, &["tsks", "--secret", "x"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(kinds(&seen), ["help-request"]);
    assert_eq!(seen.envelopes[0]["argv"], json!(["tsks"]));
    assert_eq!(
        text(&output.stderr),
        "error (unknown-command): unknown command: tsks\nDid you mean: gecko tasks.list\n"
    );
}

#[test]
fn no_binding_and_no_list() {
    let plan = Plan {
        no_binding: true,
        ..Plan::default()
    };
    let (output, _) = run("no-binding", plan, &["--json", "tasks.list"]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(json_of(&output)["code"], "no-binding");

    let (output, seen) = run("no-list", Plan::default(), &["--json", "tasks.list"]);
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(json_of(&output)["code"], "tab-unreachable");
    assert!(seen.envelopes.is_empty());
}

#[test]
fn an_old_tab_is_unsupported() {
    let plan = Plan {
        capabilities: Some(json!({ "v": 1, "kind": "capabilities", "commands": [] })),
        ..Plan::default()
    };
    let (output, _) = run("old-tab", plan, &["--json", "tasks.list"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_of(&output)["code"], "unsupported-version");
}

#[test]
fn a_lost_connection_after_a_change_started_is_unknown() {
    let plan = Plan {
        capabilities: standard_caps(),
        replies: vec![
            json!({ "kind": "received", "hidden": false, "dataAgeMs": null,
                               "change": true, "viewRelevant": false }),
        ],
        close_after: true,
        ..Plan::default()
    };
    let (output, _) = run("lost-change", plan, &["--json", "tasks.stop", "--id", "1"]);
    assert_eq!(output.status.code(), Some(5));
    let result = json_of(&output);
    assert_eq!(
        (result["status"].as_str(), result["code"].as_str()),
        (Some("unknown"), Some("unknown-outcome"))
    );
}

#[test]
fn a_preview_names_the_confirmed_line() {
    let plan = Plan {
        capabilities: standard_caps(),
        replies: vec![
            json!({ "kind": "received", "hidden": false, "dataAgeMs": null, "change": false, "viewRelevant": false }),
            json!({ "kind": "finished", "outcome": { "status": "ok", "summary": "1 task stops" },
                    "chunks": 0, "bytes": 0, "preview": true,
                    "suggest": ["tasks.stop", "--id", "8812", "--yes"] }),
        ],
        ..Plan::default()
    };
    let (output, _) = run("preview", plan, &["tasks.stop", "--id", "8812"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        text(&output.stdout),
        "Preview: 1 task stops. To run it: gecko tasks.stop --id 8812 --yes\n"
    );
}

#[test]
fn rate_limited_sends_are_retried() {
    let plan = Plan {
        capabilities: standard_caps(),
        replies: read_ok(),
        rate_limited: 2,
        ..Plan::default()
    };
    let (output, seen) = run("rate", plan, &["--json", "tasks.list"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(seen.methods, ["status", "send", "send", "send"]);
}

#[test]
fn the_name_is_checked_before_connecting() {
    let (output, seen) = run(
        "bad-name",
        Plan::default(),
        &["--json", "--line", "'quoted' x"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_of(&output)["code"], "usage");
    assert!(seen.methods.is_empty());
}

fn rows_reply() -> Vec<Value> {
    let data = r#"[{"id":7,"title":"Görev","note":null}]"#;
    vec![
        json!({ "kind": "received", "hidden": false, "dataAgeMs": null, "change": false, "viewRelevant": false }),
        json!({ "kind": "chunk", "index": 0, "total": 1, "bytes": data.len(), "data": data }),
        json!({ "kind": "finished", "outcome": { "status": "ok" }, "chunks": 1, "bytes": data.len() }),
    ]
}

#[test]
fn rows_print_as_a_table() {
    let plan = Plan {
        capabilities: standard_caps(),
        replies: rows_reply(),
        ..Plan::default()
    };
    let (output, _) = run("table", plan, &["tasks.list"]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "id  title\n--  -----\n7   Görev\n(empty columns hidden: note)\n"
    );
}

#[test]
fn out_writes_the_result_to_a_file() {
    let plan = Plan {
        capabilities: standard_caps(),
        replies: rows_reply(),
        ..Plan::default()
    };
    let (host, _) = hub(plan);
    let dir = signed_in("out", &host);
    let file = dir.join("result.json");
    let output = gecko(
        &dir,
        &["--json", "--out", file.to_str().unwrap(), "tasks.list"],
    )
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(output.stdout.is_empty());
    let written: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written["data"][0]["title"], "Görev");
}

#[test]
fn a_slow_tab_shows_the_wait_indicator() {
    let plan = Plan {
        capabilities: standard_caps(),
        replies: read_ok(),
        delay_ms: 1000,
        ..Plan::default()
    };
    let (host, _) = hub(plan);
    let dir = signed_in("indicator", &host);
    let output = gecko(&dir, &["tasks.list"])
        .env("GECKO_TEST_TTY", "1")
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let err = text(&output.stderr);
    assert!(err.contains("\r\u{1b}[2Krunning in the tab… "), "{err:?}");
    assert!(err.ends_with("\r\u{1b}[2K"), "{err:?}");

    // With --json nothing is drawn.
    let output = gecko(&dir, &["--json", "tasks.list"])
        .env("GECKO_TEST_TTY", "1")
        .output()
        .unwrap();
    assert!(output.stderr.is_empty(), "{:?}", text(&output.stderr));
}
