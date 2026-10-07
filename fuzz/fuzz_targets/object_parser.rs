//! Parses arbitrary bytes as a direct object and as an indirect object.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vellora_cos::{Limits, Parser, write_object};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    if let Ok(object) = Parser::new(data, &limits).parse_object() {
        // A parsed value must serialise (streams are the one documented refusal).
        let mut out = Vec::new();
        let _ = write_object(&mut out, &object, &limits);
    }
    let _ = Parser::new(data, &limits).parse_indirect_object();
});
