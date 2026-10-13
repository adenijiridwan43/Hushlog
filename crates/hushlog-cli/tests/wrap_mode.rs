//! Wrap mode: the child's output is filtered, and its exit code is not.

mod common;

use common::{CHILD_STDERR, CHILD_STDOUT};

/// Run `hushlog <flags> -- <child>` and hand back the finished process.
fn wrap(flags: &[&str], mode: &str, exit: i32) -> std::process::Output {
    common::wrap(flags, mode, exit)
        .output()
        .expect("hushlog runs")
}

#[test]
fn a_clean_child_exits_zero() {
    let out = wrap(&["--no-summary"], "silent", 0);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

#[test]
fn a_failing_child_exits_one() {
    let out = wrap(&["--no-summary"], "silent", 1);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
}

#[test]
fn an_unusual_exit_code_comes_back_unchanged() {
    // The whole point of wrap mode. A build that fails with 42 must exit 42,
    // not 1 and certainly not 0.
    for code in [2, 42, 99, 127, 255] {
        let out = wrap(&["--no-summary"], "silent", code);
        assert_eq!(out.status.code(), Some(code), "exit {code} did not survive");
    }
}

#[test]
fn the_childs_stdout_and_stderr_are_captured_separately() {
    // Not "both end up somewhere" -- separately, so that a line-numbering or
    // fingerprint bug that merged the streams would show up here.
    let out = wrap(&["--no-summary"], "both-streams", 0);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains(CHILD_STDOUT),
        "the child's stdout must reach hushlog's output:\n{stdout}"
    );
    assert!(
        stdout.contains(CHILD_STDERR),
        "the child's stderr must reach hushlog's output too:\n{stdout}"
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "the markers are the child's own, not hushlog's error output"
    );
}

#[test]
fn identical_text_on_both_streams_stays_two_lines() {
    // `hushlog-core` fingerprints on the stream as well as the text, precisely
    // so that a line appearing on both is two events. This pins the behaviour
    // the CLI is responsible for preserving.
    let out = wrap(&["--no-summary"], "same-text", 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout
            .lines()
            .filter(|l| l.contains("hushlog-same-text"))
            .count(),
        2,
        "one line on each stream must stay two lines:\n{stdout}"
    );
}

#[test]
fn the_tee_file_holds_the_childs_output() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let tee = dir.path().join("child.log");

    let out = common::wrap_with_tee(&["--no-summary"], &tee, "both-streams", 0)
        .output()
        .expect("hushlog runs");
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    let teed = std::fs::read_to_string(&tee).expect("the tee file exists");
    assert!(
        teed.contains(CHILD_STDOUT),
        "the tee file must hold the child's stdout:\n{teed}"
    );
    assert!(
        teed.contains(CHILD_STDERR),
        "the tee file must hold the child's stderr:\n{teed}"
    );
}

#[test]
fn raw_preserves_both_child_streams_separately() {
    // `--raw` means "as if hushlog were not here". That includes which stream
    // each byte went to, which the merged filtered output does not do.
    let out = wrap(&["--raw", "--no-summary"], "both-streams", 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        stdout.contains(CHILD_STDOUT) && !stdout.contains(CHILD_STDERR),
        "stdout must carry only the child's stdout:\n{stdout}"
    );
    assert!(
        stderr.contains(CHILD_STDERR),
        "stderr must carry the child's stderr:\n{stderr}"
    );
}

#[test]
fn a_child_that_cannot_be_spawned_fails_loudly_without_inventing_an_exit_code() {
    let mut cmd = common::hushlog();
    cmd.args(["--no-summary"]);
    cmd.arg("--").arg("hushlog-no-such-program-9d2f");
    let out = cmd.output().expect("hushlog runs");

    assert!(!out.status.success(), "a missing program must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("hushlog-no-such-program-9d2f"),
        "the program must be named:\n{stderr}"
    );
}

#[test]
fn a_failing_child_still_produces_its_output_and_its_summary() {
    let out = wrap(&[], "both-streams", 3);
    assert_eq!(out.status.code(), Some(3));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(CHILD_STDOUT),
        "a failed run must still show what the child said:\n{stdout}"
    );
    assert!(
        stdout.contains("lines ->"),
        "a failed run still gets its accounting:\n{stdout}"
    );
}

#[test]
fn explain_works_in_wrap_mode_too() {
    let out = wrap(&["--explain", "--no-summary"], "both-streams", 0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[hushlog]"),
        "wrap mode must annotate as well:\n{stderr}"
    );
}

#[test]
fn a_child_command_with_arguments_receives_them_verbatim() {
    // `hushlog -- cargo build --release` must not let `--release` be eaten by
    // hushlog's own parser.
    let mut cmd = common::hushlog();
    cmd.args(["--no-summary"]);
    cmd.arg("--")
        .arg("hushlog-no-such-program-9d2f")
        .args(["--release", "-j", "8"]);
    let out = cmd.output().expect("hushlog runs");
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The spawn fails, but only because the program is missing -- if `--release`
    // had been parsed as a hushlog flag, the message would be about that instead.
    assert!(!stderr.contains("unexpected argument"), "{stderr}");
    assert!(stderr.contains("hushlog-no-such-program-9d2f"), "{stderr}");
}

#[test]
fn a_child_writing_a_lot_of_output_is_filtered_completely() {
    let out = wrap(&["--no-summary"], "flood", 0);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let shown = stdout
        .lines()
        .filter(|l| l.trim() == "hushlog-repeating-line")
        .count();
    assert!(
        shown < 500,
        "500 identical lines should not all be reprinted:\n{stdout}"
    );
    assert!(
        stdout.contains("similar lines folded"),
        "the collapse must be reported:\n{stdout}"
    );
    assert!(
        stdout.contains("error: hushlog-flood-error"),
        "and the unique error must survive the flood:\n{stdout}"
    );
    assert!(
        stdout.contains("some unique information that should survive"),
        "and so must unique non-error output:\n{stdout}"
    );
}
