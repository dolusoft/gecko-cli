//! Which transport an environment uses (Node: src/transport/select.ts).
//!
//! The development hub for a localhost address, the real hub (SignalR) otherwise.
//! `GECKO_TRANSPORT=dev` picks the development transport for any address, but that
//! transport still refuses a host that is not localhost: a release build never reaches a
//! development hub by accident.

/// The transport for one environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Dev,
    SignalR,
}

/// `host` is the environment's `host[:port]` (`127.0.0.1:4400`, `develop.example`);
/// `forced` is the value of `GECKO_TRANSPORT`.
pub fn choose(host: &str, forced: Option<&str>) -> Choice {
    if forced == Some("dev") || is_local_host(host) {
        Choice::Dev
    } else {
        Choice::SignalR
    }
}

/// `localhost`, `127.0.0.1` or `[::1]`, with an optional port, letters in any case.
pub fn is_local_host(host: &str) -> bool {
    let name = match host.rsplit_once(':') {
        // `[::1]:4400` splits at the last colon; a bare `[::1]` ends with `]`.
        Some((name, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => host,
    };
    name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1" || name == "[::1]"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hosts() {
        for host in [
            "localhost",
            "LocalHost:4400",
            "127.0.0.1",
            "127.0.0.1:4400",
            "[::1]",
            "[::1]:4400",
        ] {
            assert!(is_local_host(host), "{host}");
        }
        for host in [
            "develop.example",
            "localhost.example",
            "127.0.0.2",
            "::1",
            "localhost:",
            "localhost:44a",
            "127.0.0.1:4400:1",
            "",
        ] {
            assert!(!is_local_host(host), "{host}");
        }
    }

    #[test]
    fn choice_follows_the_host_and_the_override() {
        assert_eq!(choose("127.0.0.1:4400", None), Choice::Dev);
        assert_eq!(choose("develop.example", None), Choice::SignalR);
        assert_eq!(choose("develop.example", Some("dev")), Choice::Dev);
        assert_eq!(choose("develop.example", Some("signalr")), Choice::SignalR);
    }
}
