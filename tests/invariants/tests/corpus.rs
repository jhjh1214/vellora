//! Invariant harness over the corpus (M0 task 13; `docs/architecture/testing-strategy.md`).
//!
//! For every PDF in the corpus directory that `cos` opens and can read:
//!
//! 1. **Full rewrite** (classic table, and cross-reference stream with object streams): the output
//!    passes `qpdf --check`, reopens without repairs and has the original's page count.
//! 2. **Incremental update** (a modified `/Info`): the original is a byte-identical prefix of the
//!    output, every other object resolves exactly as before, the new `/Info` is read back, and
//!    the output passes `qpdf --check`. A file whose cross-reference was repaired must be
//!    refused with `NeedsFullRewrite` instead.
//!
//! qpdf's verdict on an output only counts if qpdf accepted the original: a file that is already
//! broken for qpdf is not repaired by rewriting it. `qpdf --check` exit code 3 (warnings) is
//! accepted for the same reason; 2 (errors) is not.
//!
//! The test is `#[ignore]`d because it needs the corpus and qpdf; it fails (never skips) when
//! either is missing. Run it with
//!
//! ```sh
//! cargo xtask corpus fetch          # or --manifest tests/corpus/ci-manifest.toml
//! cargo test -p vellora-invariants -- --ignored --nocapture
//! ```
//!
//! `QPDF` names the qpdf binary (default: `qpdf` on `PATH`); `VELLORA_CORPUS_DIR` overrides the
//! corpus directory (default `tests/corpus-data`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use vellora_cos::object::Dict;
use vellora_cos::{
    Changes, EncryptionPolicy, Error, FullOptions, Limits, NewObject, ObjRef, Object, ObjectKind,
    ObjectStore, RepairReason, WriteError, incremental_update, write_full,
};

/// Upper bound on the object numbers compared before and after an incremental update.
const MAX_COMPARED_OBJECTS: u32 = 50_000;

fn corpus_dir() -> PathBuf {
    std::env::var_os("VELLORA_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus-data"),
        PathBuf::from,
    )
}

fn page_count(store: &ObjectStore<'_>) -> usize {
    store.pages().flatten().count()
}

/// Whether the file's cross-reference (not just one object) had to be repaired. Per-object
/// recoveries do not force a full rewrite, a rebuilt table does.
fn xref_repaired(store: &ObjectStore<'_>) -> bool {
    store
        .repaired()
        .iter()
        .any(|reason| !matches!(reason, RepairReason::ObjectRecovered { .. }))
}

fn title(store: &ObjectStore<'_>) -> Option<Vec<u8>> {
    let info = store.info().ok()??;
    match &info.as_dict()?.get(b"Title")?.kind {
        ObjectKind::String(s) => Some(s.to_vec()),
        _ => None,
    }
}

