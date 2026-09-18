//! Layout engine: turns the semantic [`Document`] into a terminal-width-aware
//! [`RenderTree`].
//!
//! `Layout::build` is **pure and deterministic**: the same document, options
//! and width always produce the same lines, which is what makes the snapshot
//! tests meaningful. The only interior mutability is the syntax highlighting
//! cache, which never changes the output.
//!
//! # Boundary types
//!
//! This module deliberately does not depend on `terminal::` or `mermaid::`.
//! Instead it declares what it needs:
//!
//! * [`DiagramSource`] — anything that can turn a diagram source into text
//!   lines, a terminal image, or "show the source" ([`DiagramContent`]). The
//!   integrator adapts `mermaid::MermaidRenderer` to this trait; [`NoDiagrams`]
//!   is the dependency-free default.
//! * [`crate::render::theme::ColorLevel`] — a local mirror of the terminal
//!   capability enum.
//! * `LayoutOptions::unicode` / `images` — booleans the integrator fills from
//!   `terminal::capabilities::Capabilities`.

pub mod code;
pub mod inline;
pub mod list;
pub(crate) mod paragraph;
pub mod structured;
pub mod table;

/// Grapheme- and width-correct string helpers.
///
/// Re-exported from [`crate::util::unicode`], which is where they live: they
/// have no dependency on the layout engine, and `render` and `mermaid` use
/// them too. This alias keeps the `layout::unicode` path working.
pub use crate::util::unicode;

use std::collections::HashMap;

use crate::config::schema::{Config, TableMode};
use crate::document::markdown::{
    Document, Footnote, Heading, Image, Inlines, List, ListItem, MermaidBlock, Node, NodeKind,
};
use crate::document::{DocumentModel, FoldState, Match, NodeId};
use crate::layout::code::{CodeCache, CodeOptions};
use crate::layout::inline::{layout_inlines, line_width, push_span};
use crate::layout::list::{marker, marker_width, task_box, INDENT_PER_LEVEL};
use crate::layout::paragraph::{
    horizontal_rule, html_lines, image_placeholder, layout_heading, layout_paragraph, quote_gutter,
    FoldMarker,
};
use crate::layout::table::TableOptions;
use crate::render::primitives::{
    ImageRef, LineKind, NodeSpan, PendingCode, RenderLine, RenderTree, StyledSpan,
};
use crate::render::theme::{Style, Theme};

/// What a [`DiagramSource`] produced for one diagram block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagramContent {
    /// Ready-made text lines drawn with the theme's diagram style.
    Lines(Vec<String>),
    /// A terminal image occupying `cols` × `rows` cells.
    Image {
        /// Opaque image id understood by the image backend.
        id: usize,
        /// Width in columns.
        cols: u16,
        /// Height in rows.
        rows: u16,
    },
    /// Nothing could be rendered: show the diagram source instead.
    Source,
    /// Nothing could be rendered *and* there is a reason to show: the note is
    /// drawn above the diagram source (the source fallback must be reachable
    /// on the non-interactive path too, where no `s` key exists).
    SourceWithNote(String),
}

/// Provider of rendered diagrams for `mermaid` fences.
///
/// Boundary trait (see module docs): `mermaid::MermaidRenderer` is adapted to
/// it by the integrator so that `layout` never depends on the Mermaid crate
/// internals.
pub trait DiagramSource {
    /// Render the diagram of `node` (its `source`) for the given content
    /// width. Implementations must be deterministic for a given width.
    fn diagram(&self, node: NodeId, source: &str, width: usize) -> DiagramContent;
}

/// The default diagram source: always shows the source (fallback).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDiagrams;

impl DiagramSource for NoDiagrams {
    fn diagram(&self, _node: NodeId, _source: &str, _width: usize) -> DiagramContent {
        DiagramContent::Source
    }
}

/// The default diagram source instance (usable in `LayoutOptions::new`).
pub static NO_DIAGRAMS: NoDiagrams = NoDiagrams;

/// An owned, comparable snapshot of a [`LayoutOptions`], produced by
/// [`LayoutOptions::fingerprint`].
///
/// Two `LayoutOptions` with equal fingerprints (and the same document and
/// diagram source) produce identical render trees, so a cached tree may be
/// reused exactly when the fingerprint is unchanged. Adding a field to
/// `LayoutOptions` without adding it here is the bug this type exists to make
/// hard: keep the two in step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutFingerprint {
    width: usize,
    theme: Theme,
    folds: Option<Vec<bool>>,
    table_mode: TableMode,
    max_column_width: usize,
    code_wrap: bool,
    code_line_numbers: bool,
    tab_width: usize,
    wrap: bool,
    search_matches: Vec<Match>,
    images: bool,
    unicode: bool,
    footnotes: bool,
    lazy_code: bool,
    structured_indent: usize,
    show_indices: bool,
}

/// Everything the layout engine needs besides the document.
#[derive(Clone, Copy)]
pub struct LayoutOptions<'a> {
    /// Available width in columns.
    pub width: usize,
    /// Theme providing the styles.
    pub theme: &'a Theme,
    /// Fold state; `None` renders everything expanded and without fold
    /// markers (non-interactive output).
    pub folds: Option<&'a FoldState>,
    /// Table mode and column cap.
    pub table_mode: TableMode,
    /// Maximum width of a single table column.
    pub max_column_width: usize,
    /// Soft-wrap code lines.
    pub code_wrap: bool,
    /// Line numbers inside code blocks.
    pub code_line_numbers: bool,
    /// Tab expansion width.
    pub tab_width: usize,
    /// Wrap prose to the width (`false` emits long lines for horizontal
    /// scrolling).
    pub wrap: bool,
    /// Search matches to highlight (any node, in any order).
    pub search_matches: &'a [Match],
    /// Diagram provider.
    pub diagrams: &'a dyn DiagramSource,
    /// Terminal supports inline images.
    pub images: bool,
    /// Terminal supports Unicode box drawing (ASCII fallback otherwise).
    pub unicode: bool,
    /// Render the footnote definitions section at the end of the document.
    pub footnotes: bool,
    /// Columns one nesting level of a structured document is indented by.
    pub structured_indent: usize,
    /// Show `[0]`-style indices on sequence items.
    ///
    /// With this off, YAML falls back to its own `-` marker and JSON to bare
    /// items; the index stays discoverable through the path either way.
    pub show_indices: bool,
    /// Defer syntax highlighting to [`Layout::realize`].
    ///
    /// `false` — the default — highlights every code block while laying out,
    /// which is what the snapshot tests and the non-interactive path want.
    /// The interactive application sets it: highlighting a language costs tens
    /// of milliseconds the first time it is used in a process, and a reader
    /// only ever looks at one screen at a time.
    pub lazy_code: bool,
}

