//! Stage 5 -- Renderers. **Owner: WP5.**
//!
//! Five destinations share one event stream: TTY (colour), plain, GitHub
//! Actions folds, GitLab collapsible sections, and JSON.
//!
//! Contract: implements [`hushlog_core::Renderer`]. A renderer may reformat
//! for its destination but must never *omit* an [`hushlog_core::Event`] that
//! carries error content.
//!
//! ```text
//! [1] Ingest -> [2] Normalize -> [3] Classify -> [4] Group -> [5] Render
//! ```
//!
//! # Choosing a renderer
//!
//! [`Format`] is selected by [`renderer`]. [`Format::Tty`] is the only format
//! that can emit ANSI, and it asks the process ([`anstream`], `NO_COLOR`,
//! `CLICOLOR_FORCE`) rather than assuming. Pass [`Color::Always`] or
//! [`Color::Never`] through [`TtyRenderer::with_color`] when the sink is not
//! stdout and the automatic answer would be wrong.
//!
//! # Buffering
//!
//! No renderer buffers. A line is written as soon as the event arrives, so a
//! panic or a killed process cannot lose accepted input. Callers that want
//! fewer syscalls should wrap the sink in `std::io::BufWriter` and flush it on
//! the way out; every renderer exposes [`flush`](TtyRenderer::flush) for that.

mod common;
mod github;
mod gitlab;
mod json;
mod plain;
mod tty;

pub use github::GithubRenderer;
pub use gitlab::GitlabRenderer;
pub use json::JsonRenderer;
pub use plain::PlainRenderer;
pub use tty::{Color, TtyRenderer};

use std::fmt;
use std::io::{self, Write};
use std::str::FromStr;

use hushlog_core::Event;

/// The trait every destination implements, re-exported so a caller can name it
/// without depending on `hushlog-core` directly.
pub use hushlog_core::Renderer;

/// Where the event stream is going, and therefore how it has to be dressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Format {
    /// Colour, when the destination can show it. The human default on a
    /// terminal.
    Tty,
    /// No ANSI, no markers: pipes, files, bug reports. The default, because
    /// hushlog's output is very often pasted somewhere that cannot render it.
    #[default]
    Plain,
    /// GitHub Actions log commands: `::group::` / `::endgroup::`.
    Github,
    /// GitLab collapsible sections: `section_start` / `section_end`.
    Gitlab,
    /// Newline-delimited JSON, one record per event, for machines and agents.
    Json,
}

impl Format {
    /// Every format, in the order `--help` lists them.
    pub const ALL: [Format; 5] = [
        Format::Tty,
        Format::Plain,
        Format::Github,
        Format::Gitlab,
        Format::Json,
    ];

    /// The canonical name, as accepted by [`Format::from_str`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Format::Tty => "tty",
            Format::Plain => "plain",
            Format::Github => "github",
            Format::Gitlab => "gitlab",
            Format::Json => "json",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Format {
    type Err = ParseFormatError;

    /// Case-insensitive, with the short aliases people actually type.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "tty" | "color" | "colour" => Ok(Format::Tty),
            "plain" | "text" => Ok(Format::Plain),
            "github" | "gh" => Ok(Format::Github),
            "gitlab" | "gl" => Ok(Format::Gitlab),
            "json" => Ok(Format::Json),
            other => Err(ParseFormatError {
                got: other.to_string(),
            }),
        }
    }
}

/// [`Format`] did not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown output format {got:?}; expected tty, plain, github, gitlab or json")]
pub struct ParseFormatError {
    got: String,
}

/// Build the renderer for `format`, writing to `out`.
///
/// The TTY renderer resolves colour automatically here; use
/// [`TtyRenderer::with_color`] when the sink is not stdout.
pub fn renderer(format: Format, out: impl Write + 'static) -> Box<dyn Renderer> {
    match format {
        Format::Tty => Box::new(TtyRenderer::with_color(out, Color::Auto)),
        Format::Plain => Box::new(PlainRenderer::new(out)),
        Format::Github => Box::new(GithubRenderer::new(out)),
        Format::Gitlab => Box::new(GitlabRenderer::new(out)),
        Format::Json => Box::new(JsonRenderer::new(out)),
    }
}

/// Render a batch of events. The pipeline itself streams event by event; this
/// is here for callers that already hold a `Vec<Event>`.
///
/// # Errors
///
/// Returns the first write failure. Renderers never swallow one: a dropped
/// write is a silent loss, and silent loss is what this crate exists to
/// prevent (`AGENTS.md` section 1).
pub fn render_events<R>(renderer: &mut R, events: impl IntoIterator<Item = Event>) -> io::Result<()>
where
    R: Renderer + ?Sized,
{
    for event in events {
        renderer.render(&event)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use hushlog_core::Summary;

    #[test]
    fn every_format_parses_from_its_own_name() {
        for format in Format::ALL {
            assert_eq!(format.as_str().parse::<Format>(), Ok(format));
            assert_eq!(format.to_string(), format.as_str());
        }
    }

    #[test]
    fn unknown_format_names_the_valid_ones() {
        let err = "rtf".parse::<Format>().expect_err("rtf is not a format");
        assert!(err.to_string().contains("github"), "{err}");
        assert_eq!(
            err,
            ParseFormatError {
                got: "rtf".to_string()
            }
        );
    }

    #[test]
    fn format_selection_is_case_insensitive_and_trims() {
        assert_eq!("  GitHub \n".parse::<Format>(), Ok(Format::Github));
    }

    #[test]
    fn render_events_hands_over_every_event() {
        let events = [Event::Summary(Summary {
            lines_in: 2,
            errors: 1,
            ..Summary::default()
        })];
        let mut r = PlainRenderer::new(std::io::sink());
        render_events(&mut r, events).expect("a sink that discards cannot fail");
        r.flush().expect("a sink that discards cannot fail");
    }

    #[test]
    fn render_events_stops_at_the_first_failure() {
        /// Fails on the second write, so the first event gets through.
        struct Flaky(bool);

        impl Write for Flaky {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                if self.0 {
                    return Err(io::Error::other("gone"));
                }
                self.0 = true;
                Ok(buf.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut r = PlainRenderer::new(Flaky(false));
        let events = [
            Event::Emit(classified("one")),
            Event::Emit(classified("two")),
            Event::Emit(classified("three")),
        ];
        assert!(render_events(&mut r, events).is_err());
    }

    fn classified(text: &str) -> hushlog_core::Classified {
        hushlog_core::Classified {
            line: hushlog_core::Line::new(text, text, hushlog_core::Stream::Stdout, 1),
            level: hushlog_core::Level::Error,
            rule_id: None,
            highlight: false,
        }
    }
}
