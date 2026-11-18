//! Stage 3: the classifier.
//!
//! Every rule's pattern goes into one [`regex::RegexSet`], compiled once at load time.
//! Classification is then a single pass over the line regardless of how many
//! rules exist, which is what keeps the 200 MB/s target in AGENTS.md section 7
//! reachable: the work per line does not grow with the size of the pack.
//!
//! Matching is done against `Line::plain`, not `Line::raw`: ANSI escapes are
//! stripped before this stage runs (AGENTS.md section 2) and a rule written by a
//! human should never have to know that.

use std::collections::HashSet;

use hushlog_core::{Classified, Classifier, Level, Line, RuleId};
use regex::RegexSetBuilder;

use crate::error::{Location, RuleError};
use crate::schema::{BlockMarkers, LevelMap, Matcher, Rule, RuleLevel, RulePack, RuleSample};

/// Ceiling on rules in one classifier.
///
/// [`RuleId`] is a `u16`, so this is a hard limit, not a tuning knob. Exceeding
/// it is an error rather than a truncation: silently dropping rules means
/// silently losing errors.
pub const MAX_RULES: usize = u16::MAX as usize;

/// A rule that has been compiled and indexed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledRule {
    /// Index into the rule set. This is the [`RuleId`] handed to the pipeline.
    pub index: u16,
    /// The rule's id, exactly as written in the pack.
    pub id: String,
    /// The compiled regex source, stored so `capture_value` can re-run it to
    /// read named capture groups without re-validating at match time.
    pub pattern: String,
    /// Severity assigned to matching lines.
    pub level: Level,
    /// Name of the capture group whose value selects the level, if the pack
    /// declared one (lnav's `level-field`). The classifier consults
    /// [`Self::level_map`] when this is set.
    pub level_field: Option<String>,
    /// Level→pattern map used with [`Self::level_field`], tried most-severe
    /// first.
    pub level_map: Option<LevelMap>,
    /// Name of the capture group whose value becomes this line's operation id.
    ///
    /// When set, the classifier extracts this group from each matching line and
    /// carries it on [`hushlog_core::Classified::opid`]. The collapser folds
    /// lines only with the same opid, so `Retry 1/10 for pod abc-123` under
    /// `op-1` and the same masked line under `op-2` stay in separate fold
    /// groups -- two retry storms never collapse into one just because they
    /// look alike.
    pub opid: Option<String>,
    /// Whether the renderer should draw attention to matching lines.
    pub highlight: bool,
    /// Whether repeats of this line may be folded.
    pub collapse: bool,
    /// How many lines around a match belong to it.
    pub context_lines: Option<usize>,
    /// Explicit block delimiters, if the rule declared any.
    pub block: Option<BlockMarkers>,
    /// Where the rule came from, for `--explain` and for error messages.
    pub location: Location,
    /// Sample lines this rule must recognise, validated at load time.
    pub sample: Vec<RuleSample>,
}

/// Assigns a [`Level`] to each line using a set of compiled rules.
///
/// Construct with [`RuleClassifier::new`]. Compilation is the only place that
/// costs anything; [`Classifier::classify`] allocates nothing.
#[derive(Debug, Clone)]
pub struct RuleClassifier {
    set: regex::RegexSet,
    rules: Vec<CompiledRule>,
    /// Each rule's own pattern, compiled once at load time.
    ///
    /// Reading a named capture group means re-running the rule's pattern
    /// against the line. It used to be compiled from scratch on every
    /// matching line, and the level map compiled its patterns again on top:
    /// up to five regex compilations per line, on exactly the lines that
    /// needed a level field. That was most of what classification cost, and
    /// it contradicted the promise on [`RegexPattern`] that the classifier
    /// never touches `regex::Regex::new` on the hot path.
    ///
    /// Kept parallel to `rules` rather than inside [`CompiledRule`] so that
    /// `CompiledRule` keeps its derives and its public shape.
    regexes: Vec<regex::Regex>,
    /// Each rule's level map, compiled once at load time, parallel to `rules`.
    level_maps: Vec<Option<Vec<(RuleLevel, regex::Regex)>>>,
}

