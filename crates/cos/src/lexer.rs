//! Lexer for PDF syntax (ISO 32000-2:2020 §7.2).
//!
//! The lexer turns bytes into [`Token`]s, each with the byte span it came from, so later layers
//! can splice and re-serialise byte ranges without re-parsing. Whitespace and comments are
//! tracked as tokens too: [`Lexer::next_raw`] returns them, [`Lexer::next_token`] skips them.
//!
//! Design rules:
//! - Never panics on any input and always makes progress: every call either consumes at least one
//!   byte or returns `Ok(None)` at the end. After an `Err` the position is past the offending
//!   construct, so a recovery scan can keep going.
//! - Tokens borrow from the input where possible (names and strings without escapes).
//! - Strings and names obey [`Limits`] (`max_string_bytes`, `max_name_bytes`).
//!
//! Tolerance (what real files do that the spec forbids), all documented here and tested:
//! - Several leading signs on a number (`--5`) mean one sign; a `-` anywhere makes it negative.
//! - An integer that does not fit `i64` becomes a [`TokenKind::Real`]; a real that overflows
//!   `f64` is clamped to `f64::MAX`.
//! - A word that starts like a number but is not one (`1.2.3`, `12-34`, a lone `-`) is a
//!   [`TokenKind::Keyword`] and the caller decides what to do.
//! - Non-hex characters inside a hex string are ignored; an odd final digit is padded with `0`
//!   (§7.3.4.3).
//! - `#` in a name that is not followed by two hex digits stays a literal `#`.
//! - A stray `)` or a lone `>` is returned as [`TokenKind::Stray`].

use std::borrow::Cow;
use std::ops::Range;

use crate::error::{Error, Result, SyntaxKind};
use crate::limits::{LimitKind, Limits};

/// White-space characters (§7.2.3, Table 1).
#[must_use]
pub fn is_whitespace(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

/// Delimiter characters (§7.2.3, Table 2).
#[must_use]
pub fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Regular characters: neither white space nor delimiters.
#[must_use]
pub fn is_regular(b: u8) -> bool {
    !is_whitespace(b) && !is_delimiter(b)
}

/// One lexical element and where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Token<'a> {
    /// What it is.
    pub kind: TokenKind<'a>,
    /// Byte range in the lexer's input, including delimiters (`(`, `/`, `<`, `%` ...).
    pub span: Range<usize>,
}

