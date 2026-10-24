//! Shared harness for the renderer tests. **Owner: WP5.**
//!
//! Two test binaries include this module, so not every helper is used by both;
//! that is the only reason for the `dead_code` waiver below.

#![allow(dead_code)]

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;

use hushlog_core::{Classified, Event, Level, Line, RuleId, Stream, Summary};
use hushlog_render::{Color, Format, Renderer, TtyRenderer, renderer};

/// A real log, not an invented string (AGENTS.md 5.3).
///
/// WP7 replaces the synthetic fixtures with captured ones. When it does, this
/// path is the only line in the crate that has to change.
const FIXTURE: &str = include_str!("../../../../tests/fixtures/synthetic-npm-build.log");

/// The first real error in the fixture: a repeated one for the snapshot tests.
pub const SAMPLE_ERROR: &str =
    "src/handlers/user.ts(42,9): error TS6133: 'ctx' is declared but its value is never read.";

/// A real error the stand-in classifier does not highlight, so the colour
/// assertions have one variable less in them.
pub const SAMPLE_ERROR_PLAIN: &str = "npm ERR! code ELIFECYCLE";

/// Fingerprint of the three identical `request@2.88.2` deprecation lines.
const FP_REQUEST: u64 = 0x8f3c_1d2e_5b7a_9041;
/// Fingerprint of the two identical `tough-cookie@2.5.0` lines.
const FP_TOUGH_COOKIE: u64 = 0x2a91_0c44_be6d_17f3;
/// Identifier carried by the npm deprecation box.
const BLOCK_DEPRECATION_BOX: u64 = 0x00b1_0000_0000_0007;

/// Deliberately tiny stand-in for `hushlog-rules`.
///
/// AGENTS.md 4 says the crates talk only through the traits in section 3, so
/// these snapshots must not depend on the rules engine. All they need from a
/// classifier is a `Level` per line.
pub fn classify(raw: &str, no: u64) -> Classified {
    let (level, rule_id) = if raw.contains("error TS") || raw.starts_with("npm ERR!") {
        (Level::Error, Some(RuleId(1)))
    } else if raw.starts_with("npm WARN") {
        (Level::Warning, Some(RuleId(2)))
    } else if raw.starts_with("npm notice") || raw.starts_with('>') {
        (Level::Info, Some(RuleId(3)))
    } else {
        (Level::Trace, None)
    };

    Classified {
        line: Line::new(raw, raw, Stream::Stdout, no),
        level,
        rule_id,
        opid: None,
        // The first type error is the one a human is looking for.
        highlight: raw.contains("user.ts(42,9)"),
    }
}

/// One representative event sequence containing every `Event` variant.
pub fn sample_events() -> Vec<Event> {
    let lines: Vec<&str> = FIXTURE.lines().collect();
    let at = |i: usize| classify(lines[i - 1], i as u64);

    let mut events = vec![
        // A deprecation warning, then the two repeats folded into it.
        Event::Emit(at(1)),
        Event::Folded {
            fp: FP_REQUEST,
            count: 3,
            sample: at(1),
        },
        Event::Emit(at(4)),
        Event::Folded {
            fp: FP_TOUGH_COOKIE,
            count: 2,
            sample: at(4),
        },
        // The boxed deprecation notice is a multi-line block.
        Event::BlockStart(BLOCK_DEPRECATION_BOX),
    ];
    events.extend((13..=18).map(|i| Event::Emit(at(i))));
    events.push(Event::BlockEnd(BLOCK_DEPRECATION_BOX));

    events.extend([
        Event::Emit(at(20)),
        Event::Emit(at(21)),
        // Errors are never folded: AGENTS.md 1.1. They arrive as plain Emits.
        Event::Emit(at(23)),
        Event::Emit(at(24)),
        Event::Emit(at(26)),
        Event::Emit(at(30)),
        Event::Emit(at(32)),
        Event::Summary(sample_summary()),
    ]);
    events
}

/// End-of-run accounting, including the `--tee` path that makes the full
/// verbatim log reachable (AGENTS.md 1).
pub fn sample_summary() -> Summary {
    Summary {
        lines_in: 12_345,
        lines_out: 18,
        errors: 5,
        groups: 2,
        lines_folded: 3,
        tee_path: Some("hushlog-full.log".to_string()),
    }
}

/// Every line the sequence classified as an error, taken from the events
/// themselves so the test cannot drift from what the pipeline emitted.
pub fn error_lines(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Emit(c) if c.level == Level::Error => Some(c.line.raw.as_str()),
            _ => None,
        })
        .collect()
}

/// A sink the renderer can own while the test still reads the bytes back.
#[derive(Clone, Default)]
pub struct SharedBuf(Rc<RefCell<Vec<u8>>>);

impl SharedBuf {
    pub fn text(&self) -> String {
        String::from_utf8(self.0.borrow().clone()).expect("renderers only ever write utf-8")
    }
}

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Render `events` through a renderer built by `make`.
pub fn render_with(make: impl FnOnce(SharedBuf) -> Box<dyn Renderer>, events: &[Event]) -> String {
    let buf = SharedBuf::default();
    let mut r = make(buf.clone());
    for event in events {
        r.render(event).expect("an in-memory sink never fails");
    }
    buf.text()
}

/// Render `events` with the renderer [`hushlog_render::renderer`] builds.
pub fn render(format: Format, events: &[Event]) -> String {
    render_with(|out| renderer(format, out), events)
}

/// Render `events` with the TTY renderer, colour pinned so snapshots are
/// deterministic on a developer's machine and in CI.
pub fn render_tty(color: Color, events: &[Event]) -> String {
    render_with(|out| Box::new(TtyRenderer::with_color(out, color)), events)
}
