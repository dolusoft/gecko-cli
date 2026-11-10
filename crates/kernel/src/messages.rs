//! Messages several commands share.

/// What a bound command says when its binding has ended.
pub const BIND_AGAIN: &str =
    "to bind again: gecko attach <code> (copy the line from the Gecko tab)";

/// Why a binding ended, for the reader; the hub's `reason` stays the machine value.
pub fn gone_message(reason: Option<&str>) -> String {
    let why = match reason.unwrap_or("") {
        "replaced" => "the tab was bound again with a new code",
        "closed" => "the bound tab was closed",
        "detached" => "the binding was removed",
        "logged-out" => "the browser session was signed out",
        "revoked" => "the CLI access was revoked in the profile",
        "suspended-timeout" => "the bound tab did not come back within 60 s",
        _ => "the binding ended",
    };
    format!("{why}; {BIND_AGAIN}")
}
