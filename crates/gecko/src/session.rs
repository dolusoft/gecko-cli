//! One `gecko` call's connection to the hub (Node: src/cli/session.ts): the stored
//! token, the transport for the environment and the hub's `status`. Every path that
//! fails after connecting closes the connection before it returns.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gecko_hub_client::dev::{DevOptions, DevTransport};
use gecko_hub_client::hub::{Binding, HubStatus, PairCode};
use gecko_hub_client::proxy::ProxyRule;
use gecko_hub_client::select::{Choice, choose};
use gecko_hub_client::{CallError, Closed, HubEvent, Transport};
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use crate::auth::{Auth, resolve_env};
use crate::browser;
use crate::config::Config;
use crate::interrupt::Interrupts;
use crate::output::{Failure, Io};

/// An open connection and what the hub said about it.
#[derive(Debug)]
pub struct Session {
    pub hub: DevTransport,
    pub status: HubStatus,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Why the connection could not be opened, in the CLI's terms.
pub fn open_failure(closed: Closed) -> Failure {
    match closed {
        Closed::Unauthorized => Failure::new(
            "unauthorized",
            "the token is invalid or the grant was revoked: run gecko login",
        ),
        Closed::Busy => Failure::new(
            "busy",
            "another gecko call is running (one live connection per grant)",
        ),
        Closed::Revoked => Failure::new("revoked", "the CLI grant was revoked"),
        other => Failure::new(
            "unreachable",
            format!("cannot reach the hub: {}", describe(&other)),
        ),
    }
}

/// Why an open connection ended during a call.
pub fn lost_failure(closed: Closed) -> Failure {
    match closed {
        Closed::Unauthorized | Closed::Busy | Closed::Revoked => open_failure(closed),
        other => Failure::new(
            "connection-lost",
            format!("the hub connection was lost: {}", describe(&other)),
        ),
    }
}

fn describe(closed: &Closed) -> String {
    match closed {
        Closed::Handshake(text) | Closed::Transport(text) => text.clone(),
        Closed::Server(Some(text)) => format!("closed by the hub ({text})"),
        Closed::Server(None) => "closed by the hub".to_owned(),
        Closed::ServerTimeout => "the hub stopped answering".to_owned(),
        Closed::Local => "closed".to_owned(),
        Closed::Unauthorized => "unauthorized".to_owned(),
        Closed::Busy => "busy".to_owned(),
        Closed::Revoked => "revoked".to_owned(),
    }
}

/// A failed hub call. A call that timed out may still have taken effect; `effect` says
/// whether that matters (then the outcome is unknown, exit 5).
pub fn call_failure(error: CallError, effect: bool) -> Failure {
    match error {
        CallError::Closed(closed) => lost_failure(closed),
        CallError::Hub(error) => {
            let message = error.message.unwrap_or_else(|| error.code.clone());
            Failure::new(error.code, message)
        }
        CallError::TimedOut if effect => Failure::new(
            "unknown-outcome",
            "the hub did not answer in time; the outcome is unknown (check with gecko status)",
        ),
        CallError::TimedOut => Failure::new("unreachable", "the hub did not answer in time"),
    }
}

fn read<T: DeserializeOwned>(raw: &RawValue, what: &str) -> Result<T, Failure> {
    serde_json::from_str(raw.get()).map_err(|error| {
        Failure::new(
            "failed",
            format!("the hub's {what} answer is not understood: {error}"),
        )
    })
}

/// Connects with the stored token; when the hub refuses it, refreshes it once and tries
/// again.
pub async fn open(env: &str, config: &Config) -> Result<Session, Failure> {
    let host = resolve_env(env)?;
    let forced = std::env::var("GECKO_TRANSPORT").ok();
    if choose(&host, forced.as_deref()) == Choice::SignalR {
        return Err(Failure::new(
            "failed",
            format!(
                "the Rust build cannot reach {host} yet (the SignalR hub follows in step 4b); use the Node build"
            ),
        ));
    }
    let auth = Auth::new(env, ProxyRule::from_env())?;
    let token = auth.current_token(config, false).await?;
    let mut hub = match DevTransport::connect(&DevOptions::new(&host, &token.access_token)).await {
        Ok(hub) => hub,
        Err(Closed::Unauthorized) => {
            let token = auth.current_token(config, true).await?;
            DevTransport::connect(&DevOptions::new(&host, &token.access_token))
                .await
                .map_err(open_failure)?
        }
        Err(closed) => return Err(open_failure(closed)),
    };
    let status = match hub.status().await {
        Ok(raw) => read::<HubStatus>(&raw, "status"),
        Err(error) => Err(call_failure(error, false)),
    };
    match status {
        Ok(status) => Ok(Session { hub, status }),
        Err(failure) => {
            hub.close().await;
            Err(failure)
        }
    }
}

/// Pairs the grant with a tab (§5.2): a code from the hub, the pairing page in the
/// browser, then the user's approval there. Two minutes at most, on the hub's clock.
pub async fn pair(
    session: &mut Session,
    io: &mut Io<'_>,
    interrupts: &mut Interrupts,
) -> Result<Binding, Failure> {
    let raw = session
        .hub
        .pair()
        .await
        .map_err(|error| call_failure(error, false))?;
    let code: PairCode = read(&raw, "pair")?;
    let opened = browser::open(&code.url);
    io.info(&if opened {
        format!(
            "Code: {} - approve the binding in the Gecko tab that opened in the browser",
            code.code
        )
    } else {
        format!("Code: {} - open {}", code.code, code.url)
    });
    io.info(&format!(
        "(or in the terminal of an open Gecko tab: cli.attach {})",
        code.code
    ));

    let wait = code
        .expires_at
        .saturating_sub(now_ms())
        .clamp(1000, 120_000)
        + 1000;
    let deadline = tokio::time::sleep(Duration::from_millis(wait));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            event = session.hub.next_event() => match event {
                Ok(HubEvent::Paired(raw)) => return read(&raw, "paired"),
                Ok(HubEvent::PairRefused(code)) if code == "other-account" => {
                    // The code stays valid; the user can switch accounts (§5.5).
                    io.info("another account is signed in to the browser; approve the code in a tab signed in with this CLI's account");
                }
                Ok(HubEvent::PairRefused(code)) => {
                    let message = if code == "no-terminal-access" {
                        "this account has no terminal access".to_owned()
                    } else {
                        format!("the binding was refused: {code}")
                    };
                    let code = if code.is_empty() { "refused".to_owned() } else { code };
                    return Err(Failure::new(code, message));
                }
                Ok(_) => {}
                Err(closed) => return Err(lost_failure(closed)),
            },
            () = &mut deadline => {
                return Err(Failure::new("pair-timeout", "the binding was not approved within 2 minutes"));
            }
            () = interrupts.next() => return Err(Failure::new("cancelled", "cancelled")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::exit_code_of;
    use gecko_hub_client::HubError;

    #[test]
    fn closed_reasons_have_their_codes() {
        let code = |f: Failure| (f.code.clone(), exit_code_of(&f.code));
        assert_eq!(
            code(open_failure(Closed::Unauthorized)),
            ("unauthorized".into(), 6)
        );
        assert_eq!(code(open_failure(Closed::Busy)), ("busy".into(), 4));
        assert_eq!(code(open_failure(Closed::Revoked)), ("revoked".into(), 6));
        assert_eq!(
            code(open_failure(Closed::Transport("refused".into()))),
            ("unreachable".into(), 4)
        );
        assert_eq!(
            code(lost_failure(Closed::Server(None))),
            ("connection-lost".into(), 4)
        );
        assert_eq!(code(lost_failure(Closed::Busy)), ("busy".into(), 4));
    }

    #[test]
    fn call_failures() {
        let hub = CallError::Hub(HubError {
            code: "no-binding".into(),
            message: None,
            reason: None,
            retry_after_ms: None,
        });
        assert_eq!(
            call_failure(hub, false),
            Failure::new("no-binding", "no-binding")
        );
        assert_eq!(
            call_failure(CallError::TimedOut, true).code,
            "unknown-outcome"
        );
        assert_eq!(call_failure(CallError::TimedOut, false).code, "unreachable");
    }
}
