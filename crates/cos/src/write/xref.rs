//! Writing cross-reference sections: classic tables (§7.5.4) and streams (§7.5.8).

use std::io::Write as _;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::error::{Result, WriteError};
use crate::object::{Dict, Object, ObjectKind};
use crate::xref::XrefEntry;

use super::serialize::Serializer;

/// Offsets in a classic table have ten digits.
const MAX_TABLE_OFFSET: u64 = 9_999_999_999;

/// Consecutive runs of object numbers: `(first, count)`. `entries` must be sorted by number.
fn runs(entries: &[(u32, XrefEntry)]) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &(number, _) in entries {
        match runs.last_mut() {
            Some((first, count)) if first.checked_add(*count) == Some(number) => *count += 1,
            _ => runs.push((number, 1)),
        }
    }
    runs
}

/// Writes `xref`, its subsections and `trailer << ... >>` (without `startxref`). Each entry is
/// exactly 20 bytes (`nnnnnnnnnn ggggg n` + space + LF).
///
/// `entries` must be sorted by object number, free and in-use only; offsets are relative to the
/// header, as in the file.
pub(super) fn write_table(
    out: &mut Vec<u8>,
    entries: &[(u32, XrefEntry)],
    trailer: &Dict<'_>,
    serializer: &Serializer<'_>,
) -> Result<()> {
    out.extend_from_slice(b"xref\n");
    let mut rest = entries;
    for (first, count) in runs(entries) {
        out.extend_from_slice(format!("{first} {count}\n").as_bytes());
        let (run, tail) = usize::try_from(count)
            .ok()
            .and_then(|n| rest.split_at_checked(n))
            .unwrap_or((rest, &[]));
        rest = tail;
        for (_, entry) in run {
            match *entry {
                XrefEntry::InUse { offset, generation } => {
                    if offset > MAX_TABLE_OFFSET {
                        return Err(WriteError::OffsetTooLarge(offset).into());
                    }
                    out.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
                }
                XrefEntry::Free {
                    next_free,
                    generation,
                } => {
                    out.extend_from_slice(
                        format!("{next_free:010} {generation:05} f \n").as_bytes(),
                    );
                }
                // A classic table cannot say "inside an object stream"; the callers never ask.
                _ => return Err(WriteError::EntryNotRepresentable.into()),
            }
        }
    }
    out.extend_from_slice(b"trailer\n");
    serializer.value(
        out,
        &Object::new(ObjectKind::Dict(trailer.clone())),
        crate::ObjRef::new(0, 0),
        0,
        false,
    )?;
    out.push(b'\n');
    Ok(())
}

/// Bytes needed to hold `value` big-endian (at least 1).
fn width(value: u64) -> usize {
    let bits = u64::BITS - value.leading_zeros();
    usize::try_from(bits.div_ceil(8)).unwrap_or(8).max(1)
}

fn push_be(out: &mut Vec<u8>, value: u64, width: usize) {
    let bytes = value.to_be_bytes();
    out.extend_from_slice(bytes.get(8 - width..).unwrap_or(&bytes));
}

