//! An open connection together with the hub's `status` read right after it opened.
//! A value: opening the connection is the binary's (its composition root).

use crate::hub::HubStatus;

/// An open connection and what the hub said about it.
#[derive(Debug)]
pub struct Session<H> {
    pub hub: H,
    pub status: HubStatus,
}
