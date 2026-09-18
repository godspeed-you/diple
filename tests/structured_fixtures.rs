//! The structured (JSON/YAML) fixture corpus, end to end.
//!
//! Two kinds of test live here, for the same reason they do in
//! `render_snapshots.rs`:
//!
//! * the **corpus sweep** holds for *every* structured fixture — it parses,
//!   it has nodes, an outline, a search index and rows — so a fixture added
//!   to `tests/fixtures/` is covered the moment it is dropped in;
//! * the **per-fixture** tests pin the one thing each fixture exists to pin:
//!   duplicate keys stay duplicated, a stream keeps its three documents, a
//!   `%YAML` directive survives, an alias stays an alias.
//!
//! The error-path fixtures are held to the same standard: an invalid document
//! must come back as a [`DocumentError`] that says *where*, because "it did
//! not panic" is not a contract a reader can use.

mod common;

use std::path::Path;

use diple::document::structured::{
    Directive, ScalarKind, ScalarStyle, StructuredDocument, StructuredNodeKind,
};
use diple::document::{
    load, DocumentKind, DocumentModel, FormatRequest, MatchField, SourceDocument,
};
use diple::layout::{Layout, LayoutOptions};
use diple::render::theme::Theme;

/// Fixtures that are deliberately unparseable; the error path is what they
/// are for.
const INVALID: [&str; 2] = ["invalid.json", "invalid.yaml"];

/// Fixtures that parse but genuinely carry nothing. An empty YAML stream is a
/// valid document with no nodes, and every question asked of it must have an
/// answer rather than a panic.
const EMPTY: [&str; 1] = ["empty.yaml"];

const WIDTHS: [usize; 3] = [40, 80, 120];

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .expect("fixture file name")
        .to_string()
}

/// Load a fixture the way the pager does: by name, so the extension picks the
/// format and a parse failure is an error rather than a fallback to Markdown.
fn open(name: &str) -> Result<DocumentModel, diple::document::DocumentError> {
    let path = common::fixture(name);
    let text = std::fs::read_to_string(&path).expect("read fixture");
    load(FormatRequest::Auto, SourceDocument::new(name, &text)).map(|d| d.model)
}

/// A structured fixture that must parse.
fn structured(name: &str) -> StructuredDocument {
    let model = open(name).unwrap_or_else(|e| panic!("{name} must parse:\n{}", e.report()));
    model
        .as_structured()
        .unwrap_or_else(|| panic!("{name} is not structured"))
        .clone()
}

/// Every structured fixture, paired with its format.
fn structured_fixtures() -> Vec<(String, DocumentKind)> {
    let all: Vec<(String, DocumentKind)> = common::fixtures_by_format()
        .into_iter()
        .filter(|(_, kind)| kind.is_structured())
        .map(|(path, kind)| (file_name(&path), kind))
        .collect();
    assert!(
        all.len() >= 16,
        "the structured corpus shrank to {} fixtures",
        all.len()
    );
    all
}

/// The key of the first mapping entry in the document, if it has one.
fn first_key(doc: &StructuredDocument) -> Option<String> {
    doc.nodes()
        .iter()
        .find_map(|n| n.key().map(|k| k.text.clone()))
}

// ---------------------------------------------------------------------------
// the corpus sweep
// ---------------------------------------------------------------------------