impl RuleClassifier {
    /// Compile every rule in every pack into one set.
    ///
    /// Packs are concatenated in the order given, and rules keep their
    /// declaration order within a pack. When two rules both match a line the
    /// earlier one wins, so a pack that needs a later rule to fire has to put
    /// it first. Ids must be unique across all packs.
    ///
    /// # Errors
    ///
    /// Returns [`RuleError`] if a pattern is not a valid regex, or if two rules
    /// share an id. Both are reported with the file and line they came from.
    pub fn new(packs: &[RulePack]) -> Result<Self, RuleError> {
        let total: usize = packs.iter().map(|p| p.rules.len()).sum();
        if total > MAX_RULES {
            return Err(RuleError::Rule {
                location: Location::file("<rule set>"),
                id: "<rule set>".to_string(),
                message: format!(
                    "{total} rules loaded, but a rule set holds at most {MAX_RULES}: \
                     a RuleId is a u16"
                ),
            });
        }

        let mut patterns: Vec<String> = Vec::with_capacity(total);
        let mut rules: Vec<CompiledRule> = Vec::with_capacity(total);
        let mut regexes: Vec<regex::Regex> = Vec::with_capacity(total);
        let mut level_maps: Vec<Option<Vec<(RuleLevel, regex::Regex)>>> = Vec::with_capacity(total);
        let mut seen: HashSet<&str> = HashSet::with_capacity(total);

        for pack in packs {
            for rule in &pack.rules {
                let pattern = compile(rule);

                // Compile once on its own so a bad regex can be attributed to a
                // rule. Building the set would otherwise report the first
                // failure with no idea which pack it came from.
                regex::Regex::new(&pattern).map_err(|e| RuleError::Rule {
                    location: pattern_location(rule).clone(),
                    id: rule.id_str().to_string(),
                    message: format!("invalid pattern: {}", first_line(&e.to_string())),
                })?;

                // When a rule declares an opid capture group, it must exist in
                // the pattern: a pack that names a group the regex never opens
                // would silently never produce an opid, which is exactly the
                // kind of silent loss this crate refuses (AGENTS.md section 1).
                if rule.opid.is_some() {
                    validate_opid_group(rule, &pattern)?;
                }

                // The level map's patterns are compiled here, so a bad one
                // fails at load time rather than silently matching nothing at
                // runtime -- and so the classifier does not compile them again
                // on every line the rule matches.
                let level_map = compile_level_map(rule)?;

                let id = rule.id_str();
                if !seen.insert(id) {
                    return Err(RuleError::Rule {
                        location: rule.location().clone(),
                        id: id.to_string(),
                        message: format!("duplicate rule id `{id}`"),
                    });
                }

                // Every declared sample must be recognised by this rule: a pack
                // that does not match its own samples fails loudly instead of
                // silently under-matching (AGENTS.md section 1).
                let re = regex::Regex::new(&pattern).map_err(|e| RuleError::Rule {
                    location: pattern_location(rule).clone(),
                    id: rule.id_str().to_string(),
                    message: format!("invalid pattern: {}", first_line(&e.to_string())),
                })?;
                for sample in &rule.sample {
                    if !re.is_match(&sample.line) {
                        return Err(RuleError::Rule {
                            location: rule.location().clone(),
                            id: id.to_string(),
                            message: format!(
                                "sample line does not match rule `{id}` pattern: {}",
                                escape_for_error(&sample.line)
                            ),
                        });
                    }
                    if let Some(expected) = sample.level {
                        let derived = re
                            .captures(&sample.line)
                            .and_then(|caps| {
                                rule.level_field
                                    .as_ref()
                                    .map(|s| s.get_ref().as_str())
                                    .and_then(|field| {
                                        caps.name(field).map(|m| m.as_str().to_string())
                                    })
                            })
                            .and_then(|value| level_from_map(level_map.as_ref(), &value))
                            .unwrap_or(rule.level.into());
                        if derived != Level::from(expected) {
                            return Err(RuleError::Rule {
                                location: rule.location().clone(),
                                id: id.to_string(),
                                message: format!(
                                    "sample line derives level `{derived:?}` but declares `{expected:?}`"
                                ),
                            });
                        }
                    }
                }

                rules.push(CompiledRule::from_rule(
                    index_of(rules.len()),
                    rule,
                    pattern.clone(),
                ));
                regexes.push(re);
                level_maps.push(level_map);
                patterns.push(pattern);
            }
        }

        // Every pattern above compiled on its own, so this cannot fail. Mapping
        // it to a typed error anyway keeps a surprise from becoming a panic in
        // the middle of a CI run.
        let set = RegexSetBuilder::new(&patterns)
            .build()
            .map_err(|e| RuleError::Rule {
                location: Location::file("<rule set>"),
                id: "<rule set>".to_string(),
                message: format!("could not build rule set: {}", first_line(&e.to_string())),
            })?;

        Ok(Self {
            set,
            rules,
            regexes,
            level_maps,
        })
    }

