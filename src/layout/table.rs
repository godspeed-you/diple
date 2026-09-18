//! Table layout.
//!
//! The algorithm, in order:
//!
//! 1. determine the available width (terminal width minus the surrounding
//!    indentation, passed in by the caller),
//! 2. measure a **minimum** width per column — the longest unbreakable token,
//!    capped by [`MIN_WIDTH_CAP`] and by the column's preferred width,
//! 3. measure a **preferred** width per column — the widest cell content,
//!    capped by `max_column_width`,
//! 4. allocate: if all preferred widths plus the border overhead fit, use
//!    them; otherwise shrink proportionally to each column's slack
//!    (`preferred - minimum`), never below the minimum,
//! 5. wrap cell content to the allocated width; row height is the tallest
//!    cell, cells align to the top,
//! 6. columns whose content is entirely numeric or inline code are shrunk
//!    **last**, so code and numeric fields keep their width where
//!    possible,
//! 7. if even the minimum widths do not fit, `auto` switches to `scroll` and
//!    emits full-width lines for horizontal scrolling.
//!
//! Everything is clamped: ragged rows, zero columns and zero-width terminals
//! must never panic.

use crate::config::schema::TableMode;
use crate::document::markdown::ast::inlines_to_text;
use crate::document::markdown::{Alignment, Inlines, Table};
use crate::document::Match;
use crate::layout::inline::{layout_inlines, push_span, spans_unwrapped};
use crate::render::primitives::StyledSpan;
use crate::render::theme::{Style, Theme};
use crate::util::unicode;

/// Upper bound for a column's minimum width: a longer unbreakable token is
/// hard broken rather than forcing the whole table to grow.
pub const MIN_WIDTH_CAP: usize = 16;

/// Options for table layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableOptions {
    /// Layout mode.
    pub mode: TableMode,
    /// Maximum width of a single column.
    pub max_column_width: usize,
    /// Unicode box drawing available (ASCII `+-|` fallback otherwise).
    pub unicode: bool,
}

impl Default for TableOptions {
    fn default() -> Self {
        Self {
            mode: TableMode::Auto,
            max_column_width: 60,
            unicode: true,
        }
    }
}

/// Measurements of one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnMeasure {
    /// Smallest width the column can be squeezed into.
    pub min: usize,
    /// Width at which no cell needs wrapping (capped by `max_column_width`).
    pub preferred: usize,
    /// `true` when every non-empty cell is numeric or inline code: these
    /// columns are shrunk last.
    pub priority: bool,
}

/// Box drawing characters (Unicode or ASCII fallback).
struct Glyphs {
    h: &'static str,
    v: &'static str,
    tl: &'static str,
    tm: &'static str,
    tr: &'static str,
    ml: &'static str,
    mm: &'static str,
    mr: &'static str,
    bl: &'static str,
    bm: &'static str,
    br: &'static str,
}

const UNICODE_GLYPHS: Glyphs = Glyphs {
    h: "─",
    v: "│",
    tl: "┌",
    tm: "┬",
    tr: "┐",
    ml: "├",
    mm: "┼",
    mr: "┤",
    bl: "└",
    bm: "┴",
    br: "┘",
};

const ASCII_GLYPHS: Glyphs = Glyphs {
    h: "-",
    v: "|",
    tl: "+",
    tm: "+",
    tr: "+",
    ml: "+",
    mm: "+",
    mr: "+",
    bl: "+",
    bm: "+",
    br: "+",
};

/// Number of columns, tolerating ragged rows.
pub fn column_count(table: &Table) -> usize {
    table
        .header
        .len()
        .max(table.alignments.len())
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0))
}

fn longest_token(text: &str) -> usize {
    unicode::tokenize(text)
        .iter()
        .filter(|t| t.kind == unicode::TokenKind::Word)
        .map(|t| t.width)
        .max()
        .unwrap_or(0)
}

fn is_numeric(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty()
        && t.chars().all(|c| {
            c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+' | '%' | ':' | '/' | ' ' | '_')
        })
}

fn is_code_cell(cell: &Inlines) -> bool {
    !cell.is_empty()
        && cell
            .iter()
            .all(|i| matches!(i, crate::document::markdown::Inline::Code(_)))
}

