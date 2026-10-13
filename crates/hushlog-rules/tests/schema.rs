//! Schema tests: what a rule pack is allowed to contain.
//!
//! The rule for this file is AGENTS.md section 1: a pack that does not mean
//! what it says is worse than no pack at all, so every deviation is an error.

use hushlog_rules::{RuleLevel, parse_pack};

const VALID_PACK: &str = r#"
name = "example"
description = "A pack that is entirely made up."

[[rule]]
id = "example.error"
match = { kind = "regex", pattern = "^error(?:\\[E\\d+\\])?: " }
level = "Error"
highlight = true

[[rule]]
id = "example.warn"
match = { kind = "literal", pattern = "npm WARN deprecated" }
level = "warning"
collapse = true
context_lines = 3

[rule.block]
start = { kind = "regex", pattern = "^warning: " }
end = { kind = "literal", pattern = "" }
"#;

#[test]
fn valid_pack_loads() {
    let pack = parse_pack("example.toml", VALID_PACK).expect("pack should load");

    assert_eq!(pack.name.as_deref(), Some("example"));
    assert_eq!(pack.rules.len(), 2);

    let first = &pack.rules[0];
    assert_eq!(first.id.get_ref(), "example.error");
    assert_eq!(first.level, RuleLevel::Error);
    assert!(first.highlight);
    assert!(!first.collapse);
    assert_eq!(first.context_lines, None);

    let second = &pack.rules[1];
    assert_eq!(second.level, RuleLevel::Warning);
    assert!(!second.highlight);
    assert!(second.collapse);
    assert_eq!(second.context_lines, Some(3));
    assert!(second.block.as_ref().expect("block").end.is_some());
}

#[test]
fn rule_defaults_are_conservative() {
    // Everything optional defaults to the quietest possible value: nothing is
    // highlighted, nothing is collapsed. A pack has to opt in to being loud.
    let pack = parse_pack("m.toml", MINIMAL_PACK).expect("pack should load");
    let rule = &pack.rules[0];

    assert!(!rule.highlight);
    assert!(!rule.collapse);
    assert_eq!(rule.context_lines, None);
    assert!(rule.block.is_none());
}

const MINIMAL_PACK: &str = r#"
[[rule]]
id = "only"
match = { kind = "regex", pattern = "^x" }
level = "info"
"#;

#[test]
fn unknown_field_in_a_rule_is_rejected() {
    let err = parse_pack("bad.toml", UNKNOWN_FIELD_PACK).expect_err("must not load");

    // Loudly, not silently skipped: AGENTS.md section 1.
    let text = err.to_string();
    assert!(
        text.contains("severity"),
        "error should name the offending field, got: {text}"
    );
    assert!(
        text.contains("bad.toml"),
        "error should name the file, got: {text}"
    );
}

const UNKNOWN_FIELD_PACK: &str = r#"
[[rule]]
id = "typo"
match = { kind = "literal", pattern = "x" }
level = "info"
severity = "warning"
"#;

#[test]
fn unknown_field_in_a_matcher_is_rejected() {
    let err = parse_pack("bad.toml", UNKNOWN_MATCHER_FIELD_PACK).expect_err("must not load");
    assert!(err.to_string().contains("pattrn"), "got: {err}");
}

const UNKNOWN_MATCHER_FIELD_PACK: &str = r#"
[[rule]]
id = "typo"
match = { kind = "literal", pattrn = "x" }
level = "info"
"#;

#[test]
fn bad_level_is_rejected() {
    let err = parse_pack("bad.toml", BAD_LEVEL_PACK).expect_err("must not load");

    let text = err.to_string();
    assert!(
        text.contains("Fatal"),
        "error should quote the bad value, got: {text}"
    );
    for expected in ["error", "warning", "info", "progress", "trace"] {
        assert!(
            text.contains(expected),
            "error should list `{expected}`, got: {text}"
        );
    }
}

const BAD_LEVEL_PACK: &str = r#"
[[rule]]
id = "typo"
match = { kind = "literal", pattern = "x" }
level = "Fatal"
"#;

