//! Stage 2.5 -- anonymization of sensitive data.
//!
//! Replaces personally identifying and sensitive information in rendered
//! output with consistent fake values. Opt-in via `--anonymize`; the full-log
//! tee file is written byte-exact upstream of ingestion and is never
//! anonymized, so `--tee` plus `--anonymize` gives both a private display
//! and a complete recovery copy.
//!
//! Replacements are deterministic: the same sensitive value always maps to
//! the same fake value, so a user can still correlate related lines across a
//! log without learning the real values, and two runs over the same log
//! produce the same output.
//!
//! Distinct values get distinct fakes, for the same reason: a reader must be
//! able to tell two addresses apart.
//!
//! Some fakes are also *recognisable* as fakes -- an email local part, a URL
//! path segment, a card number -- and a second pass leaves those alone.
//! An address or a UUID is not: a fake address has to stay distinguishable
//! from the address it replaced, so a second pass gives it a different fake.
//! The guarantee is determinism, not idempotence.
//!
//! This module is *not* used for fingerprinting -- that is the normalizer's
//! job. It is applied to `Line::raw` (what gets rendered) after
//! classification, so rule matching sees the original text.
//!
//! # What it redacts
//!
//! IPv4 and IPv6 addresses, MAC addresses, UUIDs, email addresses, URLs,
//! credit-card numbers, and hex dumps.
//!
//! # What it leaves alone
//!
//! Everything else, byte for byte. Text that is not valid UTF-8 cannot reach
//! this module -- the ingest stage replaces it -- and multi-byte characters
//! are copied through as characters, not as bytes.

use xxhash_rust::xxh3::Xxh3;

/// The fake local parts an email address can get, one per hash bucket.
///
/// A local part that is already one of these is left alone, so anonymizing a
/// line twice gives the same answer as anonymizing it once.
const FAKE_USERS: [&str; 8] = [
    "otter", "badger", "wombat", "koala", "panda", "lynx", "orca", "ibex",
];

/// Anonymizes sensitive values in log output.
///
/// Stateless: every fake is derived from the value it replaces, so two
/// anonymizers produce the same output for the same input. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct Anonymizer;

impl Anonymizer {
    /// Create a new anonymizer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Anonymize `text`, replacing sensitive values with fake ones.
    ///
    /// Values already seen map to the same fake value as before.
    #[must_use]
    pub fn anonymize(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let b = text.as_bytes();
        let mut i = 0usize;
        // Where the next verbatim run starts. A replacement ends here, and the
        // text between the end of the last replacement and the start of the
        // next one is copied through untouched.
        let mut copy_from = 0usize;

        while i < b.len() {
            let Some((start, end, replacement)) = match_token(b, i, copy_from) else {
                i += 1;
                continue;
            };
            if start > copy_from {
                // Both offsets are character boundaries: every match starts on
                // an ASCII byte, and ASCII bytes are always boundaries in
                // UTF-8.
                out.push_str(&text[copy_from..start]);
            }
            out.push_str(&replacement);
            i = end;
            copy_from = end;
        }

        if copy_from < b.len() {
            out.push_str(&text[copy_from..]);
        }
        out
    }
}

/// Try to match a sensitive token at byte position `i`.
///
/// Returns the start and end of the match and the replacement text. The
/// start can be *before* `i`: the email matcher starts at the `@` and
/// reaches back over the local part, so the replacement has to begin where
/// the local part does.
///
/// `floor` is the end of the previous match. A matcher must not reach back
/// past it, or the bytes in between would be replaced twice.
fn match_token(b: &[u8], i: usize, floor: usize) -> Option<(usize, usize, String)> {
    let c = b[i];

    // A MAC address is six colon-separated hex pairs. It is checked before the
    // numeric forms because it starts with a hex digit.
    if is_mac_start(c) && is_mac_addr(b, i) {
        return match_mac_addr(b, i);
    }

    // An IPv6 address: colon-separated hex groups. Checked before the
    // numeric forms because it can start with a digit too.
    if c.is_ascii_hexdigit() {
        if let Some(end) = match_ipv6(b, i) {
            let value = std::str::from_utf8(&b[i..end]).ok()?;
            return Some((i, end, fake_ipv6(value)));
        }
    }

    if c.is_ascii_digit() {
        // An IPv4 address: four dotted octets.
        if let Some(end) = match_ipv4(b, i) {
            let value = std::str::from_utf8(&b[i..end]).ok()?;
            return Some((i, end, fake_ipv4(value)));
        }
        // A UUID: eight-four-four-four-twelve hex digits.
        if let Some(end) = match_uuid(b, i) {
            let value = std::str::from_utf8(&b[i..end]).ok()?;
            return Some((i, end, fake_uuid(value)));
        }
        // A credit card: thirteen to nineteen digits that pass Luhn.
        if let Some((start, end, replacement)) = match_credit_card(b, i) {
            return Some((start, end, replacement));
        }
    }

    match c {
        b'<' if is_hex_dump(b, i) => match_hex_dump(b, i),
        b'@' if is_email_at(b, i) => match_email_at(b, i, floor),
        b'e' if is_email_start(b, i) => match_email(b, i),
        b'h' if is_http_url(b, i) => match_http_url(b, i),
        b'/' if is_url_path(b, i) => match_url_path(b, i),
        _ => None,
    }
}

