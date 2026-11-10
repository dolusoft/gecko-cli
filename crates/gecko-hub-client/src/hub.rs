//! The hub's own answers, which the CLI reads (docs/browser-side.md; #1974 item 10):
//! `status`, a binding, a pairing code. Unknown fields are ignored and an unknown
//! binding state reads as `Unknown`, so a newer hub does not break an older CLI.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
}

/// The state of a binding (§5.3 of the desktop CLI spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BindingState {
    Active,
    Suspended,
    Gone,
    #[serde(other)]
    Unknown,
}

/// Where the bound tab is (§5.9, `publishWhere`): a title and a path without query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Where {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub binding_id: String,
    pub state: BindingState,
    /// Why a `gone` binding ended (`replaced`, `closed`, `detached`, ...).
    #[serde(default)]
    pub reason: Option<String>,
    pub page_id: String,
    /// Milliseconds since the epoch.
    #[serde(default)]
    pub approved_at: Option<u64>,
    /// The tab's published capability list, kept unread here (§5.8).
    #[serde(default)]
    pub capabilities: Option<Box<RawValue>>,
    #[serde(default, rename = "where")]
    pub location: Option<Where>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubStatus {
    pub user: User,
    pub env: String,
    pub grant_id: String,
    pub binding: Option<Binding>,
}

/// The answer to `pair`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairCode {
    pub code: String,
    /// Milliseconds since the epoch, on the hub's clock.
    pub expires_at: u64,
    /// The pairing page, `<web origin>/cli-pair#<code>`.
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_of_the_node_hub() {
        let status: HubStatus = serde_json::from_str(
            r#"{"user":{"id":"u1","name":"Zahit"},"env":"127.0.0.1:4400","grantId":"g1",
                "binding":{"bindingId":"b1","state":"active","pageId":"p1","approvedAt":5,
                "capabilities":{"kind":"capabilities"},"extra":1}}"#,
        )
        .unwrap();
        let binding = status.binding.unwrap();
        assert_eq!(binding.state, BindingState::Active);
        assert_eq!(binding.approved_at, Some(5));
        assert_eq!(
            binding.capabilities.unwrap().get(),
            "{\"kind\":\"capabilities\"}"
        );
        assert_eq!(binding.location, None);
    }

    #[test]
    fn new_fields_and_unknown_state() {
        let binding: Binding = serde_json::from_str(
            r#"{"bindingId":"b","state":"paused","pageId":"p","where":{"title":"Tasks","path":"/tasks"}}"#,
        )
        .unwrap();
        assert_eq!(binding.state, BindingState::Unknown);
        assert_eq!(
            binding.location,
            Some(Where {
                title: Some("Tasks".into()),
                path: Some("/tasks".into())
            })
        );
        let none: HubStatus = serde_json::from_str(
            r#"{"user":{"id":"u","name":"n"},"env":"e","grantId":"g","binding":null}"#,
        )
        .unwrap();
        assert!(none.binding.is_none());
    }

    #[test]
    fn pair_code() {
        let code: PairCode = serde_json::from_str(
            r#"{"code":"4UAP-6CKG","expiresAt":1794315219579,"url":"http://127.0.0.1:1/cli-pair#4UAP-6CKG"}"#,
        )
        .unwrap();
        assert_eq!(code.code, "4UAP-6CKG");
    }
}
