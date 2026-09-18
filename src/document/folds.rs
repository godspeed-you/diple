//! Fold state, independent of what a fold *is*.
//!
//! A fold target is whatever the backend decided a reader can collapse: a
//! Markdown section, a JSON object, a YAML sequence. All this module knows is
//! that the targets form a forest — each one has an optional parent — and
//! that collapsing one hides everything below it.
//!
//! The targets are numbered densely in document order, which is what makes
//! `zM`/`zR` a single pass and a reveal a walk up a parent chain.

/// Index of a foldable unit within a document, dense and in document order.
pub type FoldId = usize;

/// Per-session fold state indexed by [`FoldId`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FoldState {
    collapsed: Vec<bool>,
    parents: Vec<Option<FoldId>>,
}

impl FoldState {
    /// An all-expanded state over a forest given as one optional parent per
    /// target, in document order.
    pub fn from_parents(parents: Vec<Option<FoldId>>) -> Self {
        Self {
            collapsed: vec![false; parents.len()],
            parents,
        }
    }

    /// Number of fold targets tracked.
    pub fn len(&self) -> usize {
        self.collapsed.len()
    }

    /// `true` if the document has nothing to fold.
    pub fn is_empty(&self) -> bool {
        self.collapsed.is_empty()
    }

    /// The parent of a target, if it has one.
    pub fn parent(&self, fold: FoldId) -> Option<FoldId> {
        self.parents.get(fold).copied().flatten()
    }

    /// Whether `fold` itself is collapsed (ancestors are not considered).
    pub fn is_collapsed(&self, fold: FoldId) -> bool {
        self.collapsed.get(fold).copied().unwrap_or(false)
    }

    /// Toggle a target. Returns the new collapsed state.
    pub fn toggle(&mut self, fold: FoldId) -> bool {
        if let Some(c) = self.collapsed.get_mut(fold) {
            *c = !*c;
            *c
        } else {
            false
        }
    }

    /// Collapse a target.
    pub fn collapse(&mut self, fold: FoldId) {
        if let Some(c) = self.collapsed.get_mut(fold) {
            *c = true;
        }
    }

    /// Expand a target (ancestors unchanged; see [`FoldState::reveal`]).
    pub fn expand(&mut self, fold: FoldId) {
        if let Some(c) = self.collapsed.get_mut(fold) {
            *c = false;
        }
    }

    /// Collapse every target (`zM`).
    pub fn collapse_all(&mut self) {
        self.collapsed.iter_mut().for_each(|c| *c = true);
    }

    /// Expand every target (`zR`).
    pub fn expand_all(&mut self) {
        self.collapsed.iter_mut().for_each(|c| *c = false);
    }

    /// Expand `fold` and all of its ancestors so that its content is visible
    /// (used when jumping to a search match). Unrelated folds are untouched.
    pub fn reveal(&mut self, fold: FoldId) {
        let mut cur = Some(fold);
        let mut guard = 0usize;
        while let Some(f) = cur {
            self.expand(f);
            cur = self.parent(f);
            guard += 1;
            if guard > self.parents.len() {
                break;
            }
        }
    }

    /// `true` if any strict ancestor of `fold` is collapsed.
    pub fn ancestor_collapsed(&self, fold: FoldId) -> bool {
        let mut cur = self.parent(fold);
        let mut guard = 0usize;
        while let Some(p) = cur {
            if self.is_collapsed(p) {
                return true;
            }
            cur = self.parent(p);
            guard += 1;
            if guard > self.parents.len() {
                break;
            }
        }
        false
    }

    /// `true` if `fold` is collapsed or lies under a collapsed ancestor —
    /// i.e. its content is not on screen.
    pub fn content_hidden(&self, fold: FoldId) -> bool {
        self.is_collapsed(fold) || self.ancestor_collapsed(fold)
    }

    /// The collapsed flags, in target order.
    ///
    /// Used as part of the layout cache key: two fold states that collapse
    /// the same targets produce the same render tree.
    pub fn flags(&self) -> &[bool] {
        &self.collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0 ─┬─ 1 ── 2
    ///    └─ 3
    /// 4
    fn forest() -> FoldState {
        FoldState::from_parents(vec![None, Some(0), Some(1), Some(0), None])
    }

    #[test]
    fn collapsing_hides_descendants_but_not_siblings() {
        let mut folds = forest();
        assert_eq!(folds.len(), 5);
        assert!(!folds.is_empty());
        folds.collapse(0);
        assert!(folds.is_collapsed(0));
        assert!(folds.ancestor_collapsed(1));
        assert!(folds.ancestor_collapsed(2));
        assert!(folds.ancestor_collapsed(3));
        assert!(!folds.ancestor_collapsed(4), "a root sibling is untouched");
        assert!(folds.content_hidden(0));
    }

    #[test]
    fn reveal_opens_the_ancestor_chain_and_nothing_else() {
        let mut folds = forest();
        folds.collapse_all();
        folds.reveal(2);
        assert!(!folds.is_collapsed(0));
        assert!(!folds.is_collapsed(1));
        assert!(!folds.is_collapsed(2));
        assert!(folds.is_collapsed(3), "an unrelated branch stays collapsed");
        assert!(folds.is_collapsed(4));
    }

    #[test]
    fn toggling_round_trips_and_out_of_range_ids_are_inert() {
        let mut folds = forest();
        assert!(folds.toggle(1));
        assert!(!folds.toggle(1));
        folds.collapse(99);
        assert!(!folds.toggle(99));
        assert!(!folds.is_collapsed(99));
        assert_eq!(folds.parent(99), None);
        assert!(!folds.ancestor_collapsed(99));
    }

    #[test]
    fn a_cycle_in_the_parents_cannot_hang_a_walk() {
        // Never produced by a backend, but the guards exist so that a bug in
        // one cannot lock the terminal up.
        let mut folds = FoldState::from_parents(vec![Some(1), Some(0)]);
        folds.collapse_all();
        folds.reveal(0);
        assert!(!folds.ancestor_collapsed(0));
    }

    #[test]
    fn flags_describe_the_whole_state() {
        let mut folds = forest();
        folds.collapse(1);
        assert_eq!(folds.flags(), [false, true, false, false, false]);
        folds.expand_all();
        assert!(folds.flags().iter().all(|c| !c));
    }
}
