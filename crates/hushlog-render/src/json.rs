//! JSON for machines and agents: one object per event, newline-delimited.
//!
//! Newline-delimited rather than one big array, because the pipeline streams
//! (`AGENTS.md` section 2). A consumer can read the first record before the
//! last line of input has been seen, and `jq`, `grep` and line-oriented tools
//! all understand the format without a parser for the envelope.

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

use hushlog_core::{Classified, Event, Renderer, RuleId, Stream};
use serde::Serialize;

use crate::common::{Sink, level_name};

/// The schema version carried by every JSON record. A field name or type can
/// only change with a bump; new fields are additive (see `docs/JSON-CONTRACT.md`).
const SCHEMA_VERSION: &str = "1";

/// A shared, replaceable snapshot of rule names, indexed by [`RuleId`].
///
/// `RuleId` is an index into the classifier's rule set, not a stable name.
/// The pipeline fills this from the classifier and replaces it if format
/// detection reorders the rules, so a name always describes the rule that
/// actually fired for the line being rendered.
#[derive(Clone, Debug, Default)]
pub struct RuleNames(Rc<RefCell<Vec<String>>>);

impl RuleNames {
    /// Build a snapshot from stable rule names, in rule-id order.
    #[must_use]
    pub fn from_names(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self(Rc::new(RefCell::new(
            names.into_iter().map(Into::into).collect(),
        )))
    }

    /// Replace the snapshot, e.g. after format detection reorders the rules.
    pub fn replace(&self, names: impl IntoIterator<Item = impl Into<String>>) {
        *self.0.borrow_mut() = names.into_iter().map(Into::into).collect();
    }

    fn get(&self, id: RuleId) -> Option<String> {
        self.0.borrow().get(usize::from(id.0)).cloned()
    }
}

/// Emits one compact JSON record per [`Event`].
#[derive(Debug)]
pub struct JsonRenderer {
    sink: Sink,
    /// Monotonic position in this run's record stream. Unlike `no`, which is
    /// per input stream, this never resets or repeats.
    sequence: u64,
    names: RuleNames,
}

/// Where a line came from. `no` is the v0 line number and stays as a legacy
/// alias; `message` is the text a person would read.
#[derive(Serialize)]
struct LineRec<'a> {
    no: u64,
    stream: &'static str,
    message: &'a str,
    raw: &'a str,
    plain: &'a str,
}

#[derive(Serialize)]
struct EmitRec<'a> {
    schema_version: &'static str,
    sequence: u64,
    no: u64,
    stream: &'static str,
    level: &'static str,
    /// Legacy v0 alias for `rule`, kept so existing consumers do not break.
    rule_id: Option<u16>,
    rule: Option<String>,
    highlight: bool,
    folded: bool,
    fold_count: u64,
    /// `None`: an emitted line has not entered a fold group yet.
    fingerprint: Option<String>,
    message: &'a str,
    raw: &'a str,
    plain: &'a str,
}

#[derive(Serialize)]
struct FoldedRec<'a> {
    schema_version: &'static str,
    sequence: u64,
    /// Legacy v0 aliases for `fingerprint` and `fold_count`.
    fp: u64,
    count: u64,
    fingerprint: String,
    folded: bool,
    fold_count: u64,
    level: &'static str,
    rule_id: Option<u16>,
    rule: Option<String>,
    message: &'a str,
    sample: LineRec<'a>,
}

#[derive(Serialize)]
struct SummaryRec<'a> {
    schema_version: &'static str,
    sequence: u64,
    lines_in: u64,
    lines_out: u64,
    errors: u64,
    groups: u64,
    lines_folded: u64,
    lines_suppressed: u64,
    tee_path: Option<&'a str>,
}

/// The exact accounting for an opt-in `--max-tokens` run. Emitted immediately
/// after `summary`, only when a budget was requested.
#[derive(Serialize)]
struct BudgetSummaryRec<'a> {
    schema_version: &'static str,
    sequence: u64,
    max_tokens: u64,
    token_estimate: u64,
    tokens_suppressed: u64,
    lines_suppressed: u64,
    tee_path: Option<&'a str>,
}

