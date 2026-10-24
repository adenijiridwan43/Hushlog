# hushlog

Smart noise filter for terminal and CI logs. Sits in a pipe (`cmd | hushlog`)
or wraps a command (`hushlog -- cargo build`), highlights what actually failed,
and collapses repeated warning blocks into counted summaries.

**Lossless by default.** The full log is always preserved, every fold is
expandable, and error lines are never collapsed without an explicit flag.
See [AGENTS.md](AGENTS.md#1-the-non-negotiable-principle-lossless-by-default).

## Install

No release yet. Build from source:

```sh
git clone https://github.com/adenijiridwan43/Hushlog
cd Hushlog
cargo build --release
./target/release/hushlog --help
```

A release pipeline builds static binaries for five targets via
`.github/workflows/release.yml` on a `v*` tag; `main` runs a dry-run check on
every push so the configuration is verified before a tag exists.

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
[`hushlog-core`](crates/hushlog-core/src/lib.rs). See
[`AGENTS.md`](AGENTS.md) for the full contracts and the module ownership map.

## Contributing

Read [`AGENTS.md`](AGENTS.md) first — it is the contract. The fixtures in
`tests/fixtures/` are the real specification; see the
[README there](tests/fixtures/README.md) for what still needs collecting.

AI agents wrote much of this code; every change was reviewed before landing,
and the workspace gate (`cargo clippy -- -D warnings`, `cargo test`) must stay
green on every push. Design decisions — lossless-by-default, the contracts,
and the measured performance targets — are human calls recorded in
`AGENTS.md`.

## Licence

MIT OR Apache-2.0
