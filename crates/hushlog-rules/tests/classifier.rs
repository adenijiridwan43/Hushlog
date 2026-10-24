//! Classifier tests: level assignment, rule resolution and first-match-wins.

use std::fmt::Write as _;

use hushlog_core::{Classifier, Level, Line, Stream};
use hushlog_rules::{RuleClassifier, parse_pack};

/// Build a classifier from inline TOML.
fn build(src: &str) -> RuleClassifier {
    let pack = parse_pack("test.toml", src).expect("pack should parse");
    RuleClassifier::from_pack(&pack).expect("pack should compile")
}

fn classify(c: &mut RuleClassifier, text: &str) -> hushlog_core::Classified {
    let line = Line::new(text, text, Stream::Stdout, 1);
    c.classify(line)
}

const CARGO_LIKE: &str = r#"
[[rule]]
id = "cargo.error"
match = { kind = "regex", pattern = "^error(?:\\[E\\d+\\])?: " }
level = "error"
highlight = true

[[rule]]
id = "cargo.warning"
match = { kind = "regex", pattern = "^warning: " }
level = "warning"

[[rule]]
id = "cargo.note"
match = { kind = "literal", pattern = "note:" }
level = "info"
"#;

#[test]
fn a_warning_line_is_a_warning() {
    let mut c = build(CARGO_LIKE);
    let got = classify(&mut c, "warning: unused variable: `ctx`");
    assert_eq!(got.level, Level::Warning);
    assert!(!got.highlight);
}

#[test]
fn an_error_line_is_an_error() {
    let mut c = build(CARGO_LIKE);
    let got = classify(&mut c, "error[E0308]: mismatched types");
    assert_eq!(got.level, Level::Error);
    assert!(got.highlight, "the rule asked for highlight");
}

#[test]
fn a_bare_error_line_is_also_an_error() {
    let mut c = build(CARGO_LIKE);
    let got = classify(&mut c, "error: could not compile `hushlog-core`");
    assert_eq!(got.level, Level::Error);
}

#[test]
fn an_unknown_line_defaults_to_info() {
    let mut c = build(CARGO_LIKE);
    let got = classify(&mut c, "    Checking hushlog-core v0.1.0");
    assert_eq!(got.level, Level::Info);
    assert_eq!(got.rule_id, None, "no rule fired, so there is no RuleId");
    assert!(!got.highlight);
}

#[test]
fn rule_id_points_at_the_rule_that_fired() {
    let mut c = build(CARGO_LIKE);

    let got = classify(&mut c, "warning: unused import: `std::time::Duration`");
    let id = got.rule_id.expect("a rule fired");
    assert_eq!(c.rule(id).expect("id resolves").id, "cargo.warning");

    let got = classify(&mut c, "error[E0433]: failed to resolve");
    let id = got.rule_id.expect("a rule fired");
    assert_eq!(c.rule(id).expect("id resolves").id, "cargo.error");
}

#[test]
fn rule_ids_are_indices_into_the_rule_set() {
    let mut c = build(CARGO_LIKE);
    for (index, rule) in c.rules().iter().enumerate() {
        assert_eq!(
            rule.index,
            u16::try_from(index).expect("small"),
            "index must match position"
        );
    }

    // Resolve by walking RuleIds rather than by trusting the one we got back.
    let got = classify(&mut c, "note: `#[warn(unused_variables)]` on by default");
    let id = got.rule_id.expect("a rule fired");
    assert_eq!(id.0 as usize, 2);
    assert_eq!(c.rule(id).expect("id resolves").id, "cargo.note");
}

#[test]
fn out_of_range_rule_ids_resolve_to_none() {
    let c = build(CARGO_LIKE);
    assert!(c.rule(hushlog_core::RuleId(9999)).is_none());
}

