# Capturing a real log

Every useful bug hushlog has found came from a log a real tool printed. A
synthetic log only proves that the code agrees with itself; a real capture
tells you whether the rules are right. That is why the logs in
`tests/fixtures/` are the specification, and why they are captured verbatim.

This is the workflow for turning a noisy process into a fixture.

---

## The short version

1. Build the binary with `cargo build --release`.
2. Capture the noisy process with `--tee`, so you keep every byte.
3. Read the summary, then use `--explain` on anything that looks wrong.
4. Trim the capture by removing whole lines, and put it in `tests/fixtures/`.
5. Add a test that checks the counts and proves every error line survived.

No release, installer or network access is needed. The binary at
`target/release/hushlog` is all you need.

---

## Build first

```sh
git clone https://github.com/adenijiridwan43/Hushlog
cd Hushlog
cargo build --release
```

The binary is `./target/release/hushlog` (or `.\target\release\hushlog.exe` on
Windows). Put it on `PATH` if you like, or call it by full path:

```sh
# Linux/macOS
alias hushlog="$PWD/target/release/hushlog"

# PowerShell
$env:Path += ";$PWD\target\release"
```

A quick sanity check:

```sh
printf 'error: boom\nwarning: meh\n' | hushlog
hushlog --version
```

---

## Capture the noise

A dev server under load is an ideal capture: high volume, heavy repetition and
real errors that must all survive. There are three ways to catch the stream.

**A. Filter it live and keep the raw bytes.**

```sh
npm run dev 2>&1 | hushlog --tee raw-dev.log
```

`--tee` writes every input byte to `raw-dev.log` before filtering, so ANSI
colour, invalid UTF-8 and a line truncated by a killed job are preserved.

**B. Let hushlog run the command.**

```sh
hushlog --tee raw-dev.log -- npm run dev
```

Wrap mode reads stdout and stderr as two tagged streams and propagates the
child's exit code. Use it when you want hushlog to own the process.

**C. Follow a log that already exists.**

```sh
docker logs -f <container> 2>&1 | hushlog --tee raw-dev.log
kubectl logs -f <pod> 2>&1 | hushlog --tee raw-dev.log
tail -f app.log | hushlog --tee raw-dev.log
```

On Windows, do not pipe a live dev server through `cmd` — `cmd` strips ANSI
before hushlog sees it. Redirect to a file first, then filter the file.

A storm running at a few thousand lines a second only needs 10–20 seconds of
capture. That is already tens of thousands of lines, and it stays reviewable.

---

## Keep it safe

A real capture can contain host names, IPs, connection strings and credentials.

- `--anonymize` replaces IPs, UUIDs, MAC addresses, emails, URLs and card
  numbers in the **displayed output**. The tee file is never anonymised, so
  keep it local.
- Read the raw file before committing any of it. If it contains a password or
  a customer name, trim those lines out. When in doubt, do not commit it.

---

## Check what hushlog did

The summary line is the ground truth:

```
hushlog: 61,204 lines -> 38 shown | 4,102 errors | 12 warning groups (57,000 lines folded) | full log: raw-dev.log
```

- The **error** count must equal the number of error lines in the raw capture.
  If it does not, a rule is wrong.
- Shown lines plus folded lines must account for the input. Nothing may be
  unaccounted for.
- The tee file must contain the input byte for byte. Prove it with:

```sh
hushlog --tee copy.log < raw-dev.log > /dev/null
cmp raw-dev.log copy.log
```

To see why a line was kept or folded:

```sh
hushlog --explain --format plain < raw-dev.log 2> explain.txt
```

To pull just the errors into a machine-readable file:

```sh
hushlog --query 'level == error' --format json < raw-dev.log > errors.json
```

---

## Turn the capture into a fixture

Fixtures live in `tests/fixtures/` and are stored verbatim; `.gitattributes`
marks them `-text` so line endings are not normalised.

1. Copy the capture in under its real name, without a `synthetic-` prefix:
   `tests/fixtures/postgres-error-storm.log`, for example.
2. If it is large, trim it by **removing whole lines from the middle**. Never
   reword a line: a reworded line tests the rewording, not the tool. A few
   hundred lines with repetition and a handful of distinct errors is plenty.
3. Add an integration test that runs the fixture through the real binary and
   asserts the measured counts and that every error line appears in the output.
4. Fix any rule that misfired. A real Postgres error the packs miss is a new
   rule, not a reason to edit the fixture.

---

## Why a storm is worth keeping

- **Folding:** thousands of near-identical lines should become counted groups,
  but repeated errors must still be emitted. That is the product's most
  important promise.
- **Fingerprints:** `Retry 3/10 for pod abc-123` and `Retry 4/10 for pod
  xyz-789` must share a fingerprint after masking. Real errors carry exactly
  those volatile IDs and ports.
- **Bounded memory:** watch the process while it runs. Memory must stay flat as
  the input grows.
- **The invariant:** every error line survives. This is the test that protects
  the product's reputation, and a real storm is its strongest input.

---

## Checklist

- [ ] `cargo build --release` succeeds
- [ ] Capture 10–20 seconds with `npm run dev 2>&1 | hushlog --tee raw-dev.log`
- [ ] Read the raw capture for secrets; trim or redact if needed
- [ ] Write down the summary counts by hand
- [ ] Verify the tee file is byte-identical
- [ ] Extract the errors with `--query` and compare the count
- [ ] Trim to a fixture under `tests/fixtures/`
- [ ] Add the integration test, and fix any rule that misfired
