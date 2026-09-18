//! Shared fixture helpers for the diple benchmarks.
//!
//! Everything here is deterministic: the synthetic documents are generated
//! from the checked-in fixtures with a fixed recipe, so two runs on the same
//! commit benchmark exactly the same input (benchmarks run in CI and
//! meaningful regressions should flag the build).

#![allow(dead_code)]

use std::path::PathBuf;

use diple::document::search::{MatchField, SearchIndex, SearchIndexBuilder};
use diple::document::structured::StructuredDocument;
use diple::document::{json, yaml, DocumentModel, SourceDocument};

/// Absolute path of a checked-in fixture.
pub fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Read a checked-in fixture.
pub fn fixture(name: &str) -> String {
    match std::fs::read_to_string(fixture_path(name)) {
        Ok(text) => text,
        Err(error) => panic!("cannot read fixture {name}: {error}"),
    }
}

/// The README-sized document used for the startup benchmarks.
pub fn readme() -> String {
    fixture("readme.md")
}

/// A "typical" documentation page: every fixture once, which exercises
/// headings, lists, tables, code blocks, footnotes and Mermaid in one pass.
pub fn mixed() -> String {
    const FIXTURES: [&str; 8] = [
        "readme.md",
        "code-blocks.md",
        "nested-lists.md",
        "wide-table.md",
        "narrow-table.md",
        "mixed-formatting.md",
        "unicode-cjk-emoji.md",
        "footnotes.md",
    ];
    let mut out = String::new();
    for name in FIXTURES {
        out.push_str(&fixture(name));
        out.push_str("\n\n");
    }
    out
}

/// A synthetic document of at least `target_bytes` bytes.
///
/// Built by repeating [`mixed`] with a per-repetition heading so that headings
/// stay unique (anchor de-duplication is part of what we measure) and the
/// section tree keeps growing.
pub fn large(target_bytes: usize) -> String {
    let unit = mixed();
    let mut out = String::with_capacity(target_bytes + unit.len());
    let mut chapter = 0usize;
    while out.len() < target_bytes {
        chapter += 1;
        out.push_str(&format!("\n\n# Chapter {chapter}\n\n"));
        out.push_str(&unit);
    }
    out
}

/// A large single code block (Rust), used for the highlighting benchmark.
pub fn big_code_block(lines: usize) -> String {
    let mut code = String::with_capacity(lines * 48);
    for i in 0..lines {
        code.push_str(&format!(
            "pub fn item_{i}(value: &str, count: usize) -> Option<String> {{\n    \
             let mut out = String::from(\"item {i}\"); // note\n    \
             if count > {i} {{ out.push_str(value); }}\n    Some(out)\n}}\n"
        ));
    }
    format!("```rust\n{code}```\n")
}

// ---------------------------------------------------------------------------
// Structured documents (JSON and YAML)
// ---------------------------------------------------------------------------
//
// The spec asks for three representative sizes per format. Only the small one
// is a plausible repository fixture; a 1 MiB nested document and a 10–50 MiB
// export are generated here instead, because a benchmark input that has to be
// cloned with the repository is an input nobody will keep up to date.
//
// The recipes are deterministic — the "random" variation is arithmetic on the
// record index — so two runs on the same commit measure the same bytes, and
// they contain what the respective format actually costs work for: nesting,
// long strings, escapes, non-ASCII text, and for YAML comments, anchors,
// aliases, tags and block scalars.

/// A small document: the 5–20 KiB tier (an API response, a service config).
pub const SMALL: usize = 12 * 1024;

/// A medium document: the 1 MiB tier.
pub const MEDIUM: usize = 1024 * 1024;

/// The large tier (10–50 MiB), in bytes, when it is switched on.
///
/// Off by default: a 16 MiB document takes criterion several minutes per
/// phase, which is not what `cargo bench` should cost on a laptop or in CI.
/// Set `DIPLE_BENCH_LARGE_MIB=16` (or any size in the 10–50 range) to include
/// it; that is the run the accidental-quadratic check of §19.4 needs.
pub fn large_bytes() -> Option<usize> {
    let raw = std::env::var("DIPLE_BENCH_LARGE_MIB").ok()?;
    let mib: usize = raw
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("DIPLE_BENCH_LARGE_MIB must be a number, got {raw:?}"));
    (mib > 0).then(|| mib * 1024 * 1024)
}

