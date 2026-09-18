#![no_main]
//! Fuzz target `fold_reveal` — see `fuzz/src/lib.rs` for the body.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    diple_fuzz::fold_reveal(data);
});
