//! The structured document model shared by JSON and YAML.
//!
//! # Shape
//!
//! A structured document is a flat [`Vec`] of nodes numbered densely in
//! pre-order, exactly like the Markdown AST's node ids, plus one root per
//! source document (JSON has one; a YAML stream may have several).
//!
//! A node is **a value together with how it is reached from its parent**.
//! `replicas: 3` is one node — relation `MappingEntry { key: "replicas" }`,
//! kind `Scalar(3)` — not a key node and a value node, because a reader
//! selects one row and a spurious "entry" node would be a cursor stop with
//! nothing behind it. `metadata:` followed by a nested mapping is likewise
//! one node: relation `MappingEntry { key: "metadata" }`, kind `Mapping`, and
//! it is that node that folds.
//!
//! # Why the tree is flat
//!
//! Untrusted input decides the nesting depth. A tree of owned `Box`es would
//! recurse on drop; a flat vector cannot. Every traversal here — descendants,
//! ancestors, the outline, the path — is iterative for the same reason.
//! Because the numbering is pre-order and every node knows where its subtree
//! ends, children and siblings are found by arithmetic rather than stored,
//! which also keeps a large document's memory close to its text.
//!
//! # Fidelity
//!
//! The model is a *reader's* model, so it keeps what the source says rather
//! than what the data means: duplicate keys stay duplicated, member order is
//! source order, a YAML alias stays a reference instead of being expanded
//! into a copy of its anchor, a merge key stays a `<<` entry, and tags,
//! anchors, comments and directives all survive.

use crate::document::folds::{FoldId, FoldState};
use crate::document::format::DocumentKind;
use crate::document::path::{DocumentPath, PathSegment};
use crate::document::search::SearchIndex;
use crate::document::source::{SourceDocument, SourceSpan};
use crate::document::NodeId;

/// Deepest nesting a structured document may have.
///
/// Untrusted input decides this number, so there has to be one: a parser
/// reports a normal error past the limit instead of consuming memory until
/// the process dies. It is far above anything a human writes — Kubernetes
/// manifests live around ten — and above what generated documents reach.
pub const MAX_DEPTH: usize = 1024;

/// How a node hangs off its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeRelation {
    /// The root of one source document (0-based index within the stream).
    Root {
        /// Position in the stream; always 0 for JSON.
        document: usize,
    },
    /// A member of a mapping/object.
    MappingEntry {
        /// The key, as written.
        key: StructuredKey,
    },
    /// An item of a sequence/array.
    SequenceItem {
        /// 0-based position.
        index: usize,
    },
}

/// A mapping key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredKey {
    /// The key text, decoded — what is displayed and searched.
    pub text: String,
    /// Where the key is in the source.
    pub span: SourceSpan,
    /// `true` when the key was not a scalar (a YAML explicit or collection
    /// key); `text` is then the key's source form rather than a decoded
    /// scalar, because there is nothing simpler that stays truthful.
    pub complex: bool,
}

impl StructuredKey {
    /// A plain scalar key.
    pub fn plain(text: impl Into<String>, span: SourceSpan) -> Self {
        Self {
            text: text.into(),
            span,
            complex: false,
        }
    }
}

/// The type a scalar resolves to.
///
/// Type is never signalled by colour alone: the renderer distinguishes a
/// string by its quotes and `null`/booleans by their spelling, so the
/// distinction survives `NO_COLOR` and a 16-colour terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarKind {
    /// Text.
    String,
    /// A number, in whatever lexical form the source used.
    Number,
    /// `true` / `false`.
    Boolean,
    /// `null` (`~`, or an empty YAML value).
    Null,
}

/// How a scalar was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ScalarStyle {
    /// Unquoted.
    #[default]
    Plain,
    /// `'single'`.
    SingleQuoted,
    /// `"double"` — also every JSON string.
    DoubleQuoted,
    /// A `|` block: line breaks are content.
    Literal,
    /// A `>` block: lines are folded into paragraphs.
    Folded,
}

impl ScalarStyle {
    /// Whether the style is a YAML block scalar, which renders over several
    /// lines beneath its key.
    pub fn is_block(self) -> bool {
        matches!(self, ScalarStyle::Literal | ScalarStyle::Folded)
    }

    /// The bare source indicator (`|`, `>`) for a block style, without the
    /// indent and chomping indicators that may follow it. A scalar read from
    /// YAML carries the header it was actually written with; this is the
    /// fallback for one that does not, such as a value diple constructs.
    pub fn block_indicator(self) -> Option<&'static str> {
        match self {
            ScalarStyle::Literal => Some("|"),
            ScalarStyle::Folded => Some(">"),
            _ => None,
        }
    }
}

