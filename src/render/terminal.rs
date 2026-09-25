//! ratatui widgets drawing a window of a [`RenderTree`], the status bar, the
//! TOC sidebar and the help overlay.
//!
//! This module converts [`Style`] into `ratatui` styles honouring the
//! [`ColorLevel`]. It never emits terminal escape sequences itself: OSC 8
//! hyperlinks and image protocols belong to Workstream A, which uses
//! [`crate::render::primitives::StyledSpan::link`] and
//! [`crate::render::primitives::LineKind::Image`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Modifier, Style as RStyle};
use ratatui::symbols::border;
use ratatui::text::{Line as RLine, Span as RSpan};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

use crate::config::actions::Action;
use crate::document::{DocumentPath, LinkId, Match, OutlineEntry};
use crate::render::primitives::RenderTree;
use crate::render::theme::{Color, ColorLevel, Style, Theme};
use crate::util::unicode;
use crate::util::viewport::slice_line;

/// Convert a colour for the given level.
fn color(c: Color, level: ColorLevel) -> Option<RColor> {
    c.downgrade(level).map(|c| match c {
        Color::Rgb(r, g, b) => RColor::Rgb(r, g, b),
        Color::Indexed(i) => RColor::Indexed(i),
    })
}

/// Convert a [`Style`] into a `ratatui` style, honouring the colour level.
pub(crate) fn to_ratatui(style: Style, level: ColorLevel) -> RStyle {
    let style = style.downgrade(level);
    let mut out = RStyle::default();
    if let Some(fg) = style.fg.and_then(|c| color(c, level)) {
        out = out.fg(fg);
    }
    if let Some(bg) = style.bg.and_then(|c| color(c, level)) {
        out = out.bg(bg);
    }
    let mut m = Modifier::empty();
    if style.bold {
        m |= Modifier::BOLD;
    }
    if style.italic {
        m |= Modifier::ITALIC;
    }
    if style.underline {
        m |= Modifier::UNDERLINED;
    }
    if style.dim {
        m |= Modifier::DIM;
    }
    if style.strikethrough {
        m |= Modifier::CROSSED_OUT;
    }
    if style.reverse {
        m |= Modifier::REVERSED;
    }
    out.add_modifier(m)
}

/// Widget drawing a window of a [`RenderTree`].
///
/// Built as a struct literal like every other widget here, so that adding a
/// field is a compile error at each call site rather than one more positional
/// argument nobody can read.
pub(crate) struct DocumentView<'a> {
    /// The laid-out document.
    pub(crate) tree: &'a RenderTree,
    /// Absolute line at the top of the viewport.
    pub(crate) top_line: usize,
    /// Horizontal scroll offset in columns.
    pub(crate) h_offset: usize,
    /// Link drawn as selected, if any.
    pub(crate) selected_link: Option<LinkId>,
    /// The search match drawn as the current one, if any.
    pub(crate) current_match: Option<Match>,
    /// Absolute line the semantic cursor sits on, for the formats that show
    /// it.
    ///
    /// A *draw-time* input on purpose: a highlight that fed into layout would
    /// re-lay the document out on every scroll, and the cursor's identity is
    /// the node, never the row.
    pub(crate) cursor_line: Option<usize>,
    /// Theme.
    pub(crate) theme: &'a Theme,
    /// Colour level.
    pub(crate) level: ColorLevel,
}

impl DocumentView<'_> {
    /// The lines this view would draw, as ratatui text (also used by tests).
    pub(crate) fn lines(&self, width: usize, height: usize) -> Vec<RLine<'static>> {
        let mut out = Vec::with_capacity(height);
        for (offset, line) in self
            .tree
            .visible_slice(self.top_line, height)
            .iter()
            .enumerate()
        {
            let on_cursor = self.cursor_line == Some(self.top_line + offset);
            let mut spans = Vec::new();
            for span in slice_line(line, self.h_offset, width) {
                let mut style = span.style;
                if on_cursor {
                    style = style.patch(self.theme.structured.cursor_row);
                }
                if span.search_match {
                    let current = self
                        .current_match
                        .map(|m| m.node == line.node)
                        .unwrap_or(false);
                    style = style.patch(if current {
                        self.theme.search_current
                    } else {
                        self.theme.search_match
                    });
                }
                if span.link.is_some() && span.link == self.selected_link {
                    style = style.patch(self.theme.link_selected);
                }
                spans.push(RSpan::styled(span.text, to_ratatui(style, self.level)));
            }
            // A cursor row reads as a row, so the highlight runs to the right
            // edge rather than stopping where the text happens to end.
            if on_cursor {
                let used: usize = spans.iter().map(|s| unicode::width(&s.content)).sum();
                if used < width {
                    spans.push(RSpan::styled(
                        " ".repeat(width - used),
                        to_ratatui(self.theme.structured.cursor_row, self.level),
                    ));
                }
            }
            out.push(RLine::from(spans));
        }
        out
    }
}

impl Widget for DocumentView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let lines = self.lines(area.width as usize, area.height as usize);
        Paragraph::new(lines).render(area, buf);
    }
}

