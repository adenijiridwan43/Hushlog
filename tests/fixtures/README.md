# Test fixtures

## These three are SYNTHETIC

`synthetic-cargo-warnings.log`, `synthetic-retry-storm.log` and
`synthetic-npm-build.log` were hand-written to bootstrap the snapshot harness.
They are **placeholders, not evidence.** They do not reflect what npm, cargo or
a Kubernetes scheduler actually emit.

**They get replaced.** See `AGENTS.md` section 5: fixtures are real logs.

## One of them is binary on purpose

`synthetic-truncated-utf8.log` is not valid UTF-8 and cannot be. It is the
fixture for the `--tee` guarantee in `AGENTS.md` section 1.0: a log containing
a truncated multi-byte character, a lone `0xff`, a `NUL`, an ANSI colour, a `\r`
progress frame, and a final line cut in half by a killed job. `crates/hushlog`
asserts that `--tee` reproduces it byte for byte and that `--raw` copies it
byte for byte.

Editing it in a text editor will corrupt it. It is `gitattributes`-marked
`-text` for the same reason the others are.

## What needs to be collected

Real output from at least these, captured verbatim:

- [ ] `cargo build` with warnings
- [ ] `npm install` with deprecation output
- [ ] `docker build`, multi-stage, with repeated buildkit lines
- [ ] `kubectl logs` with repeated container warnings
- [ ] **a failing GitHub Actions run** — must contain real errors
- [ ] Maven/Gradle build with deprecation output
- [ ] `pip install` with resolver chatter
- [ ] a Rust test run with a panic and a stack trace (needed for block detection)

The failing-run fixture is the most important of the eight. Everything else
exercises noise; that one exercises the promise in `AGENTS.md` section 1.

## Naming

Real fixtures drop the `synthetic-` prefix:

```
tests/fixtures/cargo-warnings.log
tests/fixtures/npm-install.log
tests/fixtures/gh-actions-failure.log
```

## Why this matters more than it looks

Agents can write the pipeline. Agents cannot invent what these tools actually
print. If the fixtures are synthetic, the snapshot tests confirm only that the
code agrees with itself, and the first real-world run is where the folding
rules get their first honest test.