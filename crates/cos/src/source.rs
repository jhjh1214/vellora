//! Immutable byte sources for the original PDF file.
//!
//! A [`ByteSource`] is random access by range. Reads outside the source fail with
//! [`Error::OutOfRange`]; they never panic and never clamp silently.
//!
//! Implementations here are safe Rust only (`cos` forbids `unsafe`):
//! - [`MemorySource`]: an in-memory buffer; reads borrow, no copy.
//! - [`FileSource`]: positioned reads on an open file ("read handle" in the plan).
//!
//! A memory-mapped source needs `unsafe` (`memmap2::Mmap::map`), so it must live in a crate that
//! is allowed to use it and implement this trait there. [`ByteSource::slice`] returns a
//! `Cow` so a mapped implementation can hand out borrowed bytes without a copy.

use std::borrow::Cow;
use std::fs::File;
use std::io;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use crate::error::{Error, Result};

/// Random access to the bytes of the original file. Implementations are immutable while open.
pub trait ByteSource: Send + Sync {
    /// Total length in bytes.
    fn len(&self) -> u64;

    /// Whether the source holds no bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fills `buf` with the bytes starting at `offset`.
    ///
    /// # Errors
    /// [`Error::OutOfRange`] if `offset + buf.len()` is past the end, [`Error::Io`] if the
    /// underlying handle fails.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// The bytes in `range`, borrowed when the source can do so without copying.
    ///
    /// The default implementation allocates `range.len()` bytes after checking the range
    /// against [`len`](Self::len), so the allocation is bounded by the source size. Callers that
    /// take a size from the file must check it against their limits first.
    ///
    /// # Errors
    /// [`Error::OutOfRange`] if the range is empty-inverted or past the end.
    fn slice(&self, range: Range<u64>) -> Result<Cow<'_, [u8]>> {
        let len = check_range(&range, self.len())?;
        let mut buf = vec![0u8; len];
        self.read_at(range.start, &mut buf)?;
        Ok(Cow::Owned(buf))
    }
}

/// Validates `range` against `source_len` and returns its length as `usize`.
fn check_range(range: &Range<u64>, source_len: u64) -> Result<usize> {
    let out_of_range = || Error::OutOfRange {
        start: range.start,
        end: range.end,
        len: source_len,
    };
    if range.start > range.end || range.end > source_len {
        return Err(out_of_range());
    }
    // A range inside a source that fits in memory always fits `usize`; a larger one is rejected
    // as out of range rather than truncated.
    usize::try_from(range.end - range.start).map_err(|_| out_of_range())
}

/// An in-memory byte source.
#[derive(Debug, Clone)]
pub struct MemorySource {
    bytes: Arc<[u8]>,
}

impl MemorySource {
    /// Wraps a buffer.
    #[must_use]
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    fn bytes_in(&self, range: &Range<u64>) -> Result<&[u8]> {
        let out_of_range = || Error::OutOfRange {
            start: range.start,
            end: range.end,
            len: self.len(),
        };
        check_range(range, self.len())?;
        let start = usize::try_from(range.start).map_err(|_| out_of_range())?;
        let end = usize::try_from(range.end).map_err(|_| out_of_range())?;
        self.bytes.get(start..end).ok_or_else(out_of_range)
    }
}

impl ByteSource for MemorySource {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let wanted = offset..offset.saturating_add(buf.len() as u64);
        buf.copy_from_slice(self.bytes_in(&wanted)?);
        Ok(())
    }

    fn slice(&self, range: Range<u64>) -> Result<Cow<'_, [u8]>> {
        Ok(Cow::Borrowed(self.bytes_in(&range)?))
    }
}

/// A byte source over an open file using positioned reads. The file is never written.
///
/// The length is taken when the source is created. If the file is truncated afterwards, reads
/// past the new end fail with [`Error::Io`] (unexpected end of file) instead of returning
/// garbage; callers that need change detection must compare file metadata themselves.
#[derive(Debug)]
pub struct FileSource {
    file: File,
    len: u64,
}

