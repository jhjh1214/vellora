//! Outline, destinations and page labels on corpus v0 (M1 task 12a).
//!
//! Needs the corpus (`cargo xtask corpus fetch`), so it only runs on request:
//!
//! ```sh
//! cargo test -p vellora-cos --test navigation_corpus -- --ignored --nocapture
//! ```
//!
//! For every file that opens: read up to 2,000 outline items, expanding every item that has
//! children (breadth first), resolve their destinations, and read the page labels. Nothing may
//! panic or hang (60 s guard per file); failures must be typed errors, and encrypted files that
//! need a password are skipped.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use vellora_cos::{DestinationResolver, Limits, ObjectStore, Outline, PageLabels};

#[derive(Default)]
struct Report {
    items: usize,
    with_destination: usize,
    labelled_pages: usize,
}

fn walk(data: &[u8]) -> Option<Report> {
    let store = ObjectStore::open(data, Limits::default()).ok()?;
    if store.is_locked() {
        return None;
    }
    let resolver = DestinationResolver::new(&store);
    let outline = Outline::new(&resolver);
    let mut report = Report::default();
    // Parents still to expand (None is the top level).
    let mut queue: VecDeque<Option<u32>> = VecDeque::from([None]);
    while let Some(parent) = queue.pop_front() {
        let mut after = None;
        let mut seen = 0u32;
        while let Ok(page) = outline.children(parent, after, seen, 500) {
            for item in &page.items {
                report.items += 1;
                report.with_destination += usize::from(item.destination.is_some());
                if item.has_children {
                    queue.push_back(Some(item.id));
                }
            }
            seen += u32::try_from(page.items.len()).unwrap();
            after = page.items.last().map(|item| item.id);
            if !page.more || report.items >= 2000 {
                break;
            }
        }
        if report.items >= 2000 {
            break;
        }
    }
    if let Ok(Some(labels)) = PageLabels::read(&resolver) {
        let pages = u32::try_from(resolver.pages().len()).unwrap();
        report.labelled_pages = labels.window(0, pages.min(5000), pages).len();
    }
    Some(report)
}

#[test]
#[ignore = "needs the corpus; run with --ignored"]
fn the_outline_and_labels_of_every_corpus_file_can_be_read() {
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

    let (mut read, mut with_outline, mut with_labels, mut items) = (0, 0, 0, 0);
    let mut problems = Vec::new();
    let mut circular_items = None;
    for path in &paths {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let data = std::fs::read(path).unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(walk(&data));
        });
        match receiver.recv_timeout(Duration::from_secs(60)) {
            Ok(Some(report)) => {
                read += 1;
                with_outline += usize::from(report.items > 0);
                with_labels += usize::from(report.labelled_pages > 0);
                items += report.items;
                if name == "pdfium-bookmarks_circular" {
                    circular_items = Some(report.items);
                }
            }
            Ok(None) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => problems.push(format!("{name}: hang")),
            Err(mpsc::RecvTimeoutError::Disconnected) => problems.push(format!("{name}: panic")),
        }
    }
    println!(
        "files {}: read {read}, {with_outline} with an outline ({items} items), {with_labels} with page labels; \
         pdfium-bookmarks_circular: {circular_items:?} items",
        paths.len()
    );
    assert!(
        problems.is_empty(),
        "panics or hangs:\n{}",
        problems.join("\n")
    );
    // The circular outline is read, and finitely.
    assert!(
        circular_items.is_some(),
        "pdfium-bookmarks_circular missing"
    );
    assert!(
        with_outline >= 5,
        "only {with_outline} files with an outline"
    );
    assert!(
        with_labels >= 2,
        "only {with_labels} files with page labels"
    );
}
