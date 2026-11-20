# `pin-utils`: the crate you never asked for

*A field guide to the unmaintained Rust dependency that arrives through someone
else's default features — and what to do when it does.*

---

## The short version

`pin-utils` is a small crate of macros for pinning values in Rust — about 150
lines of library code. It was useful between 2019 and 2021. Since Rust 1.68 it
has had a standard-library replacement (`core::pin::pin!`) and a safe one
(`pin-project-lite`). Its upstream repository was archived on 2026-10-01, and
the RustSec advisory against it — **RUSTSEC-2026-0441**, filed the same day
and issued on 2026-11-05 — records the fact that there is no patched
version, because there is nothing left to patch.

None of that is the interesting part.

The interesting part is that **`pin-utils` is almost never a dependency
anyone chooses.** It arrives the way most unmaintained crates arrive: as a
default-feature side effect of a dependency nobody was thinking about, three
or four levels down a tree that someone else built.

In this repository — a small Rust CLI that filters logs — `pin-utils` entered
through a PTY library, which got it from a Unix syscall wrapper, which got it
from a feature called `aio` that nobody enabled on purpose.

This article is about that mechanism: how a deprecated crate breaks into your
project, how to find out whether it actually matters to you, and the three
ways out.

---

## 1. What `pin-utils` actually is

### The problem it solved

Rust's `Pin` type guarantees that a value will not be moved after it has been
given a stable address. That's the foundation of `async` — a future must not
move while it is being awaited.

But `Pin<&mut T>` only pins the *outer* struct. To reach a field, you need
*structural pinning*: a projection from `Pin<&mut Outer>` to
`Pin<&mut Inner>`.

Before Rust 1.68, that projection was genuinely unsafe to write by hand.
`pin-utils` provided the macro that did it:

```rust
use pin_utils::unsafe_pinned;
use std::marker::Unpin;
use std::pin::Pin;

struct Foo<T> {
    field: T,
}

impl<T> Foo<T> {
    unsafe_pinned!(field: T);

    fn baz(mut self: Pin<&mut Self>) {
        let _: Pin<&mut T> = self.field(); // Pinned reference to the field
    }
}

impl<T: Unpin> Unpin for Foo<T> {} // Conditional Unpin impl
```

The three macros were:

| Macro | Purpose | Status |
|---|---|---|
| `pin_mut!(x)` | Pins a value on the stack | **Soft-deprecated** — use `core::pin::pin!` (Rust 1.68+) |
| `unsafe_pinned!(field: T)` | Projects `Pin<&mut Self>` → `Pin<&mut Field>` | **Deprecated** — use `pin-project` |
| `unsafe_unpinned!(field: T)` | Projects to a plain `&mut Field` | **Deprecated** — use `pin-project` |

The crate's own documentation says so, at the top, in the first three lines:

```rust
//! **Note:** Since Rust 1.68, all APIs in this crate are now soft-deprecated or deprecated:
//!
//! - `pin_utils::pin_mut!` is soft-deprecated in favor of [`pin!` macro in the standard library]
//! - `pin_utils::{unsafe_pinned,unsafe_unpin}` are **deprecated** in favor of safe alternatives:
//!   [pin-project], [pin-project-lite]
```

### The replacements

**For stack pinning**, the standard library took over completely:

```rust
// Before — pin-utils
let mut foo = Foo { /* ... */ };
pin_mut!(foo);
let _: Pin<&mut Foo> = foo;

// After — Rust 1.68+, std
let mut foo = Foo { /* ... */ };
let foo = std::pin::pin!(foo);
let _: Pin<&mut Foo> = foo;
```

**For structural pinning**, `pin-project-lite` is the safe replacement. The
shape changes — you get a projection type instead of one method per field —
but the guarantees are the same or better:

```rust
// Before — pin-utils, unsafe, and you must justify the safety yourself
use pin_utils::unsafe_pinned;

struct Foo<T> { field: T }

impl<T> Foo<T> {
    unsafe_pinned!(field: T);
}

// After — pin-project-lite, safe, and the macro checks the invariants
use std::pin::Pin;
use pin_project_lite::pin_project;

pin_project! {
    struct Foo<T> {
        #[pin]
        field: T,
    }
}

impl<T> Foo<T> {
    fn baz(self: Pin<&mut Self>) {
        let this = self.project();
        let _: Pin<&mut T> = this.field; // Pinned reference to the field
    }
}
```

The difference in *kind* is the point. `unsafe_pinned!` asks you to promise
three things by hand:

- if the struct implements `Drop`, `drop` must not move the field;
- if the struct implements `Unpin`, it must do so *conditionally* — only when
  the field is `Unpin`;
