//! Stage 2 -- Normalize and fingerprint. **Owner: WP2.**
//!
//! Masks volatile tokens -- timestamps, UUIDs, hashes, IPs, numbers, absolute
//! paths -- so that two lines differing only in those tokens hash to the same
//! fingerprint. `Retry 3/10 for pod abc-123` and `Retry 4/10 for pod xyz-789`
//! must produce the same `u64`.
//!
//! Contract: `Classified`-shaped input in, fingerprint out. Masking is for
//! matching only and must never be rendered -- [`Line::raw`] is the single
//! source of truth for display.
//!
//! # How it works
//!
//! [`scan`] walks the line once. Bytes that cannot begin a token are copied
//! through in runs; at every position that can begin one, an ordered set of
//! byte matchers is tried and the winner becomes a placeholder. Matching is
//! ASCII-only and byte-oriented, so no UTF-8 boundary is ever split and no
//! regular expression is involved -- this crate needs no `regex` dependency.
//!
//! Two properties are load-bearing, and both are covered by tests:
//!
//! * **Stability** -- lines differing only in masked tokens fingerprint
//!   identically. This is the whole point of the module.
//! * **Idempotence** -- `mask(mask(x)) == mask(x)`. Placeholders contain no
//!   digits and no token separators, so a second pass finds nothing left to
//!   mask. A fixed point is what lets the collapser treat a fingerprint as a
//!   property of the text rather than of the run order.
//!
//! Deliberately *not* masked, because over-masking merges errors that a human
//! needs to tell apart: relative paths (`src/main.rs`), URLs, hyphenated prose
//! (`use-std`), and hex runs shorter than [`MIN_HASH_LEN`].

use std::borrow::Cow;

use xxhash_rust::xxh3::Xxh3;

/// Replacement for a date or a time.
const PH_TIMESTAMP: &str = "<ts>";
/// Replacement for a UUID.
const PH_UUID: &str = "<uuid>";
/// Replacement for a long run of hex digits.
const PH_HASH: &str = "<hash>";
/// Replacement for an IPv4 address.
const PH_IP: &str = "<ip>";
/// Replacement for a bare number.
const PH_NUM: &str = "<num>";
/// Replacement for a generated identifier such as `abc-123`.
const PH_ID: &str = "<id>";
/// Replacement for an absolute path.
const PH_PATH: &str = "<path>";

/// Shortest hex run treated as a hash rather than as a word.
///
/// Eight digits is where English runs out: `deadbeef` is a hash, `defaced` is
/// a word.
const MIN_HASH_LEN: usize = 8;

/// Shortest run that counts as an absolute path rather than a lone root.
const MIN_PATH_SEPARATORS: usize = 2;

/// One masked token: the byte range it covered and what replaced it.
#[derive(Debug, Clone, Copy)]
struct Token {
    /// Where the token began. Never before the last masked token ended.
    start: usize,
    /// One past the last byte of the token.
    end: usize,
    /// The literal written in its place.
    placeholder: &'static str,
}

impl Token {
    fn new(start: usize, end: usize, placeholder: &'static str) -> Self {
        Self {
            start,
            end,
            placeholder,
        }
    }
}

/// Volatile-token masking and fingerprinting. See module docs.
///
/// Stateless, so it is `Send + Sync + Copy`-cheap to share across threads.
#[derive(Debug, Default, Clone, Copy)]
pub struct Normalizer;

