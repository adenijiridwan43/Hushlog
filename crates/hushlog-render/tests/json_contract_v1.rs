//! The additive v1 JSON contract: new fields, stable rule names, and the
//! legacy keys still present as aliases. **Owner: WP5.**

mod support;

use hushlog_core::Event;
use hushlog_render::{Format, RuleNames, renderer_with_names};
use serde_json::Value;
use support::{render_with, sample_events, sample_summary};

fn names() -> RuleNames {
    // `RuleId` is an index into the rule set; the fixture uses ids 1, 2 and 3.
    RuleNames::from_names([
        "unused.zero",
        "cargo.error",
        "cargo.warning",
        "cargo.progress",
    ])
}

fn records(out: &str) -> Vec<Value> {
    out.lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON record"))
        .collect()
}

#[test]
fn every_record_declares_v1_and_a_sequence() {
    let events = sample_events();
    let out = render_with(
        |sink| renderer_with_names(Format::Json, sink, names()),
        &events,
    );
    let recs = records(&out);
    assert_eq!(recs.len(), events.len());

    for (index, record) in recs.iter().enumerate() {
        assert_eq!(record["schema_version"], "1", "{record}");
        assert_eq!(record["sequence"], (index + 1) as u64, "{record}");
        assert!(record["type"].is_string(), "{record}");
    }
}

#[test]
fn folded_records_carry_the_new_fields_and_the_old_aliases() {
    let out = render_with(
        |sink| renderer_with_names(Format::Json, sink, names()),
        &sample_events(),
    );
    let recs = records(&out);

    let folded = recs
        .iter()
        .find(|record| record["type"] == "folded")
        .expect("the fixture folds a warning");
    assert_eq!(folded["folded"], true);
    assert_eq!(folded["fold_count"], 3);
    assert_eq!(folded["rule"], "cargo.warning");
    assert_eq!(folded["message"], folded["sample"]["plain"]);
    assert_eq!(folded["fingerprint"].as_str().map(str::len), Some(16));
    assert!(
        folded["fingerprint"]
            .as_str()
            .is_some_and(|fp| fp.bytes().all(|b| b.is_ascii_hexdigit()))
    );

    // The v0 keys remain, so an existing consumer does not break on the bump.
    assert_eq!(folded["fp"], 0x8f3c_1d2e_5b7a_9041_u64);
    assert_eq!(folded["count"], 3);
    assert_eq!(folded["rule_id"], 2);
    assert!(folded["sample"]["raw"].is_string());
}

#[test]
fn emitted_records_have_a_stable_rule_name_and_no_fold() {
    let out = render_with(
        |sink| renderer_with_names(Format::Json, sink, names()),
        &sample_events(),
    );
    let recs = records(&out);

    let error = recs
        .iter()
        .find(|record| record["type"] == "emit" && record["level"] == "error")
        .expect("the fixture emits errors");
    assert_eq!(error["rule"], "cargo.error");
    assert_eq!(error["rule_id"], 1);
    assert_eq!(error["folded"], false);
    assert_eq!(error["fold_count"], 1);
    assert_eq!(error["message"], error["plain"]);
    assert_eq!(error["highlight"], true);
    assert_eq!(error["stream"], "stdout");
}

#[test]
fn a_budgeted_run_ends_with_an_exact_budget_record() {
    let mut summary = sample_summary();
    summary.lines_suppressed = 7;
    summary.token_estimate = 91;
    summary.tokens_suppressed = 23;
    summary.max_tokens = Some(80);
    let out = render_with(
        |sink| renderer_with_names(Format::Json, sink, names()),
        &[Event::Summary(summary)],
    );
    let recs = records(&out);
    assert_eq!(recs.len(), 2);

    let last = &recs[1];
    assert_eq!(last["type"], "budget_summary");
    assert_eq!(last["schema_version"], "1");
    assert_eq!(last["sequence"], 2);
    assert_eq!(last["max_tokens"], 80);
    assert_eq!(last["token_estimate"], 91);
    assert_eq!(last["tokens_suppressed"], 23);
    assert_eq!(last["lines_suppressed"], 7);
    assert_eq!(last["tee_path"], "hushlog-full.log");
}

#[test]
fn summary_and_block_records_keep_the_type_envelope() {
    let out = render_with(
        |sink| renderer_with_names(Format::Json, sink, names()),
        &sample_events(),
    );
    let recs = records(&out);

    let summary = recs
        .iter()
        .find(|record| record["type"] == "summary")
        .expect("the fixture ends with a summary");
    assert_eq!(summary["lines_in"], 12_345);
    assert_eq!(summary["lines_out"], 18);
    assert_eq!(summary["errors"], 5);
    assert_eq!(summary["groups"], 2);
    assert_eq!(summary["lines_folded"], 3);
    assert_eq!(summary["lines_suppressed"], 0);
    assert_eq!(summary["tee_path"], "hushlog-full.log");

    assert!(recs.iter().any(|record| record["type"] == "block_start"));
    assert!(recs.iter().any(|record| record["type"] == "block_end"));
}
