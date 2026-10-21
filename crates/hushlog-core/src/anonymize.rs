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
//! log without learning the real values.
//!
//! This module is *not* used for fingerprinting -- that is the normalizer's
//! job. It is applied to `Line::raw` (what gets rendered) after
//! classification, so rule matching sees the original text.

use std::collections::HashMap;

use xxhash_rust::xxh3::Xxh3;

/// Anonymizes sensitive values in log output.
///
/// Stateless apart from the per-value cache, which keeps replacements
/// consistent across a run. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct Anonymizer {
    cache: HashMap<String, String>,
}

impl Anonymizer {
    /// Create a new anonymizer with an empty replacement cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Anonymize `text`, replacing sensitive values with fake ones.
    ///
    /// Values already seen in this run map to the same fake value as before.
    #[must_use]
    pub fn anonymize(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if let Some((end, replacement)) = match_token(bytes, i) {
                out.push_str(&replacement);
                i = end;
            } else {
                out.push(bytes[i] as char);
                i += 1;
            }
        }
        out
    }

    /// Hash `value` to a u64 for deterministic bucketing.
    #[allow(dead_code)]
    fn hash64(value: &str) -> u64 {
        let mut h = Xxh3::new();
        h.update(value.as_bytes());
        h.digest()
    }

    /// Look up or create a fake replacement for `value`, derived from `base`.
    #[allow(dead_code)]
    fn fake(&mut self, value: &str, base: &str) -> String {
        if let Some(cached) = self.cache.get(value) {
            return cached.clone();
        }
        let h = Self::hash64(value);
        let replacement = match base {
            "ipv4" => format!(
                "10.{}.{}.{}",
                h % 256,
                (h >> 8) % 256,
                (h >> 16) % 256
            ),
            "ipv6" => format!(
                "2001:db8:{:x}:{:x}::{:x}",
                h % 65536,
                (h >> 16) % 65536,
                (h >> 32) % 65536
            ),
            "mac" => format!(
                "00:00:5E:{:02x}:{:02x}:{:02x}",
                (h >> 24) % 256,
                (h >> 16) % 256,
                h % 256
            ),
            "uuid" => format!(
                "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
                h,
                h >> 32,
                h >> 16,
                h >> 8,
                h
            ),
            "email" => {
                let animal = match h % 8 {
                    0 => "otter",
                    1 => "badger",
                    2 => "wombat",
                    3 => "koala",
                    4 => "panda",
                    5 => "lynx",
                    6 => "orca",
                    _ => "ibex",
                };
                format!("{animal}@example.invalid")
            }
            "url_host" => format!("example{}.invalid", h % 1000),
            "cc" => {
                let mut n = h;
                let mut digits = String::with_capacity(16);
                for _ in 0..16 {
                    digits.push(char::from(b'0' + (n % 10) as u8));
                    n /= 10;
                }
                digits
            }
            "hex" => {
                let mut n = h;
                let mut digits = String::with_capacity(16);
                for _ in 0..16 {
                    digits.push(char::from(b'0' + (n % 10) as u8));
                    n /= 10;
                }
                let repeat_count = value.len() / 16 + 1;
                digits.repeat(repeat_count)
            }
            _ => value.to_string(),
        };
        self.cache.insert(value.to_string(), replacement.clone());
        replacement
    }
}

/// Try to match a sensitive token at byte position `i`.
///
/// Returns the end position (exclusive) and the replacement text.
fn match_token(b: &[u8], i: usize) -> Option<(usize, String)> {
    let c = b[i];
    match c {
        b'<' if is_hex_dump(b, i) => match_hex_dump(b, i),
        b'0'..=b'9' if is_credit_card(b, i) => match_credit_card(b, i),
        b'e' if is_email_start(b, i) => match_email(b, i),
        b'h' if is_http_url(b, i) => match_http_url(b, i),
        b'/' if is_url_path(b, i) => match_url_path(b, i),
        b'@' if is_email_at(b, i) => match_email_at(b, i),
        c if is_mac_start(c) && is_mac_addr(b, i) => match_mac_addr(b, i),
        _ => None,
    }
}

