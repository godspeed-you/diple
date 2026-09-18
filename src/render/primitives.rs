//! Render primitives — the contract between [`crate::layout`] and every
//! terminal renderer.
//!
//! A [`RenderTree`] is a flat list of [`RenderLine`]s. Each line knows the
//! [`NodeId`] it was produced from, so the application can anchor the viewport
//! semantically instead of by absolute line number.
//!
//! Field names of [`RenderTree`], [`RenderLine`], [`StyledSpan`], [`LineKind`]
//! and [`ImageRef`] are a stable cross-workstream contract: extend them
//! additively, never rename.

use std::collections::HashMap;

use crate::document::{LinkId, NodeId};
use crate::render::theme::Style;

/// One styled run of text inside a [`RenderLine`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StyledSpan {
    /// Text of the run. Never contains `\n` or `\t`.
    pub text: String,
    /// Visual style.
    pub style: Style,
    /// Link this run belongs to, if any (used for link selection and for the
    /// integrator's OSC 8 emission — this module never emits escapes).
    pub link: Option<LinkId>,
    /// `true` when the run overlaps a search match.
    pub search_match: bool,
}

impl StyledSpan {
    /// A plain run with a style.
    /// A span of text in a style.
    ///
    /// Every piece of document-controlled text reaches the terminal through
    /// here, so this is where it is made safe: an ESC, a BEL, a carriage
    /// return or a bidirectional override inside a JSON string, a YAML
    /// comment or a Markdown paragraph is replaced before it can become
    /// something the terminal obeys (see [`crate::util::text`]). Ordinary
    /// text is borrowed through unchanged.
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        let text = text.into();
        Self {
            text: match crate::util::text::sanitized(&text) {
                std::borrow::Cow::Borrowed(_) => text,
                std::borrow::Cow::Owned(clean) => clean,
            },
            style,
            link: None,
            search_match: false,
        }
    }

    /// A run with a link id attached.
    pub fn with_link(mut self, link: Option<LinkId>) -> Self {
        self.link = link;
        self
    }

    /// Display width of the run in terminal cells.
    pub fn width(&self) -> usize {
        crate::util::unicode::width(&self.text)
    }

    /// Whether two spans can be merged (same styling and provenance).
    pub fn mergeable(&self, other: &Self) -> bool {
        self.style == other.style
            && self.link == other.link
            && self.search_match == other.search_match
    }
}

/// Reference to an image (or diagram image) placed on a line.
///
/// The layout engine never encodes image data; it only reserves the cells.
/// Emitting the actual protocol escape is Workstream A's job.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageRef {
    /// Opaque id assigned by the image/diagram provider.
    pub id: usize,
    /// Reserved width in terminal columns.
    pub cols: u16,
    /// Reserved height in terminal rows.
    pub rows: u16,
    /// Alt text, shown when the image cannot be drawn.
    pub alt: String,
}

/// What kind of content a line carries. Renderers may treat kinds specially
/// (e.g. images), but must render `spans` for every kind.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LineKind {
    /// Prose, list items, quotes, footnotes.
    #[default]
    Text,
    /// A heading line of the given level (1..=6); the underline of a heading
    /// is also reported as `Heading`.
    Heading(u8),
    /// The row of a structured container, at the given nesting depth.
    ///
    /// The structural counterpart of [`LineKind::Heading`] for JSON and YAML:
    /// what the outline nests, what folds, and what `[`/`]` move between.
    Structural(u8),
    /// A line inside a code block.
    Code,
    /// A table border, header or body line.
    TableRow,
    /// A line of a text-rendered diagram.
    Diagram,
    /// A line that reserves space for a terminal image.
    Image(ImageRef),
    /// The single visible line of a collapsed section.
    FoldedMarker,
    /// Intentionally empty spacing line.
    Blank,
}

/// One laid-out terminal line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderLine {
    /// Node this line was produced from (semantic anchoring).
    pub node: NodeId,
    /// Content runs, left to right.
    pub spans: Vec<StyledSpan>,
    /// Content classification.
    pub kind: LineKind,
    /// Display width in columns. May exceed the viewport width, in which case
    /// the viewport scrolls horizontally.
    pub width: usize,
    /// 0-based index of this line within the consecutive run of lines that
    /// belong to `node` (additive extension; used for viewport anchoring).
    pub node_offset: usize,
}