/// Widget drawing the status bar (`file    37%  142/380`) plus an optional
/// mode/message text or search prompt.
pub(crate) struct StatusBar<'a> {
    /// Document name shown on the left.
    pub(crate) filename: &'a str,
    /// The format the document was read as (`Markdown`, `JSON`, `YAML`).
    ///
    /// Spec §6.6 requires the detected format to be visible: a reader who
    /// piped something in needs to know whether diple agreed with them.
    pub(crate) format: &'a str,
    /// The semantic path of the selected node, for formats that have one.
    ///
    /// Already truncated by the caller is not assumed — the widget is what
    /// knows the width, so it left-truncates here (§24.3).
    pub(crate) path: Option<&'a DocumentPath>,
    /// `structured.path = "always"`: keep the breadcrumb even on a line too
    /// narrow to hold it and the progress counters both, dropping the
    /// counters instead. Under `auto` the counters win.
    pub(crate) path_always: bool,
    /// Scroll percentage.
    pub(crate) percent: u8,
    /// 1-based number of the last line displayed. The same position
    /// `percent` describes, so the two agree (`less` convention).
    pub(crate) line: usize,
    /// Total number of lines.
    pub(crate) total: usize,
    /// Mode or transient message shown after the counters.
    pub(crate) message: Option<&'a str>,
    /// Active search prompt (`/query`), drawn on its own line when the area
    /// is at least two rows high.
    pub(crate) search: Option<&'a str>,
    /// Theme.
    pub(crate) theme: &'a Theme,
    /// Colour level.
    pub(crate) level: ColorLevel,
    /// Terminal supports Unicode box drawing (ASCII fallback otherwise).
    pub(crate) unicode: bool,
}

impl StatusBar<'_> {
    /// The plain status text (without the search prompt).
    ///
    /// The line reads `name  FORMAT  message … path   42%  12/380`. When the
    /// terminal is too narrow for all of it, the parts go in the order §24.3
    /// gives: the path is dropped first, and only then is the whole line
    /// truncated with an ellipsis — filename, format and progress are what a
    /// reader orients by.
    pub(crate) fn text(&self, width: usize) -> String {
        // A breadcrumb needs room to say anything at all; below that the two
        // spaces of separation are worth more than a lone ellipsis.
        const MIN_PATH: usize = 8;
        let right = format!("{}%  {}/{}", self.percent, self.line, self.total);
        let mut left = format!("{}  {}", self.filename, self.format);
        let rw = unicode::width(&right);
        if let Some(m) = self.message.filter(|m| !m.is_empty()) {
            // §24.3 ranks the name, the format, the progress and the current
            // node above anything else; a message that would leave the path
            // no room gives way to it.
            let with = format!("{left}  {m}");
            let path_squeezed = self.path.is_some()
                && unicode::width(&with) + rw + MIN_PATH + 4 > width
                && unicode::width(&left) + rw + MIN_PATH + 4 <= width;
            if !path_squeezed {
                left = with;
            }
        }
        let lw = unicode::width(&left);
        let gap = width.saturating_sub(lw + rw);
        let crumb = self.path.and_then(|path| {
            let room = if gap >= MIN_PATH + 4 {
                gap - 4
            } else if self.path_always {
                // Orientation was asked to outrank progress, so the room the
                // counters wanted goes to the breadcrumb instead.
                width.saturating_sub(lw + 2)
            } else {
                return None;
            };
            Some(path.breadcrumb_within(room, self.unicode)).filter(|c| !c.is_empty())
        });

        if let Some(crumb) = crumb {
            let cw = unicode::width(&crumb);
            if lw + 2 + cw + 2 + rw <= width {
                let pad = width - lw - 2 - cw - rw;
                return format!("{left}  {crumb}{}{right}", " ".repeat(pad));
            }
            if self.path_always {
                return unicode::truncate_with_ellipsis(
                    &format!("{left}  {crumb}"),
                    width,
                    ellipsis(self.unicode),
                );
            }
        }

        if lw + rw + 2 > width {
            // Too narrow for everything. The progress counters were on this
            // line in 1.x and a reader orients by them, so they outlast the
            // format label, which goes first, and then the message, which
            // is shortened; only a line too narrow for the name and the
            // counters loses the counters too.
            // §24.3 ranks the format above a message, so the message is what
            // gets shortened or dropped first, and the label goes only when
            // the name and the counters alone fill the line.
            let room = width.saturating_sub(rw + 2);
            let named = format!("{}  {}", self.filename, self.format);
            let mut short = if unicode::width(&named) <= room {
                named
            } else {
                self.filename.to_string()
            };
            if let Some(m) = self.message.filter(|m| !m.is_empty()) {
                short = format!("{short}  {m}");
            }
            if unicode::width(self.filename) <= room {
                let short = unicode::truncate_with_ellipsis(&short, room, ellipsis(self.unicode));
                let pad = width - unicode::width(&short) - rw;
                return format!("{short}{}{right}", " ".repeat(pad));
            }
            return unicode::truncate_with_ellipsis(
                &format!("{left}  {right}"),
                width,
                ellipsis(self.unicode),
            );
        }
        format!("{left}{}{right}", " ".repeat(gap))
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let style = to_ratatui(self.theme.status_bar, self.level);
        let mut lines = Vec::new();
        if let Some(query) = self.search {
            lines.push(RLine::from(RSpan::styled(
                unicode::pad_to_width(query, area.width as usize),
                to_ratatui(self.theme.text, self.level),
            )));
        }
        lines.push(RLine::from(RSpan::styled(
            self.text(area.width as usize),
            style,
        )));
        Paragraph::new(lines).render(area, buf);
    }
}

/// The ASCII fallback border set (ratatui only ships Unicode ones).
const ASCII_BORDER: border::Set<'static> = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// The border glyph set for the terminal's Unicode capability.
///
/// ratatui's default set is Unicode box drawing; a terminal that cannot show
/// it gets [`ASCII_BORDER`], so the chrome degrades exactly like the document
/// body does.
fn border_set(unicode: bool) -> border::Set<'static> {
    if unicode {
        border::PLAIN
    } else {
        ASCII_BORDER
    }
}

/// The truncation marker for the terminal's Unicode capability.
fn ellipsis(unicode: bool) -> &'static str {
    if unicode {
        "…"
    } else {
        "..."
    }
}

