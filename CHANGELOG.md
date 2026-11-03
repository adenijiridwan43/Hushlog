# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Distribution channels beyond GitHub Releases: a Homebrew formula pushed to
  `adenijiridwan43/homebrew-tap`, a dependency-ordered crates.io publish, and a
  `ghcr.io` container image. See [`docs/DISTRIBUTION.md`](docs/DISTRIBUTION.md).
- A `Dockerfile` building the static musl binary `FROM scratch`, with a
  `.dockerignore` so build artifacts and captured logs never enter a layer.
- [`docs/DISTRIBUTION.md`](docs/DISTRIBUTION.md): channel list, one-time secret
  setup, release checklist, Windows notes, troubleshooting.
- [`docs/CAPTURING-LOGS.md`](docs/CAPTURING-LOGS.md): how to capture a real noisy
  process as a fixture.

### Fixed

- The `.gitignore` did not cover secrets: `.env` files, `*.pem`/`*.key`,
  `id_rsa`, `id_ed25519`, `credentials.toml` and `.cargo/credentials*` were all
  committable. Ad-hoc log captures, insta `*.snap.new` files and OpenCode's local
  state are ignored too. `.env.example` stays committable as a template.

## [0.1.0-alpha.1] - 2026-11-02

The first release. Binaries for five platforms, a Homebrew formula, crates on
crates.io, and a container image.

### Added

- Workspace with four crates: `hushlog-core`, `hushlog-rules`, `hushlog-render`,
  `hushlog`.
- The `hushlog` binary: pipe mode, wrap mode, byte-exact `--tee`, `--raw`,
  `--explain`, `--format`, `--rules`, `--collapse-errors`, `--max-tracked`,
  `--no-summary`, `--anonymize`, `--query`, and opt-in `--pty`.
- Pipeline contracts in `hushlog-core`: `Line`, `Level`, `RuleId`, `Classified`,
  `Classifier`, `Event`, `Collapser`, `Renderer`, `Summary`, `CollapsePolicy`.
- `Ingest` (framing, `\r` progress-frame collapse, ANSI
  strip-for-match/preserve-for-display), `Normalizer` (volatile-token masking +
  xxh3 fingerprinting), `FoldingCollapser` (bounded LRU), `BlockDetector`,
  `FormatDetector`, `Anonymizer`.
- `hushlog-rules`: TOML rule schema, loader that fails loudly with `file:line`,
  single-`RegexSet` classifier, and eleven built-in packs: cargo, npm, docker,
  kubectl, syslog, journald, otel, pino, bunyan, spdlog and Apache error logs.
- `hushlog-render`: tty, plain, github (`::group::`), gitlab, json, csv.
- Format auto-detection, ported from lnav's sample-driven format model.
- Auto-decompression of gzip and bzip2 input on ingest.
- Publishable crates.io metadata on all four crates, including `repository`
  (without which every publish is rejected).
- CI: fmt, clippy (`-D warnings`), tests on Linux/macOS/Windows, MSRV 1.85,
  docs, a throughput regression guard, `cargo-deny` and `cargo-audit`.
- README Acknowledgements describing how AI assistance was used and reviewed.

### Fixed

- **A repetition storm did not get shorter.** The collapse stage counted a
  repeated group correctly, but every renderer printed one `... N similar lines
  folded` notice *per repeat*, so a 441-line real capture of one repeated shape
  came out 441 lines long -- 400 of them notices. A group is now announced once,
  on its first fold. The count shown is the value at that moment; the end-of-run
  summary carries the exact total, and `--tee` still holds every byte. On a
  terminal the one notice stays live and is rewritten in place as the count
  grows, so a run that never ends (`tail -f`, a dev server) still shows the
  group the moment it appears. `Event::Folded` gained a `first_fold` field to
  carry this (AGENTS.md section 4: contract change, human-reviewed).
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

- **The first real fixture landed**: `tests/fixtures/real-relanflow-ratelimit-storm.log`,
  a 441-line capture of a rate-limit storm. It is the reason the fold-notice bug
  above was found. The remaining packs are still exercised only by synthetic
  logs; see [`tests/fixtures/README.md`](tests/fixtures/README.md) for the
  captures still to be collected.
- Throughput is ~3.8% of a bare byte-scan baseline, measured rather than assumed.
  Masking dominates the cost; the hash was never the bottleneck. The original
  200 MB/s target was removed as unreachable.
- Crates are prefixed `hushlog-` because `hush-core` and `hush-cli` are already
  published on crates.io by an unrelated project.

[Unreleased]: https://github.com/adenijiridwan43/Hushlog/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/adenijiridwan43/Hushlog/releases/tag/v0.1.0-alpha.1