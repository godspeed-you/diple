//! The Markdown side of the search index.
//!
//! Every node — including nested ones, table cells and code — contributes one
//! [`MatchField::Body`] entry: a Markdown node renders as a run of prose, and
//! there is nothing inside it a reader would want highlighted separately.
//! Structured documents split a node into a key, a value, a tag and a comment
//! instead, which is exactly the difference the field label exists to carry.

use crate::document::search::{MatchField, SearchIndex, SearchIndexBuilder};

use super::ast::{inlines_to_text, Document, Node, NodeKind};

/// Build the index from a document (pre-order, so results are in document
/// order).
pub fn build(doc: &Document) -> SearchIndex {
    let mut builder = SearchIndexBuilder::with_capacity(doc.node_count());
    for node in doc.walk() {
        builder.push(node.id, MatchField::Body, node_text(node));
    }
    builder.build()
}

/// Plain text of a single node (its own content, not nested block children —
/// those are separate nodes with their own entries).
pub fn node_text(node: &Node) -> String {
    match &node.kind {
        NodeKind::Heading(h) => h.text.clone(),
        NodeKind::Paragraph(inlines) => inlines_to_text(inlines),
        NodeKind::Table(t) => {
            let mut out = String::new();
            for cell in &t.header {
                push_cell(&mut out, &inlines_to_text(cell));
            }
            for row in &t.rows {
                out.push('\n');
                for cell in row {
                    push_cell(&mut out, &inlines_to_text(cell));
                }
            }
            out
        }
        NodeKind::CodeBlock(c) => c.code.clone(),
        NodeKind::Mermaid(m) => m.source.clone(),
        NodeKind::Image(i) => i.alt.clone(),
        NodeKind::Html(h) => h.clone(),
        NodeKind::FootnoteDefinition(f) => f.label.clone(),
        NodeKind::List(_) | NodeKind::Quote(_) | NodeKind::HorizontalRule => String::new(),
    }
}

fn push_cell(out: &mut String, cell: &str) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push(' ');
    }
    out.push_str(cell);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::markdown::parse;

    /// Case-insensitive by default, and reported in reading order.
    ///
    /// The offsets are checked by slicing the indexed text with them — that
    /// is what every consumer does — rather than by hard-coding byte
    /// positions, which would break the moment the search text of a node is
    /// assembled differently.
    #[test]
    fn case_insensitive_default_in_document_order() {
        let doc = parse("# Hello\n\nhello world, HELLO again\n\n- list hello\n");
        let idx = build(&doc);
        let m = idx.find("hello", false);

        let hits: Vec<&str> = m
            .iter()
            .map(|hit| {
                let text = idx.text(hit.node, hit.field).expect("indexed node");
                &text[hit.start..hit.end]
            })
            .collect();
        assert_eq!(
            hits,
            ["Hello", "hello", "HELLO", "hello"],
            "every hit, in reading order, with its original casing"
        );

        // Reading order: nodes never go backwards, and within a node the
        // offsets increase and do not overlap.
        assert!(m.windows(2).all(|w| w[0].node <= w[1].node));
        assert!(m
            .windows(2)
            .all(|w| w[0].node < w[1].node || w[0].end <= w[1].start));

        // The two hits in the same paragraph share a node; the heading and
        // the list item are separate nodes.
        assert_eq!(m[1].node, m[2].node, "both paragraph hits are one node");
        assert_ne!(m[0].node, m[1].node, "the heading is its own node");
        assert_ne!(m[2].node, m[3].node, "the list item is its own node");
        assert!(m.iter().all(|hit| hit.field == MatchField::Body));
    }

    #[test]
    fn case_sensitive() {
        let doc = parse("Hello hello\n");
        let idx = build(&doc);
        assert_eq!(idx.find("Hello", true).len(), 1);
        assert_eq!(idx.find("hello", true).len(), 1);
        assert_eq!(idx.find("HELLO", true).len(), 0);
        assert_eq!(idx.find("HELLO", false).len(), 2);
    }

    #[test]
    fn searches_table_cells_code_and_mermaid() {
        let doc = parse("| h1 | h2 |\n|---|---|\n| needle | x |\n\n```\nlet needle = 1;\n```\n\n```mermaid\ngraph LR\nNeedle --> B\n```\n");
        let idx = build(&doc);
        let m = idx.find("needle", false);
        assert_eq!(m.iter().map(|m| m.node).collect::<Vec<_>>(), [0, 1, 2]);
        let text = idx.text(0, MatchField::Body).unwrap();
        assert_eq!(&text[m[0].start..m[0].end], "needle");
    }

    #[test]
    fn empty_query_and_no_match() {
        let doc = parse("text\n");
        let idx = build(&doc);
        assert!(idx.find("", false).is_empty());
        assert!(idx.find("zzz", false).is_empty());
    }

    #[test]
    fn unicode_case_folding_keeps_offsets_valid() {
        let doc = parse("Straße ÜBER straße\n");
        let idx = build(&doc);
        let text = idx.text(0, MatchField::Body).unwrap();
        let m = idx.find("über", false);
        assert_eq!(m.len(), 1);
        assert_eq!(&text[m[0].start..m[0].end], "ÜBER");
        assert_eq!(idx.find("straße", false).len(), 2);
        // Slicing must never panic even for chars whose lowercase is longer.
        let doc = parse("İstanbul\n");
        let idx = build(&doc);
        for m in idx.find("i̇stanbul", false) {
            let t = idx.text(m.node, m.field).unwrap();
            assert!(t.is_char_boundary(m.start) && t.is_char_boundary(m.end));
        }
    }

    #[test]
    fn matches_inside_emphasis_and_links() {
        let doc = parse("a **bold needle** and [link needle](http://x)\n");
        let idx = build(&doc);
        assert_eq!(idx.find("needle", false).len(), 2);
    }
}
