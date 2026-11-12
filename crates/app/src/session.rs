//! One `gecko` call's connection to the hub: the binding's token, the transport for its
//! hub and the hub's `status`. Every path that fails after connecting closes the
//! connection before it returns. What a closed connection means as a failure is
//! `gecko_hub_protocol` (`failure.rs`); the open connection with its status is
//! `gecko_hub_protocol::Session`.

use gecko_binding_contract::{StoredToken, TokenStore};
use gecko_binding_module::Auth;
use std::time::Duration;

use gecko_hub_client::{DevOptions, DevTransport, Hub, ProxyRule};
use gecko_hub_client::{SignalrOptions, SignalrTransport};
use gecko_hub_protocol::HubStatus;
use gecko_hub_protocol::{Choice, choose};
use gecko_hub_protocol::{Closed, HubConnection, Session};
use gecko_hub_protocol::{call_failure, open_failure};
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

/// The hosted hub's CLI endpoint behind the gateway (#1974 KA-5).
const SIGNALR_PATH: &str = "/api/v0/layouts/command-relay/cli";

/// The hosted hub pings every 4 s (#1974); the CLI pings as often.
const SIGNALR_KEEP_ALIVE: Duration = Duration::from_secs(4);

/// The SignalR endpoint of a hub origin.
fn signalr_url(hub: &str) -> String {
    format!("{}{SIGNALR_PATH}", hub.trim_end_matches('/'))
}

/// One connection attempt with `access_token`, by the transport `choice` picked.
async fn connect(
    choice: Choice,
    hub: &str,
    host: &str,
    access_token: &str,
    proxy: &ProxyRule,
) -> Result<Hub, Closed> {
    match choice {
        Choice::Dev => Ok(Hub::Dev(
            DevTransport::connect(&DevOptions::new(host, access_token)).await?,
        )),
        Choice::SignalR => {
            let mut options = SignalrOptions::new(signalr_url(hub), access_token);
            options.keep_alive = SIGNALR_KEEP_ALIVE;
            options.proxy = proxy.clone();
            Ok(Hub::Signalr(SignalrTransport::connect(&options).await?))
        }
    }
}

/// Connects with the stored token; without one, `no-binding`.
pub async fn open(
    store: &impl TokenStore,
    ports: &Ports<impl Clock, impl IdSource>,
) -> Result<Session<Hub>, Failure> {
    let token = store.read_token().ok_or_else(no_binding)?;
    open_with(store, token, ports).await
}

/// Connects with `token`; when the hub refuses it, refreshes it once and tries again.
/// A refused refresh means the binding ended: the token goes (attach spec §6).
pub async fn open_with(
    store: &impl TokenStore,
    token: StoredToken,
    ports: &Ports<impl Clock, impl IdSource>,
) -> Result<Session<Hub>, Failure> {
    let host = host_of(&token.hub)?;
    let choice = choose(&host, ports.settings.transport.as_deref());
    let proxy = &ports.proxy;
    let auth = Auth::new(&token.hub, ports.proxy.clone())?;
    let clock = &ports.clock;
    let token = auth.current_token(store, token, false, clock).await?;
    let mut hub = match connect(choice, &token.hub, &host, &token.access_token, proxy).await {
        Ok(hub) => hub,
        Err(Closed::Unauthorized) => {
            let token = auth.current_token(store, token, true, clock).await?;
            connect(choice, &token.hub, &host, &token.access_token, proxy)
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
    fn the_signalr_endpoint_is_behind_the_gateway() {
        assert_eq!(
            signalr_url("https://gecko.example"),
            "https://gecko.example/api/v0/layouts/command-relay/cli"
        );
        assert_eq!(
            signalr_url("https://gecko.example/"),
            "https://gecko.example/api/v0/layouts/command-relay/cli"
        );
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
