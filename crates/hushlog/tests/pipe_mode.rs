//! Pipe mode: what hushlog shows, what it explains, and what it counts.

mod common;

use hushlog_core::{Classifier, Ingest, Level, Stream};
use hushlog_render::Format;

/// A real npm install that failed, with real errors in it.
const NPM: &str = "synthetic-npm-build.log";
/// A real-ish scheduler log: mostly repetition, two real failures.
const RETRY_STORM: &str = "synthetic-retry-storm.log";

/// The lines a fixture's classifier calls errors, computed from the same engine
/// hushlog uses rather than from a hard-coded list that could drift.
fn error_lines(fixture: &str) -> Vec<String> {
    let mut classifier = hushlog_rules::builtin::classifier().expect("built-in packs compile");
    let mut ingest = Ingest::new(Stream::Stdout);
    let bytes = common::fixture_bytes(fixture);
    let mut lines = ingest.push(&bytes);
    lines.extend(ingest.finish());
    lines
        .into_iter()
        .filter(|line| classifier.classify(line.clone()).level == Level::Error)
        .map(|line| line.raw)
        .collect()
}

/// A log with the same error repeated many times, which is what `--collapse-errors`
/// is about.
fn repeated_errors(times: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for _ in 0..times {
        bytes.extend_from_slice(b"error: connection refused\n");
    }
    bytes
}

#[test]
fn filtering_a_fixture_keeps_every_error_line() {
    let errors = error_lines(NPM);
    assert!(
        !errors.is_empty(),
        "the fixture must contain errors to be worth testing"
    );

    let mut cmd = common::hushlog();
    cmd.arg("--no-summary");
    cmd.write_stdin(common::fixture_bytes(NPM));
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    for error in &errors {
        assert!(
            stdout.contains(error.as_str()),
            "an error line was lost from the filtered output.\n  missing: {error}\n  output:\n{stdout}"
        );
    }
}

#[test]
fn filtering_actually_reduces_the_repeated_content() {
    // The plain renderer spends one short note per fold rather than dropping
    // lines outright -- streaming is worth more than brevity -- so the measure
    // that matters is how many times the *repeated text* is shown, not how many
    // lines come out.
    let input = String::from_utf8_lossy(&common::fixture_bytes(NPM)).into_owned();
    let mut cmd = common::hushlog();
    cmd.arg("--no-summary");
    cmd.write_stdin(common::fixture_bytes(NPM));
    let out = cmd.output().expect("hushlog runs");
    let shown = String::from_utf8_lossy(&out.stdout).into_owned();

    let repeated = "npm WARN deprecated request@2.88.2:";
    assert_eq!(
        input.matches(repeated).count(),
        3,
        "the fixture should repeat it"
    );
    assert_eq!(
        shown.matches(repeated).count(),
        1,
        "a repeated line should be shown once:\n{shown}"
    );
    assert!(
        shown.contains("2 similar lines folded"),
        "and the reader must be told the count:\n{shown}"
    );
}

#[test]
fn the_summary_accounts_for_every_line_that_came_in() {
    // `AGENTS.md` section 1: the counts are the safety net. `lines_in` must be
    // `lines_out + lines_folded` whatever the renderers did.
    let bytes = common::fixture_bytes(RETRY_STORM);
    let total = String::from_utf8_lossy(&bytes).lines().count();

    let mut cmd = common::hushlog();
    cmd.write_stdin(bytes);
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();

    let summary = parse_summary(&stdout);
    assert_eq!(
        summary.lines_in, total as u64,
        "every input line must be counted"
    );
    assert_eq!(
        summary.lines_in,
        summary.lines_out + summary.lines_folded,
        "nothing may be silently dropped: {stdout}"
    );
    assert_eq!(summary.errors, 3, "the fixture's errors must be counted");
    assert!(
        summary.groups > 0 && summary.groups <= summary.lines_folded,
        "there were groups to fold, and no more than the lines they took:\n{stdout}"
    );
}