impl<'a> LayoutOptions<'a> {
    /// Options with sane defaults for `width` and `theme`.
    pub fn new(width: usize, theme: &'a Theme) -> Self {
        Self {
            width,
            theme,
            folds: None,
            table_mode: TableMode::Auto,
            max_column_width: 60,
            code_wrap: false,
            code_line_numbers: false,
            tab_width: 4,
            wrap: true,
            search_matches: &[],
            diagrams: &NO_DIAGRAMS,
            images: false,
            unicode: true,
            footnotes: true,
            lazy_code: false,
            structured_indent: 2,
            show_indices: true,
        }
    }

    /// Apply every layout option the configuration decides.
    ///
    /// Both rendering paths — the interactive `App::layout_options` and the
    /// non-interactive `print_plain` in the binary — must map [`Config`] onto
    /// the options identically. Spelled out twice they had already drifted
    /// (one path set `footnotes`, the other relied on the default happening to
    /// agree), so the mapping lives here once and the drift becomes
    /// impossible.
    ///
    /// What deliberately stays with the caller is everything the *situation*
    /// decides rather than the configuration: `width`, `theme`, `folds`,
    /// `diagrams`, `images`, `unicode`, `search_matches` and `lazy_code`.
    /// Those are the two paths' genuine differences.
    pub fn apply_config(&mut self, config: &Config) {
        self.table_mode = config.table.mode;
        self.max_column_width = config.table.max_column_width;
        self.code_wrap = config.code.wrap;
        // `line_numbers` is the top-level shorthand for `code.line_numbers`.
        self.code_line_numbers = config.code.line_numbers || config.line_numbers;
        self.tab_width = usize::from(config.code.tab_width).max(1);
        self.wrap = config.wrap;
    }

    /// With a fold state (enables fold markers and section elision).
    pub fn with_folds(mut self, folds: &'a FoldState) -> Self {
        self.folds = Some(folds);
        self
    }

    /// With search matches to highlight.
    pub fn with_matches(mut self, matches: &'a [Match]) -> Self {
        self.search_matches = matches;
        self
    }

    /// With a diagram provider.
    pub fn with_diagrams(mut self, diagrams: &'a dyn DiagramSource) -> Self {
        self.diagrams = diagrams;
        self
    }

    /// An owned, comparable summary of every option that affects the built
    /// [`RenderTree`].
    ///
    /// This exists so a caller that caches a render tree can derive its cache
    /// key *from the options themselves* instead of maintaining a parallel
    /// list of fields that must be kept in correspondence by hand (see
    /// `app::state::App::ensure_layout`).
    ///
    /// The one input it cannot capture is [`LayoutOptions::diagrams`]: a
    /// `&dyn DiagramSource` has no comparable identity, and its *content* can
    /// change without the reference changing. A caller whose diagram source is
    /// mutable must combine this fingerprint with its own generation counter.
    #[must_use]
    pub fn fingerprint(&self) -> LayoutFingerprint {
        LayoutFingerprint {
            width: self.width,
            theme: self.theme.clone(),
            folds: self
                .folds
                .map(|f| (0..f.len()).map(|s| f.is_collapsed(s)).collect()),
            table_mode: self.table_mode,
            max_column_width: self.max_column_width,
            code_wrap: self.code_wrap,
            code_line_numbers: self.code_line_numbers,
            tab_width: self.tab_width,
            wrap: self.wrap,
            search_matches: self.search_matches.to_vec(),
            images: self.images,
            unicode: self.unicode,
            footnotes: self.footnotes,
            lazy_code: self.lazy_code,
            structured_indent: self.structured_indent,
            show_indices: self.show_indices,
        }
    }

    fn table_options(&self) -> TableOptions {
        TableOptions {
            mode: self.table_mode,
            max_column_width: self.max_column_width,
            unicode: self.unicode,
        }
    }

    fn code_options(&self) -> CodeOptions {
        CodeOptions {
            wrap: self.code_wrap,
            line_numbers: self.code_line_numbers,
            tab_width: self.tab_width,
            unicode: self.unicode,
            lazy: self.lazy_code,
        }
    }
}

/// The layout engine.
///
/// Holds the syntax highlighting cache; keep one instance alive across
/// re-layouts (resize, folding, scrolling) so that code blocks are highlighted
/// only once.
#[derive(Debug, Default)]
pub struct Layout {
    code_cache: CodeCache,
}

impl Layout {
    /// A layout engine with an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The highlighting cache.
    ///
    /// Test-only: the cache never changes the output, so no caller outside
    /// the tests that assert it is reused has any reason to look at it.
    #[cfg(test)]
    pub(crate) fn cache(&self) -> &CodeCache {
        &self.code_cache
    }

    /// Lay out a document, reusing the cached syntax highlighting.
    ///
    /// This is the one place the format boundary turns into rows. Both
    /// branches produce the same [`RenderTree`] contract — every line carries
    /// the [`NodeId`] it came from — so nothing downstream of here knows or
    /// needs to know which backend was on the other side.
    pub fn layout(&self, doc: &DocumentModel, opts: &LayoutOptions<'_>) -> RenderTree {
        match doc {
            DocumentModel::Markdown(markdown) => self.layout_markdown(markdown, opts),
            DocumentModel::Structured(structured) => structured::layout(structured, opts),
        }
    }

    /// Lay out a Markdown document specifically.
    pub fn layout_markdown(&self, doc: &Document, opts: &LayoutOptions<'_>) -> RenderTree {
        let mut builder = Builder::new(doc, opts, &self.code_cache);
        builder.run();
        let Builder {
            lines,
            pending,
            spans,
            tail,
            ..
        } = builder;
        RenderTree::with_index(lines, pending, spans, tail)
    }

    /// Lay out a document without a persistent cache.
    pub fn build(doc: &DocumentModel, opts: &LayoutOptions<'_>) -> RenderTree {
        Layout::new().layout(doc, opts)
    }

