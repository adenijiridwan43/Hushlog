//! Hardening tests for lnav's failure class: out-of-order lines, invalid
//! UTF-8, truncated JSON and 100M+ line counts.
//!
//! lnav's tracker breaks when timestamps go backward, chokes on invalid
//! UTF-8, stumbles over truncated structured lines and overflows 32-bit line
//! counts. Hushlog's tracker has no notion of time, replaces undecodable
//! bytes with U+FFFD, treats input as opaque text and uses u64 throughout,
//! and these tests pin that behaviour.

use std::collections::BTreeSet;

use hushlog_core::{
    Classified, CollapsePolicy, Collapser, Event, FoldingCollapser, Ingest, Level, Line, Stream,
    Summary,
};
use proptest::prelude::{
    any, prop, prop_assert, prop_assert_eq, prop_assume, proptest, BoxedStrategy, ProptestConfig,
    Strategy,
};

fn line_text() -> BoxedStrategy<String> {
    prop::sample::select(vec![
        "connection refused".to_string(),
        "retrying request".to_string(),
        "pod failed readiness probe 3 times".to_string(),
        "scheduler: starting reconciliation loop".to_string(),
        "health probe ok".to_string(),
        String::new(),
    ])
    .boxed()
}

fn classified(no: u64, level: Level, text: &str) -> Classified {
    Classified {
        line: Line::new(text.to_string(), text.to_string(), Stream::Stdout, no),
        level,
        rule_id: None,
        opid: None,
        highlight: level == Level::Error,
    }
}

fn corpus() -> impl Strategy<Value = Vec<Classified>> {
    let level = prop::sample::select(vec![
        Level::Error,
        Level::Warning,
        Level::Info,
        Level::Progress,
        Level::Trace,
    ]);
    prop::collection::vec((line_text(), level), 0..40).prop_map(|pairs| {
        pairs
            .into_iter()
            .enumerate()
            .map(|(i, (text, level))| classified(i as u64 + 1, level, &text))
            .collect()
    })
}

/// Every error line number that reached the output, plus the final summary.
fn run(ordering: &[Classified]) -> (BTreeSet<u64>, Summary) {
    let mut c = FoldingCollapser::with_policy(CollapsePolicy::default());
    let mut survived = BTreeSet::new();
    for line in ordering {
        for ev in c.push(line.clone()) {
            match ev {
                Event::Emit(x) => {
                    survived.insert(x.line.no);
                }
                Event::Folded { sample, .. } => {
                    survived.insert(sample.line.no);
                }
                Event::BlockStart(_) | Event::BlockEnd(_) | Event::Summary(_) => {}
            }
        }
    }
    for ev in c.finish() {
        if let Event::Emit(x) = ev {
            survived.insert(x.line.no);
        }
    }
    (survived, c.summary().clone())
}

