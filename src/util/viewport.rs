//! Viewport helpers: vertical windowing and grapheme/width-correct horizontal
//! slicing.

use crate::render::primitives::{RenderLine, RenderTree, StyledSpan};
use crate::util::unicode;

/// Largest sensible `top_line` for a document of `total` lines in a window of
/// `height` lines (the last screen is never scrolled past).
pub fn max_top_line(total: usize, height: usize) -> usize {
    total.saturating_sub(height.max(1))
}

/// Scroll percentage shown in the status bar.
pub fn percent(top_line: usize, height: usize, total: usize) -> u8 {
    if total == 0 {
        return 100;
    }
    let bottom = (top_line + height.max(1)).min(total);
    ((bottom * 100) / total).min(100) as u8
}

impl RenderTree {
    /// The lines visible in a window of `height` lines starting at `top_line`.
    pub fn visible_slice(&self, top_line: usize, height: usize) -> &[RenderLine] {
        let start = top_line.min(self.lines.len());
        let end = start.saturating_add(height).min(self.lines.len());
        &self.lines[start..end]
    }
}

/// Slice one line to the columns `[h_offset, h_offset + width)`.
///
/// Never splits a grapheme cluster: a wide character straddling either edge is
/// replaced by spaces for the half that is visible, so the result is exactly
/// as wide as the visible region.
pub fn slice_line(line: &RenderLine, h_offset: usize, width: usize) -> Vec<StyledSpan> {
    if width == 0 {
        return Vec::new();
    }
    let end = h_offset.saturating_add(width);
    let mut out: Vec<StyledSpan> = Vec::new();
    let mut col = 0usize;
    for span in &line.spans {
        let w = span.width();
        let span_end = col + w;
        if span_end <= h_offset {
            col = span_end;
            continue;
        }
        if col >= end {
            break;
        }
        let vis_start = col.max(h_offset);
        let vis_end = span_end.min(end);
        let text = unicode::slice_columns(&span.text, vis_start - col, vis_end - vis_start);
        if !text.is_empty() {
            out.push(StyledSpan {
                text,
                style: span.style,
                link: span.link,
                search_match: span.search_match,
            });
        }
        col = span_end;
    }
    out
}

/// Plain text of a horizontally sliced line.
///
/// Only the unit tests below need the flattened form; the application always
/// keeps the styled spans, so this is not part of the crate's API.
#[cfg(test)]
pub fn slice_line_text(line: &RenderLine, h_offset: usize, width: usize) -> String {
    slice_line(line, h_offset, width)
        .iter()
        .map(|s| s.text.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::markdown::parse;
    use crate::document::SearchIndex;
    use crate::layout::{Layout, LayoutOptions};
    use crate::render::theme::Theme;

    /// Lay a document out for real: the lines these tests slice are the lines
    /// the reader sees, not hand-assembled `RenderLine` literals.
    use crate::testing::render as tree;

    /// Horizontal scrolling is grapheme- and width-correct: a wide character
    /// straddling either edge of the window becomes a space, so the slice is
    /// exactly as wide as the region it fills and never half a character.
    #[test]
    fn horizontal_slice_across_a_wide_char() {
        // `a日bc` laid out without wrapping is a single line of width 5.
        let t = tree("a日bc\n", 40);
        let l = t.lines.first().expect("one line");
        assert_eq!(l.width, 5);
        assert_eq!(slice_line_text(l, 0, 5), "a日bc");
        assert_eq!(slice_line_text(l, 0, 2), "a ");
        assert_eq!(slice_line_text(l, 2, 3), " bc");
        assert_eq!(slice_line_text(l, 1, 2), "日");
        assert_eq!(slice_line_text(l, 1, 1), " ");
        assert_eq!(slice_line_text(l, 10, 5), "");
        assert_eq!(slice_line_text(l, 0, 0), "");
        // Whatever the offset, the slice never exceeds the window.
        for off in 0..8 {
            for w in 0..8 {
                assert!(unicode::width(&slice_line_text(l, off, w)) <= w);
            }
        }
    }

    /// Styling survives horizontal scrolling: a reader who scrolls right must
    /// still see the bold run bold and the search hit highlighted. Asserting
    /// on the span properties is the point here — they *are* the behaviour.
    #[test]
    fn slicing_preserves_span_styles_and_search_marks() {
        let doc = parse("hello **world**\n");
        let theme = Theme::dark();
        let idx = SearchIndex::build(&doc);
        let matches = idx.find("world", false);
        assert_eq!(matches.len(), 1, "the fixture has one hit");
        let opts = LayoutOptions::new(40, &theme).with_matches(&matches);
        let t = Layout::build(&doc, &opts);
        let l = t.lines.first().expect("one line");
        assert_eq!(l.to_text(), "hello world");

        let out = slice_line(l, 3, 6);
        let text: String = out.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(text, "lo wor");
        let last = out.last().expect("the sliced bold run");
        assert_eq!(last.text, "wor");
        assert!(last.style.bold, "the strong run is still bold");
        assert!(last.search_match, "the search hit is still marked");
    }

    #[test]
    fn visible_slice_is_clamped() {
        let t = tree("a\n\nb\n\nc\n\nd\n\ne\n", 40);
        let total = t.len();
        assert!(total >= 5);
        assert_eq!(t.visible_slice(0, 3).len(), 3);
        assert_eq!(t.visible_slice(total - 2, 5).len(), 2);
        assert_eq!(t.visible_slice(total + 40, 5).len(), 0);
        assert_eq!(t.visible_slice(0, 0).len(), 0);
    }

    /// Pure scroll arithmetic, kept at unit level: `max_top_line` and
    /// `percent` are the shared definitions the application and the status
    /// bar both call, and the edge cases (a document shorter than the window,
    /// an empty document) have no rendered output to assert on.
    #[test]
    fn scroll_maths() {
        assert_eq!(max_top_line(100, 20), 80);
        assert_eq!(max_top_line(10, 20), 0);
        assert_eq!(max_top_line(0, 0), 0);
        assert_eq!(percent(0, 20, 100), 20);
        assert_eq!(percent(80, 20, 100), 100);
        assert_eq!(percent(0, 20, 0), 100);
    }
}