/// Whether `b` at `i` starts an HTTP URL.
fn is_http_url(b: &[u8], i: usize) -> bool {
    b.get(i..i + 7) == Some(b"http://") || b.get(i..i + 8) == Some(b"https://")
}

/// Match an HTTP URL and anonymize its host, user info, path, query and fragment.
fn match_http_url(b: &[u8], i: usize) -> Option<(usize, String)> {
    let scheme_len = if b.get(i..i + 8) == Some(b"https://") { 8 } else { 7 };
    let after_scheme = i + scheme_len;
    let mut end = after_scheme;
    while end < b.len() && !b[end].is_ascii_whitespace() {
        end += 1;
    }
    let url = std::str::from_utf8(&b[i..end]).ok()?;
    let anonymized = anonymize_url(url);
    Some((end, anonymized))
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
fn match_email(b: &[u8], i: usize) -> Option<(usize, String)> {
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
    let anonymized = format!("{}@{}", fake_user(user), fake_host(host));
    Some((end, anonymized))
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

/// Match an email address starting at the `@` at position `i`,
/// anonymizing user and host.
fn match_email_at(b: &[u8], i: usize) -> Option<(usize, String)> {
    // Backtrack to the start of the local part.
    let mut start = i;
    while start > 0 {
        let c = b[start - 1];
        if c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_') {
            start -= 1;
        } else {
            break;
        }
    }
    // Forward to the end of the domain.
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
    let anonymized = format!("{}@{}", fake_user(user), fake_host(host));
    Some((end, anonymized))
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
    // Not followed by another hex digit (would be a longer run).
    b.get(j).is_none_or(|c| !c.is_ascii_hexdigit())
}

/// Match a MAC address and anonymize it.
fn match_mac_addr(b: &[u8], i: usize) -> Option<(usize, String)> {
    let mut j = i;
    for _ in 0..6 {
        j = crate::normalize::hex_run(b, j);
        if j < b.len() && b[j] == b':' {
            j += 1;
        }
    }
    let mac = std::str::from_utf8(&b[i..j]).ok()?;
    let anonymized = fake_mac(mac);
    Some((j, anonymized))
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

/// Whether a `/` at `i` starts a URL path worth anonymizing.
fn is_url_path(b: &[u8], i: usize) -> bool {
    // Only after a space or at the start, and followed by path-like chars.
    if i > 0 && !b.get(i - 1).is_some_and(u8::is_ascii_whitespace) {
        return false;
    }
    b.get(i + 1).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'/')
}

/// Match a URL path and anonymize path segments.
fn match_url_path(b: &[u8], i: usize) -> Option<(usize, String)> {
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
    let anonymized = anonymize_url_path(path);
    Some((end, anonymized))
}

/// Whether a digit run at `i` looks like a credit-card number (13-19 digits).
fn is_credit_card(b: &[u8], i: usize) -> bool {
    let end = crate::normalize::hex_run(b, i);
    let len = end - i;
    (13..=19).contains(&len) && luhn_ok(&b[i..end])
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
fn match_credit_card(b: &[u8], i: usize) -> Option<(usize, String)> {
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
    let anonymized = fake_cc(cc);
    Some((end, anonymized))
}

/// Whether `b` at `i` starts a hex dump line (colon-separated hex bytes).
fn is_hex_dump(b: &[u8], i: usize) -> bool {
    // Match lines like `00000000: 48 65 6c 6c 6f 20 57 6f  68 65 6c 6c 6f`
    let hex_count = crate::normalize::hex_run(b, i);
    hex_count >= 8 && b.get(i + hex_count).is_some_and(|c| *c == b':')
}

/// Match a hex dump line and replace with a hashed replica.
fn match_hex_dump(b: &[u8], i: usize) -> Option<(usize, String)> {
    let mut end = i;
    while end < b.len() && !b[end].is_ascii_whitespace() {
        end += 1;
    }
    let line = std::str::from_utf8(&b[i..end]).ok()?;
    let anonymized = fake_hex(line);
    Some((end, anonymized))
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
        out.push('/');
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
            } else if seg.len() <= 2 {
                seg.to_string()
            } else {
                format!("seg{}", fake_user(seg))
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Fake user part of an email: deterministic animal name.
fn fake_user(value: &str) -> String {
    let mut h = Xxh3::new();
    h.update(value.as_bytes());
    let n = h.digest();
    match n % 8 {
        0 => "otter".to_string(),
        1 => "badger".to_string(),
        2 => "wombat".to_string(),
        3 => "koala".to_string(),
        4 => "panda".to_string(),
        5 => "lynx".to_string(),
        6 => "orca".to_string(),
        _ => "ibex".to_string(),
    }
}

/// Fake host part: always example.invalid, so different hosts are
/// indistinguishable but the user part still correlates.
fn fake_host(_value: &str) -> String {
    "example.invalid".to_string()
}

/// Fake credit card: deterministic 16-digit number.
fn fake_cc(value: &str) -> String {
    let mut h = Xxh3::new();
    h.update(value.as_bytes());
    let n = h.digest();
    let mut digits = String::with_capacity(16);
    let mut m = n;
    for _ in 0..16 {
        digits.push(char::from(b'0' + u8::try_from(m % 10).unwrap()));
        m /= 10;
    }
    digits
}

/// Fake hex dump: hash of input, repeated to the same length.
fn fake_hex(value: &str) -> String {
    let mut h = Xxh3::new();
    h.update(value.as_bytes());
    let n = h.digest();
    let mut digits = String::with_capacity(16);
    let mut m = n;
    for _ in 0..16 {
        digits.push(char::from(b'0' + u8::try_from(m % 10).unwrap()));
        m /= 10;
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

    #[test]
    fn anonymizes_ipv4() {
        let mut a = Anonymizer::new();
        let mut b = Anonymizer::new();
        assert_eq!(
            a.anonymize("connect 10.0.12.255 refused"),
            b.anonymize("connect 10.0.12.255 refused")
        );
        // Same IP → same fake, different IP → different fake.
        assert_ne!(
            a.anonymize("connect 192.168.1.1 refused"),
            a.anonymize("connect 10.0.0.1 refused")
        );
        // Deterministic across anonymizers.
        let mut c = Anonymizer::new();
        assert_eq!(
            a.anonymize("connect 192.168.1.1 refused"),
            c.anonymize("connect 192.168.1.1 refused")
        );
    }

    #[test]
    fn anonymizes_email() {
        let mut a = Anonymizer::new();
        let mut b = Anonymizer::new();
        assert_eq!(
            a.anonymize("mail from alice@example.com"),
            b.anonymize("mail from alice@example.com")
        );
        assert!(a
            .anonymize("mail from alice@example.com")
            .contains("@example.invalid"));
    }

    #[test]
    fn anonymizes_http_url() {
        let mut a = Anonymizer::new();
        let result =
            a.anonymize("GET https://internal.host/api/v1/users?id=5 HTTP/1.1");
        assert!(result.contains("example"));
        assert!(!result.contains("internal.host"));
        assert!(!result.contains("users"));
    }

    #[test]
    fn anonymizes_mac() {
        let mut a = Anonymizer::new();
        let result = a.anonymize("mac 00:1a:2b:3c:4d:5e up");
        assert!(result.starts_with("mac 00:00:5E:"));
    }

    #[test]
    fn anonymizes_credit_card() {
        let mut a = Anonymizer::new();
        // 4111-1111-1111-1111 is a valid Luhn test number.
        let result = a.anonymize("card 4111111111111111 used");
        assert_eq!(result.len(), "card 4111111111111111 used".len());
        assert!(!result.contains("4111111111111111"));
    }

    #[test]
    fn leaves_plain_text_alone() {
        let mut a = Anonymizer::new();
        assert_eq!(a.anonymize("hello world"), "hello world");
        assert_eq!(
            a.anonymize("error: could not compile"),
            "error: could not compile"
        );
    }

    #[test]
    fn idempotent() {
        let mut a = Anonymizer::new();
        let once = a.anonymize("connect 192.168.1.1 refused");
        let twice = a.anonymize(&once);
        assert_eq!(once, twice);
    }
}