    /// Compile a single pack.
    ///
    /// # Errors
    ///
    /// As [`RuleClassifier::new`].
    pub fn from_pack(pack: &RulePack) -> Result<Self, RuleError> {
        Self::new(std::slice::from_ref(pack))
    }

    /// Every compiled rule, in load order. `rules()[i].index == i`.
    #[must_use]
    pub fn rules(&self) -> &[CompiledRule] {
        &self.rules
    }

    /// Resolve a [`RuleId`] back to the rule that produced it.
    ///
    /// Renderers and `--explain` need this: `RuleId` is an index precisely so
    /// classification stays allocation-free, so something has to map it back.
    #[must_use]
    pub fn rule(&self, id: RuleId) -> Option<&CompiledRule> {
        self.rules.get(usize::from(id.0))
    }

    /// The id of the rule a [`RuleId`] refers to, if it is in range.
    #[must_use]
    pub fn rule_name(&self, id: RuleId) -> Option<&str> {
        self.rule(id).map(|r| r.id.as_str())
    }
}

impl Classifier for RuleClassifier {
    fn classify(&mut self, line: Line) -> Classified {
        // `matches` returns a bitset over rule indices; the first set bit is the
        // lowest index, which is the first rule in load order. That is the whole
        // first-match-wins rule.
        //
        // `iter()` rather than `into_iter()` on purpose. Both walk the same
        // `Box<[bool]>`, but `into_iter` re-tests each slot through `contains`,
        // which bounds-checks on every element. Measured at 200 rules that is
        // about 2x slower, and at 1000 rules about 3x. `iter` skips straight to
        // the first set bit.
        match self.set.matches(&line.plain).iter().next() {
            Some(index) => {
                let index = index_of(index);
                let slot = usize::from(index);
                let rule = &self.rules[slot];
                // The rule's own pattern and its level map, compiled once at
                // load time. See `RuleClassifier::regexes`.
                let pattern = &self.regexes[slot];
                let level = rule
                    .level_field
                    .as_deref()
                    .and_then(|field| capture_from(pattern, &line.plain, field))
                    .and_then(|value| level_from_map(self.level_maps[slot].as_ref(), &value));
                let opid = rule
                    .opid
                    .as_deref()
                    .and_then(|field| capture_from(pattern, &line.plain, field));
                Classified {
                    level: level.unwrap_or(rule.level),
                    rule_id: Some(RuleId(index)),
                    highlight: rule.highlight,
                    line,
                    opid,
                }
            }
            // A line no rule matched is ordinary output. Saying so here does not
            // lose it: the run summary accounts for every line that came in.
            None => Classified {
                level: Level::Info,
                rule_id: None,
                highlight: false,
                line,
                opid: None,
            },
        }
    }
}

