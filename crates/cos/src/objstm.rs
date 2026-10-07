//! Object streams (ISO 32000-2:2020 §7.5.7): many objects packed into one stream.
//!
//! The stream is decoded once. Its header (`N` pairs of object number and offset) is read
//! eagerly because it is small, but the objects themselves are parsed only when asked for, so
//! opening a document never parses the contents of every object stream.
//!
//! Decryption is a separate concern: an encrypted file's object stream is decrypted as a whole
//! *before* it reaches [`ObjectStream::new`], and the objects inside are not encrypted again.

use crate::error::{Error, Result, SyntaxKind};
use crate::filter::decode_flate_only;
use crate::lexer::{Lexer, TokenKind};
use crate::limits::{DecodeBudget, LimitKind, Limits};
use crate::object::{Dict, Object, ObjectKind};
use crate::parser::Parser;

/// A decoded object stream with its header read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectStream {
    data: Vec<u8>,
    /// `(object number, offset relative to `first`)` for each contained object.
    entries: Vec<(u32, usize)>,
    first: usize,
}

fn malformed(offset: usize) -> Error {
    Error::Syntax {
        kind: SyntaxKind::MalformedObjectStream,
        offset: offset as u64,
    }
}

impl ObjectStream {
    /// Reads the header of an already decoded object stream with `n` objects whose data starts
    /// at `first` (the `/N` and `/First` of its dictionary).
    ///
    /// Offsets in errors are positions in `decoded`, not in the file.
    ///
    /// # Errors
    /// [`Error::LimitExceeded`] if `n` is over the array-entry limit,
    /// [`SyntaxKind::MalformedObjectStream`] if `first` is past the data or the header does not
    /// hold `n` pairs of non-negative integers.
    pub fn new(decoded: Vec<u8>, n: usize, first: usize, limits: &Limits) -> Result<Self> {
        limits.check(LimitKind::ArrayEntries, n as u64, None)?;
        let header = decoded.get(..first).ok_or_else(|| malformed(0))?;
        // A pair needs at least four bytes ("1 0 "), so a larger `n` cannot be real; this keeps
        // the allocation below proportional to the data actually present.
        if n > header.len() / 2 + 1 {
            return Err(malformed(0));
        }
        let mut lexer = Lexer::new(header, limits);
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let number = integer(&mut lexer)?;
            let offset = integer(&mut lexer)?;
            let (Ok(number), Ok(offset)) = (u32::try_from(number), usize::try_from(offset)) else {
                return Err(malformed(lexer.position()));
            };
            entries.push((number, offset));
        }
        Ok(Self {
            data: decoded,
            entries,
            first,
        })
    }

    /// Decodes the stream described by `dict` (its raw bytes are `raw`) and reads the header.
    /// Only no filter and `FlateDecode` work until the full filter chain exists (M0 task 10).
    ///
    /// `/N` and `/First` must be direct integers, as they are in every file that follows the
    /// spec.
    ///
    /// # Errors
    /// [`SyntaxKind::MalformedObjectStream`] for a missing or negative `/N` or `/First`, plus
    /// the errors of [`new`](Self::new) and of decoding.
    pub fn from_stream(
        dict: &Dict<'_>,
        raw: &[u8],
        limits: &Limits,
        budget: Option<&mut DecodeBudget>,
        offset: Option<u64>,
    ) -> Result<Self> {
        let count = |key: &[u8]| match dict.get(key).map(|o| &o.kind) {
            Some(ObjectKind::Integer(n)) => usize::try_from(*n).ok(),
            _ => None,
        };
        let (Some(n), Some(first)) = (count(b"N"), count(b"First")) else {
            return Err(malformed(0));
        };
        let decoded = decode_flate_only(dict, raw, limits, budget, offset)?;
        Self::new(decoded, n, first, limits)
    }

    /// Number of objects in the stream.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Size of the decoded data in bytes (what keeping the stream in memory costs).
    #[must_use]
    pub fn decoded_len(&self) -> usize {
        self.data.len()
    }

    /// Whether the stream holds no objects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The object number at `index`, from the header.
    #[must_use]
    pub fn object_number(&self, index: usize) -> Option<u32> {
        self.entries.get(index).map(|&(number, _)| number)
    }

    /// The index of the first object with this number.
    #[must_use]
    pub fn index_of(&self, number: u32) -> Option<usize> {
        self.entries.iter().position(|&(n, _)| n == number)
    }

    /// Parses the object at `index`. Spans in the result are positions in the decoded data.
    /// The caller checks [`object_number`](Self::object_number) against the number the
    /// cross-reference said to expect.
    ///
    /// # Errors
    /// [`Error::OutOfRange`] for an index past the end, [`SyntaxKind::MalformedObjectStream`]
    /// if the offset points outside the data, and any parser error for the object itself.
    pub fn object<'s>(&'s self, index: usize, limits: &Limits) -> Result<Object<'s>> {
        let &(_, offset) = self.entries.get(index).ok_or(Error::OutOfRange {
            start: index as u64,
            end: index as u64 + 1,
            len: self.entries.len() as u64,
        })?;
        let position = self
            .first
            .checked_add(offset)
            .filter(|&p| p < self.data.len())
            .ok_or_else(|| malformed(self.first))?;
        Parser::at(&self.data, position, limits).parse_object()
    }
}