impl RenderLine {
    /// Build a line from spans, computing `width`.
    pub fn new(node: NodeId, kind: LineKind, spans: Vec<StyledSpan>) -> Self {
        let width = spans.iter().map(StyledSpan::width).sum();
        Self {
            node,
            spans,
            kind,
            width,
            node_offset: 0,
        }
    }

    /// Concatenated plain text of the line (trailing spaces kept).
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for s in &self.spans {
            out.push_str(&s.text);
        }
        out
    }
}

/// A code block whose syntax highlighting has been deferred.
///
/// The layout engine emits code with plain code styling and records where the
/// block landed, so that the application can ask for the highlighting of the
/// blocks the reader is about to see — and only those — before the frame is
/// drawn. Realizing a pending block never changes the number of lines it
/// occupies, nor their widths, so no line index and no viewport anchor can
/// move underneath the reader.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingCode {
    /// Node the block was produced from.
    pub node: NodeId,
    /// Index of the block's first line in the tree.
    pub start: usize,
    /// Number of lines the block occupies.
    pub len: usize,
    /// Prefix spans (quote gutters, list indentation) each line carries.
    pub prefix: Vec<StyledSpan>,
    /// Content width the block was laid out at.
    pub width: usize,
}

/// The line range one top-level document node produced.
///
/// One entry per top-level node, in document order, including nodes that
/// produced nothing (`len == 0`, e.g. the body of a collapsed section). This
/// is what makes an incremental re-layout of a node range possible: a fold
/// changes a contiguous run of these and leaves everything else alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeSpan {
    /// The top-level node.
    pub node: NodeId,
    /// Index of its first line.
    pub start: usize,
    /// Number of lines it produced (0 when it is hidden).
    pub len: usize,
}

/// Merge neighbouring spans that carry identical styling and provenance.
///
/// The layout engine always produces lines in this merged form, so re-merging
/// after clearing the search flags restores exactly the spans it emitted.
fn merge_spans(spans: &mut Vec<StyledSpan>) {
    let mut out: Vec<StyledSpan> = Vec::with_capacity(spans.len());
    for span in spans.drain(..) {
        match out.last_mut() {
            Some(last) if last.mergeable(&span) => last.text.push_str(&span.text),
            _ => out.push(span),
        }
    }
    *spans = out;
}

/// Byte ranges of every occurrence of `needle` in `haystack`.
///
/// Case-insensitive matching lower-cases both sides. Lower-casing can change
/// a string's byte length (`İ`), which would misplace the offsets, so the
/// insensitive path only runs when both sides lower-case length-preservingly
/// — otherwise nothing is marked, which is a missing highlight and never a
/// wrong one.
fn find_all(haystack: &str, needle: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if needle.is_empty() || haystack.is_empty() {
        return out;
    }
    let (lowered_hay, lowered_pat);
    let (hay, pat) = if case_sensitive {
        (haystack, needle)
    } else {
        lowered_hay = haystack.to_lowercase();
        if lowered_hay.len() != haystack.len() {
            return out;
        }
        lowered_pat = needle.to_lowercase();
        (lowered_hay.as_str(), lowered_pat.as_str())
    };
    if pat.is_empty() {
        return out;
    }
    let mut from = 0usize;
    while let Some(found) = hay.get(from..).and_then(|rest| rest.find(pat)) {
        let start = from + found;
        let end = start + pat.len();
        if !haystack.is_char_boundary(start) || !haystack.is_char_boundary(end) {
            break;
        }
        out.push((start, end));
        from = end.max(start + 1);
    }
    out
}

