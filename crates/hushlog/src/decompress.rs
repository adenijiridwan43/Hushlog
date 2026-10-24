//! Auto-decompression of gzip/bzip2 input on ingest.
//!
//! `lnav` transparently decompresses gzip and bzip2 logs; hushlog does the same
//! here. The first chunk's magic bytes decide the format: gzip opens with
//! `1f 8b`, bzip2 with `42 5a` ("BZ"). Everything after that is decompressed
//! before the pipeline sees it, so a `.gz` or `.bz2` log is filtered exactly
//! like a plain one.
//!
//! `# Losslessness` (AGENTS.md section 1)
//!
//! * `--tee` writes the raw (still-compressed) bytes upstream of this module,
//!   so the tee file is always a byte-exact copy of the input, compression and
//!   all. It is never rebuilt from decompressed `Line`s.
//! * `--raw` never decompresses: it is a pure byte pass-through.
//! * Nothing downstream is decompressed unless the magic bytes say so, so a
//!   plain log that happens to begin with two coincidental bytes is not
//!   mangled.
//!
//! `# Streaming and bounded memory`
//!
//! `push` is called once per read chunk and may return zero, one, or many
//! decompressed bytes. Undecoded input is held in `pending`, which never grows
//! beyond one undecoded fragment, so memory stays bounded regardless of input
//! size. Chunk boundaries are irrelevant: the decompressed output for a stream
//! is identical no matter how it is chunked.

use std::io::{self, Read};

/// Gzip magic: `\x1f\x8b`.
const GZIP_MAGIC: [u8; 2] = *b"\x1f\x8b";
/// Bzip2 magic: `BZ`.
const BZIP2_MAGIC: [u8; 2] = *b"BZ";

/// Detected compression format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Gzip,
    Bzip2,
}

