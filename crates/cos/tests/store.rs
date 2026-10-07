//! Tests for the lazy object store and the page-tree walk on small synthetic files.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;

use vellora_cos::{
    Error, LimitKind, Limits, ObjRef, Object, ObjectKind, ObjectStore, Recovery, RepairReason,
};

/// Builds a file with a correct classic cross-reference table; gaps in the object numbers are
/// free entries.
#[derive(Default)]
struct Pdf {
    data: Vec<u8>,
    offsets: BTreeMap<u32, usize>,
    /// Offsets to write wrongly into the table: object number to byte shift.
    skew: HashMap<u32, i64>,
}

impl Pdf {
    fn new() -> Self {
        Self {
            data: b"%PDF-1.7\n".to_vec(),
            ..Self::default()
        }
    }

    fn object(&mut self, num: u32, body: &str) -> &mut Self {
        self.offsets.insert(num, self.data.len());
        self.data
            .extend(format!("{num} 0 obj\n{body}\nendobj\n").bytes());
        self
    }

    fn finish(&self, trailer: &str) -> Vec<u8> {
        let mut data = self.data.clone();
        let size = self.offsets.keys().next_back().copied().unwrap_or(0) + 1;
        let at = data.len();
        let mut table = format!("xref\n0 {size}\n");
        for num in 0..size {
            match self.offsets.get(&num) {
                Some(&offset) => {
                    let skewed = i64::try_from(offset).unwrap() + self.skew.get(&num).unwrap_or(&0);
                    writeln!(table, "{skewed:010} 00000 n ").unwrap();
                }
                None => table.push_str("0000000000 65535 f \n"),
            }
        }
        write!(
            table,
            "trailer\n<< /Size {size} {trailer} >>\nstartxref\n{at}\n%%EOF\n"
        )
        .unwrap();
        data.extend(table.bytes());
        data
    }
}

/// A file with a catalog (object 1, pointing at `/Pages 2 0 R`) and the given objects.
fn doc(objects: &[(u32, &str)]) -> Vec<u8> {
    let mut pdf = Pdf::new();
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    for &(num, body) in objects {
        pdf.object(num, body);
    }
    pdf.finish("/Root 1 0 R")
}

fn open(data: &[u8]) -> ObjectStore<'_> {
    ObjectStore::open(data, Limits::default()).unwrap()
}

fn is_null(object: &Object<'_>) -> bool {
    matches!(object.kind, ObjectKind::Null)
}

fn dict_int(object: &Object<'_>, key: &[u8]) -> Option<i64> {
    object.as_dict()?.get(key)?.as_integer()
}

fn int(value: Option<&Arc<Object<'static>>>) -> Option<i64> {
    value?.as_integer()
}

fn ints(value: Option<&Arc<Object<'static>>>) -> Option<Vec<i64>> {
    match &value?.kind {
        ObjectKind::Array(items) => items.iter().map(Object::as_integer).collect(),
        _ => None,
    }
}

fn r(num: u32) -> ObjRef {
    ObjRef::new(num, 0)
}

fn page_refs(store: &ObjectStore<'_>) -> Vec<u32> {
    store.pages().map(|p| p.unwrap().reference.num).collect()
}

#[test]
fn store_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<ObjectStore<'static>>();
}

#[test]
fn opening_reads_no_objects() {
    let data = doc(&[(2, "<< /Type /Pages /Kids [] >>")]);
    let store = open(&data);
    assert_eq!(store.objects_loaded(), 0);
    assert_eq!(store.repaired(), []);
}

#[test]
fn resolves_objects_and_missing_ones_are_null() {
    let data = doc(&[(2, "<< /A 42 >>"), (4, "(text)")]);
    let store = open(&data);
    assert_eq!(dict_int(&store.resolve(r(2)).unwrap(), b"A"), Some(42));
    assert!(matches!(
        &store.resolve(r(4)).unwrap().kind,
        ObjectKind::String(s) if s.as_ref() == b"text"
    ));
    // 3 is a free entry, 0 the free list head, 99 is past the table.
    for missing in [0, 3, 99, u32::MAX] {
        assert!(is_null(&store.resolve(r(missing)).unwrap()), "{missing}");
    }
    assert_eq!(store.repaired(), []);
}

