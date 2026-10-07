//! Canonical, deterministic serialisation of objects (ISO 32000-2 §7.3).
//!
//! The same object always gives the same bytes, and parsing those bytes gives the same object
//! back (spans aside), which the property tests rely on:
//!
//! - Tokens are separated by exactly one space; dictionaries are `<< /K v /K2 v2 >>`.
//! - Reals are written in plain decimal, never with an exponent, and always with a `.` so that
//!   they do not read back as integers. A non-finite real is written as `0.0`.
//! - A string made only of printable ASCII is written as a literal string with `\`, `(` and `)`
//!   escaped; anything else (and nothing else) as a hexadecimal string in upper case.
//! - A name keeps regular characters as they are and writes every other byte, and `#`, as `#XX`.
//! - A dictionary with a repeated key is written once, at the position of the **last** entry:
//!   the same entry readers use (see [`Dict::get`]).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use crate::crypt::Decryptor;
use crate::error::Result;
use crate::limits::{LimitKind, Limits};
use crate::object::{Dict, DictEntry, ObjRef, Object, ObjectKind};

/// Writes `object` canonically (see the [module documentation](self)). References are written as
/// they are; a stream (which has no data here) is an error, because a stream is only meaningful
/// as an indirect object.
///
/// # Errors
/// [`WriteError::StreamWithoutData`](crate::error::WriteError::StreamWithoutData) for a stream,
/// and [`Error::LimitExceeded`](crate::Error::LimitExceeded) when arrays and dictionaries nest
/// deeper than [`Limits::max_nesting_depth`].
pub fn write_object(out: &mut Vec<u8>, object: &Object<'_>, limits: &Limits) -> Result<()> {
    Serializer::new(limits).value(out, object, ObjRef::new(0, 0), 0, false)
}

/// Serialisation settings for one writer run.
pub(crate) struct Serializer<'c> {
    limits: &'c Limits,
    /// Old object number to new object number (generation 0). `None` keeps references as they
    /// are; with a map, a reference to a number that is not in it is written as `null`.
    remap: Option<&'c HashMap<u32, u32>>,
    /// Encrypts strings and stream data when set.
    crypt: Option<&'c Decryptor>,
}

impl<'c> Serializer<'c> {
    pub(crate) fn new(limits: &'c Limits) -> Self {
        Self {
            limits,
            remap: None,
            crypt: None,
        }
    }

    #[must_use]
    pub(crate) fn with_remap(mut self, remap: &'c HashMap<u32, u32>) -> Self {
        self.remap = Some(remap);
        self
    }

    #[must_use]
    pub(crate) fn with_crypt(mut self, crypt: Option<&'c Decryptor>) -> Self {
        self.crypt = crypt;
        self
    }

    /// `n g obj <value> endobj`. `id` is the identity the object is written under (and, with
    /// encryption, the one its strings are encrypted for).
    pub(crate) fn indirect(
        &self,
        out: &mut Vec<u8>,
        id: ObjRef,
        object: &Object<'_>,
    ) -> Result<()> {
        out.extend_from_slice(format!("{} {} obj\n", id.num, id.generation).as_bytes());
        self.value(out, object, id, 0, self.crypt.is_some())?;
        out.extend_from_slice(b"\nendobj\n");
        Ok(())
    }

