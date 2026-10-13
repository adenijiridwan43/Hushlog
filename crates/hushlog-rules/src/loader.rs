//! Reading rule packs off disk, and refusing the ones that do not make sense.
//!
//! AGENTS.md section 1: an invalid rule must fail loudly and say where. There
//! is no "skip the broken rule and carry on" path here, because a pack that
//! silently drops its `error` rule is a pack that silently hides errors.

use std::collections::HashSet;
use std::path::Path;

use crate::error::{Location, RuleError};
use crate::schema::{Rule, RuleLevel, RulePack};

/// Parse one rule pack from a string, naming it `file` in any error.
///
/// `file` is a display string, not necessarily a real path: tests and embedded
/// packs pass whatever is useful to read in the message.
///
/// # Errors
///
/// Returns [`RuleError::Syntax`] if the TOML is malformed, contains an unknown
/// field, or breaks the schema, and [`RuleError::Rule`] if a rule is
/// semantically wrong -- a blank id, a duplicate id, or an attempt to collapse
/// errors. Nothing is skipped: the first bad rule fails the whole pack.
pub fn parse_pack(file: &str, source: &str) -> Result<RulePack, RuleError> {
    let mut pack: RulePack = toml::from_str(source).map_err(|e| syntax_error(file, source, &e))?;

    if pack.rules.is_empty() {
        return Err(RuleError::Syntax {
            location: Location::file(file),
            message: "rule pack defines no rules".to_string(),
        });
    }

    if pack.name.is_none() {
        pack.name = file_stem(file);
    }
    pack.set_location(Location::file(file));

    let mut seen: HashSet<&str> = HashSet::with_capacity(pack.rules.len());
    for rule in &mut pack.rules {
        validate_rule(file, source, rule, &mut seen)?;
    }

    Ok(pack)
}

/// Read and validate one rule pack from disk.
///
/// # Errors
///
/// Returns [`RuleError::Io`] if the file cannot be read, or the same schema and
/// rule errors [`parse_pack`] reports.
pub fn load_pack(path: impl AsRef<Path>) -> Result<RulePack, RuleError> {
    let path = path.as_ref();
    let source = std::fs::read_to_string(path).map_err(|source| RuleError::Io {
        location: Location::from_path(path),
        source,
    })?;

    parse_pack(&path.display().to_string(), &source)
}

/// Read and validate several packs in order. Stops at the first bad one.
///
/// # Errors
///
/// As [`load_pack`]. Pack order is significant: it decides which rule wins when
/// two match the same line.
pub fn load_packs<I, P>(paths: I) -> Result<Vec<RulePack>, RuleError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    paths.into_iter().map(load_pack).collect()
}

/// Everything a rule must get right before it is allowed to run.
fn validate_rule<'a>(
    file: &str,
    source: &str,
    rule: &'a mut Rule,
    seen: &mut HashSet<&'a str>,
) -> Result<(), RuleError> {
    rule.set_location(Location::from_span(file, source, Some(rule.id().span())));
    rule.set_pattern_location(Location::from_span(file, source, Some(rule.matcher.span())));

    let id = rule.id_str();
    let location = rule.location().clone();

    let invalid = |message: String| RuleError::Rule {
        location: location.clone(),
        id: id.to_string(),
        message,
    };

    if id.is_empty() || id.trim().is_empty() {
        return Err(invalid("`id` must not be blank".to_string()));
    }
    if id.chars().any(char::is_whitespace) {
        return Err(invalid(
            "`id` must not contain whitespace; it is shown in `--explain` output".to_string(),
        ));
    }
    if !seen.insert(id) {
        return Err(invalid(format!("duplicate rule id `{id}` in this pack")));
    }

    // AGENTS.md section 1.1. A pack is not a user opt-in, so a pack asking for
    // errors to be folded is refused here rather than quietly honoured.
    if rule.collapse && rule.level == RuleLevel::Error {
        return Err(invalid(
            "`collapse = true` is not allowed on a rule with `level = \"error\"`; \
             errors are never collapsed without an explicit user opt-in \
             (AGENTS.md section 1.1)"
                .to_string(),
        ));
    }

    if rule.context_lines == Some(0) {
        return Err(invalid(
            "`context_lines` must be at least 1 when present".to_string(),
        ));
    }

    Ok(())
}

/// Wrap a `toml` parse failure, keeping the file and line the caret points at.
fn syntax_error(file: &str, source: &str, error: &toml::de::Error) -> RuleError {
    RuleError::Syntax {
        location: Location::from_span(file, source, error.span()),
        // `Display` on a toml error prints the whole multi-line diagnostic
        // block. The location is already carried separately and rendered in
        // our own prefix, so only the message itself is carried across.
        message: error.message().to_string(),
    }
}

/// `some/dir/cargo.toml` -> `Some("cargo")`
fn file_stem(file: &str) -> Option<String> {
    Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
}
