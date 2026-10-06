//! Classic cross-reference tables, trailers and the `/Prev` chain (ISO 32000-2:2020 §7.5.4–7.5.6).
//!
//! [`Xref::parse`] finds `startxref` near the end of the file, reads the cross-reference table it
//! points to and follows `/Prev` back through older sections, producing one [`Revision`] per
//! section. Cross-reference streams and hybrid files are a later task (M0 task 7); a file whose
//! newest section is a stream is reported as [`SyntaxKind::ExpectedXrefKeyword`].
//!
//! Entry offsets are **not validated here** (never trust offsets without validation): they are
//! raw values for the object store to check when it reads an object.
//!
//! Tolerance, each case reported as an [`XrefWarning`]:
//! - A subsection that starts at 1 although its first entry is the free head entry (`65535 f`)
//!   is renumbered to start at 0, a common writer bug that qpdf and PDFium also repair.
//! - A subsection with fewer entries than declared, ended by `trailer`.
//!
//! Entries are read as tokens, not as fixed 20-byte records, so differing end-of-line markers
//! and 19-byte entries are accepted.
//!
//! Offsets are relative to the `%PDF-` header when the file has junk before it (§7.5.4 note);
//! [`Xref::base`] is that header offset and callers add it to entry offsets.

use std::collections::HashSet;
use std::ops::Range;

use crate::error::{Error, Result, SyntaxKind};
use crate::lexer::{Lexer, Token, TokenKind};
use crate::limits::{LimitKind, Limits};
use crate::object::{Dict, ObjectKind};
use crate::parser::Parser;

/// How far from the end of the file `startxref` is searched. The spec says the last 1024
/// bytes; trailing junk after `%%EOF` is common, so this is wider.
pub const STARTXREF_SEARCH_BYTES: usize = 4096;

/// How far into the file `%PDF-` is searched when the file has junk before the header.
pub const HEADER_SEARCH_BYTES: usize = 1024;

/// One cross-reference entry (§7.5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum XrefEntry {
    /// A free object: it is deleted or never existed.
    Free {
        /// Object number of the next free object (the free list link).
        next_free: u64,
        /// Generation number to use if the number is reused.
        generation: u16,
    },
    /// An object stored at a byte offset.
    InUse {
        /// Offset of `n g obj`, relative to the header (see [`Xref::base`]). Not validated.
        offset: u64,
        /// Generation number.
        generation: u16,
    },
}

/// Something tolerated while reading a section; feeds the document's repaired reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum XrefWarning {
    /// A subsection declared a first object number of 1 but started with the free head entry,
    /// so its numbers were shifted down by one.
    SubsectionStartShifted,
    /// A subsection ended (at `trailer`) with fewer entries than it declared.
    FewerEntriesThanDeclared,
}

/// One cross-reference table with its trailer.
#[derive(Debug, Clone, PartialEq)]
pub struct XrefSection<'a> {
    /// Offset of the `xref` keyword in the file.
    pub offset: usize,
    /// Entries as `(object number, entry)` in file order.
    pub entries: Vec<(u32, XrefEntry)>,
    /// The trailer dictionary.
    pub trailer: Dict<'a>,
    /// Bytes from `xref` through the end of the trailer dictionary.
    pub span: Range<usize>,
    /// Anything tolerated while reading.
    pub warnings: Vec<XrefWarning>,
}

/// One revision of the document: a cross-reference section and the bytes it covers.
#[derive(Debug, Clone, PartialEq)]
pub struct Revision<'a> {
    /// The section.
    pub section: XrefSection<'a>,
    /// The bytes of this revision: from the end of the previous (older) revision, or the start
    /// of the file, through the `%%EOF` after this section's `startxref` (or the last byte of
    /// that `startxref` line if the marker is missing).
    pub bytes: Range<usize>,
}

/// The `startxref` found at the end of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Startxref {
    /// Offset of the `startxref` keyword.
    pub keyword: usize,
    /// The offset it declares, relative to the header.
    pub offset: u64,
}

/// The whole chain of cross-reference sections of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Xref<'a> {
    /// Offset of `%PDF-` (0 for a normal file). Entry offsets are relative to it.
    pub base: usize,
    /// Revisions, **newest first** (the order in which objects are looked up).
    pub revisions: Vec<Revision<'a>>,
}

