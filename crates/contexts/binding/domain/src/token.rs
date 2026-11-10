//! The binding's token over time (attach spec §6, §10.2): when it is renewed, when a
//! renewed one expires, and what the hub's refusals mean. The time comes in as a value.

use std::time::Duration;

use gecko_kernel::{BIND_AGAIN, Failure};
use serde_json::{Map, Value};

use crate::code;

/// A token that expires within this many milliseconds is renewed before it is used.
const RENEW_BEFORE_MS: u64 = 30_000;

/// Whether the token that expires at `expires_at` is renewed at `now` (both ms since the
/// epoch): when it expires within 30 s, or always with `force`.
pub fn needs_refresh(expires_at: u64, now: u64, force: bool) -> bool {
    force || expires_at.saturating_sub(now) <= RENEW_BEFORE_MS
}

/// When the access token in `body` expires, counted from `now` (ms since the epoch).
pub fn expires_at(body: &Map<String, Value>, now: u64) -> u64 {
    let seconds = body
        .get("expires_in")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    now + u64::try_from(bounded(seconds).as_millis()).unwrap_or(0)
}

/// Seconds from the server, bounded: negative or absurd values do not stall the CLI.
fn bounded(value: f64) -> Duration {
    if value.is_nan() {
        return Duration::ZERO;
    }
    Duration::from_secs_f64(value.clamp(0.0, 24.0 * 3600.0))
}

/// What the hub's refusal of an attach code means; `error` is its `error` field, empty
/// when it has none.
pub fn attach_refused(status: u16, error: &str) -> Failure {
    match error {
        "invalid-code" => code::invalid(),
        "no-terminal-access" => {
            Failure::new("no-terminal-access", "this account has no terminal access")
        }
        "too-many-attempts" => Failure::new(
            "too-many-attempts",
            "too many wrong codes from this address; wait a minute, then copy a new code from the tab",
        ),
        "" => Failure::new(
            "failed",
            format!("the hub refused the code (HTTP {status})"),
        ),
        other => Failure::new("failed", format!("the hub refused the code: {other}")),
    }
}

/// A refused refresh (`invalid_grant`, any 4xx): the grant ended with its binding
/// (attach spec §6).
pub fn refresh_refused(status: u16) -> bool {
    (400..500).contains(&status)
}

/// The failure of a refused refresh: the binding ended.
pub fn binding_ended() -> Failure {
    Failure::new("gone", format!("the binding ended; {BIND_AGAIN}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_seconds_are_bounded() {
        assert_eq!(bounded(0.25), Duration::from_millis(250));
        assert_eq!(bounded(-3.0), Duration::ZERO);
        assert_eq!(bounded(1e12), Duration::from_secs(24 * 3600));
        assert_eq!(bounded(f64::NAN), Duration::ZERO);
    }

    #[test]
    fn renewed_within_30_seconds_of_expiry() {
        assert!(!needs_refresh(100_000, 69_999, false));
        assert!(needs_refresh(100_000, 70_000, false));
        assert!(needs_refresh(100_000, 200_000, false));
        assert!(needs_refresh(100_000, 0, true));
    }
}
