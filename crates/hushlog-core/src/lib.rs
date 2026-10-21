//! Pure pipeline core for **hushlog**.
//!
//! This crate holds the contracts every other crate is built against. It
//! performs no I/O, spawns no processes, reads no clock and no environment.
//! It takes data and returns data. See `AGENTS.md` section 3.1.
//!
//! # Pipeline
//!
//! ```text
//! [1] Ingest -> [2] Normalize -> [3] Classify -> [4] Group -> [5] Render
//! ```
//!
//! Stages 1-4 live in this crate and communicate only through the traits
//! [`Classifier`], [`Collapser`] and [`Renderer`]. Stage 5 and all process
//! handling live elsewhere.

#![forbid(unsafe_code)]

mod anonymize;
mod block;
mod collapse;
mod format;
mod ingest;
mod normalize;

pub use anonymize::Anonymizer;
pub use block::BlockDetector;
pub use collapse::FoldingCollapser;
pub use format::FormatDetector;
pub use ingest::Ingest;
pub use normalize::Normalizer;

/// Which of a child process's output streams a line came from.
///
/// Wrap mode captures stdout and stderr separately so that interleaving stays
/// interpretable; see the pipeline note in `AGENTS.md` section 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// One line of input, after framing but before classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The line exactly as it arrived, with ANSI escapes intact.
    ///
    /// This is what gets rendered and what `--tee` writes. Never mutate it.
    pub raw: String,
    /// `raw` with ANSI escapes stripped, used for matching only.
    pub plain: String,
    /// Origin stream in wrap mode; `Stdout` in pipe mode.
    pub stream: Stream,
    /// 1-based line number from the start of the stream.
    pub no: u64,
}

impl Line {
    /// Convenience constructor used by tests and the ingest stage.
    #[must_use]
    pub fn new(raw: impl Into<String>, plain: impl Into<String>, stream: Stream, no: u64) -> Self {
        Self {
            raw: raw.into(),
            plain: plain.into(),
            stream,
            no,
        }
    }
}

/// Severity of a line, assigned by the classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    /// A real failure. Never collapsed without an explicit opt-in.
    Error,
    Warning,
    Info,
    Progress,
    Trace,
}