/// Finds the last `startxref` in the final [`STARTXREF_SEARCH_BYTES`] of `data` and reads the
/// offset after it.
///
/// # Errors
/// [`SyntaxKind::StartxrefNotFound`] if there is none or no integer follows it.
pub fn find_startxref(data: &[u8], limits: &Limits) -> Result<Startxref> {
    const KEYWORD: &[u8] = b"startxref";
    let window_start = data.len().saturating_sub(STARTXREF_SEARCH_BYTES);
    let tail = data.get(window_start..).unwrap_or_default();
    let not_found = || Error::Syntax {
        kind: SyntaxKind::StartxrefNotFound,
        offset: data.len() as u64,
    };
    let rel = tail
        .windows(KEYWORD.len())
        .rposition(|w| w == KEYWORD)
        .ok_or_else(not_found)?;
    let keyword = window_start + rel;
    let mut lexer = Lexer::at(data, keyword + KEYWORD.len(), limits);
    match lexer.next_token() {
        Ok(Some(t)) => match t.kind {
            TokenKind::Integer(n) => u64::try_from(n).map_or_else(
                |_| Err(not_found()),
                |offset| Ok(Startxref { keyword, offset }),
            ),
            _ => Err(not_found()),
        },
        _ => Err(not_found()),
    }
}

/// Parses the cross-reference table at `pos` (which must be at the `xref` keyword, white space
/// before it is skipped) together with its trailer dictionary.
///
/// `entries_so_far` is the number of entries already read from newer sections of the same
/// file, so the [`Limits::max_xref_entries`] limit covers the whole chain.
///
/// # Errors
/// [`SyntaxKind::ExpectedXrefKeyword`], [`SyntaxKind::MalformedXrefSection`],
/// [`SyntaxKind::TrailerNotDictionary`], [`Error::LimitExceeded`] for too many entries, or any
/// error from parsing the trailer dictionary.
pub fn parse_xref_section<'a>(
    data: &'a [u8],
    pos: usize,
    limits: &Limits,
    entries_so_far: u64,
) -> Result<XrefSection<'a>> {
    let mut lexer = Lexer::at(data, pos, limits);

    let start = match lexer.next_token()? {
        Some(t) if t.kind == TokenKind::Keyword(b"xref") => t.span.start,
        other => {
            return Err(syntax(
                SyntaxKind::ExpectedXrefKeyword,
                other.map_or(pos, |t| t.span.start),
            ));
        }
    };

    let mut entries: Vec<(u32, XrefEntry)> = Vec::new();
    let mut warnings = Vec::new();
    let mut first_subsection = true;
    loop {
        let at = lexer.position();
        let Some(token) = lexer.next_token()? else {
            return Err(syntax(SyntaxKind::MalformedXrefSection, at));
        };
        let first = match token.kind {
            TokenKind::Keyword(b"trailer") => break,
            TokenKind::Integer(first) => first,
            _ => return Err(syntax(SyntaxKind::MalformedXrefSection, token.span.start)),
        };
        let count = match lexer.next_token()? {
            Some(Token {
                kind: TokenKind::Integer(count),
                ..
            }) => count,
            Some(t) => return Err(syntax(SyntaxKind::MalformedXrefSection, t.span.start)),
            None => return Err(syntax(SyntaxKind::MalformedXrefSection, at)),
        };
        let (Ok(first), Ok(count)) = (u32::try_from(first), u64::try_from(count)) else {
            return Err(syntax(SyntaxKind::MalformedXrefSection, token.span.start));
        };
        limits.check(
            LimitKind::XrefEntries,
            entries_so_far
                .saturating_add(entries.len() as u64)
                .saturating_add(count),
            Some(token.span.start as u64),
        )?;

        let section_first_entry = entries.len();
        read_subsection(&mut lexer, first, count, &mut entries, &mut warnings)?;
        if first_subsection {
            repair_off_by_one(&mut entries, section_first_entry, first, &mut warnings);
        }
        first_subsection = false;
    }

    let mut parser = Parser::at(data, lexer.position(), limits);
    let trailer = parser.parse_object().map_err(|e| match e {
        Error::Syntax { .. } => syntax(
            SyntaxKind::TrailerNotDictionary,
            usize::try_from(e.offset().unwrap_or(0)).unwrap_or(0),
        ),
        other => other,
    })?;
    let end = trailer.span.end;
    let ObjectKind::Dict(trailer) = trailer.kind else {
        return Err(syntax(SyntaxKind::TrailerNotDictionary, trailer.span.start));
    };
    Ok(XrefSection {
        offset: start,
        entries,
        trailer,
        span: start..end,
        warnings,
    })
}

