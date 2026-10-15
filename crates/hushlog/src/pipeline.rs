//! The five stages, wired together. **Owner: WP6.**
//!
//! Stages 1 to 4 are `hushlog-core`; stage 5 is `hushlog-render`. This module
//! owns nothing of its own except the order they run in and the rule that
//! `--tee` is fed from the read buffer *upstream* of stage 1 -- see
//! [`Tee`](crate::tee) and `AGENTS.md` section 1.0.
//!
//! ```text
//! bytes -> Ingest -> Normalize -> RuleClassifier -> FoldingCollapser -> Renderer
//!   |                                                                 |
//!   +------------------------- Tee ----------------------------------+
//! ```
//!
//! `Normalize` does not sit between `Ingest` and the classifier for matching
//! purposes: rules are written against the text a human sees, with ANSI
//! stripped, which is exactly `Line::plain` as `Ingest` leaves it. Masking is a
//! *grouping* concern, so it is applied to `plain` after classification, where
//! it can only make two lines look alike to the fingerprint and can never stop
//! an error rule from firing.

use std::io;

use anyhow::Context as _;
use hushlog_core::{
    BlockDetector, Classifier, CollapsePolicy, Collapser, Event, FoldingCollapser, Ingest,
    Normalizer, Renderer, Stream,
};
use hushlog_rules::RuleClassifier;

use crate::args::{self, Config};
use crate::explain::Explainer;
use crate::sink::SharedOut;

/// Which of the child's two streams a batch of bytes came from, when wrap mode
/// interleaves them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The child's stdout.
    ChildOut,
    /// The child's stderr.
    ChildErr,
}

impl From<Origin> for Stream {
    fn from(origin: Origin) -> Self {
        match origin {
            Origin::ChildOut => Stream::Stdout,
            Origin::ChildErr => Stream::Stderr,
        }
    }
}

/// The assembled pipeline. One instance per run.
pub(crate) struct Pipeline {
    classifier: RuleClassifier,
    ingests: [Ingest; 2],
    collapser: FoldingCollapser,
    renderer: Box<dyn Renderer>,
    out: SharedOut,
    explain: Option<Explainer>,
    /// Kept so `--explain` can show why two lines were considered the same.
    normalizer: Normalizer,
    summary: bool,
    /// Where the verbatim log went, for the summary's "full log: PATH" tail.
    /// `hushlog-core` builds the `Summary` and knows nothing about `--tee`,
    /// so the CLI fills this in on the way out.
    tee_path: Option<String>,
    /// True once stdout is gone (`hushlog ... | head`), so we stop writing.
    stdout_closed: bool,
}

impl Pipeline {
    /// Build the pipeline from the parsed configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if a rule pack fails to compile. The message names the
    /// file and line, because a pack that does not load is the user's bug to
    /// fix and "some error occurred" is not a bug report they can act on.
    pub(crate) fn new(
        config: &Config,
        out: SharedOut,
        mut explain: Option<Explainer>,
        tee_path: Option<String>,
    ) -> anyhow::Result<Self> {
        let packs = args::load_packs(&config.rules)?;
        let classifier = RuleClassifier::new(&packs)
            .map_err(|e| anyhow::Error::msg(e.to_string()))
            .context("compiling the rule set")?;

        let policy = CollapsePolicy {
            collapse_errors: config.policy.collapse_errors,
            max_tracked: config.policy.max_tracked,
        };
        let detector = block_detector_or_warn(&classifier);
        let collapser = FoldingCollapser::with_policy_and_detector(policy, detector);

        let renderer = hushlog_render::renderer(config.format, out.clone());

        if let Some(explain) = explain.as_mut() {
            // The banner states the policy the whole run is governed by, so a
            // `--explain` transcript can be read without the command line.
            explain.note(&format!(
                "policy: {} rules, collapse_errors={}, max_tracked={}, format={}, summary={}",
                classifier.rules().len(),
                policy.collapse_errors,
                policy.max_tracked,
                config.format,
                config.summary,
            ))?;
        }

        Ok(Self {
            classifier,
            ingests: [Ingest::new(Stream::Stdout), Ingest::new(Stream::Stderr)],
            collapser,
            renderer,
            out,
            explain,
            normalizer: Normalizer::new(),
            summary: config.summary,
            tee_path,
            stdout_closed: false,
        })
    }