/// A scalar value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarValue {
    /// The decoded content: escapes resolved, block scalars joined with the
    /// newlines they denote.
    pub text: String,
    /// What the value resolves to.
    pub kind: ScalarKind,
    /// How it was written.
    pub style: ScalarStyle,
    /// The lexical source form, when showing it is more truthful than showing
    /// `text`: `1e6` rather than `1000000`, `~` rather than `null`.
    pub source: Option<String>,
    /// The whole block header of a block scalar, as the source wrote it:
    /// `|`, `|+`, `>-`, `|2-`.
    ///
    /// It is kept apart from `source` because the two answer different
    /// questions. `source` is what to show *instead of* `text` — `display()`
    /// returns it, and it is what search indexes as the value — whereas the
    /// header is not a spelling of the value at all: it introduces it, and the
    /// content still renders on the lines beneath. Folding it into `source`
    /// would make the scalar read and search as `|+` and lose its content.
    ///
    /// The chomping indicator is the difference between keeping and stripping
    /// the trailing newlines and the indent indicator the difference between
    /// content and indentation, so dropping either would have diple show a
    /// document that says something its source does not (spec §10.10).
    pub block_header: Option<String>,
}

impl ScalarValue {
    /// A scalar of `kind` written plainly.
    pub fn plain(text: impl Into<String>, kind: ScalarKind) -> Self {
        Self {
            text: text.into(),
            kind,
            style: ScalarStyle::Plain,
            source: None,
            block_header: None,
        }
    }

    /// A double-quoted string.
    pub fn string(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: ScalarKind::String,
            style: ScalarStyle::DoubleQuoted,
            source: None,
            block_header: None,
        }
    }

    /// The text to display: the lexical source form when one was kept, the
    /// decoded content otherwise.
    pub fn display(&self) -> &str {
        self.source.as_deref().unwrap_or(&self.text)
    }

    /// The header to render where a block scalar's content begins: the one the
    /// source wrote when it is known, the style's bare indicator otherwise.
    pub fn block_indicator(&self) -> Option<&str> {
        match &self.block_header {
            Some(header) => Some(header),
            None => self.style.block_indicator(),
        }
    }
}

/// What a node is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuredNodeKind {
    /// A JSON object or a YAML mapping.
    Mapping,
    /// A JSON array or a YAML sequence.
    Sequence,
    /// A scalar value.
    Scalar(ScalarValue),
    /// A YAML alias (`*name`): a reference to an anchor, never a copy of it.
    Alias {
        /// The anchor name referenced.
        name: String,
    },
}

impl StructuredNodeKind {
    /// Whether this node can be collapsed.
    pub fn is_container(&self) -> bool {
        matches!(
            self,
            StructuredNodeKind::Mapping | StructuredNodeKind::Sequence
        )
    }
}

/// Index into [`StructuredDocument::comments`].
pub type CommentId = usize;

/// A source comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// The comment text without the leading `#`, trimmed of the trailing
    /// newline. Shown and searched as written.
    pub text: String,
    /// Where it is in the source.
    pub span: SourceSpan,
}

/// A YAML directive, kept so that `%YAML` and `%TAG` do not disappear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Directive {
    /// `%YAML major.minor`.
    Version {
        /// Major version.
        major: u32,
        /// Minor version.
        minor: u32,
    },
    /// `%TAG handle prefix`.
    Tag {
        /// The handle, e.g. `!e!`.
        handle: String,
        /// The prefix the handle expands to.
        prefix: String,
    },
}

impl Directive {
    /// The directive as it is written in the source.
    pub fn text(&self) -> String {
        match self {
            Directive::Version { major, minor } => format!("%YAML {major}.{minor}"),
            Directive::Tag { handle, prefix } => format!("%TAG {handle} {prefix}"),
        }
    }
}

/// Per-node metadata that only YAML ever produces.
///
/// Boxed and optional: a JSON document never allocates one, which is what
/// keeps a large API response's memory close to its text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NodeMeta {
    /// An anchor defined on this node (`&name`).
    pub anchor: Option<String>,
    /// An explicit tag, in source form (`!!timestamp`, `!MyType`).
    pub tag: Option<String>,
    /// Comments written above the node, in source order.
    pub above: Vec<CommentId>,
    /// A comment written after the node on the same line.
    pub right: Option<CommentId>,
    /// Whether the entry's key is YAML's merge key `<<`.
    ///
    /// Recorded so the renderer can mark it; diple never performs the merge,
    /// because it reads a document rather than resolving a configuration.
    pub merge_key: bool,
}

impl NodeMeta {
    /// Whether anything is recorded (an empty meta is not stored).
    pub fn is_empty(&self) -> bool {
        self.anchor.is_none()
            && self.tag.is_none()
            && self.above.is_empty()
            && self.right.is_none()
            && !self.merge_key
    }
}

