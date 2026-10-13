# Security Policy

## Supported versions

Hushlog is pre-alpha. No version is supported yet.

## Reporting a vulnerability

Please report security issues privately rather than opening a public issue.
Use GitHub's "Report a vulnerability" button on the Security tab, which opens a
private advisory.

## What counts as a security issue here

Hushlog's entire value proposition is that you can trust its output. That makes
some things security-relevant that would be ordinary bugs elsewhere:

- **Hiding an error line.** Any input that causes a line classified `Level::Error`
  to be omitted from output. This is the failure mode the whole design guards
  against; see [`AGENTS.md` section 1.1](AGENTS.md#11-errors-are-never-collapsed).
- **Unbounded memory growth.** The collapser is supposed to hold a bounded LRU.
  Input that makes it grow without limit violates the memory guarantee.
- **Lossy `--tee`.** `--tee` promises a verbatim copy. Anything that alters or
  truncates that file is a bug worth reporting.
- **Exit code tampering.** Wrap mode must propagate the child's exit code.
  Anything that masks a failure exit is serious.

## What is not a security issue

- Crashing on malformed input.
- Incorrect highlighting or classification that is not an omission of an error.
- Performance below target.

Those are bugs. Report them normally.