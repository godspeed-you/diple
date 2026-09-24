//! The structured document model shared by JSON and YAML.
//!
//! * [`ast`] — the node model itself,
//! * [`build`] — the builder both parsers push events at,
//! * this file — the navigation and outline derived from the model.

pub mod ast;
pub mod build;

pub use ast::{
    Comment, CommentId, Directive, NodeMeta, NodeRelation, ScalarKind, ScalarStyle, ScalarValue,
    StructuredDocument, StructuredKey, StructuredNode, StructuredNodeKind, StructuredRoot,
    MAX_DEPTH,
};
pub use build::{Builder, DepthExceeded};

use crate::document::folds::FoldState;
use crate::document::outline::{preview, OutlineEntry};
use crate::document::NodeId;

impl StructuredDocument {
    /// The next visible container after `from`, in document order.
    ///
    /// Containers are the structural units of a structured document in
    /// exactly the sense that headings are the structural units of Markdown:
    /// they are what folds, what the outline nests, and what a reader moves
    /// between when skimming. Scalars are deliberately not stops — `]` on a
    /// large array would otherwise be indistinguishable from `j`.
    pub fn next_container(&self, from: NodeId, folds: &FoldState) -> Option<NodeId> {
        self.nodes()
            .iter()
            .skip(from.saturating_add(1))
            .find(|n| n.is_container() && !self.is_hidden(n.id, folds))
            .map(|n| n.id)
    }

    /// The previous visible container before `from`, in document order.
    pub fn previous_container(&self, from: NodeId, folds: &FoldState) -> Option<NodeId> {
        self.nodes()
            .iter()
            .take(from.min(self.node_count()))
            .rev()
            .find(|n| n.is_container() && !self.is_hidden(n.id, folds))
            .map(|n| n.id)
    }

    /// The first visible container at or after `from` — where `]` lands when
    /// the cursor is not on one yet.
    pub fn container_at_or_after(&self, from: NodeId, folds: &FoldState) -> Option<NodeId> {
        self.nodes()
            .iter()
            .skip(from.min(self.node_count()))
            .find(|n| n.is_container() && !self.is_hidden(n.id, folds))
            .map(|n| n.id)
    }

    /// The next sibling of `from`, climbing to an ancestor's next sibling
    /// when `from` is the last child.
    ///
    /// Climbing mirrors Markdown's "same or higher level" reading of `}`: a
    /// reader at the end of a branch expects to leave it, not to stop.
    pub fn next_sibling_or_uncle(&self, from: NodeId) -> Option<NodeId> {
        let mut cur = from;
        let mut guard = 0usize;
        loop {
            if let Some(next) = self.next_sibling(cur) {
                return Some(next);
            }
            cur = self.parent(cur)?;
            guard += 1;
            if guard > self.node_count() {
                return None;
            }
        }
    }

    /// The previous sibling of `from`, or its parent when it is the first
    /// child — the mirror of [`Self::next_sibling_or_uncle`].
    pub fn previous_sibling_or_parent(&self, from: NodeId) -> Option<NodeId> {
        self.previous_sibling(from).or_else(|| self.parent(from))
    }

    /// The nearest node at or above `from` that a cursor can meaningfully sit
    /// on given the fold state: `from` itself when visible, otherwise its
    /// nearest visible ancestor.
    pub fn visible_anchor(&self, from: NodeId, folds: &FoldState) -> Option<NodeId> {
        if self.node(from).is_some() && !self.is_hidden(from, folds) {
            return Some(from);
        }
        let mut cur = self.parent(from);
        let mut guard = 0usize;
        while let Some(id) = cur {
            if !self.is_hidden(id, folds) {
                return Some(id);
            }
            cur = self.parent(id);
            guard += 1;
            if guard > self.node_count() {
                break;
            }
        }
        None
    }

    /// The outline: one entry per node, in document order.
    ///
    /// The policy is deliberately complete rather than filtered. A structured
    /// document has no headings to pick out, so any "interesting nodes only"
    /// rule would be a guess the reader cannot predict; a complete tree is
    /// what makes the outline a reliable map. Scalar values appear as short
    /// previews so that a 4 KiB string cannot decide the sidebar's width.
    pub fn outline(&self) -> Vec<OutlineEntry> {
        let mut entries = Vec::with_capacity(self.node_count());
        for node in self.nodes() {
            entries.push(OutlineEntry {
                node: node.id,
                fold: node.fold,
                depth: node.depth,
                text: self.outline_text(node.id),
            });
        }
        entries
    }

