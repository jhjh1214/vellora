//! Open → write → reopen: whatever the store can read, the writers must be able to write back
//! so that it reads again with the same pages.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vellora_cos::{
    Changes, EncryptionPolicy, FullOptions, Limits, NewObject, Object, ObjectKind, ObjectStore,
    incremental_update, write_full,
};
use vellora_cos::object::Dict;

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
        let after = ObjectStore::open(&bytes, Limits::default())
            .expect("a full rewrite must reopen");
        assert!(after.repaired().is_empty(), "a full rewrite must not need repair");
        assert_eq!(pages(&after), original_pages, "a full rewrite lost pages");
    }

    let mut dict = Dict::default();
    dict.set(b"Title", Object::new(ObjectKind::String(b"Fuzz".to_vec().into())));
    let mut changes = Changes::new();
    let info = changes.add_object(&store, NewObject::Value(Object::new(ObjectKind::Dict(dict))));
    changes.set_info(info);
    if let Ok(appended) = incremental_update(&store, &changes) {
        let output = [data, &appended].concat();
        let after = ObjectStore::open(&output, Limits::default())
            .expect("an incremental update must reopen");
        assert_eq!(pages(&after), original_pages, "an incremental update lost pages");
    }
});
