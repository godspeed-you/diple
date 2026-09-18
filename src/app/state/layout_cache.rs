//! The layout cache: when the [`RenderTree`] is rebuilt, when it is spliced,
//! and what the viewport is anchored to.
//!
//! This file exists on its own because it holds a performance *contract*, and
//! a contract interleaved with keyboard handling is a contract nobody reads.
//! Three rules make it up.
//!
//! # 1. The build key must name every layout input
//!
//! [`App::ensure_layout`] rebuilds only when an input actually changed, so a
//! forgotten input means a stale tree on screen — the worst failure mode this
//! module has, because it is silent. Rather than maintain a second list of
//! fields by hand, [`BuildKey`] *is* the options: its `options` field is a
//! [`LayoutFingerprint`] derived structurally from the very [`LayoutOptions`]
//! the tree would be built from, so the correspondence holds by construction
//! and cannot drift when a field is added to the options. Only the two inputs
//! a fingerprint cannot see — the colour level, applied when drawing but baked
//! into the highlighted tree, and the diagram generation, since the options
//! carry only a `&dyn DiagramSource` with no comparable identity — are spelled
//! out as separate fields.
//!
//! # 2. The search query is not a layout input
//!
//! This is deliberate, not an oversight. Match highlighting is painted over
//! the cached tree on the drawing path ([`RenderTree::mark_search`]), so a
//! keystroke in the `/` prompt costs a viewport of painting, not a re-layout
//! of the document. The price is that painted state is *not* part of the key:
//! anything that changes the tree must call [`App::clear_painted`] so the
//! next frame repaints, which is why [`App::splice_section`] and
//! [`App::invalidate`] both do.
//!
//! # 3. Folding one section splices; everything else rebuilds
//!
//! A collapse or expand changes one contiguous run of top-level nodes, so
//! [`App::splice_section`] re-lays out just that run and leaves the rest of
//! the tree alone. It returns `false` — and the caller must then rebuild —
//! whenever the equivalence is not provable: no cached tree to splice into, a
//! section whose node range does not line up, or a range touching footnote
//! definitions, whose layout at the end of the document depends on fold state
//! outside the range. A spliced tree is required to be indistinguishable from
//! a rebuilt one; the tests in this file assert exactly that.
//!
//! Realization is lazy in the same spirit: only the visible lines plus
//! [`REALIZE_LOOKAHEAD`] on each side are syntax-highlighted and
//! search-painted, so opening a large document costs a screen, not a file.
//!
//! Scrolling, link cycling, TOC selection and help scrolling never reach any
//! of this.

use std::time::Instant;

use super::App;
use crate::document::{FoldId, NodeId};
use crate::layout::{LayoutFingerprint, LayoutOptions};
use crate::render::theme::ColorLevel;
use crate::util::viewport::max_top_line;

/// Lines realized (and search-painted) beyond the viewport in both
/// directions, so that a block straddling an edge is ready before it is
/// scrolled into view.
const REALIZE_LOOKAHEAD: usize = 16;

/// The inputs the current [`RenderTree`] was built from. A change in any of
/// them invalidates the cached layout.
///
/// **The rule:** the key must mention every input [`App::layout_options`]
/// reads, or [`App::ensure_layout`] will keep a stale tree on screen. Rather
/// than maintaining a second list of fields by hand, the key *is* the options:
/// [`LayoutFingerprint`] is derived from the very `LayoutOptions` the tree
/// would be built with, so the correspondence holds by construction. Only the
/// two inputs a fingerprint cannot see are spelled out here.
///
/// The search query is deliberately *not* among them: match highlighting is
/// painted over the cached tree on the drawing path
/// ([`RenderTree::mark_search`]), so typing in the `/` prompt costs a viewport,
/// not a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BuildKey {
    /// Everything [`App::layout_options`] puts into the options.
    options: LayoutFingerprint,
    /// The colour level, which the options do not carry (it is applied when
    /// drawing) but which the highlighter bakes into the tree.
    color: ColorLevel,
    /// Bumped whenever the diagram provider's *content* changes; the options
    /// only carry a `&dyn DiagramSource`, which has no comparable identity.
    diagram_generation: u64,
}

impl App {
    pub(super) fn build_key(&self) -> BuildKey {
        BuildKey {
            options: self.layout_options().fingerprint(),
            color: self.color,
            diagram_generation: self.diagram_generation,
        }
    }

