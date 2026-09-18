//! Outline sidebar state.
//!
//! The entry list mirrors the document's hierarchy exactly — it is derived
//! from the [`DocumentModel`], never from rendered lines — and carries its own
//! selection and scroll offset so that long documents stay navigable. For
//! Markdown that hierarchy is the heading tree, which is why the sidebar is
//! still called the table of contents there; for JSON and YAML it is the
//! structure of the document itself.
//!
//! # Why the entries are built lazily
//!
//! A structured outline has one entry per node, and a large export has a great
//! many nodes. Building the list when the reader first opens the sidebar keeps
//! that cost off the path to the first frame, and a document whose outline is
//! never opened never pays it at all. Re-deriving it is never a reparse (see
//! [`DocumentModel::outline`]), so a document that changes shape — a fold, a
//! reload — simply invalidates the cache.

use crate::document::{DocumentModel, NodeId, OutlineEntry};

/// Widest the sidebar may grow, however long the entries are.
///
/// The same number as [`crate::app::hints::MIN_DOCUMENT_WIDTH`], and for the
/// same reason: 40 columns is the narrowest thing this program still calls
/// readable, so it is also the most a navigation aid may take from the text.
/// An entry that does not fit is scrolled to, not accommodated.
pub(crate) const MAX_WIDTH: u16 = 40;

/// Narrowest the sidebar may shrink on a screen with room for it.
pub(crate) const MIN_WIDTH: u16 = 12;

/// Sidebar visibility, selection and scroll offset.
#[derive(Debug, Clone, Default)]
pub(crate) struct TocState {
    /// Whether the sidebar is drawn.
    pub(crate) open: bool,
    /// Index of the selected entry.
    pub(crate) selected: usize,
    /// Index of the first drawn entry.
    pub(crate) scroll: usize,
    /// First visible column, for entries wider than [`MAX_WIDTH`].
    pub(crate) h_scroll: usize,
    /// Entries in document order, derived on first use.
    pub(crate) entries: Vec<OutlineEntry>,
    /// Whether [`TocState::entries`] has been derived yet.
    built: bool,
}

impl TocState {
    /// State for a document, closed and not yet derived.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Derive the entries if they are not there yet.
    pub(crate) fn ensure(&mut self, doc: &DocumentModel) {
        if !self.built {
            self.entries = doc.outline();
            self.built = true;
        }
    }

    /// Forget the derived entries; the next [`TocState::ensure`] rebuilds them.
    pub(crate) fn invalidate(&mut self) {
        self.entries.clear();
        self.built = false;
        self.selected = 0;
        self.scroll = 0;
        self.h_scroll = 0;
    }

    /// Number of entries.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the derived list is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Widest entry row in columns, borders excluded.
    ///
    /// One column for the current-entry marker, two per nesting level for the
    /// tree connectors, and the entry itself. Both the marker and the
    /// connectors are one cell wide in either glyph set, so this does not
    /// depend on whether the terminal draws them in Unicode or ASCII.
    pub(crate) fn content_width(&self) -> usize {
        self.entries
            .iter()
            .map(|e| 1 + 2 * e.depth + crate::util::unicode::width(&e.text))
            .max()
            .unwrap_or(0)
    }

    /// Sidebar width for a screen of `total` columns, including the border.
    ///
    /// The sidebar is as wide as its widest entry and no wider, so a document
    /// of short entries gives the columns it does not need back to the text.
    /// Two ceilings bound it: [`MAX_WIDTH`], and a third of the screen so it
    /// can never dominate a narrow terminal. Whatever the ceilings cut off is
    /// reachable by scrolling the sidebar sideways.
    pub(crate) fn width(&self, total: u16) -> u16 {
        let wanted = u16::try_from(self.content_width().saturating_add(1)).unwrap_or(u16::MAX);
        wanted
            .min(MAX_WIDTH)
            .min(total / 3)
            .max(if total >= MIN_WIDTH { MIN_WIDTH } else { total })
            .min(total)
    }

