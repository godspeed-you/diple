//! The format boundary: one document, whatever format it came from.
//!
//! # What this type is for
//!
//! Above this line the application knows only that it has *a document*: it
//! asks what the document can do, where the cursor can go, what folds, what
//! the outline says and where a node sits in the structure. Below this line
//! each backend keeps the semantic model its format actually has — Markdown
//! keeps headings, sections, anchors, footnotes and links; JSON and YAML keep
//! mappings, sequences, scalars, anchors and comments.
//!
//! # Why an enum and not a trait
//!
//! A trait object would force every backend through one method table and,
//! worse, would push the application towards a lowest-common-denominator tree
//! — which is precisely what would break the moment a format arrives whose
//! structure is not a tree. An enum keeps the dispatch visible, the ownership
//! simple, the matches exhaustive, and each backend's own model reachable for
//! the few things that genuinely are format-specific (Mermaid diagrams and
//! link following are Markdown's, and asking for them says so).
//!
//! # Adding a format
//!
//! A new backend adds one variant, reports its [`DocumentCapabilities`], and
//! answers the questions below. Nothing in `app`, `layout` or `render` needs
//! to learn its name: a capability it does not have simply makes the
//! corresponding command report that there is nothing to do, and the key
//! hints stop offering it.

use super::capabilities::DocumentCapabilities;
use super::folds::{FoldId, FoldState};
use super::format::DocumentKind;
use super::markdown;
use super::outline::OutlineEntry;
use super::path::DocumentPath;
use super::search::SearchIndex;
use super::source::SourceDocument;
use super::structured::StructuredDocument;
use super::NodeId;

/// A loaded document in whatever format it was.
#[derive(Debug, Clone)]
pub enum DocumentModel {
    /// A Markdown document, with its own AST.
    Markdown(Box<markdown::Document>),
    /// A JSON or YAML document, in the shared structured model.
    Structured(Box<StructuredDocument>),
}

impl Default for DocumentModel {
    fn default() -> Self {
        DocumentModel::Markdown(Box::default())
    }
}

impl DocumentModel {
    /// A Markdown document.
    pub fn markdown(doc: markdown::Document) -> Self {
        DocumentModel::Markdown(Box::new(doc))
    }

    /// A structured (JSON or YAML) document.
    pub fn structured(doc: StructuredDocument) -> Self {
        DocumentModel::Structured(Box::new(doc))
    }

    /// Which format this is.
    pub fn kind(&self) -> DocumentKind {
        match self {
            DocumentModel::Markdown(_) => DocumentKind::Markdown,
            DocumentModel::Structured(doc) => doc.kind(),
        }
    }

    /// What the document supports.
    pub fn capabilities(&self) -> DocumentCapabilities {
        match self {
            DocumentModel::Markdown(_) => DocumentCapabilities::MARKDOWN,
            DocumentModel::Structured(_) => DocumentCapabilities::STRUCTURED,
        }
    }

    /// The Markdown model, for the operations only Markdown has.
    ///
    /// Callers must have a Markdown-specific reason — following a link,
    /// resolving an anchor, toggling a Mermaid diagram. Anything a reader
    /// expects in every format belongs on this type instead.
    pub fn as_markdown(&self) -> Option<&markdown::Document> {
        match self {
            DocumentModel::Markdown(doc) => Some(doc),
            DocumentModel::Structured(_) => None,
        }
    }

    /// The structured model, for the operations only structured data has.
    pub fn as_structured(&self) -> Option<&StructuredDocument> {
        match self {
            DocumentModel::Structured(doc) => Some(doc),
            DocumentModel::Markdown(_) => None,
        }
    }

    /// The source the document was parsed from.
    pub fn source(&self) -> &SourceDocument {
        match self {
            DocumentModel::Markdown(doc) => &doc.source,
            DocumentModel::Structured(doc) => doc.source(),
        }
    }

    /// A title derived from the document, if it has one.
    pub fn title(&self) -> Option<&str> {
        match self {
            DocumentModel::Markdown(doc) => doc.title.as_deref(),
            DocumentModel::Structured(_) => None,
        }
    }

    /// The search index over everything the reader can see.
    pub fn search_index(&self) -> &SearchIndex {
        match self {
            DocumentModel::Markdown(doc) => &doc.search,
            DocumentModel::Structured(doc) => doc.search_index(),
        }
    }

