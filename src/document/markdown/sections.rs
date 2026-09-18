//! Section hierarchy and fold state.
//!
//! Sections are derived from *top-level* headings only; a heading nested in a
//! blockquote or list item is rendered as a heading but does not fold.
//! Skipped levels (`H1 → H3`) simply nest the deeper heading under the
//! nearest shallower one.

use super::ast::{Document, NodeId, NodeKind};
use crate::document::folds::{FoldId, FoldState};
use crate::document::outline::OutlineEntry;

/// Index into [`Document::sections`].
///
/// A section is also Markdown's foldable unit, so a `SectionId` is exactly a
/// [`FoldId`]: the format-neutral fold state indexes this hierarchy directly
/// rather than through a translation table.
pub type SectionId = FoldId;

/// A foldable section introduced by a top-level heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Own id (== index in `Document::sections`).
    pub id: SectionId,
    /// The heading node.
    pub heading: NodeId,
    /// Heading level 1..=6.
    pub level: u8,
    /// Enclosing section.
    pub parent: Option<SectionId>,
    /// Directly nested sections in document order.
    pub children: Vec<SectionId>,
    /// Direct non-heading top-level nodes belonging to this section (before
    /// the first child section).
    pub body: Vec<NodeId>,
    /// Exclusive end: the first node id that is no longer part of this
    /// section (including children). Equals `node_count` for the last section.
    pub end: NodeId,
}

impl Section {
    /// `true` if `node` (any id, nested or not) lies within this section,
    /// including the heading itself.
    pub fn contains(&self, node: NodeId) -> bool {
        node >= self.heading && node < self.end
    }
}

/// Nesting depth of a section: how many ancestors it has.
///
/// Depth is not the heading level — `# A` followed by `### B` nests B one
/// deep, not two — because the outline shows the hierarchy the document has
/// rather than the levels it happens to have skipped.
pub fn depth_of(doc: &Document, section: SectionId) -> usize {
    let mut depth = 0usize;
    let mut cur = doc.sections.get(section).and_then(|s| s.parent);
    while let Some(parent) = cur {
        depth += 1;
        if depth > doc.sections.len() {
            break;
        }
        cur = doc.sections.get(parent).and_then(|s| s.parent);
    }
    depth
}

/// Build `doc.sections` and `doc.node_section` from top-level headings.
pub fn build(doc: &mut Document) {
    let mut sections: Vec<Section> = Vec::new();
    // Stack of open section ids (ancestors of the current position).
    let mut open: Vec<SectionId> = Vec::new();
    let total = doc.node_count();

    for node in &doc.nodes {
        match &node.kind {
            NodeKind::Heading(h) => {
                // Close sections at the same or deeper level.
                while let Some(&top) = open.last() {
                    if sections[top].level >= h.level {
                        sections[top].end = node.id;
                        open.pop();
                    } else {
                        break;
                    }
                }
                let id = sections.len();
                let parent = open.last().copied();
                sections.push(Section {
                    id,
                    heading: node.id,
                    level: h.level,
                    parent,
                    children: Vec::new(),
                    body: Vec::new(),
                    end: total,
                });
                if let Some(p) = parent {
                    sections[p].children.push(id);
                }
                open.push(id);
            }
            _ => {
                if let Some(&cur) = open.last() {
                    sections[cur].body.push(node.id);
                }
            }
        }
    }

    let mut node_section = vec![None; total];
    for s in &sections {
        for slot in node_section
            .iter_mut()
            .take(s.end.min(total))
            .skip(s.heading)
        {
            *slot = Some(s.id);
        }
    }
    // Ancestor sections cover the same range as their descendants; the loop
    // above processed parents first, so children overwrote them — which is
    // what we want (innermost section wins).
    doc.sections = sections;
    doc.node_section = node_section;
}

impl Document {
    /// The innermost section containing `node` (any id, nested or top-level).
    pub fn section_of(&self, node: NodeId) -> Option<SectionId> {
        self.node_section.get(node).copied().flatten()
    }

