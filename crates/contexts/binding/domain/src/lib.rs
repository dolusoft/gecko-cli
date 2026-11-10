//! The binding context's rules (spec §4.1): the attach code and its hub address
//! (`code`), the trust question and the result of `attach` (`attach`), what `detach`
//! reports (`detach`), the token's lifetime and the hub's refusals (`token`), and the
//! status as a result (`status`). No I/O, no clock: the time comes in as a value
//! (`clippy.toml`).

pub mod attach;
pub mod code;
pub mod detach;
pub mod status;
pub mod token;
