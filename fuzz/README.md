# Fuzzing diple

The rule: *the application must never panic on arbitrary document input* —
and, since 2.0 reads untrusted JSON and YAML, must not hang, recurse until the
stack overflows, or emit terminal escapes the document chose (spec §23.4).

The structured targets therefore assert rather than discard: a model that
parses must have a dense, acyclic, containment-respecting node arena; a parse
that fails must say where; a rendered row must name a real node and carry no
control character.

This is a separate crate with its own workspace, so `cargo build`, `cargo test`
and `cargo clippy` in the repository root never see it.

## Targets

| Target | Entry point |
|---|---|
| `parse_markdown` | `document::parser::parse` on arbitrary UTF-8 |
| `layout` | `parse` + `Layout::build` at a fuzzed width, theme and wrap mode |
| `table` | `layout::table::layout_table` with fuzzed cells, alignments and width |
| `unicode` | `layout::unicode` width / split / pad / wrap / tab helpers |
| `config` | `config::loader::load_file` (TOML parsing and validation) |
| `mermaid` | `mermaid::parser::parse` + the native terminal renderer |
| `format_detect` | `document::load` — detection must be deterministic and must honour an explicit `--format` |
| `json_model` | `document::json::parse` — the strict JSON parser and the arena it builds |
| `yaml_model` | `document::yaml::parse` — anchors, aliases, merge keys, tags, multi-document streams |
| `structured_layout` | `layout::structured` — the AC-18 target: no row may carry a terminal control |
| `structured_path` | `DocumentPath::breadcrumb` / `breadcrumb_within` / `canonical` and their width contract |
| `fold_reveal` | `FoldState` operations and `StructuredDocument::reveal` |

The bodies live in `src/lib.rs`; `fuzz_targets/*.rs` are thin libFuzzer shims.
Seed corpora are in `corpus/<target>/`, derived from `tests/fixtures/`.

## Coverage-guided run (needs nightly)

```bash
cargo install cargo-fuzz
rustup toolchain install nightly
cargo +nightly fuzz run parse_markdown -- -max_total_time=60
```

`+nightly` is required explicitly because the repository pins stable in
`rust-toolchain.toml`.

To keep the checked-in seed corpus clean, write new inputs elsewhere:

```bash
cargo +nightly fuzz run layout /tmp/diple-fuzz/layout fuzz/corpus/layout -- -max_total_time=60
```

## Stable-toolchain smoke run (CI)

`cargo fuzz` needs nightly and libFuzzer instrumentation, which is not
available on every builder. The `smoke` binary runs the same target bodies
over the seed corpora plus deterministic mutations, on stable:

```bash
cargo run --release --bin smoke -- 30            # 30 s per target
cargo run --release --bin smoke -- 10 table      # one target only
```

A panic aborts the process with a non-zero exit status.