/// The counts out of `hushlog: 19 lines -> 8 shown | 3 errors | 3 groups (11 lines folded)`.
///
/// Positional, because `Summary`'s own `Display` is the contract here and a
/// second parser of the same sentence would be a second thing to keep in step.
fn parse_summary(text: &str) -> Counts {
    let line = text
        .lines()
        .find(|l| l.starts_with("hushlog: ") && l.contains("lines ->"))
        .unwrap_or_else(|| panic!("no run summary in:\n{text}"));
    let words: Vec<&str> = line.split_whitespace().collect();
    let number = |at: usize| -> u64 {
        words[at]
            .trim_matches(|c: char| !c.is_ascii_digit() && c != ',')
            .replace(',', "")
            .parse()
            .unwrap_or_else(|e| panic!("{at}th word {words:?} is not a count: {e}"))
    };
    Counts {
        lines_in: number(1),
        lines_out: number(4),
        errors: number(7),
        groups: number(10),
        lines_folded: number(12),
    }
}

struct Counts {
    lines_in: u64,
    lines_out: u64,
    errors: u64,
    groups: u64,
    lines_folded: u64,
}

#[test]
fn no_summary_removes_the_summary_and_nothing_else() {
    let with = {
        let mut cmd = common::hushlog();
        cmd.write_stdin(common::fixture_bytes(RETRY_STORM));
        cmd.output().expect("hushlog runs").stdout
    };
    let without = {
        let mut cmd = common::hushlog();
        cmd.arg("--no-summary");
        cmd.write_stdin(common::fixture_bytes(RETRY_STORM));
        cmd.output().expect("hushlog runs").stdout
    };

    assert!(String::from_utf8_lossy(&with).contains("hushlog: "));
    assert!(!String::from_utf8_lossy(&without).contains("hushlog: "));
    // Every line that survived with the summary is still there without it.
    for line in String::from_utf8_lossy(&with)
        .lines()
        .filter(|l| l.starts_with("2026-"))
    {
        assert!(
            String::from_utf8_lossy(&without).contains(line),
            "removing the summary also removed a log line: {line}"
        );
    }
}

#[test]
fn repeated_errors_all_appear_by_default() {
    // `AGENTS.md` section 1.1. The default is the promise; this is the test that
    // stops it being tuned away.
    let times = 12;
    let mut cmd = common::hushlog();
    cmd.arg("--no-summary");
    cmd.write_stdin(repeated_errors(times));
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let shown = stdout
        .lines()
        .filter(|l| l.trim() == "error: connection refused")
        .count();
    assert_eq!(
        shown, times,
        "every error must be shown in full when --collapse-errors is off:\n{stdout}"
    );
}

#[test]
fn collapse_errors_folds_repeated_errors_when_asked() {
    let times = 10;
    let mut cmd = common::hushlog();
    cmd.args(["--collapse-errors", "--no-summary"]);
    cmd.write_stdin(repeated_errors(times));
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    // The renderer deliberately re-shows an error sample inside a fold, so the
    // measure is the group count the run reports, not the number of times the
    // text appears on screen.
    assert!(
        stdout.contains(&format!("{times} similar lines folded")),
        "the opt-in should collapse the repeats into one counted group:\n{stdout}"
    );
    assert!(
        !stdout.contains(&format!("{} similar lines folded", times + 1)),
        "the group count must not run past what was actually seen:\n{stdout}"
    );
}

#[test]
fn collapse_errors_still_reports_what_it_folded() {
    // Folding is only acceptable because the count survives, in the fold note
    // and again in the summary.
    let mut cmd = common::hushlog();
    cmd.args(["--collapse-errors"]);
    cmd.write_stdin(repeated_errors(4));
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("3 similar lines folded"),
        "the count of what was folded must be reported:\n{stdout}"
    );
    assert_eq!(
        parse_summary(&stdout).lines_folded,
        3,
        "and accounted for in the summary:\n{stdout}"
    );
}

#[test]
fn every_format_renders() {
    for format in Format::ALL {
        let mut cmd = common::hushlog();
        cmd.args(["--format", format.as_str(), "--no-summary"]);
        cmd.write_stdin(common::fixture_bytes(NPM));
        let out = cmd.output().expect("hushlog runs");
        let stdout = String::from_utf8_lossy(&out.stdout);

        assert!(
            out.status.success(),
            "--format {format} failed: {:?}",
            out.status
        );
        assert!(
            !stdout.is_empty(),
            "--format {format} rendered nothing at all"
        );
        assert!(
            stdout.contains("error TS6133"),
            "--format {format} lost an error line:\n{stdout}"
        );
    }
}