fn syntax(kind: SyntaxKind, offset: usize) -> Error {
    Error::Syntax {
        kind,
        offset: offset as u64,
    }
}

/// Reads up to `count` entries of one subsection that starts at object number `first`.
fn read_subsection(
    lexer: &mut Lexer<'_, '_>,
    first: u32,
    count: u64,
    entries: &mut Vec<(u32, XrefEntry)>,
    warnings: &mut Vec<XrefWarning>,
) -> Result<()> {
    let malformed = |offset: usize| syntax(SyntaxKind::MalformedXrefSection, offset);
    for read in 0..count {
        let before = lexer.position();
        let Some(offset_token) = lexer.next_token()? else {
            return Err(malformed(before));
        };
        let TokenKind::Integer(offset) = offset_token.kind else {
            if offset_token.kind == TokenKind::Keyword(b"trailer") {
                // Fewer entries than declared: put `trailer` back for the caller.
                lexer.seek(offset_token.span.start);
                warnings.push(XrefWarning::FewerEntriesThanDeclared);
                return Ok(());
            }
            return Err(malformed(offset_token.span.start));
        };
        let start = offset_token.span.start;
        let Some(TokenKind::Integer(generation)) = lexer.next_token()?.map(|t| t.kind) else {
            return Err(malformed(start));
        };
        let in_use = match lexer.next_token()?.map(|t| t.kind) {
            Some(TokenKind::Keyword(b"n")) => true,
            Some(TokenKind::Keyword(b"f")) => false,
            _ => return Err(malformed(start)),
        };
        let (Ok(offset), Ok(generation)) = (u64::try_from(offset), u16::try_from(generation))
        else {
            return Err(malformed(start));
        };
        let number = u32::try_from(u64::from(first) + read).map_err(|_| malformed(start))?;
        let entry = if in_use {
            XrefEntry::InUse { offset, generation }
        } else {
            XrefEntry::Free {
                next_free: offset,
                generation,
            }
        };
        entries.push((number, entry));
    }
    Ok(())
}

/// ISO 32000-2 §7.5.4: the free list head is object 0 with generation 65535. A first
/// subsection that says "1 N" but begins with that entry is off by one, so renumber it.
fn repair_off_by_one(
    entries: &mut [(u32, XrefEntry)],
    section_first_entry: usize,
    declared_first: u32,
    warnings: &mut Vec<XrefWarning>,
) {
    let starts_with_head = matches!(
        entries.get(section_first_entry),
        Some((
            1,
            XrefEntry::Free {
                generation: 65535,
                ..
            }
        ))
    );
    if declared_first == 1 && starts_with_head {
        for (number, _) in entries.iter_mut().skip(section_first_entry) {
            *number -= 1;
        }
        warnings.push(XrefWarning::SubsectionStartShifted);
    }
}

impl<'a> Xref<'a> {
    /// Reads the whole chain of classic cross-reference sections of `data`.
    ///
    /// # Errors
    /// [`SyntaxKind::StartxrefNotFound`], [`SyntaxKind::InvalidXrefOffset`] for an offset past
    /// the end of the file, [`SyntaxKind::XrefPrevLoop`] if `/Prev` leads back to a section
    /// already read, [`Error::LimitExceeded`] for too many revisions or entries, and any error
    /// from [`parse_xref_section`].
    pub fn parse(data: &'a [u8], limits: &Limits) -> Result<Self> {
        let header = data
            .get(..HEADER_SEARCH_BYTES.min(data.len()))
            .unwrap_or_default();
        let base = header
            .windows(b"%PDF-".len())
            .position(|w| w == b"%PDF-")
            .unwrap_or(0);

        let startxref = find_startxref(data, limits)?;
        let invalid = |offset: u64| Error::Syntax {
            kind: SyntaxKind::InvalidXrefOffset,
            offset,
        };
        let locate = |declared: u64, origin: u64| -> Result<usize> {
            usize::try_from(declared)
                .ok()
                .and_then(|d| d.checked_add(base))
                .filter(|&p| p < data.len())
                .ok_or_else(|| invalid(origin))
        };

        let mut pos = locate(startxref.offset, startxref.keyword as u64)?;
        let mut visited = HashSet::new();
        let mut revisions: Vec<Revision<'a>> = Vec::new();
        let mut total_entries = 0u64;
        let mut ends = Vec::new();
        loop {
            if !visited.insert(pos) {
                return Err(Error::Syntax {
                    kind: SyntaxKind::XrefPrevLoop,
                    offset: pos as u64,
                });
            }
            limits.check(
                LimitKind::Revisions,
                revisions.len() as u64 + 1,
                Some(pos as u64),
            )?;
            let section = parse_xref_section(data, pos, limits, total_entries)?;
            total_entries += section.entries.len() as u64;
            ends.push(revision_end(data, section.span.end, limits));

            let prev = match section.trailer.get(b"Prev").map(|o| &o.kind) {
                Some(ObjectKind::Integer(n)) => Some((*n, section.offset)),
                _ => None,
            };
            revisions.push(Revision {
                section,
                bytes: 0..0,
            });
            let Some((prev, origin)) = prev else { break };
            let prev = u64::try_from(prev).map_err(|_| invalid(origin as u64))?;
            pos = locate(prev, origin as u64)?;
        }

        // Byte ranges: each revision starts where the next older one ended.
        let mut start = 0;
        for (revision, end) in revisions.iter_mut().zip(ends).rev() {
            let end = end.max(start);
            revision.bytes = start..end;
            start = end;
        }
        Ok(Self { base, revisions })
    }

