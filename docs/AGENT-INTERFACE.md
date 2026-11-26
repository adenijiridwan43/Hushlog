# Working with scripts and AI assistants

hushlog is useful at a terminal, and it is also useful as the first stage of a
script or an AI assistant's log input. Both use the same pipeline. Neither
throws anything away.

There are three ways to use it, and they solve different problems:

| | What it does | When to reach for it |
|---|---|---|
| `hushlog` on its own | Folds repeated lines into counted notices | The log is noisy, but all of it still matters |
| `hushlog --query '...' --format json` | Emits only the lines matching a predicate | You know what you are looking for |
| `hushlog --max-tokens <N>` | Aims for a shorter result without hiding errors | The log must fit an agent's context window |

If a line is not shown, it is still in the `--tee` file, and every fold is
counted in the summary. A smaller input for a program never means a lost log.

## What that looks like in practice

These numbers come from the real captures in `tests/fixtures/`, not estimates.

### A repetition storm

A rate-limit storm from a Node backend: 441 lines, ~400 of them repetitions of
six request shapes.

```sh
hushlog < tests/fixtures/real-relanflow-ratelimit-storm.log
```

```
hushlog: 441 lines -> 41 shown | 1 errors | 2 groups (400 lines folded)
```

441 lines become 43 — 41 readable lines and two notices. Repeat that capture
2,000 times, which is closer to a busy server, and 45 MB folds down to about
2 KB.

### Just the failures

A failing test run made from 2,000 copies of a real `cargo test` panic capture:
7.1 MB, 112,000 lines, roughly 1.8 million tokens.

```sh
hushlog --query 'level == error' --format csv < build.log
```

| Output | Size | Tokens | Smaller |
|---|---|---|---|
| the raw log | 7,146,000 bytes | ~1,786,500 | — |
| errors as CSV | 1,907,948 bytes | ~477,000 | **73%** |
| errors as JSON | 2,897,896 bytes | ~724,000 | **59%** |

CSV is smaller here because a JSON record carries the line twice — `raw` and
`plain` — plus field names. If your program can read CSV, use it. The remaining
27% is the stack traces, which are the part you actually wanted.

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

`=~` and `!~` take regular expressions. Match against `plain`: it is the text
the rules see, so `plain =~ "retry"` finds the same lines the retry rule fires
on.

```sh
# Errors only, as JSON records
hushlog --query 'level == error' --format json -- npm test

# Warnings as CSV
hushlog --query 'level == warning' --format csv -- cargo build

# Errors mentioning a retry, from stderr only
hushlog --query 'level == error && plain =~ "retry" && stream == "stderr"' --format json -- cargo build

# Everything that is not ordinary progress
hushlog --query 'level != info && level != progress' --format csv -- cargo build
```

`--query` implies JSON output unless you ask for CSV. `tty`, `github` and
`gitlab` are rejected with a query, because they cannot carry structured
records.

## In a pipeline

```sh
# Filtered output for a person, complete log on disk
hushlog --tee build.log -- cargo build

# Filtered records for a program, complete log on disk
hushlog --query 'level == error' --format csv --tee build.log -- cargo build
```

The tee file is written upstream of the pipeline, byte for byte, including
bytes that are not valid UTF-8. That is the difference between filtering a log
and losing one: the program sees less, and you still have all of it.

## With a token budget

```sh
# A shorter transcript for an agent, with the complete log kept on disk
hushlog --max-tokens 4000 --tee build.log -- cargo build

# The same budget as structured records
hushlog --max-tokens 4000 --format json -- cargo build
```

A budget is a display decision, not a filter. Tokens are estimated at four
bytes each. As the estimate closes on the cap, hushlog suppresses trace lines,
then info and progress, then warnings. Errors are never suppressed; a run may
exceed the cap because an error still has to be shown.

Every hidden line is counted. The summary satisfies
`lines_in == lines_out + lines_folded + lines_suppressed`, and JSON ends with a
`budget_summary` carrying the cap, estimate, suppressed tokens and suppressed
lines. `--tee` remains byte-exact, so a suppressed line is recoverable. Use
`--tee` whenever an agent needs to be able to follow up on something it did not
see.

## What hushlog deliberately does not do

- **It does not summarise.** There is no "here is what happened" mode. A summary
  that drops a line is a line you cannot get back. Use `--query` to select and
  `--explain` to understand a decision.
- **It does not guess.** An unrecognised line is `info`, not "probably fine".
  That is the safe default.
- **It does not fold errors.** Not unless you pass `--collapse-errors`, and
  then only because you asked.
