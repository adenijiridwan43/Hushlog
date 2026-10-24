//! The promise in AGENTS.md 1: nothing is silently dropped, and error content
//! is never hidden behind a fold. **Owner: WP5.**

mod support;

use hushlog_core::Event;
use hushlog_render::{Color, Format};
use support::{
    SAMPLE_ERROR, SAMPLE_ERROR_PLAIN, classify, error_lines, render, render_tty, sample_events,
};

/// Render through one format. The TTY renderer gets its colour pinned, so a
/// developer's terminal cannot decide what these assertions see.
fn render_for(format: Format, events: &[Event]) -> String {
    match format {
        Format::Tty => render_tty(Color::Always, events),
        other => render(other, events),
    }
}

/// The sample error has to appear in every renderer, verbatim.
#[test]
fn every_error_line_reaches_every_destination() {
    let events = sample_events();
    let errors = error_lines(&events);
    assert_eq!(errors.len(), 5, "fixture should carry five error lines");

    for format in Format::ALL {
        let out = render_for(format, &events);
        for line in &errors {
            assert!(
                out.contains(line),
                "{format} dropped the error line {line:?}"
            );
        }
    }
}

/// The fixture and the hand-written expectations have not drifted apart.
#[test]
fn sample_error_is_the_fixture_it_claims_to_be() {
    assert_eq!(SAMPLE_ERROR, classify(SAMPLE_ERROR, 23).line.raw);
    assert!(error_lines(&sample_events()).contains(&SAMPLE_ERROR));
}

/// Folding is opt-in for errors (AGENTS.md 1.1), but if a caller asks for it
/// the renderers must still put the text in front of the user.
#[test]
fn folded_error_content_is_still_shown() {
    let events = vec![Event::Folded {
        fp: 0x0bad_f00d_dead_beef,
        count: 12,
        sample: classify(SAMPLE_ERROR, 23),
    }];

    for format in Format::ALL {
        let out = render_for(format, &events);
        assert!(out.contains(SAMPLE_ERROR), "{format} hid a folded error");
        if format == Format::Json || format == Format::Csv {
            // Machine format: the count is a field, not a sentence.
            assert!(out.contains("12"), "{format} lost the count");
        } else {
            assert!(
                out.contains("12 similar lines folded"),
                "{format} lost the count"
            );
        }
    }
}

/// Level colours, and the bold that marks a highlighted line.
#[test]
fn tty_colours_by_level() {
    let plain_error = render_tty(
        Color::Always,
        &[Event::Emit(classify(SAMPLE_ERROR_PLAIN, 30))],
    );
    assert!(
        plain_error.contains("\u{1b}[31m"),
        "errors are red: {plain_error:?}"
    );

    let highlighted = render_tty(Color::Always, &[Event::Emit(classify(SAMPLE_ERROR, 23))]);
    assert!(
        highlighted.contains("\u{1b}[31;1m"),
        "highlighted: {highlighted:?}"
    );

    let folded = render_tty(
        Color::Always,
        &[Event::Folded {
            fp: 1,
            count: 4,
            sample: classify(SAMPLE_ERROR_PLAIN, 30),
        }],
    );
    assert!(
        folded.contains("\u{1b}[2m"),
        "fold notes are dim: {folded:?}"
    );

    let sample = render(Format::Plain, &sample_events());
    assert!(sample.contains(SAMPLE_ERROR), "the plain renderer must too");
}

/// Colour must not leak into a pipe, a file or a bug report.
#[test]
fn no_ansi_when_colour_is_off() {
    let events = sample_events();
    for out in [
        render_tty(Color::Never, &events),
        render(Format::Plain, &events),
        render(Format::Github, &events),
        render(Format::Gitlab, &events),
        render(Format::Json, &events),
    ] {
        assert!(
            !out.contains('\u{1b}'),
            "escape sequence leaked into plain output"
        );
    }
}

/// GitHub Actions folds: two folded groups plus one block, each closed.
#[test]
fn github_emits_balanced_fold_markers() {
    let out = render(Format::Github, &sample_events());
    assert!(out.contains("::group::3 similar lines folded"), "{out}");
    assert!(out.contains("::group::2 similar lines folded"), "{out}");
    assert!(out.contains("::group::block "), "{out}");
    assert_eq!(out.matches("::group::").count(), 3, "{out}");
    assert_eq!(out.matches("::endgroup::").count(), 3, "{out}");
}