impl Level {
    /// Recognise a severity token from the vocabularies real logs use.
    ///
    /// Ported from lnav's level table (`docs.lnav.org/en/latest/formats.html`):
    /// lnav recognises `trace`/`debug`/`info`/`notice`/`stats`/`warning`/
    /// `error`/`critical`/`fatal`, their common aliases, and single-letter
    /// abbreviations. Hushlog has five levels, so the extra names collapse onto
    /// the nearest one: `debug` and below become [`Level::Trace`], `notice` and
    /// `stats` become [`Level::Info`], and `critical`/`fatal` become
    /// [`Level::Error`].
    ///
    /// Matching is case-insensitive. It allocates one small `String` for the
    /// case-folded token; this is a normalization helper for pack authors, not
    /// the classify hot path.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        let token = token.trim();
        if token.is_empty() {
            return None;
        }
        let lower = token.to_ascii_lowercase();
        if lower.len() == 1 {
            return match lower.as_str() {
                "t" | "d" | "v" => Some(Self::Trace),
                "i" | "s" | "n" => Some(Self::Info),
                "w" => Some(Self::Warning),
                "e" | "c" | "f" => Some(Self::Error),
                _ => None,
            };
        }
        Some(match lower.as_str() {
            "trace" | "verbose" | "debug" | "debug2" | "debug3" | "debug4" | "debug5" => {
                Self::Trace
            }
            "info" | "system" | "notice" | "note" | "log" | "stats" => Self::Info,
            "warn" | "warning" | "deprecation" => Self::Warning,
            "err" | "error" | "fail" | "critical" | "severe" | "alert" | "fatal"
            | "emergency" => Self::Error,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod level_token_tests {
    use super::Level;

    #[test]
    fn canonical_names_resolve() {
        assert_eq!(Level::from_token("error"), Some(Level::Error));
        assert_eq!(Level::from_token("warning"), Some(Level::Warning));
        assert_eq!(Level::from_token("info"), Some(Level::Info));
        assert_eq!(Level::from_token("trace"), Some(Level::Trace));
    }

    #[test]
    fn aliases_and_case_are_accepted() {
        assert_eq!(Level::from_token("ERROR"), Some(Level::Error));
        assert_eq!(Level::from_token("Warn"), Some(Level::Warning));
        assert_eq!(Level::from_token("deprecation"), Some(Level::Warning));
        assert_eq!(Level::from_token("err"), Some(Level::Error));
        assert_eq!(Level::from_token("fail"), Some(Level::Error));
        assert_eq!(Level::from_token("fatal"), Some(Level::Error));
        assert_eq!(Level::from_token("critical"), Some(Level::Error));
        assert_eq!(Level::from_token("notice"), Some(Level::Info));
        assert_eq!(Level::from_token("stats"), Some(Level::Info));
        assert_eq!(Level::from_token("debug"), Some(Level::Trace));
    }

    #[test]
    fn single_letter_abbreviations_resolve() {
        assert_eq!(Level::from_token("E"), Some(Level::Error));
        assert_eq!(Level::from_token("W"), Some(Level::Warning));
        assert_eq!(Level::from_token("I"), Some(Level::Info));
        assert_eq!(Level::from_token("T"), Some(Level::Trace));
    }

    #[test]
    fn unknown_tokens_are_none() {
        assert_eq!(Level::from_token(""), None);
        assert_eq!(Level::from_token("   "), None);
        assert_eq!(Level::from_token("verbose-ish"), None);
        assert_eq!(Level::from_token("2"), None);
    }
}

/// Opaque handle to the rule that classified a line.
///
/// This is an index into the owning [`Classifier`]'s rule set, not the rule's
/// name, so that classification stays allocation-free on the hot path.
/// Resolve it to a string through the rule set in `hushlog-rules`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RuleId(pub u16);

/// A [`Line`] together with the classifier's verdict on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classified {
    pub line: Line,
    pub level: Level,
    /// The rule that fired, if any. Built-in heuristics may leave this `None`.
    pub rule_id: Option<RuleId>,
    /// Whether the renderer should draw attention to this line.
    pub highlight: bool,
}

/// Assigns a [`Level`] to each line.
pub trait Classifier {
    fn classify(&mut self, line: Line) -> Classified;
}

/// What the pipeline decided to do with a line or group of lines.
#[derive(Debug, Clone)]
pub enum Event {
    /// Show this line as-is.
    Emit(Classified),
    /// A repeat occurrence was folded into the earlier one.
    Folded {
        /// Fingerprint shared with every occurrence in this group.
        fp: u64,
        /// Total occurrences seen, including the first.
        count: u64,
        /// The first occurrence, kept verbatim so the group is never lossy.
        sample: Classified,
    },
    /// A multi-line block (stack trace, warning paragraph) opened.
    BlockStart(u64),
    /// ...and closed.
    BlockEnd(u64),
    /// End-of-run accounting.
    Summary(Summary),
}

/// Collapses repeated lines and blocks, and is the only stage permitted to
/// decide that output is shorter than input.
pub trait Collapser {
    /// Feed one classified line; returns whatever should happen now.
    fn push(&mut self, c: Classified) -> Vec<Event>;
    /// Flush any group still open at end of input.
    fn finish(&mut self) -> Vec<Event>;
}

/// Writes events to a destination.
pub trait Renderer {
    /// Write one event.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination rejects the write. Renderers must
    /// not swallow I/O failures: dropping a write would violate the lossless
    /// principle in `AGENTS.md` section 1.
    fn render(&mut self, e: &Event) -> std::io::Result<()>;
}

/// End-of-run accounting shown to the user.
///
/// The counts here are the safety net for the lossless principle: whatever the
/// renderers did, these totals account for every line that came in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Total lines read from input.
    pub lines_in: u64,
    /// Lines actually written to the output stream.
    pub lines_out: u64,
    /// Lines classified as errors. Always fully emitted.
    pub errors: u64,
    /// Distinct groups that were collapsed.
    pub groups: u64,
    /// Total lines absorbed into those groups.
    pub lines_folded: u64,
    /// Where the verbatim log was written, if `--tee` was used.
    pub tee_path: Option<String>,
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "hushlog: {} lines -> {} shown | {} errors | {} groups ({} lines folded)",
            thousands(self.lines_in),
            thousands(self.lines_out),
            thousands(self.errors),
            thousands(self.groups),
            thousands(self.lines_folded),
        )?;
        if let Some(p) = &self.tee_path {
            write!(f, " | full log: {p}")?;
        }
        Ok(())
    }
}

/// Insert thousands separators so large counts stay readable at a glance.
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Controls what the [`Collapser`] is allowed to fold.
///
/// The `collapse_errors` field defaults to `false` and that default is the
/// product's core promise. Flipping it requires the user to pass an explicit
/// flag; nothing in the library may set it on their behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollapsePolicy {
    /// Allow `Level::Error` lines to be folded. **Off by default.**
    pub collapse_errors: bool,
    /// Upper bound on distinct fingerprints held in memory.
    ///
    /// Bounds memory regardless of input size; see the performance targets in
    /// `AGENTS.md` section 7.
    pub max_tracked: usize,
}

impl Default for CollapsePolicy {
    fn default() -> Self {
        Self {
            collapse_errors: false,
            max_tracked: 10_000,
        }
    }
}