/// Every valid structured fixture parses into a coherent model: dense
/// pre-order ids, a resolvable parent chain, in-bounds spans.
#[test]
fn every_structured_fixture_parses_coherently() {
    let mut checked = 0usize;
    for (name, kind) in structured_fixtures() {
        if INVALID.contains(&name.as_str()) {
            continue;
        }
        let doc = structured(&name);
        assert_eq!(doc.kind(), kind, "{name}: format");
        let source = doc.source().text();

        if EMPTY.contains(&name.as_str()) {
            assert_eq!(doc.node_count(), 0, "{name}: an empty file has no nodes");
            assert!(doc.is_empty(), "{name}");
            checked += 1;
            continue;
        }
        assert!(doc.node_count() > 0, "{name}: no nodes");

        for (id, node) in doc.nodes().iter().enumerate() {
            assert_eq!(node.id, id, "{name}: ids are not dense pre-order");
            assert!(node.end > id, "{name}: node {id} ends before it starts");
            assert!(node.end <= doc.node_count(), "{name}: subtree past the end");
            if let Some(parent) = node.parent {
                assert!(parent < id, "{name}: node {id} precedes its parent");
                assert!(
                    doc.node(parent).is_some_and(|p| p.end >= node.end),
                    "{name}: node {id} is not inside its parent's subtree"
                );
            } else {
                assert!(
                    doc.roots().iter().any(|r| r.node == id),
                    "{name}: parentless node {id} is not a root"
                );
            }
            assert!(node.span.start <= node.span.end, "{name}: bad span");
            assert!(node.span.end <= source.len(), "{name}: span past EOF");
            assert!(
                source.is_char_boundary(node.span.start) && source.is_char_boundary(node.span.end),
                "{name}: span not on a char boundary"
            );
        }
        assert!(!doc.roots().is_empty(), "{name}: no root");
        checked += 1;
    }
    assert!(checked >= 14, "only {checked} valid fixtures swept");
}

/// Every valid, non-empty structured fixture has an outline in document
/// order, and its first entry is at depth 0.
#[test]
fn every_structured_fixture_has_an_outline() {
    for (name, _) in structured_fixtures() {
        if INVALID.contains(&name.as_str()) || EMPTY.contains(&name.as_str()) {
            continue;
        }
        let model = open(&name).expect("parses");
        let outline = model.outline();
        assert!(model.has_outline(), "{name}: no outline");
        assert_eq!(
            outline.len(),
            model.node_count(),
            "{name}: one outline entry per node"
        );
        assert_eq!(outline[0].depth, 0, "{name}: first entry is a root");
        for (i, entry) in outline.iter().enumerate() {
            assert_eq!(entry.node, i, "{name}: outline is not in document order");
            assert!(!entry.text.is_empty(), "{name}: empty outline label");
            assert!(
                entry.text.lines().count() <= 1,
                "{name}: outline label {:?} spans lines",
                entry.text
            );
            if let Some(fold) = entry.fold {
                assert!(
                    fold < model.fold_parents().len(),
                    "{name}: fold out of range"
                );
            }
        }
        assert_eq!(model.outline(), outline, "{name}: deriving it twice agrees");
    }
}

/// The search index is built by the parser and finds what the document says:
/// searching a fixture's own first key hits that key's row, in its label.
#[test]
fn every_structured_fixture_is_searchable() {
    let mut checked = 0usize;
    for (name, _) in structured_fixtures() {
        if INVALID.contains(&name.as_str()) || EMPTY.contains(&name.as_str()) {
            continue;
        }
        let doc = structured(&name);
        let Some(key) = first_key(&doc) else {
            // `array-of-objects.json` and the scalar roots have no key; a
            // miss must still be a clean empty result.
            assert!(
                doc.search_index().find("qwertzuiop", false).is_empty(),
                "{name}: a miss found something"
            );
            continue;
        };
        let hits = doc.search_index().find(&key, false);
        assert!(!hits.is_empty(), "{name}: the key {key:?} is not findable");
        assert!(
            hits.iter().any(|m| m.field == MatchField::Label),
            "{name}: {key:?} was not found in a label"
        );
        assert!(
            hits.windows(2).all(|w| w[0].node <= w[1].node),
            "{name}: results are not in document order"
        );
        for m in &hits {
            assert!(m.node < doc.node_count(), "{name}: hit on a missing node");
            assert!(m.start < m.end, "{name}: empty match range");
        }
        assert!(
            doc.search_index().find("qwertzuiop", false).is_empty(),
            "{name}: a miss found something"
        );
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} fixtures searched");
}

