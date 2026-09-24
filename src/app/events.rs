//! The crossterm event loop and the draw pass.
//!
//! # Keyboard input when the document comes from stdin
//!
//! `cat README.md | diple` hands the document to stdin, so stdin cannot also
//! deliver key events. crossterm solves this itself: both its event source and
//! `enable_raw_mode` call `tty_fd()`, which uses `STDIN_FILENO` only when it
//! is a terminal and otherwise opens `/dev/tty` read-write. No `dup2` onto fd
//! 0 is needed, and none is performed — diple only verifies up front, via
//! [`crate::terminal::lifecycle::open_input_tty`], that a controlling terminal
//! exists at all, and falls back to non-interactive output when it does not.
//!
//! # Idle behaviour
//!
//! The loop blocks in `event::poll` with a timeout and only redraws after an
//! event, so an idle diple uses no CPU — with one bounded exception: while it
//! is idle it highlights the deferred code blocks in the screen above and
//! below the viewport (`App::realize_ahead`), which is what keeps the first
//! block of a new language from costing a frame when it is scrolled into view.
//! That work is finite: once the reader's surroundings are highlighted the
//! loop goes back to doing nothing at all.
//!
//! # Images
//!
//! ratatui cannot draw images. After the ratatui pass, `draw_images` moves
//! the cursor to the first cell of every fully visible image line and writes
//! the encoded protocol sequence directly. Anything that fails simply skips
//! the image; the reserved cells keep the placeholder text, so a failure can
//! never produce garbage on screen.

use std::io::{self, Write};
use std::time::Duration;

use crossterm::cursor::{MoveTo, RestorePosition, SavePosition};
use crossterm::event::{self};
use crossterm::queue;
use crossterm::style::Print;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::app::state::{App, HelpKind, Mode};
use crate::app::workspace::Workspace;
use crate::config::schema::PathMode;
use crate::render::primitives::LineKind;
use crate::render::terminal::{DocumentView, HelpOverlay, KeyHintsSidebar, StatusBar, TocSidebar};

/// How long the loop waits for an event before checking its own state again.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The screen areas of one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Areas {
    /// TOC sidebar (zero-width when closed).
    pub sidebar: Rect,
    /// Document area.
    pub content: Rect,
    /// Key hints sidebar on the right edge (zero-width when closed or when
    /// the terminal is too narrow for it).
    pub hints: Rect,
    /// Status bar (plus the search prompt row when searching).
    pub status: Rect,
}

/// Split a frame into TOC sidebar, content, hints sidebar and status areas.
///
/// The widths come from [`App::sidebar_widths`], the same function the layout
/// engine is fed from, so the document is drawn at exactly the width it was
/// laid out at.
pub(crate) fn areas(app: &App, area: Rect) -> Areas {
    let chrome = app.chrome_rows().min(area.height);
    let body_height = area.height.saturating_sub(chrome);
    let (sidebar_width, hints_width) = app.sidebar_widths();
    let sidebar_width = sidebar_width.min(area.width);
    let hints_width = hints_width.min(area.width.saturating_sub(sidebar_width));
    let available = area
        .width
        .saturating_sub(sidebar_width)
        .saturating_sub(hints_width);
    // The document may be narrower than the space between the sidebars —
    // `max_width` caps it and `center` splits the remainder into two margins.
    // The sidebars keep the screen edges either way, so a centred document
    // has them outside its text rather than beside it.
    let content_width = u16::try_from(app.content_width())
        .unwrap_or(u16::MAX)
        .min(available);
    let margin = u16::try_from(app.content_margin())
        .unwrap_or(0)
        .min(available - content_width);
    Areas {
        sidebar: Rect::new(area.x, area.y, sidebar_width, body_height),
        content: Rect::new(
            area.x + sidebar_width + margin,
            area.y,
            content_width,
            body_height,
        ),
        hints: Rect::new(
            area.x + sidebar_width + available,
            area.y,
            hints_width,
            body_height,
        ),
        status: Rect::new(area.x, area.y + body_height, area.width, chrome),
    }
}