#[test]
fn the_first_matching_rule_wins() {
    // `boom` is matched by every rule below. Declaration order decides, not
    // specificity: a pack author who needs a later rule to fire has to order
    // the pack, and the ordering is visible in the file.
    let src = r#"
[[rule]]
id = "first"
match = { kind = "literal", pattern = "boom" }
level = "error"
highlight = true

[[rule]]
id = "second"
match = { kind = "regex", pattern = "^boom" }
level = "warning"

[[rule]]
id = "third"
match = { kind = "literal", pattern = "boom boom" }
level = "info"
"#;
    let mut c = build(src);
    let got = classify(&mut c, "boom");
    let id = got.rule_id.expect("a rule fired");
    assert_eq!(c.rule(id).expect("id resolves").id, "first");
    assert_eq!(got.level, Level::Error);

    // ...and it stays the same one every time.
    for _ in 0..50 {
        let got = classify(&mut c, "boom");
        let id = got.rule_id.expect("a rule fired");
        assert_eq!(c.rule(id).expect("id resolves").id, "first");
    }
}

#[test]
fn first_match_is_stable_under_ambiguous_input() {
    // Two rules, both matching, neither obviously "more specific". Whatever the
    // pack says is what happens, and it does not drift.
    let src = r#"
[[rule]]
id = "a"
match = { kind = "literal", pattern = "retry" }
level = "info"

[[rule]]
id = "b"
match = { kind = "regex", pattern = "[Rr]etry" }
level = "warning"
"#;
    let mut c = build(src);
    // Lowercase, so that both rules really do match this line: `a` as a
    // substring, `b` as an anchored pattern. Which one wins is a decision the
    // pack makes by ordering, and it must not drift between runs.
    let line = "retry 3/10 for pod checkout-api";
    let first = classify(&mut c, line).rule_id;
    for _ in 0..100 {
        let again = classify(&mut c, line).rule_id;
        assert_eq!(
            first, again,
            "the same input must always pick the same rule"
        );
    }
    assert_eq!(
        c.rule(first.expect("a rule fired")).expect("resolves").id,
        "a"
    );
}

#[test]
fn matching_uses_the_plain_text_not_the_raw_text() {
    let mut c = build(CARGO_LIKE);
    // Same line, but the prefix is an ANSI escape. Ingest strips it into
    // `plain` before classification ever runs.
    let raw = "\u{1b}[1m\u{1b}[33mwarning\u{1b}[0m: unused variable: `ctx`";
    let line = Line::new(raw, "warning: unused variable: `ctx`", Stream::Stdout, 1);
    let got = c.classify(line);

    assert_eq!(got.level, Level::Warning);
    assert_eq!(got.line.raw, raw, "raw is handed through untouched");
}

#[test]
fn the_line_is_passed_through_verbatim() {
    let mut c = build(CARGO_LIKE);
    let text = "warning: unused variable: `ctx`";
    let got = classify(&mut c, text);
    assert_eq!(got.line.raw, text);
    assert_eq!(got.line.plain, text);
    assert_eq!(got.line.no, 1);
    assert_eq!(got.line.stream, Stream::Stdout);
}

#[test]
fn literal_patterns_are_not_treated_as_regexes() {
    let src = r#"
[[rule]]
id = "dots"
match = { kind = "literal", pattern = "a.b" }
level = "warning"
"#;
    let mut c = build(src);
    assert!(
        classify(&mut c, "a.b").rule_id.is_some(),
        "literal matches itself"
    );
    assert!(
        classify(&mut c, "axb").rule_id.is_none(),
        "literal must not match as a regex"
    );
}

#[test]
fn an_empty_rule_set_classifies_everything_as_info() {
    let mut c = RuleClassifier::new(&[]).expect("empty set is legal");
    assert_eq!(c.rules().len(), 0);
    let got = classify(&mut c, "error: this matches nothing at all");
    assert_eq!(got.level, Level::Info);
    assert_eq!(got.rule_id, None);
}

#[test]
fn later_packs_extend_earlier_ones() {
    let a = parse_pack("a.toml", CARGO_LIKE).expect("parse");
    let b = parse_pack(
        "b.toml",
        "[[rule]]\nid = \"npm.deprecated\"\nmatch = { kind = \"literal\", pattern = \"npm WARN deprecated\" }\nlevel = \"warning\"\ncollapse = true\n",
    )
    .expect("parse");

    let mut c = RuleClassifier::new(&[a, b]).expect("compile");
    assert_eq!(c.rules().len(), 4);

    let got = classify(
        &mut c,
        "npm WARN deprecated request@2.88.2: request has been deprecated",
    );
    let id = got.rule_id.expect("a rule fired");
    let rule = c.rule(id).expect("resolves");
    assert_eq!(rule.id, "npm.deprecated");
    assert!(rule.collapse);
}

