//! The binding context's use cases (spec §4.1): `attach`, `detach` and `status`
//! (`features`). The token store, the trust prompt and the opening of a hub connection
//! come in `gecko_binding_contract` ports; the reader's stderr as a `Reporter`, Ctrl+C
//! as `Interrupts`. The token file and the hub's HTTP routes are this crate's adapters
//! (`adapters::fs`, `adapters::http`); I/O lives only there. The rules are the domain's.

pub mod adapters;
mod code;
pub mod features;

pub use features::attach::attach;
pub use features::detach::detach;
pub use features::status::status;
pub use gecko_binding_contract::{AttachRequest, Ports};
