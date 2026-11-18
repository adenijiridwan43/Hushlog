# The agent interface

`hushlog` was built to sit in front of an AI agent the way it sits in front of
a CI log: read everything, keep what matters, drop nothing.

Two things make that work, and they are different tools for different jobs.

| | What it does | When to use it |
|---|---|---|
| `hushlog` alone | Collapses repeated lines into counted notices | The log is noisy but everything in it matters |
| `hushlog --query '...' --format json` | Emits only the lines matching a predicate | You know what you are looking for |

Neither is a filter that throws lines away. Everything that is not shown is
still in the `--tee` file, and every fold is counted in the end-of-run summary.

## The numbers, measured

All of these are measurements on the real captures in `tests/fixtures/`, not
estimates.

### Collapsing a repetition storm

The rate-limit storm, 441 lines of real access log with ~400 repetitions of
six request shapes:

```sh
hushlog < tests/fixtures/real-relanflow-ratelimit-storm.log
```

```
hushlog: 441 lines -> 41 shown | 1 errors | 2 groups (400 lines folded)
```

**441 lines become 43** — 41 shown and 2 fold notices. 90% smaller. The same
capture repeated 2000 times, which is what a busy server actually produces,
folds to 2 KB from 45 MB.

### Selecting what an agent needs

A failing test run, 2000 copies of the real `cargo test` panic capture: 7.1 MB,
112,000 lines, roughly **1.8 million tokens**.
```sh
hushlog --query 'level == error' --format csv < build.log
```

| Output | Size | Tokens | Smaller |
|---|---|---|---|
| the raw log | 7,146,000 bytes | ~1,786,500 | — |
| `--query 'level == error' --format csv` | 1,907,948 bytes | ~477,000 | **73%** |
| `--query 'level == error' --format json` | 2,897,896 bytes | ~724,000 | **59%** |

CSV is smaller than JSON here because a JSON record carries the line twice —
`raw` and `plain` — plus the field names. If the agent can parse CSV, it
should: same information, a third fewer tokens.

The remaining 27% is the stack traces, which are the part you actually wanted.
Everything else is gone.

## The query language

```
expr       := or_expr
or_expr    := and_expr ( "||" | "or" and_expr )*
and_expr   := not_expr ( "&&" | "and" not_expr )*
not_expr   := "!" not_expr | comparison
comparison := field ("==" | "!=" | "=~" | "!~" | ">" | "<" | ">=" | "<=") value
```

| Field | Holds |
|---|---|
| `level` | `error`, `warning`, `info`, `progress`, `trace` |
| `plain` | the line with ANSI stripped and volatile tokens masked |
| `raw` | the line exactly as it arrived |
| `rule_id` | the rule that fired, if any |
| `stream` | `stdout` or `stderr` |
| `no` | the line number |

`=~` and `!~` are regular expressions. `plain` is the one to match against: it
is the text a rule would have matched, so `plain =~ "retry"` finds the same
lines the retry rule fires on.

```sh
# Only the errors, as JSON records
hushlog --query 'level == error' --format json -- npm test

# Warnings, as CSV
hushlog --query 'level == warning' --format csv -- cargo build

# Errors mentioning a retry, from stderr only
hushlog --query 'level == error && plain =~ "retry" && stream == "stderr"' --format json -- cargo build

# Everything that is not ordinary progress
hushlog --query 'level != info && level != progress' --format csv -- cargo build
```

`--query` implies `--format json` unless `--format csv` is given.
`--format tty`, `github` and `gitlab` are rejected, because they cannot carry
structured records.

## Putting it in a pipeline

```sh
# The whole build, filtered, with the full log kept on disk
hushlog --tee build.log -- cargo build

# Only what an agent needs, and a complete copy of everything else
hushlog --query 'level == error' --format csv --tee build.log -- cargo build
```

The `--tee` file is written upstream of the pipeline, byte for byte, so it
holds the log exactly as the tool produced it — including bytes that are not
valid UTF-8. That is the difference between *filtering a log* and *losing a
log*: with `--tee`, the agent sees less and you still have all of it.

## What it does not do

- **It does not summarise.** There is no "here is what happened" mode. A
  summary that drops a line is a line you cannot get back, and hushlog's
  promise is that it never does that. Use `--query` to select, `--explain` to
  understand a decision.
- **It does not guess.** A line that no rule recognises is `info`, not
  "probably fine". That is the safe default, and `--query 'level == error'`
  is how you ask for the failures.
- **It does not fold errors.** Not unless you pass `--collapse-errors`, and
  then only because you asked.
