//! The argument surface: clap definition, validation, and the resolved
//! [`Config`] the run actually uses. **Owner: WP6.**
//!
//! Two shapes of invocation, one flag surface:
//!
//! ```text
//! cmd | hushlog [flags]            pipe mode
//! hushlog [flags] -- cmd [args]    wrap mode
//! ```
//!
//! Everything after a bare `--` is the child command and is never parsed as a
//! hushlog flag, so `hushlog -- cargo build --release` works without quoting.
//!
//! No flag here is allowed to make hushlog silently drop something. See
//! `AGENTS.md` section 1: the recovery paths are `--tee`, `--raw`, `--explain`
//! and the end-of-run summary, and the flags below are exactly those.

use std::path::PathBuf;

use clap::{ArgAction, Parser};
use hushlog_core::CollapsePolicy;
use hushlog_render::Format;

use anyhow::{Context as _, Result, bail};

/// Which of the three run shapes the arguments asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Read stdin, filter, write stdout.
    Pipe,
    /// Spawn a child, filter its two streams, propagate its exit code.
    Wrap,
    /// As [`Mode::Wrap`], but the child runs on a pseudo-terminal.
    Pty,
}

/// Everything a run needs, after parsing and after every "this combination does
/// not make sense" check has passed.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    pub(crate) mode: Mode,
    /// Where to write the verbatim input, if anywhere.
    pub(crate) tee: Option<PathBuf>,
    /// Pass every input byte straight through, unfiltered.
    pub(crate) raw: bool,
    /// Annotate every decision on stderr.
    pub(crate) explain: bool,
    pub(crate) format: Format,
    /// Extra rule packs, in command-line order, appended after the built-ins.
    pub(crate) rules: Vec<PathBuf>,
    pub(crate) policy: CollapsePolicy,
    /// Emit the end-of-run summary.
    pub(crate) summary: bool,
    /// The child command and its arguments, for wrap and PTY mode.
    pub(crate) command: Vec<String>,
}

/// `hushlog` -- smart noise filter for terminal and CI logs.
#[derive(Debug, Clone, Parser)]
#[command(
    name = "hushlog",
    version,
    about = "Smart noise filter for terminal and CI logs",
    long_about = "\
hushlog sits in a pipe or wraps a command, and keeps what failed.

It is lossless by default: error lines are never collapsed, nothing is dropped
without being counted in the end-of-run summary, and --tee writes the complete
verbatim input to disk byte for byte -- including bytes that are not valid
UTF-8.

  cmd | hushlog [flags]           pipe mode
  hushlog [flags] -- cmd [args]   wrap mode, child's exit code propagated
",
    after_long_help = "\
Recovery paths, in the words of AGENTS.md section 1:
  --tee FILE        the full input, byte for byte, written upstream of the
                    pipeline so a log containing invalid UTF-8 survives intact
  --raw             no filtering at all; every input byte reaches stdout
                    unchanged
  --explain         every decision (kept, folded, classified as) annotated
                    on stderr, so the rendered output stays clean
  summary           exact counts and the --tee path at the end of the run
                    (--no-summary turns this off; it is then the only way left
                    to find out what was folded, so use it deliberately)

Wrap mode reads the child's stdout and stderr as two tagged streams through one
pipeline -- so their line numbers, fingerprints and multi-line blocks stay
correct -- and writes the combined filtered output to stdout. The JSON format
keeps the origin stream on every record. A child's stderr is merged rather than
mirrored, so `hushlog -- cargo build > build.log` captures the build. Under
--raw, which is a pass-through, each stream stays where the child put it.
"
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) lossless: Lossless,

    #[command(flatten)]
    pub(crate) collapsing: Collapsing,

    #[command(flatten)]
    pub(crate) terminal: Terminal,

    /// Output format
    #[arg(long, value_name = "FORMAT", default_value = "plain", value_parser = parse_format)]
    pub(crate) format: Format,

    /// Load an extra rule pack (repeatable; the built-in packs always load)
    #[arg(long = "rules", value_name = "FILE", action = ArgAction::Append)]
    pub(crate) rules: Vec<PathBuf>,

    /// Upper bound on distinct fingerprints held in memory (LRU capacity)
    #[arg(long, value_name = "N", default_value_t = CollapsePolicy::default().max_tracked)]
    pub(crate) max_tracked: usize,

    /// Command to run, after `--`
    #[arg(value_name = "CMD")]
    pub(crate) command: Vec<String>,
}

