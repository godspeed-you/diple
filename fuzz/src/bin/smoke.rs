//! Corpus + mutation smoke runner for the fuzz targets.
//!
//! `cargo fuzz` needs a nightly toolchain and libFuzzer instrumentation, which
//! is not always available (musl builders, pinned-stable CI images). This
//! binary runs the very same target bodies on a stable toolchain: every seed
//! in `fuzz/corpus/<target>/` plus deterministic random mutations of it. It is
//! a smoke test, not a replacement for coverage-guided fuzzing — but it is
//! what makes the "must never panic on arbitrary document input" rule
//! checkable in every pipeline.
//!
//! Usage:
//!
//! ```text
//! cargo run --release --bin smoke -- [seconds-per-target] [target ...]
//! ```
//!
//! A panic in any target aborts the process with a non-zero exit status, which
//! is exactly the failure signal CI needs.

use std::time::{Duration, Instant};

type Target = (&'static str, fn(&[u8]));

const TARGETS: [Target; 12] = [
    ("parse_markdown", diple_fuzz::parse_markdown),
    ("layout", diple_fuzz::layout),
    ("table", diple_fuzz::table),
    ("unicode", diple_fuzz::unicode_helpers),
    ("config", diple_fuzz::config),
    ("mermaid", diple_fuzz::mermaid),
    ("format_detect", diple_fuzz::format_detect),
    ("json_model", diple_fuzz::json_model),
    ("yaml_model", diple_fuzz::yaml_model),
    ("structured_layout", diple_fuzz::structured_layout),
    ("structured_path", diple_fuzz::structured_path),
    ("fold_reveal", diple_fuzz::fold_reveal),
];

/// xorshift64* — a deterministic PRNG so a failing run is reproducible from
/// the printed seed alone.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

/// Apply a few random byte-level edits to a seed.
fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut out = seed.to_vec();
    let edits = 1 + rng.below(8);
    for _ in 0..edits {
        match rng.below(5) {
            0 if !out.is_empty() => {
                let i = rng.below(out.len());
                out[i] = rng.below(256) as u8;
            }
            1 if !out.is_empty() => {
                let i = rng.below(out.len());
                out.remove(i);
            }
            2 => {
                let i = rng.below(out.len() + 1);
                out.insert(i, rng.below(256) as u8);
            }
            3 if out.len() > 2 => {
                // Duplicate a slice: grows structure (nested lists, table rows).
                let a = rng.below(out.len());
                let b = a + rng.below(out.len() - a);
                let chunk = out[a..=b].to_vec();
                let at = rng.below(out.len());
                out.splice(at..at, chunk);
            }
            _ => {
                let i = rng.below(out.len() + 1);
                for byte in b"|`#*>-\n\t[]() " {
                    out.insert(i, *byte);
                }
            }
        }
        // Keep inputs small enough that one iteration stays fast.
        out.truncate(64 * 1024);
    }
    out
}

fn corpus(target: &str) -> Vec<Vec<u8>> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .join(target);
    let mut seeds: Vec<Vec<u8>> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if let Ok(bytes) = std::fs::read(&path) {
                seeds.push(bytes);
            }
        }
    }
    if seeds.is_empty() {
        seeds.push(Vec::new());
    }
    seeds
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds: u64 = args
        .first()
        .and_then(|a| a.parse().ok())
        .unwrap_or(30);
    let wanted: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();

    for (name, run) in TARGETS {
        if !wanted.is_empty() && !wanted.contains(&name) {
            continue;
        }
        let seeds = corpus(name);
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut execs: u64 = 0;

        // Every seed verbatim first, then mutations until the deadline.
        for seed in &seeds {
            run(seed);
            execs += 1;
        }
        while Instant::now() < deadline {
            for _ in 0..256 {
                let seed = &seeds[rng.below(seeds.len())];
                let input = mutate(seed, &mut rng);
                run(&input);
                execs += 1;
            }
        }
        println!("{name}: {execs} executions in {seconds}s, no panic");
    }
}
