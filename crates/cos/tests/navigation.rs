//! Destinations, the outline and page labels on small synthetic files (M1 task 12a).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

use vellora_cos::{
    Destination, DestinationResolver, Fit, Limits, ObjectStore, Outline, OutlineItem, PageLabels,
};

/// Builds a file with a correct classic cross-reference table.
#[derive(Default)]
struct Pdf {
    data: Vec<u8>,
    offsets: BTreeMap<u32, usize>,
}

impl Pdf {
    fn object(&mut self, num: u32, body: &str) {
        if self.data.is_empty() {
            self.data = b"%PDF-1.7\n".to_vec();
        }
        self.offsets.insert(num, self.data.len());
        self.data
            .extend(format!("{num} 0 obj\n{body}\nendobj\n").bytes());
    }

    fn finish(&self) -> Vec<u8> {
        let mut data = self.data.clone();
        let size = self.offsets.keys().next_back().copied().unwrap_or(0) + 1;
        let at = data.len();
        let mut table = format!("xref\n0 {size}\n");
        for num in 0..size {
            match self.offsets.get(&num) {
                Some(offset) => writeln!(table, "{offset:010} 00000 n ").unwrap(),
                None => table.push_str("0000000000 65535 f \n"),
            }
        }
        write!(
            table,
            "trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{at}\n%%EOF\n"
        )
        .unwrap();
        data.extend(table.bytes());
        data
    }
}

/// A document of `pages` pages (objects 10, 11, ...) whose catalog has the extra entries
/// `catalog`, plus `objects`.
fn doc(pages: u32, catalog: &str, objects: &[(u32, &str)]) -> Vec<u8> {
    let mut pdf = Pdf::default();
    pdf.object(1, &format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"));
    let kids = (0..pages).fold(String::new(), |mut kids, i| {
        write!(kids, "{} 0 R ", 10 + i).unwrap();
        kids
    });
    pdf.object(
        2,
        &format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
    );
    for i in 0..pages {
        pdf.object(
            10 + i,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
        );
    }
    for &(num, body) in objects {
        pdf.object(num, body);
    }
    pdf.finish()
}

fn open(data: &[u8]) -> ObjectStore<'_> {
    ObjectStore::open(data, Limits::default()).unwrap()
}

#[allow(clippy::unnecessary_wraps)]
fn dest(page: u32, fit: Fit) -> Option<Destination> {
    Some(Destination { page, fit })
}

/// Resolves the explicit destination array `array` in a document of five pages.
fn explicit(array: &str) -> Option<Destination> {
    let data = doc(5, &format!("/Dests << /D {array} >>"), &[]);
    let store = open(&data);
    DestinationResolver::new(&store).resolve_name(b"D").unwrap()
}

#[test]
fn every_kind_of_explicit_destination_is_read() {
    assert_eq!(
        explicit("[12 0 R /XYZ 10 20.5 1.5]"),
        dest(
            2,
            Fit::Xyz {
                left: Some(10.0),
                top: Some(20.5),
                zoom: Some(1.5)
            }
        )
    );
    // null operands and a zoom of 0 leave things as they are
    assert_eq!(
        explicit("[10 0 R /XYZ null null 0]"),
        dest(
            0,
            Fit::Xyz {
                left: None,
                top: None,
                zoom: None
            }
        )
    );
    assert_eq!(explicit("[11 0 R /Fit]"), dest(1, Fit::Fit));
    assert_eq!(
        explicit("[11 0 R /FitH 700]"),
        dest(1, Fit::FitH { top: Some(700.0) })
    );
    assert_eq!(
        explicit("[11 0 R /FitH null]"),
        dest(1, Fit::FitH { top: None })
    );
    assert_eq!(
        explicit("[11 0 R /FitV 5]"),
        dest(1, Fit::FitV { left: Some(5.0) })
    );
    assert_eq!(
        explicit("[13 0 R /FitR 1 2 3 4]"),
        dest(
            3,
            Fit::FitR {
                left: 1.0,
                bottom: 2.0,
                right: 3.0,
                top: 4.0
            }
        )
    );
    assert_eq!(explicit("[13 0 R /FitB]"), dest(3, Fit::FitB));
    assert_eq!(
        explicit("[14 0 R /FitBH 9]"),
        dest(4, Fit::FitBH { top: Some(9.0) })
    );
    assert_eq!(
        explicit("[14 0 R /FitBV -3]"),
        dest(4, Fit::FitBV { left: Some(-3.0) })
    );
}

