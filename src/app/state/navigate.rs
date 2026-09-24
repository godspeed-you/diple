//! Moving through the document: folding, heading jumps, search and links
//!
//! These four groups share one file because they share one job — they all
//! change *where the reader is* — and because they call each other constantly.
//! A search jump reveals a match inside a collapsed section, which unfolds it;
//! following an internal link resolves an anchor to a node and then reveals
//! it exactly the way a search match is revealed; a heading jump and a link
//! jump both scroll with the same leading context. Splitting them into
//! `folds.rs`, `search.rs` and `links.rs` would be four short files tied
//! together by call edges crossing every boundary.
//!
//! Two invariants hold across the whole file. Position is always expressed as
//! a node, never a line number, so it survives the re-layout a fold or a
//! reveal may cause. And every path that changes fold state ends in
//! [`App::after_section_fold`] or [`App::after_fold_change`], the two entry
//! points into the [`layout_cache`](super::layout_cache) splice-or-rebuild
//! decision.
//!
//! Opening an external link spawns a detached child process; [`App::reap_children`]
//! is called from the event loop so those never become zombies.

use std::process::{Command, Stdio};

use super::{App, Mode};
use crate::document::markdown::LinkKind;
use crate::document::{FoldId, LinkId, NodeId};

/// Which fold operation a key requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FoldOp {
    Toggle,
    Collapse,
    Expand,
}

impl App {
    // -- folding ----------------------------------------------------------

    /// A fold changed somewhere in the document: rebuild everything.
    ///
    /// Only [`Action::CollapseAll`]/[`Action::ExpandAll`] still need this;
    /// a single section takes [`App::after_section_fold`].
    pub(super) fn after_fold_change(&mut self) {
        self.invalidate();
        self.ensure_layout();
    }

    /// A single foldable unit was collapsed or expanded.
    ///
    /// Folding changes one contiguous run of rows and nothing else, so the
    /// cached tree is spliced instead of rebuilt, which avoids a full-document
    /// redraw. A full rebuild is still the fallback whenever the splice is not
    /// provably equivalent.
    pub(super) fn after_one_fold(&mut self, fold: FoldId) {
        if self.splice_fold(fold) {
            // The tree now matches the new fold state, so the build key does
            // too; `ensure_layout` must not rebuild what was just spliced.
            self.built = Some(self.build_key());
            self.restore_anchor();
            self.prepare_frame();
        } else {
            self.after_fold_change();
        }
    }

    pub(super) fn fold_current(&mut self, op: FoldOp) {
        if !self.doc.capabilities().folding {
            self.set_message("this document has no foldable structure");
            return;
        }
        let Some(fold) = self.current_fold() else {
            self.set_message("no foldable node here");
            return;
        };
        match op {
            FoldOp::Toggle => {
                self.folds.toggle(fold);
            }
            FoldOp::Collapse => self.folds.collapse(fold),
            FoldOp::Expand => self.folds.expand(fold),
        }
        // Keep the unit's own row at the top so nested folds stay predictable:
        // the heading of a Markdown section, the key row of a container.
        if let Some(node) = self.doc.fold_node(fold) {
            self.anchor = (node, 0);
            self.cursor = node;
        }
        self.after_one_fold(fold);
    }

    // -- structural navigation ---------------------------------------------

    /// Whether the cursor sits on a primary structural node — a Markdown
    /// heading, a JSON/YAML container — including a collapsed one.
    pub(super) fn cursor_on_structural(&self) -> bool {
        self.cursor_node()
            .is_some_and(|node| self.doc.is_structural(node))
    }