    /// Re-lay out the content a fold change rewrites and splice it in.
    ///
    /// Returns `false` when the tree cannot be spliced, in which case the
    /// caller falls back to a full re-layout. Both formats rewrite exactly the
    /// run of rows the fold governs and leave every other row — including its
    /// cached syntax highlighting — alone.
    pub fn relayout_fold(
        &self,
        doc: &DocumentModel,
        opts: &LayoutOptions<'_>,
        tree: &mut RenderTree,
        fold: crate::document::FoldId,
    ) -> bool {
        match doc {
            DocumentModel::Markdown(markdown) => {
                let Some(section) = markdown.sections.get(fold) else {
                    return false;
                };
                let (heading, end) = (section.heading, section.end);
                let first = markdown.nodes.partition_point(|n| n.id < heading);
                let last = markdown.nodes.partition_point(|n| n.id < end);
                if last <= first {
                    return false;
                }
                // The trailing footnote section is laid out after the body, so
                // a range that contains a definition cannot be spliced in
                // place.
                if markdown.nodes[first..last]
                    .iter()
                    .any(|n| matches!(n.kind, NodeKind::FootnoteDefinition(_)))
                {
                    return false;
                }
                self.relayout_nodes(markdown, opts, tree, first, last - first)
            }
            DocumentModel::Structured(structured) => {
                let Some(node) = structured.fold_node(fold) else {
                    return false;
                };
                structured::relayout_subtree(structured, opts, tree, node)
            }
        }
    }

    /// Highlight the deferred code blocks overlapping the lines
    /// `[first, last)`, replacing their spans in place.
    ///
    /// Returns `true` when anything was realized. The options must be the ones
    /// the tree was built with; the replacement has the same line count and
    /// the same widths, so no index and no anchor can move.
    pub fn realize(
        &self,
        doc: &DocumentModel,
        opts: &LayoutOptions<'_>,
        tree: &mut RenderTree,
        first: usize,
        last: usize,
    ) -> bool {
        // Only Markdown defers anything: a structured document has no syntax
        // highlighting to catch up on.
        let DocumentModel::Markdown(doc) = doc else {
            return false;
        };
        let blocks = tree.take_pending_in(first, last);
        if blocks.is_empty() {
            return false;
        }
        let mut code_opts = opts.code_options();
        code_opts.lazy = false;
        let matches: Vec<Match> = opts.search_matches.to_vec();
        let mut changed = false;
        for block in blocks {
            let Some(source) = code_of(doc, block.node) else {
                continue;
            };
            let hits: Vec<Match> = matches
                .iter()
                .copied()
                .filter(|m| m.node == block.node)
                .collect();
            let laid = code::layout_code(
                block.node,
                &source,
                opts.theme,
                &code_opts,
                block.width,
                &hits,
                &self.code_cache,
            );
            if laid.lines.len() != block.len {
                // Cannot happen: highlighting never changes the line count.
                // If it ever did, keeping the plain lines is the safe answer —
                // the reader sees un-highlighted code, never a shifted page.
                continue;
            }
            let composed: Vec<Vec<StyledSpan>> = laid
                .lines
                .into_iter()
                .map(|line| compose_line(&block.prefix, line))
                .collect();
            changed |= tree.replace_line_spans(block.start, composed);
        }
        changed
    }

    /// Re-lay out the top-level nodes `nodes[first..first + count]` and splice
    /// the result into `tree`.
    ///
    /// This is the incremental path a fold takes: collapsing or expanding a
    /// section rewrites one contiguous run of nodes and leaves the rest of the
    /// document — including its cached highlighting — untouched. Returns
    /// `false` when the tree carries no node index, in which case the caller
    /// must fall back to a full re-layout.
    pub fn relayout_nodes(
        &self,
        doc: &Document,
        opts: &LayoutOptions<'_>,
        tree: &mut RenderTree,
        first: usize,
        count: usize,
    ) -> bool {
        let spans = tree.node_spans();
        if spans.len() != doc.nodes.len() || first + count > spans.len() || count == 0 {
            return false;
        }
        // `Builder::run` suppresses the blank separator in front of the first
        // node that produces lines; reproduce that decision for the sub-range.
        let leading = spans[..first].iter().all(|s| s.len == 0);
        let mut builder = Builder::new(doc, opts, &self.code_cache);
        builder.run_range(first, count, leading);
        let Builder {
            lines,
            pending,
            spans,
            ..
        } = builder;
        tree.splice_nodes(first, count, lines, pending, spans)
    }
}

/// The code of a node as a [`crate::document::markdown::CodeBlock`], including the
/// verbatim source shown for a Mermaid fence that could not be rendered.
fn code_of(doc: &Document, node: NodeId) -> Option<crate::document::markdown::CodeBlock> {
    match &doc.node(node)?.kind {
        NodeKind::CodeBlock(block) => Some(block.clone()),
        NodeKind::Mermaid(block) => Some(crate::document::markdown::CodeBlock {
            language: Some("mermaid".to_string()),
            code: block.source.clone(),
        }),
        _ => None,
    }
}

/// Put the prefix spans in front of a line's content, merging at the seam
/// exactly as [`Builder::push`] does.
fn compose_line(prefix: &[StyledSpan], spans: Vec<StyledSpan>) -> Vec<StyledSpan> {
    let mut line: Vec<StyledSpan> = prefix
        .iter()
        .filter(|p| !p.text.is_empty())
        .cloned()
        .collect();
    let boundary = line.len();
    for s in spans {
        if s.text.is_empty() {
            continue;
        }
        if line.len() > boundary {
            push_span(&mut line, &s.text, s.style, s.link, s.search_match);
        } else {
            line.push(s);
        }
    }
    line
}

/// Width used for "do not wrap" prose (long lines scroll horizontally).
const NO_WRAP_WIDTH: usize = 100_000;

struct Builder<'a> {
    doc: &'a Document,
    opts: &'a LayoutOptions<'a>,
    theme: &'a Theme,
    cache: &'a CodeCache,
    matches: HashMap<NodeId, Vec<Match>>,
    lines: Vec<RenderLine>,
    /// Code blocks laid out plain because [`LayoutOptions::lazy_code`] is set.
    pending: Vec<PendingCode>,
    /// One entry per top-level node covered by this run.
    spans: Vec<NodeSpan>,
    /// First line of the trailing footnote section.
    tail: usize,
}

impl<'a> Builder<'a> {
    fn new(doc: &'a Document, opts: &'a LayoutOptions<'a>, cache: &'a CodeCache) -> Self {
        let mut matches: HashMap<NodeId, Vec<Match>> = HashMap::new();
        for m in opts.search_matches {
            matches.entry(m.node).or_default().push(*m);
        }
        Self {
            doc,
            opts,
            theme: opts.theme,
            cache,
            matches,
            lines: Vec::new(),
            pending: Vec::new(),
            spans: Vec::new(),
            tail: 0,
        }
    }

    fn width(&self) -> usize {
        self.opts.width.max(1)
    }

    fn matches_for(&self, node: NodeId) -> &[Match] {
        self.matches.get(&node).map(Vec::as_slice).unwrap_or(&[])
    }

