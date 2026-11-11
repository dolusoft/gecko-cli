//! The rules of a line for the bound tab (§5.1–§5.3, §5.7, §5.8): the checks that need
//! no network, the binding to send to, the tab's capability list against this CLI, the
//! pre-filter and the delivery window.
//!
//! The pre-filter is not a permission; the tab decides. It keeps two promises: a
//! sensitive line never leaves the CLI, and a command the tab does not list never
//! reaches it with its arguments (§5.2).

use std::time::Duration;

use gecko_hub_protocol::{Binding, BindingState, HubStatus};
use gecko_kernel::{Failure, Outcome, gone_message, no_binding};
use gecko_wire::{Capabilities, read_parts, version_below};
use gecko_wire::{HelpResult, Status};
use gecko_wire::{Input, check_limits, command_name};
use serde::Deserialize;
use serde_json::Value;

/// The capability list the hub keeps for the binding: `{ revision, parts }`.
#[derive(Deserialize)]
struct Published {
    parts: Vec<String>,
}

fn too_old() -> Failure {
    Failure::new(
        "unsupported-version",
        "the Gecko tab is too old for this CLI; reload the page or wait for the environment to be updated",
    )
}

/// The binding's capability list, checked against what this CLI (`cli_version`)
/// needs (§5.7, §5.8).
pub fn capabilities_of(binding: &Binding, cli_version: &str) -> Result<Capabilities, Failure> {
    let unreadable = || {
        Failure::new(
            "tab-unreachable",
            "the bound tab has not published its command list; reload the Gecko page",
        )
    };
    let raw = binding.capabilities.as_ref().ok_or_else(unreadable)?;
    let value: Value = serde_json::from_str(raw.get()).map_err(|_| unreadable())?;
    if value.get("kind").is_some() {
        // The list as a `capabilities` reply: a tab from before command-console 1.7.0.
        return Err(too_old());
    }
    let published: Published = serde_json::from_value(value).map_err(|_| unreadable())?;
    let caps = read_parts(&published.parts).map_err(|_| unreadable())?;
    if !caps.missing_features().is_empty() || caps.envelope().is_none() {
        return Err(too_old());
    }
    if let Some(min) = &caps.min_cli
        && version_below(cli_version, min) == Some(true)
    {
        return Err(Failure::new(
            "unsupported-version",
            format!(
                "this gecko ({cli_version}) is older than the tab supports ({min}); update gecko"
            ),
        ));
    }
    Ok(caps)
}

/// The binding to send to, or why there is none. A suspended one counts: while its page
/// comes back, the hub holds the send for a few seconds (attach spec §4).
pub fn live_binding(status: &HubStatus) -> Result<&Binding, Failure> {
    match &status.binding {
        None => Err(no_binding()),
        Some(b) if b.state == BindingState::Gone => {
            Err(Failure::new("gone", gone_message(b.reason.as_deref())))
        }
        Some(b) => Ok(b),
    }
}

/// Checks that need no network: the name rule and the limits (§5.1, §5.3).
pub fn precheck(input: &Input) -> Result<String, Failure> {
    let name = command_name(input).ok_or_else(|| {
        Failure::new(
            "usage",
            "the command name is written plain, without quotes or spaces (letters, digits, . _ -)",
        )
    })?;
    check_limits(input).map_err(|why| Failure::new("too-large", why))?;
    Ok(name.to_owned())
}

/// What the pre-filter does with a command name, given the tab's list (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The tab lists it: the line goes out.
    Send,
    /// A sensitive command: it never leaves the CLI.
    Sensitive,
    /// The tab does not list it: only the name goes, for the tab's suggestion.
    NotListed,
}

/// The pre-filter of a name against the tab's list.
pub fn pre_filter(caps: &Capabilities, name: &str) -> Verdict {
    if caps.is_sensitive(name) {
        Verdict::Sensitive
    } else if caps.command(name).is_none() {
        Verdict::NotListed
    } else {
        Verdict::Send
    }
}

/// Why a sensitive command does not run from the CLI.
pub fn sensitive(name: &str) -> Failure {
    Failure::new(
        "not-allowed",
        format!(
            "{name} is a sensitive command; it does not run from the CLI, type it in the browser"
        ),
    )
}

/// A name the tab does not list, after it was asked and the list read once more.
pub fn unknown_command(name: &str) -> Outcome {
    Outcome::error("unknown-command", format!("unknown command: {name}"), 2)
}

/// The tab's answer about a name it does not list (§5.2 step 3): its `unknown-command`
/// and suggestion, or `None` when the tab does know it (the list was stale).
pub fn not_listed(help: &HelpResult, name: &str) -> Option<Outcome> {
    if help.status == Status::Ok {
        return None;
    }
    let code = help
        .failure
        .code
        .clone()
        .unwrap_or_else(|| "unknown-command".to_owned());
    let message = help
        .failure
        .message
        .clone()
        .unwrap_or_else(|| format!("unknown command: {name}"));
    let mut outcome = Outcome::error(code, message, help.exit_code());
    // The suggestion names a command: `gecko <name>`.
    outcome.suggest = help.suggest.clone();
    if help.failure.message.is_some() {
        outcome.untrusted.push("message");
    }
    Some(outcome)
}

/// The hub's stamp on a sent envelope; read from the hub's answer only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stamp {
    stamped_at: u64,
    expires_at: u64,
}

impl Stamp {
    /// How long after the send an unanswered request counts as not delivered: the hub's
    /// window plus `margin` (`GECKO_DELIVERY_MARGIN_MS`).
    pub fn delivery_window(&self, margin: Duration) -> Duration {
        Duration::from_millis(self.expires_at.saturating_sub(self.stamped_at)) + margin
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delivery_window_is_the_hubs_plus_the_margin() {
        let stamp = Stamp {
            stamped_at: 1_000,
            expires_at: 6_000,
        };
        assert_eq!(
            stamp.delivery_window(Duration::from_secs(2)),
            Duration::from_secs(7)
        );
        let late = Stamp {
            stamped_at: 6_000,
            expires_at: 1_000,
        };
        assert_eq!(
            late.delivery_window(Duration::from_millis(5)),
            Duration::from_millis(5)
        );
    }

    #[test]
    fn a_line_without_a_plain_name_is_a_usage_error() {
        let failure = precheck(&Input::Line("\"tasks list\"".into())).unwrap_err();
        assert_eq!(failure.code, "usage");
        assert_eq!(
            precheck(&Input::Line("tasks.list --all".into())).unwrap(),
            "tasks.list"
        );
    }
}