/// Narrow a rule index to the `u16` a [`RuleId`] holds.
///
/// [`RuleClassifier::new`] refuses more than [`MAX_RULES`] rules before it gets
/// here, so this cannot truncate. It is written as a conversion rather than an
/// `as` so that a future change to that check fails to compile rather than
/// quietly reassigning the wrong rule.
fn index_of(index: usize) -> u16 {
    u16::try_from(index).expect("rule count is bounded by MAX_RULES before indexing")
}

/// Read a named capture group out of a line, using an already-compiled rule.
///
/// The rule's pattern was compiled at load time, so this is a match, not a
/// compilation.
fn capture_from(pattern: &regex::Regex, line: &str, group: &str) -> Option<String> {
    pattern
        .captures(line)?
        .name(group)
        .map(|m| m.as_str().to_string())
}

/// Compile a rule's level map, failing loudly on a bad pattern.
///
/// The map's patterns used to be compiled on the hot path, once per line the
/// rule matched, so a broken one failed silently at runtime: the rule fell
/// back to its declared level and nobody was told. Now it fails at load time,
/// naming the rule and its file.
fn compile_level_map(rule: &Rule) -> Result<Option<Vec<(RuleLevel, regex::Regex)>>, RuleError> {
    let Some(map) = &rule.level_map else {
        return Ok(None);
    };
    let mut out = Vec::with_capacity(map.entries.len());
    for (level, pattern) in &map.entries {
        let re = case_insensitive(&pattern.0).map_err(|e| RuleError::Rule {
            location: rule.location().clone(),
            id: rule.id_str().to_string(),
            message: format!(
                "level_map pattern `{}` is not a valid regex: {e}",
                pattern.0
            ),
        })?;
        out.push((*level, re));
    }
    Ok(Some(out))
}

/// Compile a level-map pattern so that it matches any spelling of the level.
///
/// Level names are case-insensitive everywhere else in hushlog: the schema
/// accepts `ERROR` and `error` alike, and [`Level::from_token`] folds the
/// aliases. A level map that only matched one spelling was a trap, and it had
/// already caught one -- the OpenTelemetry pack, whose `severity` field is
/// uppercase by definition, so its level map could never fire. The rule fell
/// back to its declared level for every line, and the pack's own samples
/// still passed, because the sample check did not fold case and the runtime
/// path did.
///
/// Folding case in the pattern rather than in the value keeps both paths
/// agreeing, and keeps a pack author's pattern readable.
fn case_insensitive(pattern: &str) -> Result<regex::Regex, regex::Error> {
    regex::Regex::new(&format!("(?i){pattern}"))
}

/// The level a compiled level map assigns to a value.
///
/// Mirrors [`CompiledRule::level_for`], but over patterns that were compiled
/// once rather than on every line.
fn level_from_map(map: Option<&Vec<(RuleLevel, regex::Regex)>>, value: &str) -> Option<Level> {
    let map = map?;
    let value = value.to_ascii_lowercase();
    map.iter()
        .find_map(|(level, re)| re.is_match(&value).then_some(Level::from(*level)))
}

impl CompiledRule {
    fn from_rule(index: u16, rule: &Rule, pattern: String) -> Self {
        Self {
            index,
            id: rule.id_str().to_string(),
            pattern,
            level: rule.level.into(),
            level_field: rule.level_field.as_ref().map(|s| s.get_ref().clone()),
            level_map: rule.level_map.clone(),
            highlight: rule.highlight,
            collapse: rule.collapse,
            context_lines: rule.context_lines,
            block: rule.block.clone(),
            location: rule.location().clone(),
            sample: rule.sample.clone(),
            opid: rule.opid.as_ref().map(|s| s.get_ref().clone()),
        }
    }

