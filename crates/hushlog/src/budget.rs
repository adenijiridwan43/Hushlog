//! The opt-in `--max-tokens` gate. **Owner: WP6.**
//!
//! A budget is a display decision, not a filtering decision: it sits between
//! stage 4 (collapse) and stage 5 (render), and only ever hides lines that are
//! not errors. Every hidden line is counted, and the byte-exact copy is still
//! in `--tee`. See `AGENTS.md` section 1.
//!
//! The policy is deliberately simple and streaming. Tokens are estimated at
//! four bytes each (`ceil((bytes + 1) / 4)`, counting the newline). As the
//! estimate approaches the cap, the gate starts hiding progressively more
//! useful levels:
//!
//! | Remaining budget | Lines hidden |
//! |---|---|
//! | more than 75% | trace / debug |
//! | more than 10% | trace / debug, info, progress |
//! | 10% or less | every non-error line |
//!
//! Errors are never hidden, at any pressure. They can push a run past its cap;
//! the cap is a target for noise, not a permission to lose a failure.

use hushlog_core::{Classified, Event, Level, Summary};

#[derive(Debug, Clone, Default)]
pub(crate) struct Budget {
    cap: Option<u64>,
    token_estimate: u64,
    tokens_suppressed: u64,
    lines_suppressed: u64,
}

impl Budget {
    pub(crate) fn new(cap: Option<u64>) -> Self {
        Self {
            cap,
            ..Self::default()
        }
    }

