//! Stage 1 -- Ingest. **Owner: WP1.**
//!
//! Turns a byte stream into [`Line`]s: frames on newlines, collapses `\r`
//! progress-bar frames to their final state, and holds a partial line back
//! until the bytes that finish it arrive.
//!
//! Contract: input bytes in, `Vec<Line>` out. No classification, no I/O
//! decisions beyond framing. [`Ingest::flush_partial`] is the hook the CLI uses
//! to make live tailing feel live; it needs no clock, because timing lives in
//! `hushlog` -- see `AGENTS.md` section 3.1.
//!
//! # Losslessness
//!
//! The only bytes this stage ever drops are intermediate `\r` progress frames,
//! and only because the terminal that produced them overwrote them: the user
//! sees the final frame. Every one of those bytes still reaches the `--tee` log
//! and the `--raw` pass-through, which read the byte stream itself. Nothing else
//! is discarded: blank lines, ANSI escapes and lines without a trailing
//! newline all survive. See `AGENTS.md` section 1.
//!
//! # Example
//!
//! ```
//! use hushlog_core::{Ingest, Stream};
//!
//! let mut ingest = Ingest::new(Stream::Stdout);
//!
//! // Chunk boundaries fall wherever the OS puts them.
//! let lines = ingest.push(
//!     b"\x1b[31merror\x1b[0m: no such file\nbuilding 10%\rbuilding 100%\ndone",
//! );
//! assert_eq!(lines.len(), 2);
//! assert_eq!(lines[0].raw, "\x1b[31merror\x1b[0m: no such file");
//! assert_eq!(lines[0].plain, "error: no such file");
//! // Only the progress frame the user would see survives the `\r`.
//! assert_eq!(lines[1].raw, "building 100%");
//! assert_eq!(lines[1].no, 2);
//!
//! // `done` has no newline yet, so it is still buffered.
//! assert_eq!(ingest.buffered(), 4);
//!
//! // End of input flushes the trailing partial line.
//! let tail = ingest.finish();
//! assert_eq!(tail[0].raw, "done");
//! assert_eq!(tail[0].no, 3);
//! ```

use crate::{Line, Stream};

/// Byte-to-line framing. See module docs.
#[derive(Debug)]
pub struct Ingest {
    /// Bytes of the current, not-yet-terminated line.
    buf: Vec<u8>,
    /// Line number to stamp on the next emitted line.
    next_no: u64,
    /// Stream stamped onto every line this framer emits.
    stream: Stream,
}

impl Default for Ingest {
    /// Pipe mode: a single stream read from stdin.
    fn default() -> Self {
        Self::new(Stream::Stdout)
    }
}

impl Ingest {
    /// Create a framer that tags every emitted line with `stream`.
    #[must_use]
    pub fn new(stream: Stream) -> Self {
        Self {
            buf: Vec::new(),
            next_no: 1,
            stream,
        }
    }

    /// The stream every line from this framer is tagged with.
    #[must_use]
    pub fn stream(&self) -> Stream {
        self.stream
    }

    /// Bytes of the partial line currently held back.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// How many lines have been emitted so far.
    #[must_use]
    pub fn lines_emitted(&self) -> u64 {
        self.next_no - 1
    }

    /// Feed the next chunk of bytes, returning every line that is now complete.
    ///
    /// Chunk boundaries are irrelevant: bytes may arrive one at a time, split
    /// down the middle of a line, or split down the middle of a `\r\n`. The
    /// result is identical either way, which
    /// `chunk_boundaries_never_change_the_output` pins down.
    #[must_use]
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Line> {
        if bytes.is_empty() {
            return Vec::new();
        }
        self.buf.extend_from_slice(bytes);

        let stream = self.stream;
        let mut next_no = self.next_no;
        let mut out = Vec::new();
        // First byte of the frame we would actually show on screen. Everything
        // before it in this line was overwritten by a later `\r` frame.
        let mut frame = 0;
        // First byte not yet turned into a line; everything before is dead.
        let mut cut = 0;
        let mut i = 0;

        while i < self.buf.len() {
            match self.buf[i] {
                b'\n' => {
                    out.push(build(&self.buf[frame..i], stream, &mut next_no));
                    i += 1;
                    cut = i;
                    frame = i;
                }
                // A CR in the very last position could still turn out to be the
                // first half of a CRLF, so hold it back for the next chunk.
                b'\r' if i + 1 == self.buf.len() => break,
                b'\r' => {
                    if self.buf[i + 1] == b'\n' {
                        out.push(build(&self.buf[frame..i], stream, &mut next_no));
                        i += 2;
                        cut = i;
                        frame = i;
                    } else {
                        // Progress frame: the bytes before it are overwritten.
                        i += 1;
                        frame = i;
                    }
                }
                _ => i += 1,
            }
        }

        self.buf.drain(..cut);
        self.next_no = next_no;
        out
    }

