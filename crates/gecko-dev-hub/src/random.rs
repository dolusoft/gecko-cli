//! Codes, secrets and ids from the operating system's random source.

/// No 0/O, 1/I/L, 5/S, 2/Z: a code is read off a screen and typed.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRTUVWXY346789";

fn bytes<const N: usize>() -> [u8; N] {
    let mut buffer = [0u8; N];
    // Without a random source the hub cannot hand out secrets at all.
    getrandom::fill(&mut buffer).expect("the operating system's random source");
    buffer
}

/// A code a person types: `ABCD-EFGH`.
pub fn code() -> String {
    let mut text = String::with_capacity(9);
    for (i, byte) in bytes::<8>().iter().enumerate() {
        text.push(char::from(
            CODE_ALPHABET[usize::from(*byte) % CODE_ALPHABET.len()],
        ));
        if i == 3 {
            text.push('-');
        }
    }
    text
}

/// 24 random bytes, base64url without padding.
pub fn secret() -> String {
    base64url(&bytes::<24>())
}

/// A random (version 4) UUID.
pub fn uuid() -> String {
    let mut b = bytes::<16>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A code as the user may type it: case and the dash do not matter.
pub fn normalize_code(value: &str) -> String {
    let plain: String = value
        .to_uppercase()
        .chars()
        .filter(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        .collect();
    if plain.len() == 8 {
        format!("{}-{}", &plain[..4], &plain[4..])
    } else {
        plain
    }
}

fn base64url(input: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut text = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | u32::from(*byte) << (16 - 8 * i));
        let digits = chunk.len() + 1;
        for i in 0..digits {
            text.push(char::from(TABLE[((n >> (18 - 6 * i)) & 0x3f) as usize]));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_use_the_alphabet_and_a_dash() {
        let code = code();
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        assert!(
            code.bytes()
                .filter(|b| *b != b'-')
                .all(|b| CODE_ALPHABET.contains(&b))
        );
    }

    #[test]
    fn typed_codes_are_normalized() {
        assert_eq!(normalize_code("abcd efgh"), "ABCD-EFGH");
        assert_eq!(normalize_code("ABCD-EFGH"), "ABCD-EFGH");
        assert_eq!(normalize_code("abc"), "ABC");
        assert_eq!(normalize_code(""), "");
    }

    #[test]
    fn base64url_matches_known_values() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(secret().len(), 32);
    }

    #[test]
    fn uuids_are_version_4() {
        let id = uuid();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
    }
}