#[test]
fn json_format_emits_one_valid_record_per_line() {
    let mut cmd = common::hushlog();
    cmd.args(["--format", "json"]);
    cmd.write_stdin(common::fixture_bytes(NPM));
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let mut records = 0;
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "--format json emitted something that is not a JSON object: {line}"
        );
        records += 1;
    }
    assert!(records > 1, "expected one record per event, got {records}");
}

#[test]
fn github_format_emits_group_markers() {
    let mut cmd = common::hushlog();
    cmd.args(["--format", "github", "--no-summary"]);
    cmd.write_stdin(common::fixture_bytes(NPM));
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("::group::"),
        "the GitHub format should use log commands:\n{stdout}"
    );
}

#[test]
fn gitlab_format_emits_section_markers() {
    let mut cmd = common::hushlog();
    cmd.args(["--format", "gitlab", "--no-summary"]);
    cmd.write_stdin(common::fixture_bytes(NPM));
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("section_start"),
        "the GitLab format should use collapsible sections:\n{stdout}"
    );
}

#[test]
fn explain_annotates_every_decision_on_stderr_and_leaves_stdout_alone() {
    let plain = {
        let mut cmd = common::hushlog();
        cmd.arg("--no-summary");
        cmd.write_stdin(common::fixture_bytes(NPM));
        cmd.output().expect("hushlog runs").stdout
    };
    let (explained_stdout, stderr) = {
        let mut cmd = common::hushlog();
        cmd.args(["--explain", "--no-summary"]);
        cmd.write_stdin(common::fixture_bytes(NPM));
        let out = cmd.output().expect("hushlog runs");
        (
            out.stdout,
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    assert_eq!(
        plain, explained_stdout,
        "--explain is a diagnostic; it must not change the rendered output"
    );

    let lines = String::from_utf8_lossy(&common::fixture_bytes(NPM))
        .lines()
        .count();
    assert!(
        stderr.contains("[hushlog]"),
        "annotations must be marked as the tool's own:\n{stderr}"
    );
    // One annotation per line, plus the policy banner.
    let annotations = stderr
        .lines()
        .filter(|l| l.starts_with("[hushlog]"))
        .count();
    assert!(
        annotations >= lines,
        "expected an annotation for each of {lines} lines plus a banner, got {annotations}:\n{stderr}"
    );
}

#[test]
fn explain_names_the_rule_that_fired_and_why_a_line_folded() {
    let mut cmd = common::hushlog();
    cmd.args(["--explain", "--no-summary"]);
    cmd.write_stdin(common::fixture_bytes(NPM));
    let out = cmd.output().expect("hushlog runs");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stderr.contains("npm.warn.deprecated"),
        "an annotation must name the rule that classified a line:\n{stderr}"
    );
    assert!(
        stderr.contains("folded into group"),
        "an annotation must say when a line was folded:\n{stderr}"
    );
    assert!(
        stderr.contains("masked"),
        "a fold explanation must show the masked text, which is why the lines matched:\n{stderr}"
    );
    assert!(
        stderr.contains("no rule matched"),
        "an unmatched line must say so rather than being left blank:\n{stderr}"
    );
}

#[test]
fn explain_and_the_json_format_compose() {
    // `--explain` on stderr is what lets it be combined with a machine-readable
    // format without corrupting it.
    let mut cmd = common::hushlog();
    cmd.args(["--explain", "--format", "json", "--no-summary"]);
    cmd.write_stdin(common::fixture_bytes(NPM));
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "--explain leaked into the JSON stream: {line}"
        );
    }
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("[hushlog]"),
        "the annotations should still be on stderr"
    );
}

#[test]
fn an_extra_rule_pack_is_loaded_and_applied() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let pack = common::write_pack(
        dir.path(),
        "extra.toml",
        r#"
        name = "extra"
        [[rule]]
        id = "extra.marker"
        match = { kind = "literal", pattern = "FRIBIDET" }
        level = "error"
        highlight = true
        "#,
    );

    let mut cmd = common::hushlog();
    cmd.args([
        "--rules",
        pack.to_str().expect("utf-8 path"),
        "--no-summary",
    ]);
    cmd.write_stdin(b"nothing special here\nsomething FRIBIDET happened\n");
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("something FRIBIDET happened"),
        "the extra rule did not fire:\n{stdout}"
    );

    // And it is a real error, which means it is never folded away.
    let mut cmd = common::hushlog();
    cmd.args([
        "--rules",
        pack.to_str().expect("utf-8 path"),
        "--no-summary",
    ]);
    cmd.write_stdin(b"FRIBIDET\nFRIBIDET\nFRIBIDET\n");
    let out = cmd.output().expect("hushlog runs");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.trim() == "FRIBIDET")
            .count(),
        3,
        "a rule loaded with `--rules` is still bound by section 1.1"
    );
}

