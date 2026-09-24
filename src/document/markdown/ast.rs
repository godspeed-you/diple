//! The Markdown AST.
//!
//! Node ids are assigned in pre-order over the whole tree — including nested
//! nodes inside lists, quotes and footnotes — so they are unique, dense
//! (`0..node_count`) and monotonically increasing in source order. That is
//! the same contract the structured model keeps, which is what lets the
//! layout engine, the viewport anchor and the search index name a semantic
//! unit the same way whatever format produced it.

use super::anchors::AnchorIndex;
use super::links::Link;
use super::sections::{Section, SectionId};
use crate::document::folds::FoldId;
use crate::document::source::SourceDocument;

pub use crate::document::source::SourceSpan;
pub use crate::document::NodeId;

pub use crate::document::LinkId;

/// A parsed Markdown document together with its derived navigation data.
#[derive(Debug, Clone, Default)]
pub struct Document {
    /// The source the document was parsed from.
    pub source: SourceDocument,
    /// Top-level block nodes in source order.
    pub nodes: Vec<Node>,
    /// Section hierarchy derived from top-level headings (see `sections.rs`).
    pub sections: Vec<Section>,
    /// Heading anchor → heading node id.
    pub anchors: AnchorIndex,
    /// Footnote definitions in source order.
    pub footnotes: Vec<Footnote>,
    /// Text of the first H1 heading, if any.
    pub title: Option<String>,
    /// All links in document order, indexed by [`LinkId`].
    pub links: Vec<Link>,
    /// For every node id, the id of its top-level ancestor (itself for
    /// top-level nodes). Indexed by `NodeId`.
    pub top_level: Vec<NodeId>,
    /// For every node id, the section it belongs to (if any). Indexed by
    /// `NodeId`; filled in by `sections::build`.
    pub node_section: Vec<Option<SectionId>>,
    /// Full-text index over every node, in document order — built on the
    /// first search rather than at parse time, because a document piped to
    /// another program is never searched and 1.x never paid for it either.
    /// See [`Document::search_index`].
    pub(crate) search: std::sync::OnceLock<crate::document::search::SearchIndex>,
}

/// A block-level node.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Unique pre-order id.
    pub id: NodeId,
    /// Block content.
    pub kind: NodeKind,
    /// Source byte span.
    pub span: SourceSpan,
}

/// Block-level node kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    /// ATX or setext heading.
    Heading(Heading),
    /// Paragraph of inline content.
    Paragraph(Inlines),
    /// Ordered or unordered (possibly task) list.
    List(List),
    /// GFM table.
    Table(Table),
    /// Fenced or indented code block (non-Mermaid).
    CodeBlock(CodeBlock),
    /// Blockquote containing nested blocks.
    Quote(Vec<Node>),
    /// Fenced block with the `mermaid` language.
    Mermaid(MermaidBlock),
    /// Thematic break.
    HorizontalRule,
    /// Paragraph consisting of a single image.
    Image(Image),
    /// `[^label]: ...` footnote definition.
    FootnoteDefinition(Footnote),
    /// Raw HTML block, kept verbatim (rendered as dim text).
    Html(String),
}

/// A heading node.
#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    /// 1..=6.
    pub level: u8,
    /// Inline content.
    pub inlines: Inlines,
    /// Plain-text flattening of `inlines`.
    pub text: String,
    /// De-duplicated GitHub-style slug (or explicit `{#id}` attribute).
    pub anchor: String,
}

/// A list node.
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    /// `true` for `1.`-style lists.
    pub ordered: bool,
    /// Start number of an ordered list.
    pub start: Option<u64>,
    /// Items in order.
    pub items: Vec<ListItem>,
}

/// One list item.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// `Some(done)` for task-list items, `None` otherwise.
    pub checked: Option<bool>,
    /// Nested block content (paragraphs, sub-lists, code, …).
    pub blocks: Vec<Node>,
}

/// Column alignment of a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    /// No explicit alignment.
    #[default]
    None,
    /// `:---`
    Left,
    /// `:---:`
    Center,
    /// `---:`
    Right,
}

/// A GFM table.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// Per-column alignment.
    pub alignments: Vec<Alignment>,
    /// Header cells.
    pub header: Vec<Inlines>,
    /// Body rows; each row has one entry per column (padded if short).
    pub rows: Vec<Vec<Inlines>>,
}

/// A code block.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeBlock {
    /// Fence info string's first word, if any.
    pub language: Option<String>,
    /// Raw code including trailing newline as written.
    pub code: String,
}

/// A Mermaid fenced block.
#[derive(Debug, Clone, PartialEq)]
pub struct MermaidBlock {
    /// Diagram source.
    pub source: String,
}

