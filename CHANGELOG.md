# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Workspace with four crates: `hushlog-core`, `hushlog-rules`, `hushlog-render`,
  `hushlog`.
- The `hushlog` binary: pipe mode, wrap mode, byte-exact `--tee`, `--raw`,
  `--explain`, `--format`, `--rules`, `--collapse-errors`, `--max-tracked`,
  `--no-summary`, and opt-in `--pty`.
- Pipeline contracts in `hushlog-core`: `Line`, `Level`, `RuleId`, `Classified`,
  `Classifier`, `Event`, `Collapser`, `Renderer`, `Summary`, `CollapsePolicy`.
- `Ingest` (framing, `\r` progress-frame collapse, ANSI
  strip-for-match/preserve-for-display), `Normalizer` (volatile-token masking +
  xxh3 fingerprinting), `FoldingCollapser` (bounded LRU), `BlockDetector`.
- `hushlog-rules`: TOML rule schema, loader that fails loudly with `file:line`,
  single-`RegexSet` classifier, built-in cargo/npm/docker/kubectl packs.
- `hushlog-render`: tty, plain, github (`::group::`), gitlab, json.
- Publishable crates.io metadata on all four crates, including `repository`
  (without which every publish is rejected).
- CI: fmt, clippy (`-D warnings`), tests on Linux/macOS/Windows, MSRV 1.85,
  docs, a throughput regression guard, `cargo-deny` and `cargo-audit`.
- README Acknowledgements describing how AI assistance was used and reviewed.

### Fixed

- `hushlog-rules` shipped without its rule packs: `include_str!` reached outside
  the crate directory, so `cargo package` omitted them and a published crate
  would have failed to compile for every dependent. The packs now live inside
  the crate.
- `block_detector` failed on every run, because two built-in packs declare
  pattern block markers and the bridge only accepted literals. `BlockDetector`
  now has compiled pattern markers; an invalid pattern fails loudly.
- The kubectl events-table rule was written title-case and so never matched real
  `kubectl` output, which prints the header upper case.
- Exit codes on Windows: a child that was *terminated* rather than exiting
  returned `None` from `ExitStatus::code()` and was reported as exit 1. It is
  now 128, distinguishable from a genuine 1.

### Notes

- Nothing has been released yet. There are no tags and no binaries.
- The rule packs have only ever been tested against **synthetic** logs. See
  [`tests/fixtures/README.md`](tests/fixtures/README.md) for the real captures
  still to be collected. Treat the folding rules as unproven.
- Throughput is ~2% of a bare byte-scan baseline, measured rather than assumed.
  Masking is 60% of the cost; the hash was never the bottleneck. The original
  200 MB/s target was removed as unreachable.
- Crates are prefixed `hushlog-` because `hush-core` and `hush-cli` are already
  published on crates.io by an unrelated project.

[Unreleased]: https://github.com/adenijiridwan43/Hushlog/compare/v0.1.0...HEAD