/// Whether `b` at `i` starts an HTTP URL.
fn is_http_url(b: &[u8], i: usize) -> bool {
    b.get(i..i + 7) == Some(b"http://") || b.get(i..i + 8) == Some(b"https://")
}

/// Match an HTTP URL and anonymize its host, user info, path, query and fragment.
fn match_http_url(b: &[u8], i: usize) -> Option<(usize, usize, String)> {
    let scheme_len = if b.get(i..i + 8) == Some(b"https://") {
        8
    } else {
        7
    };
    let after_scheme = i + scheme_len;
    let mut end = after_scheme;
    while end < b.len() && !b[end].is_ascii_whitespace() {
        end += 1;
    }
    let url = std::str::from_utf8(&b[i..end]).ok()?;
    Some((i, end, anonymize_url(url)))
}

/// Whether `b` at `i` looks like the start of an email local part.
fn is_email_start(b: &[u8], i: usize) -> bool {
    // Must start at a word boundary.
    if i > 0 && b.get(i - 1).is_some_and(u8::is_ascii_alphanumeric) {
        return false;
    }
    // Need an @ somewhere ahead with a dot after it.
    let Some(at_offset) = b[i..].iter().position(|&c| c == b'@') else {
        return false;
    };
    let at_pos = i + at_offset;
    let Some(dot_offset) = b[i + at_pos + 1..].iter().position(|&c| c == b'.') else {
        return false;
    };
    (1..=63).contains(&dot_offset)
}

/// Match an email address and anonymize user and host.
fn match_email(b: &[u8], i: usize) -> Option<(usize, usize, String)> {
    let at_offset = b[i..].iter().position(|&c| c == b'@')?;
    let at_pos = i + at_offset;
    let mut end = at_pos + 1;
    while end < b.len() {
        let c = b[end];
        if c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_') {
            end += 1;
        } else {
            break;
        }
    }
    let email = std::str::from_utf8(&b[i..end]).ok()?;
    let (user, host) = email.split_once('@')?;
    Some((i, end, format!("{}@{}", fake_user(user), fake_host(host))))
}

/// Whether `b` at `i` is an `@` that looks like an email separator:
/// alphanumeric before it and a dot after it.
fn is_email_at(b: &[u8], i: usize) -> bool {
    let before = b.get(i.saturating_sub(1));
    let after = b.get(i + 1);
    before.is_some_and(u8::is_ascii_alphanumeric)
        && after.is_some_and(u8::is_ascii_alphanumeric)
        && b[i + 1..].iter().position(|&c| c == b'.').is_some()
}

/// Match an email address starting at the `@` at position `i`.
///
/// The local part is reached back over, and the replacement starts *there*:
/// the whole address has to go, not just the part after the `@`. Reaching
/// back stops at `floor`, so a local part never crosses a previous match.
fn match_email_at(b: &[u8], i: usize, floor: usize) -> Option<(usize, usize, String)> {
    let mut start = i.max(floor);
    while start > floor {
        let c = b[start - 1];
        if c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_') {
            start -= 1;
        } else {
            break;
        }
    }
    let mut end = i + 1;
    while end < b.len() {
        let c = b[end];
        if c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_') {
            end += 1;
        } else {
            break;
        }
    }
    let email = std::str::from_utf8(&b[start..end]).ok()?;
    let (user, host) = email.split_once('@')?;
    Some((
        start,
        end,
        format!("{}@{}", fake_user(user), fake_host(host)),
    ))
}

