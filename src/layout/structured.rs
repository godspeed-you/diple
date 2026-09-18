//! Laying out a structured document.
//!
//! # Rows come from nodes, not from pretty-printed text
//!
//! The obvious shortcut — serialise the document to a string, highlight it as
//! a code block, fold by line range — would look right and be wrong in every
//! way that matters: a fold would be a line range rather than a subtree, a
//! resize would change what "line 40" means, the cursor would have no
//! identity, and the path could not be computed. So every row here is emitted
//! from one semantic node and carries its [`NodeId`], exactly as the Markdown
//! engine does. One node may produce many rows — a block scalar, a comment
//! block — and those rows never become the node's identity.
//!
//! # What a row looks like
//!
//! ```text
//! ▼ spec: {                      a container, expanded (JSON)
//! ▶   containers: [3 items]      a container, collapsed
//!     replicas: 3                a scalar entry
//!   # keep in sync               a comment written above its entry
//! ```
//!
//! JSON keeps its braces so it still reads as JSON; YAML keeps its
//! indentation and its `-`/`[0]` markers so it still reads as YAML. Neither
//! grows a closing-delimiter row: a `}` on its own line is a cursor stop that
//! stands for nothing, and the indentation already says where the container
//! ends.
//!
//! # Type is never colour alone
//!
//! A string is quoted, a null is spelled, a boolean is spelled, a number is
//! bare, a tag is written as the source wrote it. The palette makes that
//! faster to read; it is never the only signal, because `NO_COLOR` and a
//! 16-colour SSH session are ordinary places to read a document.

use crate::document::structured::{
    ScalarKind, ScalarStyle, StructuredDocument, StructuredNodeKind,
};
use crate::document::{DocumentKind, FoldState, Match, MatchField, NodeId};
use crate::render::primitives::{LineKind, NodeSpan, RenderLine, RenderTree, StyledSpan};
use crate::render::theme::{Style, Theme};

use super::LayoutOptions;

/// Columns reserved for the fold marker in the interactive view.
const MARKER_WIDTH: usize = 2;

/// Lay out a whole structured document.
pub fn layout(doc: &StructuredDocument, opts: &LayoutOptions<'_>) -> RenderTree {
    let mut builder = Builder::new(doc, opts);
    builder.run();
    let Builder { lines, spans, .. } = builder;
    let tail = lines.len();
    RenderTree::with_index(lines, Vec::new(), spans, tail)
}

/// Re-lay out the subtree rooted at `node` and splice it into `tree`.
///
/// This is the incremental path a fold takes. The rows of a node's subtree are
/// contiguous, so collapsing or expanding one rewrites exactly that range and
/// leaves the rest of the document — and every line index outside it —
/// untouched. Returns `false` when the node has no rows to replace, in which
/// case the caller falls back to a full re-layout.
pub fn relayout_subtree(
    doc: &StructuredDocument,
    opts: &LayoutOptions<'_>,
    tree: &mut RenderTree,
    node: NodeId,
) -> bool {
    let Some(start) = tree.first_line_of(node) else {
        return false;
    };
    let Some(end_node) = doc.node(node).map(|n| n.end) else {
        return false;
    };
    // The subtree's rows run from the node's first row up to the first row
    // that belongs to a node outside it.
    let mut old_len = 0usize;
    for line in tree.lines.iter().skip(start) {
        if line.node < node || line.node >= end_node {
            break;
        }
        old_len += 1;
    }
    if old_len == 0 {
        return false;
    }
    let mut builder = Builder::new(doc, opts);
    builder.node(node);
    let Builder { lines, .. } = builder;
    tree.splice_lines(start, old_len, lines)
}

struct Builder<'a> {
    doc: &'a StructuredDocument,
    opts: &'a LayoutOptions<'a>,
    theme: &'a Theme,
    /// Match offsets bucketed per node and field.
    matches: std::collections::HashMap<(NodeId, MatchField), Vec<Match>>,
    lines: Vec<RenderLine>,
    /// One entry per root document, for the splice bookkeeping.
    spans: Vec<NodeSpan>,
    /// How many indent levels the current document's nodes sit above the left
    /// margin.
    depth_offset: usize,
}

