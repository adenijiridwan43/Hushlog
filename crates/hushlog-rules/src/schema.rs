//! The rule-pack schema: what a TOML file is allowed to say.
//!
//! Everything here is `deny_unknown_fields`. A typo in a pack is far more
//! dangerous than an error, because the pack author believes the rule is doing
//! something it is not -- see AGENTS.md section 1.

use serde::de::{self, Error, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use toml::Spanned;

use crate::error::Location;

/// A whole rule pack: one `rules/*.toml` file.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulePack {
    /// Pack name. Defaults to the file stem when omitted.
    pub name: Option<String>,
    /// Free-text note for whoever reads the pack later.
    pub description: Option<String>,
    /// The rules, in declaration order. Declaration order decides which rule
    /// wins when more than one matches; see [`Matcher`].
    ///
    /// Spelled `[[rule]]` in TOML, singular, because that is what Cargo does
    /// for its own array-of-tables and every other Rust tool does too. This
    /// field is the only place the two spellings differ.
    #[serde(rename = "rule")]
    pub rules: Vec<Rule>,
    /// Where this pack was loaded from, for error messages.
    #[serde(skip)]
    location: Location,
}

impl RulePack {
    /// Human-readable origin of this pack, used in diagnostics.
    #[must_use]
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// Record where the pack was loaded from. Set by the loader, not by serde.
    pub(crate) fn set_location(&mut self, location: Location) {
        self.location = location;
    }
}

/// One rule.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Unique across every loaded pack. Rendered by `--explain`, so it should
    /// read as a sentence fragment: `cargo.deprecated-use`.
    pub id: Spanned<String>,
    /// How a line is recognised.
    ///
    /// Spelled `match` in TOML, which is the name the pipeline diagram in
    /// AGENTS.md section 2 uses. `matcher` in Rust, because `match` is a
    /// keyword and would need `r#match` everywhere it is read.
    #[serde(rename = "match")]
    pub matcher: Matcher,
    /// Severity assigned to matching lines.
    pub level: RuleLevel,
    /// Ask the renderer to draw attention to this line. Off by default: a pack
    /// has to opt in to being loud.
    #[serde(default)]
    pub highlight: bool,
    /// Permit folding repeats of this line. Off by default, and refused
    /// outright on `Level::Error` -- see AGENTS.md section 1.1.
    #[serde(default)]
    pub collapse: bool,
    /// How many lines around a match belong to it.
    #[serde(default)]
    pub context_lines: Option<usize>,
    /// Explicit multi-line block delimiters, for logs where indentation
    /// heuristics are not enough (stack traces, deprecation paragraphs).
    #[serde(default)]
    pub block: Option<BlockMarkers>,
    /// Name of the capture group whose value selects the level, for formats
    /// whose severity varies line to line -- lnav's `level-field`.
    ///
    /// Only meaningful with [`Self::level_map`]; the classifier reads the
    /// capture, tests it against each level's pattern, and uses the first
    /// match. A line whose level field matches nothing falls back to
    /// [`Self::level`].
    #[serde(default)]
    pub level_field: Option<Spanned<String>>,
    /// Maps a level to the pattern that identifies it in [`Self::level_field`].
    ///
    /// Mirrors lnav's `level` object: the key is the level name, the value is
    /// a regular expression matched against the level field's value. Keys are
    /// matched case-insensitively. Patterns are tried most-severe first, so the
    /// order they are written in does not matter.
    #[serde(default)]
    pub level_map: Option<LevelMap>,
    /// Sample lines that must be recognised by this rule.
    ///
    /// Every sample is checked when the rule set is built: the rule's pattern
    /// must match the line, and when the sample declares a `level`, the level
    /// the rule derives for it must equal it. A pack that does not recognise
    /// its own samples fails loudly instead of silently under-matching -- the
    /// same "fail loudly" contract as the rest of the loader (AGENTS.md §1).
    #[serde(default)]
    pub sample: Vec<RuleSample>,
    /// Where this rule sits in the source, for error messages.
    #[serde(skip)]
    location: Location,
    /// Where this rule's pattern sits. Finer-grained than `location`, so a bad
    /// regex is reported against the regex rather than against the id above it.
    #[serde(skip)]
    pattern_location: Location,
}