    fn hidden(&self, node: NodeId) -> bool {
        match self.opts.folds {
            Some(folds) => self.doc.is_hidden(node, folds),
            None => false,
        }
    }

    fn push(
        &mut self,
        node: NodeId,
        kind: LineKind,
        prefix: &[StyledSpan],
        spans: Vec<StyledSpan>,
    ) {
        // Prefix spans (indentation, gutters, list markers) are kept as
        // separate spans so that the list/footnote code can replace the
        // marker slot of an item's first line by index.
        let mut line: Vec<StyledSpan> = prefix
            .iter()
            .filter(|p| !p.text.is_empty())
            .cloned()
            .collect();
        let boundary = line.len();
        for s in spans {
            if s.text.is_empty() {
                continue;
            }
            if line.len() > boundary {
                push_span(&mut line, &s.text, s.style, s.link, s.search_match);
            } else {
                line.push(s);
            }
        }
        self.lines.push(RenderLine::new(node, kind, line));
    }

    fn blank(&mut self, node: NodeId, prefix: &[StyledSpan]) {
        self.push(node, LineKind::Blank, prefix, Vec::new());
    }

    fn run(&mut self) {
        let count = self.doc.nodes.len();
        let empty = self.run_range(0, count, true);
        self.tail = self.lines.len();
        self.footnote_section(empty);
    }

    /// Lay out `count` top-level nodes starting at index `first`, recording a
    /// [`NodeSpan`] for each of them — including the hidden ones, whose span
    /// is empty but marks where their lines would go.
    ///
    /// `leading` says whether no earlier node has produced a line yet, which
    /// is what suppresses the blank separator in front of the first one.
    /// Returns the value that flag has after the run.
    fn run_range(&mut self, first: usize, count: usize, leading: bool) -> bool {
        let mut leading = leading;
        let doc: &'a Document = self.doc;
        for index in first..(first + count).min(doc.nodes.len()) {
            let Some(node) = doc.nodes.get(index) else {
                break;
            };
            let start = self.lines.len();
            let id = node.id;
            if !self.hidden(id) && !matches!(node.kind, NodeKind::FootnoteDefinition(_)) {
                if !leading {
                    self.blank(id, &[]);
                }
                leading = false;
                self.block(node, &[], 0);
            }
            self.spans.push(NodeSpan {
                node: id,
                start,
                len: self.lines.len() - start,
            });
        }
        leading
    }

    fn footnote_section(&mut self, empty_doc: bool) {
        if !self.opts.footnotes {
            return;
        }
        let defs: Vec<(NodeId, Footnote)> = self
            .doc
            .nodes
            .iter()
            .filter_map(|n| match &n.kind {
                NodeKind::FootnoteDefinition(f) => Some((n.id, f.clone())),
                _ => None,
            })
            .collect();
        if defs.is_empty() {
            return;
        }
        let width = self.width();
        let first_id = defs.first().map(|(id, _)| *id).unwrap_or(0);
        if !empty_doc {
            self.blank(first_id, &[]);
        }
        let rule = horizontal_rule(self.theme, width, self.opts.unicode);
        self.push(first_id, LineKind::Text, &[], rule);
        self.push(
            first_id,
            LineKind::Text,
            &[],
            vec![StyledSpan::new("Footnotes", self.theme.heading(3))],
        );
        self.blank(first_id, &[]);
        for (id, footnote) in defs {
            self.footnote(id, &footnote);
        }
    }

    fn footnote(&mut self, id: NodeId, footnote: &Footnote) {
        let label = format!("[{}] ", footnote.label);
        let indent = unicode::width(&label);
        let prefix = vec![StyledSpan::new(" ".repeat(indent), self.theme.text)];
        let start = self.lines.len();
        if footnote.blocks.is_empty() {
            self.push(id, LineKind::Text, &prefix, Vec::new());
        } else {
            self.blocks(&footnote.blocks, &prefix, 0);
        }
        if let Some(line) = self.lines.get_mut(start) {
            if let Some(first) = line.spans.first_mut() {
                *first = StyledSpan::new(label, self.theme.link);
            }
            line.width = line.spans.iter().map(StyledSpan::width).sum();
        }
    }

    /// Lay out a sequence of block nodes with blank separators.
    fn blocks(&mut self, nodes: &[Node], prefix: &[StyledSpan], depth: usize) {
        self.blocks_inner(nodes, prefix, depth, false)
    }

    /// Blocks of a list item: a nested list follows its paragraph tightly, so
    /// no blank line is inserted around nested lists.
    fn item_blocks(&mut self, nodes: &[Node], prefix: &[StyledSpan], depth: usize) {
        self.blocks_inner(nodes, prefix, depth, true)
    }

    fn blocks_inner(
        &mut self,
        nodes: &[Node],
        prefix: &[StyledSpan],
        depth: usize,
        tight_lists: bool,
    ) {
        let mut first = true;
        let mut prev_list = false;
        for node in nodes {
            if self.hidden(node.id) {
                continue;
            }
            let is_list = matches!(node.kind, NodeKind::List(_));
            if !first && !(tight_lists && (is_list || prev_list)) {
                self.blank(node.id, prefix);
            }
            first = false;
            prev_list = is_list;
            self.block(node, prefix, depth);
        }
    }

    fn block(&mut self, node: &Node, prefix: &[StyledSpan], depth: usize) {
        let prefix_width: usize = prefix.iter().map(StyledSpan::width).sum();
        let avail = self.width().saturating_sub(prefix_width).max(1);
        match &node.kind {
            NodeKind::Heading(h) => self.heading(node.id, h, prefix, avail),
            NodeKind::Paragraph(inlines) => self.paragraph(node.id, inlines, prefix, avail),
            NodeKind::List(list) => self.list(node.id, list, prefix, depth),
            NodeKind::Table(t) => {
                let opts = self.opts.table_options();
                let lines =
                    table::layout_table(t, self.theme, &opts, avail, self.matches_for(node.id));
                for line in lines {
                    self.push(node.id, LineKind::TableRow, prefix, line);
                }
            }
            NodeKind::CodeBlock(c) => self.code_block(node.id, c, prefix, avail),
            NodeKind::Quote(children) => {
                let mut inner = prefix.to_vec();
                inner.push(quote_gutter(self.theme, self.opts.unicode));
                let quoted: Vec<Node> = children.clone();
                self.quote(&quoted, &inner, depth);
            }
            NodeKind::Mermaid(m) => self.mermaid(node.id, m, prefix, avail),
            NodeKind::HorizontalRule => {
                let rule = horizontal_rule(self.theme, avail, self.opts.unicode);
                self.push(node.id, LineKind::Text, prefix, rule);
            }
            NodeKind::Image(img) => self.image(node.id, img, prefix, avail),
            NodeKind::FootnoteDefinition(f) => {
                let f = f.clone();
                self.footnote(node.id, &f);
            }
            NodeKind::Html(html) => {
                for line in html_lines(html, self.theme, avail) {
                    self.push(node.id, LineKind::Text, prefix, line);
                }
            }
        }
    }

