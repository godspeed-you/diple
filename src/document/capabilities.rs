//! What a loaded document can do.
//!
//! The application asks a document what it supports rather than assuming it
//! has Markdown headings. That is what lets `[`, `}`, `za` and `t` mean the
//! analogous thing in every format, keeps a key that means nothing here from
//! cluttering the hints sidebar, and leaves room for a format whose structure
//! is not a tree at all — a paged PDF being the one this design is explicitly
//! written for.
//!
//! A capability answers "is this action meaningful for this document", never
//! "is it available right now": whether the cursor happens to sit on a
//! foldable node is a question for the document, not for this struct.

/// The structural features a document supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocumentCapabilities {
    /// Previous/next primary structural node (`[` / `]`).
    pub hierarchy_navigation: bool,
    /// Previous/next sibling, and parent/first child (`{` / `}`).
    pub sibling_navigation: bool,
    /// Foldable units (`Enter`, `za`, `zc`, `zo`, `zM`, `zR`).
    pub folding: bool,
    /// A navigable outline (`t`).
    pub outline: bool,
    /// A semantic path to the selected node, shown in the status line.
    pub semantic_path: bool,
    /// Followable hyperlinks.
    pub links: bool,
    /// Pages, as a PDF backend would report.
    pub pages: bool,
    /// A raw/source view distinct from the rendered one.
    pub source_view: bool,
}

impl DocumentCapabilities {
    /// Nothing supported: the starting point for a backend that adds what it
    /// has, so a new capability defaults to absent rather than to a lie.
    pub const NONE: DocumentCapabilities = DocumentCapabilities {
        hierarchy_navigation: false,
        sibling_navigation: false,
        folding: false,
        outline: false,
        semantic_path: false,
        links: false,
        pages: false,
        source_view: false,
    };

    /// What Markdown supports: headings form the hierarchy, sections fold,
    /// the table of contents is the outline, and links are followable. No
    /// key path — a Markdown section path is not what a reader asks for.
    pub const MARKDOWN: DocumentCapabilities = DocumentCapabilities {
        hierarchy_navigation: true,
        sibling_navigation: true,
        folding: true,
        outline: true,
        semantic_path: false,
        links: true,
        pages: false,
        source_view: false,
    };

    /// What a structured document (JSON, YAML) supports: containers form the
    /// hierarchy and fold, every node has a path, and nothing is a hyperlink.
    pub const STRUCTURED: DocumentCapabilities = DocumentCapabilities {
        hierarchy_navigation: true,
        sibling_navigation: true,
        folding: true,
        outline: true,
        semantic_path: true,
        links: false,
        pages: false,
        source_view: false,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presets_differ_where_the_formats_differ() {
        assert!(DocumentCapabilities::MARKDOWN.links);
        assert!(!DocumentCapabilities::MARKDOWN.semantic_path);
        assert!(!DocumentCapabilities::STRUCTURED.links);
        assert!(DocumentCapabilities::STRUCTURED.semantic_path);
        // Both are readers of a hierarchy that folds.
        for caps in [
            DocumentCapabilities::MARKDOWN,
            DocumentCapabilities::STRUCTURED,
        ] {
            assert!(caps.hierarchy_navigation && caps.folding && caps.outline);
            assert!(!caps.pages, "no format in 2.0 has pages");
        }
        assert_eq!(DocumentCapabilities::NONE, DocumentCapabilities::default());
    }
}
