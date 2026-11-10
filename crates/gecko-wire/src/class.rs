//! Values the tab sends that decide the CLI's behaviour (§5.6, §5.7 rule 4).

use serde::Deserialize;

/// `class` of a failed result (§5.6). Exit codes: failed 1, usage 2, denied 3,
/// not-run 4, unknown 5, cancelled 1 (K3). An unknown value exits with 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorClass {
    Failed,
    Usage,
    Denied,
    NotRun,
    Unknown,
    Cancelled,
    #[serde(other)]
    Unrecognized,
}

impl ErrorClass {
    pub fn exit_code(self) -> u8 {
        match self {
            ErrorClass::Failed | ErrorClass::Cancelled | ErrorClass::Unrecognized => 1,
            ErrorClass::Usage => 2,
            ErrorClass::Denied => 3,
            ErrorClass::NotRun => 4,
            ErrorClass::Unknown => 5,
        }
    }
}

/// `danger` of a command. An unknown value counts as `change` (§5.7 rule 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Danger {
    Read,
    Change,
    #[serde(other)]
    Unrecognized,
}

impl Danger {
    pub fn is_change(self) -> bool {
        !matches!(self, Danger::Read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(value: &str) -> ErrorClass {
        serde_json::from_str(&format!("\"{value}\"")).unwrap()
    }

    #[test]
    fn class_exit_codes() {
        let table = [
            ("failed", 1),
            ("usage", 2),
            ("denied", 3),
            ("not-run", 4),
            ("unknown", 5),
            ("cancelled", 1),
            ("added-later", 1),
        ];
        for (value, code) in table {
            assert_eq!(class(value).exit_code(), code, "{value}");
        }
    }

    #[test]
    fn unknown_danger_is_a_change() {
        let danger: Danger = serde_json::from_str("\"destroy\"").unwrap();
        assert!(danger.is_change());
        assert!(
            !serde_json::from_str::<Danger>("\"read\"")
                .unwrap()
                .is_change()
        );
        assert!(
            serde_json::from_str::<Danger>("\"change\"")
                .unwrap()
                .is_change()
        );
    }
}