    /// `[` / `]`: the previous or next primary structural node.
    ///
    /// Markdown walks its headings; JSON and YAML walk their containers. Both
    /// are "the thing that folds and that the outline nests", which is what
    /// makes one key mean the analogous thing in every format. Nodes without a
    /// row — hidden by a fold — are skipped, so the traversal is stable under
    /// folding.
    pub(super) fn jump_structural(&mut self, forward: bool) {
        if !self.doc.capabilities().hierarchy_navigation {
            self.set_message("this document has no structure to move through");
            return;
        }
        if self.tree.heading_lines().is_empty() {
            self.set_message(self.no_structure_message());
            return;
        }
        let from = self.cursor_node().unwrap_or(0);
        // Backwards from a body row means "the node I am inside", the way `[`
        // has always gone to the heading you are under.
        let start = if !forward && !self.cursor_on_structural() {
            match self.doc.enclosing_structural(from) {
                Some(node) if self.tree.first_line_of(node).is_some() => {
                    self.goto_node(node);
                    return;
                }
                _ => from,
            }
        } else {
            from
        };
        let mut cursor = start;
        loop {
            let next = if forward {
                self.doc.next_structural(cursor)
            } else {
                self.doc.previous_structural(cursor)
            };
            let Some(node) = next else {
                self.set_message(if forward {
                    "no further structural node"
                } else {
                    "no previous structural node"
                });
                return;
            };
            if self.tree.first_line_of(node).is_some() {
                self.goto_node(node);
                return;
            }
            cursor = node;
        }
    }

    /// `{` / `}`: the previous or next sibling, climbing out of a finished
    /// branch the way Markdown's "same or higher level" always has.
    pub(super) fn jump_sibling(&mut self, forward: bool) {
        if !self.doc.capabilities().sibling_navigation {
            self.set_message("this document has no siblings to move between");
            return;
        }
        let Some(from) = self.cursor_node() else {
            self.set_message(self.no_structure_message());
            return;
        };
        let mut cursor = from;
        loop {
            let next = if forward {
                self.doc.next_sibling(cursor)
            } else {
                self.doc.previous_sibling(cursor)
            };
            let Some(node) = next else {
                self.set_message(if forward {
                    "no further sibling"
                } else {
                    "no previous sibling"
                });
                return;
            };
            if self.tree.first_line_of(node).is_some() {
                self.goto_node(node);
                return;
            }
            cursor = node;
        }
    }

    /// Move to the enclosing node.
    pub(super) fn jump_parent(&mut self) {
        let Some(from) = self.cursor_node() else {
            return;
        };
        match self
            .doc
            .parent(from)
            .filter(|n| self.tree.first_line_of(*n).is_some())
        {
            Some(node) => self.goto_node(node),
            None => self.set_message("already at the outermost level"),
        }
    }

    /// Move to the first node inside this one.
    pub(super) fn jump_first_child(&mut self) {
        let Some(from) = self.cursor_node() else {
            return;
        };
        match self
            .doc
            .first_child(from)
            .filter(|n| self.tree.first_line_of(*n).is_some())
        {
            Some(node) => self.goto_node(node),
            None => self.set_message("nothing inside this node"),
        }
    }

    /// Scroll to a node's row and put the cursor on it.
    pub(super) fn goto_node(&mut self, node: NodeId) {
        if let Some(line) = self.tree.landmark_line_of(node) {
            self.scroll_with_context(line);
            self.place_cursor(node);
        }
    }

