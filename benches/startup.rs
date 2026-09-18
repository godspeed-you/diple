//! Startup-path benchmarks.
//!
//! These three measurements together account for everything diple does
//! between `main` and the first frame for a typical README:
//!
//! * `parse/readme` — Markdown → semantic [`Document`].
//! * `parse_layout/readme@80` — parse *and* lay out at width 80, i.e. the cold
//!   path including the first syntax-highlighting of every fenced block.
//! * `relayout/readme@{80,120}` — the warm path taken after a resize, where
//!   the highlighting cache is already populated (resize).
//!
//! * `first_frame/readme@80x24` — what the interactive path actually does
//!   before it can draw: parse, lay out with deferred highlighting, then
//!   highlight only the code blocks inside the first screen.
//!
//! One caveat, and it is the important one: syntect compiles a syntax
//! definition's regexes lazily, once per *process*, and that single compile is
//! what dominates a cold start — 44 ms for the `bash` syntax on the reference
//! machine, against ~1 ms for loading the syntax and theme dumps and ~0.6 ms
//! for laying a README out. Criterion's warm-up absorbs it entirely, so every
//! number in this file — `first_frame` included — is a steady-state number and
//! **not** a time to first frame. The startup budget must be measured out of
//! process, one `diple --debug` run per sample, on a pty; these benchmarks
//! only guard against the layout work itself regressing.

mod common;

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use diple::document::markdown::parse;
use diple::document::{json, DocumentModel, FormatRequest, SourceDocument};
use diple::layout::{Layout, LayoutOptions};
use diple::render::theme::Theme;

fn bench_parse(c: &mut Criterion) {
    let src = common::readme();
    c.bench_function("parse/readme", |b| {
        b.iter(|| black_box(parse(black_box(&src))));
    });
}

fn bench_parse_and_layout(c: &mut Criterion) {
    let src = common::readme();
    let theme = Theme::dark();
    c.bench_function("parse_layout/readme@80", |b| {
        b.iter(|| {
            let doc = DocumentModel::markdown(parse(black_box(&src)));
            let opts = LayoutOptions::new(80, &theme);
            black_box(Layout::build(&doc, &opts))
        });
    });
}

fn bench_relayout(c: &mut Criterion) {
    let doc = DocumentModel::markdown(parse(&common::readme()));
    let theme = Theme::dark();
    // One warm-up layout populates the highlighting cache; the measured
    // iterations then only re-wrap, which is what a resize costs.
    let layout = Layout::new();
    let _ = layout.layout(&doc, &LayoutOptions::new(80, &theme));

    let mut group = c.benchmark_group("relayout");
    for width in [80usize, 120] {
        group.bench_function(format!("readme@{width}"), |b| {
            b.iter(|| {
                let opts = LayoutOptions::new(width, &theme);
                black_box(layout.layout(&doc, &opts))
            });
        });
    }
    group.finish();
}

/// The cold path of the interactive pager: parse, lay out without
/// highlighting anything, then realize just the first screen.
///
/// This is the work the `p50 < 30 ms` budget is spent on, minus the one-off
/// per-language regex compile that criterion cannot see (see the module docs).
fn bench_first_frame(c: &mut Criterion) {
    let src = common::readme();
    let theme = Theme::dark();
    const SCREEN: usize = 24;
    c.bench_function("first_frame/readme@80x24", |b| {
        b.iter(|| {
            let doc = DocumentModel::markdown(parse(black_box(&src)));
            let mut opts = LayoutOptions::new(80, &theme);
            opts.lazy_code = true;
            // A fresh engine per iteration: an empty highlighting cache is
            // what a new process starts with.
            let engine = Layout::new();
            let mut tree = engine.layout(&doc, &opts);
            engine.realize(&doc, &opts, &mut tree, 0, SCREEN);
            black_box(tree)
        });
    });
}

/// The same startup path for the structured formats, on the size a reader
/// actually opens: §19.1 asks for each format to be measured on its own, and
/// for opening a normal file to still feel immediate.
///
/// The four phases of §19.3 are separated as far as the backends allow:
///
/// * `parse/yaml-12KiB` is the parser alone — granit's event stream, driven
///   the way `document::yaml` drives it but without building anything. JSON
///   has no such split: its parser *is* the model builder, one pass, so
///   `model/json-12KiB` (`json::is_valid`, which parses and builds and then
///   throws the result away) is the earliest point that can be measured.
/// * `model/json-12KiB` — parse and build the node tree, no search index.
/// * `load/{json,yaml}-12KiB` — what opening the file really costs: parse,
///   model and search index, the whole of `json::parse` / `yaml::parse`.
/// * `search_index/{json,yaml}-12KiB` — index construction on its own,
///   rebuilt through the public builder (see `common::rebuild_search_index`).
/// * `first_frame/{json,yaml}-12KiB@80x24` — parse and lay out the document,
///   which for structured data is the whole first frame: there is no deferred
///   syntax highlighting to realize.
fn bench_structured_startup(c: &mut Criterion) {
    let theme = Theme::dark();
    let json_src = common::json_export(common::SMALL);
    let yaml_src = common::yaml_manifests(common::SMALL);

    c.bench_function("parse/yaml-12KiB", |b| {
        b.iter(|| {
            let parser = granit_parser::Parser::new_from_str(black_box(&yaml_src));
            let mut events = 0usize;
            for event in parser {
                if event.is_err() {
                    break;
                }
                events += 1;
            }
            black_box(events)
        });
    });

    c.bench_function("model/json-12KiB", |b| {
        b.iter(|| black_box(json::is_valid(black_box(&json_src))));
    });

    c.bench_function("load/json-12KiB", |b| {
        b.iter(|| black_box(common::parse_json(black_box(&json_src))));
    });
    c.bench_function("load/yaml-12KiB", |b| {
        b.iter(|| black_box(common::parse_yaml(black_box(&yaml_src))));
    });

    let json_doc = common::parse_json(&json_src);
    let yaml_doc = common::parse_yaml(&yaml_src);
    c.bench_function("search_index/json-12KiB", |b| {
        b.iter(|| black_box(common::rebuild_search_index(black_box(&json_doc))));
    });
    c.bench_function("search_index/yaml-12KiB", |b| {
        b.iter(|| black_box(common::rebuild_search_index(black_box(&yaml_doc))));
    });

    for (format, src) in [("json", &json_src), ("yaml", &yaml_src)] {
        c.bench_function(&format!("first_frame/{format}-12KiB@80x24"), |b| {
            b.iter(|| {
                let doc = if format == "json" {
                    common::parse_json(black_box(src))
                } else {
                    common::parse_yaml(black_box(src))
                };
                let model = DocumentModel::structured(doc);
                black_box(Layout::build(&model, &LayoutOptions::new(80, &theme)))
            });
        });
    }

    // Opening a file without a usable extension probes the format first, so
    // a slow probe is a slow open.
    c.bench_function("open/json-12KiB", |b| {
        b.iter(|| {
            let loaded = diple::document::load(
                FormatRequest::Auto,
                SourceDocument::new("bench.json", black_box(&json_src)),
            );
            // A load that fails is a load that is fast: measure only the real
            // path.
            black_box(loaded.unwrap_or_else(|e| panic!("{}", e.report())))
        });
    });
}

criterion_group!(
    startup,
    bench_parse,
    bench_parse_and_layout,
    bench_first_frame,
    bench_relayout,
    bench_structured_startup
);
criterion_main!(startup);