impl FileSource {
    /// Opens `path` read-only.
    ///
    /// # Errors
    /// [`Error::Io`] if the file cannot be opened.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path).map_err(|source| Error::Io {
            source,
            offset: None,
        })?;
        Self::from_file(file)
    }

    /// Uses an already open file.
    ///
    /// # Errors
    /// [`Error::Io`] if its metadata cannot be read.
    pub fn from_file(file: File) -> Result<Self> {
        let len = file
            .metadata()
            .map_err(|source| Error::Io {
                source,
                offset: None,
            })?
            .len();
        Ok(Self { file, len })
    }
}

impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset.saturating_add(buf.len() as u64);
        check_range(&(offset..end), self.len)?;
        read_exact_at(&self.file, offset, buf).map_err(|source| Error::Io {
            source,
            offset: Some(offset),
        })
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        match file.seek_read(buf, offset) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                buf = &mut buf[n..];
                offset += n as u64;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const DATA: &[u8] = b"%PDF-1.7\n0123456789";

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("vellora-cos-{name}-{}", std::process::id()));
        let mut f = File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path
    }

    /// The same behaviour must hold for every implementation.
    fn exercise(source: &dyn ByteSource) {
        assert_eq!(source.len(), DATA.len() as u64);
        assert!(!source.is_empty());

        assert_eq!(&*source.slice(0..8).unwrap(), b"%PDF-1.7");
        assert_eq!(&*source.slice(9..19).unwrap(), b"0123456789");
        // The whole source and the empty range at the end are valid.
        assert_eq!(&*source.slice(0..DATA.len() as u64).unwrap(), DATA);
        assert!(source.slice(19..19).unwrap().is_empty());

        let mut buf = [0u8; 4];
        source.read_at(15, &mut buf).unwrap();
        assert_eq!(&buf, b"6789");
        source.read_at(19, &mut []).unwrap();

        // Out-of-range reads fail cleanly and report what was asked.
        let past_end = source.slice(0..20).unwrap_err();
        assert!(
            matches!(
                past_end,
                Error::OutOfRange {
                    start: 0,
                    end: 20,
                    len: 19
                }
            ),
            "{past_end}"
        );
        assert!(source.slice(20..21).is_err());
        assert!(source.slice(u64::MAX - 1..u64::MAX).is_err());
        assert!(source.slice(0..u64::MAX).is_err());
        #[allow(clippy::reversed_empty_ranges)]
        let inverted = 5..3;
        assert!(source.slice(inverted).is_err());
        let mut buf = [0u8; 4];
        assert!(source.read_at(16, &mut buf).is_err());
        assert!(source.read_at(u64::MAX, &mut buf).is_err());
        assert!(source.read_at(u64::MAX - 1, &mut buf).is_err());
    }

    #[test]
    fn memory_source_behaves() {
        exercise(&MemorySource::new(DATA));
    }

    #[test]
    fn memory_source_borrows_instead_of_copying() {
        let source = MemorySource::new(DATA);
        assert!(matches!(source.slice(0..4).unwrap(), Cow::Borrowed(_)));
    }

    #[test]
    fn file_source_behaves() {
        let path = temp_file("behaves", DATA);
        exercise(&FileSource::open(&path).unwrap());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn file_source_open_error_is_typed() {
        let err =
            FileSource::open(std::env::temp_dir().join("vellora-cos-does-not-exist")).unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "{err}");
    }

    #[test]
    fn file_source_reports_truncation_as_io_error() {
        let path = temp_file("truncated", DATA);
        let source = FileSource::open(&path).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(10)
            .unwrap();
        let err = source.slice(0..19).unwrap_err();
        assert!(
            matches!(
                err,
                Error::Io {
                    offset: Some(0),
                    ..
                }
            ),
            "{err}"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn empty_sources() {
        let memory = MemorySource::new(Vec::<u8>::new());
        assert!(memory.is_empty());
        assert!(memory.slice(0..0).unwrap().is_empty());
        assert!(memory.slice(0..1).is_err());
        let path = temp_file("empty", b"");
        let file = FileSource::open(&path).unwrap();
        assert!(file.is_empty());
        assert!(file.slice(0..1).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
