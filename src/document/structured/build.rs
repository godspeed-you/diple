//! Building a [`StructuredDocument`].
//!
//! Both structured parsers push events at this builder rather than
//! constructing nodes themselves, so the invariants the rest of diple relies
//! on — dense pre-order ids, correct subtree ends, fold targets over exactly
//! the containers, a search index in reading order — are established once and
//! cannot drift between JSON and YAML.
//!
//! The builder keeps an explicit stack: the nesting depth of a document is
//! decided by whoever wrote it, and a recursive builder would let a crafted
//! file end the process with a stack overflow instead of an error message.

use crate::document::folds::FoldId;
use crate::document::format::DocumentKind;
use crate::document::search::{MatchField, SearchIndexBuilder};
use crate::document::source::{SourceDocument, SourceSpan};
use crate::document::NodeId;

use super::ast::{
    Comment, CommentId, Directive, NodeMeta, NodeRelation, ScalarValue, StructuredDocument,
    StructuredNode, StructuredNodeKind, StructuredRoot, MAX_DEPTH,
};

/// The document nests deeper than [`MAX_DEPTH`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthExceeded {
    /// The limit that was hit.
    pub limit: usize,
}

/// Accumulates nodes, comments and roots into a [`StructuredDocument`].
#[derive(Debug)]
pub struct Builder {
    kind: DocumentKind,
    nodes: Vec<StructuredNode>,
    roots: Vec<StructuredRoot>,
    comments: Vec<Comment>,
    trailing_comments: Vec<CommentId>,
    fold_parents: Vec<Option<FoldId>>,
    fold_nodes: Vec<NodeId>,
    /// Open containers, outermost first.
    stack: Vec<NodeId>,
    /// The document a node pushed at depth 0 belongs to.
    document: usize,
    /// Whether [`Builder::begin_document`] has been called for the document
    /// currently being filled.
    open_document: Option<(bool, Vec<Directive>)>,
}

impl Builder {
    /// A builder for a document of `kind`.
    ///
    /// The source is handed over at [`Builder::finish`] rather than here, so
    /// that format detection can attempt a parse and throw it away without
    /// having copied the text.
    pub fn new(kind: DocumentKind) -> Self {
        Self {
            kind,
            nodes: Vec::new(),
            roots: Vec::new(),
            comments: Vec::new(),
            trailing_comments: Vec::new(),
            fold_parents: Vec::new(),
            fold_nodes: Vec::new(),
            stack: Vec::new(),
            document: 0,
            open_document: None,
        }
    }

    /// Reserve room for `n` nodes.
    pub fn reserve(&mut self, n: usize) {
        self.nodes.reserve(n);
    }

    /// Start a source document. The next node pushed at depth 0 becomes its
    /// root, whatever relation the caller passes.
    pub fn begin_document(&mut self, explicit: bool, directives: Vec<Directive>) {
        self.open_document = Some((explicit, directives));
    }

    /// Whether a document was begun but has no root node yet.
    pub fn document_is_pending(&self) -> bool {
        self.open_document.is_some()
    }

    /// Number of documents started so far.
    pub fn document_count(&self) -> usize {
        self.document
    }

    /// Record a comment and return its id.
    pub fn comment(&mut self, text: impl Into<String>, span: SourceSpan) -> CommentId {
        self.comments.push(Comment {
            text: text.into(),
            span,
        });
        self.comments.len() - 1
    }

    /// Record a comment that belongs to no node (it follows the last one).
    pub fn trailing_comment(&mut self, id: CommentId) {
        self.trailing_comments.push(id);
    }

    /// The node currently being filled, if any.
    pub fn current_container(&self) -> Option<NodeId> {
        self.stack.last().copied()
    }

    /// Current nesting depth.
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Attach metadata to an already-pushed node, merging with what is there.
    ///
    /// The YAML backend needs this because a comment's placement is only
    /// known once the following token has been seen.
    pub fn attach_meta(&mut self, id: NodeId, f: impl FnOnce(&mut NodeMeta)) {
        let Some(node) = self.nodes.get_mut(id) else {
            return;
        };
        let mut meta = node.meta.take().unwrap_or_default();
        f(&mut meta);
        node.meta = if meta.is_empty() {
            None
        } else {
            Some(Box::new(*meta))
        };
    }

    /// Open a mapping/object. Must be balanced by [`Builder::close`].
    pub fn open_mapping(
        &mut self,
        relation: NodeRelation,
        span: SourceSpan,
        meta: Option<NodeMeta>,
    ) -> Result<NodeId, DepthExceeded> {
        self.open(relation, StructuredNodeKind::Mapping, span, meta)
    }

