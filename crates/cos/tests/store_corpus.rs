//! Smoke test of the object store and the page-tree walk on corpus v0.
//!
//! Needs the corpus (`cargo xtask corpus fetch`), so it only runs on request:
//!
//! ```sh
//! cargo test -p vellora-cos --test store_corpus -- --ignored --nocapture
//! ```
//!
//! For every file: open the store, resolve the catalog and walk all pages. Nothing may panic or
//! hang (60 s guard per file); failures must be typed errors. Encrypted files cannot be read
//! beyond their plain objects until decryption exists (M0 task 11) and are counted separately.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use vellora_cos::{Limits, ObjectStore};

/// What happened to one file.
struct Report {
    opened: bool,
    encrypted: bool,
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
    report.repaired = store.repaired().len();
    report
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
         {with_pages} with pages, {total_pages} pages in all, {} open errors",
        paths.len(),
        open_errors.len()
    );
    for line in &open_errors {
        println!("  open error: {line}");
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
