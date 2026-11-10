//! The CLI's side of a hub connection (spec §7.2).
//!
//! A transport carries opaque envelopes; it never reads the tab's payload. Hub errors come
//! back as values (`HubError`), and the end of a connection comes back as a reason
//! (`Closed`) rather than a close code (B18). Two transports: the development hub
//! (WebSocket frames, step 4b) and SignalR (`signalr`, the narrow client chosen in step
//! 4a). Neither reconnects, so a command is never sent twice.
//!
//! The contract types and the address rules live in `gecko-hub-protocol`; they are
//! re-exported here under their old paths.

pub mod dev;
pub mod net;
pub mod proxy;
pub mod signalr;
pub mod tcp;

use std::future::Future;

pub use gecko_hub_protocol::address as select;
pub use gecko_hub_protocol::{CallError, Closed, HubError, HubEvent, hub};
use serde_json::value::RawValue;

/// One short connection to a hub, opened per `gecko` call. Values come back raw: the
/// transport never reads the tab's payload, and the CLI reads the hub's own answers.
pub trait Transport {
    /// `status`: who is signed in and what the binding holds (capabilities, where).
    fn status(&mut self) -> impl Future<Output = Result<Box<RawValue>, CallError>>;
    /// `send`: one envelope to the bound tab. The hub stamps and routes it unread; the
    /// answer is the stamp (`stampedAt`, `expiresAt`).
    fn send(
        &mut self,
        envelope: &RawValue,
    ) -> impl Future<Output = Result<Box<RawValue>, CallError>>;
    /// `detach`: removes the binding.
    fn detach(&mut self) -> impl Future<Output = Result<(), CallError>>;
    /// The next event from the hub, or why the connection ended.
    fn next_event(&mut self) -> impl Future<Output = Result<HubEvent, Closed>>;
    /// Closes the connection and reports how it ended.
    fn close(self) -> impl Future<Output = Closed>;
}
