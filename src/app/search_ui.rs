//! Search prompt state and match cycling.
//!
//! Searching runs over the document's [`SearchIndex`], never over rendered
//! terminal lines, so matches survive re-layout, folding and resizing.
//! The prompt is incremental: every keystroke re-runs the query.

use crate::document::{Match, SearchIndex};

/// State of the `/` prompt and of the current result set.
#[derive(Debug, Clone, Default)]
pub(crate) struct SearchState {
    /// Query as typed (also while the prompt is open).
    pub(crate) query: String,
    /// The query the current match list was produced from.
    pub(crate) committed: String,
    /// All matches, in document order.
    pub(crate) matches: Vec<Match>,
    /// Index of the current match inside [`SearchState::matches`].
    pub(crate) current: usize,
    /// Whether the last search was case sensitive.
    pub(crate) case_sensitive: bool,
    /// The query text before the prompt was opened (restored on `Esc`).
    pub(crate) saved: String,
}

/// Smart case: a query containing an upper-case character is case sensitive,
/// everything else matches case-insensitively.
pub(crate) fn smart_case(query: &str) -> bool {
    query.chars().any(char::is_uppercase)
}

impl SearchState {
    /// `true` when there is at least one match.
    pub(crate) fn has_matches(&self) -> bool {
        !self.matches.is_empty()
    }

    /// The current match, if any.
    pub(crate) fn current_match(&self) -> Option<Match> {
        self.matches.get(self.current).copied()
    }

    /// Re-run the query over the index (incremental search).
    pub(crate) fn refresh(&mut self, index: &SearchIndex) {
        self.case_sensitive = smart_case(&self.query);
        self.matches = if self.query.is_empty() {
            Vec::new()
        } else {
            index.find(&self.query, self.case_sensitive)
        };
        self.committed = self.query.clone();
        if self.current >= self.matches.len() {
            self.current = 0;
        }
    }

    /// Clear the result set (used when the prompt is cancelled).
    pub(crate) fn clear(&mut self) {
        self.query.clear();
        self.committed.clear();
        self.matches.clear();
        self.current = 0;
    }

    /// Select the first match at or after `line_hint` in document order,
    /// where the hint is expressed as a node id. Returns the selected index.
    pub(crate) fn select_near(&mut self, node: usize) -> Option<usize> {
        if self.matches.is_empty() {
            return None;
        }
        let idx = self
            .matches
            .iter()
            .position(|m| m.node >= node)
            .unwrap_or(0);
        self.current = idx;
        Some(idx)
    }

    /// Advance to the next match. Returns `true` when the list wrapped.
    pub(crate) fn next_match(&mut self) -> bool {
        if self.matches.is_empty() {
            return false;
        }
        if self.current + 1 >= self.matches.len() {
            self.current = 0;
            true
        } else {
            self.current += 1;
            false
        }
    }

    /// Step back to the previous match. Returns `true` when the list wrapped.
    pub(crate) fn previous_match(&mut self) -> bool {
        if self.matches.is_empty() {
            return false;
        }
        if self.current == 0 {
            self.current = self.matches.len() - 1;
            true
        } else {
            self.current -= 1;
            false
        }
    }

    /// Status text for the prompt line, e.g. `/needle  (3/12)`.
    pub(crate) fn prompt(&self) -> String {
        if self.matches.is_empty() {
            if self.query.is_empty() {
                "/".to_string()
            } else {
                format!("/{}  (no matches)", self.query)
            }
        } else {
            format!(
                "/{}  ({}/{})",
                self.query,
                self.current + 1,
                self.matches.len()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::markdown::parse;

    fn state(doc_src: &str, query: &str) -> SearchState {
        let doc = parse(doc_src);
        let index = doc.search.clone();
        let mut s = SearchState {
            query: query.to_string(),
            ..SearchState::default()
        };
        s.refresh(&index);
        s
    }

    #[test]
    fn smart_case_detection() {
        assert!(!smart_case("needle"));
        assert!(smart_case("Needle"));
    }

    #[test]
    fn cycling_wraps_in_both_directions() {
        let mut s = state("alpha alpha alpha\n", "alpha");
        assert_eq!(s.matches.len(), 3);
        assert!(!s.next_match());
        assert!(!s.next_match());
        assert_eq!(s.current, 2);
        assert!(s.next_match(), "wraps at the end");
        assert_eq!(s.current, 0);
        assert!(s.previous_match(), "wraps at the start");
        assert_eq!(s.current, 2);
    }

    #[test]
    fn empty_query_has_no_matches() {
        let s = state("alpha\n", "");
        assert!(!s.has_matches());
        assert_eq!(s.prompt(), "/");
    }

    #[test]
    fn prompt_reports_position() {
        let s = state("alpha alpha\n", "alpha");
        assert_eq!(s.prompt(), "/alpha  (1/2)");
        let s = state("alpha\n", "zzz");
        assert_eq!(s.prompt(), "/zzz  (no matches)");
    }
}