impl<'a> Builder<'a> {
    fn new(doc: &'a StructuredDocument, opts: &'a LayoutOptions<'a>) -> Self {
        let mut matches: std::collections::HashMap<(NodeId, MatchField), Vec<Match>> =
            std::collections::HashMap::new();
        for m in opts.search_matches {
            matches.entry((m.node, m.field)).or_default().push(*m);
        }
        Self {
            doc,
            opts,
            theme: opts.theme,
            matches,
            lines: Vec::new(),
            spans: Vec::new(),
            depth_offset: 0,
        }
    }

    // ---- traversal -------------------------------------------------------

    fn run(&mut self) {
        let roots: Vec<NodeId> = self.doc.roots().iter().map(|r| r.node).collect();
        for root in roots {
            let start = self.lines.len();
            self.node(root);
            self.spans.push(NodeSpan {
                node: root,
                start,
                len: self.lines.len() - start,
            });
        }
    }

    /// Emit `node` and, unless it is folded away, its descendants.
    ///
    /// Iterative: the nesting depth of a structured document is decided by
    /// whoever wrote it, and a recursive walk would turn a crafted file into a
    /// stack overflow.
    fn node(&mut self, node: NodeId) {
        self.depth_offset = self.depth_offset_for(node);
        let mut stack = vec![node];
        while let Some(id) = stack.pop() {
            let Some(current) = self.doc.node(id) else {
                continue;
            };
            if self.hidden(id) {
                continue;
            }
            if current.parent.is_none() {
                self.depth_offset = self.depth_offset_for(id);
                self.root_rows(id);
            } else {
                self.entry_rows(id);
            }
            if self.collapsed(id) {
                continue;
            }
            // Push the children in reverse so they come back off the stack in
            // document order.
            for child in self.doc.children(id).into_iter().rev() {
                stack.push(child);
            }
        }
    }

    /// How many indent levels to subtract so that the outermost visible row
    /// sits at the left margin.
    fn depth_offset_for(&self, node: NodeId) -> usize {
        let Some(root) = self.root_of(node) else {
            return 0;
        };
        match self.doc.kind() {
            // JSON shows its root brace, so its members are indented under it.
            DocumentKind::Json => 0,
            // YAML shows a document row only when there is more than one
            // document to tell apart; a single document's entries start at
            // the margin, the way the file does.
            _ => {
                if self.doc.roots().len() > 1 {
                    0
                } else {
                    let _ = root;
                    1
                }
            }
        }
    }

    fn root_of(&self, node: NodeId) -> Option<NodeId> {
        let mut cur = node;
        let mut guard = 0usize;
        while let Some(parent) = self.doc.parent(cur) {
            cur = parent;
            guard += 1;
            if guard > self.doc.node_count() {
                break;
            }
        }
        Some(cur)
    }

    fn hidden(&self, node: NodeId) -> bool {
        self.opts
            .folds
            .is_some_and(|folds| self.doc.is_hidden(node, folds))
    }

    fn collapsed(&self, node: NodeId) -> bool {
        let Some(folds) = self.opts.folds else {
            return false;
        };
        self.doc
            .node(node)
            .and_then(|n| n.fold)
            .is_some_and(|fold| folds.is_collapsed(fold))
    }

    fn indent_of(&self, node: NodeId) -> usize {
        self.doc
            .node(node)
            .map(|n| n.depth.saturating_sub(self.depth_offset))
            .unwrap_or(0)
    }

    // ---- rows ------------------------------------------------------------

    /// The rows a document root contributes: its directives, a document
    /// marker when the stream has several, and — for JSON — the opening
    /// brace of the document itself.
    fn root_rows(&mut self, node: NodeId) {
        let Some(index) = self
            .doc
            .roots()
            .iter()
            .position(|r| r.node == node)
        else {
            return;
        };
        let directives: Vec<String> = self.doc.roots()[index]
            .directives
            .iter()
            .map(|d| d.text())
            .collect();
        for text in directives {
            let mut row = self.row(node, 0, false);
            row.push(&text, self.theme.structured.directive);
            self.push(row, LineKind::Text);
        }
        self.comments_above(node);

        let multi = self.doc.roots().len() > 1;
        if self.doc.kind() == DocumentKind::Json || multi {
            let mut row = self.row(node, 0, false);
            if multi {
                row.push("--- ", self.theme.structured.punctuation);
                row.push(&self.doc.label(node), self.theme.structured.key);
            }
            self.value_part(&mut row, node, multi);
            self.finish_row(row, node);
        } else if !self
            .doc
            .node(node)
            .is_some_and(|n| n.kind.is_container())
        {
            // A YAML document that is just a scalar still needs a row.
            let mut row = self.row(node, 0, false);
            self.value_part(&mut row, node, false);
            self.finish_row(row, node);
        }
    }

