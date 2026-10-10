//! M1 task 15a: searching the text of a document through the real engine executable.
//!
//! The documents are generated: every page has a line of Helvetica text, so what is checked is the
//! protocol and the engine's behaviour (hits, boxes, snippets, options, errors, cancellation, the
//! speed the acceptance criteria ask for), not PDFium's text extraction.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use support::Session;
use vellora_ipc::{ErrorKind, Request, RequestId, Response, SearchHit, SearchOutcome, SearchQuery};

fn id(n: u64) -> RequestId {
    RequestId(n)
}

/// A document of `texts.len()` pages, page `i` showing `texts[i]` as one line of Helvetica
/// (`\n` in a text starts a new line below).
fn document(texts: &[String]) -> Vec<u8> {
    let count = texts.len();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        String::new(), // the page tree, once the kids are known
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_owned(),
    ];
    let mut kids = String::new();
    for text in texts {
        let page = objects.len() + 1;
        write!(kids, "{page} 0 R ").unwrap();
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] /Contents {} 0 R \
             /Resources << /Font << /F1 3 0 R >> >> >>",
            page + 1
        ));
        let mut content = String::from("BT /F1 12 Tf 14 TL 10 280 Td");
        for line in text.split('\n') {
            write!(content, " ({line}) Tj T*").unwrap();
        }
        content.push_str(" ET");
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ));
    }
    objects[1] = format!("<< /Type /Pages /Kids [{kids}] /Count {count} >>");

    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        writeln!(out, "{} 0 obj\n{object}\nendobj", index + 1).unwrap();
    }
    let xref = out.len();
    writeln!(out, "xref\n0 {}\n0000000000 65535 f ", objects.len() + 1).unwrap();
    for offset in offsets {
        writeln!(out, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    out.into_bytes()
}

fn opened(bytes: &[u8]) -> Session {
    let mut engine = Session::start(bytes);
    engine.handshake();
    assert!(
        matches!(engine.open(), Response::Opened { .. }),
        "the document must open"
    );
    engine
}

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.into(),
        case_sensitive: false,
        whole_word: false,
        regex: false,
    }
}

/// What a finished search reported.
struct Found {
    hits: Vec<SearchHit>,
    outcome: SearchOutcome,
    total: u32,
    pages_done: u32,
    /// The progress counts of the messages, in order.
    progress: Vec<u32>,
}

