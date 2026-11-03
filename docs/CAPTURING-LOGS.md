# Capturing real logs — the fixture workflow

`AGENTS.md` section 5.3: **fixtures are real logs, not invented strings.** The
whole value of hushlog rests on whether it handles what tools actually print.
Until a real log has been through the rules, the folding rules are unproven.

This document is the procedure for turning a real, noisy process into a fixture
and hardening the rules against it.

---

## 1. The chicken-and-egg question, answered

You asked: *do we build hushlog first, or point it at the project first?*

**Build first. It is one command and takes a couple of minutes.** hushlog is a
single binary; there is no installer to run and no service to start. The order is:

```
1. Build the binary         cargo build --release        (~1–2 min)
2. Capture the noisy log    <your project> 2>&1 | tee raw.log
3. Point hushlog at it      hushlog --tee raw.log < raw.log
4. Look at what it did      the summary line + --explain
5. Turn the log into a fixture, fix any rule that misfired
```

You do **not** need a release, a package manager, or the network. Step 1 produces
`target/release/hushlog` and you run that path directly.

---

## 2. Build the binary

```sh
git clone https://github.com/adenijiridwan43/Hushlog
cd Hushlog
cargo build --release
```

The binary is at:

- Linux/macOS: `./target/release/hushlog`
- Windows: `.\target\release\hushlog.exe`

Copy it somewhere on `PATH` if you like, or just call it by full path:

```sh
# Linux/macOS
alias hushlog="$PWD/target/release/hushlog"

# PowerShell
$env:Path += ";$PWD\target\release"
```

Sanity check:

```sh
printf 'error: boom\nwarning: meh\n' | hushlog
hushlog --version
```

---

## 3. Capturing the noisy process

Your case is a dev server (npm) throwing a high-rate storm of Postgres and
network errors — roughly every 0.3 ms. That is the ideal fixture: high volume,
heavy repetition, and real error lines that **must all survive**.

### 3.1 It is a stream, not a file — use a pipe

A dev server logs to stdout/stderr continuously. You do not need to find a log
file; you capture the stream. Three ways, in order of preference:

**A. Pipe the dev server straight into hushlog (live, no file).**

```sh
# Linux/macOS — merge stderr into stdout, filter live
npm run dev 2>&1 | hushlog

# Windows PowerShell
npm run dev 2>&1 | .\target\release\hushlog.exe
```

**B. Keep the full raw log while filtering (the lossless path).**

```sh
npm run dev 2>&1 | hushlog --tee raw-dev.log
```

`--tee` writes every input byte to `raw-dev.log` **upstream of the filter**, so
even a log with invalid UTF-8, ANSI colour, or a truncated line is preserved
byte-for-byte. This is the copy the fixture comes from.

**C. Wrap the command (hushlog spawns it).**

```sh
hushlog --tee raw-dev.log -- npm run dev
```

Wrap mode reads the child's stdout and stderr as two tagged streams and
propagates the exit code. Use this when you want hushlog to own the process.

> **Watch out for the storm rate.** At one error per 0.3 ms that is ~3000
> lines/second. Let it run for 10–20 seconds, not minutes — 20 s is ~60,000
> lines, which is already a strong fixture and keeps the file reviewable.

### 3.2 If the process is already running elsewhere

If the dev server runs in another terminal, a container, or a remote box:

```sh
# Docker
docker logs -f <container> 2>&1 | hushlog --tee raw-dev.log

# Kubernetes
kubectl logs -f <pod> 2>&1 | hushlog --tee raw-dev.log

# A file that is still being written
tail -f app.log | hushlog --tee raw-dev.log
```

### 3.3 Redact before it leaves your machine

A captured Postgres/network log can contain host names, IPs, connection strings,
and credentials. Two protections:

- `--anonymize` replaces IPs, UUIDs, MACs, emails, URLs and card numbers in the
  **rendered** output with consistent fake values. The `--tee` file is always
  raw, so keep it local.
- Manually scan `raw-dev.log` before committing any of it as a fixture. If it
  contains a password or a customer name, do not commit it — trim the capture
  down to the parts that exercise the rules.

---

## 4. Reading what hushlog did

The summary line is the ground truth. It reports exact counts and the tee path:

```
hushlog: 61,204 lines -> 38 shown | 4,102 errors | 12 warning groups (57,000 lines folded) | full log: raw-dev.log
```

- **errors** must equal the number of error lines in the raw capture. If it does
  not, classify is wrong.
- **lines shown** plus **lines folded** plus block overhead must equal the input
  total. Nothing may be unaccounted for (`AGENTS.md` section 1).
- The tee file must be byte-identical to the input:
  ```sh
  cmp raw-dev.log raw-dev.log   # trivially true; compare against a second capture
  ```

To see why any line was folded or kept:

```sh
hushlog --explain --format plain < raw-dev.log 2> explain.txt
head explain.txt
```

To pull just the errors as records (useful and agent-readable):

```sh
hushlog --query 'level == error' --format json < raw-dev.log > errors.json
```

---

## 5. Turning the capture into a fixture

Fixtures live in `tests/fixtures/` and are captured **verbatim** (`.gitattributes`
marks them `-text` so line endings are not normalised).

1. Copy the capture in under the real name — **no `synthetic-` prefix**:

   ```
   tests/fixtures/postgres-error-storm.log
   tests/fixtures/npm-devserver-network.log
   ```

2. If it is large, trim it — but by **removing whole lines from the middle**,
   never by editing a line. The fixture must stay something the tool really
   emitted. Aim for a few hundred lines that carry the repetition and a handful
   of distinct errors.

3. Add a case to the hushlog integration tests that runs the fixture through the
   real binary and asserts:
   - the summary counts are what you measured by hand, and
   - every error line in the fixture appears in the output
     (the `error_never_lost` invariant, extended to a real log).

4. Run the fixture through the rules and fix any rule that misfires — a real
   log with a Postgres error the `cargo`/`npm` packs miss becomes a new rule.

---

## 6. Why the storm is the perfect test

A high-rate repetition storm exercises the exact thing this product exists to do:

- **Collapse**: thousands of near-identical errors should fold into counted
  groups — but never to zero. Repeated **errors** are not collapsed by default
  (`AGENTS.md` section 1.1), so the storm proves the collapser respects the
  policy under load.
- **Bounded memory**: at 3000 lines/second the LRU must hold steady. Watch the
  process's memory while it runs; it must not climb (`AGENTS.md` section 7).
- **Fingerprinting**: `Retry 3/10 for pod abc-123` and `Retry 4/10 for pod
  xyz-789` must share a fingerprint after masking. Real Postgres errors carry
  connection ids and ports — exactly the volatile tokens the normalizer masks.
- **The invariant**: every error line must survive. This is the one test that
  protects the product's reputation, and your storm is the strongest input we
  have for it.

---

## 7. Checklist

- [ ] `cargo build --release` succeeds
- [ ] Capture a 10–20 s window: `npm run dev 2>&1 | hushlog --tee raw-dev.log`
- [ ] Scan `raw-dev.log` for secrets; redact or trim if needed
- [ ] Note the summary counts by hand
- [ ] `hushlog --query 'level == error' --format json < raw-dev.log > errors.json`
- [ ] Compare the error count in `errors.json` with the raw capture
- [ ] Trim to a committed fixture under `tests/fixtures/`
- [ ] Add the integration test and fix any rule that misfired
- [ ] Commit the fixture with the raw counts written into the test
