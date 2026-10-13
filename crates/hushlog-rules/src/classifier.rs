//! Stage 3: the classifier.
//!
//! Every rule's pattern goes into one [`RegexSet`], compiled once at load time.
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
use crate::schema::{BlockMarkers, Matcher, Rule, RulePack};

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
    /// Severity assigned to matching lines.
    pub level: Level,
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
}

/// Assigns a [`Level`] to each line using a set of compiled rules.
///
/// Construct with [`RuleClassifier::new`]. Compilation is the only place that
/// costs anything; [`Classifier::classify`] allocates nothing.
#[derive(Debug, Clone)]
pub struct RuleClassifier {
    set: regex::RegexSet,
    rules: Vec<CompiledRule>,
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

                let id = rule.id_str();
                if !seen.insert(id) {
                    return Err(RuleError::Rule {
                        location: rule.location().clone(),
                        id: id.to_string(),
                        message: format!("duplicate rule id `{id}`"),
                    });
                }

                patterns.push(pattern);
                rules.push(CompiledRule::from_rule(index_of(rules.len()), rule));
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

        Ok(Self { set, rules })
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
                let rule = &self.rules[usize::from(index)];
                Classified {
                    level: rule.level,
                    rule_id: Some(RuleId(index)),
                    highlight: rule.highlight,
                    line,
                }
            }
            // A line no rule matched is ordinary output. Saying so here does not
            // lose it: the run summary accounts for every line that came in.
            None => Classified {
                level: Level::Info,
                rule_id: None,
                highlight: false,
                line,
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

impl CompiledRule {
    fn from_rule(index: u16, rule: &Rule) -> Self {
        Self {
            index,
            id: rule.id_str().to_string(),
            level: rule.level.into(),
            highlight: rule.highlight,
            collapse: rule.collapse,
            context_lines: rule.context_lines,
            block: rule.block.clone(),
            location: rule.location().clone(),
        }
    }
}

/// The regex source for a rule. Literals are escaped so they match literally.
fn compile(rule: &Rule) -> String {
    match &rule.matcher {
        Matcher::Regex { pattern } => pattern.get_ref().clone(),
        Matcher::Literal { pattern } => regex::escape(pattern.get_ref()),
    }
}

/// Prefer the pattern's own span, so the error points at the regex, not the id.
fn pattern_location(rule: &Rule) -> &Location {
    rule.pattern_location()
}

/// The `regex` crate puts its explanation on several lines; our errors are one.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).trim().to_string()
}
