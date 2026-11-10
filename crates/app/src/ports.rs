//! The binary's side of the kernel's ports (spec §4.1): the system clock, request ids,
//! and the values one call is composed of. `main` builds them; the environment is read
//! there and nowhere else.

use std::time::{SystemTime, UNIX_EPOCH};

use gecko_hub_client::proxy::ProxyRule;
use gecko_kernel::{Clock, IdSource, Settings};

/// The operating system's wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
}

/// Request ids: 128 random bits from the standard library's hasher keys.
#[derive(Debug, Clone, Copy, Default)]
pub struct RandomIds;

impl IdSource for RandomIds {
    fn new_id(&self) -> String {
        use std::hash::BuildHasher;
        let random =
            || std::collections::hash_map::RandomState::new().hash_one(SystemClock.now_ms());
        format!("{:016x}{:016x}", random(), random())
    }
}

/// What one call is composed of, besides the streams and Ctrl+C.
#[derive(Debug)]
pub struct Ports<C, D> {
    pub settings: Settings,
    pub clock: C,
    pub ids: D,
    /// This CLI's version, for the tab's `minCli`.
    pub cli_version: &'static str,
    /// The proxy rule every connection of the call follows.
    pub proxy: ProxyRule,
}

impl<C, D> Ports<C, D> {
    /// What the command context takes of them.
    pub fn command(&self) -> gecko_command_contract::Ports<'_, C, D> {
        gecko_command_contract::Ports {
            settings: &self.settings,
            clock: &self.clock,
            ids: &self.ids,
            cli_version: self.cli_version,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_128_bits_of_hex_and_differ() {
        let (a, b) = (RandomIds.new_id(), RandomIds.new_id());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
