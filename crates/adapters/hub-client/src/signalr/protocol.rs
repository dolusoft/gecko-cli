//! SignalR JSON hub protocol, the part the CLI uses.
//!
//! Supported: JSON protocol version 1 over WebSockets, Invocation (1), Completion (3),
//! Ping (6), Close (7). Every other message type is ignored, so Ack (8) and Sequence (9)
//! from stateful reconnect never break a connection. Not supported, on purpose:
//! MessagePack, long polling, server-sent events, stateful reconnect, streaming.

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::HubError;

/// Record separator that ends every JSON record.
pub const RS: char = '\u{1e}';

/// The handshake request, already framed.
pub const HANDSHAKE_REQUEST: &str = "{\"protocol\":\"json\",\"version\":1}\u{1e}";

/// Splits incoming text into records. A WebSocket message may hold several records,
/// and a record is kept across messages until its separator arrives.
#[derive(Debug, Default)]
pub struct RecordReader {
    pending: String,
}

impl RecordReader {
    pub fn push(&mut self, text: &str) {
        self.pending.push_str(text);
    }

    /// The next complete record without its separator.
    pub fn next_record(&mut self) -> Option<String> {
        let end = self.pending.find(RS)?;
        let record = self.pending[..end].to_owned();
        self.pending.drain(..end + RS.len_utf8());
        Some(record)
    }