    /// What one outline line reads as.
    pub fn outline_text(&self, id: NodeId) -> String {
        let Some(node) = self.node(id) else {
            return String::new();
        };
        let label = self.label(id);
        match &node.kind {
            StructuredNodeKind::Mapping | StructuredNodeKind::Sequence => label,
            StructuredNodeKind::Alias { name } => format!("{label}: *{name}"),
            StructuredNodeKind::Scalar(value) => {
                let shown = preview(value.display());
                // Quoted where the rows quote it — every JSON string, a YAML
                // scalar the source quoted — so the string "3" does not pass
                // for the number 3 here either (spec P3).
                let quote = match value.style {
                    ScalarStyle::DoubleQuoted => Some('"'),
                    ScalarStyle::SingleQuoted => Some('\''),
                    _ => None,
                };
                if let Some(quote) = quote {
                    format!("{label}: {quote}{shown}{quote}")
                } else if shown.is_empty() {
                    label
                } else {
                    format!("{label}: {shown}")
                }
            }
        }
    }

    /// The document (0-based) a node belongs to.
    pub fn document_of(&self, id: NodeId) -> Option<usize> {
        let mut cur = Some(id);
        let mut guard = 0usize;
        while let Some(node_id) = cur {
            let node = self.node(node_id)?;
            if let NodeRelation::Root { document } = node.relation {
                return Some(document);
            }
            cur = node.parent;
            guard += 1;
            if guard > self.node_count() {
                break;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::format::DocumentKind;
    use crate::document::outline::PREVIEW_LIMIT;
    use crate::document::source::{SourceDocument, SourceSpan};

    /// ```text
    /// 0 root {
    /// 1   meta {
    /// 2     name: nginx
    /// 3   }
    ///   items [
    /// 4     [0] {
    /// 5       id: 1
    ///       }
    /// 6     [1]: two
    ///     ]
    /// ```
    fn doc() -> StructuredDocument {
        let mut b = Builder::new(DocumentKind::Yaml);
        b.begin_document(false, Vec::new());
        b.open_mapping(root(), SourceSpan::default(), None).unwrap();
        b.open_mapping(key("meta"), SourceSpan::default(), None)
            .unwrap();
        b.scalar(
            key("name"),
            ScalarValue::plain("nginx", ScalarKind::String),
            SourceSpan::default(),
            None,
        );
        b.close();
        b.open_sequence(key("items"), SourceSpan::default(), None)
            .unwrap();
        b.open_mapping(item(0), SourceSpan::default(), None)
            .unwrap();
        b.scalar(
            key("id"),
            ScalarValue::plain("1", ScalarKind::Number),
            SourceSpan::default(),
            None,
        );
        b.close();
        b.scalar(
            item(1),
            ScalarValue::plain("two", ScalarKind::String),
            SourceSpan::default(),
            None,
        );
        b.close();
        b.close();
        b.finish(SourceDocument::new("t", ""))
    }

    fn root() -> NodeRelation {
        NodeRelation::Root { document: 0 }
    }

    fn key(name: &str) -> NodeRelation {
        NodeRelation::MappingEntry {
            key: StructuredKey::plain(name, SourceSpan::default()),
        }
    }

    fn item(index: usize) -> NodeRelation {
        NodeRelation::SequenceItem { index }
    }

    fn folds(d: &StructuredDocument) -> FoldState {
        FoldState::from_parents(d.fold_parents().to_vec())
    }

    #[test]
    fn structural_navigation_visits_containers_only() {
        let d = doc();
        let f = folds(&d);
        // 0 root, 1 meta, 3 items, 4 [0] are the containers.
        assert_eq!(d.next_container(0, &f), Some(1));
        assert_eq!(d.next_container(1, &f), Some(3));
        assert_eq!(d.next_container(2, &f), Some(3), "skips the scalar");
        assert_eq!(d.next_container(3, &f), Some(4));
        assert_eq!(d.next_container(4, &f), None);
        assert_eq!(d.previous_container(4, &f), Some(3));
        assert_eq!(d.previous_container(2, &f), Some(1));
        assert_eq!(d.previous_container(0, &f), None);
        assert_eq!(d.container_at_or_after(3, &f), Some(3));
        assert_eq!(d.container_at_or_after(5, &f), None);
    }

    #[test]
    fn structural_navigation_skips_what_a_fold_hides() {
        let d = doc();
        let mut f = folds(&d);
        let items_fold = d.node(3).unwrap().fold.unwrap();
        f.collapse(items_fold);
        assert_eq!(
            d.next_container(1, &f),
            Some(3),
            "the collapsed container itself is still a stop"
        );
        assert_eq!(
            d.next_container(3, &f),
            None,
            "what it hides is not a stop any more"
        );
    }

    #[test]
    fn sibling_navigation_climbs_out_of_a_finished_branch() {
        let d = doc();
        assert_eq!(d.next_sibling_or_uncle(1), Some(3), "meta -> items");
        assert_eq!(d.next_sibling_or_uncle(2), Some(3), "name -> items");
        assert_eq!(d.next_sibling_or_uncle(5), Some(6), "id -> [1]");
        assert_eq!(d.next_sibling_or_uncle(6), None, "the end of the document");
        assert_eq!(d.previous_sibling_or_parent(6), Some(4));
        assert_eq!(d.previous_sibling_or_parent(5), Some(4), "up to [0]");
        assert_eq!(d.previous_sibling_or_parent(0), None);
    }

    #[test]
    fn the_outline_is_the_whole_tree_with_short_previews() {
        let d = doc();
        let outline = d.outline();
        let shown: Vec<(usize, &str)> =
            outline.iter().map(|e| (e.depth, e.text.as_str())).collect();
        assert_eq!(
            shown,
            [
                (0, "root"),
                (1, "meta"),
                (2, "name: nginx"),
                (1, "items"),
                (2, "[0]"),
                (3, "id: 1"),
                (2, "[1]: two"),
            ]
        );
        // Every entry points at a node; only foldable containers carry a fold,
        // and a document root is not one.
        assert_eq!(outline[0].fold, None, "the root is not a fold target");
        assert_eq!(outline[1].fold, Some(0), "`meta` is the first one");
        assert_eq!(outline[2].fold, None, "a scalar does not fold");
        assert_eq!(outline[3].fold, Some(1));
        assert_eq!(outline[4].node, 4);
        assert_eq!(outline.len(), d.node_count(), "the policy is complete");
    }

    #[test]
    fn an_outline_preview_never_lets_one_scalar_decide_the_width() {
        let mut b = Builder::new(DocumentKind::Yaml);
        b.begin_document(false, Vec::new());
        b.open_mapping(root(), SourceSpan::default(), None).unwrap();
        b.scalar(
            key("script"),
            ScalarValue::plain("x".repeat(4096), ScalarKind::String),
            SourceSpan::default(),
            None,
        );
        b.scalar(
            key("empty"),
            ScalarValue::plain("", ScalarKind::Null),
            SourceSpan::default(),
            None,
        );
        b.close();
        let d = b.finish(SourceDocument::new("t", ""));

        let outline = d.outline();
        let long = &outline[1].text;
        assert!(long.starts_with("script: x"), "{long}");
        assert!(
            crate::util::unicode::width(long) <= "script: ".len() + PREVIEW_LIMIT,
            "{long}"
        );
        assert_eq!(
            outline[2].text, "empty",
            "an empty value leaves the key alone"
        );
    }

    #[test]
    fn a_deep_document_gets_a_complete_outline_without_recursing() {
        let mut b = Builder::new(DocumentKind::Json);
        b.begin_document(true, Vec::new());
        b.open_mapping(root(), SourceSpan::default(), None).unwrap();
        for _ in 1..MAX_DEPTH {
            b.open_sequence(item(0), SourceSpan::default(), None)
                .unwrap();
        }
        let d = b.finish(SourceDocument::new("t", ""));

        let outline = d.outline();
        assert_eq!(outline.len(), MAX_DEPTH);
        assert_eq!(outline[MAX_DEPTH - 1].depth, MAX_DEPTH - 1);
        assert_eq!(outline[MAX_DEPTH - 1].text, "[0]");
        assert_eq!(outline[0].fold, None);
        assert_eq!(outline[1].fold, Some(0), "the fold ids stay dense");
        assert_eq!(d.document_of(MAX_DEPTH - 1), Some(0));
    }

    #[test]
    fn a_collapsed_ancestor_is_what_a_search_reveal_has_to_open() {
        let d = doc();
        let mut f = folds(&d);
        f.collapse_all();
        // `name`, deep inside `meta`.
        let hits = d.search_index().find("nginx", false);
        let hit = hits.first().expect("a match");
        assert!(d.is_hidden(hit.node, &f));
        assert_eq!(d.visible_anchor(hit.node, &f), Some(1), "up to `meta`");

        d.reveal(hit.node, &mut f);
        assert!(!d.is_hidden(hit.node, &f));
        assert_eq!(d.visible_anchor(hit.node, &f), Some(hit.node));
        assert!(
            f.is_collapsed(d.node(3).unwrap().fold.unwrap()),
            "`items` is unrelated and stays collapsed"
        );
    }

    #[test]
    fn a_visible_anchor_climbs_to_the_nearest_shown_ancestor() {
        let d = doc();
        let mut f = folds(&d);
        assert_eq!(d.visible_anchor(5, &f), Some(5));
        f.collapse(d.node(3).unwrap().fold.unwrap());
        assert_eq!(d.visible_anchor(5, &f), Some(3), "up to `items`");
    }

    #[test]
    fn every_node_knows_which_document_it_came_from() {
        let d = doc();
        assert_eq!(d.document_of(5), Some(0));
        assert_eq!(d.document_of(999), None);
    }
}
