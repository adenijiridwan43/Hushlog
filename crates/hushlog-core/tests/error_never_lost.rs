//! The error-never-lost invariant.
//!
//! Every line the classifier marks `Level::Error` must reach the output. Not
//! summarised, not counted away, not folded. This is the promise in
//! `AGENTS.md` section 1, and this file is its enforcement.
//!
//! Written before any collapse logic existed, deliberately. It passes
//! trivially today. Its value is that it must never stop passing: WP4 adds the
//! real collapser and has to keep this green while making output shorter.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use hushlog_core::{Classified, CollapsePolicy, Collapser, Event, Level, Line, Stream};
use proptest::prelude::*;

/// An adversarial stand-in for the real collapser.
///
/// It folds *everything* it can, which is exactly the failure mode we are
/// guarding against. The only thing standing between it and a hidden error is
/// [`CollapsePolicy::collapse_errors`], which is `false` by default.
///
/// WP4 replaces this with the production [`Collapser`]; this harness stays as
/// the oracle both are measured against.
struct GreedyCollapser {
    seen: HashMap<u64, (Classified, u64)>,
    policy: CollapsePolicy,
}

impl Collapser for GreedyCollapser {
    fn push(&mut self, c: Classified) -> Vec<Event> {
        let fp = fingerprint(&c.line.plain);

        let Some((_, count)) = self.seen.get_mut(&fp) else {
            self.seen.insert(fp, (c.clone(), 1));
            return vec![Event::Emit(c)];
        };

        *count += 1;
        let count = *count;

        // The policy is the whole point: errors pass through untouched.
        let may_fold = c.level != Level::Error || self.policy.collapse_errors;
        let sample = self.seen[&fp].0.clone();

        if may_fold {
            vec![Event::Folded { fp, count, sample }]
        } else {
            vec![Event::Emit(c)]
        }
    }

    fn finish(&mut self) -> Vec<Event> {
        Vec::new()
    }
}

fn fingerprint(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn classify(no: u64, level: Level, text: &str) -> Classified {
    Classified {
        line: Line::new(text.to_string(), text.to_string(), Stream::Stdout, no),
        level,
        rule_id: None,
        highlight: level == Level::Error,
    }
}

/// Run a batch through the pipeline and return every line number that reached
/// the output, by any route.
fn survivors(input: &[Classified]) -> std::collections::BTreeSet<u64> {
    let mut collapser = GreedyCollapser {
        seen: HashMap::new(),
        policy: CollapsePolicy::default(),
    };

    let mut out = std::collections::BTreeSet::new();
    for c in input {
        for ev in collapser.push(c.clone()) {
            match ev {
                Event::Emit(c) => {
                    out.insert(c.line.no);
                }
                Event::Folded { sample, .. } => {
                    out.insert(sample.line.no);
                }
                Event::BlockStart(_) | Event::BlockEnd(_) | Event::Summary(_) => {}
            }
        }
    }
    for ev in collapser.finish() {
        if let Event::Emit(c) = ev {
            out.insert(c.line.no);
        }
    }
    out
}

fn line_strategy() -> impl Strategy<Value = Classified> {
    (
        1u64..500,
        prop::sample::select(vec![
            Level::Error,
            Level::Warning,
            Level::Info,
            Level::Progress,
            Level::Trace,
        ]),
        "(a|b|c|error|warn|retry|deprecated).{0,24}",
    )
        .prop_map(|(no, level, text)| classify(no, level, &text))
}

proptest! {
    /// The invariant itself.
    #[test]
    fn every_error_line_survives(input in prop::collection::vec(line_strategy(), 0..200)) {
        let expected: Vec<u64> = input.iter().filter(|c| c.level == Level::Error).map(|c| c.line.no).collect();
        let got = survivors(&input);

        for &no in &expected {
            prop_assert!(
                got.contains(&no),
                "error line {no} was lost. expected {expected:?}, got {got:?}"
            );
        }
    }

    /// Guard against the invariant passing vacuously because nothing was
    /// classified as an error in the first place.
    #[test]
    fn invariant_is_not_vacuous(input in prop::collection::vec(line_strategy(), 1..200)) {
        prop_assume!(input.iter().any(|c| c.level == Level::Error));
        let got = survivors(&input);
        prop_assert!(!got.is_empty());
    }
}

/// The human-readable statement of the same promise, so a failure here is
/// legible without reading generator internals.
#[test]
fn a_repeated_error_is_still_shown_every_time() {
    let input: Vec<Classified> = (1..=4)
        .map(|no| classify(no, Level::Error, "connection refused"))
        .collect();

    // Greedy collapser wants to fold these into one. It must not be allowed to.
    let got = survivors(&input);
    assert_eq!(
        got.len(),
        4,
        "all four error lines must appear, got {got:?}"
    );
}
