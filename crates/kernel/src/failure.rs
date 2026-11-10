//! The CLI's own failures and their exit codes (§6 of the desktop CLI spec).

/// A failure the CLI itself reports, with a machine code (`not-logged-in`, `usage`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

impl Failure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Failure {
        Failure {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// The exit code of one of the CLI's own failure codes. Errors from the tab carry their
/// class instead (§5.6); this table is only for what the CLI decides by itself.
pub fn exit_code_of(code: &str) -> u8 {
    match code {
        "usage" | "unsupported-version" | "unknown-command" => 2,
        "not-allowed" | "no-terminal-access" | "untrusted-hub" | "insecure-hub"
        | "approval-dropped" => 3,
        "unreachable" | "connection-lost" | "busy" | "no-binding" | "gone" | "tab-unreachable"
        | "not-delivered" | "rate-limited" | "too-many-attempts" => 4,
        "unknown-outcome" => 5,
        "invalid-code" | "unauthorized" | "revoked" => 6,
        "cancelled" | "cancel-requested" | "cancelled-before-start" => 130,
        _ => 1,
    }
}
