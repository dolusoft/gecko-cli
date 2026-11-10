//! `gecko attach` (attach spec §3): the code opened, its hub trusted (asked once on a
//! terminal, or `--trust-hub`), the code spent, the previous binding ended, the token
//! kept. A binding starts only from a code the user copied out of a Gecko tab; its token
//! lives and ends with it.

use std::time::Duration;

use gecko_binding_contract::{AttachRequest, HubOpener, Ports, TokenStore, TrustPrompt};
use gecko_binding_domain::attach::{
    agreed, attached, declined, may_ask, trust_question, untrusted,
};
use gecko_hub_client::proxy::ProxyRule;
use gecko_hub_protocol::HubConnection;
use gecko_kernel::{Clock, Failure, Interrupts, Outcome, Reporter};

use crate::adapters::http::Auth;
use crate::code;

/// How long ending the previous binding may take before `attach` goes on without it.
const OLD_BINDING_WAIT: Duration = Duration::from_secs(5);

/// Asks once on the terminal whether to trust a new hub address.
async fn ask_trust(
    hub: &str,
    io: &mut impl Reporter,
    prompt: &mut impl TrustPrompt,
    interrupts: &mut impl Interrupts,
) -> Result<bool, Failure> {
    io.clear_progress();
    io.prompt(&trust_question(hub));
    tokio::select! {
        line = prompt.answer() => {
            io.prompt("\n");
            Ok(agreed(&line))
        }
        () = interrupts.next() => Err(Failure::new("cancelled", "cancelled")),
    }
}

/// Ends the binding of the store, if any: the hub's `detach` when it answers in time,
/// then the token goes either way.
async fn end_old_binding(ports: &Ports<'_, impl Clock, impl TokenStore, impl HubOpener>) -> bool {
    let Some(old) = ports.store.read_token() else {
        return false;
    };
    let _ = tokio::time::timeout(OLD_BINDING_WAIT, async {
        if let Ok(mut session) = ports.opener.open_with(old).await {
            let _ = session.hub.detach().await;
            session.hub.close().await;
        }
    })
    .await;
    let _ = ports.store.delete_token();
    true
}

/// Binds to the tab `request.code` came from. `proxy` is the rule the hub's HTTP
/// routes follow.
pub async fn attach(
    request: &AttachRequest<'_>,
    ports: &Ports<'_, impl Clock, impl TokenStore, impl HubOpener>,
    proxy: &ProxyRule,
    io: &mut impl Reporter,
    prompt: &mut impl TrustPrompt,
    interrupts: &mut impl Interrupts,
) -> Result<Outcome, Failure> {
    let code = code::open(request.code)?;
    io.info(&format!("Connecting to {}", code.hub));
    if !ports.store.trusts(&code.hub) {
        let ask = may_ask(request.json, request.from_stdin, request.terminal);
        let trusted = if request.trust_hub {
            true
        } else if ask {
            ask_trust(&code.hub, io, prompt, interrupts).await?
        } else {
            return Err(untrusted(&code.hub));
        };
        if !trusted {
            return Err(declined(&code.hub));
        }
        ports.store.trust(&code.hub).map_err(|error| {
            Failure::new("failed", format!("cannot save the trusted hub: {error}"))
        })?;
    }
    let auth = Auth::new(&code.hub, proxy.clone())?;
    let token = auth.attach(&code.secret, ports.clock).await?;
    // One binding at a time: once the new one stands, the old one and its token end
    // (attach spec §3.4). A refused code leaves the old binding as it was.
    if end_old_binding(ports).await {
        io.info("The previous binding ended");
    }
    ports
        .store
        .write_token(&token)
        .map_err(|error| Failure::new("failed", format!("cannot save the token: {error}")))?;
    Ok(attached(&token, request.json))
}
