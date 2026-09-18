//! Large-document benchmarks.
//!
//! The document is synthesised from the checked-in fixtures, so it is
//! deterministic but still contains the constructs that cost real work:
//! tables, fenced code, nested lists, footnotes and CJK/emoji text.
//!
//! Sizes are kept at ~1 MB for the layout benchmarks and ~2 MB for parsing and
//! search so that the whole file still finishes in a couple of minutes in CI.
//!
//! Note that the synthetic document repeats the same headings thousands of
//! times, which is the worst case for anchor de-duplication; see the note in
//! `bench_parse_large`.

mod common;

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use diple::document::markdown::{self, parse};
use diple::document::{DocumentModel, FoldState};
use diple::layout::{Layout, LayoutOptions};
use diple::render::theme::Theme;

const MB: usize = 1024 * 1024;

/// Parsing a multi-megabyte document.
///
/// This is deliberately a pessimistic input: `common::large` repeats the same
/// fixtures, so thousands of headings share a slug and
/// `document::anchors::AnchorIndex::insert` probes `slug`, `slug-1`, `slug-2`,
/// … linearly for each of them — quadratic in the number of duplicates. A
/// regression here is therefore as likely to be an anchor-indexing change as a
/// parser change.
fn bench_parse_large(c: &mut Criterion) {
    let src = common::large(2 * MB);
    let mut group = c.benchmark_group("large");
    group.throughput(Throughput::Bytes(src.len() as u64));
    group.sample_size(10);
    group.bench_function("parse/2MB", |b| {
        b.iter(|| black_box(parse(black_box(&src))));
    });
    group.finish();
}

fn bench_layout_large(c: &mut Criterion) {
    let doc = DocumentModel::markdown(parse(&common::large(MB)));
    let theme = Theme::dark();
    let mut group = c.benchmark_group("large");
    group.sample_size(10);
    group.bench_function("layout/1MB@80", |b| {
        b.iter(|| black_box(Layout::build(&doc, &LayoutOptions::new(80, &theme))));
    });

    let layout = Layout::new();
    let _ = layout.layout(&doc, &LayoutOptions::new(80, &theme));
    group.bench_function("relayout/1MB@120", |b| {
        b.iter(|| black_box(layout.layout(&doc, &LayoutOptions::new(120, &theme))));
    });
    group.finish();
}

fn bench_search_large(c: &mut Criterion) {
    let doc = parse(&common::large(2 * MB));
    // The index Markdown parsing already built; rebuilding it is measured below.
    let index = &doc.search;
    let mut group = c.benchmark_group("large");
    group.sample_size(10);
    group.bench_function("search_index/2MB", |b| {
        b.iter(|| black_box(markdown::search::build(black_box(&doc))));
    });
    // "Chapter" hits once per repetition, "qwertzuiop" never: the hit and the
    // miss have very different costs and both matter for the search UI.
    group.bench_function("search_find/hit", |b| {
        b.iter(|| black_box(index.find(black_box("Chapter"), false)));
    });
    group.bench_function("search_find/miss", |b| {
        b.iter(|| black_box(index.find(black_box("qwertzuiop"), false)));
    });
    group.finish();
}

