//! Carrying a reader's place from one parse of a document to the next.
//!
//! # Why this exists
//!
//! [`NodeId`]s and [`FoldId`]s are dense and in document order, which makes
//! them cheap and stable *within* one parse — and meaningless across two. When
//! a document is read again because its file changed, a paragraph inserted
//! near the top renumbers everything below it, so the viewport anchor, the
//! cursor and the collapsed folds of the old parse would land on the wrong
//! content if they were simply reused.
//!
//! A [`Relocation`] translates them. It is computed once per reload, from the
//! old and the new model, and then answers "where is this old node now?" and
//! "which new fold is this old fold?" in constant time.
//!
//! # What identity means per format
//!
//! * **Markdown** — a section is named by its heading text and the headings
//!   above it, with an occurrence count for two siblings that share a title. A
//!   node inside a section keeps its distance from the heading, clamped to the
//!   section's own body, so a paragraph added *above* the section does not
//!   move the reader. When the section itself is gone, the nearest preceding
//!   section that survived takes its place: the reader lands on the heading
//!   above where they were.
//! * **JSON and YAML** — a node is named by the step from its parent (a key,
//!   an index, a document of a stream) and its parent's identity, with an
//!   occurrence count for duplicate keys. A node that is gone falls back to
//!   its nearest surviving ancestor.
//!
//! Both are computed in one pass in document order, never by building a full
//! path per node: nesting depth is decided by untrusted input, and a path per
//! node would make a deeply nested document quadratic.
//!
//! A document whose format changed between the two parses has nothing in
//! common with its old self; every lookup then answers `None` and the caller
//! falls back to a position by line.

use std::collections::HashMap;

use super::folds::FoldId;
use super::markdown;
use super::model::DocumentModel;
use super::structured::{NodeRelation, StructuredDocument};
use super::NodeId;

/// The old-to-new translation for one reload.
#[derive(Debug, Clone, Default)]
pub struct Relocation {
    /// For every old node, where it is now (exactly or approximately).
    nodes: Vec<Option<NodeId>>,
    /// For every old fold target, the new fold that is the same unit.
    folds: Vec<Option<FoldId>>,
}

impl Relocation {
    /// Work out where everything in `old` went in `new`.
    pub fn new(old: &DocumentModel, new: &DocumentModel) -> Self {
        match (old, new) {
            (DocumentModel::Markdown(old), DocumentModel::Markdown(new)) => {
                markdown_relocation(old, new)
            }
            (DocumentModel::Structured(old), DocumentModel::Structured(new))
                if old.kind() == new.kind() =>
            {
                structured_relocation(old, new)
            }
            _ => Relocation::default(),
        }
    }

    /// Where `node` of the old document is in the new one.
    ///
    /// Approximate by design: a node that is gone answers with the closest
    /// thing that survived, so the reader stays near where they were.
    pub fn node(&self, node: NodeId) -> Option<NodeId> {
        self.nodes.get(node).copied().flatten()
    }

    /// The new fold that is the same unit as `fold` of the old document.
    ///
    /// Exact by design: a fold is only carried over when the section or the
    /// container it stands for is still there, because collapsing whatever
    /// happens to be nearby would hide content the reader never folded.
    pub fn fold(&self, fold: FoldId) -> Option<FoldId> {
        self.folds.get(fold).copied().flatten()
    }
}

// ---- Markdown ----------------------------------------------------------------

/// Identity of a section relative to its parent: the heading text and how
/// many earlier siblings had the same text.
type SectionStep = (Option<markdown::SectionId>, String, usize);

fn heading_text(doc: &markdown::Document, section: markdown::SectionId) -> String {
    doc.heading_of(section)
        .map(|h| h.text.clone())
        .unwrap_or_default()
}

