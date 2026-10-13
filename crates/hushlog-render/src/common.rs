//! Pieces every renderer needs: the sink, the wording of a fold note, and the
//! escaping rules the two CI formats share.

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Write};

use hushlog_core::{Classified, Level};

/// The writer a renderer owns.
///
/// No buffering happens here on purpose. `AGENTS.md` section 1 is the whole
/// product: a line that has been accepted but not yet written is a line the
/// user may never see, so the renderer writes as it renders and lets the
/// caller decide how to buffer.
pub(crate) struct Sink(Box<dyn Write>);

impl Sink {
    pub(crate) fn new(out: impl Write + 'static) -> Self {
        Self(Box::new(out))
    }

    /// Write one line and its terminator.
    ///
    /// Two writes instead of one `format!`, so the hot path allocates nothing
    /// for the line itself.
    pub(crate) fn line(&mut self, text: &str) -> io::Result<()> {
        self.0.write_all(text.as_bytes())?;
        self.0.write_all(b"\n")
    }

    /// Terminate a record the caller has written itself, byte for byte.
    pub(crate) fn newline(&mut self) -> io::Result<()> {
        self.0.write_all(b"\n")
    }

    pub(crate) fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// A sink has no interesting interior; every renderer that holds one still
/// deserves a readable `Debug` in a panic message.
impl fmt::Debug for Sink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Sink")
    }
}

/// Indent for anything hushlog adds itself, kept apart from the input lines so
/// a reader can tell the tool's words from the tool's data.
pub(crate) const INDENT: &str = "  ";

/// The headline of a folded group: what collapsed and by how much.
///
/// Doubles as the title of a GitHub fold or a GitLab section, so it has to
/// read on its own with the group collapsed.
pub(crate) fn fold_summary(count: u64) -> String {
    let noun = if count == 1 { "line" } else { "lines" };
    format!("{count} similar {noun} folded")
}

/// The indented note that goes inside a human-facing fold.
pub(crate) fn fold_note(count: u64) -> String {
    format!("{INDENT}... {}", fold_summary(count))
}

/// Whether a line carries error content the user must be able to read.
///
/// Folding errors is opt-in (`AGENTS.md` 1.1), but when a caller asks for it
/// the renderers still put the text in front of them.
pub(crate) const fn is_error(c: &Classified) -> bool {
    matches!(c.level, Level::Error)
}

/// Render a folded group for a human-facing destination.
///
/// The count is always shown. The sample line is shown verbatim when it is an
/// error, because a reader who cannot see what was folded cannot judge whether
/// it mattered.
pub(crate) fn write_fold(sink: &mut Sink, count: u64, sample: &Classified) -> io::Result<()> {
    if is_error(sample) {
        sink.line(&sample.line.raw)?;
    }
    sink.line(&fold_note(count))
}

/// Escape the payload of a CI log command.
///
/// GitHub Actions treats `%` as an escape introducer and splits commands on
/// newlines, so a literal newline in a group title would silently truncate the
/// command and spill the rest into the log.
pub(crate) fn escape_command_data(text: &str) -> Cow<'_, str> {
    if !text.contains(['%', '\n', '\r']) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '%' => out.push_str("%25"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            other => out.push(other),
        }
    }
    Cow::Owned(out)
}

/// Lower-case level name, the spelling the JSON records use.
pub(crate) const fn level_name(level: Level) -> &'static str {
    match level {
        Level::Error => "error",
        Level::Warning => "warning",
        Level::Info => "info",
        Level::Progress => "progress",
        Level::Trace => "trace",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_summary_agrees_with_its_count() {
        assert_eq!(fold_summary(1), "1 similar line folded");
        assert_eq!(fold_summary(3), "3 similar lines folded");
        assert_eq!(fold_note(3), "  ... 3 similar lines folded");
    }

    #[test]
    fn command_payloads_are_escaped() {
        assert_eq!(escape_command_data("100% done"), "100%25 done");
        assert_eq!(escape_command_data("a\nb"), "a%0Ab");
        assert_eq!(escape_command_data("a\rb"), "a%0Db");
        // Nothing to escape: no copy.
        assert!(matches!(
            escape_command_data("plain title"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn level_names_are_stable() {
        assert_eq!(level_name(Level::Error), "error");
        assert_eq!(level_name(Level::Warning), "warning");
        assert_eq!(level_name(Level::Info), "info");
        assert_eq!(level_name(Level::Progress), "progress");
        assert_eq!(level_name(Level::Trace), "trace");
    }
}
