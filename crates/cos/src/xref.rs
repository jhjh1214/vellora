//! Cross-reference tables and streams, trailers and the `/Prev` chain (ISO 32000-2:2020
//! §7.5.4–7.5.8).
//!
//! [`Xref::parse`] finds `startxref` near the end of the file, reads the cross-reference section
//! it points to (a classic table or a cross-reference stream) and follows `/Prev` back through
//! older sections, producing one [`Revision`] per section. Hybrid-reference files (§7.5.8.4) are
//! read too: the table's `/XRefStm` stream is stored with the same revision in
//! [`XrefSection::stream_entries`] and ranks between the table and `/Prev`.
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

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::error::{Error, Result, SyntaxKind};
use crate::filter::decode_flate_only;
use crate::lexer::{Lexer, Token, TokenKind};
use crate::limits::{LimitKind, Limits};
use crate::object::{Dict, ObjectKind, Recovery};
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
    /// An object stored inside an object stream (§7.5.7); only cross-reference streams can say
    /// so. Its generation number is always 0.
    Compressed {
        /// Object number of the object stream. Not validated.
        stream: u32,
        /// Index of the object inside that stream. Not validated.
        index: u32,
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
    /// A free entry had a generation number over 65535 (writers that emit 65536 for the free
    /// list head); it was clamped to 65535.
    FreeGenerationClamped,
    /// A cross-reference stream holds fewer rows than its `/Index` and `/Size` promise.
    FewerRowsThanDeclared,
    /// The cross-reference stream object itself needed a repair (for example a wrong
    /// `/Length`).
    StreamObjectRecovered(Recovery),
}

/// Whether a section is a classic table or a cross-reference stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    /// `xref ... trailer << ... >>` (§7.5.4).
    Table,
    /// An indirect stream object with `/Type /XRef` (§7.5.8).
    Stream,
}

/// One cross-reference section with its trailer.
#[derive(Debug, Clone, PartialEq)]
pub struct XrefSection<'a> {
    /// Table or stream.
    pub kind: SectionKind,
    /// Offset of the `xref` keyword, or of the `n g obj` header of a cross-reference stream.
    pub offset: usize,
    /// Entries as `(object number, entry)` in file order.
    pub entries: Vec<(u32, XrefEntry)>,
    /// Entries of the `/XRefStm` stream of a hybrid-reference file (§7.5.8.4), else empty. For
    /// an object number both lists have, the table's in-use entry wins; the table's free entry
    /// (how hybrid files hide compressed objects from old readers) yields to a stream entry that
    /// is not free.
    pub stream_entries: Vec<(u32, XrefEntry)>,
    /// The trailer dictionary (for a stream, the stream dictionary).
    pub trailer: Dict<'a>,
    /// Bytes from `xref` through the end of the trailer dictionary, or the whole stream object.
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
        kind: SectionKind::Table,
        offset: start,
        entries,
        stream_entries: Vec::new(),
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
        let Ok(offset) = u64::try_from(offset) else {
            return Err(malformed(start));
        };
        // Some writers emit generation 65536 for the free list head instead of 65535. For a free
        // entry the generation only matters if the number is reused, so clamp it and say so; an
        // in-use object with such a generation cannot be referenced and stays an error.
        let generation = match u16::try_from(generation) {
            Ok(generation) => generation,
            Err(_) if !in_use && generation > 0 => {
                warnings.push(XrefWarning::FreeGenerationClamped);
                u16::MAX
            }
            Err(_) => return Err(malformed(start)),
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
    /// Reads the whole chain of cross-reference sections of `data`.
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
            let mut section = parse_section(data, pos, limits, total_entries)?;
            total_entries += section.entries.len() as u64;
            // Hybrid-reference file: the table names a cross-reference stream (§7.5.8.4).
            if section.kind == SectionKind::Table
                && let Some(ObjectKind::Integer(stm)) =
                    section.trailer.get(b"XRefStm").map(|o| &o.kind)
            {
                let origin = section.offset as u64;
                let stm_pos = u64::try_from(*stm)
                    .map_err(|_| invalid(origin))
                    .and_then(|stm| locate(stm, origin))?;
                let hybrid = parse_xref_stream_section(data, stm_pos, limits, total_entries)?;
                total_entries += hybrid.entries.len() as u64;
                section.stream_entries = hybrid.entries;
                section.warnings.extend(hybrid.warnings);
            }
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

    /// The entry that applies to each object number: newest revision first, and inside a hybrid
    /// revision the table's in-use entry, then the stream's entry, then the table's free entry.
    #[must_use]
    pub fn merged(&self) -> HashMap<u32, XrefEntry> {
        let mut merged: HashMap<u32, XrefEntry> = HashMap::new();
        for revision in &self.revisions {
            let section = &revision.section;
            let mut from_stream: HashMap<u32, XrefEntry> = HashMap::new();
            for (number, entry) in &section.stream_entries {
                from_stream.entry(*number).or_insert(*entry);
            }
            for (number, entry) in &section.entries {
                // The table's free entry hides a compressed object from old readers, so a
                // stream entry that says more than "free" replaces it.
                let chosen = match (entry, from_stream.get(number)) {
                    (XrefEntry::Free { .. }, Some(stream_entry))
                        if !matches!(stream_entry, XrefEntry::Free { .. }) =>
                    {
                        *stream_entry
                    }
                    _ => *entry,
                };
                merged.entry(*number).or_insert(chosen);
            }
            for (number, entry) in from_stream {
                merged.entry(number).or_insert(entry);
            }
        }
        merged
    }

    /// How many objects exist, compressed ones included: each object number is looked up as in
    /// [`merged`](Self::merged), so an object freed by a later revision does not count.
    #[must_use]
    pub fn in_use_count(&self) -> usize {
        self.merged()
            .values()
            .filter(|e| !matches!(e, XrefEntry::Free { .. }))
            .count()
    }
}

