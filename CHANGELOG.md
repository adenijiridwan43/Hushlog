# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.2] - 2026-11-24

### Added

- Distribution channels beyond GitHub Releases: a Homebrew formula pushed to
  `adenijiridwan43/homebrew-tap`, a dependency-ordered crates.io publish, and a
  `ghcr.io` container image.
- A `Dockerfile` building the static musl binary `FROM scratch`, with a
  `.dockerignore` so build artifacts and captured logs never enter a layer.
- [`docs/CAPTURING-LOGS.md`](docs/CAPTURING-LOGS.md): how to capture a real noisy
  process as a fixture.
- [`docs/AGENT-INTERFACE.md`](docs/AGENT-INTERFACE.md): the query grammar, the
  two levers an agent has, and the measured savings on the real captures.
- **Four real fixtures**, captured verbatim from the tools themselves:
  `cargo-warnings.log` (a `cargo build` with warnings), `rust-test-panic.log`
  (a `cargo test` with two panics and full stack traces), `npm-install.log`
  (an `npm install` with thirteen deprecation warnings) and `pip-install.log`
  (a `pip install` with resolver chatter). The synthetic placeholders they
  replace are still there; the real ones are what the rules are now proved
  against.
- `--anonymize` is documented in the README's flags table, with a section on
  what it redacts and the guarantee that the `--tee` file is never anonymized.
- **A `scoped` rule pack**, covering the bracketed scope-and-level prefix that
  application loggers use -- `[db:error] Database connection failed`,
  `[server:warn] ...`. It is the twelfth built-in pack, and it exists because
  the real rate-limit-storm capture contains a genuine Postgres `FATAL 08P01`
  that no pack matched, so hushlog reported "0 errors" for a log with a
  database failure in it.
- A README section listing the twelve built-in rule packs and what each covers.
- A README performance section, so a reader can judge whether hushlog suits
  their log volume before installing it.
- A README opening built on the real capture: 441 lines in, 43 out.

### Changed

- The README has been rewritten for people who are evaluating and using
  hushlog: a real before/after, copyable examples, the full flag list, and
  plain explanations of the guarantees and the benchmark.
- `docs/CAPTURING-LOGS.md` and `docs/AGENT-INTERFACE.md` have been rewritten
  in the same voice as the README, with practical examples and the install
  commands brought up to date.
- **The pipeline is about a third faster end to end.** The benchmark measured a
  proxy pipeline that the binary never ran, so the cost of the real one was
  invisible. Measuring the real shape -- ingest, classify, mask into an owned
  string, detect blocks, collapse -- found three things:

  - The block detector ran **twenty-one separate substring searches on every
    line**: eight start markers, six continuation markers, five end markers and
    two URL probes, whether or not a block was even open. They are now one
    Aho-Corasick pass over the line, which the workspace already depended on
    and did not use.
  - The classifier **compiled a regular expression from scratch on every
    matching line** -- once for the rule's own pattern to read its capture
    groups, and again for each entry of its level map. A rule with a level
    field cost up to five compilations per line, which was most of what
    classification cost. Patterns are compiled once, at load time. This also
    makes the classifier honest about the promise on `RegexPattern`: it never
    touches `regex::Regex::new` on the hot path.
  - The normalizer scanned every line **twice** -- once to ask whether it
    could contain a token, once to mask it -- and read the word around every
    `a` through `f` twice, because ordinary prose is full of them. It now
    scans once, and reads each word once.

  Measured by running the old and new binaries alternately on the same input,
  so machine drift cannot favour either: **11.5 MB/s -> 15.3 MB/s** on the
  real capture. Output is byte-identical in all four text formats.
- `stage_bench` measures the pipeline the binary actually runs, as cumulative
  prefixes, and runs the real capture as well as the synthetic corpus: the
  synthetic one is mostly unique lines and measures the per-line cost, the
  real one is mostly repeats and measures the per-fold cost. It prints a
  `RATIO real_pipeline_mbps=` line for the CI regression guard.
- Level maps are matched case-insensitively, whichever way the pattern is
  spelled.