    /// Columns the entries extend past an inner width of `inner`.
    pub(crate) fn max_h_scroll(&self, inner: usize) -> usize {
        self.content_width().saturating_sub(inner)
    }

    /// Scroll the entries sideways by `delta` columns, clamped.
    pub(crate) fn scroll_h(&mut self, delta: isize, inner: usize) {
        let max = self.max_h_scroll(inner) as isize;
        self.h_scroll = (self.h_scroll as isize + delta).clamp(0, max.max(0)) as usize;
    }

    /// The node the selected entry refers to.
    pub(crate) fn selected_node(&self) -> Option<NodeId> {
        self.entries.get(self.selected).map(|e| e.node)
    }

    /// The entry index for a node id.
    pub(crate) fn index_of_node(&self, node: NodeId) -> Option<usize> {
        self.entries.iter().position(|e| e.node == node)
    }

    /// The entry index that best describes where `node` is: its own entry, or
    /// the last entry at or before it in document order.
    ///
    /// The current-entry marker uses this, so a reader scrolling through a
    /// body paragraph or a scalar still sees which part of the outline they
    /// are in.
    pub(crate) fn index_covering(&self, node: NodeId) -> Option<usize> {
        if let Some(exact) = self.index_of_node(node) {
            return Some(exact);
        }
        self.entries
            .iter()
            .rposition(|e| e.node <= node)
    }

