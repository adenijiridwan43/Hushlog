//! GitHub Actions: `::group::` / `::endgroup::`, the folds the Actions UI
//! renders as collapsible triangles.

use std::io::{self, Write};

use hushlog_core::{Event, Renderer};

use crate::common::{Sink, escape_command_data, fold_note, fold_summary, is_error};

/// Emits GitHub Actions workflow commands around folded groups and blocks.
#[derive(Debug)]
pub struct GithubRenderer {
    sink: Sink,
}

impl GithubRenderer {
    /// Write GitHub log commands to `out`.
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

impl Renderer for GithubRenderer {
    /// # Errors
    ///
    /// Returns the first write failure. It is never swallowed: see
    /// `AGENTS.md` section 1.
    fn render(&mut self, event: &Event) -> io::Result<()> {
        match event {
            Event::Emit(c) => self.sink.line(&c.line.raw),
            Event::Folded { count, sample, .. } => {
                // A collapsed group is still in the log, but it is one click
                // away from being invisible. Anything with error content is
                // written first, outside the fold, where it cannot be missed.
                if is_error(sample) {
                    self.sink.line(&sample.line.raw)?;
                }
                let summary = fold_summary(*count);
                let title = escape_command_data(&summary);
                self.sink.line(&format!("::group::{title}"))?;
                self.sink.line(&fold_note(*count))?;
                self.sink.line("::endgroup::")
            }
            Event::BlockStart(id) => self.sink.line(&format!("::group::block {id}")),
            Event::BlockEnd(_) => self.sink.line("::endgroup::"),
            Event::Summary(summary) => {
                self.sink.line("")?;
                self.sink.line(&summary.to_string())
            }
        }
    }
}
