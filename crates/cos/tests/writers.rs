//! Tests for the writers (M0 task 12): the canonical serializer's integration with the store, the
//! incremental writer and the full writer, on synthetic files and on the encryption fixtures.
//!
//! The test that runs `qpdf --check` on every output needs qpdf (on `PATH`, or its path in the
//! `QPDF` environment variable, see `docs/dev/setup.md`) and fails without it; CI installs it on
//! every OS.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use vellora_cos::filter::decode_stream;
use vellora_cos::xref::XrefEntry;
use vellora_cos::{
    Changes, EncryptionError, EncryptionPolicy, Error, FullOptions, LimitKind, Limits, NewObject,
    ObjRef, Object, ObjectKind, ObjectStore, PasswordRole, RepairReason, SectionKind, WriteError,
    Xref, incremental_update, write_full,
};

const USER: &[u8] = b"user-pw";

// ---- building files -------------------------------------------------------------------------

const ID0: [u8; 16] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
];

/// Appends objects with correct offsets, then a classic table.
struct Builder {
    data: Vec<u8>,
    offsets: Vec<(u32, usize)>,
}

impl Builder {
    fn new(version: &str) -> Self {
        Self {
            data: format!("%PDF-{version}\n").into_bytes(),
            offsets: Vec::new(),
        }
    }

    fn object(&mut self, number: u32, body: &str) {
        self.offsets.push((number, self.data.len()));
        write!(self.data_string(), "{number} 0 obj\n{body}\nendobj\n").unwrap();
    }

    fn stream(&mut self, number: u32, dict: &str, content: &str) {
        let body = format!(
            "<< {dict} /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        );
        self.object(number, &body);
    }

    fn data_string(&mut self) -> StringSink<'_> {
        StringSink(&mut self.data)
    }
}

/// Lets `write!` append to the byte buffer.
struct StringSink<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for StringSink<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

const SAMPLE_TRAILER: &str = "/Root 1 0 R /Info 6 0 R /ID [<00112233445566778899AABBCCDDEEFF> <FFEEDDCCBBAA99887766554433221100>]";

/// Catalog 1, pages 2, two pages 3 and 4 (3 has the content stream 5), info 6, and 7 that
/// nothing refers to. A classic table.
fn sample(version: &str) -> Vec<u8> {
    let mut pdf = Builder::new(version);
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R /Missing 99 0 R >>");
    pdf.object(
        2,
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /Resources << >> >>",
    );
    pdf.object(
        3,
        "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /MediaBox [0 0 200 200] >>",
    );
    pdf.object(4, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>");
    pdf.stream(5, "", "BT (A) Tj ET");
    pdf.object(6, "<< /Title (Original) >>");
    pdf.object(7, "<< /Junk true >>");
    finish_table(pdf, SAMPLE_TRAILER)
}

fn finish_table(pdf: Builder, trailer: &str) -> Vec<u8> {
    let mut data = pdf.data;
    let size = pdf.offsets.iter().map(|&(n, _)| n).max().unwrap() + 1;
    let at = data.len();
    let mut table = format!("xref\n0 {size}\n");
    for number in 0..size {
        match pdf.offsets.iter().find(|&&(n, _)| n == number) {
            Some(&(_, offset)) => writeln!(table, "{offset:010} 00000 n ").unwrap(),
            None => table.push_str("0000000000 65535 f \n"),
        }
    }
    write!(
        table,
        "trailer\n<< /Size {size} {trailer} >>\nstartxref\n{at}\n%%EOF\n"
    )
    .unwrap();
    data.extend_from_slice(table.as_bytes());
    data
}

/// A hybrid-reference file (§7.5.8.4): page 3 lives in object stream 4, which only the
/// cross-reference stream 5 (named by `/XRefStm`) says; the table calls 3 free.
fn hybrid() -> Vec<u8> {
    let mut pdf = Builder::new("1.5");
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    pdf.object(
        2,
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << >> >>",
    );
    let member = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>";
    let header = "3 0 ";
    pdf.stream(
        4,
        &format!("/Type /ObjStm /N 1 /First {}", header.len()),
        &format!("{header}{member}"),
    );
    // One row: object 3 is compressed in stream 4 at index 0 (type 2, field 2 = 4, field 3 = 0).
    let rows = "\u{2}\u{0}\u{4}\u{0}";
    let stream_at = pdf.data.len();
    pdf.offsets.push((5, stream_at));
    write!(
        pdf.data_string(),
        "5 0 obj\n<< /Type /XRef /W [1 2 1] /Index [3 1] /Size 6 /Length 4 >>\nstream\n{rows}\nendstream\nendobj\n"
    )
    .unwrap();
    let trailer = format!("/Root 1 0 R /XRefStm {stream_at}");
    let mut data = finish_table(pdf, &trailer);
    // The table lists 3 as free already (a gap is a free entry), as hybrid files do.
    assert!(Xref::parse(&data, &Limits::default()).is_ok());
    data.shrink_to_fit();
    data
}

fn options(xref_stream: bool, object_streams: bool, encryption: EncryptionPolicy) -> FullOptions {
    let mut options = FullOptions::default();
    options.xref_stream = xref_stream;
    options.object_streams = object_streams;
    options.encryption = encryption;
    options
}

fn full(data: &[u8], options: FullOptions) -> Vec<u8> {
    let store = open(data);
    write_full(&store, &options).unwrap()
}

fn open(data: &[u8]) -> ObjectStore<'_> {
    ObjectStore::open(data, Limits::default()).unwrap()
}

