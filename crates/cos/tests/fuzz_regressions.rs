//! Inputs that crashed the fuzz targets (`fuzz/`), kept as regression tests. Each is documented in
//! `tests/fixtures/fuzz/README.md`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use vellora_cos::{EncryptionPolicy, FullOptions, Limits, ObjectStore, write_full};

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
