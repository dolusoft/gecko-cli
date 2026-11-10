//! The CLI's side of a hub connection (spec §7.2).
//!
//! A transport carries opaque envelopes; it never reads the tab's payload. Hub errors come
//! back as values (`HubError`), and the end of a connection comes back as a reason
//! (`Closed`) rather than a close code (B18). Two transports: the development hub
//! (WebSocket frames, step 4b) and SignalR (`signalr`, the narrow client chosen in step
//! 4a). Neither reconnects, so a command is never sent twice.

pub mod dev;
pub mod hub;
pub mod net;
pub mod proxy;
pub mod select;
pub mod signalr;
pub mod tcp;

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

    /// Maps a WebSocket close code and reason (the development hub).
    pub fn from_close_code(code: u16, reason: &str) -> Closed {
        let text = format!("{code} {reason}");
        Closed::from_close_text(Some(text.trim_end()))
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
    /// No answer within the per-call limit. The call may still have taken effect, so its
    /// outcome is unknown; the connection stays open.
    TimedOut,
}

/// What the hub pushes to the CLI (#1974 methods table).
#[derive(Debug)]
pub enum HubEvent {
    /// `reply`: a message from the bound tab, unread by the hub.
    Reply(Box<RawValue>),
    /// `binding`: the binding changed state (suspended, active again, gone).
    Binding(Box<RawValue>),
    /// `paired`: the browser approved the pairing code.
    Paired(Box<RawValue>),
    /// `pair-refused`: the browser refused it; carries the hub's code.
    PairRefused(String),
    /// An event this CLI does not know; kept so a newer hub does not break it.
    Other {
        name: String,
        payload: Box<RawValue>,
    },
}

impl HubEvent {
    /// Sorts an event by its name. A `pair-refused` without a string `code` keeps an
    /// empty code rather than failing.
    pub fn from_parts(name: &str, payload: Box<RawValue>) -> HubEvent {
        match name {
            "reply" => HubEvent::Reply(payload),
            "binding" => HubEvent::Binding(payload),
            "paired" => HubEvent::Paired(payload),
            "pair-refused" => {
                #[derive(Deserialize)]
                struct Refused {
                    code: Option<String>,
                }
                let code = serde_json::from_str::<Refused>(payload.get())
                    .ok()
                    .and_then(|refused| refused.code)
                    .unwrap_or_default();
                HubEvent::PairRefused(code)
            }
            _ => HubEvent::Other {
                name: name.to_owned(),
                payload,
            },
        }
    }
}

/// One short connection to a hub, opened per `gecko` call. Values come back raw: the
/// transport never reads the tab's payload, and the CLI reads the hub's own answers.
pub trait Transport {
    /// `status`: who is signed in and what the binding holds (capabilities, where).
    fn status(&mut self) -> impl Future<Output = Result<Box<RawValue>, CallError>>;
    /// `pair`: a pairing code for `gecko attach`.
    fn pair(&mut self) -> impl Future<Output = Result<Box<RawValue>, CallError>>;
    /// `send`: one envelope to the bound tab. The hub stamps and routes it unread; the
    /// answer is the stamp (`stampedAt`, `expiresAt`).
    fn send(
        &mut self,
        envelope: &RawValue,
    ) -> impl Future<Output = Result<Box<RawValue>, CallError>>;
    /// `detach`: removes the binding.
    fn detach(&mut self) -> impl Future<Output = Result<(), CallError>>;
    /// The next event from the hub, or why the connection ended.
    fn next_event(&mut self) -> impl Future<Output = Result<HubEvent, Closed>>;
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
    fn close_codes_map_to_reasons() {
        assert_eq!(Closed::from_close_code(4409, "busy"), Closed::Busy);
        assert_eq!(Closed::from_close_code(4410, ""), Closed::Revoked);
        assert_eq!(
            Closed::from_close_code(4403, "forbidden"),
            Closed::Unauthorized
        );
        assert_eq!(
            Closed::from_close_code(1001, ""),
            Closed::Server(Some("1001".to_owned()))
        );
        assert_eq!(
            Closed::from_close_code(1000, "hub stopping"),
            Closed::Server(Some("1000 hub stopping".to_owned()))
        );
    }

    #[test]
    fn events_are_sorted_by_name() {
        let raw = |json: &str| RawValue::from_string(json.to_owned()).unwrap();
        assert!(matches!(
            HubEvent::from_parts("reply", raw("{\"v\":1}")),
            HubEvent::Reply(payload) if payload.get() == "{\"v\":1}"
        ));
        assert!(matches!(
            HubEvent::from_parts("binding", raw("{}")),
            HubEvent::Binding(_)
        ));
        assert!(matches!(
            HubEvent::from_parts("paired", raw("{}")),
            HubEvent::Paired(_)
        ));
        assert!(matches!(
            HubEvent::from_parts("pair-refused", raw("{\"code\":\"expired\"}")),
            HubEvent::PairRefused(code) if code == "expired"
        ));
        assert!(matches!(
            HubEvent::from_parts("pair-refused", raw("null")),
            HubEvent::PairRefused(code) if code.is_empty()
        ));
        assert!(matches!(
            HubEvent::from_parts("later", raw("1")),
            HubEvent::Other { name, .. } if name == "later"
        ));
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