/// The original file followed by the appended section.
fn updated(original: &[u8], changes: &Changes) -> Vec<u8> {
    let store = open(original);
    let appended = incremental_update(&store, changes).unwrap();
    [original, &appended].concat()
}

// ---- reading back ---------------------------------------------------------------------------

fn text(value: &str) -> Object<'static> {
    Object::new(ObjectKind::String(value.as_bytes().to_vec().into()))
}

fn name(value: &str) -> Object<'static> {
    Object::new(ObjectKind::Name(value.as_bytes().to_vec().into()))
}

fn dictionary(entries: Vec<(&str, Object<'static>)>) -> Object<'static> {
    let mut dict = vellora_cos::object::Dict::default();
    for (key, value) in entries {
        dict.set(key.as_bytes(), value);
    }
    Object::new(ObjectKind::Dict(dict))
}

fn title_changes(number: u32, title: &str) -> Changes {
    let mut changes = Changes::new();
    changes.set_object(
        number,
        NewObject::Value(dictionary(vec![("Title", text(title))])),
    );
    changes
}

fn string_entry(object: &Object<'_>, key: &str) -> Option<Vec<u8>> {
    match &object.as_dict()?.get(key.as_bytes())?.kind {
        ObjectKind::String(s) => Some(s.to_vec()),
        _ => None,
    }
}

fn title(store: &ObjectStore<'_>) -> Vec<u8> {
    let info = store.info().unwrap().expect("an /Info dictionary");
    string_entry(&info, "Title").expect("a /Title string")
}

fn ids(store: &ObjectStore<'_>) -> Option<(Vec<u8>, Vec<u8>)> {
    let trailer = store.trailer();
    let ObjectKind::Array(items) = &trailer.get(b"ID")?.kind else {
        return None;
    };
    let string = |o: &Object<'_>| match &o.kind {
        ObjectKind::String(s) => s.to_vec(),
        _ => Vec::new(),
    };
    Some((string(&items[0]), string(&items[1])))
}

fn decoded(store: &ObjectStore<'_>, reference: ObjRef) -> Vec<u8> {
    let stream = store.resolve(reference).unwrap();
    let raw = store.stream_decrypted(reference).unwrap().unwrap();
    decode_stream(
        stream.as_dict().unwrap(),
        &raw,
        &Limits::default(),
        None,
        None,
    )
    .unwrap()
    .data
}

fn reference_of(object: &Object<'_>, key: &str) -> ObjRef {
    match object.as_dict().unwrap().get(key.as_bytes()).unwrap().kind {
        ObjectKind::Ref(reference) => reference,
        _ => panic!("/{key} is not a reference"),
    }
}

fn newest_section(data: &[u8]) -> vellora_cos::XrefSection<'_> {
    Xref::parse(data, &Limits::default())
        .unwrap()
        .revisions
        .remove(0)
        .section
}

fn revisions(data: &[u8]) -> usize {
    Xref::parse(data, &Limits::default())
        .unwrap()
        .revisions
        .len()
}

fn page_count(store: &ObjectStore<'_>) -> usize {
    store.pages().map(|p| p.unwrap()).collect::<Vec<_>>().len()
}

// ---- incremental writer: synthetic files ----------------------------------------------------

/// The documents the incremental tests run on, by xref style.
fn styles() -> Vec<(&'static str, Vec<u8>)> {
    let classic = sample("1.4");
    let stream = full(&classic, options(true, false, EncryptionPolicy::Remove));
    let packed = full(&classic, options(true, true, EncryptionPolicy::Remove));
    vec![
        ("table", classic),
        ("xref stream", stream),
        ("object streams", packed),
    ]
}

#[test]
fn an_update_is_the_original_bytes_plus_an_appended_section() {
    for (style, original) in styles() {
        let before = open(&original);
        let (id0, id1) = ids(&before).unwrap();
        let mut changes = title_changes(reference_of_trailer(&before, "Info").num, "Changed");
        let added = changes.add_object(
            &before,
            NewObject::Value(dictionary(vec![("Note", text("new"))])),
        );
        let after_bytes = updated(&original, &changes);

        assert_eq!(
            &after_bytes[..original.len()],
            original.as_slice(),
            "{style}: the original bytes are a prefix"
        );
        let after = open(&after_bytes);
        assert!(
            after.repaired().is_empty(),
            "{style}: {:?}",
            after.repaired()
        );
        assert_eq!(title(&after), b"Changed", "{style}");
        let note = after.resolve(added).unwrap();
        assert_eq!(string_entry(&note, "Note").unwrap(), b"new", "{style}");
        assert_eq!(revisions(&after_bytes), revisions(&original) + 1, "{style}");
        assert_eq!(page_count(&after), 2, "{style}: pages are untouched");

        let (new_id0, new_id1) = ids(&after).unwrap();
        assert_eq!(new_id0, id0, "{style}: /ID[0] never changes");
        assert_ne!(new_id1, id1, "{style}: /ID[1] follows the update");

        let section = newest_section(&after_bytes);
        let expected_kind = if style == "table" {
            SectionKind::Table
        } else {
            SectionKind::Stream
        };
        assert_eq!(
            section.kind, expected_kind,
            "{style}: same style as the file"
        );
        assert!(after_bytes.ends_with(b"%%EOF\n"), "{style}");
    }
}

fn reference_of_trailer(store: &ObjectStore<'_>, key: &str) -> ObjRef {
    match store.trailer().get(key.as_bytes()).unwrap().kind {
        ObjectKind::Ref(reference) => reference,
        _ => panic!("/{key} is not a reference"),
    }
}

