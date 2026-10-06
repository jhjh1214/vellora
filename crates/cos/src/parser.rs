//! Object parser (ISO 32000-2:2020 §7.3).
//!
//! Parses direct objects (null, booleans, numbers, strings, names, arrays, dictionaries and
//! references) and indirect objects `n g obj ... endobj`, including streams. Every object keeps
//! its source span.
//!
//! Limits: nesting of arrays and dictionaries, entries per array and dictionary, and string and
//! name lengths (through the lexer) all come from [`Limits`]. Recursion depth equals
//! `max_nesting_depth`, so keep that limit modest (the default is 64).
//!
//! Recovery (reported through [`Recovery`], never silent):
//! - A wrong or missing `/Length` makes the parser search for `endstream`; the search is bounded
//!   by `max_decoded_stream_bytes`. If the first `endstream` is inside binary data the stream
//!   comes out short; a later layer can notice through other checks.
//! - A missing `endobj` is accepted, and the next token is left unread.
//!
//! Not recovered, because the document layer decides how to repair: syntax errors inside a
//! direct object (a stray `]`, a dictionary key that is not a name, an unterminated array).
//!
//! Duplicate dictionary keys are all kept; lookups use the last one ([`Dict::get`]).

use crate::error::{Error, Result, SyntaxKind};
use crate::lexer::{Lexer, StreamEol, Token, TokenKind};
use crate::limits::{LimitKind, Limits};
use crate::object::{
    Dict, DictEntry, IndirectObject, ObjRef, Object, ObjectKind, Recovery, Stream,
};

/// Resolves an indirect `/Length` to its value. Return `None` if it cannot be resolved.
pub type LengthResolver<'l> = &'l dyn Fn(ObjRef) -> Option<i64>;

/// A parser over an in-memory slice. Offsets are positions in that slice.
pub struct Parser<'a, 'l> {
    lexer: Lexer<'a, 'l>,
    limits: &'l Limits,
    resolver: Option<LengthResolver<'l>>,
}

impl<'a, 'l> Parser<'a, 'l> {
    /// A parser at the start of `data`.
    #[must_use]
    pub fn new(data: &'a [u8], limits: &'l Limits) -> Self {
        Self::at(data, 0, limits)
    }

    /// A parser starting at `pos` (clamped to the end of `data`).
    #[must_use]
    pub fn at(data: &'a [u8], pos: usize, limits: &'l Limits) -> Self {
        Self {
            lexer: Lexer::at(data, pos, limits),
            limits,
            resolver: None,
        }
    }

    /// Lets the parser read an indirect `/Length` while parsing streams.
    #[must_use]
    pub fn with_length_resolver(mut self, resolver: LengthResolver<'l>) -> Self {
        self.resolver = Some(resolver);
        self
    }

    /// Current position.
    #[must_use]
    pub fn position(&self) -> usize {
        self.lexer.position()
    }

    /// Moves to `pos` (clamped to the end of the input).
    pub fn seek(&mut self, pos: usize) {
        self.lexer.seek(pos);
    }