#[test]
fn odd_destinations_are_none_or_a_plain_page() {
    // A rectangle with missing sides shows the page.
    assert_eq!(explicit("[11 0 R /FitR 1 2]"), dest(1, Fit::Fit));
    // A page given as its index; one that does not exist
    assert_eq!(explicit("[2 /Fit]"), dest(2, Fit::Fit));
    assert_eq!(explicit("[5 /Fit]"), None);
    assert_eq!(explicit("[-1 /Fit]"), None);
    // not a page of the document, no fit, an unknown fit, not an array, empty
    assert_eq!(explicit("[99 0 R /Fit]"), None);
    assert_eq!(explicit("[11 0 R]"), None);
    assert_eq!(explicit("[11 0 R /Bogus 1]"), None);
    assert_eq!(explicit("[/Fit]"), None);
    assert_eq!(explicit("[]"), None);
    assert_eq!(explicit("42"), None);
}

#[test]
fn named_destinations_come_from_the_dictionary_and_the_name_tree() {
    let data = doc(
        5,
        "/Dests 3 0 R /Names << /Dests 4 0 R >>",
        &[
            (
                3,
                "<< /Old [10 0 R /Fit] /Boxed << /D [11 0 R /Fit] >> /Loop /Old >>",
            ),
            (4, "<< /Kids [5 0 R 6 0 R] >>"),
            (
                5,
                "<< /Limits [(a) (mz)] /Names [(alpha) [12 0 R /Fit] (mid) << /D [13 0 R /FitB] >>] >>",
            ),
            (
                6,
                "<< /Limits [(n) (zz)] /Names [(zeta) [14 0 R /FitB] (Old) [11 0 R /FitV null]] >>",
            ),
        ],
    );
    let store = open(&data);
    let resolver = DestinationResolver::new(&store);
    let by_name = |name: &[u8]| resolver.resolve_name(name).unwrap();
    assert_eq!(by_name(b"Old"), dest(0, Fit::Fit));
    assert_eq!(by_name(b"Boxed"), dest(1, Fit::Fit));
    assert_eq!(by_name(b"alpha"), dest(2, Fit::Fit));
    assert_eq!(by_name(b"mid"), dest(3, Fit::FitB));
    assert_eq!(by_name(b"zeta"), dest(4, Fit::FitB));
    assert_eq!(by_name(b"missing"), None);
    // a name whose value is another name is not followed
    assert_eq!(by_name(b"Loop"), None);
}

#[test]
fn a_string_is_looked_up_in_the_tree_and_then_the_dictionary() {
    let data = doc(
        5,
        "/Dests << /OnlyInDict [11 0 R /Fit] /Both [10 0 R /Fit] >> /Names << /Dests << /Names [(Both) [12 0 R /Fit]] >> >>",
        &[
            (20, "<< /Dest (Both) >>"),
            (21, "<< /Dest (OnlyInDict) >>"),
            (22, "<< /Dest /Both >>"),
            (23, "<< /A << /S /GoTo /D (Both) >> >>"),
            (24, "<< /A << /S /URI /URI (http://example.com) >> >>"),
        ],
    );
    let store = open(&data);
    let resolver = DestinationResolver::new(&store);
    let of = |num: u32| {
        let item = store.resolve(vellora_cos::ObjRef::new(num, 0)).unwrap();
        let dict = item.as_dict().unwrap();
        match (dict.get(b"Dest"), dict.get(b"A")) {
            (Some(dest), _) => resolver.resolve(dest).unwrap(),
            (None, Some(action)) => resolver.resolve_action(action).unwrap(),
            _ => None,
        }
    };
    assert_eq!(of(20), dest(2, Fit::Fit)); // string: the tree first
    assert_eq!(of(21), dest(1, Fit::Fit)); // ... then the dictionary
    assert_eq!(of(22), dest(0, Fit::Fit)); // name: the dictionary first
    assert_eq!(of(23), dest(2, Fit::Fit)); // GoTo action
    assert_eq!(of(24), None); // another kind of action
}