#[test]
fn the_appended_section_has_the_expected_shape() {
    let original = sample("1.4");
    let appended = {
        let store = open(&original);
        incremental_update(&store, &title_changes(6, "Changed")).unwrap()
    };
    let text = String::from_utf8(appended.clone()).unwrap();
    // The sample ends with a newline, so the section starts with the object.
    let object_at = original.len();
    assert!(
        text.starts_with("6 0 obj\n<< /Title (Changed) >>\nendobj\n"),
        "{text}"
    );
    let xref_at = text.find("xref\n").unwrap();
    assert_eq!(&text[xref_at..xref_at + "xref\n6 1\n".len()], "xref\n6 1\n");
    assert!(
        text[xref_at..].contains(&format!("{object_at:010} 00000 n \n")),
        "{text}"
    );
    assert!(text.contains("/Size 8"), "{text}");
    assert!(text.contains("/Root 1 0 R"), "{text}");
    assert!(text.contains("/Info 6 0 R"), "{text}");
    let previous = original
        .windows(10)
        .rposition(|w| w == b"startxref\n")
        .map(|at| {
            String::from_utf8_lossy(&original[at + 10..])
                .lines()
                .next()
                .unwrap()
                .to_string()
        })
        .unwrap();
    assert!(text.contains(&format!("/Prev {previous}")), "{text}");
    assert!(
        text.ends_with(&format!("startxref\n{}\n%%EOF\n", original.len() + xref_at)),
        "{text}"
    );
}

#[test]
fn an_update_starts_on_a_new_line_when_the_file_has_no_final_eol() {
    let mut original = sample("1.4");
    while original.last() == Some(&b'\n') {
        original.pop();
    }
    let appended = {
        let store = open(&original);
        incremental_update(&store, &title_changes(6, "Changed")).unwrap()
    };
    assert_eq!(appended[0], b'\n');
    let after = [original.as_slice(), &appended].concat();
    assert_eq!(title(&open(&after)), b"Changed");
}

#[test]
fn freed_objects_are_chained_through_object_zero_and_read_as_null() {
    for (style, original) in styles() {
        let store = open(&original);
        let junk = match style {
            // Numbers differ after a full rewrite: free the document's last object.
            "table" => 7,
            _ => {
                u32::try_from(
                    Xref::parse(&original, &Limits::default())
                        .unwrap()
                        .merged()
                        .len(),
                )
                .unwrap()
                    - 1
            }
        };
        let before = store.resolve(ObjRef::new(junk, 0)).unwrap();
        assert!(!matches!(before.kind, ObjectKind::Null), "{style}");
        let mut changes = Changes::new();
        changes.free(junk);
        changes.free(junk); // twice is the same as once
        drop(store);
        let after_bytes = updated(&original, &changes);
        let after = open(&after_bytes);
        assert!(
            matches!(
                after.resolve(ObjRef::new(junk, 0)).unwrap().kind,
                ObjectKind::Null
            ),
            "{style}"
        );
        let section = newest_section(&after_bytes);
        let entries: Vec<_> = section.entries.clone();
        assert!(
            entries.contains(&(
                0,
                XrefEntry::Free {
                    next_free: u64::from(junk),
                    generation: u16::MAX
                }
            )),
            "{style}: {entries:?}"
        );
        assert!(
            entries.contains(&(
                junk,
                XrefEntry::Free {
                    next_free: 0,
                    generation: 1
                }
            )),
            "{style}: {entries:?}"
        );
    }
}

#[test]
fn a_replaced_object_keeps_its_generation_and_a_new_one_gets_the_next_number() {
    let original = sample("1.4");
    let store = open(&original);
    let mut changes = Changes::new();
    let first = changes.add_object(&store, NewObject::Value(text("a")));
    let second = changes.add_object(&store, NewObject::Value(text("b")));
    assert_eq!((first.num, second.num), (8, 9));
    let after_bytes = updated(&original, &changes);
    let after = open(&after_bytes);
    assert!(matches!(
        &after.resolve(second).unwrap().kind,
        ObjectKind::String(s) if s.as_ref() == b"b"
    ));
    assert_eq!(
        newest_section(&after_bytes)
            .trailer
            .get(b"Size")
            .and_then(Object::as_integer),
        Some(10)
    );
}

#[test]
fn updates_chain_and_each_keeps_the_ones_before_it_byte_for_byte() {
    let original = sample("1.4");
    let first = updated(&original, &title_changes(6, "One"));
    let second = updated(&first, &title_changes(6, "Two"));
    let third = updated(&second, &title_changes(6, "Three"));
    assert_eq!(&third[..first.len()], first.as_slice());
    assert_eq!(&third[..original.len()], original.as_slice());
    assert_eq!(revisions(&third), 4);
    let store = open(&third);
    assert_eq!(title(&store), b"Three");
    let (id0, id1) = ids(&store).unwrap();
    assert_eq!(id0, ID0);
    let all: Vec<_> = [&original, &first, &second, &third]
        .iter()
        .map(|d| ids(&open(d)).unwrap().1)
        .collect();
    assert!(all.iter().all(|id| id.len() == 16));
    assert_eq!(all[3], id1);
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(a, b, "every revision has its own /ID[1]");
        }
    }
}

#[test]
fn updates_are_deterministic() {
    let original = sample("1.4");
    assert_eq!(
        updated(&original, &title_changes(6, "Same")),
        updated(&original, &title_changes(6, "Same"))
    );
}

