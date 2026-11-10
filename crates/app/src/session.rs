//! One `gecko` call's connection to the hub: the binding's token, the transport for its
//! hub and the hub's `status`. Every path that fails after connecting closes the
//! connection before it returns. What a closed connection means as a failure is
//! `gecko_hub_protocol::failure`; the open connection with its status is
//! `gecko_hub_protocol::Session`.

use gecko_binding_contract::{StoredToken, TokenStore};
use gecko_binding_module::adapters::http::Auth;
use gecko_hub_client::dev::{DevOptions, DevTransport};
use gecko_hub_protocol::address::{Choice, choose};
use gecko_hub_protocol::failure::{call_failure, open_failure};
use gecko_hub_protocol::hub::HubStatus;
use gecko_hub_protocol::{Closed, HubConnection, Session};
use gecko_kernel::{Clock, Failure, IdSource, no_binding};
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use url::Url;

use crate::ports::Ports;

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
    store: &impl TokenStore,
    ports: &Ports<impl Clock, impl IdSource>,
) -> Result<Session<DevTransport>, Failure> {
    let token = store.read_token().ok_or_else(no_binding)?;
    open_with(store, token, ports).await
}

/// Connects with `token`; when the hub refuses it, refreshes it once and tries again.
/// A refused refresh means the binding ended: the token goes (attach spec §6).
pub async fn open_with(
    store: &impl TokenStore,
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
    let token = auth.current_token(store, token, false, clock).await?;
    let mut hub = match DevTransport::connect(&DevOptions::new(&host, &token.access_token)).await {
        Ok(hub) => hub,
        Err(Closed::Unauthorized) => {
            let token = auth.current_token(store, token, true, clock).await?;
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

    #[test]
    fn hosts_of_origins() {
        assert_eq!(host_of("http://127.0.0.1:4400").unwrap(), "127.0.0.1:4400");
        assert_eq!(host_of("https://hub.example").unwrap(), "hub.example");
        assert_eq!(host_of("http://[::1]:9").unwrap(), "[::1]:9");
        assert!(host_of("not an address").is_err());
    }

    #[test]
    fn an_answer_not_understood_fails() {
        let raw = RawValue::from_string("[1]".to_owned()).unwrap();
        let failure = read::<HubStatus>(&raw, "status").unwrap_err();
        assert_eq!(failure.code, "failed");
        assert!(
            failure
                .message
                .starts_with("the hub's status answer is not understood: "),
            "{}",
            failure.message
        );
    }
}