/// Every section's step, keyed by the step and naming the section.
fn section_index(doc: &markdown::Document) -> HashMap<SectionStep, markdown::SectionId> {
    let mut seen: HashMap<(Option<markdown::SectionId>, String), usize> = HashMap::new();
    let mut index = HashMap::with_capacity(doc.sections.len());
    for section in &doc.sections {
        let text = heading_text(doc, section.id);
        let count = seen.entry((section.parent, text.clone())).or_insert(0);
        index.insert((section.parent, text, *count), section.id);
        *count += 1;
    }
    index
}

/// The first node that no longer belongs to `section`'s own body: its first
/// child's heading, or the end of the section.
fn own_end(doc: &markdown::Document, section: &markdown::Section) -> NodeId {
    section
        .children
        .first()
        .and_then(|child| doc.sections.get(*child))
        .map_or(section.end, |child| child.heading)
}

fn markdown_relocation(old: &markdown::Document, new: &markdown::Document) -> Relocation {
    let index = section_index(new);

    // Sections first, in document order, so a parent is always mapped before
    // its children. A section maps exactly when its parent did and the parent
    // still has a child with the same title at the same occurrence.
    let mut seen: HashMap<(Option<markdown::SectionId>, String), usize> = HashMap::new();
    let mut folds: Vec<Option<FoldId>> = Vec::with_capacity(old.sections.len());
    for section in &old.sections {
        let text = heading_text(old, section.id);
        let count = seen.entry((section.parent, text.clone())).or_insert(0);
        let occurrence = *count;
        *count += 1;
        let parent = match section.parent {
            None => Some(None),
            Some(parent) => folds.get(parent).copied().flatten().map(Some),
        };
        let mapped = parent.and_then(|parent| index.get(&(parent, text, occurrence)).copied());
        folds.push(mapped);
    }

    // The section a vanished one hands its readers to: the nearest preceding
    // section that survived, entered at its heading.
    let mut survivor: Vec<Option<(markdown::SectionId, bool)>> = Vec::with_capacity(folds.len());
    for (id, mapped) in folds.iter().enumerate() {
        let entry = match mapped {
            Some(new_id) => Some((*new_id, true)),
            None => id
                .checked_sub(1)
                .and_then(|previous| survivor[previous])
                .map(|(new_id, _)| (new_id, false)),
        };
        survivor.push(entry);
    }

    let new_count = new.node_count();
    let first_heading = new.sections.first().map_or(new_count, |s| s.heading);
    let nodes = (0..old.node_count())
        .map(|node| {
            if new_count == 0 {
                return None;
            }
            let Some(section) = old.section_of(node) else {
                // Before the first heading: keep the position within the
                // prelude, or start at the top when there is none any more.
                return Some(if first_heading == 0 {
                    0
                } else {
                    node.min(first_heading - 1)
                });
            };
            let Some((target, exact)) = survivor.get(section).copied().flatten() else {
                return Some(0);
            };
            let target = new.sections.get(target)?;
            let delta = if exact {
                node - old.sections.get(section).map_or(node, |s| s.heading)
            } else {
                0
            };
            let last = own_end(new, target).saturating_sub(1).max(target.heading);
            Some((target.heading + delta).min(last))
        })
        .collect();

    Relocation { nodes, folds }
}

// ---- JSON and YAML -------------------------------------------------------------

/// How a node hangs off its parent, as far as identity is concerned.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Step {
    Document(usize),
    Key(String),
    Index(usize),
}

fn step(doc: &StructuredDocument, node: NodeId) -> Option<Step> {
    Some(match &doc.node(node)?.relation {
        NodeRelation::Root { document } => Step::Document(*document),
        NodeRelation::MappingEntry { key } => Step::Key(key.text.clone()),
        NodeRelation::SequenceItem { index } => Step::Index(*index),
    })
}

