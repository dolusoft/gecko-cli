//! `gecko detach`: the hub ends the binding when it can be reached; the token goes
//! either way.

use gecko_binding_contract::{HubOpener, Ports, TokenStore};
use gecko_binding_domain::detach::{detached, nothing_to_detach};
use gecko_hub_protocol::HubConnection;
use gecko_kernel::{Clock, Failure, Outcome};

pub async fn detach(
    ports: &Ports<'_, impl Clock, impl TokenStore, impl HubOpener>,
) -> Result<Outcome, Failure> {
    let Some(token) = ports.store.read_token() else {
        return Ok(nothing_to_detach());
    };
    let ended = match ports.opener.open_with(token).await {
        Ok(mut session) => {
            let detached = session.hub.detach().await.is_ok();
            session.hub.close().await;
            detached
        }
        // The hub had ended it already.
        Err(failure) => failure.code == "gone",
    };
    ports
        .store
        .delete_token()
        .map_err(|error| Failure::new("failed", format!("cannot delete the token: {error}")))?;
    Ok(detached(ended))
}