    /// Four bytes per token, with the line terminator included.
    pub(crate) fn estimate_tokens(text: &str) -> u64 {
        let bytes = u64::try_from(text.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        bytes.div_ceil(4)
    }

    /// Decide whether an event reaches the renderer and account for the choice.
    ///
    /// Fold notices and block markers are never suppressed: they are the
    /// accounting for lines that were already collapsed, so hiding them would
    /// turn a counted fold into a silent one. Their text still contributes to
    /// the token estimate, because it is written to the destination.
    pub(crate) fn admit(&mut self, event: &Event) -> bool {
        let text = match event {
            Event::Emit(c) => &c.line.raw,
            Event::Folded { sample, .. } => &sample.line.raw,
            Event::BlockStart(_) | Event::BlockEnd(_) | Event::Summary(_) => return true,
        };
        let tokens = Self::estimate_tokens(text);
        self.token_estimate = self.token_estimate.saturating_add(tokens);

        let Event::Emit(c) = event else {
            return true;
        };
        if !self.should_suppress(c) {
            return true;
        }
        self.tokens_suppressed = self.tokens_suppressed.saturating_add(tokens);
        self.lines_suppressed = self.lines_suppressed.saturating_add(1);
        false
    }

    /// Write the exact budget counts into the end-of-run summary.
    ///
    /// The collapser counts an emitted line before the budget sees it, so the
    /// suppressed lines have to be subtracted here. The resulting accounting
    /// is `lines_in == lines_out + lines_folded + lines_suppressed`.
    pub(crate) fn apply_to_summary(&self, summary: &mut Summary) {
        summary.lines_out = summary.lines_out.saturating_sub(self.lines_suppressed);
        summary.lines_suppressed = self.lines_suppressed;
        summary.token_estimate = self.token_estimate;
        summary.tokens_suppressed = self.tokens_suppressed;
        summary.max_tokens = self.cap;
    }

    fn should_suppress(&self, c: &Classified) -> bool {
        let Some(max) = self.cap else {
            return false;
        };
        // AGENTS.md 1.1: this check has no opt-in and no exception budget.
        if c.level == Level::Error {
            return false;
        }

        let remaining = max.saturating_sub(self.token_estimate);
        if remaining.saturating_mul(4) > max.saturating_mul(3) {
            c.level == Level::Trace
        } else if remaining.saturating_mul(10) > max {
            matches!(c.level, Level::Trace | Level::Info | Level::Progress)
        } else {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::io;
    use std::rc::Rc;

    use hushlog_core::{Collapser, FoldingCollapser, Line, Stream};
    use hushlog_render::Format;
    use proptest::prelude::*;

    use super::*;

    fn classified(no: u64, level: Level, text: &str) -> Classified {
        Classified {
            line: Line::new(text.to_string(), text.to_string(), Stream::Stdout, no),
            level,
            rule_id: None,
            opid: None,
            highlight: level == Level::Error,
        }
    }

    /// A writer that can be inspected after the renderer is dropped.
    #[derive(Clone, Default)]
    struct SharedVec(Rc<RefCell<Vec<u8>>>);

    impl io::Write for SharedVec {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn run_budgeted(input: &[Classified], cap: u64) -> (String, Summary) {
        let mut collapser = FoldingCollapser::default();
        let mut budget = Budget::new(Some(cap));
        let mut events = Vec::new();
        for c in input {
            for event in collapser.push(c.clone()) {
                if budget.admit(&event) {
                    events.push(event);
                }
            }
        }
        events.extend(collapser.finish());
        let summary = events
            .iter_mut()
            .find_map(|e| match e {
                Event::Summary(s) => {
                    budget.apply_to_summary(s);
                    Some(s.clone())
                }
                _ => None,
            })
            .expect("the collapser ends with a summary");

        let sink = SharedVec::default();
        let mut renderer = hushlog_render::renderer(Format::Plain, sink.clone());
        for event in events {
            renderer.render(&event).expect("a Vec cannot fail");
        }
        (
            String::from_utf8(sink.0.borrow().clone()).expect("plain text"),
            summary,
        )
    }

    #[test]
    fn four_bytes_are_one_token_and_a_line_has_a_terminator() {
        assert_eq!(Budget::estimate_tokens(""), 1);
        assert_eq!(Budget::estimate_tokens("abc"), 1);
        assert_eq!(Budget::estimate_tokens("abcd"), 2);
        assert_eq!(Budget::estimate_tokens("abcde"), 2);
    }

    #[test]
    fn an_error_survives_a_zero_budget() {
        let input = [
            classified(1, Level::Info, "a noisy thing"),
            classified(2, Level::Error, "error: the one that matters"),
        ];
        let (out, summary) = run_budgeted(&input, 0);
        assert!(out.contains("error: the one that matters"), "{out}");
        assert!(!out.contains("a noisy thing"), "{out}");
        assert_eq!(summary.lines_suppressed, 1);
        assert_eq!(summary.lines_out, 1);
    }

    fn line_strategy() -> impl Strategy<Value = Classified> {
        (
            1u64..500,
            prop::sample::select(vec![
                Level::Error,
                Level::Warning,
                Level::Info,
                Level::Progress,
                Level::Trace,
            ]),
            "[a-z]{1,24}",
        )
            .prop_map(|(no, level, text)| {
                let text = if level == Level::Error {
                    format!("error-{no}-{text}")
                } else {
                    format!("line-{no}-{text}")
                };
                classified(no, level, &text)
            })
    }

    proptest! {
        /// A second invariant, separate from the protected `error_never_lost`
        /// property test: adding a token budget must not weaken the first one.
        #[test]
        fn every_error_reaches_the_rendered_output_with_a_budget(
            input in prop::collection::vec(line_strategy(), 0..120),
            cap in 0u64..400,
        ) {
            let expected: BTreeSet<String> = input
                .iter()
                .filter(|c| c.level == Level::Error)
                .map(|c| c.line.raw.clone())
                .collect();
            let (out, _) = run_budgeted(&input, cap);
            for text in &expected {
                prop_assert!(
                    out.contains(text),
                    "budget hid {:?}:\n{}",
                    text,
                    out
                );
            }
        }

        #[test]
        fn the_budget_accounts_for_every_line(
            input in prop::collection::vec(line_strategy(), 0..120),
            cap in 0u64..400,
        ) {
            let (_, summary) = run_budgeted(&input, cap);
            prop_assert_eq!(
                summary.lines_in,
                summary.lines_out + summary.lines_folded + summary.lines_suppressed,
                "{:?}",
                summary
            );
            prop_assert!(summary.token_estimate >= summary.tokens_suppressed);
        }
    }
}