/// GitLab collapsible sections, same three.
#[test]
fn gitlab_emits_balanced_section_markers() {
    let out = render(Format::Gitlab, &sample_events());
    assert!(
        out.contains("section_start::name=3 similar lines folded[collapsed=true]"),
        "{out}"
    );
    assert_eq!(out.matches("section_start::name=").count(), 3, "{out}");
    assert_eq!(
        out.lines().filter(|l| l.trim() == "section_end").count(),
        3,
        "{out}"
    );
}

/// The summary is the safety net for AGENTS.md 1: exact counts, plus where the
/// full verbatim log went.
#[test]
fn summary_reports_counts_and_tee_path() {
    let events = sample_events();
    let summary = events.last().expect("sample ends with a summary");
    let Event::Summary(s) = summary else {
        panic!("expected a summary");
    };
    let expected = s.to_string();

    for format in Format::ALL {
        let out = render_for(format, &events);
        if format == Format::Json {
            assert!(out.contains("\"lines_in\":12345"), "{format}: {out}");
            assert!(
                out.contains("\"tee_path\":\"hushlog-full.log\""),
                "{format}: {out}"
            );
        } else if format == Format::Csv {
            assert!(out.contains("lines_in=12345"), "{format}: {out}");
            assert!(out.contains("hushlog-full.log"), "{format}: {out}");
        } else {
            assert!(out.contains(&expected), "{format} lost the summary");
        }
    }

    assert!(expected.contains("12,345 lines -> 18 shown"), "{expected}");
    assert!(expected.contains("5 errors"), "{expected}");
    assert!(
        expected.contains("full log: hushlog-full.log"),
        "{expected}"
    );
}

/// One JSON object per event, in order, so a consumer can stream it.
#[test]
fn json_is_one_object_per_event() {
    let events = sample_events();
    let out = render(Format::Json, &events);
    let records: Vec<serde_json::Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l:?} is not json: {e}")))
        .collect();

    assert_eq!(records.len(), events.len());

    let kinds: Vec<&str> = records
        .iter()
        .map(|r| r["type"].as_str().expect("every record carries a type"))
        .collect();
    assert_eq!(kinds.first(), Some(&"emit"));
    assert_eq!(kinds.last(), Some(&"summary"));
    for expected in ["emit", "folded", "block_start", "block_end", "summary"] {
        assert!(
            kinds.contains(&expected),
            "no {expected} record in {kinds:?}"
        );
    }

    let summary = records.last().expect("at least one record");
    assert_eq!(summary["lines_in"], 12_345);
    assert_eq!(summary["errors"], 5);
    assert_eq!(summary["tee_path"], "hushlog-full.log");

    let folded = records
        .iter()
        .find(|r| r["type"] == "folded")
        .expect("a folded record");
    assert_eq!(folded["count"], 3);
    assert!(folded["sample"]["raw"].is_string());
}

/// `Format::from_str` is what `--format` will be parsed with.
#[test]
fn format_names_round_trip() {
    for format in Format::ALL {
        assert_eq!(format.as_str().parse::<Format>(), Ok(format));
    }
    assert_eq!("GH".parse::<Format>(), Ok(Format::Github));
    assert_eq!("text".parse::<Format>(), Ok(Format::Plain));
    assert!("rtf".parse::<Format>().is_err());
    assert_eq!(Format::default(), Format::Plain);
}

/// A write failure must surface, never be swallowed (AGENTS.md 1).
#[test]
fn write_failures_are_reported() {
    use std::io::{self, Write};

    /// A sink that is full after `limit` bytes.
    struct Full(u64);

    impl Write for Full {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.0 == 0 {
                return Err(io::Error::other("disk full"));
            }
            self.0 -= 1;
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let events = sample_events();
    for format in Format::ALL {
        let mut r = hushlog_render::renderer(format, Full(0));
        let result = events.iter().try_for_each(|e| r.render(e));
        assert!(result.is_err(), "{format} swallowed a write failure");
    }
}

/// Levels survive the trip into JSON, whatever the destination.
#[test]
fn json_preserves_levels() {
    let events = sample_events();
    let out = render(Format::Json, &events);
    assert!(out.contains("\"level\":\"error\""), "{out}");
    assert!(out.contains("\"level\":\"warning\""), "{out}");
    assert!(out.contains("\"level\":\"info\""), "{out}");
}