/// The size tiers to benchmark, as `(label, bytes)` pairs.
///
/// Labels are stable benchmark-id components, so a run today is comparable
/// with a run after the next refactor.
pub fn structured_sizes() -> Vec<(String, usize)> {
    let mut sizes = vec![("12KiB".to_string(), SMALL), ("1MiB".to_string(), MEDIUM)];
    if let Some(bytes) = large_bytes() {
        sizes.push((format!("{}MiB", bytes / (1024 * 1024)), bytes));
    }
    sizes
}

/// A JSON export of at least `target_bytes` bytes: a metadata header and an
/// array of records, each a small nested object.
///
/// Shaped like the thing a reader actually opens — an API response or a
/// `kubectl get -o json` dump — rather than like a parser stress test: a few
/// levels of nesting, arrays of scalars, strings that need escaping, numbers,
/// booleans and nulls.
pub fn json_export(target_bytes: usize) -> String {
    let mut out = String::with_capacity(target_bytes + 1024);
    out.push_str(
        "{\n  \"apiVersion\": \"v1\",\n  \"kind\": \"ExportList\",\n  \
         \"metadata\": {\n    \"generated\": \"2026-01-01T00:00:00Z\",\n    \
         \"source\": \"diple benchmark\"\n  },\n  \"items\": [\n",
    );
    let mut index = 0usize;
    while out.len() < target_bytes {
        if index > 0 {
            out.push_str(",\n");
        }
        let replicas = index % 7;
        let ready = index % 3 == 0;
        out.push_str(&format!(
            "    {{\n      \"id\": {index},\n      \"name\": \"service-{index:06}\",\n      \
             \"namespace\": \"team-{}\",\n      \"labels\": {{\n        \
             \"app\": \"service-{index:06}\",\n        \"tier\": \"{}\",\n        \
             \"owner\": \"gruppe-\\u00fcbergreifend\"\n      }},\n      \
             \"spec\": {{\n        \"replicas\": {replicas},\n        \
             \"image\": \"registry.example.com/service:{index}.0.{}\",\n        \
             \"ports\": [{}, {}],\n        \"env\": [\n          \
             {{\"name\": \"LOG_LEVEL\", \"value\": \"info\"}},\n          \
             {{\"name\": \"NOTE\", \"value\": \"line one\\nline \\\"two\\\"\"}}\n        ]\n      }},\n      \
             \"status\": {{\n        \"ready\": {ready},\n        \
             \"lastError\": null,\n        \"uptimeSeconds\": {}\n      }}\n    }}",
            index % 12,
            if index % 2 == 0 { "backend" } else { "frontend" },
            index % 5,
            8000 + (index % 100),
            9000 + (index % 100),
            index * 37 % 100_000,
        ));
        index += 1;
    }
    out.push_str("\n  ]\n}\n");
    out
}

/// A YAML stream of at least `target_bytes` bytes: Kubernetes-like manifests,
/// one document each.
///
/// Carries everything a YAML reader has to keep and a deserializer throws
/// away: comments above and beside entries, an anchor with an alias and a
/// merge key, an explicit tag, a literal block scalar and a folded one.
pub fn yaml_manifests(target_bytes: usize) -> String {
    let mut out = String::with_capacity(target_bytes + 1024);
    out.push_str("%YAML 1.2\n");
    let mut index = 0usize;
    while out.len() < target_bytes {
        let replicas = 1 + index % 5;
        out.push_str(&format!(
            "---\n# Deployment {index}, generated for the benchmark corpus.\n\
             apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  \
             name: service-{index:06}\n  namespace: team-{}\n  labels: &labels-{index}\n    \
             app: service-{index:06}\n    tier: {}          # the tier decides the node pool\n    \
             owner: gruppe-übergreifend\nspec:\n  replicas: {replicas}\n  selector:\n    \
             matchLabels:\n      <<: *labels-{index}\n  template:\n    metadata:\n      \
             labels:\n        <<: *labels-{index}\n    spec:\n      containers:\n        \
             - name: service\n          image: registry.example.com/service:{index}.0.{}\n          \
             ports:\n            - containerPort: {}\n              protocol: TCP\n          \
             env:\n            # Kept verbose on purpose: comments are content.\n            \
             - name: LOG_LEVEL\n              value: info\n            - name: STARTED_AT\n              \
             value: !!timestamp 2026-01-01T00:00:00Z\n          readinessProbe:\n            \
             exec:\n              command:\n                - /bin/sh\n                - -c\n                \
             - |\n                  set -eu\n                  curl -sf http://localhost:{}/healthz\n                  \
             echo \"service-{index:06} ready\"\n          description: >\n            \
             Service {index} is part of the generated benchmark corpus and exists\n            \
             only so that the parser has a folded block scalar to fold.\n",
            index % 12,
            if index % 2 == 0 { "backend" } else { "frontend" },
            index % 5,
            8000 + (index % 100),
            8000 + (index % 100),
        ));
        index += 1;
    }
    out
}

