//! What the CLI's rules ask of the world, as traits the binary implements (spec §4.1):
//! the reader's stderr (`Reporter`), Ctrl+C (`Interrupts`), the wall clock (`Clock`)
//! and request ids (`IdSource`). `Settings` is a value: the binary reads it from the
//! environment once, in its composition root.

use std::future::Future;
use std::time::Duration;

/// Where the reader sees what a call does: lines on stderr and the one-line wait
/// indicator. The result itself is not written through it.
pub trait Reporter {
    /// A line for the reader on stderr (progress, codes, hints).
    fn info(&mut self, text: &str);
    /// A question on stderr, without a line end: the answer is typed after it.
    fn prompt(&mut self, text: &str);
    /// Draws the wait indicator over its previous text; nothing when it may not be drawn.
    fn progress(&mut self, text: &str);
    /// Removes the indicator line, if one is shown.
    fn clear_progress(&mut self);
    /// Whether the wait indicator may be drawn (a terminal, no `--json`).
    fn shows_progress(&self) -> bool;
    /// `--json`: the result is one JSON object.
    fn json(&self) -> bool;
    /// The terminal's width when stdout is one (tables are cut to it).
    fn columns(&self) -> Option<usize>;
}

/// Ctrl+C, counted.
pub trait Interrupts {
    /// Resolves at the next interrupt not yet seen, even one that arrived before the
    /// call; so an interrupt between two waits is never lost.
    fn next(&mut self) -> impl Future<Output = ()>;
}

/// The wall clock.
pub trait Clock {
    /// Milliseconds since the epoch.
    fn now_ms(&self) -> u64;
}

/// Ids of the requests sent to the tab.
pub trait IdSource {
    /// A new id, unique enough that two requests never share one.
    fn new_id(&self) -> String;
}

/// The environment's settings the CLI's rules read (docs/test-hooks.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `GECKO_TRANSPORT`: the transport forced for any address (`dev`).
    pub transport: Option<String>,
    /// `GECKO_TEST_TTY=1`: the streams count as a terminal.
    pub test_tty: bool,
    /// `GECKO_DELIVERY_MARGIN_MS`: added to the hub's delivery window.
    pub delivery_margin: Duration,
    /// `GECKO_CANCEL_WAIT_MS`: how long the tab's answer to a cancel is awaited (§6.6).
    pub cancel_wait: Duration,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            transport: None,
            test_tty: false,
            delivery_margin: Duration::from_secs(2),
            cancel_wait: Duration::from_secs(3),
        }
    }
}

impl Settings {
    /// The settings from a variable lookup; a missing or unreadable value keeps its
    /// default.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Settings {
        let defaults = Settings::default();
        let millis = |name: &str, default: Duration| {
            get(name)
                .and_then(|v| v.parse::<u64>().ok())
                .map_or(default, Duration::from_millis)
        };
        Settings {
            transport: get("GECKO_TRANSPORT"),
            test_tty: get("GECKO_TEST_TTY").is_some_and(|v| v == "1"),
            delivery_margin: millis("GECKO_DELIVERY_MARGIN_MS", defaults.delivery_margin),
            cancel_wait: millis("GECKO_CANCEL_WAIT_MS", defaults.cancel_wait),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_from_a_lookup() {
        assert_eq!(Settings::from_lookup(|_| None), Settings::default());
        let set = Settings::from_lookup(|name| {
            match name {
                "GECKO_TRANSPORT" => Some("dev"),
                "GECKO_TEST_TTY" => Some("1"),
                "GECKO_DELIVERY_MARGIN_MS" => Some("250"),
                "GECKO_CANCEL_WAIT_MS" => Some("soon"),
                _ => None,
            }
            .map(str::to_owned)
        });
        assert_eq!(set.transport.as_deref(), Some("dev"));
        assert!(set.test_tty);
        assert_eq!(set.delivery_margin, Duration::from_millis(250));
        assert_eq!(set.cancel_wait, Duration::from_secs(3));
        assert!(!Settings::from_lookup(|_| Some("0".to_owned())).test_tty);
    }
}