/// Measure every column of the table (steps 2, 3 and 6 of the algorithm).
pub fn measure_columns(table: &Table, max_column_width: usize) -> Vec<ColumnMeasure> {
    let ncols = column_count(table);
    let cap = max_column_width.max(1);
    let mut out = Vec::with_capacity(ncols);
    for c in 0..ncols {
        let mut preferred = 0usize;
        let mut min = 0usize;
        let mut priority = true;
        let mut any = false;
        let mut cells: Vec<&Inlines> = Vec::with_capacity(table.rows.len() + 1);
        if let Some(cell) = table.header.get(c) {
            cells.push(cell);
        }
        for row in &table.rows {
            if let Some(cell) = row.get(c) {
                cells.push(cell);
            }
        }
        for (i, cell) in cells.iter().enumerate() {
            let text = inlines_to_text(cell);
            preferred = preferred.max(unicode::width(&text));
            min = min.max(longest_token(&text));
            if !text.trim().is_empty() {
                any = true;
                // The header does not decide numeric-ness, but must not veto
                // it either.
                if i > 0 || table.header.get(c).is_none() {
                    priority &= is_numeric(&text) || is_code_cell(cell);
                }
            }
        }
        let preferred = preferred.min(cap).max(1);
        let min = min.min(MIN_WIDTH_CAP).min(preferred).max(1);
        out.push(ColumnMeasure {
            min,
            preferred,
            priority: priority && any,
        });
    }
    out
}

/// Distribute `avail` content columns over the measured columns.
///
/// Returns the allocated content widths (without padding or borders). When
/// the preferred widths fit they are used verbatim; otherwise the excess is
/// removed proportionally to each column's slack, taking from non-priority
/// columns first and from numeric/code columns only when that is not enough.
/// If not even the minimum widths fit, the minimum widths are returned (the
/// caller then decides whether to scroll).
pub fn allocate(measures: &[ColumnMeasure], avail: usize) -> Vec<usize> {
    let n = measures.len();
    if n == 0 {
        return Vec::new();
    }
    let pref: Vec<usize> = measures.iter().map(|m| m.preferred.max(1)).collect();
    let min: Vec<usize> = measures
        .iter()
        .enumerate()
        .map(|(i, m)| m.min.max(1).min(pref[i]))
        .collect();
    let total_pref: usize = pref.iter().sum();
    if total_pref <= avail {
        return pref;
    }
    let total_min: usize = min.iter().sum();
    if total_min >= avail {
        return min;
    }

    let mut widths = pref.clone();
    let mut excess = total_pref - avail;
    for priority_pass in [false, true] {
        if excess == 0 {
            break;
        }
        let idxs: Vec<usize> = (0..n)
            .filter(|&i| measures[i].priority == priority_pass)
            .collect();
        let slack: usize = idxs.iter().map(|&i| widths[i] - min[i]).sum();
        if slack == 0 {
            continue;
        }
        let take = excess.min(slack);
        let mut given = 0usize;
        // Proportional share, floor; deterministic largest-slack-first
        // distribution of the rounding remainder.
        let mut shares: Vec<(usize, usize)> = Vec::with_capacity(idxs.len());
        for &i in &idxs {
            let s = (widths[i] - min[i]) * take / slack;
            shares.push((i, s));
            given += s;
        }
        let mut order: Vec<usize> = idxs.clone();
        order.sort_by_key(|&i| (std::cmp::Reverse(widths[i] - min[i]), i));
        let mut rest = take - given;
        for &i in &order {
            if rest == 0 {
                break;
            }
            if let Some(entry) = shares.iter_mut().find(|(j, _)| *j == i) {
                if widths[i] - min[i] > entry.1 {
                    entry.1 += 1;
                    // Defensive: `rest` is non-zero here by the loop guard.
                    rest = rest.saturating_sub(1);
                }
            }
        }
        for (i, s) in shares {
            widths[i] -= s;
            // Defensive: `sum(shares) == take <= excess` by construction, but a
            // `usize` underflow here would be a debug panic / release wrap.
            excess = excess.saturating_sub(s);
        }
    }
    widths
}

/// Content of one laid-out cell.
struct Cell {
    lines: Vec<Vec<StyledSpan>>,
}

