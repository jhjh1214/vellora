//! Typed errors for `cos`.
//!
//! Every error that comes from input carries the byte offset it was found at when one is known,
//! so a failure can be traced back to a place in the file.

use std::io;

use crate::limits::LimitKind;

/// Convenience alias used across `cos`.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong in `cos`.
///
/// The enum is non-exhaustive: later parser, filter and writer tasks add variants.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A read asked for bytes outside the byte source.
    #[error("read of bytes {start}..{end} is outside the source (length {len})")]
    OutOfRange {
        /// First requested byte.
        start: u64,
        /// One past the last requested byte.
        end: u64,
        /// Length of the byte source.
        len: u64,
    },

    /// A resource limit from [`Limits`](crate::limits::Limits) was exceeded.
    #[error("{} limit exceeded: {value} > {max}{}", .limit.describe(), at(*.offset))]
    LimitExceeded {
        /// Which limit.
        limit: LimitKind,
        /// The configured maximum.
        max: u64,
        /// The value that exceeded it. For a ratio limit this is the observed ratio.
        value: u64,
        /// Byte offset in the file where the limit was hit, if known.
        offset: Option<u64>,
    },

    /// The input is not valid PDF syntax at this point.
    #[error("{kind} at byte offset {offset}")]
    Syntax {
        /// What is wrong.
        kind: SyntaxKind,
        /// Byte offset where the offending construct starts.
        offset: u64,
    },

    /// A stream filter could not decode its input.
    #[error("{filter} decode error: {detail}{}", at(*.offset))]
    Decode {
        /// Filter name, e.g. `FlateDecode`.
        filter: &'static str,
        /// What went wrong.
        detail: &'static str,
        /// Byte offset of the stream data in the file, if known.
        offset: Option<u64>,
    },

    /// The document is encrypted in a way that cannot be read, or it needs a password.
    #[error("encryption: {kind}")]
    Encryption {
        /// What is wrong.
        kind: EncryptionError,
    },

    /// The requested write cannot be done (see [`crate::write`]).
    #[error("write: {kind}")]
    Write {
        /// What is wrong.
        kind: WriteError,
    },

    /// The underlying file or handle failed.
    #[error("I/O error{}: {source}", at(*.offset))]
    Io {
        /// The operating system error.
        #[source]
        source: io::Error,
        /// Byte offset of the failed read, if known.
        offset: Option<u64>,
    },
}

impl Error {
    /// The byte offset in the file this error refers to, when known.
    #[must_use]
    pub fn offset(&self) -> Option<u64> {
        match self {
            Self::OutOfRange { start, .. } => Some(*start),
            Self::Syntax { offset, .. } => Some(*offset),
            Self::LimitExceeded { offset, .. }
            | Self::Decode { offset, .. }
            | Self::Io { offset, .. } => *offset,
            Self::Encryption { .. } | Self::Write { .. } => None,
        }
    }
}

impl From<EncryptionError> for Error {
    fn from(kind: EncryptionError) -> Self {
        Self::Encryption { kind }
    }
}

impl From<WriteError> for Error {
    fn from(kind: WriteError) -> Self {
        Self::Write { kind }
    }
}

/// Why a write request was refused (see [`crate::write`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WriteError {
    /// The file's cross-reference was rebuilt by the recovery scan (or the file has no usable
    /// `/Root`), so there is no revision chain to append to: use the full writer.
    #[error("the file was repaired and cannot be updated incrementally; write it in full")]
    NeedsFullRewrite,
    /// Object number 0 (the head of the free list), or a number past what the cross-reference
    /// can hold.
    #[error("object number {0} cannot be written")]
    InvalidObjectNumber(u32),
    /// The change replaces or frees the `/Encrypt` dictionary, which the writer never touches.
    #[error("object {0} is the encryption dictionary and cannot be changed")]
    EncryptionDictionary(u32),
    /// An object value of kind stream was given without its data; use
    /// [`NewObject::Stream`](crate::write::NewObject::Stream).
    #[error("a stream object needs its data")]
    StreamWithoutData,
    /// A file offset needs more digits than a classic cross-reference table has (10).
    #[error("file offset {0} does not fit a cross-reference table entry")]
    OffsetTooLarge(u64),
    /// A cross-reference entry that the chosen section style cannot hold (an entry for an object
    /// inside an object stream in a classic table).
    #[error("this cross-reference style cannot hold the entry")]
    EntryNotRepresentable,
    /// The document has no `/Root`, so there is nothing to write.
    #[error("the document has no /Root")]
    NoRoot,
    /// Re-encryption was requested for a document that is not encrypted.
    #[error("the document is not encrypted")]
    NotEncrypted,
}