/// The three switches that keep a hidden line reachable. `AGENTS.md` section 1
/// names them, and they are grouped here for the same reason.
///
/// Split out as its own struct so the help text can head them as a group, and
/// so this one struct does not carry every boolean in the binary.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Lossless {
    /// Write the full verbatim input log to FILE, byte for byte (created or truncated)
    #[arg(long, value_name = "FILE")]
    pub(crate) tee: Option<PathBuf>,

    /// Pass every input byte through with no filtering at all
    #[arg(long, conflicts_with = "explain")]
    pub(crate) raw: bool,

    /// Annotate every filtering decision on stderr
    #[arg(long)]
    pub(crate) explain: bool,

    /// Suppress the end-of-run summary
    #[arg(long = "no-summary")]
    pub(crate) no_summary: bool,
}

/// What may be folded. `AGENTS.md` section 1.1 is the reason errors need a flag
/// here at all.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Collapsing {
    /// Allow repeated error lines to be folded (off by default; see --help)
    #[arg(long = "collapse-errors")]
    pub(crate) collapse_errors: bool,
}

/// How the wrapped command is given a terminal, if any. `AGENTS.md` section 8:
/// pipes are the default, so this has to be asked for.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Terminal {
    /// Run the wrapped command on a pseudo-terminal instead of pipes
    #[arg(long, requires = "command")]
    pub(crate) pty: bool,
}

impl Cli {
    /// Validate the combination and resolve it into a [`Config`].
    ///
    /// # Errors
    ///
    /// Returns an error when the flags contradict each other -- `--pty` with
    /// nothing to run, a wrapped command whose executable is missing -- rather
    /// than picking a silent default.
    pub(crate) fn config(self) -> Result<Config> {
        let mode = match (self.terminal.pty, self.command.is_empty()) {
            (true, true) => bail!("--pty needs a command to run: hushlog --pty -- <cmd>"),
            (true, false) => Mode::Pty,
            (false, false) => Mode::Wrap,
            (false, true) => Mode::Pipe,
        };

        if self.command.first().is_some_and(String::is_empty) {
            bail!("the wrapped command's program name is empty");
        }

        Ok(Config {
            mode,
            tee: self.lossless.tee,
            raw: self.lossless.raw,
            explain: self.lossless.explain,
            format: self.format,
            rules: self.rules,
            policy: CollapsePolicy {
                collapse_errors: self.collapsing.collapse_errors,
                max_tracked: self.max_tracked,
            },
            summary: !self.lossless.no_summary,
            command: self.command,
        })
    }
}

impl Config {
    /// Build the process to spawn for wrap mode.
    ///
    /// # Errors
    ///
    /// Returns an error in pipe mode, where there is no command to build.
    pub(crate) fn command_line(&self) -> Result<std::process::Command> {
        let Some(program) = self.command.first() else {
            bail!("no command given: hushlog filters stdin in pipe mode");
        };
        let mut command = std::process::Command::new(program);
        command.args(&self.command[1..]);
        Ok(command)
    }
}

/// Parse a `--format` value, reporting the accepted spellings on a typo.
fn parse_format(value: &str) -> std::result::Result<Format, String> {
    value.parse::<Format>().map_err(|e| e.to_string())
}

