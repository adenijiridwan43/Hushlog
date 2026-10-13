//! Errors, and the source locations they point at.
//!
//! AGENTS.md section 1: a rule pack that does not load is a bug the user has to
//! fix. "some error occurred" is not a bug report they can act on, so every
//! failure carries the file it came from and the line it went wrong on.

use std::fmt;
use std::ops::Range;
use std::path::Path;

/// Where something in a rule pack lives.
///
/// Rendered as `file:line:column`, or just `file` when the parser could not
/// pin the failure to a line (rare, but possible for structural errors such as
/// a missing `rules` array).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Location {
    pub file: String,
    /// 1-based line number, when known.
    pub line: Option<usize>,
    /// 1-based column number, when known.
    pub column: Option<usize>,
}

impl Location {
    /// A location naming a file but no position within it.
    #[must_use]
    pub fn file(file: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line: None,
            column: None,
        }
    }

    /// Turn a byte range in `text` into a `file:line:column` location.
    ///
    /// A missing or out-of-bounds span degrades to a file-only location rather
    /// than guessing at line 1, so "unknown" never reads as "the first line".
    #[must_use]
    pub fn from_span(file: &str, text: &str, span: Option<Range<usize>>) -> Self {
        let mut loc = Self::file(file);
        let Some(span) = span else { return loc };
        let Some(prefix) = text.get(..span.start.min(text.len())) else {
            return loc;
        };

        loc.line = Some(prefix.bytes().filter(|b| *b == b'\n').count() + 1);
        let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
        // Count characters, not bytes, so a column points at something a human
        // editing the file would recognise.
        loc.column = Some(prefix[line_start..].chars().count() + 1);
        loc
    }

    /// Turn a path into a file-only location.
    #[must_use]
    pub fn from_path(path: &Path) -> Self {
        Self::file(path.display().to_string())
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.file)?;
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
            if let Some(column) = self.column {
                write!(f, ":{column}")?;
            }
        }
        Ok(())
    }
}

/// Anything that can go wrong turning rule packs into a classifier.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RuleError {
    /// The pack could not be read off disk.
    #[error("cannot read rule pack {location}: {source}")]
    Io {
        location: Location,
        #[source]
        source: std::io::Error,
    },

    /// The file is not valid TOML, or does not match the schema.
    #[error("{location}: {message}")]
    Syntax { location: Location, message: String },

    /// The file is well-formed TOML but the rule itself is wrong.
    #[error("{location}: rule `{id}`: {message}")]
    Rule {
        location: Location,
        id: String,
        message: String,
    },
}

impl RuleError {
    /// Where the failure happened.
    #[must_use]
    pub fn location(&self) -> &Location {
        match self {
            Self::Io { location, .. }
            | Self::Syntax { location, .. }
            | Self::Rule { location, .. } => location,
        }
    }
}
