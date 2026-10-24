//! CSV output: one row per [`Event`](hushlog_core::Event), newline-terminated.
//!
//! Every row begins with a `type` column so a consumer can tell what it is
//! holding (`emit`, `folded`, `block_start`, `block_end`, `summary`). Fields
//! that contain a comma, double quote, newline or carriage return are wrapped
//! in double quotes with inner quotes doubled, per RFC 4180.

use std::borrow::Cow;
use std::io::{self, Write};

use hushlog_core::{Event, Renderer, Stream};

use crate::common::Sink;

/// Renders [`Event`]s as CSV rows.
#[derive(Debug)]
pub struct CsvRenderer {
    sink: Sink,
    header_written: bool,
}

impl CsvRenderer {
    /// Write CSV rows to `out`.
    #[must_use]
    pub fn new(out: impl Write + 'static) -> Self {
        Self {
            sink: Sink::new(out),
            header_written: false,
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

/// One CSV row: a `type` column plus the event's fields.
#[derive(Debug)]
struct CsvRow<'a> {
    typ: &'a str,
    no: &'a str,
    stream: &'a str,
    level: &'a str,
    rule_id: &'a str,
    count: &'a str,
    detail: &'a str,
    raw: &'a str,
    plain: &'a str,
}

impl Renderer for CsvRenderer {
    fn render(&mut self, event: &Event) -> io::Result<()> {
        if !self.header_written {
            self.sink.line("type,no,stream,level,rule_id,count,detail,raw,plain")?;
            self.header_written = true;
        }

        match event {
            Event::Emit(c) => self.emit_row(&CsvRow {
                typ: "emit",
                no: &c.line.no.to_string(),
                stream: stream_name(c.line.stream),
                level: level_name(c.level),
                rule_id: &rule_id_text(c.rule_id),
                count: "",
                detail: "",
                raw: &c.line.raw,
                plain: &c.line.plain,
            }),
            Event::Folded { fp, count, sample } => self.emit_row(&CsvRow {
                typ: "folded",
                no: &sample.line.no.to_string(),
                stream: stream_name(sample.line.stream),
                level: level_name(sample.level),
                rule_id: &rule_id_text(sample.rule_id),
                count: &count.to_string(),
                detail: &fp.to_string(),
                raw: &sample.line.raw,
                plain: &sample.line.plain,
            }),
            Event::BlockStart(fp) => self.emit_row(&CsvRow {
                typ: "block_start",
                no: "",
                stream: "",
                level: "",
                rule_id: "",
                count: "",
                detail: &fp.to_string(),
                raw: "",
                plain: "",
            }),
            Event::BlockEnd(fp) => self.emit_row(&CsvRow {
                typ: "block_end",
                no: "",
                stream: "",
                level: "",
                rule_id: "",
                count: "",
                detail: &fp.to_string(),
                raw: "",
                plain: "",
            }),
            Event::Summary(summary) => self.emit_row(&CsvRow {
                typ: "summary",
                no: "",
                stream: "",
                level: "",
                rule_id: "",
                count: "",
                detail: &format!(
                    "lines_in={} lines_out={} errors={} groups={} lines_folded={} tee_path={:?}",
                    summary.lines_in,
                    summary.lines_out,
                    summary.errors,
                    summary.groups,
                    summary.lines_folded,
                    summary.tee_path
                ),
                raw: "",
                plain: "",
            }),
        }
    }
}

impl CsvRenderer {
    fn emit_row(&mut self, row: &CsvRow<'_>) -> io::Result<()> {
        let fields = [
            csv_field(row.typ),
            csv_field(row.no),
            csv_field(row.stream),
            csv_field(row.level),
            csv_field(row.rule_id),
            csv_field(row.count),
            csv_field(row.detail),
            csv_field(row.raw),
            csv_field(row.plain),
        ]
        .join(",");
        self.sink.line(&fields)
    }
}

fn rule_id_text(id: Option<hushlog_core::RuleId>) -> String {
    match id {
        Some(id) => id.0.to_string(),
        None => String::new(),
    }
}

const fn stream_name(stream: Stream) -> &'static str {
    match stream {
        Stream::Stdout => "stdout",
        Stream::Stderr => "stderr",
    }
}

const fn level_name(level: hushlog_core::Level) -> &'static str {
    match level {
        hushlog_core::Level::Error => "error",
        hushlog_core::Level::Warning => "warning",
        hushlog_core::Level::Info => "info",
        hushlog_core::Level::Progress => "progress",
        hushlog_core::Level::Trace => "trace",
    }
}

/// Escape a single CSV field per RFC 4180.
fn csv_field(text: &str) -> Cow<'_, str> {
    if !text.contains([',', '"', '\n', '\r']) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 8);
    out.push('"');
    for ch in text.chars() {
        if ch == '"' {
            out.push_str("\"\"");
        } else {
            out.push(ch);
        }
    }
    out.push('"');
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_field_quotes_only_when_needed() {
        assert_eq!(csv_field("plain"), Cow::Borrowed("plain"));
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("a\nb"), "\"a\nb\"");
    }
}