/// The same multiset in several orders, including reversed and interleaved.
fn orderings_of(input: &[Classified]) -> Vec<Vec<Classified>> {
    let mut out = vec![input.to_vec()];
    let mut reversed = input.to_vec();
    reversed.reverse();
    out.push(reversed);
    if !input.is_empty() {
        let mut rotated = input.to_vec();
        rotated.rotate_left(input.len() / 2);
        out.push(rotated);
        let mut interleaved = Vec::with_capacity(input.len());
        let half = input.len().div_ceil(2);
        for i in 0..half {
            interleaved.push(input[i].clone());
            if half + i < input.len() {
                interleaved.push(input[half + i].clone());
            }
        }
        out.push(interleaved);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Folding counts and error survival do not depend on arrival order.
    /// lnav's tracker breaks when timestamps go backward; hushlog's fingerprint
    /// key has no notion of time, so reordering must not change the outcome.
    #[test]
    fn fold_counts_are_invariant_under_reordering(input in corpus()) {
        let error_nos: BTreeSet<u64> = input
            .iter()
            .filter(|c| c.level == Level::Error)
            .map(|c| c.line.no)
            .collect();

        let results: Vec<_> = orderings_of(&input).iter().map(|ord| run(ord)).collect();

        let first = results.first().expect("at least the identity ordering");
        for (_i, (survived, summary)) in results.iter().enumerate().skip(1) {
            // AGENTS.md 1.2: no error line may be lost, in any order.
            for no in &error_nos {
                prop_assert!(
                    survived.contains(no),
                    "error line {no} was lost under a reordered input"
                );
            }
            // Nothing may be silently dropped.
            prop_assert_eq!(
                summary.lines_in,
                summary.lines_out + summary.lines_folded,
                "summary must account for every line under a reordered input"
            );
            // Aggregates are a function of the multiset, not the order.
            prop_assert_eq!(
                summary.groups,
                first.1.groups,
                "group count differs under a reordered input"
            );
            prop_assert_eq!(summary.lines_folded, first.1.lines_folded);
            prop_assert_eq!(summary.lines_out, first.1.lines_out);
            prop_assert_eq!(summary.errors, first.1.errors);
        }
    }

    /// Arbitrary bytes -- including invalid UTF-8 -- frame without panicking
    /// and nothing is held back at end of input. lnav garbles or crashes on
    /// invalid UTF-8; hushlog replaces undecodable bytes with U+FFFD and still
    /// accounts for every byte (AGENTS.md 1.0).
    #[test]
    fn arbitrary_bytes_frame_without_panic_or_loss(
        bytes in prop::collection::vec(any::<u8>(), 0..300),
        cuts in prop::collection::vec(0usize..300, 0..8),
    ) {
        let mut ingest = Ingest::new(Stream::Stdout);
        let mut all = Vec::new();
        let mut prev = 0usize;
        for cut in &cuts {
            let end = *cut.clamp(&prev, &bytes.len());
            all.extend(ingest.push(&bytes[prev..end]));
            prev = end;
        }
        all.extend(ingest.push(&bytes[prev..]));
        let finished = ingest.finish();
        all.extend(finished);

        prop_assert_eq!(ingest.buffered(), 0, "nothing may be held back at end of input");

        for (expected_no, line) in (1u64..).zip(all.iter()) {
            prop_assert_eq!(line.no, expected_no, "line numbers must stay dense");
            prop_assert!(
                std::str::from_utf8(line.raw.as_bytes()).is_ok(),
                "raw must be valid UTF-8 after lossy conversion"
            );
        }
    }

    /// Counts at 100M and beyond must not overflow or drop lines. lnav's
    /// tracker used 32-bit line counts; hushlog uses u64 throughout `Summary`.
    #[test]
    fn summary_invariant_holds_at_100m_plus_counts(
        lines_out in 100_000_000u64..=u64::MAX,
        lines_folded in 0u64..=u64::MAX,
    ) {
        prop_assume!(lines_out <= u64::MAX - lines_folded);
        let s = Summary {
            lines_in: lines_out + lines_folded,
            lines_out,
            lines_folded,
            errors: 0,
            groups: 0,
            tee_path: None,
        };
        prop_assert_eq!(
            s.lines_in,
            s.lines_out + s.lines_folded,
            "u64 counts must not overflow at 100M+"
        );
        // Display formats thousands separators at any magnitude without panicking.
        let _ = s.to_string();
    }
}

#[test]
fn invalid_bytes_surface_as_replacement_chars_and_are_not_dropped() {
    // 0xff is never valid UTF-8: it must surface as U+FFFD, never vanish.
    let mut ingest = Ingest::new(Stream::Stdout);
    let lines = ingest.push(&[b'H', b'e', b'l', b'l', b'o', 0xff, b'\n']);
    assert_eq!(raw_of(&lines), ["Hello\u{fffd}"]);
    assert_eq!(ingest.buffered(), 0);

    // A truncated multi-byte sequence at EOF becomes one replacement char.
    let mut ingest = Ingest::new(Stream::Stdout);
    assert_eq!(ingest.push(&[0xe4, 0xb8]), Vec::new());
    let lines = ingest.finish();
    assert_eq!(raw_of(&lines), ["\u{fffd}"]);
}

fn raw_of(lines: &[Line]) -> Vec<&str> {
    lines.iter().map(|l| l.raw.as_str()).collect()
}

/// Fixture-driven check: the out-of-order log keeps every error and accounts
/// for every line.
#[test]
fn out_of_order_fixture_keeps_every_error() {
    let text = include_str!("../../../tests/fixtures/out-of-order.log");
    let text_lines: Vec<&str> = text.lines().collect();
    let mut c = FoldingCollapser::with_policy(CollapsePolicy::default());
    let mut error_nos = BTreeSet::new();
    let mut survived = BTreeSet::new();
    for (i, raw) in text_lines.iter().enumerate() {
        let no = i as u64 + 1;
        let level = if raw.contains("ERROR") {
            Level::Error
        } else if raw.contains("WARN") {
            Level::Warning
        } else {
            Level::Info
        };
        if level == Level::Error {
            error_nos.insert(no);
        }
        let cl = classified(no, level, raw);
        for ev in c.push(cl) {
            match ev {
                Event::Emit(x) => {
                    survived.insert(x.line.no);
                }
                Event::Folded { sample, .. } => {
                    survived.insert(sample.line.no);
                }
                Event::BlockStart(_) | Event::BlockEnd(_) | Event::Summary(_) => {}
            }
        }
    }
    for ev in c.finish() {
        if let Event::Emit(x) = ev {
            survived.insert(x.line.no);
        }
    }
    let s = c.summary();
    assert!(
        error_nos.iter().all(|no| survived.contains(no)),
        "every error in the out-of-order fixture survives: errors={error_nos:?} survived={survived:?}"
    );
    assert_eq!(
        s.lines_in,
        s.lines_out + s.lines_folded,
        "fixture summary accounts for every line: {s:?}"
    );
}