/// A block-level image.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// Alt text.
    pub alt: String,
    /// Destination URL.
    pub url: String,
    /// Optional title.
    pub title: Option<String>,
}

/// A footnote definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Footnote {
    /// Footnote label (without `^`).
    pub label: String,
    /// Definition body.
    pub blocks: Vec<Node>,
}

/// Inline sequence.
pub type Inlines = Vec<Inline>;

/// Inline node kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    /// Plain text.
    Text(String),
    /// Inline code span.
    Code(String),
    /// Emphasis (`*x*`).
    Emph(Inlines),
    /// Strong emphasis (`**x**`).
    Strong(Inlines),
    /// Strikethrough (`~~x~~`).
    Strike(Inlines),
    /// Link.
    Link {
        /// Link text.
        inlines: Inlines,
        /// Destination.
        url: String,
        /// Optional title.
        title: Option<String>,
        /// Index into [`Document::links`].
        id: LinkId,
    },
    /// Inline image.
    Image {
        /// Alt text.
        alt: String,
        /// Destination.
        url: String,
    },
    /// Soft line break.
    SoftBreak,
    /// Hard line break.
    HardBreak,
    /// Footnote reference `[^label]`.
    FootnoteRef(String),
}

impl Node {
    /// Direct child block nodes (list item blocks, quote children, footnote
    /// body). Empty for leaf blocks.
    pub fn children(&self) -> Vec<&Node> {
        match &self.kind {
            NodeKind::Quote(nodes) => nodes.iter().collect(),
            NodeKind::List(list) => list.items.iter().flat_map(|i| i.blocks.iter()).collect(),
            NodeKind::FootnoteDefinition(f) => f.blocks.iter().collect(),
            _ => Vec::new(),
        }
    }

    /// Pre-order walk over this node and all its descendants.
    pub fn walk(&self) -> Walk<'_> {
        Walk { stack: vec![self] }
    }
}

/// Pre-order iterator over nodes (see [`Document::walk`]).
pub struct Walk<'a> {
    stack: Vec<&'a Node>,
}

impl<'a> Iterator for Walk<'a> {
    type Item = &'a Node;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        let children = node.children();
        self.stack.extend(children.into_iter().rev());
        Some(node)
    }
}

impl Document {
    /// The full-text index over every node, built on first use.
    pub fn search_index(&self) -> &crate::document::search::SearchIndex {
        self.search
            .get_or_init(|| crate::document::markdown::search::build(self))
    }

    /// Pre-order iterator over every node, including nested ones. Ids come out
    /// in increasing order.
    pub fn walk(&self) -> Walk<'_> {
        Walk {
            stack: self.nodes.iter().rev().collect(),
        }
    }

    /// Total number of nodes (top-level and nested).
    pub fn node_count(&self) -> usize {
        self.top_level.len()
    }

    /// Look up any node (top-level or nested) by id.
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        let top = *self.top_level.get(id)?;
        // Top-level ids are in increasing order; binary search for the
        // top-level ancestor, then walk its subtree.
        let idx = self.nodes.binary_search_by_key(&top, |n| n.id).ok()?;
        self.nodes.get(idx)?.walk().find(|n| n.id == id)
    }

    /// Heading node for a section.
    pub fn heading_of(&self, section: SectionId) -> Option<&Heading> {
        let s = self.sections.get(section)?;
        match &self.node(s.heading)?.kind {
            NodeKind::Heading(h) => Some(h),
            _ => None,
        }
    }

    /// Ids of all heading nodes that define sections, in document order.
    pub fn heading_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.sections.iter().map(|s| s.heading)
    }

    /// The fold forest: Markdown's foldable units are its sections, so a
    /// [`FoldId`] *is* a [`SectionId`] and the parent chain is the section
    /// hierarchy.
    pub fn fold_parents(&self) -> Vec<Option<FoldId>> {
        self.sections.iter().map(|s| s.parent).collect()
    }
}

/// Plain-text flattening of an inline sequence (used for heading text,
/// anchors, search and TOC).
pub fn inlines_to_text(inlines: &[Inline]) -> String {
    let mut out = String::new();
    push_inline_text(inlines, &mut out);
    out
}

fn push_inline_text(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(t) | Inline::Code(t) => out.push_str(t),
            Inline::Emph(i) | Inline::Strong(i) | Inline::Strike(i) => push_inline_text(i, out),
            Inline::Link { inlines, .. } => push_inline_text(inlines, out),
            Inline::Image { alt, .. } => out.push_str(alt),
            Inline::SoftBreak | Inline::HardBreak => out.push(' '),
            Inline::FootnoteRef(label) => {
                out.push('[');
                out.push_str(label);
                out.push(']');
            }
        }
    }
}