/// A centred overlay rectangle covering `pct` percent of `area`.
fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    // `u16` arithmetic overflows for widths >= 820 at `pct_x = 80`, and
    // crossterm reports terminal sizes up to `u16::MAX`, so widen first.
    let scale = |extent: u16, pct: u16| {
        let scaled = u32::from(extent) * u32::from(pct) / 100;
        u16::try_from(scaled).unwrap_or(u16::MAX).max(1).min(extent)
    };
    let w = scale(area.width, pct_x);
    let h = scale(area.height, pct_y);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Draw one frame of a single-view session.
///
/// The whole terminal is the document's, which is what every test of the draw
/// pass wants; [`draw_in`] is the general form the workspace uses.
#[cfg(test)]
pub(crate) fn draw(app: &App, frame: &mut ratatui::Frame<'_>) {
    draw_in(app, frame, frame.area(), true);
}

/// Draw one view into `area`.
///
/// `focused` says whether this view has the keyboard. An unfocused pane draws
/// exactly the same document — only the marker in front of its file name
/// differs, so the reader can see where the next key will land.
pub(crate) fn draw_in(app: &App, frame: &mut ratatui::Frame<'_>, area: Rect, focused: bool) {
    let all = areas(app, area);

    // A theme may own the screen itself rather than borrowing the terminal's
    // background. Painted first, under everything, and a no-op for a theme
    // that sets no background — which is every theme but `crt`.
    if app.theme.screen.bg.is_some() {
        frame.render_widget(
            ratatui::widgets::Block::default().style(crate::render::terminal::to_ratatui(
                app.theme.screen,
                app.color,
            )),
            area,
        );
    }

    if all.sidebar.width > 0 {
        let current = app.cursor_node().and_then(|n| app.toc.index_covering(n));
        frame.render_widget(
            TocSidebar {
                entries: &app.toc.entries,
                selected: (app.mode() == Mode::Toc).then_some(app.toc.selected),
                current,
                scroll: app.toc.scroll,
                h_scroll: app.toc.h_scroll,
                theme: &app.theme,
                level: app.color,
                unicode: app.caps.unicode_box,
            },
            all.sidebar,
        );
    }

    if all.hints.width > 0 {
        let groups = app.hint_groups();
        frame.render_widget(
            KeyHintsSidebar {
                groups: &groups,
                theme: &app.theme,
                level: app.color,
                unicode: app.caps.unicode_box,
            },
            all.hints,
        );
    }

    frame.render_widget(
        DocumentView {
            tree: app.tree(),
            top_line: app.top_line(),
            h_offset: app.h_offset(),
            selected_link: app.selected_link(),
            current_match: app.search.current_match(),
            // Markdown's cursor is already spoken for by the heading it
            // scrolled to; structured data has no such landmark, so the row
            // says where you are.
            cursor_line: app.format().is_structured().then(|| app.cursor_line()),
            theme: &app.theme,
            level: app.color,
        },
        all.content,
    );

    // The two prompts share the status area's second row: only one of them
    // can have the keyboard, so only one can be drawn.
    let prompt = match app.mode() {
        Mode::Command => app.command.prompt(),
        _ => app.search.prompt(),
    };
    let message = status_message(app);
    // With a second pane on screen the status bar is also the focus
    // indicator: the marker says which document the next key belongs to.
    let name = if app.views().1 > 1 {
        let marker = if focused {
            if app.caps.unicode_box {
                "▸ "
            } else {
                "> "
            }
        } else {
            "  "
        };
        format!("{marker}{}", app.filename())
    } else {
        app.filename().to_string()
    };
    // Markdown has no key path, so this is `None` there and the status line
    // keeps the shape 1.x readers know.
    let path_mode = app.config.structured.path;
    let path = match path_mode {
        PathMode::Never => None,
        PathMode::Auto | PathMode::Always => app.cursor_path(),
    };
    frame.render_widget(
        StatusBar {
            filename: &name,
            format: app.format().label(),
            path: path.as_ref(),
            path_always: path_mode == PathMode::Always,
            percent: app.percent(),
            line: app.bottom_line(),
            total: app.tree().len(),
            message: message.as_deref(),
            search: (focused && matches!(app.mode(), Mode::Search | Mode::Command))
                .then_some(prompt.as_str()),
            theme: &app.theme,
            level: app.color,
            unicode: app.caps.unicode_box,
        },
        all.status,
    );

    if app.mode() == Mode::Help {
        let area = centered(area, 80, 80);
        let entries = match app.help_kind() {
            HelpKind::Settings => crate::config::settings::help_entries(),
            HelpKind::Keys => app.help_entries(),
        };
        let rows = usize::from(area.height).saturating_sub(2).max(1);
        let scroll = app.help_scroll().min(entries.len().saturating_sub(1));
        let window: Vec<(String, String)> = entries.into_iter().skip(scroll).take(rows).collect();
        frame.render_widget(
            HelpOverlay {
                entries: &window,
                theme: &app.theme,
                level: app.color,
                unicode: app.caps.unicode_box,
            },
            area,
        );
    }
}