/// Load the built-in packs plus every `--rules` file, in order.
///
/// Built-ins come first so that they win when two rules match the same line:
/// first match wins in `hushlog-rules`, and an explicitly loaded pack is an
/// addition rather than a replacement.
///
/// # Errors
///
/// Fails loudly, naming the file and line, if a pack cannot be read or does
/// not compile -- see `AGENTS.md` section 1.
pub(crate) fn load_packs(extra: &[PathBuf]) -> Result<Vec<hushlog_rules::RulePack>> {
    let mut packs = hushlog_rules::builtin::packs().context("loading the built-in rule packs")?;
    for path in extra {
        let pack = hushlog_rules::load_pack(path)
            .with_context(|| format!("loading rule pack {}", path.display()))?;
        packs.push(pack);
    }
    Ok(packs)
}

/// Help text shared by `--help` and the crate docs, kept next to the flags.
#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn the_argument_surface_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn every_format_is_accepted_by_name() {
        for format in Format::ALL {
            let cli = Cli::try_parse_from(["hushlog", "--format", format.as_str()])
                .unwrap_or_else(|e| panic!("`{}` must parse: {e}", format.as_str()));
            assert_eq!(cli.format, format);
        }
    }

    #[test]
    fn an_unknown_format_lists_the_real_ones() {
        let err = Cli::try_parse_from(["hushlog", "--format", "rtf"]).expect_err("not a format");
        let message = err.to_string();
        for name in ["tty", "plain", "github", "gitlab", "json"] {
            assert!(
                message.contains(name),
                "`{name}` should be offered: {message}"
            );
        }
    }

    #[test]
    fn a_bare_invocation_is_pipe_mode() {
        let config = Cli::try_parse_from(["hushlog"])
            .expect("no flags")
            .config()
            .unwrap();
        assert_eq!(config.mode, Mode::Pipe);
        assert!(config.summary, "the summary is on unless asked otherwise");
        assert!(
            !config.policy.collapse_errors,
            "AGENTS.md 1.1: errors never fold by default"
        );
        assert_eq!(config.format, Format::Plain);
    }

    #[test]
    fn everything_after_the_double_dash_is_the_child() {
        let cli = Cli::try_parse_from([
            "hushlog",
            "--format",
            "json",
            "--",
            "cargo",
            "build",
            "--release",
        ])
        .expect("the `--` marker ends option parsing");
        assert_eq!(cli.command, ["cargo", "build", "--release"]);
        assert_eq!(cli.format, Format::Json);
    }

    #[test]
    fn child_flags_are_not_eaten_by_hushlog() {
        let cli = Cli::try_parse_from(["hushlog", "--raw", "--", "ls", "--color=never", "-la"])
            .expect("child flags must reach the child");
        assert!(cli.lossless.raw);
        assert_eq!(cli.command, ["ls", "--color=never", "-la"]);
    }

    #[test]
    fn pty_without_a_command_is_refused() {
        let err = Cli::try_parse_from(["hushlog", "--pty"])
            .expect_err("--pty needs something to run")
            .to_string();
        assert!(err.contains("pty"), "{err}");
    }

    #[test]
    fn the_default_lru_capacity_matches_the_core_default() {
        let cli = Cli::try_parse_from(["hushlog"]).unwrap().config().unwrap();
        assert_eq!(
            cli.policy.max_tracked,
            CollapsePolicy::default().max_tracked
        );
    }

    #[test]
    fn max_tracked_takes_a_number() {
        let cli = Cli::try_parse_from(["hushlog", "--max-tracked", "17"])
            .unwrap()
            .config()
            .unwrap();
        assert_eq!(cli.policy.max_tracked, 17);
    }

    #[test]
    fn rules_accumulate_in_order() {
        let cli = Cli::try_parse_from(["hushlog", "--rules", "a.toml", "--rules", "b.toml"])
            .unwrap()
            .config()
            .unwrap();
        assert_eq!(
            cli.rules,
            [PathBuf::from("a.toml"), PathBuf::from("b.toml")]
        );
    }

    #[test]
    fn wrap_mode_keeps_the_child_verbatim() {
        let cli = Cli::try_parse_from(["hushlog", "--", "cargo", "build", "--jobs", "8"])
            .unwrap()
            .config()
            .unwrap();
        assert_eq!(cli.mode, Mode::Wrap);
        assert_eq!(cli.command, ["cargo", "build", "--jobs", "8"]);
    }
}