fn structured_relocation(old: &StructuredDocument, new: &StructuredDocument) -> Relocation {
    // (new parent, step, occurrence) → new node.
    let mut index: HashMap<(Option<NodeId>, Step, usize), NodeId> =
        HashMap::with_capacity(new.node_count());
    let mut seen: HashMap<(Option<NodeId>, Step), usize> = HashMap::new();
    for id in 0..new.node_count() {
        let (Some(node), Some(step)) = (new.node(id), step(new, id)) else {
            continue;
        };
        let count = seen.entry((node.parent, step.clone())).or_insert(0);
        index.insert((node.parent, step, *count), id);
        *count += 1;
    }

    // Pre-order numbering puts every parent before its children, so one pass
    // in id order always finds the parent already mapped.
    let mut exact: Vec<Option<NodeId>> = Vec::with_capacity(old.node_count());
    let mut nodes: Vec<Option<NodeId>> = Vec::with_capacity(old.node_count());
    let mut seen: HashMap<(Option<NodeId>, Step), usize> = HashMap::new();
    for id in 0..old.node_count() {
        let (Some(node), Some(step)) = (old.node(id), step(old, id)) else {
            exact.push(None);
            nodes.push(None);
            continue;
        };
        let count = seen.entry((node.parent, step.clone())).or_insert(0);
        let occurrence = *count;
        *count += 1;
        let new_parent = match node.parent {
            None => Some(None),
            Some(parent) => exact.get(parent).copied().flatten().map(Some),
        };
        let mapped = new_parent.and_then(|parent| index.get(&(parent, step, occurrence)).copied());
        exact.push(mapped);
        let approximate = mapped.or_else(|| {
            node.parent
                .and_then(|parent| nodes.get(parent).copied().flatten())
        });
        nodes.push(approximate.or_else(|| (new.node_count() > 0).then_some(0)));
    }

    let folds = (0..old.fold_parents().len())
        .map(|fold| {
            let node = exact.get(old.fold_node(fold)?).copied().flatten()?;
            new.node(node)?.fold
        })
        .collect();

    Relocation { nodes, folds }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{json_model, model, yaml_model};

    /// The node of `doc` whose heading text is `title`.
    fn heading(doc: &DocumentModel, title: &str) -> NodeId {
        let md = doc.as_markdown().expect("markdown");
        md.sections
            .iter()
            .find(|s| heading_text(md, s.id) == title)
            .map(|s| s.heading)
            .unwrap_or_else(|| panic!("no heading {title}"))
    }

    #[test]
    fn a_paragraph_above_does_not_move_a_section_or_its_body() {
        let old = model("# A\n\none\n\n# B\n\ntwo\n\nthree\n");
        let new = model("intro\n\n# A\n\none\n\nadded\n\n# B\n\ntwo\n\nthree\n");
        let r = Relocation::new(&old, &new);
        let (old_b, new_b) = (heading(&old, "B"), heading(&new, "B"));
        assert_eq!(r.node(old_b), Some(new_b));
        assert_eq!(r.node(old_b + 2), Some(new_b + 2), "`three`, two below B");
        assert_eq!(r.fold(1), Some(1), "section B is still the second fold");
    }

    #[test]
    fn a_node_past_the_end_of_a_shortened_section_stays_in_it() {
        let old = model("# A\n\none\n\ntwo\n\nthree\n\n# B\n");
        let new = model("# A\n\none\n\n# B\n");
        let r = Relocation::new(&old, &new);
        let a = heading(&new, "A");
        assert_eq!(
            r.node(heading(&old, "A") + 3),
            Some(a + 1),
            "clamped to `one`"
        );
    }

    #[test]
    fn a_vanished_section_hands_over_to_the_one_above() {
        let old = model("# A\n\none\n\n# Gone\n\ntwo\n\n# C\n");
        let new = model("# A\n\none\n\n# C\n");
        let r = Relocation::new(&old, &new);
        assert_eq!(r.node(heading(&old, "Gone") + 1), Some(heading(&new, "A")));
        assert_eq!(
            r.fold(1),
            None,
            "a fold is never carried to another section"
        );
        assert_eq!(r.fold(2), Some(1), "C is still C");
    }

    #[test]
    fn duplicate_titles_are_told_apart_by_occurrence_and_parent() {
        let old = model("# A\n\n## Notes\n\n# B\n\n## Notes\n\n## Notes\n");
        let new = model("# A\n\n## Notes\n\n# B\n\n## Notes\n\n## Notes\n");
        let r = Relocation::new(&old, &new);
        for fold in 0..5 {
            assert_eq!(r.fold(fold), Some(fold));
        }
        // Renaming A loses A and the `Notes` below it, and only those.
        let renamed = model("# A2\n\n## Notes\n\n# B\n\n## Notes\n\n## Notes\n");
        let r = Relocation::new(&old, &renamed);
        assert_eq!((r.fold(0), r.fold(1)), (None, None));
        assert_eq!(
            (r.fold(2), r.fold(3), r.fold(4)),
            (Some(2), Some(3), Some(4))
        );
    }

    #[test]
    fn the_prelude_keeps_its_place_or_yields_the_top() {
        let old = model("one\n\ntwo\n\n# A\n");
        let r = Relocation::new(&old, &model("one\n\ntwo\n\n# A\n"));
        assert_eq!(r.node(1), Some(1));
        let r = Relocation::new(&old, &model("# A\n"));
        assert_eq!(r.node(1), Some(0));
        let r = Relocation::new(&old, &model(""));
        assert_eq!(r.node(1), None, "nothing to land on in an empty document");
    }

    #[test]
    fn a_key_added_before_does_not_move_a_json_node() {
        let old = json_model(r#"{"a": 1, "b": {"c": 2}}"#);
        let new = json_model(r#"{"new": 0, "a": 1, "b": {"x": 9, "c": 2}}"#);
        let r = Relocation::new(&old, &new);
        let path = |doc: &DocumentModel, node| doc.path(node).map(|p| p.breadcrumb(false));
        // old: 0 root, 1 a, 2 b, 3 c
        for node in 0..4 {
            let mapped = r.node(node).expect("mapped");
            assert_eq!(path(&new, mapped), path(&old, node), "node {node}");
        }
        let b = (0..old.fold_parents().len())
            .find(|f| old.fold_node(*f) == Some(2))
            .expect("b is a fold target");
        assert_eq!(new.fold_node(r.fold(b).expect("b still folds")), r.node(2));
    }

    #[test]
    fn a_removed_json_node_falls_back_to_its_container() {
        let old = json_model(r#"{"b": {"c": 2, "d": 3}}"#);
        let new = json_model(r#"{"b": {"c": 2}}"#);
        let r = Relocation::new(&old, &new);
        assert_eq!(r.node(3), Some(1), "`d` is gone, `b` is not");
    }

    #[test]
    fn duplicate_keys_and_yaml_streams_keep_their_identity() {
        let old = json_model(r#"{"k": 1, "k": 2}"#);
        let r = Relocation::new(&old, &json_model(r#"{"k": 1, "k": 2}"#));
        assert_eq!((r.node(1), r.node(2)), (Some(1), Some(2)));

        let old = yaml_model("a: 1\n---\na: 2\n");
        let new = yaml_model("a: 1\n---\nz: 0\na: 2\n");
        let r = Relocation::new(&old, &new);
        let path = |doc: &DocumentModel, node| doc.path(node).map(|p| p.breadcrumb(false));
        let last = old.node_count() - 1;
        assert_eq!(
            path(&new, r.node(last).expect("mapped")),
            path(&old, last),
            "the second document's `a`"
        );
    }

    #[test]
    fn a_change_of_format_shares_nothing() {
        let r = Relocation::new(&model("# A\n"), &json_model("{}"));
        assert_eq!((r.node(0), r.fold(0)), (None, None));
        let r = Relocation::new(&json_model(r#"{"a": 1}"#), &yaml_model("a: 1\n"));
        assert_eq!(r.node(1), None, "JSON and YAML are different documents");
    }
}