#[test]
fn a_file_without_an_id_gets_one_and_a_file_with_one_keeps_its_first_string() {
    let mut pdf = Builder::new("1.7");
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    pdf.object(2, "<< /Type /Pages /Kids [] /Count 0 >>");
    let original = finish_table(pdf, "/Root 1 0 R");
    let after_bytes = updated(&original, &Changes::new());
    let after = open(&after_bytes);
    let (a, b) = ids(&after).unwrap();
    assert_eq!((a.len(), a), (16, b));
}

#[test]
fn an_update_can_set_the_info_reference() {
    let mut pdf = Builder::new("1.7");
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    pdf.object(2, "<< /Type /Pages /Kids [] /Count 0 >>");
    let original = finish_table(pdf, "/Root 1 0 R");
    assert!(open(&original).info().unwrap().is_none());
    let store = open(&original);
    let mut changes = Changes::new();
    let info = changes.add_object(
        &store,
        NewObject::Value(dictionary(vec![("Title", text("Added"))])),
    );
    changes.set_info(info);
    drop(store);
    let after = updated(&original, &changes);
    assert_eq!(title(&open(&after)), b"Added");
}

#[test]
fn a_stream_is_written_with_its_length_and_data() {
    let original = sample("1.4");
    let store = open(&original);
    let mut changes = Changes::new();
    // A wrong /Length in the given dictionary must not matter.
    let mut dict = vellora_cos::object::Dict::default();
    dict.set(b"Length", Object::new(ObjectKind::Integer(1)));
    let stream = changes.add_object(
        &store,
        NewObject::Stream {
            dict,
            data: b"hello stream".to_vec(),
        },
    );
    drop(store);
    let after_bytes = updated(&original, &changes);
    let after = open(&after_bytes);
    let object = after.resolve(stream).unwrap();
    assert_eq!(
        object
            .as_dict()
            .unwrap()
            .get(b"Length")
            .and_then(Object::as_integer),
        Some(12),
        "the writer replaces /Length"
    );
    assert_eq!(decoded(&after, stream), b"hello stream");
}

#[test]
fn replacing_an_object_inside_an_object_stream_and_in_a_hybrid_file() {
    let page = dictionary(vec![
        ("Type", name("Page")),
        ("MediaBox", {
            let n = |v| Object::new(ObjectKind::Integer(v));
            Object::new(ObjectKind::Array(vec![n(0), n(0), n(20), n(20)]))
        }),
    ]);
    let hybrid = hybrid();
    let packed = full(
        &sample("1.4"),
        options(true, true, EncryptionPolicy::Remove),
    );
    for (style, original) in [("hybrid", hybrid), ("packed", packed)] {
        let store = open(&original);
        let page_ref = store.pages().next().unwrap().unwrap().reference;
        assert!(
            matches!(
                Xref::parse(&original, &Limits::default())
                    .unwrap()
                    .lookup(page_ref.num),
                Some(XrefEntry::Compressed { .. })
            ),
            "{style}: the page starts inside an object stream"
        );
        let mut changes = Changes::new();
        changes.set_object(page_ref.num, NewObject::Value(page.clone()));
        drop(store);
        let after_bytes = updated(&original, &changes);
        let after = open(&after_bytes);
        assert!(after.repaired().is_empty(), "{style}");
        let replaced = after.resolve(page_ref).unwrap();
        let media = match &replaced.as_dict().unwrap().get(b"MediaBox").unwrap().kind {
            ObjectKind::Array(items) => items.iter().filter_map(Object::as_integer).collect(),
            _ => Vec::new(),
        };
        assert_eq!(media, [0, 0, 20, 20], "{style}");
        assert!(
            matches!(
                Xref::parse(&after_bytes, &Limits::default())
                    .unwrap()
                    .lookup(page_ref.num),
                Some(XrefEntry::InUse { generation: 0, .. })
            ),
            "{style}: now stored as a plain object"
        );
    }
}

// ---- incremental writer: refusals -----------------------------------------------------------

#[test]
fn a_repaired_file_is_refused_and_the_full_writer_takes_it() {
    let mut original = sample("1.4");
    let at = original
        .windows(9)
        .rposition(|w| w == b"startxref")
        .unwrap();
    original.truncate(at);
    let store = open(&original);
    assert_ne!(store.repaired().len(), 0);
    let error = incremental_update(&store, &Changes::new()).unwrap_err();
    assert!(
        matches!(
            error,
            Error::Write {
                kind: WriteError::NeedsFullRewrite
            }
        ),
        "{error:?}"
    );
    let rewritten = write_full(&store, &FullOptions::default()).unwrap();
    let again = open(&rewritten);
    assert!(again.repaired().is_empty(), "{:?}", again.repaired());
    assert_eq!(page_count(&again), 2);
    assert_eq!(title(&again), b"Original");
}

/// An object stream that does not decode, holding the page tree (object 2). The catalog also
/// refers to the page (3), so that the writer reads it.
fn with_unreadable_object_stream(pdf: &mut Builder) {
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R /Extra 3 0 R >>");
    pdf.object(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>");
    pdf.stream(
        8,
        "/Type /ObjStm /N 1 /First 4 /Filter /FlateDecode",
        "not flate data",
    );
}

#[test]
fn a_full_rewrite_refuses_a_file_whose_repair_lost_an_object_stream() {
    // Found by fuzzing (write_roundtrip): the damaged object stream's objects are `null` after
    // the repair scan, and the writer used to write the document without its pages.
    let mut pdf = Builder::new("1.5");
    with_unreadable_object_stream(&mut pdf);
    let mut data = pdf.data;
    data.extend_from_slice(
        b"trailer
<< /Root 1 0 R >>
",
    );
    let store = open(&data);
    assert!(
        store
            .repaired()
            .contains(&RepairReason::ObjectStreamNotExpanded { stream: 8 }),
        "{:?}",
        store.repaired()
    );
    for (xref_stream, object_streams) in [(false, false), (true, true)] {
        let error = write_full(
            &store,
            &options(xref_stream, object_streams, EncryptionPolicy::Remove),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                Error::Write {
                    kind: WriteError::ObjectStreamLost { stream: 8 }
                }
            ),
            "{error:?}"
        );
    }
}

