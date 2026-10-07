//! Tokenises arbitrary bytes: must neither panic nor loop.
#![no_main]

use libfuzzer_sys::fuzz_target;
use vellora_cos::Limits;
use vellora_cos::lexer::Lexer;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    let mut lexer = Lexer::new(data, &limits);
    // Every token consumes at least one byte, so this bound is only reached by a lexer that
    // stops advancing: that is a bug, reported as a crash.
    for _ in 0..=data.len() {
        match lexer.next_token() {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return,
        }
    }
    assert!(lexer.is_eof(), "lexer produced more tokens than bytes");
});
