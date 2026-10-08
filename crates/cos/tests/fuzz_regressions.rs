//! Inputs that crashed the fuzz targets (`fuzz/`), kept as regression tests. Each is documented in
//! `tests/fixtures/fuzz/README.md`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use vellora_cos::{
    EncryptionPolicy, Error, FullOptions, LimitKind, Limits, ObjRef, ObjectStore, write_full,
};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/fuzz")
            .join(name),
    )
    .unwrap()
}

fn pages(store: &ObjectStore<'_>) -> usize {
    store.pages().flatten().count()
}

/// Every offset after byte 7192 is wrong, which the store only finds out when it reads an object
/// there; it then rebuilds the cross-reference, and the rebuilt trailer names another catalog
/// than the one it had read. The first page walk used to end up with no pages at all (so did a
/// rewrite started from the stale catalog), the second one found the page.
#[test]
fn a_repair_during_the_first_read_does_not_lose_the_pages() {
    let data = fixture("linearized-shifted-offsets.pdf");
    let store = ObjectStore::open(&data, Limits::default()).unwrap();
    assert_eq!(pages(&store), 1, "first walk");
    assert_eq!(pages(&store), 1, "second walk");

    // The same file, but the first thing read is the rewrite.
    let store = ObjectStore::open(&data, Limits::default()).unwrap();
    for (xref_stream, object_streams) in [(false, false), (true, true)] {
        let mut options = FullOptions::default();
        options.xref_stream = xref_stream;
        options.object_streams = object_streams;
        options.encryption = EncryptionPolicy::Remove;
        let bytes = write_full(&store, &options).unwrap();
        let after = ObjectStore::open(&bytes, Limits::default()).unwrap();
        assert_eq!(after.repaired(), []);
        assert_eq!(pages(&after), 1);
    }
}

/// The table says object 4 is at an offset where `2 0 obj` (a Font) now stands, so reading object 4
/// rebuilds the cross-reference, and the rebuilt table resolves 2 to the later, second definition
/// instead of the page tree. Before, a rewrite depended on what had been read before it: the
/// first one (the table variant) was made from the page tree, the second from the Font.
#[test]
fn a_rewrite_does_not_depend_on_what_was_read_before_it() {
    let data = fixture("duplicate-object-after-bad-offset.pdf");
    let mut outputs = Vec::new();
    let store = ObjectStore::open(&data, Limits::default()).unwrap();
    for _ in 0..2 {
        // Same options twice: the second call must not see a different document.
        outputs.push(write_full(&store, &FullOptions::default()).unwrap());
    }
    assert_eq!(outputs[0], outputs[1]);

    // And it is the document a fresh store sees after the repair.
    let fresh = ObjectStore::open(&data, Limits::default()).unwrap();
    let _ = fresh.resolve(vellora_cos::ObjRef::new(4, 0));
    let after = ObjectStore::open(&outputs[0], Limits::default()).unwrap();
    assert_eq!(pages(&after), pages(&fresh));
}

/// A well-formed file whose cross-reference is right but whose `count` stream objects all
/// declare a `/Length` that is too long and have no `endstream` at all, so reading each one
/// searches to the end of the file. Generated here, not stored: it is 1.5 MB of filler.
fn streams_with_wrong_length(count: u32) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    let mut object = |out: &mut Vec<u8>, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    };
    object(
        &mut out,
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    );
    object(
        &mut out,
        "2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n",
    );
    for n in 3..3 + count {
        let filler = "x".repeat(100);
        object(
            &mut out,
            &format!("{n} 0 obj\n<< /Length 99999999 >>\nstream\n{filler}\nendobj\n"),
        );
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Without a shared budget every one of the 10,000 reads scans the rest of the file (about 7 GB
/// in all). With it, the first reads fail as `MissingEndstream`, the budget runs out, and the
/// rest fail as `LimitExceeded(ScanBytes)` at once; the document stays usable throughout.
#[test]
fn thousands_of_broken_stream_lengths_do_not_cost_thousands_of_scans() {
    const COUNT: u32 = 10_000;
    let data = streams_with_wrong_length(COUNT);
    let started = std::time::Instant::now();
    let store = ObjectStore::open(&data, Limits::default()).unwrap();
    let (mut missing, mut over_budget) = (0, 0);
    for n in 3..3 + COUNT {
        match store.resolve(ObjRef::new(n, 0)) {
            Err(Error::Syntax { .. }) => missing += 1,
            Err(Error::LimitExceeded {
                limit: LimitKind::ScanBytes,
                ..
            }) => over_budget += 1,
            other => panic!("object {n}: unexpected {other:?}"),
        }
    }
    let elapsed = started.elapsed();
    assert!(missing > 0, "the budget should allow some searches");
    assert!(over_budget > 0, "the budget should run out");
    assert_eq!(missing + over_budget, COUNT);
    // The failure is per object: the rest of the document is still readable.
    assert!(store.resolve(ObjRef::new(1, 0)).is_ok());
    assert_eq!(pages(&store), 0);
    // Linear work takes well under a second; the CI limit is generous.
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "took {elapsed:?}"
    );
}

/// The budget is not a hidden cap on honest files: with it raised past the cost, the same
/// file is searched every time and no read reports the budget.
#[test]
fn a_larger_scan_budget_lets_every_search_run() {
    const COUNT: u32 = 300;
    let data = streams_with_wrong_length(COUNT);
    let mut limits = Limits::default();
    limits.min_scan_bytes = u64::MAX;
    let store = ObjectStore::open(&data, limits).unwrap();
    for n in 3..3 + COUNT {
        assert!(
            matches!(store.resolve(ObjRef::new(n, 0)), Err(Error::Syntax { .. })),
            "object {n}"
        );
    }
}
