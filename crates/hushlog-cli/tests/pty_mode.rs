//! `--pty`: opt-in, off by default, and honest about what it changes.
//!
//! `AGENTS.md` section 8 says pipes first and PTY opt-in, so these tests cover
//! two things: that pipes really are the default, and that `--pty` still
//! filters and still propagates the exit code.
//!
//! There is deliberately no assertion that a PTY tee file is byte-identical to
//! the child's output. It cannot be. A terminal is part of the stream when a
//! terminal is attached, and it translates line endings. The module docs in
//! `pty.rs` say so; claiming otherwise in a test would be the dishonest choice.

mod common;

#[test]
fn pipes_are_the_default() {
    // No `--pty`, so no pseudo-terminal is involved and the child's streams are
    // captured separately -- the thing `AGENTS.md` section 8 asks to be the
    // default.
    let out = common::wrap(&["--no-summary"], "both-streams", 0)
        .output()
        .expect("hushlog runs");

    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(common::CHILD_STDOUT) && stdout.contains(common::CHILD_STDERR),
        "both child streams must be captured by default:\n{stdout}"
    );
}

#[test]
fn pty_mode_runs_the_child_and_propagates_its_exit_code() {
    for code in [0, 1, 42] {
        let out = common::wrap(&["--pty", "--no-summary"], "silent", code)
            .output()
            .expect("hushlog runs");
        assert_eq!(
            out.status.code(),
            Some(code),
            "--pty must propagate exit {code}; got {:?}\nstderr: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn pty_mode_still_filters_and_reports() {
    let out = common::wrap(&["--pty"], "silent", 0)
        .output()
        .expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("lines ->"),
        "a pty run still gets its accounting:\n{stdout}"
    );
}

#[test]
fn pty_mode_writes_a_tee_file() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let tee = dir.path().join("pty.log");

    let mut cmd = common::wrap_with_tee(&["--pty", "--no-summary"], &tee, "both-streams", 0);
    let out = cmd.output().expect("hushlog runs");
    assert!(out.status.success(), "{out:?}");

    let teed = std::fs::read_to_string(&tee).expect("the tee file exists");
    assert!(
        teed.contains(common::CHILD_STDOUT) || teed.contains(common::CHILD_STDERR),
        "--pty must still write a verbatim log of what the terminal carried:\n{teed}"
    );
}

#[test]
fn pty_mode_needs_a_command_to_run() {
    let out = common::hushlog()
        .arg("--pty")
        .output()
        .expect("hushlog runs");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("pty"));
}
