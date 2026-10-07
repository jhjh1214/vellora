//! Smoke test of the object store and the page-tree walk on corpus v0.
//!
//! Needs the corpus (`cargo xtask corpus fetch`), so it only runs on request:
//!
//! ```sh
//! cargo test -p vellora-cos --test store_corpus -- --ignored --nocapture
//! ```
//!
//! For every file: open the store, resolve the catalog and walk all pages. Nothing may panic or
//! hang (60 s guard per file); failures must be typed errors. Encrypted files are opened with the
//! empty password; if that works, the content streams of their first pages must decrypt and decode
//! (a wrong key makes Flate fail), and files that need a password are counted as locked.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use vellora_cos::filter::decode_stream;
use vellora_cos::{Limits, ObjRef, ObjectKind, ObjectStore};

/// What happened to one file.
struct Report {
    opened: bool,
    encrypted: bool,
    locked: bool,
    /// Content streams of encrypted files that decrypted and decoded completely.
    streams_ok: usize,
    /// ... and the ones that did not, with the first error.
    streams_bad: usize,
    first_stream_error: Option<String>,
    repaired: usize,
    pages: usize,
    page_errors: usize,
    first_page_error: Option<String>,
    open_error: Option<String>,
}

fn walk(data: &[u8]) -> Report {
    let mut report = Report {
        opened: false,
        encrypted: false,
        locked: false,
        streams_ok: 0,
        streams_bad: 0,
        first_stream_error: None,
        repaired: 0,
        pages: 0,
        page_errors: 0,
        first_page_error: None,
        open_error: None,
    };
    let store = match ObjectStore::open(data, Limits::default()) {
        Ok(store) => store,
        Err(error) => {
            report.open_error = Some(error.to_string());
            return report;
        }
    };
    report.opened = true;
    report.encrypted = store.encrypt().ok().flatten().is_some();
    report.locked = store.is_locked();
    let _ = store.root();
    let _ = store.info();
    for item in store.pages().take(200_000) {
        match item {
            Ok(_) => report.pages += 1,
            Err(error) => {
                report.page_errors += 1;
                report
                    .first_page_error
                    .get_or_insert_with(|| error.to_string());
            }
        }
    }
    if report.encrypted && !report.locked {
        check_content_streams(&store, &mut report);
    }
    report.repaired = store.repaired().len();
    report
}

/// The `/Contents` streams of the first pages: decrypt, run the filters, and count how many came
/// out complete.
fn check_content_streams(store: &ObjectStore<'_>, report: &mut Report) {
    let limits = Limits::default();
    for page in store.pages().take(3).flatten() {
        let Some(contents) = page.object.as_dict().and_then(|d| d.get(b"Contents")) else {
            continue;
        };
        let mut refs: Vec<ObjRef> = Vec::new();
        match &store.deref(contents).map(|o| o.kind.clone()) {
            Ok(ObjectKind::Array(items)) => {
                refs.extend(items.iter().filter_map(|o| match o.kind {
                    ObjectKind::Ref(r) => Some(r),
                    _ => None,
                }));
            }
            _ => {
                if let ObjectKind::Ref(r) = contents.kind {
                    refs.push(r);
                }
            }
        }
        for reference in refs {
            let outcome = (|| {
                let raw = store.stream_decrypted(reference)?.ok_or("not a stream")?;
                let stream = store.resolve(reference)?;
                let dict = stream.as_dict().ok_or("not a dictionary")?;
                let decoded = decode_stream(dict, &raw, &limits, None, None)?;
                if decoded.complete {
                    Ok(())
                } else {
                    Err("decoded incompletely".into())
                }
            })();
            let outcome: Result<(), Box<dyn std::error::Error>> = outcome;
            match outcome {
                Ok(()) => report.streams_ok += 1,
                Err(error) => {
                    report.streams_bad += 1;
                    report
                        .first_stream_error
                        .get_or_insert_with(|| error.to_string());
                }
            }
        }
    }
}