    /// The trailer of the newest revision.
    #[must_use]
    pub fn trailer(&self) -> Option<&Dict<'a>> {
        self.revisions.first().map(|r| &r.section.trailer)
    }

    /// How many objects are in use, looking each object number up newest revision first, so an
    /// object freed by a later revision does not count.
    #[must_use]
    pub fn in_use_count(&self) -> usize {
        let mut seen = HashSet::new();
        let mut in_use = 0;
        for revision in &self.revisions {
            for (number, entry) in &revision.section.entries {
                if seen.insert(*number) && matches!(entry, XrefEntry::InUse { .. }) {
                    in_use += 1;
                }
            }
        }
        in_use
    }
}

/// End of a revision: after `startxref <n>` and the `%%EOF` marker that follows the trailer at
/// `trailer_end`; falls back to what is there if the marker or keyword is missing.
fn revision_end(data: &[u8], trailer_end: usize, limits: &Limits) -> usize {
    let mut lexer = Lexer::at(data, trailer_end, limits);
    let mut end = trailer_end;
    if let Ok(Some(t)) = lexer.next_token()
        && t.kind == TokenKind::Keyword(b"startxref")
    {
        end = t.span.end;
        if let Ok(Some(n)) = lexer.next_token()
            && matches!(n.kind, TokenKind::Integer(_))
        {
            end = n.span.end;
            // The marker is a comment, so read raw tokens to see it.
            while let Ok(Some(raw)) = lexer.next_raw() {
                match raw.kind {
                    TokenKind::Whitespace => {}
                    TokenKind::Comment(b"%EOF") => {
                        end = raw.span.end;
                        break;
                    }
                    _ => break,
                }
            }
        }
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a file from `(object text)` pieces with a correct classic xref table.
    struct Builder {
        data: Vec<u8>,
        offsets: Vec<(u32, usize)>,
    }

    impl Builder {
        fn new() -> Self {
            Self {
                data: b"%PDF-1.7\n".to_vec(),
                offsets: Vec::new(),
            }
        }

        fn object(&mut self, num: u32, body: &str) {
            self.offsets.push((num, self.data.len()));
            self.data
                .extend(format!("{num} 0 obj\n{body}\nendobj\n").bytes());
        }

        /// Writes a table covering the objects added since the last call, one subsection per
        /// object, plus the free head entry when `with_head`.
        fn section(&mut self, with_head: bool, trailer_extra: &str) -> usize {
            let at = self.data.len();
            self.data.extend(b"xref\n");
            if with_head {
                self.data
                    .extend(b"0 1\n0000000000 65535 f \n".iter().copied());
            }
            for (num, off) in std::mem::take(&mut self.offsets) {
                self.data
                    .extend(format!("{num} 1\n{off:010} 00000 n \n").bytes());
            }
            self.data
                .extend(format!("trailer\n<< /Size 10 /Root 1 0 R {trailer_extra}>>\n").bytes());
            self.data
                .extend(format!("startxref\n{at}\n%%EOF\n").bytes());
            at
        }
    }

    fn parse(data: &[u8]) -> Xref<'_> {
        Xref::parse(data, &Limits::default()).unwrap()
    }

    fn error_kind(data: &[u8]) -> SyntaxKind {
        match Xref::parse(data, &Limits::default()).unwrap_err() {
            Error::Syntax { kind, .. } => kind,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn single_revision() {
        let mut b = Builder::new();
        b.object(1, "<< /Type /Catalog >>");
        b.object(2, "<< /Type /Pages >>");
        let at = b.section(true, "");
        let xref = parse(&b.data);
        assert_eq!(xref.base, 0);
        assert_eq!(xref.revisions.len(), 1);
        let rev = &xref.revisions[0];
        assert_eq!(rev.section.offset, at);
        assert_eq!(rev.section.entries.len(), 3);
        assert_eq!(
            rev.section.entries[0],
            (
                0,
                XrefEntry::Free {
                    next_free: 0,
                    generation: 65535
                }
            )
        );
        assert_eq!(
            rev.section.entries[1],
            (
                1,
                XrefEntry::InUse {
                    offset: 9,
                    generation: 0
                }
            )
        );
        assert_eq!(xref.in_use_count(), 2);
        assert_eq!(rev.section.warnings, Vec::new());
        assert_eq!(
            xref.trailer().unwrap().get(b"Size").unwrap().as_integer(),
            Some(10)
        );
        // The revision covers the entire file, through the final %%EOF.
        assert_eq!(rev.bytes, 0..b.data.len() - 1);
        assert_eq!(&b.data[rev.bytes.end - 5..rev.bytes.end], b"%%EOF");
        assert_eq!(&b.data[rev.section.span.clone()][..4], b"xref");
    }

    #[test]
    fn multiple_revisions_newest_first_with_byte_ranges() {
        let mut b = Builder::new();
        b.object(1, "<< /Type /Catalog >>");
        b.object(2, "<< /V 1 >>");
        let first = b.section(true, "");
        let end_first = b.data.len() - 1;
        b.object(2, "<< /V 2 >>"); // replaced
        b.object(3, "<< /New true >>");
        let second = b.section(false, &format!("/Prev {first} "));
        let end_second = b.data.len() - 1;
        b.object(4, "<< /Another 1 >>");
        let third = b.section(false, &format!("/Prev {second} "));
        let xref = parse(&b.data);

        assert_eq!(xref.revisions.len(), 3);
        let offsets: Vec<_> = xref.revisions.iter().map(|r| r.section.offset).collect();
        assert_eq!(offsets, [third, second, first]);
        // Byte ranges tile the file with no gaps.
        let ranges: Vec<_> = xref.revisions.iter().map(|r| r.bytes.clone()).collect();
        assert_eq!(ranges[2], 0..end_first);
        assert_eq!(ranges[1], end_first..end_second);
        assert_eq!(ranges[0], end_second..b.data.len() - 1);
        // 1, 2 (twice), 3, 4: four distinct objects in use.
        assert_eq!(xref.in_use_count(), 4);
        // The newest trailer is the document trailer.
        assert!(xref.trailer().unwrap().get(b"Prev").is_some());
    }

    #[test]
    fn an_object_freed_by_a_later_revision_is_not_in_use() {
        let mut b = Builder::new();
        b.object(1, "<< >>");
        b.object(2, "<< >>");
        let first = b.section(true, "");
        // Second revision frees object 2.
        let at = b.data.len();
        b.data.extend(b"xref\n2 1\n0000000000 00001 f \n");
        b.data.extend(
            format!("trailer\n<< /Size 3 /Prev {first} >>\nstartxref\n{at}\n%%EOF\n").bytes(),
        );
        let xref = parse(&b.data);
        assert_eq!(xref.revisions.len(), 2);
        assert_eq!(xref.in_use_count(), 1);
    }

    #[test]
    fn prev_loops_are_detected() {
        // A section whose /Prev points at itself.
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        data.extend(
            format!("xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 1 /Prev {at} >>\nstartxref\n{at}\n%%EOF\n")
                .bytes(),
        );
        assert_eq!(error_kind(&data), SyntaxKind::XrefPrevLoop);

        // A two-section cycle: A -> B -> A.
        let mut data = b"%PDF-1.7\n".to_vec();
        let a = data.len();
        let template = |prev: usize| {
            format!("xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 1 /Prev {prev:04} >>\n")
        };
        let section_len = template(0).len();
        let b = a + section_len;
        data.extend(template(b).bytes());
        data.extend(template(a).bytes());
        data.extend(format!("startxref\n{a}\n%%EOF\n").bytes());
        assert_eq!(error_kind(&data), SyntaxKind::XrefPrevLoop);
    }

    #[test]
    fn startxref_lookup() {
        let limits = Limits::default();
        let found = find_startxref(b"%PDF-1.7\nstartxref\n1234\n%%EOF\n", &limits).unwrap();
        assert_eq!(
            found,
            Startxref {
                keyword: 9,
                offset: 1234
            }
        );
        // The last occurrence wins.
        let found = find_startxref(b"startxref\n1\n%%EOF\nstartxref\n2\n%%EOF", &limits).unwrap();
        assert_eq!(found.offset, 2);
        // Trailing junk after %%EOF within the window is fine.
        let mut data = b"startxref\n7\n%%EOF\n".to_vec();
        data.extend(vec![b'x'; 3000]);
        assert_eq!(find_startxref(&data, &limits).unwrap().offset, 7);
        // ... beyond the window it is not found.
        data.extend(vec![b'x'; 2000]);
        assert!(find_startxref(&data, &limits).is_err());
        for bad in [
            &b""[..],
            b"no keyword here",
            b"startxref",
            b"startxref\n-5",
            b"startxref\nabc",
            b"startxref\n(1)",
        ] {
            assert!(
                matches!(
                    find_startxref(bad, &limits),
                    Err(Error::Syntax {
                        kind: SyntaxKind::StartxrefNotFound,
                        ..
                    })
                ),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn offsets_outside_the_file_are_errors() {
        assert_eq!(
            error_kind(b"%PDF-1.7\nstartxref\n999999\n%%EOF\n"),
            SyntaxKind::InvalidXrefOffset
        );
        // /Prev beyond the end.
        let mut b = Builder::new();
        b.object(1, "<< >>");
        b.section(true, "/Prev 99999999 ");
        assert_eq!(error_kind(&b.data), SyntaxKind::InvalidXrefOffset);
        // Negative /Prev.
        let mut b = Builder::new();
        b.object(1, "<< >>");
        b.section(true, "/Prev -5 ");
        assert_eq!(error_kind(&b.data), SyntaxKind::InvalidXrefOffset);
    }

    #[test]
    fn startxref_pointing_at_something_else() {
        let data = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\nstartxref\n9\n%%EOF\n";
        assert_eq!(error_kind(data), SyntaxKind::ExpectedXrefKeyword);
        let data = b"%PDF-1.7\nstartxref\n0\n%%EOF\n";
        assert_eq!(error_kind(data), SyntaxKind::ExpectedXrefKeyword);
    }

    #[test]
    fn tolerates_unusual_entry_formatting() {
        // CR-only line ends, LF-only entries without the trailing space (19 bytes), extra blanks.
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        data.extend(
            b"xref\r0 3\r0000000000 65535 f\n0000000009 00000 n\n  0000000100   00000  n \r\n",
        );
        data.extend(format!("trailer\r<< /Size 3 >>\rstartxref\r{at}\r%%EOF").bytes());
        let xref = parse(&data);
        assert_eq!(xref.in_use_count(), 2);
        let entries = &xref.revisions[0].section.entries;
        assert_eq!(
            entries[2],
            (
                2,
                XrefEntry::InUse {
                    offset: 100,
                    generation: 0
                }
            )
        );
    }

    #[test]
    fn multiple_subsections_and_gaps() {
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        data.extend(
            b"xref\n0 1\n0000000000 65535 f \n5 2\n0000000010 00000 n \n0000000020 00002 n \n9 1\n0000000030 00000 n \n",
        );
        data.extend(format!("trailer\n<< /Size 10 >>\nstartxref\n{at}\n%%EOF\n").bytes());
        let xref = parse(&data);
        let numbers: Vec<u32> = xref.revisions[0]
            .section
            .entries
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(numbers, [0, 5, 6, 9]);
        assert_eq!(
            xref.revisions[0].section.entries[2].1,
            XrefEntry::InUse {
                offset: 20,
                generation: 2
            }
        );
    }

    #[test]
    fn off_by_one_subsection_start_is_repaired_and_reported() {
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        data.extend(b"xref\n1 3\n0000000000 65535 f \n0000000009 00000 n \n0000000050 00000 n \n");
        data.extend(format!("trailer\n<< /Size 3 >>\nstartxref\n{at}\n%%EOF\n").bytes());
        let xref = parse(&data);
        let section = &xref.revisions[0].section;
        let numbers: Vec<u32> = section.entries.iter().map(|(n, _)| *n).collect();
        assert_eq!(numbers, [0, 1, 2]);
        assert_eq!(section.warnings, [XrefWarning::SubsectionStartShifted]);
        // A table starting at 1 whose first entry is a normal in-use object is left alone.
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        data.extend(b"xref\n1 1\n0000000009 00000 n \n");
        data.extend(format!("trailer\n<< /Size 2 >>\nstartxref\n{at}\n%%EOF\n").bytes());
        let xref = parse(&data);
        assert_eq!(xref.revisions[0].section.entries[0].0, 1);
        assert_eq!(xref.revisions[0].section.warnings, Vec::new());
    }

    #[test]
    fn fewer_entries_than_declared_is_reported() {
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        data.extend(b"xref\n0 5\n0000000000 65535 f \n0000000009 00000 n \n");
        data.extend(format!("trailer\n<< /Size 5 >>\nstartxref\n{at}\n%%EOF\n").bytes());
        let xref = parse(&data);
        let section = &xref.revisions[0].section;
        assert_eq!(section.entries.len(), 2);
        assert_eq!(section.warnings, [XrefWarning::FewerEntriesThanDeclared]);
    }

    #[test]
    fn malformed_tables_are_typed_errors() {
        let wrap = |body: &str| {
            let mut data = b"%PDF-1.7\n".to_vec();
            let at = data.len();
            data.extend(body.as_bytes());
            data.extend(format!("\nstartxref\n{at}\n%%EOF\n").bytes());
            data
        };
        for (name, body, kind) in [
            (
                "entry without n/f",
                "xref\n0 1\n0000000000 65535 x \ntrailer\n<< >>",
                SyntaxKind::MalformedXrefSection,
            ),
            (
                "entry missing generation",
                "xref\n0 1\n0000000000 n \ntrailer\n<< >>",
                SyntaxKind::MalformedXrefSection,
            ),
            (
                "negative offset",
                "xref\n0 1\n-5 00000 n \ntrailer\n<< >>",
                SyntaxKind::MalformedXrefSection,
            ),
            (
                "generation too large",
                "xref\n0 1\n0000000009 70000 n \ntrailer\n<< >>",
                SyntaxKind::MalformedXrefSection,
            ),
            (
                "negative count",
                "xref\n0 -1\ntrailer\n<< >>",
                SyntaxKind::MalformedXrefSection,
            ),
            (
                "junk instead of subsection",
                "xref\nfoo\ntrailer\n<< >>",
                SyntaxKind::MalformedXrefSection,
            ),
            ("count missing", "xref\n0", SyntaxKind::MalformedXrefSection),
            (
                "no trailer keyword",
                "xref\n0 1\n0000000000 65535 f \n",
                SyntaxKind::MalformedXrefSection,
            ),
            (
                "trailer not a dictionary",
                "xref\n0 1\n0000000000 65535 f \ntrailer\n[1 2]",
                SyntaxKind::TrailerNotDictionary,
            ),
            (
                "trailer broken",
                "xref\n0 1\n0000000000 65535 f \ntrailer\n<< /A",
                SyntaxKind::TrailerNotDictionary,
            ),
            (
                "object number overflow",
                "xref\n4294967295 2\n0000000009 00000 n \n0000000009 00000 n \ntrailer\n<< >>",
                SyntaxKind::MalformedXrefSection,
            ),
        ] {
            assert_eq!(error_kind(&wrap(body)), kind, "{name}");
        }
    }

    #[test]
    fn entry_and_revision_limits_are_enforced() {
        let mut b = Builder::new();
        b.object(1, "<< >>");
        b.object(2, "<< >>");
        b.object(3, "<< >>");
        b.section(true, "");
        let limits = Limits {
            max_xref_entries: 3,
            ..Limits::default()
        };
        // 4 entries (head + 3 subsections of 1) exceed 3; the declared count is checked up front.
        let err = Xref::parse(&b.data, &limits).unwrap_err();
        assert!(
            matches!(
                err,
                Error::LimitExceeded {
                    limit: LimitKind::XrefEntries,
                    ..
                }
            ),
            "{err:?}"
        );
        // A huge declared count fails before anything is allocated for it.
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        data.extend(b"xref\n0 4000000000\n");
        data.extend(format!("trailer\n<< >>\nstartxref\n{at}\n%%EOF\n").bytes());
        let err = Xref::parse(&data, &Limits::default()).unwrap_err();
        assert!(
            matches!(
                err,
                Error::LimitExceeded {
                    limit: LimitKind::XrefEntries,
                    value: 4_000_000_000,
                    ..
                }
            ),
            "{err:?}"
        );

        // The entry limit covers the whole chain, not each section.
        let mut b = Builder::new();
        b.object(1, "<< >>");
        b.object(2, "<< >>");
        let first = b.section(true, "");
        b.object(3, "<< >>");
        b.object(4, "<< >>");
        b.section(false, &format!("/Prev {first} "));
        let tight = Limits {
            max_xref_entries: 4,
            ..Limits::default()
        };
        assert!(matches!(
            Xref::parse(&b.data, &tight).unwrap_err(),
            Error::LimitExceeded {
                limit: LimitKind::XrefEntries,
                ..
            }
        ));
        let enough = Limits {
            max_xref_entries: 5,
            ..Limits::default()
        };
        assert_eq!(Xref::parse(&b.data, &enough).unwrap().revisions.len(), 2);

        // Revision count.
        let few = Limits {
            max_revisions: 1,
            ..Limits::default()
        };
        assert!(matches!(
            Xref::parse(&b.data, &few).unwrap_err(),
            Error::LimitExceeded {
                limit: LimitKind::Revisions,
                max: 1,
                value: 2,
                ..
            }
        ));
    }

    #[test]
    fn junk_before_the_header_shifts_offsets_by_the_header_position() {
        let mut b = Builder::new();
        b.object(1, "<< /Type /Catalog >>");
        b.section(true, "");
        let junk = b"GARBAGE\r\n".to_vec();
        let mut data = junk.clone();
        data.extend(&b.data);
        let xref = parse(&data);
        assert_eq!(xref.base, junk.len());
        assert_eq!(xref.in_use_count(), 1);
        // The entry offset stays relative to the header; the caller adds `base`.
        let (_, entry) = xref.revisions[0].section.entries[1];
        let XrefEntry::InUse { offset, .. } = entry else {
            panic!()
        };
        assert_eq!(
            &data[xref.base + usize::try_from(offset).unwrap()..][..7],
            b"1 0 obj"
        );
    }

    #[test]
    fn a_missing_eof_marker_still_gives_a_revision_range() {
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        data.extend(
            format!("xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 1 >>\nstartxref\n{at}")
                .bytes(),
        );
        let xref = parse(&data);
        assert_eq!(xref.revisions[0].bytes, 0..data.len());
    }

    #[test]
    fn empty_and_tiny_inputs_do_not_panic() {
        for data in [&b""[..], b"%PDF-", b"xref", b"startxref\n0"] {
            assert!(Xref::parse(data, &Limits::default()).is_err());
        }
    }

    mod props {
        use proptest::prelude::*;

        use super::*;

        proptest! {
            /// Arbitrary bytes never panic.
            #[test]
            fn arbitrary_bytes_never_panic(data in proptest::collection::vec(any::<u8>(), 0..600)) {
                let _ = Xref::parse(&data, &Limits::default());
            }

            /// Mutating a valid file in one place never panics, and any Ok result is internally
            /// consistent (ranges inside the file, ordered, tiling).
            #[test]
            fn mutations_of_a_valid_file_are_safe(pos in 0usize..400, byte in any::<u8>(), cut in 0usize..400) {
                let mut b = Builder::new();
                b.object(1, "<< /Type /Catalog >>");
                b.object(2, "<< >>");
                let first = b.section(true, "");
                b.object(3, "<< >>");
                b.section(false, &format!("/Prev {first} "));
                let mut data = b.data.clone();
                if let Some(slot) = data.get_mut(pos) {
                    *slot = byte;
                }
                data.truncate(data.len().saturating_sub(cut % 40));
                if let Ok(xref) = Xref::parse(&data, &Limits::default()) {
                    let mut expected_start = 0;
                    for rev in xref.revisions.iter().rev() {
                        prop_assert_eq!(rev.bytes.start, expected_start);
                        prop_assert!(rev.bytes.end >= rev.bytes.start);
                        prop_assert!(rev.bytes.end <= data.len());
                        prop_assert!(rev.section.span.end <= data.len());
                        expected_start = rev.bytes.end;
                    }
                }
            }
        }
    }
}