/// Reads the cross-reference section at `pos`: a classic table if the first token is `xref`,
/// otherwise a cross-reference stream.
///
/// # Errors
/// As [`parse_xref_section`] and [`parse_xref_stream_section`].
pub fn parse_section<'a>(
    data: &'a [u8],
    pos: usize,
    limits: &Limits,
    entries_so_far: u64,
) -> Result<XrefSection<'a>> {
    let mut lexer = Lexer::at(data, pos, limits);
    if let Ok(Some(t)) = lexer.next_token()
        && t.kind == TokenKind::Keyword(b"xref")
    {
        return parse_xref_section(data, pos, limits, entries_so_far);
    }
    parse_xref_stream_section(data, pos, limits, entries_so_far)
}

/// Parses a cross-reference stream (§7.5.8) at `pos`: the indirect object, its `/W`, `/Index` and
/// `/Size`, and its entries. Only no filter and `FlateDecode` (with predictors) are supported
/// until the full filter chain exists (M0 task 10).
///
/// # Errors
/// [`SyntaxKind::ExpectedXrefKeyword`] if there is no cross-reference stream at `pos`,
/// [`SyntaxKind::MalformedXrefSection`] for a bad `/W`, `/Index` or `/Size` or an unusable
/// row, [`Error::LimitExceeded`] for too many entries or output, [`Error::Decode`] if the data
/// cannot be decoded.
pub fn parse_xref_stream_section<'a>(
    data: &'a [u8],
    pos: usize,
    limits: &Limits,
    entries_so_far: u64,
) -> Result<XrefSection<'a>> {
    let not_a_section = || syntax(SyntaxKind::ExpectedXrefKeyword, pos);
    let malformed = || syntax(SyntaxKind::MalformedXrefSection, pos);

    let indirect = Parser::at(data, pos, limits)
        .parse_indirect_object()
        .map_err(|e| match e {
            Error::Syntax { .. } => not_a_section(),
            other => other,
        })?;
    let span = indirect.span.clone();
    let recoveries = indirect.recoveries;
    let ObjectKind::Stream(stream) = indirect.object.kind else {
        return Err(not_a_section());
    };
    let dict = stream.dict;
    let is_xref_type = matches!(dict.get(b"Type").map(|o| &o.kind), Some(ObjectKind::Name(n)) if n.as_ref() == b"XRef");
    if !is_xref_type {
        return Err(not_a_section());
    }

    let widths = xref_widths(&dict).ok_or_else(malformed)?;
    let row_len: usize = widths.iter().sum();
    if row_len == 0 {
        return Err(malformed());
    }
    let size = match dict.get(b"Size").map(|o| &o.kind) {
        Some(ObjectKind::Integer(n)) => u64::try_from(*n).map_err(|_| malformed())?,
        _ => return Err(malformed()),
    };
    let ranges = xref_index(&dict, size).ok_or_else(malformed)?;
    let total = ranges
        .iter()
        .try_fold(0u64, |acc, &(_, count)| acc.checked_add(count))
        .ok_or_else(malformed)?;
    limits.check(
        LimitKind::XrefEntries,
        entries_so_far.saturating_add(total),
        Some(pos as u64),
    )?;

    let raw = data.get(stream.data.clone()).unwrap_or_default();
    let decoded = decode_flate_only(&dict, raw, limits, None, Some(stream.data.start as u64))?;

    let mut warnings: Vec<XrefWarning> = recoveries
        .into_iter()
        .map(XrefWarning::StreamObjectRecovered)
        .collect();
    let available = (decoded.len() / row_len) as u64;
    if available < total {
        warnings.push(XrefWarning::FewerRowsThanDeclared);
    }
    let mut entries = Vec::with_capacity(usize::try_from(total.min(available)).unwrap_or(0));
    let mut rows = decoded.chunks_exact(row_len);
    'ranges: for (first, count) in ranges {
        for k in 0..count {
            let Some(row) = rows.next() else {
                break 'ranges;
            };
            let number = first
                .checked_add(k)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(malformed)?;
            if let Row::Entry(entry) = xref_stream_entry(row, widths).ok_or_else(malformed)? {
                entries.push((number, entry));
            }
        }
    }

    Ok(XrefSection {
        kind: SectionKind::Stream,
        offset: pos,
        entries,
        stream_entries: Vec::new(),
        trailer: dict,
        span,
        warnings,
    })
}