/// The three structural operations on a large document.
///
/// * `fold` collapses and expands one section. It splices the affected node
///   range into the cached tree instead of rebuilding the document, so the
///   cost is the index fix-up, not the layout.
/// * `search_paint` marks and unmarks the query over one viewport, which is
///   all an incremental search costs now that the query is not a layout input.
/// * `resize` is the honest full rebuild: widths really do change, so the
///   whole document is laid out again. It is the one operation on a
///   multi-megabyte document that cannot be done inside a frame.
fn bench_structural_large(c: &mut Criterion) {
    let model = DocumentModel::markdown(parse(&common::large(MB)));
    let doc = model.as_markdown().expect("markdown");
    let theme = Theme::dark();
    let engine = Layout::new();
    let folds = model.fold_state();
    let mut opts = LayoutOptions::new(80, &theme);
    opts.folds = Some(&folds);
    opts.lazy_code = true;

    let mut group = c.benchmark_group("large");
    group.sample_size(10);

    // The middle section of the document, so the splice has to move the
    // indices of half a million lines.
    let section = doc.sections.len() / 2;
    if let Some(s) = doc.sections.get(section) {
        let first = doc.nodes.partition_point(|n| n.id < s.heading);
        let last = doc.nodes.partition_point(|n| n.id < s.end);
        let mut collapsed = FoldState::from_parents(doc.fold_parents());
        collapsed.collapse(section);
        let mut folded = LayoutOptions::new(80, &theme);
        folded.folds = Some(&collapsed);
        folded.lazy_code = true;
        group.bench_function("fold/1MB", |b| {
            b.iter_batched(
                || engine.layout(&model, &opts),
                |mut tree| {
                    engine.relayout_nodes(doc, &folded, &mut tree, first, last - first);
                    engine.relayout_nodes(doc, &opts, &mut tree, first, last - first);
                    black_box(tree)
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }

    let mut tree = engine.layout(&model, &opts);
    group.bench_function("search_paint/1MB@80x40", |b| {
        b.iter(|| {
            tree.mark_search(0, 40, black_box("widget"), false);
            tree.clear_search(0, 40);
        });
    });

    group.bench_function("resize/1MB@80->120", |b| {
        b.iter(|| {
            let mut wide = LayoutOptions::new(120, &theme);
            wide.folds = Some(&folds);
            wide.lazy_code = true;
            black_box(engine.layout(&model, &wide))
        });
    });
    group.finish();
}

/// The structured formats at every size tier, one criterion group per format.
///
/// §19.1 wants JSON and YAML measured separately, and §19.4 wants the numbers
/// to be comparable *between* the tiers: every id below exists at every size,
/// so dividing a 1 MiB time by the 12 KiB time of the same id answers the
/// accidental-quadratic question directly (a linear phase lands near the byte
/// ratio, ~85x).
///
/// The phases are the ones of §19.3 — load, search-index construction, first
/// layout — plus the interactive operations of §19.2: navigation, folding,
/// search and outline construction.
fn bench_structured_sizes(c: &mut Criterion) {
    let theme = Theme::dark();
    for format in ["json", "yaml"] {
        let mut group = c.benchmark_group(format);
        group.sample_size(10);
        for (label, bytes) in common::structured_sizes() {
            let src = if format == "json" {
                common::json_export(bytes)
            } else {
                common::yaml_manifests(bytes)
            };
            let parse = |text: &str| {
                if format == "json" {
                    common::parse_json(text)
                } else {
                    common::parse_yaml(text)
                }
            };
            // The model owns the parsed document and the index is read
            // through it: a `clone` here would double the resident bytes of
            // the largest tier for no measurement.
            let model = DocumentModel::structured(parse(&src));
            let doc = model.as_structured().expect("structured");

            group.throughput(Throughput::Bytes(src.len() as u64));
            group.bench_function(format!("load/{label}"), |b| {
                b.iter(|| black_box(parse(black_box(&src))));
            });
            group.bench_function(format!("search_index/{label}"), |b| {
                b.iter(|| black_box(common::rebuild_search_index(black_box(doc))));
            });
            group.bench_function(format!("layout/{label}@80"), |b| {
                b.iter(|| black_box(Layout::build(&model, &LayoutOptions::new(80, &theme))));
            });
            group.bench_function(format!("outline/{label}"), |b| {
                b.iter(|| black_box(model.outline()));
            });

            // "service" hits every record, "qwertzuiop" nothing: the two ends
            // of what the search prompt does while a reader is typing.
            group.bench_function(format!("search_find/{label}-hit"), |b| {
                b.iter(|| black_box(model.search_index().find(black_box("service"), false)));
            });
            group.bench_function(format!("search_find/{label}-miss"), |b| {
                b.iter(|| black_box(model.search_index().find(black_box("qwertzuiop"), false)));
            });

            // Navigation walks structural nodes the way `]` does, and asks
            // for the path of each the way the status bar does. A fixed
            // number of steps at every size: the per-step cost is what a key
            // press costs, and it must not grow with the document.
            const STEPS: usize = 200;
            let start = model.first_semantic().unwrap_or(0);
            group.bench_function(format!("navigate/{label}-200-steps"), |b| {
                b.iter(|| {
                    let mut at = start;
                    for _ in 0..STEPS {
                        let Some(next) = model.next_structural(at) else {
                            break;
                        };
                        at = next;
                        black_box(model.path(at));
                    }
                    black_box(at)
                });
            });

            // Folding splices one container's rows in and out of a laid-out
            // tree; the cost must be the subtree's size, not the document's.
            let folds = model.fold_state();
            let engine = Layout::new();
            let opts = LayoutOptions::new(80, &theme).with_folds(&folds);
            let middle = folds.len() / 2;
            if !folds.is_empty() {
                let mut collapsed = model.fold_state();
                collapsed.collapse(middle);
                let folded = LayoutOptions::new(80, &theme).with_folds(&collapsed);
                group.bench_function(format!("fold/{label}"), |b| {
                    b.iter_batched(
                        || engine.layout(&model, &opts),
                        |mut tree| {
                            engine.relayout_fold(&model, &folded, &mut tree, middle);
                            engine.relayout_fold(&model, &opts, &mut tree, middle);
                            black_box(tree)
                        },
                        criterion::BatchSize::LargeInput,
                    );
                });

                // Collapsing everything is the `zM` a reader presses to see
                // the shape of an unfamiliar file: a whole-document relayout.
                let mut all = model.fold_state();
                all.collapse_all();
                let all_folded = LayoutOptions::new(80, &theme).with_folds(&all);
                group.bench_function(format!("collapse_all/{label}@80"), |b| {
                    b.iter(|| black_box(engine.layout(&model, &all_folded)));
                });
            }
        }
        group.finish();
    }
}

/// The large-scalar risk of §20.3, kept apart from the size tiers.
///
/// Sixteen 512 KiB strings are 8 MiB of document in 17 nodes. Parsing is
/// linear in the bytes; the search index is the question, because it keeps a
/// lower-cased copy *and* a `usize` per byte of every field, so it costs
/// roughly nine times the source in memory before it costs anything in time.
fn bench_big_scalars(c: &mut Criterion) {
    let src = common::json_big_scalars(16, 512 * 1024);
    let model = DocumentModel::structured(common::parse_json(&src));
    let doc = model.as_structured().expect("structured");
    let theme = Theme::dark();

    let mut group = c.benchmark_group("json");
    group.sample_size(10);
    group.throughput(Throughput::Bytes(src.len() as u64));
    group.bench_function("load/16x512KiB-scalars", |b| {
        b.iter(|| black_box(common::parse_json(black_box(&src))));
    });
    group.bench_function("search_index/16x512KiB-scalars", |b| {
        b.iter(|| black_box(common::rebuild_search_index(black_box(doc))));
    });
    group.bench_function("layout/16x512KiB-scalars@80", |b| {
        b.iter(|| black_box(Layout::build(&model, &LayoutOptions::new(80, &theme))));
    });
    group.finish();
}

/// Deep nesting, the other half of §20 — a document that is small in bytes
/// and extreme in depth.
///
/// Every traversal §20.1 lists has to stay iterative *and* cheap: laying the
/// document out, asking for the path of the deepest node (which walks the
/// whole ancestor chain), and revealing that node out of a fully collapsed
/// tree. A regression here is a depth problem, not a size problem, which is
/// why it is not folded into the byte-size tiers.
fn bench_deep_nesting(c: &mut Criterion) {
    const DEPTH: usize = 128;
    let src = common::json_deep(DEPTH);
    let model = DocumentModel::structured(common::parse_json(&src));
    let deepest = model.node_count().saturating_sub(1);
    let theme = Theme::dark();

    let mut group = c.benchmark_group("json");
    group.bench_function(format!("load/depth-{DEPTH}"), |b| {
        b.iter(|| black_box(common::parse_json(black_box(&src))));
    });
    group.bench_function(format!("layout/depth-{DEPTH}@80"), |b| {
        b.iter(|| black_box(Layout::build(&model, &LayoutOptions::new(80, &theme))));
    });
    group.bench_function(format!("path/depth-{DEPTH}"), |b| {
        b.iter(|| black_box(model.path(black_box(deepest))));
    });
    group.bench_function(format!("reveal/depth-{DEPTH}"), |b| {
        b.iter_batched(
            || {
                let mut folds = model.fold_state();
                folds.collapse_all();
                folds
            },
            |mut folds| {
                model.reveal(deepest, &mut folds);
                black_box(folds)
            },
            criterion::BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    large,
    bench_parse_large,
    bench_layout_large,
    bench_structural_large,
    bench_search_large,
    bench_structured_sizes,
    bench_big_scalars,
    bench_deep_nesting
);
criterion_main!(large);