- the struct must not be `#[repr(packed)]`.

`pin-project-lite` checks all three at compile time. It is not just a
migration; it is the same functionality with the footguns removed.

One caveat worth knowing before you reach for the lite version: it has **no
support for tuple structs or tuple variants**, and no support for a custom
`Unpin` implementation (`UnsafeUnpin`). If you need either, use the full
`pin-project`. The lite version also gives no useful error messages — if the
macro rejects your code, re-run the same input through `pin-project` to get
a diagnosis.

### Why it was archived

`pin-utils` had exactly one job, and the job was taken twice — once by the
standard library and once by `pin-project`. With `core::pin::pin!` covering
stack pinning and `pin-project` covering projection, there was nothing left
for it to do. A crate with no remaining purpose cannot be maintained, and an
unmaintained crate that other people still depend on is a liability.

The advisory is filed under RustSec's **INFO / Unmaintained** classification,
and its remediation is not a version bump but a rewrite of the *consumer*:

> For local pinning, replace `pin_mut!` with `core::pin::pin!`, available
> since Rust 1.68. For pin projection, replace `unsafe_pinned!` and
> `unsafe_unpin!` with `pin-project` or `pin-project-lite`.
>
> — RUSTSEC-2026-0441

That is the shape of most unmaintained advisories: there is no fix, because
the fix is to stop depending on it.

None of this makes `pin-utils` dangerous. It makes it **abandoned** — which
is a different kind of problem. A vulnerability means someone can attack you.
An abandoned transitive dependency means nobody is fixing the thing that
depends on it, and you may not find out until it is too late to do anything
cheap.

---

## 2. How it gets into a project you didn't write

This is the section that matters, and it is the part that is specific to Rust
because of how Cargo resolves features.

### The chain in this repository

Run `cargo tree -i pin-utils` in the hushlog workspace and you get:

```
pin-utils v0.1.1
└── nix v0.25.1
    └── portable-pty v0.8.1
        └── hushlog v0.1.0-alpha.1
```

Read that bottom-up. hushlog depends on `portable-pty` — for one thing: the
opt-in `--pty` mode that gives a child process a real terminal. `portable-pty`
depends on `nix`, a set of safe wrappers over Unix syscalls. And `nix`
depends on `pin-utils`.

**hushlog's `Cargo.toml` never mentions `pin-utils`.** Neither does
`portable-pty`'s. The dependency is created in `nix`'s manifest, and it is
*optional* there:

```toml
[dependencies.pin-utils]
version = "0.1.0"
optional = true
```

Optional dependencies only exist in the graph if some feature enables them.
And here is the line that does it:

```toml
[features]
aio = ["pin-utils"]
default = [
    "acct",
    "aio",      # <-- this one
    "dir",
    "env",
    # ... 30 more
]
```

`nix`'s `default` feature list includes `aio`, and `aio` enables
`pin-utils`. So the chain is:

```
hushlog ──default──> portable-pty ──default──> nix ──default──> aio ──> pin-utils
```

Cargo confirms this precisely. `cargo tree -i pin-utils -e features` prints
the path with every feature edge named:

```
pin-utils v0.1.1
└── pin-utils feature "default"
    └── nix v0.25.1
        ├── nix feature "acct"
        │   └── nix feature "default"
        │       └── portable-pty feature "default"
        │           └── hushlog feature "default" (command-line)
        ├── nix feature "aio"
        │   └── nix feature "default" (*)
        ├── nix feature "default" (*)
        ...
```

Every edge in that tree is a **default** feature. Not one line of code in
this repository asked for `pin-utils`, `nix`'s `aio` feature, or even `nix` at
all. They all came along for free.

### The three rules that make this possible

**Rule 1: default features are additive, not exclusive.**
`portable-pty` declares `nix = { version = "0.25", features = ["term"] }`. That
looks restrained — it asks for exactly one feature. But it does not say
`default-features = false`, so `nix`'s entire default set comes along too,
`aio` included. The `term` feature is the tip of the iceberg; the default set
is the iceberg.

**Rule 2: features unify across the whole graph.**
If *any* crate in the dependency graph enables `nix/aio`, every crate that
links `nix` gets it. There is no per-dependency scoping. So even if hushlog
depended on `nix` directly with `default-features = false`, it would gain
nothing — `portable-pty` would still pull `aio` in for both of them. Feature
resolution is a union across the graph, not a negotiation per consumer.

