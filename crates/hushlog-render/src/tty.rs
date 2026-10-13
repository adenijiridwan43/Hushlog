//! The human default: the log, in colour, when the destination can show it.

use std::io::{self, Write};

use hushlog_core::{Classified, Event, Level, Renderer};
use owo_colors::{OwoColorize, Style};

use crate::common::{Sink, fold_note, is_error};

/// The log with ANSI colour, when the destination can show it.
#[derive(Debug)]
pub struct TtyRenderer {
    sink: Sink,
    color: bool,
}

/// Whether to colour, and how to decide.
///
/// Defaults to [`Color::Never`]: a library that guesses "yes" leaks escape
/// sequences into files and bug reports, and that is the failure mode
/// `AGENTS.md` section 1 is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Color {
    /// Ask the process: is stdout a terminal, and does the environment want
    /// colour (`NO_COLOR`, `CLICOLOR_FORCE`, `CI`)?
    Auto,
    /// Colour, whatever the destination is.
    Always,
    /// Never colour.
    #[default]
    Never,
}

impl Color {
    /// Collapse the question to the yes/no the renderer asks.
    ///
    /// `Auto` inspects stdout, because that is where hushlog writes by default
    /// and an `impl Write` cannot be interrogated. A caller writing somewhere
    /// else -- a file, a socket -- should say [`Color::Never`] or
    /// [`Color::Always`] rather than trust the answer.
    fn resolve(self) -> bool {
        match self {
            Color::Always => true,
            Color::Never => false,
            Color::Auto => {
                anstream::AutoStream::choice(&std::io::stdout()) != anstream::ColorChoice::Never
            }
        }
    }
}

impl TtyRenderer {
    /// Write to `out`, colouring when stdout is a terminal that wants it.
    #[must_use]
    pub fn new(out: impl Write + 'static) -> Self {
        Self::with_color(out, Color::Auto)
    }

    /// Write to `out`, colouring as `color` says.
    #[must_use]
    pub fn with_color(out: impl Write + 'static, color: Color) -> Self {
        Self {
            sink: Sink::new(out),
            color: color.resolve(),
        }
    }

    /// Whether this renderer will emit ANSI.
    #[must_use]
    pub const fn color_enabled(&self) -> bool {
        self.color
    }

    /// Push anything the sink has buffered towards the destination.
    ///
    /// # Errors
    ///
    /// Returns the sink's failure, if any.
    pub fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()
    }

    /// Colour a line by level, bolding the ones the classifier highlighted.
    fn paint(&self, text: &str, level: Level, highlight: bool) -> String {
        let style = match level {
            Level::Error => Style::new().red(),
            Level::Warning => Style::new().yellow(),
            Level::Info => Style::new().cyan(),
            Level::Progress => Style::new().blue(),
            Level::Trace => Style::new().dimmed(),
        };
        let style = if highlight { style.bold() } else { style };
        self.wrap(text, style)
    }

    /// Wrap `text` in `style`, or leave it alone when colour is off.
    fn wrap(&self, text: &str, style: Style) -> String {
        if self.color {
            text.style(style).to_string()
        } else {
            text.to_string()
        }
    }

    /// The accounting for a folded group: the sample when it carries error
    /// content, then the count of what was folded.
    fn fold(&mut self, count: u64, sample: &Classified) -> io::Result<()> {
        if is_error(sample) {
            let painted = self.paint(&sample.line.raw, sample.level, sample.highlight);
            self.sink.line(&painted)?;
        }
        let dimmed = self.wrap(&fold_note(count), Style::new().dimmed());
        self.sink.line(&dimmed)
    }
}

impl Renderer for TtyRenderer {
    /// # Errors
    ///
    /// Returns the first write failure. It is never swallowed: see
    /// `AGENTS.md` section 1.
    fn render(&mut self, event: &Event) -> io::Result<()> {
        match event {
            Event::Emit(c) => {
                let painted = self.paint(&c.line.raw, c.level, c.highlight);
                self.sink.line(&painted)
            }
            Event::Folded { count, sample, .. } => self.fold(*count, sample),
            // Nothing to draw: the lines inside the block arrive as `Emit`
            // events, already coloured by level. A marker here would be the
            // only thing on the terminal that is not from the log.
            Event::BlockStart(_) | Event::BlockEnd(_) => Ok(()),
            Event::Summary(summary) => {
                self.sink.line("")?;
                // A run that produced errors gets attention; a clean run should
                // not shout.
                let style = if summary.errors > 0 {
                    Style::new().yellow().bold()
                } else {
                    Style::new().dimmed()
                };
                let painted = self.wrap(&summary.to_string(), style);
                self.sink.line(&painted)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_never_and_always_are_not_guesses() {
        assert!(!Color::Never.resolve());
        assert!(Color::Always.resolve());
    }

    #[test]
    fn the_default_is_not_to_colour() {
        assert_eq!(Color::default(), Color::Never);
        assert!(!TtyRenderer::with_color(std::io::sink(), Color::default()).color_enabled());
    }
}
