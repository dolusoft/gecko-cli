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

pub mod dev;
pub mod net;
pub mod proxy;
pub mod signalr;
pub mod tcp;

pub use gecko_hub_protocol::HubConnection as Transport;
pub use gecko_hub_protocol::{CallError, Closed, HubError, HubEvent};