**Rule 3: optional dependencies are not optional in practice.**
`pin-utils` is `optional = true` in `nix`. That is a statement about `nix`'s
API surface — *you* may choose not to use `aio`. It says nothing about what
ends up in your build. The only thing that decides is whether some feature
anywhere in the graph turns it on.

### The uncomfortable corollary

You cannot fix this from your own `Cargo.toml`.

The instinct is to add the dependency yourself and turn the feature off:

```toml
# This does not work.
[dependencies]
nix = { version = "0.25", default-features = false }
```

It does not work, and the reason is Rule 2. Your `nix` and `portable-pty`'s
`nix` are the *same* `nix` in the resolver's view. Features unify. Adding
`nix` with `default-features = false` on your side does not subtract the
default set that `portable-pty`'s side requested — it only ever adds.

This was tested, not assumed, while writing this article. The `pin-utils`
node was still in the tree after adding that line and re-resolving.

**The dependency belongs to the crate that asked for it.** If `portable-pty`
wants `nix`'s defaults, you get `nix`'s defaults. Your options are to change
`portable-pty`, change `nix`, or accept it.

---

## 3. When it matters and when it does not

Before deciding what to do, work out whether the crate is actually *in* your
binary. "It is in the lockfile" and "it is in the shipped artifact" are very
different questions.

### Is it compiled into the binary?

Cargo's resolver is target-agnostic by default: `cargo tree -i pin-utils`
shows the crate whether or not your build target can use it. So the tree
answers "is it in the graph", not "is it in the binary".

For that you need the target, and then you need to look at the crate itself:

```sh
# What does the tree look like for the target you ship?
cargo tree --target x86_64-unknown-linux-musl -i pin-utils
cargo tree --target x86_64-pc-windows-msvc -i pin-utils
```

For hushlog the answer is more interesting than "it is absent". `pin-utils`
shows up in *both* trees — but `nix`, the crate that owns it, is
`#![cfg(unix)]`:

```rust
// nix 0.25.1, src/lib.rs
#![cfg(unix)]
```

The entire crate compiles to nothing on Windows. `pin-utils` is therefore in
the graph for every target, and in the *binary* only on Unix.

This is why the target-specific tree is worth running: it shows you which
crate to interrogate next. The answer was not "pin-utils is missing on
Windows"; it was "look at `nix`, and `nix` is Unix-only, and `nix`'s AIO
module is the only thing in it that touches `pin-utils` at all".

### Is it reachable at runtime?

This is the question that decides between "fix it" and "document it". A crate
can be compiled in, never called, and still be a liability — but the severity
is very different from a crate on the hot path.

`pin-utils` is used by `nix` in exactly one module: `src/sys/aio.rs`, the
Linux AIO (asynchronous I/O) bindings.

```rust
pub struct AioFsync {
    aiocb: AioCb,
    _pin: PhantomPinned,
}

impl AioFsync {
    unsafe_pinned!(aiocb: AioCb);
    // ...
}
```

That is the whole of it: six `unsafe_pinned!` invocations in one file,
guarding the pointer inside a Linux AIO control block, so that the block is
not moved while the kernel holds a pointer to it.

hushlog does not use Linux AIO. It uses `portable-pty` for one thing — the
opt-in `--pty` mode — and `nix`'s AIO wrappers are not among the things
`portable-pty` calls either. The `aio` feature is enabled by default and
used by nothing.

### The three-question test

Before deciding what to do about an unmaintained transitive dependency, ask:

1. **Is it in the tree?** (`cargo tree -i <crate>`) If not, stop here.
2. **Is it in the shipped binary?** Check the target, then check the owning
   crate — Cargo's tree is target-agnostic, so the tree alone will not tell
   you. If the owner is `#![cfg(unix)]` or the dependency is a dev-dependency,
   it is a lockfile entry, not a supply-chain risk.
3. **Is it reachable from code you actually run?** If it is behind an
   opt-in flag or a code path nothing takes, its risk is bounded and
   *documentable*.

For hushlog the answers were: yes (in the tree for every target), yes in the
Linux binary, and no — `pin-utils` is used only by `nix`'s Linux AIO module,
which nothing in the project calls. That answer is what justified
documentation instead of surgery.

Note how much work question 3 is. Questions 1 and 2 are two commands.
Question 3 required reading the *source* of a crate two levels down the
tree, to find the one file in it that touches the deprecated crate at all.
There is no tool that answers it. That is the real cost of a leaked
dependency, and it is worth naming before you decide whether to pay it.

---

## 4. The three ways out

When the answer to question three is "yes, it is on your hot path", you have
three options. They are worth knowing in order, because they get harder as
you go.

### Option A: upgrade the dependency that pulls it in

