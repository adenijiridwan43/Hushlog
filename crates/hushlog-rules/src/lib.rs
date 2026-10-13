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
pub use schema::{BlockMarkers, Matcher, Rule, RuleLevel, RulePack};

/// The rule packs shipped with hushlog, compiled into the binary.
///
/// Keeping the built-ins in the executable means a user does not have to find a
/// `rules/` directory for hushlog to be useful, and it means the packs cannot
/// go missing between release and run. The files in the workspace `rules/`
/// directory are the same TOML and are covered by the tests, so a pack cannot
/// drift from what is embedded here without CI noticing.
pub mod builtin {
    use super::{RuleClassifier, RuleError, RulePack};

    /// `(name, source)` for each pack built into the binary.
    pub const PACKS: [(&str, &str); 4] = [
        ("cargo", include_str!("../../../rules/cargo.toml")),
        ("npm", include_str!("../../../rules/npm.toml")),
        ("docker", include_str!("../../../rules/docker.toml")),
        ("kubectl", include_str!("../../../rules/kubectl.toml")),
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
