//! `gecko login` and `gecko logout` as processes, against a scripted OAuth server.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

/// One request the server saw: path and form body.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    body: String,
}

type Script = Box<dyn FnMut(&str, &str) -> (u16, Value) + Send>;

/// Serves `script` on 127.0.0.1 until the test ends; returns `host:port` and the log.
fn server(mut script: Script) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = listener.local_addr().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            let path = first.split(' ').nth(1).unwrap_or("").to_owned();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(value) = lower.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();
            let body = String::from_utf8(body).unwrap();
            log.lock().unwrap().push(Seen {
                path: path.clone(),
                body: body.clone(),
            });
            let (status, answer) = script(&path, &body);
            let text = answer.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    (host, seen)
}

fn start(host: &str) -> Value {
    json!({
        "device_code": "dev-code-1",
        "user_code": "ABCD-EFGH",
        "verification_uri": format!("http://{host}/cli-login"),
        "verification_uri_complete": format!("http://{host}/cli-login#ABCD-EFGH"),
        "expires_in": 60,
        "interval": 0.01
    })
}

fn tokens() -> Value {
    json!({
        "access_token": "at-1",
        "token_type": "Bearer",
        "expires_in": 900,
        "refresh_token": "rt-1",
        "refresh_expires_in": 86400,
        "grant_id": "g_1",
        "user": { "id": "u1", "name": "Zahit" }
    })
}

/// A device flow whose token polls answer with `polls` in turn.
fn flow(host_cell: Arc<Mutex<String>>, polls: Vec<(u16, Value)>) -> Script {
    let mut polls = polls.into_iter();
    Box::new(move |path, _| match path {
        "/oauth/device_authorization" => (200, start(&host_cell.lock().unwrap())),
        "/oauth/token" => polls
            .next()
            .unwrap_or((400, json!({ "error": "authorization_pending" }))),
        "/oauth/revoke" => (200, json!({})),
        _ => (404, json!({ "error": "not_found" })),
    })
}

fn scripted(polls: Vec<(u16, Value)>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let cell = Arc::new(Mutex::new(String::new()));
    let (host, seen) = server(flow(cell.clone(), polls));
    *cell.lock().unwrap() = host.clone();
    (host, seen)
}

fn config_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("login")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn gecko(dir: &PathBuf, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gecko"));
    command
        .args(args)
        .env("GECKO_CONFIG_DIR", dir)
        .env("GECKO_BROWSER", "none")
        .env_remove("GECKO_TEST_INTERRUPT_STDIN")
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .stdin(Stdio::null());
    command
}

fn run(dir: &PathBuf, args: &[&str]) -> Output {
    gecko(dir, args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn login_polls_until_approved_and_saves_the_token() {
    let (host, seen) = scripted(vec![
        (400, json!({ "error": "authorization_pending" })),
        (400, json!({ "error": "slow_down", "interval": 0.02 })),
        (200, tokens()),
    ]);
    let dir = config_dir("approved");
    let output = run(&dir, &["login", "--env", &host]);
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(text(&output.stdout), format!("Signed in: Zahit ({host})\n"));
    assert!(
        stderr.contains(&format!(
            "Code: ABCD-EFGH - open http://{host}/cli-login and enter the code"
        )),
        "{stderr}"
    );

    let key = host.replace(':', "_");
    let token: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join(format!("token-{key}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(token["env"], host.as_str());
    assert_eq!(token["accessToken"], "at-1");
    assert_eq!(token["refreshToken"], "rt-1");
    assert_eq!(token["grantId"], "g_1");
    assert_eq!(token["user"]["name"], "Zahit");
    assert!(token["accessExpiresAt"].as_u64().unwrap() > 1_700_000_000_000);
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).unwrap()).unwrap();
    assert_eq!(config["env"], host.as_str());

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].path, "/oauth/device_authorization");
    assert_eq!(seen[0].body, "client_id=gecko-cli");
    assert_eq!(seen.len(), 4);
    assert_eq!(
        seen[1].body,
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code&device_code=dev-code-1&client_id=gecko-cli"
    );
}