    /// The rows one mapping entry or sequence item contributes.
    fn entry_rows(&mut self, node: NodeId) {
        self.comments_above(node);
        let indent = self.indent_of(node);
        let foldable = self.doc.node(node).and_then(|n| n.fold).is_some();
        let mut row = self.row(node, indent, foldable);
        self.label_part(&mut row, node);
        self.value_part(&mut row, node, true);
        self.finish_row(row, node);
    }

    /// The comments written above a node, one row each, at its indentation.
    fn comments_above(&mut self, node: NodeId) {
        let Some(current) = self.doc.node(node) else {
            return;
        };
        let ids: Vec<usize> = current.comments_above().to_vec();
        if ids.is_empty() {
            return;
        }
        let indent = self.indent_of(node);
        // Match offsets index the node's whole comment field, which is the
        // comments joined by newlines; each row therefore starts at the
        // offset the previous ones ended at.
        let mut base = 0usize;
        for id in ids {
            let Some(comment) = self.doc.comment(id) else {
                continue;
            };
            let text = format!("#{}", comment.text);
            let mut row = self.row(node, indent, false);
            // The `#` is not part of the indexed text, so the highlight
            // offsets shift by one column.
            row.push_matched(
                &text,
                self.theme.structured.comment,
                self.field_matches(node, MatchField::Comment),
                base.wrapping_sub(1),
            );
            self.push(row, LineKind::Text);
            base += comment.text.len() + 1;
        }
    }

    /// The key or index that names a node.
    fn label_part(&mut self, row: &mut Row, node: NodeId) {
        let Some(current) = self.doc.node(node) else {
            return;
        };
        let yaml = self.doc.kind() != DocumentKind::Json;
        match &current.relation {
            crate::document::structured::NodeRelation::MappingEntry { key } => {
                let style = if current.is_merge_key() {
                    self.theme.structured.merge_key
                } else {
                    self.theme.structured.key
                };
                row.push_matched(
                    &key.text,
                    style,
                    self.field_matches(node, MatchField::Label),
                    0,
                );
                row.push(":", self.theme.structured.punctuation);
                row.push(" ", Style::new());
            }
            crate::document::structured::NodeRelation::SequenceItem { index } => {
                if self.opts.show_indices {
                    row.push(&format!("[{index}]"), self.theme.structured.index);
                    row.push(if yaml { " " } else { ": " }, self.theme.structured.punctuation);
                } else {
                    row.push(if yaml { "- " } else { "" }, self.theme.structured.punctuation);
                }
            }
            crate::document::structured::NodeRelation::Root { .. } => {}
        }
    }

    /// The value itself: a scalar, an alias, or a container's marker.
    fn value_part(&mut self, row: &mut Row, node: NodeId, labelled: bool) {
        let Some(current) = self.doc.node(node) else {
            return;
        };
        if let Some(anchor) = current.anchor() {
            row.push(&format!("&{anchor}"), self.theme.structured.anchor);
            row.push(" ", Style::new());
        }
        if let Some(tag) = current.tag() {
            row.push(tag, self.theme.structured.tag);
            row.push(" ", Style::new());
        }
        match &current.kind {
            StructuredNodeKind::Alias { name } => {
                row.push_matched(
                    &format!("*{name}"),
                    self.theme.structured.alias,
                    self.field_matches(node, MatchField::Anchor),
                    1,
                );
            }
            StructuredNodeKind::Scalar(value) => {
                self.scalar_part(row, node, value, labelled);
                return;
            }
            StructuredNodeKind::Mapping | StructuredNodeKind::Sequence => {
                self.container_part(row, node, current.child_count, labelled);
            }
        }
    }

    fn container_part(&mut self, row: &mut Row, node: NodeId, children: usize, labelled: bool) {
        let json = self.doc.kind() == DocumentKind::Json;
        let sequence = matches!(
            self.doc.node(node).map(|n| &n.kind),
            Some(StructuredNodeKind::Sequence)
        );
        let (open, close) = if sequence { ("[", "]") } else { ("{", "}") };
        if children == 0 {
            // An empty container must stay explicit in both dialects: `{}` is
            // a fact about the document, and a bare key would hide it.
            row.push(&format!("{open}{close}"), self.theme.structured.punctuation);
            return;
        }
        if self.collapsed(node) {
            let summary = self
                .doc
                .collapsed_summary(node)
                .unwrap_or_else(|| format!("{open}{close}"));
            row.push(&summary, self.theme.structured.folded);
            return;
        }
        if json {
            row.push(open, self.theme.structured.punctuation);
        } else if !labelled {
            // A YAML document row with nothing after it.
        }
    }