/// Compute the tree connector prefix for every entry.
///
/// `unicode` picks the box-drawing glyphs (`├ `, `└ `, `│ `) or their ASCII
/// fallbacks (`+ `, `` ` ``, `| `).
pub(crate) fn toc_connectors(entries: &[OutlineEntry], unicode: bool) -> Vec<String> {
    let (vertical, last, branch) = if unicode {
        ("│ ", "└ ", "├ ")
    } else {
        ("| ", "` ", "+ ")
    };
    let depths: Vec<usize> = entries.iter().map(|e| e.depth).collect();
    let mut out = Vec::with_capacity(entries.len());
    for (i, d) in depths.iter().copied().enumerate() {
        let mut prefix = String::new();
        for level in 1..d {
            let mut more = false;
            for dj in depths.iter().skip(i + 1) {
                if *dj < level {
                    break;
                }
                if *dj == level {
                    more = true;
                    break;
                }
            }
            prefix.push_str(if more { vertical } else { "  " });
        }
        if d > 0 {
            let mut is_last = true;
            for dj in depths.iter().skip(i + 1) {
                if *dj < d {
                    break;
                }
                if *dj == d {
                    is_last = false;
                    break;
                }
            }
            prefix.push_str(if is_last { last } else { branch });
        }
        out.push(prefix);
    }
    out
}

/// TOC sidebar widget with its own scroll offset.
pub(crate) struct TocSidebar<'a> {
    /// Entries in document order.
    pub(crate) entries: &'a [OutlineEntry],
    /// Index of the highlighted (selected) entry.
    pub(crate) selected: Option<usize>,
    /// Index of the entry containing the viewport (current section).
    pub(crate) current: Option<usize>,
    /// First visible entry.
    pub(crate) scroll: usize,
    /// First visible column, for entries wider than the sidebar.
    pub(crate) h_scroll: usize,
    /// Theme.
    pub(crate) theme: &'a Theme,
    /// Colour level.
    pub(crate) level: ColorLevel,
    /// Terminal supports Unicode box drawing (ASCII fallback otherwise).
    pub(crate) unicode: bool,
}

impl TocSidebar<'_> {
    /// Rendered entry texts (without styles), used by tests.
    pub(crate) fn texts(&self, width: usize) -> Vec<String> {
        let connectors = toc_connectors(self.entries, self.unicode);
        let marker_current = if self.unicode { "▸" } else { ">" };
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let marker = if self.current == Some(i) {
                    marker_current
                } else {
                    " "
                };
                let prefix = connectors.get(i).cloned().unwrap_or_default();
                // Replaced rather than left to the backend, which drops
                // controls silently: the outline shows `U+FFFD` where the
                // document does (`util::text`).
                let row = format!("{marker}{prefix}{}", crate::util::text::sanitized(&e.text));
                let width = width.max(1);
                // One column past the right edge, so a row that still has
                // content out there is the one that gets the ellipsis; the
                // rest of it is reachable by scrolling sideways.
                let visible = unicode::slice_columns(&row, self.h_scroll, width + 1);
                unicode::truncate_with_ellipsis(&visible, width, ellipsis(self.unicode))
            })
            .collect()
    }
}

impl Widget for TocSidebar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_set(border_set(self.unicode))
            .border_style(to_ratatui(self.theme.table_border, self.level));
        let inner = block.inner(area);
        block.render(area, buf);
        let width = inner.width as usize;
        let texts = self.texts(width);
        let mut lines = Vec::new();
        for (i, text) in texts
            .into_iter()
            .enumerate()
            .skip(self.scroll)
            .take(inner.height as usize)
        {
            let style = if self.selected == Some(i) {
                self.theme.toc_selected
            } else {
                self.theme.toc
            };
            lines.push(RLine::from(RSpan::styled(
                unicode::pad_to_width(&text, width),
                to_ratatui(style, self.level),
            )));
        }
        Paragraph::new(lines).render(inner, buf);
    }
}

/// Widget drawing the tab bar: one label per open document.
///
/// Only drawn when a second document is open — with one tab there is nothing
/// to switch to and the row is better spent on the document.
pub(crate) struct TabBar<'a> {
    /// The label of each tab, in order (usually the file name).
    pub(crate) labels: &'a [String],
    /// Index of the tab being shown.
    pub(crate) active: usize,
    /// Theme.
    pub(crate) theme: &'a Theme,
    /// Colour level.
    pub(crate) level: ColorLevel,
}

impl TabBar<'_> {
    /// The text of each tab, in order, including its number and padding.
    ///
    /// The number is part of the label because it is also the key that
    /// selects the tab (`Alt-1` … `Alt-9`); past nine there is no key, so
    /// there is no number either.
    pub(crate) fn labels(&self) -> Vec<String> {
        self.labels
            .iter()
            .enumerate()
            .map(|(i, label)| match i {
                0..=8 => format!(" {} {label} ", i + 1),
                _ => format!(" {label} "),
            })
            .collect()
    }

    /// Where each tab sits in a bar `width` columns wide, as
    /// `(start column, width)`.
    ///
    /// Tabs that no longer fit get no span at all, so a click past the last
    /// visible one selects nothing rather than the wrong document.
    pub(crate) fn spans(&self, width: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut column = 0usize;
        for label in self.labels() {
            let w = unicode::width(&label);
            if column + w > width {
                break;
            }
            out.push((column, w));
            column += w;
        }
        out
    }
}

