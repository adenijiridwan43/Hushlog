# Hush — Smart Noise Filter for Terminal & CI Logs

> Working name: **Hush** (verify availability on crates.io, GitHub, Homebrew and as a shell command before committing).
> Language: **Rust**. License: **MIT OR Apache-2.0**.
> Document date: 2026-10-12.

---

## 1. One-paragraph pitch

Modern CI/CD and container logs are flooded with verbose, repetitive warnings. **Hush** is a single-binary CLI that sits in a pipe (`cmd | hush`) or wraps a command (`hush -- cmd`), highlights actionable errors, and collapses repetitive warning blocks into counted summaries, while guaranteeing that nothing is ever lost (full log is always preserved and every fold is expandable).

**Non-negotiable principle:** *lossless by default.* DevOps engineers will never trust a tool that might hide the one line they needed.

---

## 2. Market check: what already exists (read before you build)

This space is **not empty**. Findings from a search on 2026-10-12 (verify each one yourself before positioning):

| Tool | Language | What it does | Gap relative to Hush |
|---|---|---|---|
| **tailspin** (`tspin`) | Rust | Zero-config log *highlighter* with pager; reported ~7.8k GitHub stars | Highlights only; does not fold or dedupe warning blocks |
| **lnav** | C++ | Mature interactive log navigator with format detection | Interactive viewer, not a pipe filter for CI output |
| **hl** | Rust | Fast structured-log processor/pager | Structured-log focus |
| **fblog** | Rust | JSON log viewer | JSON only |
| **RTK (Rust Token Killer)** | Rust | CLI proxy that compresses command output for LLM coding agents; includes deduplicated logs and "errors/warnings only" modes | Aimed at saving LLM tokens, wired into AI-tool hooks |
| **logslim** | (MCP/CLI) | Compacts noisy test/build/lint output for agents and CI summaries; dedupes repeated lines | Agent/token focus; PR-summary angle |
| **OpenToken** | TypeScript | Multi-stage compression of tool output for LLMs, includes log folding | LLM-context focus |
| **log-compactor** | Python | Merges duplicate structured log lines in real time via YAML config | Python, config-heavy, structured events |
| **timber-rs** | Rust | Log file analysis (levels, trends, dedup stats) | Analysis tool, not a live CI filter |
| **CI-native folding** | n/a | GitHub Actions `::group::`, GitLab collapsible sections | Manual: someone must wrap steps in groups |
| **Platform pattern clustering** (Datadog, Grafana Loki, etc.) | SaaS | Cluster similar log lines server-side | Requires a logging platform; not a local CLI |

### What this means

- The *general* idea ("dedupe noisy logs") is already done several times, especially for the **LLM-token-saving** use case. A generic "collapse repeated lines" tool will not stand out.
- The *gap* that looks open: a **CI-native, human-first, trust-first** filter with:
  1. **Block-aware** folding (multi-line warnings, stack traces, deprecation notices), not just identical-line dedup.
  2. **Wrap mode that preserves exit codes** and emits **native CI fold markers** (GitHub Actions / GitLab) automatically.
  3. **Community rule packs** per ecosystem (npm, cargo, docker, kubectl, maven/gradle, terraform, pip).
  4. **`--explain`**: shows exactly why every line was folded or kept (this is the trust feature).
  5. **Dual output**: human-readable *and* compact/JSON mode for LLM agents. The agent-token market is hot, so supporting it is a growth lever.
- **Action before Day 1:** install tailspin, RTK and lnav, run them on 5 real noisy logs, and write down what each fails to do. That list becomes your README's "why Hush".

---

## 3. Scope

### In scope for v1.0
1. Pipe mode and wrap mode.
2. ANSI-aware processing (strip for matching, preserve for display).
3. Classification: error / warning / info / progress / stack-trace.
4. Highlighting of actionable errors with configurable context lines.
5. Fingerprint-based collapsing of repeated lines **and multi-line blocks**.
6. Lossless guarantee: `--tee <file>` full log, `--raw` passthrough, `--explain`.
7. Renderers: TTY (colour), plain, GitHub Actions folds, GitLab folds, JSON.
8. TOML rule packs (built-in + user-supplied).
9. End-of-run summary.
10. Exit-code passthrough and signal forwarding in wrap mode.

### Out of scope for v1.0
ML/learned template mining, web UI, log shipping/aggregation, daemon mode, plugin system beyond TOML rules.

---

## 4. Architecture

Streaming pipeline, line-by-line, bounded memory:

