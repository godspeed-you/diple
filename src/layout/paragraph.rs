//! Block primitives for prose: paragraphs, headings, horizontal rules,
//! blockquote gutters, image placeholders and raw HTML.

use crate::document::markdown::{Heading, Inlines};
use crate::document::Match;
use crate::layout::inline::{layout_inlines, line_width, push_span};
use crate::render::primitives::StyledSpan;
use crate::render::theme::{Style, Theme};
use crate::util::unicode;

/// Blockquote gutter (Unicode and ASCII).
pub(crate) const QUOTE_GUTTER: &str = "▌ ";
/// ASCII blockquote gutter.
pub(crate) const QUOTE_GUTTER_ASCII: &str = "| ";
/// Fold marker for a collapsed section.
pub(crate) const FOLD_COLLAPSED: &str = "▶ ";
/// Fold marker for an expanded section.
pub(crate) const FOLD_EXPANDED: &str = "▼ ";
/// ASCII fold marker for a collapsed section.
pub(crate) const FOLD_COLLAPSED_ASCII: &str = "> ";
/// ASCII fold marker for an expanded section.
pub(crate) const FOLD_EXPANDED_ASCII: &str = "v ";

/// Wrap a paragraph's inline content to `width` with an optional hanging
/// indent for continuation lines.
pub(crate) fn layout_paragraph(
    inlines: &Inlines,
    theme: &Theme,
    base: Style,
    matches: &[Match],
    width: usize,
    hanging_indent: usize,
) -> Vec<Vec<StyledSpan>> {
    let width = width.max(1);
    let rest = width.saturating_sub(hanging_indent).max(1);
    let mut lines = layout_inlines(inlines, theme, base, matches, 0, width, rest);
    if hanging_indent > 0 {
        for line in lines.iter_mut().skip(1) {
            line.insert(0, StyledSpan::new(" ".repeat(hanging_indent), base));
        }
    }
    lines
}

/// How a heading's fold state should be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoldMarker {
    /// The heading does not start a foldable section.
    None,
    /// Foldable and currently expanded (`▼`).
    Expanded,
    /// Foldable and currently collapsed (`▶`).
    Collapsed,
}

/// Indent added per heading level below H2 (depth cue).
pub(crate) const HEADING_INDENT_PER_LEVEL: usize = 2;

/// Lay out a heading: the heading line itself plus an underline for H1/H2.
///
/// H1 is underlined with `═`, H2 with `─` (ASCII `=` / `-`). H3 and deeper get
/// the theme's per-level heading style plus
/// [`HEADING_INDENT_PER_LEVEL`] columns of indentation per level below H2, so
/// the outline depth is visible without the literal `###` Markdown markers —
/// raw Markdown syntax must never appear in rendered output.
///
/// A *collapsed* heading is a single line: the `═`/`─` rule is suppressed so
/// that `▶ Heading` matches the documented mock-up.
pub(crate) fn layout_heading(
    heading: &Heading,
    theme: &Theme,
    matches: &[Match],
    width: usize,
    fold: FoldMarker,
    unicode_box: bool,
) -> Vec<Vec<StyledSpan>> {
    let width = width.max(1);
    let level = heading.level.clamp(1, 6);
    let style = theme.heading(level);

    let mut prefix: Vec<StyledSpan> = Vec::new();
    if level >= 3 {
        prefix.push(StyledSpan::new(
            " ".repeat(HEADING_INDENT_PER_LEVEL * usize::from(level - 2)),
            theme.text,
        ));
    }
    match fold {
        FoldMarker::None => {}
        FoldMarker::Expanded => prefix.push(StyledSpan::new(
            if unicode_box {
                FOLD_EXPANDED
            } else {
                FOLD_EXPANDED_ASCII
            },
            theme.fold_marker,
        )),
        FoldMarker::Collapsed => prefix.push(StyledSpan::new(
            if unicode_box {
                FOLD_COLLAPSED
            } else {
                FOLD_COLLAPSED_ASCII
            },
            theme.fold_marker,
        )),
    }
    let prefix_width = line_width(&prefix);
    let avail = width.saturating_sub(prefix_width).max(1);

    let body = layout_inlines(&heading.inlines, theme, style, matches, 0, avail, avail);
    let mut out: Vec<Vec<StyledSpan>> = Vec::with_capacity(body.len() + 1);
    for (i, line) in body.into_iter().enumerate() {
        let mut full = Vec::with_capacity(line.len() + prefix.len());
        if i == 0 {
            full.extend(prefix.iter().cloned());
        } else if prefix_width > 0 {
            full.push(StyledSpan::new(" ".repeat(prefix_width), style));
        }
        full.extend(line);
        out.push(full);
    }

    // A collapsed section shows a single `▶ Heading` line: its
    // rule would otherwise dangle under a heading with no body.
    if level <= 2 && fold != FoldMarker::Collapsed {
        let ch = match (level, unicode_box) {
            (1, true) => "═",
            (1, false) => "=",
            (_, true) => "─",
            (_, false) => "-",
        };
        let text_width = out.iter().map(|l| line_width(l)).max().unwrap_or(0);
        let n = text_width.clamp(1, width);
        out.push(vec![StyledSpan::new(ch.repeat(n), style)]);
    }
    out
}