impl Widget for TabBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let width = usize::from(area.width);
        let labels = self.labels();
        let spans = self.spans(width);
        let mut rendered: Vec<RSpan<'static>> = Vec::new();
        for (i, (_, _)) in spans.iter().enumerate() {
            let style = if i == self.active {
                self.theme.toc_selected
            } else {
                self.theme.status_bar
            };
            rendered.push(RSpan::styled(
                labels[i].clone(),
                to_ratatui(style, self.level),
            ));
        }
        let used: usize = spans.iter().map(|(_, w)| *w).sum();
        if used < width {
            // The tabs that did not fit are worth saying so rather than
            // silently dropping.
            let hidden = labels.len() - spans.len();
            let rest = if hidden > 0 {
                let marker = format!(" +{hidden}");
                unicode::pad_to_width(&marker, width - used)
            } else {
                " ".repeat(width - used)
            };
            rendered.push(RSpan::styled(
                rest,
                to_ratatui(self.theme.status_bar, self.level),
            ));
        }
        Paragraph::new(RLine::from(rendered)).render(area, buf);
    }
}

/// Widget drawing the one-column rule between two side-by-side panes.
pub(crate) struct PaneDivider<'a> {
    /// Theme.
    pub(crate) theme: &'a Theme,
    /// Colour level.
    pub(crate) level: ColorLevel,
    /// Terminal supports Unicode box drawing (ASCII fallback otherwise).
    pub(crate) unicode: bool,
}

impl Widget for PaneDivider<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let glyph = if self.unicode { "│" } else { "|" };
        let style = to_ratatui(self.theme.table_border, self.level);
        let lines: Vec<RLine<'static>> = (0..area.height)
            .map(|_| RLine::from(RSpan::styled(glyph.to_string(), style)))
            .collect();
        Paragraph::new(lines).render(area, buf);
    }
}

/// One row of the key hints sidebar: a key label and what it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HintRow {
    /// Key label, taken verbatim from the live key map.
    pub(crate) keys: String,
    /// What the key does in the current context.
    pub(crate) label: String,
    /// The action the row stands for (used by the mouse handler).
    pub(crate) action: Action,
}

/// A labelled group of [`HintRow`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HintGroup {
    /// Group heading, e.g. `Move`.
    pub(crate) title: &'static str,
    /// Rows in display order.
    pub(crate) rows: Vec<HintRow>,
    /// Drop priority: the group with the *highest* value is dropped first
    /// when the groups do not fit the terminal height.
    pub(crate) priority: u8,
}

impl HintGroup {
    /// Rows plus the title row.
    pub(crate) fn height(&self) -> usize {
        self.rows.len() + 1
    }
}

/// What one rendered sidebar row stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HintLine {
    /// A group title.
    Title,
    /// Blank separator between groups.
    Blank,
    /// A key row for this action.
    Row(Action),
}

/// Fit `groups` into `height` rows, returning the groups kept and whether the
/// blank separator between them survived.
///
/// Degradation happens in two steps, cheapest first:
///
/// 1. groups are dropped least-important-first (highest `priority`, and the
///    later of two equals) until the spaced layout fits;
/// 2. if that cost a group, the unspaced layout is tried as well — losing the
///    blank rows is a much smaller loss than losing a whole group, so whenever
///    dropping the separators keeps strictly more groups, it wins.
///
/// The last group is never dropped; if it alone is still too tall the caller
/// truncates its rows. Nothing here can panic, `height == 0` included.
pub(crate) fn fit_hint_groups(groups: Vec<HintGroup>, height: usize) -> (Vec<HintGroup>, bool) {
    fn shrink(mut groups: Vec<HintGroup>, height: usize, spaced: bool) -> Vec<HintGroup> {
        let total = |groups: &[HintGroup]| -> usize {
            let rows: usize = groups.iter().map(HintGroup::height).sum();
            if spaced {
                rows + groups.len().saturating_sub(1)
            } else {
                rows
            }
        };
        while groups.len() > 1 && total(&groups) > height {
            let Some(victim) = groups
                .iter()
                .enumerate()
                .max_by_key(|(i, g)| (g.priority, *i))
                .map(|(i, _)| i)
            else {
                break;
            };
            groups.remove(victim);
        }
        groups
    }

    let spaced = shrink(groups.clone(), height, true);
    if spaced.len() == groups.len() {
        return (spaced, true);
    }
    let tight = shrink(groups, height, false);
    if tight.len() > spaced.len() {
        (tight, false)
    } else {
        (spaced, true)
    }
}

/// Key hints sidebar: the commands available right now, grouped and labelled.
///
/// The mirror image of [`TocSidebar`] on the right-hand edge. It never
/// scrolls: [`fit_hint_groups`] drops the least important groups until the
/// remainder fits, and the last group is row-truncated if it still does not.
pub(crate) struct KeyHintsSidebar<'a> {
    /// Groups in display order.
    pub(crate) groups: &'a [HintGroup],
    /// Theme.
    pub(crate) theme: &'a Theme,
    /// Colour level.
    pub(crate) level: ColorLevel,
    /// Terminal supports Unicode box drawing (ASCII fallback otherwise).
    pub(crate) unicode: bool,
}

impl KeyHintsSidebar<'_> {
    /// The rows this sidebar draws, as `(text, kind)`, already fitted to
    /// `width` and `height`.
    pub(crate) fn rows(&self, width: usize, height: usize) -> Vec<(String, HintLine)> {
        let width = width.max(1);
        let (groups, spaced) = fit_hint_groups(self.groups.to_vec(), height);
        let keyw = groups
            .iter()
            .flat_map(|g| g.rows.iter())
            .map(|r| unicode::width(&r.keys))
            .max()
            .unwrap_or(0)
            .min(width.saturating_sub(2).max(1));
        let mut out: Vec<(String, HintLine)> = Vec::new();
        for (i, group) in groups.iter().enumerate() {
            if i > 0 && spaced {
                out.push((String::new(), HintLine::Blank));
            }
            out.push((group.title.to_string(), HintLine::Title));
            for row in &group.rows {
                let text = format!("{} {}", unicode::pad_to_width(&row.keys, keyw), row.label);
                out.push((
                    unicode::truncate_with_ellipsis(&text, width, ellipsis(self.unicode)),
                    HintLine::Row(row.action),
                ));
            }
        }
        out.truncate(height);
        out
    }

    /// The plain texts, for tests.
    #[cfg(test)]
    pub(crate) fn texts(&self, width: usize, height: usize) -> Vec<String> {
        self.rows(width, height)
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }
}

