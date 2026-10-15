//! `AGENTS.md` section 1.0: `--tee` and `--raw` are byte-exact, or they are
//! not features.
//!
//! These are the tests that protect the promise the whole product rests on. The
//! input is a fixture containing bytes that `String::from_utf8_lossy` cannot
//! represent: a truncated three-byte character, a lone `0xff`, a `NUL`, a
//! character cut in half at end of file. Every one of those becomes `U+FFFD`
//! the moment a `Line` exists.
//!
//! A tee file rebuilt from `Line`s still *looks* like a log. It is the same
//! size, it has the same lines in the same order, and the corruption is
//! invisible in a diff unless you know to look for it. That is what makes these
//! tests necessary rather than tidy.

mod common;

use common::fixture_bytes;

/// `synthetic-truncated-utf8.log`: a job killed mid-write. See
/// `tests/fixtures/README.md`.
const TRUNCATED: &str = "synthetic-truncated-utf8.log";

/// An ordinary, entirely valid log, for the control case.
const RETRY_STORM: &str = "synthetic-retry-storm.log";

/// The bytes that must never appear in a tee file or in `--raw` output, because
/// the input does not contain them and lossy decoding would have invented them.
const LOSSY_MARKER: &[u8] = "�".as_bytes();

fn assert_no_lossy_substitution(bytes: &[u8], what: &str) {
    assert!(
        !bytes.windows(LOSSY_MARKER.len()).any(|w| w == LOSSY_MARKER),
        "{what} contains U+FFFD, which the input never had: the bytes went \
         through String::from_utf8_lossy somewhere"
    );
}

#[test]
fn the_truncated_fixture_really_does_contain_bytes_that_cannot_be_decoded() {
    let bytes = fixture_bytes(TRUNCATED);
    assert!(
        std::str::from_utf8(&bytes).is_err(),
        "this fixture is only useful if it is not valid UTF-8"
    );
    assert_no_lossy_substitution(&bytes, "the fixture");
}

#[test]
fn tee_is_byte_identical_to_the_input_including_invalid_utf8() {
    let input = fixture_bytes(TRUNCATED);
    let dir = tempfile::tempdir().expect("a temp dir");
    let tee = dir.path().join("full.log");

    let mut cmd = common::hushlog();
    cmd.args(["--tee", tee.to_str().expect("utf-8 path")]);
    cmd.write_stdin(input.clone());

    let out = cmd.output().expect("hushlog runs");
    assert!(out.status.success(), "hushlog failed: {out:?}");

    let written = std::fs::read(&tee).expect("the tee file exists");
    assert_eq!(
        written.len(),
        input.len(),
        "the tee file lost bytes: {} in, {} out",
        input.len(),
        written.len()
    );
    assert!(
        written == input,
        "the tee file is not byte-identical to the input (AGENTS.md section 1.0)"
    );
    assert_no_lossy_substitution(&written, "the tee file");
}

#[test]
fn raw_output_is_byte_identical_to_the_input_including_invalid_utf8() {
    let input = fixture_bytes(TRUNCATED);

    let mut cmd = common::hushlog();
    cmd.args(["--raw", "--no-summary"]);
    cmd.write_stdin(input.clone());

    let out = cmd.output().expect("hushlog runs");
    assert!(out.status.success(), "hushlog failed: {out:?}");
    assert!(
        out.stdout == input,
        "--raw is not byte-identical: {} in, {} out",
        input.len(),
        out.stdout.len()
    );
    assert_no_lossy_substitution(&out.stdout, "--raw stdout");
}

#[test]
fn raw_preserves_a_log_with_no_trailing_newline() {
    // The last line of the truncated fixture ends mid-character with no `\n`.
    // A pipeline that normalises line endings or adds a terminator would fail
    // here and nowhere else.
    let input = fixture_bytes(TRUNCATED);
    assert_ne!(
        input.last(),
        Some(&b'\n'),
        "the fixture should end unterminated"
    );

    let mut cmd = common::hushlog();
    cmd.args(["--raw", "--no-summary"]);
    cmd.write_stdin(input.clone());
    let out = cmd.output().expect("hushlog runs");
    assert_eq!(out.stdout, input);
}