impl Rule {
    /// Human-readable origin of this rule, used in diagnostics.
    #[must_use]
    pub fn location(&self) -> &Location {
        &self.location
    }

    /// The rule's id, including the span it occupied in the pack.
    #[must_use]
    pub fn id(&self) -> &Spanned<String> {
        &self.id
    }

    /// The rule's id, without the source span.
    #[must_use]
    pub fn id_str(&self) -> &str {
        self.id.get_ref()
    }

    /// Record where the rule sits in its pack. Set by the loader, not by serde.
    pub(crate) fn set_location(&mut self, location: Location) {
        self.location = location;
    }

    /// Where this rule's pattern sits in its pack.
    #[must_use]
    pub fn pattern_location(&self) -> &Location {
        &self.pattern_location
    }

    /// Record where the pattern sits. Set by the loader, not by serde.
    pub(crate) fn set_pattern_location(&mut self, location: Location) {
        self.pattern_location = location;
    }
}

/// How a rule matches a line.
///
/// Deliberately a tagged enum rather than a bare string. `match = "x"` reads as
/// "contains x" to most people and as "regex" to the regex crate, and a pack
/// author who guesses wrong gets silence instead of a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Matcher {
    /// A Rust `regex` crate pattern, matched anywhere in the line.
    Regex { pattern: Spanned<String> },
    /// A plain substring. Metacharacters mean themselves.
    Literal { pattern: Spanned<String> },
}

impl Matcher {
    /// The pattern as written in the pack.
    #[must_use]
    pub fn pattern(&self) -> &str {
        match self {
            Self::Regex { pattern } | Self::Literal { pattern } => pattern.get_ref(),
        }
    }

    /// Where the pattern sits in the source file.
    #[must_use]
    pub fn span(&self) -> std::ops::Range<usize> {
        match self {
            Self::Regex { pattern } | Self::Literal { pattern } => pattern.span(),
        }
    }
}

impl<'de> Deserialize<'de> for Matcher {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        de.deserialize_map(MatcherVisitor)
    }
}

struct MatcherVisitor;

impl<'de> Visitor<'de> for MatcherVisitor {
    type Value = Matcher;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "a matcher table with `kind` and `pattern`, \
             e.g. { kind = \"regex\", pattern = \"^error\" } or \
             { kind = \"literal\", pattern = \"npm WARN\" }",
        )
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut kind: Option<Spanned<String>> = None;
        let mut pattern: Option<Spanned<String>> = None;

        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => set_once(&mut kind, map.next_value()?, "kind")?,
                "pattern" => set_once(&mut pattern, map.next_value()?, "pattern")?,
                other => return Err(de::Error::unknown_field(other, &["kind", "pattern"])),
            }
        }

        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        let pattern = pattern.ok_or_else(|| de::Error::missing_field("pattern"))?;

        match kind.get_ref().as_str() {
            "regex" => Ok(Matcher::Regex { pattern }),
            "literal" => Ok(Matcher::Literal { pattern }),
            other => Err(de::Error::custom(format!(
                "unknown matcher kind `{other}`, expected `regex` or `literal`"
            ))),
        }
    }
}

/// Store `value` in `slot`, or complain that the field was given twice.
fn set_once<T, E: de::Error>(slot: &mut Option<T>, value: T, field: &'static str) -> Result<(), E> {
    if slot.is_some() {
        return Err(de::Error::duplicate_field(field));
    }
    *slot = Some(value);
    Ok(())
}

/// Explicit delimiters for a multi-line block.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockMarkers {
    /// Opens the block.
    pub start: Matcher,
    /// Closes it. When absent, the block runs to the end of the input.
    #[serde(default)]
    pub end: Option<Matcher>,
}

/// Severity a rule assigns.
///
/// Serde's own case-sensitive matching would reject `WARNING` in a pack a human
/// wrote, so matching is case-insensitive. Everything else about it is strict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleLevel {
    Error,
    Warning,
    Info,
    Progress,
    Trace,
}