#[test]
fn the_generation_of_a_reference_is_not_compared() {
    let data = doc(&[(2, "<< /A 1 >>")]);
    let store = open(&data);
    let object = store.resolve(ObjRef::new(2, 7)).unwrap();
    assert_eq!(dict_int(&object, b"A"), Some(1));
}

#[test]
fn newest_revision_wins() {
    let mut data = doc(&[(2, "<< /V 1 >>")]);
    // The table's own `xref` keyword, not the one inside `startxref`.
    let first = data.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
    let new_obj = data.len();
    data.extend(b"2 0 obj\n<< /V 2 >>\nendobj\n");
    let section = data.len();
    data.extend(format!("xref\n2 1\n{new_obj:010} 00000 n \n").bytes());
    data.extend(
        format!("trailer\n<< /Size 3 /Root 1 0 R /Prev {first} >>\nstartxref\n{section}\n%%EOF\n")
            .bytes(),
    );
    let store = open(&data);
    assert_eq!(dict_int(&store.resolve(r(2)).unwrap(), b"V"), Some(2));
    assert_eq!(store.repaired(), []);
}

#[test]
fn indirect_objects_that_are_references_are_followed() {
    let data = doc(&[(5, "6 0 R"), (6, "7 0 R"), (7, "(end)")]);
    let store = open(&data);
    assert!(matches!(
        &store.resolve(r(5)).unwrap().kind,
        ObjectKind::String(s) if s.as_ref() == b"end"
    ));
}

#[test]
fn a_reference_cycle_is_a_limit_error_not_a_hang() {
    let data = doc(&[(5, "6 0 R"), (6, "5 0 R"), (7, "7 0 R")]);
    let store = open(&data);
    for start in [5, 7] {
        let error = store.resolve(r(start)).unwrap_err();
        assert!(
            matches!(
                error,
                Error::LimitExceeded {
                    limit: LimitKind::ReferenceDepth,
                    max: 32,
                    ..
                }
            ),
            "{error:?}"
        );
    }
}

#[test]
fn deref_follows_references_and_copies_direct_objects() {
    let data = doc(&[(5, "(five)")]);
    let store = open(&data);
    let direct = Object {
        kind: ObjectKind::Integer(3),
        span: 0..1,
    };
    assert_eq!(store.deref(&direct).unwrap().as_integer(), Some(3));
    let reference = Object {
        kind: ObjectKind::Ref(r(5)),
        span: 0..1,
    };
    assert!(matches!(
        &store.deref(&reference).unwrap().kind,
        ObjectKind::String(s) if s.as_ref() == b"five"
    ));
}

#[test]
fn an_object_that_cannot_be_parsed_is_an_error_for_that_object_only() {
    let data = doc(&[(3, "[ 1 2"), (4, "<< /A 4 >>")]);
    let store = open(&data);
    assert!(store.resolve(r(3)).is_err());
    assert_eq!(dict_int(&store.resolve(r(4)).unwrap(), b"A"), Some(4));
}

#[test]
fn objects_are_cached() {
    let data = doc(&[(2, "<< /A 1 >>")]);
    let store = open(&data);
    let first = store.resolve(r(2)).unwrap();
    let second = store.resolve(r(2)).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(store.objects_loaded(), 1);
}

#[test]
fn a_cache_too_small_for_an_object_still_returns_it() {
    let data = doc(&[(2, "<< /A 1 >>")]);
    let mut limits = Limits::default();
    limits.max_cache_bytes = 1;
    let store = ObjectStore::open(&data, limits).unwrap();
    for _ in 0..3 {
        assert_eq!(dict_int(&store.resolve(r(2)).unwrap(), b"A"), Some(1));
    }
    assert_eq!(store.objects_loaded(), 3);
}

