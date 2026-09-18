//! The semantic path to a node — "where am I in this document".
//!
//! Stored as segments rather than as a preformatted string, so the status
//! line can render a breadcrumb that truncates on a narrow terminal while the
//! program keeps a canonical form that is unambiguous even when a key
//! contains the separator.

use std::fmt;

/// One step from a parent to a child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// A mapping/object key.
    Key(String),
    /// A sequence/array index.
    Index(usize),
    /// A document within a multi-document stream (1-based, as YAML readers
    /// count them).
    Document(usize),
}

impl PathSegment {
    /// The breadcrumb form: a key as itself, an index in brackets.
    ///
    /// Sanitised, because this is the one piece of document text that reaches
    /// the terminal without passing through
    /// [`StyledSpan::new`](crate::render::primitives::StyledSpan::new): the
    /// status line draws the breadcrumb itself. A key containing an ESC would
    /// otherwise be a way for a document to write to the terminal after all
    /// (spec §20.4, AC-18). [`DocumentPath::canonical`] is deliberately not
    /// sanitised — it is the internal, copyable form and has to stay true to
    /// the source.
    pub fn label(&self) -> String {
        match self {
            PathSegment::Key(key) => crate::util::text::sanitize(key),
            PathSegment::Index(i) => format!("[{i}]"),
            PathSegment::Document(n) => format!("Document {n}"),
        }
    }
}

/// The separator between breadcrumb segments.
const SEPARATOR: &str = " \u{203a} ";
/// ASCII fallback for terminals without Unicode box drawing.
const SEPARATOR_ASCII: &str = " > ";

/// A path from the document root to one semantic node.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DocumentPath {
    segments: Vec<PathSegment>,
}

impl DocumentPath {
    /// A path from its segments, root first.
    pub fn new(segments: Vec<PathSegment>) -> Self {
        Self { segments }
    }

    /// The segments, root first.
    pub fn segments(&self) -> &[PathSegment] {
        &self.segments
    }

    /// Whether the path names the root itself.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// Number of segments.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// The human breadcrumb, e.g. `spec › containers › [0] › image`.
    ///
    /// `unicode` picks the separator; without it the ASCII `>` is used, which
    /// is also what the tests and the non-interactive path read.
    pub fn breadcrumb(&self, unicode: bool) -> String {
        let sep = if unicode { SEPARATOR } else { SEPARATOR_ASCII };
        self.segments
            .iter()
            .map(PathSegment::label)
            .collect::<Vec<_>>()
            .join(sep)
    }

    /// The breadcrumb truncated to `width` columns, dropping segments from
    /// the left so that the node the reader selected always survives.
    ///
    /// Returns an empty string for a zero width, and an ellipsis-prefixed
    /// tail when something had to go.
    pub fn breadcrumb_within(&self, width: usize, unicode: bool) -> String {
        if width == 0 || self.segments.is_empty() {
            return String::new();
        }
        let sep = if unicode { SEPARATOR } else { SEPARATOR_ASCII };
        let ellipsis = if unicode { "\u{2026}" } else { "..." };
        let full = self.breadcrumb(unicode);
        if crate::util::unicode::width(&full) <= width {
            return full;
        }
        // Grow the tail one segment at a time while it still fits with the
        // leading ellipsis.
        let mut best = String::new();
        for start in (0..self.segments.len()).rev() {
            let tail = self.segments[start..]
                .iter()
                .map(PathSegment::label)
                .collect::<Vec<_>>()
                .join(sep);
            let candidate = format!("{ellipsis}{sep}{tail}");
            if crate::util::unicode::width(&candidate) > width {
                break;
            }
            best = candidate;
        }
        if best.is_empty() {
            // Not even the last segment fits: show as much of it as there is
            // room for rather than nothing at all.
            let last = self
                .segments
                .last()
                .map(PathSegment::label)
                .unwrap_or_default();
            return crate::util::unicode::truncate_with_ellipsis(&last, width, ellipsis);
        }
        best
    }