```
 stdin / child process (stdout + stderr via pipes or PTY)
        │
   [1] Ingest      read bytes → lines; handle partial lines, \r progress frames, ANSI
        │
   [2] Normalize   strip ANSI for matching (keep original for display);
        │          mask volatile tokens (timestamps, UUIDs, hashes, IPs, numbers, paths)
        │
   [3] Classify    level + category via rule engine (RegexSet / aho-corasick)
        │
   [4] Group       block detection + fingerprinting + dedupe/collapse (bounded LRU)
        │
   [5] Render      TTY / plain / GitHub / GitLab / JSON; end-of-run summary
        │
   stdout  (+ optional full-log tee file, + optional JSON report)
```

### 4.1 Components

**Ingest**
- Pipe mode: read `stdin`. Wrap mode: spawn child, capture stdout and stderr separately, forward signals (SIGINT/SIGTERM), return the child's exit code.
- Use a PTY (`portable-pty`) only when the child needs TTY behaviour (colours, progress bars); otherwise plain pipes.
- Collapse `\r` progress frames to the final frame. Flush partial lines on a short timeout so live tailing feels live.

**Normalizer / fingerprinting**
- Replace timestamps, UUIDs, hex hashes, IPs, numbers, and absolute paths with placeholders, then hash (xxhash/ahash).
- `Retry 3/10 for pod abc-123` and `Retry 4/10 for pod xyz-789` produce the same fingerprint.

**Classifier**
- Rule shape (TOML): `match` (regex or literal) → `level`, `highlight`, `collapse`, `context_lines`, optional block `start`/`end` markers.
- All rules compiled once into a `RegexSet`; single pass per line.

**Block detector**
- Detects multi-line units (stack traces, indented continuation lines, Maven/Gradle warnings, deprecation notices with URLs) via indentation heuristics plus rule-defined markers; fingerprints the whole block.

**Collapser**
- Bounded LRU: `fingerprint → {count, first_line_no, last_line_no, sample}`.
- Policy: show first occurrence in full; later repeats become `⚠ <summary> (×N)`; summary at the end.
- **Errors are never collapsed** unless the user opts in explicitly.

**Renderer**
- TTY: colours (`anstream`/`owo-colors`), optional live counter line.
- CI: plain text, auto-detected via env (`GITHUB_ACTIONS`, `GITLAB_CI`), emitting native group markers.
- `--format json`: counts, top offenders, error list, fold map.

**Summary line**
```
hush: 18,204 lines → 41 shown | 3 errors | 5 warning groups (2,310 lines folded) | full log: ./build.log
```

### 4.2 Core Rust interfaces (starting contract for agents)

```rust
// hush-core
pub struct Line { pub raw: String, pub plain: String, pub stream: Stream, pub no: u64 }

pub enum Level { Error, Warning, Info, Progress, Trace }

pub struct Classified { pub line: Line, pub level: Level, pub rule_id: Option<RuleId>, pub highlight: bool }

pub trait Classifier { fn classify(&mut self, line: Line) -> Classified; }

pub enum Event {
    Emit(Classified),                       // show as-is
    Folded { fp: u64, count: u64, sample: Classified }, // collapsed repeat
    BlockStart(u64), BlockEnd(u64),
    Summary(Summary),
}

pub trait Collapser { fn push(&mut self, c: Classified) -> Vec<Event>; fn finish(&mut self) -> Vec<Event>; }

pub trait Renderer { fn render(&mut self, e: &Event) -> std::io::Result<()>; }
```

`hush-core` must be **pure** (no I/O, no process spawning) so it is trivially testable and reusable (GitHub Action, WASM playground, editor extensions).

### 4.3 Crate stack

| Concern | Crate |
|---|---|
| CLI args | `clap` |
| Regex / multi-pattern | `regex`, `aho-corasick` |
| Concurrency | plain threads + `crossbeam-channel` for v1 (add `tokio` only if needed) |
| PTY / subprocess | `portable-pty`, `std::process` |
| Colours / TTY | `anstream`, `owo-colors`, `is-terminal` |
| Config / rules | `serde`, `toml` |
| Hashing | `xxhash-rust` or `ahash` |
| Errors | `anyhow`, `thiserror` |
| Tests | `insta` (snapshots), `proptest`, `assert_cmd`, `criterion` |

---

## 5. Repository layout

```
hush/
├── Cargo.toml                 # workspace
├── crates/
│   ├── hush-core/             # normalizer, classifier, block detector, collapser (pure)
│   ├── hush-rules/            # rule schema, loader, built-in packs
│   ├── hush-render/           # tty / plain / github / gitlab / json
│   └── hush-cli/              # binary: args, pipe & wrap modes, PTY, signals
├── rules/                     # community rule packs (npm.toml, cargo.toml, docker.toml, ...)
├── tests/
│   └── fixtures/              # REAL logs + expected output snapshots  <-- most valuable asset
├── benches/
├── docs/                      # mdBook
├── .github/workflows/         # ci.yml, release.yml, audit.yml
├── CONTRIBUTING.md  CODE_OF_CONDUCT.md  SECURITY.md  CHANGELOG.md  README.md
└── LICENSE-MIT  LICENSE-APACHE
```