    /// The value of a rule's named capture group in a line, if the rule
    /// declared one and it took part in the match.
    ///
    /// Re-runs just this rule's pattern because [`regex::RegexSet`] matches report
    /// indices but not capture text. The pattern already compiled successfully
    /// at load time, so this cannot fail.
    #[must_use]
    pub fn capture_value(&self, line: &str, group: &str) -> Option<String> {
        regex::Regex::new(&self.pattern)
            .ok()?
            .captures(line)?
            .name(group)
            .map(|m| m.as_str().to_string())
    }

    /// The level derived from `value` by testing it against this rule's
    /// [`level_map`](Self::level_map), most-severe first. Returns `None` when
    /// nothing matches, so the rule's declared [`level`](Self::level) applies.
    ///
    /// Matching is case-insensitive: `ERROR`, `Error` and `error` all match the
    /// `error` pattern, mirroring lnav's level table.
    ///
    /// This compiles the map's patterns on demand. The classifier itself uses
    /// the copy it compiled at load time.
    #[must_use]
    pub fn level_for(&self, value: &str) -> Option<Level> {
        let value = value.to_ascii_lowercase();
        self.level_map
            .as_ref()?
            .entries
            .iter()
            .find_map(|(level, pattern)| {
                case_insensitive(&pattern.0)
                    .ok()?
                    .is_match(&value)
                    .then_some(Level::from(*level))
            })
    }
}

/// The regex source for a rule. Literals are escaped so they match literally.
fn compile(rule: &Rule) -> String {
    match &rule.matcher {
        Matcher::Regex { pattern } => pattern.get_ref().clone(),
        Matcher::Literal { pattern } => regex::escape(pattern.get_ref()),
    }
}

/// When a rule declares an opid capture group, it must exist in the pattern:
/// a pack that names a group the regex never opens would silently never
/// produce an opid, which is exactly the kind of silent loss this crate
/// refuses (AGENTS.md section 1).
///
/// # Errors
///
/// Returns [`RuleError`] naming the rule and its source location when the
/// declared capture group is not present in the pattern.
fn validate_opid_group(rule: &Rule, pattern: &str) -> Result<(), RuleError> {
    let Some(group) = &rule.opid else {
        return Ok(());
    };
    let re = regex::Regex::new(pattern).map_err(|e| RuleError::Rule {
        location: pattern_location(rule).clone(),
        id: rule.id_str().to_string(),
        message: format!("invalid pattern: {}", first_line(&e.to_string())),
    })?;
    if re.capture_names().all(|name| name != Some(group.get_ref())) {
        return Err(RuleError::Rule {
            location: rule.location().clone(),
            id: rule.id_str().to_string(),
            message: format!(
                "rule `{}` declares opid capture group {} but the pattern has no group of that name",
                rule.id_str(),
                group.get_ref()
            ),
        });
    }
    Ok(())
}

/// Prefer the pattern's own span, so the error points at the regex, not the id.
fn pattern_location(rule: &Rule) -> &Location {
    rule.pattern_location()
}

/// The `regex` crate puts its explanation on several lines; our errors are one.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).trim().to_string()
}

use std::fmt::Write as _;

/// Escape a sample line so it can be embedded in an error message without
/// swallowing the surrounding punctuation.
fn escape_for_error(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for ch in line.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => {
                let _ = write!(out, "U+{:04X}", ch as u32);
            }
            ch => out.push(ch),
        }
    }
    out
}

/// Test `value` against a level map's entries, most-severe first, returning the
/// first level whose pattern matches. `None` when nothing matches.
///
/// The classifier uses [`level_from_map`] instead, over patterns compiled once
/// at load time. This stays for callers holding a raw [`LevelMap`].
#[allow(dead_code)]
fn level_for_map(map: &LevelMap, value: &str) -> Option<Level> {
    let value = value.to_ascii_lowercase();
    map.entries.iter().find_map(|(level, pattern)| {
        case_insensitive(&pattern.0)
            .ok()?
            .is_match(&value)
            .then_some(Level::from(*level))
    })
}