#[test]
fn unknown_matcher_kind_is_rejected() {
    let err = parse_pack("bad.toml", UNKNOWN_MATCHER_KIND_PACK).expect_err("must not load");
    let text = err.to_string();
    assert!(text.contains("regex"), "got: {text}");
    assert!(text.contains("literal"), "got: {text}");
}

const UNKNOWN_MATCHER_KIND_PACK: &str = r#"
[[rule]]
id = "typo"
match = { kind = "globb", pattern = "x" }
level = "info"
"#;

#[test]
fn a_bare_string_is_not_a_valid_matcher() {
    // The matcher is a tagged enum on purpose: `match = "x"` is far too easy to
    // write by accident and silently means "substring" to half the people who
    // write it and "regex" to the other half.
    let err = parse_pack("bad.toml", BARE_STRING_PACK).expect_err("must not load");
    assert!(err.to_string().contains("regex"), "got: {err}");
}

const BARE_STRING_PACK: &str = r#"
[[rule]]
id = "typo"
match = "npm WARN"
level = "info"
"#;

#[test]
fn level_is_case_insensitive_but_not_guessing() {
    for spelling in ["error", "Error", "ERROR", "eRrOr"] {
        let src = format!(
            "[[rule]]\nid = \"x\"\nmatch = {{ kind = \"literal\", pattern = \"y\" }}\nlevel = \"{spelling}\"\n"
        );
        let pack = parse_pack("case.toml", &src).expect("case should not matter");
        assert_eq!(pack.rules[0].level, RuleLevel::Error);
    }
}

#[test]
fn pack_name_defaults_to_the_file_stem() {
    let pack = parse_pack("some/dir/my-rules.toml", MINIMAL_PACK).expect("pack should load");
    assert_eq!(pack.name.as_deref(), Some("my-rules"));
}

#[test]
fn explicit_name_beats_the_file_stem() {
    let src = format!("name = \"chosen\"\n{MINIMAL_PACK}");
    let pack = parse_pack("my-rules.toml", &src).expect("pack should load");
    assert_eq!(pack.name.as_deref(), Some("chosen"));
}

#[test]
fn a_pack_without_rules_is_rejected() {
    // An empty pack means a typo, and a typo that silently disables every rule
    // would be invisible in exactly the situation hushlog exists to handle.
    let err = parse_pack("empty.toml", "name = \"empty\"\n").expect_err("must not load");
    assert!(err.to_string().contains("rule"), "got: {err}");
}

#[test]
fn blank_rule_id_is_rejected() {
    let src = "[[rule]]\nid = \"  \"\nmatch = { kind = \"literal\", pattern = \"y\" }\nlevel = \"info\"\n";
    let err = parse_pack("bad.toml", src).expect_err("must not load");
    assert!(err.to_string().contains("id"), "got: {err}");
}

#[test]
fn rule_id_with_spaces_is_rejected() {
    let src = "[[rule]]\nid = \"my rule\"\nmatch = { kind = \"literal\", pattern = \"y\" }\nlevel = \"info\"\n";
    let err = parse_pack("bad.toml", src).expect_err("must not load");
    assert!(err.to_string().contains("whitespace"), "got: {err}");
}

#[test]
fn duplicate_rule_ids_are_rejected() {
    let src = r#"
[[rule]]
id = "twice"
match = { kind = "literal", pattern = "a" }
level = "info"

[[rule]]
id = "twice"
match = { kind = "literal", pattern = "b" }
level = "info"
"#;
    let err = parse_pack("dupes.toml", src).expect_err("must not load");
    let text = err.to_string();
    assert!(text.contains("twice"), "got: {text}");
    assert!(text.contains("dupes.toml"), "got: {text}");
}

#[test]
fn collapsing_errors_is_refused() {
    // AGENTS.md section 1.1. A rule pack is not a user opt-in, so a pack that
    // asks for errors to be folded is a mistake we refuse at load time rather
    // than a behaviour we honour later.
    let src = r#"
[[rule]]
id = "danger"
match = { kind = "literal", pattern = "a" }
level = "error"
collapse = true
"#;
    let err = parse_pack("danger.toml", src).expect_err("must not load");
    let text = err.to_string();
    assert!(text.contains("collapse"), "got: {text}");
    assert!(text.contains("error"), "got: {text}");
}
