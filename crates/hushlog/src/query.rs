//! `--query` mode: a small predicate language over a classified line.
//!
//! `hushlog --query 'level == error && plain =~ "retry"'` prints only the lines
//! the predicate matches, as CSV or JSON. The log is still parsed end to end
//! (ingest, normalize, classify) but nothing is folded -- only matching
//! records are emitted. It is the headless query mode, comparable to
//! lnav's `-n -c`.
//!
//! Grammar (precedence, low to high):
//!
//! ```text
//! `expr`     := `or_expr`
//! `or_expr`  := `and_expr` ( "||" | "or" `and_expr` )*
//! `and_expr` := `not_expr` ( "&&" | "and" `not_expr` )*
//! `not_expr` := "!" `not_expr` | `comparison`
//! `comparison` := `field` ("==" | "!=" | "=~" | "!~" | ">" | "<" | ">=" | "<=") `value`
//! `field`    := "level" | "stream" | "no" | "raw" | "plain" | "rule_id"
//! `value`    := `STRING` | `NUMBER` | `WORD`
//! ```
//!
//! `WORD` is a bare identifier, handy for `level == error`. The logical words
//! `and` and `or` are reserved. String values must be double-quoted; a bare
//! word is treated as a string. `rule_id` compares numerically.

use std::borrow::Cow;

use hushlog_core::{Classified, Level, Stream};

/// A parsed query, ready to evaluate against many lines.
#[derive(Debug, Clone)]
pub struct Query {
    expr: Expr,
}

impl Query {
    /// Parse `input` into a query.
    ///
    /// # Errors
    ///
    /// Returns a [`QueryError`] naming the byte offset when `input` is not a
    /// valid query expression.
    pub fn parse(input: &str) -> Result<Self, QueryError> {
        let mut parser = Parser::new(input);
        let expr = parser.parse_expr()?;
        parser.expect_end()?;
        Ok(Self { expr })
    }

    /// Evaluate the predicate against one classified line.
    #[must_use]
    pub fn matches(&self, classified: &Classified) -> bool {
        match &self.expr {
            Expr::And(a, b) => a.matches(classified) && b.matches(classified),
            Expr::Or(a, b) => a.matches(classified) || b.matches(classified),
            Expr::Not(e) => !e.matches(classified),
            Expr::Compare(c) => c.matches(classified),
        }
    }
}

/// One node of the parsed expression tree.
#[derive(Debug, Clone)]
enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Compare(Compare),
}

impl Expr {
    fn matches(&self, c: &Classified) -> bool {
        match self {
            Expr::And(a, b) => a.matches(c) && b.matches(c),
            Expr::Or(a, b) => a.matches(c) || b.matches(c),
            Expr::Not(e) => !e.matches(c),
            Expr::Compare(cmp) => cmp.matches(c),
        }
    }
}

/// A single field comparison.
#[derive(Debug, Clone)]
struct Compare {
    field: Field,
    op: Op,
    /// For `=~` / `!~`, the compiled regex. For everything else the literal.
    value: Value,
    regex: Option<regex::Regex>,
}

impl Compare {
    fn matches(&self, c: &Classified) -> bool {
        match &self.op {
            Op::Eq => self.field_value(c) == self.value.as_str(),
            Op::Ne => self.field_value(c) != self.value.as_str(),
            Op::Gt | Op::Lt | Op::Ge | Op::Le => {
                let Some(no) = self.field_no(c) else {
                    return false; // ordering only defined on `no`
                };
                let num = match &self.value {
                    Value::Num(n) => i128::from(*n),
                    Value::Str(s) => match s.parse::<i128>() {
                        Ok(n) => n,
                        Err(_) => return false,
                    },
                };
                match self.op {
                    Op::Gt => i128::from(no) > num,
                    Op::Lt => i128::from(no) < num,
                    Op::Ge => i128::from(no) >= num,
                    Op::Le => i128::from(no) <= num,
                    _ => unreachable!(),
                }
            }
            Op::RegexMatch | Op::RegexNotMatch => {
                let re = self.regex.as_ref().expect("regex compiled at parse");
                let text = self.field_value(c);
                let hit = re.is_match(&text);
                matches!(self.op, Op::RegexMatch) == hit
            }
        }
    }

    /// The field's value as a string, for equality and regex matching.
    fn field_value(&self, c: &Classified) -> String {
        match self.field {
            Field::Level => level_name(c.level).to_string(),
            Field::Stream => stream_name(c.line.stream).to_string(),
            Field::No => c.line.no.to_string(),
            Field::Raw => c.line.raw.clone(),
            Field::Plain => c.line.plain.clone(),
            Field::RuleId => match c.rule_id {
                Some(id) => id.0.to_string(),
                None => String::new(),
            },
        }
    }

