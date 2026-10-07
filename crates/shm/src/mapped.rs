//! A read-only memory-mapped file, usable as a `vellora-cos` byte source.

use std::borrow::Cow;
use std::fs::File;
use std::ops::Range;

use memmap2::Mmap;
use vellora_cos::ByteSource;

use crate::Error;

/// The bytes of an open file, mapped read-only into the address space.
///
/// Opening a 524 MB file takes about 2 ms and 5 MB of resident memory, against 176 ms and 505 MB
/// for reading it (ADR-0013). Pages are loaded by the operating system on demand and may be
/// dropped again under memory pressure.
///
/// # The file must not change while it is mapped
///
/// Invariant 3 of the architecture: the original is never mutated while open. The parent holds
/// the file open for reading and should deny other writers where the platform allows it. If
/// something truncates the file anyway, touching a lost page raises a fatal signal (SIGBUS on
/// Unix, an access violation on Windows) that ends the engine process. That is the same outcome
/// as any other engine crash: the client reports it and restarts, and no memory unsafety reaches
/// the UI process. Content that changes (but does not shrink) is read as ordinary, possibly
/// inconsistent, untrusted bytes.
#[derive(Debug)]
pub struct MappedFile {
    // `None` for an empty file, which cannot be mapped on every platform.
    map: Option<Mmap>,
}

impl MappedFile {
    /// Maps `file`, which must be at most `max_len` bytes.
    ///
    /// # Errors
    ///
    /// [`Error::TooLarge`] above `max_len` (checked before mapping), [`Error::Io`] if the file
    /// cannot be inspected or mapped.
    pub fn new(file: &File, max_len: u64) -> Result<Self, Error> {
        let len = file
            .metadata()
            .map_err(|source| Error::io("cannot read the size of the document", source))?
            .len();
        if len > max_len {
            return Err(Error::TooLarge { len, max: max_len });
        }
        if len == 0 {
            return Ok(Self { map: None });
        }
        // SAFETY: the mapping is read-only and private to this process's view of the file. The
        // type documentation states the one remaining hazard (the file shrinking while mapped):
        // it can only end this process, never corrupt memory elsewhere, and invariant 3 makes the
        // parent responsible for not modifying the file while it is open.
        let map = unsafe { Mmap::map(file) }
            .map_err(|source| Error::io("cannot map the document", source))?;
        Ok(Self { map: Some(map) })
    }

    /// The whole file.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        self.map.as_deref().unwrap_or_default()
    }
}

impl AsRef<[u8]> for MappedFile {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl ByteSource for MappedFile {
    fn len(&self) -> u64 {
        self.as_slice().len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> vellora_cos::Result<()> {
        let end = offset.saturating_add(buf.len() as u64);
        buf.copy_from_slice(&self.slice(offset..end)?);
        Ok(())
    }

    fn slice(&self, range: Range<u64>) -> vellora_cos::Result<Cow<'_, [u8]>> {
        let out_of_range = || vellora_cos::Error::OutOfRange {
            start: range.start,
            end: range.end,
            len: self.len(),
        };
        let start = usize::try_from(range.start).map_err(|_| out_of_range())?;
        let end = usize::try_from(range.end).map_err(|_| out_of_range())?;
        self.as_slice()
            .get(start..end)
            .map(Cow::Borrowed)
            .ok_or_else(out_of_range)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Seek, SeekFrom, Write};

    use super::*;

    const DATA: &[u8] = b"%PDF-1.7\n0123456789";

    fn file_with(bytes: &[u8]) -> File {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(bytes).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        file
    }

    #[test]
    fn reads_like_any_other_byte_source() {
        let mapped = MappedFile::new(&file_with(DATA), u64::MAX).unwrap();
        assert_eq!(mapped.len(), DATA.len() as u64);
        assert_eq!(mapped.as_slice(), DATA);
        assert!(matches!(mapped.slice(0..8).unwrap(), Cow::Borrowed(b) if b == b"%PDF-1.7"));
        assert!(mapped.slice(19..19).unwrap().is_empty());

        let mut buf = [0_u8; 4];
        mapped.read_at(15, &mut buf).unwrap();
        assert_eq!(&buf, b"6789");

        // Out of range fails cleanly, whatever the arithmetic.
        assert!(mapped.slice(0..20).is_err());
        assert!(mapped.slice(20..21).is_err());
        assert!(mapped.slice(u64::MAX - 1..u64::MAX).is_err());
        assert!(mapped.slice(0..u64::MAX).is_err());
        #[allow(clippy::reversed_empty_ranges)]
        let inverted = 5..3;
        assert!(mapped.slice(inverted).is_err());
        assert!(mapped.read_at(16, &mut buf).is_err());
        assert!(mapped.read_at(u64::MAX, &mut buf).is_err());
    }

    #[test]
    fn an_empty_file_maps_to_an_empty_source() {
        let mapped = MappedFile::new(&file_with(b""), u64::MAX).unwrap();
        assert!(mapped.is_empty());
        assert_eq!(mapped.as_slice(), &[] as &[u8]);
        assert!(mapped.slice(0..1).is_err());
    }

    #[test]
    fn a_file_over_the_limit_is_refused_before_mapping() {
        let error = MappedFile::new(&file_with(DATA), 18).unwrap_err();
        assert!(
            matches!(error, Error::TooLarge { len: 19, max: 18 }),
            "{error}"
        );
        MappedFile::new(&file_with(DATA), 19).unwrap();
    }
}
