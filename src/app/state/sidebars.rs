//! The panes beside the document, and the one toggle that behaves like them:
//! the TOC sidebar, the key hints sidebar and the Mermaid source switch
//!
//! What groups these is a shared consequence rather than a shared subject.
//! Opening or closing either sidebar changes the content width, so each takes
//! the exact path a terminal resize takes — [`App::invalidate`] forces the
//! rebuild and [`App::ensure_layout`] restores the semantic anchor, so the
//! same content stays at the top of the screen. Toggling a diagram between
//! its rendered form and its source changes that node's height, which does
//! the same thing to the lines below it. Every one of them is a re-layout the
//! reader must not feel.
//!
//! The hint context lives here too: which hint groups are shown depends on
//! what is currently on screen — whether a link is visible, whether a diagram
//! is near the cursor — so it is derived from the render tree at draw time and
//! never cached.

use super::{App, Mode};
use crate::app::hints::HintContext;
use crate::document::{DocumentKind, NodeId};
use crate::render::terminal::HintGroup;

impl App {
    // -- TOC --------------------------------------------------------------

    pub(super) fn toggle_toc(&mut self) {
        if !self.doc.capabilities().outline || !self.doc.has_outline() {
            self.set_message(match self.doc.kind() {
                DocumentKind::Markdown => "document has no headings",
                _ => "document has no structure to outline",
            });
            return;
        }
        if self.toc.open && self.mode == Mode::Toc {
            self.toc.open = false;
            self.mode = Mode::Normal;
        } else {
            // The entries are derived here rather than at load time: a large
            // structured document has one per node, and a reader who never
            // opens the sidebar should never pay for them.
            self.toc.ensure(&self.doc);
            self.toc.open = true;
            self.mode = Mode::Toc;
            // Opening it shows the left edge of the outline, whatever the
            // reader had scrolled to before closing it.
            self.toc.h_scroll = 0;
            if let Some(index) = self
                .cursor_node()
                .and_then(|node| self.toc.index_covering(node))
            {
                let height = self.content_height();
                self.toc.select(index, height);
            }
        }
        self.invalidate();
    }

    // -- key hints --------------------------------------------------------

    /// Toggle the right-hand key hints sidebar (`K`).
    ///
    /// Opening or closing it changes the document width, so it takes exactly
    /// the same path a resize takes: [`App::invalidate`] forces the rebuild
    /// and [`App::ensure_layout`] restores the semantic anchor afterwards, so
    /// the same content stays at the top of the screen.
    pub(super) fn toggle_key_hints(&mut self) {
        self.hints.open = !self.hints.open;
        self.invalidate();
        if self.hints.open && self.sidebar_widths().1 == 0 {
            self.set_message("terminal too narrow for the key hints sidebar");
        }
    }

    // -- mouse ------------------------------------------------------------

    /// Toggle mouse reporting (`m`).
    ///
    /// While diple asks the terminal for mouse events, the terminal cannot
    /// use the mouse for its own text selection. Turning reporting off hands
    /// it back: dragging selects and copies exactly as it does in any other
    /// program, at the cost of the wheel and of clicking the sidebars until
    /// it is turned back on. The event loop mirrors the flag onto the
    /// terminal; nothing here writes to it.
    pub(super) fn toggle_mouse(&mut self) {
        if !self.caps.mouse {
            self.set_message("terminal does not report mouse events");
            return;
        }
        self.mouse_on = !self.mouse_on;
        self.set_message(if self.mouse_on {
            "mouse on: wheel scrolls, clicks select"
        } else {
            "mouse off: drag to select text"
        });
    }