    fn scalar_part(
        &mut self,
        row: &mut Row,
        node: NodeId,
        value: &crate::document::structured::ScalarValue,
        _labelled: bool,
    ) {
        let style = self.scalar_style(value.kind);
        let matches = self.field_matches(node, MatchField::Value);
        if value.style.is_block() {
            // A block scalar keeps its shape: the indicator on the key's row,
            // the content on rows of its own beneath it.
            if let Some(indicator) = value.style.block_indicator() {
                row.push(indicator, self.theme.structured.punctuation);
            }
            let indent = self.indent_of(node) + 1;
            let right = self.right_comment(node);
            let mut header = std::mem::replace(row, self.row(node, 0, false));
            if let Some((text, style, base)) = right {
                header.push("  ", Style::new());
                header.push_matched(&text, style, self.field_matches(node, MatchField::Comment), base);
            }
            self.push(header, LineKind::Text);
            let mut base = 0usize;
            for line in value.text.split('\n') {
                if base >= value.text.len() && line.is_empty() {
                    break;
                }
                let mut content = self.row(node, indent, false);
                content.push_matched(line, style, matches.clone(), base);
                self.push(content, LineKind::Text);
                base += line.len() + 1;
            }
            // The caller's `finish_row` must not emit another row.
            row.suppressed = true;
            return;
        }
        let shown = value.display();
        match value.kind {
            // A string is quoted so that its type survives without colour,
            // and so that leading or trailing space is visible.
            ScalarKind::String if self.quote_strings(value.style) => {
                row.push("\"", self.theme.structured.punctuation);
                row.push_matched(shown, style, matches, 0);
                row.push("\"", self.theme.structured.punctuation);
            }
            _ => row.push_matched(shown, style, matches, 0),
        }
    }

    /// Whether a string of this style is shown with quotes.
    ///
    /// JSON strings always are — that is how JSON is written. A YAML plain
    /// scalar is not, because quoting it would be a claim about the source
    /// that is not true; a YAML quoted scalar keeps the quotes it had.
    fn quote_strings(&self, style: ScalarStyle) -> bool {
        match self.doc.kind() {
            DocumentKind::Json => true,
            _ => matches!(
                style,
                ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted
            ),
        }
    }

    fn scalar_style(&self, kind: ScalarKind) -> Style {
        match kind {
            ScalarKind::String => self.theme.structured.string,
            ScalarKind::Number => self.theme.structured.number,
            ScalarKind::Boolean => self.theme.structured.boolean,
            ScalarKind::Null => self.theme.structured.null,
        }
    }

    /// The same-line comment of a node, with the match offset it starts at
    /// inside the node's comment field.
    fn right_comment(&self, node: NodeId) -> Option<(String, Style, usize)> {
        let current = self.doc.node(node)?;
        let id = current.comment_right()?;
        let comment = self.doc.comment(id)?;
        // The comment field is the comments above joined with newlines, then
        // this one; its offset is everything before it.
        let base: usize = current
            .comments_above()
            .iter()
            .filter_map(|c| self.doc.comment(*c))
            .map(|c| c.text.len() + 1)
            .sum();
        Some((
            format!("#{}", comment.text),
            self.theme.structured.comment,
            base.wrapping_sub(1),
        ))
    }

    fn field_matches(&self, node: NodeId, field: MatchField) -> Vec<Match> {
        self.matches
            .get(&(node, field))
            .cloned()
            .unwrap_or_default()
    }

    // ---- row plumbing ----------------------------------------------------

    fn row(&self, node: NodeId, indent: usize, foldable: bool) -> Row {
        let mut row = Row {
            node,
            spans: Vec::new(),
            suppressed: false,
        };
        if self.opts.folds.is_some() {
            let marker = if !foldable {
                "  "
            } else if self.collapsed(node) {
                if self.opts.unicode {
                    "\u{25b6} "
                } else {
                    "> "
                }
            } else if self.opts.unicode {
                "\u{25bc} "
            } else {
                "v "
            };
            row.push(marker, self.theme.fold_marker);
        }
        let width = indent.saturating_mul(self.opts.structured_indent);
        if width > 0 {
            row.push(&" ".repeat(width), Style::new());
        }
        row
    }

