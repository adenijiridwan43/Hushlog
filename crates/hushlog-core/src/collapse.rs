//! Stage 4a -- Collapse repeated lines. **Owner: WP4.**
//!
//! A bounded LRU keyed on fingerprint. First occurrence is shown verbatim;
//! later ones become a counted summary. Errors pass through untouched unless
//! `CollapsePolicy::collapse_errors` is explicitly set -- see AGENTS.md 1.1.
//!
//! This module is pre-declared so that no parallel agent needs to edit
//! `lib.rs`. Implement `Collapser` here.
//!
//! # What is guaranteed
//!
//! * The first sighting of a fingerprint is emitted verbatim and kept as that
//!   group's `sample`, forever, so no group is ever lossy on its own.
//! * [`Level::Error`] lines are emitted in full on *every* occurrence while
//!   [`CollapsePolicy::collapse_errors`] is `false`, which is the default.
//! * The number of live groups never exceeds [`CollapsePolicy::max_tracked`].
//! * The [`Summary`] emitted by `finish` accounts for every line pushed:
//!   `lines_in == lines_out + lines_folded`, always.
//!
//! # Fingerprints
//!
//! A line is fingerprinted from its [`Line::plain`] text plus its level and
//! stream. `plain` is what stage 2 hands over, so once masking is wired in,
//! two lines differing only in a timestamp, address or path hash the same.
//! `raw` is never used for matching: it is the display source of truth.
//! Grouping by level as well as text keeps a line the classifier called a
//! warning from being summarised under one it called info.
//!
//! # Blocks
//!
//! Constructed via [`FoldingCollapser::with_policy_and_detector`], the collapser
//! also groups multi-line units with a [`BlockDetector`]. Lines of an open
//! block are buffered (bounded by the detector's line cap) until the block
//! closes and its fingerprint is known; the block is then emitted as
//! `BlockStart(fp)`, its lines, `BlockEnd(fp)`. A block that repeats and holds
//! no error line folds into a single [`Event::Folded`]. Blocks containing an
//! error are never folded, exactly as for single lines.

use std::collections::HashMap;

use xxhash_rust::xxh3::Xxh3;

use crate::{BlockDetector, Classified, CollapsePolicy, Event, Level, Line, Summary};

/// Domain tag for line fingerprints, so they cannot collide with the block
/// fingerprints produced by [`BlockDetector`].
const LINE_DOMAIN: u8 = 0x4C;

/// Sentinel for "no slot" in the LRU links.
const NONE: usize = usize::MAX;

fn level_tag(level: Level) -> u8 {
    match level {
        Level::Error => 0,
        Level::Warning => 1,
        Level::Info => 2,
        Level::Progress => 3,
        Level::Trace => 4,
    }
}

fn stream_tag(stream: crate::Stream) -> u8 {
    match stream {
        crate::Stream::Stdout => 0,
        crate::Stream::Stderr => 1,
    }
}

/// Fingerprint one line for grouping purposes.
fn fingerprint(line: &Line, level: Level) -> u64 {
    let mut hash = Xxh3::new();
    // Fixed-width tags keep the encoding unambiguous: byte 0 is the domain,
    // byte 1 the level, byte 2 the stream, the rest is the masked text.
    hash.update(&[LINE_DOMAIN, level_tag(level), stream_tag(line.stream)]);
    hash.update(line.plain.as_bytes());
    hash.digest()
}

/// One tracked group: a fingerprint, its verbatim first sighting and a count.
#[derive(Debug)]
struct Slot {
    fp: u64,
    sample: Classified,
    count: u64,
    /// Whether this group has already been reported as collapsed, so that
    /// `Summary::groups` counts distinct groups rather than fold events.
    collapsed: bool,
    prev: usize,
    next: usize,
}

/// Outcome of [`Lru::upsert`].
enum Upsert<'a> {
    /// First sighting. The slot now holds the new sample with `count == 1`;
    /// there is nothing for the caller to do.
    New,
    /// A repeat. The caller owns bumping `count` and reading `sample`;
    /// `first_fold` is true only the first time this group actually folds.
    Repeat {
        slot: &'a mut Slot,
        first_fold: bool,
    },
}

