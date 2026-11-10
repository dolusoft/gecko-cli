//! One `gecko` call's connection to the hub: the binding's token, the transport for its
//! hub and the hub's `status`. Every path that fails after connecting closes the
//! connection before it returns.

use std::time::{SystemTime, UNIX_EPOCH};

use gecko_hub_client::dev::{DevOptions, DevTransport};
use gecko_hub_client::hub::HubStatus;
use gecko_hub_client::proxy::ProxyRule;
use gecko_hub_client::select::{Choice, choose};
use gecko_hub_client::{CallError, Closed, Transport};
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use url::Url;

use crate::auth::{Auth, BIND_AGAIN};
use crate::config::{Config, StoredToken};
use crate::output::Failure;

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

/// What a command says without a binding: there is no token to use.
pub fn no_binding() -> Failure {
    Failure::new(
        "no-binding",
        "no bound tab; to bind: gecko attach <code> (copy the line from the Gecko tab)",
    )
}

/// Why the connection could not be opened, in the CLI's terms.
pub fn open_failure(closed: Closed) -> Failure {
    match closed {
        Closed::Unauthorized => Failure::new(
            "unauthorized",
            format!("the hub refused the token; {BIND_AGAIN}"),
        ),
        Closed::Busy => Failure::new(
            "busy",
            "another gecko call is running (one live connection per grant)",
        ),
        Closed::Revoked => Failure::new("revoked", format!("the binding ended; {BIND_AGAIN}")),
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

/// `host[:port]` of a hub origin, as the transports take it.
fn host_of(hub: &str) -> Result<String, Failure> {
    let url = Url::parse(hub)
        .map_err(|error| Failure::new("failed", format!("{hub} is not an address: {error}")))?;
    let host = url
        .host_str()
        .ok_or_else(|| Failure::new("failed", format!("{hub} has no host")))?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    })
}

/// Connects with the stored token; without one, `no-binding`.
pub async fn open(config: &Config) -> Result<Session, Failure> {
    let token = config.read_token().ok_or_else(no_binding)?;
    open_with(config, token).await
}

/// Connects with `token`; when the hub refuses it, refreshes it once and tries again.
/// A refused refresh means the binding ended: the token file goes (attach spec §6).
pub async fn open_with(config: &Config, token: StoredToken) -> Result<Session, Failure> {
    let host = host_of(&token.hub)?;
    let forced = std::env::var("GECKO_TRANSPORT").ok();
    if choose(&host, forced.as_deref()) == Choice::SignalR {
        return Err(Failure::new(
            "failed",
            format!(
                "cannot reach {}: the hosted hub is not supported yet; use the development hub",
                token.hub
            ),
        ));
    }
    let auth = Auth::new(&token.hub, ProxyRule::from_env())?;
    let token = auth.current_token(config, token, false).await?;
    let mut hub = match DevTransport::connect(&DevOptions::new(&host, &token.access_token)).await {
        Ok(hub) => hub,
        Err(Closed::Unauthorized) => {
            let token = auth.current_token(config, token, true).await?;
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
        assert_eq!(code(no_binding()), ("no-binding".into(), 4));
        assert!(no_binding().message.contains("gecko attach <code>"));
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

    #[test]
    fn hosts_of_origins() {
        assert_eq!(host_of("http://127.0.0.1:4400").unwrap(), "127.0.0.1:4400");
        assert_eq!(host_of("https://hub.example").unwrap(), "hub.example");
        assert_eq!(host_of("http://[::1]:9").unwrap(), "[::1]:9");
    }
}
