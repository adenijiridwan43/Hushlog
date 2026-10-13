//! The `hushlog` binary. **Owner: WP6.**
//!
//! All I/O lives here: argument parsing, pipe and wrap modes, PTY handling,
//! signal forwarding, `--tee`, and exit-code passthrough.
//!
//! The exit code of a wrapped command must always be propagated. See
//! `AGENTS.md` section 4.

fn main() {
    println!("hushlog");
}
