//! Why a connection ended or a hub call failed, in the CLI's terms: a `Closed` reason
//! or a `CallError` becomes a `Failure` with its code (and so its exit code).

use gecko_kernel::{BIND_AGAIN, Failure};

use crate::{CallError, Closed};

/// Why the connection could not be opened, in the CLI's terms.
pub fn open_failure(closed: Closed) -> Failure {
    match closed {
        Closed::Unauthorized => Failure::new(
            "unauthorized",
            format!("the hub refused the token; {BIND_AGAIN}"),
        ),
        Closed::Busy => Failure::new(
            "busy",
            "another gecko call is running (one live connection per grant)",
        ),
        Closed::Revoked => Failure::new("revoked", format!("the binding ended; {BIND_AGAIN}")),
        other => Failure::new(
            "unreachable",
            format!("cannot reach the hub: {}", describe(&other)),
        ),
    }
}

/// Why an open connection ended during a call.
pub fn lost_failure(closed: Closed) -> Failure {
    match closed {
        Closed::Unauthorized | Closed::Busy | Closed::Revoked => open_failure(closed),
        other => Failure::new(
            "connection-lost",
            format!("the hub connection was lost: {}", describe(&other)),
        ),
    }
}

fn describe(closed: &Closed) -> String {
    match closed {
        Closed::Handshake(text) | Closed::Transport(text) => text.clone(),
        Closed::Server(Some(text)) => format!("closed by the hub ({text})"),
        Closed::Server(None) => "closed by the hub".to_owned(),
        Closed::ServerTimeout => "the hub stopped answering".to_owned(),
        Closed::Local => "closed".to_owned(),
        Closed::Unauthorized => "unauthorized".to_owned(),
        Closed::Busy => "busy".to_owned(),
        Closed::Revoked => "revoked".to_owned(),
    }
}

/// A failed hub call. A call that timed out may still have taken effect; `effect` says
/// whether that matters (then the outcome is unknown, exit 5).
pub fn call_failure(error: CallError, effect: bool) -> Failure {
    match error {
        CallError::Closed(closed) => lost_failure(closed),
        CallError::Hub(error) => {
            let message = error.message.unwrap_or_else(|| error.code.clone());
            Failure::new(error.code, message)
        }
        CallError::TimedOut if effect => Failure::new(
            "unknown-outcome",
            "the hub did not answer in time; the outcome is unknown (check with gecko status)",
        ),
        CallError::TimedOut => Failure::new("unreachable", "the hub did not answer in time"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HubError;
    use gecko_kernel::exit_code_of;

    #[test]
    fn closed_reasons_have_their_codes() {
        let code = |f: Failure| (f.code.clone(), exit_code_of(&f.code));
        assert_eq!(
            code(open_failure(Closed::Unauthorized)),
            ("unauthorized".into(), 6)
        );
        assert_eq!(code(open_failure(Closed::Busy)), ("busy".into(), 4));
        assert_eq!(code(open_failure(Closed::Revoked)), ("revoked".into(), 6));
        assert_eq!(
            code(open_failure(Closed::Transport("refused".into()))),
            ("unreachable".into(), 4)
        );
        assert_eq!(
            code(lost_failure(Closed::Server(None))),
            ("connection-lost".into(), 4)
        );
        assert_eq!(code(lost_failure(Closed::Busy)), ("busy".into(), 4));
    }

    #[test]
    fn every_closed_reason_is_described() {
        let cases = [
            (Closed::Handshake("bad hello".into()), "bad hello"),
            (Closed::Transport("reset".into()), "reset"),
            (
                Closed::Server(Some("4499 odd".into())),
                "closed by the hub (4499 odd)",
            ),
            (Closed::Server(None), "closed by the hub"),
            (Closed::ServerTimeout, "the hub stopped answering"),
            (Closed::Local, "closed"),
            (Closed::Unauthorized, "unauthorized"),
            (Closed::Busy, "busy"),
            (Closed::Revoked, "revoked"),
        ];
        for (closed, text) in cases {
            assert_eq!(describe(&closed), text);
        }
        assert_eq!(
            call_failure(CallError::Closed(Closed::ServerTimeout), false),
            Failure::new(
                "connection-lost",
                "the hub connection was lost: the hub stopped answering"
            )
        );
    }

    #[test]
    fn call_failures() {
        let hub = CallError::Hub(HubError {
            code: "no-binding".into(),
            message: None,
            reason: None,
            retry_after_ms: None,
        });
        assert_eq!(
            call_failure(hub, false),
            Failure::new("no-binding", "no-binding")
        );
        assert_eq!(
            call_failure(CallError::TimedOut, true).code,
            "unknown-outcome"
        );
        assert_eq!(call_failure(CallError::TimedOut, false).code, "unreachable");
    }
}
