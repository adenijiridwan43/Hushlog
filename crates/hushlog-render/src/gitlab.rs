//! GitLab: `section_start` / `section_end`, the collapsible sections the job
//! log renders as `<details>` blocks.

use std::io::{self, Write};

use hushlog_core::{Event, Renderer};

use crate::common::{Sink, escape_command_data, fold_note, fold_summary, is_error};

/// Emits GitLab collapsible section markers around folded groups and blocks.
#[derive(Debug)]
pub struct GitlabRenderer {
    sink: Sink,
}

impl GitlabRenderer {
    /// Write GitLab section markers to `out`.
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

impl Renderer for GitlabRenderer {
    /// # Errors
    ///
    /// Returns the first write failure. It is never swallowed: see
    /// `AGENTS.md` section 1.
    fn render(&mut self, event: &Event) -> io::Result<()> {
        match event {
            Event::Emit(c) => self.sink.line(&c.line.raw),
            Event::Folded { count, sample, .. } => {
                // Same rule as the GitHub renderer: error content stays outside
                // the collapsed section.
                if is_error(sample) {
                    self.sink.line(&sample.line.raw)?;
                }
                let summary = fold_summary(*count);
                let name = escape_command_data(&summary);
                self.sink
                    .line(&format!("section_start::name={name}[collapsed=true]"))?;
                self.sink.line(&fold_note(*count))?;
                self.sink.line("section_end")
            }
            Event::BlockStart(id) => self
                .sink
                .line(&format!("section_start::name=block {id}[collapsed=true]")),
            Event::BlockEnd(_) => self.sink.line("section_end"),
            Event::Summary(summary) => {
                self.sink.line("")?;
                self.sink.line(&summary.to_string())
            }
        }
    }
}
