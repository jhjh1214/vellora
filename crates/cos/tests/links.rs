//! Link annotations and their actions on small synthetic files (M1 task 13a).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::fmt::Write as _;

use vellora_cos::links::{MAX_KIND_BYTES, MAX_URI_BYTES, read_links};
use vellora_cos::{
    Destination, DestinationResolver, Fit, Limits, LinkAction, NamedAction, ObjectStore,
};

/// A classic file: catalog 1, page tree 2, the page under test 10, a second page 11, and `objects`.
fn doc(page: &str, catalog: &str, objects: &[(u32, String)]) -> Vec<u8> {
    let mut data = b"%PDF-1.7\n".to_vec();
    let mut offsets: BTreeMap<u32, usize> = BTreeMap::new();
    let mut put = |data: &mut Vec<u8>, num: u32, body: &str| {
        offsets.insert(num, data.len());
        data.extend(format!("{num} 0 obj\n{body}\nendobj\n").bytes());
    };
    put(
        &mut data,
        1,
        &format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
    );
    put(
        &mut data,
        2,
        "<< /Type /Pages /Kids [10 0 R 11 0 R] /Count 2 >>",
    );
    put(&mut data, 10, page);
    put(
        &mut data,
        11,
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
    );
    for (num, body) in objects {
        put(&mut data, *num, body);
    }
    let size = offsets.keys().next_back().copied().unwrap() + 1;
    let at = data.len();
    let mut table = format!("xref\n0 {size}\n");
    for num in 0..size {
        match offsets.get(&num) {
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

/// A page with the given extra entries.
fn page(extra: &str) -> String {
    format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] {extra} >>")
}

/// The links of the first page of `data`.
fn links_of(data: &[u8], skip: usize, limit: usize) -> vellora_cos::LinkPage {
    let store = ObjectStore::open(data, Limits::default()).unwrap();
    let resolver = DestinationResolver::new(&store);
    let first = store.pages().next().unwrap().unwrap();
    read_links(&resolver, &first, skip, limit).unwrap()
}

fn actions(data: &[u8]) -> Vec<LinkAction> {
    links_of(data, 0, 1000)
        .links
        .into_iter()
        .map(|link| link.action)
        .collect()
}

/// `/Annots [ ... ]` of link annotations with the rectangle `10 10 20 20` and these entries.
fn annots(entries: &[&str]) -> String {
    let items = entries
        .iter()
        .map(|entry| {
            format!("<< /Type /Annot /Subtype /Link /Rect [10 10 20 20] /Border [0 0 0] {entry} >>")
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("/Annots [ {items} ]")
}

#[test]
fn every_kind_of_action_is_classified() {
    let a = annots(&[
        "/Dest [11 0 R /Fit]",
        "/Dest /named",
        "/A << /S /GoTo /D [11 0 R /XYZ 5 6 null] >>",
        "/A << /S /URI /URI (https://example.org/a?b=c) >>",
        "/A << /S /Named /N /NextPage >>",
        "/A << /S /Named /N /PrevPage >>",
        "/A << /S /Named /N /FirstPage >>",
        "/A << /S /Named /N /LastPage >>",
        "/A << /S /Named /N /Print >>",
        "/A << /S /GoToR /F (other.pdf) /D [0 /Fit] >>",
        "/A << /S /Launch /F (calc.exe) >>",
        "/A << /S /JavaScript /JS (app.alert(1)) >>",
        "/A << /S /SubmitForm /F (http://x) >>",
        "/A << /S /ImportData /F (x.fdf) >>",
        "/A << /S /GoToE /T << >> >>",
        "/A << /S /Thread /D 0 >>",
    ]);
    let data = doc(&page(&a), "/Dests << /named [11 0 R /FitH 50] >>", &[]);
    let found = actions(&data);
    let go = |page, fit| LinkAction::GoTo(Destination { page, fit });
    let inert = |name: &str| LinkAction::Inert(name.to_owned());
    assert_eq!(
        found,
        [
            go(1, Fit::Fit),
            go(1, Fit::FitH { top: Some(50.0) }),
            go(
                1,
                Fit::Xyz {
                    left: Some(5.0),
                    top: Some(6.0),
                    zoom: None
                }
            ),
            LinkAction::Uri("https://example.org/a?b=c".to_owned()),
            LinkAction::Named(NamedAction::NextPage),
            LinkAction::Named(NamedAction::PrevPage),
            LinkAction::Named(NamedAction::FirstPage),
            LinkAction::Named(NamedAction::LastPage),
            inert("Named:Print"),
            inert("GoToR"),
            inert("Launch"),
            inert("JavaScript"),
            inert("SubmitForm"),
            inert("ImportData"),
            inert("GoToE"),
            inert("Thread"),
        ]
    );
}

#[test]
fn things_that_are_not_clickable_links_are_left_out() {
    let a = "/Annots [
        << /Subtype /Widget /Rect [0 0 5 5] /A << /S /URI /URI (https://w) >> >>
        << /Subtype /Link /Rect [0 0 5 5] >>
        << /Subtype /Link /Rect [0 0 5 5] /F 2 /A << /S /URI /URI (https://hidden) >> >>
        << /Subtype /Link /Rect [0 0 5 5] /F 32 /A << /S /URI /URI (https://noview) >> >>
        << /Subtype /Link /Rect [0 0 5] /A << /S /URI /URI (https://short) >> >>
        << /Subtype /Link /Rect [3 3 3 9] /A << /S /URI /URI (https://flat) >> >>
        99 0 R
        42
        << /Subtype /Link /Rect [0 0 5 5] /F 4 /A << /S /URI /URI (https://shown) >> >>
    ]";
    let data = doc(&page(a), "", &[]);
    assert_eq!(
        actions(&data),
        [LinkAction::Uri("https://shown".to_owned())]
    );
}

#[test]
fn a_destination_that_goes_nowhere_is_a_link_that_goes_nowhere() {
    let a = annots(&[
        "/Dest /missing",
        "/A << /S /GoTo /D [999 0 R /Fit] >>",
        "/A << /S /GoTo >>",
        "/A << /S 7 >>",
        "/A 5",
    ]);
    let data = doc(&page(&a), "", &[]);
    assert_eq!(actions(&data), vec![LinkAction::Unresolved; 5]);
}

#[test]
fn an_address_too_long_to_show_whole_is_not_offered_as_one() {
    let long = "a".repeat(MAX_URI_BYTES + 1);
    let exact = "b".repeat(MAX_URI_BYTES);
    let a = annots(&[
        &format!("/A << /S /URI /URI ({long}) >>"),
        &format!("/A << /S /URI /URI ({exact}) >>"),
        "/A << /S /URI >>",
    ]);
    let found = actions(&doc(&page(&a), "", &[]));
    assert_eq!(found[0], LinkAction::Inert("URI".to_owned()));
    assert_eq!(found[1], LinkAction::Uri(exact));
    assert_eq!(found[2], LinkAction::Inert("URI".to_owned()));
}

#[test]
fn the_name_of_an_inert_action_is_cut() {
    let name = "N".repeat(500);
    let a = annots(&[&format!("/A << /S /{name} >>")]);
    let LinkAction::Inert(kind) = &actions(&doc(&page(&a), "", &[]))[0] else {
        panic!("not inert");
    };
    assert_eq!(kind.len(), MAX_KIND_BYTES);
}

#[test]
fn links_are_read_in_windows() {
    let entries: Vec<String> = (0..10)
        .map(|n| format!("/A << /S /URI /URI (https://e/{n}) >>"))
        .collect();
    let refs: Vec<&str> = entries.iter().map(String::as_str).collect();
    let data = doc(&page(&annots(&refs)), "", &[]);

    let first = links_of(&data, 0, 4);
    assert_eq!((first.links.len(), first.more), (4, true));
    let second = links_of(&data, 4, 4);
    assert_eq!((second.links.len(), second.more), (4, true));
    let last = links_of(&data, 8, 4);
    assert_eq!((last.links.len(), last.more), (2, false));
    assert_eq!(
        last.links[0].action,
        LinkAction::Uri("https://e/8".to_owned())
    );
    // Exactly the rest: nothing more. Past the end: nothing.
    assert!(!links_of(&data, 6, 4).more);
    let past = links_of(&data, 50, 4);
    assert_eq!((past.links.len(), past.more), (0, false));
}

#[test]
fn the_rectangle_is_in_the_page_as_shown() {
    // A 100 x 50 crop box that does not start at the origin, turned a quarter clockwise into a
    // 50 x 100 page. The link starts 10 points from the box's left edge and 10 below its top edge;
    // after the turn that is 10 points from the top edge and, as the old top edge is now the
    // right one, 10 points in from the right edge (x from 30 to 40).
    let extra = "/CropBox [100 200 200 250] /Rotate 90 /Annots [
        << /Subtype /Link /Rect [110 230 130 240] /A << /S /URI /URI (https://r) >> >> ]";
    let data = doc(&page(extra), "", &[]);
    let links = links_of(&data, 0, 10).links;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].rect, [30.0, 10.0, 40.0, 30.0]);

    // The same link on an upright page: x from the left of the box, y down from its top.
    let upright = "/CropBox [100 200 200 250] /Annots [
        << /Subtype /Link /Rect [110 230 130 240] /A << /S /URI /URI (https://r) >> >> ]";
    let links = links_of(&doc(&page(upright), "", &[]), 0, 10).links;
    assert_eq!(links[0].rect, [10.0, 10.0, 30.0, 20.0]);
}

#[test]
fn annotations_given_as_references_and_an_indirect_array_are_followed() {
    let data = doc(
        &page("/Annots 20 0 R"),
        "",
        &[
            (20, "[21 0 R 22 0 R]".to_owned()),
            (
                21,
                "<< /Subtype /Link /Rect [0 0 9 9] /A 23 0 R >>".to_owned(),
            ),
            (
                22,
                "<< /Subtype /Link /Rect [0 0 9 9] /Dest [11 0 R /Fit] >>".to_owned(),
            ),
            (23, "<< /S /URI /URI (https://indirect) >>".to_owned()),
        ],
    );
    assert_eq!(
        actions(&data),
        [
            LinkAction::Uri("https://indirect".to_owned()),
            LinkAction::GoTo(Destination {
                page: 1,
                fit: Fit::Fit
            })
        ]
    );
}

#[test]
fn a_page_without_annotations_has_no_links_and_a_broken_array_is_no_array() {
    for extra in ["", "/Annots 5", "/Annots null", "/Annots << >>"] {
        let found = links_of(&doc(&page(extra), "", &[]), 0, 10);
        assert!(found.links.is_empty() && !found.more, "{extra:?}");
    }
    // An annotation array that contains itself is read once.
    let data = doc(
        &page("/Annots 20 0 R"),
        "",
        &[
            (20, "[20 0 R 21 0 R]".to_owned()),
            (
                21,
                "<< /Subtype /Link /Rect [0 0 9 9] /A << /S /Launch >> >>".to_owned(),
            ),
        ],
    );
    assert_eq!(actions(&data), [LinkAction::Inert("Launch".to_owned())]);
}