    /// Feed one chunk of input bytes through every stage.
    ///
    /// `stream` is [`Stream::Stdout`] in pipe mode. Wrap mode uses one
    /// [`Ingest`] per stream so that line numbers stay per-stream and a
    /// half-finished line on one stream is not completed by a newline from the
    /// other.
    ///
    /// # Errors
    ///
    /// Returns the renderer's error. A closed stdout (`hushlog | head`) is not
    /// an error: it is recorded and the rest of the input is read past, so the
    /// run summary still accounts for every line.
    pub(crate) fn push(&mut self, bytes: &[u8], stream: Stream) -> io::Result<()> {
        let idx = Self::index_of(stream);
        let lines = self.ingests[idx].push(bytes);
        self.handle(lines)
    }

    /// Flush each stream's partial line, then close the collapser and render
    /// whatever it had buffered.
    ///
    /// # Errors
    ///
    /// As [`Pipeline::push`], plus any failure flushing the output.
    pub(crate) fn finish(&mut self) -> io::Result<()> {
        let mut tail = Vec::new();
        for ingest in &mut self.ingests {
            tail.extend(ingest.finish());
        }
        // In wrap mode both streams are flushed together so their partial lines
        // land in one place, in the order they were read.
        self.handle(tail)?;

        let events = self.collapser.finish();
        self.render_all(events)?;
        if let Some(explain) = self.explain.as_mut() {
            explain.flush()?;
        }
        self.out.flush()
    }

    /// Classify, collapse and render a batch of freshly framed lines.
    fn handle(&mut self, lines: Vec<hushlog_core::Line>) -> io::Result<()> {
        for line in lines {
            let mut classified = self.classifier.classify(line);
            // Masking is for grouping only. Applied here, after the rule engine
            // has had its look at the text a human would read, so masking can
            // never suppress an error rule.
            classified.line.plain = self.normalizer.mask(&classified.line.plain).into_owned();
            let events = self.collapser.push(classified);
            self.render_all(events)?;
        }
        Ok(())
    }

