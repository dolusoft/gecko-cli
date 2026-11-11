//! The CLI's side of a hub connection (spec §7.2).
//!
//! A transport carries opaque envelopes; it never reads the tab's payload. Hub errors come
//! back as values (`HubError`), and the end of a connection comes back as a reason
//! (`Closed`) rather than a close code (B18). Two transports: the development hub
//! (WebSocket frames, step 4b) and SignalR (`signalr`, the narrow client chosen in step
//! 4a). Neither reconnects, so a command is never sent twice.
//!
//! The contract types, the address rules and the connection port (`HubConnection`) live
//! in `gecko-hub-protocol`; the types a transport hands back are re-exported here. The
//! development transport implements the port; the SignalR connection does not yet (the
//! CLI does not open it).
//!
//! The modules are private; every name is exported from the crate root. The SignalR
//! names carry a `Signalr` prefix where a bare name would be ambiguous.

mod dev;
mod net;
mod proxy;
mod signalr;
mod tcp;

pub use dev::{DevOptions, DevTransport};
pub use net::http_client;
pub use proxy::ProxyRule;
pub use signalr::{
    Connection as SignalrConnection, Event as SignalrEvent, InvokeError, Options as SignalrOptions,
    hub_result,
};

pub use gecko_hub_protocol::HubConnection as Transport;
pub use gecko_hub_protocol::{CallError, Closed, HubError, HubEvent};