/// Compresses `data` with zlib (`/FlateDecode`). Deterministic.
pub(super) fn flate_compress(data: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    // Writing to a Vec cannot fail.
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

/// The dictionary and data of a cross-reference stream for `entries` (sorted by object number,
/// any kind). `trailer` supplies the trailer keys (`/Size`, `/Root`, ...); this adds `/Type`,
/// `/W`, `/Index` and `/Filter`. The caller writes it as an indirect object.
pub(super) fn xref_stream(
    entries: &[(u32, XrefEntry)],
    trailer: &Dict<'_>,
) -> (Dict<'static>, Vec<u8>) {
    let (mut max_field2, mut max_field3) = (0u64, 0u64);
    for (_, entry) in entries {
        let (second, third) = fields(entry);
        max_field2 = max_field2.max(second);
        max_field3 = max_field3.max(third);
    }
    let (w2, w3) = (width(max_field2), width(max_field3));
    let mut data = Vec::with_capacity(entries.len() * (1 + w2 + w3));
    for (_, entry) in entries {
        let (second, third) = fields(entry);
        data.push(match entry {
            XrefEntry::Free { .. } => 0,
            XrefEntry::InUse { .. } => 1,
            _ => 2,
        });
        push_be(&mut data, second, w2);
        push_be(&mut data, third, w3);
    }

    let integer = |n: u64| Object::new(ObjectKind::Integer(i64::try_from(n).unwrap_or(i64::MAX)));
    let mut dict = Dict::default();
    dict.set(
        b"Type",
        Object::new(ObjectKind::Name(b"XRef".to_vec().into())),
    );
    for entry in &trailer.entries {
        dict.set(&entry.key, entry.value.clone().into_owned());
    }
    dict.set(
        b"W",
        Object::new(ObjectKind::Array(vec![
            integer(1),
            integer(w2 as u64),
            integer(w3 as u64),
        ])),
    );
    let index = runs(entries)
        .into_iter()
        .flat_map(|(first, count)| [integer(u64::from(first)), integer(u64::from(count))])
        .collect();
    dict.set(b"Index", Object::new(ObjectKind::Array(index)));
    dict.set(
        b"Filter",
        Object::new(ObjectKind::Name(b"FlateDecode".to_vec().into())),
    );
    (dict, flate_compress(&data))
}

/// The second and third field of a cross-reference stream row (§7.5.8.3, Table 20).
fn fields(entry: &XrefEntry) -> (u64, u64) {
    match *entry {
        XrefEntry::Free {
            next_free,
            generation,
        } => (next_free, u64::from(generation)),
        XrefEntry::InUse { offset, generation } => (offset, u64::from(generation)),
        XrefEntry::Compressed { stream, index } => (u64::from(stream), u64::from(index)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::Limits;

    fn in_use(offset: u64) -> XrefEntry {
        XrefEntry::InUse {
            offset,
            generation: 0,
        }
    }

    #[test]
    fn runs_group_consecutive_numbers() {
        let entries = [
            (0, in_use(1)),
            (1, in_use(1)),
            (2, in_use(1)),
            (5, in_use(1)),
            (7, in_use(1)),
            (8, in_use(1)),
        ];
        assert_eq!(runs(&entries), vec![(0, 3), (5, 1), (7, 2)]);
        assert_eq!(runs(&[]), Vec::<(u32, u32)>::new());
    }

    #[test]
    fn table_entries_are_twenty_bytes_each() {
        let limits = Limits::default();
        let entries = [
            (
                0,
                XrefEntry::Free {
                    next_free: 4,
                    generation: 65535,
                },
            ),
            (3, in_use(123)),
            (4, in_use(9_999_999_999)),
        ];
        let mut out = Vec::new();
        write_table(
            &mut out,
            &entries,
            &Dict::default(),
            &Serializer::new(&limits),
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text,
            "xref\n0 1\n0000000004 65535 f \n3 2\n0000000123 00000 n \n9999999999 00000 n \ntrailer\n<< >>\n"
        );
        for line in ["0000000004 65535 f \n", "0000000123 00000 n \n"] {
            assert_eq!(line.len(), 20);
        }
    }

    #[test]
    fn a_table_refuses_an_offset_it_cannot_write() {
        let limits = Limits::default();
        let error = write_table(
            &mut Vec::new(),
            &[(1, in_use(10_000_000_000))],
            &Dict::default(),
            &Serializer::new(&limits),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            crate::Error::Write {
                kind: WriteError::OffsetTooLarge(10_000_000_000)
            }
        ));
    }

    #[test]
    fn stream_rows_use_the_narrowest_widths() {
        let entries = [
            (
                0,
                XrefEntry::Free {
                    next_free: 0,
                    generation: 65535,
                },
            ),
            (1, in_use(300)),
            (
                2,
                XrefEntry::Compressed {
                    stream: 7,
                    index: 3,
                },
            ),
        ];
        let (dict, data) = xref_stream(&entries, &Dict::default());
        let w = dict.get(b"W").and_then(|o| match &o.kind {
            ObjectKind::Array(items) => Some(
                items
                    .iter()
                    .filter_map(Object::as_integer)
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        });
        assert_eq!(w, Some(vec![1, 2, 2]));
        let limits = Limits::default();
        let decoded = crate::filter::flate_decode(&data, &limits, None, None).unwrap();
        assert_eq!(
            decoded.data,
            [0, 0, 0, 0xFF, 0xFF, 1, 1, 0x2C, 0, 0, 2, 0, 7, 0, 3]
        );
    }
}
