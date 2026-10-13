//! Bridge between rule-declared block markers and the core [`BlockDetector`].
//!
//! A rule pack may declare `block.start` / `block.end` as either a `literal`
//! or a `regex` ([`Matcher`]). The detector needs both: substring markers are
//! the fast path, and pattern markers exist for boundaries that no fixed
//! substring can capture -- a Kubernetes events-table header is
//! `TYPE    REASON    AGE...` with column padding that varies.
//!
//! Handing a regex to a substring search is the bug this module exists to
//! prevent: `^npm ERR!` would be searched for as that literal text, never
//! match, and the block would silently never open. So the two kinds are routed
//! to two different matching paths rather than being conflated.
//!
//! Built-in markers are kept and rule markers are added on top, because
//! extending cannot stop a stack trace from being detected the way replacing
//! can.

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
/// Built-in markers are kept and rule markers are added on top.
///
/// # Errors
///
/// Returns [`RuleError::Rule`] naming the rule and its source location if a
/// declared pattern is not a valid regex. Substring markers can never fail.
/// A rule that cannot work fails here rather than silently never firing.
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
    let pattern = matcher.pattern();

    let compiled = match (matcher, which) {
        (Matcher::Literal { .. }, Mark::Start) => {
            detector.add_start_marker(pattern);
            return Ok(());
        }
        (Matcher::Literal { .. }, Mark::End) => {
            detector.add_end_marker(pattern);
            return Ok(());
        }
        (Matcher::Regex { .. }, Mark::Start) => detector.try_add_regex_start_marker(pattern),
        (Matcher::Regex { .. }, Mark::End) => detector.try_add_regex_end_marker(pattern),
    };

    compiled.map(|_| ()).map_err(|e| RuleError::Rule {
        location: rule.location.clone(),
        id: rule.id.clone(),
        message: format!("{kind} is not a valid regex: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use hushlog_core::{Line, Stream};

    use super::*;
    use crate::RuleClassifier;

    fn line(s: &str) -> Line {
        Line::new(s.to_string(), s.to_string(), Stream::Stdout, 1)
    }

    fn detector_for(toml_src: &str) -> (RuleClassifier, BlockDetector) {
        let pack: crate::RulePack = toml::from_str(toml_src).expect("pack should parse the schema");
        let classifier = crate::RuleClassifier::from_pack(&pack).expect("pack should compile");
        let detector = block_detector(classifier.rules()).expect("markers should register");
        (classifier, detector)
    }

    #[test]
    fn a_literal_marker_opens_a_block() {
        let (_, mut d) = detector_for(
            r#"
            [[rule]]
            id = "demo.block"
            match = { kind = "literal", pattern = "ERROR" }
            level = "warning"
            [rule.block]
            start = { kind = "literal", pattern = "BEGIN-FAILURE" }
            end   = { kind = "literal", pattern = "END-FAILURE" }
        "#,
        );
        assert!(!d.has_regex_markers(), "a literal rule adds no regex work");

        assert!(
            d.push(&line("BEGIN-FAILURE")).opened,
            "literal start must open"
        );
        // An end marker joins the block and arms the close; the close lands on
        // the following line, exactly as in the core's own
        // `an_end_marker_joins_then_closes`.
        assert!(
            d.push(&line("END-FAILURE")).joined,
            "the end line belongs to the block"
        );
        assert!(
            d.push(&line("after the block")).closed.is_some(),
            "the line after an end marker closes the block"
        );
        assert!(!d.is_open());
    }

    #[test]
    fn a_regex_marker_opens_a_block_with_variable_whitespace() {
        // The case that motivated pattern markers at all: a table header whose
        // column padding varies, so no fixed substring can capture it.
        //
        // NOTE: TOML literal strings ('...') do not process backslash escapes,
        // so these patterns carry single backslashes.
        let (_, mut d) = detector_for(
            r#"
            [[rule]]
            id = "events.block"
            match = { kind = "literal", pattern = "Warning" }
            level = "warning"
            [rule.block]
            start = { kind = "regex", pattern = '(?i)^\s*type\s+reason\s+age\s+from\s+message' }
            end   = { kind = "regex", pattern = '^\s*----' }
        "#,
        );
        assert!(
            d.has_regex_markers(),
            "a regex rule must register pattern markers"
        );

        for header in [
            "TYPE     REASON     AGE   FROM      MESSAGE",
            "Type  Reason  Age  From  Message",
            "  type reason age from message",
        ] {
            assert!(
                d.push(&line(header)).opened,
                "pattern start must match {header:?} whatever the padding and case"
            );
            assert!(
                d.push(&line("     ----")).closed.is_some()
                    || d.push(&line("next")).closed.is_some(),
                "the separator must close the block"
            );
            d.force_close();
        }
    }

    #[test]
    fn the_shipped_built_in_packs_register_without_error() {
        // Regression: these packs used to fail to register at all, which made
        // every single run print a warning.
        let classifier = crate::builtin::classifier().expect("built-in packs compile");
        let detector = block_detector(classifier.rules())
            .expect("built-in packs must register their block markers");
        assert!(
            detector.has_regex_markers(),
            "the kubectl events header genuinely needs a pattern marker"
        );
    }

    #[test]
    fn an_invalid_regex_marker_fails_loudly_with_its_location() {
        let pack: crate::RulePack = toml::from_str(
            r#"
            [[rule]]
            id = "broken.block"
            match = { kind = "literal", pattern = "ERROR" }
            level = "warning"
            [rule.block]
            start = { kind = "regex", pattern = '([unclosed' }
        "#,
        )
        .expect("the schema accepts it; the regex is rejected at compile time");

        let classifier = crate::RuleClassifier::from_pack(&pack).expect("pack compiles");
        let err = block_detector(classifier.rules())
            .expect_err("an invalid pattern must not be silently dropped");

        let message = err.to_string();
        assert!(
            message.contains("broken.block"),
            "must name the rule, got: {message}"
        );
        assert!(
            message.contains("block.start"),
            "must name the field, got: {message}"
        );
    }
}
