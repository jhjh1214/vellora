//! Smoke test of the stream filters on corpus v0.
//!
//! Needs the corpus (`cargo xtask corpus fetch`), so it only runs on request:
//!
//! ```sh
//! cargo test -p vellora-cos --test filters_corpus -- --ignored --nocapture
//! ```
//!
//! Every stream of every unencrypted file goes through `decode_stream`. Nothing may panic;
//! failures must be typed errors. Encrypted files are skipped until decryption exists (M0 task
//! 11), and so are streams with an indirect `/Filter` or `/DecodeParms` (the caller resolves
//! those). Prints how often each filter was decoded, left incomplete or failed.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vellora_cos::filter::decode_stream;
use vellora_cos::{DecodeBudget, Limits, ObjRef, ObjectKind, ObjectStore};

#[derive(Default)]
struct Tally {
    decoded: usize,
    incomplete: usize,
    failed: usize,
    first_failure: Option<String>,
}

#[test]
#[ignore = "needs the corpus; run with --ignored"]
fn every_corpus_stream_decodes_or_fails_with_a_typed_error() {
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

    let limits = Limits::default();
    let mut by_filter: BTreeMap<String, Tally> = BTreeMap::new();
    for path in &paths {
        let data = std::fs::read(path).unwrap();
        let Ok(store) = ObjectStore::open(&data, limits.clone()) else {
            continue;
        };
        if store.encrypt().ok().flatten().is_some() {
            continue;
        }
        let size = match store.trailer().get(b"Size").map(|o| &o.kind) {
            Some(ObjectKind::Integer(n)) => u32::try_from(*n).unwrap_or(0).min(200_000),
            _ => 0,
        };
        let mut budget = DecodeBudget::new(&limits);
        for number in 1..size {
            let Ok(object) = store.resolve(ObjRef::new(number, 0)) else {
                continue;
            };
            let ObjectKind::Stream(stream) = &object.kind else {
                continue;
            };
            let indirect = |key: &[u8]| {
                matches!(
                    stream.dict.get(key).map(|o| &o.kind),
                    Some(ObjectKind::Ref(_))
                )
            };
            if indirect(b"Filter") || indirect(b"DecodeParms") {
                continue;
            }
            let label = match stream.dict.get(b"Filter").map(|o| &o.kind) {
                Some(ObjectKind::Name(n)) => String::from_utf8_lossy(n).into_owned(),
                Some(ObjectKind::Array(items)) => items
                    .iter()
                    .map(|i| match &i.kind {
                        ObjectKind::Name(n) => String::from_utf8_lossy(n).into_owned(),
                        _ => "?".to_owned(),
                    })
                    .collect::<Vec<_>>()
                    .join("+"),
                _ => "(none)".to_owned(),
            };
            let raw = store.stream_raw(stream).unwrap();
            let tally = by_filter.entry(label).or_default();
            match decode_stream(
                &stream.dict,
                raw,
                &limits,
                Some(&mut budget),
                Some(stream.data.start as u64),
            ) {
                Ok(d) if d.complete => tally.decoded += 1,
                Ok(_) => tally.incomplete += 1,
                Err(error) => {
                    tally.failed += 1;
                    tally.first_failure.get_or_insert_with(|| {
                        format!("{}: object {number}: {error}", path.display())
                    });
                }
            }
        }
    }

    for (label, tally) in &by_filter {
        println!(
            "{label:40} decoded {:6}  incomplete {:4}  failed {:4}",
            tally.decoded, tally.incomplete, tally.failed
        );
        if let Some(failure) = &tally.first_failure {
            println!("    first failure: {failure}");
        }
    }
    assert!(!by_filter.is_empty(), "no streams were found");
}