#[test]
fn a_name_tree_that_loops_or_lies_about_its_limits_ends() {
    let data = doc(
        5,
        "/Names << /Dests 4 0 R >>",
        &[
            // 4 -> 5 -> 4 and 6 (limits that exclude everything), plus a good leaf.
            (4, "<< /Kids [5 0 R 6 0 R 7 0 R] >>"),
            (5, "<< /Kids [4 0 R 5 0 R] >>"),
            (6, "<< /Limits [(a) (b)] /Names [(k) [10 0 R /Fit]] >>"),
            (7, "<< /Names [(k) [14 0 R /Fit]] >>"),
        ],
    );
    let store = open(&data);
    let resolver = DestinationResolver::new(&store);
    // The leaf with limits that exclude "k" is skipped, the one without limits is searched.
    assert_eq!(resolver.resolve_name(b"k").unwrap(), dest(4, Fit::Fit));
}

#[test]
fn a_name_tree_with_too_many_nodes_is_a_limit_error() {
    let data = doc(
        1,
        "/Names << /Dests 4 0 R >>",
        &[
            (4, "<< /Kids [5 0 R 6 0 R 7 0 R] >>"),
            (5, "<< /Names [] >>"),
            (6, "<< /Names [] >>"),
            (7, "<< /Names [] >>"),
        ],
    );
    let mut limits = Limits::default();
    limits.max_tree_nodes = 3;
    let store = ObjectStore::open(&data, limits).unwrap();
    let resolver = DestinationResolver::new(&store);
    assert!(resolver.resolve_name(b"x").is_err());
}

fn top(store: &ObjectStore<'_>, limit: usize) -> Vec<OutlineItem> {
    let resolver = DestinationResolver::new(store);
    Outline::new(&resolver)
        .children(None, None, 0, limit)
        .unwrap()
        .items
}

/// An outline of three top-level items; the first has two children, the second of which has one.
fn outline_doc() -> Vec<u8> {
    doc(
        5,
        "/Outlines 30 0 R",
        &[
            (
                30,
                "<< /Type /Outlines /First 31 0 R /Last 33 0 R /Count 5 >>",
            ),
            (
                31,
                "<< /Title (Intro) /Parent 30 0 R /Next 32 0 R /First 34 0 R /Last 35 0 R /Count 3 /Dest [10 0 R /XYZ 0 700 null] /F 2 >>",
            ),
            (
                32,
                "<< /Title <FEFF00C900E900200032> /Parent 30 0 R /Prev 31 0 R /Next 33 0 R /Count 0 /A << /S /GoTo /D [12 0 R /Fit] >> /F 1 >>",
            ),
            (
                33,
                "<< /Title (Last\\nline) /Parent 30 0 R /Prev 32 0 R /First 36 0 R /Last 36 0 R /Count -1 /Dest [99 0 R /Fit] >>",
            ),
            (
                34,
                "<< /Title (1.1) /Parent 31 0 R /Next 35 0 R /Dest [11 0 R /Fit] >>",
            ),
            (
                35,
                "<< /Title (1.2) /Parent 31 0 R /Prev 34 0 R /First 37 0 R /Last 37 0 R /Count 1 >>",
            ),
            (36, "<< /Title (3.1) /Parent 33 0 R >>"),
            (
                37,
                "<< /Title (1.2.1) /Parent 35 0 R /Dest [13 0 R /Fit] >>",
            ),
        ],
    )
}

#[test]
fn the_outline_is_read_a_level_at_a_time() {
    let data = outline_doc();
    let store = open(&data);
    let items = top(&store, 10);
    let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["Intro", "\u{C9}\u{E9} 2", "Last line"]);
    assert_eq!(items[0].id, 31);
    assert_eq!(
        items[0].destination,
        dest(
            0,
            Fit::Xyz {
                left: Some(0.0),
                top: Some(700.0),
                zoom: None
            }
        )
    );
    assert!(items[0].has_children && items[0].open);
    assert!(items[0].style.bold && !items[0].style.italic);
    // an action, no children, italic
    assert_eq!(items[1].destination, dest(2, Fit::Fit));
    assert!(!items[1].has_children && !items[1].open);
    assert!(items[1].style.italic && !items[1].style.bold);
    // closed children, and a destination that is not a page
    assert!(items[2].has_children && !items[2].open);
    assert_eq!(items[2].destination, None);

    let resolver = DestinationResolver::new(&store);
    let outline = Outline::new(&resolver);
    let below = outline.children(Some(31), None, 0, 10).unwrap();
    assert_eq!(
        below.items.iter().map(|i| i.id).collect::<Vec<_>>(),
        [34, 35]
    );
    assert!(!below.more);
    let deeper = outline.children(Some(35), None, 0, 10).unwrap();
    assert_eq!(deeper.items[0].title, "1.2.1");
    assert_eq!(deeper.items[0].destination, dest(3, Fit::Fit));
    // not an item, and an item without children
    assert_eq!(
        outline.children(Some(36), None, 0, 10).unwrap().items,
        Vec::new()
    );
    assert_eq!(
        outline.children(Some(999), None, 0, 10).unwrap().items,
        Vec::new()
    );
}