    /// Close a value row, appending its same-line comment.
    fn finish_row(&mut self, mut row: Row, node: NodeId) {
        if row.suppressed {
            return;
        }
        if let Some((text, style, base)) = self.right_comment(node) {
            row.push("  ", Style::new());
            row.push_matched(
                &text,
                style,
                self.field_matches(node, MatchField::Comment),
                base,
            );
        }
        let foldable = self.doc.node(node).and_then(|n| n.fold).is_some();
        let kind = if foldable && self.collapsed(node) {
            LineKind::FoldedMarker
        } else if self
            .doc
            .node(node)
            .is_some_and(|n| n.kind.is_container())
        {
            let depth = self.indent_of(node).min(u8::MAX as usize) as u8;
            LineKind::Structural(depth)
        } else {
            LineKind::Text
        };
        self.push(row, kind);
    }

    fn push(&mut self, row: Row, kind: LineKind) {
        let Row { node, spans, .. } = row;
        self.lines.push(RenderLine::new(node, kind, spans));
    }

    /// Reserved columns the content of a row starts after, for the callers
    /// that need to know the usable width.
    #[allow(dead_code)]
    fn gutter(&self) -> usize {
        if self.opts.folds.is_some() {
            MARKER_WIDTH
        } else {
            0
        }
    }
}

/// One row under construction.
struct Row {
    node: NodeId,
    spans: Vec<StyledSpan>,
    /// Set when the row has already been emitted in pieces (a block scalar).
    suppressed: bool,
}

impl Row {
    fn push(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.spans.push(StyledSpan::new(text, style));
    }