    /// The field's numeric line number, if the field is `no`.
    fn field_no(&self, c: &Classified) -> Option<u64> {
        if matches!(self.field, Field::No) {
            return Some(c.line.no);
        }
        None
    }
}

/// A field that can be compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Level,
    Stream,
    No,
    Raw,
    Plain,
    RuleId,
}

impl Field {
    fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "level" => Field::Level,
            "stream" => Field::Stream,
            "no" => Field::No,
            "raw" => Field::Raw,
            "plain" => Field::Plain,
            "rule_id" => Field::RuleId,
            _ => return None,
        })
    }
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    RegexMatch,
    RegexNotMatch,
    Gt,
    Lt,
    Ge,
    Le,
}

/// A literal value.
#[derive(Debug, Clone)]
enum Value {
    Str(String),
    Num(u64),
}

impl Value {
    fn as_str(&self) -> Cow<'_, str> {
        match self {
            Value::Str(s) => Cow::Borrowed(s),
            Value::Num(n) => Cow::Owned(n.to_string()),
        }
    }
}

/// A query that failed to parse, with the offending byte offset.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid query at byte {offset}: {message}")]
pub struct QueryError {
    pub offset: usize,
    pub message: String,
}

/// Recursive-descent parser over a `&str`.
struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    /// Byte offset of the next char to read.
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            chars: input.chars().peekable(),
            pos: 0,
        }
    }

    /// Byte offset of the current (peeked) character.
    fn offset(&self) -> usize {
        self.pos
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            let _ = self.next();
        }
    }

    fn expect_end(&mut self) -> Result<(), QueryError> {
        self.skip_ws();
        if self.peek().is_some() {
            return Err(self.err("unexpected character after expression"));
        }
        Ok(())
    }

    fn err(&self, message: impl Into<String>) -> QueryError {
        QueryError {
            offset: self.offset(),
            message: message.into(),
        }
    }

    /// `expr` := `or_expr`
    fn parse_expr(&mut self) -> Result<Expr, QueryError> {
        self.parse_or()
    }

    /// `or_expr` := `and_expr` ( "||" | "or" `and_expr` )*
    fn parse_or(&mut self) -> Result<Expr, QueryError> {
        let mut left = self.parse_and()?;
        loop {
            self.skip_ws();
            if !self.match_text("||") && !self.match_word("or") {
                break;
            }
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// `and_expr` := `not_expr` ( "&&" | "and" `not_expr` )*
    fn parse_and(&mut self) -> Result<Expr, QueryError> {
        let mut left = self.parse_not()?;
        loop {
            self.skip_ws();
            if !self.match_text("&&") && !self.match_word("and") {
                break;
            }
            let right = self.parse_not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// `not_expr` := "!" `not_expr` | `comparison`
    /// comparison := "(" expr ")" | field op value
    fn parse_not(&mut self) -> Result<Expr, QueryError> {
        self.skip_ws();
        if self.peek() == Some('(') {
            self.next();
            let inner = self.parse_expr()?;
            self.skip_ws();
            if self.next() != Some(')') {
                return Err(self.err("expected ')'"));
            }
            return Ok(inner);
        }
        if self.peek() == Some('!') {
            self.next();
            let inner = self.parse_not()?;
            return Ok(Expr::Not(Box::new(inner)));
        }
        self.parse_comparison()
    }

    /// `comparison` := `field` op `value`
    fn parse_comparison(&mut self) -> Result<Expr, QueryError> {
        self.skip_ws();
        let field = self.parse_field()?;
        self.skip_ws();
        let op = self.parse_op()?;
        self.skip_ws();
        let (value, regex) = self.parse_value(op)?;
        Ok(Expr::Compare(Compare {
            field,
            op,
            value,
            regex,
        }))
    }

    fn parse_field(&mut self) -> Result<Field, QueryError> {
        self.skip_ws();
        let word = self.read_word()?;
        Field::from_word(&word).ok_or_else(|| {
            self.err(format!("expected a field (level, stream, no, raw, plain, rule_id), found {word:?}"))
        })
    }

    fn parse_op(&mut self) -> Result<Op, QueryError> {
        self.skip_ws();
        let c = self.peek().ok_or_else(|| self.err("expected a comparison operator"))?;
        let op = match c {
            '=' => {
                self.next();
                if self.peek() == Some('=') {
                    self.next();
                    Op::Eq
                } else if self.peek() == Some('~') {
                    self.next();
                    Op::RegexMatch
                } else {
                    return Err(self.err("expected '==' or '=~'"));
                }
            }
            '!' => {
                self.next();
                if self.peek() == Some('=') {
                    self.next();
                    Op::Ne
                } else if self.peek() == Some('~') {
                    self.next();
                    Op::RegexNotMatch
                } else {
                    return Err(self.err("expected '!=' or '!~'"));
                }
            }
            '>' => {
                self.next();
                if self.peek() == Some('=') {
                    self.next();
                    Op::Ge
                } else {
                    Op::Gt
                }
            }
            '<' => {
                self.next();
                if self.peek() == Some('=') {
                    self.next();
                    Op::Le
                } else {
                    Op::Lt
                }
            }
            '~' => {
                self.next();
                if self.peek() == Some('=') {
                    self.next();
                    Op::RegexMatch
                } else {
                    return Err(self.err("expected '=~'"));
                }
            }
            _ => return Err(self.err(format!("unexpected operator character {c:?}"))),
        };
        Ok(op)
    }

    fn parse_value(&mut self, op: Op) -> Result<(Value, Option<regex::Regex>), QueryError> {
        self.skip_ws();
        match self.peek() {
            Some('"') => {
                self.next();
                let s = self.read_string()?;
                let regex = match op {
                    Op::RegexMatch | Op::RegexNotMatch => {
                        Some(regex::Regex::new(&s).map_err(|e| self.err(format!("invalid regex {s:?}: {e}")))?)
                    }
                    _ => None,
                };
                Ok((Value::Str(s), regex))
            }
            Some(c) if c.is_ascii_digit() => {
                let n = self.read_number()?;
                Ok((Value::Num(n), None))
            }
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let word = self.read_word()?;
                // A bare word is a string, e.g. `level == error`.
                let regex = match op {
                    Op::RegexMatch | Op::RegexNotMatch => {
                        Some(regex::Regex::new(&word).map_err(|e| self.err(format!("invalid regex {word:?}: {e}")))?)
                    }
                    _ => None,
                };
                Ok((Value::Str(word), regex))
            }
            _ => Err(self.err("expected a string, number or word")),
        }
    }

    /// Match a literal text (no word-boundary check).
    fn match_text(&mut self, text: &str) -> bool {
        let start = self.offset();
        let saved = self.chars.clone();
        for expected in text.chars() {
            if self.next() != Some(expected) {
                self.chars = saved;
                self.pos = start;
                return false;
            }
        }
        true
    }

    /// Match a whole word (`and`/`or`), ensuring a word boundary after it.
    fn match_word(&mut self, word: &str) -> bool {
        let start = self.offset();
        let saved = self.chars.clone();
        for expected in word.chars() {
            if self.next() != Some(expected) {
                self.chars = saved;
                self.pos = start;
                return false;
            }
        }
        // Word boundary: next char must not be alphanumeric or underscore.
        if self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            self.chars = saved;
            self.pos = start;
            return false;
        }
        true
    }

    /// Read a run of word characters (identifier / bare word).
    fn read_word(&mut self) -> Result<String, QueryError> {
        let mut word = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                word.push(c);
                let _ = self.next();
            } else {
                break;
            }
        }
        if word.is_empty() {
            return Err(self.err("expected a word"));
        }
        Ok(word)
    }

    /// Read a double-quoted string with `\` escapes.
    fn read_string(&mut self) -> Result<String, QueryError> {
        let mut out = String::new();
        loop {
            match self.next() {
                Some('"') => return Ok(out),
                Some('\\') => match self.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(c) => out.push(c),
                    None => return Err(self.err("unterminated escape in string")),
                },
                Some(c) => out.push(c),
                None => return Err(self.err("unterminated string")),
            }
        }
    }

    /// Read a run of digits as u64.
    fn read_number(&mut self) -> Result<u64, QueryError> {
        let mut digits = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                digits.push(c);
                let _ = self.next();
            } else {
                break;
            }
        }
        digits
            .parse::<u64>()
            .map_err(|_| self.err("number too large"))
    }
}

