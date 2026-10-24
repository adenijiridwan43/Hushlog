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
    Anonymizer, BlockDetector, Classifier, CollapsePolicy, Collapser, Event, FoldingCollapser,
    FormatDetector, Ingest, Level, Normalizer, Renderer, Summary, Stream,
};
use hushlog_rules::RuleClassifier;

use crate::args::{self, Config};
use crate::explain::Explainer;
use crate::query::Query;
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
#[allow(clippy::struct_excessive_bools)]
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
    /// Running counts for the current query-mode run; `summary` (the bool)
    /// gates whether the final `Event::Summary` is rendered.
    stats: Summary,
    /// Where the verbatim log went, for the summary's "full log: PATH" tail.
    /// `hushlog-core` builds the `Summary` and knows nothing about `--tee`,
    /// so the CLI fills this in on the way out.
    tee_path: Option<String>,
    /// True once stdout is gone (`hushlog ... | head`), so we stop writing.
    stdout_closed: bool,
    /// Whether `--anonymize` is active: sensitive values in rendered output
    /// are replaced with consistent fake values. The tee file is never
    /// anonymized, so `--tee` plus `--anonymize` is safe.
    anonymize: bool,
    /// The anonymizer, only allocated when `--anonymize` is active.
    anonymizer: Option<Anonymizer>,
    /// The parsed `--query` predicate, if any. When set the pipeline skips the
    /// collapser and emits only matching lines as structured records.
    query: Option<Query>,
    /// Loaded packs, kept so the classifier can be rebuilt once a format is
    /// detected (the detected pack's rules are tried first).
    packs: Vec<hushlog_rules::RulePack>,
    /// Tries each pack's `detect` patterns against the first few lines,
    /// ported from lnav's format auto-detection. `None` when no pack declares
    /// detect patterns. Once detection is complete (a pack matched or the
    /// window expired), this is set to `None` so subsequent lines skip the
    /// detection pass entirely.
    format_detector: Option<FormatDetector>,
    /// True once detection has completed (either a pack matched or the window
    /// expired). After this, classification runs normally.
    detection_complete: bool,
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

        // Format detection: collect each pack's `detect` patterns. When no pack
        // declares any, detection is skipped and the run uses all packs as
        // before.
        let candidates: Vec<(String, Vec<String>)> = packs
            .iter()
            .filter_map(|p| {
                p.detect.as_ref().map(|d| {
                    (
                        p.name.clone().unwrap_or_else(|| p.location().to_string()),
                        d.patterns.iter().map(|pat| pat.0.clone()).collect(),
                    )
                })
            })
            .collect();
        let format_detector = if candidates.is_empty() {
            None
        } else {
            Some(
                FormatDetector::new(candidates)
                    .map_err(|e| anyhow::anyhow!("building format detector: {e}"))?,
            )
        };

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

        let query_mode = config.query.is_some();
        // Query mode classifies against all packs and must not let format
        // detection swap the classifier out from under a running predicate.
        let detection_complete = format_detector.is_none() || query_mode;
        let anonymize = config.anonymize;
        let query = config
            .query
            .as_deref()
            .map(Query::parse)
            .transpose()
            .map_err(|e| anyhow::anyhow!("invalid --query expression: {e}"))?;
        Ok(Self {
            classifier,
            ingests: [Ingest::new(Stream::Stdout), Ingest::new(Stream::Stderr)],
            collapser,
            renderer,
            out,
            explain,
            normalizer: Normalizer::new(),
            summary: config.summary,
            stats: Summary::default(),
            tee_path,
            stdout_closed: false,
            anonymize,
            anonymizer: if anonymize {
                Some(Anonymizer::new())
            } else {
                None
            },
            packs,
            format_detector,
            detection_complete,
            query,
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
        self.handle(tail)?;

        if self.query.is_some() {
            // Query mode bypasses the collapser, so the summary is rendered here.
            self.render_all(vec![Event::Summary(self.stats.clone())])?;
        } else {
            let events = self.collapser.finish();
            self.render_all(events)?;
        }
        if let Some(explain) = self.explain.as_mut() {
            explain.flush()?;
        }
        self.out.flush()
    }

    /// Classify, collapse and render a batch of freshly framed lines.
    ///
    /// While format detection is still open, each line is first probed against
    /// each pack's `detect` patterns (ported from lnav's format auto-detection).
    /// The first pack to match wins: the classifier is rebuilt with that pack's
    /// rules tried first, and detection is closed. If no pack matches within
    /// the window, detection closes and the run continues with all packs (the
    /// pre-detection behaviour).
    ///
    /// Lines are classified immediately as they arrive; detection does not
    /// buffer or delay output.
    fn handle(&mut self, lines: Vec<hushlog_core::Line>) -> io::Result<()> {
        if self.query.is_some() {
            return self.handle_query(lines);
        }
        for line in lines {
            // Run format detection before classification, while it is still
            // open. The detected pack's rules are tried first for subsequent
            // lines; lines already classified are unaffected. Detection does
            // not buffer or delay output -- every line is classified
            // immediately.
            if !self.detection_complete {
                let detected_name = self
                    .format_detector
                    .as_mut()
                    .and_then(|detector| detector.push(&line.plain).map(str::to_string));
                // Close detection when a pack matches or the window expires.
                let has_match = detected_name.is_some();
                let window_expired =
                    self.format_detector.as_ref().is_none_or(|d| !d.is_open());
                if let Some(name) = detected_name {
                    self.rebuild_classifier_for(&name)
                        .map_err(io::Error::other)?;
                }
                if has_match || window_expired {
                    // A pack matched, or the window expired without a match:
                    // detection is done, fall back to all packs for subsequent
                    // lines.
                    self.detection_complete = true;
                    self.format_detector = None;
                }
            }

            // Masking is for grouping only. Applied here, after the rule engine
            // has had its look at the text a human would read, so masking can
            // never suppress an error rule.
            let mut classified = self.classifier.classify(line);
            classified.line.plain = self.normalizer.mask(&classified.line.plain).into_owned();
            // Anonymize the rendered text (what the user sees). The tee file
            // is written byte-exact upstream of ingestion and is never
            // anonymized, so `--tee` plus `--anonymize` is safe.
            if self.anonymize {
                if let Some(anon) = self.anonymizer.as_mut() {
                    classified.line.raw = anon.anonymize(&classified.line.raw);
                }
            }
            let events = self.collapser.push(classified);
            self.render_all(events)?;
        }
        Ok(())
    }

    /// Query mode: classify and mask each line, then emit it as an [`Event::Emit`]
    /// only when the `--query` predicate matches. The collapser is bypassed, so
    /// nothing is folded and every matching line appears verbatim.
    fn handle_query(&mut self, lines: Vec<hushlog_core::Line>) -> io::Result<()> {
        for line in lines {
            // Masking is for grouping only; applied after classification so it
            // can never suppress an error rule.
            let mut classified = self.classifier.classify(line);
            classified.line.plain = self.normalizer.mask(&classified.line.plain).into_owned();
            if self.anonymize {
                if let Some(anon) = self.anonymizer.as_mut() {
                    classified.line.raw = anon.anonymize(&classified.line.raw);
                }
            }

            let matched = {
                let q = self.query.as_ref().expect("query mode is active");
                q.matches(&classified)
            };

            self.stats.lines_in += 1;
            if matched {
                if classified.level == Level::Error {
                    self.stats.errors += 1;
                }
                self.stats.lines_out += 1;
                self.render_all(vec![Event::Emit(classified)])?;
            }
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

    /// Rebuild the classifier so the detected pack's rules are tried first,
    /// followed by every other pack's rules in their original order. Called
    /// once, when format detection locks on to a pack.
    fn rebuild_classifier_for(&mut self, name: &str) -> anyhow::Result<()> {
        let mut ordered: Vec<hushlog_rules::RulePack> = self
            .packs
            .iter()
            .filter(|p| p.name.as_deref() == Some(name))
            .cloned()
            .collect();
        ordered.extend(
            self.packs
                .iter()
                .filter(|p| p.name.as_deref() != Some(name))
                .cloned(),
        );
        self.classifier = RuleClassifier::new(&ordered)
            .map_err(|e| anyhow::Error::msg(e.to_string()))
            .context(format!("rebuilding rule set for detected format {name}"))?;

        // Rebuild the block detector against the new rule order, so block
        // markers from the detected pack take effect.
        let policy = self.collapser.policy();
        let detector = block_detector_or_warn(&self.classifier);
        self.collapser = FoldingCollapser::with_policy_and_detector(policy, detector);

        if let Some(explain) = self.explain.as_mut() {
            explain.note(&format!("format detected: {name}"))?;
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
