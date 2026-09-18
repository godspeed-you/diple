//! Ordered link list: every link in the document in source order, with a
//! classification into internal anchors vs. external / relative targets.

use super::ast::{LinkId, NodeId};

/// Classification of a link destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// `#anchor` — jumps inside the document. Holds the anchor without `#`.
    Internal(String),
    /// Absolute URL with a scheme (`https://…`, `mailto:…`).
    External,
    /// Relative path (`docs/foo.md`, `../x`), opened via the opener.
    Relative,
}

/// A link occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Dense index (== position in `Document::links`).
    pub id: LinkId,
    /// Destination as written.
    pub url: String,
    /// Plain-text link label.
    pub text: String,
    /// Optional title.
    pub title: Option<String>,
    /// Node that contains the link.
    pub node: NodeId,
    /// Classification.
    pub kind: LinkKind,
}

impl Link {
    /// Classify a destination string.
    pub fn classify(url: &str) -> LinkKind {
        if let Some(anchor) = url.strip_prefix('#') {
            return LinkKind::Internal(anchor.to_string());
        }
        if has_scheme(url) {
            LinkKind::External
        } else {
            LinkKind::Relative
        }
    }

    /// `true` if this link targets an anchor in the same document.
    pub fn is_internal(&self) -> bool {
        matches!(self.kind, LinkKind::Internal(_))
    }
}

/// RFC 3986 scheme check: `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`.
fn has_scheme(url: &str) -> bool {
    let mut chars = url.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for c in chars {
        if c == ':' {
            return true;
        }
        if !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert_eq!(Link::classify("#intro"), LinkKind::Internal("intro".into()));
        assert_eq!(Link::classify("https://example.com"), LinkKind::External);
        assert_eq!(Link::classify("mailto:a@b.c"), LinkKind::External);
        assert_eq!(Link::classify("docs/x.md"), LinkKind::Relative);
        assert_eq!(Link::classify("./x.md#frag"), LinkKind::Relative);
        assert_eq!(Link::classify("C:\\path"), LinkKind::External);
        assert_eq!(Link::classify(""), LinkKind::Relative);
    }
}
