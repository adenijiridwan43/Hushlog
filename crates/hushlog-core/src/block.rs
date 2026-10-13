//! Stage 4b -- Multi-line block detection. **Owner: WP4.**
//!
//! Detects units that span several lines -- stack traces, indented
//! continuations, deprecation notices with URLs -- via indentation heuristics
//! plus rule-defined markers, and fingerprints the block as a whole.
//!
//! The detector is a pure state machine over [`Line`]s: it never buffers the
//! lines it has seen (it folds them into a running hash as they arrive), so its
//! memory cost is one hash state plus a few counters regardless of block size.
//! The [`Collapser`](crate::Collapser) is the consumer: it drives the detector,
//! buffers the lines of the block that is currently open, and emits
//! [`Event::BlockStart`](crate::Event::BlockStart) /
//! [`Event::BlockEnd`](crate::Event::BlockEnd) around them once the block's
//! fingerprint is known.
//!
//! This module is pre-declared so that no parallel agent needs to edit
//! `lib.rs`. Implement `BlockDetector` here.

use std::fmt;

use xxhash_rust::xxh3::Xxh3;

use crate::{Line, Stream};

/// Domain separator, so a block fingerprint can never collide with the line
/// fingerprints computed by `collapse`.
const BLOCK_DOMAIN: u8 = 0x42;

/// How many unindented, unmarked lines a *marker-opened* block may still
/// swallow before it is considered finished.
///
/// A Rust panic is the reason this exists:
///
/// ```text
/// thread 'main' panicked at src/main.rs:5:9:
/// assertion failed: a == b          <-- column zero, but part of the block
/// stack backtrace:
/// ```
///
/// The payload line carries no indentation and no marker. One grace line is
/// enough to absorb it, and one is few enough that unrelated log lines are not
/// swallowed. Blocks opened by indentation get no grace at all.
const PAYLOAD_GRACE: u8 = 1;

/// How many leading ` ` or `\t` characters a line has.
fn leading_whitespace(text: &str) -> usize {
    text.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// A deprecation notice or documentation pointer is treated as block content
/// even when nothing else about the line is indented.
fn has_url(text: &str) -> bool {
    text.contains("http://") || text.contains("https://")
}

fn to_owned(markers: &[&str]) -> Vec<String> {
    markers.iter().map(|m| (*m).to_string()).collect()
}

/// What [`BlockDetector::push`] did with a line.
///
/// A single line can close one block *and* open the next, which is why
/// [`closed`](Self::closed) is reported independently of
/// [`opened`](Self::opened) / [`joined`](Self::joined).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BlockUpdate {
    /// A block closed immediately *before* this line was considered, carrying
    /// that block's final fingerprint. `None` if nothing closed.
    pub closed: Option<u64>,
    /// This line opened a new block.
    pub opened: bool,
    /// This line was appended to the block that was already open.
    pub joined: bool,
}

/// The marker sets used when no rule overrides them.
///
/// Substring matching on [`Line::plain`], i.e. after ANSI stripping and -- once
/// stage 2 is wired in -- after volatile tokens have been masked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BlockMarkers {
    starts: Vec<String>,
    continues: Vec<String>,
    ends: Vec<String>,
}

impl BlockMarkers {
    fn builtins() -> Self {
        Self {
            starts: to_owned(BlockDetector::BUILTIN_START_MARKERS),
            continues: to_owned(BlockDetector::BUILTIN_CONTINUE_MARKERS),
            ends: to_owned(BlockDetector::BUILTIN_END_MARKERS),
        }
    }

    fn matches_start(&self, text: &str) -> bool {
        self.starts.iter().any(|m| text.contains(m.as_str()))
    }

    fn matches_continue(&self, text: &str) -> bool {
        self.continues.iter().any(|m| text.contains(m.as_str()))
    }

    fn matches_end(&self, text: &str) -> bool {
        self.ends.iter().any(|m| text.contains(m.as_str()))
    }
}