    /// Move the selection by `delta` entries, clamped.
    pub(crate) fn move_selection(&mut self, delta: isize, height: usize) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() - 1;
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, last as isize) as usize;
        self.scroll_into_view(height);
    }

    /// Select an entry by index (mouse click), clamped.
    pub(crate) fn select(&mut self, index: usize, height: usize) {
        if self.entries.is_empty() {
            return;
        }
        self.selected = index.min(self.entries.len() - 1);
        self.scroll_into_view(height);
    }

    /// Keep the selection inside a window of `height` rows.
    pub(crate) fn scroll_into_view(&mut self, height: usize) {
        let height = height.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
        let max_scroll = self.entries.len().saturating_sub(height);
        self.scroll = self.scroll.min(max_scroll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{load, FormatRequest, SourceDocument};

    const DOC: &str = "# One\n\ntext\n\n## Two\n\ntext\n\n### Three\n\ntext\n\n# Four\n\ntext\n";

    fn model(name: &str, src: &str) -> DocumentModel {
        load(FormatRequest::Auto, SourceDocument::new(name, src))
            .expect(src)
            .model
    }

    fn built(name: &str, src: &str) -> (DocumentModel, TocState) {
        let doc = model(name, src);
        let mut toc = TocState::new();
        toc.ensure(&doc);
        (doc, toc)
    }

    #[test]
    fn entries_are_not_derived_until_they_are_needed() {
        let doc = model("t.md", DOC);
        let mut toc = TocState::new();
        assert!(toc.is_empty(), "nothing is built at construction");
        toc.ensure(&doc);
        assert_eq!(toc.len(), 4);
        // Deriving twice costs nothing and changes nothing.
        toc.ensure(&doc);
        assert_eq!(toc.len(), 4);
        toc.invalidate();
        assert!(toc.is_empty());
        toc.ensure(&doc);
        assert_eq!(toc.len(), 4);
    }

    #[test]
    fn markdown_entries_mirror_the_heading_hierarchy() {
        let (_, toc) = built("t.md", DOC);
        let shape: Vec<(usize, &str)> = toc
            .entries
            .iter()
            .map(|e| (e.depth, e.text.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![(0, "One"), (1, "Two"), (2, "Three"), (0, "Four")]
        );
    }

    #[test]
    fn structured_entries_mirror_the_document_structure() {
        let (_, toc) = built("t.yaml", "meta:\n  name: nginx\nports:\n  - 80\n");
        let shape: Vec<(usize, &str)> = toc
            .entries
            .iter()
            .map(|e| (e.depth, e.text.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (0, "root"),
                (1, "meta"),
                (2, "name: nginx"),
                (1, "ports"),
                (2, "[0]: 80"),
            ]
        );
    }

    #[test]
    fn selection_clamps_and_scrolls() {
        let (_, mut toc) = built("t.md", DOC);
        toc.move_selection(-5, 2);
        assert_eq!(toc.selected, 0);
        toc.move_selection(99, 2);
        assert_eq!(toc.selected, 3);
        assert_eq!(toc.scroll, 2, "scrolled to keep the selection visible");
        toc.move_selection(-99, 2);
        assert_eq!(toc.selected, 0);
        assert_eq!(toc.scroll, 0);
    }

    #[test]
    fn the_selected_entry_maps_back_to_a_node() {
        let (doc, mut toc) = built("t.md", DOC);
        toc.select(2, 10);
        let node = toc.selected_node().expect("a node");
        assert!(doc.is_structural(node), "an outline entry is a heading");
        assert_eq!(toc.index_of_node(node), Some(2));
    }

    #[test]
    fn the_current_entry_is_the_one_the_cursor_is_under() {
        let (doc, toc) = built("t.md", DOC);
        let second = toc.entries[1].node;
        // A body node just after the "Two" heading belongs to that entry.
        assert_eq!(toc.index_covering(second + 1), Some(1));
        assert_eq!(toc.index_covering(second), Some(1));
        assert_eq!(toc.index_covering(0), Some(0));
        assert_eq!(
            toc.index_covering(doc.node_count() + 10),
            Some(toc.len() - 1),
            "past the end is the last entry"
        );
    }

    #[test]
    fn width_follows_the_widest_entry_within_its_ceilings() {
        let (_, toc) = built("t.md", DOC);
        // "  └ Three" — marker, two levels of connector, five letters.
        assert_eq!(toc.content_width(), 10);
        // Short entries do not claim the full ceiling: content plus border,
        // lifted to the floor a usable sidebar needs.
        assert_eq!(toc.width(120), MIN_WIDTH);

        let long = format!("# {}\n\ntext\n", "a".repeat(80));
        let (_, wide) = built("t.md", &long);
        assert_eq!(wide.width(200), MAX_WIDTH, "capped by MAX_WIDTH");
        assert_eq!(wide.width(60), 20, "capped by a third of the screen");
        assert_eq!(wide.width(30), MIN_WIDTH, "the floor beats a small third");
        assert!(wide.width(10) <= 10, "never wider than the screen itself");
    }

    #[test]
    fn horizontal_scrolling_is_clamped_to_the_overflow() {
        let long = format!("# {}\n\ntext\n", "a".repeat(80));
        let (_, mut toc) = built("t.md", &long);
        // 1 marker + 80 letters, shown through the 39 inner columns of a
        // sidebar at MAX_WIDTH.
        assert_eq!(toc.content_width(), 81);
        assert_eq!(toc.max_h_scroll(39), 42);

        toc.scroll_h(8, 39);
        assert_eq!(toc.h_scroll, 8);
        toc.scroll_h(999, 39);
        assert_eq!(toc.h_scroll, 42, "never past the last column of the text");
        toc.scroll_h(-999, 39);
        assert_eq!(toc.h_scroll, 0, "and never before the first");

        // Nothing to scroll when everything already fits.
        let (_, mut fits) = built("t.md", DOC);
        assert_eq!(fits.max_h_scroll(39), 0);
        fits.scroll_h(8, 39);
        assert_eq!(fits.h_scroll, 0);
    }

    #[test]
    fn a_long_scalar_cannot_decide_the_sidebar_width() {
        let long = "x".repeat(500);
        let (_, toc) = built("t.yaml", &format!("a: 1\nk: \"{long}\"\n"));
        assert!(
            toc.content_width() < 60,
            "previews are trimmed: {}",
            toc.content_width()
        );
    }
}