impl Normalizer {
    /// Create a normalizer with default masks.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Fingerprint one line: mask the volatile tokens, then hash what is left.
    ///
    /// This is the hot path. It never allocates -- the masked bytes are fed
    /// straight into `xxh3` -- so the only cost per line is the scan itself.
    ///
    /// ```
    /// use hushlog_core::Normalizer;
    ///
    /// let n = Normalizer::new();
    /// let a = n.fingerprint("Retry 3/10 for pod abc-123");
    /// let b = n.fingerprint("Retry 4/10 for pod xyz-789");
    /// assert_eq!(a, b);
    /// ```
    #[must_use]
    pub fn fingerprint(&self, plain: &str) -> u64 {
        let mut hash = Xxh3::new();
        let mut sink = |chunk: &str| hash.update(chunk.as_bytes());
        if !scan(plain, &mut sink) {
            // Nothing was masked, so `scan` had nothing to flush. Feed the
            // line whole rather than hashing an empty input.
            hash.update(plain.as_bytes());
        }
        hash.digest()
    }

    /// The masked form of a line, exposed for tests and `--explain`.
    ///
    /// Borrows the input when there was nothing to mask. **Never render this.**
    /// It exists to compare two lines or to show a user why two lines were
    /// treated as the same; the text a user should read is
    /// [`Line::raw`](crate::Line::raw), which `--tee` also writes verbatim.
    ///
    /// ```
    /// use hushlog_core::Normalizer;
    ///
    /// assert_eq!(Normalizer::new().mask("built at 2026-10-12"), "built at <ts>");
    /// ```
    #[must_use]
    pub fn mask<'a>(&self, plain: &'a str) -> Cow<'a, str> {
        let mut buf = String::new();
        let mut sink = |chunk: &str| buf.push_str(chunk);
        if scan(plain, &mut sink) {
            Cow::Owned(buf)
        } else {
            Cow::Borrowed(plain)
        }
    }
}

/// Walk `s` once, handing verbatim runs and placeholders to `sink`.
///
/// Returns whether anything was masked. The sink is never called at all when
/// nothing was, which is what lets [`Normalizer::mask`] hand back a borrow
/// instead of an allocation.
fn scan(s: &str, sink: &mut impl FnMut(&str)) -> bool {
    let b = s.as_bytes();
    let mut copy_from = 0;
    let mut masked = false;
    let mut i = 0;

    while i < b.len() {
        if !is_stop(b, i) {
            i += 1;
            continue;
        }
        let Some(token) = match_token(b, i, copy_from) else {
            i += 1;
            continue;
        };
        if token.start > copy_from {
            sink(&s[copy_from..token.start]);
        }
        sink(token.placeholder);
        masked = true;
        i = token.end;
        copy_from = token.end;
    }

    if masked {
        sink(&s[copy_from..]);
    }
    masked
}

/// Try to mask a token at `i`, first reaching back over the identifier run.
///
/// `floor` is the end of the token masked just before us; the search for a
/// token start never crosses it, so text is never masked twice.
fn match_token(b: &[u8], i: usize, floor: usize) -> Option<Token> {
    // A token can begin with a letter the stop set stopped short of:
    // `dead1234...` and `deadbeef-1111-...`. Walk back over the identifier
    // run and prefer that reading, so the whole token is masked rather than
    // the numeric tail of it.
    if starts_a_token(b, i) {
        let mut start = i;
        while start > floor && is_word_join(b[start - 1]) {
            start -= 1;
        }
        if start < i {
            if let Some(token) = match_at(b, start, floor) {
                return Some(token);
            }
        }
    }
    match_at(b, i, floor)
}

/// Whether reaching back from `i` could find a token starting earlier.
///
/// A token often opens with a letter the stop set cannot see: `dead1234...`
/// behind its first digit, `abc-123` behind its hyphen. A `:` or a path
/// separator opens something far more rarely -- only behind a drive letter or
/// a hex letter -- so `scheduler:` must not pay to walk back over the whole
/// word.
fn starts_a_token(b: &[u8], i: usize) -> bool {
    if b[i].is_ascii_digit() || b[i].is_ascii_alphabetic() || b[i] == b'-' {
        return true;
    }
    i.checked_sub(1)
        .map(|k| b[k])
        .is_some_and(|prev| prev.is_ascii_hexdigit() || prev == b'-' || prev == b'_')
}