    /// The context the hints sidebar selects its rows from.
    pub(crate) fn hint_context(&self) -> HintContext {
        HintContext {
            mode: self.mode,
            can_scroll_horizontally: if self.mode == Mode::Toc {
                self.toc.max_h_scroll(self.toc_inner_width()) > 0
            } else {
                self.tree.max_width() > self.content_width()
            },
            link_in_view: self.link_in_view(),
            capabilities: self.capabilities(),
            format: self.doc.kind(),
            cursor_on_structural: self.cursor_on_structural(),
            near_diagram: self.near_diagram(),
            search_active: self.search.has_matches(),
            mouse_available: self.caps.mouse,
            mouse_on: self.mouse_on,
            tabs: self.views.0,
            panes: self.views.1,
        }
    }

    /// The hint groups for the current context.
    pub(crate) fn hint_groups(&self) -> Vec<HintGroup> {
        crate::app::hints::groups(&self.hint_context(), &self.keymap)
    }

    /// Whether any line inside the viewport carries a link.
    fn link_in_view(&self) -> bool {
        self.tree
            .visible_slice(self.top_line, self.content_height())
            .iter()
            .any(|line| line.spans.iter().any(|s| s.link.is_some()))
    }

    /// Whether a Mermaid diagram is at or near the cursor: the same
    /// "nearest diagram" `s` would act on, and only when it is close enough
    /// to be on screen.
    fn near_diagram(&self) -> bool {
        let Some(node) = self.nearest_diagram() else {
            return false;
        };
        let Some(line) = self.tree.first_line_of(node) else {
            return false;
        };
        let top = self.top_line;
        let bottom = top.saturating_add(self.content_height());
        // At or near: inside the viewport, or within one screen of it, which
        // is exactly the reach `nearest_diagram` gives `s`.
        line < bottom && line + self.content_height() >= top
    }

    /// Jump to the selected TOC entry, keeping the focus in the sidebar.
    ///
    /// A jump is a move, not a decision: the reader is browsing the outline,
    /// and the next `j`/`k` belongs to the outline too. So the mode stays
    /// [`Mode::Toc`] and `j`/`k` keep walking the headings; `Esc` (or `t`)
    /// leaves the sidebar, and that is the only way out of it.
    pub(crate) fn toc_jump(&mut self) {
        let Some(node) = self.toc.selected_node() else {
            return;
        };
        self.reveal_node(node);
        self.ensure_layout();
        // A single-document YAML root is in the outline — it names the
        // document — but deliberately has no row of its own, so going there
        // means going to where the document starts.
        let target = if self.tree.first_line_of(node).is_some() {
            Some(node)
        } else {
            self.doc
                .first_child(node)
                .filter(|n| self.tree.first_line_of(*n).is_some())
        };
        if let Some(target) = target {
            // Through `goto_node`, so that arriving from the outline puts the
            // row in the same place `]` would.
            self.goto_node(target);
        }
    }

    // -- mermaid ----------------------------------------------------------

    /// Toggle between the rendered diagram and its source for the diagram at
    /// or nearest to the cursor (`s`).
    pub(super) fn toggle_mermaid_source(&mut self) {
        let Some(node) = self.nearest_diagram() else {
            self.set_message("no diagram in view");
            return;
        };
        let now_source = self.diagrams.toggle_source(node);
        self.diagram_generation += 1;
        self.anchor = (node, 0);
        self.cursor = node;
        self.invalidate();
        self.ensure_layout();
        self.set_message(if now_source {
            "showing Mermaid source"
        } else {
            "showing rendered diagram"
        });
    }

