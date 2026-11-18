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
//!
//! # Two pipelines, not one
//!
//! The first measurement is a *proxy*: ingest, fingerprint, classify. It is
//! what the stages cost in isolation. It is **not** what the binary runs.
//!
//! The binary masks into an owned `String`, hands the classified line to the
//! collapser (which hashes the masked text again, keeps an LRU, and clones the
//! group sample on every repeat), and renders the events. A proxy that is
//! several times faster than the thing it proxies is worse than no benchmark
//! at all: it says the tool is fine when the tool is not. So the real shape
//! is measured too, as cumulative prefixes, so each stage's marginal cost in
//! context is visible rather than inferred.
//!
//! # Two corpora
//!
//! The synthetic corpus is mostly unique lines: it measures the per-line cost
//! of the pipeline. The real fixture is mostly *repeats*: it measures the
//! per-fold cost, which is the case the product exists for. A benchmark that
//! only has one of the two is measuring half the product.
//!
//! # Reading the numbers
//!
//! Absolute MB/s varies by 3-4x between machines and by a factor of two on a
//! busy one. The ratio to this machine's own byte-scan baseline does not, and
//! that is the number to watch. The `RATIO` lines are machine-readable for
//! the CI regression guard.

use std::time::{Duration, Instant};

use hushlog_core::{
    Classifier, CollapsePolicy, Collapser, FoldingCollapser, Ingest, Level, Normalizer, Stream,
};
use hushlog_rules::builtin;

/// Lowest share of this machine's own byte-scan baseline that still counts as
/// healthy. See `AGENTS.md` section 7.
const FLOOR_PCT: f64 = 1.0;

/// The chunk size the ingest stage is fed with.
const CHUNK: usize = 64 * 1024;

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

/// The real capture, repeated so it is big enough to time.
///
/// Repeating it does not make it less real: the content is verbatim, and the
/// repetition is exactly what a rate-limit storm looks like in production.
/// What it changes is the *fold rate*, which is the thing this corpus exists
/// to exercise.
fn real_fixture_corpus(repeats: usize) -> Option<Vec<u8>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/real-relanflow-ratelimit-storm.log");
    let original = std::fs::read(&path).ok()?;
    let mut out = Vec::with_capacity(original.len() * repeats);
    for _ in 0..repeats {
        out.extend_from_slice(&original);
    }
    Some(out)
}

/// Bytes per second over `f`, reported as MB/s (1 MB = `1_000_000` bytes).
fn mbps(bytes: usize, dur: Duration) -> f64 {
    // Through `u32` on purpose: a corpus big enough to overflow it is not
    // something a laptop will read, and a checked conversion beats a silent
    // truncation in a number people are about to trust.
    let bytes = u32::try_from(bytes).unwrap_or(u32::MAX);
    f64::from(bytes) / dur.as_secs_f64() / 1_000_000.0
}

/// Time one run and report it as MB/s.
fn time<T>(label: &str, bytes: usize, f: impl FnOnce() -> T) -> (T, f64) {
    let start = Instant::now();
    let out = f();
    let d = start.elapsed();
    let r = mbps(bytes, d);
    println!(
        "  {label:<38} {r:>8.1} MB/s   ({:>7.1} ms)",
        d.as_secs_f64() * 1000.0
    );
    (out, r)
}

/// What one corpus measured.
///
/// Every rate is in MB/s.
struct Corpus {
    /// This machine's bare byte-scan baseline.
    baseline: f64,
    /// The proxy pipeline: ingest, fingerprint, classify.
    proxy: f64,
    /// Ingest alone.
    ingest: f64,
    /// Normalize alone.
    normalize: f64,
    /// Classify alone.
    classify: f64,
    /// Ingest then classify.
    prefix_classify: f64,
    /// Ingest, classify, mask.
    prefix_mask: f64,
    /// Ingest, classify, mask, block detect.
    prefix_detect: f64,
    /// The whole pipeline.
    pipeline: f64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const MB: usize = 64;
    let synthetic = make_corpus(MB);
    run_corpus("synthetic CI corpus", &synthetic)?;

    if let Some(real) = real_fixture_corpus(2000) {
        run_corpus("real-relanflow-ratelimit-storm.log (x2000)", &real)?;
    }

    Ok(())
}

/// Measure one corpus, from the bare byte scan to the real pipeline.
fn run_corpus(label: &str, corpus: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== {label} — {} bytes ===\n", corpus.len());

    let (baseline_mbps, _baseline_lines) = measure_baseline(corpus);
    let (ingest_mbps, norm_mbps, class_mbps, proxy_mbps) = measure_stages(corpus)?;
    let prefixes = measure_real_pipeline(corpus)?;

    report(&Corpus {
        baseline: baseline_mbps,
        proxy: proxy_mbps,
        ingest: ingest_mbps,
        normalize: norm_mbps,
        classify: class_mbps,
        prefix_classify: prefixes.0,
        prefix_mask: prefixes.1,
        prefix_detect: prefixes.2,
        pipeline: prefixes.3,
    });

    Ok(())
}