/// The text shown after the file name in the status bar.
fn status_message(app: &App) -> Option<String> {
    if let Some(message) = app.message() {
        return Some(message.to_string());
    }
    if !app.pending().is_empty() {
        return Some(app.pending().to_string());
    }
    match app.mode() {
        Mode::Toc => Some("TOC: j/k move, Enter jump, Esc close".to_string()),
        Mode::Help => Some(match app.help_kind() {
            HelpKind::Settings => "settings: j/k scroll, Esc close".to_string(),
            HelpKind::Keys => "help: j/k scroll, Esc close".to_string(),
        }),
        // What Tab last offered, until the next keystroke replaces it.
        Mode::Command => Some(app.command.candidate_hint().unwrap_or_else(|| {
            "Tab completes; `:help` lists settings, `:open` adds a document".to_string()
        })),
        _ if app.search.has_matches() => Some(app.search.prompt()),
        _ => None,
    }
}

/// One image to be painted after the ratatui pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlacedImage {
    /// Registry id.
    pub id: usize,
    /// Absolute column of the first cell.
    pub x: u16,
    /// Absolute row of the first cell.
    pub y: u16,
    /// Width in columns.
    pub cols: u16,
    /// Height in rows.
    pub rows: u16,
}

/// Which diagram images are fully visible in the current viewport.
///
/// Partially scrolled images (vertically or horizontally) are skipped rather
/// than clipped, because no image protocol can be cropped reliably in every
/// terminal — the reserved cells then show the placeholder instead.
///
/// The "is this the top row of the image?" test is made against the previous
/// [`LineKind::Image`] line rather than against `node_offset`: the layout
/// engine attributes the blank separator line between two top-level blocks to
/// the *following* node, so the first reserved row of a diagram that is not
/// the document's very first node carries `node_offset == 1` and used to be
/// skipped. Moving that blank to the preceding node instead would
/// change the meaning of `node_offset` for every node in the document, and
/// `node_offset` is the viewport anchor coordinate — it drives resize and fold
/// restoration and every snapshot. Asking the local question directly is both
/// smaller and more precise.
pub(crate) fn placed_images(app: &App, content: Rect) -> Vec<PlacedImage> {
    let mut out = Vec::new();
    if !app.caps.images.is_some() || app.h_offset() > 0 || content.height == 0 {
        return out;
    }
    let height = usize::from(content.height);
    let top = app.top_line();
    for (row, line) in app.tree().visible_slice(top, height).iter().enumerate() {
        let LineKind::Image(image) = &line.kind else {
            continue;
        };
        // Only the first reserved row starts the image. When the row above is
        // part of the same image, the image begins above the viewport and is
        // skipped rather than clipped.
        let continues_above = top
            .checked_add(row)
            .and_then(|abs| abs.checked_sub(1))
            .and_then(|abs| app.tree().lines.get(abs))
            .is_some_and(|prev| matches!(&prev.kind, LineKind::Image(p) if p.id == image.id));
        if continues_above {
            continue;
        }
        let Some(registered) = app.diagrams.image(image.id) else {
            continue; // a plain Markdown image, not a rendered diagram
        };
        if row + usize::from(registered.rows) > height {
            continue; // would be clipped at the bottom
        }
        if usize::from(registered.cols) > usize::from(content.width) {
            continue; // would be clipped on the right
        }
        out.push(PlacedImage {
            id: image.id,
            x: content.x,
            y: content.y + row as u16,
            cols: registered.cols,
            rows: registered.rows,
        });
    }
    out
}