/// `/W [a b c]`: the byte widths of the three fields of a row (§7.5.8.2). Each is 0..=8.
fn xref_widths(dict: &Dict<'_>) -> Option<[usize; 3]> {
    let ObjectKind::Array(items) = &dict.get(b"W")?.kind else {
        return None;
    };
    if items.len() != 3 {
        return None;
    }
    let mut widths = [0usize; 3];
    for (slot, item) in widths.iter_mut().zip(items) {
        let width = usize::try_from(item.as_integer()?).ok()?;
        if width > 8 {
            return None;
        }
        *slot = width;
    }
    Some(widths)
}

/// `/Index [first count ...]`, defaulting to `[0 Size]`.
fn xref_index(dict: &Dict<'_>, size: u64) -> Option<Vec<(u64, u64)>> {
    let Some(index) = dict.get(b"Index") else {
        return Some(vec![(0, size)]);
    };
    let ObjectKind::Array(items) = &index.kind else {
        return None;
    };
    if items.len() % 2 != 0 {
        return None;
    }
    let (pairs, _) = items.as_chunks::<2>();
    pairs
        .iter()
        .map(|[first, count]| {
            let first = u64::try_from(first.as_integer()?).ok()?;
            let count = u64::try_from(count.as_integer()?).ok()?;
            Some((first, count))
        })
        .collect()
}

/// What one row of a cross-reference stream says.
enum Row {
    /// A usable entry.
    Entry(XrefEntry),
    /// An entry type the spec says to treat as a reference to the null object.
    NullObject,
}