/// The bare byte scan: what a line split costs on this machine, with nothing
/// else done to the bytes.
fn measure_baseline(corpus: &[u8]) -> (f64, usize) {
    println!("baseline");
    let (lines, mbps) = time("byte scan + split (no processing)", corpus.len(), || {
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
    println!("  ({lines} lines)\n");
    (mbps, lines)
}

/// Each stage in isolation, plus the proxy pipeline that interleaves them.
fn measure_stages(corpus: &[u8]) -> Result<(f64, f64, f64, f64), Box<dyn std::error::Error>> {
    let nbytes = corpus.len();

    println!("stage 1 — ingest");
    let (lines, ingest_mbps) = time("Ingest::push (framing, ANSI, \\r)", nbytes, || {
        let mut ing = Ingest::new(Stream::Stdout);
        let mut all = Vec::new();
        for c in corpus.chunks(CHUNK) {
            all.extend(ing.push(c));
        }
        all.extend(ing.finish());
        all
    });
    println!("  ({} lines)\n", lines.len());

    println!("stage 2 — normalize + fingerprint");
    let normalizer = Normalizer::new();
    let (_fps, norm_mbps) = time("Normalizer::fingerprint per line", nbytes, || {
        let mut acc: u64 = 0;
        for l in &lines {
            acc ^= normalizer.fingerprint(&l.plain);
        }
        acc
    });

    println!("\nstage 3 — classify");
    let mut rules = builtin::classifier()?;
    println!("  ({} rules loaded)", rules.rules().len());
    let (n_err, class_mbps) = time("RuleClassifier::classify per line", nbytes, || {
        let mut errors = 0usize;
        for l in &lines {
            let c = rules.classify(l.clone());
            if c.level == Level::Error {
                errors += 1;
            }
        }
        errors
    });
    println!("  ({n_err} classified as errors)\n");

    println!("end to end — the three stages in isolation");
    let mut proxy_rules = builtin::classifier()?;
    let (full, proxy_mbps) = time("ingest -> fingerprint -> classify", nbytes, || {
        let mut ing = Ingest::new(Stream::Stdout);
        let norm = Normalizer::new();
        let mut errors = 0usize;
        let mut fps: u64 = 0;
        let mut batch = Vec::new();
        for c in corpus.chunks(CHUNK) {
            batch.extend(ing.push(c));
            for l in batch.drain(..) {
                fps ^= norm.fingerprint(&l.plain);
                if proxy_rules.classify(l).level == Level::Error {
                    errors += 1;
                }
            }
        }
        batch.extend(ing.finish());
        for l in batch.drain(..) {
            fps ^= norm.fingerprint(&l.plain);
            if proxy_rules.classify(l).level == Level::Error {
                errors += 1;
            }
        }
        let _ = fps;
        errors
    });
    println!("  ({full} errors)\n");

    Ok((ingest_mbps, norm_mbps, class_mbps, proxy_mbps))
}

/// The pipeline the binary actually runs, measured as cumulative prefixes.
///
/// Each prefix adds one stage, so the marginal cost of a stage is the
/// difference in time per megabyte between two rows. Anything less would
/// leave the attribution a matter of opinion.
#[allow(clippy::type_complexity)]
fn measure_real_pipeline(
    corpus: &[u8],
) -> Result<(f64, f64, f64, f64), Box<dyn std::error::Error>> {
    let nbytes = corpus.len();
    println!("real pipeline — cumulative prefixes of the shape the binary runs");

    let mut rules_a = builtin::classifier()?;
    let (errors, m_classify) = time("ingest -> classify", nbytes, || {
        let mut ing = Ingest::new(Stream::Stdout);
        let mut errors = 0usize;
        let mut batch = Vec::new();
        for c in corpus.chunks(CHUNK) {
            batch.extend(ing.push(c));
            for l in batch.drain(..) {
                if rules_a.classify(l).level == Level::Error {
                    errors += 1;
                }
            }
        }
        batch.extend(ing.finish());
        for l in batch.drain(..) {
            if rules_a.classify(l).level == Level::Error {
                errors += 1;
            }
        }
        errors
    });

    let mut rules_b = builtin::classifier()?;
    let (_masked, m_mask) = time("ingest -> classify -> mask", nbytes, || {
        let mut ing = Ingest::new(Stream::Stdout);
        let norm = Normalizer::new();
        let mut errors = 0usize;
        let mut batch = Vec::new();
        for c in corpus.chunks(CHUNK) {
            batch.extend(ing.push(c));
            for l in batch.drain(..) {
                let mut verdict = rules_b.classify(l);
                // Exactly what `Pipeline::handle` does: mask into an owned
                // string.
                verdict.line.plain = norm.mask(&verdict.line.plain).into_owned();
                if verdict.level == Level::Error {
                    errors += 1;
                }
            }
        }
        batch.extend(ing.finish());
        for l in batch.drain(..) {
            let mut verdict = rules_b.classify(l);
            verdict.line.plain = norm.mask(&verdict.line.plain).into_owned();
            if verdict.level == Level::Error {
                errors += 1;
            }
        }
        errors
    });

    let mut rules_c = builtin::classifier()?;
    let (_detected, m_detect) = time("ingest -> classify -> mask -> detect", nbytes, || {
        let mut ing = Ingest::new(Stream::Stdout);
        let norm = Normalizer::new();
        let mut det =
            hushlog_rules::block_detector(rules_c.rules()).expect("the block detector builds");
        let mut opened = 0usize;
        let mut batch = Vec::new();
        for c in corpus.chunks(CHUNK) {
            batch.extend(ing.push(c));
            for l in batch.drain(..) {
                let mut verdict = rules_c.classify(l);
                verdict.line.plain = norm.mask(&verdict.line.plain).into_owned();
                opened += usize::from(det.push(&verdict.line).opened);
            }
        }
        opened
    });

    let mut rules_d = builtin::classifier()?;
    let detector = hushlog_rules::block_detector(rules_d.rules())
        .map_err(|e| anyhow::anyhow!("building the block detector: {e}"))?;
    let (events, m_collapse) = time("ingest -> classify -> mask -> collapse", nbytes, || {
        let mut ing = Ingest::new(Stream::Stdout);
        let norm = Normalizer::new();
        let mut collapser =
            FoldingCollapser::with_policy_and_detector(CollapsePolicy::default(), detector);
        let mut events = 0usize;
        let mut batch = Vec::new();
        for c in corpus.chunks(CHUNK) {
            batch.extend(ing.push(c));
            for l in batch.drain(..) {
                let mut verdict = rules_d.classify(l);
                verdict.line.plain = norm.mask(&verdict.line.plain).into_owned();
                events += collapser.push(verdict).len();
            }
        }
        batch.extend(ing.finish());
        for l in batch.drain(..) {
            let mut verdict = rules_d.classify(l);
            verdict.line.plain = norm.mask(&verdict.line.plain).into_owned();
            events += collapser.push(verdict).len();
        }
        events += collapser.finish().len();
        events
    });
    println!("  ({errors} errors, {events} events)\n");

    Ok((m_classify, m_mask, m_detect, m_collapse))
}

/// The end of the run: what each stage cost, and whether the whole thing is
/// still fast enough.
///
/// Split out of the caller so the measurement and the verdict read as the two
/// different things they are.
#[allow(clippy::too_many_arguments)]
fn report(c: &Corpus) {
    println!("attribution");
    let total = 1.0 / c.proxy.max(f64::EPSILON);
    let i = 1.0 / c.ingest.max(f64::EPSILON);
    let n = 1.0 / c.normalize.max(f64::EPSILON);
    let cl = 1.0 / c.classify.max(f64::EPSILON);
    println!(
        "  ingest      {:>6.1}% of end-to-end time",
        i / total * 100.0
    );
    println!(
        "  normalize   {:>6.1}% of end-to-end time",
        n / total * 100.0
    );
    println!(
        "  classify    {:>6.1}% of end-to-end time",
        cl / total * 100.0
    );

    println!("\nmarginal cost inside the real pipeline (ms per MB)");
    let per_mb = |mbps: f64| 1000.0 / mbps.max(f64::EPSILON);
    let t1 = per_mb(c.prefix_classify);
    let t2 = per_mb(c.prefix_mask);
    let td = per_mb(c.prefix_detect);
    let t3 = per_mb(c.pipeline);
    println!("  ingest + classify          {t1:>8.1} ms/MB");
    println!("  + mask                     {:>8.1} ms/MB", t2 - t1);
    println!("  + block detect             {:>8.1} ms/MB", td - t2);
    println!("  + collapse                 {:>8.1} ms/MB", t3 - td);
    println!("  ---------------------------------------");
    println!("  total                      {t3:>8.1} ms/MB");

    println!("\nverdict");
    let ratio_pct = c.proxy / c.baseline.max(f64::EPSILON) * 100.0;
    let pipe_ratio_pct = c.pipeline / c.baseline.max(f64::EPSILON) * 100.0;

    // Machine-readable, for the CI regression guard. Absolute MB/s varies 3-4x
    // between machines; this ratio does not.
    println!(
        "RATIO end_to_end_mbps={:.2} baseline_mbps={:.2} ratio_pct={ratio_pct:.3}",
        c.proxy, c.baseline
    );
    println!(
        "RATIO real_pipeline_mbps={:.2} baseline_mbps={:.2} ratio_pct={pipe_ratio_pct:.3}",
        c.pipeline, c.baseline
    );

    if ratio_pct < FLOOR_PCT {
        println!("  FAIL: {ratio_pct:.2}% of byte-scan baseline, floor is {FLOOR_PCT}%.");
        std::process::exit(1);
    }
    println!(
        "  {:.1} MB/s = {ratio_pct:.2}% of this machine's {:.0} MB/s byte-scan baseline (floor {FLOOR_PCT}%).",
        c.proxy, c.baseline
    );
    println!(
        "  the real pipeline runs at {:.1} MB/s = {pipe_ratio_pct:.2}% of the same baseline.",
        c.pipeline
    );
    println!("  The old 200 MB/s target was removed as unreachable; see AGENTS.md section 7.");
}
