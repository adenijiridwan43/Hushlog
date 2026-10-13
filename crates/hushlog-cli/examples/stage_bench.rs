//! Per-stage throughput measurement for the hushlog pipeline.
//!
//! Run with `cargo run --release --example stage_bench`.
//!
//! This exists because two agents measured the same pipeline in isolation and
//! disagreed: WP2 reported 25-90 MB/s for normalization alone, WP3 reported
//! 90-230 MB/s for a bare `RegexSet`. Neither number meant anything on its own,
//! and neither measured the thing `AGENTS.md` section 7 actually promises.
//!
//! What that section promises is end-to-end throughput over a log. So this
//! measures exactly that, stage by stage, so the cost can be attributed rather
//! than guessed at. It also measures a bare byte-splitting baseline: without it
//! the absolute numbers are unreadable, because a slow box and a slow program
//! look identical.

use std::time::{Duration, Instant};

use hushlog_core::{Classifier, Ingest, Level, Normalizer, Stream};
use hushlog_rules::builtin;

/// Roughly what CI output looks like: mostly progress noise, a quarter repeated
/// warnings worth collapsing, a few errors, and some unique information.
fn make_corpus(mb: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(mb * 1024 * 1024);
    let mut n: u64 = 0;

    while out.len() < mb * 1024 * 1024 {
        n += 1;
        let line = match n % 20 {
            0..=11 => format!(
                "2026-10-12T09:14:{:02}Z INFO  scheduler: reconciliation pass {n} healthy\n",
                n % 60
            ),
            12..=16 => format!(
                "2026-10-12T09:14:{sec:02}Z WARN  scheduler: Retry {attempt}/10 for pod checkout-api-{pod:09x}-{shard:04x} after backoff {backoff}ms\n",
                sec = n % 60,
                attempt = n % 10,
                pod = n % 10_000_000,
                shard = n % 10_000,
                backoff = 100 * (n % 64)
            ),
            17 | 18 => format!(
                "    Checking hushlog-{} v0.1.0 (C:\\Users\\usr\\projects\\hushlog\\crates\\hushlog-{})\n",
                n % 7,
                n % 7
            ),
            _ => format!(
                "src/handlers/user.rs({n},9): error TS6133: 'ctx_{n}' is declared but its value is never read.\n"
            ),
        };
        out.extend_from_slice(line.as_bytes());
    }
    out
}

/// Bytes per second over `f`, reported as MB/s (1 MB = 1_000_000 bytes).
fn mbps(bytes: usize, dur: Duration) -> f64 {
    bytes as f64 / dur.as_secs_f64() / 1_000_000.0
}

fn time<T>(label: &str, bytes: usize, f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let out = f();
    let d = start.elapsed();
    let r = mbps(bytes, d);
    println!("  {label:<38} {r:>8.1} MB/s   ({:>7.1} ms)", d.as_secs_f64() * 1000.0);
    (out, r)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const MB: usize = 64;
    let corpus = make_corpus(MB);
    let nbytes = corpus.len();
    println!("hushlog stage benchmark — {nbytes} bytes of synthetic CI output\n");

    // ---- Baseline: what a bare line split costs on this machine. ----------
    println!("baseline");
    let (base_lines, base_mbps) = time("byte scan + split (no processing)", nbytes, || {
        let mut count = 0usize;
        let mut start = 0usize;
        for (i, b) in corpus.iter().enumerate() {
            if *b == b'\n' {
                let _ = &corpus[start..i];
                count += 1;
                start = i + 1;
            }
        }
        count
    });
    println!("  ({base_lines} lines)\n");

    // ---- Stage 1: ingest ---------------------------------------------------
    println!("stage 1 — ingest");
    let chunk = 64 * 1024;
    let (lines, ingest_mbps) = time("Ingest::push (framing, ANSI, \\r)", nbytes, || {
        let mut ing = Ingest::new(Stream::Stdout);
        let mut all = Vec::new();
        for c in corpus.chunks(chunk) {
            all.extend(ing.push(c));
        }
        all.extend(ing.finish());
        all
    });
    println!("  ({} lines)\n", lines.len());

    // ---- Stage 2: normalize + fingerprint ----------------------------------
    println!("stage 2 — normalize + fingerprint");
    let normalizer = Normalizer::new();
    let (_fps, norm_mbps) = time("Normalizer::fingerprint per line", nbytes, || {
        let mut acc: u64 = 0;
        for l in &lines {
            acc ^= normalizer.fingerprint(&l.plain);
        }
        acc
    });

    // ---- Stage 3: classify --------------------------------------------------
    println!("\nstage 3 — classify");
    let mut classifier = builtin::classifier()?;
    println!("  ({} rules loaded)", classifier.rules().len());
    let (n_err, class_mbps) = time("RuleClassifier::classify per line", nbytes, || {
        let mut errors = 0usize;
        for l in &lines {
            let c = classifier.classify(l.clone());
            if c.level == Level::Error {
                errors += 1;
            }
        }
        errors
    });
    println!("  ({n_err} classified as errors)\n");

    // ---- All three, interleaved the way the pipeline runs -------------------
    println!("end to end — all three stages");
    let mut cls = builtin::classifier()?;
    let (full, full_mbps) = time("ingest -> fingerprint -> classify", nbytes, || {
        let mut ing = Ingest::new(Stream::Stdout);
        let norm = Normalizer::new();
        let mut errors = 0usize;
        let mut fps: u64 = 0;
        let mut consume = |batch: &mut Vec<hushlog_core::Line>| {
            for l in batch.iter() {
                fps ^= norm.fingerprint(&l.plain);
                if cls.classify(l.clone()).level == Level::Error {
                    errors += 1;
                }
            }
            batch.clear();
        };
        let mut batch = Vec::new();
        for c in corpus.chunks(chunk) {
            batch.extend(ing.push(c));
            consume(&mut batch);
        }
        batch.extend(ing.finish());
        consume(&mut batch);
        let _ = fps;
        errors
    });
    println!("  ({full} errors)\n");

    // ---- Attribution --------------------------------------------------------
    println!("attribution");
    let total = 1.0 / full_mbps.max(f64::EPSILON);
    let i = 1.0 / ingest_mbps.max(f64::EPSILON);
    let n = 1.0 / norm_mbps.max(f64::EPSILON);
    let c = 1.0 / class_mbps.max(f64::EPSILON);
    println!("  ingest      {:>6.1}% of end-to-end time", i / total * 100.0);
    println!("  normalize   {:>6.1}% of end-to-end time", n / total * 100.0);
    println!("  classify    {:>6.1}% of end-to-end time", c / total * 100.0);

    println!("\nverdict");
    let ratio_pct = full_mbps / base_mbps.max(f64::EPSILON) * 100.0;

    // Machine-readable, for the CI regression guard. Absolute MB/s varies 3-4x
    // between machines; this ratio does not.
    println!(
        "RATIO end_to_end_mbps={full_mbps:.2} baseline_mbps={base_mbps:.2} ratio_pct={ratio_pct:.3}"
    );

    const FLOOR_PCT: f64 = 1.0;
    if ratio_pct < FLOOR_PCT {
        println!("  FAIL: {ratio_pct:.2}% of byte-scan baseline, floor is {FLOOR_PCT}%.");
        std::process::exit(1);
    }
    println!(
        "  {full_mbps:.1} MB/s = {ratio_pct:.2}% of this machine's {base_mbps:.0} MB/s byte-scan baseline (floor {FLOOR_PCT}%)."
    );
    println!("  The old 200 MB/s target was removed as unreachable; see AGENTS.md section 7.");

    Ok(())
}