    /// Emit the buffered partial line now, without waiting for a newline.
    ///
    /// This is what makes a live tail feel live; the caller decides when the
    /// wait has gone on long enough. Only the final `\r` frame is emitted --
    /// the earlier ones were overwritten on screen and are never resurrected --
    /// and a multi-byte character that is still in flight is kept back rather
    /// than cut in half.
    ///
    /// Calling this and then [`Ingest::finish`] is always safe; `finish` simply
    /// flushes whatever is left.
    #[must_use]
    pub fn flush_partial(&mut self) -> Vec<Line> {
        let keep = char_boundary(&self.buf);
        if keep == 0 {
            // Either nothing buffered, or only the start of a character.
            return Vec::new();
        }
        let mut next_no = self.next_no;
        let line = build(final_frame(&self.buf[..keep]), self.stream, &mut next_no);
        // At most three bytes: the tail of a character split by the flush.
        let leftover = self.buf[keep..].to_vec();
        self.buf.clear();
        self.buf = leftover;
        self.next_no = next_no;
        vec![line]
    }

    /// End of input: flush whatever is still buffered and stop.
    ///
    /// Idempotent -- a second call returns nothing. Any bytes still buffered
    /// here are a final line that never got its newline, and they are emitted
    /// in full.
    #[must_use]
    pub fn finish(&mut self) -> Vec<Line> {
        let mut out = self.flush_partial();
        if !self.buf.is_empty() {
            out.push(build(
                final_frame(&self.buf),
                self.stream,
                &mut self.next_no,
            ));
            self.buf.clear();
        }
        out
    }
}

/// Turn one logical line's bytes into a [`Line`], advancing the counter.
///
/// Bytes that are not valid UTF-8 become `U+FFFD`; the `--tee` log still holds
/// the original bytes, so this is a display artefact, not data loss.
fn build(bytes: &[u8], stream: Stream, no: &mut u64) -> Line {
    let raw = String::from_utf8_lossy(bytes);
    let line_no = *no;
    *no += 1;
    Line::new(raw.clone(), strip_ansi(&raw), stream, line_no)
}

/// The last `\r`-separated frame of a logical line.
///
/// A progress bar overwrites one terminal row, so the frame the user would
/// actually see is the only one worth keeping. A trailing empty frame -- the
/// stream ended on a bare `\r` -- falls back to the frame before it rather
/// than reporting a blank line.
fn final_frame(buf: &[u8]) -> &[u8] {
    let mut end = buf.len();
    loop {
        let start = buf[..end]
            .iter()
            .rposition(|&b| b == b'\r')
            .map_or(0, |p| p + 1);
        if start < end || start == 0 {
            return &buf[start..end];
        }
        // `start == end == len > 0`: an empty frame, step back over its `\r`.
        end = start - 1;
    }
}

/// Length of the longest prefix of `buf` that is complete UTF-8.
///
/// Used only by [`Ingest::flush_partial`], so that flushing a live tail never
/// cuts a multi-byte character in half and never stalls on bad bytes.
fn char_boundary(buf: &[u8]) -> usize {
    match std::str::from_utf8(buf) {
        // Truncated at the very end: stop before the incomplete character.
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        // Valid, or invalid bytes that will never become valid: emit them.
        _ => buf.len(),
    }
}

/// `ESC`, the introducer of every escape sequence.
const ESC: u8 = 0x1b;
/// `BEL`, the common terminator of an OSC string.
const BEL: u8 = 0x07;

