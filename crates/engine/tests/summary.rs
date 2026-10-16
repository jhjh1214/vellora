//! M1 task 18b: the document report through the real engine. `GetSummary` is answered with the
//! text `vellora inspect --json` prints for the same document, so the Inspector of the shell and
//! the command line cannot drift apart.
//!
//! The comparison with the CLI is made on a small set of fixtures that always runs, and on twenty
//! documents of the corpus when it is there (`#[ignore]`, `cargo xtask corpus fetch` first):
//!
//! ```sh
//! cargo test -p vellora-engine --test summary -- --ignored --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use support::{GOLDEN_PDF, Session};
use vellora_inspect::{Options, inspect};
use vellora_ipc::{ErrorKind, MAX_SUMMARY_JSON_BYTES, Request, RequestId, Response};

fn id(n: u64) -> RequestId {
    RequestId(n)
}

/// What `vellora inspect --json` prints for `data` (without the newline `println!` adds).
fn cli_json(data: &[u8], password: Option<&str>) -> String {
    let mut options = Options::default();
    options.password = password.map(str::to_owned);
    let summary = inspect(data, &options).unwrap();
    serde_json::to_string_pretty(&summary).unwrap()
}

/// Asks an open session for the report.
fn report(engine: &mut Session) -> String {
    engine.send(&Request::GetSummary { req_id: id(70) });
    match engine.recv() {
        Response::Summary { req_id, json } => {
            assert_eq!(req_id, id(70));
            json
        }
        other => panic!("expected a summary, got {other:?}"),
    }
}

fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../cos/tests/fixtures/encryption")
            .join(name),
    )
    .unwrap()
}

/// A one-page document with `extra` objects, a classic cross-reference table and an /Info.
fn document(catalog_extra: &str, page_extra: &str, objects: &[(u32, String)]) -> Vec<u8> {
    let mut data = b"%PDF-1.7\n".to_vec();
    let mut offsets = std::collections::BTreeMap::new();
    let mut add = |data: &mut Vec<u8>, num: u32, body: &str| {
        offsets.insert(num, data.len());
        data.extend(format!("{num} 0 obj\n{body}\nendobj\n").bytes());
    };
    add(
        &mut data,
        1,
        &format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
    );
    add(&mut data, 2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    add(
        &mut data,
        3,
        &format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] {page_extra} >>"),
    );
    add(
        &mut data,
        4,
        "<< /Title (An inspected document) /Producer (Vellora tests) >>",
    );
    for (num, body) in objects {
        add(&mut data, *num, body);
    }
    let size = offsets.keys().next_back().unwrap() + 1;
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
        "trailer\n<< /Size {size} /Root 1 0 R /Info 4 0 R >>\nstartxref\n{at}\n%%EOF\n"
    )
    .unwrap();
    data.extend(table.bytes());
    data
}

#[test]
fn the_engine_reports_what_the_cli_prints() {
    for (name, data) in [
        ("the golden document", GOLDEN_PDF.to_vec()),
        ("a plain document", document("", "", &[])),
        (
            "links and attachments",
            document(
                "/Names << /EmbeddedFiles 10 0 R >>",
                "/Annots [11 0 R]",
                &[
                    (10, "<< /Names [(notes.txt) 12 0 R] >>".to_owned()),
                    (
                        11,
                        "<< /Type /Annot /Subtype /Link /A << /S /URI /URI (https://example.org/x) >> >>"
                            .to_owned(),
                    ),
                ],
            ),
        ),
    ] {
        let mut engine = Session::start(&data);
        engine.handshake();
        assert!(matches!(engine.open(), Response::Opened { .. }), "{name}");
        assert_eq!(report(&mut engine), cli_json(&data, None), "{name}");
    }
}

#[test]
fn the_report_of_a_protected_document_is_made_with_the_password_it_was_opened_with() {
    let data = fixture("r6-aes-256-user-password.pdf");
    let mut engine = Session::start(&data);
    engine.handshake();
    assert!(matches!(
        engine.open_with_password(Some("user-pw")),
        Response::Opened { .. }
    ));
    let json = report(&mut engine);
    assert_eq!(json, cli_json(&data, Some("user-pw")));
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["locked"], false);
    assert_eq!(value["encryption"]["unlocked_with"], "user");
    assert_eq!(value["pages"], 1);

    // With the owner password, the report says so.
    let mut engine = Session::start(&data);
    engine.handshake();
    assert!(matches!(
        engine.open_with_password(Some("owner-pw")),
        Response::Opened { .. }
    ));
    let json = report(&mut engine);
    assert_eq!(json, cli_json(&data, Some("owner-pw")));
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["encryption"]["unlocked_with"], "owner");
    assert_eq!(value["encryption"]["allowed"]["print"], true);
}