/// Every valid fixture lays out at every width, every row belongs to a real
/// node, and the rows are deterministic.
#[test]
fn every_structured_fixture_renders() {
    let theme = Theme::dark();
    for (name, _) in structured_fixtures() {
        if INVALID.contains(&name.as_str()) {
            continue;
        }
        let model = open(&name).expect("parses");
        for width in WIDTHS {
            let opts = LayoutOptions::new(width, &theme);
            let tree = Layout::build(&model, &opts);
            for line in &tree.lines {
                assert!(
                    line.node < model.node_count().max(1),
                    "{name}@{width}: row on node {} of {}",
                    line.node,
                    model.node_count()
                );
            }
            let text = tree.to_plain_text();
            if EMPTY.contains(&name.as_str()) {
                assert!(tree.lines.is_empty(), "{name}: an empty file has no rows");
                continue;
            }
            assert!(!tree.lines.is_empty(), "{name}@{width}: nothing rendered");
            assert!(!text.trim().is_empty(), "{name}@{width}: blank output");
            assert_eq!(
                Layout::build(&model, &LayoutOptions::new(width, &theme)).to_plain_text(),
                text,
                "{name}@{width}: layout is not deterministic"
            );
        }
    }
}

/// Folding a structured document hides content and revealing brings it back —
/// for every fixture that has more than one foldable container.
#[test]
fn every_structured_fixture_folds() {
    let mut checked = 0usize;
    for (name, _) in structured_fixtures() {
        if INVALID.contains(&name.as_str()) || EMPTY.contains(&name.as_str()) {
            continue;
        }
        let model = open(&name).expect("parses");
        let mut folds = model.fold_state();
        assert_eq!(
            folds.len(),
            model.fold_parents().len(),
            "{name}: fold state does not match the forest"
        );
        for node in 0..model.node_count() {
            assert!(!model.is_hidden(node, &folds), "{name}: hidden by default");
        }
        // A document root is the document, not a foldable unit: exactly the
        // non-root containers fold.
        let doc = model.as_structured().expect("structured");
        assert_eq!(
            folds.len(),
            doc.nodes()
                .iter()
                .filter(|n| n.is_container() && n.parent.is_some())
                .count(),
            "{name}: fold count does not match the non-root containers"
        );
        if folds.is_empty() {
            continue;
        }
        folds.collapse_all();
        let Some(hidden) = (0..model.node_count()).find(|n| model.is_hidden(*n, &folds)) else {
            continue;
        };
        model.reveal(hidden, &mut folds);
        assert!(
            !model.is_hidden(hidden, &folds),
            "{name}: reveal did nothing"
        );
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} fixtures folded");
}

/// A broken document comes back as an error that names the format and the
/// place, and whose report quotes the offending line.
#[test]
fn invalid_fixtures_report_where_they_broke() {
    for name in INVALID {
        let error = match open(name) {
            Ok(_) => panic!("{name} parsed; it is supposed to be invalid"),
            Err(error) => error,
        };
        assert_eq!(error.source_name, name);
        assert_eq!(
            error.format,
            diple::document::format::from_extension(name).expect("a known suffix"),
            "{name}: reported as the wrong format"
        );
        let position = error
            .position
            .unwrap_or_else(|| panic!("{name}: no position; a reader cannot act on that"));
        let text = std::fs::read_to_string(common::fixture(name)).expect("read fixture");
        let lines = text.lines().count();
        assert!(
            (1..=lines + 1).contains(&position.line),
            "{name}: line {} is outside the file's {lines} lines",
            position.line
        );
        assert!(position.column >= 1, "{name}: columns are 1-based");
        assert!(!error.message.is_empty(), "{name}: no message");

        let report = error.report();
        assert!(report.contains(name), "{name}: the report does not name it");
        assert!(
            report.contains(&format!("{}", position.line)),
            "{name}: the report does not give the line"
        );
        assert!(report.contains('^'), "{name}: the report has no caret");
    }
}

// ---------------------------------------------------------------------------
// what each fixture exists to pin
// ---------------------------------------------------------------------------

