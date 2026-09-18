//! The document outline — the table of contents for Markdown, the structure
//! tree for JSON and YAML.
//!
//! One entry per unit worth listing, in document order. The entry names the
//! semantic node to jump to and, when there is one, the fold target whose
//! state the sidebar draws; it deliberately carries no format-specific
//! vocabulary, so the sidebar widget is the same for every format.

use super::folds::FoldId;
use super::NodeId;

/// One line of the outline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    /// The semantic node this entry stands for; jumping here scrolls to it.
    pub node: NodeId,
    /// The foldable unit the entry belongs to, when it is foldable itself.
    pub fold: Option<FoldId>,
    /// Nesting depth (0 = outermost).
    pub depth: usize,
    /// What the sidebar shows.
    pub text: String,
}

/// A document's outline entries, in document order.
pub type Outline = Vec<OutlineEntry>;

/// Longest label shown for a scalar preview in the outline.
///
/// A 4 KiB string value must not decide the sidebar's width, and a reader
/// scanning an outline wants the shape of the document rather than its
/// contents.
pub const PREVIEW_LIMIT: usize = 32;

/// Shorten `value` to [`PREVIEW_LIMIT`] columns for an outline preview,
/// collapsing whitespace so a multi-line scalar stays one line.
pub fn preview(value: &str) -> String {
    let mut flat = String::with_capacity(value.len().min(PREVIEW_LIMIT * 2));
    let mut space = false;
    for ch in value.chars() {
        if ch.is_whitespace() {
            space = !flat.is_empty();
            continue;
        }
        if space {
            flat.push(' ');
            space = false;
        }
        flat.push(ch);
        if crate::util::unicode::width(&flat) > PREVIEW_LIMIT {
            break;
        }
    }
    crate::util::unicode::truncate_with_ellipsis(&flat, PREVIEW_LIMIT, "\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preview_is_one_short_line() {
        assert_eq!(preview("nginx"), "nginx");
        assert_eq!(preview("echo hello\necho world"), "echo hello echo world");
        assert_eq!(preview("   padded   "), "padded");
        assert_eq!(preview(""), "");
        let long = "x".repeat(200);
        let short = preview(&long);
        assert!(crate::util::unicode::width(&short) <= PREVIEW_LIMIT);
        assert!(short.ends_with('\u{2026}'), "{short}");
    }

    #[test]
    fn a_preview_never_splits_a_character() {
        let wide = "本".repeat(100);
        let short = preview(&wide);
        assert!(crate::util::unicode::width(&short) <= PREVIEW_LIMIT);
        assert!(short.chars().all(|c| c == '本' || c == '\u{2026}'));
    }
}