    /// A canonical, copyable form: [RFC 6901] JSON Pointer, prefixed with the
    /// document number when the source held more than one.
    ///
    /// Unlike the breadcrumb this is unambiguous — `/` and `~` inside a key
    /// are escaped — which is what makes it the form to hand to another tool.
    ///
    /// [RFC 6901]: https://www.rfc-editor.org/rfc/rfc6901
    pub fn canonical(&self) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            match segment {
                PathSegment::Document(n) => out.push_str(&format!("#{n}")),
                PathSegment::Key(key) => {
                    out.push('/');
                    out.push_str(&key.replace('~', "~0").replace('/', "~1"));
                }
                PathSegment::Index(i) => {
                    out.push('/');
                    out.push_str(&i.to_string());
                }
            }
        }
        if out.is_empty() {
            out.push('/');
        }
        out
    }
}

impl fmt::Display for DocumentPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.breadcrumb(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> DocumentPath {
        DocumentPath::new(vec![
            PathSegment::Key("spec".into()),
            PathSegment::Key("containers".into()),
            PathSegment::Index(0),
            PathSegment::Key("image".into()),
        ])
    }

    #[test]
    fn the_breadcrumb_shows_keys_and_indices() {
        assert_eq!(path().breadcrumb(false), "spec > containers > [0] > image");
        assert_eq!(
            path().breadcrumb(true),
            "spec \u{203a} containers \u{203a} [0] \u{203a} image"
        );
        assert!(DocumentPath::default().is_empty());
        assert_eq!(DocumentPath::default().breadcrumb(false), "");
    }

    #[test]
    fn a_narrow_status_line_truncates_from_the_left() {
        let p = path();
        assert_eq!(p.breadcrumb_within(80, false), p.breadcrumb(false));
        let narrow = p.breadcrumb_within(20, false);
        assert!(
            narrow.ends_with("image"),
            "the selected node survives: {narrow}"
        );
        assert!(narrow.starts_with("..."), "{narrow}");
        assert!(crate::util::unicode::width(&narrow) <= 20);
        // Not even one segment fits.
        let tiny = p.breadcrumb_within(3, false);
        assert!(crate::util::unicode::width(&tiny) <= 3, "{tiny:?}");
        assert_eq!(p.breadcrumb_within(0, false), "");
    }

    #[test]
    fn the_canonical_form_is_a_json_pointer_and_escapes_separators() {
        assert_eq!(path().canonical(), "/spec/containers/0/image");
        let odd = DocumentPath::new(vec![
            PathSegment::Key("a/b".into()),
            PathSegment::Key("c~d".into()),
            PathSegment::Key("x > y".into()),
        ]);
        assert_eq!(odd.canonical(), "/a~1b/c~0d/x > y");
        // The breadcrumb is ambiguous for such a key; the canonical form is
        // what disambiguates, which is why both exist.
        assert_eq!(odd.breadcrumb(false), "a/b > c~d > x > y");
        assert_eq!(DocumentPath::default().canonical(), "/");
    }

    #[test]
    fn a_multi_document_path_names_its_document() {
        let p = DocumentPath::new(vec![
            PathSegment::Document(2),
            PathSegment::Key("spec".into()),
        ]);
        assert_eq!(p.breadcrumb(false), "Document 2 > spec");
        assert_eq!(p.canonical(), "#2/spec");
        assert_eq!(p.len(), 2);
    }

    /// AC-18: the status line draws the breadcrumb itself, so a key that
    /// carries terminal control sequences must not be able to write to the
    /// terminal through it.
    #[test]
    fn a_hostile_key_cannot_write_to_the_terminal_through_the_breadcrumb() {
        let hostile = "\u{1b}[31mred\u{7}\r\u{202e}flip";
        let p = DocumentPath::new(vec![
            PathSegment::Key(hostile.to_string()),
            PathSegment::Index(0),
        ]);
        for rendered in [
            p.breadcrumb(true),
            p.breadcrumb(false),
            p.breadcrumb_within(200, true),
            p.breadcrumb_within(12, false),
            p.to_string(),
        ] {
            assert!(!rendered.contains('\u{1b}'), "ESC survived: {rendered:?}");
            assert!(!rendered.contains('\u{7}'), "BEL survived: {rendered:?}");
            assert!(!rendered.contains('\r'), "CR survived: {rendered:?}");
            assert!(
                !rendered.contains('\u{202e}'),
                "a bidi override survived: {rendered:?}"
            );
        }
        // The canonical form is the internal, copyable one and stays true to
        // the source; it never reaches the terminal.
        assert!(p.canonical().contains('\u{1b}'));
    }
}
