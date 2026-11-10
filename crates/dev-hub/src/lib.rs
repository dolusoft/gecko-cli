//! The development hub behind `gecko dev-hub` (K1): §10.3 of the desktop CLI
//! specification in one process, with fake users. It listens on 127.0.0.1 only. It is
//! compiled into the binary only with the `dev-hub` feature: developer and CI builds
//! have it, release builds do not.

mod hub;
mod random;
mod server;

use std::io::Write;
use std::sync::Arc;

use serde_json::Value;

pub use hub::{Options, Timings, User, is_loopback_origin};
pub use server::{Running, start};

const USAGE: &str = "\
Usage: gecko dev-hub [--port <n>] [--web <origin>] [--public-url <url>]

Starts the development hub on 127.0.0.1. The first stdout line is its address.

  --port <n>          port (default 4400; 0 picks a free one)
  --web <origin>      origin of a local Gecko web server; the one origin the
                      browser may call the hub from (default: any loopback origin)
  --public-url <url>  the hub address written into attach codes (default: the
                      first stdout line)

A Gecko tab makes the attach code; the CLI binds with: gecko attach <code>

Environment: GECKO_DEV_HUB_RATE, GECKO_DEV_HUB_BURST, GECKO_DEV_HUB_USERS,
GECKO_DEV_HUB_TIMINGS, GECKO_DEV_HUB_STDIN (docs/test-hooks.md).
";

/// Runs `gecko dev-hub <args>` until Ctrl+C (or the end of stdin with
/// `GECKO_DEV_HUB_STDIN=1`) and returns the exit code.
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let mut options = match options(args, |name| std::env::var(name).ok()) {
        Ok(Some(options)) => options,
        Ok(None) => {
            let _ = out.write_all(USAGE.as_bytes());
            return 0;
        }
        Err(error) => {
            let _ = writeln!(err, "gecko dev-hub: {error}\nSee: gecko dev-hub --help");
            return 2;
        }
    };
    options.log = Some(Arc::new(|line: &str| {
        let _ = writeln!(std::io::stderr(), "{line}");
    }));
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = writeln!(err, "gecko dev-hub: cannot start: {error}");
            return 1;
        }
    };
    let port = options.port;
    let code = runtime.block_on(async {
        let hub = match start(options).await {
            Ok(hub) => hub,
            Err(error) => {
                let _ = writeln!(
                    err,
                    "gecko dev-hub: cannot listen on 127.0.0.1:{port}: {error}"
                );
                return 1;
            }
        };
        // The first stdout line is the address; tests read it.
        let _ = writeln!(out, "{}", hub.url);
        let _ = out.flush();
        let _ = writeln!(
            err,
            "Bind from a Gecko tab: its attach line runs gecko attach <code>"
        );
        stopped().await;
        hub.close();
        0
    });
    runtime.shutdown_background();
    code
}

/// Ctrl+C, SIGTERM, or the end of stdin when `GECKO_DEV_HUB_STDIN=1`.
async fn stopped() {
    let (stdin_tx, stdin_rx) = tokio::sync::oneshot::channel::<()>();
    let watch_stdin = std::env::var("GECKO_DEV_HUB_STDIN").as_deref() == Ok("1");
    if watch_stdin {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            let _ = stdin_tx.send(());
        });
    }
    let stdin_ended = async {
        if watch_stdin {
            let _ = stdin_rx.await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminated() => {}
        _ = stdin_ended => {}
    }
}

#[cfg(unix)]
async fn terminated() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            term.recv().await;
        }
        Err(_) => std::future::pending().await,
    }
}

#[cfg(not(unix))]
async fn terminated() {
    std::future::pending().await
}

/// The options of `gecko dev-hub`; `None` for `--help`.
fn options(
    args: &[String],
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        let mut value = || -> Result<String, String> {
            inline
                .clone()
                .or_else(|| rest.next().cloned())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match name {
            "--help" | "-h" => return Ok(None),
            "--port" => {
                let text = value()?;
                options.port = text
                    .parse()
                    .map_err(|_| format!("--port: not a port: {text}"))?;
            }
            "--web" => options.web_origin = Some(value()?.trim_end_matches('/').to_owned()),
            "--public-url" => {
                let text = value()?;
                let trimmed = text.trim_end_matches('/');
                let origin = url::Url::parse(trimmed)
                    .ok()
                    .filter(|u| matches!(u.scheme(), "http" | "https"))
                    .map(|u| u.origin().ascii_serialization())
                    .filter(|origin| origin == trimmed)
                    .ok_or_else(|| {
                        format!("--public-url: not an http(s) origin such as https://hub.example: {text}")
                    })?;
                options.public_url = Some(origin);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    let number = |name: &str| -> Result<Option<f64>, String> {
        match env(name) {
            None => Ok(None),
            Some(text) => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && *n > 0.0)
                .map(Some)
                .ok_or_else(|| format!("{name}: not a positive number: {text}")),
        }
    };
    if let Some(rate) = number("GECKO_DEV_HUB_RATE")? {
        options.rate_per_second = rate;
    }
    options.rate_burst = number("GECKO_DEV_HUB_BURST")?;
    if let Some(users) = env("GECKO_DEV_HUB_USERS") {
        options.users = serde_json::from_str(&users)
            .map_err(|error| format!("GECKO_DEV_HUB_USERS: {error}"))?;
    }
    if let Some(timings) = env("GECKO_DEV_HUB_TIMINGS") {
        match serde_json::from_str::<Value>(&timings) {
            Ok(Value::Object(fields)) => options.timings.merge(&fields),
            _ => return Err("GECKO_DEV_HUB_TIMINGS: not a JSON object".into()),
        }
    }
    Ok(Some(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str], env: &[(&str, &str)]) -> Result<Option<Options>, String> {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        options(&args, |name| {
            env.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn options_and_environment() {
        let options = parse(
            &[
                "--port",
                "0",
                "--web=http://localhost:5173/",
                "--public-url",
                "https://hub.example/",
            ],
            &[
                ("GECKO_DEV_HUB_RATE", "1000"),
                (
                    "GECKO_DEV_HUB_USERS",
                    r#"[{"id":"a","name":"Ada","terminal":false}]"#,
                ),
                ("GECKO_DEV_HUB_TIMINGS", r#"{"suspendMs":300,"unloadMs":9}"#),
            ],
        )
        .unwrap()
        .unwrap();
        assert_eq!(options.port, 0);
        assert_eq!(options.web_origin.as_deref(), Some("http://localhost:5173"));
        assert_eq!(options.public_url.as_deref(), Some("https://hub.example"));
        assert_eq!(options.rate_per_second, 1000.0);
        assert_eq!(options.rate_burst, None);
        assert_eq!(options.users[0].terminal, Some(false));
        assert_eq!(options.timings.suspend_ms, 300.0);
        assert_eq!(options.timings.unload_ms, 9.0);
    }

    #[test]
    fn bad_input_is_a_usage_error() {
        assert!(parse(&["--port", "x"], &[]).is_err());
        assert!(parse(&["--port"], &[]).is_err());
        assert!(parse(&["--nope"], &[]).is_err());
        assert!(parse(&["--auto-approve"], &[]).is_err());
        assert!(parse(&["--public-url", "ftp://x"], &[]).is_err());
        assert!(parse(&["--public-url", "https://x/hub"], &[]).is_err());
        assert!(parse(&[], &[("GECKO_DEV_HUB_RATE", "fast")]).is_err());
        assert!(parse(&[], &[("GECKO_DEV_HUB_TIMINGS", "[]")]).is_err());
        assert!(parse(&["--help"], &[]).unwrap().is_none());
    }
}