/// Every token shape, dispatched on the byte that opens it.
///
/// The byte class alone rules most shapes out, and the order within a class
/// is what keeps the specific readings: `2026-10-12` is a date and not five
/// numbers, `1.2.3.4` an address and not a decimal.
fn match_at(b: &[u8], i: usize, floor: usize) -> Option<Token> {
    let c = b[i];
    if c.is_ascii_digit() {
        match_digit_token(b, i)
    } else if c.is_ascii_alphabetic() {
        match_alpha_token(b, i, floor, hex_run(b, i))
    } else if c == b'/' || c == b'\\' {
        match_path(b, i, floor).map(|end| Token::new(i, end, PH_PATH))
    } else {
        // `:` on its own opens nothing: it is only ever part of a time, a
        // drive letter or a zone, and each of those is matched from its start.
        None
    }
}

/// A token opening with a digit.
///
/// One scan of the digit run serves every shape that begins with digits, and
/// the byte that ends the run says which shape to try. A bare number is by far
/// the commonest and costs nothing beyond that one scan.
fn match_digit_token(b: &[u8], i: usize) -> Option<Token> {
    let num_end = digits_run(b, i);
    let len = num_end - i;

    match b.get(num_end) {
        // `2026-10-12`, `8-4-4-4-12`, `123-abc`.
        Some(b'-') => {
            if len == 4 {
                if let Some(date_end) = match_date(b, num_end) {
                    return Some(Token::new(i, match_time_after(b, date_end), PH_TIMESTAMP));
                }
            }
            if len == 8 {
                if let Some(end) = match_uuid(b, i) {
                    return Some(Token::new(i, end, PH_UUID));
                }
            }
            match_slug(b, i).map(|end| Token::new(i, end, PH_ID))
        }
        // `10.0.0.1`, or a decimal.
        Some(b'.') => {
            if len <= 3 {
                if let Some(end) = match_ipv4(b, i) {
                    return Some(Token::new(i, end, PH_IP));
                }
            }
            None
        }
        // `09:14:02Z`.
        Some(b':') => {
            if len <= 2 {
                if let Some(end) = match_time(b, num_end) {
                    return Some(Token::new(i, match_zone(b, end), PH_TIMESTAMP));
                }
            }
            None
        }
        // Nothing else joins with an underscore.
        Some(b'_') => match_slug(b, i).map(|end| Token::new(i, end, PH_ID)),
        // The word carries on past the digits: `550e8400-...` is a UUID, and
        // `1234abcd` a hash, but the digit run alone would read as a number.
        Some(c) if c.is_ascii_alphanumeric() => {
            match_uuid(b, i).map(|end| Token::new(i, end, PH_UUID))
        }
        _ => None,
    }
    .or_else(|| match_hash(b, i).map(|end| Token::new(i, end, PH_HASH)))
    .or_else(|| Some(Token::new(i, number_tail(b, num_end), PH_NUM)))
}

/// A token opening with a letter.
///
/// `hex_end` is measured once and reused: it decides the UUID and hash
/// readings and bounds both, so the common case -- an ordinary word that
/// happens to contain `a` through `f` -- costs two scans and stops there.
fn match_alpha_token(b: &[u8], i: usize, floor: usize, hex_end: usize) -> Option<Token> {
    if hex_end - i == 8 && b.get(hex_end) == Some(&b'-') {
        if let Some(end) = match_uuid(b, i) {
            return Some(Token::new(i, end, PH_UUID));
        }
    }
    match_slug(b, i)
        .map(|end| Token::new(i, end, PH_ID))
        .or_else(|| {
            let is_hash = hex_end - i >= MIN_HASH_LEN && hex_end == alnum_run(b, i);
            is_hash.then(|| Token::new(i, hex_end, PH_HASH))
        })
        .or_else(|| match_path(b, i, floor).map(|end| Token::new(i, end, PH_PATH)))
}