/// The kinds of token.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum TokenKind<'a> {
    /// An integer (§7.3.3).
    Integer(i64),
    /// A real number (§7.3.3), also an integer that does not fit `i64`.
    Real(f64),
    /// A literal string `( ... )` with escapes resolved (§7.3.4.2).
    LiteralString(Cow<'a, [u8]>),
    /// A hexadecimal string `< ... >`, decoded to bytes (§7.3.4.3).
    HexString(Cow<'a, [u8]>),
    /// A name `/Name`, without the slash and with `#xx` escapes resolved (§7.3.5).
    Name(Cow<'a, [u8]>),
    /// `[`
    ArrayStart,
    /// `]`
    ArrayEnd,
    /// `<<`
    DictStart,
    /// `>>`
    DictEnd,
    /// `{` (PostScript calculator functions, §7.10.5)
    BraceStart,
    /// `}`
    BraceEnd,
    /// A run of regular characters that is not a number: `obj`, `R`, `true`, `stream`, operators.
    Keyword(&'a [u8]),
    /// A delimiter that cannot start a token here: a `)` or a single `>`.
    Stray(u8),
    /// A comment, without the leading `%` and the end-of-line marker (§7.2.4).
    /// Only produced by [`Lexer::next_raw`].
    Comment(&'a [u8]),
    /// A run of white space. Only produced by [`Lexer::next_raw`].
    Whitespace,
}

/// The end-of-line marker found after the `stream` keyword (§7.3.8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEol {
    /// CR LF, as the spec requires (together with a bare LF).
    CrLf,
    /// LF.
    Lf,
    /// A lone CR. Not allowed by the spec; many writers produce it.
    Cr,
    /// No end-of-line marker: the data starts right after the keyword. Not allowed by the spec.
    Missing,
}

/// Where stream data starts, from [`Lexer::skip_stream_eol`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamStart {
    /// Offset of the first data byte.
    pub data_start: usize,
    /// Which marker preceded it. Anything but `CrLf`/`Lf` is a spec violation to report.
    pub eol: StreamEol,
}

/// A lexer over an in-memory slice. Offsets are positions in that slice.
#[derive(Debug, Clone)]
pub struct Lexer<'a, 'l> {
    data: &'a [u8],
    pos: usize,
    limits: &'l Limits,
}

impl<'a, 'l> Lexer<'a, 'l> {
    /// A lexer at the start of `data`.
    #[must_use]
    pub fn new(data: &'a [u8], limits: &'l Limits) -> Self {
        Self {
            data,
            pos: 0,
            limits,
        }
    }

    /// A lexer starting at `pos` (clamped to the end of `data`).
    #[must_use]
    pub fn at(data: &'a [u8], pos: usize, limits: &'l Limits) -> Self {
        Self {
            data,
            pos: pos.min(data.len()),
            limits,
        }
    }

    /// The whole input.
    #[must_use]
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Current position.
    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Moves to `pos` (clamped to the end of the input).
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.data.len());
    }

    /// Whether everything has been consumed.
    #[must_use]
    pub fn is_eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    /// The next token, skipping white space and comments. `Ok(None)` at the end of the input.
    ///
    /// # Errors
    /// [`Error::Syntax`] for an unterminated string, [`Error::LimitExceeded`] for a name or
    /// string over its limit. The position is past the offending construct afterwards.
    pub fn next_token(&mut self) -> Result<Option<Token<'a>>> {
        loop {
            match self.next_raw()? {
                Some(Token {
                    kind: TokenKind::Whitespace | TokenKind::Comment(_),
                    ..
                }) => {}
                other => return Ok(other),
            }
        }
    }

    /// The next token including white space and comments, so the spans of consecutive tokens
    /// tile the input without gaps.
    ///
    /// # Errors
    /// Same as [`next_token`](Self::next_token).
    pub fn next_raw(&mut self) -> Result<Option<Token<'a>>> {
        let start = self.pos;
        let Some(&first) = self.data.get(start) else {
            return Ok(None);
        };
        let kind = match first {
            b if is_whitespace(b) => {
                self.pos = self.skip_while(start, is_whitespace);
                TokenKind::Whitespace
            }
            b'%' => {
                let end = self.skip_while(start + 1, |b| b != b'\r' && b != b'\n');
                self.pos = end;
                TokenKind::Comment(self.data.get(start + 1..end).unwrap_or_default())
            }
            b'/' => self.name(start)?,
            b'(' => self.literal_string(start)?,
            b'<' => {
                if self.data.get(start + 1) == Some(&b'<') {
                    self.pos = start + 2;
                    TokenKind::DictStart
                } else {
                    self.hex_string(start)?
                }
            }
            b'>' => {
                if self.data.get(start + 1) == Some(&b'>') {
                    self.pos = start + 2;
                    TokenKind::DictEnd
                } else {
                    self.pos = start + 1;
                    TokenKind::Stray(b'>')
                }
            }
            b'[' => self.single(TokenKind::ArrayStart),
            b']' => self.single(TokenKind::ArrayEnd),
            b'{' => self.single(TokenKind::BraceStart),
            b'}' => self.single(TokenKind::BraceEnd),
            b')' => self.single(TokenKind::Stray(b')')),
            _ => {
                let end = self.skip_while(start, is_regular);
                self.pos = end;
                classify_word(self.data.get(start..end).unwrap_or_default())
            }
        };
        Ok(Some(Token {
            kind,
            span: start..self.pos,
        }))
    }

    /// After a [`Keyword`](TokenKind::Keyword) `stream`, skips the end-of-line marker and
    /// returns where the data starts (§7.3.8.1).
    ///
    /// Spaces and tabs between the keyword and the marker are tolerated, but only when a marker
    /// follows them, so data that begins with a space is never eaten.
    pub fn skip_stream_eol(&mut self) -> StreamStart {
        let after_blanks = self.skip_while(self.pos, |b| b == b' ' || b == b'\t');
        let (end, eol) = match (self.data.get(after_blanks), self.data.get(after_blanks + 1)) {
            (Some(b'\r'), Some(b'\n')) => (after_blanks + 2, StreamEol::CrLf),
            (Some(b'\n'), _) => (after_blanks + 1, StreamEol::Lf),
            (Some(b'\r'), _) => (after_blanks + 1, StreamEol::Cr),
            _ => (self.pos, StreamEol::Missing),
        };
        self.pos = end;
        StreamStart {
            data_start: end,
            eol,
        }
    }

    fn single(&mut self, kind: TokenKind<'a>) -> TokenKind<'a> {
        self.pos += 1;
        kind
    }

    /// First index at or after `from` where `keep_going` is false (or the end of the input).
    fn skip_while(&self, from: usize, keep_going: impl Fn(u8) -> bool) -> usize {
        let rest = self.data.get(from..).unwrap_or_default();
        from + rest.iter().take_while(|&&b| keep_going(b)).count()
    }

    fn name(&mut self, start: usize) -> Result<TokenKind<'a>> {
        let body = start + 1;
        let end = self.skip_while(body, is_regular);
        self.pos = end;
        let raw = self.data.get(body..end).unwrap_or_default();
        self.limits
            .check(LimitKind::NameBytes, raw.len() as u64, Some(start as u64))?;
        if !raw.contains(&b'#') {
            return Ok(TokenKind::Name(Cow::Borrowed(raw)));
        }
        let mut out = Vec::with_capacity(raw.len());
        let mut i = 0;
        while let Some(&b) = raw.get(i) {
            let escaped = (b == b'#')
                .then(|| Some((hex_value(*raw.get(i + 1)?)?, hex_value(*raw.get(i + 2)?)?)))
                .flatten();
            if let Some((hi, lo)) = escaped {
                out.push(hi << 4 | lo);
                i += 3;
            } else {
                out.push(b);
                i += 1;
            }
        }
        Ok(TokenKind::Name(Cow::Owned(out)))
    }

    fn literal_string(&mut self, start: usize) -> Result<TokenKind<'a>> {
        let body = start + 1;
        // First pass: find the matching parenthesis and whether any decoding is needed.
        let mut i = body;
        let mut depth = 1usize;
        let mut needs_decode = false;
        loop {
            match self.data.get(i) {
                None => {
                    self.pos = self.data.len();
                    return Err(Error::Syntax {
                        kind: SyntaxKind::UnterminatedString,
                        offset: start as u64,
                    });
                }
                Some(b'\\') => {
                    needs_decode = true;
                    i += 2;
                }
                Some(b'(') => {
                    depth += 1;
                    i += 1;
                }
                Some(b')') => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    i += 1;
                }
                Some(b'\r') => {
                    needs_decode = true;
                    i += 1;
                }
                Some(_) => i += 1,
            }
        }
        self.pos = i + 1;
        let raw = self.data.get(body..i).unwrap_or_default();
        if !needs_decode {
            self.limits
                .check(LimitKind::StringBytes, raw.len() as u64, Some(start as u64))?;
            return Ok(TokenKind::LiteralString(Cow::Borrowed(raw)));
        }
        self.decode_literal(raw, start)
            .map(|v| TokenKind::LiteralString(Cow::Owned(v)))
    }

    /// Resolves escapes and end-of-line markers of §7.3.4.2.
    fn decode_literal(&self, raw: &[u8], start: usize) -> Result<Vec<u8>> {
        let max = self.limits.max_string_bytes;
        let mut out = Vec::with_capacity(raw.len().min(usize::try_from(max).unwrap_or(usize::MAX)));
        let mut i = 0;
        while let Some(&b) = raw.get(i) {
            i += 1;
            match b {
                b'\\' => match raw.get(i).copied() {
                    None => {}
                    Some(c @ (b'n' | b'r' | b't' | b'b' | b'f')) => {
                        out.push(match c {
                            b'n' => b'\n',
                            b'r' => b'\r',
                            b't' => b'\t',
                            b'b' => 8,
                            _ => 12,
                        });
                        i += 1;
                    }
                    // Line continuation: a backslash before an end-of-line marker is dropped.
                    Some(b'\r') => {
                        i += 1;
                        if raw.get(i) == Some(&b'\n') {
                            i += 1;
                        }
                    }
                    Some(b'\n') => i += 1,
                    Some(d @ b'0'..=b'7') => {
                        // One to three octal digits; overflow beyond a byte is ignored.
                        let mut value = u32::from(d - b'0');
                        i += 1;
                        for _ in 0..2 {
                            match raw.get(i) {
                                Some(&o @ b'0'..=b'7') => {
                                    value = value * 8 + u32::from(o - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(u8::try_from(value & 0xFF).unwrap_or(0));
                    }
                    // `\(`, `\)`, `\\` and any other escaped byte stand for themselves.
                    Some(other) => {
                        out.push(other);
                        i += 1;
                    }
                },
                // An unescaped CR or CRLF counts as a single LF.
                b'\r' => {
                    out.push(b'\n');
                    if raw.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                }
                other => out.push(other),
            }
            self.limits
                .check(LimitKind::StringBytes, out.len() as u64, Some(start as u64))?;
        }
        Ok(out)
    }

    fn hex_string(&mut self, start: usize) -> Result<TokenKind<'a>> {
        let body = start + 1;
        let Some(len) = self
            .data
            .get(body..)
            .and_then(|rest| rest.iter().position(|&b| b == b'>'))
        else {
            self.pos = self.data.len();
            return Err(Error::Syntax {
                kind: SyntaxKind::UnterminatedHexString,
                offset: start as u64,
            });
        };
        let end = body + len;
        self.pos = end + 1;
        let raw = self.data.get(body..end).unwrap_or_default();

        let mut out = Vec::with_capacity(raw.len() / 2 + 1);
        let mut pending: Option<u8> = None;
        for nibble in raw.iter().filter_map(|&b| hex_value(b)) {
            match pending.take() {
                Some(hi) => out.push(hi << 4 | nibble),
                None => pending = Some(nibble),
            }
        }
        if let Some(hi) = pending {
            out.push(hi << 4);
        }
        self.limits
            .check(LimitKind::StringBytes, out.len() as u64, Some(start as u64))?;
        Ok(TokenKind::HexString(Cow::Owned(out)))
    }
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decides whether a run of regular characters is a number or a keyword.
fn classify_word(word: &[u8]) -> TokenKind<'_> {
    let signs = word.iter().take_while(|&&b| b == b'+' || b == b'-').count();
    let negative = word.get(..signs).unwrap_or_default().contains(&b'-');
    let body = word.get(signs..).unwrap_or_default();

    let (mut dots, mut digits) = (0usize, 0usize);
    for &b in body {
        match b {
            b'.' => dots += 1,
            b'0'..=b'9' => digits += 1,
            // Anything else (`e`, `-` in the middle, letters) is not part of a PDF number.
            _ => return TokenKind::Keyword(word),
        }
    }
    if digits == 0 || dots > 1 {
        return TokenKind::Keyword(word);
    }

    if dots == 0 {
        let magnitude = body.iter().try_fold(0u64, |acc, &d| {
            acc.checked_mul(10)
                .and_then(|m| m.checked_add(u64::from(d - b'0')))
        });
        if let Some(magnitude) = magnitude {
            if let Ok(value) = i64::try_from(magnitude) {
                return TokenKind::Integer(if negative { -value } else { value });
            }
            if negative && magnitude == i64::MIN.unsigned_abs() {
                return TokenKind::Integer(i64::MIN);
            }
        }
    }

    let value = std::str::from_utf8(body)
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .map(|v| if v.is_finite() { v } else { f64::MAX });
    match value {
        Some(v) => TokenKind::Real(if negative { -v } else { v }),
        None => TokenKind::Keyword(word),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn lex_all(input: &[u8]) -> Vec<TokenKind<'_>> {
        lex_with(input, &Limits::default())
    }

    fn lex_with<'a>(input: &'a [u8], limits: &Limits) -> Vec<TokenKind<'a>> {
        let mut lexer = Lexer::new(input, limits);
        let mut out = Vec::new();
        while let Some(token) = lexer.next_token().unwrap() {
            out.push(token.kind);
        }
        out
    }

    fn one(input: &[u8]) -> TokenKind<'_> {
        let mut tokens = lex_all(input);
        assert_eq!(
            tokens.len(),
            1,
            "{:?} lexed to {tokens:?}",
            String::from_utf8_lossy(input)
        );
        tokens.remove(0)
    }

    fn string(bytes: &[u8]) -> TokenKind<'static> {
        TokenKind::LiteralString(Cow::Owned(bytes.to_vec()))
    }

    fn first_error(input: &[u8]) -> Error {
        let limits = Limits::default();
        let mut lexer = Lexer::new(input, &limits);
        loop {
            match lexer.next_token() {
                Err(e) => return e,
                Ok(None) => panic!("no error in {:?}", String::from_utf8_lossy(input)),
                Ok(Some(_)) => {}
            }
        }
    }

    #[test]
    fn integers() {
        assert_eq!(one(b"0"), TokenKind::Integer(0));
        assert_eq!(one(b"123"), TokenKind::Integer(123));
        assert_eq!(one(b"+17"), TokenKind::Integer(17));
        assert_eq!(one(b"-98"), TokenKind::Integer(-98));
        assert_eq!(one(b"007"), TokenKind::Integer(7));
        assert_eq!(one(b"-0"), TokenKind::Integer(0));
        assert_eq!(one(b"9223372036854775807"), TokenKind::Integer(i64::MAX));
        assert_eq!(one(b"-9223372036854775808"), TokenKind::Integer(i64::MIN));
    }

    #[test]
    fn reals() {
        assert_eq!(one(b"34.5"), TokenKind::Real(34.5));
        assert_eq!(one(b"-3.62"), TokenKind::Real(-3.62));
        assert_eq!(one(b"+123.6"), TokenKind::Real(123.6));
        assert_eq!(one(b"4."), TokenKind::Real(4.0));
        assert_eq!(one(b"-.002"), TokenKind::Real(-0.002));
        assert_eq!(one(b"0.0"), TokenKind::Real(0.0));
    }

    #[test]
    fn repeated_signs_are_tolerated() {
        assert_eq!(one(b"--5"), TokenKind::Integer(-5));
        assert_eq!(one(b"+-5"), TokenKind::Integer(-5));
        assert_eq!(one(b"++5"), TokenKind::Integer(5));
        assert_eq!(one(b"--1.5"), TokenKind::Real(-1.5));
    }

    #[test]
    fn huge_numbers_do_not_panic_or_wrap() {
        // One past i64::MAX and a 400-digit integer become reals.
        assert_eq!(
            one(b"9223372036854775808"),
            TokenKind::Real(9_223_372_036_854_775_808.0)
        );
        assert_eq!(
            one(b"-9223372036854775809"),
            TokenKind::Real(-9_223_372_036_854_775_809.0)
        );
        let huge = vec![b'9'; 400];
        assert_eq!(one(&huge), TokenKind::Real(f64::MAX));
        let mut negative = vec![b'-'];
        negative.extend(&huge);
        assert_eq!(one(&negative), TokenKind::Real(-f64::MAX));
        let mut real = vec![b'1'; 400];
        real.extend(b".5");
        assert_eq!(one(&real), TokenKind::Real(f64::MAX));
        // Many digits after the point are fine too.
        let mut tiny = b"0.".to_vec();
        tiny.extend(vec![b'0'; 500]);
        tiny.push(b'1');
        assert_eq!(one(&tiny), TokenKind::Real(0.0));
    }

    #[test]
    fn malformed_numbers_become_keywords() {
        for word in [
            &b"1.2.3"[..],
            b"12-34",
            b"-",
            b"+",
            b".",
            b"-.",
            b"1e5",
            b"0x10",
            b"5a",
        ] {
            assert_eq!(
                one(word),
                TokenKind::Keyword(word),
                "{}",
                String::from_utf8_lossy(word)
            );
        }
    }

    #[test]
    fn keywords() {
        assert_eq!(one(b"obj"), TokenKind::Keyword(b"obj"));
        assert_eq!(one(b"endstream"), TokenKind::Keyword(b"endstream"));
        assert_eq!(one(b"T*"), TokenKind::Keyword(b"T*"));
        assert_eq!(one(b"'"), TokenKind::Keyword(b"'"));
        assert_eq!(
            lex_all(b"1 0 R true false null"),
            [
                TokenKind::Integer(1),
                TokenKind::Integer(0),
                TokenKind::Keyword(b"R"),
                TokenKind::Keyword(b"true"),
                TokenKind::Keyword(b"false"),
                TokenKind::Keyword(b"null"),
            ]
        );
    }

    #[test]
    fn simple_literal_strings_borrow() {
        let input = b"(This is a string)";
        let TokenKind::LiteralString(s) = one(input) else {
            panic!()
        };
        assert_eq!(&*s, b"This is a string");
        assert!(matches!(s, Cow::Borrowed(_)));
        assert_eq!(one(b"()"), string(b""));
    }

    #[test]
    fn literal_strings_balance_parentheses() {
        assert_eq!(
            one(b"(Strings may contain balanced parentheses ( ) and so on.)"),
            string(b"Strings may contain balanced parentheses ( ) and so on.")
        );
        assert_eq!(one(b"((()))"), string(b"(())"));
        assert_eq!(one(b"(0.0)"), string(b"0.0"));
    }

    #[test]
    fn literal_string_escapes() {
        assert_eq!(one(br"(a\nb\rc\td\be\ff)"), string(b"a\nb\rc\td\x08e\x0cf"));
        assert_eq!(one(br"(\(\)\\)"), string(b"()\\"));
        // Unknown escapes drop the backslash.
        assert_eq!(one(br"(\q\ )"), string(b"q "));
    }

    #[test]
    fn literal_string_octal_escapes() {
        assert_eq!(one(br"(\053)"), string(b"+"));
        assert_eq!(one(br"(\53)"), string(b"+"));
        assert_eq!(one(br"(\5)"), string(&[5]));
        assert_eq!(one(br"(\0053)"), string(&[5, b'3']));
        // Digits are consumed greedily up to three; 8 and 9 are not octal.
        assert_eq!(one(br"(\1234)"), string(&[0o123, b'4']));
        assert_eq!(one(br"(\18)"), string(&[1, b'8']));
        // High-order overflow is ignored: \777 is 511, which keeps the low byte.
        assert_eq!(one(br"(\777)"), string(&[0xFF]));
    }

    #[test]
    fn literal_string_line_continuation_and_eol_normalisation() {
        assert_eq!(
            one(b"(These \\\ntwo\\\r\nstrings\\\rare the same)"),
            string(b"These twostringsare the same")
        );
        // A bare CR or CRLF inside the string reads as LF.
        assert_eq!(one(b"(a\rb\r\nc\nd)"), string(b"a\nb\nc\nd"));
    }

    #[test]
    fn escaped_parenthesis_does_not_close_or_open() {
        assert_eq!(one(br"(a\)b)"), string(b"a)b"));
        assert_eq!(one(br"(a\(b)"), string(b"a(b"));
    }

    #[test]
    fn unterminated_literal_strings_are_errors_at_the_start() {
        for input in [&b"(abc"[..], b"(a(b)", b"(", b"(abc\\", b"(abc\\)"] {
            match first_error(input) {
                Error::Syntax {
                    kind: SyntaxKind::UnterminatedString,
                    offset: 0,
                } => {}
                other => panic!("{:?}: {other:?}", String::from_utf8_lossy(input)),
            }
        }
        // The offset is where the string starts, and nothing after it is lexed.
        let e = first_error(b"1 2 (oops");
        assert_eq!(e.offset(), Some(4));
        let limits = Limits::default();
        let mut lexer = Lexer::new(b"(oops", &limits);
        assert!(lexer.next_token().is_err());
        assert!(lexer.is_eof());
        assert!(lexer.next_token().unwrap().is_none());
    }

    #[test]
    fn hex_strings() {
        let hex = |bytes: &[u8]| TokenKind::HexString(Cow::Owned(bytes.to_vec()));
        assert_eq!(
            one(b"<4E6F762073686D6F7A206B6120706F702E>"),
            hex(b"Nov shmoz ka pop.")
        );
        assert_eq!(one(b"<>"), hex(b""));
        assert_eq!(one(b"<4e6F>"), hex(b"No"));
        // Whitespace is ignored.
        assert_eq!(one(b"<4E 6F\n76\t>"), hex(b"Nov"));
    }

    #[test]
    fn hex_strings_with_odd_digit_count_pad_with_zero() {
        let hex = |bytes: &[u8]| TokenKind::HexString(Cow::Owned(bytes.to_vec()));
        assert_eq!(one(b"<901FA>"), hex(&[0x90, 0x1F, 0xA0]));
        assert_eq!(one(b"<5>"), hex(&[0x50]));
    }

    #[test]
    fn hex_strings_ignore_invalid_characters() {
        let hex = |bytes: &[u8]| TokenKind::HexString(Cow::Owned(bytes.to_vec()));
        assert_eq!(one(b"<4Z1G>"), hex(&[0x41]));
        assert_eq!(one(b"<(41)>"), hex(&[0x41]));
    }

    #[test]
    fn unterminated_hex_strings_are_errors() {
        for input in [&b"<4E6F"[..], b"<", b"<4E 6F "] {
            assert!(
                matches!(
                    first_error(input),
                    Error::Syntax {
                        kind: SyntaxKind::UnterminatedHexString,
                        offset: 0
                    }
                ),
                "{}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn names() {
        let name = |s: &'static [u8]| TokenKind::Name(Cow::Borrowed(s));
        assert_eq!(one(b"/Name1"), name(b"Name1"));
        assert_eq!(one(b"/ASomewhatLongerName"), name(b"ASomewhatLongerName"));
        assert_eq!(
            one(b"/A;Name_With-Various***Characters?"),
            name(b"A;Name_With-Various***Characters?")
        );
        assert_eq!(one(b"/1.2"), name(b"1.2"));
        assert_eq!(one(b"/$$"), name(b"$$"));
        assert_eq!(one(b"/@pattern"), name(b"@pattern"));
        assert_eq!(one(b"/"), name(b""));
        // A name ends at a delimiter or white space.
        assert_eq!(
            lex_all(b"/A/B[/C]/D(x)"),
            [
                name(b"A"),
                name(b"B"),
                TokenKind::ArrayStart,
                name(b"C"),
                TokenKind::ArrayEnd,
                name(b"D"),
                string(b"x"),
            ]
        );
    }

    #[test]
    fn name_hash_escapes() {
        let name = |s: &[u8]| TokenKind::Name(Cow::Owned(s.to_vec()));
        assert_eq!(one(b"/Name#20with#20spaces"), name(b"Name with spaces"));
        assert_eq!(one(b"/A#42"), name(b"AB"));
        assert_eq!(one(b"/#2f"), name(b"/"));
        assert_eq!(one(b"/#00"), name(&[0]));
        // Not two hex digits after `#`: kept literally.
        assert_eq!(one(b"/A#"), name(b"A#"));
        assert_eq!(one(b"/A#4"), name(b"A#4"));
        assert_eq!(one(b"/A#zz"), name(b"A#zz"));
        assert_eq!(one(b"/A#4g"), name(b"A#4g"));
    }

    #[test]
    fn delimiters_and_stray_bytes() {
        assert_eq!(
            lex_all(b"[ ] << >> { }"),
            [
                TokenKind::ArrayStart,
                TokenKind::ArrayEnd,
                TokenKind::DictStart,
                TokenKind::DictEnd,
                TokenKind::BraceStart,
                TokenKind::BraceEnd,
            ]
        );
        assert_eq!(lex_all(b")"), [TokenKind::Stray(b')')]);
        assert_eq!(lex_all(b">"), [TokenKind::Stray(b'>')]);
        // `>>>` is a dictionary end followed by a stray `>`.
        assert_eq!(
            lex_all(b">>>"),
            [TokenKind::DictEnd, TokenKind::Stray(b'>')]
        );
        // Dictionary start versus hex string.
        assert_eq!(lex_all(b"<<>>"), [TokenKind::DictStart, TokenKind::DictEnd]);
        assert_eq!(lex_all(b"<>"), [TokenKind::HexString(Cow::Owned(vec![]))]);
    }

    #[test]
    fn tokens_stick_together_without_white_space() {
        assert_eq!(
            lex_all(b"[1 2]<</K/V>>"),
            [
                TokenKind::ArrayStart,
                TokenKind::Integer(1),
                TokenKind::Integer(2),
                TokenKind::ArrayEnd,
                TokenKind::DictStart,
                TokenKind::Name(Cow::Borrowed(b"K")),
                TokenKind::Name(Cow::Borrowed(b"V")),
                TokenKind::DictEnd,
            ]
        );
    }

    #[test]
    fn comments_are_skipped_by_next_token_and_kept_by_next_raw() {
        let input = b"1 % a comment ( not a string\n2%x\r3";
        assert_eq!(
            lex_all(input),
            [
                TokenKind::Integer(1),
                TokenKind::Integer(2),
                TokenKind::Integer(3)
            ]
        );
        let limits = Limits::default();
        let mut lexer = Lexer::new(input, &limits);
        let mut raw = Vec::new();
        while let Some(t) = lexer.next_raw().unwrap() {
            raw.push(t);
        }
        assert_eq!(
            raw[2].kind,
            TokenKind::Comment(b" a comment ( not a string")
        );
        assert_eq!(raw[2].span, 2..28);
        assert_eq!(raw[1].kind, TokenKind::Whitespace);
        // The end-of-line marker is white space, not part of the comment.
        assert_eq!(raw[3].kind, TokenKind::Whitespace);
        let comments = raw
            .iter()
            .filter(|t| matches!(t.kind, TokenKind::Comment(_)))
            .count();
        assert_eq!(comments, 2);
    }

    #[test]
    fn a_comment_at_the_end_of_input_is_fine() {
        let limits = Limits::default();
        let mut lexer = Lexer::new(b"%PDF-1.7", &limits);
        let t = lexer.next_raw().unwrap().unwrap();
        assert_eq!(t.kind, TokenKind::Comment(b"PDF-1.7"));
        assert_eq!(t.span, 0..8);
        assert!(lexer.next_raw().unwrap().is_none());
    }

    #[test]
    fn whitespace_includes_nul_and_form_feed() {
        assert_eq!(
            lex_all(b"1\x002\x0c3\t4\r5\n6 7"),
            (1..=7).map(TokenKind::Integer).collect::<Vec<_>>()
        );
    }

    #[test]
    fn spans_cover_the_token_bytes() {
        let limits = Limits::default();
        let input = b"/Name (str) <41> 12 << >>";
        let mut lexer = Lexer::new(input, &limits);
        let mut spans = Vec::new();
        while let Some(t) = lexer.next_token().unwrap() {
            spans.push(&input[t.span]);
        }
        let expected: Vec<&[u8]> = vec![b"/Name", b"(str)", b"<41>", b"12", b"<<", b">>"];
        assert_eq!(spans, expected);
    }

    #[test]
    fn stream_keyword_eol_rules() {
        let limits = Limits::default();
        for (input, data_start, eol) in [
            (&b"stream\r\nDATA"[..], 8, StreamEol::CrLf),
            (b"stream\nDATA", 7, StreamEol::Lf),
            (b"stream\rDATA", 7, StreamEol::Cr),
            // Data right after the keyword; the delimiter makes it lex as `stream`.
            (b"stream<DATA", 6, StreamEol::Missing),
            // Blanks before the marker are tolerated.
            (b"stream  \r\nDATA", 10, StreamEol::CrLf),
            (b"stream \nDATA", 8, StreamEol::Lf),
            // ... but blanks that are data are not eaten.
            (b"stream  DATA", 6, StreamEol::Missing),
            // CR CR LF: the first CR is the marker, the rest is data.
            (b"stream\r\r\nDATA", 7, StreamEol::Cr),
            (b"stream", 6, StreamEol::Missing),
            (b"stream\r", 7, StreamEol::Cr),
        ] {
            let mut lexer = Lexer::new(input, &limits);
            let kw = lexer.next_token().unwrap().unwrap();
            assert_eq!(kw.kind, TokenKind::Keyword(b"stream"));
            let start = lexer.skip_stream_eol();
            assert_eq!(
                (start.data_start, start.eol),
                (data_start, eol),
                "{:?}",
                String::from_utf8_lossy(input)
            );
            assert_eq!(lexer.position(), data_start);
        }
    }

    #[test]
    fn name_length_limit_is_enforced() {
        let limits = Limits {
            max_name_bytes: 4,
            ..Limits::default()
        };
        assert_eq!(
            lex_with(b"/abcd", &limits),
            [TokenKind::Name(Cow::Borrowed(b"abcd"))]
        );
        let mut lexer = Lexer::new(b"/abcde 7", &limits);
        let err = lexer.next_token().unwrap_err();
        assert!(matches!(
            err,
            Error::LimitExceeded {
                limit: LimitKind::NameBytes,
                max: 4,
                value: 5,
                offset: Some(0)
            }
        ));
        // The position is past the name, so lexing continues.
        assert_eq!(
            lexer.next_token().unwrap().unwrap().kind,
            TokenKind::Integer(7)
        );
    }

    #[test]
    fn string_length_limit_is_enforced_for_every_string_form() {
        let limits = Limits {
            max_string_bytes: 3,
            ..Limits::default()
        };
        for ok in [&b"(abc)"[..], br"(a\nb)", b"<616263>"] {
            assert_eq!(lex_with(ok, &limits).len(), 1);
        }
        for too_long in [&b"(abcd)"[..], br"(a\nbc)", b"<61626364>", b"<6162636>"] {
            let mut lexer = Lexer::new(too_long, &limits);
            let err = lexer.next_token().unwrap_err();
            assert!(
                matches!(
                    err,
                    Error::LimitExceeded {
                        limit: LimitKind::StringBytes,
                        max: 3,
                        ..
                    }
                ),
                "{}: {err:?}",
                String::from_utf8_lossy(too_long)
            );
            assert!(lexer.is_eof(), "position after the string");
        }
    }

    #[test]
    fn lexer_can_start_in_the_middle_and_seek() {
        let limits = Limits::default();
        let data = b"1 2 3";
        let mut lexer = Lexer::at(data, 2, &limits);
        assert_eq!(
            lexer.next_token().unwrap().unwrap().kind,
            TokenKind::Integer(2)
        );
        lexer.seek(100);
        assert!(lexer.is_eof());
        assert!(lexer.next_token().unwrap().is_none());
        assert!(Lexer::at(data, 100, &limits).is_eof());
    }

    #[test]
    fn empty_input() {
        assert_eq!(lex_all(b"").len(), 0);
        assert_eq!(lex_all(b" \r\n").len(), 0);
    }

    proptest! {
        /// The lexer never panics, always terminates, and its spans tile the input.
        #[test]
        fn never_panics_and_spans_tile_the_input(input in proptest::collection::vec(any::<u8>(), 0..512)) {
            let limits = Limits::default();
            let mut lexer = Lexer::new(&input, &limits);
            let mut expected_start = 0usize;
            let mut count = 0usize;
            loop {
                let before = lexer.position();
                match lexer.next_raw() {
                    Ok(Some(t)) => {
                        prop_assert_eq!(t.span.start, expected_start);
                        prop_assert!(t.span.end > t.span.start, "empty token");
                        prop_assert!(t.span.end <= input.len());
                        expected_start = t.span.end;
                        prop_assert_eq!(lexer.position(), t.span.end);
                    }
                    Ok(None) => {
                        prop_assert_eq!(expected_start, input.len());
                        break;
                    }
                    Err(_) => {
                        // Progress is guaranteed even on error.
                        prop_assert!(lexer.position() > before);
                        expected_start = lexer.position();
                    }
                }
                count += 1;
                prop_assert!(count <= input.len() + 1, "no progress");
            }
        }

        /// Tight limits are enforced without panicking on arbitrary bytes.
        #[test]
        fn tight_limits_never_panic(input in proptest::collection::vec(any::<u8>(), 0..256)) {
            let limits = Limits { max_string_bytes: 2, max_name_bytes: 2, ..Limits::default() };
            let mut lexer = Lexer::new(&input, &limits);
            let mut steps = 0;
            while !lexer.is_eof() && steps <= input.len() {
                let _ = lexer.next_token();
                steps += 1;
            }
            prop_assert!(lexer.is_eof());
        }

        /// Printing a hex string and lexing it gives the bytes back.
        #[test]
        fn hex_string_round_trip(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let mut text = vec![b'<'];
            for b in &bytes {
                text.extend(format!("{b:02x}").bytes());
            }
            text.push(b'>');
            let limits = Limits::default();
            let token = Lexer::new(&text, &limits).next_token().unwrap().unwrap();
            prop_assert_eq!(token.kind, TokenKind::HexString(Cow::Owned(bytes)));
        }

        /// Escaping a string by hand and lexing it gives the bytes back.
        #[test]
        fn literal_string_round_trip(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let mut text = vec![b'('];
            for &b in &bytes {
                // Octal escapes for everything that is not plain printable ASCII.
                if (0x20..0x7f).contains(&b) && b != b'(' && b != b')' && b != b'\\' {
                    text.push(b);
                } else {
                    text.extend(format!("\\{b:03o}").bytes());
                }
            }
            text.push(b')');
            let limits = Limits::default();
            let token = Lexer::new(&text, &limits).next_token().unwrap().unwrap();
            prop_assert_eq!(token.kind, TokenKind::LiteralString(Cow::Owned(bytes)));
        }

        /// Integers print and lex back to themselves.
        #[test]
        fn integer_round_trip(n in any::<i64>()) {
            let text = n.to_string();
            let limits = Limits::default();
            let token = Lexer::new(text.as_bytes(), &limits).next_token().unwrap().unwrap();
            prop_assert_eq!(token.kind, TokenKind::Integer(n));
        }
    }
}
