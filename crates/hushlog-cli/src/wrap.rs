//! Wrap mode: spawn a child, filter what it says, hand back its exit code. **Owner: WP6.**
//!
//! Three things have to be right here, and none of them are the filtering.
//!
//! * **The exit code comes back exactly as the child left it.** A build that
//!   failed with 42 exits 42. A build that was killed by a signal exits
//!   `128 + signal`, which is the shell convention, because `ExitStatus::code`
//!   returns `None` for a signalled death and `unwrap_or(0)` there would turn
//!   a crash into a success.
//! * **The child's stderr is not the child's stdout.** They are read on
//!   separate threads, tagged [`Stream::Stderr`] and [`Stream::Stdout`], and
//!   framed by separate `Ingest`s, so line numbers stay per-stream and a line
//!   without a trailing newline on one stream is never completed by a newline
//!   from the other. The fingerprint includes the stream, so the same text on
//!   both streams is two groups, not one -- `hushlog-core` relies on that.
//! * **A signal reaching hushlog reaches the child.** See the `forwarding`
//!   note below, and `AGENTS.md` section 8.

use std::io::{self, Read, Write as _};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;

use anyhow::{Context as _, Result};
use crossbeam_channel::{Receiver, Sender, unbounded};

use hushlog_core::Stream;

use crate::args::Config;
use crate::explain::Explainer;
use crate::pipe::{READ_CHUNK, count_newlines, report_raw_summary};
use crate::pipeline::{Origin, Pipeline};
use crate::sink::SharedOut;
use crate::tee::Tee;

/// A batch of bytes read from one of the child's streams.
type Batch = (Origin, Vec<u8>);

/// Spawn `command`, filter its output, and propagate its exit code.
///
/// # Errors
///
/// Returns an error only for hushlog's own failures -- a rule pack that will not
/// compile, a tee file that cannot be written. The child's own failure is not
/// an error: it is the exit code this function returns.
pub(crate) fn run(config: &Config, command: &mut Command) -> Result<i32> {
    let mut tee = Tee::open(config.tee.as_deref()).context("opening the tee file")?;

    // Configure the child before anything is spawned, so a bad argument fails
    // loudly instead of after a build has already started.
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        // `io::Error` from `spawn` says "program not found" and nothing else,
        // which is useless when hushlog was run inside a script. Name it.
        .with_context(|| {
            format!(
                "spawning the wrapped command `{}`",
                command.get_program().to_string_lossy()
            )
        })?;

    // hushlog is a pipe in someone else's command line. If anything below
    // fails, the child must not be left running: it would keep writing to
    // pipes nobody is reading, with no way to stop it but hunting the process
    // down. The successful path reaps it itself.
    let status = match drive(config, &mut child, &mut tee) {
        Ok(status) => status,
        Err(err) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
    };

    tee.finish().context("flushing the tee log")?;
    Ok(exit_code(status))
}

/// Read both streams to EOF, filter them, and reap the child.
fn drive(config: &Config, child: &mut Child, tee: &mut Tee) -> Result<ExitStatus> {
    let stdout = child.stdout.take().context("child stdout was not piped")?;
    let stderr = child.stderr.take().context("child stderr was not piped")?;

    let (tx, rx) = unbounded::<Batch>();
    let readers = [
        spawn_reader(Origin::ChildOut, stdout, tx.clone()),
        spawn_reader(Origin::ChildErr, stderr, tx.clone()),
    ];
    // The original sender must go, or `rx` never sees a disconnect.
    drop(tx);

    let out = SharedOut::new();
    let explain = config.explain.then(Explainer::new);

    let result = if config.raw {
        // `--raw` is a pass-through, so the pass-through is what the child's two
        // streams get: each byte to the stream it came from, unchanged. That is
        // the one mode where hushlog does *not* merge them, because "no filtering
        // at all" includes no re-routing.
        raw_pump(&rx, tee, config.summary)
    } else {
        let mut pipeline = Pipeline::new(config, out, explain, tee.path())?;
        let pumped = pump(&rx, &mut pipeline, tee);
        pumped.and_then(|()| pipeline.finish().context("finishing the pipeline"))
    };
    result?;

    for reader in readers {
        // The readers exit on their own once both pipes hit EOF; a panic in one
        // is not worth failing the run over.
        let _ = reader.join();
    }

    child.wait().context("waiting for the wrapped command")
}

/// `--raw` in wrap mode: every byte to the stream it came from, unchanged.
fn raw_pump(rx: &Receiver<Batch>, tee: &mut Tee, summary: bool) -> Result<()> {
    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();
    let mut bytes = 0u64;
    let mut lines = 0u64;

    while let Ok((origin, chunk)) = rx.recv() {
        lines += count_newlines(&chunk);
        bytes += chunk.len() as u64;
        tee.write(&chunk).context("writing the tee log")?;
        // Deliberately not `Ingest`: a byte-exact copy must not be rebuilt from
        // framed `String`s. See `AGENTS.md` section 1.0.
        match origin {
            Origin::ChildOut => stdout.write_all(&chunk),
            Origin::ChildErr => stderr.write_all(&chunk),
        }
        .context("writing the child's output")?;
    }
    stdout.flush().context("flushing stdout")?;
    stderr.flush().context("flushing stderr")?;

    report_raw_summary(summary, lines, bytes, tee.path());
    Ok(())
}