    /// Parses one direct object. Streams are not direct objects, so a dictionary is returned as
    /// a dictionary even if `stream` follows.
    ///
    /// # Errors
    /// [`Error::Syntax`] for malformed input, [`Error::LimitExceeded`] when a limit is hit.
    pub fn parse_object(&mut self) -> Result<Object<'a>> {
        self.value(1)
    }

    /// Parses `n g obj <object> [stream ...] endobj` (§7.3.10, §7.3.8).
    ///
    /// # Errors
    /// [`Error::Syntax`] with [`SyntaxKind::MalformedObjectHeader`] if the header is not
    /// `<number> <generation> obj`, [`SyntaxKind::MissingEndstream`] if a stream's end cannot
    /// be found, and any error from [`parse_object`](Self::parse_object).
    pub fn parse_indirect_object(&mut self) -> Result<IndirectObject<'a>> {
        let (id, start) = self.object_header()?;
        let mut recoveries = Vec::new();
        let mut object = self.value(1)?;
        let mut end = object.span.end;

        let next = self.peek();
        if let Some(Token {
            kind: TokenKind::Keyword(b"stream"),
            span,
        }) = &next
        {
            let ObjectKind::Dict(_) = &object.kind else {
                return Err(syntax(SyntaxKind::UnexpectedToken, span.start));
            };
            let ObjectKind::Dict(dict) = std::mem::replace(&mut object.kind, ObjectKind::Null)
            else {
                return Err(syntax(SyntaxKind::UnexpectedToken, span.start));
            };
            self.lexer.seek(span.end);
            let (stream, stream_end) = self.stream_body(dict, span.start, &mut recoveries)?;
            object.kind = ObjectKind::Stream(stream);
            object.span.end = stream_end;
            end = stream_end;
        }

        // `endobj`; if it is missing, accept the object and leave the next token unread.
        if let Some(Token {
            kind: TokenKind::Keyword(b"endobj"),
            span,
        }) = self.peek()
        {
            self.lexer.seek(span.end);
            end = span.end;
        } else {
            self.lexer.seek(end);
            recoveries.push(Recovery::MissingEndobj);
        }
        Ok(IndirectObject {
            id,
            object,
            span: start..end,
            recoveries,
        })
    }

    /// Reads `n g obj`, returning the id and the offset where it starts.
    fn object_header(&mut self) -> Result<(ObjRef, usize)> {
        let bad = |offset: usize| syntax(SyntaxKind::MalformedObjectHeader, offset);
        let first = self.next()?.ok_or_else(|| bad(self.position()))?;
        let start = first.span.start;
        let num = match first.kind {
            TokenKind::Integer(n) => u32::try_from(n).map_err(|_| bad(start))?,
            _ => return Err(bad(start)),
        };
        let generation = match self.next()?.map(|t| t.kind) {
            Some(TokenKind::Integer(g)) => u16::try_from(g).map_err(|_| bad(start))?,
            _ => return Err(bad(start)),
        };
        match self.next()?.map(|t| t.kind) {
            Some(TokenKind::Keyword(b"obj")) => Ok((ObjRef { num, generation }, start)),
            _ => Err(bad(start)),
        }
    }

    /// Parses the stream data after the `stream` keyword (already consumed). Returns the stream
    /// and the offset just past `endstream`.
    fn stream_body(
        &mut self,
        dict: Dict<'a>,
        keyword_offset: usize,
        recoveries: &mut Vec<Recovery>,
    ) -> Result<(Stream<'a>, usize)> {
        let data = self.lexer.data();
        let start = self.lexer.skip_stream_eol();
        if !matches!(start.eol, StreamEol::CrLf | StreamEol::Lf) {
            recoveries.push(Recovery::StreamEolNonStandard);
        }
        let data_start = start.data_start;

        // Preferred path: trust /Length if `endstream` really follows the data.
        let (declared, length_problem) = match dict.get(b"Length").map(|o| &o.kind) {
            Some(ObjectKind::Integer(n)) => (Some(*n), None),
            Some(ObjectKind::Ref(r)) => match self.resolver.and_then(|resolve| resolve(*r)) {
                Some(n) => (Some(n), None),
                None => (None, Some(Recovery::StreamLengthUnresolved)),
            },
            _ => (None, Some(Recovery::StreamLengthMissing)),
        };
        if let Some(declared) = declared
            && let Some((data_end, after)) = self.verify_length(data_start, declared)
        {
            self.lexer.seek(after);
            let stream = Stream {
                dict,
                data: data_start..data_end,
            };
            return Ok((stream, after));
        }
        recoveries.push(match (declared, length_problem) {
            (Some(declared), _) => Recovery::StreamLengthWrong { declared },
            (None, Some(problem)) => problem,
            (None, None) => Recovery::StreamLengthMissing,
        });

        // Recovery: search for `endstream` within the scan bound.
        let window_end = usize::try_from(self.limits.max_decoded_stream_bytes)
            .ok()
            .and_then(|max| data_start.checked_add(max))
            .map_or(data.len(), |end| end.min(data.len()));
        let found = find_endstream(data.get(data_start..window_end).unwrap_or_default())
            .map(|i| data_start + i)
            .ok_or_else(|| syntax(SyntaxKind::MissingEndstream, keyword_offset))?;
        let data_end = trim_eol_before(data, data_start, found);
        let after = found + b"endstream".len();
        self.lexer.seek(after);
        Ok((
            Stream {
                dict,
                data: data_start..data_end,
            },
            after,
        ))
    }

    /// Checks that `endstream` follows `declared` bytes of data. Returns the data end and the
    /// offset after the keyword.
    fn verify_length(&self, data_start: usize, declared: i64) -> Option<(usize, usize)> {
        let len = usize::try_from(declared).ok()?;
        let data_end = data_start.checked_add(len)?;
        if data_end > self.lexer.data().len() {
            return None;
        }
        let mut probe = Lexer::at(self.lexer.data(), data_end, self.limits);
        match probe.next_token() {
            Ok(Some(Token {
                kind: TokenKind::Keyword(b"endstream"),
                span,
            })) => Some((data_end, span.end)),
            _ => None,
        }
    }

    /// The next token, without consuming it. A lexing error here is not the caller's concern
    /// (the token is optional), so it reads as "nothing useful follows".
    fn peek(&mut self) -> Option<Token<'a>> {
        let save = self.position();
        let token = self.lexer.next_token().ok().flatten();
        self.lexer.seek(save);
        token
    }

    /// Like [`peek`](Self::peek) but lexing errors are reported, for places where a token is
    /// required (an unterminated string must stay an `UnterminatedString` error).
    fn peek_required(&mut self) -> Result<Option<Token<'a>>> {
        let save = self.position();
        let token = self.lexer.next_token();
        self.lexer.seek(save);
        token
    }

    fn next(&mut self) -> Result<Option<Token<'a>>> {
        self.lexer.next_token()
    }

    fn value(&mut self, depth: u32) -> Result<Object<'a>> {
        let token = self
            .next()?
            .ok_or_else(|| syntax(SyntaxKind::UnexpectedEof, self.position()))?;
        let Token { kind, span } = token;
        let kind = match kind {
            TokenKind::Integer(n) => return Ok(self.integer_or_ref(n, span)),
            TokenKind::Real(x) => ObjectKind::Real(x),
            TokenKind::LiteralString(s) | TokenKind::HexString(s) => ObjectKind::String(s),
            TokenKind::Name(n) => ObjectKind::Name(n),
            TokenKind::Keyword(b"true") => ObjectKind::Bool(true),
            TokenKind::Keyword(b"false") => ObjectKind::Bool(false),
            TokenKind::Keyword(b"null") => ObjectKind::Null,
            TokenKind::ArrayStart => return self.array(span.start, depth),
            TokenKind::DictStart => return self.dict(span.start, depth),
            _ => return Err(syntax(SyntaxKind::UnexpectedToken, span.start)),
        };
        Ok(Object { kind, span })
    }

    /// After an integer, look ahead for `<generation> R`; otherwise it is just an integer.
    fn integer_or_ref(&mut self, n: i64, span: std::ops::Range<usize>) -> Object<'a> {
        let plain = |span| Object {
            kind: ObjectKind::Integer(n),
            span,
        };
        let Ok(num) = u32::try_from(n) else {
            return plain(span);
        };
        let save = self.position();
        let generation = match self.lexer.next_token() {
            Ok(Some(Token {
                kind: TokenKind::Integer(g),
                ..
            })) => u16::try_from(g).ok(),
            _ => None,
        };
        if let Some(generation) = generation
            && let Ok(Some(Token {
                kind: TokenKind::Keyword(b"R"),
                span: r,
            })) = self.lexer.next_token()
        {
            return Object {
                kind: ObjectKind::Ref(ObjRef { num, generation }),
                span: span.start..r.end,
            };
        }
        self.lexer.seek(save);
        plain(span)
    }

    fn enter(&self, depth: u32, offset: usize) -> Result<()> {
        self.limits.check(
            LimitKind::NestingDepth,
            u64::from(depth),
            Some(offset as u64),
        )
    }

    fn array(&mut self, start: usize, depth: u32) -> Result<Object<'a>> {
        self.enter(depth, start)?;
        let mut items = Vec::new();
        loop {
            // The closing bracket is checked first so `]` is not treated as a value.
            match self.peek_required()? {
                None => return Err(syntax(SyntaxKind::UnexpectedEof, start)),
                Some(Token {
                    kind: TokenKind::ArrayEnd,
                    span,
                }) => {
                    self.lexer.seek(span.end);
                    return Ok(Object {
                        kind: ObjectKind::Array(items),
                        span: start..span.end,
                    });
                }
                Some(_) => {}
            }
            self.limits.check(
                LimitKind::ArrayEntries,
                items.len() as u64 + 1,
                Some(self.position() as u64),
            )?;
            items.push(self.value(depth + 1)?);
        }
    }

    fn dict(&mut self, start: usize, depth: u32) -> Result<Object<'a>> {
        self.enter(depth, start)?;
        let mut dict = Dict::default();
        loop {
            let token = self
                .next()?
                .ok_or_else(|| syntax(SyntaxKind::UnexpectedEof, start))?;
            match token.kind {
                TokenKind::DictEnd => {
                    return Ok(Object {
                        kind: ObjectKind::Dict(dict),
                        span: start..token.span.end,
                    });
                }
                TokenKind::Name(key) => {
                    self.limits.check(
                        LimitKind::DictEntries,
                        dict.entries.len() as u64 + 1,
                        Some(token.span.start as u64),
                    )?;
                    if matches!(
                        self.peek(),
                        Some(Token {
                            kind: TokenKind::DictEnd,
                            ..
                        })
                    ) {
                        return Err(syntax(SyntaxKind::DictMissingValue, token.span.start));
                    }
                    let value = self.value(depth + 1)?;
                    dict.entries.push(DictEntry {
                        key,
                        key_span: token.span,
                        value,
                    });
                }
                _ => return Err(syntax(SyntaxKind::DictKeyNotName, token.span.start)),
            }
        }
    }
}