/// Where [`strip_ansi`] is inside an escape sequence.
#[derive(Clone, Copy)]
enum Scan {
    /// Copying ordinary text.
    Text,
    /// Read `ESC`; the next byte says which kind of sequence this is.
    Escape,
    /// Inside a CSI sequence (`ESC [`): parameters, intermediates, one final.
    Csi,
    /// After `ESC` plus intermediate bytes; one final byte still to come.
    EscapeInter,
    /// Inside a string sequence (`ESC ]`, `ESC P`, `ESC X`, `ESC ^`, `ESC _`).
    StringSeq,
    /// Inside a string sequence and just read `ESC`: `\` is `ST`.
    StringSeqEscape,
}

/// `src` with its ANSI escape sequences removed, for matching only.
///
/// Handles CSI sequences (colours, cursor movement, erase-in-line) and the
/// string sequences OSC, DCS, SOS, PM and APC. Anything else after an `ESC` is
/// treated as the single-byte escape it is. C1 controls in their 8-bit form
/// (`0x9b` and friends) are not recognised: they are not valid UTF-8 on their
/// own and never survive [`build`]'s conversion.
///
/// Malformed sequences degrade safely: a byte that cannot belong to the
/// sequence is treated as text again. Because framing happens before this
/// function sees anything, a runaway sequence can never swallow a line break.
fn strip_ansi(src: &str) -> String {
    if !src.as_bytes().contains(&ESC) {
        return src.to_owned();
    }
    let mut out = String::with_capacity(src.len());
    let mut state = Scan::Text;
    // Start of the region not yet copied to `out`; always a char boundary.
    let mut flushed = 0;

    for (i, &byte) in src.as_bytes().iter().enumerate() {
        match (state, byte) {
            // `ESC` opens a sequence: copy the text before it, drop the `ESC`.
            (Scan::Text, ESC) => {
                out.push_str(&src[flushed..i]);
                state = Scan::Escape;
                flushed = i + 1;
            }
            (Scan::Escape, b'[') => state = Scan::Csi,
            (Scan::Escape, b']' | b'P' | b'X' | b'^' | b'_') => state = Scan::StringSeq,
            (Scan::Escape, 0x20..=0x2f) => state = Scan::EscapeInter,
            (Scan::StringSeq, ESC) => state = Scan::StringSeqEscape,
            // A final byte ends the sequence and belongs to it: `m` closing a
            // colour, `K` closing an erase, `BEL` or `ST` closing a string, or
            // the one byte of a two-byte escape such as `ESC c`.
            (Scan::Escape | Scan::EscapeInter, 0x30..=0x7e)
            | (Scan::Csi, 0x40..=0x7e)
            | (Scan::StringSeq, BEL)
            | (Scan::StringSeqEscape, b'\\') => {
                state = Scan::Text;
                flushed = i + 1;
            }
            // An `ESC` in the middle of a sequence opens the next one, exactly
            // as a terminal would read it. Never leave an escape in `plain`.
            (Scan::Escape | Scan::EscapeInter | Scan::Csi, ESC) => {
                state = Scan::Escape;
                flushed = i + 1;
            }
            // Parameters and intermediates carry no text, and neither does the
            // body of a string sequence.
            (Scan::Text | Scan::StringSeq, _) | (Scan::Csi, 0x20..=0x3f) => {}
            // Anything else cannot belong to the sequence, so it is text
            // again. Every byte skipped so far was ASCII, which keeps `i` on a
            // character boundary.
            (Scan::Escape | Scan::EscapeInter | Scan::Csi, _) => {
                state = Scan::Text;
                flushed = i;
            }
            // An `ESC` that did not close the string was part of it after all.
            (Scan::StringSeqEscape, _) => state = Scan::StringSeq,
        }
    }
    out.push_str(&src[flushed..]);
    out
}

#[cfg(test)]
mod tests {
    use super::{Ingest, Stream};
    use crate::Line;
    use proptest::prelude::*;

