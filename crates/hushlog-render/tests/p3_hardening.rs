//! Truncated JSON must survive rendering as valid newline-delimited JSON.
//! lnav's tracker breaks on truncated structured lines; hushlog treats input
//! lines as opaque text and lets `serde_json` escape everything, so every
//! output record parses as JSON.

mod support;

use hushlog_core::{Classified, Event, Level, Line, Stream};
use hushlog_render::Format;
use proptest::prelude::{BoxedStrategy, ProptestConfig, Strategy, prop, proptest};
use serde_json::Value;

use support::render;

/// Fragment strategies include genuinely truncated JSON so the property is
/// not merely "valid JSON in, valid JSON out".
fn json_text() -> BoxedStrategy<String> {
    prop::collection::vec(
        prop::sample::select(vec![
            "a".to_string(),
            "\"".to_string(),
            "\\".to_string(),
            "</script>".to_string(),
            r#"{"level":"error","msg":"#.to_string(),
            "[1,2,".to_string(),
            r#"{"nested":{"#.to_string(),
        ]),
        0..60,
    )
    .prop_map(|parts| parts.concat())
    .boxed()
}

fn event_for(text: &str, no: u64) -> Event {
    let c = Classified {
        line: Line::new(text.to_string(), text.to_string(), Stream::Stdout, no),
        level: Level::Info,
        rule_id: None,
        opid: None,
        highlight: false,
    };
    Event::Emit(c)
}

fn assert_valid_ndjson(out: &str, expected_records: usize) {
    let mut lines = out.lines();
    for i in 0..expected_records {
        let record = lines
            .next()
            .unwrap_or_else(|| panic!("missing JSON record {i}"));
        serde_json::from_str::<Value>(record)
            .unwrap_or_else(|e| panic!("record {i} ({record:?}) is not valid JSON: {e}"));
    }
    assert!(lines.next().is_none(), "exactly one JSON record per event");
}

#[test]
fn truncated_json_fixture_renders_as_valid_ndjson() {
    let text = include_str!("../../../tests/fixtures/truncated-json.log");
    let events: Vec<Event> = text
        .lines()
        .enumerate()
        .map(|(i, raw)| event_for(raw, i as u64 + 1))
        .collect();
    let out = render(Format::Json, &events);
    assert_valid_ndjson(&out, text.lines().count());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_text_including_truncated_json_renders_as_valid_ndjson(text in json_text()) {
        let out = render(Format::Json, &[event_for(&text, 1)]);
        assert_valid_ndjson(&out, 1);
    }
}

// ---------------------------------------------------------------------------
// A group that folds many times must be announced once, not once per repeat.
//
// `FoldingCollapser` emits one `Event::Folded` per repeat, each carrying the
// group's running `count` (see `collapse.rs` and its
// `fold_count_stays_exact_at_large_repeat_counts` test). A renderer that prints
// a notice for every one of those events defeats the whole product: a 441-line
// log of one repeated shape came out 441 lines long, with 400 copies of
// "... N similar lines folded".
//
// Caught by running a real capture (`real-relanflow-ratelimit-storm.log`)
// through the binary. The renderer must announce a group on its first fold and
// stay quiet after that; the exact folded total is reported in the Summary, so
// nothing is lost (AGENTS.md section 1).
// ---------------------------------------------------------------------------

/// One `Emit`, then `n - 1` folds of the same fingerprint with a rising count.
fn one_group_folding(n: u64) -> Vec<Event> {
    let sample = Classified {
        line: Line::new(
            "GET /api/flows 429 0.340 ms - 56",
            "GET /api/flows 429 0.340 ms - 56",
            Stream::Stdout,
            1,
        ),
        level: Level::Info,
        rule_id: None,
        opid: None,
        highlight: false,
    };
    let mut events = vec![Event::Emit(sample.clone())];
    for count in 2..=n {
        events.push(Event::Folded {
            fp: 0xABCD,
            count,
            sample: sample.clone(),
            first_fold: count == 2,
        });
    }
    events
}

