# AGENTS.md — working agreements for Hushlog

This file is the contract. Read it before you write code, and do not change it
without a human reviewing the change.

---

## 1. The non-negotiable principle: lossless by default

Hushlog sits in a pipe (`cmd | hushlog`) or wraps a command
(`hushlog -- cargo build`). Engineers will paste its output into a bug report.
If we hide the one line they needed, the tool has destroyed their afternoon and
our credibility in a single stroke.

**Nothing is ever silently dropped.** If a line is not shown, it must be
recoverable by at least one of:

- `--tee <file>` — full verbatim log always written to disk
- `--raw` — pass-through, no filtering at all
- `--explain` — annotates every decision (folded / kept / classified as)
- an end-of-run summary reporting exact counts and the full log path

If you cannot point to one of those for a behaviour you are implementing, the
behaviour is wrong.

### 1.0 `--tee` is byte-exact, and that constrains WP6

`Line.raw` is a `String`. `Ingest` builds it with `String::from_utf8_lossy`, so
a log containing invalid UTF-8 -- truncated multibyte text, binary noise, a log
truncated mid-write by a killed job -- has its undecodable bytes replaced with
`U+FFFD`.

That is fine for **display**. It is not fine for `--tee`.

**Therefore: WP6 must tee the input byte stream, never `Line.raw`.** Write the
bytes as they are read from stdin/the child, upstream of `Ingest`. Reconstructing
the tee file from `Line`s silently corrupts every log containing invalid UTF-8,
and the corruption is invisible because the file still looks like a log.

The same rule applies to `--raw` passthrough.

### 1.1 Errors are never collapsed

`Level::Error` lines are emitted in full, always, regardless of repeat count.
Collapsing an error requires an explicit opt-in flag. This is not a default you
may tune — see `CollapsePolicy` in `hushlog-core`.

### 1.2 The invariant property test

`hushlog-core` holds a property test asserting that **every line classified
`Error` appears in the rendered output**. It must stay green. It is the single
test that protects this product's reputation. It was written before any collapse
logic existed, precisely so that it could never be weakened to make a collapse
feature pass.

---

## 2. Pipeline shape

Streaming, line by line, bounded memory. Never buffer the whole input.

```
stdin / child process
      │
 [1] Ingest     bytes -> lines; \r frames; ANSI; partial-line flush
      │
 [2] Normalize  strip ANSI for matching; mask volatile tokens; hash
      │
 [3] Classify   level + category via rule engine (RegexSet, single pass)
      │
 [4] Group      block detection; fingerprinting; dedupe/collapse (bounded LRU)
      │
 [5] Render     TTY / plain / github / gitlab / json; summary
      │
 stdout  (+ optional tee file, + optional JSON report)
```

## 3. Core contracts

These signatures live in `hushlog-core` and are the **only** way the stages talk
to each other. Do not change a signature without human review.

```rust
pub struct Line { pub raw: String, pub plain: String, pub stream: Stream, pub no: u64 }

pub enum Level { Error, Warning, Info, Progress, Trace }

pub struct Classified { pub line: Line, pub level: Level, pub rule_id: Option<RuleId>, pub highlight: bool }

pub trait Classifier { fn classify(&mut self, line: Line) -> Classified; }

pub enum Event {
    Emit(Classified),
    Folded { fp: u64, count: u64, sample: Classified },
    BlockStart(u64),
    BlockEnd(u64),
    Summary(Summary),
}

pub trait Collapser { fn push(&mut self, c: Classified) -> Vec<Event>; fn finish(&mut self) -> Vec<Event>; }

pub trait Renderer { fn render(&mut self, e: &Event) -> std::io::Result<()>; }
```

### 3.1 `hushlog-core` is pure

No I/O, no process spawning, no clock, no environment reads. It takes data and
returns data. This keeps it trivially testable and reusable (GitHub Action, WASM
playground, editor extensions). I/O lives in `hushlog-cli`.

---

## 4. Module ownership

One owner per module. Agents work in parallel and communicate **only** through
the traits in §3. If your change requires editing another owner's module, stop
and ask — do not do it.

| Module | Owner | Scope |
|---|---|---|
| `hushlog-core/src/ingest.rs` | WP1 | framing, `\r`, ANSI, partial flush |
| `hushlog-core/src/normalize.rs` | WP2 | masking, fingerprinting |
| `hushlog-rules/` | WP3 | schema, loader, `RegexSet` classifier |
| `hushlog-core/src/collapse.rs`, `block.rs` | WP4 | LRU, block detection |
| `hushlog-render/` | WP5 | all five renderers |
| `hushlog-cli/` | WP6 | args, pipe/wrap, PTY, signals |
| CI, docs, benches | WP7 | release pipeline |

---

## 5. Testing rules

1. **Test first.** Write the failing snapshot or property test, then implement.
2. `cargo clippy -- -D warnings` must pass. Workspace lints already set
   `pedantic`; do not add `#[allow]` to silence a lint you have not understood.
3. New behaviour needs a fixture. Fixtures are real logs under
   `tests/fixtures/`, not invented strings.
4. The error-never-lost property test (§1.2) is not yours to modify.

## 6. Code style

```toml
edition = "2024"
rust-version = "1.85"
```

- snake_case functions/variables, PascalCase types/traits, SCREAMING_SNAKE_CASE constants
- max line length 100
- `?` over `unwrap()`/`expect()` in library code; `expect` with a message is
  acceptable in tests
- every `unsafe` block needs a `// SAFETY:` comment explaining why it is sound
  (note: the workspace sets `unsafe_code = "warn"`, and v1 should contain none)
- errors: `thiserror` for typed errors, `anyhow` at the binary boundary

## 7. Performance targets

- startup < 10 ms
- throughput >= 200 MB/s on typical logs
- memory < 50 MB regardless of input size

These are acceptance criteria, not aspirations. If your change regresses them,
that is a bug — measure before and after with `cargo bench`.

## 8. Windows

Windows is a first-class target, not an afterthought.

- **Pipes first.** PTY is opt-in, only where a child genuinely needs TTY
  behaviour (colours, progress bars).
- Signal forwarding and console control handlers differ on Windows. Do not
  assume POSIX semantics.
- CI runs Windows from day one so this never surfaces as a surprise at release.

## 9. Naming

Project `hushlog`, binary `hushlog`. All crates carry the `hushlog-` prefix:
`hushlog-core`, `hushlog-rules`, `hushlog-render`, `hushlog-cli`.

The prefix is not cosmetic. `hush-core` and `hush-cli` are already published on
crates.io by an unrelated project, so the unprefixed names cannot be used.