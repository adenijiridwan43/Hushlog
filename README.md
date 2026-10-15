# hushlog

> **Status: pre-alpha.** The pipeline works and all 254 tests pass on Linux,
> macOS and Windows. Two things are not done yet, and both matter: the rule
> packs have only ever seen **synthetic** logs, and there is no release yet.
> Treat the folding rules as unproven against real output.

Smart noise filter for terminal and CI logs. Sits in a pipe (`cmd | hushlog`)
or wraps a command (`hushlog -- cargo build`), highlights what actually
failed, and collapses repeated warning blocks into counted summaries.

**Lossless by default.** Full log always preserved, every fold expandable, and
error lines are never collapsed. See [the invariant](AGENTS.md#1-the-non-negotiable-principle-lossless-by-default).

## Why this exists

CI output is mostly repetition. The same deprecation warning, printed once per
package. The same retry, once per attempt with a different pod name. The signal
is the four lines at the bottom, buried under thousands of lines nobody reads.

Existing tools split into two camps and neither fits:

- **Highlighters** ([tailspin](https://crates.io/crates/tailspin),
  [lnav](https://lnav.org/)) colour a log beautifully. They do not make it
  shorter, and they do not survive a pipe.
- **Agent token savers** (RTK, logslim) compress output for LLM context. They
  optimise for tokens, not for a human reading a failed build.

Hushlog targets the human first: block-aware folding, native CI fold markers,
and `--explain` to show exactly why any given line was folded or kept.

## Install

**No release yet.** Build from source:

```sh
git clone https://github.com/adenijiridwan43/Hushlog
cd Hushlog
cargo build --release
./target/release/hushlog --help
```

A release pipeline that builds static binaries for five targets is in
`.github/workflows/release.yml` and runs on a `v*` tag. The `main` branch runs
its dry-run check on every push, so the configuration is verified before a tag
exists.

## Use

```sh
# Pipe mode: filter stdin, write stdout
cargo build 2>&1 | hushlog

# Wrap mode: filter the child's output and propagate its exit code
hushlog -- cargo build

# Keep the full log on disk while still showing the filtered version
hushlog --tee build.log -- cargo build

# No filtering at all, straight through
hushlog --raw -- cargo build

# Show why each line was folded or kept (goes to stderr, so it composes
# with --format json)
hushlog --explain --format plain -- cargo build

# Native CI fold markers
hushlog --format github -- cargo test

# Machine-readable, for agents
hushlog --format json -- npm test
```

`--pty` is available and opt-in: it gives the child a real terminal so
progress bars and colours work. It is off by default, and under `--pty` the tee
file records what the terminal would have shown rather than the raw bytes.

### Flags

| Flag | Default | What it does |
|---|---|---|
| `--tee <FILE>` | off | Write the verbatim input log. Byte-exact. |
| `--raw` | off | Pass through, no filtering. |
| `--explain` | off | Annotate every fold decision, on stderr. |
| `--format <fmt>` | `plain` | `tty`, `plain`, `github`, `gitlab`, `json`. |
| `--rules <FILE>` | built-ins | Extra TOML pack. Repeatable. |
| `--collapse-errors` | **off** | Opt in to folding errors. See [§1.1](AGENTS.md#11-errors-are-never-collapsed). |
| `--max-tracked <N>` | `10000` | LRU capacity, the memory bound. |
| `--no-summary` | off | Suppress the end-of-run summary. |
| `--pty` | off | Run the child on a pseudo-terminal. |

## Architecture

Four crates, one streaming pipeline, bounded memory:

| Crate | Stage | Notes |
|---|---|---|
| `hushlog-core` | normalize, classify, collapse | **Pure.** No I/O, no clock, no env. |
| `hushlog-rules` | rule schema and loader | TOML packs, one `RegexSet` |
| `hushlog-render` | tty, plain, github, gitlab, json | |
| `hushlog-cli` | args, pipe/wrap, PTY, signals | All I/O lives here |

Stages communicate only through the traits in
[`hushlog-core`](crates/hushlog-core/src/lib.rs). See [`AGENTS.md`](AGENTS.md)
for the full contracts and the module ownership map.

## Contributing

Read [`AGENTS.md`](AGENTS.md) first — it is the contract, and it is short on
purpose. The fixtures in `tests/fixtures/` are the real specification; see the
[README there](tests/fixtures/README.md) for what still needs collecting.

## Acknowledgements

Most of this code was written with AI assistance, and it would be dishonest to
imply otherwise. Concretely:

- The pipeline crates (`hushlog-core`, `hushlog-rules`, `hushlog-render`) and the
  `hushlog` binary were written in parallel by separate Claude agents working
  against the trait contracts in [`AGENTS.md`](AGENTS.md).
- **Every change was reviewed before it landed.** Each agent's output was read,
  the workspace gate was run against the merged result, and the integration
  bugs those reviews caught are recorded in the git history — a rule bridge that
  silently ignored pattern markers, a `--tee` path that would have corrupted
  every log containing invalid UTF-8, a kubectl rule whose pattern never matched
  real kubectl output.
- Design, architecture, the lossless-by-default principle, the contracts, and
  the decision to delete the original 200 MB/s performance target in favour of
  a measured one were human decisions. See
  [`AGENTS.md` section 7](AGENTS.md#7-performance-targets) for the measurements
  that replaced it.
- The `error-never-lost` property tests were mutation-tested — flipping the
  policy default is what proves they actually catch a violation.

What the agents did *not* do: decide what the project should be, choose the
name, own the trade-offs, or sign off on the folding rules. That is the
maintainer's job and it stays that way.

## Licence

MIT OR Apache-2.0