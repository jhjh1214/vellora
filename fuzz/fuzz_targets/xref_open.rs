//! Opens arbitrary bytes as a whole document and walks what the store offers.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vellora_cos::{Limits, ObjRef, ObjectStore};

/// Pages and objects read per input; the limits bound each step, this bounds the run time.
const WALK: usize = 64;

fuzz_target!(|data: &[u8]| {
    let Ok(store) = ObjectStore::open(data, Limits::default()) else {
        return;
    };
    let _ = store.root();
    let _ = store.info();
    let _ = store.encrypt();
    for page in store.pages().take(WALK).flatten() {
        let _ = store.resolve(page.reference);
    }
    for num in 1..=WALK as u32 {
        let _ = store.resolve(ObjRef::new(num, 0));
    }
    let _ = store.repaired();
});