fn integer(lexer: &mut Lexer<'_, '_>) -> Result<i64> {
    match lexer.next_token()? {
        Some(token) => match token.kind {
            TokenKind::Integer(n) => Ok(n),
            _ => Err(malformed(token.span.start)),
        },
        None => Err(malformed(lexer.position())),
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    use super::*;
    use crate::object::ObjRef;

    /// `(number, body)` pairs packed the way a writer would.
    fn pack(objects: &[(u32, &str)]) -> (Vec<u8>, usize) {
        let mut body = String::new();
        let mut header = String::new();
        for (number, text) in objects {
            let _ = write!(header, "{number} {} ", body.len());
            body.push_str(text);
            body.push('\n');
        }
        let first = header.len();
        (format!("{header}{body}").into_bytes(), first)
    }

    fn limits() -> Limits {
        Limits::default()
    }

    #[test]
    fn objects_are_found_by_index_and_number() {
        let (data, first) = pack(&[
            (10, "<< /A 1 >>"),
            (11, "[1 2 3]"),
            (12, "(text)"),
            (13, "7 0 R"),
        ]);
        let stream = ObjectStream::new(data, 4, first, &limits()).unwrap();
        assert_eq!(stream.len(), 4);
        assert_eq!(stream.object_number(2), Some(12));
        assert_eq!(stream.object_number(4), None);
        assert_eq!(stream.index_of(11), Some(1));
        assert_eq!(stream.index_of(99), None);

        let dict = stream.object(0, &limits()).unwrap();
        assert_eq!(
            dict.as_dict().unwrap().get(b"A").unwrap().as_integer(),
            Some(1)
        );
        let array = stream.object(1, &limits()).unwrap();
        assert!(matches!(array.kind, ObjectKind::Array(items) if items.len() == 3));
        assert!(matches!(
            stream.object(2, &limits()).unwrap().kind,
            ObjectKind::String(_)
        ));
        assert_eq!(
            stream.object(3, &limits()).unwrap().kind,
            ObjectKind::Ref(ObjRef::new(7, 0))
        );
    }

    #[test]
    fn extraction_is_lazy_a_broken_object_does_not_spoil_the_others() {
        let (data, first) = pack(&[(1, "<< /Fine true >>"), (2, "<< /Broken"), (3, "42")]);
        let stream = ObjectStream::new(data, 3, first, &limits()).unwrap();
        assert!(stream.object(1, &limits()).is_err());
        assert!(stream.object(0, &limits()).is_ok());
        assert_eq!(stream.object(2, &limits()).unwrap().as_integer(), Some(42));
    }

    #[test]
    fn empty_stream_is_fine() {
        let stream = ObjectStream::new(Vec::new(), 0, 0, &limits()).unwrap();
        assert!(stream.is_empty());
        assert!(matches!(
            stream.object(0, &limits()),
            Err(Error::OutOfRange { .. })
        ));
    }

    #[test]
    fn malformed_headers_are_typed_errors() {
        let kind = |result: Result<ObjectStream>| match result.unwrap_err() {
            Error::Syntax { kind, .. } => kind,
            other => panic!("{other:?}"),
        };
        // /First beyond the data.
        assert_eq!(
            kind(ObjectStream::new(b"1 0 ".to_vec(), 1, 99, &limits())),
            SyntaxKind::MalformedObjectStream
        );
        // Fewer pairs than /N.
        assert_eq!(
            kind(ObjectStream::new(b"1 0 x".to_vec(), 2, 4, &limits())),
            SyntaxKind::MalformedObjectStream
        );
        // Non-integers, negatives.
        assert_eq!(
            kind(ObjectStream::new(b"a b  x".to_vec(), 1, 4, &limits())),
            SyntaxKind::MalformedObjectStream
        );
        assert_eq!(
            kind(ObjectStream::new(b"-1 0 x".to_vec(), 1, 5, &limits())),
            SyntaxKind::MalformedObjectStream
        );
        assert_eq!(
            kind(ObjectStream::new(b"1 -4 x".to_vec(), 1, 5, &limits())),
            SyntaxKind::MalformedObjectStream
        );
        // An /N that the header could not possibly hold does not allocate.
        assert_eq!(
            kind(ObjectStream::new(
                b"1 0 x".to_vec(),
                1_000_000,
                4,
                &limits()
            )),
            SyntaxKind::MalformedObjectStream
        );
    }

    #[test]
    fn object_count_limit_is_enforced() {
        let tight = Limits {
            max_array_entries: 2,
            ..Limits::default()
        };
        let (data, first) = pack(&[(1, "1"), (2, "2"), (3, "3")]);
        assert!(matches!(
            ObjectStream::new(data, 3, first, &tight).unwrap_err(),
            Error::LimitExceeded {
                limit: LimitKind::ArrayEntries,
                max: 2,
                value: 3,
                ..
            }
        ));
    }

    #[test]
    fn offsets_outside_the_data_are_errors_at_extraction_time() {
        let stream = ObjectStream::new(b"1 500 x".to_vec(), 1, 6, &limits()).unwrap();
        assert!(matches!(
            stream.object(0, &limits()).unwrap_err(),
            Error::Syntax {
                kind: SyntaxKind::MalformedObjectStream,
                ..
            }
        ));
        // A huge offset must not overflow.
        let huge = format!("1 {} x", i64::MAX);
        let stream =
            ObjectStream::new(huge.clone().into_bytes(), 1, huge.len() - 1, &limits()).unwrap();
        assert!(stream.object(0, &limits()).is_err());
    }

    #[test]
    fn from_stream_decodes_flate_and_reads_n_and_first() {
        let (data, first) = pack(&[(5, "<< /K /V >>"), (6, "99")]);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&data).unwrap();
        let packed = encoder.finish().unwrap();

        let text = format!("<< /Type /ObjStm /N 2 /First {first} /Filter /FlateDecode >>");
        let dict_object = Parser::new(text.as_bytes(), &limits())
            .parse_object()
            .unwrap();
        let dict = dict_object.as_dict().unwrap();
        let stream = ObjectStream::from_stream(dict, &packed, &limits(), None, Some(100)).unwrap();
        assert_eq!(stream.index_of(6), Some(1));
        assert_eq!(stream.object(1, &limits()).unwrap().as_integer(), Some(99));

        // Without a filter the raw bytes are used as they are.
        let text = format!("<< /Type /ObjStm /N 2 /First {first} >>");
        let dict_object = Parser::new(text.as_bytes(), &limits())
            .parse_object()
            .unwrap();
        let stream =
            ObjectStream::from_stream(dict_object.as_dict().unwrap(), &data, &limits(), None, None)
                .unwrap();
        assert_eq!(stream.len(), 2);

        // Missing /N or /First.
        for text in [
            "<< /First 4 >>",
            "<< /N 1 >>",
            "<< /N -1 /First 4 >>",
            "<< /N 1 0 R /First 4 >>",
        ] {
            let dict_object = Parser::new(text.as_bytes(), &limits())
                .parse_object()
                .unwrap();
            assert!(
                ObjectStream::from_stream(
                    dict_object.as_dict().unwrap(),
                    &data,
                    &limits(),
                    None,
                    None
                )
                .is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn from_stream_rejects_unsupported_filters_and_enforces_decode_limits() {
        let dict_object = Parser::new(b"<< /N 1 /First 4 /Filter /LZWDecode >>", &limits())
            .parse_object()
            .unwrap();
        let err =
            ObjectStream::from_stream(dict_object.as_dict().unwrap(), b"x", &limits(), None, None)
                .unwrap_err();
        assert!(matches!(err, Error::Decode { .. }), "{err:?}");

        let bomb = {
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&vec![0u8; 8 * 1024 * 1024]).unwrap();
            encoder.finish().unwrap()
        };
        let tight = Limits {
            max_decoded_stream_bytes: 1024 * 1024,
            ..Limits::default()
        };
        let dict_object = Parser::new(b"<< /N 1 /First 4 /Filter /FlateDecode >>", &tight)
            .parse_object()
            .unwrap();
        let err =
            ObjectStream::from_stream(dict_object.as_dict().unwrap(), &bomb, &tight, None, None)
                .unwrap_err();
        assert!(
            matches!(
                err,
                Error::LimitExceeded {
                    limit: LimitKind::DecodedStreamBytes,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    proptest::proptest! {
        #[test]
        fn arbitrary_data_never_panics(
            data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..300),
            n in 0usize..20,
            first in 0usize..300,
        ) {
            if let Ok(stream) = ObjectStream::new(data, n, first, &Limits::default()) {
                for index in 0..=stream.len() {
                    let _ = stream.object(index, &Limits::default());
                }
            }
        }
    }
}
