//! Inline layout: [`Inlines`] → styled spans → wrapped lines.
//!
//! The flattening order and the produced plain text match
//! [`crate::document::markdown::inlines_to_text`] byte for byte, which is what the
//! search index is built from. That is what makes it possible to map a
//! [`Match`] (byte offsets into the node's plain text) onto the styled spans
//! it overlaps.

use crate::document::markdown::{Inline, Inlines};
use crate::document::{LinkId, Match};
use crate::render::primitives::StyledSpan;
use crate::render::theme::{Style, Theme};
use crate::util::unicode::{self, TokenKind};

/// One un-wrapped run of inline content with its position in the node's
/// plain text.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// Text of the run (never contains `\n`).
    pub text: String,
    /// Style to render it with.
    pub style: Style,
    /// Link the run belongs to.
    pub link: Option<LinkId>,
    /// Whether the run overlaps a search match.
    pub search: bool,
    /// Byte offset of `text` in the node's plain text.
    pub plain_start: usize,
    /// `true` for an explicit hard break (the run itself is empty).
    pub hard_break: bool,
}

impl Piece {
    fn text(text: impl Into<String>, style: Style, link: Option<LinkId>, at: usize) -> Piece {
        Piece {
            text: text.into(),
            style,
            link,
            search: false,
            plain_start: at,
            hard_break: false,
        }
    }
}

/// Flatten inline content into styled pieces.
///
/// `offset` is the running byte offset into the node's plain text and is
/// advanced by exactly the number of bytes that
/// [`crate::document::markdown::inlines_to_text`] would produce.
pub fn flatten(inlines: &Inlines, theme: &Theme, base: Style, offset: &mut usize) -> Vec<Piece> {
    let mut out = Vec::new();
    flatten_into(inlines, theme, base, None, offset, &mut out);
    out
}

fn flatten_into(
    inlines: &Inlines,
    theme: &Theme,
    base: Style,
    link: Option<LinkId>,
    offset: &mut usize,
    out: &mut Vec<Piece>,
) {
    for inline in inlines {
        match inline {
            Inline::Text(t) => {
                out.push(Piece::text(t.clone(), base, link, *offset));
                *offset += t.len();
            }
            Inline::Code(t) => {
                out.push(Piece::text(
                    t.clone(),
                    base.patch(theme.code),
                    link,
                    *offset,
                ));
                *offset += t.len();
            }
            Inline::Emph(i) => flatten_into(i, theme, base.patch(theme.emph), link, offset, out),
            Inline::Strong(i) => {
                flatten_into(i, theme, base.patch(theme.strong), link, offset, out)
            }
            Inline::Strike(i) => {
                flatten_into(i, theme, base.patch(theme.strike), link, offset, out)
            }
            Inline::Link { inlines, id, .. } => flatten_into(
                inlines,
                theme,
                base.patch(theme.link),
                Some(*id),
                offset,
                out,
            ),
            Inline::Image { alt, .. } => {
                out.push(Piece::text(
                    alt.clone(),
                    base.patch(theme.warning),
                    link,
                    *offset,
                ));
                *offset += alt.len();
            }
            Inline::SoftBreak | Inline::HardBreak => {
                let hard = matches!(inline, Inline::HardBreak);
                let mut p = Piece::text(" ", base, link, *offset);
                p.hard_break = hard;
                out.push(p);
                *offset += 1;
            }
            Inline::FootnoteRef(label) => {
                let text = format!("[{label}]");
                let len = text.len();
                out.push(Piece::text(text, base.patch(theme.link), link, *offset));
                *offset += len;
            }
        }
    }
}