/// Bounded LRU over fingerprints.
///
/// Intrusive doubly linked list plus a hash index, so lookup, promotion and
/// eviction are all O(1) and the hot path never scans. Slot indices are
/// recycled through a free list, so the backing `Vec` never grows past the
/// high-water mark of live groups.
#[derive(Debug)]
struct Lru {
    cap: usize,
    slots: Vec<Slot>,
    index: HashMap<u64, usize>,
    free: Vec<usize>,
    head: usize,
    tail: usize,
    len: usize,
    evicted: u64,
}

impl Lru {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            slots: Vec::new(),
            index: HashMap::new(),
            free: Vec::new(),
            head: NONE,
            tail: NONE,
            len: 0,
            evicted: 0,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn evicted(&self) -> u64 {
        self.evicted
    }

    /// Find the group for `fp`, creating it if absent, promoting it to
    /// most-recently-used and evicting the least-recently-used group if the
    /// cache is full.
    ///
    /// `make` supplies the sample and is called only when a slot is actually
    /// created, so a repeat costs no allocation. `None` comes back when the
    /// cache can hold nothing at all (`max_tracked == 0`).
    fn upsert(&mut self, fp: u64, make: impl FnOnce() -> Classified) -> Option<Upsert<'_>> {
        if self.cap == 0 {
            return None;
        }
        if let Some(&i) = self.index.get(&fp) {
            Self::detach(&mut self.slots, &mut self.head, &mut self.tail, i);
            Self::attach(&mut self.slots, &mut self.head, &mut self.tail, i);
            let slot = &mut self.slots[i];
            let first_fold = !slot.collapsed;
            slot.collapsed = true;
            return Some(Upsert::Repeat { slot, first_fold });
        }
        while self.len >= self.cap {
            if !self.evict_tail() {
                break;
            }
        }
        let slot = Slot {
            fp,
            sample: make(),
            count: 1,
            collapsed: false,
            prev: NONE,
            next: NONE,
        };
        let i = if let Some(i) = self.free.pop() {
            self.slots[i] = slot;
            i
        } else {
            self.slots.push(slot);
            self.slots.len() - 1
        };
        self.index.insert(fp, i);
        self.len += 1;
        Self::attach(&mut self.slots, &mut self.head, &mut self.tail, i);
        Some(Upsert::New)
    }

    fn evict_tail(&mut self) -> bool {
        if self.tail == NONE {
            return false;
        }
        let i = self.tail;
        Self::detach(&mut self.slots, &mut self.head, &mut self.tail, i);
        self.index.remove(&self.slots[i].fp);
        self.free.push(i);
        self.len -= 1;
        self.evicted += 1;
        true
    }

    fn attach(slots: &mut [Slot], head: &mut usize, tail: &mut usize, i: usize) {
        slots[i].prev = NONE;
        slots[i].next = *head;
        if *head == NONE {
            *tail = i;
        } else {
            slots[*head].prev = i;
        }
        *head = i;
    }

    fn detach(slots: &mut [Slot], head: &mut usize, tail: &mut usize, i: usize) {
        let (prev, next) = (slots[i].prev, slots[i].next);
        if prev == NONE {
            *head = next;
        } else {
            slots[prev].next = next;
        }
        if next == NONE {
            *tail = prev;
        } else {
            slots[next].prev = prev;
        }
        slots[i].prev = NONE;
        slots[i].next = NONE;
    }
}

/// See module docs.
///
/// # Call order
///
/// `push` as many lines as you like, then `finish` exactly once, at end of
/// input. `finish` is idempotent -- a second call returns nothing -- but lines
/// pushed after it would not appear in the [`Summary`] already emitted.
#[derive(Debug)]
pub struct FoldingCollapser {
    policy: CollapsePolicy,
    /// Groups of individual lines.
    lines: Lru,
    /// Groups of whole blocks, keyed by the detector's block fingerprint.
    blocks: Lru,
    detector: Option<BlockDetector>,
    /// Lines of the block currently open, awaiting its fingerprint.
    pending: Vec<Classified>,
    pending_error: bool,
    summary: Summary,
    finished: bool,
}