#[test]
fn tee_is_byte_identical_across_a_read_boundary() {
    // The pipeline reads in 64 KiB chunks. An input much larger than that, with
    // a multi-byte character straddling the boundary, is where "write the chunk,
    // then frame the chunk" could quietly go wrong -- for instance if the tee
    // were fed the *framed* lines rather than the raw chunk.
    const CHUNK: usize = 64 * 1024;
    let mut input = fixture_bytes(TRUNCATED);
    input.extend(std::iter::repeat_n(b'a', CHUNK - 1 - input.len()));
    // A four-byte character placed so that a read boundary falling at 64 KiB --
    // which is the chunk size the CLI reads at -- splits it down the middle.
    input.extend_from_slice(&[0xf0, 0x9f, 0x92, b'a', 0x0a]);
    assert_eq!(input[CHUNK - 1], 0xf0, "last byte of the first chunk");
    assert_eq!(input[CHUNK], 0x9f, "first byte of the second chunk");

    let dir = tempfile::tempdir().expect("a temp dir");
    let tee = dir.path().join("full.log");

    let mut cmd = common::hushlog();
    cmd.args(["--tee", tee.to_str().expect("utf-8 path")]);
    cmd.write_stdin(input.clone());
    let out = cmd.output().expect("hushlog runs");
    assert!(out.status.success(), "{out:?}");

    let written = std::fs::read(&tee).expect("the tee file exists");
    assert!(written == input, "the tee file changed at a read boundary");
    assert_no_lossy_substitution(&written, "the tee file");
}

#[test]
fn tee_and_raw_agree_with_each_other() {
    let input = fixture_bytes(TRUNCATED);
    let dir = tempfile::tempdir().expect("a temp dir");
    let tee = dir.path().join("full.log");

    let mut raw_cmd = common::hushlog();
    raw_cmd.args(["--raw", "--no-summary"]);
    raw_cmd.write_stdin(input.clone());
    let raw = raw_cmd.output().expect("hushlog runs").stdout;

    let mut tee_cmd = common::hushlog();
    tee_cmd.args(["--tee", tee.to_str().expect("utf-8 path"), "--no-summary"]);
    tee_cmd.write_stdin(input.clone());
    tee_cmd.output().expect("hushlog runs");
    let teed = std::fs::read(&tee).expect("the tee file exists");

    assert_eq!(raw, teed, "--raw and --tee must record the same bytes");
}

#[test]
fn tee_keeps_every_byte_of_an_ordinary_log_too() {
    let input = fixture_bytes(RETRY_STORM);
    let dir = tempfile::tempdir().expect("a temp dir");
    let tee = dir.path().join("full.log");

    let mut cmd = common::hushlog();
    cmd.args(["--tee", tee.to_str().expect("utf-8 path"), "--no-summary"]);
    cmd.write_stdin(input.clone());
    cmd.output().expect("hushlog runs");

    let written = std::fs::read(&tee).expect("the tee file exists");
    assert!(written == input, "a valid log must survive --tee unchanged");
}

#[test]
fn the_summary_names_the_tee_file_so_a_filtered_line_stays_reachable() {
    // `AGENTS.md` section 1: if a line was not shown, the user has to be told
    // where to find it. The summary is how.
    let input = fixture_bytes(RETRY_STORM);
    let dir = tempfile::tempdir().expect("a temp dir");
    let tee = dir.path().join("full.log");

    let mut cmd = common::hushlog();
    cmd.args(["--tee", tee.to_str().expect("utf-8 path")]);
    cmd.write_stdin(input.clone());
    let out = cmd.output().expect("hushlog runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("full log:"),
        "the summary must point at the tee file: {stdout}"
    );
    assert!(
        stdout.contains("lines folded"),
        "the summary must account for what it folded: {stdout}"
    );
    assert!(tee.exists(), "the file the summary names must exist");
}

#[test]
fn a_tee_file_that_cannot_be_created_fails_the_run_loudly() {
    // `--tee` is one of the four recovery paths. A run that could not honour it
    // must not look like one that did.
    let dir = tempfile::tempdir().expect("a temp dir");
    let blocker = dir.path().join("not-a-directory");
    std::fs::write(&blocker, b"x").expect("seed the blocker");
    let impossible = blocker.join("full.log");

    let mut cmd = common::hushlog();
    cmd.args(["--tee", impossible.to_str().expect("utf-8 path")]);
    cmd.write_stdin(b"error: boom\n");
    let out = cmd.output().expect("hushlog runs");

    assert!(!out.status.success(), "an unwritable tee must fail the run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("tee"),
        "the error must say what failed: {stderr}"
    );
}

#[test]
fn filtering_replaces_the_undecodable_bytes_on_screen_without_touching_the_tee() {
    // The display path and the recovery path are allowed to differ -- that is
    // the entire point of section 1.0. Display may show U+FFFD; the tee file
    // may not.
    let input = fixture_bytes(TRUNCATED);
    let dir = tempfile::tempdir().expect("a temp dir");
    let tee = dir.path().join("full.log");

    let mut cmd = common::hushlog();
    cmd.args(["--tee", tee.to_str().expect("utf-8 path"), "--no-summary"]);
    cmd.write_stdin(input.clone());
    let out = cmd.output().expect("hushlog runs");

    // The error line survived, even though the log around it is damaged.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("error: the job was killed before it finished"),
        "an error line was lost from a damaged log: {stdout}"
    );
    assert!(
        std::fs::read(&tee).expect("the tee file exists") == input,
        "the tee file must not inherit the display path's lossy conversion"
    );
}