#[test]
fn a_level_is_paged_from_the_last_item_seen() {
    let data = outline_doc();
    let store = open(&data);
    let resolver = DestinationResolver::new(&store);
    let outline = Outline::new(&resolver);
    let first = outline.children(None, None, 0, 2).unwrap();
    assert_eq!(first.items.len(), 2);
    assert!(first.more);
    let rest = outline.children(None, Some(32), 2, 2).unwrap();
    assert_eq!(rest.items.iter().map(|i| i.id).collect::<Vec<_>>(), [33]);
    assert!(!rest.more);
    // exactly as many as asked for, and nothing after
    let exact = outline.children(None, None, 0, 3).unwrap();
    assert_eq!((exact.items.len(), exact.more), (3, false));
}

#[test]
fn no_outline_is_an_empty_one() {
    let data = doc(2, "", &[]);
    assert_eq!(top(&open(&data), 10), Vec::new());
    let data = doc(2, "/Outlines 30 0 R", &[(30, "<< /Type /Outlines >>")]);
    assert_eq!(top(&open(&data), 10), Vec::new());
    let data = doc(2, "/Outlines 5", &[]);
    assert_eq!(top(&open(&data), 10), Vec::new());
}

#[test]
fn circular_outlines_end() {
    // A /Next that goes round, and a /First that points at the parent.
    let data = doc(
        2,
        "/Outlines 30 0 R",
        &[
            (30, "<< /First 31 0 R /Last 32 0 R >>"),
            (
                31,
                "<< /Title (a) /Parent 30 0 R /Next 32 0 R /First 30 0 R >>",
            ),
            (
                32,
                "<< /Title (b) /Parent 30 0 R /Next 31 0 R /First 32 0 R >>",
            ),
        ],
    );
    let store = open(&data);
    let resolver = DestinationResolver::new(&store);
    let outline = Outline::new(&resolver);
    let items = outline.children(None, None, 0, 100).unwrap();
    assert_eq!(items.items.len(), 2);
    assert!(!items.more);
    // Round the sibling loop, with the caller's count as the only bound.
    let page = outline.children(None, Some(31), u32::MAX, 100).unwrap();
    assert_eq!(page.items, Vec::new());
}

#[test]
fn a_hostile_parent_chain_is_not_expanded_past_the_nesting_limit() {
    let mut objects = vec![(30, "<< /First 31 0 R >>".to_string())];
    for n in 31..=40 {
        objects.push((
            n,
            format!(
                "<< /Title (n{n}) /Parent {} 0 R /First {} 0 R >>",
                n - 1,
                n + 1
            ),
        ));
    }
    let objects: Vec<(u32, &str)> = objects.iter().map(|(n, b)| (*n, b.as_str())).collect();
    let data = doc(1, "/Outlines 30 0 R", &objects);
    let mut limits = Limits::default();
    limits.max_nesting_depth = 4;
    let store = ObjectStore::open(&data, limits).unwrap();
    let resolver = DestinationResolver::new(&store);
    let outline = Outline::new(&resolver);
    let mut parent = None;
    let mut levels = 0;
    while let Some(item) = outline
        .children(parent, None, 0, 10)
        .unwrap()
        .items
        .into_iter()
        .next()
    {
        levels += 1;
        if !item.has_children {
            break;
        }
        parent = Some(item.id);
    }
    assert!((1..=4).contains(&levels), "{levels} levels");
}