    /// The Mermaid node closest to the cursor line.
    ///
    /// Mermaid is a Markdown concept, so this asks the Markdown model
    /// directly; a structured document simply has no diagrams and `s` says so.
    fn nearest_diagram(&self) -> Option<NodeId> {
        use crate::document::markdown::NodeKind;
        let markdown = self.doc.as_markdown()?;
        let cursor = self.cursor_line();
        let mut best: Option<(usize, NodeId)> = None;
        for (idx, line) in self.tree.lines.iter().enumerate() {
            let is_mermaid = markdown
                .node(line.node)
                .is_some_and(|n| matches!(n.kind, NodeKind::Mermaid(_)));
            if !is_mermaid {
                continue;
            }
            let distance = idx.abs_diff(cursor);
            if best.map(|(d, _)| distance < d).unwrap_or(true) {
                best = Some((distance, line.node));
            }
        }
        best.map(|(_, node)| node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::test_support::*;
    use crate::config::actions::Action;
    use crate::document::markdown::NodeKind;

    /// The outline and `]` are two ways to ask for the same thing, so they
    /// must put the reader in the same place.
    #[test]
    fn an_outline_jump_lands_where_a_structural_jump_lands() {
        let mut outline = app();
        outline.apply(Action::ToggleToc);
        outline.apply(Action::ScrollDown);
        let target = outline.toc.selected_node().expect("a selection");
        outline.apply(Action::Activate);

        let mut keyed = app();
        while keyed.cursor_node() != Some(target) {
            let before = keyed.cursor_node();
            keyed.apply(Action::NextHeading);
            assert_ne!(keyed.cursor_node(), before, "walked past {target}");
        }
        assert_eq!(outline.top_line(), keyed.top_line());
        assert_eq!(outline.cursor_node(), keyed.cursor_node());
    }

    /// A single-document YAML root names the document but has no row of its
    /// own, so selecting it in the outline goes to where the document starts
    /// rather than doing nothing at all.
    #[test]
    fn an_outline_jump_to_a_rowless_root_goes_to_its_first_entry() {
        let mut a = crate::testing::AppBuilder::over(
            crate::testing::yaml_model("metadata:\n  name: nginx\nspec:\n  replicas: 3\n"),
            "t.yaml",
        )
        .build();
        a.apply(Action::ToggleToc);
        // The cursor starts below the root, so walk up onto it.
        a.apply(Action::ScrollUp);
        assert_eq!(
            a.toc.selected_node(),
            Some(0),
            "the root is the first entry"
        );
        assert!(
            a.tree().first_line_of(0).is_none(),
            "and it has no row of its own"
        );
        a.apply(Action::Activate);
        assert_eq!(
            a.cursor_node(),
            Some(1),
            "the cursor went to the first entry instead"
        );
    }

    #[test]
    fn toc_selection_maps_to_the_right_section() {
        let mut a = app();
        a.apply(Action::ToggleToc);
        assert_eq!(a.mode(), Mode::Toc);
        assert!(a.toc.open);
        a.apply(Action::ScrollDown);
        a.apply(Action::ScrollDown);
        let selected = a.toc.selected_node().expect("selection");
        let heading = md(&a).node(selected).and_then(|n| match &n.kind {
            NodeKind::Heading(h) => Some(h.text.clone()),
            _ => None,
        });
        a.apply(Action::Activate);
        let at_top = a.cursor_node().and_then(|n| md(&a).node(n));
        assert!(
            matches!(at_top.map(|n| &n.kind), Some(NodeKind::Heading(h)) if Some(&h.text) == heading.as_ref()),
            "jumped to {heading:?}"
        );
    }

    #[test]
    fn a_toc_jump_keeps_the_focus_in_the_sidebar() {
        // Browsing the outline is a sequence of jumps, so the sidebar keeps
        // the focus: `j`/`k` go on walking the headings after a jump, and
        // only `Esc` (or `t`) hands the keys back to the document.
        let mut a = app();
        a.apply(Action::ToggleToc);
        a.apply(Action::ScrollDown);
        let selected = a.toc.selected;

        a.apply(Action::Activate);
        assert_eq!(a.mode(), Mode::Toc, "the jump kept the focus");
        assert!(a.toc.open, "and the sidebar stayed open");
        assert_eq!(a.toc.selected, selected, "on the entry it jumped to");

        a.apply(Action::ScrollDown);
        assert_eq!(
            a.toc.selected,
            selected + 1,
            "`j` still moves the TOC selection, not the document"
        );

        a.apply(Action::Cancel);
        assert_eq!(a.mode(), Mode::Normal, "`Esc` is the way out");
        assert!(!a.toc.open);
    }

    #[test]
    fn toc_narrows_the_content_width() {
        let mut a = app();
        let full = a.content_width();
        a.apply(Action::ToggleToc);
        assert!(a.content_width() < full);
        a.apply(Action::Cancel);
        assert!(!a.toc.open);
        assert_eq!(a.content_width(), full);
    }

    #[test]
    fn key_hints_narrow_the_content_and_preserve_the_anchor() {
        // The single most important correctness property of the sidebar: the
        // width change must take the resize path, so the semantically
        // anchored node stays at the top of the screen.
        let mut a = app_with(DOC, (120, 24));
        let full = a.content_width();
        a.scroll_to(12);
        let before = a.anchor();
        let relayouts = a.relayout_count();

        a.apply(Action::ToggleKeyHints);
        assert!(a.hints.open);
        assert!(a.content_width() < full, "the document area shrank");
        assert_eq!(
            a.content_width(),
            full - usize::from(a.sidebar_widths().1),
            "it shrank by exactly the sidebar width"
        );
        assert!(
            a.relayout_count() > relayouts,
            "it re-laid the document out"
        );
        assert_eq!(a.anchor().0, before.0, "the top node survived opening");

        a.apply(Action::ToggleKeyHints);
        assert!(!a.hints.open);
        assert_eq!(a.content_width(), full);
        assert_eq!(a.anchor().0, before.0, "and survived closing again");
    }

    #[test]
    fn a_narrow_terminal_drops_the_hints_sidebar_and_keeps_the_toc() {
        let mut a = app_with(DOC, (120, 24));
        a.apply(Action::ToggleKeyHints);
        let (_, hints) = a.sidebar_widths();
        assert!(hints > 0, "wide enough for the hints alone");

        // Both open: at 80 columns there is no room for both, and the hints
        // give way first.
        a.resize(80, 24);
        a.apply(Action::ToggleToc);
        let (toc, hints) = a.sidebar_widths();
        assert!(toc > 0, "the TOC is navigation and stays");
        assert_eq!(hints, 0, "the hints are discoverability and go first");

        // Too narrow even on their own.
        a.apply(Action::ToggleToc);
        a.resize(48, 24);
        assert_eq!(a.sidebar_widths().1, 0);
        assert!(a.content_width() >= usize::from(crate::app::hints::MIN_DOCUMENT_WIDTH));
    }

    #[test]
    fn hint_context_follows_the_document() {
        let mut a = app_with(DOC, (120, 24));
        let ctx = a.hint_context();
        assert_eq!(ctx.mode, Mode::Normal);
        assert!(!ctx.can_scroll_horizontally, "wrapped prose fits");
        assert!(ctx.link_in_view, "the lead paragraph carries a link");
        assert!(!ctx.near_diagram);
        assert!(!ctx.search_active);

        a.apply(Action::NextHeading);
        assert!(a.hint_context().cursor_on_structural);

        // Horizontal scrolling only when the tree really is wider than the
        // viewport: an unwrapped code block in a narrow terminal.
        let wide = app_with("```text\n0123456789 0123456789 0123456789\n```\n", (20, 10));
        assert!(wide.hint_context().can_scroll_horizontally);

        // A search with matches turns the n/N rows on.
        let mut searched = app_with(DOC, (120, 24));
        searched.search.query = "needle".to_string();
        let index = searched.doc.search_index().clone();
        searched.search.refresh(&index);
        assert!(searched.hint_context().search_active);
    }

    #[test]
    fn hint_context_sees_a_diagram_only_when_it_is_near() {
        let fence = "```mermaid\ngraph LR\nA-->B\n```\n";
        let a = app_with(fence, (80, 24));
        assert!(a.hint_context().near_diagram);

        let filler = "\nfiller paragraph\n".repeat(80);
        let mut far = app_with(&format!("{fence}{filler}"), (80, 24));
        far.scroll_to(usize::MAX);
        assert!(
            !far.hint_context().near_diagram,
            "a diagram a hundred lines above is not `at or near` the cursor"
        );
    }

    #[test]
    fn mermaid_source_toggle_relayouts() {
        let mut a = app_with("# D\n\n```mermaid\ngraph LR\nA --> B\n```\n", (80, 20));
        let node = md(&a)
            .nodes
            .iter()
            .find(|n| matches!(n.kind, NodeKind::Mermaid(_)))
            .map(|n| n.id)
            .expect("a mermaid node");
        assert!(!a.diagrams.shows_source(node));
        a.apply(Action::ToggleMermaidSource);
        assert!(a.diagrams.shows_source(node));
        assert!(a.tree().to_plain_text().contains("graph LR"));
        a.apply(Action::ToggleMermaidSource);
        assert!(!a.diagrams.shows_source(node));
    }

    /// With the sidebar focused the arrows steer the sidebar, not the
    /// document behind it — that is where the reader is looking, and a
    /// heading too long for `MAX_WIDTH` is only reachable this way.
    #[test]
    fn the_arrows_scroll_the_toc_while_it_has_the_focus() {
        let doc = format!("# {}\n\ntext\n", "heading ".repeat(10));
        let mut a = app_with(&doc, (120, 24));
        a.apply(Action::ToggleToc);
        assert_eq!(a.mode(), Mode::Toc);

        let inner = a.toc_inner_width();
        assert_eq!(inner, usize::from(crate::app::toc::MAX_WIDTH) - 1);
        assert!(a.toc.max_h_scroll(inner) > 0, "the heading overflows");
        assert!(
            a.hint_context().can_scroll_horizontally,
            "and the hints say so"
        );

        a.apply(Action::ScrollRight);
        assert_eq!(a.toc.h_scroll, 8);
        assert_eq!(a.h_offset(), 0, "the document stayed put");

        for _ in 0..50 {
            a.apply(Action::ScrollRight);
        }
        assert_eq!(a.toc.h_scroll, a.toc.max_h_scroll(inner));

        // Leaving the sidebar hands the arrows back to the document, and
        // re-opening it starts at the left edge of the outline again.
        a.apply(Action::ToggleToc);
        assert_eq!(a.mode(), Mode::Normal);
        a.apply(Action::ToggleToc);
        assert_eq!(a.toc.h_scroll, 0);
    }

    /// Mouse reporting and the terminal's own text selection cannot both have
    /// the mouse, so the reader can hand it back. The hints offer the key
    /// only where the terminal reports mouse events at all.
    #[test]
    fn the_mouse_can_be_handed_back_to_the_terminal() {
        let mut a = with_mouse(app());
        assert!(a.mouse_on());
        let mouse_row = |a: &App| {
            a.hint_groups()
                .into_iter()
                .flat_map(|g| g.rows)
                .find(|r| r.action == Action::ToggleMouse)
                .map(|r| r.label)
        };
        assert_eq!(mouse_row(&a).as_deref(), Some("select text"));

        a.apply(Action::ToggleMouse);
        assert!(!a.mouse_on(), "the terminal has the mouse now");
        assert_eq!(mouse_row(&a).as_deref(), Some("mouse back on"));
        a.apply(Action::ToggleMouse);
        assert!(a.mouse_on());

        // A terminal that reports nothing has nothing to hand back.
        let mut without = app();
        assert!(!without.caps.mouse && !without.mouse_on());
        assert_eq!(mouse_row(&without), None);
        without.apply(Action::ToggleMouse);
        assert!(!without.mouse_on());
        assert_eq!(
            without.message(),
            Some("terminal does not report mouse events")
        );
    }
}
