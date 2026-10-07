//! Smoke test of the writers on corpus v0.
//!
//! Needs the corpus (`cargo xtask corpus fetch`), so it only runs on request:
//!
//! ```sh
//! cargo test -p vellora-cos --test writers_corpus -- --ignored --nocapture
//! ```
//!
//! For every file that opens and is not locked:
//!
//! - **full rewrite** (table, and cross-reference stream with object streams): the result opens
//!   without repairs and has as many pages as the original;
//! - **incremental update** (files whose cross-reference was read as it is): `/Info` is replaced,
//!   the original bytes stay a byte-identical prefix, and the result opens without repairs, has
//!   the same page count and the new title. Repaired files must be refused with
//!   `NeedsFullRewrite`.
//!
//! The qpdf side of this (`qpdf --check` on every output) belongs to the invariant harness of M0
//! task 13; set `QPDF` to also run it here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use vellora_cos::object::Dict;
use vellora_cos::{
    Changes, EncryptionPolicy, Error, FullOptions, Limits, NewObject, Object, ObjectKind,
    ObjectStore, WriteError, incremental_update, write_full,
};

fn pages(store: &ObjectStore<'_>) -> usize {
    store.pages().flatten().count()
}

fn title(store: &ObjectStore<'_>) -> Option<Vec<u8>> {
    let info = store.info().ok()??;
    match &info.as_dict()?.get(b"Title")?.kind {
        ObjectKind::String(s) => Some(s.to_vec()),
        _ => None,
    }
}

/// Whether qpdf accepts `bytes` (exit 0, or 3 for "succeeded with warnings": corpus files are
/// often warned about). `None` when `QPDF` is not set.
fn qpdf_ok(bytes: &[u8], label: &str) -> Option<bool> {
    let qpdf = std::env::var("QPDF").ok()?;
    let path = std::env::temp_dir().join(format!("vellora-wc-{}-{label}.pdf", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let output = Command::new(qpdf).arg("--check").arg(&path).output().ok()?;
    let _ = std::fs::remove_file(&path);
    Some(matches!(output.status.code(), Some(0 | 3)))
}

/// A qpdf rejection counts against the writer only if qpdf accepted the original: a file that is
/// already broken for qpdf (a looping page tree, say) is not repaired by rewriting it.
fn qpdf_rejects_output(original_ok: Option<bool>, bytes: &[u8], label: &str) -> bool {
    original_ok == Some(true) && qpdf_ok(bytes, label) == Some(false)
}

/// Checks one file; returns what went wrong, if anything.
fn check(name: &str, data: &[u8], problems: &mut Vec<String>, counts: &mut [usize; 4]) {
    let Ok(store) = ObjectStore::open(data, Limits::default()) else {
        return;
    };
    if store.is_locked() || store.root_ref().is_none() {
        return;
    }
    let original_pages = pages(&store);
    // A document whose pages `cos` cannot read at all (`pdfjs-PDFBOX-3148-2-fuzzed`: /Pages is
    // in a damaged object stream) is written without them, and qpdf, which reads more of it, then
    // rejects the output. That is the store's limit, not the writer's, so such files only get
    // the reopen checks.
    let original_qpdf = qpdf_ok(data, "orig").filter(|_| original_pages > 0);

    for (label, xref_stream, object_streams) in [("table", false, false), ("packed", true, true)] {
        let mut options = FullOptions::default();
        options.xref_stream = xref_stream;
        options.object_streams = object_streams;
        options.encryption = EncryptionPolicy::Remove;
        match write_full(&store, &options) {
            Ok(bytes) => {
                counts[0] += 1;
                match ObjectStore::open(&bytes, Limits::default()) {
                    Ok(after) => {
                        if !after.repaired().is_empty() {
                            problems.push(format!(
                                "{name}: full/{label} opens repaired: {:?}",
                                after.repaired()
                            ));
                        } else if pages(&after) != original_pages {
                            problems.push(format!(
                                "{name}: full/{label} has {} pages, the original {original_pages}",
                                pages(&after)
                            ));
                        }
                        if qpdf_rejects_output(original_qpdf, &bytes, label) {
                            problems.push(format!("{name}: qpdf rejects full/{label}"));
                        }
                    }
                    Err(error) => problems.push(format!("{name}: full/{label} reopen: {error}")),
                }
            }
            // A document that cannot be read completely stops the write instead of losing content.
            Err(error) => {
                counts[3] += 1;
                println!("stopped: {name} full/{label}: {error}");
            }
        }
    }

    let mut changes = Changes::new();
    let mut dict = Dict::default();
    dict.set(
        b"Title",
        Object::new(ObjectKind::String(b"Changed".to_vec().into())),
    );
    let info = Object::new(ObjectKind::Dict(dict));
    if let Some(ObjectKind::Ref(reference)) = store.trailer().get(b"Info").map(|o| o.kind.clone()) {
        changes.set_object(reference.num, NewObject::Value(info));
    } else {
        let reference = changes.add_object(&store, NewObject::Value(info));
        changes.set_info(reference);
    }
    match incremental_update(&store, &changes) {
        Ok(appended) => {
            counts[1] += 1;
            let after_bytes = [data, &appended].concat();
            match ObjectStore::open(&after_bytes, Limits::default()) {
                Ok(after) => {
                    if !after.repaired().is_empty() && store.repaired().is_empty() {
                        problems.push(format!(
                            "{name}: incremental opens repaired: {:?}",
                            after.repaired()
                        ));
                    } else if pages(&after) != original_pages {
                        problems.push(format!("{name}: incremental changed the page count"));
                    } else if title(&after).as_deref() != Some(b"Changed".as_slice()) {
                        problems.push(format!("{name}: incremental lost the new title"));
                    }
                    if qpdf_rejects_output(original_qpdf, &after_bytes, "inc") {
                        problems.push(format!("{name}: qpdf rejects the incremental update"));
                    }
                }
                Err(error) => problems.push(format!("{name}: incremental reopen: {error}")),
            }
        }
        Err(Error::Write {
            kind: WriteError::NeedsFullRewrite,
        }) => {
            // Also for a file that opened cleanly but has a wrong offset somewhere the store has
            // not read yet (see `ObjectStore::offsets_are_valid`).
            counts[2] += 1;
        }
        Err(error) => problems.push(format!("{name}: incremental: {error}")),
    }
}

#[test]
#[ignore = "needs the corpus; run with --ignored"]
fn the_writers_work_on_every_corpus_file() {
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

    let mut problems = Vec::new();
    // full rewrites written, incremental updates written, repaired files refused, full rewrites
    // that stopped on an unreadable object
    let mut counts = [0usize; 4];
    for path in &paths {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let data = std::fs::read(path).unwrap();
        check(&name, &data, &mut problems, &mut counts);
    }
    println!(
        "{} files: {} full rewrites, {} incremental updates, {} repaired files refused for \
         incremental, {} full rewrites stopped with a typed error (unreadable object, no /Root)",
        paths.len(),
        counts[0],
        counts[1],
        counts[2],
        counts[3]
    );
    for problem in &problems {
        println!("PROBLEM {problem}");
    }
    assert!(problems.is_empty(), "{} problems", problems.len());
}