#[test]
fn a_broken_rule_pack_fails_loudly_with_its_file_and_line() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let pack = common::write_pack(
        dir.path(),
        "broken.toml",
        "[[rule]]\nid = \"bad.pattern\"\nmatch = { kind = \"regex\", pattern = \"([unclosed\" }\nlevel = \"error\"\n",
    );

    let mut cmd = common::hushlog();
    cmd.args(["--rules", pack.to_str().expect("utf-8 path")]);
    cmd.write_stdin(b"anything\n");
    let out = cmd.output().expect("hushlog runs");

    assert!(
        !out.status.success(),
        "a broken pack must not be loaded silently"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("bad.pattern"),
        "the rule id must be named:\n{stderr}"
    );
    assert!(
        stderr.contains("broken.toml"),
        "the file must be named:\n{stderr}"
    );
    assert!(
        stderr.contains(":3:"),
        "the line the pattern is on must be named:\n{stderr}"
    );
}

#[test]
fn a_missing_rule_pack_fails_loudly() {
    let mut cmd = common::hushlog();
    cmd.args(["--rules", "no-such-pack.toml"]);
    cmd.write_stdin(b"anything\n");
    let out = cmd.output().expect("hushlog runs");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no-such-pack.toml"),
        "the missing file must be named:\n{stderr}"
    );
}

#[test]
fn max_tracked_is_accepted_and_does_not_break_the_run() {
    // The LRU bound is a memory guarantee (`AGENTS.md` section 7). It must be
    // tunable without changing what the user sees.
    let mut cmd = common::hushlog();
    cmd.args(["--max-tracked", "4", "--no-summary"]);
    cmd.write_stdin(common::fixture_bytes(NPM));
    let out = cmd.output().expect("hushlog runs");
    assert!(out.status.success(), "a small LRU must not fail the run");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("error TS2345"),
        "errors survive even with a tiny LRU"
    );
}

#[test]
fn zero_max_tracked_disables_folding_without_failing() {
    let mut cmd = common::hushlog();
    cmd.args(["--max-tracked", "0", "--no-summary"]);
    cmd.write_stdin(b"repeated\nrepeated\nrepeated\n");
    let out = cmd.output().expect("hushlog runs");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.trim() == "repeated")
            .count(),
        3,
        "a zero-capacity LRU should show everything"
    );
}

#[test]
fn raw_and_explain_cannot_be_asked_for_at_once() {
    // `--raw` makes no decisions, so `--explain` would have nothing to say.
    let mut cmd = common::hushlog();
    cmd.args(["--raw", "--explain"]);
    cmd.write_stdin(b"x\n");
    let out = cmd.output().expect("hushlog runs");
    assert!(!out.status.success(), "the contradiction must be reported");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "clap should explain itself: {stderr}"
    );
}

#[test]
fn pty_needs_a_command() {
    let mut cmd = common::hushlog();
    cmd.arg("--pty");
    let out = cmd.output().expect("hushlog runs");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("pty"));
}

#[test]
fn an_unknown_format_lists_the_real_ones() {
    let mut cmd = common::hushlog();
    cmd.args(["--format", "rtf"]);
    let out = cmd.output().expect("hushlog runs");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    for name in ["tty", "plain", "github", "gitlab", "json"] {
        assert!(
            stderr.contains(name),
            "`{name}` should be offered: {stderr}"
        );
    }
}

#[test]
fn help_documents_the_lossless_guarantees() {
    let out = common::hushlog()
        .arg("--help")
        .output()
        .expect("hushlog runs");
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in [
        "--tee",
        "--raw",
        "--explain",
        "--format",
        "--rules",
        "--collapse-errors",
        "--max-tracked",
        "--no-summary",
        "--pty",
    ] {
        assert!(
            help.contains(flag),
            "`{flag}` must appear in --help:\n{help}"
        );
    }
}

#[test]
fn version_is_reported() {
    let out = common::hushlog()
        .arg("--version")
        .output()
        .expect("hushlog runs");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains(env!("CARGO_PKG_VERSION")));
}
