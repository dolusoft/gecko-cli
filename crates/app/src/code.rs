//! The attach code (attach spec §3.2): `gk1_` and the base64url JSON
//! `{ "v": 1, "hub": <address>, "s": <secret> }`, and the check of its hub address
//! (§3.3). The address comes from text a user was handed, so only `https` and loopback
//! `http` pass.

use gecko_kernel::Failure;
use serde::Deserialize;
use url::Url;

const PREFIX: &str = "gk1_";

/// The one message of a code that cannot be used: unreadable, used or expired.
pub const INVALID_MESSAGE: &str = "the code is invalid or expired; copy a new one from the tab";

/// An opened attach code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachCode {
    /// The hub's origin (`https://hub.example`, `http://127.0.0.1:4400`).
    pub hub: String,
    pub secret: String,
}

pub fn invalid() -> Failure {
    Failure::new("invalid-code", INVALID_MESSAGE)
}

#[derive(Deserialize)]
struct Body {
    v: u64,
    hub: String,
    s: String,
}

/// base64url, with or without padding; `None` for any other character.
fn unbase64url(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(out)
}

/// Opens a code. Anything that is not a version-1 code is `invalid-code`, with the
/// same message as a used or expired one.
pub fn open(text: &str) -> Result<AttachCode, Failure> {
    let body = text.trim().strip_prefix(PREFIX).ok_or_else(invalid)?;
    let bytes = unbase64url(body).ok_or_else(invalid)?;
    let body: Body = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if body.v != 1 || body.s.is_empty() {
        return Err(invalid());
    }
    Ok(AttachCode {
        hub: hub_origin(&body.hub)?,
        secret: body.s,
    })
}

/// The hub address as an origin: `https://` for any host, `http://` for loopback only
/// (`127.0.0.1`, `localhost`, `[::1]`). An address with a path, a query or credentials
/// is not a hub address.
pub fn hub_origin(address: &str) -> Result<String, Failure> {
    let url = Url::parse(address).map_err(|_| invalid())?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        _ => {
            return Err(Failure::new(
                "insecure-hub",
                format!(
                    "the code names {address}; only https addresses and http on this computer (127.0.0.1, localhost) are accepted"
                ),
            ));
        }
    }
    let plain = url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "" | "/")
        && url.query().is_none()
        && url.fragment().is_none()
        && url.host_str().is_some();
    if !plain {
        return Err(invalid());
    }
    Ok(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base64url(input: &[u8]) -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut text = String::new();
        for chunk in input.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, byte)| n | u32::from(*byte) << (16 - 8 * i));
            for i in 0..=chunk.len() {
                text.push(char::from(TABLE[((n >> (18 - 6 * i)) & 0x3f) as usize]));
            }
        }
        text
    }

    fn code_of(json: &str) -> String {
        format!("gk1_{}", base64url(json.as_bytes()))
    }

    #[test]
    fn base64url_round_trip() {
        for input in [&b""[..], b"f", b"fo", b"foo", b"foob", &[0xfb, 0xff, 0x00]] {
            assert_eq!(unbase64url(&base64url(input)).unwrap(), input);
        }
        assert_eq!(unbase64url("Zg==").unwrap(), b"f");
        assert_eq!(unbase64url("Z+g"), None);
    }

    #[test]
    fn a_code_opens() {
        let code = code_of(r#"{"v":1,"hub":"http://127.0.0.1:4400","s":"secret"}"#);
        assert_eq!(
            open(&format!("  {code}\n")).unwrap(),
            AttachCode {
                hub: "http://127.0.0.1:4400".into(),
                secret: "secret".into()
            }
        );
        let https = code_of(r#"{"v":1,"hub":"https://Gecko.Example/","s":"x","later":1}"#);
        assert_eq!(open(&https).unwrap().hub, "https://gecko.example");
    }

    #[test]
    fn broken_codes_are_invalid() {
        for text in [
            "",
            "gk1_",
            "gk2_e30",
            "gk1_!!!",
            &code_of("[]"),
            &code_of(r#"{"v":2,"hub":"https://a.example","s":"x"}"#),
            &code_of(r#"{"v":1,"hub":"https://a.example","s":""}"#),
            &code_of(r#"{"v":1,"s":"x"}"#),
            &code_of(r#"{"v":1,"hub":"https://a.example/hub","s":"x"}"#),
            &code_of(r#"{"v":1,"hub":"https://u:p@a.example","s":"x"}"#),
        ] {
            assert_eq!(open(text).unwrap_err(), invalid(), "{text}");
        }
    }

    #[test]
    fn only_https_and_loopback_http() {
        assert_eq!(
            hub_origin("http://localhost:5173").unwrap(),
            "http://localhost:5173"
        );
        assert_eq!(hub_origin("http://[::1]:1").unwrap(), "http://[::1]:1");
        assert_eq!(
            hub_origin("https://10.0.0.5:8443").unwrap(),
            "https://10.0.0.5:8443"
        );
        for address in [
            "http://gecko.example",
            "http://10.0.0.5:4400",
            "http://127.0.0.2:4400",
            "ws://127.0.0.1:4400",
            "file:///etc/passwd",
        ] {
            assert_eq!(
                hub_origin(address).unwrap_err().code,
                "insecure-hub",
                "{address}"
            );
        }
    }
}