/// Sends `query` as request `n` and reads until it is done. Messages of other requests are not
/// expected.
fn search(engine: &mut Session, n: u64, query: &SearchQuery) -> Found {
    engine.send(&Request::Search {
        req_id: id(n),
        query: query.clone(),
    });
    let mut found = Found {
        hits: Vec::new(),
        outcome: SearchOutcome::Finished,
        total: 0,
        pages_done: 0,
        progress: Vec::new(),
    };
    loop {
        match engine.recv() {
            Response::SearchHits {
                req_id,
                hits,
                pages_done,
            } => {
                assert_eq!(req_id, id(n));
                found.hits.extend(hits);
                found.progress.push(pages_done);
            }
            Response::SearchDone {
                req_id,
                outcome,
                hits,
                pages_done,
            } => {
                assert_eq!(req_id, id(n));
                found.outcome = outcome;
                found.total = hits;
                found.pages_done = pages_done;
                return found;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

fn snippet_match(hit: &SearchHit) -> &str {
    &hit.snippet[hit.match_start as usize..][..hit.match_len as usize]
}

fn texts() -> Vec<String> {
    vec![
        "Alpha needle beta".to_owned(),
        "nothing on this page".to_owned(),
        "NEEDLE in capitals and a needlework".to_owned(),
        "a line that ends with the quick\nbrown fox on the next".to_owned(),
        String::new(),
        "colour and color and gray and grey".to_owned(),
    ]
}

#[test]
fn hits_come_in_page_order_with_boxes_and_snippets() {
    let mut engine = opened(&document(&texts()));
    let found = search(&mut engine, 1, &query("needle"));
    assert_eq!(found.outcome, SearchOutcome::Finished);
    assert_eq!((found.total, found.pages_done), (3, 6));
    assert_eq!(
        found.hits.iter().map(|h| h.page).collect::<Vec<_>>(),
        [0, 2, 2]
    );
    assert_eq!(snippet_match(&found.hits[0]).to_lowercase(), "needle");
    assert_eq!(found.hits[0].snippet, "Alpha needle beta");
    // "Alpha needle": the match starts at character 6 and covers 6.
    assert_eq!((found.hits[0].first, found.hits[0].count), (6, 6));
    let boxes = &found.hits[0].rects;
    assert_eq!(boxes.len(), 1);
    let [left, top, right, bottom] = boxes[0];
    // Page-as-shown frame: 10 points from the left, near the top (the text is at 280 of 300).
    assert!(
        left > 30.0 && right > left && right < 80.0,
        "{left} {right}"
    );
    assert!(top > 5.0 && bottom > top && bottom < 40.0, "{top} {bottom}");
    // Progress ends at the page count.
    assert_eq!(found.progress.last(), Some(&6));
}

#[test]
fn options_change_what_matches() {
    let mut engine = opened(&document(&texts()));
    let exact = SearchQuery {
        case_sensitive: true,
        ..query("needle")
    };
    let found = search(&mut engine, 1, &exact);
    assert_eq!(
        found.hits.iter().map(|h| h.page).collect::<Vec<_>>(),
        [0, 2],
        "needle on page 0 and in needlework on page 2, not NEEDLE"
    );
    let words = SearchQuery {
        whole_word: true,
        ..query("needle")
    };
    let found = search(&mut engine, 2, &words);
    assert_eq!(found.hits.len(), 2, "not needlework");
    let expression = SearchQuery {
        regex: true,
        ..query(r"colou?r|gr[ae]y")
    };
    let found = search(&mut engine, 3, &expression);
    assert_eq!(found.hits.len(), 4);
    assert!(found.hits.iter().all(|h| h.page == 5));
    // Plain text is not an expression.
    let found = search(&mut engine, 4, &query("gr[ae]y"));
    assert_eq!(found.hits.len(), 0);
    assert_eq!(found.outcome, SearchOutcome::Finished);
}

#[test]
fn a_match_may_run_over_a_line_break_and_gets_a_box_per_line() {
    let mut engine = opened(&document(&texts()));
    let found = search(&mut engine, 1, &query("quick brown"));
    assert_eq!(found.hits.len(), 1);
    let hit = &found.hits[0];
    assert_eq!(hit.page, 3);
    assert_eq!(snippet_match(hit), "quick brown");
    assert_eq!(hit.rects.len(), 2, "one box per line: {:?}", hit.rects);
    assert!(
        hit.rects[1][1] > hit.rects[0][1],
        "the second line is lower"
    );
}

#[test]
fn an_expression_that_cannot_be_used_is_refused_with_the_reason() {
    let mut engine = opened(&document(&texts()));
    for (n, bad) in ["(", r"(?<=a)b", r"(a)\1"].into_iter().enumerate() {
        let n = u64::try_from(n).unwrap() + 1;
        engine.send(&Request::Search {
            req_id: id(n),
            query: SearchQuery {
                regex: true,
                ..query(bad)
            },
        });
        match engine.recv() {
            Response::Error {
                req_id,
                kind,
                message,
            } => {
                assert_eq!((req_id, kind), (Some(id(n)), ErrorKind::InvalidRequest));
                assert!(!message.is_empty(), "{bad}");
            }
            other => panic!("expected a refusal for {bad}, got {other:?}"),
        }
    }
    // The engine goes on.
    assert_eq!(search(&mut engine, 9, &query("needle")).hits.len(), 3);
}

#[test]
fn nothing_is_searched_before_a_document_is_open() {
    let mut engine = Session::start(&document(&texts()));
    engine.handshake();
    engine.send(&Request::Search {
        req_id: id(1),
        query: query("needle"),
    });
    match engine.recv() {
        Response::Error { req_id, kind, .. } => {
            assert_eq!((req_id, kind), (Some(id(1)), ErrorKind::InvalidRequest));
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// `pages` pages of text; every `every`th page also says "needle".
fn long_document(pages: usize, every: usize) -> Vec<u8> {
    let texts: Vec<String> = (0..pages)
        .map(|n| {
            let mut text =
                format!("Page {n} of the generated document, with some more words in it");
            if n % every == 7 {
                text.push_str(" and a needle");
            }
            text
        })
        .collect();
    document(&texts)
}

/// Reads responses until `done` says one is the end; returns how long the first `SearchHits` with a
/// hit took, if there was one, and the end.
fn timed_search(engine: &mut Session, n: u64, query: &SearchQuery) -> (Option<Duration>, Found) {
    let started = Instant::now();
    engine.send(&Request::Search {
        req_id: id(n),
        query: query.clone(),
    });
    let mut first_hit = None;
    let mut found = Found {
        hits: Vec::new(),
        outcome: SearchOutcome::Finished,
        total: 0,
        pages_done: 0,
        progress: Vec::new(),
    };
    loop {
        match engine.recv() {
            Response::SearchHits {
                hits, pages_done, ..
            } => {
                if first_hit.is_none() && !hits.is_empty() {
                    first_hit = Some(started.elapsed());
                }
                found.hits.extend(hits);
                found.progress.push(pages_done);
            }
            Response::SearchDone {
                outcome,
                hits,
                pages_done,
                ..
            } => {
                found.outcome = outcome;
                found.total = hits;
                found.pages_done = pages_done;
                return (first_hit, found);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[test]
fn the_first_hit_in_ten_thousand_pages_comes_within_a_second() {
    let mut engine = opened(&long_document(10_000, 1000));
    let (first, found) = timed_search(&mut engine, 1, &query("needle"));
    let first = first.expect("a hit");
    println!("first hit after {first:?}");
    assert!(
        first < Duration::from_secs(1),
        "the first hit took {first:?}"
    );
    assert_eq!(found.outcome, SearchOutcome::Finished);
    assert_eq!((found.total, found.pages_done), (10, 10_000));
    assert_eq!(found.hits[0].page, 7);
}

#[test]
fn a_full_scan_of_a_thousand_pages_takes_under_three_seconds() {
    let mut engine = opened(&long_document(1000, 100));
    let started = Instant::now();
    let (_, found) = timed_search(&mut engine, 1, &query("needle"));
    let took = started.elapsed();
    println!("1000 pages in {took:?}");
    assert_eq!(found.pages_done, 1000);
    assert_eq!(found.total, 10);
    assert!(took < Duration::from_secs(3), "the scan took {took:?}");
}

#[test]
fn cancelling_stops_the_engine_within_a_hundred_milliseconds() {
    let mut engine = opened(&long_document(3000, 100_000));
    engine.send(&Request::Search {
        req_id: id(1),
        query: query("no such words anywhere"),
    });
    // Wait until the search is under way.
    match engine.recv() {
        Response::SearchHits { req_id, .. } => assert_eq!(req_id, id(1)),
        other => panic!("unexpected {other:?}"),
    }
    let cancelled = Instant::now();
    engine.send(&Request::Cancel { req_id: id(1) });
    let (pages_done, took) = loop {
        match engine.recv() {
            Response::SearchHits { .. } => {}
            Response::SearchDone {
                outcome,
                pages_done,
                ..
            } => {
                assert_eq!(outcome, SearchOutcome::Cancelled);
                break (pages_done, cancelled.elapsed());
            }
            other => panic!("unexpected {other:?}"),
        }
    };
    println!("stopped {took:?} after the cancel");
    assert!(took < Duration::from_millis(100), "stopped after {took:?}");
    assert!(pages_done < 3000, "it must not have finished");
    // Nothing more is sent, and the engine still answers.
    engine.send(&Request::Search {
        req_id: id(2),
        query: query("Page 1 of"),
    });
    let (_, found) = timed_search_after(&mut engine, id(2));
    assert_eq!(found.total, 1, "only page 1 starts with that");
}

/// Reads the rest of a search that was already requested.
fn timed_search_after(engine: &mut Session, request: RequestId) -> (Option<Duration>, Found) {
    let mut found = Found {
        hits: Vec::new(),
        outcome: SearchOutcome::Finished,
        total: 0,
        pages_done: 0,
        progress: Vec::new(),
    };
    loop {
        match engine.recv() {
            Response::SearchHits {
                req_id,
                hits,
                pages_done,
            } => {
                assert_eq!(req_id, request, "a cancelled search must stay quiet");
                found.hits.extend(hits);
                found.progress.push(pages_done);
            }
            Response::SearchDone {
                req_id,
                outcome,
                hits,
                pages_done,
            } => {
                assert_eq!(req_id, request, "a cancelled search must stay quiet");
                found.outcome = outcome;
                found.total = hits;
                found.pages_done = pages_done;
                return (None, found);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[test]
fn a_new_search_ends_the_one_before_it() {
    let mut engine = opened(&long_document(3000, 100_000));
    engine.send(&Request::Search {
        req_id: id(1),
        query: query("no such words anywhere"),
    });
    match engine.recv() {
        Response::SearchHits { req_id, .. } => assert_eq!(req_id, id(1)),
        other => panic!("unexpected {other:?}"),
    }
    engine.send(&Request::Search {
        req_id: id(2),
        query: query("Page 2999 of"),
    });
    let mut ended = None;
    let mut hits = Vec::new();
    loop {
        match engine.recv() {
            Response::SearchHits {
                req_id,
                hits: found,
                ..
            } => {
                assert!(
                    ended.is_some() || req_id == id(1),
                    "the new search only speaks after the old one is done"
                );
                if req_id == id(2) {
                    hits.extend(found);
                }
            }
            Response::SearchDone {
                req_id, outcome, ..
            } if req_id == id(1) => {
                assert_eq!(outcome, SearchOutcome::Cancelled);
                ended = Some(());
            }
            Response::SearchDone {
                req_id,
                outcome,
                hits: total,
                ..
            } => {
                assert_eq!(
                    (req_id, outcome, total),
                    (id(2), SearchOutcome::Finished, 1)
                );
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(ended.is_some());
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].page, 2999);
}

#[test]
fn searching_does_not_hold_up_tiles() {
    let mut engine = opened(&long_document(3000, 100_000));
    engine.send(&Request::Search {
        req_id: id(1),
        query: query("no such words anywhere"),
    });
    let started = Instant::now();
    engine.send(&Request::RenderTile {
        req_id: id(2),
        page: 0,
        scale: 1.0,
        rect: vellora_ipc::TileRect {
            x: 0,
            y: 0,
            width: 256,
            height: 256,
        },
        slot: vellora_ipc::SlotId(0),
        priority: vellora_ipc::Priority::Visible,
    });
    loop {
        match engine.recv() {
            Response::TileReady { req_id, .. } => {
                assert_eq!(req_id, id(2));
                break;
            }
            Response::SearchHits { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the tile waited {:?} behind the search",
        started.elapsed()
    );
    engine.send(&Request::Cancel { req_id: id(1) });
}
