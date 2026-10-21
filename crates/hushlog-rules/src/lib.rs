//! Stage 3 -- rule schema, loading and classification. **Owner: WP3.**
//!
//! Rules are TOML. Every rule is compiled once into a single `RegexSet`, so
//! classification costs one pass over the line regardless of rule count.
//!
//! Contract: implements [`hushlog_core::Classifier`]. Invalid rules fail loudly,
//! naming the file and line, rather than being silently skipped -- see
//! AGENTS.md section 1.
//!
//! # Example
//!
//! ```
//! use hushlog_core::{Classifier, Level, Line, Stream};
//! use hushlog_rules::parse_pack;
//!
//! let pack = parse_pack(
//!     "inline.toml",
//!     r#"
//!     [[rule]]
//!     id = "cargo.warning"
//!     match = { kind = "regex", pattern = "^warning: " }
//!     level = "warning"
//!     "#,
//! )
//! .unwrap();
//! let mut classifier = hushlog_rules::RuleClassifier::from_pack(&pack).unwrap();
//!
//! let line = Line::new("warning: unused variable", "warning: unused variable", Stream::Stdout, 1);
//! assert_eq!(classifier.classify(line).level, Level::Warning);
//! ```

#![forbid(unsafe_code)]

mod blocks;
mod classifier;
mod error;
mod loader;
mod schema;

pub use blocks::block_detector;
pub use classifier::{CompiledRule, MAX_RULES, RuleClassifier};
pub use error::{Location, RuleError};
pub use loader::{load_pack, load_packs, parse_pack};
pub use schema::{
    BlockMarkers, FormatDetect, LevelMap, Matcher, RegexPattern, Rule, RuleLevel, RulePack,
    RuleSample,
};

/// The rule packs shipped with hushlog, compiled into the binary.
///
/// Keeping the built-ins in the executable means a user does not have to find a
/// `rules/` directory for hushlog to be useful, and it means the packs cannot
/// go missing between release and run.
pub mod builtin {
    use super::{RuleClassifier, RuleError, RulePack};

    /// `(name, source)` for each pack built into the binary.
    ///
    /// These paths reach into `crates/hushlog-rules/rules/`, *inside this
    /// crate*. They used to point at a workspace-root `rules/` directory, which
    /// was tidier to browse but meant `cargo package` shipped none of them --
    /// a published `hushlog-rules` would then fail to compile for every
    /// downstream user, because `include_str!` would not resolve. Keep these
    /// files inside the crate; `cargo package -p hushlog-rules --list` must
    /// always show all of them.
    ///
    /// Order matters: built-in packs come first so they win when two rules
    /// match the same line (first match wins). Packs that declare `detect`
    /// patterns are tried first during format detection regardless of this
    /// order.
    pub const PACKS: [(&str, &str); 11] = [
        ("cargo", include_str!("../rules/cargo.toml")),
        ("npm", include_str!("../rules/npm.toml")),
        ("docker", include_str!("../rules/docker.toml")),
        ("kubectl", include_str!("../rules/kubectl.toml")),
        ("syslog", include_str!("../rules/syslog.toml")),
        ("journald", include_str!("../rules/journald.toml")),
        ("otel", include_str!("../rules/otel.toml")),
        ("pino", include_str!("../rules/pino.toml")),
        ("bunyan", include_str!("../rules/bunyan.toml")),
        ("spdlog", include_str!("../rules/spdlog.toml")),
        ("error_log", include_str!("../rules/error_log.toml")),
    ];

    /// Parse every built-in pack.
    ///
    /// # Errors
    ///
    /// If a built-in pack does not compile. This cannot happen in a release
    /// build -- the test suite asserts it -- but the type system has to assume
    /// it might, and pretending otherwise with an `expect` would trade a loud
    /// failure for a silent one.
    pub fn packs() -> Result<Vec<RulePack>, RuleError> {
        PACKS
            .iter()
            .map(|(name, source)| super::parse_pack(name, source))
            .collect()
    }

    /// Compile the built-in packs into a ready classifier.
    ///
    /// # Errors
    ///
    /// As [`packs`].
    pub fn classifier() -> Result<RuleClassifier, RuleError> {
        RuleClassifier::new(&packs()?)
    }
}
