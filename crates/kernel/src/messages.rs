//! Messages several commands share.

use crate::Failure;

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

/// What a command says without a binding: there is no token to use.
pub fn no_binding() -> Failure {
    Failure::new(
        "no-binding",
        "no bound tab; to bind: gecko attach <code> (copy the line from the Gecko tab)",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exit_code_of;

    #[test]
    fn no_binding_has_its_code() {
        assert_eq!(exit_code_of(&no_binding().code), 4);
        assert_eq!(no_binding().code, "no-binding");
        assert!(no_binding().message.contains("gecko attach <code>"));
    }
}