#[test]
fn duplicate_ids_across_packs_are_rejected() {
    let a = parse_pack("a.toml", CARGO_LIKE).expect("parse");
    let b = parse_pack("b.toml", CARGO_LIKE).expect("parse");
    let err = RuleClassifier::new(&[a, b]).expect_err("must not build");
    assert!(err.to_string().contains("cargo.error"), "got: {err}");
}

#[test]
fn a_pack_of_several_thousand_rules_still_compiles() {
    // Guard the 200 MB/s target's precondition: everything is compiled once,
    // at load, never per line. 2000 rules is far past any real pack.
    let mut src = String::new();
    for i in 0..2000 {
        writeln!(
            src,
            "[[rule]]\nid = \"r{i}\"\nmatch = {{ kind = \"literal\", pattern = \"needle-{i} \" }}\nlevel = \"info\"\n"
        )
        .expect("write to String cannot fail");
    }
    let mut c = build(&src);
    assert_eq!(c.rules().len(), 2000);

    // Anchored, because literal patterns match as substrings and `needle-1` is
    // a substring of `needle-1999`. That is the documented behaviour (see
    // `literal_patterns_are_not_treated_as_regexes`), not something to work
    // around in the pack author.
    let got = classify(&mut c, "needle-1999 ");
    let id = got.rule_id.expect("a rule fired");
    assert_eq!(c.rule(id).expect("resolves").id, "r1999");
}

#[test]
fn classification_cost_does_not_scale_with_pack_size() {
    // The architectural claim in AGENTS.md section 2 is that classification is
    // one pass over the line "regardless of rule count", because every pattern
    // is compiled once at load. This asserts that claim directly: a one-rule
    // pack against a 200-rule pack. Move compilation onto the hot path and the
    // ratio explodes by orders of magnitude, which is what this catches.
    //
    // It deliberately does NOT assert an absolute MB/s figure. This box is
    // shared with other agents building in parallel, and a bare `RegexSet`
    // measured anywhere from 90 to 230 MB/s here across runs. A test that fails
    // on machine load is worse than no test. The 200 MB/s acceptance criterion
    // in AGENTS.md section 7 belongs to `benches/`, which WP7 owns.
    const ITERATIONS: u32 = 50_000;
    let text = "    Checking hushlog-core v0.1.0 (C:\\src\\hushlog-core)";

    let per_line = |src: &str| -> std::time::Duration {
        let mut c = build(src);
        // Warm up: the first calls pay for lazy DFA construction.
        for i in 0..1_000_u32 {
            let _ = c.classify(Line::new(text, text, Stream::Stdout, u64::from(i)));
        }
        let start = std::time::Instant::now();
        for i in 0..ITERATIONS {
            let _ = c.classify(Line::new(text, text, Stream::Stdout, u64::from(i)));
        }
        start.elapsed() / ITERATIONS
    };

    let one_rule = per_line(CARGO_LIKE);

    let mut many = String::from(CARGO_LIKE);
    for i in 0..197 {
        writeln!(
            many,
            "[[rule]]\nid = \"filler{i}\"\nmatch = {{ kind = \"literal\", pattern = \"zz-unused-{i}\" }}\nlevel = \"info\"\n"
        )
        .expect("write to String cannot fail");
    }
    let two_hundred = per_line(&many);

    assert!(
        two_hundred < one_rule * 4,
        "200 rules took {two_hundred:?} per line against {one_rule:?} for 1 rule; \
         classification is supposed to cost one pass regardless of pack size"
    );
}

const LEVEL_FIELD_PACK: &str = r#"
[[rule]]
id = "klog.line"
match = { kind = "regex", pattern = "^(?<ts>\\d{4}-\\d{2}-\\d{2}T\\S*Z)\\s+(?<level>\\w+)\\s" }
level_field = "level"
level_map = { error = "^(?:err|error|fail|critical|fatal)$", warning = "^(?:warn|warning)$" }
level = "info"
"#;

const OPID_PACK: &str = r#"
[[rule]]
id = "retry.op"
match = { kind = "regex", pattern = "Retry (?<n>\\d+)/10 for pod (?<op>[a-z0-9-]+)" }
level = "warning"
collapse = true
opid = "op"
"#;

