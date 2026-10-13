# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Workspace with four crates: `hushlog-core`, `hushlog-rules`, `hushlog-render`,
  `hushlog-cli`.
- Pipeline contracts in `hushlog-core`: `Line`, `Level`, `RuleId`, `Classified`,
  `Classifier`, `Event`, `Collapser`, `Renderer`, `Summary`, `CollapsePolicy`.
- `CollapsePolicy` with `collapse_errors` defaulting to `false`.
- Property test asserting every line classified `Level::Error` reaches the
  output, written before any collapse logic existed.
- CI: fmt, clippy (`-D warnings`), tests on Linux/macOS/Windows, MSRV 1.85 check,
  doc build, plus `cargo-deny` and `cargo-audit`.
- Workspace lint policy from the
  [rust-skills](https://github.com/actionbook/rust-skills) defaults: edition
  2024, MSRV 1.85, `unsafe_code = "warn"`, clippy `pedantic`.
- Three synthetic log fixtures and a README describing the real fixtures still
  to be collected.

### Notes

- Nothing filters yet. The pipeline is contracts and scaffolding only.
- Crates are prefixed `hushlog-` because `hush-core` and `hush-cli` are already
  published on crates.io by an unrelated project.

[Unreleased]: https://github.com/<you>/hushlog/compare/v0.1.0...HEAD