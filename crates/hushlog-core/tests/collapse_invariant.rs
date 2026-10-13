//! The error-never-lost invariant, run against the **production** collapser.
//!
//! `error_never_lost.rs` checks the property against a deliberately adversarial
//! stand-in. That proves the *policy* is sufficient. This file proves the
//! *implementation* honours it -- `FoldingCollapser` in both its line mode and
//! its block mode, including the block-mode path where lines are deferred into
//! a pending buffer and flushed later.
//!
//! Block mode is the riskier half. A line held in `pending` is a line not yet
//! written, and AGENTS.md section 1 says anything not shown must be
//! recoverable. If `finish()` ever failed to flush, or a block swallowed its
//! contents, lines would vanish with no error anywhere.
//!
//! If this file ever fails, the product's central promise is broken.

use std::collections::BTreeSet;

use hushlog_core::{
    BlockDetector, Classified, CollapsePolicy, Collapser, Event, FoldingCollapser, Level, Line,
    Stream,
};
use proptest::prelude::*;

/// Every line number that reached the output, by any route.
fn survivors(collapser: &mut impl Collapser, input: &[Classified]) -> BTreeSet<u64> {
    let mut out = BTreeSet::new();
    let note = |events: Vec<Event>, out: &mut BTreeSet<u64>| {
        for ev in events {
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
    };
    for c in input {
        note(collapser.push(c.clone()), &mut out);
    }
    // finish() must flush anything still pending; skipping it would hide the
    // exact bug this file exists to catch.
    note(collapser.finish(), &mut out);
    out
}

/// A deliberately small text alphabet, so generated input repeats heavily.
/// Folding only does interesting things when lines collide.
fn input_strategy() -> impl Strategy<Value = Vec<Classified>> {
    let level = prop::sample::select(vec![
        Level::Error,
        Level::Warning,
        Level::Info,
        Level::Progress,
        Level::Trace,
    ]);
    let text = prop::sample::select(vec![
        "a",
        "b",
        "error: boom",
        "warning: deprecated",
        "  indented continuation",
        "    at src/main.rs:1:1",
        "",
    ]);
    prop::collection::vec((level, text), 0..150).prop_map(|v| {
        v.into_iter()
            .enumerate()
            .map(|(i, (level, text))| Classified {
                line: Line::new(
                    text.to_string(),
                    text.to_string(),
                    Stream::Stdout,
                    i as u64 + 1,
                ),
                level,
                rule_id: None,
                highlight: level == Level::Error,
            })
            .collect()
    })
}

fn error_line_numbers(input: &[Classified]) -> Vec<u64> {
    input
        .iter()
        .filter(|c| c.level == Level::Error)
        .map(|c| c.line.no)
        .collect()
}

proptest! {
    /// Line mode: the default pipeline configuration.
    #[test]
    fn line_mode_never_loses_an_error(input in input_strategy()) {
        let expected = error_line_numbers(&input);
        let mut c = FoldingCollapser::with_policy(CollapsePolicy::default());
        let got = survivors(&mut c, &input);
        for no in expected {
            prop_assert!(got.contains(&no), "error line {no} lost in line mode");
        }
    }

    /// Block mode: lines are deferred while a block is open. This is the path
    /// where a line can be accepted and never written.
    #[test]
    fn block_mode_never_loses_an_error(input in input_strategy()) {
        let expected = error_line_numbers(&input);
        let mut c = FoldingCollapser::with_policy_and_detector(
            CollapsePolicy::default(),
            BlockDetector::new(),
        );
        let got = survivors(&mut c, &input);
        for no in expected {
            prop_assert!(got.contains(&no), "error line {no} lost in block mode");
        }
    }

    /// Whatever the folding policy, the counts must still account for every
    /// line. This is the numeric form of AGENTS.md section 1.
    #[test]
    fn the_summary_accounts_for_every_line(input in input_strategy()) {
        let mut c = FoldingCollapser::with_policy(CollapsePolicy::default());
        let _ = survivors(&mut c, &input);
        let s = c.summary();
        prop_assert_eq!(s.lines_in, input.len() as u64, "lines_in must match input");
        prop_assert_eq!(
            s.lines_out + s.lines_folded,
            s.lines_in,
            "output plus folded must account for every line"
        );
    }
}

/// The canonical case, stated plainly: byte-identical error lines, which naive
/// deduplication would collapse to one.
#[test]
fn identical_error_lines_all_survive_in_both_modes() {
    let input: Vec<Classified> = (1..=6)
        .map(|no| Classified {
            line: Line::new(
                "connection refused".to_string(),
                "connection refused".to_string(),
                Stream::Stdout,
                no,
            ),
            level: Level::Error,
            rule_id: None,
            highlight: true,
        })
        .collect();

    let mut line_mode = FoldingCollapser::with_policy(CollapsePolicy::default());
    assert_eq!(
        survivors(&mut line_mode, &input).len(),
        6,
        "line mode dropped one"
    );

    let mut block_mode =
        FoldingCollapser::with_policy_and_detector(CollapsePolicy::default(), BlockDetector::new());
    assert_eq!(
        survivors(&mut block_mode, &input).len(),
        6,
        "block mode dropped one"
    );
}

/// The opposite direction: repeated *warnings* genuinely should fold. If this
/// ever stops happening the tool is doing nothing, and the property tests above
/// would still pass -- so this one matters as a counterweight.
#[test]
fn repeated_warnings_still_collapse() {
    let input: Vec<Classified> = (1..=50)
        .map(|no| Classified {
            line: Line::new(
                "npm WARN deprecated".to_string(),
                "npm WARN deprecated".to_string(),
                Stream::Stdout,
                no,
            ),
            level: Level::Warning,
            rule_id: None,
            highlight: false,
        })
        .collect();

    let mut c = FoldingCollapser::with_policy(CollapsePolicy::default());
    let survivors = survivors(&mut c, &input);
    let s = c.summary();

    assert_eq!(survivors.len(), 1, "50 identical warnings should show once");
    assert_eq!(s.lines_folded, 49, "the other 49 should be folded");
    assert_eq!(s.lines_out, 1);
}