#[test]
fn a_small_cache_evicts_but_never_returns_wrong_objects() {
    let mut pdf = Pdf::new();
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    for num in 2..=60 {
        pdf.object(num, &format!("<< /N {num} >>"));
    }
    let data = pdf.finish("/Root 1 0 R");
    let mut limits = Limits::default();
    limits.max_cache_bytes = 1000;
    let store = ObjectStore::open(&data, limits).unwrap();
    for pass in 0..2 {
        for num in 2..=60 {
            let object = store.resolve(r(num)).unwrap();
            assert_eq!(dict_int(&object, b"N"), Some(i64::from(num)), "pass {pass}");
        }
    }
    // Not everything fits in the cache, so the second pass loaded objects again (a sequential
    // scan is the worst case for LRU)...
    let loaded = store.objects_loaded();
    assert!(loaded > 59, "{loaded}");
    // ...but the most recently used one is still cached.
    store.resolve(r(60)).unwrap();
    assert_eq!(store.objects_loaded(), loaded);
}

#[test]
fn a_stream_with_an_indirect_length_is_read_without_repair() {
    let data = doc(&[
        (5, "<< /Length 6 0 R >>\nstream\nhello\nendstream"),
        (6, "5"),
    ]);
    let store = open(&data);
    let object = store.resolve(r(5)).unwrap();
    let ObjectKind::Stream(stream) = &object.kind else {
        panic!("not a stream: {object:?}");
    };
    assert_eq!(store.stream_raw(stream).unwrap(), b"hello");
    assert_eq!(store.repaired(), []);
}

#[test]
fn a_wrong_stream_length_is_repaired_and_reported() {
    let data = doc(&[(5, "<< /Length 3 >>\nstream\nabcdefgh\nendstream")]);
    let store = open(&data);
    let object = store.resolve(r(5)).unwrap();
    let ObjectKind::Stream(stream) = &object.kind else {
        panic!("not a stream: {object:?}");
    };
    assert_eq!(store.stream_raw(stream).unwrap(), b"abcdefgh");
    assert_eq!(
        store.repaired(),
        [RepairReason::ObjectRecovered {
            number: 5,
            recovery: Recovery::StreamLengthWrong { declared: 3 }
        }]
    );
}

#[test]
fn a_wrong_offset_rebuilds_the_cross_reference_once() {
    let mut pdf = Pdf::new();
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .object(2, "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>")
        .object(3, "<< /Type /Page /Parent 2 0 R /Tag 3 >>")
        .object(4, "<< /Type /Page /Parent 2 0 R /Tag 4 >>");
    pdf.skew.insert(3, 5);
    let data = pdf.finish("/Root 1 0 R");
    let store = open(&data);
    // The root is fine, so opening does not rebuild.
    assert_eq!(store.repaired(), []);

    assert_eq!(dict_int(&store.resolve(r(3)).unwrap(), b"Tag"), Some(3));
    assert_eq!(
        store.repaired(),
        [RepairReason::ObjectOffsetInvalid { number: 3 }]
    );
    // Objects keep resolving through the rebuilt table, and the pages are found.
    assert_eq!(dict_int(&store.resolve(r(4)).unwrap(), b"Tag"), Some(4));
    assert_eq!(page_refs(&store), [3, 4]);
}

#[test]
fn an_offset_the_rebuild_cannot_fix_makes_the_object_null() {
    // Object 6 is in the table but is not in the file at all, so the rebuild cannot find it.
    let mut pdf = Pdf::new();
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .object(2, "<< /Type /Pages /Kids [] >>")
        .object(6, "<< /Gone true >>");
    let mut data = pdf.finish("/Root 1 0 R");
    let at = data.windows(6).position(|w| w == b"6 0 ob").unwrap();
    data[at + 4..at + 7].copy_from_slice(b"xxx");
    let store = open(&data);
    assert!(is_null(&store.resolve(r(6)).unwrap()));
    assert!(
        store
            .repaired()
            .contains(&RepairReason::ObjectOffsetInvalid { number: 6 })
    );
    // A later bad offset does not scan the file again but is just as harmless.
    assert!(is_null(&store.resolve(r(6)).unwrap()));
    assert!(store.resolve(r(2)).unwrap().as_dict().is_some());
}