    /// What to say when a document has no structure to move through.
    fn no_structure_message(&self) -> &'static str {
        match self.doc.kind() {
            crate::document::DocumentKind::Markdown => "document has no headings",
            _ => "document has no structure",
        }
    }

    // -- search -----------------------------------------------------------

    pub(super) fn open_search(&mut self) {
        self.search.saved = self.search.committed.clone();
        self.search.query.clear();
        self.search_folds = Some(self.folds.clone());
        self.mode = Mode::Search;
        self.refresh_search_preview();
    }

    /// Put the folds back the way they were when the search prompt opened,
    /// closing whatever the preview revealed on its way.
    pub(super) fn restore_search_folds(&mut self) {
        let Some(before) = &self.search_folds else {
            return;
        };
        if self.folds == *before {
            return;
        }
        // The preview reveals one match at a time and is restored before the
        // next, so what differs is normally one ancestor chain: one splice.
        let tops: std::collections::BTreeSet<FoldId> = (0..self.folds.len())
            .filter(|&f| self.folds.is_collapsed(f) != before.is_collapsed(f))
            .map(|f| self.doc.outermost_fold(f))
            .collect();
        self.folds = before.clone();
        match tops.iter().collect::<Vec<_>>().as_slice() {
            [top] => self.after_one_fold(**top),
            _ => self.after_fold_change(),
        }
    }

    /// Incremental search: refresh matches and preview the first one at or
    /// after the current position.
    pub(super) fn refresh_search_preview(&mut self) {
        // A prefix of the query may have matched somewhere the full query
        // does not; what it opened is not the reader's to keep.
        self.restore_search_folds();
        self.search.refresh(self.doc.search_index());
        // No `invalidate()`: the query is not a layout input any more, so an
        // incremental search never rebuilds the document.
        self.prepare_frame();
        let from = self.cursor_node().unwrap_or(0);
        if self.search.select_near(from).is_some() {
            self.goto_current_match();
        }
    }

    pub(super) fn cycle_search(&mut self, forward: bool) {
        if !self.search.has_matches() {
            self.set_message(if self.search.committed.is_empty() {
                "no active search".to_string()
            } else {
                format!("pattern not found: {}", self.search.committed)
            });
            return;
        }
        let wrapped = if forward {
            self.search.next_match()
        } else {
            self.search.previous_match()
        };
        self.goto_current_match();
        if wrapped {
            self.set_message(if forward {
                "search wrapped to the top"
            } else {
                "search wrapped to the bottom"
            });
        }
    }

    /// Scroll to the current match, revealing a collapsed section if the match
    /// is hidden inside one.
    pub(super) fn goto_current_match(&mut self) {
        let Some(m) = self.search.current_match() else {
            return;
        };
        self.reveal_node(m.node);
        match self.match_line(m.node) {
            Some(line) => self.scroll_with_context(line),
            None => self.set_message("match is not visible"),
        }
    }

    /// Expand every collapsed ancestor of `node` and re-layout.
    ///
    /// Only the ancestors: a search that reveals a match must not also open
    /// the folds the reader deliberately closed elsewhere.
    pub(super) fn reveal_node(&mut self, node: NodeId) {
        if !self.doc.is_hidden(node, &self.folds) {
            return;
        }
        if let Some(fold) = self.doc.reveal(node, &mut self.folds) {
            self.folds.expand(fold);
            // `reveal` expanded every collapsed ancestor, so the outermost one
            // delimits the range that changed.
            self.after_one_fold(self.doc.outermost_fold(fold));
        }
    }

    /// The line inside `node` that carries the highlighted match, or the
    /// node's first line.
    fn match_line(&self, node: NodeId) -> Option<usize> {
        let first = self.tree.first_line_of(node)?;
        let mut end = first;
        while self.tree.lines.get(end).map(|l| l.node) == Some(node) {
            end += 1;
        }
        // The node's lines may not be painted yet (painting follows the
        // viewport), so the text is searched directly.
        self.tree
            .find_line_with(
                first,
                end,
                &self.search.committed,
                self.search.case_sensitive,
            )
            .or(Some(first))
    }

    // -- links ------------------------------------------------------------

    /// Link ids that are visible in the current render tree, in document
    /// order. Links inside collapsed sections are absent by construction.
    pub(crate) fn visible_links(&self) -> Vec<LinkId> {
        let mut out: Vec<LinkId> = Vec::new();
        for line in &self.tree.lines {
            for span in &line.spans {
                if let Some(id) = span.link {
                    if !out.contains(&id) {
                        out.push(id);
                    }
                }
            }
        }
        out
    }

    pub(super) fn cycle_link(&mut self, forward: bool) {
        let links = self.visible_links();
        if links.is_empty() {
            self.set_message("no links in view");
            return;
        }
        let next = match self
            .selected_link
            .and_then(|id| links.iter().position(|l| *l == id))
        {
            Some(pos) if forward => (pos + 1) % links.len(),
            Some(pos) => (pos + links.len() - 1) % links.len(),
            None if forward => 0,
            None => links.len() - 1,
        };
        let id = links[next];
        self.selected_link = Some(id);
        if let Some(line) = self.link_line(id) {
            self.reveal_line(line);
        }
        if let Some(link) = self.link(id) {
            self.set_message(format!("link: {}", link.url));
        }
    }

    /// A link by id, for the formats that have links.
    pub(crate) fn link(&self, id: LinkId) -> Option<&crate::document::markdown::Link> {
        self.doc.as_markdown()?.links.get(id)
    }

    fn link_line(&self, id: LinkId) -> Option<usize> {
        self.tree
            .lines
            .iter()
            .position(|l| l.spans.iter().any(|s| s.link == Some(id)))
    }

    /// `Enter`: toggle the section when the cursor is on a heading, otherwise
    /// open the selected link.
    pub(super) fn activate(&mut self) {
        if self.mode == Mode::Toc {
            self.toc_jump();
            return;
        }
        if self.mode == Mode::Help {
            self.mode = Mode::Normal;
            return;
        }
        // On a structural row `Enter` folds; anywhere else it follows a link,
        // for the formats that have links. The two never compete for the key
        // because a heading is not a link and a container is not either.
        if self.cursor_on_structural() {
            self.fold_current(FoldOp::Toggle);
        } else if self.doc.capabilities().links {
            self.open_selected_link();
        } else {
            self.fold_current(FoldOp::Toggle);
        }
    }

    pub(super) fn open_selected_link(&mut self) {
        let Some(id) = self
            .selected_link
            .or_else(|| self.visible_links().first().copied())
        else {
            self.set_message("no link selected");
            return;
        };
        self.selected_link = Some(id);
        let Some(link) = self.link(id).cloned() else {
            self.set_message("no link selected");
            return;
        };
        match link.kind {
            LinkKind::Internal(anchor) => self.jump_to_anchor(&anchor),
            LinkKind::External | LinkKind::Relative => self.spawn_opener(&link.url),
        }
    }

    /// Follow an internal `#anchor` link.
    pub(crate) fn jump_to_anchor(&mut self, target: &str) {
        let anchor = target.trim_start_matches('#');
        let Some(node) = self
            .doc
            .as_markdown()
            .and_then(|doc| doc.anchors.resolve(anchor))
        else {
            self.set_message(format!("unknown anchor: #{anchor}"));
            return;
        };
        self.reveal_node(node);
        self.ensure_layout();
        match self.tree.first_line_of(node) {
            Some(line) => self.scroll_with_context(line),
            None => self.set_message(format!("#{anchor} is not visible")),
        }
    }

    /// Spawn the configured opener for an external link, fully detached so a
    /// misbehaving child can never corrupt the terminal.
    fn spawn_opener(&mut self, url: &str) {
        let opener = self.config.links.opener.clone();
        let result = Command::new(&opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match result {
            Ok(child) => {
                self.children.push(child);
                self.set_message(format!("opening {url}"));
            }
            Err(e) => self.set_message(format!("cannot run `{opener}`: {e}")),
        }
    }

    /// Reap finished opener processes (called from the event loop).
    pub(crate) fn reap_children(&mut self) {
        self.children
            .retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::test_support::*;
    use crate::app::state::HEADING_CONTEXT;
    use crate::config::actions::Action;
    use crate::document::markdown::NodeKind;
    use crossterm::event::KeyCode;

    #[test]
    fn heading_navigation_is_semantic_and_ordered() {
        let mut a = app();
        let mut visited = Vec::new();
        for _ in 0..4 {
            a.apply(Action::NextHeading);
            if let Some(node) = a.tree().node_at(a.cursor_line()) {
                if let Some(crate::document::markdown::Node {
                    kind: NodeKind::Heading(h),
                    ..
                }) = md(&a).node(node)
                {
                    visited.push(h.text.clone());
                }
            }
        }
        assert_eq!(visited, vec!["Title", "Alpha", "Alpha Child", "Beta"]);
        a.apply(Action::PreviousHeading);
        let node = a.cursor_node().and_then(|n| md(&a).node(n));
        assert!(
            matches!(node.map(|n| &n.kind), Some(NodeKind::Heading(h)) if h.text == "Alpha Child")
        );
    }

    #[test]
    fn same_level_jumps_skip_nested_headings() {
        let mut a = app();
        a.apply(Action::NextHeading); // Title (H1)
        a.apply(Action::NextHeading); // Alpha (H2)
        a.apply(Action::NextHeadingSameLevel);
        let node = a.cursor_node().and_then(|n| md(&a).node(n));
        assert!(
            matches!(node.map(|n| &n.kind), Some(NodeKind::Heading(h)) if h.text == "Beta"),
            "H2 → H2 skipped the H3, got {node:?}"
        );
        a.apply(Action::PreviousHeadingSameLevel);
        let node = a.cursor_node().and_then(|n| md(&a).node(n));
        assert!(matches!(node.map(|n| &n.kind), Some(NodeKind::Heading(h)) if h.text == "Alpha"));
    }

    #[test]
    fn folding_hides_nested_children() {
        let mut a = app();
        a.apply(Action::NextHeading); // Title
        a.apply(Action::NextHeading); // Alpha
        let before = a.tree().len();
        a.apply(Action::ToggleFold);
        let after = a.tree().len();
        assert!(after < before, "collapsing removed lines");
        let text = a.tree().to_plain_text();
        assert!(!text.contains("Alpha Child"), "nested heading hidden");
        assert!(!text.contains("Alpha body"), "body hidden");
        assert!(text.contains("Beta"), "the sibling stays visible");
        a.apply(Action::ToggleFold);
        assert_eq!(a.tree().len(), before, "expanding restored the lines");
    }

    #[test]
    fn collapse_all_and_expand_all() {
        let mut a = app();
        let expanded = a.tree().len();
        a.apply(Action::CollapseAll);
        let collapsed = a.tree().len();
        assert!(collapsed < expanded);
        assert!(!a.tree().to_plain_text().contains("Alpha body"));
        a.apply(Action::ExpandAll);
        assert_eq!(a.tree().len(), expanded);
    }

    #[test]
    fn enter_on_a_heading_toggles_the_fold() {
        let mut a = app();
        a.apply(Action::NextHeading);
        a.apply(Action::NextHeading);
        let before = a.tree().len();
        a.apply(Action::Activate);
        assert!(a.tree().len() < before);
    }

    #[test]
    fn search_finds_cycles_and_wraps() {
        let mut a = app();
        key(&mut a, '/');
        for c in "needle".chars() {
            key(&mut a, c);
        }
        assert_eq!(a.mode(), Mode::Search);
        assert_eq!(a.search.matches.len(), 2, "incremental search found both");
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.mode(), Mode::Normal);
        assert_eq!(a.search.current, 0);
        a.apply(Action::NextSearch);
        assert_eq!(a.search.current, 1);
        a.apply(Action::NextSearch);
        assert_eq!(a.search.current, 0, "wrapped");
        assert!(a.message().unwrap_or_default().contains("wrapped"));
    }

    #[test]
    fn search_cancel_restores_the_previous_query() {
        let mut a = app();
        key(&mut a, '/');
        for c in "needle".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Enter);
        key(&mut a, '/');
        for c in "zzz".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.search.committed, "needle");
        assert_eq!(a.search.matches.len(), 2);
    }

    #[test]
    fn search_reveals_a_match_inside_a_collapsed_section() {
        let mut a = app();
        a.apply(Action::CollapseAll);
        assert!(!a.tree().to_plain_text().contains("needle"));
        key(&mut a, '/');
        for c in "needle".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Enter);
        let text = a.tree().to_plain_text();
        assert!(text.contains("needle"), "the section was revealed");
        let m = a.search.current_match().expect("a current match");
        assert!(!a.doc.is_hidden(m.node, &a.folds));
    }

    fn type_search(a: &mut App, query: &str) {
        key(a, '/');
        for c in query.chars() {
            key(a, c);
        }
    }

    /// AC-09: the preview reveals each match it passes through, but only the
    /// accepted one keeps its path open. `s` matches inside `meta`; `spec`
    /// is a top-level key and needs nothing opened.
    #[test]
    fn a_search_keeps_only_the_folds_its_accepted_match_needs() {
        let mut a = crate::testing::json_app(r#"{"meta": {"namespace": "n"}, "spec": {"y": 2}}"#);
        a.apply(Action::CollapseAll);
        let collapsed = a.folds.clone();

        type_search(&mut a, "s");
        assert_ne!(a.folds, collapsed, "the preview of `s` opened `meta`");
        for c in "pec".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.folds, collapsed, "`meta` closed again");
        assert_eq!(a.tree().to_plain_text(), {
            let mut b =
                crate::testing::json_app(r#"{"meta": {"namespace": "n"}, "spec": {"y": 2}}"#);
            b.apply(Action::CollapseAll);
            b.tree().to_plain_text()
        });
    }

    #[test]
    fn cancelling_a_search_closes_what_its_preview_opened() {
        let mut a = crate::testing::json_app(r#"{"meta": {"namespace": "n"}, "spec": {"y": 2}}"#);
        a.apply(Action::CollapseAll);
        let collapsed = a.folds.clone();
        type_search(&mut a, "name");
        assert_ne!(a.folds, collapsed);
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.folds, collapsed);
    }

    /// AC-09: a match on a container's own key is visible with the container
    /// collapsed, so only its ancestors open — whether the match was hidden
    /// or not.
    #[test]
    fn a_match_on_a_container_key_opens_only_its_ancestors() {
        let mut a = crate::testing::json_app(
            r#"{"zz": {"q": 1}, "outer": {"zz": {"inner": 1, "more": {"x": 2}}}}"#,
        );
        a.apply(Action::CollapseAll);
        type_search(&mut a, "zz");
        code(&mut a, KeyCode::Enter);
        a.apply(Action::NextSearch);
        let m = a.search.current_match().expect("the nested match");
        assert!(!a.doc.is_hidden(m.node, &a.folds), "the match is visible");
        let own = a.doc.fold_at(m.node).expect("a container");
        assert!(a.folds.is_collapsed(own), "`outer.zz` itself stayed closed");
        let parent = a.folds.parent(own).expect("inside `outer`");
        assert!(!a.folds.is_collapsed(parent), "`outer` was opened");
    }

    #[test]
    fn backspace_narrows_and_widens_the_result_set() {
        let mut a = app();
        key(&mut a, '/');
        for c in "needlez".chars() {
            key(&mut a, c);
        }
        assert!(a.search.matches.is_empty());
        code(&mut a, KeyCode::Backspace);
        assert_eq!(a.search.matches.len(), 2);
    }

    #[test]
    fn link_cycling_skips_hidden_links() {
        let mut a = app();
        let all = a.visible_links();
        assert_eq!(all.len(), 3, "lead, intro and internal anchor links");
        a.apply(Action::CollapseAll);
        let visible = a.visible_links();
        assert!(
            visible.len() < all.len(),
            "links inside collapsed sections are gone: {visible:?}"
        );
        a.apply(Action::NextLink);
        let selected = a.selected_link().expect("a selection");
        assert!(visible.contains(&selected));
    }

    #[test]
    fn internal_anchor_links_jump_to_the_right_node() {
        let mut a = app();
        a.jump_to_anchor("#alpha");
        let node = md(&a).anchors.resolve("alpha").expect("anchor");
        let line = a.tree().first_line_of(node).expect("visible");
        assert_eq!(a.top_line(), line.saturating_sub(HEADING_CONTEXT));
        a.jump_to_anchor("#does-not-exist");
        assert!(a.message().unwrap_or_default().contains("unknown anchor"));
    }

    #[test]
    fn an_unknown_opener_reports_instead_of_crashing() {
        let mut a = app();
        a.config.links.opener = "definitely-not-a-real-opener".to_string();
        a.apply(Action::NextLink);
        a.apply(Action::OpenLink);
        let message = a.message().unwrap_or_default();
        assert!(
            message.contains("definitely-not-a-real-opener"),
            "{message}"
        );
        assert!(!a.should_quit());
    }
}
