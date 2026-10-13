//! Shared harness for the `hushlog` integration tests.
//!
//! Every test here drives the real binary as a child process, because that is
//! the only way to assert the two things this crate exists to guarantee: what
//! comes out of stdout, and what comes out of the process's exit code.
//!
//! The wrapped command is *this test binary*, re-entering itself with a filter
//! that selects one helper test. That is deliberate. Wrap mode has to spawn a
//! real process with real pipes on both ends, and a shell would mean `cmd /c`
//! quoting rules on Windows and `/bin/sh` on Unix -- two different sources of
//! flakiness for no extra coverage. The helper reads its instructions from the
//! environment, which `std::process::Command` hands to the child, which in turn
//! hands them to whatever `hushlog` spawns.

#![allow(dead_code)]

use std::io::Write as _;
use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// Marker a child writes to stdout, so a test can prove it was captured there.
pub const CHILD_STDOUT: &str = "hushlog-child-stdout-marker";

/// Marker a child writes to stderr, so a test can prove it was captured there.
pub const CHILD_STDERR: &str = "hushlog-child-stderr-marker";

/// What the helper child should do. Inherited through hushlog by the process
/// environment, so no quoting is involved anywhere.
const CHILD_MODE: &str = "HUSHLOG_TEST_CHILD_MODE";
const CHILD_EXIT: &str = "HUSHLOG_TEST_CHILD_EXIT";

/// The mode that prints nothing at all.
const MODE_SILENT: &str = "silent";
/// One line on each stream.
const MODE_BOTH: &str = "both-streams";
/// The same text on each stream, to prove they stay distinct.
const MODE_SAME_TEXT: &str = "same-text";
/// A flood of one repeated line plus a unique error.
const MODE_FLOOD: &str = "flood";

/// A `hushlog` invocation with the environment pinned so results are the same
/// on a developer's machine and in CI.
#[must_use]
pub fn hushlog() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hushlog"));
    // Colour would otherwise depend on the machine running the test.
    cmd.env("NO_COLOR", "1");
    cmd
}

/// A `hushlog` invocation in pipe mode, wrapping the helper child.
#[must_use]
pub fn wrap(flags: &[&str], mode: &str, exit: i32) -> Command {
    let exe = std::env::current_exe().unwrap_or_else(|e| panic!("this test's own path: {e}"));
    let mut cmd = hushlog();
    cmd.args(flags);
    cmd.env(CHILD_MODE, mode);
    cmd.env(CHILD_EXIT, exit.to_string());
    cmd.arg("--")
        .arg(exe)
        .args(["--exact", "common::child_helper", "--nocapture"]);
    cmd
}

/// As [`wrap`], plus `--tee <path>`. The path has to be spliced in *before* the
/// `--`, which is why it is not just another flag in the list.
#[must_use]
pub fn wrap_with_tee(flags: &[&str], tee: &Path, mode: &str, exit: i32) -> Command {
    let tee = tee.to_string_lossy().into_owned();
    let mut all: Vec<&str> = flags.to_vec();
    all.push("--tee");
    all.push(&tee);
    wrap(&all, mode, exit)
}

/// A path into the shared fixture directory.
///
/// The fixtures are real logs, not invented strings -- `AGENTS.md` section 5.3.
#[must_use]
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// The bytes of a fixture, read verbatim.
#[must_use]
pub fn fixture_bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixture(name)).unwrap_or_else(|e| panic!("reading fixture {name}: {e}"))
}

/// Write a rule pack into `dir` and return its path.
pub fn write_pack(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, source).unwrap_or_else(|e| panic!("writing {name}: {e}"));
    path
}

/// A real process, run from inside the test binary.
///
/// Exits the whole process with the requested code, which is the only way to
/// produce a genuine 42 for the exit-code test on every platform.
#[test]
fn child_helper() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        // The harness runs this test normally too, where there is no child to
        // be and no instructions to follow. Nothing to do.
        return;
    };

    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    match mode.as_str() {
        MODE_SILENT => {}
        MODE_BOTH => {
            // Flush explicitly: `--nocapture` means these go to the real pipes
            // hushlog is reading, not into a buffer the harness holds.
            let _ = writeln!(out, "{CHILD_STDOUT}");
            let _ = out.flush();
            let _ = writeln!(err, "{CHILD_STDERR}");
            let _ = err.flush();
        }
        MODE_SAME_TEXT => {
            let _ = writeln!(out, "hushlog-same-text");
            let _ = out.flush();
            let _ = writeln!(err, "hushlog-same-text");
            let _ = err.flush();
        }
        MODE_FLOOD => {
            for _ in 0..500 {
                let _ = writeln!(out, "hushlog-repeating-line");
            }
            let _ = writeln!(out, "error: hushlog-flood-error");
            let _ = writeln!(out, "some unique information that should survive");
            let _ = out.flush();
        }
        other => panic!("unknown child mode {other}"),
    }

    let code: i32 = std::env::var(CHILD_EXIT)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    std::process::exit(code);
}
