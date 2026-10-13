//! The default for pipes and files: the log, and nothing else.

use std::io::{self, Write};

use hushlog_core::{Event, Renderer};

use crate::common::{Sink, write_fold};

/// One input line in, one line out, byte for byte.
///
/// No ANSI is added and no marker is injected, because the output of this
/// renderer is the thing engineers paste into a bug report. Whatever the
/// pipeline decided, this renderer shows it and stops.
#[derive(Debug)]
pub struct PlainRenderer {
    sink: Sink,
}

impl PlainRenderer {
    /// Write the log to `out` unchanged.
    #[must_use]
    pub fn new(out: impl Write + 'static) -> Self {
        Self {
            sink: Sink::new(out),
        }
    }

    /// Push anything the sink has buffered towards the destination.
    ///
    /// # Errors
    ///
    /// Returns the sink's failure, if any.
    pub fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()
    }
}

impl Renderer for PlainRenderer {
    /// # Errors
    ///
    /// Returns the first write failure. It is never swallowed: see
    /// `AGENTS.md` section 1.
    fn render(&mut self, event: &Event) -> io::Result<()> {
        match event {
            Event::Emit(c) => self.sink.line(&c.line.raw),
            Event::Folded { count, sample, .. } => write_fold(&mut self.sink, *count, sample),
            // Block boundaries carry no content of their own -- the lines
            // inside arrive as `Emit` events -- so printing a marker here would
            // only add text to a transcript that has to stay copy-pasteable.
            Event::BlockStart(_) | Event::BlockEnd(_) => Ok(()),
            Event::Summary(summary) => {
                self.sink.line("")?;
                self.sink.line(&summary.to_string())
            }
        }
    }
}