    /// Open a sequence/array. Must be balanced by [`Builder::close`].
    pub fn open_sequence(
        &mut self,
        relation: NodeRelation,
        span: SourceSpan,
        meta: Option<NodeMeta>,
    ) -> Result<NodeId, DepthExceeded> {
        self.open(relation, StructuredNodeKind::Sequence, span, meta)
    }

    /// Add a scalar.
    pub fn scalar(
        &mut self,
        relation: NodeRelation,
        value: ScalarValue,
        span: SourceSpan,
        meta: Option<NodeMeta>,
    ) -> NodeId {
        self.push(relation, StructuredNodeKind::Scalar(value), span, meta)
    }

    /// Add a YAML alias — a reference, never a copy of the anchored subtree.
    pub fn alias(
        &mut self,
        relation: NodeRelation,
        name: impl Into<String>,
        span: SourceSpan,
        meta: Option<NodeMeta>,
    ) -> NodeId {
        self.push(
            relation,
            StructuredNodeKind::Alias { name: name.into() },
            span,
            meta,
        )
    }

    /// Close the innermost open container.
    pub fn close(&mut self) {
        if let Some(id) = self.stack.pop() {
            let end = self.nodes.len();
            if let Some(node) = self.nodes.get_mut(id) {
                node.end = end;
            }
        }
    }

    /// Close every container still open (used on an early parser stop).
    pub fn close_all(&mut self) {
        while !self.stack.is_empty() {
            self.close();
        }
    }

    /// Extend a container's span to cover its whole source range.
    pub fn set_span_end(&mut self, id: NodeId, end: usize) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.span.end = node.span.end.max(end);
        }
    }

    fn open(
        &mut self,
        relation: NodeRelation,
        kind: StructuredNodeKind,
        span: SourceSpan,
        meta: Option<NodeMeta>,
    ) -> Result<NodeId, DepthExceeded> {
        if self.stack.len() >= MAX_DEPTH {
            return Err(DepthExceeded { limit: MAX_DEPTH });
        }
        let id = self.push(relation, kind, span, meta);
        self.stack.push(id);
        Ok(id)
    }

    fn push(
        &mut self,
        relation: NodeRelation,
        kind: StructuredNodeKind,
        span: SourceSpan,
        meta: Option<NodeMeta>,
    ) -> NodeId {
        let id = self.nodes.len();
        let parent = self.stack.last().copied();
        let depth = self.stack.len();

        // A node pushed with nothing open is the root of the document that
        // was begun last; making that the builder's job rather than the
        // caller's is what keeps the stream index and the relation in step.
        let relation = match parent {
            None => {
                let (explicit, directives) = self.open_document.take().unwrap_or((false, Vec::new()));
                let document = self.document;
                self.document += 1;
                self.roots.push(StructuredRoot {
                    node: id,
                    explicit,
                    directives,
                });
                NodeRelation::Root { document }
            }
            Some(_) => relation,
        };

        // A document root is not a foldable unit. It is the document, not a
        // unit within it — the same reason a Markdown document as a whole is
        // not a section — and making it one would turn `zM` from "show me the
        // shape of this file" into "show me one brace", since a structured
        // root always wraps everything.
        let fold = (kind.is_container() && parent.is_some()).then(|| {
            let fold = self.fold_nodes.len();
            let parent_fold = parent.and_then(|p| self.nodes.get(p)).and_then(|n| n.fold);
            self.fold_parents.push(parent_fold);
            self.fold_nodes.push(id);
            fold
        });

        if let Some(p) = parent {
            if let Some(node) = self.nodes.get_mut(p) {
                node.child_count += 1;
            }
        }

        self.nodes.push(StructuredNode {
            id,
            parent,
            relation,
            kind,
            child_count: 0,
            // A leaf's subtree is itself; a container's end is rewritten by
            // `close`.
            end: id + 1,
            depth,
            span,
            fold,
            meta: meta.filter(|m| !m.is_empty()).map(Box::new),
        });
        id
    }

    /// Finish the document, building the search index.
    pub fn finish(mut self, source: SourceDocument) -> StructuredDocument {
        self.close_all();
        let search = self.build_search_index();
        StructuredDocument::assemble(
            self.kind,
            source,
            self.nodes,
            self.roots,
            self.comments,
            self.trailing_comments,
            self.fold_parents,
            self.fold_nodes,
            search,
        )
    }

    /// Index every piece of text the reader can see.
    ///
    /// Fields are pushed per node in the order they are rendered — the
    /// comments written above a row, then its key, its anchor, its tag and
    /// its value — so results come out in reading order without a sort. The
    /// one deviation is a trailing same-line comment, which is indexed with
    /// the comments above rather than after the value; it is one field per
    /// node, which is what lets a match offset stay meaningful.
    fn build_search_index(&self) -> crate::document::search::SearchIndex {
        let mut builder = SearchIndexBuilder::with_capacity(self.nodes.len() * 2);
        for node in &self.nodes {
            let comments = comment_field(node, &self.comments);
            if !comments.is_empty() {
                builder.push(node.id, MatchField::Comment, comments);
            }
            if let NodeRelation::MappingEntry { key } = &node.relation {
                builder.push(node.id, MatchField::Label, key.text.clone());
            }
            match (&node.kind, node.anchor()) {
                (StructuredNodeKind::Alias { name }, _) => {
                    builder.push(node.id, MatchField::Anchor, name.clone());
                }
                (_, Some(anchor)) => {
                    builder.push(node.id, MatchField::Anchor, anchor.to_string());
                }
                _ => {}
            }
            if let Some(tag) = node.tag() {
                builder.push(node.id, MatchField::Tag, tag.to_string());
            }
            if let StructuredNodeKind::Scalar(value) = &node.kind {
                builder.push(node.id, MatchField::Value, value.display().to_string());
            }
        }
        builder.build()
    }
}