/// A reader's model keeps what the source says: three `name` entries stay
/// three entries, in source order, rather than collapsing to the last one.
#[test]
fn duplicate_keys_are_kept() {
    let doc = structured("duplicate-keys.json");
    let root = doc.node(0).expect("root");
    assert!(matches!(root.kind, StructuredNodeKind::Mapping));
    let names: Vec<&str> = doc
        .children(0)
        .into_iter()
        .filter_map(|id| doc.node(id))
        .filter(|n| n.key().is_some_and(|k| k.text == "name"))
        .filter_map(|n| n.scalar().map(|s| s.text.as_str()))
        .collect();
    assert_eq!(
        names,
        ["first", "second", "third"],
        "source order, all three"
    );

    let ports: Vec<&str> = doc
        .nodes()
        .iter()
        .filter(|n| n.key().is_some_and(|k| k.text == "port"))
        .filter_map(|n| n.scalar().map(|s| s.text.as_str()))
        .collect();
    assert_eq!(ports, ["8080", "9090"], "nested duplicates too");
}

/// Nesting is reachable by path, and a `null` stays a null rather than
/// becoming the string "null".
#[test]
fn nested_json_is_addressable_by_path() {
    let model = open("nested.json").expect("parses");
    let doc = model.as_structured().expect("structured");
    let backup = doc
        .nodes()
        .iter()
        .find(|n| n.key().is_some_and(|k| k.text == "backup"))
        .expect("repository > mirrors > backup");
    assert_eq!(
        model
            .path(backup.id)
            .expect("JSON has a path")
            .breadcrumb(false),
        "repository > mirrors > backup"
    );
    assert_eq!(backup.scalar().map(|s| s.kind), Some(ScalarKind::Null));

    let lto = doc
        .nodes()
        .iter()
        .find(|n| n.key().is_some_and(|k| k.text == "lto"))
        .expect("lto");
    assert_eq!(lto.scalar().map(|s| s.kind), Some(ScalarKind::Boolean));
    assert_eq!(
        model.path(lto.id).expect("path").breadcrumb(false),
        "build > profile > release > lto"
    );
}

/// An array root is a sequence whose items are addressed by index.
#[test]
fn a_json_array_root_is_a_sequence_of_objects() {
    let model = open("array-of-objects.json").expect("parses");
    let doc = model.as_structured().expect("structured");
    let root = doc.node(0).expect("root");
    assert!(matches!(root.kind, StructuredNodeKind::Sequence));
    assert_eq!(root.child_count, 3);
    for (index, id) in doc.children(0).into_iter().enumerate() {
        let node = doc.node(id).expect("item");
        assert_eq!(node.index(), Some(index));
        assert!(matches!(node.kind, StructuredNodeKind::Mapping));
    }
    let second = doc.children(0)[1];
    let name = doc
        .children(second)
        .into_iter()
        .filter_map(|id| doc.node(id))
        .find(|n| n.key().is_some_and(|k| k.text == "name"))
        .expect("name");
    assert_eq!(name.scalar().map(|s| s.text.as_str()), Some("Grace Hopper"));
    assert_eq!(
        model.path(name.id).expect("path").breadcrumb(false),
        "[1] > name"
    );
}

/// JSON escapes are decoded once, at parse time: what the reader sees and
/// searches is the text, not the backslashes.
#[test]
fn json_string_escapes_are_decoded() {
    let doc = structured("unicode-escapes.json");
    let value = |key: &str| -> String {
        doc.nodes()
            .iter()
            .find(|n| n.key().is_some_and(|k| k.text == key))
            .and_then(|n| n.scalar())
            .unwrap_or_else(|| panic!("{key} is missing"))
            .text
            .clone()
    };
    assert_eq!(value("plain"), "Grüße aus München");
    assert_eq!(value("cjk"), "日本語のテキスト");
    assert_eq!(value("escaped"), "line one\nline two\ttabbed");
    assert_eq!(value("quotes"), "she said \"hello\" and left");
    assert_eq!(value("backslash"), r"C:\Users\reader\notes.md");
    assert_eq!(value("solidus"), "a/b");
    assert_eq!(value("bmp-escape"), "äöü");
    assert_eq!(
        value("surrogate-pair"),
        "\u{1f600}",
        "a surrogate pair becomes one char"
    );
    assert!(doc
        .nodes()
        .iter()
        .any(|n| n.key().is_some_and(|k| k.text == "键")));
    // Decoded text is what search sees.
    assert!(!doc.search_index().find("münchen", false).is_empty());
    assert!(!doc.search_index().find("😀", false).is_empty());
}