/// A file whose objects 2 (`/Pages`) and 3 (a page) live in object stream 4, with a
/// cross-reference stream (object 5). `index_for_3` is the index the table claims for object 3.
fn compressed_doc(index_for_3: u16) -> Vec<u8> {
    let pages = "<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
    let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 20] >>";
    let header = format!("2 0 3 {} ", pages.len() + 1);
    let body = format!("{pages}\n{page}");
    let objstm = format!(
        "<< /Type /ObjStm /N 2 /First {} /Length {} >>\nstream\n{header}{body}\nendstream",
        header.len(),
        header.len() + body.len()
    );
    let mut data = b"%PDF-1.5\n".to_vec();
    let catalog_at = data.len();
    data.extend(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let stream_at = data.len();
    data.extend(format!("4 0 obj\n{objstm}\nendobj\n").bytes());
    let xref_at = data.len();
    let row = |kind: u8, field: u32, extra: u16| {
        let mut row = vec![kind];
        row.extend(field.to_be_bytes());
        row.extend(extra.to_be_bytes());
        row
    };
    let rows: Vec<u8> = [
        row(0, 0, 255),
        row(1, u32::try_from(catalog_at).unwrap(), 0),
        row(2, 4, 0),
        row(2, 4, index_for_3),
        row(1, u32::try_from(stream_at).unwrap(), 0),
        row(1, u32::try_from(xref_at).unwrap(), 0),
    ]
    .concat();
    data.extend(
        format!(
            "5 0 obj\n<< /Type /XRef /Size 6 /W [1 4 2] /Root 1 0 R /Length {} >>\nstream\n",
            rows.len()
        )
        .bytes(),
    );
    data.extend(&rows);
    data.extend(b"\nendstream\nendobj\n");
    data.extend(format!("startxref\n{xref_at}\n%%EOF\n").bytes());
    data
}

#[test]
fn compressed_objects_resolve_through_their_object_stream() {
    let data = compressed_doc(1);
    let store = open(&data);
    assert_eq!(store.objects_loaded(), 0);
    let pages = store.resolve(r(2)).unwrap();
    assert!(pages.as_dict().unwrap().get(b"Kids").is_some());
    assert_eq!(page_refs(&store), [3]);
    let page = store.pages().next().unwrap().unwrap();
    assert_eq!(
        ints(page.inherited.media_box.as_ref()),
        Some(vec![0, 0, 10, 20])
    );
    assert_eq!(store.repaired(), []);
}

#[test]
fn a_wrong_index_in_the_table_falls_back_to_the_stream_header() {
    for wrong in [0, 7, 60_000] {
        let data = compressed_doc(wrong);
        let store = open(&data);
        assert_eq!(page_refs(&store), [3], "index {wrong}");
    }
}

#[test]
fn an_object_missing_from_its_object_stream_is_null() {
    // Object 3 claims index 0 of stream 4, but 4 holds objects 2 and 3 only: ask for a number
    // the stream does not hold by pointing the table at it.
    let mut data = compressed_doc(1);
    // Rewrite object 3's header entry inside the stream (number 3 -> 9).
    let at = data.windows(9).position(|w| w == b"2 0 3 42 ").unwrap();
    data[at + 4] = b'9';
    let store = open(&data);
    assert!(is_null(&store.resolve(r(3)).unwrap()));
}