#[test]
fn login_json_is_one_ascii_object() {
    let (host, _) = scripted(vec![(200, tokens())]);
    let dir = config_dir("json");
    let output = run(&dir, &["--json", "login", "--env", &host]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = text(&output.stdout);
    assert!(stdout.is_ascii());
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["status"], "ok");
    assert_eq!(value["summary"], format!("Signed in: Zahit ({host})"));
}

#[test]
fn login_failures_have_their_exit_codes() {
    for (poll, code, error) in [
        ((400, json!({ "error": "access_denied" })), 3, "not-allowed"),
        (
            (400, json!({ "error": "expired_token" })),
            6,
            "token-expired",
        ),
        ((400, json!({ "error": "invalid_grant" })), 1, "failed"),
    ] {
        let (host, _) = scripted(vec![poll]);
        let dir = config_dir(error);
        let output = run(&dir, &["--json", "login", "--env", &host]);
        assert_eq!(output.status.code(), Some(code), "{error}");
        let value: Value = serde_json::from_str(&text(&output.stdout)).unwrap();
        assert_eq!(value["code"], error);
        assert_eq!(value["exitCode"], code);
        assert!(!dir.join("config.json").exists());
    }
}

#[test]
fn login_start_refused_is_failed() {
    let (host, _) = server(Box::new(|_, _| (500, json!({}))));
    let output = run(&config_dir("refused"), &["login", "--env", &host]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("error (failed): could not start the sign-in (HTTP 500)")
    );
}

#[test]
fn nothing_listening_is_unreachable() {
    let free = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = free.local_addr().unwrap().to_string();
    drop(free);
    let output = run(&config_dir("unreachable"), &["login", "--env", &host]);
    assert_eq!(output.status.code(), Some(4));
    assert!(text(&output.stderr).contains("error (unreachable): cannot reach"));
}

#[test]
fn environments_by_alias() {
    let output = run(&config_dir("alias-missing"), &["login", "--env", "prod"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("define GECKO_ENV_PROD"));

    let (host, _) = scripted(vec![(200, tokens())]);
    let dir = config_dir("alias");
    let output = gecko(&dir, &["login", "--env", "stage"])
        .env("GECKO_ENV_STAGE", &host)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "Signed in: Zahit (stage)\n");
    assert!(dir.join("token-stage.json").exists());
}

#[test]
fn no_environment_is_not_signed_in() {
    let output = run(&config_dir("no-env"), &["login"]);
    assert_eq!(output.status.code(), Some(6));
    let output = run(&config_dir("no-env"), &["logout"]);
    assert_eq!(output.status.code(), Some(6));
}

#[test]
fn logout_revokes_and_deletes() {
    let (host, seen) = scripted(vec![(200, tokens())]);
    let dir = config_dir("logout");
    assert_eq!(run(&dir, &["login", "--env", &host]).status.code(), Some(0));
    let key = host.replace(':', "_");
    std::fs::write(dir.join(format!("capabilities-{key}.json")), "{}").unwrap();

    // The environment comes from the last login.
    let output = run(&dir, &["logout"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(text(&output.stdout), format!("Signed out ({host})\n"));
    assert!(!dir.join(format!("token-{key}.json")).exists());
    assert!(!dir.join(format!("capabilities-{key}.json")).exists());
    let revoke = seen
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.path == "/oauth/revoke")
        .cloned()
        .unwrap();
    assert_eq!(revoke.body, "token=rt-1&token_type_hint=refresh_token");

    // Again: nothing to revoke, still fine.
    assert_eq!(run(&dir, &["logout"]).status.code(), Some(0));
}

#[test]
fn interrupt_ends_the_wait() {
    let (host, _) = scripted(vec![]);
    let dir = config_dir("interrupt");
    let mut child = gecko(&dir, &["--json", "login", "--env", &host])
        .env("GECKO_TEST_INTERRUPT_STDIN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.as_mut().unwrap().write_all(b"x").unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    let value: Value = serde_json::from_str(&text(&output.stdout)).unwrap();
    assert_eq!(value["status"], "cancelled");
    assert_eq!(value["code"], "cancelled");
}

#[test]
fn own_command_help() {
    let output = run(&config_dir("help"), &["login", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).starts_with("Usage: gecko login [--env <env>]"));
}