/// Paint the placed images by writing protocol escape sequences directly.
///
/// Errors are returned so the caller can decide to give up on images; the
/// document itself is never affected.
pub(crate) fn draw_images(
    app: &App,
    placed: &[PlacedImage],
    out: &mut impl Write,
) -> io::Result<()> {
    if placed.is_empty() {
        return Ok(());
    }
    queue!(out, SavePosition)?;
    for image in placed {
        let Some(registered) = app.diagrams.image(image.id) else {
            continue;
        };
        let Some(sequence) = app.caps.images.encode_for_tmux(
            &registered.source,
            image.cols,
            image.rows,
            app.caps.tmux_passthrough,
        ) else {
            continue; // encoding failed: keep the placeholder cells
        };
        queue!(out, MoveTo(image.x, image.y), Print(sequence))?;
    }
    queue!(out, RestorePosition)?;
    out.flush()
}

/// Remove previously drawn images before a frame that no longer contains them.
///
/// Kitty keeps images until they are deleted explicitly; the other protocols
/// are erased by repainting the cells, which ratatui does after the clear.
fn clear_images(caps: &crate::terminal::Capabilities, out: &mut impl Write) -> io::Result<()> {
    use crate::terminal::capabilities::ImageSupport;
    if caps.images == ImageSupport::Kitty {
        let sequence = crate::terminal::protocols::maybe_tmux(
            "\x1b_Ga=d\x1b\\".to_string(),
            caps.tmux_passthrough,
        );
        out.write_all(sequence.as_bytes())?;
        out.flush()?;
    }
    Ok(())
}

/// Emit OSC 8 hyperlinks over the visible link spans.
///
/// The text is rewritten identically (same cells, same width), only wrapped in
/// the hyperlink escape, so a terminal that ignores OSC 8 sees no difference.
fn draw_hyperlinks(app: &App, content: Rect, out: &mut impl Write) -> io::Result<()> {
    use crate::terminal::protocols::{osc8_end, osc8_start};
    use crate::util::viewport::slice_line;

    if !app.osc8_enabled() || content.width == 0 {
        return Ok(());
    }
    let width = usize::from(content.width);
    let mut wrote = false;
    for (row, line) in app
        .tree()
        .visible_slice(app.top_line(), usize::from(content.height))
        .iter()
        .enumerate()
    {
        if !line.spans.iter().any(|s| s.link.is_some()) {
            continue;
        }
        let mut column = 0usize;
        for span in slice_line(line, app.h_offset(), width) {
            let span_width = span.width();
            if let Some(id) = span.link {
                if let Some(link) = app.link(id) {
                    if !wrote {
                        queue!(out, SavePosition)?;
                        wrote = true;
                    }
                    let x = content
                        .x
                        .saturating_add(column.min(u16::MAX as usize) as u16);
                    queue!(
                        out,
                        MoveTo(x, content.y + row as u16),
                        // Written past the backend, which would otherwise
                        // be what drops a control character, so the text is
                        // made safe here as well as where it was laid out.
                        Print(format!(
                            "{}{}{}",
                            osc8_start(&link.url),
                            crate::util::text::sanitized(&span.text),
                            osc8_end()
                        ))
                    )?;
                }
            }
            column += span_width;
        }
    }
    if wrote {
        queue!(out, RestorePosition)?;
        out.flush()?;
    }
    Ok(())
}