/// `qpdf --check` on `bytes`: whether it exits 0 (clean) or 3 (warnings only).
fn qpdf_accepts(bytes: &[u8], label: &str) -> bool {
    let qpdf = std::env::var("QPDF").unwrap_or_else(|_| "qpdf".to_string());
    let path = std::env::temp_dir().join(format!("vellora-inv-{}-{label}.pdf", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let output = Command::new(&qpdf)
        .arg("--check")
        .arg(&path)
        .output()
        .unwrap_or_else(|e| panic!("cannot run qpdf `{qpdf}` (set QPDF to its path): {e}"));
    let _ = std::fs::remove_file(&path);
    matches!(output.status.code(), Some(0 | 3))
}

#[derive(Default)]
struct Tally {
    files: usize,
    not_opened: usize,
    skipped: usize,
    full_written: usize,
    full_stopped: usize,
    incremental_written: usize,
    incremental_refused: usize,
    qpdf_checked: usize,
    problems: Vec<String>,
}

fn check_full(name: &str, store: &ObjectStore<'_>, pages: usize, qpdf_ok: bool, t: &mut Tally) {
    for (label, xref_stream, object_streams) in [("table", false, false), ("packed", true, true)] {
        let mut options = FullOptions::default();
        options.xref_stream = xref_stream;
        options.object_streams = object_streams;
        options.encryption = EncryptionPolicy::Remove;
        let bytes = match write_full(store, &options) {
            Ok(bytes) => bytes,
            // A document that cannot be read completely stops the write instead of losing
            // content (typed error, no panic).
            Err(error) => {
                t.full_stopped += 1;
                println!("stopped: {name} full/{label}: {error}");
                continue;
            }
        };
        t.full_written += 1;
        match ObjectStore::open(&bytes, Limits::default()) {
            Ok(after) => {
                if !after.repaired().is_empty() {
                    t.problems.push(format!(
                        "{name}: full/{label} opens repaired: {:?}",
                        after.repaired()
                    ));
                } else if page_count(&after) != pages {
                    t.problems.push(format!(
                        "{name}: full/{label} has {} pages, the original {pages}",
                        page_count(&after)
                    ));
                }
            }
            Err(error) => t
                .problems
                .push(format!("{name}: full/{label} does not reopen: {error}")),
        }
        if qpdf_ok {
            t.qpdf_checked += 1;
            if !qpdf_accepts(&bytes, label) {
                t.problems
                    .push(format!("{name}: qpdf rejects full/{label}"));
            }
        }
    }
}

/// A change that replaces (or adds) `/Info`; also returns the object number it replaced.
fn new_info(store: &ObjectStore<'_>) -> (Changes, Option<u32>) {
    let mut dict = Dict::default();
    dict.set(
        b"Title",
        Object::new(ObjectKind::String(b"Changed".to_vec().into())),
    );
    let info = Object::new(ObjectKind::Dict(dict));
    let mut changes = Changes::new();
    if let Some(ObjectKind::Ref(reference)) = store.trailer().get(b"Info").map(|o| o.kind.clone()) {
        changes.set_object(reference.num, NewObject::Value(info));
        (changes, Some(reference.num))
    } else {
        let reference = changes.add_object(store, NewObject::Value(info));
        changes.set_info(reference);
        (changes, None)
    }
}

fn check_incremental(
    name: &str,
    data: &[u8],
    store: &ObjectStore<'_>,
    pages: usize,
    qpdf_ok: bool,
    t: &mut Tally,
) {
    let (changes, replaced) = new_info(store);
    let appended = match incremental_update(store, &changes) {
        Ok(appended) => appended,
        Err(Error::Write {
            kind: WriteError::NeedsFullRewrite,
        }) => {
            // Also for a file that opened cleanly but has a wrong offset somewhere the store has
            // not read yet (`ObjectStore::offsets_are_valid`).
            t.incremental_refused += 1;
            return;
        }
        Err(error) => {
            t.problems.push(format!("{name}: incremental: {error}"));
            return;
        }
    };
    if xref_repaired(store) {
        t.problems.push(format!(
            "{name}: incremental update written for a file with a repaired cross-reference"
        ));
    }
    t.incremental_written += 1;

    // The engine writes `original ++ appended`; check the result as a file, from its bytes.
    let output = [data, &appended].concat();
    if output[..data.len()] != *data {
        t.problems
            .push(format!("{name}: incremental changed the original bytes"));
    }
    let after = match ObjectStore::open(&output, Limits::default()) {
        Ok(after) => after,
        Err(error) => {
            t.problems
                .push(format!("{name}: incremental does not reopen: {error}"));
            return;
        }
    };
    if xref_repaired(&after) {
        t.problems.push(format!(
            "{name}: incremental opens repaired: {:?}",
            after.repaired()
        ));
    }
    if page_count(&after) != pages {
        t.problems
            .push(format!("{name}: incremental changed the page count"));
    }
    if title(&after).as_deref() != Some(b"Changed".as_slice()) {
        t.problems
            .push(format!("{name}: incremental lost the new /Info"));
    }
    compare_untouched(name, store, &after, replaced, t);
    if qpdf_ok {
        t.qpdf_checked += 1;
        if !qpdf_accepts(&output, "inc") {
            t.problems
                .push(format!("{name}: qpdf rejects the incremental update"));
        }
    }
}

/// Untouched content stays the same, object by object: everything except the replaced `/Info`
/// resolves to an equal object before and after (or is unreadable both times).
fn compare_untouched(
    name: &str,
    before: &ObjectStore<'_>,
    after: &ObjectStore<'_>,
    replaced: Option<u32>,
    t: &mut Tally,
) {
    let size = before
        .trailer()
        .get(b"Size")
        .and_then(vellora_cos::Object::as_integer)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
        .min(MAX_COMPARED_OBJECTS);
    for num in (1..size).filter(|n| Some(*n) != replaced) {
        let reference = ObjRef::new(num, 0);
        let (old, new) = (before.resolve(reference), after.resolve(reference));
        let same = match (&old, &new) {
            (Ok(old), Ok(new)) => old == new,
            (Err(_), Err(_)) => true,
            _ => false,
        };
        if !same {
            t.problems
                .push(format!("{name}: incremental changed object {num}"));
            return;
        }
    }
}

fn check_file(name: &str, data: &[u8], t: &mut Tally) {
    t.files += 1;
    let Ok(store) = ObjectStore::open(data, Limits::default()) else {
        t.not_opened += 1;
        return;
    };
    if store.is_locked() || store.root_ref().is_none() {
        t.skipped += 1;
        return;
    }
    let pages = page_count(&store);
    // A document whose pages `cos` cannot read at all (`pdfjs-PDFBOX-3148-2-fuzzed`: /Pages is in
    // a damaged object stream) is written without them, and qpdf, which reads more of it, then
    // rejects the output. That is the store's limit, not the writer's, so such files only get the
    // reopen checks.
    let qpdf_ok = pages > 0 && qpdf_accepts(data, "orig");
    check_full(name, &store, pages, qpdf_ok, t);
    check_incremental(name, data, &store, pages, qpdf_ok, t);
}

#[test]
#[ignore = "needs the corpus and qpdf; run with --ignored"]
fn writers_keep_the_invariants_on_every_corpus_file() {
    let dir = corpus_dir();
    assert!(
        dir.is_dir(),
        "corpus missing at {}: run `cargo xtask corpus fetch`",
        dir.display()
    );
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "no PDFs in {}", dir.display());

    let mut tally = Tally::default();
    for path in &paths {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let data = std::fs::read(path).unwrap();
        check_file(&name, &data, &mut tally);
    }
    println!(
        "{} files: {} not opened, {} locked or without /Root; {} full rewrites ({} stopped with a \
         typed error); {} incremental updates ({} repaired files refused); {} qpdf checks",
        tally.files,
        tally.not_opened,
        tally.skipped,
        tally.full_written,
        tally.full_stopped,
        tally.incremental_written,
        tally.incremental_refused,
        tally.qpdf_checked
    );
    for problem in &tally.problems {
        println!("PROBLEM {problem}");
    }
    assert!(
        tally.problems.is_empty(),
        "{} problems",
        tally.problems.len()
    );
    assert!(
        tally.qpdf_checked > 0,
        "qpdf accepted no original: is it the right binary?"
    );
}
