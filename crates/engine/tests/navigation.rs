//! M1 task 12b: the outline, page labels and section requests through the real engine executable.
//!
//! The documents are small synthetic files (the readers themselves are tested in `vellora-cos`);
//! what is checked here is the protocol: lazy levels and paging, the answers' contents, errors
//! before a document is open, loops in hostile outlines, encrypted files, and that navigation
//! requests and tile requests share a conversation without holding each other up.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fmt::Write as _;
use std::fs;

use support::{GOLDEN_PDF, Session};
use vellora_ipc::{
    Destination, ErrorKind, Fit, Link, LinkAction, NamedAction, OutlineEntry, Priority, Request,
    RequestId, Response, SlotId, TileRect, TitleStyle,
};

/// The object number of page `index`.
const fn page(index: u32) -> u32 {
    3 + index
}

const PAGES: u32 = 8;
/// The first object after the catalog, the page tree and the pages.
const FIRST_FREE: u32 = page(PAGES);

/// A PDF of [`PAGES`] blank pages with a classic xref table. `catalog_extra` goes into the catalog
/// dictionary; `objects` are numbered from [`FIRST_FREE`].
fn pdf(catalog_extra: &str, objects: &[String]) -> Vec<u8> {
    pdf_with_first_page(catalog_extra, "", objects)
}

