//! Narrow SignalR client for the gecko CLI trial (docs/rust-migration.md §7.4).
pub mod connection;
pub mod protocol;
pub mod stream;

pub use connection::{Connection, Event, InvokeError, Options};
pub use protocol::{hub_result, Closed, HubError};
