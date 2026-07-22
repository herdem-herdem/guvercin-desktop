//! IMAP modified UTF-7 mailbox-name codec (RFC 3501 §5.1.3).
//!
//! IMAP servers encode non-ASCII mailbox names (e.g. Turkish Gmail folders
//! like "Gönderilmiş Postalar") using a modified form of UTF-7. The wire form
//! is unreadable when shown directly (`G&APY-nderilmi&AV8- Postalar`), so we
//! decode names coming off the wire and re-encode any name we send back.
//!
//! Modified UTF-7 differs from RFC 2152 UTF-7 in two ways:
//!  * the shift character is `&` instead of `+`
//!  * the Base64 alphabet uses `,` in place of `/`

use base64::{engine::general_purpose::STANDARD_NO_PAD as BASE64, Engine as _};

/// Decode an IMAP modified-UTF-7 mailbox name into a normal UTF-8 string.
///
/// Input that is not valid modified UTF-7 is returned unchanged so a decode
/// failure never hides a mailbox from the user.
pub fn decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < bytes.len() {
        let b = bytes[i];
        if b != b'&' {
            out.push(b as char);
            i += 1;
            continue;
        }

        // `&-` is a literal ampersand.
        if i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            out.push('&');
            i += 2;
            continue;
        }

        // Collect the Base64 run up to the terminating `-`.
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'-' {
            end += 1;
        }

        let chunk = &input[start..end];
        match decode_base64_run(chunk) {
            Some(decoded) => out.push_str(&decoded),
            // Not decodable: keep the original run verbatim.
            None => out.push_str(&input[i..end.min(bytes.len())]),
        }

        // Skip the run and its terminating `-` (if present).
        i = if end < bytes.len() { end + 1 } else { end };
    }

    out
}

fn decode_base64_run(chunk: &str) -> Option<String> {
    // Modified UTF-7 uses ',' where standard Base64 uses '/'.
    let standard: String = chunk.chars().map(|c| if c == ',' { '/' } else { c }).collect();
    let bytes = BASE64.decode(standard.as_bytes()).ok()?;
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units).ok()
}

/// Encode a UTF-8 mailbox name into IMAP modified UTF-7 for the wire.
pub fn encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut buffer: Vec<u16> = Vec::new();

    for ch in input.chars() {
        if is_direct(ch) {
            flush_buffer(&mut buffer, &mut out);
            out.push(ch);
        } else if ch == '&' {
            flush_buffer(&mut buffer, &mut out);
            out.push_str("&-");
        } else {
            let mut units = [0u16; 2];
            for unit in ch.encode_utf16(&mut units) {
                buffer.push(*unit);
            }
        }
    }

    flush_buffer(&mut buffer, &mut out);
    out
}

/// ASCII printable characters (except `&`) are represented directly.
fn is_direct(ch: char) -> bool {
    ch != '&' && (0x20..=0x7e).contains(&(ch as u32))
}

fn flush_buffer(buffer: &mut Vec<u16>, out: &mut String) {
    if buffer.is_empty() {
        return;
    }
    let mut bytes = Vec::with_capacity(buffer.len() * 2);
    for unit in buffer.iter() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    let encoded = BASE64.encode(&bytes).replace('/', ",");
    out.push('&');
    out.push_str(&encoded);
    out.push('-');
    buffer.clear();
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};

    // Real Turkish Gmail folder names observed on the wire.
    const CASES: &[(&str, &str)] = &[
        ("&AMcA9g-p kutusu", "Çöp kutusu"),
        ("&ANY-nemli", "Önemli"),
        ("G&APY-nderilmi&AV8- Postalar", "Gönderilmiş Postalar"),
        ("T&APw-m Postalar", "Tüm Postalar"),
        ("Y&ATE-ld&ATE-zl&ATE-", "Yıldızlı"),
    ];

    #[test]
    fn decodes_known_names() {
        for (wire, human) in CASES {
            assert_eq!(decode(wire), *human, "decoding {wire}");
        }
    }

    #[test]
    fn encodes_known_names() {
        for (wire, human) in CASES {
            assert_eq!(encode(human), *wire, "encoding {human}");
        }
    }

    #[test]
    fn round_trips() {
        for (_, human) in CASES {
            assert_eq!(decode(&encode(human)), *human);
        }
    }

    #[test]
    fn ascii_is_unchanged() {
        assert_eq!(decode("INBOX"), "INBOX");
        assert_eq!(encode("INBOX"), "INBOX");
        assert_eq!(decode("[Gmail]/Drafts"), "[Gmail]/Drafts");
        assert_eq!(encode("[Gmail]/Drafts"), "[Gmail]/Drafts");
    }

    #[test]
    fn literal_ampersand() {
        assert_eq!(decode("Foo &- Bar"), "Foo & Bar");
        assert_eq!(encode("Foo & Bar"), "Foo &- Bar");
    }
}
