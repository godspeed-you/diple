//! SGR serialization of a [`RenderTree`] for the non-interactive path
//! (`--color always`).
//!
//! The interactive renderer hands [`Style`] to ratatui, which emits the escape
//! sequences itself. `diple doc.md --color always | less -R` has no ratatui,
//! so the styling has to be written out here. Both paths share
//! [`Style::downgrade`], so `truecolor`, `ansi256`, `ansi16` and `none` behave
//! exactly as they do on screen.
//!
//! # Balance
//!
//! Every span that carries any attribute is written as `CSI <params> m <text>
//! CSI 0 m`, so each opening sequence has its own reset and nothing can leak
//! past the end of a line, a page or the process Spans with no attributes are
//! written verbatim, so [`ColorLevel::None`] produces byte-identical output to
//! [`RenderTree::to_plain_text`].

use crate::render::primitives::RenderTree;
use crate::render::theme::{Color, ColorLevel, Style};

/// Control Sequence Introducer.
const CSI: &str = "\x1b[";
/// Reset every attribute.
const RESET: &str = "\x1b[0m";

/// The SGR parameters of `style` at `level`, or `None` when it has none.
///
/// Uses the same [`Style::downgrade`] the interactive renderer uses, so a
/// downgraded colour level drops exactly the same attributes on both paths.
pub fn sgr_params(style: Style, level: ColorLevel) -> Option<String> {
    let style = style.downgrade(level);
    let mut params: Vec<String> = Vec::new();
    if style.bold {
        params.push("1".to_string());
    }
    if style.dim {
        params.push("2".to_string());
    }
    if style.italic {
        params.push("3".to_string());
    }
    if style.underline {
        params.push("4".to_string());
    }
    if style.reverse {
        params.push("7".to_string());
    }
    if style.strikethrough {
        params.push("9".to_string());
    }
    if let Some(fg) = style.fg {
        params.push(color_params(fg, false));
    }
    if let Some(bg) = style.bg {
        params.push(color_params(bg, true));
    }
    if params.is_empty() {
        None
    } else {
        Some(params.join(";"))
    }
}

/// SGR parameters selecting `color` as foreground (or background).
fn color_params(color: Color, background: bool) -> String {
    let base = if background { 48 } else { 38 };
    match color {
        Color::Rgb(r, g, b) => format!("{base};2;{r};{g};{b}"),
        Color::Indexed(i) => format!("{base};5;{i}"),
    }
}

/// Wrap `text` in the SGR sequence for `style`, always closing with a reset.
fn styled(text: &str, style: Style, level: ColorLevel) -> String {
    match sgr_params(style, level) {
        Some(params) if !text.is_empty() => format!("{CSI}{params}m{text}{RESET}"),
        _ => text.to_string(),
    }
}

/// Serialize the whole tree with SGR escapes, one line per [`RenderLine`](crate::render::primitives::RenderLine).
///
/// Trailing whitespace is trimmed exactly as [`RenderTree::to_plain_text`]
/// trims it, so the two serializers differ only in the escapes.
pub fn to_ansi_text(tree: &RenderTree, level: ColorLevel) -> String {
    if level == ColorLevel::None {
        return tree.to_plain_text();
    }
    let mut out = String::new();
    for line in &tree.lines {
        // Find the last span with visible content; everything after it is
        // trailing padding and is dropped.
        let last = line
            .spans
            .iter()
            .rposition(|s| !s.text.trim_end().is_empty());
        if let Some(last) = last {
            for (i, span) in line.spans.iter().take(last + 1).enumerate() {
                let text = if i == last {
                    span.text.trim_end()
                } else {
                    span.text.as_str()
                };
                out.push_str(&styled(text, span.style, level));
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(src: &str) -> RenderTree {
        crate::testing::render(src, 60)
    }

    #[test]
    fn no_color_is_byte_identical_to_plain_text() {
        let t = tree("# Title\n\nSome **bold** text.\n");
        assert_eq!(to_ansi_text(&t, ColorLevel::None), t.to_plain_text());
    }

    #[test]
    fn colored_output_is_balanced_and_keeps_the_text() {
        let t = tree("# Title\n\nSome **bold** and `code` text.\n");
        let out = to_ansi_text(&t, ColorLevel::TrueColor);
        assert!(out.contains('\u{1b}'), "escapes are emitted: {out:?}");
        let opens = out.matches(CSI).count() - out.matches(RESET).count();
        assert_eq!(opens, out.matches(RESET).count(), "every SGR is reset");
        // Stripping the escapes reproduces the plain rendering exactly.
        assert_eq!(strip(&out), t.to_plain_text());
    }

    #[test]
    fn ansi16_uses_palette_indices_only() {
        let t = tree("# Title\n");
        let out = to_ansi_text(&t, ColorLevel::Ansi16);
        assert!(out.contains("38;5;"), "{out:?}");
        assert!(
            !out.contains("38;2;"),
            "no 24-bit colour at ansi16: {out:?}"
        );
    }

    #[test]
    fn a_style_without_attributes_emits_nothing() {
        assert_eq!(sgr_params(Style::new(), ColorLevel::TrueColor), None);
        assert_eq!(
            sgr_params(Style::new().bold(), ColorLevel::None),
            Some("1".into())
        );
    }

    fn strip(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(i) = rest.find('\u{1b}') {
            out.push_str(&rest[..i]);
            let after = &rest[i..];
            match after.find('m') {
                Some(end) => rest = &after[end + 1..],
                None => return out,
            }
        }
        out.push_str(rest);
        out
    }
}