/// Pattern markers, for rules that need more than a fixed substring.
///
/// There are only ever a handful of these -- one per rule pack that declares
/// them -- so a small `Vec` scanned linearly is the right structure. The
/// substring sets above stay the fast path, and this is only consulted when
/// they miss.
#[derive(Debug, Default)]
struct RegexMarkers {
    starts: Vec<regex::Regex>,
    ends: Vec<regex::Regex>,
}

impl RegexMarkers {
    fn matches_start(&self, text: &str) -> bool {
        self.starts.iter().any(|r| r.is_match(text))
    }

    fn matches_end(&self, text: &str) -> bool {
        self.ends.iter().any(|r| r.is_match(text))
    }

    fn is_empty(&self) -> bool {
        self.starts.is_empty() && self.ends.is_empty()
    }
}

/// Streaming multi-line block detector. See module docs.
///
/// A block is open when the detector believes the current line belongs to a
/// multi-line unit. Lines keep it open while they are indented, carry a
/// continuation marker, or carry a URL; a blank line, a dedent to column zero,
/// an end marker, a stream switch or [`max_lines`](Self::with_max_lines)
/// closes it.
///
/// # Example
///
/// ```
/// use hushlog_core::{BlockDetector, Line, Stream};
///
/// let mut det = BlockDetector::new();
/// let up = det.push(&Line::new(
///     "  warning: deprecated",
///     "  warning: deprecated",
///     Stream::Stdout,
///     1,
/// ));
/// assert!(up.opened);
/// assert!(det.force_close().is_some());
/// ```
pub struct BlockDetector {
    markers: BlockMarkers,
    /// Pattern markers from rule packs. Empty unless a rule declared one.
    regex: RegexMarkers,
    max_lines: usize,
    open: bool,
    /// Set by an end marker: the block closes once this line has been absorbed.
    closing_after: bool,
    grace: u8,
    lines: usize,
    start_no: u64,
    stream: Stream,
    hash: Xxh3,
}

impl Default for BlockDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for BlockDetector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockDetector")
            .field("max_lines", &self.max_lines)
            .field("open", &self.open)
            .field("closing_after", &self.closing_after)
            .field("lines", &self.lines)
            .field("start_no", &self.start_no)
            .field("stream", &self.stream)
            .finish_non_exhaustive()
    }
}

impl BlockDetector {
    /// Default upper bound on the lines in one block.
    ///
    /// Bounded so that a log which never dedents cannot grow the consumer's
    /// buffer without limit.
    pub const DEFAULT_MAX_BLOCK_LINES: usize = 64;