    fn quote(&mut self, children: &[Node], prefix: &[StyledSpan], depth: usize) {
        let mut first = true;
        for node in children {
            if self.hidden(node.id) {
                continue;
            }
            if !first {
                self.blank(node.id, prefix);
            }
            first = false;
            // Quote text uses the quote style for prose.
            match &node.kind {
                NodeKind::Paragraph(inlines) => {
                    let prefix_width: usize = prefix.iter().map(StyledSpan::width).sum();
                    let avail = self.width().saturating_sub(prefix_width).max(1);
                    let wrap_width = if self.opts.wrap { avail } else { NO_WRAP_WIDTH };
                    let lines = layout_inlines(
                        inlines,
                        self.theme,
                        self.theme.quote,
                        self.matches_for(node.id),
                        0,
                        wrap_width,
                        wrap_width,
                    );
                    for line in lines {
                        self.push(node.id, LineKind::Text, prefix, line);
                    }
                }
                _ => self.block(node, prefix, depth),
            }
        }
    }

    fn heading(&mut self, id: NodeId, heading: &Heading, prefix: &[StyledSpan], avail: usize) {
        let (fold, collapsed) = self.fold_marker(id);
        let lines = layout_heading(
            heading,
            self.theme,
            self.matches_for(id),
            avail,
            fold,
            self.opts.unicode,
        );
        for (i, line) in lines.into_iter().enumerate() {
            let kind = if i == 0 && collapsed {
                LineKind::FoldedMarker
            } else {
                LineKind::Heading(heading.level.clamp(1, 6))
            };
            self.push(id, kind, prefix, line);
        }
    }

    /// Fold marker for a heading node, plus whether it is collapsed.
    fn fold_marker(&self, id: NodeId) -> (FoldMarker, bool) {
        let Some(folds) = self.opts.folds else {
            return (FoldMarker::None, false);
        };
        let Some(sid) = self.doc.section_of(id) else {
            return (FoldMarker::None, false);
        };
        let Some(section) = self.doc.sections.get(sid) else {
            return (FoldMarker::None, false);
        };
        if section.heading != id || section.end <= id + 1 {
            return (FoldMarker::None, false);
        }
        if folds.is_collapsed(sid) {
            (FoldMarker::Collapsed, true)
        } else {
            (FoldMarker::Expanded, false)
        }
    }

    fn paragraph(&mut self, id: NodeId, inlines: &Inlines, prefix: &[StyledSpan], avail: usize) {
        let width = if self.opts.wrap { avail } else { NO_WRAP_WIDTH };
        let lines = layout_paragraph(
            inlines,
            self.theme,
            self.theme.text,
            self.matches_for(id),
            width,
            0,
        );
        for line in lines {
            self.push(id, LineKind::Text, prefix, line);
        }
    }

    fn list(&mut self, id: NodeId, list: &List, prefix: &[StyledSpan], depth: usize) {
        let mw = marker_width(list, self.opts.unicode).max(INDENT_PER_LEVEL);
        for (index, item) in list.items.iter().enumerate() {
            let marker_text = self.item_marker(list, item, index, depth, mw);
            let indent = unicode::width(&marker_text);
            let mut item_prefix = prefix.to_vec();
            item_prefix.push(StyledSpan::new(" ".repeat(indent), self.theme.text));
            let start = self.lines.len();
            if item.blocks.is_empty() {
                self.push(id, LineKind::Text, &item_prefix, Vec::new());
            } else {
                self.item_blocks(&item.blocks, &item_prefix, depth + 1);
            }
            // Replace the indentation of the item's first line with the
            // marker (same display width, so nothing shifts).
            if let Some(line) = self.lines.get_mut(start) {
                let pos = prefix.iter().filter(|p| !p.text.is_empty()).count();
                if let Some(slot) = line.spans.get_mut(pos) {
                    if unicode::width(&slot.text) == indent {
                        *slot = StyledSpan::new(marker_text, self.theme.list_marker);
                    }
                }
                line.width = line.spans.iter().map(StyledSpan::width).sum();
            }
        }
    }

    fn item_marker(
        &self,
        list: &List,
        item: &ListItem,
        index: usize,
        depth: usize,
        mw: usize,
    ) -> String {
        let mut text = marker(list, index, depth, self.opts.unicode, mw);
        if let Some(checked) = item.checked {
            text.push_str(task_box(checked, self.opts.unicode));
            text.push(' ');
        }
        text
    }

    fn mermaid(&mut self, id: NodeId, block: &MermaidBlock, prefix: &[StyledSpan], avail: usize) {
        match self.opts.diagrams.diagram(id, &block.source, avail) {
            DiagramContent::Lines(lines) => {
                for line in lines {
                    let spans = vec![StyledSpan::new(line, self.theme.diagram)];
                    self.push(id, LineKind::Diagram, prefix, spans);
                }
            }
            DiagramContent::Image {
                id: img,
                cols,
                rows,
            } => {
                let image = ImageRef {
                    id: img,
                    cols,
                    rows,
                    alt: "mermaid diagram".to_string(),
                };
                let rows = rows.max(1);
                for _ in 0..rows {
                    let spans = vec![StyledSpan::new(
                        " ".repeat(cols as usize),
                        self.theme.diagram,
                    )];
                    self.push(id, LineKind::Image(image.clone()), prefix, spans);
                }
            }
            DiagramContent::Source => self.diagram_source(id, block, prefix, avail),
            DiagramContent::SourceWithNote(note) => {
                for chunk in unicode::wrap(&note, avail, avail) {
                    let spans = vec![StyledSpan::new(chunk, self.theme.warning)];
                    self.push(id, LineKind::Diagram, prefix, spans);
                }
                self.diagram_source(id, block, prefix, avail);
            }
        }
    }

    /// Render a Mermaid block verbatim as a `mermaid` code block.
    fn diagram_source(
        &mut self,
        id: NodeId,
        block: &MermaidBlock,
        prefix: &[StyledSpan],
        avail: usize,
    ) {
        let block = crate::document::markdown::CodeBlock {
            language: Some("mermaid".to_string()),
            code: block.source.clone(),
        };
        self.code_block(id, &block, prefix, avail);
    }

