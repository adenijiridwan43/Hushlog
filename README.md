# hushlog

[![CI](https://github.com/adenijiridwan43/Hushlog/actions/workflows/ci.yml/badge.svg)](https://github.com/adenijiridwan43/Hushlog/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#licence)
[![crates.io](https://img.shields.io/crates/v/hushlog.svg)](https://crates.io/crates/hushlog)
[![docs.rs](https://img.shields.io/docsrs/hushlog)](https://docs.rs/hushlog)
[![rust](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)
[![release](https://img.shields.io/github/v/release/adenijiridwan43/Hushlog?include_prereleases&label=release)](https://github.com/adenijiridwan43/Hushlog/releases)

Smart noise filter for terminal and CI logs. Sits in a pipe (`cmd | hushlog`)
or wraps a command (`hushlog -- cargo build`), highlights what actually failed,
and collapses repeated warning blocks into counted summaries.

**Lossless by default.** The full log is always preserved, every fold is
expandable, and error lines are never collapsed without an explicit flag.
See [AGENTS.md](https://github.com/adenijiridwan43/Hushlog/blob/main/AGENTS.md#1-the-non-negotiable-principle-lossless-by-default).

## Install

`v0.1.0-alpha.1` is the first release. Install with any of:

```sh
# Linux / macOS
curl -LsSf https://github.com/adenijiridwan43/Hushlog/releases/latest/download/hushlog-installer.sh | sh

# Windows (PowerShell)
irm https://github.com/adenijiridwan43/Hushlog/releases/latest/download/hushlog-installer.ps1 | iex

# crates.io
cargo install hushlog

# Homebrew
brew install adenijiridwan43/tap/hushlog

# Container
docker run --rm -i ghcr.io/adenijiridwan43/hushlog
```

Or build from source:

```sh
git clone https://github.com/adenijiridwan43/Hushlog
cd Hushlog
cargo build --release
./target/release/hushlog --help
```

This is an alpha: the rule packs are still being proven against real logs, and
the command-line surface may change before 1.0. The release pipeline builds
static binaries for five targets (Linux x86_64/aarch64 musl, macOS
x86_64/aarch64, Windows x86_64) via `.github/workflows/release.yml` on a `v*`
tag; `main` runs a dry-run check on every push so the configuration is verified
before a tag exists.

See [docs/DISTRIBUTION.md](https://github.com/adenijiridwan43/Hushlog/blob/main/docs/DISTRIBUTION.md)
for the full channel list, the one-time secret setup, and the release checklist.

## Use

```sh
# Pipe mode: filter stdin, write stdout
cargo build 2>&1 | hushlog

# Wrap mode: filter the child's output and propagate its exit code
hushlog -- cargo build

# Keep the full log on disk while showing the filtered version
hushlog --tee build.log -- cargo build

# No filtering at all, straight through
hushlog --raw -- cargo build

# Show why each line was folded or kept (goes to stderr)
hushlog --explain --format plain -- cargo build

# Native CI fold markers
hushlog --format github -- cargo test

# Machine-readable, one JSON record per line
hushlog --format json -- npm test

# Query matching lines only, as CSV or JSON
hushlog --query 'level == error && plain =~ "retry"' -- cargo build
hushlog --query 'level == warning' --format csv -- cargo build
```

`--pty` is opt-in: it gives the child a real terminal so progress bars and
colours work. Under `--pty` the tee file records what the terminal would have
shown rather than the raw bytes.

### Flags

| Flag | Default | What it does |
|---|---|---|
| `--tee <FILE>` | off | Write the verbatim input log, byte-exact. |
| `--raw` | off | Pass through, no filtering. |
| `--explain` | off | Annotate every fold decision on stderr. |
| `--format <fmt>` | `plain` | `tty`, `plain`, `github`, `gitlab`, `json`, `csv`. |
| `--query <EXPR>` | off | Emit only lines matching the predicate. |
| `--rules <FILE>` | built-ins | Extra TOML pack. Repeatable. |
| `--collapse-errors` | **off** | Opt in to folding errors. |
| `--max-tracked <N>` | `10000` | LRU capacity, the memory bound. |
| `--no-summary` | off | Suppress the end-of-run summary. |
| `--pty` | off | Run the child on a pseudo-terminal. |

## Architecture

Four crates, one streaming pipeline, bounded memory:

| Crate | Stage | Notes |
|---|---|---|
| `hushlog-core` | normalize, classify, collapse | **Pure.** No I/O, no clock, no env. |
| `hushlog-rules` | rule schema and loader | TOML packs, one `RegexSet` |
| `hushlog-render` | tty, plain, github, gitlab, json, csv | |
| `hushlog` | args, pipe/wrap, PTY, signals | All I/O lives here |

Stages communicate only through the traits in
[`hushlog-core`](https://github.com/adenijiridwan43/Hushlog/blob/main/crates/hushlog-core/src/lib.rs).
See [AGENTS.md](https://github.com/adenijiridwan43/Hushlog/blob/main/AGENTS.md)
for the full contracts and the module ownership map.

## Contributing

Read [AGENTS.md](https://github.com/adenijiridwan43/Hushlog/blob/main/AGENTS.md)
first — it is the contract. The fixtures in `tests/fixtures/` are the real
specification; see the
[README there](https://github.com/adenijiridwan43/Hushlog/blob/main/tests/fixtures/README.md)
for what still needs collecting, and
[docs/CAPTURING-LOGS.md](https://github.com/adenijiridwan43/Hushlog/blob/main/docs/CAPTURING-LOGS.md)
for how to turn a real noisy process into a fixture.

Releasing is documented in
[docs/DISTRIBUTION.md](https://github.com/adenijiridwan43/Hushlog/blob/main/docs/DISTRIBUTION.md):
the channel list, the one-time secret setup, and the release checklist.

AI agents wrote much of this code; every change was reviewed before landing,
and the workspace gate (`cargo clippy -- -D warnings`, `cargo test`) must stay
green on every push. Design decisions — lossless-by-default, the contracts,
and the measured performance targets — are human calls recorded in
`AGENTS.md`.

## Licence

MIT OR Apache-2.0
