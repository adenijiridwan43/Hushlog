//! `--pty`: run the wrapped command on a pseudo-terminal. **Owner: WP6.**
//!
//! Pipes are the default and pipes are correct. A PTY is opt-in because it
//! changes what the child *does*, not just how hushlog reads it: with a
//! terminal on the other end, tools that check `isatty` will emit colour, draw
//! progress bars, and use cursor addressing. That is exactly what the user
//! wants from `hushlog -- cargo build`, and exactly what they do not want when
//! they are capturing a log. So they have to ask.
//!
//! See `AGENTS.md` section 8: pipes first, PTY opt-in.
//!
//! # What a PTY changes, and what it costs
//!
//! * **One stream, not two.** A PTY has a single slave device that the child's
//!   stdout and stderr are both connected to. There is no way to tell which is
//!   which downstream, so every line is tagged [`Stream::Stdout`]. The JSON
//!   format will report `stdout` for lines the child wrote to stderr. That is
//!   the honest answer -- the PTY did not know either.
//! * **The terminal driver translates.** With a terminal on the far end the
//!   line discipline is live: `\n` becomes `\r\n`, and a bare `\n` typed at the
//!   child is echoed. The tee file therefore records **what the terminal would
//!   have shown**, not the bytes the child wrote. On pipes, with no tee, the tee
//!   file is byte-identical to the child's output; under `--pty` it is not, and
//!   cannot be. `AGENTS.md` section 1.0's guarantee is stated for the input
//!   byte stream, and under `--pty` the terminal is part of that stream.
//! * **The exit code is whatever the PTY reports.** On Unix a signalled child
//!   is reported through the same `128 + signal` convention as pipe mode.

use std::io::{self, Read as _, Write};
use std::thread;
use std::time::Duration;

use anyhow::{Context as _, Result};
use crossbeam_channel::{RecvTimeoutError, unbounded};
use hushlog_core::Stream;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::args::Config;
use crate::explain::Explainer;
use crate::pipe::{READ_CHUNK, count_newlines, report_raw_summary};
use crate::pipeline::Pipeline;
use crate::sink::SharedOut;
use crate::tee::Tee;

/// Rows and columns given to the pseudo-terminal.
///
/// hushlog cannot ask the real terminal how big it is without reading an escape
/// sequence out of the terminal's own output, which would put tool-generated
/// bytes into the log it is about to tee. 120x40 is the least-wrong constant:
/// wide enough that ordinary build output does not wrap, tall enough that a
/// progress display has somewhere to go.
const PTY_ROWS: u16 = 40;
const PTY_COLS: u16 = 120;

/// How often the read loop checks whether the child is still alive.
const PTY_POLL: Duration = Duration::from_millis(20);

/// How long the read loop waits for the terminal to go quiet after the child
/// has exited before deciding the run is over.
///
/// A `ConPTY` flushes what the child wrote *after* the process is gone, so
/// cutting off immediately would truncate the log. Two hundred milliseconds is
/// long enough for that in practice and short enough that nobody notices.
const PTY_DRAIN: Duration = Duration::from_millis(200);

/// Total time to keep draining once the child is gone, independent of chunking.
///
/// The quiet window can be reset by unrelated terminal chatter, so on a busy
/// runner the per-chunk window can expire between two small reads and cut the
/// run short -- which on Windows shows up as the child being terminated rather
/// than exiting, and hushlog reporting a code it never received. This bound is
/// the backstop: drain until quiet, but never past here.
const PTY_DRAIN_MAX: Duration = Duration::from_secs(2);