impl RuleLevel {
    /// Every level, in descending severity. Used to build error messages.
    pub const ALL: [Self; 5] = [
        Self::Error,
        Self::Warning,
        Self::Info,
        Self::Progress,
        Self::Trace,
    ];

    /// The canonical lowercase spelling, as it appears in a pack.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Progress => "progress",
            Self::Trace => "trace",
        }
    }
}

impl std::fmt::Display for RuleLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<RuleLevel> for hushlog_core::Level {
    fn from(level: RuleLevel) -> Self {
        match level {
            RuleLevel::Error => Self::Error,
            RuleLevel::Warning => Self::Warning,
            RuleLevel::Info => Self::Info,
            RuleLevel::Progress => Self::Progress,
            RuleLevel::Trace => Self::Trace,
        }
    }
}

impl std::str::FromStr for RuleLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        RuleLevel::ALL
            .into_iter()
            .find(|level| level.as_str().eq_ignore_ascii_case(s))
            .ok_or_else(|| format!("unknown level `{s}`, expected {}", level_list()))
    }
}

impl<'de> Deserialize<'de> for RuleLevel {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        de.deserialize_str(LevelVisitor)
    }
}

struct LevelVisitor;

impl Visitor<'_> for LevelVisitor {
    type Value = RuleLevel;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "one of: {}", level_list())
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        // `eq_ignore_ascii_case` rather than `to_lowercase`: level names are
        // ASCII and this cannot allocate on the load path.
        RuleLevel::ALL
            .into_iter()
            .find(|level| level.as_str().eq_ignore_ascii_case(v))
            .ok_or_else(|| {
                de::Error::custom(format!("unknown level `{v}`, expected {}", level_list()))
            })
    }
}

/// `error, warning, info, progress, trace`
fn level_list() -> String {
    RuleLevel::ALL.map(RuleLevel::as_str).join(", ")
}

/// A compiled `level` map, in most-severe-first order.
///
/// Mirrors lnav's `level` object. Entries are tried in order and the first
/// pattern that matches the level field's value wins, so ordering them
/// most-severe first means severity wins regardless of how the pack author
/// wrote the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelMap {
    pub entries: Vec<(RuleLevel, RegexPattern)>,
}

/// A compiled regular expression, built once at pack load so the classifier
/// never touches `regex::Regex::new` on the hot path.
///
/// Deserialises from a bare TOML string and validates that it is a usable
/// regex at load time -- a bad pattern fails the pack loudly instead of
/// failing on the first line it meets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexPattern(pub String);

impl<'de> Deserialize<'de> for RegexPattern {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        regex::Regex::new(&s)
            .map_err(|e| Error::custom(first_line(&e.to_string())))?;
        Ok(RegexPattern(s))
    }
}

/// A sample line that this rule must recognise.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSample {
    /// The exact line text.
    pub line: String,
    /// The level the rule is expected to derive for this line, if any.
    pub level: Option<RuleLevel>,
    /// Free-text note about what this sample exercises.
    #[serde(default)]
    pub description: Option<String>,
}

impl<'de> Deserialize<'de> for LevelMap {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        use serde::de::{MapAccess, Visitor};

        struct LevelMapVisitor;

        impl<'de> Visitor<'de> for LevelMapVisitor {
            type Value = LevelMap;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map of level name to pattern, e.g. { error = \"^err\" }")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut pairs: Vec<(RuleLevel, RegexPattern)> = Vec::new();
                while let Some((key, pattern)) = map.next_entry::<String, RegexPattern>()? {
                    let level = key.parse::<RuleLevel>().map_err(de::Error::custom)?;
                    pairs.push((level, pattern));
                }
                // Most-severe first: the classifier tries entries in order and
                // stops at the first match, so severe levels win regardless of
                // how the pack author ordered the table.
                pairs.sort_by_key(|(level, _)| *level as u8);
                Ok(LevelMap {
                    entries: pairs,
                })
            }
        }

        de.deserialize_map(LevelMapVisitor)
    }
}

/// The `regex` crate puts its explanation on several lines; our errors are one.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).trim().to_string()
}
