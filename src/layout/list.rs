//! List markers and indentation rules.
//!
//! * two columns of indent per nesting level,
//! * unordered bullets cycle `•`, `◦`, `▪` by depth (ASCII `-`, `*`, `+`),
//! * ordered lists respect `start` and are right-aligned by their own width,
//! * task items render `[x]`/`[ ]` as `☑`/`☐` (ASCII `[x]`/`[ ]`),
//! * continuation blocks of an item are indented under the marker.

use crate::document::markdown::List;

/// Columns of indentation added per nesting level.
pub const INDENT_PER_LEVEL: usize = 2;

/// Unicode bullets by depth.
const BULLETS: [&str; 3] = ["•", "◦", "▪"];
/// ASCII bullets by depth.
const BULLETS_ASCII: [&str; 3] = ["-", "*", "+"];

/// The bullet for a nesting depth (0-based), cycling every three levels.
pub fn bullet(depth: usize, unicode: bool) -> &'static str {
    let set = if unicode { BULLETS } else { BULLETS_ASCII };
    set[depth % set.len()]
}

/// The checkbox for a task item.
pub fn task_box(checked: bool, unicode: bool) -> &'static str {
    match (checked, unicode) {
        (true, true) => "☑",
        (false, true) => "☐",
        (true, false) => "[x]",
        (false, false) => "[ ]",
    }
}

/// The number label of the `index`-th item (0-based) of an ordered list,
/// honouring the list's `start` value.
pub fn ordered_label(start: Option<u64>, index: usize) -> String {
    let n = start.unwrap_or(1).saturating_add(index as u64);
    format!("{n}.")
}

/// The marker text of an item, including the trailing space.
///
/// The returned string is what is drawn in the item's first line; its display
/// width is also the indentation of the item's continuation lines.
pub fn marker(
    list: &List,
    index: usize,
    depth: usize,
    unicode: bool,
    marker_width: usize,
) -> String {
    let base = if list.ordered {
        let label = ordered_label(list.start, index);
        format!("{label:<w$} ", w = marker_width.saturating_sub(1))
    } else {
        format!("{} ", bullet(depth, unicode))
    };
    base
}

/// Width of the widest marker of a list (so items line up).
pub fn marker_width(list: &List, unicode: bool) -> usize {
    if list.ordered {
        let last = ordered_label(list.start, list.items.len().saturating_sub(1));
        crate::util::unicode::width(&last) + 1
    } else {
        crate::util::unicode::width(bullet(0, unicode)) + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::markdown::parse;
    use crate::layout::{Layout, LayoutOptions};
    use crate::render::theme::Theme;

    use crate::testing::plain as render;

    fn render_ascii(src: &str, width: usize) -> String {
        render_with(src, width, false)
    }

    fn render_with(src: &str, width: usize, unicode: bool) -> String {
        let doc = parse(src);
        let theme = Theme::dark();
        let mut opts = LayoutOptions::new(width, &theme);
        opts.unicode = unicode;
        Layout::build(&doc, &opts).to_plain_text()
    }

    #[test]
    fn bullets_cycle_by_depth() {
        // Four levels: the fourth is back to the first bullet.
        let src = "- a\n  - b\n    - c\n      - d\n";
        assert_eq!(render(src, 40), "• a\n  ◦ b\n    ▪ c\n      • d\n");
        assert_eq!(render_ascii(src, 40), "- a\n  * b\n    + c\n      - d\n");
    }

    #[test]
    fn ordered_numbering_respects_start() {
        assert_eq!(render("1. one\n2. two\n", 40), "1. one\n2. two\n");
        assert_eq!(render("5. five\n6. six\n", 40), "5. five\n6. six\n");
    }

    /// CommonMark caps an ordered-list start at nine digits, so the parser
    /// can never hand `ordered_label` a value close to `u64::MAX`. `List` is
    /// a public type whose `start` a caller may set freely, though, so the
    /// overflow guard is checked where it lives: end-to-end coverage is
    /// impossible here, and a wrapping add would panic in a debug build.
    #[test]
    fn ordered_labels_saturate_instead_of_overflowing() {
        assert_eq!(ordered_label(None, 0), "1.");
        assert_eq!(ordered_label(Some(u64::MAX), 0), format!("{}.", u64::MAX));
        assert_eq!(ordered_label(Some(u64::MAX), 3), format!("{}.", u64::MAX));
    }

    #[test]
    fn task_boxes() {
        let src = "- [x] done\n- [ ] todo\n";
        assert_eq!(render(src, 40), "• ☑ done\n• ☐ todo\n");
        assert_eq!(render_ascii(src, 40), "- [x] done\n- [ ] todo\n");
    }

    /// Items of an ordered list line up under each other: the widest label in
    /// the list sets the column at which every item's text starts, so a list
    /// that runs from 8 to 10 pads the one- and two-digit labels.
    #[test]
    fn markers_align() {
        let out = render("8. a\n9. b\n10. c\n", 40);
        assert_eq!(out, "8.  a\n9.  b\n10. c\n");
        let text_columns: Vec<usize> = out
            .lines()
            .map(|l| l.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(0))
            .collect();
        assert!(
            text_columns.windows(2).all(|w| w[0] == w[1]),
            "item text starts in the same column: {text_columns:?}"
        );
    }

    /// Continuation lines of an item are indented to the marker's width, so a
    /// wrapped ordered item stays inside its own column.
    #[test]
    fn continuation_lines_clear_the_marker() {
        let out = render("10. alpha beta gamma delta epsilon\n", 16);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.len() > 1, "the item wrapped: {out:?}");
        assert!(lines[0].starts_with("10. "));
        for line in &lines[1..] {
            assert!(
                line.starts_with("    "),
                "continuation clears the marker: {line:?}"
            );
        }
    }
}
