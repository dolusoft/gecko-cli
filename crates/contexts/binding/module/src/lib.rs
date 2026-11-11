//! The binding context's use cases (spec §4.1): `attach`, `detach` and `status`
//! (`features`). The token store, the trust prompt, the opening of a hub connection and
//! the exchange of an attach code come in `gecko_binding_contract` ports; the reader's
//! stderr as a `Reporter`, Ctrl+C as `Interrupts`. The token file and the hub's HTTP
//! routes are this crate's adapters (`Config`, `Auth`, `HttpExchange`); I/O lives only
//! there. The rules are the domain's.

mod adapters;
mod code;
mod features;

pub use adapters::fs::Config;
pub use adapters::http::{Auth, HttpExchange};
pub use features::detach::detach;
pub use features::status::status;
pub use gecko_binding_contract::{AttachRequest, Ports};
use gecko_binding_contract::{HubOpener, TokenStore, TrustPrompt};
use gecko_hub_client::ProxyRule;
use gecko_kernel::{Clock, Failure, Interrupts, Outcome, Reporter};

/// `gecko attach`: binds to the tab `request.code` came from. `proxy` is the rule the
/// hub's HTTP routes follow; the code is spent through [`HttpExchange`].
pub async fn attach(
    request: &AttachRequest<'_>,
    ports: &Ports<'_, impl Clock, impl TokenStore, impl HubOpener>,
    proxy: &ProxyRule,
    io: &mut impl Reporter,
    prompt: &mut impl TrustPrompt,
    interrupts: &mut impl Interrupts,
) -> Result<Outcome, Failure> {
    let exchange = HttpExchange::new(proxy.clone());
    features::attach::attach(request, ports, &exchange, io, prompt, interrupts).await
}