    /// Total number of semantic nodes.
    pub fn node_count(&self) -> usize {
        match self {
            DocumentModel::Markdown(doc) => doc.node_count(),
            DocumentModel::Structured(doc) => doc.node_count(),
        }
    }

    // ---- folding ---------------------------------------------------------

    /// The fold forest: one optional parent per foldable unit, in document
    /// order. This is what a [`FoldState`] is built from.
    pub fn fold_parents(&self) -> Vec<Option<FoldId>> {
        match self {
            DocumentModel::Markdown(doc) => doc.fold_parents(),
            DocumentModel::Structured(doc) => doc.fold_parents().to_vec(),
        }
    }

    /// A fresh all-expanded fold state for this document.
    pub fn fold_state(&self) -> FoldState {
        FoldState::from_parents(self.fold_parents())
    }

    /// The fold target a command at `node` acts on.
    ///
    /// For a node that is itself foldable this is that node; otherwise it is
    /// the innermost foldable unit containing it — the Markdown section a
    /// paragraph belongs to, the container a scalar sits in. Deterministic in
    /// both formats, and the reason `za` on a body line folds the section a
    /// reader is reading rather than reporting that there is nothing here.
    pub fn fold_at(&self, node: NodeId) -> Option<FoldId> {
        match self {
            DocumentModel::Markdown(doc) => doc.section_of(node),
            DocumentModel::Structured(doc) => doc.fold_at(node),
        }
    }

    /// The node that a fold target's own visible row belongs to: the heading
    /// of a Markdown section, the key row of a container.
    pub fn fold_node(&self, fold: FoldId) -> Option<NodeId> {
        match self {
            DocumentModel::Markdown(doc) => doc.sections.get(fold).map(|s| s.heading),
            DocumentModel::Structured(doc) => doc.fold_node(fold),
        }
    }

    /// The outermost ancestor of a fold target — the top of the run of
    /// content a fold change rewrites.
    pub fn outermost_fold(&self, fold: FoldId) -> FoldId {
        let parents = self.fold_parents();
        let mut cur = fold;
        let mut guard = 0usize;
        while let Some(parent) = parents.get(cur).copied().flatten() {
            cur = parent;
            guard += 1;
            if guard > parents.len() {
                break;
            }
        }
        cur
    }

    /// Whether a node is hidden by the fold state.
    pub fn is_hidden(&self, node: NodeId, folds: &FoldState) -> bool {
        match self {
            DocumentModel::Markdown(doc) => doc.is_hidden(node, folds),
            DocumentModel::Structured(doc) => doc.is_hidden(node, folds),
        }
    }

    /// Expand every collapsed ancestor of `node` so its row becomes visible.
    /// Returns the fold whose subtree changed, if any.
    ///
    /// A container's key row stays visible while the container is collapsed,
    /// so revealing the container opens only what encloses it (spec AC-09).
    /// A Markdown heading keeps 1.x behaviour and opens its own section as
    /// well.
    pub fn reveal(&self, node: NodeId, folds: &mut FoldState) -> Option<FoldId> {
        let mut fold = self.fold_at(node)?;
        if let DocumentModel::Structured(_) = self {
            if self.fold_node(fold) == Some(node) {
                fold = folds.parent(fold)?;
            }
        }
        folds.reveal(fold);
        Some(fold)
    }

    // ---- navigation ------------------------------------------------------

    /// The node a cursor should start on.
    pub fn first_semantic(&self) -> Option<NodeId> {
        match self {
            DocumentModel::Markdown(doc) => doc.nodes.first().map(|n| n.id),
            DocumentModel::Structured(doc) => doc.first_semantic(),
        }
    }

    /// Whether `node` is a primary structural node — a Markdown heading, a
    /// JSON/YAML container. These are the stops `[` and `]` visit and the
    /// units the outline nests.
    pub fn is_structural(&self, node: NodeId) -> bool {
        match self {
            DocumentModel::Markdown(doc) => doc
                .node(node)
                .is_some_and(|n| matches!(n.kind, markdown::NodeKind::Heading(_))),
            DocumentModel::Structured(doc) => doc.node(node).is_some_and(|n| n.is_container()),
        }
    }