impl Widget for KeyHintsSidebar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let block = Block::default()
            .borders(Borders::LEFT)
            .border_set(border_set(self.unicode))
            .border_style(to_ratatui(self.theme.table_border, self.level));
        let inner = block.inner(area);
        block.render(area, buf);
        let width = inner.width as usize;
        let title_style = to_ratatui(self.theme.table_header, self.level);
        let row_style = to_ratatui(self.theme.toc, self.level);
        let lines: Vec<RLine> = self
            .rows(width, inner.height as usize)
            .into_iter()
            .map(|(text, kind)| {
                let style = match kind {
                    HintLine::Title => title_style,
                    _ => row_style,
                };
                RLine::from(RSpan::styled(unicode::pad_to_width(&text, width), style))
            })
            .collect();
        Paragraph::new(lines).render(inner, buf);
    }
}

/// Help overlay listing key bindings.
pub(crate) struct HelpOverlay<'a> {
    /// `(keys, description)` pairs.
    pub(crate) entries: &'a [(String, String)],
    /// Theme.
    pub(crate) theme: &'a Theme,
    /// Colour level.
    pub(crate) level: ColorLevel,
    /// Terminal supports Unicode box drawing (ASCII fallback otherwise).
    pub(crate) unicode: bool,
}

impl HelpOverlay<'_> {
    /// Rendered help lines (without styles).
    pub(crate) fn texts(&self) -> Vec<String> {
        let keyw = self
            .entries
            .iter()
            .map(|(k, _)| unicode::width(k))
            .max()
            .unwrap_or(0);
        self.entries
            .iter()
            .map(|(k, d)| format!("{}  {}", unicode::pad_to_width(k, keyw), d))
            .collect()
    }
}