    #[cfg(test)]
    pub fn has_partial(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// Reads the handshake response: `{}` or `{"error":"..."}`.
pub fn parse_handshake(record: &str) -> Result<(), String> {
    #[derive(Deserialize)]
    struct Response {
        error: Option<String>,
    }
    let response: Response =
        serde_json::from_str(record).map_err(|e| format!("bad handshake response: {e}"))?;
    match response.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[derive(Debug)]
pub enum Incoming {
    Invocation {
        invocation_id: Option<String>,
        target: String,
        arguments: Vec<Box<RawValue>>,
    },
    Completion {
        invocation_id: String,
        result: Option<Box<RawValue>>,
        error: Option<String>,
    },
    Ping,
    Close {
        error: Option<String>,
        /// Read and ignored: the CLI never reconnects by itself.
        #[allow(dead_code)]
        allow_reconnect: bool,
    },
    /// A message type the CLI does not use; carries the type number for debugging.
    Ignored(#[allow(dead_code)] u64),
}

pub fn parse_message(record: &str) -> Result<Incoming, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Raw {
        r#type: u64,
        invocation_id: Option<String>,
        target: Option<String>,
        arguments: Option<Vec<Box<RawValue>>>,
        result: Option<Box<RawValue>>,
        error: Option<String>,
        allow_reconnect: Option<bool>,
    }
    let raw: Raw = serde_json::from_str(record).map_err(|e| format!("bad message: {e}"))?;
    Ok(match raw.r#type {
        1 => Incoming::Invocation {
            invocation_id: raw.invocation_id,
            target: raw.target.ok_or("invocation without target")?,
            arguments: raw.arguments.unwrap_or_default(),
        },
        3 => Incoming::Completion {
            invocation_id: raw.invocation_id.ok_or("completion without invocationId")?,
            result: raw.result,
            error: raw.error,
        },
        6 => Incoming::Ping,
        7 => Incoming::Close {
            error: raw.error,
            allow_reconnect: raw.allow_reconnect.unwrap_or(false),
        },
        other => Incoming::Ignored(other),
    })
}

pub fn invocation(invocation_id: &str, target: &str, arguments: &[&RawValue]) -> String {
    let mut out = String::with_capacity(64);
    out.push_str("{\"type\":1,\"invocationId\":");
    out.push_str(&serde_json::to_string(invocation_id).expect("string"));
    out.push_str(",\"target\":");
    out.push_str(&serde_json::to_string(target).expect("string"));
    out.push_str(",\"arguments\":[");
    for (i, argument) in arguments.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(argument.get());
    }
    out.push_str("]}");
    out.push(RS);
    out
}

/// The answer to a server invocation that waits for a result: the CLI offers none.
pub fn no_result_completion(invocation_id: &str) -> String {
    format!(
        "{{\"type\":3,\"invocationId\":{},\"error\":\"Client didn't provide a result.\"}}{RS}",
        serde_json::to_string(invocation_id).expect("string")
    )
}

pub const PING: &str = "{\"type\":6}\u{1e}";
pub const CLOSE: &str = "{\"type\":7}\u{1e}";

/// Reads a hub method's answer (#1974): `{ ok: true, value }` or `{ ok: false, error }`.
pub fn hub_result(result: &RawValue) -> Result<Box<RawValue>, HubError> {
    #[derive(Deserialize)]
    struct Envelope {
        ok: bool,
        value: Option<Box<RawValue>>,
        error: Option<HubError>,
    }
    let malformed = |detail: &str| HubError {
        code: "malformed-result".to_owned(),
        message: Some(detail.to_owned()),
        reason: None,
        retry_after_ms: None,
    };
    let envelope: Envelope =
        serde_json::from_str(result.get()).map_err(|e| malformed(&e.to_string()))?;
    if envelope.ok {
        Ok(envelope
            .value
            .unwrap_or_else(|| RawValue::from_string("null".to_owned()).expect("null")))
    } else {
        Err(envelope
            .error
            .unwrap_or_else(|| malformed("ok is false without error")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(reader: &mut RecordReader) -> Vec<String> {
        std::iter::from_fn(|| reader.next_record()).collect()
    }

    #[test]
    fn handshake_and_first_messages_in_one_frame() {
        let mut reader = RecordReader::default();
        reader.push("{}\u{1e}{\"type\":6}\u{1e}{\"type\":1,\"target\":\"Reply\",\"arguments\":[{\"a\":1}]}\u{1e}");
        let all = records(&mut reader);
        assert_eq!(all.len(), 3);
        assert_eq!(parse_handshake(&all[0]), Ok(()));
        assert!(matches!(parse_message(&all[1]), Ok(Incoming::Ping)));
        match parse_message(&all[2]).unwrap() {
            Incoming::Invocation {
                target,
                arguments,
                invocation_id,
            } => {
                assert_eq!(target, "Reply");
                assert_eq!(arguments[0].get(), "{\"a\":1}");
                assert_eq!(invocation_id, None);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(!reader.has_partial());
    }

    #[test]
    fn record_split_across_frames_waits_for_separator() {
        let mut reader = RecordReader::default();
        reader.push("{\"type\":");
        assert_eq!(reader.next_record(), None);
        assert!(reader.has_partial());
        reader.push("6}\u{1e}");
        assert_eq!(reader.next_record().as_deref(), Some("{\"type\":6}"));
    }

    #[test]
    fn handshake_error_is_reported() {
        assert_eq!(
            parse_handshake("{\"error\":\"The protocol 'x' is not supported.\"}"),
            Err("The protocol 'x' is not supported.".to_owned())
        );
    }

    #[test]
    fn unknown_types_are_ignored() {
        assert!(matches!(
            parse_message("{\"type\":8,\"sequenceId\":3}"),
            Ok(Incoming::Ignored(8))
        ));
        assert!(matches!(
            parse_message("{\"type\":9,\"sequenceId\":3}"),
            Ok(Incoming::Ignored(9))
        ));
        assert!(matches!(
            parse_message("{\"type\":2,\"invocationId\":\"1\",\"item\":1}"),
            Ok(Incoming::Ignored(2))
        ));
    }

    #[test]
    fn close_message_and_reasons() {
        match parse_message("{\"type\":7,\"error\":\"4409 busy\",\"allowReconnect\":true}").unwrap()
        {
            Incoming::Close {
                error,
                allow_reconnect,
            } => {
                assert_eq!(
                    crate::Closed::from_close_text(error.as_deref()),
                    crate::Closed::Busy
                );
                assert!(allow_reconnect);
            }
            other => panic!("unexpected {other:?}"),
        }
        match parse_message("{\"type\":7}").unwrap() {
            Incoming::Close {
                error,
                allow_reconnect,
            } => {
                assert_eq!(error, None);
                assert!(!allow_reconnect);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn completion_with_error() {
        match parse_message("{\"type\":3,\"invocationId\":\"7\",\"error\":\"boom\"}").unwrap() {
            Incoming::Completion {
                invocation_id,
                result,
                error,
            } => {
                assert_eq!(invocation_id, "7");
                assert!(result.is_none());
                assert_eq!(error.as_deref(), Some("boom"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn invocation_is_framed() {
        let argument = RawValue::from_string("{\"kind\":\"x\"}".to_owned()).unwrap();
        assert_eq!(
            invocation("1", "Send", &[&argument]),
            "{\"type\":1,\"invocationId\":\"1\",\"target\":\"Send\",\"arguments\":[{\"kind\":\"x\"}]}\u{1e}"
        );
        assert_eq!(
            no_result_completion("4"),
            "{\"type\":3,\"invocationId\":\"4\",\"error\":\"Client didn't provide a result.\"}\u{1e}"
        );
    }

    #[test]
    fn hub_results() {
        let ok =
            RawValue::from_string("{\"ok\":true,\"value\":{\"bound\":true}}".to_owned()).unwrap();
        assert_eq!(hub_result(&ok).unwrap().get(), "{\"bound\":true}");
        let refused = RawValue::from_string(
            "{\"ok\":false,\"error\":{\"code\":\"rate-limited\",\"retryAfterMs\":1000}}".to_owned(),
        )
        .unwrap();
        let error = hub_result(&refused).unwrap_err();
        assert_eq!(error.code, "rate-limited");
        assert_eq!(error.retry_after_ms, Some(1000));
        let bad = RawValue::from_string("{\"ok\":false}".to_owned()).unwrap();
        assert_eq!(hub_result(&bad).unwrap_err().code, "malformed-result");
    }
}