#[test]
fn trailer_accessors() {
    let mut pdf = Pdf::new();
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .object(2, "<< /Type /Pages /Kids [] >>")
        .object(7, "<< /Title (t) >>");
    let data = pdf.finish("/Root 1 0 R /Info 7 0 R /Encrypt << /Filter /Standard /V 1 >>");
    let store = open(&data);
    assert_eq!(store.root_ref(), Some(r(1)));
    assert!(store.root().unwrap().unwrap().as_dict().is_some());
    assert!(
        store
            .info()
            .unwrap()
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Title")
            .is_some()
    );
    assert_eq!(dict_int(&store.encrypt().unwrap().unwrap(), b"V"), Some(1));
    assert!(store.trailer().get(b"Size").is_some());

    let plain = doc(&[(2, "<< /Type /Pages /Kids [] >>")]);
    let store = open(&plain);
    assert!(store.info().unwrap().is_none());
    assert!(store.encrypt().unwrap().is_none());
}

#[test]
fn pages_inherit_attributes_from_their_ancestors() {
    let data = doc(&[
        (
            2,
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 3 /MediaBox [0 0 100 100] /Rotate 90 \
             /Resources 9 0 R >>",
        ),
        (
            3,
            "<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 7 0 R] /MediaBox [0 0 200 200] >>",
        ),
        (
            4,
            "<< /Type /Page /Parent 2 0 R /Rotate 180 /CropBox [1 2 3 4] >>",
        ),
        (5, "<< /Type /Page /Parent 3 0 R >>"),
        (6, "[0 0 5 5]"),
        (
            7,
            "<< /Type /Page /Parent 3 0 R /MediaBox 6 0 R /Rotate null >>",
        ),
        (9, "<< /Font << >> >>"),
    ]);
    let store = open(&data);
    let pages: Vec<_> = store.pages().map(Result::unwrap).collect();
    assert_eq!(
        pages.iter().map(|p| p.reference.num).collect::<Vec<_>>(),
        [5, 7, 4]
    );

    // 5: everything from ancestors.
    assert_eq!(
        ints(pages[0].inherited.media_box.as_ref()),
        Some(vec![0, 0, 200, 200])
    );
    assert_eq!(int(pages[0].inherited.rotate.as_ref()), Some(90));
    assert!(
        pages[0]
            .inherited
            .resources
            .as_ref()
            .unwrap()
            .as_dict()
            .is_some()
    );
    assert!(pages[0].inherited.crop_box.is_none());
    // 7: its own MediaBox through a reference; a null /Rotate counts as absent.
    assert_eq!(
        ints(pages[1].inherited.media_box.as_ref()),
        Some(vec![0, 0, 5, 5])
    );
    assert_eq!(int(pages[1].inherited.rotate.as_ref()), Some(90));
    // 4: its own /Rotate and /CropBox, the root's /MediaBox.
    assert_eq!(
        ints(pages[2].inherited.media_box.as_ref()),
        Some(vec![0, 0, 100, 100])
    );
    assert_eq!(int(pages[2].inherited.rotate.as_ref()), Some(180));
    assert_eq!(
        ints(pages[2].inherited.crop_box.as_ref()),
        Some(vec![1, 2, 3, 4])
    );
    // The page object itself is the page dictionary.
    assert_eq!(
        pages[2]
            .object
            .as_dict()
            .unwrap()
            .get(b"Rotate")
            .unwrap()
            .as_integer(),
        Some(180)
    );
}

#[test]
fn a_cyclic_page_tree_ends_and_yields_each_page_once() {
    let looping = doc(&[
        (2, "<< /Type /Pages /Kids [3 0 R] >>"),
        (3, "<< /Type /Pages /Kids [2 0 R 4 0 R] >>"),
        (4, "<< /Type /Page >>"),
    ]);
    assert_eq!(page_refs(&open(&looping)), [4]);

    let own_kid = doc(&[(2, "<< /Type /Pages /Kids [2 0 R] >>")]);
    assert_eq!(page_refs(&open(&own_kid)), Vec::<u32>::new());

    let twice = doc(&[
        (2, "<< /Type /Pages /Kids [3 0 R 3 0 R] >>"),
        (3, "<< /Type /Page >>"),
    ]);
    assert_eq!(page_refs(&open(&twice)), [3]);
}

