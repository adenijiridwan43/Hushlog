//! The stdout handle shared with a renderer. **Owner: WP6.**
//!
//! `hushlog_render::renderer` takes its sink by value and hands back a
//! `Box<dyn Renderer>`, which does not expose `flush`. So the CLI cannot own a
//! plain `BufWriter<Stdout>`, pass it in, and flush it afterwards: the write
//! handle is gone the moment the renderer is built.
//!
//! [`SharedOut`] is the fix. The CLI keeps a clone, the renderer gets another,
//! and both refer to the same buffer. That gives us three things the contract
//! needs and a bare `Box<dyn Renderer>` cannot:
//!
//! * an explicit `flush` whose **error** we can report -- a `BufWriter` dropped
//!   inside the renderer would flush and discard the failure, which is exactly
//!   the silent loss `AGENTS.md` section 1 forbids;
//! * a buffer flushed on every code path out of a run, including a panic,
//!   because flushing is a method call rather than a `Drop`;
//! * a sink that survives a panic poisoning the mutex, so a half-written line
//!   does not take the rest of the run down with it.

use std::io::{self, BufWriter, Stdout, Write};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The buffered stdout, shared with whichever renderer owns a clone.
#[derive(Clone, Debug)]
pub(crate) struct SharedOut(Arc<Mutex<BufWriter<Stdout>>>);

impl SharedOut {
    /// Wrap this process's stdout in a buffer.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(BufWriter::new(io::stdout()))))
    }

    /// Push everything the renderers have written out to the operating system.
    ///
    /// # Errors
    ///
    /// Returns the first write failure. Callers must surface it; dropping the
    /// error would lose the tail of the run without saying so.
    pub(crate) fn flush(&self) -> io::Result<()> {
        self.lock().flush()
    }
}

impl SharedOut {
    /// Take the lock, ignoring poisoning.
    ///
    /// A renderer that panicked mid-line leaves the mutex poisoned but the
    /// buffer perfectly usable: a poisoned mutex is a warning about unwinding,
    /// not a corrupt memory. Refusing to write afterwards would turn one bad
    /// line into a silently truncated log, which is the worse failure.
    fn lock(&self) -> MutexGuard<'_, BufWriter<Stdout>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Write for SharedOut {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.lock().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        SharedOut::flush(self)
    }
}