#[test]
fn the_refusal_also_holds_when_the_repair_only_happens_during_the_write() {
    // The table is fine as far as `open` can tell; object 3's offset is wrong, which the store
    // finds out (and rebuilds the table for) only when the writer reads it. The same writer
    // call must refuse, and so must every call after it.
    let mut pdf = Builder::new("1.5");
    with_unreadable_object_stream(&mut pdf);
    for entry in &mut pdf.offsets {
        if entry.0 == 3 {
            entry.1 += 3;
        }
    }
    let data = finish_table(pdf, "/Root 1 0 R");
    let store = open(&data);
    assert_eq!(store.repaired(), Vec::<RepairReason>::new());
    for _ in 0..2 {
        let error = write_full(&store, &FullOptions::default()).unwrap_err();
        assert!(
            matches!(
                error,
                Error::Write {
                    kind: WriteError::ObjectStreamLost { stream: 8 }
                }
            ),
            "{error:?}"
        );
    }
}

#[test]
fn a_wrong_offset_the_update_does_not_touch_still_refuses_it() {
    // The store only notices a wrong offset when it reads that object, so a file can open without
    // repairs and still have a table an update would build on.
    let original = String::from_utf8(sample("1.4")).unwrap();
    let at = original.find("\n4 0 obj").unwrap() + 1;
    let good = format!("{at:010} 00000 n ");
    let bad = format!("{:010} 00000 n ", at + 2);
    assert!(original.contains(&good));
    let damaged = original.replacen(&good, &bad, 1).into_bytes();
    let store = open(&damaged);
    assert_eq!(store.repaired().len(), 0, "not noticed on open");
    let error = incremental_update(&store, &title_changes(6, "Changed")).unwrap_err();
    assert!(
        matches!(
            error,
            Error::Write {
                kind: WriteError::NeedsFullRewrite
            }
        ),
        "{error:?}"
    );
    // The full writer reads every object and so finds and repairs it.
    let rewritten = write_full(&store, &FullOptions::default()).unwrap();
    let again = open(&rewritten);
    assert_eq!(again.repaired().len(), 0);
    assert_eq!(page_count(&again), 2);
}

#[test]
fn bad_requests_are_typed_errors() {
    let original = sample("1.4");
    let store = open(&original);

    let mut zero = Changes::new();
    zero.set_object(0, NewObject::Value(text("x")));
    assert!(matches!(
        incremental_update(&store, &zero),
        Err(Error::Write {
            kind: WriteError::InvalidObjectNumber(0)
        })
    ));

    let mut stream = Changes::new();
    stream.set_object(
        6,
        NewObject::Value(Object::new(ObjectKind::Stream(vellora_cos::Stream {
            dict: vellora_cos::object::Dict::default(),
            data: 0..0,
        }))),
    );
    assert!(matches!(
        incremental_update(&store, &stream),
        Err(Error::Write {
            kind: WriteError::StreamWithoutData
        })
    ));

    let mut deep = Object::new(ObjectKind::Null);
    for _ in 0..100 {
        deep = Object::new(ObjectKind::Array(vec![deep]));
    }
    let mut nested = Changes::new();
    nested.set_object(6, NewObject::Value(deep));
    assert!(matches!(
        incremental_update(&store, &nested),
        Err(Error::LimitExceeded {
            limit: LimitKind::NestingDepth,
            ..
        })
    ));

    let keep = options(false, false, EncryptionPolicy::Keep);
    assert!(matches!(
        write_full(&store, &keep),
        Err(Error::Write {
            kind: WriteError::NotEncrypted
        })
    ));
}

#[test]
fn a_document_that_needs_xref_numbers_beyond_the_limit_is_refused() {
    let original = sample("1.4");
    let mut limits = Limits::default();
    limits.max_xref_entries = 10;
    let store = ObjectStore::open(&original, limits).unwrap();
    let mut changes = Changes::new();
    changes.set_object(500, NewObject::Value(text("far away")));
    assert!(matches!(
        incremental_update(&store, &changes),
        Err(Error::LimitExceeded {
            limit: LimitKind::XrefEntries,
            ..
        })
    ));
}

// ---- full writer: synthetic files -----------------------------------------------------------

