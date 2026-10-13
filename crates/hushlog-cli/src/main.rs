//! The `hushlog` binary. **Owner: WP6.**
//!
//! All I/O lives here: argument parsing, pipe and wrap modes, PTY handling,
//! `--tee`, `--explain`, and exit-code passthrough. The stages themselves are
//! `hushlog-core`; the rules are `hushlog-rules`; the five output formats are
//! `hushlog-render`. See `AGENTS.md` section 4.
//!
//! # Module map
//!
//! | Module | What it owns |
//! |---|---|
//! | [`args`] | the flag surface, and the validation that rejects combinations which do not mean anything |
//! | [`pipeline`] | the five stages in order, and the one rule that `--tee` is fed upstream of stage 1 |
//! | [`tee`] | byte-exact capture of the input stream |
//! | [`pipe`] | stdin to filtered stdout |
//! | [`wrap`] | spawn, read both streams, propagate the exit code |
//! | [`pty`] | the same, on a pseudo-terminal |
//! | [`explain`] | `--explain` annotations, on stderr so stdout stays clean |
//! | [`sink`] | the stdout handle the renderer owns, and the one we flush |
//!
//! # Two promises the binary itself has to keep
//!
//! **The exit code of a wrapped command comes back unchanged.** Not "non-zero if
//! it failed" -- unchanged. `128 + signal` when the child was killed, because
//! `ExitStatus::code()` returns `None` for that and treating it as success is
//! the one mistake this tool must not make.
//!
//! **A failure that would break the lossless guarantee stops the run.** A tee
//! file that cannot be created is a hard error, not a warning, because
//! `--tee` is one of the four ways a hidden line stays reachable
//! (`AGENTS.md` section 1).

mod args;
mod explain;
mod pipe;
mod pipeline;
mod pty;
mod sink;
mod tee;
mod wrap;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser as _;

use crate::args::{Cli, Mode};

/// Exit code for a failure that is hushlog's own, not the child's.
const EXIT_FAILURE: u8 = 1;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => match u8::try_from(code) {
            Ok(code) => ExitCode::from(code),
            // A child that exits above 255, or a wrapped `128 + signal`, still
            // has to reach the caller. `ExitCode` only carries a byte; the raw
            // code is handed to `std::process::exit` instead.
            Err(_) => std::process::exit(code),
        },
        // stdout closed underneath us (`hushlog ... | head`). The run stopped on
        // purpose, the tee is already on disk, and there is nobody left to read
        // a diagnostic. Exiting quietly is what every filter in a pipeline does.
        Err(err) if is_closed_stdout(&err) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("hushlog: {err:#}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

/// Turn the parsed arguments into a run.
fn run(cli: Cli) -> Result<i32> {
    let config = cli.config()?;
    match config.mode {
        Mode::Pipe => pipe::run(&config),
        Mode::Wrap => {
            let mut command = config.command_line()?;
            wrap::run(&config, &mut command)
        }
        Mode::Pty => {
            let Some(program) = config.command.first() else {
                anyhow::bail!("--pty needs a command to run: hushlog --pty -- <cmd>");
            };
            pty::run(&config, program, &config.command[1..])
        }
    }
}

/// Whether an error is really "the reader went away".
///
/// A closed stdout is reported by the operating system as `EPIPE` on Unix and
/// as a broken-pipe error on Windows, and `Ingest`, the renderer and the tee can
/// each be the one that notices. Walking the whole `anyhow` chain means the
/// answer does not depend on which layer happened to see it first.
fn is_closed_stdout(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_broken_pipe_anywhere_in_the_chain_is_recognised() {
        let err = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            .context("writing the tee log");
        assert!(is_closed_stdout(&err));

        let err =
            anyhow::Error::new(std::io::Error::other("disk full")).context("writing the tee log");
        assert!(
            !is_closed_stdout(&err),
            "a real failure must not be swallowed"
        );
    }

    #[test]
    fn a_broken_pipe_buried_under_context_is_still_recognised() {
        let inner = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
        let err = anyhow::Error::new(inner)
            .context("writing stdout")
            .context("finishing the pipeline");
        assert!(is_closed_stdout(&err));
    }
}