    /// The next primary structural node after `from`, in document order.
    ///
    /// This is what `]` walks: Markdown headings, JSON/YAML containers.
    /// Visibility is the caller's business — a fold that hides a node also
    /// removes its row, and the caller skips what has no row — so this stays
    /// a pure question about the document.
    pub fn next_structural(&self, from: NodeId) -> Option<NodeId> {
        match self {
            DocumentModel::Markdown(doc) => doc
                .sections
                .iter()
                .find(|s| s.heading > from)
                .map(|s| s.heading),
            DocumentModel::Structured(doc) => doc
                .nodes()
                .iter()
                .skip(from.saturating_add(1))
                .find(|n| n.is_container())
                .map(|n| n.id),
        }
    }

    /// The previous primary structural node before `from`.
    pub fn previous_structural(&self, from: NodeId) -> Option<NodeId> {
        match self {
            DocumentModel::Markdown(doc) => doc
                .sections
                .iter()
                .rev()
                .find(|s| s.heading < from)
                .map(|s| s.heading),
            DocumentModel::Structured(doc) => doc
                .nodes()
                .iter()
                .take(from.min(doc.node_count()))
                .rev()
                .find(|n| n.is_container())
                .map(|n| n.id),
        }
    }

    /// The innermost structural node containing `node`, or `node` itself when
    /// it is one. This is where `[` lands from a body row.
    pub fn enclosing_structural(&self, node: NodeId) -> Option<NodeId> {
        if self.is_structural(node) {
            return Some(node);
        }
        self.fold_at(node).and_then(|fold| self.fold_node(fold))
    }

