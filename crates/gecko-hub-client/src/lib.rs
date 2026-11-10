//! The CLI's side of a hub connection (docs/rust-migration.md §7.2).
//!
//! A transport carries opaque envelopes; it never reads the tab's payload. Hub errors come
//! back as values (`HubError`), and the end of a connection comes back as a reason
//! (`Closed`) rather than a close code (B18). Two transports: the development hub
//! (WebSocket frames, step 4b) and SignalR (`signalr`, the narrow client chosen in step
//! 4a, docs/signalr-trial.md). Neither reconnects, so a command is never sent twice.

pub mod net;
pub mod proxy;
pub mod signalr;

use std::future::Future;

use serde::Deserialize;
use serde_json::value::RawValue;

/// Why a connection ended, in the CLI's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Closed {
    /// The token is missing, unknown or was sent the wrong way (401/403, 4401, 4403).
    Unauthorized,
    /// The grant already has a live CLI connection (4409).
    Busy,
    /// The grant was revoked while bound (4410).
    Revoked,
    /// The hub refused the protocol handshake.
    Handshake(String),
    /// The hub closed with a reason the CLI does not know; carries its text.
    Server(Option<String>),
    /// Nothing arrived from the hub within the server timeout.
    ServerTimeout,
    /// The network failed or the socket ended without a reason.
    Transport(String),
    /// The CLI closed it.
    Local,
}

impl Closed {
    /// Maps a close text of the form `"<code> <name>"` (#1974) or a close code.
    pub fn from_close_text(text: Option<&str>) -> Closed {
        let Some(text) = text else {
            return Closed::Server(None);
        };
        match text.split(' ').next().unwrap_or("") {
            "4401" | "4403" => Closed::Unauthorized,
            "4409" => Closed::Busy,
            "4410" => Closed::Revoked,
            _ => Closed::Server(Some(text.to_owned())),
        }
    }
}

/// A hub method's refusal: `{ ok: false, error: { code, message?, reason?, retryAfterMs? } }`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubError {
    pub code: String,
    pub message: Option<String>,
    pub reason: Option<String>,
    pub retry_after_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The connection ended before the answer arrived.
    Closed(Closed),
    /// The hub answered with an error value.
    Hub(HubError),
}

/// One short connection to a hub, opened per `gecko` call.
pub trait Transport {
    /// `status`: who is signed in and what the binding holds (capabilities, where).
    fn status(&mut self) -> impl Future<Output = Result<Box<RawValue>, CallError>>;
    /// `send`: one envelope to the bound tab. The hub stamps and routes it unread.
    fn send(
        &mut self,
        envelope: &RawValue,
    ) -> impl Future<Output = Result<Box<RawValue>, CallError>>;
    /// The next message from the tab, or why the connection ended.
    fn next_message(&mut self) -> impl Future<Output = Result<Box<RawValue>, Closed>>;
    /// Closes the connection and reports how it ended.
    fn close(self) -> impl Future<Output = Closed>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_texts_map_to_reasons() {
        assert_eq!(Closed::from_close_text(Some("4409 busy")), Closed::Busy);
        assert_eq!(
            Closed::from_close_text(Some("4410 revoked")),
            Closed::Revoked
        );
        assert_eq!(
            Closed::from_close_text(Some("4401 unauthorized")),
            Closed::Unauthorized
        );
        assert_eq!(
            Closed::from_close_text(Some("4403 token-in-query")),
            Closed::Unauthorized
        );
        assert_eq!(Closed::from_close_text(None), Closed::Server(None));
        assert_eq!(
            Closed::from_close_text(Some("going away")),
            Closed::Server(Some("going away".to_owned()))
        );
    }

    #[test]
    fn hub_error_reads_optional_fields() {
        let error: HubError =
            serde_json::from_str(r#"{"code":"rate-limited","retryAfterMs":1000,"extra":true}"#)
                .unwrap();
        assert_eq!(error.code, "rate-limited");
        assert_eq!(error.retry_after_ms, Some(1000));
        assert_eq!(error.message, None);
    }
}
