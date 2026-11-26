//! `--max-tokens` is opt-in, but once it hides a line the accounting has to
//! say exactly what it hid and where the bytes went.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

struct TeeGuard(PathBuf);

impl Drop for TeeGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn tee_path() -> (TeeGuard, String) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("hushlog-budget-{unique}.log"));
    let text = path.to_string_lossy().into_owned();
    (TeeGuard(path), text)
}

fn json_records(out: &[u8]) -> Vec<Value> {
    let stdout = std::str::from_utf8(out).expect("JSON output is UTF-8");
    stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect()
}

fn error_texts(records: &[Value]) -> BTreeSet<String> {
    records
        .iter()
        .filter(|r| r["type"] == "emit" && r["level"] == "error")
        .map(|r| r["raw"].as_str().expect("error has raw text").to_string())
        .collect()
}

fn run(fixture: &str, extra: &[&str]) -> std::process::Output {
    let mut cmd = common::hushlog();
    cmd.args(["--format", "json"]);
    cmd.args(extra);
    cmd.write_stdin(common::fixture_bytes(fixture));
    cmd.output().expect("hushlog runs")
}

#[test]
fn a_tiny_budget_never_hides_an_error_and_accounts_for_every_line() {
    let fixture = "rust-test-panic.log";
    let baseline = run(fixture, &[]);
    assert!(baseline.status.success());
    let baseline_errors = error_texts(&json_records(&baseline.stdout));
    assert!(
        !baseline_errors.is_empty(),
        "the fixture must contain errors"
    );

    let budgeted = run(fixture, &["--max-tokens", "1"]);
    assert!(
        budgeted.status.success(),
        "{}",
        String::from_utf8_lossy(&budgeted.stderr)
    );
    let records = json_records(&budgeted.stdout);
    let got_errors = error_texts(&records);
    assert_eq!(
        baseline_errors, got_errors,
        "a budget must never suppress an error"
    );

    let summary = records
        .iter()
        .find(|r| r["type"] == "summary")
        .expect("the run ends with a summary");
    let budget = records.last().expect("the run ends with a record").clone();
    assert_eq!(budget["type"], "budget_summary");
    assert_eq!(budget["schema_version"], "1");
    assert_eq!(budget["max_tokens"], 1);
    assert_eq!(budget["lines_suppressed"], summary["lines_suppressed"]);

    let suppressed = budget["lines_suppressed"].as_u64().expect("a count");
    assert!(suppressed > 0, "a one-token budget must suppress something");
    assert_eq!(
        summary["lines_in"].as_u64().expect("lines_in")
            + summary["lines_out"].as_u64().expect("lines_out")
            + summary["lines_folded"].as_u64().expect("lines_folded")
            + summary["lines_suppressed"]
                .as_u64()
                .expect("lines_suppressed"),
        summary["lines_in"].as_u64().expect("lines_in") * 2,
        "lines_in = lines_out + lines_folded + lines_suppressed"
    );
    let estimate = budget["token_estimate"].as_u64().expect("estimate");
    let tokens_suppressed = budget["tokens_suppressed"].as_u64().expect("tokens");
    assert!(estimate >= tokens_suppressed);
}

#[test]
fn the_tee_file_stays_byte_exact_when_the_budget_hides_lines() {
    let input = common::fixture_bytes("synthetic-truncated-utf8.log");
    let (guard, path) = tee_path();

    let mut cmd = common::hushlog();
    cmd.args(["--format", "json", "--max-tokens", "1", "--tee", &path]);
    cmd.write_stdin(input.clone());
    let out = cmd.output().expect("hushlog runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let written = std::fs::read(Path::new(&path)).expect("the tee file exists");
    assert_eq!(written, input, "--tee is byte exact even in a budgeted run");

    let records = json_records(&out.stdout);
    let budget = records.last().expect("a final record");
    assert_eq!(budget["type"], "budget_summary");
    assert_eq!(budget["tee_path"], path);
    drop(guard);
}

#[test]
fn max_tokens_and_raw_are_mutually_exclusive() {
    let mut cmd = common::hushlog();
    cmd.args(["--raw", "--max-tokens", "10"]);
    let out = cmd.output().expect("clap reports the conflict");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("raw"),
        "the error names the conflicting flag"
    );
}