#[test]
fn missing_and_invalid_kids_are_skipped() {
    let data = doc(&[
        (
            2,
            "<< /Type /Pages /Kids [99 0 R (text) << /Type /Page >> 3 0 R 5 0 R 4 0 R] >>",
        ),
        (3, "42"),
        (4, "<< /Type /Page >>"),
        // No /Kids and /Type /Pages: an empty intermediate node, not a page.
        (5, "<< /Type /Pages >>"),
    ]);
    assert_eq!(page_refs(&open(&data)), [4]);

    let no_type = doc(&[(2, "<< /Type /Pages /Kids [3 0 R] >>"), (3, "<< >>")]);
    assert_eq!(page_refs(&open(&no_type)), [3]);

    let empty = doc(&[(2, "<< /Type /Pages /Kids [] >>")]);
    assert_eq!(page_refs(&open(&empty)), Vec::<u32>::new());
}

#[test]
fn a_kid_that_cannot_be_parsed_is_one_error_and_the_walk_continues() {
    let data = doc(&[
        (2, "<< /Type /Pages /Kids [3 0 R 4 0 R] >>"),
        (3, "[ 1 2"),
        (4, "<< /Type /Page >>"),
    ]);
    let store = open(&data);
    let items: Vec<_> = store.pages().collect();
    assert_eq!(items.len(), 2);
    assert!(items[0].is_err());
    assert_eq!(items[1].as_ref().unwrap().reference.num, 4);
}

#[test]
fn a_page_tree_deeper_than_the_nesting_limit_is_an_error() {
    let data = doc(&[
        (2, "<< /Type /Pages /Kids [3 0 R] >>"),
        (3, "<< /Type /Pages /Kids [4 0 R] >>"),
        (4, "<< /Type /Pages /Kids [5 0 R] >>"),
        (5, "<< /Type /Page >>"),
    ]);
    let mut limits = Limits::default();
    limits.max_nesting_depth = 2;
    let store = ObjectStore::open(&data, limits).unwrap();
    let items: Vec<_> = store.pages().collect();
    assert_eq!(items.len(), 1);
    assert!(matches!(
        items[0],
        Err(Error::LimitExceeded {
            limit: LimitKind::NestingDepth,
            ..
        })
    ));
    // Within the limit the same tree is fine.
    let store = open(&data);
    assert_eq!(page_refs(&store), [5]);
}

#[test]
fn a_ten_thousand_page_document_is_read_lazily() {
    const PAGES: u32 = 10_000;
    let mut pdf = Pdf::new();
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    let kids: String = (0..PAGES).fold(String::new(), |mut s, i| {
        write!(s, "{} 0 R ", 3 + i).unwrap();
        s
    });
    pdf.object(
        2,
        &format!("<< /Type /Pages /Kids [{kids}] /Count {PAGES} /MediaBox [0 0 612 792] >>"),
    );
    for i in 0..PAGES {
        pdf.object(3 + i, "<< /Type /Page /Parent 2 0 R >>");
    }
    let data = pdf.finish("/Root 1 0 R");

    let store = open(&data);
    assert_eq!(store.objects_loaded(), 0);
    let first = store.pages().next().unwrap().unwrap();
    assert_eq!(first.reference.num, 3);
    assert_eq!(
        ints(first.inherited.media_box.as_ref()),
        Some(vec![0, 0, 612, 792])
    );
    // Catalog, root /Pages node, first page: nothing else was touched.
    assert_eq!(store.objects_loaded(), 3);

    assert_eq!(store.pages().count(), PAGES as usize);
    assert_eq!(store.objects_loaded(), u64::from(PAGES) + 2);
}
