//! GitHub-style heading anchors with de-duplication.

use std::collections::HashMap;

use super::ast::NodeId;

/// Map from anchor slug to the heading node that defines it.
#[derive(Debug, Clone, Default)]
pub struct AnchorIndex {
    by_anchor: HashMap<String, NodeId>,
    /// Anchors in document order.
    order: Vec<String>,
    /// Per base slug, the first suffix that has not been handed out yet.
    ///
    /// Without this the de-duplication loop restarts at `0` for every
    /// duplicate, so the *k*-th heading sharing a slug costs *k* probes and
    /// the whole document costs Θ(n²) (multi-megabyte documents must stay
    /// interactive). The counter makes insertion amortised O(1) while
    /// producing exactly the same anchors: `foo`, `foo-1`, `foo-2`, …
    next_suffix: HashMap<String, usize>,
}

impl AnchorIndex {
    /// Create an empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a heading. `base` is either an explicit `{#id}` attribute or
    /// heading text; it is slugified and de-duplicated (`foo`, `foo-1`, …).
    /// Returns the final anchor.
    pub fn insert(&mut self, base: &str, node: NodeId) -> String {
        let slug = slugify(base);
        // Start where this base slug left off instead of at 0. The loop is
        // kept so that an explicit `{#foo-3}` attribute registered earlier can
        // never be overwritten.
        let mut n = self.next_suffix.get(&slug).copied().unwrap_or(0);
        let mut candidate = if n == 0 {
            slug.clone()
        } else {
            format!("{slug}-{n}")
        };
        while self.by_anchor.contains_key(&candidate) {
            n += 1;
            candidate = format!("{slug}-{n}");
        }
        self.next_suffix.insert(slug, n + 1);
        self.by_anchor.insert(candidate.clone(), node);
        self.order.push(candidate.clone());
        candidate
    }

    /// Resolve an anchor (with or without leading `#`) to a heading node.
    pub fn resolve(&self, anchor: &str) -> Option<NodeId> {
        let key = anchor.strip_prefix('#').unwrap_or(anchor);
        self.by_anchor
            .get(key)
            .or_else(|| self.by_anchor.get(&slugify(key)))
            .copied()
    }

    /// Number of anchors.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Anchors in document order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, NodeId)> + '_ {
        self.order
            .iter()
            .filter_map(|a| self.by_anchor.get(a).map(|id| (a.as_str(), *id)))
    }
}

/// GitHub slug rules: lowercase, drop everything that is not alphanumeric,
/// space, hyphen or underscore (Unicode letters/digits are kept), spaces → `-`.
pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.trim().chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' {
            out.extend(c.to_lowercase());
        } else if c == ' ' {
            out.push('-');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_basic_rules() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("  Foo.Bar, Baz!  "), "foobar-baz");
        assert_eq!(slugify("C++ & Rust"), "c--rust");
        assert_eq!(slugify("snake_case_is_kept"), "snake_case_is_kept");
        assert_eq!(slugify("Already-Hyphen"), "already-hyphen");
    }

    #[test]
    fn slug_unicode() {
        assert_eq!(slugify("Über Straße"), "über-straße");
        assert_eq!(slugify("日本語 見出し"), "日本語-見出し");
        assert_eq!(slugify("Emoji 🚀 Title"), "emoji--title");
    }

    #[test]
    fn dedupe_appends_counter() {
        let mut idx = AnchorIndex::new();
        assert_eq!(idx.insert("Foo", 0), "foo");
        assert_eq!(idx.insert("Foo", 1), "foo-1");
        assert_eq!(idx.insert("foo", 2), "foo-2");
        assert_eq!(idx.insert("Foo 1", 3), "foo-1-1");
        assert_eq!(idx.resolve("#foo-2"), Some(2));
        assert_eq!(idx.resolve("foo"), Some(0));
        assert_eq!(idx.resolve("Foo"), Some(0));
        assert_eq!(idx.resolve("nope"), None);
        assert_eq!(idx.len(), 4);
        assert_eq!(
            idx.iter().map(|(a, _)| a).collect::<Vec<_>>(),
            ["foo", "foo-1", "foo-2", "foo-1-1"]
        );
    }

    #[test]
    fn dedupe_sequence_is_unchanged_for_many_duplicates() {
        // The GitHub scheme is `foo`, `foo-1`, `foo-2`, … — the per-slug
        // counter must not shift, skip or reorder any of them.
        let mut idx = AnchorIndex::new();
        let got: Vec<String> = (0..50).map(|i| idx.insert("Added", i)).collect();
        let mut want = vec!["added".to_string()];
        want.extend((1..50).map(|i| format!("added-{i}")));
        assert_eq!(got, want);
        for (i, anchor) in want.iter().enumerate() {
            assert_eq!(idx.resolve(anchor), Some(i));
        }
    }

    #[test]
    fn an_explicit_anchor_is_never_stolen_by_the_counter() {
        let mut idx = AnchorIndex::new();
        assert_eq!(idx.insert("foo-3", 100), "foo-3");
        let got: Vec<String> = (0..6).map(|i| idx.insert("Foo", i)).collect();
        assert_eq!(got, ["foo", "foo-1", "foo-2", "foo-4", "foo-5", "foo-6"]);
        assert_eq!(idx.resolve("foo-3"), Some(100));
    }

    #[test]
    fn duplicate_heavy_documents_insert_in_linear_time() {
        // Regression guard: 5000 identical headings used to cost ~12.5M hash
        // probes plus as many `format!` allocations.
        let started = std::time::Instant::now();
        let mut idx = AnchorIndex::new();
        for i in 0..5000 {
            let _ = idx.insert("Added", i);
        }
        let elapsed = started.elapsed();
        assert_eq!(idx.len(), 5000);
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "5000 duplicate anchors took {elapsed:?}; the de-duplication is quadratic again"
        );
    }

    #[test]
    fn empty_heading_gets_empty_slug_then_counter() {
        let mut idx = AnchorIndex::new();
        assert_eq!(idx.insert("", 0), "");
        assert_eq!(idx.insert("!!!", 1), "-1");
    }
}
