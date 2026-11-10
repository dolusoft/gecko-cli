//! One result, as `--json` writes it (§6 of the desktop CLI spec).

use serde::Serialize;

use crate::failure::{Failure, exit_code_of};

/// The view a command ran in (`finished.view`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// One result, as `--json` writes it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub v: u8,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Why a binding ended (`gone`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// How a change ran, from the tab's `finished` (attach spec §5.4): `card` or `none`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewInfo>,
    /// Whether the page was hidden when it accepted the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    /// The tab's data age: a number, or `null` when it does not know.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_age_ms: Option<serde_json::Value>,
    /// The process exit code; written on an error only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u8>,
    pub preview: bool,
    /// Argv the tab offers (a preview's confirmed form, the nearest name); never run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggest: Option<Vec<String>>,
    /// Argv of the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<Vec<String>>,
    /// The command whose help an `invalid-args` points to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    pub complete: bool,
    pub untrusted: Vec<&'static str>,
    /// Lines the tab wrapped for the terminal (`help` text); plain output only.
    #[serde(skip)]
    pub lines: Option<Vec<String>>,
}

impl Outcome {
    fn base(status: &'static str) -> Outcome {
        Outcome {
            v: 1,
            status,
            code: None,
            message: None,
            reason: None,
            summary: None,
            approval: None,
            data: None,
            view: None,
            hidden: None,
            data_age_ms: None,
            exit_code: None,
            preview: false,
            suggest: None,
            next: None,
            help: None,
            complete: true,
            untrusted: Vec::new(),
            lines: None,
        }
    }

    /// A finished command with nothing but its summary line.
    pub fn done(summary: impl Into<String>) -> Outcome {
        Outcome {
            summary: Some(summary.into()),
            ..Outcome::base("ok")
        }
    }

    /// An ok result with no fields set yet.
    pub fn ok() -> Outcome {
        Outcome::base("ok")
    }

    /// A failure with its exit code given (the tab's class, or the CLI's own
    /// observation, §5.6): 5 is `unknown` and incomplete, 130 `cancelled`.
    pub fn error(code: impl Into<String>, message: impl Into<String>, exit: u8) -> Outcome {
        Outcome {
            code: Some(code.into()),
            message: Some(message.into()),
            exit_code: Some(exit),
            complete: exit != 5,
            ..Outcome::base(match exit {
                5 => "unknown",
                130 => "cancelled",
                _ => "error",
            })
        }
    }

    pub fn failed(failure: &Failure) -> Outcome {
        Outcome::error(
            failure.code.clone(),
            failure.message.clone(),
            exit_code_of(&failure.code),
        )
    }

    pub fn exit(&self) -> u8 {
        self.exit_code.unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_and_unknown_status() {
        let cancelled = Outcome::failed(&Failure::new("cancelled", "interrupted"));
        assert_eq!((cancelled.status, cancelled.exit()), ("cancelled", 130));
        let unknown = Outcome::failed(&Failure::new("unknown-outcome", "?"));
        assert_eq!((unknown.status, unknown.complete), ("unknown", false));
    }
}