    /// Rebuild the render tree if — and only if — a layout input changed.
    ///
    /// Returns `true` when a re-layout actually happened.
    pub(crate) fn ensure_layout(&mut self) -> bool {
        let key = self.build_key();
        if self.built.as_ref() == Some(&key) {
            return false;
        }
        let started = Instant::now();
        let tree = {
            let opts = self.layout_options();
            self.layout.layout(&self.doc, &opts)
        };
        self.tree = tree;
        // A fresh tree carries no search marks.
        self.painted = None;
        self.built = Some(key);
        self.relayouts += 1;
        self.restore_anchor();
        self.prepare_frame();
        if self.debug {
            eprintln!(
                "diple: relayout {} lines at width {} in {:?}",
                self.tree.len(),
                self.content_width(),
                started.elapsed()
            );
        }
        for warning in self.diagrams.take_warnings() {
            self.set_message(warning);
        }
        true
    }

    /// The layout options the current tree is (and must stay) built with.
    ///
    /// [`Layout::realize`] and [`Layout::relayout_nodes`] both need exactly
    /// the options of the original build, so there is one place that spells
    /// them out.
    fn layout_options(&self) -> LayoutOptions<'_> {
        let mut opts = LayoutOptions::new(self.content_width(), &self.theme);
        opts.apply_config(&self.config);
        opts.folds = Some(&self.folds);
        opts.diagrams = &self.diagrams;
        opts.images = self.caps.images.is_some();
        opts.unicode = self.caps.unicode_box;
        opts.footnotes = true;
        // Highlighting the whole document before showing anything is what
        // blows the startup budget; the reader only ever sees one screen
        // (`prepare_frame`).
        opts.lazy_code = true;
        opts
    }

    /// Do everything that must happen between a state change and the next
    /// frame: highlight the code the reader is about to see and repaint the
    /// search highlighting over the visible lines.
    ///
    /// Both are viewport-sized and idempotent, and neither changes a line
    /// count, a line width or the anchor — so calling this immediately before
    /// drawing can never move the page under the reader.
    pub(crate) fn prepare_frame(&mut self) {
        // A deferred re-layout (a resize) is settled here, once, however many
        // events asked for it.
        self.ensure_layout();
        self.realize_visible();
        self.paint_search();
    }

    /// Highlight deferred code just outside the viewport.
    ///
    /// Called only when the event loop is idle, so it never delays a frame,
    /// and bounded to one screen in each direction so an idle diple settles
    /// back to doing nothing. It is what keeps the *first* code block of a new
    /// language from costing a frame when it is scrolled into view: the reader
    /// is normally reading — that is, idle — before they scroll.
    ///
    /// Returns `true` when it did something. Nothing it touches is on screen,
    /// so no redraw is needed.
    pub(crate) fn realize_ahead(&mut self) -> bool {
        if self.tree.pending_code().is_empty() {
            return false;
        }
        let height = self.content_height();
        let first = self.top_line.saturating_sub(height);
        let last = self
            .top_line
            .saturating_add(height.saturating_mul(2))
            .saturating_add(height);
        let mut tree = std::mem::take(&mut self.tree);
        let changed = {
            let opts = self.layout_options();
            self.layout
                .realize(&self.doc, &opts, &mut tree, first, last)
        };
        self.tree = tree;
        changed
    }

    /// Remove the search marks painted for the previous frame.
    ///
    /// Marks live at absolute line indices, so this has to happen *before*
    /// anything splices the tree, or the marks would be stranded at lines the
    /// next repaint no longer visits.
    fn clear_painted(&mut self) {
        if let Some((a, b)) = self.painted.take() {
            self.tree.clear_search(a, b);
        }
    }

    /// The line range prepared for the current frame.
    fn frame_range(&self) -> (usize, usize) {
        let first = self.top_line.saturating_sub(REALIZE_LOOKAHEAD);
        let last = self
            .top_line
            .saturating_add(self.content_height())
            .saturating_add(REALIZE_LOOKAHEAD);
        (first, last)
    }

    /// Highlight the deferred code blocks intersecting the viewport.
    ///
    /// Exactly the viewport, with no look-ahead: this runs in the same frame
    /// that draws, so reading ahead cannot prevent a stall — it can only move
    /// the cost earlier, and the one place where that is felt is the very
    /// first frame. Highlighting a language the document has not shown yet
    /// costs tens of milliseconds, so it is paid when, and only when, the
    /// reader actually reaches that code.
    fn realize_visible(&mut self) {
        if self.tree.pending_code().is_empty() {
            return;
        }
        let first = self.top_line;
        let last = first.saturating_add(self.content_height());
        let mut tree = std::mem::take(&mut self.tree);
        {
            let opts = self.layout_options();
            self.layout
                .realize(&self.doc, &opts, &mut tree, first, last);
        }
        self.tree = tree;
    }

    /// Repaint the search highlighting over the visible lines.
    ///
    /// Marking is a pure span split — same text, same widths — so it is undone
    /// and redone per frame instead of being baked into the layout, which is
    /// what makes incremental search cost a viewport instead of a document.
    fn paint_search(&mut self) {
        let (first, last) = self.frame_range();
        let wanted = if self.search.has_matches() {
            Some((first, last))
        } else {
            None
        };
        if self.painted == wanted && self.painted_query == self.search.committed {
            return;
        }
        self.clear_painted();
        self.painted_query = self.search.committed.clone();
        if let Some((a, b)) = wanted {
            self.tree
                .mark_search(a, b, &self.search.committed, self.search.case_sensitive);
            self.painted = wanted;
        }
    }

    /// Force a re-layout on the next [`App::ensure_layout`].
    pub(crate) fn invalidate(&mut self) {
        self.built = None;
    }

    /// Restore `top_line` from the semantic anchor after a re-layout.
    pub(super) fn restore_anchor(&mut self) {
        let (node, offset) = self.anchor;
        let line = self
            .tree
            .line_index_for(node, offset)
            .or_else(|| self.visible_ancestor_line(node))
            .unwrap_or(0);
        self.top_line = line.min(max_top_line(self.tree.len(), self.content_height()));
        self.clamp_h_offset();
        // The cursor is semantic too: keep it whenever its node survived the
        // re-layout, and only fall back to the top of the viewport otherwise.
        let cursor = self.cursor;
        self.sync_anchor();
        if self.tree.first_line_of(cursor).is_some() {
            self.cursor = cursor;
        }
    }

    /// When the anchored node is hidden — its section or its container was
    /// collapsed — fall back to the nearest visible ancestor's row.
    fn visible_ancestor_line(&self, node: NodeId) -> Option<usize> {
        let mut fold = self.doc.fold_at(node);
        let mut guard = 0usize;
        while let Some(id) = fold {
            if let Some(line) = self
                .doc
                .fold_node(id)
                .and_then(|n| self.tree.first_line_of(n))
            {
                return Some(line);
            }
            fold = self.folds.parent(id);
            guard += 1;
            if guard > self.folds.len() {
                break;
            }
        }
        None
    }

    /// Update the anchor — and the semantic cursor — from the current
    /// `top_line` (called after plain scrolling).
    pub(super) fn sync_anchor(&mut self) {
        if let Some(anchor) = self.tree.anchor_at(self.top_line) {
            self.anchor = anchor;
            self.cursor = anchor.0;
        }
    }

    /// Keep the anchor at the top of the viewport but pin the cursor to
    /// `node` (heading, TOC, search and anchor jumps).
    pub(super) fn place_cursor(&mut self, node: NodeId) {
        if let Some(anchor) = self.tree.anchor_at(self.top_line) {
            self.anchor = anchor;
        }
        self.cursor = node;
    }

    /// React to a terminal resize: only the geometry changes, the anchored
    /// content stays at the top of the screen.
    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        let size = (cols.max(1), rows.max(1));
        if self.size == size {
            // Nothing to re-lay out. `App::new` already built the tree for
            // this size, and the event loop calls `resize` once at startup
            // with exactly that size; invalidating unconditionally made every
            // startup pay for two full layouts. `ensure_layout` still runs —
            // it is keyed on the build key, so it is a no-op unless some other
            // input actually changed.
            self.ensure_layout();
            return;
        }
        self.size = size;
        // A narrower screen gives the sidebar fewer columns, which can leave
        // its horizontal offset past the end of the widest entry.
        let inner = self.toc_inner_width();
        self.toc.h_scroll = self.toc.h_scroll.min(self.toc.max_h_scroll(inner));
        // The rebuild is deferred to the next `App::prepare_frame`. The event
        // loop drains every queued event before it draws, so dragging a window
        // edge — which delivers a resize event per column — costs one
        // re-layout instead of one per event (resize).
        self.invalidate();
    }

    /// How many times the render tree has actually been rebuilt.
    ///
    /// Diagnostics and regression tests (a resize to the same size must not
    /// re-lay the document out).
    #[cfg(test)]
    pub(crate) fn relayout_count(&self) -> u64 {
        self.relayouts
    }

    /// Re-lay out the rows one foldable unit governs, in place.
    ///
    /// Returns `false` when the incremental path does not apply, in which case
    /// the caller must rebuild. The decision of *what* a fold's rows are
    /// belongs to the layout engine, which knows both formats; this method
    /// only owns the cache bookkeeping around it.
    pub(super) fn splice_fold(&mut self, fold: FoldId) -> bool {
        if self.built.is_none() {
            // Some other input already invalidated the tree; splicing into a
            // stale one would keep the staleness.
            return false;
        }
        self.clear_painted();
        let started = Instant::now();
        let mut tree = std::mem::take(&mut self.tree);
        let spliced = {
            let opts = self.layout_options();
            self.layout.relayout_fold(&self.doc, &opts, &mut tree, fold)
        };
        self.tree = tree;
        if spliced && self.debug {
            eprintln!(
                "diple: spliced fold {fold}, {} lines total, in {:?}",
                self.tree.len(),
                started.elapsed()
            );
        }
        spliced
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::test_support::*;
    use crate::config::actions::Action;
    use crossterm::event::KeyCode;

    #[test]
    fn resize_preserves_the_anchored_node() {
        let mut a = app_with(DOC, (80, 12));
        a.apply(Action::PageDown);
        let before = a.anchor().0;
        a.resize(40, 12);
        assert_eq!(a.anchor().0, before, "the top node survived the resize");
        assert_eq!(a.content_width(), 40);
        a.resize(80, 12);
        assert_eq!(a.anchor().0, before);
    }

    /// The cache key must mention *every* input `layout_options` reads.
    /// Regression: it listed a hand-maintained subset, so toggling the table
    /// mode, the column cap, the tab width, the image or Unicode capability
    /// or the palette left a stale tree on screen. The key is now derived
    /// from the options themselves.

    #[test]
    fn every_layout_option_invalidates_the_cache() {
        let src = "| a | b |\n| - | - |\n| 1 | 2 |\n\n\ttabbed\n";
        type Mutation = (&'static str, fn(&mut App));
        let mutations: Vec<Mutation> = vec![
            ("table.mode", |a| {
                a.config.table.mode = crate::config::schema::TableMode::Compact;
            }),
            ("table.max_column_width", |a| {
                a.config.table.max_column_width = 4;
            }),
            ("code.tab_width", |a| a.config.code.tab_width = 8),
            ("caps.images", |a| {
                a.caps.images = crate::terminal::capabilities::ImageSupport::Kitty;
            }),
            ("caps.unicode_box", |a| {
                a.caps.unicode_box = !a.caps.unicode_box;
            }),
            ("theme palette", |a| {
                // Same name, different palette: the key used to carry only
                // the name.
                a.theme.text = a.theme.link;
                a.theme.table_border = a.theme.link;
            }),
            ("width", |a| a.resize(40, 12)),
            ("wrap", |a| a.config.wrap = !a.config.wrap),
            ("code.wrap", |a| a.config.code.wrap = !a.config.code.wrap),
            ("line_numbers", |a| a.config.line_numbers = true),
        ];
        for (what, mutate) in mutations {
            let mut a = app_with(src, (80, 12));
            let before = a.relayout_count();
            mutate(&mut a);
            assert!(a.ensure_layout(), "changing {what} must relayout");
            assert_eq!(a.relayout_count(), before + 1, "{what}");
            assert!(
                !a.ensure_layout(),
                "{what}: an unchanged state must not relayout again"
            );
        }
    }

    #[test]
    fn a_resize_to_the_same_size_never_relayouts() {
        // `App::new` lays the document out for the startup size and
        // the event loop then calls `resize` with exactly that size, so every
        // startup used to pay for two full layouts.
        let mut a = app();
        let size = a.size();
        let before = a.relayout_count();
        assert_eq!(before, 1, "constructing the app lays out exactly once");
        a.resize(size.0, size.1);
        assert_eq!(
            a.relayout_count(),
            before,
            "a resize to the same size must not rebuild the render tree"
        );
        // A real size change relayouts — once, at frame time, however many
        // resize events the terminal delivered in the meantime.
        a.resize(size.0 / 2, size.1);
        a.resize(size.0 / 3, size.1);
        a.resize(size.0 / 4, size.1);
        assert_eq!(
            a.relayout_count(),
            before,
            "a burst of resize events does not rebuild once per event"
        );
        a.prepare_frame();
        assert_eq!(a.relayout_count(), before + 1);
        a.prepare_frame();
        assert_eq!(a.relayout_count(), before + 1, "and not again");
    }

    #[test]
    fn scrolling_never_relayouts() {
        let mut a = app();
        assert!(
            !a.ensure_layout(),
            "layout is up to date after construction"
        );
        a.apply(Action::ScrollDown);
        assert!(
            !a.ensure_layout(),
            "scrolling did not invalidate the layout"
        );
        a.apply(Action::PageDown);
        assert!(!a.ensure_layout());
    }

    #[test]
    fn interaction_stays_inside_the_budget() {
        // Folding and navigation must stay far below the 50 ms hard
        // budget once the caches are warm.
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/readme.md"),
        )
        .expect("the readme fixture");
        let mut a = app_with(&source, (80, 40));
        a.apply(Action::CollapseAll); // warms the syntax and diagram caches
        a.apply(Action::ExpandAll);
        let started = std::time::Instant::now();
        a.apply(Action::CollapseAll);
        a.apply(Action::ExpandAll);
        let per_relayout = started.elapsed() / 2;
        assert!(
            per_relayout < std::time::Duration::from_millis(50),
            "re-layout took {per_relayout:?}, over the 50 ms hard budget"
        );
    }

    // -- incremental layout -----------------------------------------------

    /// The document used for the incremental-layout tests: several sections,
    /// prose long enough to wrap, and code in more than one language.
    const BIG: &str = concat!(
        "# One\n\nOne body with the word needle in it and enough text to wrap.\n\n",
        "```rust\nfn one() -> usize { 1 }\n```\n\n",
        "## One A\n\nNested body text.\n\n",
        "# Two\n\nTwo body text with another needle.\n\n",
        "```python\ndef two():\n    return 2\n```\n\n",
        "# Three\n\nThree body text.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
    );

    /// The tree an app is showing, compared as text plus every index a jump
    /// or an anchor restoration depends on.
    struct Fingerprint {
        text: String,
        headings: Vec<(usize, NodeId, u8)>,
        first_lines: Vec<(NodeId, usize)>,
    }

    fn tree_fingerprint(a: &App) -> Fingerprint {
        Fingerprint {
            text: a.tree().to_plain_text(),
            headings: a.tree().heading_lines().to_vec(),
            first_lines: a
                .doc
                .walk()
                .filter_map(|n| a.tree().first_line_of(n.id).map(|l| (n.id, l)))
                .collect(),
        }
    }

    #[test]
    fn folding_splices_instead_of_rebuilding_the_document() {
        let mut a = app_with(BIG, (60, 10));
        let before = a.relayout_count();
        a.apply(Action::CollapseFold);
        assert_eq!(
            a.relayout_count(),
            before,
            "collapsing one section must not rebuild the whole document"
        );
        a.apply(Action::ExpandFold);
        assert_eq!(a.relayout_count(), before, "and neither must expanding it");
    }

    #[test]
    fn a_spliced_fold_matches_a_full_rebuild() {
        // Every index the reader can navigate by must be exactly what a full
        // re-layout would have produced.
        for section in 0..4usize {
            let mut spliced = app_with(BIG, (60, 10));
            let mut rebuilt = app_with(BIG, (60, 10));
            for app in [&mut spliced, &mut rebuilt] {
                app.scroll_to(0);
                if let Some(s) = app.doc.sections.get(section) {
                    app.cursor = s.heading;
                }
            }
            spliced.apply(Action::ToggleFold);
            rebuilt.folds.toggle(section);
            rebuilt.invalidate();
            rebuilt.ensure_layout();
            assert_eq!(
                tree_fingerprint(&spliced).text,
                tree_fingerprint(&rebuilt).text,
                "section {section}: text"
            );
            assert_eq!(
                tree_fingerprint(&spliced).headings,
                tree_fingerprint(&rebuilt).headings,
                "section {section}: heading index"
            );
            assert_eq!(
                tree_fingerprint(&spliced).first_lines,
                tree_fingerprint(&rebuilt).first_lines,
                "section {section}: node index"
            );
            assert_eq!(spliced.tree().max_width(), rebuilt.tree().max_width());
        }
    }

    #[test]
    fn the_anchor_survives_a_fold_a_resize_and_a_search_jump() {
        let mut a = app_with(BIG, (60, 10));
        // Park the viewport inside the last section.
        a.apply(Action::Bottom);
        a.apply(Action::ScrollUp);
        let anchor = a.anchor();

        assert_eq!(
            a.anchor(),
            a.tree().anchor_at(a.top_line()).expect("an anchored line")
        );

        // Folding the section at the cursor pins its heading to the top of the
        // screen — and the anchor follows it exactly, which is what the splice
        // must not break.
        if let Some(s) = a.doc.sections.first() {
            a.cursor = s.heading;
        }
        a.apply(Action::CollapseFold);
        let heading = a.doc.sections[0].heading;
        assert_eq!(a.anchor(), (heading, 0));
        assert_eq!(a.tree().first_line_of(heading), Some(a.top_line()));
        let _ = anchor;

        // A resize keeps it too.
        let node = a.anchor().0;
        a.resize(40, 10);
        assert_eq!(a.tree().anchor_at(a.top_line()).map(|x| x.0), Some(node));

        // A search jump lands on the node that carries the match, and the
        // anchor follows the viewport.
        a.apply(Action::Search);
        for c in "needle".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Enter);
        let hit = a.search.current_match().expect("a match");
        let line = a.tree().first_line_of(hit.node).expect("visible");
        assert!(
            a.top_line() <= line && line < a.top_line() + a.content_height(),
            "the match is on screen"
        );
        assert_eq!(
            a.anchor(),
            a.tree().anchor_at(a.top_line()).expect("anchor")
        );
    }

    #[test]
    fn an_incremental_search_never_relayouts() {
        let mut a = app_with(BIG, (60, 10));
        let before = a.relayout_count();
        a.apply(Action::Search);
        for c in "needle".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Enter);
        assert_eq!(
            a.relayout_count(),
            before,
            "search highlighting is painted, not laid out"
        );
        // The visible matches really are marked.
        a.prepare_frame();
        let marked: usize = a
            .tree()
            .visible_slice(a.top_line(), a.content_height())
            .iter()
            .flat_map(|l| l.spans.iter())
            .filter(|s| s.search_match)
            .count();
        assert!(marked > 0, "the match on screen is highlighted");

        // Clearing the search removes the marks again.
        a.apply(Action::Cancel);
        a.prepare_frame();
        assert!(a
            .tree()
            .lines
            .iter()
            .all(|l| l.spans.iter().all(|s| !s.search_match)));
    }

    #[test]
    fn folding_during_a_search_leaves_no_stale_highlights() {
        // Marks live at absolute line indices; a splice moves lines, so they
        // have to be cleared before the tree changes shape.
        let mut a = app_with(BIG, (60, 10));
        a.apply(Action::Search);
        for c in "needle".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Enter);
        a.prepare_frame();
        if let Some(s) = a.doc.sections.first() {
            a.cursor = s.heading;
        }
        a.apply(Action::CollapseFold);
        a.prepare_frame();
        let (top, height) = (a.top_line(), a.content_height());
        for (index, line) in a.tree().lines.iter().enumerate() {
            if line.spans.iter().any(|s| s.search_match) {
                assert!(
                    index >= top && index < top + height + REALIZE_LOOKAHEAD,
                    "line {index} is still marked outside the painted viewport"
                );
                assert!(
                    line.to_text().to_lowercase().contains("needle"),
                    "a marked line really contains the query: {:?}",
                    line.to_text()
                );
            }
        }
    }

    #[test]
    fn code_is_highlighted_only_when_it_is_on_screen() {
        let mut a = app_with(BIG, (60, 10));
        assert!(
            !a.tree().pending_code().is_empty(),
            "code below the fold stays deferred"
        );
        let pending = a.tree().pending_code().len();
        // Page through the document: every block the reader passes is
        // realized, in the frame that shows it.
        while a.top_line() < a.tree().len().saturating_sub(a.content_height()) {
            a.apply(Action::PageDown);
            a.prepare_frame();
        }
        assert!(
            a.tree().pending_code().len() < pending,
            "scrolling realizes the code that came into view"
        );
        // Realizing never moves a line.
        let lines = a.tree().len();
        a.prepare_frame();
        assert_eq!(a.tree().len(), lines);
    }
}