/// Split a line's spans at the boundaries of every occurrence of `needle` and
/// flag the pieces inside one.
fn mark_line(line: &mut RenderLine, needle: &str, case_sensitive: bool) {
    let text = line.to_text();
    let hits = find_all(&text, needle, case_sensitive);
    if hits.is_empty() {
        return;
    }
    let mut out: Vec<StyledSpan> = Vec::with_capacity(line.spans.len() + hits.len());
    let mut at = 0usize;
    for span in line.spans.drain(..) {
        let start = at;
        let end = at + span.text.len();
        at = end;
        // Cut points inside this span.
        let mut cuts: Vec<usize> = vec![0, span.text.len()];
        for (a, b) in &hits {
            for point in [*a, *b] {
                if point > start && point < end {
                    let rel = point - start;
                    if span.text.is_char_boundary(rel) {
                        cuts.push(rel);
                    }
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for window in cuts.windows(2) {
            let (a, b) = (window[0], window[1]);
            let Some(text) = span.text.get(a..b) else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            let abs = start + a;
            let inside = hits.iter().any(|(s, e)| *s <= abs && *e > abs);
            let mut piece = span.clone();
            piece.text = text.to_string();
            piece.search_match = span.search_match || inside;
            match out.last_mut() {
                Some(last) if last.mergeable(&piece) => last.text.push_str(&piece.text),
                _ => out.push(piece),
            }
        }
    }
    line.spans = out;
}

/// The full laid-out document at one specific width.
#[derive(Debug, Clone, Default)]
pub struct RenderTree {
    /// All lines in document order.
    pub lines: Vec<RenderLine>,
    first_line: HashMap<NodeId, usize>,
    headings: Vec<(usize, NodeId, u8)>,
    max_width: usize,
    pending: Vec<PendingCode>,
    spans: Vec<NodeSpan>,
    tail: usize,
}

impl RenderTree {
    /// Build a tree from lines, computing the node index, heading index and
    /// maximum width. `node_offset` values are (re)assigned here.
    ///
    /// The tree has no node spans and no pending code blocks, so it can be
    /// neither spliced nor lazily highlighted; that is what
    /// [`RenderTree::with_index`] is for.
    pub fn new(lines: Vec<RenderLine>) -> Self {
        let tail = lines.len();
        Self::with_index(lines, Vec::new(), Vec::new(), tail)
    }

    /// Build a tree from lines plus the layout engine's bookkeeping: the
    /// per-top-level-node line spans, the deferred code blocks, and the index
    /// of the first line of the trailing footnote section.
    pub fn with_index(
        mut lines: Vec<RenderLine>,
        pending: Vec<PendingCode>,
        spans: Vec<NodeSpan>,
        tail: usize,
    ) -> Self {
        let mut first_line = HashMap::new();
        let mut headings = Vec::new();
        let mut max_width = 0;
        let mut prev: Option<NodeId> = None;
        let mut offset = 0usize;
        for (idx, line) in lines.iter_mut().enumerate() {
            if prev == Some(line.node) {
                offset += 1;
            } else {
                offset = 0;
                prev = Some(line.node);
            }
            line.node_offset = offset;
            first_line.entry(line.node).or_insert(idx);
            if let LineKind::Heading(level) = line.kind {
                if headings.last().map(|(_, n, _)| *n) != Some(line.node) {
                    headings.push((idx, line.node, level));
                }
            } else if let LineKind::FoldedMarker = line.kind {
                headings.push((idx, line.node, 0));
            }
            max_width = max_width.max(line.width);
        }
        let tail = tail.min(lines.len());
        Self {
            lines,
            first_line,
            headings,
            max_width,
            pending,
            spans,
            tail,
        }
    }

    /// Number of lines.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// `true` if the document produced no lines.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Widest line in columns (the horizontal scroll range).
    pub fn max_width(&self) -> usize {
        self.max_width
    }

    /// Index of the first line produced by `node`, if the node is visible.
    pub fn first_line_of(&self, node: NodeId) -> Option<usize> {
        self.first_line.get(&node).copied()
    }

    /// Line index for the semantic anchor `(node, offset)`; the offset is
    /// clamped to the node's own lines. Returns `None` for hidden nodes.
    pub fn line_index_for(&self, node: NodeId, offset: usize) -> Option<usize> {
        let start = self.first_line_of(node)?;
        let mut last = start;
        for (idx, line) in self.lines.iter().enumerate().skip(start) {
            if line.node != node {
                break;
            }
            last = idx;
            if line.node_offset == offset {
                return Some(idx);
            }
        }
        Some(last)
    }

    /// The node a line came from.
    pub fn node_at(&self, line: usize) -> Option<NodeId> {
        self.lines.get(line).map(|l| l.node)
    }

    /// The semantic anchor `(node, offset)` of a line.
    pub fn anchor_at(&self, line: usize) -> Option<(NodeId, usize)> {
        self.lines.get(line).map(|l| (l.node, l.node_offset))
    }

    /// All heading lines as `(line index, heading node, level)` in document
    /// order. Collapsed section markers are reported with level `0` when they
    /// are not also heading lines.
    pub fn heading_lines(&self) -> &[(usize, NodeId, u8)] {
        &self.headings
    }

    /// Code blocks whose highlighting is still deferred.
    pub fn pending_code(&self) -> &[PendingCode] {
        &self.pending
    }

    /// Remove and return the deferred code blocks overlapping the line range
    /// `[first, last)`. The caller is expected to realize every one of them.
    pub fn take_pending_in(&mut self, first: usize, last: usize) -> Vec<PendingCode> {
        let mut taken = Vec::new();
        let mut kept = Vec::with_capacity(self.pending.len());
        for entry in std::mem::take(&mut self.pending) {
            if entry.start < last && entry.start + entry.len > first {
                taken.push(entry);
            } else {
                kept.push(entry);
            }
        }
        self.pending = kept;
        taken
    }

    /// Line spans of every top-level node, in document order.
    pub fn node_spans(&self) -> &[NodeSpan] {
        &self.spans
    }

    /// Index of the first line of the trailing footnote section (or
    /// [`RenderTree::len`] when the document has none).
    pub fn tail_start(&self) -> usize {
        self.tail
    }

    /// Replace the *content* of `lines.len()` consecutive lines starting at
    /// `start`, keeping their node, kind and `node_offset`.
    ///
    /// Returns `false` — changing nothing — when the range does not exist.
    /// This is the only way deferred highlighting is applied, so a caller can
    /// never move a line by realizing it.
    pub fn replace_line_spans(&mut self, start: usize, lines: Vec<Vec<StyledSpan>>) -> bool {
        let Some(slice) = self.lines.get_mut(start..start + lines.len()) else {
            return false;
        };
        for (line, spans) in slice.iter_mut().zip(lines) {
            line.width = spans.iter().map(StyledSpan::width).sum();
            line.spans = spans;
            self.max_width = self.max_width.max(line.width);
        }
        true
    }

    /// Replace the lines of the top-level nodes `spans[first..first + count]`
    /// with a freshly laid-out block, adjusting every index in the tree.
    ///
    /// `lines`, `pending` and `spans` use offsets relative to the start of the
    /// replaced range. This is the incremental path a fold takes: the rest of
    /// the document keeps its lines and only its offsets move.
    pub fn splice_nodes(
        &mut self,
        first: usize,
        count: usize,
        lines: Vec<RenderLine>,
        pending: Vec<PendingCode>,
        spans: Vec<NodeSpan>,
    ) -> bool {
        if count == 0 || first + count > self.spans.len() {
            return false;
        }
        let start = self.spans[first].start;
        let last = &self.spans[first + count - 1];
        let old_end = last.start + last.len;
        if old_end > self.lines.len() || start > old_end {
            return false;
        }
        let added = lines.len();
        let removed = old_end - start;

        // Nodes that only existed inside the replaced range lose their entry;
        // the ones that survive are re-inserted from the new lines below.
        for line in &self.lines[start..old_end] {
            self.first_line.remove(&line.node);
        }
        self.lines.splice(start..old_end, lines);
        // Recompute `node_offset` for the new block. A top-level node boundary
        // always starts a new node id, so the count restarts at 0 exactly as a
        // full build would have it.
        let mut prev: Option<NodeId> = None;
        let mut offset = 0usize;
        for idx in start..start + added {
            let Some(line) = self.lines.get_mut(idx) else {
                break;
            };
            if prev == Some(line.node) {
                offset += 1;
            } else {
                offset = 0;
                prev = Some(line.node);
            }
            line.node_offset = offset;
        }

        // Shift everything after the range. `added - removed` can be negative,
        // so the arithmetic is done on isize and clamped at zero.
        let delta = added as isize - removed as isize;
        let moved = |value: usize| -> usize { (value as isize + delta).max(0) as usize };

        for entry in self.first_line.values_mut() {
            if *entry >= old_end {
                *entry = moved(*entry);
            }
        }
        for idx in start..start + added {
            if let Some(line) = self.lines.get(idx) {
                self.first_line.entry(line.node).or_insert(idx);
            }
        }

        // Node spans: replace the range, then shift the tail.
        let new_spans: Vec<NodeSpan> = spans
            .into_iter()
            .map(|s| NodeSpan {
                node: s.node,
                start: s.start + start,
                len: s.len,
            })
            .collect();
        let new_count = new_spans.len();
        self.spans.splice(first..first + count, new_spans);
        for entry in self.spans.iter_mut().skip(first + new_count) {
            entry.start = moved(entry.start);
        }

        // Deferred code blocks.
        self.pending
            .retain(|p| p.start < start || p.start >= old_end);
        for entry in &mut self.pending {
            if entry.start >= old_end {
                entry.start = moved(entry.start);
            }
        }
        self.pending.extend(pending.into_iter().map(|mut p| {
            p.start += start;
            p
        }));
        self.pending.sort_by_key(|p| p.start);

        // Headings.
        self.headings
            .retain(|(line, _, _)| *line < start || *line >= old_end);
        for entry in &mut self.headings {
            if entry.0 >= old_end {
                entry.0 = moved(entry.0);
            }
        }
        let mut fresh: Vec<(usize, NodeId, u8)> = Vec::new();
        let mut prev_heading: Option<NodeId> = None;
        for idx in start..start + added {
            let Some(line) = self.lines.get(idx) else {
                break;
            };
            match line.kind {
                LineKind::Heading(level) => {
                    if prev_heading != Some(line.node) {
                        fresh.push((idx, line.node, level));
                        prev_heading = Some(line.node);
                    }
                }
                LineKind::FoldedMarker => {
                    fresh.push((idx, line.node, 0));
                    prev_heading = Some(line.node);
                }
                _ => {}
            }
        }
        self.headings.extend(fresh);
        self.headings.sort_by_key(|(line, _, _)| *line);

        self.tail = moved(self.tail).min(self.lines.len());
        self.max_width = self.lines.iter().map(|l| l.width).max().unwrap_or(0);
        true
    }

    /// Mark every occurrence of `needle` in the lines `[first, last)` as a
    /// search match, splitting spans where necessary.
    ///
    /// Text, widths and line count are untouched — only `search_match` flags
    /// and span boundaries change — so this is safe to do on the drawing path
    /// over an already anchored tree.
    pub fn mark_search(&mut self, first: usize, last: usize, needle: &str, case_sensitive: bool) {
        if needle.is_empty() {
            return;
        }
        let last = last.min(self.lines.len());
        let Some(slice) = self.lines.get_mut(first..last) else {
            return;
        };
        for line in slice {
            mark_line(line, needle, case_sensitive);
        }
    }

    /// The first line in `[first, last)` whose text contains `needle`.
    ///
    /// Read-only counterpart of [`RenderTree::mark_search`], used to place the
    /// viewport on the line that actually carries a match.
    pub fn find_line_with(
        &self,
        first: usize,
        last: usize,
        needle: &str,
        case_sensitive: bool,
    ) -> Option<usize> {
        if needle.is_empty() {
            return None;
        }
        let last = last.min(self.lines.len());
        let slice = self.lines.get(first..last)?;
        slice
            .iter()
            .position(|line| !find_all(&line.to_text(), needle, case_sensitive).is_empty())
            .map(|offset| first + offset)
    }

    /// Undo [`RenderTree::mark_search`] over the lines `[first, last)`.
    pub fn clear_search(&mut self, first: usize, last: usize) {
        let last = last.min(self.lines.len());
        let Some(slice) = self.lines.get_mut(first..last) else {
            return;
        };
        for line in slice {
            if line.spans.iter().any(|s| s.search_match) {
                for span in &mut line.spans {
                    span.search_match = false;
                }
                merge_spans(&mut line.spans);
            }
        }
    }

    /// Plain-text rendering of the whole tree (used for `--color never`
    /// output, snapshot tests and debugging). Trailing spaces are trimmed.
    pub fn to_plain_text(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            out.push_str(line.to_text().trim_end());
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tree under test is a real laid-out document, so these tests do
    /// not encode how the layout engine happens to build a `RenderLine`.
    use crate::testing::render as tree;

    /// The invariant every index in a `RenderTree` must satisfy, whatever was
    /// done to it: the lookup tables and the lines agree.
    ///
    /// This is what the old `node_spans()[2].start == 1` assertion was one
    /// hand-picked instance of; checking the whole relation catches every
    /// other instance too.
    fn assert_index_is_consistent(tree: &RenderTree, what: &str) {
        let mut expected_offset = 0usize;
        let mut prev: Option<NodeId> = None;
        for (idx, line) in tree.lines.iter().enumerate() {
            if prev == Some(line.node) {
                expected_offset += 1;
            } else {
                expected_offset = 0;
                prev = Some(line.node);
                assert_eq!(
                    tree.first_line_of(line.node),
                    Some(idx),
                    "{what}: first line of node {}",
                    line.node
                );
            }
            assert_eq!(
                line.node_offset, expected_offset,
                "{what}: node_offset of line {idx}"
            );
            assert_eq!(tree.node_at(idx), Some(line.node), "{what}: node_at {idx}");
            assert_eq!(
                tree.anchor_at(idx),
                Some((line.node, line.node_offset)),
                "{what}: anchor_at {idx}"
            );
            assert_eq!(
                tree.line_index_for(line.node, line.node_offset),
                Some(idx),
                "{what}: anchor round-trip at {idx}"
            );
            assert_eq!(
                line.width,
                line.spans.iter().map(StyledSpan::width).sum::<usize>(),
                "{what}: cached width of line {idx}"
            );
        }
        assert_eq!(
            tree.max_width(),
            tree.lines.iter().map(|l| l.width).max().unwrap_or(0),
            "{what}: max_width"
        );
        // The node spans tile the body of the document, in order, without
        // gaps or overlaps, and stop where the footnote tail begins. (A span
        // covers a whole top-level node, so its lines may belong to nested
        // nodes — only the first line is the top-level node's own.)
        let mut at = 0usize;
        let mut last_node: Option<NodeId> = None;
        for span in tree.node_spans() {
            assert_eq!(span.start, at, "{what}: node span {} start", span.node);
            assert!(
                span.start + span.len <= tree.len(),
                "{what}: node span {} in range",
                span.node
            );
            if let Some(prev) = last_node {
                assert!(prev < span.node, "{what}: node spans are in document order");
            }
            last_node = Some(span.node);
            at += span.len;
        }
        if !tree.node_spans().is_empty() {
            assert_eq!(at, tree.tail_start(), "{what}: spans end at the tail");
        }
        assert!(tree.tail_start() <= tree.len(), "{what}: tail in range");
    }

    #[test]
    fn a_laid_out_document_has_a_consistent_index() {
        for src in [
            "# A\n\nbody\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            "text[^a]\n\n[^a]: the note\n",
            "",
        ] {
            for width in [12usize, 40, 80] {
                assert_index_is_consistent(&tree(src, width), &format!("{src:?}@{width}"));
            }
        }
    }

    #[test]
    fn marking_and_clearing_a_search_restores_the_original_spans() {
        let mut tree = tree("a needle in **a haystack**\n", 40);
        let original = tree.lines.clone();
        assert_eq!(original[0].to_text(), "a needle in a haystack");

        tree.mark_search(0, tree.len(), "needle", false);
        let marked = &tree.lines[0];
        assert_eq!(
            marked.to_text(),
            original[0].to_text(),
            "marking never changes the text"
        );
        assert_eq!(marked.width, original[0].width, "nor the width");
        let hit = marked
            .spans
            .iter()
            .find(|s| s.search_match)
            .expect("the hit is marked");
        assert_eq!(hit.text, "needle");
        assert!(
            marked
                .spans
                .iter()
                .filter(|s| s.search_match)
                .all(|s| s.text == "needle"),
            "only the hit is marked: {:?}",
            marked.spans
        );
        assert_index_is_consistent(&tree, "marked");

        let len = tree.len();
        tree.clear_search(0, len);
        assert_eq!(tree.lines, original, "clearing restores the spans exactly");
    }

    #[test]
    fn search_marking_is_case_insensitive_on_request() {
        let mut tree = tree("Needle and needle\n", 40);
        let len = tree.len();
        tree.mark_search(0, len, "needle", false);
        let hits: Vec<&str> = tree.lines[0]
            .spans
            .iter()
            .filter(|s| s.search_match)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(hits, ["Needle", "needle"]);

        tree.clear_search(0, len);
        tree.mark_search(0, len, "needle", true);
        let hits: Vec<&str> = tree.lines[0]
            .spans
            .iter()
            .filter(|s| s.search_match)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(hits, ["needle"]);
    }

    /// Realizing deferred highlighting rewrites a line's content and nothing
    /// else: no line may move, change node, or change kind, or the reader's
    /// scroll position would jump under them.
    #[test]
    fn realizing_a_block_keeps_every_line_in_place() {
        let mut tree = tree("para\n\n```rust\nfn main() {}\n```\n", 40);
        let before = tree.lines.clone();
        let target = tree
            .lines
            .iter()
            .position(|l| l.kind == LineKind::Code)
            .expect("a code line");

        assert!(tree.replace_line_spans(target, vec![vec![StyledSpan::new("XY", Style::new())]]));
        let line = &tree.lines[target];
        assert_eq!(line.to_text(), "XY", "the content was replaced");
        assert_eq!(line.width, 2, "the width was recomputed");
        assert_eq!(line.node, before[target].node, "the node is preserved");
        assert_eq!(line.kind, before[target].kind, "the kind is preserved");
        assert_eq!(line.node_offset, before[target].node_offset);
        assert_eq!(tree.len(), before.len(), "no line was added or removed");
        for (idx, (a, b)) in tree.lines.iter().zip(&before).enumerate() {
            if idx != target {
                assert_eq!(a, b, "line {idx} is untouched");
            }
        }

        assert!(
            !tree.replace_line_spans(tree.len(), vec![Vec::new()]),
            "an unknown range is refused"
        );
    }

    /// Hiding a node's lines and putting them back must leave every index in
    /// the tree exactly as it was — this is the fold path's core promise.
    #[test]
    fn splicing_a_node_range_moves_every_index() {
        let mut tree = tree("# A\n\nbody text\n\npara two\n\n- item\n", 40);
        let original = tree.lines.clone();
        let original_tail = tree.tail_start();
        assert!(tree.node_spans().len() >= 3, "several top-level nodes");

        // Hide the second top-level node.
        let victim = tree.node_spans()[1];
        assert!(victim.len > 0);
        let hidden: Vec<RenderLine> = original[victim.start..victim.start + victim.len].to_vec();
        let next_node = tree.node_spans()[2].node;
        let next_before = tree.first_line_of(next_node).expect("a visible node");

        assert!(tree.splice_nodes(
            1,
            1,
            Vec::new(),
            Vec::new(),
            vec![NodeSpan {
                node: victim.node,
                start: 0,
                len: 0,
            }],
        ));
        assert_eq!(tree.len(), original.len() - victim.len);
        assert_eq!(
            tree.first_line_of(victim.node),
            None,
            "the hidden node has no line"
        );
        assert_eq!(
            tree.first_line_of(next_node),
            Some(next_before - victim.len),
            "everything below moved up by exactly the hidden lines"
        );
        assert_eq!(tree.tail_start(), original_tail - victim.len);
        assert_index_is_consistent(&tree, "after hiding");

        // And back again.
        assert!(tree.splice_nodes(
            1,
            1,
            hidden,
            Vec::new(),
            vec![NodeSpan {
                node: victim.node,
                start: 0,
                len: victim.len,
            }],
        ));
        assert_eq!(
            tree.lines, original,
            "re-expanding reproduces the original lines"
        );
        assert_eq!(tree.first_line_of(next_node), Some(next_before));
        assert_eq!(tree.tail_start(), original_tail);
        assert_index_is_consistent(&tree, "after re-expanding");
    }

    #[test]
    fn find_line_with_locates_the_matching_line() {
        let tree = tree("nothing here\n\na NEEDLE here\n", 40);
        let target = tree
            .lines
            .iter()
            .position(|l| l.to_text().contains("NEEDLE"))
            .expect("the needle line");
        let len = tree.len();
        assert_eq!(tree.find_line_with(0, len, "needle", false), Some(target));
        assert_eq!(tree.find_line_with(0, len, "NEEDLE", true), Some(target));
        assert_eq!(tree.find_line_with(0, len, "needle", true), None);
        assert_eq!(tree.find_line_with(0, len, "", false), None);
        assert_eq!(
            tree.find_line_with(0, len + 99, "needle", false),
            Some(target),
            "an over-long range is clamped, not a panic"
        );
        assert_eq!(tree.find_line_with(target + 1, len, "needle", false), None);
    }
}
