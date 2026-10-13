//! One snapshot per destination, driven by a single event sequence that
//! contains every `Event` variant. **Owner: WP5.**
//!
//! Run `INSTA_UPDATE=always cargo test -p hushlog-render` to refresh the
//! `.snap` files under `tests/snapshots/`.

mod support;

use hushlog_render::Color;
use support::{render, render_tty, sample_events};

/// Colour on a real terminal. `Color::Auto` would resolve differently on a
/// developer's machine than in CI, so the snapshot pins it.
#[test]
fn tty() {
    let events = sample_events();
    insta::assert_snapshot!("tty", render_tty(Color::Always, &events));
}

#[test]
fn plain() {
    let events = sample_events();
    insta::assert_snapshot!("plain", render(hushlog_render::Format::Plain, &events));
}

#[test]
fn github() {
    let events = sample_events();
    insta::assert_snapshot!("github", render(hushlog_render::Format::Github, &events));
}

#[test]
fn gitlab() {
    let events = sample_events();
    insta::assert_snapshot!("gitlab", render(hushlog_render::Format::Gitlab, &events));
}

#[test]
fn json() {
    let events = sample_events();
    insta::assert_snapshot!("json", render(hushlog_render::Format::Json, &events));
}