/// A push-based decompressor. See module docs.
pub struct Decompressor {
    /// The detected format, `None` until the first bytes arrive.
    format: Option<Format>,
    /// Raw bytes received but not yet consumed by a decoder.
    pending: Vec<u8>,
    /// Decompressed output waiting to be handed to the pipeline.
    out: Vec<u8>,
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Decompressor {
    /// Create a decompressor that detects its format from the first bytes.
    #[must_use]
    pub fn new() -> Self {
        Self {
            format: None,
            pending: Vec::new(),
            out: Vec::new(),
        }
    }

    /// Feed the next chunk of raw bytes, returning every decompressed byte now
    /// available.
    ///
    /// Chunk boundaries never matter: the output for a stream is identical no
    /// matter how it is chunked.
    ///
    /// # Errors
    ///
    /// Returns an error if the compressed data is corrupt. A truncated stream
    /// at end of input is not an error here; it is handled by
    /// [`finish`](Self::finish).
    pub fn push(&mut self, raw: &[u8]) -> io::Result<Vec<u8>> {
        self.out.clear();

        // The first chunk decides whether this input is compressed at all.
        if self.format.is_none() && self.pending.is_empty() {
            self.format = Self::detect(raw);
        }

        match self.format {
            Some(fmt) => {
                self.pending.extend_from_slice(raw);
                let (consumed, decoded): (usize, Vec<u8>) = {
                    let mut cursor = io::Cursor::new(&self.pending);
                    let (ok, decoded_bytes) = {
                        let mut decoder: Box<dyn Read> = match fmt {
                            Format::Gzip => Box::new(flate2::read::GzDecoder::new(&mut cursor)),
                            Format::Bzip2 => Box::new(bzip2::read::BzDecoder::new(&mut cursor)),
                        };
                        let mut out = Vec::new();
                        let ok = match io::copy(&mut decoder, &mut out) {
                            Ok(_) => true,
                            // Incomplete input: keep everything and wait for more.
                            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => false,
                            Err(e) => return Err(e),
                        };
                        (ok, if ok { out } else { Vec::new() })
                    }; // decoder dropped here
                    let consumed = if ok {
                        usize::try_from(cursor.position()).expect("pending fits in usize")
                    } else {
                        0
                    };
                    (consumed, decoded_bytes)
                }; // cursor dropped here, ending the borrow of `self.pending`
                self.out.extend_from_slice(&decoded);
                if consumed > 0 {
                    self.pending.drain(..consumed);
                }
            }
            None => {
                // Not compressed (or detection pending): pass straight through.
                self.out.extend_from_slice(raw);
            }
        }

        Ok(std::mem::take(&mut self.out))
    }

    /// End of input: flush anything still buffered.
    ///
    /// A well-formed compressed stream leaves nothing behind. A truncated one
    /// leaves `pending` non-empty; we return what decoded and clear it rather
    /// than failing a run on a log that was cut off mid-write.
    pub fn finish(&mut self) -> io::Result<Vec<u8>> {
        self.out.clear();
        if let Some(fmt) = self.format {
            if !self.pending.is_empty() {
                let decoded = {
                    let mut cursor = io::Cursor::new(&self.pending);
                    let decoded = {
                        let mut decoder: Box<dyn Read> = match fmt {
                            Format::Gzip => Box::new(flate2::read::GzDecoder::new(&mut cursor)),
                            Format::Bzip2 => Box::new(bzip2::read::BzDecoder::new(&mut cursor)),
                        };
                        let mut out = Vec::new();
                        match io::copy(&mut decoder, &mut out) {
                            Ok(_) => out,
                            // Truncated stream: keep what decoded, drop the rest.
                            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => out,
                            Err(e) => return Err(e),
                        }
                    }; // decoder dropped here
                    decoded
                }; // cursor dropped here, ending the borrow of `self.pending`
                // A well-formed stream consumed everything; a truncated one
                // leaves unreadable bytes we drop rather than fail the run.
                self.pending.clear();
                self.out.extend_from_slice(&decoded);
            }
        }
        Ok(std::mem::take(&mut self.out))
    }

    /// Detect the compression format from the leading bytes.
    fn detect(bytes: &[u8]) -> Option<Format> {
        if bytes.len() < 2 {
            return None;
        }
        match &bytes[..2] {
            b if b == GZIP_MAGIC => Some(Format::Gzip),
            b if b == BZIP2_MAGIC => Some(Format::Bzip2),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn plain_bytes_pass_through_unchanged() {
        let mut d = Decompressor::new();
        let out = d.push(b"hello world\n").expect("plain input passes through");
        assert_eq!(out, b"hello world\n");
        assert_eq!(d.format, None);
    }

    /// A real gzip stream decompresses transparently, no matter how it is
    /// chunked. Detection locks on from the magic bytes.
    #[test]
    fn gzip_stream_decompresses_across_chunk_boundaries() {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"hello world\nhello world\n").unwrap();
        let compressed = encoder.finish().unwrap();

        let mut d = Decompressor::new();
        let mut got = Vec::new();
        for chunk in compressed.chunks(3) {
            got.extend_from_slice(&d.push(chunk).unwrap());
        }
        got.extend_from_slice(&d.finish().unwrap());
        assert_eq!(got, b"hello world\nhello world\n");
        assert_eq!(d.format, Some(Format::Gzip));
        assert!(d.pending.is_empty());
    }

    /// A real bzip2 stream decompresses transparently too.
    #[test]
    fn bzip2_stream_decompresses_across_chunk_boundaries() {
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
        encoder.write_all(b"hello world\nhello world\n").unwrap();
        let compressed = encoder.finish().unwrap();

        let mut d = Decompressor::new();
        let mut got = Vec::new();
        for chunk in compressed.chunks(5) {
            got.extend_from_slice(&d.push(chunk).unwrap());
        }
        got.extend_from_slice(&d.finish().unwrap());
        assert_eq!(got, b"hello world\nhello world\n");
        assert_eq!(d.format, Some(Format::Bzip2));
        assert!(d.pending.is_empty());
    }

    /// Chunk boundaries never matter for plain input either.
    #[test]
    fn chunk_boundaries_do_not_matter() {
        let mut d = Decompressor::new();
        let parts = ["he", "llo ", "wo", "rld", "\n"];
        let mut got = Vec::new();
        for part in parts {
            got.extend_from_slice(&d.push(part.as_bytes()).expect("push"));
        }
        got.extend_from_slice(&d.finish().expect("finish"));
        assert_eq!(got, b"hello world\n");
        assert!(d.pending.is_empty());
    }

    /// Push returns everything for plain input; finish then has nothing left.
    #[test]
    fn finish_is_empty_after_push_returned_everything() {
        let mut d = Decompressor::new();
        let first = d.push(b"part one").expect("push");
        let tail = d.finish().expect("finish");
        assert_eq!(first, b"part one");
        assert_eq!(tail, b"");
    }

    /// Corrupt compressed data fails loudly instead of silently producing
    /// garbage (AGENTS.md section 1).
    #[test]
    fn corrupt_gzip_header_is_an_error() {
        let mut d = Decompressor::new();
        let err = d.push(b"\x1f\x8bnot really gzip").expect_err("corrupt header must fail");
        assert!(err.to_string().contains("gzip"), "got: {err}");
    }
}