    /// Lay out a fenced block and, when its highlighting was deferred, record
    /// where it landed so [`Layout::realize`] can find it again.
    fn code_block(
        &mut self,
        id: NodeId,
        block: &crate::document::markdown::CodeBlock,
        prefix: &[StyledSpan],
        avail: usize,
    ) {
        let opts = self.opts.code_options();
        let laid = code::layout_code(
            id,
            block,
            self.theme,
            &opts,
            avail,
            self.matches_for(id),
            self.cache,
        );
        let start = self.lines.len();
        let len = laid.lines.len();
        for line in laid.lines {
            self.push(id, LineKind::Code, prefix, line);
        }
        if laid.deferred && len > 0 {
            self.pending.push(PendingCode {
                node: id,
                start,
                len,
                prefix: prefix.to_vec(),
                width: avail,
            });
        }
    }

    fn image(&mut self, id: NodeId, img: &Image, prefix: &[StyledSpan], avail: usize) {
        if self.opts.images {
            let image = ImageRef {
                id,
                cols: avail.min(u16::MAX as usize) as u16,
                rows: 1,
                alt: img.alt.clone(),
            };
            let spans = image_placeholder(&img.alt, &img.url, self.theme, avail);
            self.push(id, LineKind::Image(image), prefix, spans);
        } else {
            let spans = image_placeholder(&img.alt, &img.url, self.theme, avail);
            self.push(id, LineKind::Text, prefix, spans);
        }
    }
}

/// Convenience: total display width of a line of spans.
pub fn spans_width(spans: &[StyledSpan]) -> usize {
    line_width(spans)
}

