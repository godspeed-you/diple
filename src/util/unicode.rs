//! Unicode-aware width, truncation, padding and word wrapping.
//!
//! Every width in the layout engine is a *display width in terminal cells*
//! measured here — never `str::len()`. Wide East-Asian characters
//! count as two cells, combining marks as zero, and emoji ZWJ sequences and
//! variation-selector sequences count as one two-cell grapheme cluster.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// Zero-width joiner.
const ZWJ: char = '\u{200d}';
/// Emoji presentation selector.
const VS16: char = '\u{fe0f}';
/// Text presentation selector.
const VS15: char = '\u{fe0e}';

/// Display width of one grapheme cluster in terminal cells.
///
/// * clusters containing a ZWJ or an emoji presentation selector are treated
///   as a single emoji of width 2 (`👩‍💻`, `⚠️`),
/// * a cluster whose base character is wide counts 2,
/// * combining marks add nothing,
/// * control and bidi characters count 1, because the terminal never sees
///   them: [`crate::util::text`] draws each as a space or `U+FFFD`, and a
///   width that disagreed would let a hostile line outgrow its column.
pub fn grapheme_width(cluster: &str) -> usize {
    if cluster.is_empty() {
        return 0;
    }
    if cluster.contains(ZWJ) || cluster.contains(VS16) {
        return 2;
    }
    let mut w = 0usize;
    for c in cluster.chars() {
        if c == VS15 {
            continue;
        }
        // `unicode-width` has no width for a control, and zero for the bidi
        // marks and NUL; only that rare zero case needs the full check.
        w += match UnicodeWidthChar::width(c) {
            None => 1,
            Some(0) if crate::util::text::replacement(c).is_some() => 1,
            Some(w) => w,
        };
    }
    w
}

/// Whether `b` is a printable ASCII byte.
///
/// Every ASCII byte in `0x20..=0x7e` is exactly one cell wide; `\x7f` (DEL) and
/// everything below `0x20` are control characters that are drawn replaced,
/// which [`grapheme_width`] accounts for. The fast path below therefore has to
/// exclude them, otherwise `width` and `grapheme_width` disagree and
/// `width(split_at_width(s, n).0) > n` becomes reachable (over-counted table
/// and column widths).
fn is_printable_ascii(b: u8) -> bool {
    b.wrapping_sub(0x20) < 0x5f
}

/// Display width of a string in terminal cells.
pub fn width(s: &str) -> usize {
    let bytes = s.as_bytes();
    if bytes.iter().all(|b| is_printable_ascii(*b)) {
        // Fast path: printable ASCII is one cell per byte.
        return bytes.len();
    }
    s.graphemes(true).map(grapheme_width).sum()
}

/// Display width of a single character (0 for combining marks and controls).
pub fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Iterator over the extended grapheme clusters of `s`.
pub fn graphemes(s: &str) -> impl Iterator<Item = &str> {
    s.graphemes(true)
}

/// Split `s` at the last grapheme boundary whose prefix width is `<= max`.
///
/// Returns `(head, tail)`; `head` is never wider than `max` and never splits a
/// grapheme cluster. A wide character straddling the limit stays in `tail`.
pub fn split_at_width(s: &str, max: usize) -> (&str, &str) {
    let mut w = 0usize;
    for (idx, g) in s.grapheme_indices(true) {
        let gw = grapheme_width(g);
        if w + gw > max {
            return (&s[..idx], &s[idx..]);
        }
        w += gw;
    }
    (s, "")
}

/// Truncate `s` to at most `max` cells on a grapheme boundary.
pub fn truncate_to_width(s: &str, max: usize) -> &str {
    split_at_width(s, max).0
}

/// Truncate `s` to `max` cells, appending `ellipsis` if truncation happened.
pub fn truncate_with_ellipsis(s: &str, max: usize, ellipsis: &str) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let ew = width(ellipsis);
    let keep = max.saturating_sub(ew);
    let mut out = truncate_to_width(s, keep).to_string();
    if max >= ew {
        out.push_str(ellipsis);
    }
    out
}

