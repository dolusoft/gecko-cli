//! An attach code opened: the domain decodes it and checks its hub address; the address
//! is parsed as a URL here.

use gecko_binding_domain::code::{Address, AttachCode, decode, hub_origin};
use gecko_kernel::Failure;
use url::Url;

/// `address` parsed as a URL, in the parts the domain's rule looks at.
fn parse(address: &str) -> Option<Address> {
    let url = Url::parse(address).ok()?;
    Some(Address {
        scheme: url.scheme().to_owned(),
        host: url.host_str().map(str::to_owned),
        username: url.username().to_owned(),
        password: url.password().map(str::to_owned),
        path: url.path().to_owned(),
        query: url.query().map(str::to_owned),
        fragment: url.fragment().map(str::to_owned),
        origin: url.origin().ascii_serialization(),
    })
}

/// Opens a code: decoded, with its hub address as a checked origin.
pub fn open(text: &str) -> Result<AttachCode, Failure> {
    let code = decode(text)?;
    Ok(AttachCode {
        hub: hub_origin(&code.hub, parse(&code.hub))?,
        secret: code.secret,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gecko_binding_domain::code::invalid;

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

    fn hub_of(address: &str) -> Result<String, Failure> {
        hub_origin(address, parse(address))
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
            hub_of("http://localhost:5173").unwrap(),
            "http://localhost:5173"
        );
        assert_eq!(hub_of("http://[::1]:1").unwrap(), "http://[::1]:1");
        assert_eq!(
            hub_of("https://10.0.0.5:8443").unwrap(),
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
                hub_of(address).unwrap_err().code,
                "insecure-hub",
                "{address}"
            );
        }
    }
}
