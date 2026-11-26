# JSON contract (v1)

`--format json` writes newline-delimited JSON: one object per event, in the
order the pipeline produced it. A consumer can read the first record before the
last input line has arrived.

Every record has:

| Field | Meaning |
|---|---|
| `type` | `emit`, `folded`, `block_start`, `block_end`, `summary` or `budget_summary` |
| `schema_version` | `"1"` on every record, including block and summary records |
| `sequence` | 1-based position in this run's record stream; never repeats |

Field names and types change **only** with a `schema_version` bump. Additions
within v1 are additive: an older consumer ignores a field it does not know, and
the v0 keys stay present as deprecated aliases.

## Line records (`emit`, `folded`)

| Field | v1 | Legacy alias |
|---|---|---|
| text a reader sees | `message` | `plain` |
| exact input text | `raw` | — |
| originating stream | `stream` | — |
| input line number | `no` | — |
| severity | `level` | — |
| stable rule name | `rule` | `rule_id` (numeric index) |
| is this a fold? | `folded` | — |
| occurrences represented | `fold_count` | `count` (on `folded`) |
| group fingerprint, 16 lowercase hex digits | `fingerprint` | `fp` (numeric, on `folded`) |
| first occurrence | `sample` | — |

`rule` is resolved from the classifier at render time. `RuleId` is an index into
the loaded rule set, so it is not stable across rule-pack changes or format
detection; `rule` is the name the rules file declares, such as `cargo.error`.
A line with no matching rule has `rule: null`.

`fingerprint` is `null` on `emit`: a line has not entered a fold group yet.
On `folded` it is the group's fingerprint formatted as exactly 16 lowercase
hex digits. `fold_count` is 1 on `emit` and the group's running count on
`folded`. The count in an individual `folded` record can be smaller than the
final total; the end-of-run `summary` is authoritative.

## Block and summary records

`block_start` / `block_end` carry `block` (the block id) plus the common
fields. `summary` carries `lines_in`, `lines_out`, `errors`, `groups`,
`lines_folded`, `lines_suppressed` and `tee_path` exactly as the human-facing
summary does. In a budgeted run
`lines_in == lines_out + lines_folded + lines_suppressed`; without a budget the
new count is zero.

## Budget records

`budget_summary` is emitted at the end of a run started with `--max-tokens`.
It carries `max_tokens`, `token_estimate`, `tokens_suppressed`,
`lines_suppressed` and `tee_path`. It exists because a budgeted run hides some
lines: the exact counts and the path to the byte-exact copy make that hiding
recoverable (`AGENTS.md` section 1). See the flag's section in the README for
the suppression order.

## Compatibility policy

- A new optional field may be added within v1 without a version bump.
- A renamed, removed or retyped field requires `schema_version: "2"` and a new
  record shape; v1 records must stay parseable for at least one release.
- Legacy aliases (`rule_id`, `count`, `fp`, `plain`) are deprecated, not removed,
  in v1. New consumers should prefer the v1 names.
- `json` and `csv` are record streams: every fold event is a record, and
  nothing is merged. A single fold notice on screen is a display decision.