    /// Push `text`, splitting it where search matches fall.
    ///
    /// `base` is the offset of `text` inside the indexed field, so a row that
    /// shows one line of a multi-line field highlights the right part of it.
    /// It is allowed to wrap below zero (a `#` the index does not contain),
    /// which is why the arithmetic is done on wrapped offsets and every
    /// candidate range is bounds-checked against `text`.
    fn push_matched(&mut self, text: &str, style: Style, matches: Vec<Match>, base: usize) {
        if matches.is_empty() {
            self.push(text, style);
            return;
        }
        let mut cursor = 0usize;
        for m in matches {
            let start = m.start.wrapping_sub(base);
            let end = m.end.wrapping_sub(base);
            if start >= text.len() || end > text.len() || end <= start || start < cursor {
                continue;
            }
            if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                continue;
            }
            self.push(&text[cursor..start], style);
            self.spans.push(StyledSpan {
                text: crate::util::text::sanitize(&text[start..end]),
                style,
                link: None,
                search_match: true,
            });
            cursor = end;
        }
        self.push(&text[cursor..], style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{load, FormatRequest, SourceDocument};
    use crate::render::theme::Theme;

    fn structured(name: &str, src: &str) -> StructuredDocument {
        let loaded = load(FormatRequest::Auto, SourceDocument::new(name, src)).expect(src);
        loaded
            .model
            .as_structured()
            .expect("a structured document")
            .clone()
    }

    fn rows(name: &str, src: &str, folds: Option<&FoldState>) -> Vec<String> {
        let theme = Theme::dark();
        let mut opts = LayoutOptions::new(80, &theme);
        if let Some(folds) = folds {
            opts.folds = Some(folds);
        }
        let doc = structured(name, src);
        layout(&doc, &opts)
            .lines
            .iter()
            .map(|l| l.to_text().trim_end().to_string())
            .collect()
    }

    const JSON: &str = r#"{"metadata":{"name":"nginx","labels":{"app":"frontend"}},"spec":{"replicas":3,"containers":[{"name":"nginx","image":"nginx:1.27"}]},"empty":{},"none":[]}"#;

    const YAML: &str = "apiVersion: apps/v1\nkind: Deployment\n\nmetadata:\n  name: nginx\n  labels:\n    app: frontend\n\nspec:\n  replicas: 3\n  template:\n    spec:\n      containers:\n        - name: nginx\n          image: nginx:1.27\n";

    #[test]
    fn json_renders_as_json() {
        assert_eq!(
            rows("t.json", JSON, None),
            [
                "{",
                "  metadata: {",
                "    name: \"nginx\"",
                "    labels: {",
                "      app: \"frontend\"",
                "  spec: {",
                "    replicas: 3",
                "    containers: [",
                "      [0]: {",
                "        name: \"nginx\"",
                "        image: \"nginx:1.27\"",
                "  empty: {}",
                "  none: []",
            ]
        );
    }

    #[test]
    fn yaml_renders_as_yaml() {
        assert_eq!(
            rows("t.yaml", YAML, None),
            [
                "apiVersion: apps/v1",
                "kind: Deployment",
                "metadata:",
                "  name: nginx",
                "  labels:",
                "    app: frontend",
                "spec:",
                "  replicas: 3",
                "  template:",
                "    spec:",
                "      containers:",
                "        [0]",
                "          name: nginx",
                "          image: nginx:1.27",
            ]
        );
    }

    #[test]
    fn every_row_carries_the_node_it_came_from() {
        let theme = Theme::dark();
        let opts = LayoutOptions::new(80, &theme);
        let doc = structured("t.yaml", YAML);
        let tree = layout(&doc, &opts);
        for line in &tree.lines {
            assert!(doc.node(line.node).is_some(), "{line:?}");
        }
        // And the tree can find a node's first row again — that round trip is
        // what the viewport anchor and every jump rely on.
        for node in doc.nodes() {
            let first = tree.first_line_of(node.id);
            assert!(first.is_some(), "node {} has no row", node.id);
        }
    }

    #[test]
    fn a_collapsed_container_shows_a_summary_and_hides_its_subtree() {
        let doc = structured("t.json", JSON);
        let mut folds = FoldState::from_parents(doc.fold_parents().to_vec());
        // Collapse `spec`.
        let spec = doc
            .nodes()
            .iter()
            .find(|n| doc.label(n.id) == "spec")
            .expect("spec");
        folds.collapse(spec.fold.expect("foldable"));
        let shown = rows("t.json", JSON, Some(&folds));
        assert!(shown.iter().any(|r| r.contains("spec: {2 members}")), "{shown:?}");
        assert!(!shown.iter().any(|r| r.contains("replicas")), "{shown:?}");
        assert!(shown.iter().any(|r| r.contains("metadata")), "siblings stay");
        // The fold marker is on the collapsed row.
        assert!(shown.iter().any(|r| r.starts_with("\u{25b6}")), "{shown:?}");
    }

    #[test]
    fn folding_never_depends_on_the_width() {
        let doc = structured("t.json", JSON);
        let mut folds = FoldState::from_parents(doc.fold_parents().to_vec());
        folds.collapse_all();
        let theme = Theme::dark();
        let mut narrow = LayoutOptions::new(30, &theme);
        narrow.folds = Some(&folds);
        let mut wide = LayoutOptions::new(200, &theme);
        wide.folds = Some(&folds);
        let a = layout(&doc, &narrow);
        let b = layout(&doc, &wide);
        assert_eq!(
            a.lines.iter().map(|l| l.node).collect::<Vec<_>>(),
            b.lines.iter().map(|l| l.node).collect::<Vec<_>>(),
            "the same nodes are visible at any width"
        );
    }

    #[test]
    fn splicing_a_fold_equals_a_full_rebuild() {
        let doc = structured("t.json", JSON);
        let theme = Theme::dark();
        let mut folds = FoldState::from_parents(doc.fold_parents().to_vec());
        let spec = doc
            .nodes()
            .iter()
            .find(|n| doc.label(n.id) == "spec")
            .expect("spec");
        let fold = spec.fold.expect("foldable");

        let mut opts = LayoutOptions::new(80, &theme);
        opts.folds = Some(&folds);
        let mut spliced = layout(&doc, &opts);

        folds.collapse(fold);
        let mut opts = LayoutOptions::new(80, &theme);
        opts.folds = Some(&folds);
        assert!(relayout_subtree(&doc, &opts, &mut spliced, spec.id));

        let rebuilt = layout(&doc, &opts);
        assert_eq!(
            spliced
                .lines
                .iter()
                .map(|l| (l.node, l.to_text()))
                .collect::<Vec<_>>(),
            rebuilt
                .lines
                .iter()
                .map(|l| (l.node, l.to_text()))
                .collect::<Vec<_>>()
        );
        // And expanding again splices back to the original.
        folds.expand(fold);
        let mut opts = LayoutOptions::new(80, &theme);
        opts.folds = Some(&folds);
        assert!(relayout_subtree(&doc, &opts, &mut spliced, spec.id));
        assert_eq!(spliced.len(), layout(&doc, &opts).len());
    }

    #[test]
    fn comments_render_above_and_beside_what_they_describe() {
        let src = "# Production replicas.\nreplicas: 3  # minimum for HA\nother: 1\n";
        assert_eq!(
            rows("t.yaml", src, None),
            [
                "# Production replicas.",
                "replicas: 3  # minimum for HA",
                "other: 1",
            ]
        );
    }

    #[test]
    fn a_block_scalar_stays_multi_line() {
        let src = "script: |\n  echo hello\n  echo world\nafter: 1\n";
        assert_eq!(
            rows("t.yaml", src, None),
            ["script: |", "  echo hello", "  echo world", "after: 1"]
        );
    }

    #[test]
    fn anchors_aliases_tags_and_merge_keys_are_all_visible() {
        let src = "defaults: &defaults\n  retries: 3\nservice:\n  <<: *defaults\n  when: !!timestamp 2026-08-31\n";
        assert_eq!(
            rows("t.yaml", src, None),
            [
                "defaults: &defaults",
                "  retries: 3",
                "service:",
                "  <<: *defaults",
                "  when: !!timestamp 2026-08-31",
            ]
        );
    }

    #[test]
    fn multiple_documents_are_separated_and_named() {
        let src = "%YAML 1.2\n---\nkind: ConfigMap\n---\nkind: Deployment\n";
        assert_eq!(
            rows("t.yaml", src, None),
            [
                "%YAML 1.2",
                "--- Document 1",
                "  kind: ConfigMap",
                "--- Document 2",
                "  kind: Deployment",
            ]
        );
    }

    #[test]
    fn type_is_readable_without_colour() {
        let src = "s: \"text\"\nn: 42\nb: true\nz: null\ne: \"\"\np: plain\n";
        assert_eq!(
            rows("t.yaml", src, None),
            [
                "s: \"text\"",
                "n: 42",
                "b: true",
                "z: null",
                "e: \"\"",
                "p: plain",
            ]
        );
        // JSON quotes every string, because that is how JSON is written.
        assert_eq!(
            rows("t.json", r#"{"s":"text","n":42,"b":true,"z":null}"#, None),
            ["{", "  s: \"text\"", "  n: 42", "  b: true", "  z: null"]
        );
    }

    #[test]
    fn an_empty_value_shows_as_the_author_wrote_it() {
        assert_eq!(
            rows("t.yaml", "empty:\nnull_: null\ntilde: ~\n", None),
            ["empty:", "null_: null", "tilde: ~"]
        );
    }

    #[test]
    fn a_search_match_is_highlighted_in_the_field_it_was_found_in() {
        let doc = structured("t.json", r#"{"image":"image:1.27"}"#);
        let hits = doc.search_index().find("image", false);
        assert_eq!(hits.len(), 2);
        let theme = Theme::dark();
        let mut opts = LayoutOptions::new(80, &theme);
        opts.search_matches = &hits;
        let tree = layout(&doc, &opts);
        let row = tree
            .lines
            .iter()
            .find(|l| l.to_text().contains("image:"))
            .expect("the entry row");
        let marked: Vec<&str> = row
            .spans
            .iter()
            .filter(|s| s.search_match)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(marked, ["image", "image"], "the key and the value");
    }

    #[test]
    fn a_hostile_document_cannot_write_to_the_terminal() {
        let src = "a: \"\\u001b[31mred\\u0007\"\n";
        let shown = rows("t.yaml", src, None).join("\n");
        assert!(!shown.contains('\u{1b}'), "{shown:?}");
        assert!(!shown.contains('\u{7}'), "{shown:?}");
    }

    #[test]
    fn a_very_long_scalar_produces_one_row_not_one_per_byte() {
        let long = "x".repeat(100_000);
        let src = format!("k: \"{long}\"\n");
        let shown = rows("t.yaml", &src, None);
        assert_eq!(shown.len(), 1);
    }

    #[test]
    fn an_empty_document_lays_out_to_nothing() {
        let doc = structured("t.yaml", "");
        let theme = Theme::dark();
        let tree = layout(&doc, &LayoutOptions::new(80, &theme));
        assert!(tree.is_empty());
    }
}