/// The searchable comment text of a node: the comments above it, then the one
/// beside it, joined by newlines so that offsets stay per rendered line.
pub(super) fn comment_field(node: &StructuredNode, comments: &[Comment]) -> String {
    let mut out = String::new();
    let mut push = |id: CommentId| {
        if let Some(comment) = comments.get(id) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&comment.text);
        }
    };
    for id in node.comments_above() {
        push(*id);
    }
    if let Some(id) = node.comment_right() {
        push(id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::structured::ast::{ScalarKind, StructuredKey};

    fn builder() -> Builder {
        let mut b = Builder::new(DocumentKind::Yaml);
        b.begin_document(false, Vec::new());
        b
    }

    fn key(name: &str) -> NodeRelation {
        NodeRelation::MappingEntry {
            key: StructuredKey::plain(name, SourceSpan::default()),
        }
    }

    fn any() -> NodeRelation {
        NodeRelation::SequenceItem { index: 0 }
    }

    #[test]
    fn the_first_node_of_a_document_becomes_its_root_whatever_it_was_pushed_as() {
        let mut b = builder();
        // Deliberately the wrong relation: the builder must overrule it.
        b.open_mapping(key("ignored"), SourceSpan::default(), None)
            .unwrap();
        b.scalar(
            key("a"),
            ScalarValue::plain("1", ScalarKind::Number),
            SourceSpan::default(),
            None,
        );
        b.close();
        let d = b.finish(SourceDocument::new("t", ""));
        assert_eq!(
            d.node(0).unwrap().relation,
            NodeRelation::Root { document: 0 }
        );
        assert_eq!(d.roots().len(), 1);
        assert_eq!(d.roots()[0].node, 0);
    }

    #[test]
    fn several_documents_are_numbered_in_stream_order() {
        let mut b = Builder::new(DocumentKind::Yaml);
        for n in 0..3 {
            b.begin_document(true, vec![Directive::Version { major: 1, minor: 2 }]);
            b.open_mapping(any(), SourceSpan::default(), None).unwrap();
            b.scalar(
                key("kind"),
                ScalarValue::string(format!("k{n}")),
                SourceSpan::default(),
                None,
            );
            b.close();
        }
        let d = b.finish(SourceDocument::new("t", ""));
        assert_eq!(d.roots().len(), 3);
        for (n, root) in d.roots().iter().enumerate() {
            assert_eq!(
                d.node(root.node).unwrap().relation,
                NodeRelation::Root { document: n }
            );
            assert!(root.explicit);
            assert_eq!(root.directives.len(), 1);
        }
        // The path names the document when there is more than one.
        let second_child = d.first_child(d.roots()[1].node).unwrap();
        assert_eq!(d.path(second_child).breadcrumb(false), "Document 2 > kind");
    }

    #[test]
    fn nesting_past_the_limit_is_an_error_not_a_crash() {
        let mut b = builder();
        let mut opened = 0usize;
        let err = loop {
            match b.open_sequence(any(), SourceSpan::default(), None) {
                Ok(_) => opened += 1,
                Err(e) => break e,
            }
            assert!(opened <= MAX_DEPTH + 1, "the limit must stop the loop");
        };
        assert_eq!(err.limit, MAX_DEPTH);
        assert_eq!(opened, MAX_DEPTH);
        // The partial document is still consistent.
        let d = b.finish(SourceDocument::new("t", ""));
        assert_eq!(d.node_count(), MAX_DEPTH);
        assert_eq!(d.node(0).unwrap().end, MAX_DEPTH);
    }

    #[test]
    fn unclosed_containers_are_closed_by_finish() {
        let mut b = builder();
        b.open_mapping(any(), SourceSpan::default(), None).unwrap();
        b.open_sequence(key("a"), SourceSpan::default(), None)
            .unwrap();
        b.scalar(
            any(),
            ScalarValue::string("x"),
            SourceSpan::default(),
            None,
        );
        let d = b.finish(SourceDocument::new("t", ""));
        assert_eq!(d.node(0).unwrap().end, 3);
        assert_eq!(d.node(1).unwrap().end, 3);
        assert_eq!(d.children(1), [2]);
    }

    #[test]
    fn every_visible_piece_of_text_is_searchable() {
        let mut b = builder();
        b.open_mapping(any(), SourceSpan::default(), None).unwrap();
        let above = b.comment(" why this value", SourceSpan::default());
        let beside = b.comment(" minimum for HA", SourceSpan::default());
        let id = b.scalar(
            key("replicas"),
            ScalarValue::plain("3", ScalarKind::Number),
            SourceSpan::default(),
            Some(NodeMeta {
                anchor: Some("count".into()),
                tag: Some("!!int".into()),
                above: vec![above],
                right: Some(beside),
                merge_key: false,
            }),
        );
        b.close();
        let d = b.finish(SourceDocument::new("t", ""));

        let index = d.search_index();
        for (needle, field) in [
            ("why this", MatchField::Comment),
            ("minimum", MatchField::Comment),
            ("replicas", MatchField::Label),
            ("count", MatchField::Anchor),
            ("!!int", MatchField::Tag),
            ("3", MatchField::Value),
        ] {
            let hits = index.find(needle, false);
            assert_eq!(hits.len(), 1, "{needle}");
            assert_eq!(hits[0].node, id, "{needle}");
            assert_eq!(hits[0].field, field, "{needle}");
        }
        // Both comments live in one field, separated by a newline, so a match
        // offset still points at a rendered line.
        let text = index.text(id, MatchField::Comment).unwrap();
        assert_eq!(text, " why this value\n minimum for HA");
    }

    #[test]
    fn an_alias_is_searchable_by_name_and_is_not_expanded() {
        let mut b = builder();
        b.open_mapping(any(), SourceSpan::default(), None).unwrap();
        b.open_mapping(
            key("defaults"),
            SourceSpan::default(),
            Some(NodeMeta {
                anchor: Some("defaults".into()),
                ..NodeMeta::default()
            }),
        )
        .unwrap();
        b.scalar(
            key("retries"),
            ScalarValue::plain("3", ScalarKind::Number),
            SourceSpan::default(),
            None,
        );
        b.close();
        let alias = b.alias(
            key("<<"),
            "defaults",
            SourceSpan::default(),
            Some(NodeMeta {
                merge_key: true,
                ..NodeMeta::default()
            }),
        );
        b.close();
        let d = b.finish(SourceDocument::new("t", ""));
        assert_eq!(d.node_count(), 4, "the anchored subtree is not duplicated");
        assert_eq!(d.node(alias).unwrap().child_count, 0);
        assert!(d.node(alias).unwrap().is_merge_key());
        let hits = d.search_index().find("defaults", false);
        assert_eq!(hits.len(), 3, "the key, the anchor and the alias");
    }

    #[test]
    fn metadata_can_be_attached_after_the_node_was_pushed() {
        let mut b = builder();
        b.open_mapping(any(), SourceSpan::default(), None).unwrap();
        let id = b.scalar(
            key("a"),
            ScalarValue::string("x"),
            SourceSpan::default(),
            None,
        );
        let c = b.comment(" later", SourceSpan::default());
        b.attach_meta(id, |m| m.above.push(c));
        b.close();
        let d = b.finish(SourceDocument::new("t", ""));
        assert_eq!(d.node(id).unwrap().comments_above(), [c]);
        assert_eq!(d.search_index().find("later", false).len(), 1);
    }
}
