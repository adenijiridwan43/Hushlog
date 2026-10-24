//! Truncated JSON must survive rendering as valid newline-delimited JSON.
//! lnav's tracker breaks on truncated structured lines; hushlog treats input
//! lines as opaque text and lets `serde_json` escape everything, so every
//! output record parses as JSON.

mod support;

use hushlog_core::{Classified, Event, Level, Line, Stream};
use hushlog_render::Format;
use proptest::prelude::{prop, proptest, BoxedStrategy, ProptestConfig, Strategy};
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
    assert!(
        lines.next().is_none(),
        "exactly one JSON record per event"
    );
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
