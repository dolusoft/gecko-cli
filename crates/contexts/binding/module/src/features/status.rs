//! `gecko status`: the binding as the hub sees it, with the time a connection took
//! (a monotonic timer, here; the lines are the domain's).

use std::time::Instant;

use gecko_binding_contract::{HubOpener, Ports, TokenStore};
use gecko_binding_domain::status::{BindingData, HubReach, StatusData, status_outcome};
use gecko_hub_protocol::HubConnection;
use gecko_hub_protocol::User;
use gecko_kernel::{Clock, Failure, Outcome};

/// Gathers the status. Never binds. No binding: exit 0 and says how to bind; hub
/// unreachable: exit 4; `busy` (another gecko call holds the grant's one live
/// connection): the local part and a line that says so, exit 0. `json` is `--json`.
pub async fn status(
    json: bool,
    ports: &Ports<'_, impl Clock, impl TokenStore, impl HubOpener>,
) -> Result<Outcome, Failure> {
    let Some(token) = ports.store.read_token() else {
        let data = StatusData {
            user: None,
            hub: None,
            binding: None,
        };
        return Ok(status_outcome(&data, json, ports.now_ms()));
    };
    let user = User {
        id: token.user.id.clone(),
        name: token.user.name.clone(),
    };
    let (url, level) = (token.hub.clone(), token.level.clone());
    let started = Instant::now();
    let data = match ports.opener.open_with(token).await {
        Ok(session) => {
            let latency = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let data = StatusData {
                user: Some(session.status.user.clone()),
                hub: Some(HubReach {
                    url,
                    reachable: true,
                    latency_ms: Some(latency),
                }),
                binding: session.status.binding.as_ref().map(BindingData::of),
            };
            session.hub.close().await;
            data
        }
        // The 4409 close is the hub's own answer: it is up.
        Err(failure) if failure.code == "busy" => StatusData {
            user: Some(user),
            hub: Some(HubReach {
                url,
                reachable: true,
                latency_ms: None,
            }),
            binding: Some(BindingData::only("unknown", Some(level), Some("busy"))),
        },
        // The refresh was refused: the binding ended, and its token is gone.
        Err(failure) if failure.code == "gone" => StatusData {
            user: Some(user),
            hub: Some(HubReach {
                url,
                reachable: true,
                latency_ms: None,
            }),
            binding: Some(BindingData::only("gone", None, None)),
        },
        Err(failure) => return Err(failure),
    };
    Ok(status_outcome(&data, json, ports.now_ms()))
}
