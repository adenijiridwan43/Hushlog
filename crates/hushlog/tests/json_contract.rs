//! End-to-end check of the v1 JSON contract: the binary, not just the renderer,
//! must resolve rule names and stamp every record.

mod common;

use serde_json::Value;

#[test]
fn the_binary_emits_v1_records_with_stable_rule_names() {
    // A real `cargo build --color always` capture, so the rule name comes from
    // a rule that actually fired rather than an invented event.
    let mut cmd = common::hushlog();
    cmd.args(["--format", "json"]);
    cmd.write_stdin(common::fixture_bytes("cargo-warnings.log"));
    let out = cmd.output().expect("hushlog runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8(out.stdout).expect("JSON output is UTF-8");
    let records: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("one NDJSON object per line"))
        .collect();
    assert!(!records.is_empty(), "the fixture produced no records");

    for (index, record) in records.iter().enumerate() {
        assert_eq!(record["schema_version"], "1", "{record}");
        assert_eq!(record["sequence"], (index + 1) as u64, "{record}");
    }
    assert!(
        records
            .iter()
            .any(|record| record["type"] == "emit" && record["rule"] == "cargo.warning"),
        "the real warnings must be named by the rule that fired:\n{stdout}"
    );
    assert!(
        records.iter().any(|record| record["type"] == "summary"),
        "the run must end with an accounting record"
    );
}

#[test]
fn format_detection_keeps_rule_names_in_step_with_the_classifier() {
    // The fourth line triggers Pino detection. The classifier is reordered at
    // that point; records before and after it must each name the rule that
    // actually classified them.
    let input = b"error: something failed\n\
                   at foo.rs:1\n\
                   at bar.rs:2\n\
                   {\"level\":30,\"pid\":1,\"msg\":\"hi\"}\n\
                   error: another failure\n";
    let mut cmd = common::hushlog();
    cmd.args(["--format", "json", "--no-summary"]);
    cmd.write_stdin(input.to_vec());
    let out = cmd.output().expect("hushlog runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8(out.stdout).expect("JSON output is UTF-8");
    let errors: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON"))
        .filter(|record: &Value| record["level"] == "error")
        .collect();
    assert_eq!(
        errors.len(),
        2,
        "both errors must survive detection:\n{stdout}"
    );
    for error in errors {
        assert_eq!(error["rule"], "cargo.error", "{error}");
    }
}
