//! Loader tests.
//!
//! The contract is AGENTS.md section 1: an invalid rule must fail loudly, and
//! the error must say *where*. "some error occurred" costs a user an afternoon.

use hushlog_rules::{load_pack, parse_pack};

#[test]
fn malformed_toml_reports_the_line_it_broke_on() {
    // Line 6 is `level = ` with nothing after it.
    let src = "name = \"broken\"\n\
               \n\
               [[rule]]\n\
               id = \"a\"\n\
               match = { kind = \"literal\", pattern = \"x\" }\n\
               level = \n";

    let err = parse_pack("broken.toml", src).expect_err("must not load");
    let text = err.to_string();

    assert!(
        text.contains("broken.toml"),
        "error must name the file, got: {text}"
    );
    assert!(text.contains(":6"), "error must name line 6, got: {text}");
}

#[test]
fn unknown_field_reports_the_line_of_the_field() {
    let src = "name = \"typo\"\n\
               \n\
               [[rule]]\n\
               id = \"a\"\n\
               match = { kind = \"literal\", pattern = \"x\" }\n\
               level = \"info\"\n\
               colour = \"red\"\n";

    let err = parse_pack("typo.toml", src).expect_err("must not load");
    let text = err.to_string();

    assert!(
        text.contains("typo.toml"),
        "error must name the file, got: {text}"
    );
    assert!(text.contains(":7"), "error must name line 7, got: {text}");
    assert!(
        text.contains("colour"),
        "error must name the field, got: {text}"
    );
}

#[test]
fn bad_regex_reports_the_line_of_the_pattern() {
    let src = "name = \"badrx\"\n\
               \n\
               [[rule]]\n\
               id = \"a\"\n\
               match = { kind = \"regex\", pattern = \"^unclosed(\" }\n\
               level = \"info\"\n";

    let pack = parse_pack("badrx.toml", src).expect("schema itself is valid TOML");
    // Compilation happens when the rule set is built, which is where the
    // regex error can be attributed to a rule.
    let err = hushlog_rules::RuleClassifier::from_pack(&pack).expect_err("must not compile");

    let text = err.to_string();
    assert!(
        text.contains("badrx.toml"),
        "error must name the file, got: {text}"
    );
    assert!(text.contains(":5"), "error must name line 5, got: {text}");
    assert!(
        text.contains('a'),
        "error must name the rule id, got: {text}"
    );
}

#[test]
fn error_display_always_carries_a_location() {
    let err = parse_pack("some/deep/path/rules.toml", "not = [toml\n").expect_err("must not load");
    let text = err.to_string();
    assert!(
        text.starts_with("some/deep/path/rules.toml:"),
        "every loader error starts with file:line, got: {text}"
    );
}

#[test]
fn a_missing_file_is_an_error_not_an_empty_pack() {
    let err = load_pack("definitely/not/here.toml").expect_err("must not load");
    let text = err.to_string();
    assert!(text.contains("definitely/not/here.toml"), "got: {text}");
}

#[test]
fn loading_a_real_file_round_trips() {
    let dir = temp_dir("load-real");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("ok.toml");
    std::fs::write(
        &path,
        "name = \"ok\"\n\n[[rule]]\nid = \"r\"\nmatch = { kind = \"literal\", pattern = \"x\" }\nlevel = \"info\"\n",
    )
    .expect("write");

    let pack = load_pack(&path).expect("pack should load");
    assert_eq!(pack.rules.len(), 1);
    assert_eq!(pack.rules[0].id.get_ref(), "r");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A per-process scratch directory under the system temp dir.
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("hushlog-rules-{}-{}", std::process::id(), tag));
    dir
}
