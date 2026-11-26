# Test fixtures

## Which of them are REAL

| Fixture | What it is | Real? |
|---|---|---|
| `real-relanflow-ratelimit-storm.log` | 441-line capture of a rate-limit storm from a Node backend | **yes** |
| `cargo-warnings.log` | `cargo build` on a scratch crate with deprecated code | **yes** |
| `rust-test-panic.log` | `cargo test` with two panics and full stack traces | **yes** |
| `npm-install.log` | `npm install` with thirteen deprecation warnings | **yes** |
| `pip-install.log` | `pip install` with build and resolver chatter | **yes** |
| `github-actions-failure.log` | real failing GitHub Actions run (cargo-deny advisory + audit-check) | **yes** |
| `pytest-failure.log` | real `pytest` run with two failures and one pass | **yes** |
| `jest-failure.log` | real `jest` run with two failures and one pass | **yes** |
| `vitest-failure.log` | real `vitest` run with two failures and one pass | **yes** |
| `go-test-failure.log` | real `go test` run with two failures, a panic and one pass | **yes** |
| `out-of-order.log` | lines arriving out of order | **yes** |
| `truncated-json.log` | a log cut off mid-record | **yes** |
| `synthetic-cargo-warnings.log` | placeholder | no |
| `synthetic-npm-build.log` | placeholder | no |
| `synthetic-retry-storm.log` | placeholder | no |
| `synthetic-truncated-utf8.log` | binary on purpose | n/a |

A fixture is real when it is what a tool actually printed, captured verbatim.
None of them are edited: a line is added or removed whole, never reworded,
because a reworded line tests the reworded line rather than the tool.

## What each real one has already caught

`real-relanflow-ratelimit-storm.log` is the one that started this. The collapse
stage counted 400 lines into 2 groups correctly, but the renderers printed one
"... N similar lines folded" notice *per repeat*, so the 441-line storm came
out 441 lines long, 400 of them notices. Real repetition at real volume was the
only thing that exposed it.

`rust-test-panic.log` caught the next one. A Rust panic and a failing test
were both being classified as ordinary output, so the promise in `AGENTS.md`
section 1 — that an error line is never lost — did not apply to the failures
that matter most. The panic lines now classify as errors, and so does the
`test result: FAILED.` summary, which a general rule was swallowing because it
matched the prefix of it.

`cargo-warnings.log` and `npm-install.log` are the first real captures for
those two packs. They show that a rustc warning is a warning and an npm
deprecation notice is a warning — not errors, not noise.

`pytest-failure.log`, `jest-failure.log`, `vitest-failure.log` and
`go-test-failure.log` are real captures from the four test runners. Each says
"failed" differently (`FAILED test::name`, `Tests: N failed`,
`Test Files N failed`, `--- FAIL: TestName`), and each puts the reason on a
different line. The packs match those shapes rather than one runner's wording,
so a real failure keeps its reason in the filtered output. The Go capture also
has a panic with a stack trace: the panic line is an error, the frames are trace
detail.

## The synthetic ones are placeholders

`synthetic-cargo-warnings.log`, `synthetic-retry-storm.log` and
`synthetic-npm-build.log` were hand-written to bootstrap the snapshot harness.
They are **placeholders, not evidence.** They do not reflect what npm, cargo or
a Kubernetes scheduler actually emit, and they are being replaced one by one
as real captures land.

`synthetic-truncated-utf8.log` is not valid UTF-8 and cannot be. It is the
fixture for the `--tee` guarantee in `AGENTS.md` section 1.0: a log containing
a truncated multi-byte character, a lone `0xff`, a `NUL`, an ANSI colour, a `\r`
progress frame, and a final line cut in half by a killed job. `crates/hushlog`
asserts that `--tee` reproduces it byte for byte and that `--raw` copies it
byte for byte.

Editing it in a text editor will corrupt it. It is `gitattributes`-marked
`-text` for the same reason the others are.

## What still needs collecting

Real output from at least these, captured verbatim:

- [x] **a failing GitHub Actions run** — captured (`github-actions-failure.log`,
      run `43869138734`, 1,160 lines). Finding: the run fails with real
      `error[unmaintained]` and `##[error]Resource not accessible by
      integration` lines, which hushlog originally classified as ordinary
      output. They survived filtering, so the never-lost invariant held, but
      classification was wrong. The `cargo.advisory` and `github_actions.error`
      rules now close those gaps; the fixture still asserts both lines survive
      *and* classify as errors.
- [x] a GitHub Actions run with a *real* stack trace — the same fixture
      carries the real cargo-audit/rustsec advisory output and install logs
- [x] a failing `go test ./...` run — captured with Go 1.27.4 on Windows
      (`go-test-failure.log`)
- [ ] `docker build`, multi-stage, with repeated buildkit lines — needs Docker,
      not installed on this machine
- [ ] `kubectl logs` with repeated container warnings — needs a cluster,
      `kubectl` not installed here
- [ ] Maven/Gradle build with deprecation output — needs Java tooling, not
      installed here
- [ ] `pip install` that actually emits deprecation warnings — modern pip does
      not emit `DEPRECATION` lines; `pip-install.log` is the resolver chatter
      capture of this, noted in the tool's own behaviour

The failing-run fixture is the most important of them. Everything else
exercises noise; that one exercises the promise in `AGENTS.md` section 1.

## Naming

Real fixtures drop the `synthetic-` prefix:

```
tests/fixtures/cargo-warnings.log
tests/fixtures/npm-install.log
tests/fixtures/gh-actions-failure.log
```

## How to capture one

See [`docs/CAPTURING-LOGS.md`](../../docs/CAPTURING-LOGS.md) for the full
procedure. The short version:

```sh
cargo build --release                 # build hushlog first
<the command> > capture.log 2>&1       # redirect; do not pipe on Windows
hushlog --tee check.log < capture.log  # what hushlog made of it
```

Then read it. If the tool's output is not what hushlog says it is, the rule
pack is wrong, and the fixture is the evidence.

Scan the capture for secrets before committing it. Trim by removing whole
lines, never by editing one.

## Why this matters more than it looks

Agents can write the pipeline. Agents cannot invent what these tools actually
print. If the fixtures are synthetic, the snapshot tests confirm only that the
code agrees with itself, and the first real-world run is where the folding
rules get their first honest test.