/// Drain both reader threads into the pipeline.
fn pump(rx: &Receiver<Batch>, pipeline: &mut Pipeline, tee: &mut Tee) -> Result<()> {
    while let Ok((origin, bytes)) = rx.recv() {
        let stream: Stream = origin.into();
        // The tee write happens here, upstream of `Ingest`, on the bytes as the
        // reader thread received them. Each stream's bytes land contiguously;
        // the two may interleave, which is what a merged terminal view is.
        tee.write(&bytes).context("writing the tee log")?;
        pipeline.push(&bytes, stream)?;
    }
    pipeline.finish().context("finishing the pipeline")?;
    Ok(())
}

/// Read one stream to EOF on its own thread, handing chunks to the pipeline.
fn spawn_reader<R>(origin: Origin, mut source: R, tx: Sender<Batch>) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match source.read(&mut buf) {
                // A clean end of file, or a closed pipe, or a read that the
                // kernel will not repeat: all three mean this stream is done.
                // The bytes read so far are already in the tee file and in the
                // pipeline, so there is nothing to report.
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // A send failure means the pipeline is gone and nobody will
                    // ever read this stream again; stopping is the correct
                    // response, not an error.
                    if tx.send((origin, buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

/// The code hushlog exits with, given the child's status.
///
/// `ExitStatus::code()` is `None` on Unix when the child died from a signal.
/// Collapsing that to zero would report a segfault as a successful build, so
/// the shell convention `128 + signal` is used instead: `SIGKILL` (9) becomes
/// 137, `SIGTERM` (15) becomes 143, and a CI job that watches exit codes sees
/// what it expects. There is no `ExitStatus::signal()` on Windows and no
/// equivalent number to recover, so a Windows child that dies without an exit
/// code is reported as a plain failure rather than dressed up as one.
///
/// # Signal forwarding
///
/// hushlog installs no signal handlers, and that is deliberate rather than a
/// gap. A handler is the only thing that would let hushlog *forward* a signal,
/// and installing one needs `unsafe`, which the workspace forbids
/// (`AGENTS.md` section 6).
///
/// It does not need one. `std::process::Command` puts the child in hushlog's own
/// process group, so a signal the *terminal* sends to the foreground process
/// group -- Ctrl-C, and `kill` with no argument -- reaches the child as well as
/// hushlog, with no code from us. That covers the case a user actually hits.
///
/// What is **not** covered, on both platforms: a signal sent to hushlog's pid
/// alone (`kill -TERM <hushlog-pid>`, or `Stop-Process` on Windows) stops
/// hushlog and leaves the child running. `AGENTS.md` section 8 asks for this to
/// be documented honestly rather than pretended; this is that documentation.
/// Where the tee file is concerned the failure mode is benign: chunks are
/// written unbuffered, so whatever arrived is already on disk.
fn exit_code(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;

        // A child killed by a signal has no exit code, and reporting that as
        // 0 would tell a build pipeline a segfault was a success.
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
    }
    #[cfg(not(unix))]
    {
        // Windows has no signal number to recover, so `code()` is `None` when
        // the child was terminated rather than exiting on its own. Reporting
        // that as 1 is defensible (it is a failure) but it is not the truth:
        // the child may have been killed by a console close or a job object,
        // and a wrapper that turns "terminated" into "exited 1" is lying in a
        // way that is hard to debug from the outside.
        //
        // Windows gives no portable exit code for that case. 128 is the shell
        // convention for "died to a signal we cannot name", and it keeps the
        // value distinguishable from a genuine 1 -- which matters, because
        // `hushlog -- cmd` is often chained with `&&` and `||`.
        status.code().unwrap_or(128)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_reader_drains_its_stream_and_hands_over_every_byte() {
        let (tx, rx) = unbounded::<Batch>();
        let payload: Vec<u8> = (0u8..=255).cycle().take(10_000).collect();
        let handle = spawn_reader(Origin::ChildOut, Cursor::new(payload.clone()), tx.clone());
        drop(tx);

        let mut got = Vec::new();
        while let Ok((origin, bytes)) = rx.recv() {
            assert_eq!(origin, Origin::ChildOut);
            got.extend_from_slice(&bytes);
        }
        handle.join().expect("reader thread");
        assert_eq!(got, payload, "the reader must not drop or reorder a byte");
    }

    #[test]
    fn a_reader_with_no_readers_left_exits_promptly() {
        let (tx, rx) = unbounded::<Batch>();
        drop(rx);
        let handle = spawn_reader(Origin::ChildErr, Cursor::new(vec![b'x'; 100_000]), tx);
        // The thread must notice the dead channel rather than filling memory
        // with chunks nobody will read.
        handle.join().expect("reader thread");
    }

    #[test]
    fn exit_code_reports_a_clean_exit_verbatim() {
        // `cmd /c exit 0` and `true` are the two ways to ask the operating
        // system for a successful exit on each platform.
        let (program, args): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/c", "exit", "0"])
        } else {
            ("true", &[])
        };
        if let Ok(status) = std::process::Command::new(program).args(args).status() {
            assert_eq!(exit_code(status), 0);
        }
    }
}