- `AGENTS.md` section 7 now describes the pipeline the binary actually runs,
  and records that a benchmark measuring a proxy is worse than no benchmark.

### Fixed

- docs.rs could not build documentation for the `hushlog` package because it
  is binary-only: `rustdoc --lib` failed with "no library targets found". The
  package now has a small library target carrying its crate documentation, and
  no pipeline code had to move. The redundant `homepage` manifest field (the
  same URL as `repository`) was removed from all four crates.
- **Lines were lost when a log's format was detected part-way through.** The
  pipeline rebuilds its rule set when format detection locks onto a pack, and
  it used to rebuild the *collapser* as well. That threw away the run's
  accounting, its fold groups, and any multi-line block that was open at the
  time: the lines already read stopped being counted, and the contents of an
  open block never reached the output at all. A five-line log whose fourth
  line triggered detection came out as two lines, having silently dropped an
  error and its two stack frames. The collapser is no longer rebuilt. It is
  the only change needed, because the block detector's markers do not depend
  on the order of the rules they come from -- the literal markers are matched
  by one automaton over the whole set, and the pattern markers by "does any
  match" -- so reordering the rules yields the same detector, and that
  detector already carried every pack's markers.
- **A log line naming its own severity was not treated as one.** The
  `[db:error] Database connection failed: ...` shape -- a bracketed scope and
  level, as used by many application loggers -- matched no rule, so a real
  database failure was classified as ordinary output and the summary reported
  "0 errors" for a log that contains one. A new `scoped` rule pack covers it,
  and the real capture it came from now counts its Postgres `FATAL 08P01` as
  the error it is.
- **`--anonymize` corrupted every multi-byte character.** It worked on bytes
  and pushed each one as a separate character, so `café` came out as
  `cafâ‚¬Â©`. Text that was not ASCII was mangled in the one place that was
  supposed to be protecting it. It now works on characters.
- **`--anonymize` never redacted IPv4 addresses, IPv6 addresses or UUIDs.**
  The fakes for all three existed in the code and nothing ever reached them:
  there was no matcher for any of them. All three are redacted now. The IPv6
  matcher is deliberately strict -- it wants either the compressed `::` form
  or all eight groups -- because `09:14:02` is two colon-separated hex groups,
  and mistaking a timestamp for an address would rewrite every time in the log.
- **`--anonymize` left the original email local part in place.**
  `alice@example.com` came out as `aliceotter@example.invalid` -- half the
  real address, wearing a fake one. The whole address is replaced now.
- **`--anonymize` doubled the slash after a URL's host.**
  `https://host/api` came out as `https://example.invalid//seg...`.
- **A Rust panic and a failing test were classified as ordinary output.** The
  cargo pack had no rule for either, so the promise in `AGENTS.md` section 1 --
  that an error line is never lost -- did not apply to the failures that
  matter most. `thread '...' panicked at` is now an error, `test ... FAILED` is
  now an error, and `test result: FAILED.` is now an error.
- **A failing test suite summary was classified as ordinary output.** The rule
  that matched `test result: FAILED.` was declared *after* the rule that
  matched `test result: `, and first match wins -- so the general rule
  swallowed every failing summary. The order is fixed.
- **A level map could never fire for a rule whose level field is upper case.**
  The runtime path folded the captured value to lower case before matching it
  against the map, so a map written in upper case never matched and the rule
  silently fell back to its declared level. The OpenTelemetry pack shipped
  this way: its `severity` field is upper case by definition, so every
  OpenTelemetry line was classified `info` whatever its severity. The pack's
  own samples still passed, because the sample check did not fold case and
  the runtime path did -- which is why nothing caught it. Level maps are now
  matched case-insensitively, and the sample check and the runtime path agree.
- A level map with an invalid pattern failed silently at runtime and now
  fails at load time, naming the rule and its file.
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

[Unreleased]: https://github.com/adenijiridwan43/Hushlog/compare/v0.1.0-alpha.2...HEAD
[0.1.0-alpha.2]: https://github.com/adenijiridwan43/Hushlog/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/adenijiridwan43/Hushlog/releases/tag/v0.1.0-alpha.1