impl Default for FoldingCollapser {
    fn default() -> Self {
        Self::with_policy(CollapsePolicy::default())
    }
}

impl FoldingCollapser {
    /// Build a collapser with an explicit policy, without block grouping.
    #[must_use]
    pub fn with_policy(policy: CollapsePolicy) -> Self {
        Self {
            policy,
            lines: Lru::new(policy.max_tracked),
            blocks: Lru::new(policy.max_tracked),
            detector: None,
            pending: Vec::new(),
            pending_error: false,
            summary: Summary::default(),
            finished: false,
        }
    }

    /// Build a collapser that also groups multi-line blocks.
    ///
    /// The detector's line cap also bounds how many lines are buffered while
    /// a block is open, which is what keeps this bounded in memory.
    #[must_use]
    pub fn with_policy_and_detector(policy: CollapsePolicy, detector: BlockDetector) -> Self {
        Self {
            detector: Some(detector),
            ..Self::with_policy(policy)
        }
    }

    /// The policy in force.
    #[must_use]
    pub fn policy(&self) -> CollapsePolicy {
        self.policy
    }

    /// Distinct line fingerprints currently tracked. Never above
    /// [`CollapsePolicy::max_tracked`].
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.lines.len()
    }

    /// How many line groups have been dropped from the LRU so far.
    #[must_use]
    pub fn evicted(&self) -> u64 {
        self.lines.evicted()
    }

    /// Whether a block is open and its lines are still being buffered.
    #[must_use]
    pub fn is_block_open(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Accounting so far. The authoritative version is the [`Event::Summary`]
    /// that `finish` emits.
    #[must_use]
    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    /// Whether this line may be folded at all, under the policy in force.
    ///
    /// This is the whole of AGENTS.md section 1.1: errors are foldable only
    /// when the user explicitly asked for it.
    fn may_fold(&self, level: Level) -> bool {
        level != Level::Error || self.policy.collapse_errors
    }

    fn collapse_line(&mut self, c: Classified) -> Vec<Event> {
        let fp = fingerprint(&c.line, c.level);
        let may_fold = self.may_fold(c.level);

        // End the borrow on `self.lines` before touching the summary.
        let fold = match self.lines.upsert(fp, || c.clone()) {
            None | Some(Upsert::New) => None,
            Some(Upsert::Repeat { slot, first_fold }) => {
                slot.count += 1;
                let count = slot.count;
                may_fold.then(|| ((count, slot.sample.clone()), first_fold))
            }
        };

        if let Some(((count, sample), first_fold)) = fold {
            if first_fold {
                self.summary.groups += 1;
            }
            self.summary.lines_folded += 1;
            vec![Event::Folded { fp, count, sample }]
        } else {
            // First sighting, or an error line the policy protects: shown in
            // full, no matter how many times it recurs.
            self.summary.lines_out += 1;
            vec![Event::Emit(c)]
        }
    }

    /// Emit a block that the detector has just closed, under fingerprint `fp`.
    fn flush_block(&mut self, fp: u64) -> Vec<Event> {
        let block = std::mem::take(&mut self.pending);
        let had_error = std::mem::take(&mut self.pending_error);
        if block.is_empty() {
            return Vec::new();
        }
        debug_assert_eq!(had_error, block.iter().any(|c| c.level == Level::Error));

        let sample = block[0].clone();
        // A block folds only if every line in it may fold. Any error inside
        // pins it open, the same way it pins a single line open.
        let foldable = !had_error && block.iter().all(|c| self.may_fold(c.level));
        let counted = block.len();

        let fold = match self.blocks.upsert(fp, || sample) {
            None | Some(Upsert::New) => None,
            Some(Upsert::Repeat { slot, first_fold }) => {
                slot.count += 1;
                let count = slot.count;
                foldable.then(|| ((count, slot.sample.clone()), first_fold))
            }
        };

        if let Some(((count, sample), first_fold)) = fold {
            if first_fold {
                self.summary.groups += 1;
            }
            self.summary.lines_folded += u64::try_from(counted).unwrap_or(u64::MAX);
            return vec![
                Event::BlockStart(fp),
                Event::Folded { fp, count, sample },
                Event::BlockEnd(fp),
            ];
        }

        let mut events = Vec::with_capacity(counted + 2);
        events.push(Event::BlockStart(fp));
        for c in block {
            events.extend(self.collapse_line(c));
        }
        events.push(Event::BlockEnd(fp));
        events
    }
}

