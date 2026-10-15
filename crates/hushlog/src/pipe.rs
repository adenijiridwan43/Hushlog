//! Pipe mode: stdin in, filtered stdout out. **Owner: WP6.**
//!
//! This is the simplest path through the product and the one the `--tee`
//! guarantee in `AGENTS.md` section 1.0 is easiest to get wrong here, because
//! `Ingest` will happily hand back `String`s that have already lost the bytes
//! it could not decode. So the read buffer is written to the tee file *first*
//! and handed to `Ingest` *second*, from the same slice, and the two never
//! share anything but the original `&[u8]`.

use std::io::{self, Read as _, Write as _};

use anyhow::{Context as _, Result};
use hushlog_core::{Stream, Summary};

use crate::args::Config;
use crate::explain::Explainer;
use crate::pipeline::Pipeline;
use crate::sink::SharedOut;
use crate::tee::Tee;

/// Bytes read per syscall. Large enough that a 100 MB log is a few thousand
/// syscalls rather than a few million, small enough to stay off the heap.
pub(crate) const READ_CHUNK: usize = 64 * 1024;

/// Run `cmd | hushlog`.
///
/// # Errors
///
/// Returns an error on a failed rule load, an unwritable tee file, or an I/O
/// error reading stdin or writing stdout. Nothing is swallowed: a run that
/// cannot honour its guarantees must say so rather than produce plausible
/// output.
pub(crate) fn run(config: &Config) -> Result<i32> {
    let mut tee = Tee::open(config.tee.as_deref()).context("opening the tee file")?;
    let out = SharedOut::new();

    if config.raw {
        return raw_passthrough(config, &mut tee);
    }

    let explain = config.explain.then(Explainer::new);
    let mut pipeline = Pipeline::new(config, out, explain, tee.path())?;
    let mut stdin = io::stdin().lock();
    let mut buf = vec![0u8; READ_CHUNK];

    loop {
        match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                // The tee write happens here, upstream of `Ingest`, on the
                // bytes as read. Nothing downstream can corrupt it. See
                // `AGENTS.md` section 1.0.
                tee.write(&buf[..n]).context("writing the tee log")?;
                pipeline.push(&buf[..n], Stream::Stdout)?;
            }
            // A signal interrupted the read. Falling through to the next
            // iteration is the retry.
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e).context("reading stdin"),
        }
    }

    pipeline.finish().context("finishing the pipeline")?;
    tee.finish().context("flushing the tee log")?;
    Ok(0)
}

/// `--raw`: every input byte to stdout, unchanged, and to the tee file.
///
/// "No filtering at all" has to mean byte for byte. Reconstructing the output
/// from framed `Line`s would quietly normalise a truncated multi-byte character
/// into `U+FFFD` and drop the intermediate `\r` progress frames -- both of
/// which are defensible for *display* and neither of which belongs in a
/// byte-exact copy. So the bytes go straight through and `Ingest` is never
/// involved.
///
/// # Errors
///
/// Returns an error on any I/O failure in either direction.
fn raw_passthrough(config: &Config, tee: &mut Tee) -> Result<i32> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut buf = vec![0u8; READ_CHUNK];
    let mut lines = 0u64;
    let mut bytes = 0u64;
    let mut unterminated = false;

    loop {
        match stdin.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                lines += count_newlines(&buf[..n]);
                unterminated = buf[n - 1] != b'\n';
                bytes += n as u64;
                stdout.write_all(&buf[..n]).context("writing stdout")?;
                tee.write(&buf[..n]).context("writing the tee log")?;
            }
            // A signal interrupted the read. Falling through to the next
            // iteration is the retry.
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e).context("reading stdin"),
        }
    }
    if unterminated {
        lines += 1;
    }
    stdout.flush().context("flushing stdout")?;

    if config.summary {
        report_raw_summary(config.summary, lines, bytes, tee.path());
    }
    tee.finish().context("flushing the tee log")?;
    Ok(0)
}

/// Count the newlines in a chunk.
///
/// `--raw` never builds an `Ingest`, so there is nothing else to count lines
/// with. An explicit loop rather than `iter().filter().count()`: both are
/// correct, this one has no per-byte iterator overhead, and it is the same work
/// `memchr` would do -- on 64 KiB per call, neither is the reason a log is slow.
#[must_use]
pub(crate) fn count_newlines(bytes: &[u8]) -> u64 {
    let mut lines = 0u64;
    for byte in bytes {
        if *byte == b'\n' {
            lines += 1;
        }
    }
    lines
}

/// Report a `--raw` run on stderr.
///
/// Under `--raw` every input byte is on stdout unchanged, so the summary cannot
/// go there: adding a line would break the only guarantee the mode makes. On
/// stderr it still does its job -- the user learns what was passed and where the
/// full copy is.
pub(crate) fn report_raw_summary(show: bool, lines: u64, bytes: u64, tee_path: Option<String>) {
    if !show {
        return;
    }
    let summary = Summary {
        lines_in: lines,
        lines_out: lines,
        errors: 0,
        groups: 0,
        lines_folded: 0,
        tee_path,
    };
    eprintln!("hushlog: {summary} | {bytes} bytes passed through unfiltered");
}

#[cfg(test)]
mod tests {
    use super::count_newlines;

    #[test]
    fn newlines_are_counted_whatever_the_chunk_holds() {
        assert_eq!(count_newlines(b""), 0);
        assert_eq!(count_newlines(b"no newline here"), 0);
        assert_eq!(count_newlines(b"\n"), 1);
        assert_eq!(count_newlines(b"\n\n\n"), 3);
        assert_eq!(count_newlines(b"one\ntwo\nthree\n"), 3);
        // A trailing line with no terminator is not a line here; the caller adds
        // it, because it only knows whether it was the last chunk.
        assert_eq!(count_newlines(b"one\ntwo"), 1);
        // Newlines inside a multi-byte character cannot happen -- 0x0a never
        // appears in a UTF-8 continuation byte -- but the count must not care.
        assert_eq!(count_newlines("中\n文\n".as_bytes()), 2);
    }
}