/// Decodes one row; `None` if its numbers do not fit their types.
fn xref_stream_entry(row: &[u8], widths: [usize; 3]) -> Option<Row> {
    let field = |start: usize, width: usize| -> u64 {
        row.get(start..start + width)
            .unwrap_or_default()
            .iter()
            .fold(0u64, |acc, &b| (acc << 8) | u64::from(b))
    };
    let [w0, w1, w2] = widths;
    // A zero-width type field means type 1 (§7.5.8.2).
    let kind = if w0 == 0 { 1 } else { field(0, w0) };
    let second = field(w0, w1);
    let third = field(w0 + w1, w2);
    let generation = u16::try_from(third).unwrap_or(u16::MAX);
    Some(match kind {
        0 => Row::Entry(XrefEntry::Free {
            next_free: second,
            generation,
        }),
        1 => Row::Entry(XrefEntry::InUse {
            offset: second,
            generation,
        }),
        2 => Row::Entry(XrefEntry::Compressed {
            stream: u32::try_from(second).ok()?,
            index: u32::try_from(third).ok()?,
        }),
        _ => Row::NullObject,
    })
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
    fn free_entry_generation_over_65535_is_clamped_and_reported() {
        // Seen in the wild (pdf.js ContentStreamCycleType3insideType3): `65536 f` for the head.
        let mut data = b"%PDF-1.7
"
        .to_vec();
        let at = data.len();
        data.extend(
            b"xref
0 2
0000000000 65536 f 
0000000009 00000 n 
",
        );
        data.extend(
            format!(
                "trailer
<< /Size 2 >>
startxref
{at}
%%EOF
"
            )
            .bytes(),
        );
        let xref = parse(&data);
        let section = &xref.revisions[0].section;
        assert_eq!(
            section.entries[0].1,
            XrefEntry::Free {
                next_free: 0,
                generation: 65535
            }
        );
        assert_eq!(section.warnings, [XrefWarning::FreeGenerationClamped]);
        assert_eq!(xref.in_use_count(), 1);
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

    // ---- cross-reference streams, hybrid files (M0 task 7) ----

    const W: [usize; 3] = [1, 2, 1];

    /// One row of a `[1 2 1]` cross-reference stream.
    fn row(kind: u8, second: u16, third: u8) -> Vec<u8> {
        let mut r = vec![kind];
        r.extend(second.to_be_bytes());
        r.push(third);
        r
    }

    /// PNG "Up" predictor encoding (tag 2) of fixed-length rows.
    fn png_up(rows: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut prev = vec![0u8; rows.first().map_or(0, Vec::len)];
        for r in rows {
            out.push(2);
            out.extend(r.iter().zip(&prev).map(|(a, b)| a.wrapping_sub(*b)));
            prev.clone_from(r);
        }
        out
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    struct StreamSpec<'s> {
        num: u32,
        size: u32,
        w: [usize; 3],
        rows: Vec<Vec<u8>>,
        flate: bool,
        extra: &'s str,
    }

    impl StreamSpec<'_> {
        fn new(num: u32, size: u32, rows: Vec<Vec<u8>>) -> Self {
            Self {
                num,
                size,
                w: W,
                rows,
                flate: false,
                extra: "",
            }
        }

        fn object(&self) -> Vec<u8> {
            let row_len: usize = self.w.iter().sum();
            let body = if self.flate {
                zlib(&png_up(&self.rows))
            } else {
                self.rows.concat()
            };
            let filter = if self.flate {
                format!("/Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns {row_len} >>")
            } else {
                String::new()
            };
            let mut out = format!(
                "{} 0 obj\n<< /Type /XRef /Size {} /W [{} {} {}] /Root 1 0 R {} /Length {} {} >>\nstream\n",
                self.num, self.size, self.w[0], self.w[1], self.w[2], self.extra, body.len(), filter
            )
            .into_bytes();
            out.extend(body);
            out.extend(b"\nendstream\nendobj\n");
            out
        }
    }

    /// Appends the stream object at the end of `data` followed by `startxref`; returns its offset.
    fn append_stream(data: &mut Vec<u8>, spec: &StreamSpec<'_>) -> usize {
        let at = data.len();
        data.extend(spec.object());
        data.extend(format!("startxref\n{at}\n%%EOF\n").bytes());
        at
    }

    #[test]
    fn xref_stream_entries_of_all_three_types() {
        let mut data = b"%PDF-1.7\n".to_vec();
        let rows = vec![row(0, 0, 255), row(1, 1234, 0), row(2, 7, 3), row(1, 40, 2)];
        let at = append_stream(&mut data, &StreamSpec::new(5, 4, rows));
        let xref = parse(&data);
        assert_eq!(xref.revisions.len(), 1);
        let section = &xref.revisions[0].section;
        assert_eq!(section.kind, SectionKind::Stream);
        assert_eq!(section.offset, at);
        assert_eq!(
            section.entries,
            [
                (
                    0,
                    XrefEntry::Free {
                        next_free: 0,
                        generation: 255
                    }
                ),
                (
                    1,
                    XrefEntry::InUse {
                        offset: 1234,
                        generation: 0
                    }
                ),
                (
                    2,
                    XrefEntry::Compressed {
                        stream: 7,
                        index: 3
                    }
                ),
                (
                    3,
                    XrefEntry::InUse {
                        offset: 40,
                        generation: 2
                    }
                ),
            ]
        );
        assert_eq!(section.warnings, Vec::new());
        assert_eq!(xref.in_use_count(), 3);
        // The stream dictionary serves as the trailer.
        assert_eq!(
            xref.trailer().unwrap().get(b"Size").unwrap().as_integer(),
            Some(4)
        );
        assert!(xref.trailer().unwrap().get(b"Root").is_some());
        // The revision covers everything through %%EOF.
        assert_eq!(xref.revisions[0].bytes, 0..data.len() - 1);
        assert_eq!(&data[section.span.clone()][..5], b"5 0 o");
    }

    #[test]
    fn xref_stream_with_flate_and_png_predictor() {
        let mut data = b"%PDF-1.7\n".to_vec();
        let rows: Vec<Vec<u8>> = (0..50u16)
            .map(|i| {
                if i % 3 == 0 {
                    row(2, 9, u8::try_from(i).unwrap())
                } else {
                    row(1, 100 + i * 20, 0)
                }
            })
            .collect();
        let mut spec = StreamSpec::new(7, 50, rows);
        spec.flate = true;
        append_stream(&mut data, &spec);
        let xref = parse(&data);
        let entries = &xref.revisions[0].section.entries;
        assert_eq!(entries.len(), 50);
        assert_eq!(
            entries[0].1,
            XrefEntry::Compressed {
                stream: 9,
                index: 0
            }
        );
        assert_eq!(
            entries[1].1,
            XrefEntry::InUse {
                offset: 120,
                generation: 0
            }
        );
        assert_eq!(
            entries[48].1,
            XrefEntry::Compressed {
                stream: 9,
                index: 48
            }
        );
        assert_eq!(xref.in_use_count(), 50);
    }

    #[test]
    fn xref_stream_index_ranges_and_defaults() {
        let mut data = b"%PDF-1.7\n".to_vec();
        let mut spec = StreamSpec::new(3, 20, vec![row(1, 10, 0), row(1, 20, 0), row(1, 30, 0)]);
        spec.extra = "/Index [4 2 15 1]";
        append_stream(&mut data, &spec);
        let xref = parse(&data);
        let numbers: Vec<u32> = xref.revisions[0]
            .section
            .entries
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(numbers, [4, 5, 15]);

        // Zero-width type field: every row is type 1. Zero-width generation field: generation 0.
        let mut data = b"%PDF-1.7\n".to_vec();
        let mut spec = StreamSpec::new(3, 2, vec![vec![0, 50], vec![1, 0]]);
        spec.w = [0, 2, 0];
        append_stream(&mut data, &spec);
        let xref = parse(&data);
        assert_eq!(
            xref.revisions[0].section.entries,
            [
                (
                    0,
                    XrefEntry::InUse {
                        offset: 50,
                        generation: 0
                    }
                ),
                (
                    1,
                    XrefEntry::InUse {
                        offset: 256,
                        generation: 0
                    }
                ),
            ]
        );
    }

    #[test]
    fn xref_stream_wide_fields_and_unknown_types() {
        let mut data = b"%PDF-1.7\n".to_vec();
        let mut spec = StreamSpec::new(3, 3, Vec::new());
        spec.w = [1, 8, 2];
        let big: u64 = 0x0102_0304_0506_0708;
        let mut r0 = vec![1u8];
        r0.extend(big.to_be_bytes());
        r0.extend([0xFF, 0xFF]);
        // Type 3 is not defined: the object is treated as null and gets no entry.
        let mut r1 = vec![3u8];
        r1.extend([0u8; 10]);
        let mut r2 = vec![1u8];
        r2.extend(5u64.to_be_bytes());
        r2.extend([0x01, 0x00]); // generation 256 fits; 70000 would not (clamped, below)
        spec.rows = vec![r0, r1, r2];
        append_stream(&mut data, &spec);
        let xref = parse(&data);
        assert_eq!(
            xref.revisions[0].section.entries,
            [
                (
                    0,
                    XrefEntry::InUse {
                        offset: big,
                        generation: u16::MAX
                    }
                ),
                (
                    2,
                    XrefEntry::InUse {
                        offset: 5,
                        generation: 256
                    }
                ),
            ]
        );
    }

    #[test]
    fn malformed_xref_streams_are_typed_errors() {
        let build = |extra_dict: &str, w: &str, rows: &[u8]| {
            let mut data = b"%PDF-1.7\n".to_vec();
            let at = data.len();
            data.extend(
                format!(
                    "9 0 obj\n<< /Type /XRef {extra_dict} /W {w} /Length {} >>\nstream\n",
                    rows.len()
                )
                .bytes(),
            );
            data.extend(rows);
            data.extend(format!("\nendstream\nendobj\nstartxref\n{at}\n%%EOF\n").bytes());
            data
        };
        let row_bytes = row(1, 5, 0);
        for (name, extra, w) in [
            ("no /Size", "", "[1 2 1]"),
            ("W too short", "/Size 1", "[1 2]"),
            ("W too long", "/Size 1", "[1 2 1 1]"),
            ("W entry over 8", "/Size 1", "[1 9 1]"),
            ("W negative", "/Size 1", "[1 -2 1]"),
            ("W all zero", "/Size 1", "[0 0 0]"),
            ("W not an array", "/Size 1", "5"),
            ("odd /Index", "/Size 1 /Index [0 1 5]", "[1 2 1]"),
            ("negative /Index", "/Size 1 /Index [-1 1]", "[1 2 1]"),
            ("negative /Size", "/Size -1", "[1 2 1]"),
        ] {
            let data = build(extra, w, &row_bytes);
            assert_eq!(
                error_kind(&data),
                SyntaxKind::MalformedXrefSection,
                "{name}"
            );
        }
        // Not an xref stream: a different /Type, or no stream at all.
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        data.extend(
            b"9 0 obj\n<< /Type /ObjStm /N 1 /First 4 /Length 0 >>\nstream\n\nendstream\nendobj\n",
        );
        data.extend(format!("startxref\n{at}\n%%EOF\n").bytes());
        assert_eq!(error_kind(&data), SyntaxKind::ExpectedXrefKeyword);
        // An entry that does not fit its type (type 2 with a stream number over u32).
        let mut data = b"%PDF-1.7\n".to_vec();
        let mut spec = StreamSpec::new(3, 1, Vec::new());
        spec.w = [1, 8, 1];
        let mut bad = vec![2u8];
        bad.extend(u64::MAX.to_be_bytes());
        bad.push(0);
        spec.rows = vec![bad];
        append_stream(&mut data, &spec);
        assert_eq!(error_kind(&data), SyntaxKind::MalformedXrefSection);
        // Object numbers that overflow u32.
        let mut data = b"%PDF-1.7\n".to_vec();
        let mut spec = StreamSpec::new(3, 2, vec![row(1, 1, 0), row(1, 2, 0)]);
        spec.extra = "/Index [4294967295 2]";
        append_stream(&mut data, &spec);
        assert_eq!(error_kind(&data), SyntaxKind::MalformedXrefSection);
    }

    #[test]
    fn fewer_rows_than_declared_is_reported_not_fatal() {
        let mut data = b"%PDF-1.7\n".to_vec();
        append_stream(
            &mut data,
            &StreamSpec::new(3, 5, vec![row(1, 10, 0), row(1, 20, 0)]),
        );
        let xref = parse(&data);
        let section = &xref.revisions[0].section;
        assert_eq!(section.entries.len(), 2);
        assert_eq!(section.warnings, [XrefWarning::FewerRowsThanDeclared]);
    }

    #[test]
    fn a_wrong_stream_length_is_recovered_and_reported() {
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        let body = row(1, 77, 0);
        data.extend(b"3 0 obj\n<< /Type /XRef /Size 1 /W [1 2 1] /Length 999 >>\nstream\n");
        data.extend(&body);
        data.extend(b"\nendstream\nendobj\n");
        data.extend(format!("startxref\n{at}\n%%EOF\n").bytes());
        let xref = parse(&data);
        let section = &xref.revisions[0].section;
        assert_eq!(
            section.entries,
            [(
                0,
                XrefEntry::InUse {
                    offset: 77,
                    generation: 0
                }
            )]
        );
        assert_eq!(
            section.warnings,
            [XrefWarning::StreamObjectRecovered(
                Recovery::StreamLengthWrong { declared: 999 }
            )]
        );
    }

    #[test]
    fn xref_stream_limits_are_enforced_before_decoding() {
        // /Size and /Index promise far more entries than any real file; nothing is allocated.
        let mut data = b"%PDF-1.7\n".to_vec();
        let mut spec = StreamSpec::new(3, 1, vec![row(1, 1, 0)]);
        spec.extra = "/Index [0 99999999999]";
        append_stream(&mut data, &spec);
        assert!(matches!(
            Xref::parse(&data, &Limits::default()).unwrap_err(),
            Error::LimitExceeded {
                limit: LimitKind::XrefEntries,
                value: 99_999_999_999,
                ..
            }
        ));
        // Index counts that overflow u64 when added are malformed, not a wrap-around.
        let mut data = b"%PDF-1.7\n".to_vec();
        let mut spec = StreamSpec::new(3, 1, vec![row(1, 1, 0)]);
        spec.extra = "/Index [0 9223372036854775807 0 9223372036854775807 0 9223372036854775807]";
        append_stream(&mut data, &spec);
        assert!(Xref::parse(&data, &Limits::default()).is_err());
        // A Flate bomb in the stream hits the decode limits.
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        let bomb = zlib(&vec![0u8; 8 * 1024 * 1024]);
        data.extend(
            format!(
                "3 0 obj\n<< /Type /XRef /Size 1 /W [1 2 1] /Filter /FlateDecode /Length {} >>\nstream\n",
                bomb.len()
            )
            .bytes(),
        );
        data.extend(&bomb);
        data.extend(b"\nendstream\nendobj\n");
        data.extend(format!("startxref\n{at}\n%%EOF\n").bytes());
        assert!(matches!(
            Xref::parse(&data, &Limits::default()).unwrap_err(),
            Error::LimitExceeded {
                limit: LimitKind::DecompressionRatio,
                ..
            }
        ));
        // Unsupported filters are a typed decode error (until task 10).
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        data.extend(b"3 0 obj\n<< /Type /XRef /Size 1 /W [1 2 1] /Filter /LZWDecode /Length 4 >>\nstream\nabcd\nendstream\nendobj\n");
        data.extend(format!("startxref\n{at}\n%%EOF\n").bytes());
        assert!(matches!(
            Xref::parse(&data, &Limits::default()).unwrap_err(),
            Error::Decode { .. }
        ));
    }

    #[test]
    fn a_stream_section_can_chain_to_a_classic_table_and_back() {
        // Classic table (oldest), then an incremental update written as an xref stream.
        let mut b = Builder::new();
        b.object(1, "<< /Type /Catalog >>");
        b.object(2, "<< >>");
        let first = b.section(true, "");
        let mut data = b.data.clone();
        let mut spec = StreamSpec::new(10, 4, vec![row(1, 500, 0), row(2, 8, 1)]);
        let extra = format!("/Index [3 2] /Prev {first}");
        spec.extra = &extra;
        let at = append_stream(&mut data, &spec);
        let xref = parse(&data);
        assert_eq!(xref.revisions.len(), 2);
        assert_eq!(xref.revisions[0].section.kind, SectionKind::Stream);
        assert_eq!(xref.revisions[0].section.offset, at);
        assert_eq!(xref.revisions[1].section.kind, SectionKind::Table);
        // 1, 2 from the table, 3 and 4 (one compressed) from the stream.
        assert_eq!(xref.in_use_count(), 4);
        // Ranges tile the file.
        assert_eq!(xref.revisions[1].bytes.end, xref.revisions[0].bytes.start);
        assert_eq!(xref.revisions[0].bytes.end, data.len() - 1);
    }

    #[test]
    fn a_prev_loop_through_stream_sections_is_detected() {
        let mut data = b"%PDF-1.7\n".to_vec();
        let at = data.len();
        let mut spec = StreamSpec::new(3, 1, vec![row(1, 1, 0)]);
        let prev = format!("/Prev {at}");
        spec.extra = &prev;
        append_stream(&mut data, &spec);
        assert_eq!(error_kind(&data), SyntaxKind::XrefPrevLoop);
    }

    #[test]
    fn hybrid_file_merges_table_and_stream_with_the_right_precedence() {
        // Objects: 1 and 2 are plain; 3 and 4 live in object stream 2 and are hidden from old
        // readers as free entries in the table; the table also lists 5 in use, which the stream
        // contradicts (the table wins).
        let mut data = b"%PDF-1.7\n".to_vec();
        let stm_at = data.len();
        data.extend(
            StreamSpec::new(
                9,
                6,
                vec![
                    row(0, 0, 0),
                    row(0, 0, 0),
                    row(0, 0, 0),
                    row(2, 2, 0),
                    row(2, 2, 1),
                    row(2, 2, 2),
                ],
            )
            .object(),
        );
        let table_at = data.len();
        data.extend(
            format!(
                "xref\n0 6\n0000000000 65535 f \n0000000009 00000 n \n0000000100 00000 n \n\
                 0000000000 00000 f \n0000000000 00000 f \n0000000300 00000 n \n\
                 trailer\n<< /Size 6 /Root 1 0 R /XRefStm {stm_at} >>\nstartxref\n{table_at}\n%%EOF\n"
            )
            .bytes(),
        );
        let xref = parse(&data);
        assert_eq!(xref.revisions.len(), 1);
        let section = &xref.revisions[0].section;
        assert_eq!(section.kind, SectionKind::Table);
        assert_eq!(section.stream_entries.len(), 6);

        let merged = xref.merged();
        assert_eq!(
            merged[&0],
            XrefEntry::Free {
                next_free: 0,
                generation: 65535
            }
        );
        assert_eq!(
            merged[&1],
            XrefEntry::InUse {
                offset: 9,
                generation: 0
            }
        );
        // Free in the table, compressed in the stream: the stream's entry is used.
        assert_eq!(
            merged[&3],
            XrefEntry::Compressed {
                stream: 2,
                index: 0
            }
        );
        assert_eq!(
            merged[&4],
            XrefEntry::Compressed {
                stream: 2,
                index: 1
            }
        );
        // In use in the table, compressed in the stream: the table wins.
        assert_eq!(
            merged[&5],
            XrefEntry::InUse {
                offset: 300,
                generation: 0
            }
        );
        assert_eq!(xref.in_use_count(), 5);
    }

    #[test]
    fn hybrid_older_revision_is_shadowed_by_the_newer_hybrid_part() {
        // Older table lists object 2 in use; a newer hybrid revision's stream frees it.
        let mut b = Builder::new();
        b.object(1, "<< >>");
        b.object(2, "<< >>");
        let first = b.section(true, "");
        let mut data = b.data.clone();
        let stm_at = data.len();
        data.extend(StreamSpec::new(9, 3, vec![row(0, 0, 1)]).object_with_index("/Index [2 1]"));
        let table_at = data.len();
        data.extend(
            format!(
                "xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 3 /Prev {first} /XRefStm {stm_at} >>\n\
                 startxref\n{table_at}\n%%EOF\n"
            )
            .bytes(),
        );
        let xref = parse(&data);
        assert_eq!(xref.revisions.len(), 2);
        assert_eq!(xref.in_use_count(), 1);
    }

    #[test]
    fn broken_xrefstm_targets_are_errors() {
        let mut b = Builder::new();
        b.object(1, "<< >>");
        // XRefStm outside the file, negative, and pointing at a non-stream.
        for stm in ["99999999", "-4", "9"] {
            let mut data = b.data.clone();
            let table_at = data.len();
            data.extend(
                format!(
                    "xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 3 /XRefStm {stm} >>\nstartxref\n{table_at}\n%%EOF\n"
                )
                .bytes(),
            );
            assert!(Xref::parse(&data, &Limits::default()).is_err(), "{stm}");
        }
    }

    mod stream_props {
        use proptest::prelude::*;

        use super::*;

        proptest! {
            /// Arbitrary rows in an xref stream never panic; any Ok result stays in bounds.
            #[test]
            fn arbitrary_xref_stream_rows_are_safe(
                rows in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 4), 0..20),
                size in 0u32..40,
                flate in any::<bool>(),
            ) {
                let mut data = b"%PDF-1.7\n".to_vec();
                let mut spec = StreamSpec::new(3, size, rows);
                spec.flate = flate;
                append_stream(&mut data, &spec);
                if let Ok(xref) = Xref::parse(&data, &Limits::default()) {
                    for rev in &xref.revisions {
                        prop_assert!(rev.bytes.end <= data.len());
                        prop_assert!(rev.section.entries.len() <= 20);
                    }
                }
            }

            /// Mutating bytes of a valid stream file never panics.
            #[test]
            fn mutated_xref_stream_files_never_panic(pos in 0usize..300, byte in any::<u8>()) {
                let mut data = b"%PDF-1.7\n".to_vec();
                let mut spec = StreamSpec::new(3, 4, vec![row(1, 10, 0), row(2, 5, 1), row(1, 30, 0), row(0, 0, 0)]);
                spec.flate = true;
                append_stream(&mut data, &spec);
                if let Some(slot) = data.get_mut(pos) {
                    *slot = byte;
                }
                let _ = Xref::parse(&data, &Limits::default());
            }
        }
    }

    impl StreamSpec<'_> {
        /// Like [`StreamSpec::object`] with an extra dictionary fragment, for `/Index`.
        fn object_with_index(&self, index: &str) -> Vec<u8> {
            let spec = StreamSpec {
                num: self.num,
                size: self.size,
                w: self.w,
                rows: self.rows.clone(),
                flate: self.flate,
                extra: index,
            };
            spec.object()
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