    /// A stream object. `/Length` is replaced by the length of `data` after encryption, written
    /// as a direct integer; `data` is encrypted here when the serializer has a decryptor.
    pub(crate) fn stream(
        &self,
        out: &mut Vec<u8>,
        id: ObjRef,
        dict: &Dict<'_>,
        data: &[u8],
    ) -> Result<()> {
        let data: Cow<'_, [u8]> = match self.crypt {
            Some(crypt) => crypt.encrypt_stream(id, dict, data)?,
            None => Cow::Borrowed(data),
        };
        let mut dict = dict.clone();
        dict.set(
            b"Length",
            Object::new(ObjectKind::Integer(
                i64::try_from(data.len()).unwrap_or(i64::MAX),
            )),
        );
        out.extend_from_slice(format!("{} {} obj\n", id.num, id.generation).as_bytes());
        self.dict(out, &dict, id, 1, self.crypt.is_some())?;
        out.extend_from_slice(b"\nstream\n");
        out.extend_from_slice(&data);
        out.extend_from_slice(b"\nendstream\nendobj\n");
        Ok(())
    }

    /// Writes one value. `encrypt` says whether its strings are encrypted (when the serializer
    /// has a decryptor): not for the `/Contents` of a signature.
    pub(crate) fn value(
        &self,
        out: &mut Vec<u8>,
        object: &Object<'_>,
        id: ObjRef,
        depth: u32,
        encrypt: bool,
    ) -> Result<()> {
        self.limits
            .check(LimitKind::NestingDepth, u64::from(depth), None)?;
        match &object.kind {
            ObjectKind::Null => out.extend_from_slice(b"null"),
            ObjectKind::Bool(true) => out.extend_from_slice(b"true"),
            ObjectKind::Bool(false) => out.extend_from_slice(b"false"),
            ObjectKind::Integer(n) => out.extend_from_slice(n.to_string().as_bytes()),
            ObjectKind::Real(r) => write_real(out, *r),
            ObjectKind::String(bytes) => match self.crypt.filter(|_| encrypt) {
                Some(crypt) => write_string(out, &crypt.encrypt_string(id, bytes)),
                None => write_string(out, bytes),
            },
            ObjectKind::Name(name) => write_name(out, name),
            ObjectKind::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b' ');
                    }
                    self.value(out, item, id, depth + 1, encrypt)?;
                }
                out.push(b']');
            }
            ObjectKind::Dict(dict) => self.dict(out, dict, id, depth + 1, encrypt)?,
            ObjectKind::Ref(reference) => self.reference(out, *reference),
            ObjectKind::Stream(_) => {
                return Err(crate::error::WriteError::StreamWithoutData.into());
            }
        }
        Ok(())
    }

    fn reference(&self, out: &mut Vec<u8>, reference: ObjRef) {
        match self.remap {
            None => out.extend_from_slice(reference.to_string().as_bytes()),
            Some(map) => match map.get(&reference.num) {
                Some(new) => out.extend_from_slice(format!("{new} 0 R").as_bytes()),
                None => out.extend_from_slice(b"null"),
            },
        }
    }

    fn dict(
        &self,
        out: &mut Vec<u8>,
        dict: &Dict<'_>,
        id: ObjRef,
        depth: u32,
        encrypt: bool,
    ) -> Result<()> {
        self.limits
            .check(LimitKind::NestingDepth, u64::from(depth), None)?;
        // The /Contents of a signature is the signature itself, never encrypted (§7.6.2).
        let signature = has_type(dict, b"Sig") || has_type(dict, b"DocTimeStamp");
        out.extend_from_slice(b"<<");
        for entry in effective_entries(dict) {
            out.push(b' ');
            write_name(out, &entry.key);
            out.push(b' ');
            let encrypt = encrypt && !(signature && entry.key.as_ref() == b"Contents");
            self.value(out, &entry.value, id, depth, encrypt)?;
        }
        out.extend_from_slice(b" >>");
        Ok(())
    }
}

/// The entries a reader sees: for a repeated key only the last one, in the order of the entries
/// that remain. Linear in the size of the dictionary (a hostile one may have 2^18 entries).
pub(crate) fn effective_entries<'d, 'a>(dict: &'d Dict<'a>) -> Vec<&'d DictEntry<'a>> {
    let mut seen = HashSet::new();
    let mut kept: Vec<_> = dict
        .entries
        .iter()
        .rev()
        .filter(|entry| seen.insert(entry.key.as_ref()))
        .collect();
    kept.reverse();
    kept
}

fn has_type(dict: &Dict<'_>, name: &[u8]) -> bool {
    matches!(dict.get(b"Type").map(|t| &t.kind), Some(ObjectKind::Name(n)) if n.as_ref() == name)
}

fn write_real(out: &mut Vec<u8>, real: f64) {
    if !real.is_finite() {
        out.extend_from_slice(b"0.0");
        return;
    }
    // `Display` for f64 is the shortest text that reads back exactly and never uses an exponent.
    let text = real.to_string();
    out.extend_from_slice(text.as_bytes());
    if !text.contains('.') {
        out.extend_from_slice(b".0");
    }
}

