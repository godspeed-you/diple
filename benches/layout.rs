//! Layout hot spots: table width solving and syntax highlighting.

mod common;

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use diple::document::markdown::{parse, CodeBlock, NodeKind, Table};
use diple::layout::code::{highlight, layout_code, CodeCache, CodeOptions};
use diple::layout::table::{layout_table, TableOptions};
use diple::render::theme::Theme;

fn first_table(src: &str) -> Table {
    let doc = parse(src);
    for node in doc.walk() {
        if let NodeKind::Table(table) = &node.kind {
            return table.clone();
        }
    }
    panic!("fixture contains no table");
}

fn first_code_block(src: &str) -> CodeBlock {
    let doc = parse(src);
    for node in doc.walk() {
        if let NodeKind::CodeBlock(block) = &node.kind {
            return block.clone();
        }
    }
    panic!("fixture contains no code block");
}

fn bench_table(c: &mut Criterion) {
    let table = first_table(&common::fixture("wide-table.md"));
    let theme = Theme::dark();
    let opts = TableOptions::default();
    let mut group = c.benchmark_group("table");
    // 40 forces aggressive shrinking and wrapping, 120 fits comfortably.
    for width in [40usize, 80, 120] {
        group.bench_function(format!("wide-table@{width}"), |b| {
            b.iter(|| black_box(layout_table(&table, &theme, &opts, black_box(width), &[])));
        });
    }
    group.finish();
}

fn bench_highlight(c: &mut Criterion) {
    let block = first_code_block(&common::big_code_block(400));
    let theme = Theme::dark();
    // Warm up the syntax set and the lazily compiled regexes so the benchmark
    // measures highlighting throughput, not one-off initialisation.
    let _ = highlight("fn main() {}\n", Some("rust"), &theme, 4);

    c.bench_function("highlight/rust-2000-lines", |b| {
        b.iter(|| black_box(highlight(black_box(&block.code), Some("rust"), &theme, 4)));
    });

    let opts = CodeOptions {
        wrap: true,
        line_numbers: true,
        ..CodeOptions::default()
    };
    c.bench_function("layout_code/rust-2000-lines@80", |b| {
        // A fresh cache per iteration would measure highlighting again; a
        // shared cache measures the wrapping and gutter work only.
        let cache = CodeCache::new();
        b.iter(|| black_box(layout_code(0, &block, &theme, &opts, 80, &[], &cache)));
    });
}

criterion_group!(layout, bench_table, bench_highlight);
criterion_main!(layout);
