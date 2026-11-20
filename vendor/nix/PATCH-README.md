# Vendored `nix` 0.25.1 — hushlog patch

## Why this exists

hushlog depends on `portable-pty` for one thing: the opt-in `--pty` mode.
`portable-pty` depends on `nix` for Unix syscall wrappers. `nix` 0.25.1's
`default` feature list includes `"aio"`, and `aio = ["pin-utils"]`. So the
graph carried `pin-utils` — an unmaintained crate
(RUSTSEC-2026-0441) — even though hushlog never asked for it and never uses
`portable-pty`'s AIO paths either.

`nix` 0.28.0 fixed the upstream leak (`default = []`), but 0.9.0 of
`portable-pty` that we take it from also breaks Windows PTY, so that upgrade
path was rejected. This is the third option: patch the owning crate.

## The one-line change

In `Cargo.toml` in this directory, the `default` feature list is missing
`"aio"` compared to the original `nix-0.25.1`. Everything else is untouched.

## To redo this when nix updates

1. Find the new `nix` version in `~/.cargo/registry/src/*/nix-X.Y.Z/Cargo.toml`.
2. Replace this directory:
   `Copy-Item -Path "...\nix-X.Y.Z" -Destination "vendor/nix" -Recurse`
   (make it writable — registry copies are read-only).
3. In `vendor/nix/Cargo.toml`, delete the `"aio",` entry from the `default`
   feature list. Leave `aio = ["pin-utils"]` alone — the feature still
   exists, it is just no longer on by default.
4. Update the path pin in the workspace `Cargo.toml`:
   `[patch.crates-io] nix = { path = "vendor/nix" }` (already there).
5. `cargo generate-lockfile` — NOT `cargo update -p nix`; the lockfile still
   records the optional dep until the graph is fully re-resolved.

Verify: `cargo tree -i pin-utils` should say
"package ID specification `pin-utils` did not match any packages."