#[test]
#[ignore = "needs the corpus; run with --ignored"]
fn every_corpus_file_can_be_walked() {
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus-data");
    assert!(
        dir.is_dir(),
        "corpus missing: run `cargo xtask corpus fetch`"
    );
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    paths.sort();

    let (mut opened, mut with_pages, mut repaired, mut encrypted) = (0, 0, 0, 0);
    let (mut locked, mut streams_ok, mut streams_bad) = (0, 0, 0);
    let mut stream_errors = Vec::new();
    let mut total_pages = 0;
    let mut problems = Vec::new();
    let mut open_errors = Vec::new();
    let mut page_errors = Vec::new();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let data = std::fs::read(path).unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(walk(&data));
        });
        match receiver.recv_timeout(Duration::from_secs(60)) {
            Ok(report) => {
                if let Some(error) = report.open_error {
                    open_errors.push(format!("{name}: {error}"));
                    continue;
                }
                opened += usize::from(report.opened);
                encrypted += usize::from(report.encrypted);
                locked += usize::from(report.locked);
                streams_ok += report.streams_ok;
                streams_bad += report.streams_bad;
                if report.streams_bad > 0 {
                    stream_errors.push(format!(
                        "{name}: {} of {} content streams: {}",
                        report.streams_bad,
                        report.streams_bad + report.streams_ok,
                        report.first_stream_error.clone().unwrap_or_default()
                    ));
                }
                repaired += usize::from(report.repaired > 0);
                with_pages += usize::from(report.pages > 0);
                total_pages += report.pages;
                if report.page_errors > 0 {
                    page_errors.push(format!(
                        "{name}: {} errors, {} pages{}: {}",
                        report.page_errors,
                        report.pages,
                        if report.encrypted { " (encrypted)" } else { "" },
                        report.first_page_error.unwrap_or_default()
                    ));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => problems.push(format!("{name}: hang")),
            Err(mpsc::RecvTimeoutError::Disconnected) => problems.push(format!("{name}: panic")),
        }
    }
    println!(
        "files {}: opened {opened} ({repaired} with repairs, {encrypted} encrypted), \
         {with_pages} with pages, {total_pages} pages in all, {} open errors; encrypted:          {locked} locked, {streams_ok} content streams decrypted and decoded,          {streams_bad} not",
        paths.len(),
        open_errors.len()
    );
    for line in &open_errors {
        println!("  open error: {line}");
    }
    for line in &stream_errors {
        println!("  content stream errors: {line}");
    }
    for line in &page_errors {
        println!("  page errors: {line}");
    }
    assert!(
        problems.is_empty(),
        "panics or hangs:\n{}",
        problems.join("\n")
    );
    assert!(
        opened >= 250,
        "only {opened} of {} files opened",
        paths.len()
    );
}

/// Passwords of the encrypted corpus files that have a known one: (file, password, role it
/// opens as). The pdfium "hello world" files use the owner password "âge" and the user password
/// "hôtel", Latin-1 for revisions 2 and 3 and UTF-8 for 5 and 6 (pdfium's
/// `cpdf_security_handler_embeddertest.cpp`, which also gives the other pdfium ones).
/// `pdfjs-issue6010_1`'s passwords were found by trying common words; they are not from a pdf.js
/// document. Together these are independent vectors for every revision of the handler.
const KNOWN_PASSWORDS: &[(&str, &[u8], &str)] = &[
    ("pdfium-bug_1124998", b"test", "User"),
    ("pdfium-encrypted", b"1234", "User"),
    ("pdfium-encrypted", b"5678", "Owner"),
    ("pdfium-encrypted_hello_world_r2", b"h\xF4tel", "User"),
    ("pdfium-encrypted_hello_world_r2", b"\xE2ge", "Owner"),
    ("pdfium-encrypted_hello_world_r3", b"h\xF4tel", "User"),
    ("pdfium-encrypted_hello_world_r3", b"\xE2ge", "Owner"),
    (
        "pdfium-encrypted_hello_world_r5",
        "hôtel".as_bytes(),
        "User",
    ),
    ("pdfium-encrypted_hello_world_r5", "âge".as_bytes(), "Owner"),
    (
        "pdfium-encrypted_hello_world_r6",
        "hôtel".as_bytes(),
        "User",
    ),
    ("pdfium-encrypted_hello_world_r6", "âge".as_bytes(), "Owner"),
    ("pdfjs-issue6010_1", b"abc", "User"),
    ("pdfjs-issue6010_1", b"owner", "Owner"),
];

#[test]
#[ignore = "needs the corpus; run with --ignored"]
fn encrypted_corpus_files_open_with_their_known_passwords() {
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus-data");
    assert!(
        dir.is_dir(),
        "corpus missing: run `cargo xtask corpus fetch`"
    );
    for &(name, password, role) in KNOWN_PASSWORDS {
        let data = std::fs::read(dir.join(format!("{name}.pdf"))).unwrap();
        let store = ObjectStore::open(&data, Limits::default()).unwrap();
        assert!(store.is_locked(), "{name} has a user password");
        assert!(store.authenticate(b"not the password").is_err(), "{name}");
        let got = store.authenticate(password).unwrap();
        assert_eq!(format!("{got:?}"), role, "{name} {password:?}");
        assert!(!store.is_locked(), "{name}");

        let mut report = Report {
            opened: true,
            encrypted: true,
            locked: false,
            streams_ok: 0,
            streams_bad: 0,
            first_stream_error: None,
            repaired: 0,
            pages: 0,
            page_errors: 0,
            first_page_error: None,
            open_error: None,
        };
        check_content_streams(&store, &mut report);
        assert!(
            report.streams_ok > 0 && report.streams_bad == 0,
            "{name}: {} content streams decoded, {} did not: {:?}",
            report.streams_ok,
            report.streams_bad,
            report.first_stream_error
        );
        assert!(store.pages().next().unwrap().is_ok(), "{name}");
        println!("{name}: {role} ok, {} content streams", report.streams_ok);
    }
}