/// One node of the structured model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredNode {
    /// Own id (== index in [`StructuredDocument::nodes`]).
    pub id: NodeId,
    /// Enclosing node; `None` for a root.
    pub parent: Option<NodeId>,
    /// How it hangs off its parent.
    pub relation: NodeRelation,
    /// What it is.
    pub kind: StructuredNodeKind,
    /// Number of direct children.
    pub child_count: usize,
    /// Exclusive end of this node's subtree in pre-order: every id in
    /// `id + 1 .. end` is a descendant.
    pub end: NodeId,
    /// Nesting depth; a root is 0.
    pub depth: usize,
    /// Where the node's value is in the source.
    pub span: SourceSpan,
    /// The fold target this node *is*, for a container.
    pub fold: Option<FoldId>,
    /// YAML-only metadata, absent when there is none.
    pub meta: Option<Box<NodeMeta>>,
}

impl StructuredNode {
    /// Whether this node can be collapsed.
    pub fn is_container(&self) -> bool {
        self.kind.is_container()
    }

    /// The first child, if any.
    pub fn first_child(&self) -> Option<NodeId> {
        (self.child_count > 0).then_some(self.id + 1)
    }

    /// The key this node hangs off, if it is a mapping entry.
    pub fn key(&self) -> Option<&StructuredKey> {
        match &self.relation {
            NodeRelation::MappingEntry { key } => Some(key),
            _ => None,
        }
    }

    /// The sequence index this node hangs off, if it is a sequence item.
    pub fn index(&self) -> Option<usize> {
        match &self.relation {
            NodeRelation::SequenceItem { index } => Some(*index),
            _ => None,
        }
    }

    /// The scalar value, if this node is one.
    pub fn scalar(&self) -> Option<&ScalarValue> {
        match &self.kind {
            StructuredNodeKind::Scalar(value) => Some(value),
            _ => None,
        }
    }

    /// The anchor defined here (`&name`).
    pub fn anchor(&self) -> Option<&str> {
        self.meta.as_ref().and_then(|m| m.anchor.as_deref())
    }

    /// The explicit tag, in source form.
    pub fn tag(&self) -> Option<&str> {
        self.meta.as_ref().and_then(|m| m.tag.as_deref())
    }

    /// Comments written above this node.
    pub fn comments_above(&self) -> &[CommentId] {
        self.meta.as_ref().map_or(&[], |m| &m.above)
    }

    /// The comment written after this node on the same line.
    pub fn comment_right(&self) -> Option<CommentId> {
        self.meta.as_ref().and_then(|m| m.right)
    }

    /// Whether the entry's key is YAML's merge key `<<`.
    pub fn is_merge_key(&self) -> bool {
        self.meta.as_ref().is_some_and(|m| m.merge_key)
    }
}

/// One source document within a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredRoot {
    /// The root node.
    pub node: NodeId,
    /// Whether the source wrote an explicit `---`.
    pub explicit: bool,
    /// Directives that precede this document.
    pub directives: Vec<Directive>,
}

/// A parsed JSON or YAML document.
#[derive(Debug, Clone)]
pub struct StructuredDocument {
    kind: DocumentKind,
    source: SourceDocument,
    nodes: Vec<StructuredNode>,
    roots: Vec<StructuredRoot>,
    comments: Vec<Comment>,
    trailing_comments: Vec<CommentId>,
    fold_parents: Vec<Option<FoldId>>,
    fold_nodes: Vec<NodeId>,
    search: SearchIndex,
}