#[test]
fn a_report_that_does_not_fit_a_frame_loses_list_entries_and_says_so() {
    // 100 addresses and 100 file names of 1,024 characters of four bytes each: more than a frame.
    let emoji = "F09F9880".repeat(1024);
    let utf8_string = format!("<EFBBBF{emoji}>");
    let annotations: Vec<String> = (0..100).map(|n| format!("{} 0 R", 20 + n)).collect();
    let mut objects: Vec<(u32, String)> = (0..100u32)
        .map(|n| {
            (
                20 + n,
                format!("<< /Type /Annot /Subtype /Link /A << /S /URI /URI {utf8_string} >> >>"),
            )
        })
        .collect();
    let names = format!("{utf8_string} 11 0 R ").repeat(100);
    objects.push((10, format!("<< /Names [{names}] >>")));
    let data = document(
        "/Names << /EmbeddedFiles 10 0 R >>",
        &format!("/Annots [{}]", annotations.join(" ")),
        &objects,
    );
    // The full report is too big...
    assert!(cli_json(&data, None).len() > MAX_SUMMARY_JSON_BYTES);

    let mut engine = Session::start(&data);
    engine.handshake();
    assert!(matches!(engine.open(), Response::Opened { .. }));
    let json = report(&mut engine);
    // ...so the engine's is cut to fit, and the cut is visible in the report itself.
    assert!(json.len() <= MAX_SUMMARY_JSON_BYTES, "{}", json.len());
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    for list in ["uris", "embedded_files"] {
        assert_eq!(value[list]["total"], 100, "{list} keeps its true count");
        assert_eq!(value[list]["truncated"], true, "{list}");
        let kept = value[list]["items"].as_array().unwrap().len();
        assert!(kept > 0 && kept < 100, "{list}: {kept}");
    }
    // Everything else is as the CLI would say.
    let full: serde_json::Value = serde_json::from_str(&cli_json(&data, None)).unwrap();
    for field in [
        "file_size",
        "pages",
        "info",
        "page_sizes",
        "revision_ranges",
        "features",
    ] {
        assert_eq!(value[field], full[field], "{field}");
    }
}

#[test]
fn the_report_is_refused_before_a_document_is_open() {
    let mut engine = Session::start(GOLDEN_PDF);
    engine.handshake();
    engine.send(&Request::GetSummary { req_id: id(71) });
    match engine.recv() {
        Response::Error { req_id, kind, .. } => {
            assert_eq!(req_id, Some(id(71)));
            assert_eq!(kind, ErrorKind::InvalidRequest);
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("VELLORA_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus-data"),
        PathBuf::from,
    )
}

#[test]
#[ignore = "needs the corpus; run with --ignored"]
fn twenty_corpus_documents_have_the_same_report_in_the_engine_and_on_the_command_line() {
    let dir = corpus_dir();
    assert!(
        dir.is_dir(),
        "corpus missing: run `cargo xtask corpus fetch`"
    );
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    paths.sort();
    // Spread over the corpus rather than the first twenty names, which share a source.
    let step = (paths.len() / 60).max(1);
    let mut compared = 0;
    let mut skipped = Vec::new();
    for path in paths.iter().step_by(step) {
        if compared == 20 {
            break;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let data = fs::read(path).unwrap();
        // Only documents the CLI can report on and the engine opens without a password.
        if inspect(&data, &Options::default()).is_err() {
            skipped.push(name);
            continue;
        }
        let mut engine = Session::start(&data);
        engine.handshake();
        if !matches!(engine.open(), Response::Opened { .. }) {
            skipped.push(name);
            continue;
        }
        let engine_json = report(&mut engine);
        let cli = cli_json(&data, None);
        assert_eq!(engine_json, cli, "{name}");
        compared += 1;
    }
    println!("compared {compared} documents; not compared: {skipped:?}");
    assert_eq!(compared, 20, "fewer than 20 documents could be compared");
}
