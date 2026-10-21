# Why Hushlog — the CI log filter that never hides what matters

Hushlog is a smart noise filter for terminal and CI logs. It sits in a pipe
(`cmd | hushlog`) or wraps a command (`hushlog -- cargo build`), highlights what
actually failed, and collapses repetitive warning blocks — while guaranteeing
nothing is ever lost.

## The problem: CI output is mostly repetition

The same deprecation warning, printed once per package. The same retry, once per
attempt with a different pod name. The signal is four lines at the bottom,
buried under thousands of lines nobody reads.

Existing tools split into two camps, and neither fits:

- **Highlighters** (tailspin, lnav) colour a log beautifully. They do not make
  it shorter, and they do not survive a pipe.
- **Agent token savers** (RTK, logslim) compress output for LLM context. They
  optimise for tokens, not for a human reading a failed build.

Hushlog targets the human first: block-aware folding, native CI fold markers,
and `--explain` to show exactly why any given line was folded or kept.

## The guarantee: lossless by default

This is Hushlog's non-negotiable principle. **Nothing is ever silently dropped.**
If a line is not shown, it is recoverable:

- `--tee <file>` — full verbatim log always written to disk
- `--raw` — pass-through, no filtering at all
- `--explain` — annotates every decision (folded / kept / classified as)
- an end-of-run summary reporting exact counts and the full log path

`Level::Error` lines are emitted in full, always, regardless of repeat count.
Collapsing an error requires an explicit opt-in flag (`--collapse-errors`).

A property test in `hushlog-core` asserts that every line classified `Error`
appears in the rendered output. It was written before any collapse logic
existed, precisely so that it can never be weakened to make a collapse feature
pass.

## What Hushlog does

| Feature | What it means for you |
|---------|----------------------|
| **Block-aware folding** | Collapses repeated multi-line warnings, stack traces, and deprecation notices into counted summaries, not just identical lines |
| **CI-native output** | Native GitHub Actions `::group::` and GitLab collapsible sections — works in your CI without manual wrapping |
| **Exit-code passthrough** | Wrap mode returns the child's exact exit code; signals are forwarded |
| **Pipe and wrap mode** | `cmd \| hushlog` or `hushlog -- cmd` — both work, stdout/stderr tracked separately |
| **`--explain`** | Every fold decision annotated on stderr so your rendered output stays clean |
| **`--tee <file>`** | Byte-exact full log on disk, including invalid UTF-8 that would corrupt reconstruction from processed lines |
| **JSON output** | Machine-readable counts, top offenders, error list, fold map for agents and dashboards |
| **TOML rule packs** | Built-in packs for cargo, npm, docker, kubectl, syslog, journald, OTel, pino, bunyan, spdlog, Apache error logs — plus your own |
| **Format auto-detection** | Automatically detects the log format from the first lines and applies the right rules (ported from lnav's format registry) |
| **Bounded memory** | LRU fingerprinting keeps memory under 50 MB regardless of input size |
| **Windows-first** | PTY is opt-in; pipes are primary; CI runs on Windows from day one |

## Who it's for

1. **Platform / DevOps / SRE engineers** — the core audience; they read CI logs all day
2. **GitHub Actions users and CI maintainers** — immediate pain; a ready-made GitHub Action is the best distribution channel
3. **Open-source maintainers with noisy CI** — visible, influential, and will mention the tool
4. **Kubernetes operators** — `kubectl logs`, `stern` users drowning in repetitive container warnings
5. **AI coding-agent users** — people paying for tokens spent on noisy build output. Hushlog's `--format json` serves this audience without becoming only that
6. **Build-tool-heavy teams** — Gradle, Maven, Docker, Terraform, npm rule-pack contributors
7. **Rust community** — early adopters who share CLI tools

## Real before/after

```
hushlog: 18,204 lines → 41 shown | 3 errors | 5 warning groups (2,310 lines folded) | full log: ./build.log
```

## Try it

```sh
# Build from source (no release yet)
git clone https://github.com/adenijiridwan43/Hushlog
cd Hushlog
cargo build --release

# Pipe mode
cargo build 2>&1 | ./target/release/hushlog

# Wrap mode — exit code propagated
./target/release/hushlog -- cargo build

# Keep the full log on disk while showing the filtered version
./target/release/hushlog --tee build.log -- cargo build

# No filtering at all
./target/release/hushlog --raw -- cargo build

# Show why each line was folded or kept
./target/release/hushlog --explain --format plain -- cargo build

# Native CI fold markers
./target/release/hushlog --format github -- cargo test
```

## Built to be trusted

- MIT OR Apache-2.0 licensed
- Every change reviewed against the trait contracts in `AGENTS.md`
- `cargo clippy -- -D warnings` and 254 tests pass on Linux, macOS, Windows
- Lossless-output invariant property-tested
- Release pipeline producing signed static binaries for Linux, macOS, Windows

Hushlog reduces CI log volume while guaranteeing no error lines are dropped.
That's the tool worth putting in your pipe.