/// A bare scalar is a one-node document that still answers every question.
#[test]
fn root_scalars_and_empty_containers_are_whole_documents() {
    let scalar = structured("scalar-root.json");
    assert_eq!(scalar.node_count(), 1);
    let root = scalar.node(0).expect("root");
    assert_eq!(
        root.scalar().map(|s| (s.kind, s.text.as_str())),
        Some((ScalarKind::String, "a single JSON string at the root"))
    );
    assert!(scalar.fold_parents().is_empty(), "a scalar cannot fold");

    let empty = structured("empty-object.json");
    assert_eq!(empty.node_count(), 1);
    let root = empty.node(0).expect("root");
    assert!(matches!(root.kind, StructuredNodeKind::Mapping));
    assert_eq!(root.child_count, 0);
    assert!(
        empty.fold_parents().is_empty(),
        "a document root is not a foldable unit"
    );

    let yaml_scalar = structured("scalar.yaml");
    assert_eq!(yaml_scalar.node_count(), 1);
    assert_eq!(
        yaml_scalar
            .node(0)
            .and_then(|n| n.scalar())
            .map(|s| s.text.as_str()),
        Some("just a scalar")
    );

    let empty_file = structured("empty.yaml");
    assert_eq!(empty_file.node_count(), 0);
    assert!(empty_file.is_empty());
    let model = DocumentModel::structured(empty_file);
    assert!(model.outline().is_empty());
    assert!(!model.has_outline());
    assert_eq!(model.first_semantic(), None);
    assert!(model.fold_state().is_empty());
    assert!(
        Layout::build(&model, &LayoutOptions::new(80, &Theme::dark()))
            .lines
            .is_empty()
    );
}

/// Deep nesting is walked iteratively, so 24 levels cost 24 path segments and
/// nothing else.
#[test]
fn deep_nesting_is_flat_in_the_model() {
    let model = open("deeply-nested.json").expect("parses");
    let doc = model.as_structured().expect("structured");
    // One root mapping plus one entry per level; the last entry is the scalar.
    assert_eq!(doc.node_count(), 25);
    let deepest = doc.node_count() - 1;
    let bottom = doc.node(deepest).expect("bottom");
    assert_eq!(bottom.scalar().map(|s| s.text.as_str()), Some("bottom"));
    let path = model.path(deepest).expect("path");
    assert_eq!(path.len(), 24, "one segment per level");
    assert!(path.breadcrumb(false).starts_with("level0 > level1 > "));
    assert!(path.breadcrumb(false).ends_with("level23"));
    // Every level but the last is a container; the document root is not a
    // foldable unit, so 24 containers give 23 folds.
    assert_eq!(doc.fold_parents().len(), 23);
    assert_eq!(model.outermost_fold(22), 0, "the forest has a single root");
}

/// A Kubernetes manifest is the shape a reader actually opens: deep mappings
/// with sequences of mappings, and an outline that names them.
#[test]
fn the_kubernetes_manifest_keeps_its_shape() {
    let model = open("k8s-deployment.yaml").expect("parses");
    let doc = model.as_structured().expect("structured");
    assert_eq!(doc.kind(), DocumentKind::Yaml);
    assert_eq!(doc.roots().len(), 1);

    let replicas = doc
        .nodes()
        .iter()
        .find(|n| n.key().is_some_and(|k| k.text == "replicas"))
        .expect("spec > replicas");
    assert_eq!(
        replicas.scalar().map(|s| (s.kind, s.text.as_str())),
        Some((ScalarKind::Number, "3"))
    );
    assert_eq!(
        model.path(replicas.id).expect("path").breadcrumb(false),
        "spec > replicas"
    );

    let containers = doc
        .nodes()
        .iter()
        .find(|n| n.key().is_some_and(|k| k.text == "containers"))
        .expect("containers");
    assert!(matches!(containers.kind, StructuredNodeKind::Sequence));
    assert_eq!(containers.child_count, 1);

    // The outline names the structure a reader navigates by.
    let outline = model.outline();
    let labels: Vec<&str> = outline.iter().map(|e| e.text.as_str()).collect();
    assert!(
        labels.iter().any(|l| l.contains("apiVersion")),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|l| l.contains("containers")),
        "{labels:?}"
    );
    // Searching a value finds the value field, not the key.
    let hits = doc.search_index().find("nginx:1.27-alpine", false);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].field, MatchField::Value);
}