fn write_string(out: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.iter().all(|b| (0x20..=0x7E).contains(b)) {
        out.push(b'(');
        for &b in bytes {
            if matches!(b, b'\\' | b'(' | b')') {
                out.push(b'\\');
            }
            out.push(b);
        }
        out.push(b')');
    } else {
        out.push(b'<');
        for &b in bytes {
            out.extend_from_slice(format!("{b:02X}").as_bytes());
        }
        out.push(b'>');
    }
}

fn write_name(out: &mut Vec<u8>, name: &[u8]) {
    out.push(b'/');
    for &b in name {
        let regular = (0x21..=0x7E).contains(&b)
            && !matches!(
                b,
                b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' | b'#'
            );
        if regular {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{b:02X}").as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    fn written(source: &str) -> String {
        let limits = Limits::default();
        let object = Parser::new(source.as_bytes(), &limits)
            .parse_object()
            .expect("test input parses");
        let mut out = Vec::new();
        write_object(&mut out, &object, &limits).expect("writes");
        String::from_utf8(out).expect("canonical output is ASCII here")
    }

    #[test]
    fn scalars_are_canonical() {
        assert_eq!(written("null"), "null");
        assert_eq!(written("true"), "true");
        assert_eq!(written("-17"), "-17");
        assert_eq!(written("5.0"), "5.0");
        assert_eq!(written("0.25"), "0.25");
        assert_eq!(written("+3"), "3");
        assert_eq!(written("12 0 R"), "12 0 R");
    }

    #[test]
    fn reals_never_read_back_as_integers_or_use_exponents() {
        let mut out = Vec::new();
        write_real(&mut out, 1e21);
        assert_eq!(out, b"1000000000000000000000.0");
        out.clear();
        write_real(&mut out, 1e-7);
        assert_eq!(out, b"0.0000001");
        out.clear();
        write_real(&mut out, f64::NAN);
        assert_eq!(out, b"0.0");
        out.clear();
        write_real(&mut out, f64::INFINITY);
        assert_eq!(out, b"0.0");
    }

    #[test]
    fn strings_are_literal_when_printable_and_hex_otherwise() {
        assert_eq!(written("(hello)"), "(hello)");
        assert_eq!(written(r"(a\(b\)c\\d)"), r"(a\(b\)c\\d)");
        assert_eq!(written("()"), "()");
        assert_eq!(written(r"(line\nbreak)"), "<6C696E650A627265616B>");
        assert_eq!(written("<00ff>"), "<00FF>");
    }

    #[test]
    fn names_escape_irregular_bytes() {
        assert_eq!(written("/Name"), "/Name");
        assert_eq!(written("/A#20B"), "/A#20B");
        assert_eq!(written("/A#23B"), "/A#23B");
        assert_eq!(written("/"), "/");
        assert_eq!(written("/a#2fb"), "/a#2Fb");
    }

    #[test]
    fn containers_use_single_spaces() {
        assert_eq!(written("[1 2/A(x)3 0 R]"), "[1 2 /A (x) 3 0 R]");
        assert_eq!(
            written("<</A 1/B[1]/C<</D null>>>>"),
            "<< /A 1 /B [1] /C << /D null >> >>"
        );
        assert_eq!(written("<<>>"), "<< >>");
        assert_eq!(written("[]"), "[]");
    }

    #[test]
    fn duplicate_keys_are_written_once_with_the_last_value() {
        assert_eq!(written("<</A 1 /B 2 /A 3>>"), "<< /B 2 /A 3 >>");
    }

    #[test]
    fn nesting_beyond_the_limit_is_an_error_and_not_a_stack_overflow() {
        let mut object = Object::new(ObjectKind::Null);
        for _ in 0..200 {
            object = Object::new(ObjectKind::Array(vec![object]));
        }
        let limits = Limits::default();
        let error = write_object(&mut Vec::new(), &object, &limits).unwrap_err();
        assert!(
            matches!(
                error,
                crate::Error::LimitExceeded {
                    limit: LimitKind::NestingDepth,
                    ..
                }
            ),
            "{error:?}"
        );
    }

    #[test]
    fn a_bare_stream_value_is_refused() {
        let stream = Object::new(ObjectKind::Stream(crate::object::Stream {
            dict: Dict::default(),
            data: 0..0,
        }));
        let error = write_object(&mut Vec::new(), &stream, &Limits::default()).unwrap_err();
        assert!(matches!(
            error,
            crate::Error::Write {
                kind: crate::error::WriteError::StreamWithoutData
            }
        ));
    }

    #[test]
    fn the_remap_renumbers_references_and_nulls_unknown_ones() {
        let limits = Limits::default();
        let object = Parser::new(b"[5 2 R 6 0 R]", &limits)
            .parse_object()
            .unwrap();
        let map = HashMap::from([(5, 1)]);
        let serializer = Serializer::new(&limits).with_remap(&map);
        let mut out = Vec::new();
        serializer
            .value(&mut out, &object, ObjRef::new(0, 0), 0, false)
            .unwrap();
        assert_eq!(out, b"[1 0 R null]");
    }

    /// Equality that ignores spans (a parsed object has them, a generated one does not).
    fn same(a: &Object<'_>, b: &Object<'_>) -> bool {
        match (&a.kind, &b.kind) {
            (ObjectKind::Array(x), ObjectKind::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
            }
            (ObjectKind::Dict(x), ObjectKind::Dict(y)) => {
                x.entries.len() == y.entries.len()
                    && x.entries
                        .iter()
                        .zip(&y.entries)
                        .all(|(x, y)| x.key == y.key && same(&x.value, &y.value))
            }
            (x, y) => x == y,
        }
    }

    fn object_strategy() -> impl proptest::strategy::Strategy<Value = Object<'static>> {
        use proptest::prelude::*;
        let leaf = prop_oneof![
            Just(ObjectKind::Null),
            any::<bool>().prop_map(ObjectKind::Bool),
            any::<i32>().prop_map(|n| ObjectKind::Integer(i64::from(n))),
            any::<f64>()
                .prop_filter("finite", |r| r.is_finite())
                .prop_map(ObjectKind::Real),
            proptest::collection::vec(any::<u8>(), 0..24)
                .prop_map(|b| ObjectKind::String(b.into())),
            proptest::collection::vec(any::<u8>(), 0..24).prop_map(|b| ObjectKind::Name(b.into())),
            (1u32..100_000, 0u16..=65535).prop_map(|(n, g)| ObjectKind::Ref(ObjRef::new(n, g))),
        ]
        .prop_map(Object::new);
        leaf.prop_recursive(4, 48, 6, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..6)
                    .prop_map(|items| Object::new(ObjectKind::Array(items))),
                proptest::collection::btree_map(
                    proptest::collection::vec(any::<u8>(), 0..8),
                    inner,
                    0..6
                )
                .prop_map(|entries| {
                    let mut dict = Dict::default();
                    for (key, value) in entries {
                        dict.set(&key, value);
                    }
                    Object::new(ObjectKind::Dict(dict))
                }),
            ]
        })
    }

    proptest::proptest! {
        /// `parse(write(object)) == object`, and writing again gives the same bytes.
        #[test]
        fn writing_then_parsing_gives_the_object_back(object in object_strategy()) {
            let limits = Limits::default();
            let mut bytes = Vec::new();
            write_object(&mut bytes, &object, &limits).unwrap();
            let parsed = Parser::new(&bytes, &limits).parse_object().unwrap();
            proptest::prop_assert!(same(&object, &parsed), "{:?} vs {:?}", object, parsed);
            let mut again = Vec::new();
            write_object(&mut again, &parsed, &limits).unwrap();
            proptest::prop_assert_eq!(bytes, again);
        }
    }

    #[test]
    fn a_stream_gets_a_direct_length_of_the_stored_bytes() {
        let limits = Limits::default();
        let dict = Parser::new(b"<</Length 99 0 R /Foo /Bar>>", &limits)
            .parse_object()
            .unwrap();
        let dict = dict.as_dict().unwrap().clone();
        let mut out = Vec::new();
        Serializer::new(&limits)
            .stream(&mut out, ObjRef::new(4, 0), &dict, b"hello")
            .unwrap();
        assert_eq!(
            out,
            b"4 0 obj\n<< /Foo /Bar /Length 5 >>\nstream\nhello\nendstream\nendobj\n"
        );
    }
}