/// A JSON document of a few nodes whose scalars are megabytes long.
///
/// §20.3 calls large scalars out as a risk of their own: a document that is
/// big without being deep costs nothing to nest and everything to normalise,
/// so the search index — which keeps a lower-cased copy and a byte map of
/// every field — is measured against it separately from the node-count tiers.
pub fn json_big_scalars(count: usize, scalar_bytes: usize) -> String {
    let unit = "Lorem ipsum dolor sit amet, consetetur sadipscing elitr. ";
    let mut blob = String::with_capacity(scalar_bytes + unit.len());
    while blob.len() < scalar_bytes {
        blob.push_str(unit);
    }
    let mut out = String::with_capacity(count * blob.len() + 256);
    out.push_str("{\n");
    for i in 0..count {
        if i > 0 {
            out.push_str(",\n");
        }
        out.push_str(&format!("  \"blob-{i}\": \"{blob}\""));
    }
    out.push_str("\n}\n");
    out
}

/// A JSON document `depth` levels deep, for the nesting-cost question.
pub fn json_deep(depth: usize) -> String {
    let mut out = String::with_capacity(depth * 16);
    for i in 0..depth {
        out.push_str(&format!("{{\"level{i}\": "));
    }
    out.push_str("\"leaf\"");
    out.push_str(&"}".repeat(depth));
    out.push('\n');
    out
}

/// Parse a generated JSON document, panicking with the parse error when the
/// recipe above produced something invalid.
pub fn parse_json(text: &str) -> StructuredDocument {
    match json::parse(&SourceDocument::new("bench.json", text)) {
        Ok(doc) => doc,
        Err(error) => panic!("generated JSON is invalid:\n{}", error.report()),
    }
}

/// Parse a generated YAML stream, panicking with the parse error when the
/// recipe above produced something invalid.
pub fn parse_yaml(text: &str) -> StructuredDocument {
    match yaml::parse(&SourceDocument::new("bench.yaml", text)) {
        Ok(doc) => doc,
        Err(error) => panic!("generated YAML is invalid:\n{}", error.report()),
    }
}

/// A parsed generated document, wrapped in the model the application sees.
pub fn structured_model(doc: StructuredDocument) -> DocumentModel {
    DocumentModel::structured(doc)
}

/// Rebuild a structured document's search index through the public builder.
///
/// The index the parser builds is not reachable a second time — it is built
/// inside `Builder::finish` — so index construction is measured by pushing the
/// same fields, in the same order, through [`SearchIndexBuilder`]. That is the
/// work §19.3 asks to be measured on its own and the normalised-copy-per-node
/// cost §19.4 warns about; it is not a byte-identical rebuild of the parser's
/// index (a trailing comment is indexed here after the value rather than with
/// the comments above it), and it does not need to be for a cost measurement.
pub fn rebuild_search_index(doc: &StructuredDocument) -> SearchIndex {
    let mut builder = SearchIndexBuilder::with_capacity(doc.node_count() * 2);
    for node in doc.nodes() {
        for id in node.comments_above() {
            if let Some(comment) = doc.comment(*id) {
                builder.push(node.id, MatchField::Comment, comment.text.as_str());
            }
        }
        if let Some(key) = node.key() {
            builder.push(node.id, MatchField::Label, key.text.as_str());
        }
        if let Some(anchor) = node.anchor() {
            builder.push(node.id, MatchField::Anchor, anchor);
        }
        if let Some(tag) = node.tag() {
            builder.push(node.id, MatchField::Tag, tag);
        }
        if let Some(scalar) = node.scalar() {
            builder.push(node.id, MatchField::Value, scalar.display());
        }
        if let Some(id) = node.comment_right() {
            if let Some(comment) = doc.comment(id) {
                builder.push(node.id, MatchField::Comment, comment.text.as_str());
            }
        }
    }
    builder.build()
}
