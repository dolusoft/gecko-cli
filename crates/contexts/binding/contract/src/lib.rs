//! The binding context's contract (spec §4.1): the binding's stored token, the ports the
//! context asks of the world (the token store, the answer to the trust question, the
//! opening of a hub connection, the exchange of an attach code for a token) and what a caller hands the `attach`, `detach` and
//! `status` slices. The binary implements the trust prompt and the opener; the token
//! store is the module's file adapter. No I/O.

use std::future::Future;
use std::io;

use gecko_hub_protocol::{HubConnection, Session};
use gecko_kernel::{Clock, Failure};
use serde::{Deserialize, Serialize};

/// The user the binding acts for, as the hub names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
}

/// The token of the binding, with the hub that issued it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredToken {
    /// The hub's origin, as the attach code named it (`http://127.0.0.1:4400`).
    pub hub: String,
    pub access_token: String,
    /// Milliseconds since the epoch.
    pub access_expires_at: u64,
    pub refresh_token: String,
    pub grant_id: String,
    pub binding_id: String,
    /// The approval level chosen in the tab (`ask-server`, `never-ask`).
    pub level: String,
    pub user: User,
}

/// Where the binding's token and the trusted hub addresses are kept. A binding and its
/// token live and end together (attach spec §6), so there is one token at most.
pub trait TokenStore {
    /// The binding's token; one that cannot be read counts as none.
    fn read_token(&self) -> Option<StoredToken>;
    fn write_token(&self, token: &StoredToken) -> io::Result<()>;
    /// Removes the token; none to remove is fine.
    fn delete_token(&self) -> io::Result<()>;
    /// Whether the user trusted this hub origin before (attach spec §3.3).
    fn trusts(&self, hub: &str) -> bool;
    /// Adds a hub origin to the trusted ones.
    fn trust(&self, hub: &str) -> io::Result<()>;
}

/// The answer to the trust question. The question itself goes out through
/// `Reporter::prompt`; this reads the line the user types after it.
pub trait TrustPrompt {
    /// The typed line; empty when nothing could be read.
    fn answer(&mut self) -> impl Future<Output = String>;
}

/// Opens a hub connection with a binding's token (the binary's composition root does it:
/// the transport, the refresh of a refused token).
pub trait HubOpener {
    type Hub: HubConnection;
    /// Connects with `token` and reads the hub's `status`.
    fn open_with(
        &self,
        token: StoredToken,
    ) -> impl Future<Output = Result<Session<Self::Hub>, Failure>>;
}

/// Spends an attach code's secret at its hub (attach spec §10.2): the hub binds the tab
/// that made the code and answers with the binding's token.
pub trait CodeExchange {
    /// `hub` is the code's checked origin; `clock` dates the token's expiry.
    fn exchange(
        &self,
        hub: &str,
        secret: &str,
        clock: &impl Clock,
    ) -> impl Future<Output = Result<StoredToken, Failure>>;
}

/// The ports one binding call runs with.
#[derive(Debug)]
pub struct Ports<'a, C, S, O> {
    /// The wall clock: token lifetimes and the status's relative times.
    pub clock: &'a C,
    pub store: &'a S,
    pub opener: &'a O,
}

impl<C: Clock, S, O> Ports<'_, C, S, O> {
    /// Milliseconds since the epoch, from the clock.
    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }
}

/// `gecko attach`, as the binary read it: the code's text (from argv or stdin) and
/// what decides whether the trust question may be asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachRequest<'a> {
    /// The code as given, before it is opened.
    pub code: &'a str,
    /// The code came from stdin (`gecko attach -`).
    pub from_stdin: bool,
    /// `--trust-hub`.
    pub trust_hub: bool,
    /// `--json`.
    pub json: bool,
    /// Both stdin and stderr are a terminal (or `GECKO_TEST_TTY=1`).
    pub terminal: bool,
}