#[allow(clippy::too_many_arguments)]
fn build_cell(
    cell: &Inlines,
    theme: &Theme,
    base: Style,
    matches: &[Match],
    offset: usize,
    width: usize,
    wrap: bool,
) -> Cell {
    let lines = if wrap {
        layout_inlines(cell, theme, base, matches, offset, width, width)
    } else {
        vec![spans_unwrapped_at(cell, theme, base, matches, offset)]
    };
    Cell { lines }
}

fn spans_unwrapped_at(
    cell: &Inlines,
    theme: &Theme,
    base: Style,
    matches: &[Match],
    offset: usize,
) -> Vec<StyledSpan> {
    if offset == 0 {
        return spans_unwrapped(cell, theme, base, matches);
    }
    let mut off = offset;
    let pieces = crate::layout::inline::flatten(cell, theme, base, &mut off);
    let pieces = crate::layout::inline::apply_matches(pieces, matches);
    let mut line = Vec::new();
    for p in &pieces {
        push_span(&mut line, &p.text, p.style, p.link, p.search);
    }
    line
}

/// Plain-text offsets of every cell inside the node's search text.
///
/// Mirrors `document::search::node_text` for tables: header cells joined with
/// a space, every row prefixed by a newline, cells joined with a space.
fn cell_offsets(table: &Table) -> (Vec<usize>, Vec<Vec<usize>>) {
    let mut pos = 0usize;
    let mut first = true;
    let mut header = Vec::with_capacity(table.header.len());
    for cell in &table.header {
        if !first {
            pos += 1;
        }
        first = false;
        header.push(pos);
        pos += inlines_to_text(cell).len();
    }
    let mut rows = Vec::with_capacity(table.rows.len());
    for row in &table.rows {
        pos += 1; // '\n'
        let mut offs = Vec::with_capacity(row.len());
        let mut first_in_row = true;
        for cell in row {
            if !first_in_row {
                pos += 1;
            }
            first_in_row = false;
            offs.push(pos);
            pos += inlines_to_text(cell).len();
        }
        rows.push(offs);
    }
    (header, rows)
}