    /// The parent of a node in the document hierarchy.
    ///
    /// For Markdown this is the heading of the enclosing section — from a
    /// body paragraph, its own heading; from a heading, its parent's.
    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        match self {
            DocumentModel::Markdown(doc) => {
                let section = doc.section_of(node)?;
                let s = doc.sections.get(section)?;
                if s.heading == node {
                    doc.sections.get(s.parent?).map(|p| p.heading)
                } else {
                    Some(s.heading)
                }
            }
            DocumentModel::Structured(doc) => {
                let id = doc.node(node)?.id;
                doc.parent(id)
            }
        }
    }

    /// The first child of a node in the document hierarchy.
    pub fn first_child(&self, node: NodeId) -> Option<NodeId> {
        match self {
            DocumentModel::Markdown(doc) => {
                let section = doc.section_of(node)?;
                let s = doc.sections.get(section)?;
                // From a body node, "in" means the section it belongs to.
                if s.heading != node {
                    return Some(s.heading);
                }
                doc.sections.get(*s.children.first()?).map(|c| c.heading)
            }
            DocumentModel::Structured(doc) => doc.first_child(node),
        }
    }

    /// The next sibling at the same or a higher level (`}`).
    pub fn next_sibling(&self, node: NodeId) -> Option<NodeId> {
        match self {
            DocumentModel::Markdown(doc) => {
                let section = doc.section_of(node)?;
                let level = doc.sections.get(section)?.level;
                let next = doc.next_section_at_or_above(section, level)?;
                doc.sections.get(next).map(|s| s.heading)
            }
            DocumentModel::Structured(doc) => doc.next_sibling_or_uncle(node),
        }
    }

    /// The previous sibling at the same or a higher level (`{`).
    pub fn previous_sibling(&self, node: NodeId) -> Option<NodeId> {
        match self {
            DocumentModel::Markdown(doc) => {
                let section = doc.section_of(node)?;
                let level = doc.sections.get(section)?.level;
                let previous = doc.previous_section_at_or_above(section, level)?;
                doc.sections.get(previous).map(|s| s.heading)
            }
            DocumentModel::Structured(doc) => doc.previous_sibling_or_parent(node),
        }
    }

    // ---- outline ---------------------------------------------------------

    /// The outline: the table of contents for Markdown, the structure tree
    /// for JSON and YAML.
    ///
    /// Derived on demand from the parsed model — never by reparsing — so
    /// opening the sidebar costs one walk and closing it costs nothing.
    pub fn outline(&self) -> Vec<OutlineEntry> {
        match self {
            DocumentModel::Markdown(doc) => doc.outline(),
            DocumentModel::Structured(doc) => doc.outline(),
        }
    }

    /// Whether the document has an outline at all, answered without building
    /// one (a document with no headings has no table of contents).
    pub fn has_outline(&self) -> bool {
        match self {
            DocumentModel::Markdown(doc) => !doc.sections.is_empty(),
            DocumentModel::Structured(doc) => !doc.is_empty(),
        }
    }

    // ---- paths -----------------------------------------------------------

    /// The semantic path to a node, for formats that have one.
    pub fn path(&self, node: NodeId) -> Option<DocumentPath> {
        match self {
            DocumentModel::Markdown(_) => None,
            DocumentModel::Structured(doc) => {
                doc.node(node)?;
                Some(doc.path(node))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::json;

    fn md(src: &str) -> DocumentModel {
        DocumentModel::markdown(markdown::parse(src))
    }

    fn json_doc(src: &str) -> DocumentModel {
        DocumentModel::structured(
            json::parse(&SourceDocument::new("t.json", src)).expect("valid JSON"),
        )
    }

    const MD: &str = "# A\n\nbody\n\n## A1\n\nnested\n\n## A2\n\n# B\n";
    const JSON: &str = r#"{"meta": {"name": "nginx"}, "items": [1, 2], "n": 3}"#;

    #[test]
    fn both_formats_report_what_they_can_do() {
        assert_eq!(md(MD).kind(), DocumentKind::Markdown);
        assert_eq!(json_doc(JSON).kind(), DocumentKind::Json);

        let m = md(MD).capabilities();
        assert!(m.links && m.folding && m.outline && !m.semantic_path);
        let j = json_doc(JSON).capabilities();
        assert!(j.semantic_path && j.folding && j.outline && !j.links);
    }

    #[test]
    fn the_backend_model_is_reachable_only_through_its_own_accessor() {
        let m = md(MD);
        assert!(m.as_markdown().is_some());
        assert!(m.as_structured().is_none());
        let j = json_doc(JSON);
        assert!(j.as_structured().is_some());
        assert!(j.as_markdown().is_none());
    }

    #[test]
    fn folding_works_the_same_way_through_the_boundary() {
        for doc in [md(MD), json_doc(JSON)] {
            let mut folds = doc.fold_state();
            assert!(!folds.is_empty(), "{:?} has foldable units", doc.kind());

            // The outermost fold is a root of the forest.
            let outer = doc.outermost_fold(folds.len() - 1);
            assert_eq!(folds.parent(outer), None);

            // Collapsing everything and revealing one node opens exactly its
            // ancestors.
            folds.collapse_all();
            let deep = (0..doc.node_count())
                .find(|n| doc.is_hidden(*n, &folds))
                .expect("something is hidden");
            doc.reveal(deep, &mut folds);
            assert!(!doc.is_hidden(deep, &folds), "{:?}", doc.kind());
        }
    }

    #[test]
    fn markdown_navigates_by_sections() {
        let doc = md(MD);
        let d = doc.as_markdown().unwrap();
        let (a, a1, a2, b) = (
            d.sections[0].heading,
            d.sections[1].heading,
            d.sections[2].heading,
            d.sections[3].heading,
        );
        assert!(doc.is_structural(a));
        assert!(!doc.is_structural(a + 1), "the body is not a heading");
        assert_eq!(doc.first_child(a), Some(a1));
        assert_eq!(doc.parent(a1), Some(a));
        assert_eq!(doc.parent(a), None);
        assert_eq!(doc.next_sibling(a1), Some(a2));
        assert_eq!(doc.previous_sibling(a2), Some(a1));
        assert_eq!(doc.next_sibling(a2), Some(b));
        assert_eq!(doc.fold_node(0), Some(a));
        assert_eq!(doc.path(a), None, "Markdown has no key path");
        assert_eq!(doc.title(), Some("A"));
    }

    #[test]
    fn structured_navigates_by_containers() {
        let doc = json_doc(JSON);
        let d = doc.as_structured().unwrap();
        // 0 root, 1 meta, 2 name, 3 items, 4 [0], 5 [1], 6 n
        assert!(doc.is_structural(0) && doc.is_structural(1) && doc.is_structural(3));
        assert!(!doc.is_structural(2));
        assert_eq!(doc.first_semantic(), Some(1), "starts on the first entry");
        assert_eq!(doc.first_child(1), Some(2));
        assert_eq!(doc.parent(2), Some(1));
        assert_eq!(doc.next_sibling(1), Some(3));
        assert_eq!(doc.previous_sibling(3), Some(1));
        assert_eq!(doc.path(2).unwrap().breadcrumb(false), "meta > name");
        assert_eq!(doc.path(4).unwrap().breadcrumb(false), "items > [0]");
        assert_eq!(d.node_count(), 7);
        assert_eq!(doc.title(), None);
    }

    #[test]
    fn the_outline_is_derived_not_reparsed_and_matches_the_hierarchy() {
        let doc = md(MD);
        let outline = doc.outline();
        assert_eq!(
            outline
                .iter()
                .map(|e| (e.depth, e.text.as_str()))
                .collect::<Vec<_>>(),
            [(0, "A"), (1, "A1"), (1, "A2"), (0, "B")]
        );
        assert!(doc.has_outline());
        assert_eq!(doc.outline(), outline, "deriving it twice agrees");

        let doc = json_doc(JSON);
        assert!(doc.has_outline());
        assert_eq!(doc.outline().len(), doc.node_count());

        let empty = md("just prose\n");
        assert!(!empty.has_outline());
        assert!(empty.outline().is_empty());
    }

    #[test]
    fn search_is_one_index_whatever_the_format() {
        assert_eq!(md(MD).search_index().find("nested", false).len(), 1);
        let doc = json_doc(JSON);
        let hits = doc.search_index().find("n", false);
        assert!(!hits.is_empty());
        assert!(
            hits.windows(2).all(|w| w[0].node <= w[1].node),
            "results are in document order"
        );
    }

    #[test]
    fn an_empty_document_answers_every_question_without_panicking() {
        let doc = md("");
        assert_eq!(doc.node_count(), 0);
        assert_eq!(doc.first_semantic(), None);
        assert!(doc.fold_state().is_empty());
        assert_eq!(doc.fold_at(0), None);
        assert_eq!(doc.fold_node(0), None);
        assert_eq!(doc.parent(0), None);
        assert_eq!(doc.first_child(0), None);
        assert_eq!(doc.next_sibling(0), None);
        assert_eq!(doc.previous_sibling(0), None);
        assert!(!doc.is_hidden(0, &doc.fold_state()));
        assert_eq!(doc.reveal(0, &mut doc.fold_state()), None);
        assert!(!doc.is_structural(0));
        assert_eq!(doc.outermost_fold(0), 0);
        assert_eq!(DocumentModel::default().node_count(), 0);
    }
    /// Spec §12.3: the `{` / `}` rule "must be symmetric and test-covered".
    ///
    /// Symmetric here does not mean the two are inverses — Markdown's "same
    /// or higher level" has never been one either, because climbing out of a
    /// finished branch loses the level you came from. It means the two rules
    /// are mirror images: each moves in its own direction only, each lands on
    /// a node at the same level or shallower, and each is total, so walking
    /// either way terminates. That has to hold in every format, or one key
    /// means two things.
    #[test]
    fn sibling_navigation_is_symmetric_in_both_formats() {
        for doc in [md(MD), json_doc(JSON)] {
            let count = doc.node_count();
            assert!(count > 3, "{:?} has something to walk", doc.kind());

            for node in 0..count {
                if let Some(next) = doc.next_sibling(node) {
                    assert!(next > node, "{:?}: `}}` only moves forward", doc.kind());
                }
                if let Some(previous) = doc.previous_sibling(node) {
                    assert!(
                        previous < node,
                        "{:?}: `{{` only moves backward",
                        doc.kind()
                    );
                }
            }

            // Walking forward from the first node and then back from the last
            // both terminate and visit strictly monotonic runs.
            for forward in [true, false] {
                let mut cursor = if forward { 0 } else { count - 1 };
                let mut steps = 0usize;
                while let Some(next) = if forward {
                    doc.next_sibling(cursor)
                } else {
                    doc.previous_sibling(cursor)
                } {
                    cursor = next;
                    steps += 1;
                    assert!(steps <= count, "{:?}: the walk does not end", doc.kind());
                }
            }
        }
    }
}
