//! The narrow SignalR client (docs/signalr-trial.md, docs/rust-migration.md §7.4).
//!
//! JSON protocol over WebSockets only, one short connection per `gecko` call, no
//! reconnect. The token travels only in the `Authorization` header. The `Transport`
//! implementation on top of it comes with step 7, once #1974 fixes the method names.

mod connection;
mod protocol;

pub use connection::{Connection, Event, InvokeError, Options};
pub use protocol::hub_result;