fn align_line(
    spans: Vec<StyledSpan>,
    width: usize,
    align: Alignment,
    style: Style,
) -> Vec<StyledSpan> {
    let w: usize = spans.iter().map(StyledSpan::width).sum();
    let mut out: Vec<StyledSpan> = Vec::with_capacity(spans.len() + 2);
    if w >= width {
        return spans;
    }
    let pad = width - w;
    let (left, right) = match align {
        Alignment::Right => (pad, 0),
        Alignment::Center => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    if left > 0 {
        out.push(StyledSpan::new(" ".repeat(left), style));
    }
    out.extend(spans);
    if right > 0 {
        out.push(StyledSpan::new(" ".repeat(right), style));
    }
    out
}

/// Lay out a table into lines of spans (without any outer indentation).
pub fn layout_table(
    table: &Table,
    theme: &Theme,
    opts: &TableOptions,
    width: usize,
    matches: &[Match],
) -> Vec<Vec<StyledSpan>> {
    let ncols = column_count(table);
    if ncols == 0 {
        return Vec::new();
    }
    let width = width.max(1);
    let glyphs = if opts.unicode {
        &UNICODE_GLYPHS
    } else {
        &ASCII_GLYPHS
    };
    let measures = measure_columns(table, opts.max_column_width);
    let compact = opts.mode == TableMode::Compact;

    let overhead = if compact {
        2 * ncols.saturating_sub(1)
    } else {
        3 * ncols + 1
    };
    let avail = width.saturating_sub(overhead);
    let total_pref: usize = measures.iter().map(|m| m.preferred).sum();
    let total_min: usize = measures.iter().map(|m| m.min).sum();

    // Resolve the effective mode (`auto` scrolls only when even the minimum
    // widths do not fit). In scroll mode nothing is wrapped, so the columns
    // must be as wide as their *uncapped* content — otherwise long cells would
    // overflow their column and break the borders.
    let full = measure_columns(table, usize::MAX);
    let (widths, wrap) = match opts.mode {
        TableMode::Scroll => (full.iter().map(|m| m.preferred).collect::<Vec<_>>(), false),
        TableMode::Wrap => (allocate(&measures, avail.max(ncols)), true),
        TableMode::Compact | TableMode::Auto => {
            if total_pref <= avail {
                (
                    measures.iter().map(|m| m.preferred).collect::<Vec<_>>(),
                    true,
                )
            } else if total_min <= avail {
                (allocate(&measures, avail), true)
            } else if compact {
                (allocate(&measures, avail.max(ncols)), true)
            } else {
                (full.iter().map(|m| m.preferred).collect::<Vec<_>>(), false)
            }
        }
    };
    let widths: Vec<usize> = widths.iter().map(|w| (*w).max(1)).collect();
    let (header_offsets, row_offsets) = cell_offsets(table);

    let border_style = theme.table_border;
    let mut out: Vec<Vec<StyledSpan>> = Vec::new();

    let rule = |left: &str, mid: &str, right: &str| -> Vec<StyledSpan> {
        let mut s = String::new();
        if compact {
            for (i, w) in widths.iter().enumerate() {
                if i > 0 {
                    s.push_str("  ");
                }
                s.push_str(&glyphs.h.repeat(*w));
            }
        } else {
            s.push_str(left);
            for (i, w) in widths.iter().enumerate() {
                if i > 0 {
                    s.push_str(mid);
                }
                s.push_str(&glyphs.h.repeat(w + 2));
            }
            s.push_str(right);
        }
        vec![StyledSpan::new(s, border_style)]
    };

    let emit_row = |cells: &[Cell], is_header: bool, out: &mut Vec<Vec<StyledSpan>>| {
        let height = cells
            .iter()
            .map(|c| c.lines.len())
            .max()
            .unwrap_or(1)
            .max(1);
        for row_line in 0..height {
            let mut line: Vec<StyledSpan> = Vec::new();
            if !compact {
                push_span(&mut line, glyphs.v, border_style, None, false);
            }
            for (c, w) in widths.iter().enumerate() {
                if compact && c > 0 {
                    push_span(&mut line, "  ", Style::new(), None, false);
                }
                if !compact {
                    push_span(&mut line, " ", Style::new(), None, false);
                }
                let empty: Vec<StyledSpan> = Vec::new();
                let content = cells
                    .get(c)
                    .and_then(|cell| cell.lines.get(row_line))
                    .cloned()
                    .unwrap_or(empty);
                let align = table.alignments.get(c).copied().unwrap_or(Alignment::None);
                let style = if is_header {
                    theme.table_header
                } else {
                    Style::new()
                };
                for span in align_line(content, *w, align, style) {
                    push_span(
                        &mut line,
                        &span.text,
                        span.style,
                        span.link,
                        span.search_match,
                    );
                }
                if !compact {
                    push_span(&mut line, " ", Style::new(), None, false);
                    push_span(&mut line, glyphs.v, border_style, None, false);
                }
            }
            out.push(line);
        }
    };

    if !compact {
        out.push(rule(glyphs.tl, glyphs.tm, glyphs.tr));
    }
    let header_cells: Vec<Cell> = (0..ncols)
        .map(|c| {
            let empty = Inlines::new();
            let cell = table.header.get(c).unwrap_or(&empty);
            let offset = header_offsets.get(c).copied().unwrap_or(0);
            build_cell(
                cell,
                theme,
                theme.table_header,
                matches,
                offset,
                widths[c],
                wrap,
            )
        })
        .collect();
    emit_row(&header_cells, true, &mut out);
    out.push(rule(glyphs.ml, glyphs.mm, glyphs.mr));

    for (r, row) in table.rows.iter().enumerate() {
        let cells: Vec<Cell> = (0..ncols)
            .map(|c| {
                let empty = Inlines::new();
                let cell = row.get(c).unwrap_or(&empty);
                let offset = row_offsets
                    .get(r)
                    .and_then(|o| o.get(c))
                    .copied()
                    .unwrap_or(0);
                build_cell(cell, theme, Style::new(), matches, offset, widths[c], wrap)
            })
            .collect();
        emit_row(&cells, false, &mut out);
    }
    if !compact {
        out.push(rule(glyphs.bl, glyphs.bm, glyphs.br));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::markdown::{parse, NodeKind};

    fn table_of(src: &str) -> Table {
        let doc = parse(src);
        for n in doc.walk() {
            if let NodeKind::Table(t) = &n.kind {
                return t.clone();
            }
        }
        panic!("no table");
    }

    fn render(t: &Table, opts: &TableOptions, width: usize) -> Vec<String> {
        layout_table(t, &Theme::dark(), opts, width, &[])
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect::<String>())
            .collect()
    }

    const WIDE: &str = "\
| Identifier | Type | Count | Description |
|---|---:|---|---|
| `alpha` | string | 12 | A fairly long description that needs wrapping in narrow terminals |
| `beta` | integer | 4711 | Short |
";

    #[test]
    fn preferred_widths_when_everything_fits() {
        let t = table_of("| a | b |\n|---|---|\n| 1 | 22 |\n");
        let lines = render(&t, &TableOptions::default(), 80);
        assert_eq!(lines[0], "┌───┬────┐");
        assert_eq!(lines[1], "│ a │ b  │");
        assert_eq!(lines[2], "├───┼────┤");
        assert_eq!(lines[3], "│ 1 │ 22 │");
        assert_eq!(lines[4], "└───┴────┘");
    }

    #[test]
    fn every_line_fits_the_width_at_80_and_120() {
        let t = table_of(WIDE);
        for w in [80usize, 120] {
            let lines = render(&t, &TableOptions::default(), w);
            for l in &lines {
                assert!(
                    unicode::width(l) <= w,
                    "width {w}: line {:?} is {} wide",
                    l,
                    unicode::width(l)
                );
            }
            assert!(lines.len() >= 5);
        }
    }

    /// Widths of the drawn columns, read back out of the top border
    /// (`┌───┬────┐`) — each includes the one column of padding either side.
    fn column_widths(lines: &[String]) -> Vec<usize> {
        lines
            .first()
            .expect("a top border")
            .trim_matches(|c| c == '┌' || c == '┐')
            .split('┬')
            .map(unicode::width)
            .collect()
    }

    /// The rule: columns holding inline code or numbers keep their full width
    /// and prose columns give way, because a truncated identifier or figure is
    /// useless while a wrapped sentence is merely longer.
    ///
    /// Read off the rendered table: the identifier and count columns stay at
    /// the width their content needs at every width the table fits in, and
    /// the description column is the only one that shrinks.
    #[test]
    fn numeric_and_code_columns_shrink_last() {
        let t = table_of(WIDE);
        let widest = column_widths(&render(&t, &TableOptions::default(), 200));
        for w in [46usize, 48, 60, 80] {
            let lines = render(&t, &TableOptions::default(), w);
            let cols = column_widths(&lines);
            assert_eq!(cols.len(), 4, "width {w}: four columns");
            assert_eq!(cols[0], widest[0], "width {w}: inline-code column kept");
            assert_eq!(cols[2], widest[2], "width {w}: numeric column kept");
            assert!(
                cols[3] < widest[3],
                "width {w}: the prose column shrank: {cols:?}"
            );
            assert!(
                lines.iter().all(|l| unicode::width(l) <= w),
                "width {w}: the table fits"
            );
            // The kept columns really do show their content unwrapped.
            for needle in ["alpha", "beta", "4711"] {
                assert!(
                    lines.iter().any(|l| l.contains(needle)),
                    "width {w}: {needle:?} survives intact"
                );
            }
        }
    }

    /// Shrinking has a floor: rather than squeezing a column to nothing, the
    /// table stops at its minimum and — when even the minimums do not fit —
    /// falls back to a full-width, horizontally scrolled table.
    #[test]
    fn shrinking_never_goes_below_minimum() {
        let t = table_of(WIDE);
        for w in 4usize..=100 {
            let lines = render(&t, &TableOptions::default(), w);
            let cols = column_widths(&lines);
            if lines.iter().all(|l| unicode::width(l) <= w) {
                for (i, c) in cols.iter().enumerate() {
                    assert!(
                        *c >= 3,
                        "width {w}: column {i} keeps a padded cell of content: {cols:?}"
                    );
                }
            } else {
                // Scroll fallback: nothing was squeezed — the columns are
                // exactly the ones the explicit scroll mode draws.
                let scroll = TableOptions {
                    mode: TableMode::Scroll,
                    ..TableOptions::default()
                };
                assert_eq!(
                    cols,
                    column_widths(&render(&t, &scroll, w)),
                    "width {w}: the fallback is the scroll rendering"
                );
            }
        }
    }

    /// `max_column_width` caps a single column no matter how wide the
    /// terminal is, so one long prose column cannot push the rest off screen.
    #[test]
    fn max_column_width_is_respected() {
        let opts = TableOptions {
            max_column_width: 10,
            ..TableOptions::default()
        };
        let t = table_of(WIDE);
        let lines = render(&t, &opts, 120);
        for (i, c) in column_widths(&lines).iter().enumerate() {
            assert!(*c <= 12, "column {i} is capped at 10 plus its padding: {c}");
        }
        assert!(lines.iter().all(|l| unicode::width(l) <= 120));
        // Capped, not truncated: the long description is still all there.
        let body: String = lines.join(" ");
        for word in ["fairly", "wrapping", "terminals"] {
            assert!(body.contains(word), "{word:?} survives the cap: {body}");
        }
    }

    #[test]
    fn scroll_mode_emits_full_width_lines() {
        let t = table_of(WIDE);
        let opts = TableOptions {
            mode: TableMode::Scroll,
            ..TableOptions::default()
        };
        let lines = render(&t, &opts, 40);
        assert!(
            lines.iter().any(|l| unicode::width(l) > 40),
            "scroll mode keeps the table wide"
        );
        // No cell is wrapped: header + two rows plus 3 rules.
        assert_eq!(lines.len(), 3 + 3);
    }

    #[test]
    fn wrap_mode_always_wraps() {
        let t = table_of(WIDE);
        let opts = TableOptions {
            mode: TableMode::Wrap,
            ..TableOptions::default()
        };
        let lines = render(&t, &opts, 30);
        assert!(lines.len() > 5, "content wrapped onto several lines");
    }

    #[test]
    fn compact_mode_has_no_outer_borders() {
        let t = table_of("| a | b |\n|---|---|\n| 1 | 22 |\n");
        let opts = TableOptions {
            mode: TableMode::Compact,
            ..TableOptions::default()
        };
        let lines = render(&t, &opts, 40);
        assert_eq!(lines[0], "a  b ");
        assert_eq!(lines[1], "─  ──");
        assert_eq!(lines[2], "1  22");
        assert!(lines.iter().all(|l| !l.contains('│')));
    }

    #[test]
    fn auto_falls_back_to_scroll_when_minimums_do_not_fit() {
        let t = table_of(WIDE);
        let lines = render(&t, &TableOptions::default(), 12);
        assert!(lines.iter().any(|l| unicode::width(l) > 12));
    }

    #[test]
    fn ascii_fallback() {
        let t = table_of("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let opts = TableOptions {
            unicode: false,
            ..TableOptions::default()
        };
        let lines = render(&t, &opts, 40);
        assert_eq!(lines[0], "+---+---+");
        assert_eq!(lines[1], "| a | b |");
        assert!(lines.iter().all(|l| l.is_ascii()));
    }

    #[test]
    fn ragged_rows_do_not_panic() {
        let mut t = table_of("| a | b | c |\n|---|---|---|\n| 1 | 2 | 3 |\n");
        t.rows.push(vec![]); // too few
        let extra = t.rows[0][0].clone();
        t.rows
            .push(vec![extra.clone(), extra.clone(), extra.clone(), extra]); // too many
        t.alignments.clear();
        let lines = render(&t, &TableOptions::default(), 40);
        assert!(lines.iter().all(|l| unicode::width(l) <= 40));
        assert!(lines.len() >= 6);
    }

    #[test]
    fn zero_width_terminal_does_not_panic() {
        let t = table_of(WIDE);
        for w in [0usize, 1, 2, 3] {
            let lines = render(&t, &TableOptions::default(), w);
            assert!(!lines.is_empty());
        }
    }

    #[test]
    fn alignment_is_applied() {
        let t = table_of("| l | c | r |\n|:---|:---:|---:|\n| a | b | c |\n");
        let lines = render(&t, &TableOptions::default(), 40);
        assert_eq!(lines[1], "│ l │ c │ r │");
        let t = table_of("| head | c |\n|---:|:---:|\n| x | y |\n");
        let lines = render(&t, &TableOptions::default(), 40);
        assert_eq!(lines[3], "│    x │ y │");
    }

    #[test]
    fn cjk_cells_are_measured_by_display_width() {
        let t = table_of("| 名前 | 説明 |\n|------|------|\n| 東京 | 首都 |\n");
        let lines = render(&t, &TableOptions::default(), 40);
        let w: Vec<usize> = lines.iter().map(|l| unicode::width(l)).collect();
        assert!(
            w.windows(2).all(|x| x[0] == x[1]),
            "all lines equal width: {w:?}"
        );
    }
}