fn syntax(kind: SyntaxKind, offset: usize) -> Error {
    Error::Syntax {
        kind,
        offset: offset as u64,
    }
}

/// Index of the first `endstream` in `haystack`.
fn find_endstream(haystack: &[u8]) -> Option<usize> {
    const NEEDLE: &[u8] = b"endstream";
    let mut from = 0;
    while let Some(rel) = haystack.get(from..)?.iter().position(|&b| b == b'e') {
        let at = from + rel;
        if haystack.get(at..at + NEEDLE.len()) == Some(NEEDLE) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// The end of stream data that stops at `endstream_at`, without the one end-of-line marker
/// that precedes the keyword (§7.3.8.1). Never goes before `data_start`.
fn trim_eol_before(data: &[u8], data_start: usize, endstream_at: usize) -> usize {
    let mut end = endstream_at;
    let byte_before = |end: usize| end.checked_sub(1).and_then(|i| data.get(i)).copied();
    match byte_before(end) {
        Some(b'\n') if end > data_start => {
            end -= 1;
            if byte_before(end) == Some(b'\r') && end > data_start {
                end -= 1;
            }
        }
        Some(b'\r') if end > data_start => end -= 1,
        _ => {}
    }
    end
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use proptest::prelude::*;

    use super::*;

    fn parse(input: &[u8]) -> Object<'_> {
        let limits = Limits::default();
        Parser::new(input, &limits).parse_object().unwrap()
    }

    fn parse_err(input: &[u8]) -> Error {
        let limits = Limits::default();
        Parser::new(input, &limits).parse_object().unwrap_err()
    }

    fn indirect(input: &[u8]) -> IndirectObject<'_> {
        let limits = Limits::default();
        Parser::new(input, &limits).parse_indirect_object().unwrap()
    }

    fn name(s: &'static [u8]) -> ObjectKind<'static> {
        ObjectKind::Name(Cow::Borrowed(s))
    }

    #[test]
    fn scalars() {
        assert_eq!(parse(b"null").kind, ObjectKind::Null);
        assert_eq!(parse(b"true").kind, ObjectKind::Bool(true));
        assert_eq!(parse(b"false").kind, ObjectKind::Bool(false));
        assert_eq!(parse(b"-42").kind, ObjectKind::Integer(-42));
        assert_eq!(parse(b"3.5").kind, ObjectKind::Real(3.5));
        assert_eq!(
            parse(b"(hi)").kind,
            ObjectKind::String(Cow::Borrowed(b"hi"))
        );
        assert_eq!(
            parse(b"<6869>").kind,
            ObjectKind::String(Cow::Owned(b"hi".to_vec()))
        );
        assert_eq!(parse(b"/Type").kind, name(b"Type"));
    }

    #[test]
    fn references() {
        let o = parse(b"12 3 R");
        assert_eq!(o.kind, ObjectKind::Ref(ObjRef::new(12, 3)));
        assert_eq!(o.span, 0..6);
        assert_eq!(ObjRef::new(12, 3).to_string(), "12 3 R");
    }

    #[test]
    fn integers_that_are_not_references_stay_integers() {
        // Plain integers, even when followed by other integers or keywords.
        let o = parse(b"[1 2 3]");
        let ObjectKind::Array(items) = o.kind else {
            panic!()
        };
        assert_eq!(
            items
                .iter()
                .filter_map(Object::as_integer)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        // A negative number or an out-of-range generation cannot be a reference, so the `R`
        // is a stray keyword and the array is malformed.
        for input in [&b"[-1 0 R]"[..], b"[1 65536 R]"] {
            assert!(matches!(
                parse_err(input),
                Error::Syntax {
                    kind: SyntaxKind::UnexpectedToken,
                    ..
                }
            ));
        }
        // `R` must follow; other keywords after two integers leave them as integers.
        let ObjectKind::Array(items) = parse(b"[1 0 true]").kind else {
            panic!()
        };
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn arrays_of_mixed_values_and_references() {
        let o = parse(b"[549 2.75 false (Ralph) /SomeName 1 0 R [] <<>>]");
        let ObjectKind::Array(items) = o.kind else {
            panic!()
        };
        let kinds: Vec<_> = items.iter().map(|i| i.kind.clone()).collect();
        assert_eq!(
            kinds,
            [
                ObjectKind::Integer(549),
                ObjectKind::Real(2.75),
                ObjectKind::Bool(false),
                ObjectKind::String(Cow::Borrowed(b"Ralph")),
                name(b"SomeName"),
                ObjectKind::Ref(ObjRef::new(1, 0)),
                ObjectKind::Array(vec![]),
                ObjectKind::Dict(Dict::default()),
            ]
        );
        assert_eq!(o.span, 0..48);
    }

    #[test]
    fn dictionary_with_nested_values() {
        let input = b"<< /Type /Example /Subtype /DictionaryExample /Version 0.01 /IntegerItem 12 \
                      /StringItem (a string) /Subdictionary << /Item1 0.4 /Item2 true >> >>";
        let o = parse(input);
        let dict = o.as_dict().unwrap();
        assert_eq!(dict.entries.len(), 6);
        assert_eq!(dict.get(b"Type").unwrap().kind, name(b"Example"));
        assert_eq!(dict.get(b"IntegerItem").unwrap().as_integer(), Some(12));
        let sub = dict.get(b"Subdictionary").unwrap().as_dict().unwrap();
        assert_eq!(sub.get(b"Item2").unwrap().kind, ObjectKind::Bool(true));
        assert!(dict.get(b"Missing").is_none());
        assert_eq!(o.span, 0..input.len());
    }

    #[test]
    fn every_object_keeps_its_span() {
        let input = b"<< /A [1 2 0 R] /B (x) >>";
        let o = parse(input);
        let dict = o.as_dict().unwrap();
        let a = &dict.entries[0];
        assert_eq!(&input[a.key_span.clone()], b"/A");
        assert_eq!(&input[a.value.span.clone()], b"[1 2 0 R]");
        let ObjectKind::Array(items) = &a.value.kind else {
            panic!()
        };
        assert_eq!(&input[items[0].span.clone()], b"1");
        assert_eq!(&input[items[1].span.clone()], b"2 0 R");
        assert_eq!(&input[dict.entries[1].value.span.clone()], b"(x)");
    }

    #[test]
    fn duplicate_keys_last_one_wins_and_all_entries_are_kept() {
        let o = parse(b"<< /K 1 /Other 9 /K 2 /K 3 >>");
        let dict = o.as_dict().unwrap();
        assert_eq!(dict.get(b"K").unwrap().as_integer(), Some(3));
        assert_eq!(dict.entries.len(), 4);
        assert!(dict.has_duplicate_keys());
        let o = parse(b"<< /K 1 /Other 9 >>");
        assert!(!o.as_dict().unwrap().has_duplicate_keys());
    }

    #[test]
    fn names_are_compared_after_hash_decoding() {
        let o = parse(b"<< /A#42 1 >>");
        assert_eq!(
            o.as_dict().unwrap().get(b"AB").unwrap().as_integer(),
            Some(1)
        );
    }

    #[test]
    fn syntax_errors_in_direct_objects() {
        let kind = |input: &[u8]| match parse_err(input) {
            Error::Syntax { kind, offset } => (kind, offset),
            other => panic!("{other:?}"),
        };
        assert_eq!(kind(b""), (SyntaxKind::UnexpectedEof, 0));
        assert_eq!(kind(b"]"), (SyntaxKind::UnexpectedToken, 0));
        assert_eq!(kind(b">>"), (SyntaxKind::UnexpectedToken, 0));
        assert_eq!(kind(b"endobj"), (SyntaxKind::UnexpectedToken, 0));
        assert_eq!(kind(b")"), (SyntaxKind::UnexpectedToken, 0));
        assert_eq!(kind(b"[1 2"), (SyntaxKind::UnexpectedEof, 0));
        assert_eq!(kind(b"[1 (a"), (SyntaxKind::UnterminatedString, 3));
        assert_eq!(kind(b"<< /A 1"), (SyntaxKind::UnexpectedEof, 0));
        assert_eq!(kind(b"<< 1 2 >>"), (SyntaxKind::DictKeyNotName, 3));
        assert_eq!(kind(b"<< /A >>"), (SyntaxKind::DictMissingValue, 3));
        assert_eq!(kind(b"[1 }"), (SyntaxKind::UnexpectedToken, 3));
    }

    #[test]
    fn nesting_limit_is_enforced_without_overflowing_the_stack() {
        let limits = Limits::default();
        // Exactly at the limit is fine.
        let depth = limits.max_nesting_depth as usize;
        let mut ok = vec![b'['; depth];
        ok.extend(vec![b']'; depth]);
        assert!(Parser::new(&ok, &limits).parse_object().is_ok());
        // One more level is an error that names the limit.
        let mut deep = vec![b'['; depth + 1];
        deep.extend(vec![b']'; depth + 1]);
        let err = Parser::new(&deep, &limits).parse_object().unwrap_err();
        assert!(
            matches!(err, Error::LimitExceeded { limit: LimitKind::NestingDepth, value, .. } if value == depth as u64 + 1),
            "{err:?}"
        );
        // Mixed arrays and dictionaries count together.
        let mixed = b"<< /A [ << /B [ ".repeat(40);
        assert!(matches!(
            Parser::new(&mixed, &limits).parse_object().unwrap_err(),
            Error::LimitExceeded {
                limit: LimitKind::NestingDepth,
                ..
            }
        ));
        // A million opening brackets terminates quickly with the same error.
        let bomb = vec![b'['; 1_000_000];
        assert!(matches!(
            Parser::new(&bomb, &limits).parse_object().unwrap_err(),
            Error::LimitExceeded {
                limit: LimitKind::NestingDepth,
                ..
            }
        ));
    }

    #[test]
    fn array_and_dictionary_entry_limits() {
        let limits = Limits {
            max_array_entries: 3,
            max_dict_entries: 2,
            ..Limits::default()
        };
        assert!(Parser::new(b"[1 2 3]", &limits).parse_object().is_ok());
        assert!(matches!(
            Parser::new(b"[1 2 3 4]", &limits)
                .parse_object()
                .unwrap_err(),
            Error::LimitExceeded {
                limit: LimitKind::ArrayEntries,
                max: 3,
                value: 4,
                ..
            }
        ));
        assert!(
            Parser::new(b"<< /A 1 /B 2 >>", &limits)
                .parse_object()
                .is_ok()
        );
        assert!(matches!(
            Parser::new(b"<< /A 1 /B 2 /C 3 >>", &limits)
                .parse_object()
                .unwrap_err(),
            Error::LimitExceeded {
                limit: LimitKind::DictEntries,
                max: 2,
                value: 3,
                ..
            }
        ));
    }

    #[test]
    fn simple_indirect_object() {
        let input = b"7 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
        let o = indirect(input);
        assert_eq!(o.id, ObjRef::new(7, 0));
        assert_eq!(o.recoveries, Vec::new());
        assert_eq!(
            &input[o.span.clone()],
            b"7 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj"
        );
        assert_eq!(
            o.object.as_dict().unwrap().get(b"Pages").unwrap().kind,
            ObjectKind::Ref(ObjRef::new(2, 0))
        );
    }

    #[test]
    fn indirect_objects_of_every_simple_type_and_glued_tokens() {
        assert_eq!(
            indirect(b"1 0 obj 42 endobj").object.kind,
            ObjectKind::Integer(42)
        );
        assert_eq!(indirect(b"1 5 obj null endobj").id, ObjRef::new(1, 5));
        assert_eq!(
            indirect(b"1 0 obj(x)endobj").object.kind,
            ObjectKind::String(Cow::Borrowed(b"x"))
        );
        assert_eq!(indirect(b"1 0 obj[1]endobj").object.span, 7..10);
        assert_eq!(
            indirect(b"1 0 obj 5 0 R endobj").object.kind,
            ObjectKind::Ref(ObjRef::new(5, 0))
        );
    }

    #[test]
    fn malformed_indirect_headers() {
        let limits = Limits::default();
        for input in [
            &b""[..],
            b"obj",
            b"1 obj",
            b"1 0 foo",
            b"-1 0 obj null endobj",
            b"1 70000 obj null endobj",
            b"1.5 0 obj null endobj",
            b"/A 0 obj null endobj",
        ] {
            let err = Parser::new(input, &limits)
                .parse_indirect_object()
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    Error::Syntax {
                        kind: SyntaxKind::MalformedObjectHeader,
                        ..
                    }
                ),
                "{}: {err:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn missing_endobj_is_recovered_and_the_next_object_is_left_unread() {
        let input = b"1 0 obj << /A 1 >> 2 0 obj null endobj";
        let limits = Limits::default();
        let mut parser = Parser::new(input, &limits);
        let first = parser.parse_indirect_object().unwrap();
        assert_eq!(first.recoveries, [Recovery::MissingEndobj]);
        assert_eq!(&input[first.span.clone()], b"1 0 obj << /A 1 >>");
        assert_eq!(parser.position(), first.span.end);
        let second = parser.parse_indirect_object().unwrap();
        assert_eq!(second.id, ObjRef::new(2, 0));
        assert_eq!(second.recoveries, Vec::new());
    }

    #[test]
    fn missing_endobj_at_end_of_data() {
        let o = indirect(b"1 0 obj 5");
        assert_eq!(o.recoveries, [Recovery::MissingEndobj]);
        assert_eq!(o.object.kind, ObjectKind::Integer(5));
    }

    #[test]
    fn unparsable_garbage_after_an_object_counts_as_missing_endobj() {
        let o = indirect(b"1 0 obj 5 (unterminated");
        assert_eq!(o.recoveries, [Recovery::MissingEndobj]);
    }

    fn stream_object(length: &str, body: &[u8], tail: &[u8]) -> Vec<u8> {
        let mut v = format!("4 0 obj\n<< /Length {length} >>\nstream\n").into_bytes();
        v.extend_from_slice(body);
        v.extend_from_slice(tail);
        v
    }

    fn stream_of<'a>(o: &'a IndirectObject<'_>) -> &'a Stream<'a> {
        match &o.object.kind {
            ObjectKind::Stream(s) => s,
            other => panic!("not a stream: {other:?}"),
        }
    }

    #[test]
    fn stream_with_correct_length() {
        let input = stream_object("5", b"HELLO", b"\nendstream\nendobj\n");
        let o = indirect(&input);
        assert_eq!(o.recoveries, Vec::new());
        let s = stream_of(&o);
        assert_eq!(&input[s.data.clone()], b"HELLO");
        assert_eq!(s.dict.get(b"Length").unwrap().as_integer(), Some(5));
        assert_eq!(
            &input[o.span.clone()][input[o.span.clone()].len() - 6..],
            b"endobj"
        );
        // The object span covers the whole stream.
        assert_eq!(o.object.span.end, o.span.end - b"\nendobj".len());
    }

    #[test]
    fn stream_data_may_contain_endstream_text_when_length_is_right() {
        let input = stream_object("12", b"endstream xx", b"\nendstream\nendobj");
        let o = indirect(&input);
        assert_eq!(o.recoveries, Vec::new());
        assert_eq!(&input[stream_of(&o).data.clone()], b"endstream xx");
    }

    #[test]
    fn stream_eol_variants_and_binary_data() {
        let body = [0u8, 255, 13, 10, 7];
        for (eol, standard) in [(&b"\r\n"[..], true), (b"\n", true), (b"\r", false)] {
            let mut input = b"4 0 obj << /Length 5 >> stream".to_vec();
            input.extend_from_slice(eol);
            input.extend_from_slice(&body);
            input.extend_from_slice(b"\nendstream endobj");
            let o = indirect(&input);
            assert_eq!(&input[stream_of(&o).data.clone()], body);
            assert_eq!(o.recoveries.is_empty(), standard, "{eol:?}");
            if !standard {
                assert_eq!(o.recoveries, [Recovery::StreamEolNonStandard]);
            }
        }
    }

    #[test]
    fn wrong_stream_length_is_recovered_by_searching_for_endstream() {
        // Too long, too short, negative and huge declared lengths.
        for declared in ["100", "2", "-3", "99999999999999999999"] {
            let input = stream_object(declared, b"HELLO", b"\nendstream\nendobj\n");
            let o = indirect(&input);
            assert_eq!(&input[stream_of(&o).data.clone()], b"HELLO", "{declared}");
            assert_eq!(o.recoveries.len(), 1, "{declared}");
            assert!(
                matches!(o.recoveries[0], Recovery::StreamLengthWrong { .. })
                    || (declared.len() > 18 && o.recoveries[0] == Recovery::StreamLengthMissing),
                "{declared}: {:?}",
                o.recoveries
            );
        }
        let input = stream_object("100", b"HELLO", b"\nendstream\nendobj\n");
        assert_eq!(
            indirect(&input).recoveries,
            [Recovery::StreamLengthWrong { declared: 100 }]
        );
    }

    #[test]
    fn missing_stream_length_is_recovered() {
        let input = b"4 0 obj << /Filter /A >> stream\nHELLO\r\nendstream endobj".to_vec();
        let o = indirect(&input);
        assert_eq!(&input[stream_of(&o).data.clone()], b"HELLO");
        assert_eq!(o.recoveries, [Recovery::StreamLengthMissing]);
        // A non-integer length counts as missing.
        let input = b"4 0 obj << /Length (x) >> stream\nHI\nendstream endobj".to_vec();
        assert_eq!(indirect(&input).recoveries, [Recovery::StreamLengthMissing]);
    }

    #[test]
    fn indirect_length_uses_the_resolver_or_falls_back() {
        let input = b"4 0 obj << /Length 9 0 R >> stream\nHELLO\nendstream endobj";
        let limits = Limits::default();
        let resolve = |r: ObjRef| (r == ObjRef::new(9, 0)).then_some(5);
        let o = Parser::new(input, &limits)
            .with_length_resolver(&resolve)
            .parse_indirect_object()
            .unwrap();
        assert_eq!(o.recoveries, Vec::new());
        assert_eq!(&input[stream_of(&o).data.clone()], b"HELLO");

        // No resolver: found by searching, and flagged.
        let o = Parser::new(input, &limits).parse_indirect_object().unwrap();
        assert_eq!(o.recoveries, [Recovery::StreamLengthUnresolved]);
        assert_eq!(&input[stream_of(&o).data.clone()], b"HELLO");

        // Resolver gives a wrong value: flagged as wrong.
        let wrong = |_: ObjRef| Some(1);
        let o = Parser::new(input, &limits)
            .with_length_resolver(&wrong)
            .parse_indirect_object()
            .unwrap();
        assert_eq!(o.recoveries, [Recovery::StreamLengthWrong { declared: 1 }]);
    }

    #[test]
    fn empty_stream() {
        let input = b"4 0 obj << /Length 0 >> stream\nendstream endobj".to_vec();
        let o = indirect(&input);
        assert_eq!(o.recoveries, Vec::new());
        assert!(stream_of(&o).data.is_empty());
        let input = b"4 0 obj << >> stream\r\nendstream endobj".to_vec();
        assert!(stream_of(&indirect(&input)).data.is_empty());
    }

    #[test]
    fn missing_endstream_is_an_error() {
        let limits = Limits::default();
        let err = Parser::new(b"4 0 obj << /Length 3 >> stream\nabc endobj", &limits)
            .parse_indirect_object()
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::Syntax {
                    kind: SyntaxKind::MissingEndstream,
                    offset: 24
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn stream_recovery_search_is_bounded() {
        let limits = Limits {
            max_decoded_stream_bytes: 10,
            ..Limits::default()
        };
        let mut input = b"4 0 obj << >> stream\n".to_vec();
        input.extend(vec![b'x'; 100]);
        input.extend(b"\nendstream endobj");
        assert!(matches!(
            Parser::new(&input, &limits)
                .parse_indirect_object()
                .unwrap_err(),
            Error::Syntax {
                kind: SyntaxKind::MissingEndstream,
                ..
            }
        ));
    }

    #[test]
    fn stream_keyword_after_a_non_dictionary_is_an_error() {
        let limits = Limits::default();
        let err = Parser::new(b"1 0 obj [1] stream\nx\nendstream endobj", &limits)
            .parse_indirect_object()
            .unwrap_err();
        assert!(
            matches!(
                err,
                Error::Syntax {
                    kind: SyntaxKind::UnexpectedToken,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn direct_parse_never_makes_streams() {
        let o = parse(b"<< /Length 1 >> stream");
        assert!(matches!(o.kind, ObjectKind::Dict(_)));
    }

    #[test]
    fn parser_can_seek_to_an_offset_like_an_xref_lookup() {
        let input = b"%PDF-1.7\n1 0 obj 11 endobj\n2 0 obj 22 endobj\n";
        let limits = Limits::default();
        let mut parser = Parser::at(input, 27, &limits);
        let o = parser.parse_indirect_object().unwrap();
        assert_eq!((o.id, o.object.as_integer()), (ObjRef::new(2, 0), Some(22)));
        parser.seek(9);
        assert_eq!(
            parser.parse_indirect_object().unwrap().id,
            ObjRef::new(1, 0)
        );
    }

    #[test]
    fn trim_eol_never_goes_before_the_data_start() {
        assert_eq!(trim_eol_before(b"\nendstream", 1, 1), 1);
        assert_eq!(trim_eol_before(b"ab\r\nendstream", 0, 4), 2);
        assert_eq!(trim_eol_before(b"ab\rendstream", 0, 3), 2);
        assert_eq!(trim_eol_before(b"ab\nendstream", 0, 3), 2);
        assert_eq!(trim_eol_before(b"abendstream", 0, 2), 2);
        assert_eq!(trim_eol_before(b"", 0, 0), 0);
    }

    fn fragments() -> impl Strategy<Value = &'static str> {
        prop::sample::select(vec![
            "[",
            "]",
            "<<",
            ">>",
            "/A",
            "/Length",
            "1",
            "0",
            "5",
            "R",
            "obj",
            "endobj",
            "stream\n",
            "stream\r\n",
            "endstream",
            "(x)",
            "(",
            "<41>",
            "<",
            "true",
            "null",
            "-3",
            "2.5",
            "%c\n",
            "xref",
            "9999999999999999999",
        ])
    }

    proptest! {
        /// Arbitrary bytes never panic, and the parser always ends.
        #[test]
        fn arbitrary_bytes_never_panic(input in proptest::collection::vec(any::<u8>(), 0..512)) {
            let limits = Limits::default();
            let _ = Parser::new(&input, &limits).parse_object();
            let _ = Parser::new(&input, &limits).parse_indirect_object();
        }

        /// Token soup reaches deeper parser states than random bytes do.
        #[test]
        fn token_soup_never_panics_and_spans_stay_in_bounds(
            parts in proptest::collection::vec(fragments(), 0..40),
            prefix in prop::sample::select(vec!["", "1 0 obj ", "3 1 obj << /Length 3 >> "]),
        ) {
            let text = format!("{prefix}{}", parts.join(" "));
            let limits = Limits::default();
            if let Ok(o) = Parser::new(text.as_bytes(), &limits).parse_indirect_object() {
                prop_assert!(o.span.start <= o.span.end && o.span.end <= text.len());
                prop_assert!(o.object.span.end <= text.len());
                if let ObjectKind::Stream(s) = &o.object.kind {
                    prop_assert!(s.data.start <= s.data.end && s.data.end <= text.len());
                }
            }
            let mut parser = Parser::new(text.as_bytes(), &limits);
            if let Ok(o) = parser.parse_object() {
                prop_assert!(o.span.end <= text.len());
                prop_assert_eq!(parser.position(), o.span.end);
            }
        }

        /// A parse never moves backwards, and a successful direct parse consumes something.
        #[test]
        fn successful_parses_make_progress(parts in proptest::collection::vec(fragments(), 1..20)) {
            let text = parts.join(" ");
            let limits = Limits::default();
            let mut parser = Parser::new(text.as_bytes(), &limits);
            if parser.parse_object().is_ok() {
                prop_assert!(parser.position() > 0);
            }
        }
    }
}
