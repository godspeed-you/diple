//! Full-text search over the semantic document.
//!
//! The index is a list of *fields*: a piece of text belonging to one semantic
//! node, with a label saying which part of that node it is. Markdown nodes
//! have a single [`MatchField::Body`]; a structured node has as many as it
//! shows — its key, its value, its tag, its anchor, its comment — because a
//! row that renders several distinguishable pieces has to be able to
//! highlight the right one.
//!
//! Entries are built in document order and, within a node, in the order the
//! fields are rendered, so results come out in reading order without sorting.
//! Queries are literal substring searches, case-insensitive by default.

use super::NodeId;

/// Which part of a node a match landed in.
///
/// The renderer uses this to highlight the correct piece of a row: a match in
/// a key and a match in that key's value are both "on" the same row, but only
/// one of the two runs of text should light up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MatchField {
    /// The whole textual content of a node (Markdown).
    #[default]
    Body,
    /// A mapping key, an array index label, or another row label.
    Label,
    /// A scalar value.
    Value,
    /// A YAML comment attached to the node.
    Comment,
    /// An explicit YAML tag.
    Tag,
    /// A YAML anchor definition or alias reference.
    Anchor,
}

/// A search hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Node containing the match.
    pub node: NodeId,
    /// Which field of that node.
    pub field: MatchField,
    /// Byte offset into the field's text (see [`SearchIndex::text`]).
    pub start: usize,
    /// Exclusive byte end offset.
    pub end: usize,
}

#[derive(Debug, Clone)]
struct Entry {
    node: NodeId,
    field: MatchField,
    /// Plain text of the field.
    text: String,
    /// Lower-cased text for case-insensitive search.
    lower: String,
    /// For every byte of `lower`, the byte offset of the originating char in
    /// `text`.
    map: Vec<usize>,
}

/// Flattened searchable text, one entry per (node, field).
#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    entries: Vec<Entry>,
}

/// Accumulates the searchable fields of a document in document order.
///
/// A backend pushes what it renders; nothing else decides what is findable,
/// which is why "is the comment searchable" is answered by the YAML backend
/// calling `push` rather than by a flag somewhere else.
#[derive(Debug, Default)]
pub struct SearchIndexBuilder {
    entries: Vec<Entry>,
}

impl SearchIndexBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve room for `n` fields.
    pub fn with_capacity(n: usize) -> Self {
        Self {
            entries: Vec::with_capacity(n),
        }
    }

    /// Add one searchable field. Empty text is skipped — it can never match.
    pub fn push(&mut self, node: NodeId, field: MatchField, text: impl Into<String>) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        let mut lower = String::with_capacity(text.len());
        let mut map = Vec::with_capacity(text.len());
        for (idx, c) in text.char_indices() {
            for lc in c.to_lowercase() {
                lower.push(lc);
                map.extend(std::iter::repeat(idx).take(lc.len_utf8()));
            }
        }
        self.entries.push(Entry {
            node,
            field,
            text,
            lower,
            map,
        });
    }

    /// The finished index.
    pub fn build(self) -> SearchIndex {
        SearchIndex {
            entries: self.entries,
        }
    }
}

impl SearchIndex {
    /// Plain text of a node's field as indexed (for highlighting / context).
    pub fn text(&self, node: NodeId, field: MatchField) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.node == node && e.field == field)
            .map(|e| e.text.as_str())
    }

    /// Number of indexed fields.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether anything is indexed.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Find all occurrences of `query`, in document order. An empty query
    /// yields no matches.
    pub fn find(&self, query: &str, case_sensitive: bool) -> Vec<Match> {
        if query.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if case_sensitive {
            for e in &self.entries {
                for (start, _) in e.text.match_indices(query) {
                    out.push(Match {
                        node: e.node,
                        field: e.field,
                        start,
                        end: start + query.len(),
                    });
                }
            }
        } else {
            let q: String = query.chars().flat_map(char::to_lowercase).collect();
            for e in &self.entries {
                for (ls, _) in e.lower.match_indices(q.as_str()) {
                    let le = ls + q.len();
                    let start = e.map.get(ls).copied().unwrap_or(0);
                    let end = match e.map.get(le) {
                        Some(&b) => b,
                        None => e.text.len(),
                    };
                    if end > start {
                        out.push(Match {
                            node: e.node,
                            field: e.field,
                            start,
                            end,
                        });
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(fields: &[(NodeId, MatchField, &str)]) -> SearchIndex {
        let mut b = SearchIndexBuilder::new();
        for (node, field, text) in fields {
            b.push(*node, *field, *text);
        }
        b.build()
    }

    #[test]
    fn matches_come_back_in_the_order_the_fields_were_added() {
        let idx = index(&[
            (0, MatchField::Label, "image"),
            (0, MatchField::Value, "nginx:image"),
            (1, MatchField::Label, "image"),
        ]);
        let hits = idx.find("image", false);
        assert_eq!(
            hits.iter()
                .map(|m| (m.node, m.field))
                .collect::<Vec<_>>(),
            [
                (0, MatchField::Label),
                (0, MatchField::Value),
                (1, MatchField::Label)
            ]
        );
        // Offsets index the field, not the node.
        let value = idx.text(0, MatchField::Value).unwrap();
        assert_eq!(&value[hits[1].start..hits[1].end], "image");
    }

    #[test]
    fn case_insensitive_by_default_and_case_sensitive_on_request() {
        let idx = index(&[(0, MatchField::Body, "Hello hello HELLO")]);
        assert_eq!(idx.find("hello", false).len(), 3);
        assert_eq!(idx.find("hello", true).len(), 1);
        assert_eq!(idx.find("Hello", true).len(), 1);
        assert!(idx.find("", false).is_empty());
        assert!(idx.find("zzz", false).is_empty());
    }

    #[test]
    fn unicode_case_folding_keeps_offsets_on_character_boundaries() {
        let idx = index(&[
            (0, MatchField::Body, "Straße ÜBER straße"),
            (1, MatchField::Body, "İstanbul"),
        ]);
        let text = idx.text(0, MatchField::Body).unwrap();
        let hits = idx.find("über", false);
        assert_eq!(hits.len(), 1);
        assert_eq!(&text[hits[0].start..hits[0].end], "ÜBER");
        assert_eq!(idx.find("straße", false).len(), 2);
        for m in idx.find("i\u{307}stanbul", false) {
            let t = idx.text(m.node, m.field).unwrap();
            assert!(t.is_char_boundary(m.start) && t.is_char_boundary(m.end));
        }
    }

    #[test]
    fn an_empty_field_is_not_indexed() {
        let idx = index(&[(0, MatchField::Value, ""), (1, MatchField::Value, "x")]);
        assert_eq!(idx.len(), 1);
        assert!(idx.text(0, MatchField::Value).is_none());
        assert!(!idx.is_empty());
        assert!(SearchIndexBuilder::with_capacity(4).build().is_empty());
    }
}