/// Count a group's announcement in a CI format.
///
/// The CI formats put the count in the fold command's title *and* in the note
/// inside it -- `::group::3 similar lines folded` followed by
/// `  ... 3 similar lines folded` -- so "similar lines folded" appears twice per
/// announced group there. Counting the indented note is the format-independent
/// way to count a group's announcement.
fn fold_notice_count(out: &str) -> usize {
    out.lines()
        .filter(|l| l.trim_start().starts_with("... ") && l.contains("similar line"))
        .count()
}

#[test]
fn a_repeatedly_folded_group_is_announced_once_in_plain() {
    let out = render(Format::Plain, &one_group_folding(400));
    assert_eq!(
        fold_notice_count(&out),
        1,
        "one group must yield one notice, not one per repeat; output was:\n{out}"
    );
    assert_eq!(
        out.lines().count(),
        2,
        "an emit plus a single notice is the whole output"
    );
}

#[test]
fn a_repeatedly_folded_group_is_announced_once_in_tty_and_ci_formats() {
    // The human-facing formats must announce a group once, however many times
    // it repeats.
    for format in [Format::Tty, Format::Github, Format::Gitlab] {
        let out = render(format, &one_group_folding(50));
        let notices = fold_notice_count(&out);
        assert!(
            notices <= 1,
            "{format:?} announced one group {notices} times; output was:\n{out}"
        );
    }
}

#[test]
fn machine_formats_record_every_fold_event() {
    // json and csv are record streams, not display: a fold counter is data
    // there, so one record per event is correct and nothing may be merged away.
    let events = one_group_folding(50);
    let json = render(Format::Json, &events);
    assert_eq!(
        json.lines().count(),
        events.len(),
        "one JSON record per event"
    );
    for line in json.lines() {
        serde_json::from_str::<Value>(line).unwrap_or_else(|e| panic!("bad record {line:?}: {e}"));
    }

    let csv = render(Format::Csv, &events);
    assert_eq!(
        csv.lines().count(),
        events.len() + 1,
        "a header row plus one CSV row per event"
    );
}

#[test]
fn the_real_rate_limit_storm_collapses_to_its_group_count() {
    // The capture that exposed the bug: startup block plus ~400 access-log
    // lines built from a handful of distinct shapes.
    let text = include_str!("../../../tests/fixtures/real-relanflow-ratelimit-storm.log");
    let lines: Vec<&str> = text.lines().collect();

    // Feed the lines through the event shape the collapser produces for a
    // repeated line: first sighting emitted, later sighting folded. Grouping
    // here is by the visible shape (endpoint and status), which is what the
    // fingerprints collapse to once volatile numbers are masked.
    let shape = |s: &str| -> String {
        s.chars()
            .filter(|c| !c.is_ascii_digit())
            .collect::<String>()
    };
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut events = Vec::new();
    let mut next_fp = 1u64;
    let mut fps: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
    for (i, raw) in lines.iter().enumerate() {
        let key = shape(raw);
        let no = i as u64 + 1;
        let c = Classified {
            line: Line::new(*raw, *raw, Stream::Stdout, no),
            level: Level::Info,
            rule_id: None,
            opid: None,
            highlight: false,
        };
        match fps.get_mut(&key) {
            None => {
                let fp = next_fp;
                next_fp += 1;
                fps.insert(key.clone(), (fp, 1));
                seen.insert(key);
                events.push(Event::Emit(c));
            }
            Some((fp, count)) => {
                *count += 1;
                events.push(Event::Folded {
                    fp: *fp,
                    count: *count,
                    sample: c,
                    first_fold: *count == 2,
                });
            }
        }
    }

    // Only a shape that repeats is ever announced; a shape seen once is emitted
    // and never folds. Count the groups that actually fold.
    let repeating_groups = fps.values().filter(|(_, count)| *count > 1).count();
    let out = render(Format::Plain, &events);
    assert_eq!(
        fold_notice_count(&out),
        repeating_groups,
        "each of the {repeating_groups} repeating shapes must be announced exactly once"
    );
    assert!(
        repeating_groups > 0,
        "the fixture must contain repetition or it proves nothing"
    );
    assert!(
        out.lines().count() < lines.len() / 2,
        "collapsing must at least halve a storm of {} lines; got {}",
        lines.len(),
        out.lines().count()
    );
}
