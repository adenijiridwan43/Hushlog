//! Bridge between rule-declared block markers and the core [`BlockDetector`].
//!
//! This module exists because the two halves were built by different hands and
//! do not quite agree:
//!
//! - A rule pack may declare `block.start` / `block.end` as **either** a
//!   `literal` or a `regex` ([`Matcher`]).
//! - [`BlockDetector`] matches markers with `text.contains(...)` -- plain
//!   substring matching.
//!
//! Those cannot both be true. A rule written as `start = { kind = "regex",
//! pattern = "^npm ERR!" }` handed straight to the detector would be searched
//! for the literal text `^npm ERR!`, which never occurs, so the block would
//! silently never open. A pack author would have written a rule that looks
//! correct and does nothing.
//!
//! So this bridge accepts `literal` markers and **rejects regex ones loudly**,
//! naming the rule and the file. A pack that wants regex block boundaries has
//! to say so in an issue rather than shipping a marker that quietly fails.

use hushlog_core::BlockDetector;

use crate::classifier::CompiledRule;
use crate::error::RuleError;
use crate::schema::Matcher;

/// Which marker set a pattern belongs in.
#[derive(Debug, Clone, Copy)]
enum Mark {
    Start,
    End,
}

/// Build a [`BlockDetector`] carrying every block marker declared in `rules`.
///
/// The built-in markers are kept, and rule markers are added on top. A pack
/// that wants to *replace* the built-ins rather than extend them is not
/// supported yet; extending is the safer default because it cannot stop a
/// stack trace from being detected.
///
/// # Errors
///
/// Returns [`RuleError::Rule`] if any rule declares a **regex** block marker,
/// naming the rule and its source location, rather than passing a pattern to a
/// substring search that would never match.
pub fn block_detector(rules: &[CompiledRule]) -> Result<BlockDetector, RuleError> {
    let mut detector = BlockDetector::new();

    for rule in rules {
        let Some(block) = &rule.block else { continue };

        add(&mut detector, rule, &block.start, Mark::Start)?;
        if let Some(end) = &block.end {
            add(&mut detector, rule, end, Mark::End)?;
        }
    }

    Ok(detector)
}

fn add(
    detector: &mut BlockDetector,
    rule: &CompiledRule,
    matcher: &Matcher,
    which: Mark,
) -> Result<(), RuleError> {
    let kind = match which {
        Mark::Start => "block.start",
        Mark::End => "block.end",
    };

    match matcher {
        Matcher::Literal { pattern } => {
            let text = pattern.get_ref();
            match which {
                Mark::Start => {
                    detector.add_start_marker(text);
                }
                Mark::End => {
                    detector.add_end_marker(text);
                }
            }
            Ok(())
        }
        Matcher::Regex { .. } => Err(RuleError::Rule {
            location: rule.location.clone(),
            id: rule.id.clone(),
            message: format!(
                "{kind} must be `{{ kind = \"literal\", pattern = ... }}`. \
                 Block detection matches substrings, so a regex marker would \
                 never fire and the block would silently never open."
            ),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_regex_block_marker_is_refused_rather_than_ignored() {
        // The whole point of this module: a pattern that cannot work must not
        // be handed to a substring search and forgotten.
        let pack: crate::RulePack = toml::from_str(
            r#"
            [[rule]]
            id = "npm.err"
            match = { kind = "literal", pattern = "npm ERR!" }
            level = "error"
            [rule.block]
            start = { kind = "regex", pattern = "^npm ERR!" }
        "#,
        )
        .expect("pack should parse; the marker is rejected later, not here");

        let classifier = crate::RuleClassifier::from_pack(&pack).expect("pack compiles");
        let err = block_detector(classifier.rules())
            .expect_err("a regex block marker must be refused, not dropped");

        let message = err.to_string();
        assert!(
            message.contains("npm.err"),
            "error must name the rule, got: {message}"
        );
        assert!(
            message.contains("block.start"),
            "error must name the field, got: {message}"
        );
    }
}