/// Whether byte `c` can start a MAC address hex digit.
fn is_mac_start(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

/// Whether `b` at `i` looks like a MAC address: six colon-separated hex pairs.
fn is_mac_addr(b: &[u8], i: usize) -> bool {
    let mut j = i;
    for group in 0..6 {
        let end = crate::normalize::hex_run(b, j);
        if end - j != 2 {
            return false;
        }
        j = end;
        if group < 5 {
            if b.get(j) != Some(&b':') {
                return false;
            }
            j += 1;
        }
    }
    // Not followed by another hex digit (that would be a longer run) or by
    // another group.
    b.get(j)
        .is_none_or(|c| !c.is_ascii_hexdigit() && *c != b':')
}

/// Match a MAC address and anonymize it.
fn match_mac_addr(b: &[u8], i: usize) -> Option<(usize, usize, String)> {
    let mut j = i;
    for _ in 0..6 {
        j = crate::normalize::hex_run(b, j);
        if j < b.len() && b[j] == b':' {
            j += 1;
        }
    }
    let mac = std::str::from_utf8(&b[i..j]).ok()?;
    Some((i, j, fake_mac(mac)))
}

/// Whether a `/` at `i` starts a URL path worth anonymizing.
fn is_url_path(b: &[u8], i: usize) -> bool {
    // Only after a space or at the start, and followed by path-like chars.
    if i > 0 && !b.get(i - 1).is_some_and(u8::is_ascii_whitespace) {
        return false;
    }
    b.get(i + 1)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'/')
}

/// Match a URL path and anonymize path segments.
fn match_url_path(b: &[u8], i: usize) -> Option<(usize, usize, String)> {
    let mut end = i;
    while end < b.len() {
        let c = b[end];
        if c.is_ascii_alphanumeric() || matches!(c, b'/' | b'.' | b'_' | b'-' | b'~') {
            end += 1;
        } else if matches!(c, b'?' | b'#') {
            end += 1;
            while end < b.len() && !b[end].is_ascii_whitespace() {
                end += 1;
            }
            break;
        } else {
            break;
        }
    }
    let path = std::str::from_utf8(&b[i..end]).ok()?;
    Some((i, end, anonymize_url_path(path)))
}

/// Luhn check for credit-card validation.
fn luhn_ok(digits: &[u8]) -> bool {
    let mut sum = 0;
    let mut alternate = false;
    for &d in digits.iter().rev() {
        let mut n = u32::from(d - b'0');
        if alternate {
            n *= 2;
            if n > 9 {
                n -= 9;
            }
        }
        sum += n;
        alternate = !alternate;
    }
    sum % 10 == 0
}

/// Match a credit-card number and anonymize it.
fn match_credit_card(b: &[u8], i: usize) -> Option<(usize, usize, String)> {
    let end = crate::normalize::hex_run(b, i);
    let len = end - i;
    if !(13..=19).contains(&len) || !luhn_ok(&b[i..end]) {
        return None;
    }
    // Don't mask if surrounded by alphanumerics (part of a longer number).
    if i > 0 && b.get(i - 1).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    if b.get(end).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    let cc = std::str::from_utf8(&b[i..end]).ok()?;
    Some((i, end, fake_cc(cc)))
}

/// Whether `b` at `i` starts a hex dump line (colon-separated hex bytes).
fn is_hex_dump(b: &[u8], i: usize) -> bool {
    // Match lines like `00000000: 48 65 6c 6c 6f 20 57 6f  68 65 6c 6c 6f`
    let hex_count = crate::normalize::hex_run(b, i);
    hex_count >= 8 && b.get(i + hex_count).is_some_and(|c| *c == b':')
}

/// Match a hex dump line and replace with a hashed replica.
fn match_hex_dump(b: &[u8], i: usize) -> Option<(usize, usize, String)> {
    let mut end = i;
    while end < b.len() && !b[end].is_ascii_whitespace() {
        end += 1;
    }
    let line = std::str::from_utf8(&b[i..end]).ok()?;
    Some((i, end, fake_hex(line)))
}