    /// Frame `chunks` as one continuous stream, exactly as the CLI would.
    fn frame_all(chunks: &[&[u8]]) -> Vec<Line> {
        let mut ingest = Ingest::new(Stream::Stdout);
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(ingest.push(chunk));
        }
        out.extend(ingest.finish());
        out
    }

    fn raw_of(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|l| l.raw.as_str()).collect()
    }

    fn one(chunk: &[u8]) -> Line {
        let mut lines = frame_all(&[chunk]);
        assert_eq!(lines.len(), 1, "expected exactly one line from {chunk:?}");
        lines.swap_remove(0)
    }

    #[test]
    fn splits_a_chunk_on_newlines() {
        let lines = frame_all(&[b"one\ntwo\nthree\n"]);
        assert_eq!(raw_of(&lines), ["one", "two", "three"]);
        assert_eq!(lines[0].no, 1);
        assert_eq!(lines[1].no, 2);
        assert_eq!(lines[2].no, 3);
    }

    #[test]
    fn a_line_without_a_trailing_newline_waits_for_eof() {
        let mut ingest = Ingest::new(Stream::Stdout);
        // The first line is complete and goes out now; the second is not.
        let done = ingest.push(b"one\ntwo");
        assert_eq!(raw_of(&done), ["one"]);
        assert_eq!(ingest.buffered(), 3);
        let lines = ingest.finish();
        assert_eq!(raw_of(&lines), ["two"]);
        assert_eq!(lines[0].no, 2);
    }

    #[test]
    fn buffers_a_partial_line_until_its_newline_arrives() {
        let mut ingest = Ingest::new(Stream::Stdout);
        assert_eq!(ingest.push(b"abc"), Vec::new());
        let lines = ingest.push(b"def\n");
        assert_eq!(raw_of(&lines), ["abcdef"]);
    }

    #[test]
    fn empty_lines_are_lines_and_are_kept() {
        let lines = frame_all(&[b"\n\n\n"]);
        assert_eq!(raw_of(&lines), ["", "", ""]);
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn cr_progress_frames_collapse_to_the_final_frame() {
        let line = one(b"10%\r50%\r100%\n");
        assert_eq!(line.raw, "100%");
        assert_eq!(line.plain, "100%");
    }

    #[test]
    fn a_cr_lone_on_its_own_line_is_a_frame_separator() {
        assert_eq!(raw_of(&frame_all(&[b"\n"])), [""]);
        assert_eq!(raw_of(&frame_all(&[b"a\rb\n"])), ["b"]);
    }

    #[test]
    fn crlf_terminates_a_line_instead_of_starting_a_frame() {
        let lines = frame_all(&[b"one\r\ntwo\r\n"]);
        assert_eq!(raw_of(&lines), ["one", "two"]);
    }

    #[test]
    fn a_cr_at_end_of_input_keeps_the_frame_that_was_visible() {
        // A terminal shows `50%` here: the CR returned to column zero and
        // nothing overwrote it. Emitting a blank line would be a lie.
        assert_eq!(raw_of(&frame_all(&[b"10%\r50%\r"])), ["50%"]);
        // A buffer that is nothing but CRs is a blank line, not nothing.
        assert_eq!(raw_of(&frame_all(&[b"\r"])), [""]);
    }

    #[test]
    fn raw_keeps_ansi_and_plain_strips_it() {
        let line = one(b"\x1b[31mboom\x1b[0m done\n");
        assert_eq!(line.raw, "\x1b[31mboom\x1b[0m done");
        assert_eq!(line.plain, "boom done");
    }

    #[test]
    fn plain_strips_cursor_and_erase_sequences() {
        let line = one(b"\x1b[2Kprogress\x1b[1A\x1b[10D\n");
        assert_eq!(line.raw, "\x1b[2Kprogress\x1b[1A\x1b[10D");
        assert_eq!(line.plain, "progress");
    }

    #[test]
    fn an_escape_inside_an_escape_restarts_the_sequence() {
        // `\x1b[\x1b[31m` is a truncated CSI followed by a colour. The second
        // ESC starts a fresh sequence, so no escape may survive into `plain`.
        let line = one(b"\x1b[\x1b[31mred\n");
        assert_eq!(line.raw, "\x1b[\x1b[31mred");
        assert_eq!(line.plain, "red");
    }

    #[test]
    fn plain_strips_osc_strings() {
        let line = one(b"before\x1b]0;window title\x07after\n");
        assert_eq!(line.plain, "beforeafter");
        // The ST form, not BEL.
        let line = one(b"before\x1b]8;;https://example.com\x1b\\link\n");
        assert_eq!(line.plain, "beforelink");
    }

    #[test]
    fn a_malformed_escape_keeps_its_text() {
        // `é` cannot continue a CSI, so the sequence ends and the text resumes.
        let line = one("\x1b[é!still here\n".as_bytes());
        assert_eq!(line.plain, "é!still here");
        // A bare ESC followed by a non-ASCII byte is not a sequence.
        let line = one("\x1bé\n".as_bytes());
        assert_eq!(line.plain, "é");
        // A lone ESC at end of line is dropped, not a panic.
        let line = one(b"tail\x1b");
        assert_eq!(line.plain, "tail");
    }

    #[test]
    fn a_runaway_escape_cannot_swallow_the_next_line() {
        // Framing happens before ANSI stripping, so the unterminated CSI dies
        // at the newline and the next line survives intact.
        let lines = frame_all(&[b"\x1b[31mfirst\nsecond\n"]);
        assert_eq!(raw_of(&lines), ["\x1b[31mfirst", "second"]);
        assert_eq!(lines[1].plain, "second");
    }

    #[test]
    fn lines_are_stamped_with_the_constructor_stream() {
        let mut err = Ingest::new(Stream::Stderr);
        let lines = err.push(b"on stderr\n");
        assert_eq!(lines[0].stream, Stream::Stderr);
        let mut out = Ingest::new(Stream::Stdout);
        let lines = out.push(b"on stdout\n");
        assert_eq!(lines[0].stream, Stream::Stdout);
        assert_eq!(Ingest::new(Stream::Stderr).stream(), Stream::Stderr);
        assert_eq!(Ingest::default().stream(), Stream::Stdout);
    }

    #[test]
    fn line_numbers_keep_climbing_across_a_partial_flush() {
        let mut ingest = Ingest::new(Stream::Stdout);
        assert!(ingest.push(b"one\ntwo").first().is_some_and(|l| l.no == 1));
        assert_eq!(ingest.lines_emitted(), 1);
        let flushed = ingest.flush_partial();
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].no, 2);
        assert_eq!(flushed[0].raw, "two");
        assert_eq!(ingest.lines_emitted(), 2);
        assert_eq!(ingest.buffered(), 0);
    }

    #[test]
    fn flush_partial_emits_only_the_frame_on_screen() {
        let mut ingest = Ingest::new(Stream::Stdout);
        assert_eq!(ingest.push(b"10%\r50%\r100%"), Vec::new());
        let flushed = ingest.flush_partial();
        assert_eq!(raw_of(&flushed), ["100%"]);
        assert_eq!(ingest.buffered(), 0);
        // Nothing is pending, so a second flush has nothing to say.
        assert_eq!(ingest.flush_partial(), Vec::new());
    }

    #[test]
    fn flush_partial_never_splits_a_multibyte_character() {
        let full = "中".as_bytes();
        let mut ingest = Ingest::new(Stream::Stdout);
        // Feed the first byte of a three-byte character and ask for a flush.
        assert_eq!(ingest.push(&full[..1]), Vec::new());
        assert!(
            ingest.flush_partial().is_empty(),
            "must not cut a character"
        );
        assert_eq!(ingest.buffered(), 1);
        // The rest completes it, but there is still no newline, so it is
        // emitted at the next flush -- whole.
        assert_eq!(ingest.push(&full[1..]), Vec::new());
        assert_eq!(ingest.buffered(), full.len());
        assert_eq!(raw_of(&ingest.finish()), ["中"]);
    }

    #[test]
    fn finish_never_drops_a_character_left_half_way() {
        // At EOF there is nothing left to wait for, so the truncated character
        // is emitted as a replacement char rather than silently dropped.
        let mut ingest = Ingest::new(Stream::Stdout);
        assert_eq!(ingest.push(&[0xe4, 0xb8]), Vec::new());
        let lines = ingest.finish();
        assert_eq!(raw_of(&lines), ["\u{fffd}"]);
    }

    #[test]
    fn finish_is_idempotent() {
        let mut ingest = Ingest::new(Stream::Stdout);
        assert_eq!(ingest.push(b"trailing"), Vec::new());
        assert_eq!(ingest.finish().len(), 1);
        assert_eq!(ingest.finish(), Vec::new());
        assert_eq!(ingest.buffered(), 0);
    }

    #[test]
    fn empty_pushes_are_free_and_change_nothing() {
        let mut ingest = Ingest::new(Stream::Stdout);
        assert_eq!(ingest.push(b"x"), Vec::new());
        assert_eq!(ingest.push(b""), Vec::new());
        assert_eq!(ingest.buffered(), 1);
        assert_eq!(raw_of(&ingest.finish()), ["x"]);
    }

    #[test]
    fn multi_byte_text_survives_framing_unchanged() {
        let lines = frame_all(&["αβγ\nδεζ".as_bytes()]);
        assert_eq!(raw_of(&lines), ["αβγ", "δεζ"]);
    }

    /// Bytes that look like real terminal output: lines, progress frames,
    /// colour codes and text, in every combination.
    fn terminal_bytes() -> impl Strategy<Value = Vec<u8>> {
        prop::collection::vec(
            prop::sample::select(vec![
                "build ok",
                "warning: unused\n",
                "error: boom\n",
                "\n",
                "\r",
                "\r\n",
                "\x1b[31m",
                "\x1b[0m",
                "\x1b[2K",
                "\x1b]0;title\x07",
                "\x1b[",
                "\x1b",
                "50%",
                " ",
                "中",
            ]),
            0..48,
        )
        .prop_map(|parts| parts.concat().into_bytes())
    }

    proptest! {
        /// The correctness property: framing is a function of the byte stream
        /// and of nothing else.
        #[test]
        fn chunk_boundaries_never_change_the_output(bytes in terminal_bytes()) {
            let one_shot = frame_all(&[&bytes]);
            let byte_at_a_time = frame_all(&bytes.chunks(1).collect::<Vec<_>>());
            prop_assert_eq!(byte_at_a_time, one_shot);
        }

        /// Same bytes, arbitrary cut points.
        #[test]
        fn random_chunk_splits_agree_with_one_shot_framing(
            bytes in terminal_bytes(),
            cuts in prop::collection::vec(0usize..300, 0..12),
        ) {
            let expected = frame_all(&[&bytes]);
            let mut chunks: Vec<&[u8]> = Vec::new();
            let mut prev = 0;
            for cut in cuts {
                let end = cut.clamp(prev, bytes.len());
                chunks.push(&bytes[prev..end]);
                prev = end;
            }
            chunks.push(&bytes[prev..]);
            prop_assert_eq!(frame_all(&chunks), expected);
        }

        /// A line's `raw` never invents a break, and `plain` never keeps an
        /// escape or grows.
        #[test]
        fn raw_is_verbatim_and_plain_is_only_shorter(bytes in terminal_bytes()) {
            for line in frame_all(&[&bytes]) {
                prop_assert!(!line.raw.contains('\n'), "raw leaked a newline");
                prop_assert!(!line.raw.contains('\r'), "raw leaked a carriage return");
                prop_assert!(!line.plain.contains('\u{1b}'), "plain leaked an escape");
                prop_assert!(line.plain.len() <= line.raw.len());
                prop_assert!(line.no >= 1);
                prop_assert_eq!(line.stream, Stream::Stdout);
            }
        }

        /// Whatever the user would see on screen is what comes out.
        #[test]
        fn only_the_final_progress_frame_survives(
            frames in prop::collection::vec("[a-z ]{1,8}", 1..8),
        ) {
            let input = format!("{}\n", frames.join("\r"));
            let lines = frame_all(&[input.as_bytes()]);
            prop_assert_eq!(lines.len(), 1);
            prop_assert_eq!(
                &lines[0].raw,
                frames.last().expect("generator yields at least one frame")
            );
        }

        /// Numbers are dense and start at one: no line is silently skipped.
        #[test]
        fn line_numbers_are_dense_and_one_based(bytes in terminal_bytes()) {
            let lines = frame_all(&[&bytes]);
            for (i, line) in lines.iter().enumerate() {
                prop_assert_eq!(line.no, i as u64 + 1);
            }
        }
    }
}