/// Pad `s` with spaces on the right to exactly `target` cells (a longer string
/// is returned unchanged).
pub fn pad_to_width(s: &str, target: usize) -> String {
    let w = width(s);
    if w >= target {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + target - w);
    out.push_str(s);
    out.push_str(&" ".repeat(target - w));
    out
}

/// Pad `s` on the left (right-align).
pub fn pad_left_to_width(s: &str, target: usize) -> String {
    let w = width(s);
    if w >= target {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + target - w);
    out.push_str(&" ".repeat(target - w));
    out.push_str(s);
    out
}

/// Centre `s` within `target` cells.
pub fn center_to_width(s: &str, target: usize) -> String {
    let w = width(s);
    if w >= target {
        return s.to_string();
    }
    let left = (target - w) / 2;
    let right = target - w - left;
    let mut out = String::with_capacity(s.len() + target - w);
    out.push_str(&" ".repeat(left));
    out.push_str(s);
    out.push_str(&" ".repeat(right));
    out
}

/// Extract the columns `[start, start + len)` of `s` as a new string.
///
/// If a wide grapheme straddles either cut, it is replaced by spaces for the
/// covered half so that the result is exactly as wide as the visible region
/// and no character is ever split in half (wide-char handling).
pub fn slice_columns(s: &str, start: usize, len: usize) -> String {
    if len == 0 {
        return String::new();
    }
    let end = start.saturating_add(len);
    let mut out = String::new();
    let mut col = 0usize;
    for g in s.graphemes(true) {
        let gw = grapheme_width(g);
        if gw == 0 {
            // Zero-width marks attach to the previous cluster.
            if col > start && col <= end && !out.is_empty() {
                out.push_str(g);
            }
            continue;
        }
        let g_end = col + gw;
        if g_end <= start || col >= end {
            col = g_end;
            continue;
        }
        if col >= start && g_end <= end {
            out.push_str(g);
        } else {
            // Partially visible wide grapheme: pad the visible cells.
            let visible = g_end.min(end).saturating_sub(col.max(start));
            out.push_str(&" ".repeat(visible));
        }
        col = g_end;
    }
    out
}

/// Expand tabs to the next multiple of `tab_width` (minimum 1).
pub fn expand_tabs(line: &str, tab_width: usize) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let tab_width = tab_width.max(1);
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0usize;
    for g in line.graphemes(true) {
        if g == "\t" {
            let n = tab_width - (col % tab_width);
            out.push_str(&" ".repeat(n));
            col += n;
        } else {
            out.push_str(g);
            col += grapheme_width(g);
        }
    }
    out
}

/// Kind of a token produced by [`tokenize`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A run of non-space characters that should not be broken unless it does
    /// not fit on a line by itself.
    Word,
    /// A run of whitespace: a break opportunity, dropped at line ends.
    Space,
}

/// A token with its display width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    /// Token text.
    pub text: &'a str,
    /// Token class.
    pub kind: TokenKind,
    /// Display width.
    pub width: usize,
}

/// Split text into word/space tokens.
///
/// Break opportunities are whitespace boundaries *and* the boundaries around
/// wide (East-Asian / emoji) graphemes, so CJK text wraps at any character as
/// required for wide characters.
pub fn tokenize(s: &str) -> Vec<Token<'_>> {
    let mut out: Vec<Token<'_>> = Vec::new();
    let mut start: Option<(usize, TokenKind)> = None;
    let mut acc_width = 0usize;
    fn flush<'t>(
        s: &'t str,
        out: &mut Vec<Token<'t>>,
        start: &mut Option<(usize, TokenKind)>,
        acc: &mut usize,
        end: usize,
    ) {
        if let Some((s0, kind)) = start.take() {
            if end > s0 {
                out.push(Token {
                    text: &s[s0..end],
                    kind,
                    width: *acc,
                });
            }
        }
        *acc = 0;
    }

    for (idx, g) in s.grapheme_indices(true) {
        let gw = grapheme_width(g);
        let is_space = g.chars().all(|c| c.is_whitespace());
        let kind = if is_space {
            TokenKind::Space
        } else {
            TokenKind::Word
        };
        let wide = gw >= 2 && kind == TokenKind::Word;
        match start {
            Some((_, k)) if k == kind && !wide => {
                acc_width += gw;
            }
            Some(_) => {
                flush(s, &mut out, &mut start, &mut acc_width, idx);
                start = Some((idx, kind));
                acc_width = gw;
            }
            None => {
                start = Some((idx, kind));
                acc_width = gw;
            }
        }
        if wide {
            // A wide grapheme is a token of its own: break before and after.
            flush(s, &mut out, &mut start, &mut acc_width, idx + g.len());
        }
    }
    flush(s, &mut out, &mut start, &mut acc_width, s.len());
    out
}