#[test]
fn a_full_rewrite_keeps_the_reachable_graph_renumbered_and_drops_the_rest() {
    let original = sample("1.4");
    let rewritten = full(&original, FullOptions::default());
    assert!(rewritten.starts_with(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n"));
    assert!(rewritten.ends_with(b"%%EOF\n"));
    let store = open(&rewritten);
    assert!(store.repaired().is_empty(), "{:?}", store.repaired());
    assert_eq!(revisions(&rewritten), 1);
    let xref = Xref::parse(&rewritten, &Limits::default()).unwrap();
    // Catalog, pages, two pages, the content stream and the info: object 7 is gone.
    assert_eq!(xref.in_use_count(), 6, "the free head is not counted");
    assert_eq!(page_count(&store), 2);
    assert_eq!(title(&store), b"Original");
    let page = store.pages().next().unwrap().unwrap();
    assert_eq!(
        decoded(&store, reference_of(&page.object, "Contents")),
        b"BT (A) Tj ET"
    );
    let (a, b) = ids(&store).unwrap();
    assert_eq!((a.len(), a), (16, b), "a fresh /ID in both slots");
    assert_ne!(ids(&open(&original)).unwrap().0, ids(&store).unwrap().0);
    // A reference to a missing object reads as null (§7.3.10) and is written as `null`.
    let root = store.root().unwrap().unwrap();
    assert!(matches!(
        root.as_dict().unwrap().get(b"Missing").unwrap().kind,
        ObjectKind::Null
    ));
    // Dense numbers 1..=6.
    for number in 1..=6 {
        assert!(
            !matches!(
                store.resolve(ObjRef::new(number, 0)).unwrap().kind,
                ObjectKind::Null
            ),
            "object {number}"
        );
    }
    assert!(matches!(
        store.resolve(ObjRef::new(7, 0)).unwrap().kind,
        ObjectKind::Null
    ));
}

#[test]
fn full_rewrites_are_deterministic() {
    let original = sample("1.4");
    for (xref_stream, object_streams) in [(false, false), (true, false), (true, true)] {
        let o = options(xref_stream, object_streams, EncryptionPolicy::Remove);
        assert_eq!(full(&original, o), full(&original, o));
    }
}

#[test]
fn xref_streams_and_object_streams_need_and_set_version_1_5() {
    let original = sample("1.4");
    let stream = full(&original, options(true, false, EncryptionPolicy::Remove));
    assert!(stream.starts_with(b"%PDF-1.5\n"));
    assert_eq!(newest_section(&stream).kind, SectionKind::Stream);
    // `object_streams` alone implies a cross-reference stream.
    let packed = full(&original, options(false, true, EncryptionPolicy::Remove));
    assert_eq!(newest_section(&packed).kind, SectionKind::Stream);
    // A newer header stays.
    let seven = full(
        &sample("1.7"),
        options(true, true, EncryptionPolicy::Remove),
    );
    assert!(seven.starts_with(b"%PDF-1.7\n"));
}

#[test]
fn object_streams_hold_every_object_but_the_streams() {
    let original = sample("1.4");
    let packed = full(&original, options(true, true, EncryptionPolicy::Remove));
    let xref = Xref::parse(&packed, &Limits::default()).unwrap();
    let merged = xref.merged();
    let compressed = merged
        .values()
        .filter(|e| matches!(e, XrefEntry::Compressed { .. }))
        .count();
    // Catalog, pages, two pages and the info; the content stream and the containers are plain.
    assert_eq!(compressed, 5);
    let store = open(&packed);
    assert!(store.repaired().is_empty(), "{:?}", store.repaired());
    assert_eq!(page_count(&store), 2);
    assert_eq!(title(&store), b"Original");
    let page = store.pages().next().unwrap().unwrap();
    assert_eq!(
        decoded(&store, reference_of(&page.object, "Contents")),
        b"BT (A) Tj ET"
    );
}

#[test]
fn many_objects_are_split_over_several_object_streams() {
    let mut pdf = Builder::new("1.7");
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    let kids = (3..=253).fold(String::new(), |mut kids, n| {
        write!(kids, "{n} 0 R ").unwrap();
        kids
    });
    pdf.object(
        2,
        &format!("<< /Type /Pages /Kids [{kids}] /Count 251 /Resources << >> >>"),
    );
    for n in 3..=253 {
        pdf.object(n, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 5 5] >>");
    }
    let original = finish_table(pdf, "/Root 1 0 R");
    let packed = full(&original, options(true, true, EncryptionPolicy::Remove));
    let store = open(&packed);
    assert_eq!(page_count(&store), 251);
    assert_eq!(store.repaired().len(), 0);
    let xref = Xref::parse(&packed, &Limits::default()).unwrap();
    let containers: std::collections::BTreeSet<u32> = xref
        .merged()
        .values()
        .filter_map(|e| match e {
            XrefEntry::Compressed { stream, .. } => Some(*stream),
            _ => None,
        })
        .collect();
    assert_eq!(containers.len(), 3, "253 objects at 100 per stream");
}

#[test]
fn a_full_rewrite_of_a_hybrid_file_reads_the_same() {
    let original = hybrid();
    let rewritten = full(&original, FullOptions::default());
    let store = open(&rewritten);
    assert_eq!(store.repaired().len(), 0);
    assert_eq!(page_count(&store), 1);
    let page = store.pages().next().unwrap().unwrap();
    assert!(page.object.as_dict().unwrap().get(b"MediaBox").is_some());
}

// ---- encrypted documents --------------------------------------------------------------------

const FIXTURES: [&str; 10] = [
    "r2-rc4-40",
    "r2-rc4-40-user-password",
    "r3-rc4-128",
    "r3-rc4-128-user-password",
    "r4-rc4-128",
    "r4-aes-128",
    "r4-aes-128-user-password",
    "r4-aes-128-no-metadata",
    "r6-aes-256",
    "r6-aes-256-user-password",
];

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/encryption")
        .join(format!("{name}.pdf"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Opens a fixture, unlocking it with the user password when it has one.
fn open_fixture<'a>(name: &str, data: &'a [u8]) -> ObjectStore<'a> {
    let store = open(data);
    if name.ends_with("user-password") {
        assert!(store.is_locked(), "{name}");
        assert_eq!(store.authenticate(USER).unwrap(), PasswordRole::User);
    }
    store
}

/// Everything the fixtures contain that a writer must carry over.
#[derive(Debug, PartialEq)]
struct Content {
    marker: Vec<u8>,
    reason: Vec<u8>,
    signature: Vec<u8>,
    page_text: Vec<u8>,
    metadata: Vec<u8>,
}

fn content(store: &ObjectStore<'_>) -> Content {
    let root = store.root().unwrap().unwrap();
    let sig = store.resolve(reference_of(&root, "SigTest")).unwrap();
    let page = store.pages().next().unwrap().unwrap();
    Content {
        marker: string_entry(&root, "Marker").unwrap(),
        reason: string_entry(&sig, "Reason").unwrap(),
        signature: string_entry(&sig, "Contents").unwrap(),
        page_text: decoded(store, reference_of(&page.object, "Contents")),
        metadata: decoded(store, reference_of(&root, "Metadata")),
    }
}

#[test]
fn the_fixtures_have_the_content_the_tests_expect() {
    for name in FIXTURES {
        let data = fixture(name);
        let store = open_fixture(name, &data);
        let c = content(&store);
        assert_eq!(c.marker, b"Catalog marker", "{name}");
        assert_eq!(c.signature, b"SIGNATURE-BYTES-0123456789", "{name}");
        assert!(String::from_utf8_lossy(&c.page_text).contains("Hello, encrypted world"));
        assert_eq!(title(&store), b"Secret title", "{name}");
    }
}

#[test]
fn an_update_of_an_encrypted_file_is_encrypted_with_the_existing_key() {
    let mut saw_indirect_encrypt = false;
    for name in FIXTURES {
        let original = fixture(name);
        let store = open_fixture(name, &original);
        let before = content(&store);
        let (id0, _) = ids(&store).unwrap();
        let info = reference_of_trailer(&store, "Info");
        let mut changes = Changes::new();
        changes.set_object(
            info.num,
            NewObject::Value(dictionary(vec![
                ("Title", text("New secret title")),
                (
                    "Author",
                    Object::new(ObjectKind::String(vec![0xE9, 0x00, 0xFF].into())),
                ),
            ])),
        );
        let stream = changes.add_object(
            &store,
            NewObject::Stream {
                dict: vellora_cos::object::Dict::default(),
                data: b"plain stream text".to_vec(),
            },
        );
        let appended = incremental_update(&store, &changes).unwrap();
        let after_bytes = [original.as_slice(), &appended].concat();

        let clear = |needle: &[u8]| appended.windows(needle.len()).any(|w| w == needle);
        assert!(!clear(b"New secret title"), "{name}: strings are encrypted");
        assert!(!clear(b"(New secret"), "{name}");
        assert!(
            !clear(b"plain stream text"),
            "{name}: streams are encrypted"
        );
        assert!(
            appended.windows(8).any(|w| w == b"/Encrypt"),
            "{name}: /Encrypt is carried over"
        );

        let after = open_fixture(name, &after_bytes);
        assert!(
            after.repaired().is_empty(),
            "{name}: {:?}",
            after.repaired()
        );
        assert_eq!(title(&after), b"New secret title", "{name}");
        let info = after.info().unwrap().unwrap();
        assert_eq!(
            string_entry(&info, "Author").unwrap(),
            [0xE9, 0x00, 0xFF],
            "{name}"
        );
        assert_eq!(decoded(&after, stream), b"plain stream text", "{name}");
        assert_eq!(
            content(&after),
            before,
            "{name}: everything else reads as before"
        );
        assert_eq!(
            ids(&after).unwrap().0,
            id0,
            "{name}: /ID[0] is part of the key"
        );
        assert_eq!(after.encryption_info(), store.encryption_info(), "{name}");

        // The /Encrypt dictionary is off limits.
        if let ObjectKind::Ref(encrypt) = store.trailer().get(b"Encrypt").unwrap().kind {
            saw_indirect_encrypt = true;
            let mut forbidden = Changes::new();
            forbidden.set_object(encrypt.num, NewObject::Value(text("x")));
            assert!(
                matches!(
                    incremental_update(&store, &forbidden),
                    Err(Error::Write {
                        kind: WriteError::EncryptionDictionary(_)
                    })
                ),
                "{name}"
            );
        }
    }
    assert!(
        saw_indirect_encrypt,
        "the fixtures should cover an indirect /Encrypt"
    );
}

#[test]
fn a_locked_document_cannot_be_written() {
    let data = fixture("r4-aes-128-user-password");
    let store = open(&data);
    assert!(store.is_locked());
    let incremental = incremental_update(&store, &Changes::new()).unwrap_err();
    assert!(
        matches!(
            incremental,
            Error::Encryption {
                kind: EncryptionError::PasswordRequired
            }
        ),
        "{incremental:?}"
    );
    let rewrite = write_full(&store, &FullOptions::default()).unwrap_err();
    assert!(matches!(
        rewrite,
        Error::Encryption {
            kind: EncryptionError::PasswordRequired
        }
    ));
}

#[test]
fn a_full_rewrite_of_an_encrypted_file_is_unencrypted_by_default() {
    for name in FIXTURES {
        let original = fixture(name);
        let store = open_fixture(name, &original);
        let before = content(&store);
        for (xref_stream, object_streams) in [(false, false), (true, true)] {
            let rewritten = write_full(
                &store,
                &options(xref_stream, object_streams, EncryptionPolicy::Remove),
            )
            .unwrap();
            let after = open(&rewritten);
            assert!(!after.is_locked(), "{name}: no password needed");
            assert!(after.encryption_info().is_none(), "{name}");
            assert!(
                after.repaired().is_empty(),
                "{name}: {:?}",
                after.repaired()
            );
            assert!(
                !rewritten.windows(8).any(|w| w == b"/Encrypt"),
                "{name}: no /Encrypt left"
            );
            assert_eq!(
                content(&after),
                before,
                "{name} ({xref_stream}/{object_streams})"
            );
            assert_eq!(title(&after), b"Secret title", "{name}");
        }
    }
}

#[test]
fn re_encryption_keeps_the_key_the_dictionary_and_the_first_id() {
    for name in FIXTURES {
        let original = fixture(name);
        let store = open_fixture(name, &original);
        let before = content(&store);
        let (id0, id1) = ids(&store).unwrap();
        for (xref_stream, object_streams) in [(false, false), (true, true)] {
            let rewritten = write_full(
                &store,
                &options(xref_stream, object_streams, EncryptionPolicy::Keep),
            )
            .unwrap();
            let after = open_fixture(name, &rewritten);
            assert!(
                after.repaired().is_empty(),
                "{name}: {:?}",
                after.repaired()
            );
            assert_eq!(after.encryption_info(), store.encryption_info(), "{name}");
            assert_eq!(
                content(&after),
                before,
                "{name} ({xref_stream}/{object_streams})"
            );
            assert_eq!(title(&after), b"Secret title", "{name}");
            let (new_id0, new_id1) = ids(&after).unwrap();
            assert_eq!(new_id0, id0, "{name}");
            assert_ne!(new_id1, id1, "{name}");
            let clear = |needle: &[u8]| rewritten.windows(needle.len()).any(|w| w == needle);
            assert!(!clear(b"Secret title"), "{name}");
            assert!(!clear(b"Hello, encrypted world"), "{name}");
            // The owner password still works, so the dictionary really is the original one.
            assert_eq!(
                after.authenticate(b"owner-pw").unwrap(),
                PasswordRole::Owner,
                "{name}"
            );
        }
    }
}

// ---- qpdf -----------------------------------------------------------------------------------

fn qpdf() -> String {
    std::env::var("QPDF").unwrap_or_else(|_| "qpdf".to_string())
}

fn qpdf_check(label: &str, bytes: &[u8], password: Option<&str>) {
    let path = std::env::temp_dir().join(format!(
        "vellora-writers-{}-{}.pdf",
        std::process::id(),
        label.replace([' ', '/', ':'], "_")
    ));
    std::fs::write(&path, bytes).unwrap();
    let mut command = Command::new(qpdf());
    command.arg("--check");
    if let Some(password) = password {
        command.arg(format!("--password={password}"));
    }
    let output = command
        .arg(&path)
        .output()
        .unwrap_or_else(|e| panic!("cannot run qpdf (set QPDF to its path): {e}"));
    let _ = std::fs::remove_file(&path);
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "qpdf --check {label}: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn qpdf_accepts_every_writer_output() {
    let mut checked = 0;
    for (style, original) in styles() {
        let mut changes = title_changes(
            reference_of_trailer(&open(&original), "Info").num,
            "Changed",
        );
        changes.free(if style == "table" { 7 } else { 1000 });
        qpdf_check(&format!("{style} original"), &original, None);
        qpdf_check(
            &format!("{style} updated"),
            &updated(&original, &changes),
            None,
        );
        checked += 2;
    }
    let original = sample("1.4");
    for (label, o) in [
        ("table", options(false, false, EncryptionPolicy::Remove)),
        (
            "xref stream",
            options(true, false, EncryptionPolicy::Remove),
        ),
        (
            "object streams",
            options(true, true, EncryptionPolicy::Remove),
        ),
    ] {
        qpdf_check(&format!("full {label}"), &full(&original, o), None);
        checked += 1;
    }
    qpdf_check(
        "hybrid updated",
        &{
            let original = hybrid();
            let store = open(&original);
            let page = store.pages().next().unwrap().unwrap().reference;
            let mut changes = Changes::new();
            changes.set_object(
                page.num,
                NewObject::Value(dictionary(vec![
                    ("Type", name("Page")),
                    ("Parent", Object::new(ObjectKind::Ref(ObjRef::new(2, 0)))),
                    (
                        "MediaBox",
                        Object::new(ObjectKind::Array(
                            [0, 0, 20, 20]
                                .map(|n| Object::new(ObjectKind::Integer(n)))
                                .to_vec(),
                        )),
                    ),
                ])),
            );
            drop(store);
            updated(&original, &changes)
        },
        None,
    );
    for name in FIXTURES {
        let original = fixture(name);
        let password = name.ends_with("user-password").then_some("user-pw");
        let store = open_fixture(name, &original);
        let info = reference_of_trailer(&store, "Info");
        let mut changes = Changes::new();
        changes.set_object(
            info.num,
            NewObject::Value(dictionary(vec![("Title", text("New secret title"))])),
        );
        let appended = incremental_update(&store, &changes).unwrap();
        qpdf_check(
            &format!("{name} updated"),
            &[original.as_slice(), &appended].concat(),
            password,
        );
        for (label, o) in [
            ("remove", options(false, false, EncryptionPolicy::Remove)),
            (
                "remove packed",
                options(true, true, EncryptionPolicy::Remove),
            ),
        ] {
            qpdf_check(
                &format!("{name} {label}"),
                &write_full(&store, &o).unwrap(),
                None,
            );
        }
        for (label, o) in [
            ("keep", options(false, false, EncryptionPolicy::Keep)),
            ("keep packed", options(true, true, EncryptionPolicy::Keep)),
        ] {
            qpdf_check(
                &format!("{name} {label}"),
                &write_full(&store, &o).unwrap(),
                password,
            );
        }
        checked += 5;
    }
    println!("qpdf --check passed for {checked} outputs");
}