/// Every record carries a `type`, so a consumer can tell what it is holding
/// without a schema for the stream as a whole.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record<'a> {
    Emit(EmitRec<'a>),
    Folded(FoldedRec<'a>),
    BlockStart {
        schema_version: &'static str,
        sequence: u64,
        block: u64,
    },
    BlockEnd {
        schema_version: &'static str,
        sequence: u64,
        block: u64,
    },
    Summary(SummaryRec<'a>),
    BudgetSummary(BudgetSummaryRec<'a>),
}

impl JsonRenderer {
    /// Write JSON records to `out`, with no rule-name resolution.
    #[must_use]
    pub fn new(out: impl Write + 'static) -> Self {
        Self::with_names(out, RuleNames::default())
    }

    /// Write JSON records to `out`, resolving `rule_id` through `names`.
    #[must_use]
    pub fn with_names(out: impl Write + 'static, names: RuleNames) -> Self {
        Self {
            sink: Sink::new(out),
            sequence: 0,
            names,
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

impl JsonRenderer {
    /// Write one NDJSON record, stamping it with the next sequence number.
    fn write_record(&mut self, record: &Record<'_>) -> io::Result<()> {
        self.sequence = self.sequence.saturating_add(1);
        serde_json::to_writer(&mut self.sink, record).map_err(io::Error::other)?;
        self.sink.newline()
    }
}

impl Renderer for JsonRenderer {
    /// # Errors
    ///
    /// Returns the first write failure. It is never swallowed: see
    /// `AGENTS.md` section 1. A serialisation failure is a bug rather than an
    /// I/O condition, and is reported as one.
    fn render(&mut self, event: &Event) -> io::Result<()> {
        match event {
            Event::Emit(c) => self.write_record(&Record::Emit(EmitRec {
                schema_version: SCHEMA_VERSION,
                sequence: self.sequence.saturating_add(1),
                no: c.line.no,
                stream: stream_name(c.line.stream),
                level: level_name(c.level),
                rule_id: c.rule_id.map(|r| r.0),
                rule: c.rule_id.and_then(|id| self.names.get(id)),
                highlight: c.highlight,
                folded: false,
                fold_count: 1,
                fingerprint: None,
                message: &c.line.plain,
                raw: &c.line.raw,
                plain: &c.line.plain,
            })),
            // The sample travels with the record, so a folded error is never
            // merely counted: its text is right there in the record.
            Event::Folded {
                fp, count, sample, ..
            } => self.write_record(&Record::Folded(FoldedRec {
                schema_version: SCHEMA_VERSION,
                sequence: self.sequence.saturating_add(1),
                fp: *fp,
                count: *count,
                fingerprint: format!("{fp:016x}"),
                folded: true,
                fold_count: *count,
                level: level_name(sample.level),
                rule_id: sample.rule_id.map(|r| r.0),
                rule: sample.rule_id.and_then(|id| self.names.get(id)),
                message: &sample.line.plain,
                sample: line_rec(sample),
            })),
            Event::BlockStart(id) => self.write_record(&Record::BlockStart {
                schema_version: SCHEMA_VERSION,
                sequence: self.sequence.saturating_add(1),
                block: *id,
            }),
            Event::BlockEnd(id) => self.write_record(&Record::BlockEnd {
                schema_version: SCHEMA_VERSION,
                sequence: self.sequence.saturating_add(1),
                block: *id,
            }),
            Event::Summary(summary) => {
                self.write_record(&Record::Summary(SummaryRec {
                    schema_version: SCHEMA_VERSION,
                    sequence: self.sequence.saturating_add(1),
                    lines_in: summary.lines_in,
                    lines_out: summary.lines_out,
                    errors: summary.errors,
                    groups: summary.groups,
                    lines_folded: summary.lines_folded,
                    lines_suppressed: summary.lines_suppressed,
                    tee_path: summary.tee_path.as_deref(),
                }))?;
                if let Some(max) = summary.max_tokens {
                    self.write_record(&Record::BudgetSummary(BudgetSummaryRec {
                        schema_version: SCHEMA_VERSION,
                        sequence: self.sequence.saturating_add(1),
                        max_tokens: max,
                        token_estimate: summary.token_estimate,
                        tokens_suppressed: summary.tokens_suppressed,
                        lines_suppressed: summary.lines_suppressed,
                        tee_path: summary.tee_path.as_deref(),
                    }))?;
                }
                Ok(())
            }
        }
    }
}

fn line_rec(c: &Classified) -> LineRec<'_> {
    LineRec {
        no: c.line.no,
        stream: stream_name(c.line.stream),
        message: &c.line.plain,
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