#[test]
fn titles_are_text_without_control_characters_and_bounded() {
    let long = "x".repeat(5000);
    let data = doc(
        1,
        "/Outlines 30 0 R",
        &[
            (30, "<< /First 31 0 R >>"),
            (
                31,
                &format!("<< /Title ({long}) /Parent 30 0 R /Next 32 0 R >>"),
            ),
            (32, "<< /Parent 30 0 R /Title 5 >>"),
        ],
    );
    let items = top(&open(&data), 10);
    assert_eq!(items[0].title.chars().count(), 1025); // 1024 and an ellipsis
    assert!(items[0].title.ends_with('\u{2026}'));
    assert_eq!(items[1].title, "");
}

/// A document of 30 pages with the given `/Nums` array as its labels.
fn labelled(nums: &str) -> Vec<u8> {
    doc(
        30,
        "/PageLabels 40 0 R",
        &[(40, &format!("<< /Nums [{nums}] >>"))],
    )
}

fn read_labels(data: &[u8]) -> Option<PageLabels> {
    let store = open(data);
    let resolver = DestinationResolver::new(&store);
    PageLabels::read(&resolver).unwrap()
}

#[test]
fn page_labels_are_read_from_the_number_tree() {
    let data = labelled(
        "0 << /S /r >> 4 << /S /D /St 5 >> 10 << /S /D /P (A-) >> 12 << /P <FEFF00C9> >> 13 << /S /A /St 27 >> 15 << /S /a >>",
    );
    let labels = read_labels(&data).unwrap();
    let window = labels.window(0, 30, 30);
    assert_eq!(
        window[..18],
        [
            "i", "ii", "iii", "iv", "5", "6", "7", "8", "9", "10", "A-1", "A-2", "\u{C9}", "AA",
            "BB", "a", "b", "c"
        ]
    );
    assert_eq!(labels.find("A-2", 30), Some(11));
    assert_eq!(labels.find("iii", 30), Some(2));
    assert_eq!(labels.find("7", 30), Some(6));
    assert_eq!(labels.find("BB", 30), Some(14));
    assert_eq!(labels.find("a", 30), Some(15));
}

#[test]
fn page_labels_in_a_tree_of_nodes_and_with_odd_entries() {
    let data = doc(
        10,
        "/PageLabels 40 0 R",
        &[
            (40, "<< /Kids [41 0 R 42 0 R] >>"),
            (
                41,
                "<< /Limits [0 4] /Nums [0 << /S /R >> 3 << /S /R /St 0 >>] >>",
            ),
            (
                42,
                // negative and non-integer keys, a repeated key, a non-dictionary, a style that
                // does not exist, a lone trailing key
                "<< /Limits [5 9] /Nums [-4 << /S /D >> (x) << /S /D >> 5 << /S /Q /P (Z) >> 5 << /S /D >> 7 7 8 << /S /D /St 100 >> 9]>>",
            ),
        ],
    );
    let labels = read_labels(&data).unwrap();
    let window = labels.window(0, 10, 10);
    // A start below 1 counts as 1; a style that does not exist is no number; the first of a
    // repeated key wins; page 7's value is not a dictionary so page 6 goes on.
    assert_eq!(
        window,
        ["I", "II", "III", "I", "II", "Z", "Z", "Z", "100", "101"]
    );
}

#[test]
fn no_labels_and_unusable_labels() {
    assert!(read_labels(&doc(3, "", &[])).is_none());
    assert!(read_labels(&doc(3, "/PageLabels 7", &[])).is_none());
    assert!(read_labels(&labelled("")).is_none());
    assert!(read_labels(&labelled("(a) 1 -1 << >>")).is_none());
    // Without a range at page 0 the first pages have no label of their own.
    let labels = read_labels(&labelled("2 << /S /D >>")).unwrap();
    assert_eq!(labels.label(1), None);
    assert_eq!(labels.window(0, 4, 30), ["1", "2", "1", "2"]);
}

#[test]
fn a_page_label_tree_that_loops_ends() {
    let data = doc(
        3,
        "/PageLabels 40 0 R",
        &[
            (40, "<< /Kids [41 0 R 40 0 R] >>"),
            (41, "<< /Kids [40 0 R] /Nums [0 << /S /D >>] >>"),
        ],
    );
    let labels = read_labels(&data).unwrap();
    assert_eq!(labels.window(0, 3, 3), ["1", "2", "3"]);
}
