# hushlog

[![CI](https://github.com/adenijiridwan43/Hushlog/actions/workflows/ci.yml/badge.svg)](https://github.com/adenijiridwan43/Hushlog/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](#licence)
[![crates.io](https://img.shields.io/crates/v/hushlog.svg)](https://crates.io/crates/hushlog)
[![docs.rs](https://img.shields.io/docsrs/hushlog)](https://docs.rs/hushlog)
[![rust](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://www.rust-lang.org)
[![release](https://img.shields.io/github/v/release/adenijiridwan43/Hushlog?include_prereleases&label=release)](https://github.com/adenijiridwan43/Hushlog/releases)

## Turn noisy logs into signal — without losing a single line

hushlog is a lossless, stateful log intelligence layer that runs as a static binary on any machine — from a GitHub Actions runner to a bare VPS. It sits in a pipe (`cmd | hushlog`) or wraps a command (`hushlog -- cargo build`), highlights what actually failed, and collapses repeated warning blocks into counted summaries.

**Nothing is ever silently dropped.** Every line you don't see is still recoverable: `--tee` writes the full input byte-exact to disk, `--raw` passes everything through unchanged, `--explain` annotates every fold decision on stderr, and the end-of-run summary reports exact counts. Error lines are never collapsed unless you opt in with `--collapse-errors`.

This is the tool Datadog and Loki sit behind, not in front of — a local filter that removes noise before it reaches your backend, your CI output, or your AI agent.

### 441 lines in, 43 out

The real capture below is a rate-limit storm from a Node backend: 441 lines of ANSI-coloured access log, a genuine Postgres `FATAL 08P01` with its stack trace, and ~400 repetitions of six request shapes.

```sh
hushlog < real-relanflow-ratelimit-storm.log
```

```
hushlog: 441 lines -> 41 shown | 1 errors | 2 groups (400 lines folded)
```

90% fewer lines, and every one of them still recoverable with `--tee`.

---

## Features

| Feature | What it gives you |
|---|---|
| **Lossless by default** | Error lines are never folded; every folded line is counted in the summary; `--tee` keeps every byte |
| **Byte-exact tee** | Full input written upstream of the pipeline, including invalid UTF-8 — nothing is corrupted for display |
| **Query language** | `--query 'level == error && plain =~ "retry"'` emits only matching lines as JSON or CSV records |
| **Smart collapsing** | Repeated lines fold into one notice (announced once, on the first fold); the exact total lives in the summary |
| **`--anonymize`** | Redacts IPs, UUIDs, MACs, emails, URLs, credit-card numbers and hex dumps with deterministic fakes — opt-in, and the tee file is never anonymized |
| **Six output formats** | `tty`, `plain`, `github`, `gitlab`, `json`, `csv` — native CI fold markers and machine-readable records |
| **Twelve built-in rule packs** | cargo, npm, docker, kubectl, syslog, journald, otel, pino, bunyan, spdlog, Apache error logs, scoped app logs |
| **Format auto-detection** | Sample-driven detection ported from proven model |
| **Auto-decompression** | gzip and bzip2 input decompressed on ingest |
| **Opt-in PTY** | `--pty` gives the child a real terminal for colours and progress bars |

---

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

This is an alpha: the rule packs are still being proven against logs, and the command-line surface may change before 1.0. The release pipeline builds static binaries for five targets (Linux x86_64/aarch64 musl, macOS x86_64/aarch64, Windows x86_64) via `.github/workflows/release.yml` on a `v*` tag; `main` runs a dry-run check on every push so the configuration is verified before a tag exists.

See [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md) for the full channel list, the one-time secret setup, and the release checklist.

---

## Quick start

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

`--pty` is opt-in: it gives the child a real terminal so progress bars and colours work. Under `--pty` the tee file records what the terminal would have shown rather than the raw bytes.

---

## Flags

| Flag | Default | What it does |
|---|---|---|
| `--tee <FILE>` | off | Write the verbatim input log, byte-exact. Created or truncated. |
| `--raw` | off | Pass through, no filtering at all. |
| `--explain` | off | Annotate every fold decision on stderr. |
| `--format <fmt>` | `plain` | `tty`, `plain`, `github`, `gitlab`, `json`, `csv`. |
| `--query <EXPR>` | off | Emit only lines matching the predicate. |
| `--rules <FILE>` | built-ins | Extra TOML pack. Repeatable. |
| `--collapse-errors` | **off** | Opt in to folding errors. Errors are never folded by default. |
| `--max-tracked <N>` | `10000` | LRU capacity, the memory bound. |
| `--no-summary` | off | Suppress the end-of-run summary. |
| `--pty` | off | Run the child on a pseudo-terminal. |
| `--anonymize` | off | Redact IPs, UUIDs, MACs, emails, URLs, credit-card numbers and hex dumps with deterministic fakes. The `--tee` file is never anonymized. |

### `--anonymize` — compliance-safe redaction

`--anonymize` replaces personally identifying and sensitive information in the **rendered output** with consistent fake values: the same sensitive value always maps to the same fake value, so you can still correlate related lines across a log without learning the real values. It redacts IPv4/IPv6 addresses, MAC addresses, UUIDs, email addresses, URLs, credit-card numbers (Luhn-validated), and hex dumps.

The full-log `--tee` file is written byte-exact upstream of the pipeline and is **never** anonymized, so `--tee` plus `--anonymize` gives you both a private display and a complete recovery copy.

---

## Architecture

Four crates, one streaming pipeline, bounded memory:

| Crate | Stage | Notes |
|---|---|---|
| `hushlog-core` | normalize, classify, collapse | **Pure.** No I/O, no clock, no env. |
| `hushlog-rules` | rule schema and loader | TOML packs, one `RegexSet` |
| `hushlog-render` | tty, plain, github, gitlab, json, csv | |
| `hushlog` | args, pipe/wrap, PTY, signals | All I/O lives here |

Stages communicate only through the traits in [`hushlog-core`](https://github.com/adenijiridwan43/Hushlog/blob/main/crates/hushlog-core/src/lib.rs): `Line`, `Level`, `RuleId`, `Classified`, `Classifier`, `Event`, `Collapser`, `Renderer`, `Summary`, `CollapsePolicy`. See [AGENTS.md](AGENTS.md) for the full contracts and the module ownership map.

---

## Performance

Absolute throughput varies by 3-4x between machines, so the number that matters is the **ratio** to the same machine's bare byte-scan baseline. Measured on a 64 MB CI corpus, release build, via `cargo run --release --example stage_bench`:

| Stage | MB/s | Share of end-to-end time |
|---|---|---|
| baseline: byte scan + split | ~980 | — |
| Ingest | ~117 | 15% |
| Normalize (mask + hash) | ~37 | 47% |
| Classify (74 rules) | ~80 | 22% |
| **the real pipeline** | **~15** | — |

The benchmark measures the pipeline the binary **actually** runs — ingest, classify, mask into an owned string, detect blocks, collapse — as cumulative prefixes, so each stage's marginal cost is visible rather than inferred. It also runs the real capture, because a corpus of mostly unique lines measures the per-line cost and a corpus of mostly repeats measures the per-fold cost, and the product needs both.

Masking dominates, and the hash was never the problem: xxh3 fingerprints at ~2000 MB/s while masking runs at ~37 MB/s. A 100 MB CI log filters in a few seconds, which feels instant.

The floor is 1% of the byte-scan baseline. Halving it is a regression; doubling it is a win.

---

## Built-in rule packs

Twelve packs ship built in, compiled into the binary so they cannot go missing between release and run:

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

Every pack is exercised by synthetic logs today; real captures for each are being collected under `tests/fixtures/` (see the fixture README). Real fixtures are the product's proof of correctness.

---

## Agent interface

`--query` with `--format json`/`csv` is the machine-readable layer for scripts and AI agents:

```sh
# Recent errors only, as JSON records
hushlog --query 'level == error' --format json -- npm test

# Warnings as CSV
hushlog --query 'level == warning' --format csv -- cargo build

# Pipe a large build log into an agent cheaply
hushlog --query 'level == error && plain =~ "retry"' --format json -- cargo build
```

Measured on the real captures: a 441-line repetition storm collapses to 43
lines, and selecting the errors out of a 7.1 MB failing test run cuts it by
73% — about 1.8 million tokens down to 477,000.

`--query` implies `--format json` unless `--format csv` is given; `--format tty|github|gitlab` are rejected because they cannot carry structured records.

See **[docs/AGENT-INTERFACE.md](docs/AGENT-INTERFACE.md)** for the query grammar, the measured numbers, and worked examples.

---

## Contributing

Read [AGENTS.md](AGENTS.md) first — it is the contract. The fixtures in `tests/fixtures/` are the real specification; see the [README there](tests/fixtures/README.md) for what still needs collecting, and [docs/CAPTURING-LOGS.md](docs/CAPTURING-LOGS.md) for how to turn a real noisy process into a fixture.

Releasing is documented in [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md): the channel list, the one-time secret setup, and the release checklist.

Agents wrote part of this code; every change was reviewed before landing, and the workspace gate (`cargo clippy -- -D warnings`, `cargo test`) must stay green on every push. Design decisions — lossless-by-default, the contracts, and the measured performance targets — are calls recorded in `AGENTS.md`.

---

## Licence

Apache-2.0