/// A `---`-separated stream is three documents in one model, each with its own
/// root, and ids keep running across them.
#[test]
fn a_yaml_stream_keeps_every_document() {
    let model = open("multi-document.yaml").expect("parses");
    let doc = model.as_structured().expect("structured");
    assert_eq!(doc.roots().len(), 3, "three documents");
    let roots: Vec<_> = doc.roots().iter().map(|r| r.node).collect();
    assert_eq!(roots[0], 0);
    assert!(
        roots[0] < roots[1] && roots[1] < roots[2],
        "in stream order"
    );
    assert!(
        doc.roots()[1].explicit && doc.roots()[2].explicit,
        "the `---` separators are recorded"
    );
    for root in doc.roots() {
        assert_eq!(doc.node(root.node).and_then(|n| n.parent), None);
        assert!(doc.document_of(root.node).is_some());
    }
    assert_eq!(doc.document_of(roots[2]), Some(2));

    let kinds: Vec<&str> = doc
        .nodes()
        .iter()
        .filter(|n| n.key().is_some_and(|k| k.text == "kind"))
        .filter_map(|n| n.scalar().map(|s| s.text.as_str()))
        .collect();
    assert_eq!(kinds, ["ConfigMap", "Service", "Secret"]);
}

/// Comments are content: they are kept, attached to the row they annotate,
/// and searchable.
#[test]
fn comments_survive_and_are_searchable() {
    let doc = structured("comments.yaml");
    assert_eq!(
        doc.comments().len(),
        13,
        "every comment in the file is kept"
    );
    assert!(
        doc.nodes().iter().any(|n| !n.comments_above().is_empty()),
        "no comment was attached above a node"
    );
    let width = doc
        .nodes()
        .iter()
        .find(|n| n.key().is_some_and(|k| k.text == "width"))
        .expect("width");
    let right = width
        .comment_right()
        .and_then(|id| doc.comment(id))
        .expect("the trailing comment on `width`");
    assert!(right.text.contains("terminal width"), "{:?}", right.text);
    assert!(
        !doc.trailing_comments().is_empty(),
        "the comment after the last key is kept"
    );
    let hits = doc.search_index().find("Vim users", false);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].field, MatchField::Comment);
    // A `#` inside a value is not a comment.
    assert_eq!(
        width.scalar().map(|s| s.text.as_str()),
        Some("100"),
        "the comment is not part of the value"
    );
}

