//! Open → write → reopen: whatever the store can read, the writers must be able to write back
//! so that it reads again with the same pages.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vellora_cos::object::Dict;
use vellora_cos::{
    Changes, EncryptionPolicy, FullOptions, Limits, NewObject, Object, ObjectKind, ObjectStore,
    incremental_update, write_full,
};

fn pages(store: &ObjectStore<'_>) -> usize {
    store.pages().flatten().count()
}

fuzz_target!(|data: &[u8]| {
    let Ok(store) = ObjectStore::open(data, Limits::default()) else {
        return;
    };
    if store.is_locked() || store.root_ref().is_none() {
        return;
    }
    // A store repairs a wrong offset when it first reads it, which can change what it reports
    // (a rebuilt table may resolve a number to another object). A rewrite settles that first, so
    // take the reference from the store after one.
    let _ = write_full(&store, &FullOptions::default());
    let original_pages = pages(&store);

    for (xref_stream, object_streams) in [(false, false), (true, true)] {
        let mut options = FullOptions::default();
        options.xref_stream = xref_stream;
        options.object_streams = object_streams;
        options.encryption = EncryptionPolicy::Remove;
        // A typed error (an unreadable object stops a rewrite) is fine; a panic is not.
        let Ok(bytes) = write_full(&store, &options) else {
            continue;
        };
        let after =
            ObjectStore::open(&bytes, Limits::default()).expect("a full rewrite must reopen");
        assert!(
            after.repaired().is_empty(),
            "a full rewrite must not need repair"
        );
        assert_eq!(pages(&after), original_pages, "a full rewrite lost pages");
    }

    let mut dict = Dict::default();
    dict.set(
        b"Title",
        Object::new(ObjectKind::String(b"Fuzz".to_vec().into())),
    );
    let mut changes = Changes::new();
    let info = changes.add_object(
        &store,
        NewObject::Value(Object::new(ObjectKind::Dict(dict))),
    );
    changes.set_info(info);
    if let Ok(appended) = incremental_update(&store, &changes) {
        let output = [data, &appended].concat();
        let after = ObjectStore::open(&output, Limits::default())
            .expect("an incremental update must reopen");
        // A reference inside the page tree to an object the file never defines (`/Kids [4 0 R]`
        // with no object 4) means something else once an update defines that number. The writer
        // keeps new numbers above `/Size` and above everything the trailer names, but it cannot
        // know every dangling reference without reading every object, which a lazy store does
        // not. So the pages are compared unless the file mentions the number the new object took.
        if !mentions_reference(data, info.num) {
            assert_eq!(
                pages(&after),
                original_pages,
                "an incremental update lost pages"
            );
        }
    }
});

/// Whether `data` holds `<number> <generation> R`, a reference to object `number`.
fn mentions_reference(data: &[u8], number: u32) -> bool {
    let wanted = number.to_string().into_bytes();
    let mut from = 0;
    while let Some(found) = find(&data[from..], &wanted) {
        let at = from + found;
        from = at + 1;
        // Not the tail of a longer number.
        if at > 0 && data[at - 1].is_ascii_digit() {
            continue;
        }
        let mut rest = &data[at + wanted.len()..];
        // One space, the generation (digits), one space, `R`.
        let Some(after_space) = rest.strip_prefix(b" ") else {
            continue;
        };
        rest = after_space;
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        if digits > 0 && rest[digits..].starts_with(b" R") {
            return true;
        }
    }
    false
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
