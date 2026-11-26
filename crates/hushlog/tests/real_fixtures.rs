//! The real fixtures, and what hushlog must make of them.
//!
//! Every other test in this crate proves a property against a log somebody
//! wrote by hand. These prove it against logs a tool actually produced: a
//! `cargo build` with warnings, a `cargo test` with panics, an `npm install`
//! with deprecations, and a `pip install` with resolver chatter.
//!
//! The distinction matters because the hand-written ones cannot surprise you.
//! The first version of this file was written because the panic fixture
//! surprised us: a Rust panic and a failing test were both being classified as
//! ordinary output, so the promise in `AGENTS.md` section 1 -- that an error
//! line is never lost -- did not apply to the failures that matter most.
//!
//! Matching is done against the ANSI-stripped text, the same text hushlog
//! matches rules against. The captured logs carry colour: `cargo` puts an
//! escape between `warning` and the colon that follows it, so a literal search
//! for `warning:` finds nothing.

mod common;

use hushlog_core::{Classifier, Ingest, Level, Stream};

/// A real `cargo build` with warnings.
const CARGO_WARNINGS: &str = "cargo-warnings.log";
/// A real `cargo test` with two panics and full stack traces.
const RUST_TEST_PANIC: &str = "rust-test-panic.log";
/// A real `npm install` with twelve deprecation warnings.
const NPM_INSTALL: &str = "npm-install.log";
/// A real `pip install` with resolver chatter.
const PIP_INSTALL: &str = "pip-install.log";
/// A real failing GitHub Actions run (cargo-deny advisory + audit-check).
const GITHUB_ACTIONS_FAILURE: &str = "github-actions-failure.log";
/// A real `pytest` run with two failures and one pass.
const PYTEST_FAILURE: &str = "pytest-failure.log";
/// A real `jest` run with two failures and one pass.
const JEST_FAILURE: &str = "jest-failure.log";
/// A real `vitest` run with two failures and one pass.
const VITEST_FAILURE: &str = "vitest-failure.log";
/// A real `go test` run with two failures, a panic and one pass.
const GO_TEST_FAILURE: &str = "go-test-failure.log";

/// The level hushlog gives the first line matching `needle`.
///
/// Matching is done against the ANSI-stripped text, so a needle does not have
/// to know where the colour codes are.
fn level_of(fixture: &str, needle: &str) -> Level {
    let mut classifier = hushlog_rules::builtin::classifier().expect("built-in packs compile");
    let mut ingest = Ingest::new(Stream::Stdout);
    let bytes = common::fixture_bytes(fixture);
    let mut lines = ingest.push(&bytes);
    lines.extend(ingest.finish());
    for line in lines {
        if line.plain.contains(needle) {
            return classifier.classify(line).level;
        }
    }
    panic!("the fixture {fixture} has no line containing {needle:?}");
}

/// Whether hushlog's output for a fixture still contains a line, ignoring
/// colour.
fn output_contains(fixture: &str, needle: &str) -> bool {
    let mut cmd = common::hushlog();
    cmd.arg("--no-summary");
    cmd.write_stdin(common::fixture_bytes(fixture));
    let out = cmd.output().expect("hushlog runs");
    strip_ansi(&String::from_utf8_lossy(&out.stdout)).contains(needle)
}

/// Remove ANSI colour escapes from some text.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // Skip the escape sequence: `[` then parameters, ending in a letter.
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

#[test]
fn a_rust_panic_is_an_error() {
    // The shape changed in Rust 1.83: the thread id is now in brackets. The
    // rule matches the shape, not one spelling of it.
    assert_eq!(
        level_of(RUST_TEST_PANIC, "panicked at"),
        Level::Error,
        "a panic is a failure, and must never be filtered away"
    );
}

#[test]
fn both_panics_are_errors() {
    // The fixture holds two panics, in two different tests.
    let errors = error_lines(RUST_TEST_PANIC);
    let panics = errors
        .iter()
        .filter(|line| line.contains("panicked at"))
        .count();
    assert_eq!(panics, 2, "both panics must be errors: {errors:?}");
}

#[test]
fn a_failing_test_is_an_error() {
    assert_eq!(
        level_of(
            RUST_TEST_PANIC,
            "test tests::panics_on_a_bad_input ... FAILED"
        ),
        Level::Error,
        "a failing test is a failure"
    );
    assert_eq!(
        level_of(RUST_TEST_PANIC, "test result: FAILED."),
        Level::Error,
        "the suite summary says how many failed, and it is a failure"
    );
}

#[test]
fn a_passing_test_is_not_an_error() {
    assert_eq!(
        level_of(RUST_TEST_PANIC, "test tests::parses_a_number ... ok"),
        Level::Info,
        "a passing test is ordinary output"
    );
}

#[test]
fn a_panic_survives_filtering() {
    assert!(
        output_contains(RUST_TEST_PANIC, "panicked at"),
        "the panic must be in the output, not folded into a summary"
    );
    assert!(
        output_contains(RUST_TEST_PANIC, "test result: FAILED."),
        "the failing-suite summary must be in the output"
    );
}

#[test]
fn a_failing_github_actions_run_survives_filtering() {
    // Both shapes were real errors in a real CI run. They survive filtering
    // *and* classify as errors now, so --explain and JSON agree with the text.
    assert!(
        output_contains(GITHUB_ACTIONS_FAILURE, "error[unmaintained]"),
        "the cargo-deny error line from a real CI run must not be dropped"
    );
    assert!(
        output_contains(
            GITHUB_ACTIONS_FAILURE,
            "Resource not accessible by integration"
        ),
        "the audit-check error line from a real CI run must not be dropped"
    );
}

