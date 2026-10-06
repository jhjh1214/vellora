//! Recovery scan: rebuilding the cross-reference information of a damaged file.
//!
//! When `startxref` or the cross-reference data is missing or wrong, [`rebuild`] scans the file
//! for `n g obj` headers and `trailer` dictionaries and builds a single synthetic revision from
//! what it finds. The result is an ordinary [`Xref`] whose [`Xref::repaired`] lists why the file
//! counts as repaired, so the UI can say so and a save can choose a full rewrite.
//!
//! How the scan works:
//! - Objects are parsed one after another and the scan resumes **after** each parsed object, so
//!   text that merely looks like `1 0 obj` inside stream data is not mistaken for an object.
//!   An object that fails to parse is skipped and scanning continues just after its header.
//! - When an object number occurs more than once, the **last occurrence in the file wins**: later
//!   bytes are later revisions. Objects inside object streams follow the same rule, using the
//!   position of their container.
//! - The trailer is the last `trailer` dictionary or cross-reference stream dictionary that has a
//!   `/Root`; with none, a `/Type /Catalog` object is used and the trailer is synthesised
//!   ([`RepairReason::TrailerSynthesized`]). `/Prev` and `/XRefStm` are dropped from it, and
//!   `/Encrypt`, `/ID` and `/Info` are taken from the latest trailer that has them if the chosen
//!   one lacks them.
//! - Object streams are expanded when they can be decoded (no filter or Flate). One that cannot
//!   (encrypted, or another filter until M0 task 10) is reported as
//!   [`RepairReason::ObjectStreamNotExpanded`] and its objects are missing from the result.
//!
//! Limits: entries are bounded by `max_xref_entries`, decoding by the decode limits and the
//! caller's [`DecodeBudget`]. Attempts that fail can re-read bytes (an unterminated string runs to
//! the end of the file, and the scan resumes just after the header), so the scan keeps a work
//! budget of four times the file size plus 1 MiB; failed attempts and failed `endstream` searches
//! are charged to it and, once it is spent, no more objects are parsed.
//!
//! Offsets in the result are **not trusted by the object store either**: it must check that an
//! object header really is at an offset before using it (see [`object_header_at`]).

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

use crate::error::{Error, Result, SyntaxKind};
use crate::lexer::{Lexer, TokenKind, is_regular, is_whitespace};
use crate::limits::{DecodeBudget, LimitKind, Limits};
use crate::object::{Dict, DictEntry, ObjRef, Object, ObjectKind};
use crate::objstm::ObjectStream;
use crate::parser::Parser;
use crate::xref::{HEADER_SEARCH_BYTES, Revision, SectionKind, Xref, XrefEntry, XrefSection};

/// Why a file counts as repaired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RepairReason {
    /// The cross-reference data could not be read; carries the syntax error found.
    XrefUnreadable(SyntaxKind),
    /// A cross-reference stream could not be decoded.
    XrefUndecodable,
    /// The trailer has no `/Root`, or the cross-reference points `/Root` at something that is not
    /// an object with that number.
    RootEntryInvalid,
    /// An object was not where the cross-reference said (reported by the object store).
    ObjectOffsetInvalid {
        /// The object number.
        number: u32,
    },
    /// No usable trailer was found, so one was built from a `/Type /Catalog` object.
    TrailerSynthesized,
    /// An object stream could not be decoded, so the objects inside it are missing.
    ObjectStreamNotExpanded {
        /// Object number of the stream.
        stream: u32,
    },
}

/// Whether a real `number generation obj` header starts at `base + offset`, with the number
/// matching (the generation is not compared: producers disagree about it).
///
/// This is the check to run before trusting a cross-reference offset.
#[must_use]
pub fn object_header_at(data: &[u8], base: usize, offset: u64, number: u32) -> bool {
    let Some(position) = usize::try_from(offset)
        .ok()
        .and_then(|o| o.checked_add(base))
        .filter(|&p| p < data.len())
    else {
        return false;
    };
    // Three tiny tokens; the default limits are plenty and nothing here allocates much.
    let limits = Limits::default();
    let mut lexer = Lexer::at(data, position, &limits);
    let mut next = || lexer.next_token().ok().flatten().map(|t| t.kind);
    matches!(
        (next(), next(), next()),
        (Some(TokenKind::Integer(n)), Some(TokenKind::Integer(_)), Some(TokenKind::Keyword(b"obj")))
            if u32::try_from(n) == Ok(number)
    )
}

/// What the scan found.
enum Candidate {
    /// `n g obj`; the offset is where the number starts.
    ObjectHeader(usize),
    /// The `trailer` keyword; the offset is just after it.
    Trailer(usize),
}