impl Widget for HelpOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        Clear.render(area, buf);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border_set(self.unicode))
            .title(" Help ")
            .border_style(to_ratatui(self.theme.table_border, self.level));
        let inner = block.inner(area);
        block.render(area, buf);
        let style = to_ratatui(self.theme.text, self.level);
        let lines: Vec<RLine> = self
            .texts()
            .into_iter()
            .take(inner.height as usize)
            .map(|t| {
                RLine::from(RSpan::styled(
                    unicode::truncate_to_width(&t, inner.width as usize).to_string(),
                    style,
                ))
            })
            .collect();
        Paragraph::new(lines).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::PathSegment;

    fn tree(src: &str, width: usize) -> (RenderTree, Theme) {
        (crate::testing::render(src, width), crate::testing::theme())
    }

    // ---------------------------------------------------------------
    // Widget builders.
    //
    // Every widget below is built through one of these, so a new field on a
    // widget is a one-line change here instead of a compile error in every
    // test that happens to draw it. Each returns a plausible default the
    // test then adjusts.
    // ---------------------------------------------------------------

    fn document_view<'a>(tree: &'a RenderTree, theme: &'a Theme) -> DocumentView<'a> {
        DocumentView {
            tree,
            top_line: 0,
            h_offset: 0,
            selected_link: None,
            current_match: None,
            cursor_line: None,
            theme,
            level: ColorLevel::TrueColor,
        }
    }

    fn status_bar<'a>(theme: &'a Theme) -> StatusBar<'a> {
        StatusBar {
            filename: "README.md",
            format: "Markdown",
            path: None,
            path_always: false,
            percent: 0,
            line: 1,
            total: 1,
            message: None,
            search: None,
            theme,
            level: ColorLevel::TrueColor,
            unicode: true,
        }
    }

    fn toc_entries(items: &[(usize, &str)]) -> Vec<OutlineEntry> {
        items
            .iter()
            .enumerate()
            .map(|(index, (depth, text))| OutlineEntry {
                node: index,
                fold: Some(index),
                depth: *depth,
                text: (*text).to_string(),
            })
            .collect()
    }

    fn toc_sidebar<'a>(entries: &'a [OutlineEntry], theme: &'a Theme) -> TocSidebar<'a> {
        TocSidebar {
            entries,
            selected: None,
            current: None,
            scroll: 0,
            h_scroll: 0,
            theme,
            level: ColorLevel::TrueColor,
            unicode: true,
        }
    }

    fn key_hints<'a>(groups: &'a [HintGroup], theme: &'a Theme) -> KeyHintsSidebar<'a> {
        KeyHintsSidebar {
            groups,
            theme,
            level: ColorLevel::TrueColor,
            unicode: true,
        }
    }

    fn help_overlay<'a>(entries: &'a [(String, String)], theme: &'a Theme) -> HelpOverlay<'a> {
        HelpOverlay {
            entries,
            theme,
            level: ColorLevel::TrueColor,
            unicode: true,
        }
    }

    fn hint_group(title: &'static str, priority: u8, n: usize) -> HintGroup {
        HintGroup {
            title,
            priority,
            rows: (0..n)
                .map(|i| HintRow {
                    keys: format!("k{i}"),
                    label: format!("do {i}"),
                    action: Action::Help,
                })
                .collect(),
        }
    }

    #[test]
    fn tab_labels_carry_their_number_and_a_click_lands_on_the_right_one() {
        let theme = Theme::dark();
        let labels = vec!["README.md".to_string(), "CHANGELOG.md".to_string()];
        let bar = TabBar {
            labels: &labels,
            active: 0,
            theme: &theme,
            level: ColorLevel::None,
        };
        assert_eq!(bar.labels(), vec![" 1 README.md ", " 2 CHANGELOG.md "]);

        let spans = bar.spans(80);
        assert_eq!(spans[0], (0, 13));
        assert_eq!(spans[1], (13, 16));
        // A bar with room for one label only offers one: a click past it must
        // select nothing rather than the tab that is not drawn.
        assert_eq!(bar.spans(14).len(), 1);
        assert!(bar.spans(5).is_empty());
    }

    #[test]
    fn style_conversion_respects_color_level() {
        let s = Style::new().fg(Color::Rgb(0x82, 0xaa, 0xff)).bold();
        let full = to_ratatui(s, ColorLevel::TrueColor);
        assert_eq!(full.fg, Some(RColor::Rgb(0x82, 0xaa, 0xff)));
        let c256 = to_ratatui(s, ColorLevel::Ansi256);
        assert!(matches!(c256.fg, Some(RColor::Indexed(_))));
        let mono = to_ratatui(s, ColorLevel::None);
        assert_eq!(mono.fg, None);
        assert!(mono.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn document_view_windows_and_scrolls() {
        let (tree, theme) = tree("# Title\n\nalpha beta gamma\n", 40);
        let view = document_view(&tree, &theme);
        let lines = view.lines(40, 2);
        assert_eq!(lines.len(), 2);
        let first: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(first, "Title");

        let view = DocumentView {
            top_line: 3,
            h_offset: 2,
            level: ColorLevel::None,
            ..document_view(&tree, &theme)
        };
        let lines = view.lines(40, 5);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "pha beta gamma");
    }

    #[test]
    fn selected_link_is_highlighted() {
        let (tree, theme) = tree("see [docs](https://example.com) now\n", 40);
        let view = DocumentView {
            selected_link: Some(0),
            ..document_view(&tree, &theme)
        };
        let lines = view.lines(40, 1);
        let link = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("docs"))
            .unwrap();
        assert_eq!(
            link.style.bg,
            to_ratatui(theme.link_selected, ColorLevel::TrueColor).bg
        );
    }

    #[test]
    fn status_bar_layout() {
        let theme = Theme::dark();
        let mut bar = status_bar(&theme);
        bar.percent = 37;
        bar.line = 142;
        bar.total = 380;
        let text = bar.text(40);
        assert!(text.starts_with("README.md"));
        assert!(text.ends_with("37%  142/380"));
        assert_eq!(unicode::width(&text), 40);
        // Narrow terminals truncate instead of panicking.
        assert!(unicode::width(&bar.text(10)) <= 10);
    }

    /// §24.3: on a narrow line the current node outranks a message.
    #[test]
    fn a_narrow_status_line_keeps_the_path_over_a_message() {
        let theme = Theme::dark();
        let path = DocumentPath::new(vec![
            PathSegment::Key("spec".into()),
            PathSegment::Key("replicas".into()),
        ]);
        let mut bar = status_bar(&theme);
        bar.filename = "k8s.yaml";
        bar.format = "YAML";
        bar.path = Some(&path);
        bar.message = Some("match 3 of 12 for replicas");
        let text = bar.text(50);
        assert!(text.contains("replicas  "), "{text:?}");
        assert!(!text.contains("match 3"), "{text:?}");
        assert!(bar.text(100).contains("match 3 of 12"), "room for both");
    }

    /// With a message showing on a tight line, the counters 1.x always
    /// showed outlast the format label and the end of the message.
    #[test]
    fn a_tight_status_line_keeps_its_counters() {
        let theme = Theme::dark();
        let mut bar = status_bar(&theme);
        bar.filename = "docs/terminal-compatibility-checklist.md";
        bar.percent = 2;
        bar.line = 11;
        bar.total = 388;
        bar.message = Some("no further heading at this level");
        let text = bar.text(80);
        assert!(text.ends_with("2%  11/388"), "{text:?}");
        assert!(text.starts_with("docs/terminal-compatibility-checklist.md  Markdown  no"));
        assert_eq!(unicode::width(&text), 80);
    }

    /// A reader who piped something in has to be able to see what diple
    /// decided it was (spec §6.6).
    #[test]
    fn the_status_line_names_the_format_it_read() {
        let theme = Theme::dark();
        let mut bar = status_bar(&theme);
        bar.format = "YAML";
        assert!(bar.text(60).contains("YAML"));
        bar.format = "JSON";
        assert!(bar.text(60).contains("JSON"));
    }

    #[test]
    fn the_breadcrumb_appears_when_there_is_room_and_goes_when_there_is_not() {
        let theme = Theme::dark();
        let path = DocumentPath::new(vec![
            PathSegment::Key("spec".into()),
            PathSegment::Key("containers".into()),
            PathSegment::Index(0),
            PathSegment::Key("image".into()),
        ]);
        let mut bar = status_bar(&theme);
        bar.format = "YAML";
        bar.path = Some(&path);
        bar.percent = 42;
        bar.line = 12;
        bar.total = 380;
        assert!(
            bar.text(90).contains("spec \u{203a} containers"),
            "a Unicode terminal gets the Unicode separator"
        );
        bar.unicode = false;

        let wide = bar.text(90);
        assert!(wide.contains("spec > containers > [0] > image"), "{wide}");
        assert!(wide.ends_with("42%  12/380"), "{wide}");
        assert_eq!(unicode::width(&wide), 90);

        // Narrower: the breadcrumb keeps the node the reader selected and
        // drops ancestors from the left (§24.3).
        let middling = bar.text(52);
        assert!(middling.contains("image"), "the leaf survives: {middling}");
        assert!(
            !middling.contains("spec > containers"),
            "ancestors went first: {middling}"
        );
        assert!(middling.ends_with("42%  12/380"), "{middling}");

        // Narrower still: the path goes entirely, progress stays.
        let narrow = bar.text(34);
        assert!(narrow.ends_with("42%  12/380"), "{narrow}");
        assert!(!narrow.contains("image"), "{narrow}");
        assert!(unicode::width(&narrow) <= 34);
    }

    /// `structured.path = "always"` reverses the priority: orientation is
    /// worth more to this reader than the progress counters.
    #[test]
    fn path_always_keeps_the_breadcrumb_instead_of_the_counters() {
        let theme = Theme::dark();
        let path = DocumentPath::new(vec![
            PathSegment::Key("spec".into()),
            PathSegment::Key("image".into()),
        ]);
        let mut bar = status_bar(&theme);
        bar.format = "YAML";
        bar.path = Some(&path);
        bar.percent = 42;
        bar.line = 12;
        bar.total = 380;
        bar.unicode = false;

        let auto = bar.text(30);
        assert!(auto.contains("12/380"), "auto keeps the counters: {auto}");

        bar.path_always = true;
        let always = bar.text(30);
        assert!(always.contains("image"), "{always}");
        assert!(!always.contains("12/380"), "{always}");
        assert!(unicode::width(&always) <= 30);
    }

    /// The cursor row is painted at draw time, so it must colour the whole
    /// row and must never change how many rows there are.
    #[test]
    fn the_cursor_row_is_highlighted_across_the_full_width() {
        let (tree, theme) = tree("# Title\n\nalpha beta\n", 40);
        let plain = document_view(&tree, &theme);
        let marked = DocumentView {
            cursor_line: Some(0),
            ..document_view(&tree, &theme)
        };
        let a = plain.lines(40, 3);
        let b = marked.lines(40, 3);
        assert_eq!(a.len(), b.len(), "highlighting is not a layout input");

        let text =
            |l: &RLine<'_>| -> String { l.spans.iter().map(|s| s.content.to_string()).collect() };
        assert_eq!(text(&a[1]), text(&b[1]), "an unmarked row is untouched");
        assert!(
            text(&b[0]).starts_with("Title"),
            "the row still says what it said"
        );
        assert_eq!(
            unicode::width(&text(&b[0])),
            40,
            "the highlight runs to the right edge"
        );
        assert_ne!(
            a[0].spans[0].style, b[0].spans[0].style,
            "the marked row is styled differently"
        );
    }

    #[test]
    fn toc_tree_connectors() {
        let theme = Theme::dark();
        let entries = toc_entries(&[
            (0, "Project"),
            (1, "Install"),
            (1, "Config"),
            (2, "Basic"),
            (2, "Advanced"),
            (1, "License"),
        ]);
        let mut toc = toc_sidebar(&entries, &theme);
        toc.selected = Some(1);
        toc.current = Some(3);
        let texts = toc.texts(30);
        assert_eq!(texts[0], " Project");
        assert_eq!(texts[1], " ├ Install");
        assert_eq!(texts[2], " ├ Config");
        assert_eq!(texts[3], "▸│ ├ Basic");
        assert_eq!(texts[4], " │ └ Advanced");
        assert_eq!(texts[5], " └ License");
    }

    /// A heading too long for the sidebar is reached by scrolling sideways,
    /// not by widening the sidebar. The ellipsis marks the edge it is scrolled
    /// towards, and disappears once the end of the text is on screen.
    #[test]
    fn an_outline_entry_shows_what_it_cannot_print() {
        let theme = Theme::dark();
        let entries = toc_entries(&[(0, "red\u{1b}[31m\u{202e}x")]);
        let toc = toc_sidebar(&entries, &theme);
        assert_eq!(toc.texts(40)[0], " red\u{fffd}[31m\u{fffd}x");
    }

    #[test]
    fn toc_scrolls_sideways_past_the_sidebar_width() {
        let theme = Theme::dark();
        let entries = toc_entries(&[(0, "Project"), (1, "Configuration reference")]);
        let mut toc = toc_sidebar(&entries, &theme);

        let texts = toc.texts(12);
        assert_eq!(texts[1], " └ Configur…", "truncated at the right edge");

        toc.h_scroll = 6;
        let texts = toc.texts(12);
        assert_eq!(texts[0], "ct", "short rows simply run out");
        assert_eq!(texts[1], "figuration …");

        toc.h_scroll = 14;
        let texts = toc.texts(12);
        assert_eq!(texts[1], "on reference", "no ellipsis once the end shows");
    }

    /// On a terminal without Unicode box drawing every glyph the chrome emits
    /// must be ASCII — the sidebars, the help overlay border and the TOC tree
    /// included. Regression: the widget layer used to ignore the capability
    /// entirely and rendered replacement characters.
    #[test]
    fn chrome_falls_back_to_ascii_without_unicode_box() {
        let theme = Theme::dark();
        let entries = toc_entries(&[
            (0, "Project"),
            (1, "Install"),
            (2, "A very long heading that must be truncated"),
            (2, "Advanced"),
            (1, "License"),
        ]);
        let mut toc = toc_sidebar(&entries, &theme);
        toc.selected = Some(1);
        toc.current = Some(2);
        toc.level = ColorLevel::None;
        toc.unicode = false;
        let texts = toc.texts(16);
        assert_eq!(texts[0], " Project");
        assert_eq!(texts[1], " + Install");
        assert_eq!(texts[2], ">| + A very l...");
        assert_eq!(texts[3], " | ` Advanced");
        assert_eq!(texts[4], " ` License");
        assert!(texts.iter().all(|t| t.is_ascii()));

        let groups = vec![hint_group("Move", 0, 2)];
        let mut hints = key_hints(&groups, &theme);
        hints.level = ColorLevel::None;
        hints.unicode = false;
        assert!(hints.texts(6, 10).iter().all(|t| t.is_ascii()));

        let mut bar = status_bar(&theme);
        bar.filename = "readme.md";
        bar.percent = 37;
        bar.line = 142;
        bar.total = 380;
        bar.level = ColorLevel::None;
        bar.unicode = false;
        assert!(bar.text(12).is_ascii(), "{:?}", bar.text(12));

        // And the rendered buffers, borders included.
        let area = Rect::new(0, 0, 16, 5);
        assert!(ascii_only(toc, area));
        assert!(ascii_only(hints, area));
        let help = vec![("j".to_string(), "down".to_string())];
        let mut overlay = help_overlay(&help, &theme);
        overlay.level = ColorLevel::None;
        overlay.unicode = false;
        assert!(ascii_only(overlay, area));

        // The Unicode path is unchanged and still draws box glyphs.
        let mut buf = Buffer::empty(area);
        let mut overlay = help_overlay(&help, &theme);
        overlay.level = ColorLevel::None;
        overlay.render(area, &mut buf);
        assert!(!buffer_text(&buf, area).is_ascii());
    }

    fn buffer_text(buf: &Buffer, area: Rect) -> String {
        (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .map(|p| buf[p].symbol().to_string())
            .collect()
    }

    fn ascii_only(widget: impl Widget, area: Rect) -> bool {
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        let text = buffer_text(&buf, area);
        assert!(text.is_ascii(), "non-ASCII chrome: {text:?}");
        true
    }

    #[test]
    fn help_overlay_aligns_keys() {
        let theme = Theme::dark();
        let entries = vec![
            ("j / ↓".to_string(), "scroll down".to_string()),
            ("q".to_string(), "quit".to_string()),
        ];
        let texts = help_overlay(&entries, &theme).texts();
        assert_eq!(texts[0], "j / ↓  scroll down");
        assert_eq!(texts[1], "q      quit");
    }

    #[test]
    fn key_hints_align_keys_and_label_their_groups() {
        let theme = Theme::dark();
        let groups = vec![HintGroup {
            title: "Move",
            priority: 0,
            rows: vec![
                HintRow {
                    keys: "j/down".into(),
                    label: "down".into(),
                    action: Action::ScrollDown,
                },
                HintRow {
                    keys: "g".into(),
                    label: "top".into(),
                    action: Action::Top,
                },
            ],
        }];
        let hints = key_hints(&groups, &theme);
        assert_eq!(
            hints.texts(24, 10),
            vec!["Move", "j/down down", "g      top"]
        );
        // A narrow sidebar truncates rather than overflowing.
        assert!(hints.texts(8, 10).iter().all(|t| unicode::width(t) <= 8));
    }

    #[test]
    fn key_hints_drop_the_least_important_groups_first() {
        // Priorities as the selector assigns them: Move 0, View 1,
        // Headings 2, Fold 3.
        let theme = Theme::dark();
        let groups = vec![
            hint_group("Move", 0, 3),
            hint_group("Headings", 2, 3),
            hint_group("Fold", 3, 3),
            hint_group("View", 1, 3),
        ];
        // 4 groups = 16 rows, + 3 blanks = 19 spaced.
        let mut all = key_hints(&groups, &theme);
        all.level = ColorLevel::None;
        assert_eq!(all.texts(20, 40).len(), 19);

        // 16 rows: the separators go before any group does.
        let (kept, spaced) = fit_hint_groups(groups.clone(), 16);
        assert_eq!(
            kept.iter().map(|g| g.title).collect::<Vec<_>>(),
            vec!["Move", "Headings", "Fold", "View"]
        );
        assert!(!spaced, "the blank rows were given up, not a group");
        assert_eq!(all.texts(20, 16).len(), 16);

        // Then whole groups, least important first.
        let kept: Vec<&str> = fit_hint_groups(groups.clone(), 12)
            .0
            .iter()
            .map(|g| g.title)
            .collect();
        assert_eq!(kept, vec!["Move", "Headings", "View"], "Fold went first");
        let kept: Vec<&str> = fit_hint_groups(groups.clone(), 8)
            .0
            .iter()
            .map(|g| g.title)
            .collect();
        assert_eq!(kept, vec!["Move", "View"], "then Headings");
        let kept: Vec<&str> = fit_hint_groups(groups.clone(), 2)
            .0
            .iter()
            .map(|g| g.title)
            .collect();
        assert_eq!(kept, vec!["Move"], "the last group is never dropped");
        // A single group that still does not fit is row-truncated, not a panic.
        assert_eq!(all.texts(20, 2).len(), 2);
        assert_eq!(all.texts(20, 0).len(), 0);
    }

    #[test]
    fn widgets_render_into_a_buffer() {
        let (tree, theme) = tree("# Title\n\nbody text here\n", 20);
        let area = Rect::new(0, 0, 20, 6);
        let mut buf = Buffer::empty(area);
        DocumentView {
            level: ColorLevel::Ansi256,
            ..document_view(&tree, &theme)
        }
        .render(area, &mut buf);
        let row: String = (0..20)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect::<String>();
        assert_eq!(row.trim_end(), "Title");

        let mut bar = status_bar(&theme);
        bar.filename = "f.md";
        bar.percent = 100;
        bar.line = 3;
        bar.total = 3;
        bar.message = Some("saved");
        bar.search = Some("/needle");
        bar.level = ColorLevel::Ansi16;
        let area = Rect::new(0, 0, 20, 2);
        let mut buf = Buffer::empty(area);
        bar.render(area, &mut buf);
        let row: String = (0..20).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(row.starts_with("/needle"));
    }
}
