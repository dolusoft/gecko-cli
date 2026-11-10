//! The command context's contract: what a caller hands the line and help paths besides
//! the hub connection, the reader's stderr and Ctrl+C. The binary fills it from its
//! composition root. No I/O.

use gecko_kernel::{Clock, IdSource, Settings};

/// The values and ports one command call runs with.
#[derive(Debug)]
pub struct Ports<'a, C, D> {
    /// The environment's settings (delivery margin, cancel wait).
    pub settings: &'a Settings,
    /// The wall clock: the run's events carry its time.
    pub clock: &'a C,
    /// Ids of the requests sent to the tab.
    pub ids: &'a D,
    /// This CLI's version, for the tab's `minCli`.
    pub cli_version: &'a str,
}

impl<C: Clock, D: IdSource> Ports<'_, C, D> {
    /// Milliseconds since the epoch, from the clock.
    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    /// A new request id.
    pub fn new_id(&self) -> String {
        self.ids.new_id()
    }
}
