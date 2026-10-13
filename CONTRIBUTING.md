# Contributing to hushlog

## Read this first

[`AGENTS.md`](AGENTS.md) is the contract. It is short on purpose. Everything
else in this document elaborates on it; nothing in this document overrides it.

The one rule that matters most: **nothing is ever silently dropped.** If you
implement behaviour that hides a line, you must be able to point to `--tee`,
`--raw`, `--explain`, or the end-of-run summary as the way the user recovers it.
If you cannot, the behaviour is wrong.

## Getting set up

```sh
git clone https://github.com/<you>/hushlog
cd hushlog
cargo build
cargo test --workspace
```

Rust 1.85 or newer (edition 2024).

## The workflow we use

1. **Test first.** Write the failing snapshot or property test. Then implement.
2. **One module per owner.** See the ownership table in
   [`AGENTS.md` section 4](AGENTS.md#4-module-ownership). If your change needs
   another owner's module, open an issue first rather than editing it.
3. **`cargo clippy --workspace --all-targets -- -D warnings` must pass.** The
   workspace already sets clippy `pedantic`. If a lint fires and you are adding
   `#[allow]`, be sure you understand why it fired.
4. **`cargo fmt --all`.**
5. Open a PR describing what changed and what could not.

## Fixtures are the specification

New behaviour needs a fixture in `tests/fixtures/`, and fixtures are **real
logs**, captured verbatim — not strings you invented to make a test pass. See
[`tests/fixtures/README.md`](tests/fixtures/README.md) for what still needs
collecting and how to name things.

If your change alters output for an existing fixture, the snapshot diff is the
review artefact. Read it line by line. A snapshot change that hides an error
line is a bug, not a routine update.

## Rule packs

Rule packs (TOML under `rules/`) are reviewed by a human before merge. A wrong
folding rule can hide a real failure, so:

- Start with `collapse = false` and opt into collapsing per rule.
- Include a fixture demonstrating the rule against real output.
- Say in the PR what the rule matches and, more importantly, what it must not.

## Commit messages

Short imperative subject, body explaining *why* when it is not obvious.
Reference the issue.

## Licence

By contributing you agree your work is licensed under MIT OR Apache-2.0, at your
option.