impl StructuredDocument {
    /// Build a document from the pieces a parser produced.
    ///
    /// `nodes` must already be numbered densely in pre-order with `end`,
    /// `depth`, `child_count` and `fold` filled in; [`super::Builder`] is what
    /// guarantees that, and is the only intended way in.
    ///
    /// The argument list is long because a parsed document genuinely has this
    /// many independent parts; bundling them into a struct would only move the
    /// same nine fields one call further out, since [`super::Builder`] is the
    /// single caller and already owns each of them separately.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn assemble(
        kind: DocumentKind,
        source: SourceDocument,
        nodes: Vec<StructuredNode>,
        roots: Vec<StructuredRoot>,
        comments: Vec<Comment>,
        trailing_comments: Vec<CommentId>,
        fold_parents: Vec<Option<FoldId>>,
        fold_nodes: Vec<NodeId>,
        search: SearchIndex,
    ) -> Self {
        Self {
            kind,
            source,
            nodes,
            roots,
            comments,
            trailing_comments,
            fold_parents,
            fold_nodes,
            search,
        }
    }

    /// Which format this document came from.
    pub fn kind(&self) -> DocumentKind {
        self.kind
    }

    /// The source text the document was parsed from.
    pub fn source(&self) -> &SourceDocument {
        &self.source
    }

    /// Every node, in pre-order.
    pub fn nodes(&self) -> &[StructuredNode] {
        &self.nodes
    }

    /// A node by id.
    pub fn node(&self, id: NodeId) -> Option<&StructuredNode> {
        self.nodes.get(id)
    }

    /// Number of nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the document has no nodes at all (an empty source).
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The roots, in stream order.
    pub fn roots(&self) -> &[StructuredRoot] {
        &self.roots
    }

    /// Every comment in the source, in source order.
    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    /// A comment by id.
    pub fn comment(&self, id: CommentId) -> Option<&Comment> {
        self.comments.get(id)
    }

    /// Comments that follow the last node in the source.
    pub fn trailing_comments(&self) -> &[CommentId] {
        &self.trailing_comments
    }

    /// The search index over keys, values, comments, tags and anchors.
    pub fn search_index(&self) -> &SearchIndex {
        &self.search
    }

    /// Number of foldable containers.
    pub fn fold_count(&self) -> usize {
        self.fold_nodes.len()
    }

    /// The parent of every fold target, in target order.
    pub fn fold_parents(&self) -> &[Option<FoldId>] {
        &self.fold_parents
    }

    /// The node a fold target stands for.
    pub fn fold_node(&self, fold: FoldId) -> Option<NodeId> {
        self.fold_nodes.get(fold).copied()
    }

    /// The direct children of a node, in source order.
    ///
    /// Derived from the pre-order numbering rather than stored: a child's
    /// subtree ends where the next child begins.
    pub fn children(&self, id: NodeId) -> Vec<NodeId> {
        let Some(node) = self.node(id) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(node.child_count);
        let mut cur = id + 1;
        while cur < node.end {
            out.push(cur);
            match self.node(cur) {
                Some(child) if child.end > cur => cur = child.end,
                _ => break,
            }
        }
        out
    }

    /// The parent of a node.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id)?.parent
    }

    /// The first child of a node.
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.node(id)?.first_child()
    }

    /// The next sibling of a node, within the same parent.
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        let node = self.node(id)?;
        let limit = match node.parent {
            Some(parent) => self.node(parent)?.end,
            None => self.nodes.len(),
        };
        (node.end < limit).then_some(node.end)
    }

    /// The previous sibling of a node, within the same parent.
    pub fn previous_sibling(&self, id: NodeId) -> Option<NodeId> {
        let node = self.node(id)?;
        let first = match node.parent {
            Some(parent) => parent + 1,
            None => *self.roots.first().map(|r| &r.node)?,
        };
        if id <= first {
            return None;
        }
        let mut cur = first;
        let mut previous = None;
        while cur < id {
            previous = Some(cur);
            match self.node(cur) {
                Some(child) if child.end > cur => cur = child.end,
                _ => break,
            }
        }
        previous
    }

    /// The fold target that governs a node: the node itself when it is a
    /// container, otherwise its nearest container ancestor.
    pub fn fold_at(&self, id: NodeId) -> Option<FoldId> {
        let node = self.node(id)?;
        if let Some(fold) = node.fold {
            return Some(fold);
        }
        self.enclosing_fold(id)
    }

    /// The fold target of the nearest *ancestor* container.
    pub fn enclosing_fold(&self, id: NodeId) -> Option<FoldId> {
        let mut cur = self.node(id)?.parent;
        while let Some(p) = cur {
            let node = self.node(p)?;
            if let Some(fold) = node.fold {
                return Some(fold);
            }
            cur = node.parent;
        }
        None
    }

    /// Whether a node is hidden by the fold state.
    ///
    /// A collapsed container keeps its own row — that row is what the reader
    /// expands again — and hides everything below it.
    pub fn is_hidden(&self, id: NodeId, folds: &FoldState) -> bool {
        let Some(node) = self.node(id) else {
            return false;
        };
        match node.fold {
            Some(fold) => folds.ancestor_collapsed(fold),
            None => self
                .enclosing_fold(id)
                .is_some_and(|fold| folds.content_hidden(fold)),
        }
    }

    /// Expand every collapsed ancestor of `id` so that its row is visible.
    pub fn reveal(&self, id: NodeId, folds: &mut FoldState) {
        if let Some(fold) = self.fold_at(id) {
            folds.reveal(fold);
        }
    }

    /// The semantic path to a node, root first.
    ///
    /// Walks ancestors iteratively — the depth is decided by untrusted input.
    pub fn path(&self, id: NodeId) -> DocumentPath {
        let mut segments: Vec<PathSegment> = Vec::new();
        let mut cur = Some(id);
        let mut guard = 0usize;
        while let Some(node_id) = cur {
            let Some(node) = self.node(node_id) else {
                break;
            };
            match &node.relation {
                NodeRelation::Root { document } => {
                    // A single-document stream needs no document segment; a
                    // stream with several does, or two `spec` paths would be
                    // indistinguishable.
                    if self.roots.len() > 1 {
                        segments.push(PathSegment::Document(document + 1));
                    }
                }
                NodeRelation::MappingEntry { key } => {
                    segments.push(PathSegment::Key(key.text.clone()));
                }
                NodeRelation::SequenceItem { index } => {
                    segments.push(PathSegment::Index(*index));
                }
            }
            cur = node.parent;
            guard += 1;
            if guard > self.nodes.len() {
                break;
            }
        }
        segments.reverse();
        DocumentPath::new(segments)
    }

    /// The first node a cursor should sit on.
    ///
    /// The first entry of the first root rather than the root itself: a
    /// reader opening a Kubernetes manifest wants to be on `apiVersion`, not
    /// on the anonymous mapping that contains it.
    pub fn first_semantic(&self) -> Option<NodeId> {
        let root = self.roots.first()?.node;
        self.first_child(root).or(Some(root))
    }

    /// Whether a document root is shown as a row of its own.
    ///
    /// JSON always shows its root brace, because that is how JSON is written.
    /// A YAML stream shows a `--- Document N` row only when there is more than
    /// one document to tell apart; a single-document stream's entries start at
    /// the left margin, the way the file does (spec §5.2).
    ///
    /// The question lives here rather than in the layout engine because it
    /// decides more than one thing: a root that is not shown has no row, and a
    /// node with no row is not a place the cursor can be.
    pub fn shows_root_row(&self) -> bool {
        self.kind == DocumentKind::Json || self.roots.len() > 1
    }

    /// Whether a node contributes a row of its own to the rendered document.
    ///
    /// Everything does except the elided root of a single-document YAML
    /// stream — and even that one does when it is a bare scalar, because a
    /// document that is only `42` still has to be readable.
    pub fn has_row(&self, id: NodeId) -> bool {
        let Some(node) = self.node(id) else {
            return false;
        };
        node.parent.is_some() || self.shows_root_row() || !node.kind.is_container()
    }

    /// The label shown for a node's key or index, without its value.
    pub fn label(&self, id: NodeId) -> String {
        let Some(node) = self.node(id) else {
            return String::new();
        };
        match &node.relation {
            NodeRelation::Root { document } => {
                if self.roots.len() > 1 {
                    format!("Document {}", document + 1)
                } else {
                    "root".to_string()
                }
            }
            NodeRelation::MappingEntry { key } => key.text.clone(),
            NodeRelation::SequenceItem { index } => format!("[{index}]"),
        }
    }

    /// How many members a container has, phrased for a collapsed summary.
    ///
    /// The noun follows the format: JSON objects have members, YAML mappings
    /// have entries, and both have items in a list.
    pub fn collapsed_summary(&self, id: NodeId) -> Option<String> {
        let node = self.node(id)?;
        let n = node.child_count;
        let text = match (&node.kind, self.kind) {
            (StructuredNodeKind::Mapping, DocumentKind::Json) => {
                format!("{{{n} {}}}", plural(n, "member", "members"))
            }
            (StructuredNodeKind::Mapping, _) => {
                format!("{{{n} {}}}", plural(n, "entry", "entries"))
            }
            (StructuredNodeKind::Sequence, _) => {
                format!("[{n} {}]", plural(n, "item", "items"))
            }
            _ => return None,
        };
        Some(text)
    }
}