impl crate::Collapser for FoldingCollapser {
    fn push(&mut self, c: Classified) -> Vec<Event> {
        self.summary.lines_in += 1;
        if c.level == Level::Error {
            self.summary.errors += 1;
        }

        let Some(detector) = self.detector.as_mut() else {
            return self.collapse_line(c);
        };

        let update = detector.push(&c.line);
        let mut events = Vec::new();
        if let Some(fp) = update.closed {
            events.extend(self.flush_block(fp));
        }
        if update.opened || update.joined {
            self.pending_error |= c.level == Level::Error;
            self.pending.push(c);
        } else {
            events.extend(self.collapse_line(c));
        }
        events
    }

    fn finish(&mut self) -> Vec<Event> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;

        let mut events = Vec::new();
        if let Some(fp) = self.detector.as_mut().and_then(BlockDetector::force_close) {
            events.extend(self.flush_block(fp));
        }
        events.push(Event::Summary(self.summary.clone()));
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Collapser, Stream};

    fn line(no: u64, text: &str) -> Line {
        Line::new(text.to_string(), text.to_string(), Stream::Stdout, no)
    }

    fn classified(no: u64, level: Level, text: &str) -> Classified {
        Classified {
            line: line(no, text),
            level,
            rule_id: None,
            highlight: level == Level::Error,
        }
    }

    /// A block: one marker-opened first line plus indented continuations.
    const PANIC: [&str; 4] = [
        "thread 'main' panicked at src/main.rs:5:9:",
        "assertion failed: a == b",
        "stack backtrace:",
        "   0: rust_begin_unwind",
    ];

    #[test]
    fn first_sighting_is_emitted_then_repeats_fold() {
        let mut c = FoldingCollapser::default();
        let events = c.push(classified(1, Level::Warning, "retrying request"));
        assert!(matches!(events.as_slice(), [Event::Emit(_)]), "{events:?}");

        let events = c.push(classified(2, Level::Warning, "retrying request"));
        match events.as_slice() {
            [Event::Folded { count, sample, .. }] => {
                assert_eq!(*count, 2);
                assert_eq!(sample.line.no, 1, "sample must be the first occurrence");
            }
            other => panic!("expected one Folded, got {other:?}"),
        }

        let events = c.push(classified(3, Level::Warning, "retrying request"));
        match events.as_slice() {
            [Event::Folded { count, .. }] => assert_eq!(*count, 3),
            other => panic!("expected one Folded, got {other:?}"),
        }
    }

    #[test]
    fn repeated_errors_are_every_one_emitted() {
        let mut c = FoldingCollapser::default();
        for no in 1..=5 {
            let events = c.push(classified(no, Level::Error, "connection refused"));
            match events.as_slice() {
                [Event::Emit(line)] => assert_eq!(line.line.no, no),
                other => panic!("error line {no} was folded: {other:?}"),
            }
        }
        assert_eq!(c.tracked(), 1, "the group is tracked, just never folded");
    }

    #[test]
    fn errors_fold_only_when_the_policy_opts_in() {
        let policy = CollapsePolicy {
            collapse_errors: true,
            ..CollapsePolicy::default()
        };
        let mut c = FoldingCollapser::with_policy(policy);
        let _ = c.push(classified(1, Level::Error, "connection refused"));
        let events = c.push(classified(2, Level::Error, "connection refused"));
        match events.as_slice() {
            [Event::Folded { count, .. }] => assert_eq!(*count, 2),
            other => panic!("expected the opt-in to fold, got {other:?}"),
        }
    }

    #[test]
    fn lru_bounds_the_number_of_tracked_fingerprints() {
        let policy = CollapsePolicy {
            collapse_errors: false,
            max_tracked: 8,
        };
        let mut c = FoldingCollapser::with_policy(policy);
        for no in 1..=8 {
            let _ = c.push(classified(no, Level::Info, &format!("line {no}")));
        }
        assert_eq!(c.tracked(), 8);
        assert_eq!(c.evicted(), 0);

        let _ = c.push(classified(9, Level::Info, "line 9"));
        assert_eq!(c.tracked(), 8, "the LRU must not grow past its capacity");
        assert_eq!(c.evicted(), 1);
    }

    #[test]
    fn a_recently_used_group_survives_eviction() {
        let policy = CollapsePolicy {
            collapse_errors: false,
            max_tracked: 2,
        };
        let mut c = FoldingCollapser::with_policy(policy);
        let _ = c.push(classified(1, Level::Info, "keep me"));
        let _ = c.push(classified(2, Level::Info, "other"));
        let _ = c.push(classified(3, Level::Info, "keep me")); // promotes group 1
        let _ = c.push(classified(4, Level::Info, "newcomer")); // evicts group 2
        let events = c.push(classified(5, Level::Info, "keep me"));
        match events.as_slice() {
            [Event::Folded { count, .. }] => assert_eq!(*count, 3),
            other => panic!("the promoted group must survive, got {other:?}"),
        }
    }

    #[test]
    fn an_evicted_fingerprint_starts_a_fresh_group() {
        let policy = CollapsePolicy {
            collapse_errors: false,
            max_tracked: 2,
        };
        let mut c = FoldingCollapser::with_policy(policy);
        let _ = c.push(classified(1, Level::Info, "first"));
        let _ = c.push(classified(2, Level::Warning, "second"));
        let _ = c.push(classified(3, Level::Info, "third"));

        // "first" was evicted by "third"; it must be treated as brand new.
        let events = c.push(classified(4, Level::Info, "first"));
        match events.as_slice() {
            [Event::Emit(line)] => assert_eq!(line.line.no, 4),
            other => panic!("an evicted fingerprint must restart, got {other:?}"),
        }
        let events = c.push(classified(5, Level::Info, "first"));
        match events.as_slice() {
            [Event::Folded { count, sample, .. }] => {
                assert_eq!(*count, 2);
                assert_eq!(sample.line.no, 4);
            }
            other => panic!("expected a fold after the fresh start, got {other:?}"),
        }
    }

    #[test]
    fn finish_reports_accurate_counts() {
        let mut c = FoldingCollapser::default();
        let _ = c.push(classified(1, Level::Warning, "retry"));
        let _ = c.push(classified(2, Level::Warning, "retry"));
        let _ = c.push(classified(3, Level::Warning, "retry"));
        let _ = c.push(classified(4, Level::Info, "compiling foo"));
        let _ = c.push(classified(5, Level::Error, "boom"));
        let _ = c.push(classified(6, Level::Error, "boom"));

        let events = c.finish();
        let [Event::Summary(s)] = events.as_slice() else {
            panic!("finish must emit exactly one Summary, got {events:?}");
        };
        assert_eq!(s.lines_in, 6, "{s:?}");
        assert_eq!(
            s.lines_out, 4,
            "one warning, one info and two errors: {s:?}"
        );
        assert_eq!(s.lines_folded, 2, "{s:?}");
        assert_eq!(s.groups, 1, "{s:?}");
        assert_eq!(s.errors, 2, "{s:?}");
    }

    #[test]
    fn summary_accounts_for_every_line() {
        let mut c = FoldingCollapser::default();
        for no in 1..=50 {
            let level = match no % 3 {
                0 => Level::Error,
                1 => Level::Warning,
                _ => Level::Info,
            };
            let _ = c.push(classified(no, level, &format!("m{}", no % 4)));
        }
        let events = c.finish();
        let [Event::Summary(s)] = events.as_slice() else {
            panic!("expected a summary, got {events:?}");
        };
        assert_eq!(
            s.lines_in,
            s.lines_out + s.lines_folded,
            "nothing may be silently dropped: {s:?}"
        );
    }

    #[test]
    fn the_same_text_at_different_levels_stays_separate() {
        let mut c = FoldingCollapser::default();
        let _ = c.push(classified(1, Level::Info, "same text"));
        let events = c.push(classified(2, Level::Warning, "same text"));
        assert!(matches!(events.as_slice(), [Event::Emit(_)]), "{events:?}");
    }

    #[test]
    fn the_same_text_on_different_streams_stays_separate() {
        let mut c = FoldingCollapser::default();
        let _ = c.push(classified(1, Level::Info, "same text"));
        let err = Classified {
            line: Line::new(
                "same text".to_string(),
                "same text".to_string(),
                Stream::Stderr,
                2,
            ),
            ..classified(2, Level::Info, "")
        };
        let events = c.push(err);
        assert!(matches!(events.as_slice(), [Event::Emit(_)]), "{events:?}");
    }

    #[test]
    fn a_zero_capacity_collapses_nothing() {
        let policy = CollapsePolicy {
            collapse_errors: true,
            max_tracked: 0,
        };
        let mut c = FoldingCollapser::with_policy(policy);
        for no in 1..=3 {
            let events = c.push(classified(no, Level::Warning, "same"));
            assert!(matches!(events.as_slice(), [Event::Emit(_)]), "{events:?}");
        }
        assert_eq!(c.tracked(), 0);
    }

    #[test]
    fn finish_is_idempotent() {
        let mut c = FoldingCollapser::default();
        let _ = c.push(classified(1, Level::Warning, "retry"));
        assert_eq!(c.finish().len(), 1);
        assert!(c.finish().is_empty());
    }

    // ---- block mode -------------------------------------------------------

    fn blocky() -> FoldingCollapser {
        FoldingCollapser::with_policy_and_detector(CollapsePolicy::default(), BlockDetector::new())
    }

    #[test]
    fn a_block_is_bracketed_by_start_and_end() {
        let mut c = blocky();
        let mut events = Vec::new();
        for (i, text) in PANIC.iter().enumerate() {
            events.extend(c.push(classified(i as u64 + 1, Level::Warning, text)));
        }
        assert!(c.is_block_open());
        assert!(
            events.is_empty(),
            "an open block emits nothing until it closes: {events:?}"
        );

        events = c.push(classified(99, Level::Info, "unrelated line at column zero"));
        let fp = match events.first() {
            Some(Event::BlockStart(fp)) => *fp,
            other => panic!("expected a BlockStart, got {other:?}"),
        };
        assert_eq!(events.len(), PANIC.len() + 3, "{events:?}");
        assert!(matches!(events[1], Event::Emit(_)));
        assert!(
            matches!(events[PANIC.len() + 1], Event::BlockEnd(_)),
            "{events:?}"
        );
        assert!(matches!(events[PANIC.len() + 2], Event::Emit(_)));
        assert!(!c.is_block_open());
        let end = events.iter().find_map(|e| match e {
            Event::BlockEnd(fp) => Some(*fp),
            _ => None,
        });
        assert_eq!(
            Some(fp),
            end,
            "the start and end must carry the same fingerprint"
        );
    }

    #[test]
    fn a_repeated_block_folds_into_one_event() {
        let mut c = blocky();
        for round in 0..2u64 {
            for (i, text) in PANIC.iter().enumerate() {
                let _ = c.push(classified(round * 10 + i as u64 + 1, Level::Warning, text));
            }
            let events = c.push(classified(
                50 + round,
                Level::Info,
                "unrelated line at column zero",
            ));
            if round == 0 {
                assert_eq!(events.len(), PANIC.len() + 3, "first sighting: {events:?}");
                continue;
            }
            let kinds: Vec<&str> = events
                .iter()
                .map(|e| match e {
                    Event::BlockStart(_) => "start",
                    Event::Folded { .. } => "folded",
                    Event::BlockEnd(_) => "end",
                    _ => "other",
                })
                .collect();
            assert_eq!(kinds, ["start", "folded", "end", "folded"], "{events:?}");
            match &events[1] {
                Event::Folded { fp, count, sample } => {
                    assert_eq!(*count, 2, "the whole block counts as two occurrences");
                    assert_eq!(sample.line.no, 1);
                    assert_eq!(
                        *fp,
                        match events[0] {
                            Event::BlockStart(fp) => fp,
                            _ => 0,
                        }
                    );
                }
                other => panic!("expected a Folded, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_block_containing_an_error_is_never_folded() {
        let mut c = blocky();
        let mut events = Vec::new();
        for round in 0..2u64 {
            for (i, text) in PANIC.iter().enumerate() {
                let level = if i == 1 { Level::Error } else { Level::Warning };
                events.extend(c.push(classified(round * 10 + i as u64 + 1, level, text)));
            }
            events.extend(c.push(classified(
                50 + round,
                Level::Info,
                "unrelated line at column zero",
            )));
        }
        events.extend(c.finish());

        let errors = events
            .iter()
            .filter(|e| matches!(e, Event::Emit(c) if c.level == Level::Error))
            .count();
        assert_eq!(errors, 2, "both error lines must be emitted: {events:?}");

        let [.., Event::Summary(s)] = events.as_slice() else {
            panic!("expected a trailing summary, got {events:?}");
        };
        assert_eq!(s.errors, 2);
        assert_eq!(s.lines_in, s.lines_out + s.lines_folded, "{s:?}");
    }

    #[test]
    fn blocks_fold_when_the_policy_opts_in() {
        let policy = CollapsePolicy {
            collapse_errors: true,
            ..CollapsePolicy::default()
        };
        let mut c = FoldingCollapser::with_policy_and_detector(policy, BlockDetector::new());
        let mut events = Vec::new();
        for round in 0..2u64 {
            for (i, text) in PANIC.iter().enumerate() {
                let level = if i == 1 { Level::Error } else { Level::Warning };
                events.extend(c.push(classified(round * 10 + i as u64 + 1, level, text)));
            }
            events.extend(c.push(classified(
                50 + round,
                Level::Info,
                "unrelated line at column zero",
            )));
        }
        events.extend(c.finish());
        assert!(
            events.iter().any(|e| matches!(e, Event::Folded { .. })),
            "with errors foldable the block should collapse: {events:?}"
        );
    }

    #[test]
    fn an_open_block_is_flushed_at_end_of_input() {
        let mut c = blocky();
        for (i, text) in PANIC.iter().enumerate() {
            let _ = c.push(classified(i as u64 + 1, Level::Warning, text));
        }
        assert!(c.is_block_open());
        let events = c.finish();
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                Event::BlockStart(_) => "start",
                Event::Emit(_) => "emit",
                Event::BlockEnd(_) => "end",
                Event::Summary(_) => "summary",
                Event::Folded { .. } => "folded",
            })
            .collect();
        assert_eq!(kinds.len(), PANIC.len() + 3, "{events:?}");
        assert_eq!(*kinds.last().unwrap(), "summary");
    }

    #[test]
    fn block_mode_still_accounts_for_every_line() {
        let mut c = blocky();
        for round in 0..3u64 {
            for (i, text) in PANIC.iter().enumerate() {
                let level = if i == 3 { Level::Error } else { Level::Warning };
                let _ = c.push(classified(round * 10 + i as u64 + 1, level, text));
            }
            let _ = c.push(classified(
                50 + round,
                Level::Info,
                "unrelated line at column zero",
            ));
        }
        let events = c.finish();
        let [.., Event::Summary(s)] = events.as_slice() else {
            panic!("expected a trailing summary, got {events:?}");
        };
        assert_eq!(s.lines_in, 15);
        assert_eq!(s.lines_in, s.lines_out + s.lines_folded, "{s:?}");
        assert_eq!(s.errors, 3);
    }
}