/// Finds the next object header or `trailer` keyword at or after `from`, with the position to
/// continue from if it turns out to be nothing.
fn next_candidate(data: &[u8], from: usize) -> Option<(Candidate, usize)> {
    let mut i = from;
    while i < data.len() {
        match data.get(i) {
            Some(b'o') if data.get(i..i + 3) == Some(b"obj") => {
                let after_ok = data.get(i + 3).is_none_or(|&b| !is_regular(b));
                if after_ok && let Some(start) = header_start(data, i) {
                    return Some((Candidate::ObjectHeader(start), i + 3));
                }
            }
            Some(b't') if data.get(i..i + 7) == Some(b"trailer") => {
                let before_ok = i == 0 || data.get(i - 1).is_none_or(|&b| !is_regular(b));
                let after_ok = data.get(i + 7).is_none_or(|&b| !is_regular(b));
                if before_ok && after_ok {
                    return Some((Candidate::Trailer(i + 7), i + 7));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// For an `obj` keyword at `obj_at`, the start of `number generation` before it, if the bytes
/// before it look like two integers separated by white space.
fn header_start(data: &[u8], obj_at: usize) -> Option<usize> {
    // Walks backwards over bytes matching `keep`, returning the new position and the count.
    let back = |from: usize, keep: fn(&u8) -> bool| {
        let mut i = from;
        while i > 0 && data.get(i - 1).is_some_and(keep) {
            i -= 1;
        }
        (i, from - i)
    };
    let (i, blanks) = back(obj_at, |b| is_whitespace(*b));
    let (i, generation_digits) = back(i, u8::is_ascii_digit);
    let (i, blanks_between) = back(i, |b| is_whitespace(*b));
    let (start, number_digits) = back(i, u8::is_ascii_digit);
    let plausible = blanks > 0
        && blanks_between > 0
        && (1..=5).contains(&generation_digits)
        && (1..=10).contains(&number_digits);
    // The number must start a token, not be glued to a preceding regular character.
    let glued = start > 0 && data.get(start - 1).is_some_and(|&b| is_regular(b));
    (plausible && !glued).then_some(start)
}

/// One object seen by the scan.
struct Seen {
    position: usize,
    entry: XrefEntry,
}

/// An object stream seen by the scan, waiting to be expanded.
struct Container<'a> {
    position: usize,
    number: u32,
    dict: Dict<'a>,
    raw: Range<usize>,
}

/// Everything the scan collects.
#[derive(Default)]
struct Scan<'a> {
    objects: HashMap<u32, Seen>,
    containers: Vec<Container<'a>>,
    trailers: Vec<Dict<'a>>,
    catalogs: Vec<ObjRef>,
}

impl<'a> Scan<'a> {
    fn insert(&mut self, limits: &Limits, number: u32, seen: Seen) -> Result<()> {
        if !self.objects.contains_key(&number) {
            limits.check(LimitKind::XrefEntries, self.objects.len() as u64 + 1, None)?;
        }
        self.objects.insert(number, seen);
        Ok(())
    }

    /// Records one parsed object: its entry, and whether it is a trailer source, a catalog or an
    /// object stream.
    fn note_object(
        &mut self,
        limits: &Limits,
        base: usize,
        id: ObjRef,
        start: usize,
        object: Object<'a>,
    ) -> Result<()> {
        let offset = u64::try_from(start.saturating_sub(base)).unwrap_or(u64::MAX);
        let entry = XrefEntry::InUse {
            offset,
            generation: id.generation,
        };
        self.insert(
            limits,
            id.num,
            Seen {
                position: start,
                entry,
            },
        )?;

        let type_is = |dict: &Dict<'_>, wanted: &[u8]| matches!(dict.get(b"Type").map(|o| &o.kind), Some(ObjectKind::Name(n)) if n.as_ref() == wanted);
        match object.kind {
            ObjectKind::Dict(dict) if type_is(&dict, b"Catalog") => self.catalogs.push(id),
            ObjectKind::Stream(stream) if type_is(&stream.dict, b"XRef") => {
                self.trailers.push(stream.dict);
            }
            ObjectKind::Stream(stream) if type_is(&stream.dict, b"ObjStm") => {
                self.containers.push(Container {
                    position: start,
                    number: id.num,
                    dict: stream.dict,
                    raw: stream.data,
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// Decodes each object stream and adds the objects inside it. A container only replaces a
    /// direct object (or another container) that comes **before** it in the file.
    fn expand_containers(
        &mut self,
        data: &[u8],
        limits: &Limits,
        budget: &mut DecodeBudget,
        repaired: &mut Vec<RepairReason>,
    ) -> Result<()> {
        let containers = std::mem::take(&mut self.containers);
        for container in &containers {
            let raw = data.get(container.raw.clone()).unwrap_or_default();
            let offset = Some(container.raw.start as u64);
            let decoded =
                ObjectStream::from_stream(&container.dict, raw, limits, Some(&mut *budget), offset);
            let Ok(stream) = decoded else {
                repaired.push(RepairReason::ObjectStreamNotExpanded {
                    stream: container.number,
                });
                continue;
            };
            for index in 0..stream.len() {
                let Some(number) = stream.object_number(index) else {
                    continue;
                };
                let (Ok(index), true) = (
                    u32::try_from(index),
                    number != container.number
                        && self
                            .objects
                            .get(&number)
                            .is_none_or(|s| s.position < container.position),
                ) else {
                    continue;
                };
                let entry = XrefEntry::Compressed {
                    stream: container.number,
                    index,
                };
                self.insert(
                    limits,
                    number,
                    Seen {
                        position: container.position,
                        entry,
                    },
                )?;
            }
        }
        Ok(())
    }

    /// Picks the trailer for the rebuilt file (see the module docs).
    fn choose_trailer(&self, repaired: &mut Vec<RepairReason>) -> Result<Dict<'a>> {
        let has_root =
            |d: &Dict<'_>| matches!(d.get(b"Root").map(|o| &o.kind), Some(ObjectKind::Ref(_)));
        let mut trailer = if let Some(dict) = self.trailers.iter().rev().find(|d| has_root(d)) {
            dict.clone()
        } else if let Some(&id) = self.catalogs.last() {
            repaired.push(RepairReason::TrailerSynthesized);
            let nowhere = 0..0;
            Dict {
                entries: vec![DictEntry {
                    key: Cow::Borrowed(b"Root"),
                    key_span: nowhere.clone(),
                    value: Object {
                        kind: ObjectKind::Ref(id),
                        span: nowhere,
                    },
                }],
            }
        } else {
            return Err(Error::Syntax {
                kind: SyntaxKind::RootNotFound,
                offset: 0,
            });
        };
        // The old chain no longer exists.
        trailer
            .entries
            .retain(|e| !matches!(e.key.as_ref(), b"Prev" | b"XRefStm"));
        for key in [&b"Encrypt"[..], b"ID", b"Info"] {
            if trailer.get(key).is_none()
                && let Some(entry) = self
                    .trailers
                    .iter()
                    .rev()
                    .find_map(|d| d.entries.iter().rev().find(|e| e.key.as_ref() == key))
            {
                trailer.entries.push(entry.clone());
            }
        }
        Ok(trailer)
    }
}

/// The scan itself: finds objects and trailers and records them in `scan`.
///
/// Bytes read by attempts that **fail** are not consumed (the scan resumes just after the header
/// that started the attempt), so a hostile file like `1 0 obj (` repeated would make every
/// attempt read to the end of the file. The scan therefore keeps a work budget: each failed
/// attempt is charged the bytes it read, and a failed stream search is charged its window. When
/// the budget is gone, further attempts are skipped, so the total is a small multiple of the file
/// size however the file is built. Successful attempts are never charged: the scan resumes after
/// them.
fn scan_objects<'a>(
    data: &'a [u8],
    base: usize,
    limits: &Limits,
    scan: &mut Scan<'a>,
) -> Result<()> {
    let mut work_left = data.len().saturating_mul(4).saturating_add(1 << 20);
    let mut pos = base;
    while let Some((candidate, resume)) = next_candidate(data, pos) {
        pos = resume;
        if work_left == 0 {
            // Only plain scanning is left; nothing more is parsed.
            continue;
        }
        match candidate {
            Candidate::Trailer(after) => {
                let mut parser = Parser::at(data, after, limits);
                match parser.parse_object() {
                    Ok(Object {
                        kind: ObjectKind::Dict(dict),
                        span,
                    }) => {
                        scan.trailers.push(dict);
                        pos = span.end.max(pos);
                    }
                    Ok(_) => {}
                    Err(_) => {
                        work_left =
                            work_left.saturating_sub(parser.position().saturating_sub(after));
                    }
                }
            }
            Candidate::ObjectHeader(start) => {
                // A stream whose `endstream` is missing is searched for within what is left of
                // the budget.
                let window = limits
                    .max_decoded_stream_bytes
                    .min(u64::try_from(work_left).unwrap_or(u64::MAX));
                let local = Limits {
                    max_decoded_stream_bytes: window,
                    ..limits.clone()
                };
                let mut parser = Parser::at(data, start, &local);
                match parser.parse_indirect_object() {
                    Ok(indirect) => {
                        pos = indirect.span.end.max(pos);
                        scan.note_object(limits, base, indirect.id, start, indirect.object)?;
                    }
                    Err(Error::Syntax { kind, .. }) => {
                        let read = parser.position().saturating_sub(start);
                        let searched = if kind == SyntaxKind::MissingEndstream {
                            usize::try_from(window)
                                .unwrap_or(usize::MAX)
                                .min(data.len() - start)
                        } else {
                            0
                        };
                        work_left = work_left.saturating_sub(read.saturating_add(searched));
                    }
                    Err(Error::LimitExceeded { .. }) => {
                        work_left =
                            work_left.saturating_sub(parser.position().saturating_sub(start));
                    }
                    Err(other) => return Err(other),
                }
            }
        }
    }
    Ok(())
}

/// Rebuilds the cross-reference information of `data` by scanning it.
///
/// `reasons` says why the caller is rebuilding; the scan appends its own findings and returns all
/// of them in [`Xref::repaired`]. `budget` is charged for everything decoded (object streams).
///
/// # Errors
/// [`SyntaxKind::NothingToRecover`] if no object is found, [`SyntaxKind::RootNotFound`] if there
/// is neither a trailer with `/Root` nor a `/Type /Catalog` object, and
/// [`Error::LimitExceeded`] if the file has more objects than `max_xref_entries`.
pub fn rebuild<'a>(
    data: &'a [u8],
    limits: &Limits,
    budget: &mut DecodeBudget,
    reasons: &[RepairReason],
) -> Result<Xref<'a>> {
    let base = data
        .get(..HEADER_SEARCH_BYTES.min(data.len()))
        .unwrap_or_default()
        .windows(b"%PDF-".len())
        .position(|w| w == b"%PDF-")
        .unwrap_or(0);

    let mut repaired = reasons.to_vec();
    let mut scan = Scan::default();
    scan_objects(data, base, limits, &mut scan)?;

    if scan.objects.is_empty() && scan.containers.is_empty() {
        return Err(Error::Syntax {
            kind: SyntaxKind::NothingToRecover,
            offset: data.len() as u64,
        });
    }
    scan.expand_containers(data, limits, budget, &mut repaired)?;
    let trailer = scan.choose_trailer(&mut repaired)?;

    let mut entries: Vec<(u32, XrefEntry)> = scan
        .objects
        .into_iter()
        .map(|(number, seen)| (number, seen.entry))
        .collect();
    entries.retain(|&(number, _)| number != 0);
    entries.sort_unstable_by_key(|&(number, _)| number);
    entries.insert(
        0,
        (
            0,
            XrefEntry::Free {
                next_free: 0,
                generation: 65535,
            },
        ),
    );

    let section = XrefSection {
        kind: SectionKind::Rebuilt,
        offset: 0,
        entries,
        stream_entries: Vec::new(),
        trailer,
        span: 0..data.len(),
        warnings: Vec::new(),
    };
    Ok(Xref {
        base,
        revisions: vec![Revision {
            section,
            bytes: 0..data.len(),
        }],
        repaired,
    })
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use proptest::prelude::*;

    use super::*;

    fn open(data: &[u8]) -> Xref<'_> {
        Xref::open(data, &Limits::default()).unwrap()
    }

    fn open_err(data: &[u8]) -> Error {
        Xref::open(data, &Limits::default()).unwrap_err()
    }

    fn syntax_kind(error: &Error) -> SyntaxKind {
        match error {
            Error::Syntax { kind, .. } => *kind,
            other => panic!("{other:?}"),
        }
    }

    /// A small healthy file with a classic table: catalog (1), pages (2), page (3).
    fn healthy() -> Vec<u8> {
        let mut data = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (n, body) in [
            (1, "<< /Type /Catalog /Pages 2 0 R >>"),
            (2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, "<< /Type /Page /Parent 2 0 R >>"),
        ] {
            offsets.push(data.len());
            data.extend(format!("{n} 0 obj\n{body}\nendobj\n").bytes());
        }
        let xref_at = data.len();
        data.extend(b"xref\n0 4\n0000000000 65535 f \n");
        for o in offsets {
            data.extend(format!("{o:010} 00000 n \n").bytes());
        }
        data.extend(
            format!(
                "trailer\n<< /Size 4 /Root 1 0 R /Info 3 0 R >>\nstartxref\n{xref_at}\n%%EOF\n"
            )
            .bytes(),
        );
        data
    }

    fn numbers(xref: &Xref<'_>) -> Vec<u32> {
        xref.revisions[0]
            .section
            .entries
            .iter()
            .map(|&(n, _)| n)
            .collect()
    }

    fn assert_every_entry_is_a_real_object(data: &[u8], xref: &Xref<'_>) {
        for &(number, entry) in &xref.revisions[0].section.entries {
            if let XrefEntry::InUse { offset, .. } = entry {
                assert!(
                    object_header_at(data, xref.base, offset, number),
                    "object {number}"
                );
            }
        }
    }

    #[test]
    fn a_healthy_file_is_not_repaired() {
        let data = healthy();
        let xref = open(&data);
        assert_eq!(xref.repaired, Vec::new());
        assert_eq!(xref.revisions[0].section.kind, SectionKind::Table);
        assert!(xref.root_is_valid(&data));
    }

    #[test]
    fn missing_startxref_is_rebuilt_by_scanning() {
        let mut data = healthy();
        let cut = data.windows(9).rposition(|w| w == b"startxref").unwrap();
        data.truncate(cut);
        let xref = open(&data);
        assert_eq!(
            xref.repaired,
            [RepairReason::XrefUnreadable(SyntaxKind::StartxrefNotFound)]
        );
        assert_eq!(xref.revisions.len(), 1);
        assert_eq!(xref.revisions[0].section.kind, SectionKind::Rebuilt);
        assert_eq!(numbers(&xref), [0, 1, 2, 3]);
        assert_eq!(
            xref.lookup(0),
            Some(XrefEntry::Free {
                next_free: 0,
                generation: 65535
            })
        );
        assert_every_entry_is_a_real_object(&data, &xref);
        // The trailer comes from the file's own trailer dictionary.
        let trailer = xref.trailer().unwrap();
        assert_eq!(
            trailer.get(b"Root").unwrap().kind,
            ObjectKind::Ref(ObjRef::new(1, 0))
        );
        assert!(trailer.get(b"Info").is_some());
        assert!(xref.root_is_valid(&data));
        assert_eq!(xref.revisions[0].bytes, 0..data.len());
    }

    #[test]
    fn a_wrong_startxref_offset_is_rebuilt() {
        let data = healthy();
        let text = String::from_utf8_lossy(&data).to_string();
        let at = text.rfind("startxref\n").unwrap() + 10;
        let end = text[at..].find('\n').unwrap() + at;
        let mut broken = data[..at].to_vec();
        broken.extend(b"13"); // points into the middle of the first object
        broken.extend(&data[end..]);
        let xref = open(&broken);
        assert!(matches!(xref.repaired[0], RepairReason::XrefUnreadable(_)));
        assert_eq!(numbers(&xref), [0, 1, 2, 3]);

        let mut far = data[..at].to_vec();
        far.extend(b"99999999");
        far.extend(&data[end..]);
        assert_eq!(
            open(&far).repaired,
            [RepairReason::XrefUnreadable(SyntaxKind::InvalidXrefOffset)]
        );
    }

    #[test]
    fn a_table_that_points_the_root_at_the_wrong_place_is_rebuilt() {
        // Valid structure, but every offset is shifted: the entry for the root is not a header.
        let data = healthy();
        let text = String::from_utf8_lossy(&data).to_string();
        let table = text.find("0000000009").unwrap();
        let mut broken = data.clone();
        broken[table..table + 10].copy_from_slice(b"0000000011");
        let xref = open(&broken);
        assert_eq!(xref.repaired, [RepairReason::RootEntryInvalid]);
        assert_every_entry_is_a_real_object(&broken, &xref);
        assert_eq!(numbers(&xref), [0, 1, 2, 3]);
    }

    #[test]
    fn a_trailer_without_root_falls_back_to_scanning() {
        let data = healthy();
        let text = String::from_utf8_lossy(&data).replace("/Root 1 0 R ", "");
        let xref = open(text.as_bytes());
        assert_eq!(
            xref.repaired,
            [
                RepairReason::RootEntryInvalid,
                RepairReason::TrailerSynthesized
            ]
        );
        // The trailer is built from the catalog object.
        assert_eq!(
            xref.trailer().unwrap().get(b"Root").unwrap().kind,
            ObjectKind::Ref(ObjRef::new(1, 0))
        );
    }

    #[test]
    fn no_trailer_at_all_uses_the_catalog() {
        let data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n".to_vec();
        let xref = open(&data);
        assert_eq!(
            xref.repaired,
            [
                RepairReason::XrefUnreadable(SyntaxKind::StartxrefNotFound),
                RepairReason::TrailerSynthesized
            ]
        );
        assert!(xref.root_is_valid(&data));
    }

    #[test]
    fn objects_without_a_root_are_a_typed_error() {
        let data = b"%PDF-1.4\n1 0 obj\n<< /Foo 1 >>\nendobj\n".to_vec();
        assert_eq!(syntax_kind(&open_err(&data)), SyntaxKind::RootNotFound);
    }

    #[test]
    fn nothing_to_recover_is_a_typed_error() {
        for data in [&b""[..], b"%PDF-1.4\n", b"just some text", b"\x00\x01\x02"] {
            assert_eq!(
                syntax_kind(&open_err(data)),
                SyntaxKind::NothingToRecover,
                "{data:?}"
            );
        }
    }

    #[test]
    fn the_last_occurrence_of_an_object_number_wins() {
        let data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R /V 1 >>\nendobj\n2 0 obj\n<< >>\nendobj\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R /V 2 >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n".to_vec();
        let xref = open(&data);
        assert_eq!(numbers(&xref), [0, 1, 2]);
        let Some(XrefEntry::InUse { offset, .. }) = xref.lookup(1) else {
            panic!()
        };
        // The second definition, which starts after the first object 2.
        let text = String::from_utf8_lossy(&data).to_string();
        assert_eq!(
            usize::try_from(offset).unwrap(),
            text.rfind("1 0 obj").unwrap()
        );
        assert_every_entry_is_a_real_object(&data, &xref);
    }

    #[test]
    fn text_that_looks_like_an_object_inside_stream_data_is_not_an_object() {
        let mut data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        let payload = b"5 0 obj << /Fake true >> endobj trailer << /Root 5 0 R >>";
        data.extend(format!("2 0 obj\n<< /Length {} >>\nstream\n", payload.len()).bytes());
        data.extend(payload);
        data.extend(b"\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n");
        let xref = open(&data);
        assert_eq!(numbers(&xref), [0, 1, 2]);
        assert_eq!(
            xref.trailer().unwrap().get(b"Root").unwrap().kind,
            ObjectKind::Ref(ObjRef::new(1, 0))
        );

        // The same with a wrong /Length: the parser finds `endstream` and the scan resumes after it.
        let wrong = String::from_utf8_lossy(&data)
            .replace(&format!("/Length {}", payload.len()), "/Length 3");
        assert_eq!(numbers(&open(wrong.as_bytes())), [0, 1, 2]);
    }

    #[test]
    fn words_that_merely_contain_obj_are_not_headers() {
        let data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n2 0 object\nx3 0 obj\n4 0 objx\n/Name 5 0 obj\ntrailer\n<< /Root 1 0 R >>\n".to_vec();
        let xref = open(&data);
        // `x3 0 obj` is glued to a letter, `object` and `objx` are other words, and `5 0 obj`
        // after a name is a header but holds no object (the next token is `trailer`), so only
        // object 1 is found.
        assert_eq!(numbers(&xref), [0, 1]);
    }

    #[test]
    fn objects_that_do_not_parse_are_skipped_and_the_scan_continues() {
        let data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n2 0 obj\n<< /Broken [ \nendobj\n3 0 obj\n42\nendobj\ntrailer\n<< /Root 1 0 R >>\n".to_vec();
        let xref = open(&data);
        assert_eq!(numbers(&xref), [0, 1, 3]);
    }

    #[test]
    fn junk_before_the_header_keeps_offsets_relative_to_it() {
        let mut data = b"GARBAGE\r\n".to_vec();
        let mut body = healthy();
        let cut = body.windows(9).rposition(|w| w == b"startxref").unwrap();
        body.truncate(cut);
        data.extend(body);
        let xref = open(&data);
        assert_eq!(xref.base, 9);
        assert_eq!(numbers(&xref), [0, 1, 2, 3]);
        assert_every_entry_is_a_real_object(&data, &xref);
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    /// An object stream object (number `num`) holding `(number, body)` pairs.
    fn object_stream(num: u32, objects: &[(u32, &str)], filter: bool) -> Vec<u8> {
        let mut header = String::new();
        let mut body = String::new();
        for (n, text) in objects {
            let _ = write!(header, "{n} {} ", body.len());
            body.push_str(text);
            body.push('\n');
        }
        let first = header.len();
        let raw = format!("{header}{body}").into_bytes();
        let (bytes, filter_text) = if filter {
            (zlib(&raw), "/Filter /FlateDecode")
        } else {
            (raw, "")
        };
        let mut out = format!(
            "{num} 0 obj\n<< /Type /ObjStm /N {} /First {first} /Length {} {filter_text} >>\nstream\n",
            objects.len(),
            bytes.len()
        )
        .into_bytes();
        out.extend(bytes);
        out.extend(b"\nendstream\nendobj\n");
        out
    }

    #[test]
    fn objects_inside_object_streams_are_found() {
        let mut data = b"%PDF-1.5\n1 0 obj\n<< /Type /Catalog /Pages 10 0 R >>\nendobj\n".to_vec();
        data.extend(object_stream(
            2,
            &[
                (10, "<< /Type /Pages /Kids [11 0 R] >>"),
                (11, "<< /Type /Page >>"),
            ],
            true,
        ));
        data.extend(b"trailer\n<< /Root 1 0 R >>\n");
        let xref = open(&data);
        assert_eq!(numbers(&xref), [0, 1, 2, 10, 11]);
        assert_eq!(
            xref.lookup(10),
            Some(XrefEntry::Compressed {
                stream: 2,
                index: 0
            })
        );
        assert_eq!(
            xref.lookup(11),
            Some(XrefEntry::Compressed {
                stream: 2,
                index: 1
            })
        );
        assert!(
            !xref
                .repaired
                .iter()
                .any(|r| matches!(r, RepairReason::ObjectStreamNotExpanded { .. }))
        );
    }

    #[test]
    fn a_later_direct_object_beats_an_earlier_container_and_vice_versa() {
        // Object 10 is in container 2, then redefined directly (direct wins); object 11 is
        // defined directly first, then in container 3 (the container wins).
        let mut data = b"%PDF-1.5\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        data.extend(object_stream(2, &[(10, "1"), (12, "3")], false));
        data.extend(b"10 0 obj\n2\nendobj\n11 0 obj\n4\nendobj\n");
        data.extend(object_stream(3, &[(11, "5")], false));
        data.extend(b"trailer\n<< /Root 1 0 R >>\n");
        let xref = open(&data);
        assert!(matches!(xref.lookup(10), Some(XrefEntry::InUse { .. })));
        assert_eq!(
            xref.lookup(11),
            Some(XrefEntry::Compressed {
                stream: 3,
                index: 0
            })
        );
        assert_eq!(
            xref.lookup(12),
            Some(XrefEntry::Compressed {
                stream: 2,
                index: 1
            })
        );
    }

    #[test]
    fn an_object_stream_that_cannot_be_decoded_is_reported() {
        let mut data = b"%PDF-1.5\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        data.extend(b"2 0 obj\n<< /Type /ObjStm /N 1 /First 4 /Filter /LZWDecode /Length 3 >>\nstream\nabc\nendstream\nendobj\n");
        data.extend(b"trailer\n<< /Root 1 0 R >>\n");
        let xref = open(&data);
        assert!(
            xref.repaired
                .contains(&RepairReason::ObjectStreamNotExpanded { stream: 2 })
        );
        assert_eq!(numbers(&xref), [0, 1, 2]);
    }

    #[test]
    fn an_object_stream_bomb_is_skipped_not_expanded() {
        let mut data = b"%PDF-1.5\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        let bomb = zlib(&vec![0u8; 8 * 1024 * 1024]);
        data.extend(
            format!("2 0 obj\n<< /Type /ObjStm /N 1 /First 4 /Filter /FlateDecode /Length {} >>\nstream\n", bomb.len())
                .bytes(),
        );
        data.extend(bomb);
        data.extend(b"\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n");
        let xref = open(&data);
        assert!(
            xref.repaired
                .contains(&RepairReason::ObjectStreamNotExpanded { stream: 2 })
        );
    }

    #[test]
    fn a_cross_reference_stream_dictionary_provides_the_trailer() {
        // The stream itself is useless (no rows) but its dictionary has the root.
        let mut data = b"%PDF-1.5\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        data.extend(b"2 0 obj\n<< /Type /XRef /Size 3 /W [1 2 1] /Root 1 0 R /ID [<aa> <bb>] /Length 0 >>\nstream\n\nendstream\nendobj\n");
        data.extend(b"startxref\n999999\n%%EOF\n");
        let xref = open(&data);
        assert_eq!(
            xref.trailer().unwrap().get(b"Root").unwrap().kind,
            ObjectKind::Ref(ObjRef::new(1, 0))
        );
        assert!(xref.trailer().unwrap().get(b"ID").is_some());
        assert!(!xref.repaired.contains(&RepairReason::TrailerSynthesized));
    }

    #[test]
    fn the_rebuilt_trailer_drops_the_old_chain_and_borrows_missing_keys() {
        let data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n<< /Root 1 0 R /Info 9 0 R /ID [<aa><bb>] /Encrypt 8 0 R >>\n2 0 obj\n<< >>\nendobj\ntrailer\n<< /Root 1 0 R /Prev 5 /XRefStm 7 >>\n".to_vec();
        let xref = open(&data);
        let trailer = xref.trailer().unwrap();
        assert!(trailer.get(b"Prev").is_none());
        assert!(trailer.get(b"XRefStm").is_none());
        assert!(trailer.get(b"Info").is_some());
        assert!(trailer.get(b"ID").is_some());
        assert!(trailer.get(b"Encrypt").is_some());
    }

    #[test]
    fn the_entry_limit_applies_to_rebuilding() {
        let mut data = b"%PDF-1.4\n".to_vec();
        for n in 1..=20 {
            data.extend(format!("{n} 0 obj\n<< /Type /Catalog >>\nendobj\n").bytes());
        }
        data.extend(b"trailer\n<< /Root 1 0 R >>\n");
        let tight = Limits {
            max_xref_entries: 10,
            ..Limits::default()
        };
        let err = Xref::open(&data, &tight).unwrap_err();
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
        assert_eq!(
            Xref::open(&data, &Limits::default()).unwrap().revisions[0]
                .section
                .entries
                .len(),
            21
        );
    }

    #[test]
    fn many_streams_that_never_end_do_not_cost_a_scan_each() {
        // 3000 objects with `stream` and no `endstream`: each would search to the end of the file.
        let mut data = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();
        for n in 2..3000 {
            data.extend(
                format!("{n} 0 obj\n<< /Length 99999 >>\nstream\nxxxxxxxxxxxxxxxxxxxx\n").bytes(),
            );
        }
        data.extend(b"trailer\n<< /Root 1 0 R >>\n");
        let started = std::time::Instant::now();
        let xref = open(&data);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(xref.lookup(1).is_some());
    }

    /// Time budget for the hostile-input tests below. Linear work finishes in well under a
    /// second even in a debug build; the quadratic version takes minutes.
    const HOSTILE_TIME: std::time::Duration = std::time::Duration::from_secs(10);

    #[test]
    fn repeated_unterminated_strings_do_not_make_the_scan_quadratic() {
        // Every `1 0 obj (` starts a string that runs to the end of the file. The scan resumes
        // after each header, so without a work budget each attempt re-reads the rest of the file.
        let mut data = b"%PDF-1.4
"
        .to_vec();
        for _ in 0..200_000 {
            data.extend(b"1 0 obj (");
        }
        data.extend(
            b"
2 0 obj
<< /Type /Catalog >>
endobj
trailer
<< /Root 2 0 R >>
",
        );
        let started = std::time::Instant::now();
        let result = Xref::open(&data, &Limits::default());
        assert!(started.elapsed() < HOSTILE_TIME, "{:?}", started.elapsed());
        // Whatever it makes of the file, it does not panic and gives a typed answer.
        if let Err(e) = result {
            assert!(matches!(e, Error::Syntax { .. }), "{e:?}");
        }
    }

    #[test]
    fn repeated_unterminated_hex_strings_and_trailers_are_bounded_too() {
        let started = std::time::Instant::now();
        let mut hex = b"%PDF-1.4
"
        .to_vec();
        for _ in 0..200_000 {
            hex.extend(b"1 0 obj <");
        }
        let _ = Xref::open(&hex, &Limits::default());
        let mut trailers = b"%PDF-1.4
1 0 obj << /Type /Catalog >> endobj
"
        .to_vec();
        for _ in 0..200_000 {
            trailers.extend(b"trailer << /A (");
        }
        let _ = Xref::open(&trailers, &Limits::default());
        assert!(started.elapsed() < HOSTILE_TIME, "{:?}", started.elapsed());
    }

    #[test]
    fn repeated_unterminated_arrays_and_dictionaries_are_bounded() {
        // Each attempt reads until the next `obj` keyword, but a long run of tokens that never
        // contains one is still read once per header that precedes it.
        let started = std::time::Instant::now();
        let mut data = b"%PDF-1.4
"
        .to_vec();
        for _ in 0..2_000 {
            data.extend(b"1 0 obj [ ");
            data.extend(b"1 ".repeat(500));
        }
        let _ = Xref::open(&data, &Limits::default());
        assert!(started.elapsed() < HOSTILE_TIME, "{:?}", started.elapsed());
    }

    #[test]
    fn one_early_unterminated_string_does_not_hide_the_good_objects_after_it() {
        // A single failure costs one read to the end of the file, well inside the budget, and the
        // objects "inside" the string are still found because the scan resumes after its header.
        let mut data = b"%PDF-1.4
1 0 obj
(never closed
"
        .to_vec();
        for n in 2..60 {
            data.extend(
                format!(
                    "{n} 0 obj
<< /N {n} >>
endobj
"
                )
                .bytes(),
            );
        }
        data.extend(
            b"60 0 obj
<< /Type /Catalog >>
endobj
trailer
<< /Root 60 0 R >>
",
        );
        let xref = open(&data);
        let found = numbers(&xref);
        assert_eq!(found.len(), 1 + 58 + 1, "{found:?}");
        assert!(!found.contains(&1));
        assert!(xref.root_is_valid(&data));
    }

    #[test]
    fn object_header_checks() {
        let data = b"%PDF-1.4\n12 0 obj\n<< >>\nendobj\n";
        assert!(object_header_at(data, 0, 9, 12));
        // Wrong number, wrong place, outside the file, overflow.
        assert!(!object_header_at(data, 0, 9, 13));
        assert!(!object_header_at(data, 0, 10, 12));
        assert!(!object_header_at(data, 0, 9999, 12));
        assert!(!object_header_at(data, 0, u64::MAX, 12));
        assert!(!object_header_at(data, usize::MAX, 9, 12));
        // The base shifts the offset.
        let shifted = [&b"JUNK"[..], data].concat();
        assert!(object_header_at(&shifted, 4, 9, 12));
        // Generation is not compared.
        assert!(object_header_at(b"7 3 obj null endobj", 0, 0, 7));
        assert!(!object_header_at(b"7 obj null endobj", 0, 0, 7));
        assert!(!object_header_at(b"", 0, 0, 0));
    }

    proptest! {
        /// Opening arbitrary bytes never panics; anything that opens has real objects.
        #[test]
        fn arbitrary_bytes_never_panic(data in proptest::collection::vec(any::<u8>(), 0..800)) {
            if let Ok(xref) = Xref::open(&data, &Limits::default()) {
                for rev in &xref.revisions {
                    prop_assert!(rev.bytes.end <= data.len());
                }
            }
        }

        /// Damaging a healthy file in any way either opens it or gives a typed error; every
        /// in-use entry of a repaired file is a real object.
        #[test]
        fn damaged_files_open_with_valid_entries_or_fail(
            pos in 0usize..420,
            byte in any::<u8>(),
            cut in 0usize..200,
            drop_from in 0usize..420,
            drop_len in 0usize..30,
        ) {
            let mut data = healthy();
            if let Some(slot) = data.get_mut(pos) {
                *slot = byte;
            }
            let end = drop_from.saturating_add(drop_len).min(data.len());
            if drop_from < end {
                data.drain(drop_from..end);
            }
            data.truncate(data.len().saturating_sub(cut));
            if let Ok(xref) = Xref::open(&data, &Limits::default())
                && !xref.repaired.is_empty()
            {
                for &(number, entry) in &xref.revisions[0].section.entries {
                    if let XrefEntry::InUse { offset, .. } = entry {
                        prop_assert!(object_header_at(&data, xref.base, offset, number));
                    }
                }
            }
        }
    }
}