/// Run the interactive event loop until the user quits.
///
/// The caller owns the [`crate::terminal::TerminalGuard`]; this function never
/// enters or leaves raw mode itself.
///
/// It drives a [`Workspace`] rather than a single [`App`], because what is on
/// screen may be one document or four: the loop asks the workspace for the
/// panes and their rectangles and then does per pane exactly what it used to
/// do for the one.
pub fn run(workspace: &mut Workspace) -> io::Result<()> {
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    // No initial `Terminal::clear()`: the alternate screen is already blank,
    // and `clear` costs a cursor-position query that some terminals (and
    // pty harnesses) never answer.
    if let Ok(size) = terminal.size() {
        workspace.resize(size.width, size.height);
    }

    let mut previous_images: Vec<PlacedImage> = Vec::new();
    // What the terminal was last told about mouse reporting. `App` only holds
    // the flag; the escape sequence is written here, where the terminal is
    // owned and no frame is half-drawn.
    let mut mouse_on = workspace.mouse_on();
    let mut redraw = true;
    loop {
        if workspace.mouse_on() != mouse_on {
            mouse_on = workspace.mouse_on();
            // Best effort: a terminal that refuses the sequence simply keeps
            // reporting (or not), and the flag still says what diple reads.
            let _ = crate::terminal::lifecycle::set_mouse_reporting(mouse_on);
        }
        if redraw {
            // Syntax highlighting for the code about to be drawn and the
            // search highlighting are both applied here, in the same frame
            // that draws them: the reader never sees a code block
            // repaint after the fact, and the tree keeps its line count, so
            // the viewport anchor cannot move.
            workspace.prepare_frame();
            // One flat list for the "did the images move?" test, and the
            // per-pane split for the painting itself: an image is encoded
            // from the registry of the view that owns it.
            let per_pane: Vec<Vec<PlacedImage>> = workspace
                .pane_contents()
                .into_iter()
                .map(|(app, content)| placed_images(app, content))
                .collect();
            let images: Vec<PlacedImage> = per_pane.iter().flatten().copied().collect();
            if !previous_images.is_empty() && previous_images != images {
                let mut out = io::stdout();
                let _ = clear_images(workspace.caps(), &mut out);
                // Best effort: a terminal that cannot report its cursor
                // position simply keeps the stale cells until they change.
                let _ = terminal.clear();
            }
            terminal.draw(|frame| workspace.draw(frame))?;
            let mut out = io::stdout();
            // Image and hyperlink painting is best effort: a failure must
            // never take the pager down.
            for ((app, content), placed) in workspace.pane_contents().into_iter().zip(&per_pane) {
                let _ = draw_images(app, placed, &mut out);
                let _ = draw_hyperlinks(app, content, &mut out);
            }
            previous_images = images;
            redraw = false;
        }

        if workspace.should_quit() {
            return Ok(());
        }

        if !event::poll(POLL_INTERVAL)? {
            // Idle: nothing is waiting for a frame, so this is the moment to
            // highlight the code just outside the viewport. It is bounded to a
            // screen in each direction, so the process settles back to using
            // no CPU once the reader's surroundings are highlighted.
            workspace.realize_ahead();
            workspace.reap_children();
            continue;
        }
        // Drain everything that is already queued before redrawing, so a
        // held-down key or a burst of mouse events cannot stall the loop.
        loop {
            workspace.handle_event(event::read()?);
            redraw = true;
            if workspace.should_quit() || !event::poll(Duration::from_millis(0))? {
                break;
            }
        }
        workspace.reap_children();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::diagrams::DiagramProvider;
    use crate::testing;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::testing::app_sized as app;

    #[test]
    fn centered_survives_very_large_areas() {
        // Regression: `area.width * pct_x` overflowed `u16` at width >= 820.
        let wide = centered(Rect::new(0, 0, 900, 40), 80, 80);
        assert_eq!((wide.width, wide.height), (720, 32));

        let tall = centered(Rect::new(0, 0, 80, 4000), 80, 80);
        assert_eq!((tall.width, tall.height), (64, 3200));

        // crossterm can report up to `u16::MAX` in either direction.
        let huge = centered(Rect::new(0, 0, u16::MAX, u16::MAX), 80, 80);
        assert_eq!((huge.width, huge.height), (52428, 52428));
        // Centred, and still inside the area (no wrap-around).
        assert_eq!(huge.x, (u16::MAX - 52428) / 2);

        // Degenerate areas keep their old behaviour.
        assert_eq!(centered(Rect::new(0, 0, 0, 0), 80, 80).width, 0);
        assert_eq!(centered(Rect::new(0, 0, 1, 1), 80, 80).width, 1);
    }

    #[test]
    fn areas_reserve_the_status_row() {
        let a = app("# T\n\ntext\n", (80, 24));
        let split = areas(&a, Rect::new(0, 0, 80, 24));
        assert_eq!(split.content.height, 23);
        assert_eq!(split.status.height, 1);
        assert_eq!(split.sidebar.width, 0);
        assert_eq!(split.content.width, 80);
    }

    #[test]
    fn areas_make_room_for_the_sidebar() {
        let mut a = app("# T\n\ntext\n", (80, 24));
        a.toc.open = true;
        let split = areas(&a, Rect::new(0, 0, 80, 24));
        assert!(split.sidebar.width > 0);
        assert_eq!(split.content.x, split.sidebar.width);
        assert_eq!(split.content.width + split.sidebar.width, 80);
    }

    #[test]
    fn a_centred_document_keeps_the_sidebars_on_the_screen_edges() {
        let cfg = crate::config::Config {
            max_width: 60,
            center: true,
            ..crate::config::Config::default()
        };
        let mut a = testing::AppBuilder::new("# T\n\ntext\n")
            .size((100, 24))
            .config(cfg)
            .build();

        // No sidebars: equal margins around the document.
        let split = areas(&a, Rect::new(0, 0, 100, 24));
        assert_eq!(split.content.width, 60);
        assert_eq!(split.content.x, 20);
        assert_eq!(100 - (split.content.x + split.content.width), 20);

        // With both sidebars open they still hug the edges, and the document
        // is centred in what they leave over.
        a.apply(crate::config::actions::Action::ToggleToc);
        a.apply(crate::config::actions::Action::ToggleKeyHints);
        let split = areas(&a, Rect::new(0, 0, 100, 24));
        assert!(split.sidebar.width > 0 && split.hints.width > 0);
        assert_eq!(split.sidebar.x, 0, "the TOC keeps the left edge");
        assert_eq!(
            split.hints.x + split.hints.width,
            100,
            "the hints keep the right edge"
        );
        let available = 100 - split.sidebar.width - split.hints.width;
        assert_eq!(split.content.width, available.min(60));
        let left = split.content.x - split.sidebar.width;
        let right = split.hints.x - (split.content.x + split.content.width);
        assert!(right >= left && right - left <= 1, "{left} vs {right}");
    }

    #[test]
    fn areas_make_room_for_the_key_hints_sidebar_on_the_right() {
        let mut a = app("# T\n\ntext\n", (120, 24));
        a.apply(crate::config::actions::Action::ToggleKeyHints);
        let split = areas(&a, Rect::new(0, 0, 120, 24));
        assert!(split.hints.width > 0);
        assert_eq!(split.sidebar.width, 0);
        assert_eq!(split.content.x, 0);
        assert_eq!(split.hints.x, split.content.width, "it hugs the right edge");
        assert_eq!(split.content.width + split.hints.width, 120);
        assert_eq!(split.content.width, a.content_width() as u16);

        // Both sidebars: TOC left, hints right, document between them.
        a.apply(crate::config::actions::Action::ToggleToc);
        let split = areas(&a, Rect::new(0, 0, 120, 24));
        assert!(split.sidebar.width > 0 && split.hints.width > 0);
        assert_eq!(split.content.x, split.sidebar.width);
        assert_eq!(split.hints.x, split.sidebar.width + split.content.width);
        assert_eq!(
            split.sidebar.width + split.content.width + split.hints.width,
            120
        );

        // Too narrow for both: the hints give way, the TOC stays.
        a.resize(60, 24);
        let split = areas(&a, Rect::new(0, 0, 60, 24));
        assert!(split.sidebar.width > 0);
        assert_eq!(split.hints.width, 0);
    }

    #[test]
    fn the_key_hints_sidebar_is_drawn_and_follows_the_mode() {
        use ratatui::backend::TestBackend;
        let mut a = app("# Title\n\nSome text.\n", (100, 30));
        a.apply(crate::config::actions::Action::ToggleKeyHints);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("backend");
        terminal.draw(|f| draw(&a, f)).expect("draw");
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("Move"), "{text}");
        assert!(text.contains("quit"), "{text}");

        a.apply(crate::config::actions::Action::Search);
        terminal.draw(|f| draw(&a, f)).expect("draw");
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("confirm"), "search mode hints: {text}");
        assert!(!text.contains("toggle section"), "nothing unreachable");
    }

    #[test]
    fn a_short_terminal_keeps_the_features_that_distinguish_diple() {
        use ratatui::backend::TestBackend;
        // 100x20 is an ordinary terminal. Heading navigation and folding are
        // what diple has and `less` does not, so they must outlive the
        // generic pager rows when the sidebar has to shed groups.
        let mut a = app(
            "# One\n\ntext\n\n## Two\n\ntext\n\n## Three\n\ntext\n",
            (100, 20),
        );
        a.apply(crate::config::actions::Action::ToggleKeyHints);
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).expect("backend");
        terminal.draw(|f| draw(&a, f)).expect("draw");
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        for expected in ["Move", "Headings", "Fold", "View", "next/prev", "quit"] {
            assert!(text.contains(expected), "{expected} missing from: {text}");
        }
    }

    /// A workspace over one document, the shape every session starts in.
    fn single(src: &str, size: (u16, u16)) -> Workspace {
        Workspace::new(
            app(src, size),
            crate::config::Config::default(),
            crate::config::keys::KeyMap::with_defaults(),
            crate::terminal::capabilities::Capabilities::default(),
            None,
            false,
            crate::document::FormatRequest::Auto,
        )
    }

    #[test]
    fn a_resize_event_reaches_the_app() {
        let mut ws = single("# T\n\ntext\n", (80, 24));
        ws.handle_event(crossterm::event::Event::Resize(40, 10));
        assert_eq!(ws.focused().size(), (40, 10));
    }

    #[test]
    fn a_key_event_reaches_the_app() {
        let mut ws = single("# T\n\n", (80, 24));
        ws.handle_event(crossterm::event::Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )));
        assert!(
            ws.should_quit(),
            "the last document leaving ends the session"
        );
    }

    /// A backend that always produces a small image, so the layout engine
    /// reserves `LineKind::Image` rows.
    struct Imaging;

    impl crate::mermaid::MermaidRenderer for Imaging {
        fn render(
            &self,
            _block: &crate::document::markdown::MermaidBlock,
            _width: usize,
        ) -> crate::mermaid::MermaidRender {
            crate::mermaid::MermaidRender::ok(crate::mermaid::MermaidOutput::Image(
                crate::mermaid::ImageData {
                    png: vec![1, 2, 3],
                    width_px: 40,
                    height_px: 30,
                },
                (10, 4),
            ))
        }
    }

    fn image_app(src: &str, size: (u16, u16)) -> App {
        testing::AppBuilder::new(src)
            .size(size)
            .caps(crate::terminal::capabilities::Capabilities {
                images: crate::terminal::capabilities::ImageSupport::Kitty,
                ..Default::default()
            })
            .diagrams(DiagramProvider::new(Box::new(Imaging)))
            .build()
    }

    const FENCE: &str = "```mermaid\ngraph LR\nA-->B\n```\n";

    #[test]
    fn an_image_block_is_placed_even_when_a_heading_precedes_it() {
        // The blank separator line between two top-level blocks belongs to
        // the *following* node, so the first reserved image row carries
        // `node_offset == 1` and used to be skipped entirely — the reader saw
        // a blank rectangle and no image escape was ever transmitted.
        let content = Rect::new(0, 0, 80, 23);

        let first = image_app(FENCE, (80, 24));
        let placed_first = placed_images(&first, content);
        assert_eq!(placed_first.len(), 1, "fence as the first node");

        let after_heading = image_app(&format!("# Title\n\n{FENCE}"), (80, 24));
        let placed = placed_images(&after_heading, content);
        assert_eq!(
            placed.len(),
            1,
            "a diagram preceded by a heading must be placed too"
        );
        assert_eq!(placed[0].cols, 10);
        assert_eq!(placed[0].rows, 4);
        // It starts on the first reserved row, not on the separator blank.
        let line = after_heading
            .tree()
            .lines
            .get(usize::from(placed[0].y))
            .expect("the placed row exists");
        assert!(matches!(line.kind, LineKind::Image(_)), "{line:?}");
    }

    #[test]
    fn a_partially_scrolled_image_is_skipped_rather_than_clipped() {
        // The "skip rather than clip" rule must survive that placement fix.
        let tail = "\nfiller\n".repeat(6);
        let mut app = image_app(&format!("# Title\n\n{FENCE}{tail}"), (80, 8));
        let content = Rect::new(0, 0, 80, 7);
        let start = usize::from(placed_images(&app, content)[0].y);
        app.scroll_to(start);
        assert_eq!(
            placed_images(&app, content).len(),
            1,
            "the image is placed when its first row is at the top"
        );
        app.scroll_to(start + 1);
        assert!(
            placed_images(&app, content).is_empty(),
            "an image whose first row scrolled off the top must not be drawn"
        );
    }

    #[test]
    fn no_images_are_placed_without_a_protocol() {
        let a = app("```mermaid\ngraph LR\nA-->B\n```\n", (80, 24));
        assert!(placed_images(&a, Rect::new(0, 0, 80, 23)).is_empty());
        let mut out = Vec::new();
        draw_images(&a, &[], &mut out).expect("no-op");
        assert!(out.is_empty());
    }

    #[test]
    fn drawing_into_a_buffer_produces_a_frame() {
        use ratatui::backend::TestBackend;
        let a = app("# Title\n\nSome text with a [link](http://x).\n", (40, 10));
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).expect("backend");
        terminal.draw(|f| draw(&a, f)).expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let text: String = buffer.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Title"));
        assert!(text.contains("t.md"), "the status bar shows the file name");
    }

    #[test]
    fn a_centred_frame_pads_both_sides_of_every_document_row() {
        use ratatui::backend::TestBackend;
        let cfg = crate::config::Config {
            max_width: 20,
            center: true,
            ..crate::config::Config::default()
        };
        let a = testing::AppBuilder::new("# Title\n\nSome text that is long enough to wrap.\n")
            .size((40, 10))
            .config(cfg)
            .build();
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).expect("backend");
        terminal.draw(|f| draw(&a, f)).expect("draw");
        let buffer = terminal.backend().buffer().clone();

        let row = |y: u16| -> String { (0..40).map(|x| buffer[(x, y)].symbol()).collect() };
        // 40 columns, a 20-column document: 10 blank cells on either side of
        // every document row (the status row spans the full width).
        for y in 0..9 {
            let line = row(y);
            assert_eq!(&line[..10], " ".repeat(10), "row {y}: {line:?}");
            assert!(line.ends_with(&" ".repeat(10)), "row {y}: {line:?}");
        }
        assert!(row(0).contains("Title"), "{:?}", row(0));
    }
}
