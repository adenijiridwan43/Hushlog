//! JSON for machines and agents: one object per event, newline-delimited.
//!
//! Newline-delimited rather than one big array, because the pipeline streams
//! (`AGENTS.md` section 2). A consumer can read the first record before the
//! last line of input has been seen, and `jq`, `grep` and line-oriented tools
//! all understand the format without a parser for the envelope.

use std::io::{self, Write};

use hushlog_core::{Classified, Event, Renderer, Stream};
use serde::Serialize;

use crate::common::{Sink, level_name};

/// Emits one compact JSON record per [`Event`].
#[derive(Debug)]
pub struct JsonRenderer {
    sink: Sink,
}

/// Where a line came from.
#[derive(Serialize)]
struct LineRec<'a> {
    no: u64,
    stream: &'static str,
    raw: &'a str,
    plain: &'a str,
}

#[derive(Serialize)]
struct EmitRec<'a> {
    no: u64,
    stream: &'static str,
    level: &'static str,
    rule_id: Option<u16>,
    highlight: bool,
    raw: &'a str,
    plain: &'a str,
}

#[derive(Serialize)]
struct FoldedRec<'a> {
    fp: u64,
    count: u64,
    level: &'static str,
    rule_id: Option<u16>,
    sample: LineRec<'a>,
}

#[derive(Serialize)]
struct SummaryRec<'a> {
    lines_in: u64,
    lines_out: u64,
    errors: u64,
    groups: u64,
    lines_folded: u64,
    tee_path: Option<&'a str>,
}

/// Every record carries a `type`, so a consumer can tell what it is holding
/// without a schema for the stream as a whole.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record<'a> {
    Emit(EmitRec<'a>),
    Folded(FoldedRec<'a>),
    BlockStart { block: u64 },
    BlockEnd { block: u64 },
    Summary(SummaryRec<'a>),
}

impl JsonRenderer {
    /// Write JSON records to `out`.
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

impl Renderer for JsonRenderer {
    /// # Errors
    ///
    /// Returns the first write failure. It is never swallowed: see
    /// `AGENTS.md` section 1. A serialisation failure is a bug rather than an
    /// I/O condition, and is reported as one.
    fn render(&mut self, event: &Event) -> io::Result<()> {
        let record = match event {
            Event::Emit(c) => Record::Emit(EmitRec {
                no: c.line.no,
                stream: stream_name(c.line.stream),
                level: level_name(c.level),
                rule_id: c.rule_id.map(|r| r.0),
                highlight: c.highlight,
                raw: &c.line.raw,
                plain: &c.line.plain,
            }),
            // The sample travels with the record, so a folded error is never
            // merely counted: its text is right there in the record.
            Event::Folded { fp, count, sample } => Record::Folded(FoldedRec {
                fp: *fp,
                count: *count,
                level: level_name(sample.level),
                rule_id: sample.rule_id.map(|r| r.0),
                sample: line_rec(sample),
            }),
            Event::BlockStart(id) => Record::BlockStart { block: *id },
            Event::BlockEnd(id) => Record::BlockEnd { block: *id },
            Event::Summary(summary) => Record::Summary(SummaryRec {
                lines_in: summary.lines_in,
                lines_out: summary.lines_out,
                errors: summary.errors,
                groups: summary.groups,
                lines_folded: summary.lines_folded,
                tee_path: summary.tee_path.as_deref(),
            }),
        };

        serde_json::to_writer(&mut self.sink, &record).map_err(io::Error::other)?;
        self.sink.newline()
    }
}

fn line_rec(c: &Classified) -> LineRec<'_> {
    LineRec {
        no: c.line.no,
        stream: stream_name(c.line.stream),
        raw: &c.line.raw,
        plain: &c.line.plain,
    }
}

const fn stream_name(stream: Stream) -> &'static str {
    match stream {
        Stream::Stdout => "stdout",
        Stream::Stderr => "stderr",
    }
}
