//! The attach code (attach spec §3.2): `gk1_` and the base64url JSON
//! `{ "v": 1, "hub": <address>, "s": <secret> }`, and the check of its hub address
//! (§3.3). The address comes from text a user was handed, so only `https` and loopback
//! `http` pass. Parsing the address as a URL is the module's (the domain takes no URL
//! crate); the rule over its parts is here.

use gecko_kernel::Failure;
use serde::Deserialize;

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

/// Decodes a code: its hub address as written (not yet checked) and its secret.
/// Anything that is not a version-1 code is `invalid-code`, with the same message as a
/// used or expired one.
pub fn decode(text: &str) -> Result<AttachCode, Failure> {
    let body = text.trim().strip_prefix(PREFIX).ok_or_else(invalid)?;
    let bytes = unbase64url(body).ok_or_else(invalid)?;
    let body: Body = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if body.v != 1 || body.s.is_empty() {
        return Err(invalid());
    }
    Ok(AttachCode {
        hub: body.hub,
        secret: body.s,
    })
}

/// The parts of a parsed hub address the rule looks at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub scheme: String,
    pub host: Option<String>,
    pub username: String,
    pub password: Option<String>,
    pub path: String,
    pub query: Option<String>,
    pub fragment: Option<String>,
    /// The address's origin, serialized (`https://hub.example`).
    pub origin: String,
}

/// The hub address as an origin: `https://` for any host, `http://` for loopback only
/// (`127.0.0.1`, `localhost`, `[::1]`). An address with a path, a query or credentials
/// is not a hub address. `parsed` is `address` parsed as a URL, `None` when it is not
/// one.
pub fn hub_origin(address: &str, parsed: Option<Address>) -> Result<String, Failure> {
    let url = parsed.ok_or_else(invalid)?;
    let loopback = matches!(
        url.host.as_deref(),
        Some("127.0.0.1" | "localhost" | "[::1]")
    );
    match url.scheme.as_str() {
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
    let plain = url.username.is_empty()
        && url.password.is_none()
        && matches!(url.path.as_str(), "" | "/")
        && url.query.is_none()
        && url.fragment.is_none()
        && url.host.is_some();
    if !plain {
        return Err(invalid());
    }
    Ok(url.origin)
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

    #[test]
    fn base64url_round_trip() {
        for input in [&b""[..], b"f", b"fo", b"foo", b"foob", &[0xfb, 0xff, 0x00]] {
            assert_eq!(unbase64url(&base64url(input)).unwrap(), input);
        }
        assert_eq!(unbase64url("Zg==").unwrap(), b"f");
        assert_eq!(unbase64url("Z+g"), None);
    }
}