/// Split pieces so that every piece is either fully inside or fully outside a
/// search match, and flag the ones inside.
///
/// `matches` must refer to the same node and carry byte offsets into that
/// node's plain text.
pub fn apply_matches(pieces: Vec<Piece>, matches: &[Match]) -> Vec<Piece> {
    if matches.is_empty() {
        return pieces;
    }
    let mut out = Vec::with_capacity(pieces.len());
    for piece in pieces {
        if piece.text.is_empty() {
            out.push(piece);
            continue;
        }
        let start = piece.plain_start;
        let end = start + piece.text.len();
        let overlapping: Vec<&Match> = matches
            .iter()
            .filter(|m| m.start < end && m.end > start)
            .collect();
        if overlapping.is_empty() {
            out.push(piece);
            continue;
        }
        // Collect cut points inside the piece.
        let mut cuts: Vec<usize> = vec![0, piece.text.len()];
        for m in &overlapping {
            for p in [m.start, m.end] {
                if p > start && p < end {
                    let rel = p - start;
                    if piece.text.is_char_boundary(rel) {
                        cuts.push(rel);
                    }
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let abs = start + a;
            let inside = overlapping.iter().any(|m| m.start <= abs && m.end > abs);
            let Some(text) = piece.text.get(a..b) else {
                continue;
            };
            out.push(Piece {
                text: text.to_string(),
                style: piece.style,
                link: piece.link,
                search: inside,
                plain_start: abs,
                hard_break: piece.hard_break && b == piece.text.len(),
            });
        }
    }
    out
}

/// Append a span to a line, merging it with the previous one when possible.
pub fn push_span(
    line: &mut Vec<StyledSpan>,
    text: &str,
    style: Style,
    link: Option<LinkId>,
    search: bool,
) {
    if text.is_empty() {
        return;
    }
    // Every Markdown span is assembled here, so this is where document text is
    // made safe for the terminal — the same policy `StyledSpan::new` applies.
    let text = &*crate::util::text::sanitized(text);
    if let Some(last) = line.last_mut() {
        if last.style == style && last.link == link && last.search_match == search {
            last.text.push_str(text);
            return;
        }
    }
    line.push(StyledSpan {
        text: text.to_string(),
        style,
        link,
        search_match: search,
    });
}

/// Wrap styled pieces into lines.
///
/// `first_width` applies to the first line, `rest_width` to continuation lines
/// (hanging indent). Words never split a grapheme cluster; runs that cannot
/// fit on a line of their own are hard broken.
pub fn wrap_pieces(
    pieces: &[Piece],
    first_width: usize,
    rest_width: usize,
) -> Vec<Vec<StyledSpan>> {
    let first_width = first_width.max(1);
    let rest_width = rest_width.max(1);
    let mut lines: Vec<Vec<StyledSpan>> = Vec::new();
    let mut cur: Vec<StyledSpan> = Vec::new();
    let mut cur_w = 0usize;
    let mut pending = 0usize;
    let mut pending_style: Option<(Style, Option<LinkId>, bool)> = None;

    macro_rules! avail {
        () => {
            if lines.is_empty() {
                first_width
            } else {
                rest_width
            }
        };
    }

    for piece in pieces {
        if piece.hard_break {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
            pending = 0;
            pending_style = None;
            continue;
        }
        for token in unicode::tokenize(&piece.text) {
            match token.kind {
                TokenKind::Space => {
                    if !cur.is_empty() {
                        pending += token.width;
                        pending_style = Some((piece.style, piece.link, piece.search));
                    }
                }
                TokenKind::Word => {
                    let mut a = avail!();
                    if !cur.is_empty() && cur_w + pending + token.width > a {
                        lines.push(std::mem::take(&mut cur));
                        cur_w = 0;
                        a = avail!();
                    } else if !cur.is_empty() && pending > 0 {
                        let (st, lk, se) =
                            pending_style.unwrap_or((piece.style, piece.link, false));
                        push_span(&mut cur, &" ".repeat(pending), st, lk, se);
                        cur_w += pending;
                    }
                    pending = 0;
                    pending_style = None;

                    let mut rest = token.text;
                    loop {
                        let room = a.saturating_sub(cur_w);
                        let rw = unicode::width(rest);
                        if rw <= room {
                            push_span(&mut cur, rest, piece.style, piece.link, piece.search);
                            cur_w += rw;
                            break;
                        }
                        let (head, tail) = unicode::split_at_width(rest, room);
                        let (head, tail) = if head.is_empty() && cur.is_empty() {
                            // Guarantee progress for a grapheme wider than the
                            // whole line.
                            let end = unicode::graphemes(rest)
                                .next()
                                .map(str::len)
                                .unwrap_or(rest.len());
                            rest.split_at(end.min(rest.len()))
                        } else {
                            (head, tail)
                        };
                        push_span(&mut cur, head, piece.style, piece.link, piece.search);
                        rest = tail;
                        lines.push(std::mem::take(&mut cur));
                        cur_w = 0;
                        a = avail!();
                        if rest.is_empty() {
                            break;
                        }
                    }
                }
            }
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Full inline pipeline: flatten, apply search matches, wrap.
///
/// `base_offset` is the plain-text offset the inline sequence starts at inside
/// its node (0 for a paragraph, non-zero for table cells).
#[allow(clippy::too_many_arguments)]
pub fn layout_inlines(
    inlines: &Inlines,
    theme: &Theme,
    base: Style,
    matches: &[Match],
    base_offset: usize,
    first_width: usize,
    rest_width: usize,
) -> Vec<Vec<StyledSpan>> {
    let mut offset = base_offset;
    let pieces = flatten(inlines, theme, base, &mut offset);
    let pieces = apply_matches(pieces, matches);
    wrap_pieces(&pieces, first_width, rest_width)
}

/// Flatten inline content into a single line of spans without wrapping.
pub fn spans_unwrapped(
    inlines: &Inlines,
    theme: &Theme,
    base: Style,
    matches: &[Match],
) -> Vec<StyledSpan> {
    let mut offset = 0usize;
    let pieces = apply_matches(flatten(inlines, theme, base, &mut offset), matches);
    let mut line = Vec::new();
    for p in &pieces {
        push_span(&mut line, &p.text, p.style, p.link, p.search);
    }
    line
}

/// Total display width of a line of spans.
pub fn line_width(spans: &[StyledSpan]) -> usize {
    spans.iter().map(StyledSpan::width).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::markdown::{parse, NodeKind};

    fn para(src: &str) -> Inlines {
        let doc = parse(src);
        match &doc.nodes[0].kind {
            NodeKind::Paragraph(i) => i.clone(),
            other => panic!("not a paragraph: {other:?}"),
        }
    }

    #[test]
    fn styles_applied() {
        let theme = Theme::dark();
        let inlines = para("plain *em* **strong** ~~gone~~ `code` [text](http://x)\n");
        let spans = spans_unwrapped(&inlines, &theme, Style::new(), &[]);
        let find = |t: &str| spans.iter().find(|s| s.text.contains(t)).cloned();
        assert!(find("em").map(|s| s.style.italic).unwrap_or(false));
        assert!(find("strong").map(|s| s.style.bold).unwrap_or(false));
        assert!(find("gone").map(|s| s.style.strikethrough).unwrap_or(false));
        assert_eq!(find("code").map(|s| s.style.fg), Some(theme.code.fg));
        let link = find("text").unwrap();
        assert_eq!(link.link, Some(0));
        assert!(link.style.underline);
    }

    #[test]
    fn search_matches_split_spans() {
        let src = "a needle in **needle** stack\n";
        let doc = parse(src);
        let matches = doc.search_index().find("needle", false);
        assert_eq!(matches.len(), 2);
        let inlines = para(src);
        let spans = spans_unwrapped(&inlines, &Theme::dark(), Style::new(), &matches);
        let hits: Vec<_> = spans
            .iter()
            .filter(|s| s.search_match)
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(hits, ["needle", "needle"]);
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "a needle in needle stack");
    }

    #[test]
    fn wrapping_with_hanging_indent() {
        let inlines = para("alpha beta gamma delta epsilon\n");
        let lines = layout_inlines(&inlines, &Theme::dark(), Style::new(), &[], 0, 12, 8);
        let texts: Vec<String> = lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect();
        assert_eq!(texts, ["alpha beta", "gamma", "delta", "epsilon"]);
        assert!(lines.iter().all(|l| line_width(l) <= 12));
    }

    #[test]
    fn hard_break_forces_new_line() {
        let inlines = para("one  \ntwo\n");
        let lines = layout_inlines(&inlines, &Theme::dark(), Style::new(), &[], 0, 40, 40);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn footnote_reference_text() {
        let doc = parse("text[^a]\n\n[^a]: note\n");
        let NodeKind::Paragraph(inlines) = &doc.nodes[0].kind else {
            panic!()
        };
        let spans = spans_unwrapped(inlines, &Theme::dark(), Style::new(), &[]);
        let joined: String = spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "text[a]");
    }
}