The obvious move. `portable-pty` 0.9 moves to `nix` 0.28, whose `default`
feature list is empty:

```toml
# nix 0.28.0
[features]
aio = ["pin-utils"]
default = []      # <-- pin-utils is now opt-in only
```

`nix` 0.28 still *offers* `aio` and still lists `pin-utils` — but no longer
enables it by default, so the crate leaves the graph entirely.

Measured, the upgrade removes eight crates and adds two:

```
removed: ioctl-rs, memoffset, pin-utils, serial, serial-core,
         serial-unix, serial-windows, termios
added:   cfg_aliases, serial2
```

Both unmaintained crates go — `pin-utils`, and `serial` (RUSTSEC-2017-0008),
replaced by the maintained `serial2`. The lockfile shrinks from 138 crates to
132.

**This is the right answer, and it is also where the story gets interesting,
because it broke something.**

`portable-pty` 0.9.0 added a flag to the `CreatePseudoConsole` call:

```rust
// portable-pty 0.9.0, src/win/psuedocon.rs
pub const PSUEDOCONSOLE_INHERIT_CURSOR: DWORD = 0x1;

let result = unsafe {
    (CONPTY.CreatePseudoConsole)(
        size,
        input.as_raw_handle() as _,
        output.as_raw_handle() as _,
        PSUEDOCONSOLE_INHERIT_CURSOR        // <-- new in 0.9.0
            | PSEUDOCONSOLE_RESIZE_QUIRK
            | PSEUDOCONSOLE_WIN32_INPUT_MODE,
        &mut con,
    )
};
```

That flag makes the pseudoconsole inherit the cursor position from the
parent console. On a machine with no interactive console to inherit from, the
call blocks forever.

The symptom was not subtle. A `--pty` run produced **no output at all** and
never exited:

```
$ hushlog --pty -- cmd /c "echo hello"
(hangs indefinitely)
```

And the two PTY integration tests went from 8.5 seconds to 1056 seconds
before failing.

**How it was isolated.** The upstream source diff between 0.8.1 and 0.9.0
shows that on the Windows side, that one flag is the *only* change:

```sh
git diff --no-index \
  ~/.cargo/registry/src/*/portable-pty-0.8.1/src \
  ~/.cargo/registry/src/*/portable-pty-0.9.0/src
```

```
 .../cmdbuilder.rs"                                 | 116 ++++++++++++++++-----
 .../lib.rs"                                        |  19 +++-
 .../serial.rs"                                     |  60 ++++++-----
 .../unix.rs"                                        |  48 ++++++++-
 .../win/psuedocon.rs"                              |   5 +-
 5 files changed, 188 insertions(+), 60 deletions(-)
```

Then the confirmation: vendor 0.9.0 locally with `[patch.crates-io]`,
remove **only** that flag, rebuild, and the run completes normally with
byte-identical output. One variable, controlled.

