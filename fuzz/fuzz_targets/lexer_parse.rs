#![no_main]

//! Fuzz the spec-condition lexer (UTF-8 only). Requires `cargo-fuzz` (`cargo install cargo-fuzz`).
//! From this directory: `cargo +nightly fuzz run lexer_parse`

use blvm_spec_lock_core::parser::lexer::Lexer;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let mut lexer = Lexer::new(s);
    let _ = lexer.lex();
});
