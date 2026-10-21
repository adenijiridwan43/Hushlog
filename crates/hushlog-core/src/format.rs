//! Stage 2.5 -- format auto-detection, ported from lnav.
//!
//! lnav tries each log format's regular expressions against the first few
//! lines of a file, picks the first match, and locks on to that format for
//! the rest of the file (`docs.lnav.org/en/latest/formats.html`). Hushlog does
//! the same for rule packs: each pack may declare `detect` patterns, and the
//! first pack whose pattern matches a line wins. The run then locks on to
//! that pack's rules, so a mixed log (cargo + npm + tsc output) is classified
//! by the format that actually matches, not by a flat rule list.
//!
//! Unlike lnav, hushlog is a pipe filter: it reads from stdin and must keep
//! producing output. Detection therefore runs on a bounded window of the first
//! lines; if no pack matches within the window, the run falls back to all
//! packs (the current behaviour).

/// Maximum lines buffered before giving up on detection and falling back.
///
/// lnav uses 15,000 lines. A pipe filter can't afford that much delay, so a
/// much smaller window is used: enough to identify a build log, not enough to
/// stall output.
const MAX_DETECT_LINES: u64 = 200;

/// One candidate pack for format detection.
#[derive(Debug, Clone)]
pub struct FormatCandidate {
    /// The pack's name, as written in its `name` field (or file stem).
    pub name: String,
    /// Compiled detection patterns, tried against each line.
    pub patterns: Vec<regex::Regex>,
}

/// Detects which rule pack's format matches the input stream.
///
/// `push` is called once per line. It returns `Some(name)` the first time a
/// candidate's pattern matches a line, after which the run locks on to that
/// pack. If no candidate matches within [`MAX_DETECT_LINES`], `push` keeps
/// returning `None` and the caller falls back to all packs.
#[derive(Debug, Clone)]
pub struct FormatDetector {
    candidates: Vec<FormatCandidate>,
    detected: Option<String>,
    lines_seen: u64,
}

impl FormatDetector {
    /// Build a detector from pack names and their raw detect pattern strings.
    ///
    /// # Errors
    ///
    /// Returns the first pattern that fails to compile, naming the pack it
    /// came from.
    pub fn new(
        candidates: Vec<(String, Vec<String>)>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut packed = Vec::with_capacity(candidates.len());
        for (name, patterns) in candidates {
            let mut compiled = Vec::with_capacity(patterns.len());
            for pattern in patterns {
                compiled.push(regex::Regex::new(&pattern).map_err(|e| {
                    format!("pack {name}: invalid detect pattern: {e}")
                })?);
            }
            packed.push(FormatCandidate {
                name,
                patterns: compiled,
            });
        }
        Ok(Self {
            candidates: packed,
            detected: None,
            lines_seen: 0,
        })
    }

    /// Feed one line. Returns `Some(pack_name)` when a candidate's pattern
    /// matches this line, after which the detector is locked on.
    pub fn push(&mut self, line: &str) -> Option<&str> {
        if self.detected.is_some() {
            return self.detected.as_deref();
        }

        self.lines_seen += 1;
        if self.lines_seen > MAX_DETECT_LINES {
            return None;
        }

        for candidate in &self.candidates {
            if candidate.patterns.iter().any(|re| re.is_match(line)) {
                self.detected = Some(candidate.name.clone());
                return self.detected.as_deref();
            }
        }
        None
    }

    /// The detected pack name, if any.
    #[must_use]
    pub fn detected(&self) -> Option<&str> {
        self.detected.as_deref()
    }

    /// Whether detection is still open (no match yet, within the window).
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.detected.is_none() && self.lines_seen <= MAX_DETECT_LINES
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_matching_pack_wins() {
        let mut det = FormatDetector::new(vec![
            ("cargo".to_string(), vec!["^Compiling ".to_string()]),
            ("npm".to_string(), vec!["^npm ".to_string()]),
        ])
        .expect("patterns compile");

        assert_eq!(det.push("warning: unused"), None);
        assert_eq!(det.push("Checking hushlog v0.1"), None);
        assert_eq!(det.push("Compiling hushlog v0.1"), Some("cargo"));
        // Locked on: subsequent lines still report cargo.
        assert_eq!(det.push("npm WARN deprecated"), Some("cargo"));
    }

    #[test]
    fn no_match_returns_none() {
        let mut det = FormatDetector::new(vec![("cargo".to_string(), vec!["^Compiling ".to_string()])])
        .expect("patterns compile");

        for _ in 0..MAX_DETECT_LINES {
            assert_eq!(det.push("random noise line"), None);
        }
        // Window exhausted: still None, caller falls back to all packs.
        assert_eq!(det.push("more noise"), None);
    }

    #[test]
    fn invalid_pattern_is_an_error() {
        let err = FormatDetector::new(vec![("bad".to_string(), vec!["*".to_string()])])
            .expect_err("bad regex must fail");
        assert!(err.to_string().contains("bad"), "got: {err}");
    }
}