/// `8-4-4-4-12` hex, in either case.
fn match_uuid(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    for (want, more) in [(8, true), (4, true), (4, true), (4, true), (12, false)] {
        let end = hex_run(b, j);
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

/// The `T09:14:02Z` or ` 09:14:02` that may follow a date.
fn match_time_after(b: &[u8], date_end: usize) -> usize {
    if !matches!(b.get(date_end), Some(b'T' | b't' | b' ')) {
        return date_end;
    }
    let hour_end = digits_run(b, date_end + 1);
    match match_time(b, hour_end) {
        Some(end) => match_zone(b, end),
        None => date_end,
    }
}

/// `YYYY-MM-DD`, refusing anything that continues into more digits.
///
/// `year_end` is the end of the four digit year, already measured by the
/// caller along with the `-` that has to follow it.
fn match_date(b: &[u8], year_end: usize) -> Option<usize> {
    let month_end = digits_run(b, year_end + 1);
    if month_end - year_end - 1 != 2 || b.get(month_end) != Some(&b'-') {
        return None;
    }
    let end = digits_run(b, month_end + 1);
    if end - month_end - 1 != 2 {
        return None;
    }
    if b.get(end).is_some_and(|c| c.is_ascii_digit() || *c == b'-') {
        return None;
    }
    Some(end)
}

/// `HH:MM:SS`, `H:MM:SS`, optionally with fractional seconds.
///
/// `hour_end` is the end of the hour, already measured by the caller, as is
/// the `:` that has to follow it.
fn match_time(b: &[u8], hour_end: usize) -> Option<usize> {
    let min_end = digits_run(b, hour_end + 1);
    if min_end - hour_end - 1 != 2 || b.get(min_end) != Some(&b':') {
        return None;
    }
    let mut end = digits_run(b, min_end + 1);
    if end - min_end - 1 != 2 {
        return None;
    }
    if b.get(end) == Some(&b'.') {
        let frac_end = digits_run(b, end + 1);
        if frac_end > end + 1 {
            end = frac_end;
        }
    }
    if b.get(end).is_some_and(|c| c.is_ascii_digit() || *c == b':') {
        return None;
    }
    Some(end)
}

/// `Z`, `+02:00`, `+0200`. Anything unrecognised leaves the time alone.
fn match_zone(b: &[u8], end: usize) -> usize {
    match b.get(end) {
        Some(b'Z' | b'z') => end + 1,
        Some(&sign) if sign == b'+' || sign == b'-' => {
            let hour_end = digits_run(b, end + 1);
            if hour_end - end - 1 != 2 {
                return end;
            }
            match b.get(hour_end) {
                Some(b':') => {
                    let min_end = digits_run(b, hour_end + 1);
                    if min_end - hour_end - 1 == 2 {
                        min_end
                    } else {
                        end
                    }
                }
                Some(c) if c.is_ascii_digit() => {
                    let min_end = digits_run(b, hour_end);
                    if min_end - hour_end == 2 {
                        min_end
                    } else {
                        end
                    }
                }
                _ => hour_end,
            }
        }
        _ => end,
    }
}

/// Four dotted octets, each at most 255.
fn match_ipv4(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    for part in 0..4 {
        let end = digits_run(b, j);
        if !(1..=3).contains(&(end - j)) || octet_value(&b[j..end]) > 255 {
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

/// A hyphen- or underscore-joined token in which at least one segment looks
/// machine-generated: `abc-123`, `checkout-api-7f9c4d8b6-x2k9m`.
///
/// Segments that are all plain words are structure, not noise, and stay put:
/// `use-std` must not fold into `use-libc`.
fn match_slug(b: &[u8], i: usize) -> Option<usize> {
    if !b.get(i).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    let mut segments = 0_usize;
    let mut volatile = false;
    let mut j = i;
    loop {
        let end = alnum_run(b, j);
        segments += 1;
        volatile |= is_volatile_segment(&b[j..end]);
        j = end;
        let joined = matches!(b.get(j), Some(b'-' | b'_'))
            && b.get(j + 1).is_some_and(u8::is_ascii_alphanumeric);
        if !joined {
            break;
        }
        j += 1;
    }
    (segments >= 2 && volatile).then_some(j)
}

/// Whether one segment of a slug looks machine-generated.
fn is_volatile_segment(seg: &[u8]) -> bool {
    let any = |f: fn(&u8) -> bool| seg.iter().any(f);
    let all = |f: fn(&u8) -> bool| seg.iter().all(f);
    all(u8::is_ascii_digit) || (any(u8::is_ascii_digit) && any(u8::is_ascii_alphabetic)) || {
        all(u8::is_ascii_hexdigit) && seg.len() >= MIN_HASH_LEN
    }
}

/// A hex run of at least [`MIN_HASH_LEN`] digits, and the whole of its word.
fn match_hash(b: &[u8], i: usize) -> Option<usize> {
    let end = hex_run(b, i);
    (end - i >= MIN_HASH_LEN && end == alnum_run(b, i)).then_some(end)
}

/// The rest of a number: the fractional part after a run of digits.
fn number_tail(b: &[u8], num_end: usize) -> usize {
    if b.get(num_end) == Some(&b'.') {
        let frac_end = digits_run(b, num_end + 1);
        if frac_end > num_end + 1 {
            frac_end
        } else {
            num_end
        }
    } else {
        num_end
    }
}

/// A Windows drive path or a POSIX/UNC absolute path.
fn match_path(b: &[u8], i: usize, floor: usize) -> Option<usize> {
    if let Some(end) = match_drive_path(b, i) {
        return Some(end);
    }
    let lead = match b.get(i) {
        Some(b'/') => 1,
        Some(b'\\') if b.get(i + 1) == Some(&b'\\') => 2,
        _ => return None,
    };
    // `http://host/a/b` is a URL, and the `/b` of `src/main.rs` is relative.
    if matches!(b.get(i + lead), Some(b'/' | b'\\')) || !path_starts_here(b, i, floor) {
        return None;
    }
    let end = path_run(b, i + lead);
    // A root plus one separator. Anything shorter is a fragment.
    let separators = b[i..end]
        .iter()
        .filter(|c| matches!(c, b'/' | b'\\'))
        .count();
    (separators >= MIN_PATH_SEPARATORS).then_some(end)
}

/// `C:\src\main.rs` or `C:/src/main.rs`.
fn match_drive_path(b: &[u8], i: usize) -> Option<usize> {
    if !b.get(i).is_some_and(u8::is_ascii_alphabetic)
        || b.get(i + 1) != Some(&b':')
        || !matches!(b.get(i + 2), Some(b'\\' | b'/'))
    {
        return None;
    }
    let end = path_run(b, i + 3);
    // `C:\` on its own is a shell prompt, not a path.
    (end > i + 3).then_some(end)
}

/// Whether an absolute path may begin at `i`.
///
/// A path directly after an alphanumeric is the tail of something else --
/// `foo/bar`, `12/a/b` -- unless that alphanumeric is part of a token this
/// pass already masked, in which case what follows really is a path. The
/// distinction is what keeps masking idempotent: the second pass sees `>`
/// where the first saw a digit.
fn path_starts_here(b: &[u8], i: usize, floor: usize) -> bool {
    match i.checked_sub(1).map(|k| b[k]) {
        None => true,
        Some(b'/' | b'\\' | b':') => false,
        Some(prev) => !prev.is_ascii_alphanumeric() || i == floor,
    }
}

/// Whether byte `i` might begin a token.
///
/// Hex letters count, because they open hashes and UUIDs -- `deadbeef` is a
/// hash and `aaaaaaaa` is one too. Any other letter counts only when a `-`
/// follows, which is what catches `abc-123` without making every letter of
/// every word a stop.
fn is_stop(b: &[u8], i: usize) -> bool {
    let c = b[i];
    c.is_ascii_hexdigit()
        || matches!(c, b':' | b'/' | b'\\')
        || (c.is_ascii_alphabetic() && b.get(i + 1) == Some(&b'-'))
}

/// Bytes that may appear inside an identifier, including its separators.
const fn is_word_join(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

/// Bytes that may appear inside a path. Notably not space, which keeps
/// `at /usr/bin tool` from swallowing `tool`.
const fn is_path_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b'/' | b'\\' | b':')
}

fn digits_run(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while b.get(j).is_some_and(u8::is_ascii_digit) {
        j += 1;
    }
    j
}

pub(crate) fn hex_run(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while b.get(j).is_some_and(u8::is_ascii_hexdigit) {
        j += 1;
    }
    j
}

fn alnum_run(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while b.get(j).is_some_and(u8::is_ascii_alphanumeric) {
        j += 1;
    }
    j
}

fn path_run(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while b.get(j).is_some_and(|c| is_path_char(*c)) {
        j += 1;
    }
    j
}

fn octet_value(digits: &[u8]) -> u16 {
    digits
        .iter()
        .fold(0_u16, |v, c| v * 10 + u16::from(c - b'0'))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::{
        BoxedStrategy, ProptestConfig, Strategy, any, prop, prop_assert_eq, proptest,
    };

    use super::*;

    fn norm() -> Normalizer {
        Normalizer::new()
    }

    // ---- one test per token class -------------------------------------

    #[test]
    fn masks_iso_timestamps() {
        assert_eq!(
            norm().mask("2026-10-12T09:14:02.418+02:00 booting"),
            "<ts> booting"
        );
        assert_eq!(norm().mask("built at 2026-10-12"), "built at <ts>");
        assert_eq!(norm().mask("step [09:14:02.418] done"), "step [<ts>] done");
    }

    #[test]
    fn masks_uuids() {
        assert_eq!(
            norm().mask("request 550e8400-e29b-41d4-a716-446655440000 timed out"),
            "request <uuid> timed out"
        );
        // A UUID whose first group starts with a hex letter still matches whole.
        assert_eq!(
            norm().mask("request deadbeef-1111-2222-3333-444444444444 ok"),
            "request <uuid> ok"
        );
    }

    #[test]
    fn masks_long_hex_hashes() {
        assert_eq!(
            norm().mask("commit dead1234abcdef0123 checked out"),
            "commit <hash> checked out"
        );
        assert_eq!(norm().mask("sha 1234567890abcdef"), "sha <hash>");
        // A hash made only of letters is still a hash.
        assert_eq!(norm().mask("sha deadbeef"), "sha <hash>");
        // Six hex digits is a word, not a hash.
        assert_eq!(norm().mask("defaced"), "defaced");
    }

    #[test]
    fn masks_ipv4_addresses() {
        assert_eq!(
            norm().mask("connect 10.0.12.255 refused"),
            "connect <ip> refused"
        );
        assert_eq!(
            norm().mask("peer 192.168.1.1:8080 up"),
            "peer <ip>:<num> up"
        );
        // Five dotted groups is a version string, not an address: each
        // decimal group masks as a number instead.
        assert_eq!(norm().mask("v1.2.3.4.5"), "v<num>.<num>.<num>");
    }

    #[test]
    fn masks_bare_numbers() {
        assert_eq!(norm().mask("retry 7 in 1.5s"), "retry <num> in <num>s");
        assert_eq!(norm().mask("processed 0 items"), "processed <num> items");
    }

    #[test]
    fn masks_absolute_paths() {
        assert_eq!(
            norm().mask("cannot open /usr/local/lib/node_modules/x.js"),
            "cannot open <path>"
        );
        assert_eq!(
            norm().mask(r"cannot open C:\Users\dev\project\main.rs"),
            "cannot open <path>"
        );
        // UNC shares are absolute paths too.
        assert_eq!(norm().mask(r"at \\build01\share\out.txt"), "at <path>");
        // A URL is not a path, and neither is a relative one.
        assert_eq!(
            norm().mask("see http://example.com/a/b"),
            "see http://example.com/a/b"
        );
        assert_eq!(norm().mask("in src/main.rs"), "in src/main.rs");
        // A path right after a masked number is still a path: the first pass
        // has to see what the second one would see, or masking is not a fixed
        // point.
        assert_eq!(norm().mask("cd 12/a/b/c"), "cd <num><path>");
    }

    #[test]
    fn masks_generated_identifiers() {
        assert_eq!(
            norm().mask("Retry 3/10 for pod abc-123"),
            "Retry <num>/<num> for pod <id>"
        );
        assert_eq!(norm().mask("pod checkout-api-7f9c4d8b6-x2k9m"), "pod <id>");
        // Plain hyphenated words are structure, not noise.
        assert_eq!(norm().mask("retry-storm"), "retry-storm");
        assert_eq!(norm().mask("use-std"), "use-std");
    }

    // ---- the properties this module exists for ------------------------

    /// The headline case from the module docs.
    #[test]
    fn volatile_lines_share_a_fingerprint() {
        let n = norm();
        let a = n.mask("Retry 3/10 for pod abc-123");
        let b = n.mask("Retry 4/10 for pod xyz-789");
        assert_eq!(a, b, "both lines should mask to the same text");
        assert_eq!(
            n.fingerprint("Retry 3/10 for pod abc-123"),
            n.fingerprint("Retry 4/10 for pod xyz-789")
        );
    }

    /// The same property on lines shaped like the real retry-storm fixture.
    #[test]
    fn retry_storm_lines_share_a_fingerprint() {
        let n = norm();
        let a = "2026-10-12T09:14:03Z WARN  scheduler: Retry 3/10 for pod \
                 checkout-api-7f9c4d8b6-x2k9m after backoff 200ms";
        let b = "2026-10-12T09:14:04Z WARN  scheduler: Retry 4/10 for pod \
                 checkout-api-5d81f6c94-w7t2p after backoff 400ms";
        let c = "2026-10-12T09:14:05Z WARN  scheduler: Retry 5/10 for pod \
                 inventory-api-2c8e3f6a9-d4k7m after backoff 800ms";
        assert_eq!(n.mask(a), n.mask(b));
        assert_eq!(n.mask(b), n.mask(c));
        assert_eq!(n.fingerprint(a), n.fingerprint(b));
        assert_eq!(n.fingerprint(b), n.fingerprint(c));
    }

    /// Masking must not be so aggressive that distinct lines merge.
    #[test]
    fn distinct_lines_get_distinct_fingerprints() {
        let n = norm();
        let corpus = [
            "",
            "connection refused",
            "permission denied",
            "thread panicked at src/main.rs",
            "thread panicked at src/lib.rs",
            "error[E0308]: mismatched types",
            "error[E0433]: failed to resolve: use std::collections::HashMap",
            "Compiling serde v1.0.200",
        ];
        for (i, a) in corpus.iter().enumerate() {
            for b in &corpus[i + 1..] {
                assert_ne!(n.fingerprint(a), n.fingerprint(b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn masking_is_deterministic() {
        let a = norm();
        let b = norm();
        let line = "2026-10-12T09:14:02Z ERR abc-123 10.0.0.1 /a/b/c";
        assert_eq!(a.mask(line), b.mask(line));
        assert_eq!(a.fingerprint(line), b.fingerprint(line));
    }

    /// Nothing masked means nothing allocated.
    #[test]
    fn mask_borrows_when_there_is_nothing_to_mask() {
        assert!(matches!(
            norm().mask("nothing volatile here"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(norm().mask("2026-10-12"), Cow::Owned(_)));
    }

    // ---- property tests -----------------------------------------------

    fn hex_str(len: std::ops::Range<usize>) -> BoxedStrategy<String> {
        let digits: Vec<char> = "0123456789abcdef".chars().collect();
        prop::collection::vec(prop::sample::select(digits), len)
            .prop_map(|cs| cs.into_iter().collect::<String>())
            .boxed()
    }

    fn uuid_str() -> BoxedStrategy<String> {
        (
            hex_str(8..9),
            hex_str(4..5),
            hex_str(4..5),
            hex_str(4..5),
            hex_str(12..13),
        )
            .prop_map(|(a, b, c, d, e)| format!("{a}-{b}-{c}-{d}-{e}"))
            .boxed()
    }

    fn timestamp() -> BoxedStrategy<String> {
        (
            1970u32..3000,
            1u32..=12,
            1u32..=28,
            0u32..=23,
            0u32..=59,
            0u32..=59,
            0u32..1000,
        )
            .prop_map(|(y, mo, d, h, mi, s, ms)| {
                if ms == 0 {
                    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
                } else {
                    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{ms:03}Z")
                }
            })
            .boxed()
    }

    fn ipv4() -> BoxedStrategy<String> {
        (0u8..=255, 0u8..=255, 0u8..=255, 0u8..=255)
            .prop_map(|(a, b, c, d)| format!("{a}.{b}.{c}.{d}"))
            .boxed()
    }

    fn abs_path() -> BoxedStrategy<String> {
        prop::sample::select(vec![
            "/usr/local/lib/node_modules/pkg/build/out.js".to_string(),
            "/tmp/hushlog-run-9".to_string(),
            r"C:\Users\dev\project\src\main.rs".to_string(),
        ])
        .boxed()
    }

    fn any_text() -> BoxedStrategy<String> {
        prop::collection::vec(any::<char>(), 0..80)
            .prop_map(|cs| cs.into_iter().collect::<String>())
            .boxed()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1024))]

        /// The headline property, generalised: swapping every masked token for
        /// a different one must not change the fingerprint.
        #[test]
        fn volatile_tokens_do_not_change_the_fingerprint(
            ts1 in timestamp(),
            ts2 in timestamp(),
            uid1 in uuid_str(),
            uid2 in uuid_str(),
            h1 in hex_str(8..40),
            h2 in hex_str(8..40),
            ip1 in ipv4(),
            ip2 in ipv4(),
            n1 in any::<u64>(),
            n2 in any::<u64>(),
            p1 in abs_path(),
            p2 in abs_path(),
        ) {
            let n = norm();
            let a = format!("{ts1} WARN worker {n1} lost {p1} (conn {ip1} id {uid1} build {h1})");
            let b = format!("{ts2} WARN worker {n2} lost {p2} (conn {ip2} id {uid2} build {h2})");
            prop_assert_eq!(
                n.mask(&a).into_owned(),
                n.mask(&b).into_owned(),
                "\n a = {:?}\n b = {:?}",
                a,
                b
            );
            prop_assert_eq!(n.fingerprint(&a), n.fingerprint(&b), "\n a = {:?}\n b = {:?}", a, b);
        }

        /// Same input, same answer, no hidden state creeping into the hot path.
        #[test]
        fn masking_is_stateless(text in any_text()) {
            let a = norm();
            let b = norm();
            prop_assert_eq!(a.mask(&text).into_owned(), b.mask(&text).into_owned());
            prop_assert_eq!(a.fingerprint(&text), b.fingerprint(&text));
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4096))]

        /// Masking must be a fixed point, whatever the input. Without this a
        /// fold group could depend on the order lines happened to arrive in.
        #[test]
        fn masking_is_idempotent(text in any_text()) {
            let n = norm();
            let once = n.mask(&text).into_owned();
            let twice = n.mask(&once).into_owned();
            prop_assert_eq!(
                &once,
                &twice,
                "\n input = {:?}\n once  = {:?}\n twice = {:?}",
                text,
                once,
                twice
            );
        }
    }
}