/// [`pdf`] with `first_page_extra` in the dictionary of the first page.
fn pdf_with_first_page(catalog_extra: &str, first_page_extra: &str, objects: &[String]) -> Vec<u8> {
    let kids = (0..PAGES)
        .map(|i| format!("{} 0 R", page(i)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut all = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
        format!("<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>"),
    ];
    all.extend((0..PAGES).map(|i| {
        let extra = if i == 0 { first_page_extra } else { "" };
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] {extra} >>")
    }));
    all.extend(objects.iter().cloned());

    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in all.iter().enumerate() {
        offsets.push(out.len());
        writeln!(out, "{} 0 obj\n{object}\nendobj", index + 1).unwrap();
    }
    let xref = out.len();
    writeln!(out, "xref\n0 {}\n0000000000 65535 f ", all.len() + 1).unwrap();
    for offset in offsets {
        writeln!(out, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        all.len() + 1
    )
    .unwrap();
    out.into_bytes()
}

/// Ids of the outline objects in [`documented`].
const ROOT: u32 = FIRST_FREE;
const PREFACE: u32 = ROOT + 1;
const CHAPTER: u32 = ROOT + 2;
const SECTION_1: u32 = ROOT + 3;
const SECTION_2: u32 = ROOT + 4;
const APPENDIX: u32 = ROOT + 5;

/// Eight pages labelled i, ii, 1, 2, 3, A-1, A-2, A-3, with this outline:
///
/// - Preface (page 1)
/// - Chapter 1 (page 3, open)
///   - Section 1.1 (page 3)
///   - Section 1.2 (page 5)
/// - Appendix (page 6, bold italic, by the named destination `appendix`)
fn documented() -> Vec<u8> {
    let at = |index: u32, rest: &str| format!("[{} 0 R {rest}]", page(index));
    let catalog = format!(
        "/Outlines {ROOT} 0 R /Dests << /appendix {} >> \
         /PageLabels << /Nums [0 << /S /r >> 2 << /S /D >> 5 << /S /D /P (A-) >>] >>",
        at(5, "/XYZ 10 20 null"),
    );
    let objects = [
        format!("<< /Type /Outlines /First {PREFACE} 0 R /Last {APPENDIX} 0 R /Count 3 >>"),
        format!(
            "<< /Title (Preface) /Parent {ROOT} 0 R /Next {CHAPTER} 0 R /Dest {} >>",
            at(0, "/Fit")
        ),
        format!(
            "<< /Title (Chapter 1) /Parent {ROOT} 0 R /Prev {PREFACE} 0 R /Next {APPENDIX} 0 R \
             /First {SECTION_1} 0 R /Last {SECTION_2} 0 R /Count 2 /Dest {} >>",
            at(2, "/XYZ 72 700 2")
        ),
        format!(
            "<< /Title (Section 1.1) /Parent {CHAPTER} 0 R /Next {SECTION_2} 0 R /Dest {} >>",
            at(2, "/FitH 500")
        ),
        format!(
            "<< /Title (Section 1.2) /Parent {CHAPTER} 0 R /Prev {SECTION_1} 0 R /Dest {} >>",
            at(4, "/FitR 1 2 3 4")
        ),
        format!(
            "<< /Title (Appendix) /Parent {ROOT} 0 R /Prev {CHAPTER} 0 R /F 3 /Dest /appendix >>"
        ),
    ];
    pdf(&catalog, &objects)
}

fn started(bytes: &[u8]) -> Session {
    let mut engine = Session::start(bytes);
    engine.handshake();
    engine
}

fn opened(bytes: &[u8]) -> Session {
    let mut engine = started(bytes);
    assert!(
        matches!(engine.open(), Response::Opened { .. }),
        "the document must open"
    );
    engine
}

fn id(n: u64) -> RequestId {
    RequestId(n)
}

fn outline(
    engine: &mut Session,
    parent: Option<u32>,
    after: Option<u32>,
    already: u32,
    limit: u32,
) -> (Vec<OutlineEntry>, bool) {
    engine.send(&Request::GetOutline {
        req_id: id(1),
        parent,
        after,
        already,
        limit,
    });
    match engine.recv() {
        Response::Outline {
            req_id,
            items,
            more,
        } => {
            assert_eq!(req_id, id(1));
            (items, more)
        }
        other => panic!("expected an outline, got {other:?}"),
    }
}

fn path(engine: &mut Session, page: u32) -> Vec<u32> {
    engine.send(&Request::GetOutlinePath {
        req_id: id(2),
        page,
    });
    match engine.recv() {
        Response::OutlinePath { req_id, path } => {
            assert_eq!(req_id, id(2));
            path
        }
        other => panic!("expected a path, got {other:?}"),
    }
}

fn labels(engine: &mut Session, first: u32, count: u32) -> (bool, Vec<String>) {
    engine.send(&Request::GetPageLabels {
        req_id: id(3),
        first,
        count,
    });
    match engine.recv() {
        Response::PageLabels {
            req_id,
            first: echoed,
            defined,
            labels,
        } => {
            assert_eq!((req_id, echoed), (id(3), first));
            (defined, labels)
        }
        other => panic!("expected labels, got {other:?}"),
    }
}

fn find(engine: &mut Session, text: &str) -> Option<u32> {
    engine.send(&Request::FindPageLabel {
        req_id: id(4),
        text: text.to_owned(),
    });
    match engine.recv() {
        Response::PageFound { req_id, page } => {
            assert_eq!(req_id, id(4));
            page
        }
        other => panic!("expected a page, got {other:?}"),
    }
}

fn titles(items: &[OutlineEntry]) -> Vec<&str> {
    items.iter().map(|item| item.title.as_str()).collect()
}

fn ids(items: &[OutlineEntry]) -> Vec<u32> {
    items.iter().map(|item| item.id).collect()
}

fn dest(page: u32, fit: Fit) -> Destination {
    Destination { page, fit }
}

#[test]
fn the_top_level_comes_first_with_destinations_and_flags() {
    let mut engine = opened(&documented());
    let (items, more) = outline(&mut engine, None, None, 0, 128);
    assert!(!more);
    assert_eq!(titles(&items), ["Preface", "Chapter 1", "Appendix"]);
    assert_eq!(ids(&items), [PREFACE, CHAPTER, APPENDIX]);

    assert_eq!(items[0].destination, Some(dest(0, Fit::Fit)));
    assert_eq!(
        items[1].destination,
        Some(dest(
            2,
            Fit::Xyz {
                left: Some(72.0),
                top: Some(700.0),
                zoom: Some(2.0)
            }
        ))
    );
    // A named destination, found in the catalog's /Dests.
    assert_eq!(
        items[2].destination,
        Some(dest(
            5,
            Fit::Xyz {
                left: Some(10.0),
                top: Some(20.0),
                zoom: None
            }
        ))
    );
    assert_eq!(
        items
            .iter()
            .map(|i| (i.has_children, i.open))
            .collect::<Vec<_>>(),
        [(false, false), (true, true), (false, false)]
    );
    assert_eq!(
        items[2].style,
        TitleStyle {
            bold: true,
            italic: true
        }
    );
    assert_eq!(items[0].style, TitleStyle::default());
}

#[test]
fn children_are_asked_for_by_their_parent() {
    let mut engine = opened(&documented());
    let (items, more) = outline(&mut engine, Some(CHAPTER), None, 0, 128);
    assert!(!more);
    assert_eq!(titles(&items), ["Section 1.1", "Section 1.2"]);
    assert_eq!(
        items[0].destination,
        Some(dest(2, Fit::FitH { top: Some(500.0) }))
    );
    assert_eq!(
        items[1].destination,
        Some(dest(
            4,
            Fit::FitR {
                left: 1.0,
                bottom: 2.0,
                right: 3.0,
                top: 4.0
            }
        ))
    );
    // A leaf has none; an id that is not an outline item has none either.
    for parent in [PREFACE, 4_000] {
        let (items, more) = outline(&mut engine, Some(parent), None, 0, 128);
        assert_eq!((items.len(), more), (0, false), "parent {parent}");
    }
}

#[test]
fn a_level_is_read_a_page_at_a_time() {
    let mut engine = opened(&documented());
    let (first, more) = outline(&mut engine, None, None, 0, 1);
    assert_eq!((ids(&first), more), (vec![PREFACE], true));
    let (second, more) = outline(&mut engine, None, Some(PREFACE), 1, 1);
    assert_eq!((ids(&second), more), (vec![CHAPTER], true));
    let (third, more) = outline(&mut engine, None, Some(CHAPTER), 2, 1);
    assert_eq!((ids(&third), more), (vec![APPENDIX], false));
    // After the last item there is nothing, and that is not an error.
    let (none, more) = outline(&mut engine, None, Some(APPENDIX), 3, 1);
    assert_eq!((none.len(), more), (0, false));
}

#[test]
fn the_section_of_a_page_is_the_path_to_the_latest_start_before_it() {
    let mut engine = opened(&documented());
    for (page, expected) in [
        (0, vec![PREFACE]),
        (1, vec![PREFACE]),
        (2, vec![CHAPTER, SECTION_1]),
        (3, vec![CHAPTER, SECTION_1]),
        (4, vec![CHAPTER, SECTION_2]),
        (5, vec![APPENDIX]),
        (7, vec![APPENDIX]),
        // Past the last page: the last section still.
        (500, vec![APPENDIX]),
    ] {
        assert_eq!(path(&mut engine, page), expected, "page {page}");
    }
}

#[test]
fn labels_come_in_windows_and_a_label_finds_its_page() {
    let mut engine = opened(&documented());
    let all = ["i", "ii", "1", "2", "3", "A-1", "A-2", "A-3"];
    assert_eq!(
        labels(&mut engine, 0, 1024),
        (true, all.map(String::from).to_vec())
    );
    assert_eq!(
        labels(&mut engine, 6, 1024),
        (true, vec!["A-2".to_owned(), "A-3".to_owned()])
    );
    assert_eq!(
        labels(&mut engine, 1, 2),
        (true, vec!["ii".to_owned(), "1".to_owned()])
    );
    assert_eq!(labels(&mut engine, 8, 10), (true, Vec::new()));

    for (text, page) in [
        ("i", Some(0)),
        ("ii", Some(1)),
        ("1", Some(2)),
        ("3", Some(4)),
        ("A-3", Some(7)),
        // Case differences are forgiven when nothing matches exactly.
        ("II", Some(1)),
        ("a-1", Some(5)),
        ("4", None),
        ("iii", None),
        ("", None),
    ] {
        assert_eq!(find(&mut engine, text), page, "{text:?}");
    }
}

#[test]
fn a_document_without_an_outline_or_labels_answers_with_nothing() {
    let mut engine = opened(GOLDEN_PDF);
    let (items, more) = outline(&mut engine, None, None, 0, 128);
    assert_eq!((items.len(), more), (0, false));
    assert_eq!(path(&mut engine, 2), Vec::<u32>::new());
    // The page numbers stand in for the labels.
    assert_eq!(
        labels(&mut engine, 0, 1024),
        (false, vec!["1".to_owned(), "2".to_owned(), "3".to_owned()])
    );
    assert_eq!(find(&mut engine, "1"), None);
}

#[test]
fn nothing_is_answered_before_a_document_is_open() {
    let mut engine = started(&documented());
    let requests = [
        Request::GetOutline {
            req_id: id(7),
            parent: None,
            after: None,
            already: 0,
            limit: 10,
        },
        Request::GetOutlinePath {
            req_id: id(8),
            page: 0,
        },
        Request::GetPageLabels {
            req_id: id(9),
            first: 0,
            count: 10,
        },
        Request::FindPageLabel {
            req_id: id(10),
            text: "1".into(),
        },
    ];
    for (request, expected) in requests.iter().zip([7, 8, 9, 10]) {
        engine.send(request);
        match engine.recv() {
            Response::Error {
                req_id,
                kind,
                message,
            } => {
                assert_eq!(
                    (req_id, kind),
                    (Some(id(expected)), ErrorKind::InvalidRequest),
                    "{message}"
                );
                assert!(message.contains("no document"), "{message}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
    // The session is still usable: the document opens.
    assert!(matches!(engine.open(), Response::Opened { .. }));
}

/// A tile of page 1.
fn tile_request(n: u64) -> Request {
    Request::RenderTile {
        req_id: id(n),
        page: 0,
        scale: 1.0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: 64,
            height: 64,
        },
        slot: SlotId(0),
        priority: Priority::Visible,
    }
}

#[test]
fn navigation_and_tiles_share_a_conversation() {
    let mut engine = opened(&documented());
    engine.send(&tile_request(100));
    engine.send(&Request::GetOutline {
        req_id: id(101),
        parent: None,
        after: None,
        already: 0,
        limit: 128,
    });
    engine.send(&Request::GetPageLabels {
        req_id: id(102),
        first: 0,
        count: 8,
    });
    engine.send(&Request::GetOutlinePath {
        req_id: id(103),
        page: 4,
    });
    // Answers may arrive in any order; each has the id of its request.
    let mut answered = Vec::new();
    for _ in 0..4 {
        answered.push(match engine.recv() {
            Response::TileReady { req_id, .. }
            | Response::Outline { req_id, .. }
            | Response::PageLabels { req_id, .. }
            | Response::OutlinePath { req_id, .. } => req_id.0,
            other => panic!("unexpected {other:?}"),
        });
    }
    answered.sort_unstable();
    assert_eq!(answered, [100, 101, 102, 103]);
}

/// A hostile outline: the top level goes round in a circle, and Chapter 1's first child is its own
/// ancestor.
fn looping() -> Vec<u8> {
    let at = |index: u32| format!("[{} 0 R /Fit]", page(index));
    let objects = [
        format!("<< /Type /Outlines /First {PREFACE} 0 R /Last {CHAPTER} 0 R >>"),
        format!(
            "<< /Title (Preface) /Parent {ROOT} 0 R /Next {CHAPTER} 0 R /Dest {} >>",
            at(0)
        ),
        format!(
            "<< /Title (Chapter 1) /Parent {ROOT} 0 R /Next {PREFACE} 0 R /First {SECTION_1} 0 R \
             /Dest {} >>",
            at(2)
        ),
        format!(
            "<< /Title (Section) /Parent {CHAPTER} 0 R /First {CHAPTER} 0 R /Next {SECTION_1} 0 R \
             /Dest {} >>",
            at(3)
        ),
    ];
    pdf(&format!("/Outlines {ROOT} 0 R"), &objects)
}

#[test]
fn loops_in_the_outline_end_and_do_not_stop_the_engine() {
    let mut engine = opened(&looping());
    let (items, more) = outline(&mut engine, None, None, 0, 128);
    assert_eq!(ids(&items), [PREFACE, CHAPTER]);
    assert!(!more);
    // The section whose child is its own ancestor can be expanded, one level at a time, and the
    // search for a page's section finishes.
    let (children, _) = outline(&mut engine, Some(CHAPTER), None, 0, 128);
    assert_eq!(ids(&children), [SECTION_1]);
    assert_eq!(path(&mut engine, 3), [CHAPTER, SECTION_1]);
    assert_eq!(path(&mut engine, 2), [CHAPTER]);
    assert_eq!(path(&mut engine, 0), [PREFACE]);
    // Still serving tiles.
    engine.send(&tile_request(1));
    assert!(matches!(engine.recv(), Response::TileReady { .. }));
}

/// Page labels whose number tree is broken in one place: the range with no usable entry is
/// skipped and the rest still reads.
#[test]
fn damaged_label_entries_do_not_hide_the_good_ones() {
    let catalog = "/PageLabels << /Nums [0 (nonsense) 2 << /S /a /P (x) >> 4 5] >>";
    let mut engine = opened(&pdf(catalog, &[]));
    let (defined, listed) = labels(&mut engine, 0, 8);
    assert!(defined);
    assert_eq!(listed.len(), 8);
    assert_eq!(&listed[2..4], ["xa", "xb"]);
}

/// The protected fixtures have no outline, but opening them for navigation needs the password:
/// an answer other than "refused" shows that the navigator unlocked the document.
#[test]
fn protected_documents_are_navigable_once_they_are_opened_with_their_password() {
    for name in [
        "r2-rc4-40-user-password",
        "r3-rc4-128-user-password",
        "r4-aes-128-user-password",
        "r6-aes-256-user-password",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../cos/tests/fixtures/encryption")
            .join(format!("{name}.pdf"));
        let bytes = fs::read(&path).unwrap();
        for password in ["user-pw", "owner-pw"] {
            let mut engine = started(&bytes);
            let answer = engine.open_with_password(Some(password));
            assert!(
                matches!(answer, Response::Opened { .. }),
                "{name}: {answer:?}"
            );
            let (items, more) = outline(&mut engine, None, None, 0, 128);
            assert_eq!((items.len(), more), (0, false), "{name} / {password}");
            let (defined, listed) = labels(&mut engine, 0, 10);
            assert!(!defined, "{name}");
            assert_eq!(listed, ["1"], "{name}");
        }
    }
}

// ---- links (task 13a) ----

fn get_links(engine: &mut Session, page: u32, skip: u32, limit: u32) -> (Vec<Link>, bool) {
    engine.send(&Request::GetLinks {
        req_id: id(5),
        page,
        skip,
        limit,
    });
    match engine.recv() {
        Response::Links {
            req_id,
            page: answered,
            links,
            more,
        } => {
            assert_eq!((req_id, answered), (id(5), page));
            (links, more)
        }
        other => panic!("expected links, got {other:?}"),
    }
}

/// A first page with one link of every kind the protocol distinguishes, and a second page to go to.
fn linked() -> Vec<u8> {
    let link = |rect: &str, entry: &str| {
        format!("<< /Type /Annot /Subtype /Link /Rect [{rect}] {entry} >>")
    };
    let annots = [
        link("10 10 20 20", &format!("/Dest [{} 0 R /Fit]", page(1))),
        link(
            "10 30 20 40",
            "/A << /S /URI /URI (https://example.org/x) >>",
        ),
        link("10 50 20 60", "/A << /S /Named /N /NextPage >>"),
        link("10 70 20 80", "/A << /S /Launch /F (calc.exe) >>"),
        link("30 10 40 20", "/A << /S /JavaScript /JS (app.alert(1)) >>"),
        link(
            "30 30 40 40",
            "/A << /S /GoToR /F (other.pdf) /D [0 /Fit] >>",
        ),
        link("30 50 40 60", "/A << /S /SubmitForm /F (http://x/post) >>"),
        link("30 70 40 80", "/A << /S /ImportData /F (data.fdf) >>"),
        link("50 10 60 20", "/Dest /nowhere"),
    ]
    .join(" ");
    pdf_with_first_page("", &format!("/Annots [{annots}]"), &[])
}

#[test]
fn every_kind_of_link_action_is_described_and_none_is_run() {
    let mut engine = opened(&linked());
    let (links, more) = get_links(&mut engine, 0, 0, 256);
    assert!(!more);
    let actions: Vec<LinkAction> = links.iter().map(|link| link.action.clone()).collect();
    let inert = |name: &str| LinkAction::Inert(name.to_owned());
    assert_eq!(
        actions,
        [
            LinkAction::GoTo(Destination {
                page: 1,
                fit: Fit::Fit
            }),
            LinkAction::Uri("https://example.org/x".to_owned()),
            LinkAction::Named(NamedAction::NextPage),
            inert("Launch"),
            inert("JavaScript"),
            inert("GoToR"),
            inert("SubmitForm"),
            inert("ImportData"),
            LinkAction::Unresolved,
        ]
    );
    // The page is 100 x 100: a link at y 10 to 20 (up from the bottom) is 80 to 90 down from the top.
    assert_eq!(links[0].rect, [10.0, 80.0, 20.0, 90.0]);
    // Still serving: the engine did not launch anything or end.
    engine.send(&tile_request(1));
    assert!(matches!(engine.recv(), Response::TileReady { .. }));
}

#[test]
fn links_come_in_windows_and_other_pages_have_none() {
    let mut engine = opened(&linked());
    let (first, more) = get_links(&mut engine, 0, 0, 4);
    assert_eq!((first.len(), more), (4, true));
    let (second, more) = get_links(&mut engine, 0, 4, 4);
    assert_eq!((second.len(), more), (4, true));
    let (last, more) = get_links(&mut engine, 0, 8, 4);
    assert_eq!((last.len(), more), (1, false));
    assert_eq!(last[0].action, LinkAction::Unresolved);
    let (none, more) = get_links(&mut engine, 3, 0, 4);
    assert_eq!((none.len(), more), (0, false));

    // A page that does not exist is refused, naming the request.
    engine.send(&Request::GetLinks {
        req_id: id(6),
        page: PAGES,
        skip: 0,
        limit: 1,
    });
    match engine.recv() {
        Response::Error { req_id, kind, .. } => {
            assert_eq!((req_id, kind), (Some(id(6)), ErrorKind::InvalidRequest));
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn links_are_refused_before_a_document_is_open() {
    let mut engine = started(&linked());
    engine.send(&Request::GetLinks {
        req_id: id(9),
        page: 0,
        skip: 0,
        limit: 1,
    });
    match engine.recv() {
        Response::Error { req_id, kind, .. } => {
            assert_eq!((req_id, kind), (Some(id(9)), ErrorKind::InvalidRequest));
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Acceptance of task 13: describing links, and rendering the page they are on, touches no
/// network. Links to a local listener (an address, a form submission, a remote jump) are read and
/// the page is rendered; the listener must see nothing.
#[test]
fn describing_links_makes_no_network_connection() {
    use std::net::TcpListener;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/x");
    let link = |action: String| {
        format!("<< /Type /Annot /Subtype /Link /Rect [10 10 20 20] /A << {action} >> >>")
    };
    let annots = [
        link(format!("/S /URI /URI ({url})")),
        link(format!("/S /SubmitForm /F ({url})")),
        link(format!("/S /GoToR /F ({url}) /D [0 /Fit]")),
        link(format!("/S /ImportData /F ({url})")),
        link(format!("/S /Launch /F ({url})")),
    ]
    .join(" ");
    let mut engine = opened(&pdf_with_first_page(
        "",
        &format!("/Annots [{annots}]"),
        &[],
    ));
    let (links, _) = get_links(&mut engine, 0, 0, 256);
    assert_eq!(links.len(), 5);
    assert_eq!(links[0].action, LinkAction::Uri(url));
    engine.send(&tile_request(1));
    assert!(matches!(engine.recv(), Response::TileReady { .. }));

    std::thread::sleep(Duration::from_millis(500));
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("the engine connected to a link target: {other:?}"),
    }
}
