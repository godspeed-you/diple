//! Smoke test: every fixture parses without panicking and yields a coherent
//! document (unique pre-order ids, resolvable lookups, valid spans).

mod common;

use diple::document::markdown;
use diple::document::FoldState;

#[test]
fn all_fixtures_parse_coherently() {
    let dir = common::fixtures_dir();
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        count += 1;
        let source = std::fs::read_to_string(&path).expect("read fixture");
        let doc = markdown::parse(&source);
        let name = path.display();
        assert!(!doc.nodes.is_empty(), "{name}: no nodes");
        let ids: Vec<_> = doc.walk().map(|n| n.id).collect();
        assert_eq!(
            ids,
            (0..doc.node_count()).collect::<Vec<_>>(),
            "{name}: ids not pre-order"
        );
        for node in doc.walk() {
            assert!(
                doc.node(node.id).is_some(),
                "{name}: node {} unresolvable",
                node.id
            );
            assert!(node.span.start <= node.span.end, "{name}: bad span");
            assert!(node.span.end <= source.len(), "{name}: span past EOF");
            assert!(
                source.is_char_boundary(node.span.start),
                "{name}: span not on boundary"
            );
            assert!(
                source.is_char_boundary(node.span.end),
                "{name}: span not on boundary"
            );
        }
        let folds = FoldState::from_parents(doc.fold_parents());
        for s in &doc.sections {
            assert!(
                !doc.is_hidden(s.heading, &folds),
                "{name}: heading hidden by default"
            );
        }
        let index = markdown::search::build(&doc);
        let _ = index.find("the", false);
    }
    assert_eq!(count, 10, "expected the ten fixtures");
}

#[test]
fn mermaid_fixture_detects_diagrams() {
    let source = std::fs::read_to_string(common::fixture("mermaid.md")).expect("mermaid fixture");
    let doc = markdown::parse(&source);
    let mermaid = doc
        .walk()
        .filter(|n| matches!(n.kind, markdown::NodeKind::Mermaid(_)))
        .count();
    assert_eq!(mermaid, 4);
    let code = doc
        .walk()
        .filter(|n| matches!(n.kind, markdown::NodeKind::CodeBlock(_)))
        .count();
    assert_eq!(code, 1, "the `text` fence is not mermaid");
}