/// Run `command` on a PTY, filter its output, propagate its exit code.
///
/// # Errors
///
/// Returns an error if the pseudo-terminal cannot be opened or the child cannot
/// be spawned. A failure *inside* the child is the child's exit code, not an
/// error here.
pub(crate) fn run(config: &Config, program: &str, args: &[String]) -> Result<i32> {
    let mut tee = Tee::open(config.tee.as_deref()).context("opening the tee file")?;
    let raw = config.raw;

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: PTY_ROWS,
            cols: PTY_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("opening a pseudo-terminal")?;

    let mut command = CommandBuilder::new(program);
    for arg in args {
        command.arg(arg);
    }

    let mut child = pair
        .slave
        .spawn_command(command)
        .with_context(|| format!("spawning `{program}` on the pseudo-terminal"))?;
    // The child holds its own handle to the slave; ours must go so that reading
    // the master sees EOF when the child exits rather than hanging forever.
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .context("reading the pty master")?;
    let mut writer = pair
        .master
        .take_writer()
        .context("writing to the pty master")?;

    // Forward our stdin so `hushlog --pty -- npm run dev` still sees input. This
    // thread is fire-and-forget: when stdin is a terminal it will simply block
    // until the run ends, and the process is about to exit anyway.
    let stdin_thread = thread::spawn(move || forward_stdin(&mut writer));

    // Reading the master on its own thread, because on Windows the read never
    // ends: a ConPTY keeps its output pipe open long after the child has gone,
    // so there is no EOF to wait for. The child is the authority on when the run
    // is over, and the loop below is built around that instead.
    let (tx, rx) = unbounded::<Vec<u8>>();
    let reader_thread = thread::spawn(move || {
        let mut buf = vec![0u8; READ_CHUNK];
        loop {
            match reader.read(&mut buf) {
                // A pty master whose slave side has gone returns an error rather
                // than a clean end of file. Both mean the same thing here: this
                // stream is done.
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // `--raw` must not build the pipeline at all, or the rule engine would see
    // bytes it has no business seeing on their way to stdout.
    let mut pipeline = (!raw)
        .then(|| {
            Pipeline::new(
                config,
                SharedOut::new(),
                config.explain.then(Explainer::new),
                tee.path(),
            )
        })
        .transpose()?;
    let mut stdout = io::stdout().lock();
    let mut bytes = 0u64;
    let mut lines = 0u64;
    let mut quiet = Duration::ZERO;
    let mut draining = Duration::ZERO;

    loop {
        match rx.recv_timeout(PTY_POLL) {
            Ok(chunk) => {
                quiet = Duration::ZERO;
                bytes += chunk.len() as u64;
                lines += count_newlines(&chunk);
                tee.write(&chunk).context("writing the tee log")?;
                match pipeline.as_mut() {
                    // A PTY merges the child's two streams; see the module docs.
                    Some(p) => p.push(&chunk, Stream::Stdout)?,
                    None => stdout.write_all(&chunk).context("writing stdout")?,
                }
            }
            // The reader thread finished: a real EOF, which is what Unix gives.
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                if child.try_wait().context("polling the pty child")?.is_some() {
                    // The child has exited. Give whatever it wrote a moment to
                    // arrive -- the ConPTY buffer drains after the process is
                    // gone -- and then stop, or `hushlog --pty` never returns on
                    // Windows.
                    quiet += PTY_POLL;
                    draining += PTY_POLL;
                    if quiet >= PTY_DRAIN || draining >= PTY_DRAIN_MAX {
                        break;
                    }
                }
            }
        }
    }
    stdout.flush().context("flushing stdout")?;

    let status = child.wait().context("waiting for the pty child")?;
    // Both of these are parked on a blocking read that may never finish; the
    // process is exiting, so neither is joined.
    drop(stdin_thread);
    drop(reader_thread);

    if let Some(pipeline) = pipeline.as_mut() {
        pipeline.finish().context("finishing the pipeline")?;
    } else if config.summary {
        report_raw_summary(config.summary, lines, bytes, tee.path());
    }
    tee.finish().context("flushing the tee log")?;
    Ok(exit_code(&status))
}

/// Copy stdin into the pty until it runs out. Errors are expected and ignored:
/// a child that never reads its stdin will close the far end, and that is not a
/// hushlog failure.
fn forward_stdin<W: Write>(writer: &mut W) {
    let mut stdin = io::stdin().lock();
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if writer.write_all(&buf[..n]).is_err() || writer.flush().is_err() {
                    break;
                }
            }
        }
    }
}

/// The exit code, matching pipe mode's convention. See `crate::wrap`.
fn exit_code(status: &portable_pty::ExitStatus) -> i32 {
    let code = status.exit_code();
    i32::try_from(code).unwrap_or(1)
}