/// `one` for exactly one, `many` otherwise.
fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::structured::Builder;

    /// ```text
    /// root                      0
    ///   a: {                    1
    ///     b: 1                  2
    ///     c: [                  3
    ///       [0]: 2              4
    ///     ]
    ///   }
    ///   d: 3                    5
    /// ```
    fn doc() -> StructuredDocument {
        let mut b = Builder::new(DocumentKind::Json);
        b.begin_document(true, Vec::new());
        b.open_mapping(root(), SourceSpan::default(), None).unwrap();
        b.open_mapping(key("a"), SourceSpan::default(), None)
            .unwrap();
        b.scalar(
            key("b"),
            ScalarValue::plain("1", ScalarKind::Number),
            SourceSpan::default(),
            None,
        );
        b.open_sequence(key("c"), SourceSpan::default(), None)
            .unwrap();
        b.scalar(
            NodeRelation::SequenceItem { index: 0 },
            ScalarValue::plain("2", ScalarKind::Number),
            SourceSpan::default(),
            None,
        );
        b.close();
        b.close();
        b.scalar(
            key("d"),
            ScalarValue::plain("3", ScalarKind::Number),
            SourceSpan::default(),
            None,
        );
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

    #[test]
    fn pre_order_numbering_makes_the_family_arithmetic() {
        let d = doc();
        assert_eq!(d.node_count(), 6);
        assert_eq!(d.children(0), [1, 5]);
        assert_eq!(d.children(1), [2, 3]);
        assert_eq!(d.children(3), [4]);
        assert_eq!(d.children(2), Vec::<NodeId>::new());
        assert_eq!(d.parent(4), Some(3));
        assert_eq!(d.parent(0), None);
        assert_eq!(d.first_child(1), Some(2));
        assert_eq!(d.first_child(2), None);
        assert_eq!(d.next_sibling(2), Some(3));
        assert_eq!(d.next_sibling(3), None, "last child of `a`");
        assert_eq!(d.next_sibling(1), Some(5));
        assert_eq!(d.previous_sibling(5), Some(1));
        assert_eq!(d.previous_sibling(3), Some(2));
        assert_eq!(d.previous_sibling(2), None);
        assert_eq!(d.node(1).unwrap().end, 5, "subtree of `a` is 1..5");
        assert_eq!(d.node(1).unwrap().depth, 1);
        assert_eq!(d.node(4).unwrap().depth, 3);
    }

    #[test]
    fn folds_cover_the_containers_and_nothing_else() {
        let d = doc();
        assert_eq!(d.fold_count(), 2, "`a` and `c`");
        assert_eq!(
            d.node(0).unwrap().fold,
            None,
            "a root is the document, not a unit within it"
        );
        assert_eq!(d.node(1).unwrap().fold, Some(0));
        assert_eq!(d.node(2).unwrap().fold, None, "a scalar does not fold");
        assert_eq!(d.node(3).unwrap().fold, Some(1));
        assert_eq!(d.fold_at(2), Some(0), "a scalar folds with its container");
        assert_eq!(d.fold_at(4), Some(1));
        assert_eq!(
            d.fold_at(5),
            None,
            "a top-level scalar entry has nothing to fold"
        );
        assert_eq!(d.fold_parents(), [None, Some(0)]);
        assert_eq!(d.fold_node(0), Some(1));
        assert_eq!(d.fold_node(1), Some(3));
        assert_eq!(d.fold_node(2), None, "the ids are dense");
    }

    #[test]
    fn a_collapsed_container_keeps_its_own_row_and_hides_the_rest() {
        let d = doc();
        let mut folds = FoldState::from_parents(d.fold_parents().to_vec());
        folds.collapse(0); // `a`
        assert!(!d.is_hidden(0, &folds), "the root is never hidden");
        assert!(!d.is_hidden(1, &folds), "the container's own row stays");
        assert!(d.is_hidden(2, &folds));
        assert!(d.is_hidden(3, &folds), "a nested container is hidden");
        assert!(d.is_hidden(4, &folds));
        assert!(!d.is_hidden(5, &folds), "a sibling is untouched");

        folds.expand_all();
        folds.collapse(1); // `c`
        assert!(!d.is_hidden(3, &folds));
        assert!(d.is_hidden(4, &folds));
    }

    #[test]
    fn a_childs_fold_state_survives_its_parents_collapse_and_expand() {
        let d = doc();
        let mut folds = FoldState::from_parents(d.fold_parents().to_vec());
        folds.collapse(1); // `c`
        folds.collapse(0); // `a`, which encloses it
        assert!(d.is_hidden(3, &folds), "`c` is inside a collapsed `a`");

        folds.expand(0);
        assert!(!d.is_hidden(3, &folds), "`c` has its row back");
        assert!(
            folds.is_collapsed(1) && d.is_hidden(4, &folds),
            "and is still collapsed itself"
        );
    }

    #[test]
    fn collapse_all_shows_the_shape_of_the_file_not_one_brace() {
        let d = doc();
        let mut folds = FoldState::from_parents(d.fold_parents().to_vec());
        folds.collapse_all();
        // The root is not a fold target, so `zM` leaves the top level legible.
        assert!(!d.is_hidden(0, &folds));
        assert!(!d.is_hidden(1, &folds), "`a` keeps its summary row");
        assert!(!d.is_hidden(5, &folds), "`d` is not inside any fold");
        assert!(d.is_hidden(2, &folds) && d.is_hidden(3, &folds) && d.is_hidden(4, &folds));

        folds.expand_all();
        assert!(
            (0..d.node_count()).all(|id| !d.is_hidden(id, &folds)),
            "`zR` shows everything again"
        );
    }

    #[test]
    fn a_search_match_is_revealed_through_every_collapsed_ancestor() {
        let d = doc();
        let mut folds = FoldState::from_parents(d.fold_parents().to_vec());
        folds.collapse_all();
        let hits = d.search_index().find("2", false);
        let hit = hits.first().expect("the value of `c[0]`");
        assert_eq!(hit.node, 4);
        assert!(d.is_hidden(hit.node, &folds));

        d.reveal(hit.node, &mut folds);
        assert!(!d.is_hidden(hit.node, &folds));
        assert!(!folds.is_collapsed(0) && !folds.is_collapsed(1));
    }

    #[test]
    fn an_empty_container_keeps_its_own_row_and_an_honest_summary() {
        let mut b = Builder::new(DocumentKind::Json);
        b.begin_document(true, Vec::new());
        b.open_mapping(root(), SourceSpan::default(), None).unwrap();
        b.open_mapping(key("empty"), SourceSpan::default(), None)
            .unwrap();
        b.close();
        b.open_sequence(key("none"), SourceSpan::default(), None)
            .unwrap();
        b.close();
        b.close();
        let d = b.finish(SourceDocument::new("t", ""));

        assert_eq!(d.node_count(), 3);
        assert_eq!(d.children(0), [1, 2]);
        assert_eq!(d.children(1), Vec::<NodeId>::new());
        assert_eq!(d.node(1).unwrap().end, 2, "an empty subtree is the node");
        assert_eq!(d.collapsed_summary(1).as_deref(), Some("{0 members}"));
        assert_eq!(d.collapsed_summary(2).as_deref(), Some("[0 items]"));

        // Empty or not, a container is a fold target; collapsing it hides
        // nothing because there is nothing under it.
        assert_eq!(d.fold_count(), 2);
        let mut folds = FoldState::from_parents(d.fold_parents().to_vec());
        folds.collapse_all();
        assert!(!d.is_hidden(1, &folds) && !d.is_hidden(2, &folds));
    }

    #[test]
    fn a_document_that_is_only_a_scalar_is_still_a_document() {
        for kind in [DocumentKind::Json, DocumentKind::Yaml] {
            let mut b = Builder::new(kind);
            b.begin_document(true, Vec::new());
            b.scalar(
                root(),
                ScalarValue::plain("42", ScalarKind::Number),
                SourceSpan::default(),
                None,
            );
            let d = b.finish(SourceDocument::new("t", ""));

            assert_eq!(d.node_count(), 1);
            assert_eq!(d.fold_count(), 0, "there is nothing to fold");
            assert_eq!(
                d.first_semantic(),
                Some(0),
                "the cursor has exactly one place to be"
            );
            assert_eq!(d.fold_at(0), None);
            assert!(!d.is_hidden(0, &FoldState::from_parents(Vec::new())));
            assert_eq!(d.path(0).breadcrumb(false), "");
            assert!(
                d.has_row(0),
                "a scalar root is shown even where a mapping root is elided"
            );
        }
    }

    #[test]
    fn a_root_row_is_shown_for_json_and_for_a_stream_but_not_for_one_yaml_document() {
        let json = doc();
        assert!(json.shows_root_row() && json.has_row(0));

        let mut b = Builder::new(DocumentKind::Yaml);
        b.begin_document(false, Vec::new());
        b.open_mapping(root(), SourceSpan::default(), None).unwrap();
        b.scalar(
            key("a"),
            ScalarValue::plain("1", ScalarKind::Number),
            SourceSpan::default(),
            None,
        );
        b.close();
        let one = b.finish(SourceDocument::new("t", ""));
        assert!(!one.shows_root_row());
        assert!(!one.has_row(0), "spec §5.2: no root row, no cursor stop");
        assert!(one.has_row(1));
        assert_eq!(one.first_semantic(), Some(1));

        let mut b = Builder::new(DocumentKind::Yaml);
        for _ in 0..2 {
            b.begin_document(true, Vec::new());
            b.open_mapping(root(), SourceSpan::default(), None).unwrap();
            b.scalar(
                key("a"),
                ScalarValue::plain("1", ScalarKind::Number),
                SourceSpan::default(),
                None,
            );
            b.close();
        }
        let stream = b.finish(SourceDocument::new("t", ""));
        assert!(stream.shows_root_row(), "`--- Document N` tells them apart");
        assert!(stream.has_row(0) && stream.has_row(2));
    }

    #[test]
    fn a_deeply_nested_document_is_walked_without_recursion() {
        // One mapping root plus `MAX_DEPTH - 1` nested sequences: the deepest
        // structure a parser is allowed to hand over.
        let mut b = Builder::new(DocumentKind::Json);
        b.begin_document(true, Vec::new());
        b.open_mapping(root(), SourceSpan::default(), None).unwrap();
        for _ in 1..MAX_DEPTH {
            b.open_sequence(
                NodeRelation::SequenceItem { index: 0 },
                SourceSpan::default(),
                None,
            )
            .unwrap();
        }
        let d = b.finish(SourceDocument::new("t", ""));

        let deepest = d.node_count() - 1;
        assert_eq!(d.node_count(), MAX_DEPTH);
        assert_eq!(d.node(deepest).unwrap().depth, MAX_DEPTH - 1);
        assert_eq!(d.fold_count(), MAX_DEPTH - 1, "every node but the root");

        // Each of these walks the whole chain; none of them may recurse.
        assert_eq!(
            d.path(deepest).canonical().matches('/').count(),
            MAX_DEPTH - 1
        );
        assert_eq!(d.fold_at(deepest), Some(MAX_DEPTH - 2));
        assert_eq!(d.enclosing_fold(deepest), Some(MAX_DEPTH - 3));

        let mut folds = FoldState::from_parents(d.fold_parents().to_vec());
        folds.collapse(0);
        assert!(!d.is_hidden(1, &folds), "the outermost unit keeps its row");
        assert!(d.is_hidden(deepest, &folds));

        folds.collapse_all();
        d.reveal(deepest, &mut folds);
        assert!(!d.is_hidden(deepest, &folds));
    }

    #[test]
    fn reveal_opens_only_the_ancestors_of_the_target() {
        let d = doc();
        let mut folds = FoldState::from_parents(d.fold_parents().to_vec());
        folds.collapse_all();
        assert!(d.is_hidden(4, &folds));
        d.reveal(4, &mut folds);
        assert!(!d.is_hidden(4, &folds));
        assert!(!folds.is_collapsed(0) && !folds.is_collapsed(1));
    }

    #[test]
    fn paths_name_keys_and_indices_and_skip_a_lone_document() {
        let d = doc();
        assert_eq!(d.path(4).breadcrumb(false), "a > c > [0]");
        assert_eq!(d.path(4).canonical(), "/a/c/0");
        assert_eq!(d.path(0).breadcrumb(false), "", "the root has no segments");
        assert_eq!(d.path(5).breadcrumb(false), "d");
    }

    #[test]
    fn collapsed_summaries_are_pluralised_per_format() {
        let d = doc();
        assert_eq!(d.collapsed_summary(1).as_deref(), Some("{2 members}"));
        assert_eq!(d.collapsed_summary(3).as_deref(), Some("[1 item]"));
        assert_eq!(d.collapsed_summary(2), None, "a scalar has no summary");
        assert_eq!(d.label(4), "[0]");
        assert_eq!(d.label(1), "a");
        assert_eq!(d.label(0), "root");
    }

    #[test]
    fn the_cursor_starts_on_the_first_entry_not_the_anonymous_root() {
        assert_eq!(doc().first_semantic(), Some(1));
    }

    #[test]
    fn scalar_display_prefers_the_source_spelling() {
        let mut v = ScalarValue::plain("1000000", ScalarKind::Number);
        assert_eq!(v.display(), "1000000");
        v.source = Some("1e6".to_string());
        assert_eq!(v.display(), "1e6", "a reader should not see a rewrite");
        assert_eq!(ScalarValue::string("x").style, ScalarStyle::DoubleQuoted);
        assert!(ScalarStyle::Literal.is_block());
        assert_eq!(ScalarStyle::Folded.block_indicator(), Some(">"));
        assert_eq!(ScalarStyle::Plain.block_indicator(), None);
    }

    #[test]
    fn a_block_scalar_shows_the_header_it_was_written_with() {
        let mut v = ScalarValue::plain("a\n\n", ScalarKind::String);
        v.style = ScalarStyle::Literal;
        assert_eq!(
            v.block_indicator(),
            Some("|"),
            "a value with no remembered header falls back to the bare indicator"
        );
        v.block_header = Some("|+".to_string());
        assert_eq!(v.block_indicator(), Some("|+"));
        assert_eq!(
            v.display(),
            "a\n\n",
            "the header introduces the content; it does not replace it"
        );
    }

    #[test]
    fn metadata_is_absent_until_something_needs_it() {
        let d = doc();
        let node = d.node(2).unwrap();
        assert!(node.meta.is_none(), "JSON allocates no metadata");
        assert_eq!(node.anchor(), None);
        assert_eq!(node.tag(), None);
        assert!(node.comments_above().is_empty());
        assert_eq!(node.comment_right(), None);
        assert!(!node.is_merge_key());
        assert!(NodeMeta::default().is_empty());
    }
}