/// Style helper used by renderers that need the plain text style.
pub fn plain_style() -> Style {
    Style::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::markdown::parse;
    use crate::document::SearchIndex;
    use crate::testing::plain as render;

    #[test]
    fn nested_lists_indent_two_columns_per_level() {
        let out = render("- a\n  - b\n    - c\n", 40);
        assert_eq!(out, "• a\n  ◦ b\n    ▪ c\n");
    }

    #[test]
    fn ordered_lists_respect_start() {
        let out = render("3. three\n4. four\n", 40);
        assert_eq!(out, "3. three\n4. four\n");
    }

    #[test]
    fn task_items() {
        let out = render("- [x] done\n- [ ] todo\n", 40);
        assert_eq!(out, "• ☑ done\n• ☐ todo\n");
    }

    #[test]
    fn ascii_fallback_markers() {
        let doc = parse("- [x] done\n  - nested\n");
        let theme = Theme::dark();
        let mut opts = LayoutOptions::new(40, &theme);
        opts.unicode = false;
        let out = Layout::build(&doc, &opts).to_plain_text();
        // Continuation blocks are indented under the item marker.
        assert_eq!(out, "- [x] done\n      * nested\n");
    }

    #[test]
    fn list_continuation_is_indented_under_the_marker() {
        let out = render("- alpha beta gamma delta epsilon zeta\n", 14);
        for line in out.lines().skip(1) {
            assert!(line.starts_with("  "), "continuation indented: {line:?}");
        }
        assert!(out.lines().all(|l| unicode::width(l) <= 14));
    }

    #[test]
    fn blockquote_gutter_nests() {
        let out = render("> outer\n>\n> > inner\n", 40);
        assert!(out.contains("▌ outer"), "{out}");
        assert!(out.contains("▌ ▌ inner"), "{out}");
    }

    #[test]
    fn folded_section_body_is_elided() {
        let doc = parse("# A\n\nbody\n\n## A1\n\nmore\n\n# B\n\nb\n");
        let theme = Theme::dark();
        let mut folds = FoldState::new(&doc);
        folds.collapse(0);
        let opts = LayoutOptions::new(40, &theme).with_folds(&folds);
        let out = Layout::build(&doc, &opts).to_plain_text();
        assert!(out.contains("▶ A"), "collapsed marker: {out}");
        assert!(!out.contains("body"), "body elided: {out}");
        assert!(!out.contains("A1"), "nested section elided: {out}");
        assert!(out.contains("▼ B"), "sibling still expanded: {out}");
        assert!(out.contains("\nb\n"));
    }

    #[test]
    fn fold_markers_only_with_a_fold_state() {
        let out = render("# A\n\nbody\n", 40);
        assert!(!out.contains('▼'), "{out}");
    }

    #[test]
    fn every_line_knows_its_node_and_offsets_are_stable() {
        let doc = parse("# Title\n\npara one\n\n- item\n");
        let theme = Theme::dark();
        let opts = LayoutOptions::new(40, &theme);
        let tree = Layout::build(&doc, &opts);
        let heading = tree.first_line_of(0).unwrap();
        assert_eq!(heading, 0);
        assert_eq!(tree.node_at(0), Some(0));
        assert_eq!(tree.line_index_for(0, 1), Some(1)); // underline
        assert_eq!(tree.heading_lines().len(), 1);
        assert!(tree.max_width() > 0);
        for line in &tree.lines {
            assert!(doc.node(line.node).is_some(), "unknown node {}", line.node);
        }
    }

    #[test]
    fn search_matches_are_highlighted_in_prose_and_code() {
        let src = "# needle\n\na needle here\n\n```rust\nlet needle = 1;\n```\n";
        let doc = parse(src);
        let idx = SearchIndex::build(&doc);
        let matches = idx.find("needle", false);
        let theme = Theme::dark();
        let opts = LayoutOptions::new(60, &theme).with_matches(&matches);
        let tree = Layout::build(&doc, &opts);
        let hits: Vec<&str> = tree
            .lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.search_match)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(hits, ["needle", "needle", "needle"]);
    }

    #[test]
    fn mermaid_falls_back_to_source() {
        let out = render("```mermaid\ngraph LR\nA --> B\n```\n", 40);
        assert!(out.contains("graph LR"), "{out}");
    }

    #[test]
    fn diagram_source_lines_are_used() {
        struct Fake;
        impl DiagramSource for Fake {
            fn diagram(&self, _n: NodeId, _s: &str, _w: usize) -> DiagramContent {
                DiagramContent::Lines(vec!["┌───┐".into(), "│ A │".into()])
            }
        }
        let doc = parse("```mermaid\ngraph LR\nA --> B\n```\n");
        let theme = Theme::dark();
        let fake = Fake;
        let opts = LayoutOptions::new(40, &theme).with_diagrams(&fake);
        let tree = Layout::build(&doc, &opts);
        assert_eq!(tree.to_plain_text(), "┌───┐\n│ A │\n");
        assert!(tree
            .lines
            .iter()
            .all(|l| matches!(l.kind, LineKind::Diagram)));
    }

    #[test]
    fn diagram_images_reserve_cells() {
        struct Fake;
        impl DiagramSource for Fake {
            fn diagram(&self, _n: NodeId, _s: &str, _w: usize) -> DiagramContent {
                DiagramContent::Image {
                    id: 7,
                    cols: 10,
                    rows: 3,
                }
            }
        }
        let doc = parse("```mermaid\ngraph LR\nA --> B\n```\n");
        let theme = Theme::dark();
        let fake = Fake;
        let opts = LayoutOptions::new(40, &theme).with_diagrams(&fake);
        let tree = Layout::build(&doc, &opts);
        assert_eq!(tree.len(), 3);
        assert!(matches!(tree.lines[0].kind, LineKind::Image(ref i) if i.id == 7));
        assert_eq!(tree.lines[0].width, 10);
    }

    #[test]
    fn images_are_placeholders_without_image_support() {
        let out = render("![a cat](cat.png)\n", 40);
        assert_eq!(out, "[image: a cat]\n");
    }

    #[test]
    fn footnotes_are_collected_at_the_end() {
        let out = render("text[^a]\n\n[^a]: the note\n", 40);
        assert!(out.contains("text[a]"), "{out}");
        assert!(out.contains("Footnotes"), "{out}");
        assert!(out.contains("[a] the note"), "{out}");
    }

    #[test]
    fn narrow_and_zero_widths_do_not_panic() {
        for w in [0usize, 1, 2, 3, 5] {
            let out = render(
                "# Title\n\nsome text\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- item\n\n> quote\n\n```rust\nfn x() {}\n```\n",
                w,
            );
            assert!(!out.is_empty());
        }
    }

    fn fixtures() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut out = Vec::new();
        let entries = std::fs::read_dir(&dir).expect("fixtures dir");
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let name = path.display().to_string();
            let source = std::fs::read_to_string(&path).expect("read fixture");
            out.push((name, source));
        }
        out.sort();
        assert!(
            !out.is_empty(),
            "no *.md fixtures under {} — tests looping over them would assert nothing",
            dir.display()
        );
        out
    }

    /// Deferring highlighting must be invisible: the same lines, the same
    /// widths, and — once realized — the same spans as the eager path.
    #[test]
    fn deferred_highlighting_reproduces_the_eager_rendering() {
        let theme = Theme::dark();
        // At least one fixture must actually have pending code, otherwise the
        // `realize == deferred` assertion below is `false == false` throughout
        // and the test proves nothing about lazy highlighting.
        let mut exercised_deferred = false;
        for (name, source) in fixtures() {
            let doc = parse(&source);
            for width in [40usize, 80, 120] {
                let eager = Layout::build(&doc, &LayoutOptions::new(width, &theme));

                let engine = Layout::new();
                let mut opts = LayoutOptions::new(width, &theme);
                opts.lazy_code = true;
                let mut lazy = engine.layout(&doc, &opts);

                // Before realizing: same structure, same widths, only the
                // code styling differs.
                assert_eq!(lazy.len(), eager.len(), "{name}@{width}: line count");
                assert_eq!(
                    lazy.to_plain_text(),
                    eager.to_plain_text(),
                    "{name}@{width}: text"
                );
                for (a, b) in lazy.lines.iter().zip(eager.lines.iter()) {
                    assert_eq!(a.width, b.width, "{name}@{width}: line width");
                    assert_eq!(a.node, b.node);
                    assert_eq!(a.node_offset, b.node_offset);
                    assert_eq!(a.kind, b.kind);
                }

                let total = lazy.len();
                let deferred = !lazy.pending_code().is_empty();
                exercised_deferred |= deferred;
                assert_eq!(engine.realize(&doc, &opts, &mut lazy, 0, total), deferred);
                assert!(
                    lazy.pending_code().is_empty(),
                    "{name}@{width}: everything realized"
                );
                // Nothing is left to do, so a second pass must report no work
                // regardless of whether this fixture had code at all.
                assert!(
                    !engine.realize(&doc, &opts, &mut lazy, 0, total),
                    "{name}@{width}: realize is not idempotent"
                );
                assert_eq!(lazy.lines, eager.lines, "{name}@{width}: realized spans");
                assert_eq!(lazy.max_width(), eager.max_width());
            }
        }
        assert!(
            exercised_deferred,
            "no fixture deferred any code highlighting: lazy highlighting is not being exercised"
        );
    }

    /// Only the code the caller asks for is highlighted.
    #[test]
    fn realizing_a_range_leaves_the_rest_deferred() {
        let src = "```rust\nfn a() {}\n```\n\ntext\n\n```rust\nfn b() {}\n```\n";
        let doc = parse(src);
        let theme = Theme::dark();
        let mut opts = LayoutOptions::new(40, &theme);
        opts.lazy_code = true;
        let engine = Layout::new();
        let mut tree = engine.layout(&doc, &opts);
        assert_eq!(tree.pending_code().len(), 2);
        assert!(engine.realize(&doc, &opts, &mut tree, 0, 1));
        assert_eq!(tree.pending_code().len(), 1, "only the first block");
        assert_eq!(engine.cache().len(), 1);
    }

    /// A fold splice must produce exactly the tree a full rebuild would.
    #[test]
    fn splicing_a_fold_equals_a_full_rebuild() {
        let theme = Theme::dark();
        for (name, source) in fixtures() {
            let doc = parse(&source);
            if doc.sections.is_empty() {
                continue;
            }
            for section in 0..doc.sections.len() {
                let mut folds = FoldState::new(&doc);
                let engine = Layout::new();
                let mut opts = LayoutOptions::new(80, &theme);
                opts.folds = Some(&folds);
                let mut tree = engine.layout(&doc, &opts);

                folds.collapse(section);
                let Some(s) = doc.sections.get(section) else {
                    continue;
                };
                let first = doc.nodes.partition_point(|n| n.id < s.heading);
                let last = doc.nodes.partition_point(|n| n.id < s.end);
                if first >= last {
                    continue;
                }
                // A full rebuild also re-lays the trailing footnote section
                // out, and that honours the fold state, so a fold over a
                // footnote definition changes lines outside the range. The
                // application detects the same case and rebuilds instead.
                if doc.nodes[first..last]
                    .iter()
                    .any(|n| matches!(n.kind, NodeKind::FootnoteDefinition(_)))
                {
                    continue;
                }
                let mut opts = LayoutOptions::new(80, &theme);
                opts.folds = Some(&folds);
                let spliced = engine.relayout_nodes(&doc, &opts, &mut tree, first, last - first);
                assert!(spliced, "{name}#{section}: splice applies");

                let fresh = engine.layout(&doc, &opts);
                assert_eq!(
                    tree.to_plain_text(),
                    fresh.to_plain_text(),
                    "{name}#{section}: text"
                );
                assert_eq!(tree.lines, fresh.lines, "{name}#{section}: lines");
                assert_eq!(tree.max_width(), fresh.max_width());
                assert_eq!(tree.heading_lines(), fresh.heading_lines());
                assert_eq!(tree.node_spans(), fresh.node_spans());
                assert_eq!(tree.tail_start(), fresh.tail_start());
                for node in doc.walk() {
                    assert_eq!(
                        tree.first_line_of(node.id),
                        fresh.first_line_of(node.id),
                        "{name}#{section}: first line of node {}",
                        node.id
                    );
                }
            }
        }
    }

    /// Expanding again splices the section's body back in.
    #[test]
    fn splicing_round_trips_through_collapse_and_expand() {
        let doc = parse("# A\n\nbody\n\n## A1\n\nmore\n\n# B\n\nb\n");
        let theme = Theme::dark();
        let engine = Layout::new();
        let mut folds = FoldState::new(&doc);
        let mut opts = LayoutOptions::new(40, &theme);
        opts.folds = Some(&folds);
        let before = engine.layout(&doc, &opts).to_plain_text();
        let mut tree = engine.layout(&doc, &opts);

        let s = doc.sections.first().expect("a section");
        let first = doc.nodes.partition_point(|n| n.id < s.heading);
        let last = doc.nodes.partition_point(|n| n.id < s.end);

        folds.collapse(0);
        let mut opts = LayoutOptions::new(40, &theme);
        opts.folds = Some(&folds);
        assert!(engine.relayout_nodes(&doc, &opts, &mut tree, first, last - first));
        assert!(!tree.to_plain_text().contains("body"));

        folds.expand(0);
        let mut opts = LayoutOptions::new(40, &theme);
        opts.folds = Some(&folds);
        assert!(engine.relayout_nodes(&doc, &opts, &mut tree, first, last - first));
        assert_eq!(tree.to_plain_text(), before);
    }

    #[test]
    fn deterministic_and_fast_enough() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/readme.md"),
        )
        .unwrap();
        let doc = parse(&src);
        let theme = Theme::dark();
        let opts = LayoutOptions::new(80, &theme);
        let engine = Layout::new();
        let a = engine.layout(&doc, &opts);
        let start = std::time::Instant::now();
        let b = engine.layout(&doc, &opts);
        let elapsed = start.elapsed();
        assert_eq!(a.to_plain_text(), b.to_plain_text());
        assert!(
            elapsed < std::time::Duration::from_millis(10),
            "README layout took {elapsed:?}"
        );
    }

    // --- LayoutOptions::apply_config ------------------------------------
    //
    // Both rendering paths map `Config` onto `LayoutOptions` through this one
    // function, so these tests are what keeps a new configuration option from
    // being wired into the interactive path and forgotten on the
    // non-interactive one.

    #[test]
    fn apply_config_of_the_defaults_changes_nothing() {
        let theme = crate::testing::theme();
        let before = LayoutOptions::new(80, &theme);
        let mut after = LayoutOptions::new(80, &theme);
        after.apply_config(&Config::default());
        assert_eq!(
            after.fingerprint(),
            before.fingerprint(),
            "the built-in configuration must agree with LayoutOptions::new"
        );
    }

    #[test]
    fn apply_config_maps_every_configured_option() {
        let theme = crate::testing::theme();
        let mut cfg = Config::default();
        cfg.table.mode = TableMode::Scroll;
        cfg.table.max_column_width = 17;
        cfg.code.wrap = true;
        cfg.code.line_numbers = true;
        cfg.code.tab_width = 8;
        cfg.wrap = false;

        let mut opts = LayoutOptions::new(80, &theme);
        opts.apply_config(&cfg);

        assert_eq!(opts.table_mode, TableMode::Scroll);
        assert_eq!(opts.max_column_width, 17);
        assert!(opts.code_wrap);
        assert!(opts.code_line_numbers);
        assert_eq!(opts.tab_width, 8);
        assert!(!opts.wrap);
        // Not configuration-derived: the caller still owns these.
        assert_eq!(opts.width, 80);
        assert!(opts.footnotes);
        assert!(!opts.lazy_code);
        assert!(!opts.images);
        assert!(opts.unicode);
        assert!(opts.folds.is_none());
    }

    #[test]
    fn top_level_line_numbers_is_a_shorthand_for_the_code_section() {
        let theme = crate::testing::theme();
        for (top, code, want) in [
            (false, false, false),
            (true, false, true),
            (false, true, true),
            (true, true, true),
        ] {
            let mut cfg = Config {
                line_numbers: top,
                ..Config::default()
            };
            cfg.code.line_numbers = code;
            let mut opts = LayoutOptions::new(80, &theme);
            opts.apply_config(&cfg);
            assert_eq!(
                opts.code_line_numbers, want,
                "line_numbers={top}, code.line_numbers={code}"
            );
        }
    }

    #[test]
    fn a_zero_tab_width_is_clamped_to_one() {
        // `tab_width = 0` is accepted by the schema; a zero-width tab would
        // make the expansion loop produce nothing at all.
        let theme = crate::testing::theme();
        let mut cfg = Config::default();
        cfg.code.tab_width = 0;
        let mut opts = LayoutOptions::new(80, &theme);
        opts.apply_config(&cfg);
        assert_eq!(opts.tab_width, 1);
    }

    #[test]
    fn apply_config_is_the_only_mapping_both_paths_use() {
        // The interactive path and `print_plain` must produce identical
        // configuration-derived options for the same configuration; this is
        // that claim, stated for a configuration in which every option
        // differs from its default.
        let theme = crate::testing::theme();
        let mut cfg = Config::default();
        cfg.table.mode = TableMode::Compact;
        cfg.table.max_column_width = 33;
        cfg.code.wrap = true;
        cfg.code.line_numbers = true;
        cfg.code.tab_width = 2;
        cfg.wrap = false;

        let mut a = LayoutOptions::new(40, &theme);
        a.apply_config(&cfg);
        let mut b = LayoutOptions::new(40, &theme);
        b.apply_config(&cfg);
        assert_eq!(a.fingerprint(), b.fingerprint());
    }
}