/// Where a run of decimal digits ends.
fn digits_run(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while b.get(j).is_some_and(u8::is_ascii_digit) {
        j += 1;
    }
    j
}

/// Match four dotted octets, each at most 255.
fn match_ipv4(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    for part in 0..4 {
        let end = digits_run(b, j);
        if end == j || end - j > 3 {
            return None;
        }
        let value = b[j..end]
            .iter()
            .fold(0_u32, |v, c| v * 10 + u32::from(c - b'0'));
        if value > 255 {
            return None;
        }
        j = end;
        if part < 3 {
            if b.get(j) != Some(&b'.') {
                return None;
            }
            j += 1;
        }
    }
    // `1.2.3.4.5` is a version string, not an address.
    if b.get(j).is_some_and(|c| c.is_ascii_digit() || *c == b'.') {
        return None;
    }
    Some(j)
}

/// Match `8-4-4-4-12` hex, in either case.
fn match_uuid(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    for (want, more) in [
        (8_usize, true),
        (4, true),
        (4, true),
        (4, true),
        (12, false),
    ] {
        let end = crate::normalize::hex_run(b, j);
        if end - j != want {
            return None;
        }
        j = end;
        if more {
            if b.get(j) != Some(&b'-') {
                return None;
            }
            j += 1;
        }
    }
    // A longer hex run means we latched onto the front of a bigger token.
    if b.get(j)
        .is_some_and(|c| c.is_ascii_hexdigit() || *c == b'-')
    {
        return None;
    }
    Some(j)
}

/// Anonymize a full URL: scheme + anonymized host + path/query/fragment.
fn anonymize_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    // Host ends at the first /, ? or #.
    let host_end = rest
        .chars()
        .position(|c| matches!(c, '/' | '?' | '#'))
        .unwrap_or(rest.len());
    let (host, rest) = rest.split_at(host_end);
    let fake_host = fake_host(host);
    // rest starts with /, ? or # (or is empty).
    let (path, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, _) = path.split_once('?').unwrap_or((path, ""));
    let mut out = String::with_capacity(url.len());
    if !scheme.is_empty() {
        out.push_str(scheme);
        out.push_str("://");
    }
    out.push_str(&fake_host);
    if !path.is_empty() {
        // No extra separator: `path` still carries the slash that followed
        // the host, and `anonymize_url_path` splits on it, so adding one here
        // would double it.
        out.push_str(&anonymize_url_path(path));
    }
    if !fragment.is_empty() {
        out.push('#');
        out.push_str(&anonymize_url_path(fragment));
    }
    out
}

