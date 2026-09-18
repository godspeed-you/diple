#![no_main]
//! Fuzz target `yaml_model` — see `fuzz/src/lib.rs` for the body.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    diple_fuzz::yaml_model(data);
});
