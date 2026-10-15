//! Every pack shipped in this crate's `rules/` directory must load, compile and
//! be reachable.
//!
//! A built-in pack that does not parse is a broken release, so this walks the
//! directory rather than a hard-coded list: adding a pack to `rules/` adds it
//! here for free.

use std::path::{Path, PathBuf};

use hushlog_core::{Classifier, Level, Line, Stream};
use hushlog_rules::{RuleClassifier, load_pack};

fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn rules_dir() -> PathBuf {
    crate_root().join("rules")
}

/// Every `*.toml` directly under `rules/`, sorted so failures are stable.
fn pack_paths() -> Vec<PathBuf> {
    let dir = rules_dir();
    assert!(
        dir.is_dir(),
        "built-in rule packs directory is missing: {}",
        dir.display()
    );

    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read rules/")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no built-in rule packs found in {}",
        dir.display()
    );
    paths
}

fn load_all() -> RuleClassifier {
    let packs: Vec<_> = pack_paths()
        .iter()
        .map(|p| load_pack(p).unwrap_or_else(|e| panic!("{} must load: {e}", p.display())))
        .collect();
    RuleClassifier::new(&packs).expect("built-in packs must compile together")
}

fn classify(c: &mut RuleClassifier, text: &str) -> hushlog_core::Classified {
    let line = Line::new(text, text, Stream::Stdout, 1);
    c.classify(line)
}

#[test]
fn every_builtin_pack_loads_and_compiles() {
    let c = load_all();
    assert!(
        c.rules().len() >= 4,
        "expected at least the four shipped packs"
    );
}

#[test]
fn the_four_named_packs_are_present() {
    let names: Vec<String> = pack_paths()
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();

    for expected in ["cargo", "npm", "docker", "kubectl"] {
        assert!(
            names.contains(&expected.to_string()),
            "missing pack: {expected} (have {names:?})"
        );
    }
}

#[test]
fn every_builtin_rule_has_a_namespaced_id() {
    let c = load_all();
    for rule in c.rules() {
        assert!(
            rule.id.contains('.'),
            "rule id `{}` should be namespaced `pack.name`",
            rule.id
        );
        assert!(
            !rule.id.chars().any(char::is_whitespace),
            "rule id `{}` contains whitespace",
            rule.id
        );
    }
}

#[test]
fn no_builtin_pack_collapses_errors() {
    // AGENTS.md section 1.1. This is asserted over the shipped packs, not just
    // the schema, because a hand-written pack could try to sneak past the
    // loader check by another route.
    let c = load_all();
    for rule in c.rules() {
        assert!(
            !(rule.collapse && rule.level == Level::Error),
            "built-in rule `{}` collapses errors",
            rule.id
        );
    }
}

#[test]
fn cargo_warnings_and_errors_are_classified() {
    let mut c = load_all();

    let got = classify(&mut c, "warning: unused variable: `ctx`");
    assert_eq!(got.level, Level::Warning);

    let got = classify(&mut c, "error[E0308]: mismatched types");
    assert_eq!(got.level, Level::Error);

    let got = classify(&mut c, "error: could not compile `hushlog-core`");
    assert_eq!(got.level, Level::Error);
}

#[test]
fn npm_deprecations_and_failures_are_classified() {
    let mut c = load_all();

    let got = classify(
        &mut c,
        "npm WARN deprecated request@2.88.2: request has been deprecated, see https://github.com/request/request/issues/3142",
    );
    assert_eq!(got.level, Level::Warning);
    assert!(got.highlight, "a deprecated dependency deserves attention");

    let got = classify(&mut c, "npm ERR! code ELIFECYCLE");
    assert_eq!(got.level, Level::Error);
}

#[test]
fn docker_build_output_is_classified() {
    let mut c = load_all();

    assert_eq!(
        classify(
            &mut c,
            "DEPRECATION NOTICE: Docker Image Format v1 is deprecated"
        )
        .level,
        Level::Warning
    );
    assert_eq!(
        classify(
            &mut c,
            "ERROR: failed to solve: process \"/bin/sh -c apt-get\" did not complete successfully"
        )
        .level,
        Level::Error
    );
}

#[test]
fn kubectl_events_are_classified() {
    let mut c = load_all();

    let got = classify(
        &mut c,
        "  Warning  BackOff        2m    kubelet            Back-off restarting failed container",
    );
    assert_eq!(got.level, Level::Warning);

    let got = classify(
        &mut c,
        "  Warning  FailedScheduling 3m    default-scheduler  0/3 nodes are available: 3 Insufficient cpu.",
    );
    assert_eq!(got.level, Level::Warning);

    let got = classify(
        &mut c,
        "Error from server (NotFound): pods \"nope\" not found",
    );
    assert_eq!(got.level, Level::Error);
}

#[test]
fn build_tools_classify_against_the_real_fixtures() {
    // tests/fixtures/ is owned elsewhere in the workspace; these are read-only
    // here. The point is that a pack which loads but does not fire on real
    // output is not a working pack.
    let mut c = load_all();
    let fixtures = crate_root()
        .join("..")
        .join("..")
        .join("tests")
        .join("fixtures");

    let checks: &[(&str, &[&str])] = &[
        (
            "synthetic-cargo-warnings.log",
            &[
                "warning: unused variable: `ctx`",
                "warning: unused import: `std::time::Duration`",
            ],
        ),
        (
            "synthetic-npm-build.log",
            &[
                "npm WARN deprecated request@2.88.2: request has been deprecated, see https://github.com/request/request/issues/3142",
                "npm ERR! code ELIFECYCLE",
            ],
        ),
    ];

    for (file, lines) in checks {
        let path = fixtures.join(file);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for line in *lines {
            assert!(
                body.lines().any(|l| l == *line),
                "fixture {file} no longer contains the line this test asserts on:\n  {line}"
            );
            let got = classify(&mut c, line);
            assert!(
                got.rule_id.is_some(),
                "no rule matched a real {file} line:\n  {line}"
            );
        }
    }
}