/// Anonymize a URL path or fragment, segment by segment.
fn anonymize_url_path(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            if seg.is_empty() {
                String::new()
            } else if seg.len() <= 2 || is_fake_segment(seg) {
                seg.to_string()
            } else {
                format!("seg{}", fake_user(seg))
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether a path segment is already a fake this module produced.
fn is_fake_segment(seg: &str) -> bool {
    seg.strip_prefix("seg").is_some_and(is_fake_user)
}

/// Fake user part of an email: deterministic animal name.
fn fake_user(value: &str) -> String {
    if is_fake_user(value) {
        return value.to_string();
    }
    let mut h = Xxh3::new();
    h.update(value.as_bytes());
    let bucket = h.digest() % u64::try_from(FAKE_USERS.len()).unwrap_or(u64::MAX);
    let index = usize::try_from(bucket).unwrap_or_default();
    FAKE_USERS[index].to_string()
}

/// Whether a value is already one of the fake local parts.
fn is_fake_user(value: &str) -> bool {
    FAKE_USERS.contains(&value)
}

/// Fake host part: always example.invalid, so different hosts are
/// indistinguishable but the user part still correlates.
fn fake_host(_value: &str) -> String {
    "example.invalid".to_string()
}

/// Fake IPv4 address: deterministic, in the `10.x.y.z` range.
fn fake_ipv4(value: &str) -> String {
    let mut h = Xxh3::new();
    h.update(value.as_bytes());
    let n = h.digest();
    format!(
        "10.{}.{}.{}",
        (n >> 8) % 256,
        (n >> 16) % 256,
        (n >> 24) % 256
    )
}

/// Match an IPv6 address.
///
/// Either the compressed form, with a `::`, or the full eight groups. The bar
/// is deliberately high. A timestamp like `09:14:02` is two colon-separated
/// hex groups, and mistaking one for an address would rewrite every time in
/// the log -- which is a far worse failure than missing an address.
fn match_ipv6(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    let mut groups = 0usize;
    let mut compressed = false;
    loop {
        let start = j;
        while j < b.len() && b[j].is_ascii_hexdigit() {
            j += 1;
        }
        let len = j - start;
        // A group is at most four hex digits.
        if len > 4 {
            return None;
        }
        if len > 0 {
            groups += 1;
        }
        if b.get(j) != Some(&b':') {
            break;
        }
        j += 1;
        if b.get(j) == Some(&b':') {
            // Only one `::` per address.
            if compressed {
                return None;
            }
            compressed = true;
            j += 1;
        }
    }
    if compressed || groups == 8 {
        Some(j)
    } else {
        None
    }
}

/// Fake IPv6 address: deterministic, in the documentation range.
fn fake_ipv6(value: &str) -> String {
    let mut h = Xxh3::new();
    h.update(value.as_bytes());
    let n = h.digest();
    format!(
        "2001:db8:{:x}:{:x}::{:x}",
        n % 65536,
        (n >> 16) % 65536,
        (n >> 32) % 65536
    )
}

/// Fake UUID: deterministic, and still shaped like a UUID.
fn fake_uuid(value: &str) -> String {
    let mut h = Xxh3::new();
    h.update(value.as_bytes());
    let n = h.digest();
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        n >> 32,
        n >> 16,
        n >> 8,
        n,
        n
    )
}

/// Fake MAC address: deterministic, in the 00:00:5E:00:53:00 range.
fn fake_mac(value: &str) -> String {
    let mut h = Xxh3::new();
    h.update(value.as_bytes());
    let n = h.digest();
    format!(
        "00:00:5E:{:02x}:{:02x}:{:02x}",
        (n >> 24) % 256,
        (n >> 16) % 256,
        n % 256
    )
}

/// Fake credit card: deterministic 16-digit number.
///
/// The fake is deliberately **not** Luhn-valid, which does two things: it does
/// not look like a working card number to anything downstream, and a second
/// pass recognises it as already redacted and leaves it alone.
fn fake_cc(value: &str) -> String {
    let mut h = Xxh3::new();
    h.update(value.as_bytes());
    let mut n = h.digest();
    let mut digits = String::with_capacity(16);
    for _ in 0..15 {
        digits.push(char::from(b'0' + (n % 10) as u8));
        n /= 10;
    }
    let check = (0..10)
        .map(|d| char::from(b'0' + d))
        .find(|d| !luhn_ok(format!("{digits}{d}").as_bytes()))
        .unwrap_or('0');
    digits.push(check);
    digits
}

/// Fake hex dump: hash of input, repeated to the same length.
fn fake_hex(value: &str) -> String {
    let mut h = Xxh3::new();
    h.update(value.as_bytes());
    let mut n = h.digest();
    let mut digits = String::with_capacity(16);
    for _ in 0..16 {
        digits.push(char::from(b'0' + (n % 10) as u8));
        n /= 10;
    }
    let repeat_count = value.len() / 16 + 1;
    digits
        .repeat(repeat_count)
        .chars()
        .take(value.len())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anon(text: &str) -> String {
        Anonymizer::new().anonymize(text)
    }

    #[test]
    fn anonymizes_ipv4() {
        let a = anon("connect 10.0.12.255 refused");
        let b = anon("connect 192.168.1.1 refused");
        assert!(a.starts_with("connect 10."), "{a}");
        assert!(!a.contains("10.0.12.255"), "{a}");
        assert_ne!(a, b);
        // Deterministic across anonymizers.
        assert_eq!(a, anon("connect 10.0.12.255 refused"));
    }

    #[test]
    fn anonymizes_a_uuid() {
        let out = anon("request 550e8400-e29b-41d4-a716-446655440000 timed out");
        assert!(!out.contains("550e8400"), "{out}");
        assert!(out.contains('-'), "a fake UUID keeps its shape: {out}");
        assert_eq!(
            out,
            anon("request 550e8400-e29b-41d4-a716-446655440000 timed out")
        );
    }

    #[test]
    fn anonymizes_email() {
        let out = anon("mail from alice@example.com");
        assert!(out.contains("@example.invalid"), "{out}");
        assert!(!out.contains("example.com"), "{out}");
        // The whole address goes, local part included. Leaving the original
        // local part in place and appending a fake one to it is worse than not
        // redacting at all: it looks like an address and it is half real.
        assert!(!out.contains("alice"), "{out}");
    }

    #[test]
    fn anonymizes_http_url() {
        let out = anon("GET https://internal.host/api/v1/users?id=5 HTTP/1.1");
        assert!(out.contains("example.invalid"), "{out}");
        assert!(!out.contains("internal.host"), "{out}");
        assert!(!out.contains("users"), "{out}");
    }

    #[test]
    fn anonymizes_mac() {
        let out = anon("mac 00:1a:2b:3c:4d:5e up");
        assert!(out.starts_with("mac 00:00:5E:"), "{out}");
        assert!(!out.contains("1a:2b:3c"), "{out}");
    }

    #[test]
    fn anonymizes_credit_card() {
        let out = anon("card 4111111111111111 used");
        assert_eq!(out.len(), "card 4111111111111111 used".len());
        assert!(!out.contains("4111111111111111"), "{out}");
    }

    #[test]
    fn leaves_plain_text_alone() {
        assert_eq!(anon("hello world"), "hello world");
        assert_eq!(anon("error: could not compile"), "error: could not compile");
    }

    #[test]
    fn anonymizes_ipv6() {
        let out = anon("peer 2001:db8::1 connected");
        assert!(!out.contains("db8::1"), "{out}");
        assert!(out.contains("2001:db8:"), "a fake keeps its shape: {out}");
        // The full eight-group form.
        let out = anon("peer 2001:0db8:85a3:0000:0000:8a2e:0370:7334 connected");
        assert!(!out.contains("85a3"), "{out}");
    }

    #[test]
    fn leaves_a_timestamp_alone() {
        // `09:14:02` has hex groups and colons, but it is not an address.
        let out = anon("step [09:14:02] done");
        assert_eq!(out, "step [09:14:02] done");
        // A full timestamp with a date in front of it.
        let out = anon("2026-10-12T09:14:02Z started");
        assert_eq!(out, "2026-10-12T09:14:02Z started");
    }

    #[test]
    fn keeps_multi_byte_characters_intact() {
        // Copying bytes one at a time turns every byte above 0x7f into a
        // separate character, which turns "café" into mojibake.
        let out = anon("café naïve — unicode text");
        assert_eq!(out, "café naïve — unicode text");
    }

    #[test]
    fn deterministic() {
        // The guarantee: the same sensitive value always maps to the same
        // fake. Two runs over the same log produce the same output, so two
        // runs can be diffed.
        for input in [
            "connect 192.168.1.1 refused",
            "mail from alice@example.com",
            "GET https://internal.host/api/v1/users?id=5",
            "request 550e8400-e29b-41d4-a716-446655440000 timed out",
            "mac 00:1a:2b:3c:4d:5e up",
            "card 4111111111111111 used",
        ] {
            assert_eq!(anon(input), anon(input), "not deterministic for {input:?}");
        }
    }

    #[test]
    fn idempotent_where_the_fake_is_recognisable() {
        // A fake email local part, a fake URL segment and a fake card number
        // are all recognisable as fakes, so a second pass leaves them alone.
        for input in [
            "mail from alice@example.com",
            "GET https://internal.host/api/v1/users?id=5",
            "card 4111111111111111 used",
        ] {
            let once = anon(input);
            assert_eq!(once, anon(&once), "not idempotent for {input:?}");
        }
    }

    #[test]
    fn distinct_values_get_distinct_fakes() {
        // The other half of the promise: two different addresses must not
        // collapse into one, or a reader could no longer tell them apart.
        let a = anon("connect 10.0.12.255 refused");
        let b = anon("connect 192.168.1.1 refused");
        assert_ne!(a, b);
        let c = anon("request 550e8400-e29b-41d4-a716-446655440000 timed out");
        let d = anon("request 6ba7b810-9dad-11d1-80b4-00c04fd430c8 timed out");
        assert_ne!(c, d);
    }
}
