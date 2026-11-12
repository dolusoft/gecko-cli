//! The narrow SignalR client (spec §7.4).
//!
//! JSON protocol over WebSockets only, one short connection per `gecko` call, no
//! reconnect. The token travels only in the `Authorization` header. `Transport` is the
//! CLI's port on top of it, with the hub method names of #1974.

mod connection;
mod protocol;
mod transport;

pub use connection::{Connection, Event, InvokeError, Options};
pub use protocol::hub_result;
pub use transport::Transport;
