//! The PDF object model (ISO 32000-2:2020 §7.3).
//!
//! Objects borrow from the input where they can and remember the byte span they were parsed
//! from, so later layers can splice or re-serialise exactly those bytes.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

/// An indirect reference: object number and generation (§7.3.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjRef {
    /// Object number.
    pub num: u32,
    /// Generation number (0..=65535).
    pub generation: u16,
}

impl ObjRef {
    /// A reference to object `num` with generation `generation`.
    #[must_use]
    pub fn new(num: u32, generation: u16) -> Self {
        Self { num, generation }
    }
}

impl fmt::Display for ObjRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} R", self.num, self.generation)
    }
}

/// A parsed object and the bytes it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Object<'a> {
    /// The value.
    pub kind: ObjectKind<'a>,
    /// Byte range in the parser's input. For an array or dictionary it runs from the opening
    /// to the closing delimiter, for a reference from the number to the `R`, for a stream from
    /// the dictionary's `<<` to the end of the `endstream` keyword.
    pub span: Range<usize>,
}

/// The kinds of PDF object.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ObjectKind<'a> {
    /// `null`
    Null,
    /// `true` or `false`
    Bool(bool),
    /// An integer.
    Integer(i64),
    /// A real number.
    Real(f64),
    /// A string (literal or hexadecimal), decoded to bytes.
    String(Cow<'a, [u8]>),
    /// A name without the leading slash, with `#xx` escapes resolved.
    Name(Cow<'a, [u8]>),
    /// An array.
    Array(Vec<Object<'a>>),
    /// A dictionary.
    Dict(Dict<'a>),
    /// A stream; only produced for indirect objects (§7.3.8).
    Stream(Stream<'a>),
    /// An indirect reference.
    Ref(ObjRef),
}

impl Object<'_> {
    /// The integer value, if this is an integer.
    #[must_use]
    pub fn as_integer(&self) -> Option<i64> {
        match self.kind {
            ObjectKind::Integer(n) => Some(n),
            _ => None,
        }
    }

    /// The dictionary, for a dictionary or the dictionary of a stream.
    #[must_use]
    pub fn as_dict(&self) -> Option<&Dict<'_>> {
        match &self.kind {
            ObjectKind::Dict(d) => Some(d),
            ObjectKind::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }
}

/// One `key value` pair of a dictionary.
#[derive(Debug, Clone, PartialEq)]
pub struct DictEntry<'a> {
    /// The key, a name without the slash.
    pub key: Cow<'a, [u8]>,
    /// Where the key (including its slash) is in the input.
    pub key_span: Range<usize>,
    /// The value.
    pub value: Object<'a>,
}

/// A dictionary (§7.3.7).
///
/// All entries are kept in source order, **including repeated keys**: the file is the source of
/// truth and a writer may need the original bytes. Lookups follow the rule used by common
/// readers: **the last entry for a key wins** ([`get`](Self::get)).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dict<'a> {
    /// The entries in source order.
    pub entries: Vec<DictEntry<'a>>,
}

impl<'a> Dict<'a> {
    /// The value for `key` (a name without the slash). The last entry wins on duplicates.
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<&Object<'a>> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.key.as_ref() == key)
            .map(|e| &e.value)
    }

    /// Whether a key occurs more than once.
    #[must_use]
    pub fn has_duplicate_keys(&self) -> bool {
        self.entries
            .iter()
            .enumerate()
            .any(|(i, e)| self.entries.iter().take(i).any(|p| p.key == e.key))
    }
}

/// A stream (§7.3.8): its dictionary and where its raw bytes are. The data is not copied or
/// decoded here.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream<'a> {
    /// The stream dictionary.
    pub dict: Dict<'a>,
    /// Byte range of the raw (still encoded and, if applicable, encrypted) data, without the
    /// end-of-line markers around it.
    pub data: Range<usize>,
}

/// Something the parser had to repair to read an indirect object. Reporting these is what lets
/// the document layer mark a file as repaired and the UI warn about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Recovery {
    /// The stream dictionary has no usable `/Length`; the end was found by searching.
    StreamLengthMissing,
    /// `/Length` is an indirect reference that could not be resolved here; the end was found
    /// by searching.
    StreamLengthUnresolved,
    /// `/Length` did not lead to `endstream`; the end was found by searching.
    StreamLengthWrong {
        /// The length the dictionary declared.
        declared: i64,
    },
    /// The `stream` keyword was not followed by CRLF or LF (§7.3.8.1).
    StreamEolNonStandard,
    /// No `endobj` followed the object.
    MissingEndobj,
}

/// An object with its identity: `n g obj ... endobj` (§7.3.10).
#[derive(Debug, Clone, PartialEq)]
pub struct IndirectObject<'a> {
    /// Object number and generation from the header.
    pub id: ObjRef,
    /// The object (a [`Stream`](ObjectKind::Stream) if the dictionary was followed by `stream`).
    pub object: Object<'a>,
    /// Byte range from the start of the object number through `endobj` (or through the last
    /// byte consumed when `endobj` is missing).
    pub span: Range<usize>,
    /// Repairs that were needed; empty for a well-formed object.
    pub recoveries: Vec<Recovery>,
}