/// Why encrypted content cannot be read (see [`crate::crypt`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EncryptionError {
    /// The document needs a password and none has been accepted yet.
    #[error("a password is required")]
    PasswordRequired,
    /// Neither the user nor the owner password matches.
    #[error("incorrect password")]
    IncorrectPassword,
    /// A security handler other than `/Standard` (for example a public-key handler).
    #[error("unsupported security handler")]
    UnsupportedHandler,
    /// An algorithm version (`/V`) that is not supported.
    #[error("unsupported encryption version {version}")]
    UnsupportedVersion {
        /// The `/V` value.
        version: i64,
    },
    /// A security handler revision (`/R`) that does not exist or is not supported.
    #[error("unsupported security handler revision {revision}")]
    UnsupportedRevision {
        /// The `/R` value.
        revision: i64,
    },
    /// A crypt filter with a method (`/CFM`) that is not supported.
    #[error("unsupported crypt filter method")]
    UnsupportedCryptFilter,
    /// `/StmF`, `/StrF` or a stream's `/Crypt` filter names a crypt filter that `/CF` lacks.
    #[error("unknown crypt filter")]
    UnknownCryptFilter,
    /// The `/Encrypt` dictionary is missing something or has an unusable value.
    #[error("malformed /Encrypt dictionary: {0}")]
    Malformed(&'static str),
}

/// What kind of syntax error was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SyntaxKind {
    /// A literal string `( ... )` reached the end of the data before its closing parenthesis.
    #[error("unterminated literal string")]
    UnterminatedString,
    /// A hexadecimal string `< ... >` reached the end of the data before its closing `>`.
    #[error("unterminated hexadecimal string")]
    UnterminatedHexString,
    /// The data ended in the middle of an array, dictionary or indirect object.
    #[error("unexpected end of data")]
    UnexpectedEof,
    /// A token that cannot appear here (a stray `]`, `>>`, `)` or an unknown keyword).
    #[error("unexpected token")]
    UnexpectedToken,
    /// A dictionary key that is not a name.
    #[error("dictionary key is not a name")]
    DictKeyNotName,
    /// A dictionary key without a value before `>>`.
    #[error("dictionary key has no value")]
    DictMissingValue,
    /// An indirect object that does not start with `<number> <generation> obj`.
    #[error("malformed indirect object header")]
    MalformedObjectHeader,
    /// A stream whose `endstream` keyword could not be found.
    #[error("missing endstream")]
    MissingEndstream,
    /// No `startxref` keyword with an offset in the searched tail of the file.
    #[error("startxref not found")]
    StartxrefNotFound,
    /// An offset (from `startxref` or `/Prev`) that points outside the file.
    #[error("cross-reference offset is outside the file")]
    InvalidXrefOffset,
    /// The offset does not lead to the `xref` keyword (for example a cross-reference stream,
    /// which a later task handles).
    #[error("expected the xref keyword")]
    ExpectedXrefKeyword,
    /// A broken subsection header or entry in a cross-reference table.
    #[error("malformed cross-reference table")]
    MalformedXrefSection,
    /// The trailer is missing or is not a dictionary.
    #[error("trailer is not a dictionary")]
    TrailerNotDictionary,
    /// The `/Prev` chain leads back to a section that was already read.
    #[error("cross-reference /Prev chain loops")]
    XrefPrevLoop,
    /// An object stream whose header or offsets do not hold together.
    #[error("malformed object stream")]
    MalformedObjectStream,
    /// The recovery scan found no objects at all.
    #[error("no objects found to recover")]
    NothingToRecover,
    /// The recovery scan found objects but neither a trailer with `/Root` nor a catalog.
    #[error("no document root found")]
    RootNotFound,
}

fn at(offset: Option<u64>) -> String {
    offset.map_or_else(String::new, |o| format!(" at byte offset {o}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_is_reported_per_variant() {
        let e = Error::OutOfRange {
            start: 10,
            end: 20,
            len: 5,
        };
        assert_eq!(e.offset(), Some(10));
        let e = Error::LimitExceeded {
            limit: LimitKind::NestingDepth,
            max: 64,
            value: 65,
            offset: Some(7),
        };
        assert_eq!(e.offset(), Some(7));
        let e = Error::Io {
            source: io::Error::other("x"),
            offset: None,
        };
        assert_eq!(e.offset(), None);
    }

    #[test]
    fn messages_include_the_offset() {
        let e = Error::LimitExceeded {
            limit: LimitKind::NestingDepth,
            max: 64,
            value: 65,
            offset: Some(7),
        };
        let text = e.to_string();
        assert!(text.contains("nesting depth"), "{text}");
        assert!(text.contains("65 > 64"), "{text}");
        assert!(text.contains("byte offset 7"), "{text}");
    }
}
