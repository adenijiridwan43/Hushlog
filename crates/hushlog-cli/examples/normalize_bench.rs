//! Normaliser profiling: where does `fingerprint` actually spend its time?
//!
//! Run with `cargo run --release --example normalize_bench`.
//!
//! The stage benchmark attributes ~60% of end-to-end time to
//! `Normalizer::fingerprint`. This asks whether that cost is inherent to the
//! work or an artefact of how it is written, by measuring the pieces against
//! each other on the same input.
//!
//! Three candidate explanations, in order of how likely they are:
//!
//! 1. It builds a `String` per line even when nothing changes, and the corpus
//!    is mostly lines with nothing to mask.
//! 2. It allocates twice per masked line (scratch buffer, then `Cow::Owned`).
//! 3. The xxh3 hash itself is slow, which would mean the design is wrong
//!    rather than the implementation.
//!
//! If (3) dominates, masking should be folded into the hash and the String
//! dropped entirely. If (1) dominates, the fix is to avoid allocating when the
//! masked form equals the input.

use std::borrow::Cow;
use std::time::Instant;

use hushlog_core::Normalizer;

fn corpus() -> Vec<String> {
    let mut v = Vec::new();
    for n in 1..=400_000u64 {
        v.push(match n % 4 {
            // no volatile tokens at all -- the common case in real logs
            0 => format!("    Checking hushlog-{} v0.1.0 (C:\\Users\\usr\\projects\\hushlog)", n % 7),
            // a timestamp and an id -- the case masking exists for
            1 => format!(
                "2026-10-12T09:14:{:02}Z WARN  scheduler: Retry {}/10 for pod checkout-api-{:09x}",
                n % 60,
                n % 10,
                n % 10_000_000
            ),
            // short, no mask
            2 => format!("test result: ok. {} passed; 0 failed", n % 200),
            _ => format!("{}", n), // pure number: masks to <num>
        });
    }
    v
}

fn bench(label: &str, bytes: usize, iters: usize, mut f: impl FnMut()) {
    // one warm pass so the first call's page faults are not counted
    f();
    let t = Instant::now();
    for _ in 0..iters {
        f();
    }
    let d = t.elapsed();
    let total = bytes * iters;
    println!(
        "  {label:<44} {:>8.1} MB/s   ({:>7.1} ms)",
        total as f64 / d.as_secs_f64() / 1e6,
        d.as_secs_f64() * 1000.0
    );
}

fn main() {
    let lines = corpus();
    let bytes: usize = lines.iter().map(String::len).sum();
    let iters = 5;
    let norm = Normalizer::new();

    println!("normalize profiling — {} lines, {} bytes, {iters} iters\n", lines.len(), bytes);

    // Control: just touch every byte.
    bench("touch every byte (no work)", bytes, iters, || {
        let mut acc = 0u64;
        for l in &lines {
            acc = acc.wrapping_add(l.len() as u64);
        }
        std::hint::black_box(acc);
    });

    // Control: hash with nothing masked at all. Isolates the hash cost.
    bench("xxh3 per line, no masking", bytes, iters, || {
        let mut acc = 0u64;
        for l in &lines {
            acc ^= xxhash_rust::xxh3::xxh3_64(l.as_bytes());
        }
        std::hint::black_box(acc);
    });

    // The mask itself, allocating when it changes something.
    bench("mask() only (allocates on change)", bytes, iters, || {
        for l in &lines {
            let m = norm.mask(l);
            std::hint::black_box(&m);
        }
    });

    // The real thing.
    bench("fingerprint() — mask then hash", bytes, iters, || {
        let mut acc = 0u64;
        for l in &lines {
            acc ^= norm.fingerprint(l);
        }
        std::hint::black_box(acc);
    });

    // How much does the line actually change? If most lines are unchanged,
    // an allocation-free fast path is worth having.
    let changed = lines.iter().filter(|l| matches!(norm.mask(l), Cow::Owned(_))).count();
    println!(
        "\n  {}/{} lines ({:.0}%) actually change when masked",
        changed,
        lines.len(),
        changed as f64 / lines.len() as f64 * 100.0
    );
    if changed * 4 < lines.len() {
        println!("  -> most lines are unchanged; a no-alloc fast path is available.");
    }
}