//! The port of a hub connection (spec §4.1): what the CLI's rules call on an open
//! connection. A transport (`gecko-hub-client`) implements it; callers take it as a
//! generic parameter (`H: HubConnection`), not as `dyn`.

use std::future::Future;

use serde_json::value::RawValue;

use crate::{CallError, Closed, HubEvent};

/// One short connection to a hub, opened per `gecko` call. Values come back raw: the
/// transport never reads the tab's payload, and the CLI reads the hub's own answers.
pub trait HubConnection {
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
