//! One `gecko` call's connection to the hub: the binding's token, the transport for its
//! hub and the hub's `status`. Every path that fails after connecting closes the
//! connection before it returns. What a closed connection means as a failure is
//! `gecko_hub_protocol::failure`.

use gecko_hub_client::dev::{DevOptions, DevTransport};
use gecko_hub_protocol::address::{Choice, choose};
use gecko_hub_protocol::failure::{call_failure, open_failure};
use gecko_hub_protocol::hub::HubStatus;
use gecko_hub_protocol::{Closed, HubConnection};
use gecko_kernel::{Clock, Failure, IdSource};
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use url::Url;

use crate::auth::Auth;
use crate::config::{Config, StoredToken};
use crate::ports::Ports;

/// An open connection and what the hub said about it.
#[derive(Debug)]
pub struct Session<H> {
    pub hub: H,
    pub status: HubStatus,
}

/// What a command says without a binding: there is no token to use.
pub fn no_binding() -> Failure {
    Failure::new(
        "no-binding",
        "no bound tab; to bind: gecko attach <code> (copy the line from the Gecko tab)",
    )
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
pub async fn open(
    config: &Config,
    ports: &Ports<impl Clock, impl IdSource>,
) -> Result<Session<DevTransport>, Failure> {
    let token = config.read_token().ok_or_else(no_binding)?;
    open_with(config, token, ports).await
}

/// Connects with `token`; when the hub refuses it, refreshes it once and tries again.
/// A refused refresh means the binding ended: the token file goes (attach spec §6).
pub async fn open_with(
    config: &Config,
    token: StoredToken,
    ports: &Ports<impl Clock, impl IdSource>,
) -> Result<Session<DevTransport>, Failure> {
    let host = host_of(&token.hub)?;
    if choose(&host, ports.settings.transport.as_deref()) == Choice::SignalR {
        return Err(Failure::new(
            "failed",
            format!(
                "cannot reach {}: the hosted hub is not supported yet; use the development hub",
                token.hub
            ),
        ));
    }
    let auth = Auth::new(&token.hub, ports.proxy.clone())?;
    let clock = &ports.clock;
    let token = auth.current_token(config, token, false, clock).await?;
    let mut hub = match DevTransport::connect(&DevOptions::new(&host, &token.access_token)).await {
        Ok(hub) => hub,
        Err(Closed::Unauthorized) => {
            let token = auth.current_token(config, token, true, clock).await?;
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
    use gecko_kernel::exit_code_of;

    #[test]
    fn no_binding_has_its_code() {
        assert_eq!(exit_code_of(&no_binding().code), 4);
        assert_eq!(no_binding().code, "no-binding");
        assert!(no_binding().message.contains("gecko attach <code>"));
    }

    #[test]
    fn hosts_of_origins() {
        assert_eq!(host_of("http://127.0.0.1:4400").unwrap(), "127.0.0.1:4400");
        assert_eq!(host_of("https://hub.example").unwrap(), "hub.example");
        assert_eq!(host_of("http://[::1]:9").unwrap(), "[::1]:9");
    }
}