const fn level_name(level: Level) -> &'static str {
    match level {
        Level::Error => "error",
        Level::Warning => "warning",
        Level::Info => "info",
        Level::Progress => "progress",
        Level::Trace => "trace",
    }
}

const fn stream_name(stream: Stream) -> &'static str {
    match stream {
        Stream::Stdout => "stdout",
        Stream::Stderr => "stderr",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hushlog_core::RuleId;
    use hushlog_core::Line;

    fn classified(level: Level, text: &str, rule_id: Option<u16>) -> Classified {
        Classified {
            line: Line::new(text, text, Stream::Stdout, 1),
            level,
            rule_id: rule_id.map(RuleId),
            opid: None,
            highlight: false,
        }
    }

    fn parse_query(input: &str) -> Query {
        Query::parse(input).unwrap_or_else(|e| panic!("query {input:?} failed to parse: {e}"))
    }

    #[test]
    fn level_equality_matches() {
        let q = parse_query("level == error");
        assert!(q.matches(&classified(Level::Error, "boom", None)));
        assert!(!q.matches(&classified(Level::Info, "ok", None)));
    }

    #[test]
    fn level_inequality_matches() {
        let q = parse_query("level != info");
        assert!(!q.matches(&classified(Level::Info, "ok", None)));
        assert!(q.matches(&classified(Level::Error, "boom", None)));
    }

    #[test]
    fn stream_matches() {
        let err = Classified {
            line: Line::new("x", "x", Stream::Stderr, 1),
            level: Level::Info,
            rule_id: None,
            opid: None,
            highlight: false,
        };
        let q = parse_query(r#"stream == "stderr""#);
        assert!(q.matches(&err));
        let out = Classified {
            line: Line::new("x", "x", Stream::Stdout, 1),
            level: Level::Info,
            rule_id: None,
            opid: None,
            highlight: false,
        };
        assert!(!q.matches(&out));
    }

    #[test]
    fn line_number_comparison() {
        let q = parse_query("no > 3");
        let c3 = classified(Level::Info, "three", None);
        let c5 = Classified {
            line: Line::new("five", "five", Stream::Stdout, 5),
            level: Level::Info,
            rule_id: None,
            opid: None,
            highlight: false,
        };
        assert!(!q.matches(&c3));
        assert!(q.matches(&c5));
    }

    #[test]
    fn plain_regex_match() {
        let q = parse_query(r#"plain =~ "retry""#);
        assert!(q.matches(&classified(Level::Warning, "retrying request", None)));
        assert!(!q.matches(&classified(Level::Warning, "done", None)));
    }

    #[test]
    fn plain_regex_not_match() {
        let q = parse_query(r#"plain !~ "retry""#);
        assert!(!q.matches(&classified(Level::Warning, "retrying request", None)));
        assert!(q.matches(&classified(Level::Warning, "done", None)));
    }

    #[test]
    fn logical_and_or_and_not() {
        let q = parse_query(r#"level == error && stream == "stderr""#);
        let stderr_err = Classified {
            line: Line::new("boom", "boom", Stream::Stderr, 1),
            level: Level::Error,
            rule_id: None,
            opid: None,
            highlight: false,
        };
        assert!(q.matches(&stderr_err));
        let stdout_err = Classified {
            line: Line::new("boom", "boom", Stream::Stdout, 1),
            level: Level::Error,
            rule_id: None,
            opid: None,
            highlight: false,
        };
        assert!(!q.matches(&stdout_err));

        let q = parse_query("level == error || level == warning");
        assert!(q.matches(&classified(Level::Warning, "w", None)));

        let q = parse_query("!(level == info)");
        assert!(!q.matches(&classified(Level::Info, "ok", None)));
        assert!(q.matches(&classified(Level::Error, "boom", None)));
    }

    #[test]
    fn parentheses_group() {
        let q = parse_query("(level == error || level == warning) && plain =~ \"boom\"");
        assert!(q.matches(&classified(Level::Error, "boom", None)));
        assert!(!q.matches(&classified(Level::Error, "ok", None)));
        assert!(!q.matches(&classified(Level::Info, "boom", None)));
    }

    #[test]
    fn rule_id_numeric_comparison() {
        let q = parse_query("rule_id == 7");
        assert!(q.matches(&classified(Level::Info, "x", Some(7))));
        assert!(!q.matches(&classified(Level::Info, "x", None)));
        assert!(!q.matches(&classified(Level::Info, "x", Some(3))));
    }

    #[test]
    fn bare_word_level() {
        let q = parse_query("level == error");
        assert!(q.matches(&classified(Level::Error, "x", None)));
    }

    #[test]
    fn syntax_errors_report_offset() {
        assert!(Query::parse("level == ").is_err());
        assert!(Query::parse("frob == 1").is_err());
        assert!(Query::parse(r#"level == "unterminated"#).is_err());
        assert!(Query::parse("level == error extra").is_err());
        assert!(Query::parse("level ===").is_err());
    }
}