/// An alias stays a reference and a merge key stays a `<<` entry: diple reads
/// the document rather than resolving it.
#[test]
fn anchors_aliases_and_merge_keys_are_not_expanded() {
    let doc = structured("anchors.yaml");
    let anchors: Vec<&str> = doc.nodes().iter().filter_map(|n| n.anchor()).collect();
    assert!(
        anchors.contains(&"defaults") && anchors.contains(&"logging"),
        "{anchors:?}"
    );
    let aliases: Vec<&str> = doc
        .nodes()
        .iter()
        .filter_map(|n| match &n.kind {
            StructuredNodeKind::Alias { name } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        aliases.iter().filter(|n| **n == "defaults").count() >= 4,
        "aliases: {aliases:?}"
    );
    assert!(aliases.contains(&"name"), "a scalar alias too: {aliases:?}");

    // A merge key stays a `<<` entry rather than being performed.
    let merges = doc
        .nodes()
        .iter()
        .filter(|n| n.key().is_some_and(|k| k.text == "<<"))
        .count();
    assert_eq!(merges, 3, "three `<<` entries");
    // The merged-in keys were *not* copied into `development`.
    let development = doc
        .nodes()
        .iter()
        .find(|n| n.key().is_some_and(|k| k.text == "development"))
        .expect("development");
    let keys: Vec<&str> = doc
        .children(development.id)
        .into_iter()
        .filter_map(|id| doc.node(id))
        .filter_map(|n| n.key().map(|k| k.text.as_str()))
        .collect();
    assert_eq!(keys, ["<<", "database"], "no expansion into the mapping");
    // The anchor name is findable.
    assert!(doc
        .search_index()
        .find("defaults", false)
        .iter()
        .any(|m| m.field == MatchField::Anchor));
}

/// Explicit tags are kept in source form on the node they were written on.
#[test]
fn explicit_tags_are_kept() {
    let doc = structured("tags.yaml");
    let tag_of = |key: &str| -> Option<String> {
        doc.nodes()
            .iter()
            .find(|n| n.key().is_some_and(|k| k.text == key))
            .and_then(|n| n.tag())
            .map(str::to_string)
    };
    assert_eq!(tag_of("explicit_string").as_deref(), Some("!!str"));
    assert_eq!(tag_of("explicit_int").as_deref(), Some("!!int"));
    assert_eq!(tag_of("custom_tag").as_deref(), Some("!secret"));
    assert_eq!(tag_of("local_scalar").as_deref(), Some("!duration"));
    assert!(
        doc.nodes().iter().filter(|n| n.tag().is_some()).count() >= 10,
        "most entries in this fixture carry a tag"
    );
    // `!!str 2026` is a string even though it looks like a number.
    let s = doc
        .nodes()
        .iter()
        .find(|n| n.key().is_some_and(|k| k.text == "explicit_string"))
        .and_then(|n| n.scalar())
        .expect("explicit_string");
    assert_eq!(s.kind, ScalarKind::String, "the tag decides the type");
    assert!(doc
        .search_index()
        .find("!secret", false)
        .iter()
        .any(|m| m.field == MatchField::Tag));
}

/// A literal block keeps its line breaks, a folded block records that it was
/// folded, and neither is mistaken for YAML syntax.
#[test]
fn block_scalars_keep_their_style_and_their_text() {
    let doc = structured("block-scalars.yaml");
    let node = |key: &str| {
        doc.nodes()
            .iter()
            .find(|n| n.key().is_some_and(|k| k.text == key))
            .and_then(|n| n.scalar())
            .unwrap_or_else(|| panic!("{key} is missing"))
    };
    let literal = node("literal");
    assert_eq!(literal.style, ScalarStyle::Literal);
    assert!(literal.style.is_block());
    assert!(
        literal.text.contains("line one\nline two"),
        "line breaks are content: {:?}",
        literal.text
    );
    assert!(
        literal.text.contains("  indented line"),
        "{:?}",
        literal.text
    );

    let folded = node("folded");
    assert_eq!(folded.style, ScalarStyle::Folded);
    assert!(
        folded
            .text
            .starts_with("This long paragraph is folded onto one line"),
        "{:?}",
        folded.text
    );
    assert!(
        folded
            .text
            .contains("\nA blank line stays a paragraph break."),
        "the blank line is kept as a break: {:?}",
        folded.text
    );

    // A `#` and a `:` inside a literal block are text, not syntax.
    let script = node("script");
    assert!(script.text.contains("# not a comment in a literal block"));
    assert!(script.text.contains("echo \"hello: world\""));

    let plain = node("plain_multiline");
    assert_eq!(plain.style, ScalarStyle::Plain);
    assert_eq!(
        plain.text, "this value continues on the next line",
        "a plain multi-line scalar folds to one line"
    );
}

/// A `%YAML` directive is kept with the document it introduces.
#[test]
fn the_yaml_directive_survives() {
    let doc = structured("directive.yaml");
    let root = doc.roots().first().expect("one document");
    assert!(root.explicit, "the `---` after the directive is recorded");
    assert_eq!(
        root.directives,
        vec![Directive::Version { major: 1, minor: 2 }]
    );
    assert_eq!(root.directives[0].text(), "%YAML 1.2");
    assert!(doc.node_count() > 1);
}
