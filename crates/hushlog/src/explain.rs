//! `--explain`: every decision, annotated. **Owner: WP6.**
//!
//! This is the trust feature and one of the four recovery paths in `AGENTS.md`
//! section 1. If a line was folded, a reader needs to know which group and why;
//! if it was kept, which rule decided that. The answer is written to **stderr**,
//! never to stdout, so `--explain` can be combined with `--format json` or
//! `--format github` without corrupting the machine-readable output.
//!
//! The masking shown next to a folded line is the point: it is why two lines
//! the user can see are identical to the tool. `Normalizer::mask` exists in
//! `hushlog-core` for this caller and for tests, and this is that caller.

use std::io::{self, Write};

use hushlog_core::{Classified, Event, Level, Normalizer, RuleId, Stream, Summary};
use hushlog_rules::RuleClassifier;

/// Writes `[hushlog]`-prefixed annotations to stderr as the pipeline runs.
#[derive(Debug)]
pub(crate) struct Explainer {
    err: io::Stderr,
    /// Rules seen so far, so the first time one fires it can be described.
    explained: std::collections::HashSet<RuleId>,
}

impl Explainer {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            err: io::stderr(),
            explained: std::collections::HashSet::new(),
        }
    }

    /// Write a one-off note of the CLI's own, prefixed like every other line.
    ///
    /// # Errors
    ///
    /// Returns stderr's error.
    pub(crate) fn note(&mut self, text: &str) -> io::Result<()> {
        writeln!(self.err, "[hushlog] {text}")
    }

    /// Annotate one event, whatever kind it is.
    ///
    /// # Errors
    ///
    /// Returns stderr's error. Annotations are a recovery path; if they cannot
    /// be written the run says so rather than pretending the user will be told
    /// what happened.
    pub(crate) fn event(
        &mut self,
        event: &Event,
        classifier: &RuleClassifier,
        normalizer: Normalizer,
    ) -> io::Result<()> {
        match event {
            Event::Emit(c) => self.emit(c, classifier),
            Event::Folded {
                fp, count, sample, ..
            } => self.folded(*fp, *count, sample, classifier, normalizer),
            Event::BlockStart(fp) => writeln!(self.err, "[hushlog] block {fp:#018x} start"),
            Event::BlockEnd(fp) => writeln!(self.err, "[hushlog] block {fp:#018x} end"),
            Event::Summary(s) => self.summary(s),
        }
    }

    fn emit(&mut self, c: &Classified, classifier: &RuleClassifier) -> io::Result<()> {
        let rule = self.describe(c, classifier);
        writeln!(
            self.err,
            "[hushlog] line {} {:<7} {:<8} {} kept",
            c.line.no,
            stream_name(c.line.stream),
            level_name(c.level),
            rule
        )
    }

    fn folded(
        &mut self,
        fp: u64,
        count: u64,
        sample: &Classified,
        classifier: &RuleClassifier,
        normalizer: Normalizer,
    ) -> io::Result<()> {
        let masked = normalizer.mask(&sample.line.plain);
        // A `RuleId` is an index, not a name. Anything that shows one to a user
        // has to resolve it first.
        let rule = sample
            .rule_id
            .and_then(|id| classifier.rule_name(id))
            .unwrap_or("<no rule>");
        writeln!(
            self.err,
            "[hushlog] line {} {:<7} {:<8} rule={rule} folded into group {fp:#018x} \
             (count {count}, sample line {}, masked {masked:?})",
            sample.line.no,
            stream_name(sample.line.stream),
            level_name(sample.level),
            sample.line.no,
        )
    }

    fn summary(&mut self, s: &Summary) -> io::Result<()> {
        // `Summary`'s own Display already carries the `hushlog: ` prefix, which
        // would read as `hushlog: hushlog:` here.
        let text = s.to_string();
        let text = text.strip_prefix("hushlog: ").unwrap_or(&text);
        writeln!(self.err, "[hushlog] summary: {text}")
    }

    /// Name the rule that fired, mentioning it in full the first time.
    fn describe(&mut self, c: &Classified, classifier: &RuleClassifier) -> String {
        let Some(id) = c.rule_id else {
            return "no rule matched; kept as ordinary output".to_string();
        };
        let name = classifier
            .rule_name(id)
            .unwrap_or("<unknown rule>")
            .to_string();
        if self.explained.insert(id) {
            let detail = classifier.rule(id).map_or_else(String::new, |r| {
                let mut bits = vec![format!("collapse={}", r.collapse)];
                if let Some(ctx) = r.context_lines {
                    bits.push(format!("context={ctx}"));
                }
                if r.block.is_some() {
                    bits.push("block-markers".to_string());
                }
                format!("{} ({})", r.location, bits.join(", "))
            });
            return format!("{name} [{detail}]");
        }
        name
    }

    /// Push any buffered annotation out. `stderr` is unbuffered, so this is
    /// only about the write to have actually landed.
    pub(crate) fn flush(&mut self) -> io::Result<()> {
        self.err.flush()
    }
}

impl Default for Explainer {
    fn default() -> Self {
        Self::new()
    }
}

/// Which of the child's streams a line came from, for the annotation.
fn stream_name(stream: Stream) -> &'static str {
    match stream {
        Stream::Stdout => "stdout",
        Stream::Stderr => "stderr",
    }
}

/// The severity word used in every annotation.
const fn level_name(level: Level) -> &'static str {
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
    use hushlog_core::Line;
    use hushlog_rules::builtin;

    fn classified(level: Level, no: u64, text: &str) -> Classified {
        Classified {
            line: Line::new(text, text, Stream::Stdout, no),
            level,
            rule_id: None,
            opid: None,
            highlight: false,
        }
    }

    #[test]
    fn stream_and_level_names_are_stable() {
        assert_eq!(stream_name(Stream::Stdout), "stdout");
        assert_eq!(stream_name(Stream::Stderr), "stderr");
        assert_eq!(level_name(Level::Error), "error");
    }

    #[test]
    fn a_line_with_no_rule_says_so() {
        let classifier = builtin::classifier().expect("built-ins compile");
        let mut x = Explainer::new();
        x.event(
            &Event::Emit(classified(Level::Info, 1, "just some output")),
            &classifier,
            Normalizer::new(),
        )
        .expect("stderr writes");
    }

    #[test]
    fn every_event_variant_can_be_annotated() {
        let classifier = builtin::classifier().expect("built-ins compile");
        let mut x = Explainer::new();
        for event in [
            Event::Emit(classified(Level::Error, 1, "error: boom")),
            Event::Folded {
                fp: 7,
                count: 3,
                sample: classified(Level::Warning, 2, "warning: again"),
            },
            Event::BlockStart(9),
            Event::BlockEnd(9),
            Event::Summary(Summary::default()),
        ] {
            x.event(&event, &classifier, Normalizer::new())
                .expect("stderr writes");
        }
    }
}
