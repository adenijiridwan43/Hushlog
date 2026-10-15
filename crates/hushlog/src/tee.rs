//! Byte-exact capture of the input stream. **Owner: WP6.**
//!
//! This module is the reason the pipeline shape in `AGENTS.md` section 2 puts
//! `--tee` outside the stages: the bytes are written **as they arrive**, before
//! `Ingest` sees them, and are never reconstructed from a [`hushlog_core::Line`].
//!
//! That distinction is not pedantry. `Ingest` builds `Line::raw` with
//! `String::from_utf8_lossy`, so a log that is not valid UTF-8 -- truncated
//! multibyte text, binary noise, a log cut off by a killed job -- has its
//! undecodable bytes replaced with `U+FFFD` on the way in. Rebuilding the tee
//! file from those `String`s would produce a file that still looks like a log
//! and has quietly corrupted exactly the bytes the user was trying to keep.
//!
//! So [`Tee::write`] takes `&[u8]` and the CLI feeds it from the raw read
//! buffer, in the same statement that hands the same slice to `Ingest`.
//!
//! [`Ingest`]: hushlog_core::Ingest

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Write the input byte stream to disk verbatim, upstream of `Ingest`.
#[derive(Debug)]
pub(crate) struct Tee {
    /// `None` when `--tee` was not passed; every method is then a no-op.
    file: Option<File>,
    path: Option<PathBuf>,
}

impl Tee {
    /// Open the tee file, or a no-op tee when no path was given.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created. `--tee` is one of the
    /// four recovery paths named in `AGENTS.md` section 1, so a tee that
    /// silently does not exist would remove a guarantee rather than honour one:
    /// this fails loudly and the run does not start.
    pub(crate) fn open(path: Option<&Path>) -> io::Result<Self> {
        let Some(path) = path else {
            return Ok(Self {
                file: None,
                path: None,
            });
        };
        let file = File::create(path).map_err(|source| {
            io::Error::new(source.kind(), format!("--tee {}: {source}", path.display()))
        })?;
        Ok(Self {
            file: Some(file),
            path: Some(path.to_path_buf()),
        })
    }

    /// Append `bytes` to the tee file, exactly as they were read.
    ///
    /// Unbuffered on purpose: every chunk that has been read is already in the
    /// operating system's hands, so a panic or a signal part-way through a run
    /// leaves a complete log rather than a truncated one. See the note in
    /// `AGENTS.md` section 6 about why the release profile does not abort on
    /// panic.
    ///
    /// # Errors
    ///
    /// Returns the file's error. A failed tee must not be turned into a
    /// continued run, because everything downstream assumes the full log is on
    /// disk.
    pub(crate) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.write_all(bytes),
            None => Ok(()),
        }
    }

    /// The path recorded in the end-of-run summary.
    #[must_use]
    pub(crate) fn path(&self) -> Option<String> {
        self.path.as_ref().map(|p| p.display().to_string())
    }

    /// Flush and sync the tee file, reporting anything that went wrong.
    ///
    /// # Errors
    ///
    /// Returns the first failure from either the flush or the fsync. This is
    /// what makes the summary's promise -- "full log: PATH" -- true.
    pub(crate) fn finish(&mut self) -> io::Result<()> {
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        file.flush()?;
        file.sync_all()?;
        Ok(())
    }
}

impl Drop for Tee {
    /// Best-effort flush on the way out, including a panic.
    ///
    /// [`Tee::finish`] is the checked path and is what a normal run uses. This
    /// exists for the paths that never reach it -- an unwinding renderer, an
    /// `expect` somewhere in a dependency -- where losing the tail of the log
    /// would be the whole failure.
    fn drop(&mut self) {
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
            let _ = file.sync_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    /// A file path that does not exist yet, in a directory that does.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hushlog-tee-{}-{}", std::process::id(), tag));
        std::fs::create_dir_all(&dir).expect("a scratch directory should be creatable");
        dir.join("full.log")
    }

    #[test]
    fn a_tee_with_no_path_writes_nothing_and_succeeds() {
        let mut tee = Tee::open(None).expect("no path is not a failure");
        tee.write(b"anything at all")
            .expect("a disabled tee cannot fail");
        tee.finish().expect("a disabled tee cannot fail");
        assert!(tee.path().is_none());
    }

    #[test]
    fn bytes_arrive_unchanged_including_those_that_are_not_utf8() {
        let path = scratch("bytes");
        let raw: Vec<u8> = vec![
            0x00, 0xff, 0xfe, b'e', b'r', b'r', 0x80, b'\n', 0xe4, 0xb8, b'x', 0x1b, b'[', 0x31,
            b'm', b'\r', 0xc3,
        ];
        let mut tee = Tee::open(Some(&path)).expect("scratch path");
        tee.write(&raw[..7]).expect("first chunk");
        tee.write(&raw[7..]).expect("second chunk");
        tee.finish().expect("finish");

        let mut back = Vec::new();
        std::fs::File::open(&path)
            .expect("the tee file should exist")
            .read_to_end(&mut back)
            .expect("readable");
        assert_eq!(back, raw, "the tee must be byte-identical to the input");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_tee_is_truncated_not_appended_to() {
        let path = scratch("truncate");
        std::fs::write(&path, b"stale contents from an earlier run\n").expect("seed the file");
        let mut tee = Tee::open(Some(&path)).expect("scratch path");
        tee.write(b"fresh\n").expect("write");
        tee.finish().expect("finish");
        let back = std::fs::read(&path).expect("readable");
        assert_eq!(back, b"fresh\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_unwritable_tee_path_fails_loudly() {
        // A path whose parent is a file, not a directory: `File::create` must
        // fail rather than let the run start with a tee that is not there.
        let dir = scratch("unwritable");
        std::fs::create_dir_all(&dir).expect("scratch directory");
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"x").expect("seed the blocker");
        let result = Tee::open(Some(&blocker.join("nested.log")));
        assert!(
            result.is_err(),
            "a tee that cannot be created must not be ignored"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