    /// Hand a batch of events to the explainer and then the renderer.
    fn render_all(&mut self, events: Vec<Event>) -> io::Result<()> {
        for mut event in events {
            // `hushlog-core` builds the Summary and has no idea `--tee` exists.
            // Filling in the path here is what turns the counts into a recovery
            // path: a reader who wants a folded line is told where to find it.
            if let Event::Summary(summary) = &mut event {
                summary.tee_path.clone_from(&self.tee_path);
            }

            // `--explain` and the summary are the two things that keep
            // `--no-summary` honest: a reader who turned the summary off must
            // still be able to see what was folded.
            if let Some(explain) = self.explain.as_mut() {
                explain.event(&event, &self.classifier, self.normalizer)?;
            }
            if self.stdout_closed {
                continue;
            }
            if matches!(event, Event::Summary(_)) && !self.summary {
                continue;
            }
            match self.renderer.render(&event) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
                    self.stdout_closed = true;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn index_of(stream: Stream) -> usize {
        match stream {
            Stream::Stdout => 0,
            Stream::Stderr => 1,
        }
    }
}

/// Build the block detector from whatever markers the loaded packs declare.
///
/// `hushlog_rules::block_detector` returns an error for a rule that declares a
/// **regex** block marker, because block detection matches substrings and a
/// regex would never fire. Silently dropping the marker would mean a pack
/// author wrote something that looks right and does nothing, which `AGENTS.md`
/// section 1 rules out. So the error is reported, naming the rule and its file
/// and line, and the run continues with the built-in markers -- losing a block
/// grouping is recoverable through `--explain` and `--tee`; losing the error
/// lines inside it is not.
fn block_detector_or_warn(classifier: &RuleClassifier) -> BlockDetector {
    match hushlog_rules::block_detector(classifier.rules()) {
        Ok(detector) => detector,
        Err(err) => {
            eprintln!(
                "hushlog: {err}\n\
                 hushlog: falling back to the built-in block markers; no line is lost, \
                 only this rule's grouping changes."
            );
            BlockDetector::new()
        }
    }
}

/// Load only the built-in packs, for tests that do not need a config.
#[cfg(test)]
pub(crate) fn builtin_classifier() -> anyhow::Result<RuleClassifier> {
    hushlog_rules::builtin::classifier().map_err(|e| anyhow::Error::msg(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hushlog_core::{Classified, Line};
    use hushlog_render::Format;

    fn test_pipeline() -> Pipeline {
        let config = Config {
            mode: args::Mode::Pipe,
            tee: None,
            raw: false,
            explain: false,
            format: Format::Plain,
            rules: Vec::new(),
            policy: CollapsePolicy::default(),
            summary: true,
            command: Vec::new(),
        };
        let out = SharedOut::new();
        Pipeline::new(&config, out, None, None).expect("a valid config builds a pipeline")
    }

    #[test]
    fn the_built_in_packs_compile() {
        let classifier = builtin_classifier().expect("built-in packs must compile");
        assert!(!classifier.rules().is_empty(), "packs should not be empty");
    }

    #[test]
    fn an_error_line_survives_a_run_of_noise() {
        let mut p = test_pipeline();
        p.push(b"warning: a\n", Stream::Stdout).expect("io");
        p.push(b"error: the thing broke\n", Stream::Stdout)
            .expect("io");
        p.finish().expect("io");
        let s = p.collapser.summary();
        assert_eq!(s.lines_in, 2);
        assert_eq!(s.errors, 1);
        assert!(s.lines_out >= 1);
    }

    #[test]
    fn both_streams_get_their_own_line_numbering() {
        let mut p = test_pipeline();
        p.push(b"out one\nout two\n", Stream::Stdout).expect("io");
        p.push(b"err one\n", Stream::Stderr).expect("io");
        assert_eq!(p.ingests[0].lines_emitted(), 2);
        assert_eq!(p.ingests[1].lines_emitted(), 1);
    }

    #[test]
    fn a_partial_line_on_one_stream_is_not_completed_by_another() {
        let mut p = test_pipeline();
        // "error: boom" has no newline yet on stdout.
        p.push(b"error: boom", Stream::Stdout).expect("io");
        p.push(b"completely unrelated\n", Stream::Stderr)
            .expect("io");
        let before = p.collapser.summary().lines_in;
        assert_eq!(before, 1, "only the stderr line is complete");
        p.finish().expect("io");
        assert_eq!(
            p.collapser.summary().lines_in,
            2,
            "the stdout partial is flushed at the end"
        );
        assert_eq!(p.collapser.summary().errors, 1);
    }

    #[test]
    fn masking_happens_after_classification_so_errors_still_fire() {
        let mut p = test_pipeline();
        // `error[E0432]: unresolved import` -- the bracketed code must survive
        // masking for `cargo.error` to match at all.
        p.push(b"error[E0432]: unresolved import `foo`\n", Stream::Stdout)
            .expect("io");
        p.finish().expect("io");
        assert_eq!(p.collapser.summary().errors, 1);
    }

    #[test]
    fn origin_maps_to_the_core_stream() {
        assert_eq!(Stream::from(Origin::ChildOut), Stream::Stdout);
        assert_eq!(Stream::from(Origin::ChildErr), Stream::Stderr);
    }

    #[test]
    fn a_classified_line_reports_its_rule_name() {
        let mut p = test_pipeline();
        let line = Line::new("error: boom", "error: boom", Stream::Stdout, 1);
        let c: Classified = p.classifier.classify(line);
        let name = c.rule_id.and_then(|id| p.classifier.rule_name(id));
        assert!(name.is_some_and(|n| n.starts_with("cargo.")), "{name:?}");
    }
}