    /// Substrings that open a block even at column zero.
    pub const BUILTIN_START_MARKERS: &'static [&'static str] = &[
        "panicked at",
        "Traceback (most recent call last):",
        "Exception in thread",
        "During handling of the above exception",
        "The above exception was the direct cause",
        "The following exception was the direct cause",
        "error: ",
        "warning: ",
    ];

    /// Substrings that keep an open block alive even at column zero.
    pub const BUILTIN_CONTINUE_MARKERS: &'static [&'static str] = &[
        "stack backtrace:",
        " --> ",
        " = note:",
        " = help:",
        "Caused by:",
        "Suppressed:",
    ];

    /// Substrings that belong to the block but close it once absorbed.
    pub const BUILTIN_END_MARKERS: &'static [&'static str] = &[
        "note: run with `RUST_BACKTRACE=1`",
        "note: this error originates in",
        "note: required by",
        "read the full backtrace",
        "end of the program",
    ];

    /// Create a detector with default heuristics.
    #[must_use]
    pub fn new() -> Self {
        Self::blank(BlockMarkers::builtins(), Self::DEFAULT_MAX_BLOCK_LINES)
    }

    /// The single struct literal every constructor funnels through.
    fn blank(markers: BlockMarkers, max_lines: usize) -> Self {
        Self {
            markers,
            regex: RegexMarkers::default(),
            max_lines: max_lines.max(1),
            open: false,
            closing_after: false,
            grace: 0,
            lines: 0,
            start_no: 0,
            stream: Stream::Stdout,
            hash: Xxh3::new(),
        }
    }

    /// Create a detector whose marker sets replace the built-in ones.
    ///
    /// This is the rules' override point: pass `(&[], &[], &[])` to reduce the
    /// detector to pure indentation, or pass project-specific markers.
    #[must_use]
    pub fn with_markers(starts: &[&str], continues: &[&str], ends: &[&str]) -> Self {
        Self::blank(
            BlockMarkers {
                starts: to_owned(starts),
                continues: to_owned(continues),
                ends: to_owned(ends),
            },
            Self::DEFAULT_MAX_BLOCK_LINES,
        )
    }

    /// Add one marker to the start set.
    pub fn add_start_marker(&mut self, marker: &str) -> &mut Self {
        self.markers.starts.push(marker.to_string());
        self
    }

    /// Add one marker to the continuation set.
    pub fn add_continue_marker(&mut self, marker: &str) -> &mut Self {
        self.markers.continues.push(marker.to_string());
        self
    }

    /// Add one marker to the end set.
    pub fn add_end_marker(&mut self, marker: &str) -> &mut Self {
        self.markers.ends.push(marker.to_string());
        self
    }

    /// Add a **pattern** marker to the start set.
    ///
    /// For rules whose block boundaries vary in a way no fixed substring can
    /// capture -- `^\s*Type\s+Reason\s+Age` matches a table header whatever the
    /// column padding is.
    ///
    /// # Errors
    ///
    /// Returns the compile error if the pattern is not a valid regex. A caller
    /// gets a loud failure rather than a marker that silently never fires.
    pub fn try_add_regex_start_marker(&mut self, pattern: &str) -> Result<&mut Self, regex::Error> {
        self.regex.starts.push(regex::Regex::new(pattern)?);
        Ok(self)
    }

    /// Add a **pattern** marker to the end set. See
    /// [`try_add_regex_start_marker`](Self::try_add_regex_start_marker).
    ///
    /// # Errors
    ///
    /// Returns the compile error if the pattern is not a valid regex.
    pub fn try_add_regex_end_marker(&mut self, pattern: &str) -> Result<&mut Self, regex::Error> {
        self.regex.ends.push(regex::Regex::new(pattern)?);
        Ok(self)
    }

    /// Whether any pattern marker has been added.
    #[must_use]
    pub fn has_regex_markers(&self) -> bool {
        !self.regex.is_empty()
    }

    /// Cap how many lines one block may hold. Values below 1 are clamped to 1.
    #[must_use]
    pub fn with_max_lines(mut self, max_lines: usize) -> Self {
        self.max_lines = max_lines.max(1);
        self
    }

    /// The line-number cap currently in force.
    #[must_use]
    pub fn max_lines(&self) -> usize {
        self.max_lines
    }

    /// Feed one line; returns what the detector did with it.
    ///
    /// Cheap and allocation-free: the line's `plain` text is folded straight
    /// into a running hash and then dropped.
    #[must_use]
    pub fn push(&mut self, line: &Line) -> BlockUpdate {
        let mut closed = None;
        // A pending end marker, or a change of stream in wrap mode, closes the
        // block before this line is judged.
        if self.open && (self.closing_after || self.stream != line.stream) {
            closed = self.close_block();
        }

        let text = line.plain.as_str();
        let blank = text.trim().is_empty();
        let indented = leading_whitespace(text) > 0;
        let starts = !blank && (self.markers.matches_start(text) || self.regex.matches_start(text));
        let continues = !blank && self.markers.matches_continue(text);
        let ends = !blank && (self.markers.matches_end(text) || self.regex.matches_end(text));
        let links = !blank && has_url(text);

        if self.open {
            let joins = if blank {
                false
            } else if starts || continues || ends || links || indented {
                true
            } else {
                // Column zero, no marker: only a marker-opened block may claim
                // it, and only while it still has payload grace left.
                let granted = self.grace > 0;
                self.grace = self.grace.saturating_sub(1);
                granted
            };

            if joins && self.lines < self.max_lines {
                if ends {
                    self.closing_after = true;
                }
                self.push_text(text);
                return BlockUpdate {
                    closed,
                    opened: false,
                    joined: true,
                };
            }
            if let Some(fp) = self.close_block() {
                closed = Some(fp);
            }
        }

        if !blank && (starts || links || indented) {
            self.start_block(line.no, line.stream, starts);
            self.push_text(text);
            return BlockUpdate {
                closed,
                opened: true,
                joined: false,
            };
        }

        BlockUpdate {
            closed,
            opened: false,
            joined: false,
        }
    }

    /// Whether a block is currently open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Lines absorbed into the block that is currently open.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines
    }

    /// Whether no block is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines == 0
    }

    /// Line number, in the original stream, of the open block's first line.
    #[must_use]
    pub fn start_no(&self) -> u64 {
        self.start_no
    }

    /// Fingerprint the block being built, as far as it has been read.
    ///
    /// Equal to what [`force_close`](Self::force_close) will report for that
    /// block. `None` when no block is open.
    #[must_use]
    pub fn open_fingerprint(&self) -> Option<u64> {
        if !self.open {
            return None;
        }
        let mut hash = self.hash.clone();
        hash.update(&u64::try_from(self.lines).unwrap_or(u64::MAX).to_le_bytes());
        Some(hash.digest())
    }

    /// Close the open block at end of input and report its fingerprint.
    ///
    /// Returns `None` when no block is open, so it is safe to call twice.
    pub fn force_close(&mut self) -> Option<u64> {
        self.close_block()
    }

    /// Forget the current block, keeping the configured markers and line cap.
    pub fn reset(&mut self) {
        let markers = std::mem::take(&mut self.markers);
        let max_lines = self.max_lines;
        *self = Self::blank(markers, max_lines);
    }

    fn start_block(&mut self, no: u64, stream: Stream, by_marker: bool) {
        self.open = true;
        self.closing_after = false;
        // Only a marker says "more of this is coming"; indentation alone is a
        // weaker promise and must not eat the following column-zero line.
        self.grace = if by_marker { PAYLOAD_GRACE } else { 0 };
        self.lines = 0;
        self.start_no = no;
        self.stream = stream;
        self.hash.reset();
        self.hash.update(&[BLOCK_DOMAIN]);
    }

    fn push_text(&mut self, text: &str) {
        // Length-prefixed so that ("ab","c") and ("a","bc") cannot collide.
        let len = u64::try_from(text.len()).unwrap_or(u64::MAX);
        self.hash.update(&len.to_le_bytes());
        self.hash.update(text.as_bytes());
        self.lines += 1;
    }

    fn close_block(&mut self) -> Option<u64> {
        if !self.open {
            return None;
        }
        let count = u64::try_from(self.lines).unwrap_or(u64::MAX);
        self.hash.update(&count.to_le_bytes());
        let fp = self.hash.digest();
        self.open = false;
        self.closing_after = false;
        self.grace = 0;
        self.lines = 0;
        Some(fp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(no: u64, text: &str) -> Line {
        Line::new(text.to_string(), text.to_string(), Stream::Stdout, no)
    }

    fn feed(det: &mut BlockDetector, lines: &[&str]) -> Vec<BlockUpdate> {
        lines
            .iter()
            .enumerate()
            .map(|(i, text)| det.push(&line(i as u64 + 1, text)))
            .collect()
    }

    fn rust_panic() -> Vec<&'static str> {
        vec![
            "thread 'main' panicked at src/main.rs:5:9:",
            "assertion failed: a == b",
            "stack backtrace:",
            "   0: rust_begin_unwind",
            "             at ./library/core/src/panishing.rs:590:5",
        ]
    }

    #[test]
    fn indented_continuations_form_one_block() {
        let mut det = BlockDetector::new();
        let ups = feed(&mut det, &["  first", "    deeper", " \ttabbed"]);
        assert!(ups[0].opened, "{ups:?}");
        assert!(ups[1].joined && ups[2].joined, "{ups:?}");
        assert!(det.is_open());
        assert_eq!(det.len(), 3);
        assert_eq!(det.start_no(), 1);

        let ups = feed(&mut det, &["back to zero"]);
        assert!(ups[0].closed.is_some(), "{ups:?}");
        assert!(!det.is_open());
        assert_eq!(det.len(), 0);
    }

    #[test]
    fn a_rustc_diagnostic_is_one_block() {
        let mut det = BlockDetector::new();
        let ups = feed(
            &mut det,
            &[
                "warning: unused variable: `x`",
                " --> src/lib.rs:9:9",
                "  |",
                "9 |     let x = 1;",
                "  |         ^ help: prefix it with an underscore",
            ],
        );
        assert!(ups[0].opened, "{ups:?}");
        assert!(ups[1..].iter().all(|u| u.joined), "{ups:?}");
        assert_eq!(det.len(), 5);
        assert!(det.open_fingerprint().is_some());

        let ups = feed(&mut det, &["help: consider prefixing"]);
        assert!(
            ups[0].closed.is_some(),
            "column zero without a marker ends the block: {ups:?}"
        );
        assert!(!det.is_open());
    }

    #[test]
    fn a_dedent_closes_the_block() {
        let mut det = BlockDetector::new();
        let ups = feed(
            &mut det,
            &["  indented opener", "  continuation", "back to column zero"],
        );
        assert!(ups[2].closed.is_some(), "{ups:?}");
        assert!(!ups[2].joined && !ups[2].opened);
        assert!(!det.is_open());
    }

    #[test]
    fn a_blank_line_closes_the_block() {
        let mut det = BlockDetector::new();
        let ups = feed(&mut det, &["  indented opener", "   ", "next thing"]);
        assert!(ups[1].closed.is_some(), "{ups:?}");
        assert!(!ups[1].joined, "a blank line is never part of a block");
        assert!(!det.is_open());
    }

    #[test]
    fn stack_traces_are_one_block() {
        let mut det = BlockDetector::new();
        let lines = rust_panic();
        let ups = feed(&mut det, &lines);
        assert!(ups[0].opened, "{ups:?}");
        assert!(
            ups[1..].iter().all(|u| u.joined),
            "payload grace must cover the message: {ups:?}"
        );
        assert_eq!(det.len(), lines.len());
    }

    #[test]
    fn identical_blocks_share_a_fingerprint() {
        let mut a = BlockDetector::new();
        let mut b = BlockDetector::new();
        feed(&mut a, &rust_panic());
        let fp_a = a.force_close();
        feed(&mut b, &rust_panic());
        let fp_b = b.force_close();
        assert!(fp_a.is_some());
        assert_eq!(
            fp_a, fp_b,
            "structurally identical blocks must collapse together"
        );
    }

    #[test]
    fn the_open_fingerprint_matches_the_final_one() {
        let mut det = BlockDetector::new();
        feed(&mut det, &rust_panic());
        let open = det.open_fingerprint();
        assert_eq!(open, det.force_close());
    }

    #[test]
    fn different_blocks_have_different_fingerprints() {
        let mut a = BlockDetector::new();
        let mut b = BlockDetector::new();
        feed(&mut a, &rust_panic());
        feed(
            &mut b,
            &["  a totally different block", "   with other content"],
        );
        assert_ne!(a.force_close(), b.force_close());
    }

    #[test]
    fn line_order_is_part_of_the_fingerprint() {
        let mut a = BlockDetector::new();
        let mut b = BlockDetector::new();
        feed(&mut a, &["  one", "  two"]);
        feed(&mut b, &["  two", "  one"]);
        assert_ne!(a.force_close(), b.force_close());
    }

    #[test]
    fn an_end_marker_joins_then_closes() {
        let mut det = BlockDetector::new();
        let ups = feed(
            &mut det,
            &[
                "thread 'main' panicked at src/main.rs:5:9:",
                "note: this error originates in `src/main.rs`",
                "compiling something else",
            ],
        );
        assert!(ups[1].joined, "the note belongs to the panic: {ups:?}");
        assert!(ups[2].closed.is_some(), "{ups:?}");
        assert!(!det.is_open());
    }

    #[test]
    fn a_capped_block_closes_before_overflowing() {
        let mut det = BlockDetector::new().with_max_lines(2);
        let ups = feed(
            &mut det,
            &["  one", "   two", "   three", "   four", "  five", "   six"],
        );
        assert!(ups[1].joined, "{ups:?}");
        assert!(ups[2].closed.is_some() && ups[2].opened, "{ups:?}");
        assert!(ups[4].closed.is_some() && ups[4].opened, "{ups:?}");
        assert_eq!(det.max_lines(), 2);
        assert!(
            det.len() <= det.max_lines(),
            "the cap must never be exceeded: {ups:?}"
        );
    }

    #[test]
    fn urls_join_a_deprecation_notice() {
        let mut det = BlockDetector::new();
        let ups = feed(
            &mut det,
            &["see https://example.invalid/docs for details", "plain line"],
        );
        assert!(ups[0].opened, "a URL line opens a block: {ups:?}");
        assert!(ups[1].closed.is_some(), "{ups:?}");
    }

    #[test]
    fn a_stream_change_closes_the_block() {
        let mut det = BlockDetector::new();
        let _ = det.push(&line(1, "  indented opener"));
        let text = "  from stderr";
        let other = Line::new(text.to_string(), text.to_string(), Stream::Stderr, 2);
        let up = det.push(&other);
        assert!(up.closed.is_some(), "{up:?}");
        assert!(up.opened, "the stderr line starts its own block: {up:?}");
    }

    #[test]
    fn force_close_is_a_no_op_when_nothing_is_open() {
        let mut det = BlockDetector::new();
        feed(&mut det, &["just a line"]);
        assert_eq!(det.force_close(), None);
        assert_eq!(det.open_fingerprint(), None);
    }

    #[test]
    fn reset_returns_to_a_clean_state() {
        let mut det = BlockDetector::with_markers(&["<<<"], &[], &[">>>"]).with_max_lines(3);
        feed(&mut det, &["<<< open"]);
        det.reset();
        assert!(!det.is_open());
        assert_eq!(det.max_lines(), 3);
        let ups = feed(&mut det, &["  still block forming"]);
        assert!(ups[0].opened, "markers survive a reset: {ups:?}");
    }

    #[test]
    fn markers_can_be_overridden() {
        let mut det = BlockDetector::with_markers(&["<<<"], &[], &[">>>"]);
        let ups = feed(
            &mut det,
            &[
                "plain",
                "<<< begin",
                "still going",
                ">>> end",
                "plain again",
            ],
        );
        assert!(
            !ups[0].opened,
            "the built-in start markers must be gone: {ups:?}"
        );
        assert!(ups[1].opened, "{ups:?}");
        assert!(ups[2].joined);
        assert!(ups[3].joined && ups[4].closed.is_some(), "{ups:?}");
    }

    #[test]
    fn added_markers_extend_the_builtins() {
        let mut det = BlockDetector::new();
        det.add_start_marker("<<<")
            .add_continue_marker("|||")
            .add_end_marker(">>>");
        let ups = feed(&mut det, &["  opener", "||| continues", ">>> ends", "next"]);
        assert!(ups[1].joined && ups[2].joined, "{ups:?}");
        assert!(ups[3].closed.is_some(), "{ups:?}");
    }
}