---

## 6. Infrastructure

**CI (GitHub Actions)**
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` on Linux, macOS, Windows.
- `cargo deny` (licences, bans) and `cargo audit` (vulnerabilities).
- Snapshot tests over the fixtures corpus.
- Benchmark job with regression threshold.
- MSRV check.

**Releases**
- `cargo-dist` (or `cross`) building static binaries: Linux x86_64/aarch64 (musl), macOS x86_64/aarch64, Windows x86_64.
- Channels: GitHub Releases, crates.io, Homebrew tap, shell installer, `FROM scratch` Docker image; later apt/AUR/Nix/Scoop.
- Checksums + signed artefacts (cosign).

**Quality gates (the "trust" tests)**
- **Invariant property test:** for any generated input, every line classified `Error` must appear in the output. This one test protects the product's reputation.
- **Exit-code test:** wrapped command's exit code is always propagated.
- **Memory test:** bounded memory on a multi-GB synthetic stream.

**Performance targets**
- Startup < 10 ms; throughput ≥ 200 MB/s on typical logs; memory < 50 MB regardless of input size.

---

## 7. Can AI agents finish v1 in a week?

**Honest answer: they can build a strong v0.1–v0.2 in a week. Full v1.0 in one week is unrealistic, and the limit isn't coding speed.**

Why the code is fast: the pipeline is modular, the interfaces are small, and Rust's compiler gives agents tight feedback.

Why v1.0 is slower:
1. **Fixtures need real logs.** Agents cannot invent realistic noisy output from npm/docker/k8s/maven reliably. You have to collect them, and they are the spec.
2. **Trust takes validation.** Folding rules must be reviewed by a human; one wrongly hidden error destroys credibility.
3. **Cross-platform edge cases** (PTY on Windows, signal forwarding, interleaved stdout/stderr) surface only through real use.
4. **Feedback from real users** is calendar-bound, not compute-bound.
5. Agents sometimes hallucinate crate APIs or over-engineer; review and `cargo check` loops are needed.

### Realistic schedule

| When | Milestone | Done means |
|---|---|---|
| Day 1 | Foundation (human + 1 agent) | Workspace, CI, trait contracts above, 10+ real log fixtures committed, snapshot harness |
| Day 2–3 | Core engine (parallel agents) | Normalizer, fingerprinting, classifier, collapser pass fixture snapshots; invariant property test green |
| Day 4 | CLI + rendering | Pipe mode, `--tee`, `--raw`, TTY + plain renderers, summary line |
| Day 5 | Wrap mode + CI formats | Exit-code/signal passthrough, GitHub/GitLab fold output |
| Day 6 | Rule packs + docs | npm, cargo, docker, kubectl packs; README with before/after demo; CONTRIBUTING |
| Day 7 | **Public alpha v0.2** | Binaries released, Show HN-ready demo, issue templates |
| Weeks 2–4 | Hardening → v0.3–v0.9 | Block/stack-trace detection, `--explain`, JSON format, Windows PTY, more packs, perf tuning, fixes from first users |
| Weeks 4–6 | **v1.0** | Stable rule schema, semver commitment, Homebrew + Docker, docs site |

### Agent work packages (parallelisable once contracts exist)

| WP | Owner type | Deliverable | Acceptance |
|---|---|---|---|
| WP0 | You + agent | Repo, CI, traits, fixtures | CI green on empty pipeline |
| WP1 | Agent A | Ingest (lines, `\r`, ANSI, partial flush) | Unit + proptest on chunk boundaries |
| WP2 | Agent B | Normalizer + fingerprint | Same-fingerprint tests for volatile tokens |
| WP3 | Agent C | Rule schema, loader, `RegexSet` classifier | Rules load/validate; fixtures classified |
| WP4 | Agent D | Collapser + block detector | Snapshot tests; error-never-lost invariant |
| WP5 | Agent E | Renderers (TTY/plain/GH/GitLab/JSON) | Snapshot tests per renderer |
| WP6 | Agent F | CLI: pipe/wrap, PTY, signals, exit codes | `assert_cmd` integration tests |
| WP7 | Agent G | Release pipeline, docs, benchmarks | Tagged release produces all binaries |

### Rules for working with agents
- **Tests first:** write the failing snapshot/property test, then the implementation.
- **One module per agent**, communicating only through the traits in §4.2; no cross-crate edits without review.
- **Pin dependency versions**; require `cargo check`/`clippy` to pass before any merge.
- **You review every rule pack** and every change to collapse policy.
- Keep an `AGENTS.md` in the repo stating the lossless principle, the contracts, and "never collapse errors by default".

---

## 8. Will this build your CV?

**Yes, if you ship it and it gets used. A repository alone does not build a CV; evidence of impact does.** Because agents wrote much of the code, hiring managers will focus on what *you* decided and what *happened*:

What makes it count:
- **Adoption numbers:** stars, installs (crates.io/Homebrew downloads), contributors, merged external rule packs.
- **Engineering judgement you can explain:** why lossless-by-default, why fingerprinting over ML, why a pure core crate, how you tested the "never hide an error" invariant.
- **Measured results:** benchmarks (MB/s, memory), and a real before/after (e.g. "18,204 lines → 41 shown").
- **Public writing:** a launch post and a "how it works" technical post.
- **Ownership signals:** release cadence, issue triage, a changelog, responding to users.

Example CV bullets (use only once true):
- "Designed and shipped **Hush**, an open-source Rust CLI that reduces CI log volume by X% while guaranteeing no error lines are dropped; N★ on GitHub, used by M teams."
- "Built a streaming log-fingerprinting engine processing 200+ MB/s with <50 MB memory; property-tested lossless-output invariant."
- "Established CI/CD release pipeline producing signed static binaries for Linux, macOS and Windows; distributed via crates.io and Homebrew."

Be transparent that AI agents assisted; describe your role as architect, reviewer and maintainer. That is increasingly viewed as a legitimate skill when you can show you understand the code.

---

## 9. Who to market to (in priority order)

1. **Platform / DevOps / SRE engineers** — the core audience; they read CI logs all day.
2. **GitHub Actions users and CI maintainers** — pain is immediate; offer a ready-made GitHub Action (`uses: you/hush-action`). This is your best distribution channel.
3. **Open-source maintainers with noisy CI** — they are visible, influential and will mention the tool.
4. **Kubernetes operators** (`kubectl logs`, `stern` users) — repetitive container warnings.
5. **AI coding-agent users** — people paying for tokens spent on noisy build output. Evidence of demand: tools like RTK, logslim and OpenToken exist for this. A `--format compact`/JSON mode lets you serve this audience too, but don't let it become your only identity (it's crowded).
6. **Build-tool-heavy teams** (Gradle/Maven/Docker/Terraform/npm) — rule-pack contributors.
7. **Rust community** — early adopters who share CLI tools.

**Channels:** Show HN, r/rust, r/devops, r/kubernetes, Lobsters, This Week in Rust, dev.to/Hashnode, LinkedIn and X posts with a before/after GIF, awesome-lists (awesome-rust, awesome-cli-apps, awesome-sre), GitHub Marketplace for the Action, Homebrew.

**Launch checklist**
- [ ] 15-second terminal GIF (`vhs`/`asciinema`): noisy log → clean log.
- [ ] README opening with the before/after numbers.
- [ ] "Why not tailspin / lnav / RTK?" honest comparison section.
- [ ] 3 "good first issue" rule-pack tasks for contributors.
- [ ] One launch post + one technical deep-dive.

---

## 10. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Hiding something important | Errors never collapsed by default; `--tee`; `--raw`; `--explain`; invariant property test |
| Crowded niche; "just another dedupe tool" | Block-aware folding, CI-native output, rule packs, trust features; differentiate in the README |
| Block detection is hard across tools | Start with 5–6 ecosystems; grow via community packs |
| Interleaved stdout/stderr, buildkit-style parallel output | Per-stream state; real fixtures; document known limits |
| Windows PTY quirks | Pipes-first on Windows; PTY optional; Windows CI from day 1 |
| Name collision | Check crates.io/GitHub/Homebrew now; fallbacks: `hushlog`, `sift-log`, `squelch`, `foldlog` |
| Agent-generated code quality | Tests first, small modules, human review, clippy-deny |

---

## 11. Roadmap

- **v0.1** — pipe mode, ANSI handling, generic error/warning detection, fingerprint collapsing, summary, `--tee`.
- **v0.2** — wrap mode (exit code passthrough), TOML rule packs (npm, cargo, docker, kubectl), GitHub/GitLab fold output. *(Week-1 target.)*
- **v0.3** — block / stack-trace detection, `--format json`, config file, `--explain`.
- **v1.0** — stable rule schema, semver commitment, community rule registry, Homebrew + Docker distribution, docs site.
- **Later** — Drain-style template mining, GitHub Action wrapper, interactive TUI for expanding folds, WASM playground, editor integrations.

---

## 12. Immediate next steps (today)

1. Collect 10–15 **real** noisy logs (npm install, cargo build, docker build, kubectl logs, a failing GitHub Actions run, Maven/Gradle build, pip install).
2. Run tailspin, lnav and RTK on them; note what each doesn't do.
3. Confirm the name's availability; create the GitHub repo.
4. Scaffold the workspace and commit the §4.2 trait contracts + `AGENTS.md`.
5. Launch WP1–WP3 in parallel; add the "errors never lost" property test before any collapse logic.