#[test]
fn github_actions_errors_are_classified_as_errors() {
    assert_eq!(
        level_of(GITHUB_ACTIONS_FAILURE, "error[unmaintained]"),
        Level::Error,
        "a cargo-deny advisory is a failure even with a CI prefix"
    );
    assert_eq!(
        level_of(
            GITHUB_ACTIONS_FAILURE,
            "##[error]Resource not accessible by integration"
        ),
        Level::Error,
        "a GitHub Actions error annotation is a step failure"
    );
    assert_eq!(
        level_of(GITHUB_ACTIONS_FAILURE, "##[warning]3 warnings found!"),
        Level::Warning,
        "a warning annotation is not an error"
    );
}

#[test]
fn pytest_failures_are_classified_and_survive_filtering() {
    assert_eq!(
        level_of(PYTEST_FAILURE, "FAILED test_sample.py::test_failure"),
        Level::Error,
        "the short summary names each failed test"
    );
    assert_eq!(
        level_of(PYTEST_FAILURE, "E       assert (1 + 1) == 3"),
        Level::Error,
        "the traceback detail carries the reason for the failure"
    );
    assert_eq!(
        level_of(PYTEST_FAILURE, "2 failed, 1 passed"),
        Level::Error,
        "the suite summary is a failure"
    );
    assert!(
        output_contains(PYTEST_FAILURE, "assert (1 + 1) == 3"),
        "pytest's reason must be in the output"
    );
}

#[test]
fn vitest_failures_are_classified_and_survive_filtering() {
    assert_eq!(
        level_of(VITEST_FAILURE, "FAIL  sample.test.js > a real vitest run"),
        Level::Error
    );
    assert_eq!(
        level_of(VITEST_FAILURE, "AssertionError: expected 2 to be 3"),
        Level::Error
    );
    assert_eq!(
        level_of(VITEST_FAILURE, "Test Files  1 failed"),
        Level::Error
    );
    assert!(
        output_contains(VITEST_FAILURE, "expected 2 to be 3"),
        "vitest's reason must be in the output"
    );
}

#[test]
fn jest_failures_are_classified_and_survive_filtering() {
    assert_eq!(
        level_of(JEST_FAILURE, "FAIL ./sample.test.js"),
        Level::Error
    );
    assert_eq!(
        level_of(JEST_FAILURE, "Test Suites: 1 failed"),
        Level::Error
    );
    assert_eq!(
        level_of(JEST_FAILURE, "Tests:       2 failed"),
        Level::Error
    );
    assert!(
        output_contains(JEST_FAILURE, "boom from a real jest run"),
        "jest's reason must be in the output"
    );
}

#[test]
fn go_test_failures_are_classified_and_survive_filtering() {
    assert_eq!(
        level_of(GO_TEST_FAILURE, "--- FAIL: TestFail"),
        Level::Error,
        "go names each failed test before the reason"
    );
    assert_eq!(
        level_of(GO_TEST_FAILURE, "sample_test.go:12: expected 2 to be 3"),
        Level::Error,
        "the failure reason must keep its level"
    );
    assert_eq!(
        level_of(GO_TEST_FAILURE, "panic: boom from a real go test run"),
        Level::Error,
        "a panic is a failure even with the recovered/repanicked suffix"
    );
    assert_eq!(
        level_of(GO_TEST_FAILURE, "FAIL\texample.com/hushlogcapture"),
        Level::Error,
        "the package result is a failure"
    );
    assert!(
        output_contains(GO_TEST_FAILURE, "expected 2 to be 3"),
        "go's reason must be in the output"
    );
    assert!(
        output_contains(GO_TEST_FAILURE, "boom from a real go test run"),
        "go's panic must be in the output"
    );
}

#[test]
fn a_cargo_warning_is_a_warning() {
    assert_eq!(
        level_of(CARGO_WARNINGS, "use of deprecated function"),
        Level::Warning,
        "a rustc warning is a warning, not an error and not noise"
    );
}

#[test]
fn an_npm_deprecation_is_a_warning() {
    assert_eq!(
        level_of(NPM_INSTALL, "npm warn deprecated inflight@1.0.6"),
        Level::Warning,
        "a deprecation notice is a warning"
    );
}

#[test]
fn the_real_fixtures_all_load() {
    // A fixture that is empty, or that hushlog cannot read, would make every
    // other test in this file vacuous.
    for name in [
        CARGO_WARNINGS,
        RUST_TEST_PANIC,
        NPM_INSTALL,
        PIP_INSTALL,
        GITHUB_ACTIONS_FAILURE,
        PYTEST_FAILURE,
        JEST_FAILURE,
        VITEST_FAILURE,
        GO_TEST_FAILURE,
    ] {
        let bytes = common::fixture_bytes(name);
        assert!(
            bytes.len() > 100,
            "the fixture {name} is suspiciously small: {} bytes",
            bytes.len()
        );
    }
}

/// Every line the classifier calls an error, in the fixture.
fn error_lines(fixture: &str) -> Vec<String> {
    let mut classifier = hushlog_rules::builtin::classifier().expect("built-in packs compile");
    let mut ingest = Ingest::new(Stream::Stdout);
    let bytes = common::fixture_bytes(fixture);
    let mut lines = ingest.push(&bytes);
    lines.extend(ingest.finish());
    lines
        .into_iter()
        .filter(|line| classifier.classify(line.clone()).level == Level::Error)
        .map(|line| line.plain)
        .collect()
}