#[test]
fn level_field_map_picks_most_severe_match() {
    let mut c = build(LEVEL_FIELD_PACK);

    let got = classify(&mut c, "2026-10-12T09:14:03Z ERROR scheduler: boom");
    assert_eq!(got.level, Level::Error);

    let got = classify(&mut c, "2026-10-12T09:14:03Z WARN scheduler: retrying");
    assert_eq!(got.level, Level::Warning);

    let got = classify(&mut c, "2026-10-12T09:14:03Z INFO scheduler: started");
    assert_eq!(got.level, Level::Info);

    // Unknown level token falls back to the rule's declared level.
    let got = classify(&mut c, "2026-10-12T09:14:03Z TRACE scheduler: tracing");
    assert_eq!(got.level, Level::Info);
}

#[test]
fn sample_line_that_does_not_match_is_rejected() {
    let src = r#"
[[rule]]
id = "bad.sample"
match = { kind = "literal", pattern = "actual" }
level = "info"
[[rule.sample]]
line = "not present"
"#;
    let err = parse_pack("bad.sample.toml", src)
        .and_then(|p| RuleClassifier::from_pack(&p))
        .expect_err("sample that does not match must fail");
    let text = err.to_string();
    assert!(text.contains("bad.sample.toml"), "got: {text}");
    assert!(
        text.contains("does not match"),
        "error should name the mismatch, got: {text}"
    );
}

#[test]
fn sample_line_with_wrong_declared_level_is_rejected() {
    let src = r#"
[[rule]]
id = "wrong.level"
match = { kind = "literal", pattern = "error: x" }
level = "info"
[[rule.sample]]
line = "error: x"
level = "warning"
"#;
    let err = parse_pack("wrong.level.toml", src)
        .and_then(|p| RuleClassifier::from_pack(&p))
        .expect_err("sample level mismatch must fail");
    let text = err.to_string();
    assert!(text.contains("derives level"), "got: {text}");
    assert!(text.contains("Warning"), "got: {text}");
}

#[test]
fn opid_is_extracted_from_named_capture() {
    let mut c = build(OPID_PACK);

    let got = classify(&mut c, "Retry 3/10 for pod checkout-api-7f9c4d8b6-x2k9m");
    assert_eq!(got.opid.as_deref(), Some("checkout-api-7f9c4d8b6-x2k9m"));
    assert_eq!(got.level, Level::Warning);

    let got = classify(&mut c, "Retry 4/10 for pod inventory-api-2c8e3f6a9-d4k7m");
    assert_eq!(got.opid.as_deref(), Some("inventory-api-2c8e3f6a9-d4k7m"));
}

#[test]
fn opid_is_none_when_rule_has_no_opid() {
    let mut c = build(CARGO_LIKE);
    let got = classify(&mut c, "warning: unused variable: `ctx`");
    assert_eq!(got.opid, None);
}

#[test]
fn invalid_opid_group_is_rejected() {
    let src = r#"
[[rule]]
id = "bad.opid"
match = { kind = "regex", pattern = "Retry \\d+/10 for pod [a-z-]+" }
level = "warning"
opid = "no_such_group"
"#;
    let err = parse_pack("bad.opid.toml", src)
        .and_then(|p| RuleClassifier::from_pack(&p))
        .expect_err("opid group not in pattern must fail");
    let text = err.to_string();
    assert!(text.contains("bad.opid"), "must name the rule, got: {text}");
    assert!(text.contains("opid"), "must mention the opid, got: {text}");
}

#[test]
fn opid_and_level_field_can_coexist() {
    let src = r#"
[[rule]]
id = "klog.op"
match = { kind = "regex", pattern = "^(?<ts>\\S+) (?<level>\\w+) op=(?<op>\\S+)" }
level = "info"
level_field = "level"
level_map = { error = "^error$", warning = "^warning$" }
opid = "op"
"#;
    let mut c = build(src);
    let got = classify(&mut c, "2026-10-12T09:14:03Z ERROR op=abc-123");
    assert_eq!(got.level, Level::Error);
    assert_eq!(got.opid.as_deref(), Some("abc-123"));
}