Upstream `main` still carries the flag (commit `9d834586`, "add
PSUEDOCONSOLE_INHERIT_CURSOR as a flag", February 2024), and 0.9.0 is the
newest published version — so there is no upstream release to take instead.

**The decision.** A broken opt-in feature on Windows is a worse outcome than
two unmaintained crates that the default path never reaches. AGENTS.md §8
makes Windows a first-class target and `--pty` is a shipped feature. So the
upgrade was reverted, and the reasoning was written into `deny.toml` where the
next person would look for it.

### Option B: patch the crate that owns the feature

When the upstream that owns the leak will not move — and the leak is a
default-feature choice rather than a code bug — you can patch the owning
crate's manifest yourself.

`nix`'s problem is one line in its `Cargo.toml`:

```toml
[features]
aio = ["pin-utils"]
default = [
    "acct",
    "aio",     # <-- remove this one entry
    "dir",
    # ...
]
```

Vendor `nix`, delete that one entry, and point Cargo at the copy:

```toml
# Cargo.toml
[patch.crates-io]
nix = { path = "vendor/nix" }
```

**This works, and it was verified.** The result:

```
$ cargo tree -p hushlog -i pin-utils
error: package ID specification `pin-utils` did not match any packages
```

The crate is gone from the graph entirely. The project builds, the two PTY
integration tests pass in 12.9 seconds — because `portable-pty` never calls
into `nix`'s AIO wrappers, so removing the feature removes dead code rather
than functionality — and the full suite passes: **312 tests, 0 failures.**

Two practical notes, both learned the hard way:

- **`[patch.crates-io]`, not a workspace dependency.** Declaring
  `nix = { version = "0.25", default-features = false }` in
  `[workspace.dependencies]` does nothing (Rule 2, above). `[patch.crates-io]`
  is the mechanism that actually substitutes a different copy of the crate.
- **Re-resolve the lockfile.** `cargo update -p nix` alone will not drop the
  crate, because the lockfile still records the optional dependency as
  present. `cargo generate-lockfile` (or deleting `Cargo.lock`) is what
  actually rebuilds the graph.

The cost is real: you now own a fork of `nix`'s manifest, and you have to
repeat it whenever `nix` is updated. That is a fair trade for a crate that
is unreachable, and a bad trade for one that is not.

**This is the route hushlog took on 2026-11-19.** It was found and verified
while writing this article, after Option A had already been rejected. The
vendored copy lives in `vendor/nix` (see its `PATCH-README.md`), `deny.toml`
documents the decision, and the external review's suggestion — a headless
Windows PTY smoke test with a hard timeout, added to
`crates/hushlog/tests/pty_mode.rs` — guards the class of regression. The
alternative left standing is the accepted-with-reason acceptance of the
`serial` advisory.

### Option C: accept it, and write down why

Sometimes the honest answer is that the crate cannot be removed without
disproportionate cost, and that its presence is bounded and understood.

That is where hushlog ended up after Option A failed. The rule for
accepting an advisory is not "it is only a warning" — it is that you can
state *why* it is safe, in a form that survives you forgetting.

In `deny.toml`:

```toml
[advisories]
version = 2
yanked = "deny"

ignore = [
    { id = "RUSTSEC-2017-0008", reason = "unmaintained, transitive via portable-pty, only reachable behind opt-in --pty" },
    { id = "RUSTSEC-2026-0441", reason = "unmaintained, transitive via nix, only reachable behind opt-in --pty; the portable-pty 0.9 upgrade that removes it breaks Windows PTY" },
]
```

The `reason` field is not decoration. It is the thing that makes the entry
auditable: a future reader can check whether the reason still holds, and
`cargo deny` fails loudly when the crate's situation changes. An ignore
entry with no reason is a decision nobody can review.

**What makes acceptance defensible rather than lazy:**

- The crate is unmaintained, not vulnerable. There is no exploit; there is a
  maintenance vacuum.
- Its reachability is bounded and *stated*: only behind an opt-in flag, and
  only on a platform where the calling code is dead.
- The fix was found, measured, and rejected on evidence — not overlooked.
- The reasoning is recorded where the next person will actually look.

---

## 5. The general shape

`pin-utils` is a small crate with a large lesson. The pattern it belongs to
is this:

> **Your dependency tree is not the sum of what you chose. It is the sum of
> what everybody else chose, times the defaults everybody forgot to turn off.**

A deprecated macro helper, archived in 2026, reached a project in 2026 through
a PTY library, a syscall wrapper, and a feature named after a Linux I/O
subsystem that nothing in the project uses.

Three habits catch it:

**Look at the tree, not the manifest.**
`cargo tree -i <crate>` and `cargo tree --target <your-target> -i <crate>`.
The manifest is what you asked for. The tree is what you got.

**Read the feature edges, not just the packages.**
`cargo tree -e features` is the only way to see *why* a crate is there. In
this case every single edge was a default feature — which is also the
diagnosis: nothing anyone did was wrong.

**Fix it at the layer that owns the leak.**
The leak lives in `nix`'s manifest. `nix` 0.28 fixed it upstream. A vendored
patch fixes it locally. An ignore entry documents it. Upgrading `portable-pty`
moves the problem rather than solving it — and, here, broke something else
on the way.

The one thing not to do is not to look.

---

## Appendix: the commands

```sh
# 1. Is it in the tree at all?
cargo tree -i pin-utils

# 2. Why is it in the tree? Every edge named.
cargo tree -i pin-utils -e features

# 3. Is it in the binary you ship, for the target you ship?
cargo tree --target x86_64-unknown-linux-musl -i pin-utils
cargo tree --target x86_64-pc-windows-msvc -i pin-utils

# 4. Can the version that owns the leak be moved independently?
cargo update -p nix --precise 0.28.0
# error: failed to select a version for the requirement `nix = "^0.25"`
#   required by package `portable-pty v0.8.1`

# 5. What does the audit say, and what does the linter say?
cargo audit --json --file ./Cargo.lock
cargo deny check advisories

# 6. Does adding the dependency yourself with default-features = false help?
#    (It does not. Features unify across the graph.)
```

---

*Draft — for review. Measurements and code excerpts are taken from the
hushlog workspace at commit `b11743d` plus the uncommitted work of
2026-11-16.*