/// Word-wrap plain text.
///
/// `first_width` applies to the first line, `rest_width` to all following ones
/// (hanging indent). Words that do not fit on a line of their own are hard
/// broken at grapheme boundaries (long URLs). Widths of `0` are clamped to `1`
/// so this function always terminates.
pub fn wrap(text: &str, first_width: usize, rest_width: usize) -> Vec<String> {
    let first_width = first_width.max(1);
    let rest_width = rest_width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    let mut pending: Option<usize> = None;

    let avail = |lines: &Vec<String>| {
        if lines.is_empty() {
            first_width
        } else {
            rest_width
        }
    };

    for token in tokenize(text) {
        match token.kind {
            TokenKind::Space => {
                if !cur.is_empty() {
                    pending = Some(pending.unwrap_or(0) + token.width.min(rest_width));
                }
            }
            TokenKind::Word => {
                let pad = pending.take().unwrap_or(0);
                let mut a = avail(&lines);
                if !cur.is_empty() && cur_w + pad + token.width > a {
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                    a = avail(&lines);
                } else if !cur.is_empty() && pad > 0 {
                    cur.push_str(&" ".repeat(pad));
                    cur_w += pad;
                }
                let mut rest = token.text;
                loop {
                    let room = a.saturating_sub(cur_w);
                    if width(rest) <= room {
                        cur.push_str(rest);
                        cur_w += width(rest);
                        break;
                    }
                    let (head, tail) = split_at_width(rest, room);
                    if head.is_empty() && cur.is_empty() {
                        // The very first grapheme is wider than the line:
                        // emit it anyway to guarantee progress.
                        let mut it = rest.grapheme_indices(true);
                        let end = it.nth(1).map(|(i, _)| i).unwrap_or(rest.len());
                        cur.push_str(&rest[..end]);
                        rest = &rest[end..];
                    } else {
                        cur.push_str(head);
                        rest = tail;
                    }
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                    a = avail(&lines);
                    if rest.is_empty() {
                        break;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_cjk_widths() {
        assert_eq!(width("hello"), 5);
        assert_eq!(width("日本語"), 6);
        assert_eq!(width("日本語のテキスト"), 16);
        assert_eq!(width("한국어"), 6);
        assert_eq!(width("中文a"), 5);
        assert_eq!(width(""), 0);
    }

    #[test]
    fn control_characters_are_as_wide_as_what_replaces_them() {
        // They never reach the terminal as themselves: `util::text` draws each
        // as a space or U+FFFD, one cell. Counting them as zero let a hostile
        // line outgrow its column once it was sanitised. The fast path must
        // still agree with `grapheme_width`, or `width(split_at_width(s,
        // n).0) > n` becomes reachable.
        assert_eq!(width("\u{1}"), 1);
        assert_eq!(width("\u{7f}"), 1, "DEL");
        assert_eq!(width("a\u{1}b"), 3);
        assert_eq!(width("a\u{202e}b"), 3, "a bidi override");
        assert_eq!(width(crate::util::text::sanitized("a\u{1}b").as_ref()), 3);
        for s in ["a\u{1}b", "\u{7}\u{7}\u{7}", "x\u{1b}[0m"] {
            assert_eq!(
                width(s),
                s.graphemes(true).map(grapheme_width).sum::<usize>(),
                "the fast path must agree with grapheme_width for {s:?}"
            );
            for n in 0..=4 {
                assert!(
                    width(split_at_width(s, n).0) <= n,
                    "split_at_width({s:?}, {n}) over-counted"
                );
            }
        }
    }

    #[test]
    fn combining_marks_are_zero_width() {
        assert_eq!(width("e\u{0301}"), 1); // e + combining acute
        assert_eq!(width("é"), 1);
        assert_eq!(width("a\u{0300}\u{0301}b"), 2);
    }

    #[test]
    fn emoji_widths() {
        assert_eq!(width("🚀"), 2);
        assert_eq!(width("👩‍💻"), 2, "ZWJ sequence is one emoji");
        assert_eq!(width("👨‍👩‍👧‍👦"), 2, "family ZWJ sequence");
        assert_eq!(width("⚠️"), 2, "variation selector 16");
        assert_eq!(width("🇩🇪"), 2, "regional indicator pair");
        assert_eq!(width("✅"), 2);
    }

    #[test]
    fn truncate_never_splits_a_grapheme() {
        assert_eq!(truncate_to_width("日本語", 3), "日");
        assert_eq!(truncate_to_width("日本語", 4), "日本");
        assert_eq!(truncate_to_width("abc", 10), "abc");
        assert_eq!(truncate_to_width("👩‍💻x", 2), "👩‍💻");
        assert_eq!(truncate_to_width("👩‍💻x", 1), "");
        assert_eq!(truncate_with_ellipsis("abcdef", 4, "…"), "abc…");
        assert_eq!(truncate_with_ellipsis("abc", 4, "…"), "abc");
    }

    #[test]
    fn padding() {
        assert_eq!(pad_to_width("日", 4), "日  ");
        assert_eq!(pad_left_to_width("日", 4), "  日");
        assert_eq!(center_to_width("ab", 6), "  ab  ");
        assert_eq!(pad_to_width("abcdef", 3), "abcdef");
    }

    #[test]
    fn slicing_across_a_wide_char_pads_with_spaces() {
        // "a日b": columns a=0, 日=1..2, b=3
        assert_eq!(slice_columns("a日b", 0, 4), "a日b");
        assert_eq!(slice_columns("a日b", 2, 2), " b");
        assert_eq!(slice_columns("a日b", 0, 2), "a ");
        assert_eq!(slice_columns("a日b", 1, 1), " ");
        assert_eq!(slice_columns("日本語", 1, 4), " 本 ");
        assert_eq!(slice_columns("abc", 5, 3), "");
        assert_eq!(slice_columns("abc", 0, 0), "");
        assert_eq!(width(&slice_columns("日本語", 1, 4)), 4);
    }

    #[test]
    fn tab_expansion() {
        assert_eq!(expand_tabs("a\tb", 4), "a   b");
        assert_eq!(expand_tabs("\tx", 4), "    x");
        assert_eq!(expand_tabs("abcd\tx", 4), "abcd    x");
        assert_eq!(expand_tabs("no tabs", 4), "no tabs");
        assert_eq!(expand_tabs("a\tb", 1), "a b");
    }

    #[test]
    fn wrapping_words() {
        assert_eq!(
            wrap("the quick brown fox", 10, 10),
            ["the quick", "brown fox"]
        );
        assert_eq!(wrap("one two", 20, 20), ["one two"]);
        assert_eq!(wrap("", 10, 10), [""]);
    }

    #[test]
    fn hanging_indent_widths() {
        let out = wrap("aaa bbb ccc ddd", 7, 4);
        assert_eq!(out, ["aaa bbb", "ccc", "ddd"]);
    }

    #[test]
    fn unbreakable_runs_are_hard_broken() {
        let url = "https://example.com/a/very/long/path";
        let out = wrap(url, 10, 10);
        assert!(out.iter().all(|l| width(l) <= 10));
        assert_eq!(out.concat(), url);
    }

    #[test]
    fn cjk_breaks_anywhere() {
        let out = wrap("日本語のテキストです", 6, 6);
        assert!(out.iter().all(|l| width(l) <= 6), "{out:?}");
        assert_eq!(out.concat(), "日本語のテキストです");
    }

    #[test]
    fn zero_width_never_loops() {
        let out = wrap("abc def", 0, 0);
        assert!(!out.is_empty());
        assert!(out.iter().all(|l| width(l) <= 1));
    }

    #[test]
    fn tokenize_splits_wide_graphemes() {
        let t = tokenize("ab 日x");
        let texts: Vec<_> = t.iter().map(|t| t.text).collect();
        assert_eq!(texts, ["ab", " ", "日", "x"]);
        assert_eq!(t[2].width, 2);
    }
}