/// A horizontal rule spanning `width` columns.
pub(crate) fn horizontal_rule(theme: &Theme, width: usize, unicode_box: bool) -> Vec<StyledSpan> {
    let ch = if unicode_box { "─" } else { "-" };
    vec![StyledSpan::new(ch.repeat(width.max(1)), theme.table_border)]
}

/// `[image: alt]` placeholder line for terminals without image support.
pub(crate) fn image_placeholder(
    alt: &str,
    url: &str,
    theme: &Theme,
    width: usize,
) -> Vec<StyledSpan> {
    let label = if alt.trim().is_empty() {
        format!("[image: {url}]")
    } else {
        format!("[image: {alt}]")
    };
    vec![StyledSpan::new(
        unicode::truncate_with_ellipsis(&label, width.max(1), "…"),
        theme.warning,
    )]
}

/// Raw HTML rendered verbatim as dim text, one line per source line.
pub(crate) fn html_lines(html: &str, theme: &Theme, width: usize) -> Vec<Vec<StyledSpan>> {
    let style = theme.text.dim();
    let mut out = Vec::new();
    for line in html.lines() {
        if line.trim().is_empty() {
            continue;
        }
        for chunk in unicode::wrap(line.trim_end(), width.max(1), width.max(1)) {
            let mut spans = Vec::new();
            push_span(&mut spans, &chunk, style, None, false);
            out.push(spans);
        }
    }
    out
}