    /// Whether `node` is hidden by the fold state. The heading of a collapsed
    /// section stays visible; its body and nested sections are hidden.
    pub fn is_hidden(&self, node: NodeId, folds: &FoldState) -> bool {
        let Some(section) = self.section_of(node) else {
            return false;
        };
        let Some(s) = self.sections.get(section) else {
            return false;
        };
        if s.heading == node {
            folds.ancestor_collapsed(section)
        } else {
            folds.is_collapsed(section) || folds.ancestor_collapsed(section)
        }
    }

    /// Sections whose heading level is `<= level` that come after `from`.
    pub fn next_section_at_or_above(&self, from: SectionId, level: u8) -> Option<SectionId> {
        self.sections
            .iter()
            .skip(from + 1)
            .find(|s| s.level <= level)
            .map(|s| s.id)
    }

    /// Sections whose heading level is `<= level` that come before `from`.
    pub fn previous_section_at_or_above(&self, from: SectionId, level: u8) -> Option<SectionId> {
        self.sections
            .iter()
            .take(from)
            .rev()
            .find(|s| s.level <= level)
            .map(|s| s.id)
    }

    /// The document outline: one entry per section, in document order.
    ///
    /// This is the table of contents, expressed in the format-neutral
    /// vocabulary the sidebar consumes, so that the same widget draws a
    /// Markdown TOC and a JSON structure tree.
    pub fn outline(&self) -> Vec<OutlineEntry> {
        self.sections
            .iter()
            .filter_map(|section| {
                let heading = self.heading_of(section.id)?;
                Some(OutlineEntry {
                    node: section.heading,
                    fold: Some(section.id),
                    depth: depth_of(self, section.id),
                    text: heading.text.clone(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::markdown::parse;

    const DOC: &str = "\
# A
a-body
## A.1
a1-body
### A.1.a
a1a-body
## A.2
a2-body
# B
b-body
";

    #[test]
    fn hierarchy() {
        let doc = parse(DOC);
        assert_eq!(doc.sections.len(), 5);
        let levels: Vec<_> = doc.sections.iter().map(|s| (s.level, s.parent)).collect();
        assert_eq!(
            levels,
            [
                (1, None),
                (2, Some(0)),
                (3, Some(1)),
                (2, Some(0)),
                (1, None)
            ]
        );
        assert_eq!(doc.sections[0].children, [1, 3]);
        assert_eq!(doc.sections[1].children, [2]);
        assert_eq!(doc.sections[0].body, [1]); // "a-body"
        assert_eq!(doc.sections[0].end, doc.sections[4].heading);
        assert_eq!(doc.sections[4].end, doc.node_count());
        assert_eq!(doc.sections[2].end, doc.sections[3].heading);
    }

    #[test]
    fn skipped_levels() {
        let doc = parse("# A\n\n### Deep\n\ntext\n\n## Mid\n\n#### Deeper\n");
        let rel: Vec<_> = doc.sections.iter().map(|s| (s.level, s.parent)).collect();
        assert_eq!(rel, [(1, None), (3, Some(0)), (2, Some(0)), (4, Some(2))]);
        assert_eq!(doc.sections[0].children, [1, 2]);
    }

    #[test]
    fn content_before_first_heading_has_no_section() {
        let doc = parse("intro\n\n# H\n\nbody\n");
        assert_eq!(doc.section_of(0), None);
        assert_eq!(doc.section_of(1), Some(0));
        assert_eq!(doc.section_of(2), Some(0));
    }

    #[test]
    fn section_of_nested_nodes() {
        let doc = parse("# H\n\n- item\n  - nested\n\n> quote\n\n# H2\n");
        let list = &doc.nodes[1];
        let nested_id = list.walk().last().map(|n| n.id).unwrap();
        assert_ne!(nested_id, list.id);
        assert_eq!(doc.section_of(nested_id), Some(0));
        let NodeKind::Quote(q) = &doc.nodes[2].kind else {
            panic!()
        };
        assert_eq!(doc.section_of(q[0].id), Some(0));
        assert_eq!(doc.section_of(doc.nodes[3].id), Some(1));
    }

    /// The format-neutral fold forest must be exactly the section hierarchy:
    /// that identity is what lets `zM`, `zR` and a search reveal work on a
    /// Markdown document through the same code as on a JSON one.
    #[test]
    fn the_fold_forest_is_the_section_hierarchy() {
        let doc = parse(DOC);
        assert_eq!(doc.fold_parents(), [None, Some(0), Some(1), Some(0), None]);
        let folds = FoldState::from_parents(doc.fold_parents());
        assert_eq!(folds.len(), doc.sections.len());
        for section in &doc.sections {
            assert_eq!(folds.parent(section.id), section.parent);
        }
    }

    #[test]
    fn depth_is_nesting_rather_than_heading_level() {
        let doc = parse("# A\n\n### Deep\n\n## Mid\n\n#### Deeper\n");
        let depths: Vec<usize> = doc.sections.iter().map(|s| depth_of(&doc, s.id)).collect();
        assert_eq!(depths, [0, 1, 1, 2]);
    }

    #[test]
    fn hidden_nodes_when_collapsed() {
        let doc = parse(DOC);
        let mut folds = FoldState::from_parents(doc.fold_parents());
        let s = &doc.sections;
        folds.collapse(0); // collapse "A"
        assert!(
            !doc.is_hidden(s[0].heading, &folds),
            "collapsed heading stays visible"
        );
        assert!(doc.is_hidden(s[0].body[0], &folds));
        assert!(doc.is_hidden(s[1].heading, &folds), "child heading hidden");
        assert!(
            doc.is_hidden(s[2].body[0], &folds),
            "grandchild body hidden"
        );
        assert!(
            !doc.is_hidden(s[4].heading, &folds),
            "sibling section unaffected"
        );
        assert!(!doc.is_hidden(s[4].body[0], &folds));

        folds.expand_all();
        folds.collapse(2); // collapse A.1.a only
        assert!(!doc.is_hidden(s[2].heading, &folds));
        assert!(doc.is_hidden(s[2].body[0], &folds));
        assert!(!doc.is_hidden(s[1].body[0], &folds));
        assert!(!doc.is_hidden(s[3].heading, &folds));
    }

    #[test]
    fn reveal_expands_ancestors() {
        let doc = parse(DOC);
        let mut folds = FoldState::from_parents(doc.fold_parents());
        folds.collapse_all();
        let target = doc.sections[2].body[0];
        assert!(doc.is_hidden(target, &folds));
        folds.reveal(2);
        assert!(!doc.is_hidden(target, &folds));
        assert!(!folds.is_collapsed(0));
        assert!(!folds.is_collapsed(1));
        assert!(!folds.is_collapsed(2));
        assert!(folds.is_collapsed(3), "unrelated sections stay collapsed");
        assert!(folds.is_collapsed(4));
    }

    #[test]
    fn same_level_navigation() {
        let doc = parse(DOC);
        assert_eq!(doc.next_section_at_or_above(1, 2), Some(3));
        assert_eq!(doc.next_section_at_or_above(3, 2), Some(4));
        assert_eq!(doc.next_section_at_or_above(4, 1), None);
        assert_eq!(doc.previous_section_at_or_above(3, 2), Some(1));
        assert_eq!(doc.previous_section_at_or_above(4, 1), Some(0));
        assert_eq!(doc.previous_section_at_or_above(0, 1), None);
    }

    #[test]
    fn no_headings() {
        let doc = parse("just text\n\nmore\n");
        assert!(doc.sections.is_empty());
        let folds = FoldState::from_parents(doc.fold_parents());
        assert!(folds.is_empty());
        assert!(!doc.is_hidden(0, &folds));
        assert_eq!(doc.section_of(1), None);
    }
}
