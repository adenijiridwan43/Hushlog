# hushlog

[![CI](https://github.com/adenijiridwan43/Hushlog/actions/workflows/ci.yml/badge.svg)](https://github.com/adenijiridwan43/Hushlog/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](#licence)
[![crates.io](https://img.shields.io/crates/v/hushlog.svg)](https://crates.io/crates/hushlog)
[![docs.rs](https://img.shields.io/docsrs/hushlog)](https://docs.rs/hushlog)
[![rust](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)
[![release](https://img.shields.io/github/v/release/adenijiridwan43/Hushlog?include_prereleases&label=release)](https://github.com/adenijiridwan43/Hushlog/releases)

**Logs are fine in small doses number. At three in the morning, a thousand near-identical warnings are not.** hushlog turns a noisy stream into something a person can read — while keeping a complete copy of everything it hides.

Put it in a pipe:

```sh
cargo build 2>&1 | hushlog
```

Or let it run the command for you:

```sh
hushlog -- cargo build
```

It highlights what failed, folds repeated lines into counted notices, and tells you exactly what it did. **Nothing is silently dropped:** the full input can always be written to a `--tee` file, byte for byte, and `--raw` passes everything through untouched.

---

## What it looks like

This is a real capture from a Node backend under a rate-limit storm: ANSI-coloured access logs, a genuine Postgres `FATAL 08P01`, and roughly four hundred repetitions of six request shapes.

```sh
hushlog < real-relanflow-ratelimit-storm.log
```

```
hushlog: 441 lines -> 41 shown | 1 errors | 2 groups (400 lines folded)
```

441 lines become 43: 41 readable lines and two fold notices. The database failure is still there, the repeats are counted instead of printed, and every one of the 400 hidden lines is recoverable from the tee file.

---

## Install

The current release is `v0.1.0-alpha.2`. This is an alpha: the tool works, the rule packs are still being proven against real logs, and the command line may change before 1.0.

Pick whichever suits your machine:

```sh
# Linux / macOS
curl -LsSf https://github.com/adenijiridwan43/Hushlog/releases/download/v0.1.0-alpha.2/hushlog-installer.sh | sh

# Windows (PowerShell)
irm https://github.com/adenijiridwan43/Hushlog/releases/download/v0.1.0-alpha.2/hushlog-installer.ps1 | iex

# crates.io
cargo install hushlog --version 0.1.0-alpha.2

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

Release binaries are built for Linux (x86_64 and aarch64, musl), macOS (x86_64 and aarch64) and Windows (x86_64).

---

## Everyday use

```sh
# Make a build readable
cargo build 2>&1 | hushlog

# Wrap a command and keep its exit code
hushlog -- cargo test

# Keep the original log on disk while showing the clean version
hushlog --tee build.log -- cargo build

# See nothing but the failures
hushlog --query 'level == error' -- npm test

# Explain every decision: what was kept, what was folded, and why
hushlog --explain --format plain -- cargo build

# Native fold markers for CI logs
hushlog --format github -- cargo test
```

By default, errors are **never** folded. If you really want them collapsed, you have to ask for it with `--collapse-errors`, and the summary will still account for every line.

`--pty` is available for programs that need a real terminal for colours or progress bars. It is opt-in; normal pipes and wrapped commands do not use it.

---

## What it understands

Twelve rule packs ship inside the binary, so they cannot go missing between release and run:

| Pack | Covers |
|---|---|
| cargo | Rust compiler warnings and errors |
| npm | Install output and deprecation notices |
| docker | BuildKit lines and multi-stage build output |
| kubectl | Kubernetes events and object output |
| syslog | Traditional syslog facility/severity lines |
| journald | systemd journal entries |
| otel | OpenTelemetry log records |
| pino | Node.js structured JSON logs |
| bunyan | Node.js structured JSON logs |
| spdlog | C++ structured log lines |
| error_log | Apache HTTP Server error logs |
| scoped | Bracketed scope-and-level prefixes, e.g. `[db:error] ...` |

Unrecognised lines are treated as ordinary output, never thrown away. You can add your own pack with `--rules <FILE>`; the format is documented with the built-in packs in the repository.

Input can also be gzip or bzip2 compressed, and hushlog detects the log format from the first lines rather than requiring you to name it.

### Redaction

`--anonymize` replaces IP addresses, UUIDs, MAC addresses, email addresses, URLs, credit-card numbers and hex dumps in the **displayed output** with consistent fake values. The same real value always maps to the same fake, so you can follow a thread through the log without exposing the original data.

The `--tee` file is written before redaction and is never anonymised. If you need a safe output for sharing and a complete copy for yourself, use both flags together.

---

## Flags

| Flag | Default | What it does |
|---|---|---|
| `--tee <FILE>` | off | Write the verbatim input log, byte-exact. Created or truncated. |
| `--raw` | off | Pass everything through, unchanged. |
| `--explain` | off | Annotate each fold decision on stderr. |
| `--format <fmt>` | `plain` | `tty`, `plain`, `github`, `gitlab`, `json`, `csv`. |
| `--query <EXPR>` | off | Emit only lines matching the predicate. |
| `--rules <FILE>` | built-ins | Add a TOML rule pack. Repeatable. |
| `--collapse-errors` | **off** | Opt in to folding errors. |
| `--max-tracked <N>` | `10000` | LRU capacity; the bound on memory use. |
| `--no-summary` | off | Suppress the end-of-run summary. |
| `--pty` | off | Run the child on a pseudo-terminal. |
| `--anonymize` | off | Redact sensitive values in the rendered output; the tee file stays untouched. |

---

## How it works

Five small stages, one line at a time:

1. **Ingest** turns bytes into lines, handles carriage-return progress frames and ANSI colour, and flushes a final partial line.
2. **Normalize** strips colour for matching, masks volatile tokens and fingerprints the line.
3. **Classify** decides the level and which rule fired.
4. **Group** detects multi-line blocks and folds repeated shapes into counted groups, with a bounded LRU.
5. **Render** writes the result for a terminal, a CI system, or a machine, plus the summary.

The core is pure Rust with no I/O, clock or environment reads, and all pipeline stages talk through a small set of contracts. That is why it is easy to test, and why the same logic can be reused outside the CLI. See [AGENTS.md](https://github.com/adenijiridwan43/Hushlog/blob/main/AGENTS.md) for the contracts and the design rules.

---

## Speed

Absolute throughput varies by machine, so the number that matters is the ratio to a bare byte scan on the same hardware. On a 64 MB CI corpus with a release build:

| Stage | MB/s |
|---|---|
| byte scan + split | ~980 |
| ingest | ~117 |
| normalise (mask + fingerprint) | ~37 |
| classify (79 rules) | ~80 |
| **the full pipeline** | **~15** |

A 100 MB build log takes a few seconds. The benchmark measures the pipeline the binary actually runs, on both a corpus of mostly unique lines and a real capture of mostly repeats. The project's floor is 1% of the byte-scan baseline. You can check it yourself with `cargo run --release --example stage_bench`.

---

## Machine-readable output

For scripts, dashboards and AI assistants, `--format json` or `--format csv` emits one compact record per event. `--query` filters by level, text, rule, stream or line number:

```sh
hushlog --query 'level == error && plain =~ "retry"' --format json -- cargo build
hushlog --query 'level == warning' --format csv -- cargo build
```

`--query` implies JSON output unless you ask for CSV. Every record carries a `type`, so a consumer can tell an emitted line, a folded group, a block or the final summary apart without guessing. The grammar and measured examples are in [docs/AGENT-INTERFACE.md](https://github.com/adenijiridwan43/Hushlog/blob/main/docs/AGENT-INTERFACE.md).

---

## Contributing

Start with [AGENTS.md](https://github.com/adenijiridwan43/Hushlog/blob/main/AGENTS.md). It is the contract: lossless by default, bounded memory, a pure core, and a gate that must stay green (`fmt`, `clippy -D warnings`, rustdoc, tests and the release dry run).

The real logs under `tests/fixtures/` are the specification. Synthetic logs are useful for bootstrapping, but every real capture so far has found a bug that synthetic ones missed. [docs/CAPTURING-LOGS.md](https://github.com/adenijiridwan43/Hushlog/blob/main/docs/CAPTURING-LOGS.md) explains how to capture one safely, including scanning it for secrets first.

---

## Licence

Apache-2.0.