/// The blockquote gutter span for one nesting level.
pub(crate) fn quote_gutter(theme: &Theme, unicode_box: bool) -> StyledSpan {
    StyledSpan::new(
        if unicode_box {
            QUOTE_GUTTER
        } else {
            QUOTE_GUTTER_ASCII
        },
        theme.quote_gutter,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::markdown::{parse, NodeKind};
    use crate::document::DocumentModel;
    use crate::layout::{Layout, LayoutOptions};
    use crate::render::primitives::{LineKind, RenderTree};

    /// Markdown in, rendered plain text out — the same instinct as
    /// `layout::tests::render`. Everything below goes through the real
    /// pipeline so that the block helpers in this module may be reshaped
    /// freely as long as the output is unchanged.
    fn render(src: &str, width: usize) -> String {
        render_with(src, width, true)
    }

    fn render_ascii(src: &str, width: usize) -> String {
        render_with(src, width, false)
    }

    fn render_with(src: &str, width: usize, unicode_box: bool) -> String {
        let doc = DocumentModel::markdown(parse(src));
        let theme = Theme::dark();
        let mut opts = LayoutOptions::new(width, &theme);
        opts.unicode = unicode_box;
        Layout::build(&doc, &opts).to_plain_text()
    }

    /// Render with a fold state active — which is what enables the fold
    /// markers — optionally collapsing one section.
    fn render_folded(
        src: &str,
        width: usize,
        unicode_box: bool,
        collapse: Option<usize>,
    ) -> String {
        let doc = DocumentModel::markdown(parse(src));
        let theme = Theme::dark();
        let mut folds = doc.fold_state();
        if let Some(section) = collapse {
            folds.collapse(section);
        }
        let mut opts = LayoutOptions::new(width, &theme).with_folds(&folds);
        opts.unicode = unicode_box;
        Layout::build(&doc, &opts).to_plain_text()
    }

    fn tree_of(doc: &DocumentModel, theme: &Theme, width: usize) -> RenderTree {
        Layout::build(doc, &LayoutOptions::new(width, theme))
    }

    #[test]
    fn h1_and_h2_underlines() {
        assert_eq!(
            render("# Title\n\n## Sub\n", 40),
            "Title\n═════\n\nSub\n───\n"
        );
        assert_eq!(
            render_ascii("# Title\n\n## Sub\n", 40),
            "Title\n=====\n\nSub\n---\n"
        );
    }

    /// Raw Markdown syntax must never appear in rendered output. Literal
    /// `###` markers once leaked into the rendered text *and* were accepted
    /// into the snapshots, so this asserts on what a reader actually sees:
    /// the indentation is the depth cue, there is no rule, and no heading
    /// line contains a `#` anywhere.
    #[test]
    fn deep_headings_are_indented_not_underlined_and_never_show_hashes() {
        let src = "# T\n\n### Deep\n\n#### Deeper\n\n##### D5\n\n###### D6\n";
        assert_eq!(
            render(src, 40),
            "T\n═\n\n  Deep\n\n    Deeper\n\n      D5\n\n        D6\n"
        );
        assert!(!render(src, 40).contains('#'), "no literal Markdown marker");
        assert!(!render_ascii(src, 40).contains('#'));

        // Belt and braces: no line the layout attributes to a heading may
        // start with `#`, at any width, under either box-drawing mode.
        let doc = DocumentModel::markdown(parse(src));
        let theme = Theme::dark();
        for width in [4usize, 12, 40, 120] {
            for unicode_box in [true, false] {
                let mut opts = LayoutOptions::new(width, &theme);
                opts.unicode = unicode_box;
                for line in &Layout::build(&doc, &opts).lines {
                    if matches!(line.kind, LineKind::Heading(_)) {
                        let text = line.to_text();
                        assert!(
                            !text.trim_start().starts_with('#'),
                            "heading line {text:?} leaks its Markdown marker"
                        );
                    }
                }
            }
        }

        // The per-level style is what distinguishes H3..H6 once the hashes
        // are gone, so it is asserted directly (and only it).
        let doc = DocumentModel::markdown(parse("### Deep\n"));
        let theme = Theme::dark();
        let tree = tree_of(&doc, &theme, 40);
        let heading = tree.lines.first().expect("the heading line");
        let title = heading.spans.last().expect("the heading text span");
        assert_eq!(title.text, "Deep");
        assert_eq!(title.style, theme.heading(3));
        assert_ne!(theme.heading(3), theme.heading(4));
    }

    /// A related finding: a collapsed section is a single `▶ Heading`
    /// line — its `═`/`─` rule would otherwise dangle under an empty body.
    #[test]
    fn a_collapsed_heading_is_a_single_line() {
        assert_eq!(render_folded("# A\n\nbody\n", 40, true, Some(0)), "▶ A\n");
        assert_eq!(render_folded("## B\n\nbody\n", 40, true, Some(0)), "▶ B\n");
        // Expanded, the rule is back.
        assert_eq!(
            render_folded("# A\n\nbody\n", 40, true, None),
            "▼ A\n═══\n\nbody\n",
            "an expanded foldable heading keeps its rule"
        );
    }

    #[test]
    fn fold_markers() {
        assert!(render_folded("# A\n\nbody\n", 40, true, None).starts_with("▼ A"));
        assert!(render_folded("# A\n\nbody\n", 40, true, Some(0)).starts_with("▶ A"));
        assert!(render_folded("# A\n\nbody\n", 40, false, Some(0)).starts_with("> A"));
        assert!(render_folded("# A\n\nbody\n", 40, false, None).starts_with("v A"));
    }

    #[test]
    fn long_heading_wraps_and_underline_is_clamped() {
        let out = render("# aaaa bbbb cccc dddd eeee\n", 12);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.len() >= 3, "the heading wrapped: {out:?}");
        assert!(
            lines.iter().all(|l| unicode::width(l) <= 12),
            "nothing overflows the width: {out:?}"
        );
        let (rule, text) = lines.split_last().expect("an underline");
        assert!(
            rule.chars().all(|c| c == '═'),
            "the last line is the rule: {rule:?}"
        );
        let widest = text.iter().map(|l| unicode::width(l)).max().unwrap_or(0);
        assert_eq!(
            unicode::width(rule),
            widest,
            "the rule is as wide as the widest wrapped line, not the terminal"
        );
    }

    #[test]
    fn placeholders_and_rules() {
        assert_eq!(render("![a cat](cat.png)\n", 40), "[image: a cat]\n");
        assert_eq!(render("![](cat.png)\n", 40), "[image: cat.png]\n");
        assert_eq!(render("a\n\n---\n\nb\n", 5), "a\n\n─────\n\nb\n");
        assert_eq!(render_ascii("a\n\n---\n\nb\n", 5), "a\n\n-----\n\nb\n");
        // A degenerate width still draws one cell rather than nothing.
        assert_eq!(render("---\n", 0), "─\n");
    }

    /// `layout_paragraph`'s hanging indent has no caller in the layout engine
    /// today (every block passes 0), so there is no document that exercises
    /// it end to end. It is kept as a unit test deliberately: it is the only
    /// coverage the parameter has.
    #[test]
    fn paragraph_hanging_indent() {
        let doc = parse("alpha beta gamma delta\n");
        let NodeKind::Paragraph(inlines) = &doc.nodes[0].kind else {
            panic!("a paragraph")
        };
        let lines = layout_paragraph(inlines, &Theme::dark(), Style::new(), &[], 12, 3);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert_eq!(text[0], "alpha beta");
        assert!(
            text[1].starts_with("   "),
            "continuation is indented: {text:?}"
        );
        assert!(lines.iter().all(|l| line_width(l) <= 12));
    }
}
