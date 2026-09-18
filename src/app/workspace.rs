//! The set of open documents: tabs, the two panes of a split, and the focus.
//!
//! # Why this is not part of `App`
//!
//! [`App`] is one document: its render tree, its folds, its search, its
//! viewport anchor. Everything in it is keyed to *that* document, so the
//! cheapest way to show a second one is to have a second `App` — not a second
//! set of fields inside the first. The workspace owns those `App`s and
//! nothing else: it decides which of them has the keyboard, which rectangle
//! each one is drawn into, and what happens to the session when one of them
//! is closed.
//!
//! # How a view asks for something it cannot do
//!
//! An `App` cannot open a document, switch tabs or end the session, because
//! it does not know that any other document exists. Instead it records a
//! [`Request`], which the workspace takes after every event and carries out.
//! Key handling therefore stays where it was — one key map, one dispatcher —
//! and the workspace never has to guess what a key meant.
//!
//! # Geometry
//!
//! One row at the top is the tab bar, and only when a second tab is open. The
//! rest is the body, which one pane fills or two panes share — left and right
//! ([`Split::SideBySide`], with a one-column rule between them) or above and
//! below ([`Split::Stacked`]). Each pane's `App` is resized to its own
//! rectangle, so every width the layout engine is asked for is the width the
//! document is actually drawn at, split or not.
//!
//! # Settings are session-wide
//!
//! `:` writes into the focused view's [`Config`]. A setting is a property of
//! the session, though, not of one document, so after every event the
//! workspace compares that config with the one it last handed out and passes
//! any change on to every other view.

use std::path::Path;

use ratatui::layout::Rect;

use crate::app::command::OpenTarget;
use crate::app::events::areas;
use crate::app::paths;
use crate::app::state::{App, AppEnv, AppOptions};
use crate::config::keys::KeyMap;
use crate::config::Config;
use crate::document::FormatRequest;
use crate::render::terminal::{PaneDivider, TabBar};
use crate::terminal::capabilities::Capabilities;

/// Narrowest a pane may be before a side-by-side split is refused.
///
/// Below this the document is all wrapping and no text; the reader is better
/// served by a tab and a clear message than by two unreadable columns.
const MIN_PANE_WIDTH: u16 = 24;

/// Shortest a pane may be before a stacked split is refused: one status row
/// plus enough document to be worth looking at.
const MIN_PANE_HEIGHT: u16 = 5;

/// How the two panes of a tab share the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Split {
    /// Left and right.
    SideBySide,
    /// Above and below.
    Stacked,
}

/// Something only the workspace can do, recorded by a view.
///
/// Taking the request is what executes it — see [`App::take_request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Request {
    /// Open another document (`:open`).
    Open {
        /// Where to put it.
        target: OpenTarget,
        /// The path as it was typed.
        path: String,
    },
    /// Move the focus to the other pane of a split.
    FocusOtherPane,
    /// Show the next tab.
    NextTab,
    /// Show the previous tab.
    PreviousTab,
    /// Show the tab with this index.
    GotoTab(usize),
    /// Close the focused view, but never end the session (`:close`).
    Close,
    /// End the session, whatever is open (`Ctrl-C`, `:qa`).
    QuitAll,
}

/// One tab: one document, or two side by side.
struct Tab {
    /// The open panes, in screen order (left to right, or top to bottom).
    panes: Vec<App>,
    /// Index into `panes` of the one with the keyboard.
    focus: usize,
    /// How two panes share the screen; ignored while there is only one.
    split: Split,
}

impl Tab {
    fn single(app: App) -> Tab {
        Tab {
            panes: vec![app],
            focus: 0,
            split: Split::SideBySide,
        }
    }

    /// The rectangle of each pane, plus the divider between them.
    fn geometry(&self, body: Rect) -> (Vec<Rect>, Option<Rect>) {
        if self.panes.len() < 2 {
            return (vec![body], None);
        }
        match self.split {
            Split::SideBySide => {
                let left = body.width.saturating_sub(1) / 2;
                let right = body.width.saturating_sub(left + 1);
                (
                    vec![
                        Rect::new(body.x, body.y, left, body.height),
                        Rect::new(body.x + left + 1, body.y, right, body.height),
                    ],
                    Some(Rect::new(body.x + left, body.y, 1, body.height)),
                )
            }
            Split::Stacked => {
                let top = body.height / 2;
                let bottom = body.height.saturating_sub(top);
                (
                    vec![
                        Rect::new(body.x, body.y, body.width, top),
                        Rect::new(body.x, body.y + top, body.width, bottom),
                    ],
                    None,
                )
            }
        }
    }

    fn focused(&self) -> &App {
        &self.panes[self.focus.min(self.panes.len() - 1)]
    }

    fn focused_mut(&mut self) -> &mut App {
        let index = self.focus.min(self.panes.len() - 1);
        &mut self.panes[index]
    }

    /// The name shown in the tab bar: the focused document's file name
    /// without its directories, because a bar of paths is a bar of prefixes.
    fn label(&self) -> String {
        let name = self.focused().filename();
        Path::new(name)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.to_string())
    }
}

/// Every open document, and which one has the keyboard.
pub struct Workspace {
    tabs: Vec<Tab>,
    active: usize,
    /// The configuration every view is kept in step with.
    config: Config,
    keymap: KeyMap,
    caps: Capabilities,
    width_override: Option<u16>,
    debug: bool,
    /// The format request every document opened in this session is read with,
    /// so `:open` agrees with the command line.
    format: FormatRequest,
    size: (u16, u16),
    quit: bool,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("tabs", &self.tabs.len())
            .field("active", &self.active)
            .field("panes", &self.tabs[self.active].panes.len())
            .field("focus", &self.tabs[self.active].focus)
            .finish()
    }
}

impl Workspace {
    /// A workspace showing one document.
    ///
    /// The configuration, key map and capabilities are kept so that a
    /// document opened later is built exactly like this one was.
    pub fn new(
        first: App,
        config: Config,
        keymap: KeyMap,
        caps: Capabilities,
        width_override: Option<u16>,
        debug: bool,
        format: FormatRequest,
    ) -> Workspace {
        let size = first.size();
        let mut workspace = Workspace {
            tabs: vec![Tab::single(first)],
            active: 0,
            config,
            keymap,
            caps,
            width_override,
            debug,
            format,
            size,
            quit: false,
        };
        workspace.sync_views();
        workspace
    }

    // -- accessors --------------------------------------------------------

    /// The view with the keyboard.
    pub(crate) fn focused(&self) -> &App {
        self.tabs[self.active].focused()
    }

    /// The view with the keyboard, mutably.
    pub(crate) fn focused_mut(&mut self) -> &mut App {
        let active = self.active;
        self.tabs[active].focused_mut()
    }

    /// The detected terminal capabilities (the same for every view).
    pub(crate) fn caps(&self) -> &Capabilities {
        &self.caps
    }

    /// Whether the session is over.
    pub(crate) fn should_quit(&self) -> bool {
        self.quit
    }

    /// Whether diple is currently asking the terminal for mouse events.
    pub(crate) fn mouse_on(&self) -> bool {
        self.focused().mouse_on()
    }

    /// The visible panes with the rectangle each is drawn into.
    pub(crate) fn panes(&self) -> Vec<(&App, Rect)> {
        let tab = &self.tabs[self.active];
        let (rects, _) = tab.geometry(self.body());
        tab.panes.iter().zip(rects).collect()
    }

    /// One row at the top, but only once a second tab is open.
    fn tab_bar_height(&self) -> u16 {
        u16::from(self.tabs.len() > 1).min(self.size.1)
    }

    /// The rectangle the panes share.
    fn body(&self) -> Rect {
        let bar = self.tab_bar_height();
        Rect::new(0, bar, self.size.0, self.size.1.saturating_sub(bar).max(1))
    }

    /// The tab bar over `labels`, which the caller holds: the widget borrows
    /// them rather than owning them.
    fn tab_bar<'a>(&'a self, labels: &'a [String]) -> TabBar<'a> {
        TabBar {
            labels,
            active: self.active,
            theme: &self.focused().theme,
            level: self.focused().color,
        }
    }

    /// One label per tab, in order.
    fn tab_labels(&self) -> Vec<String> {
        self.tabs.iter().map(Tab::label).collect()
    }

    // -- frame ------------------------------------------------------------

    /// Draw every visible pane, the divider and the tab bar.
    pub(crate) fn draw(&self, frame: &mut ratatui::Frame<'_>) {
        let tab = &self.tabs[self.active];
        let (rects, divider) = tab.geometry(self.body());
        for (index, (app, rect)) in tab.panes.iter().zip(rects).enumerate() {
            crate::app::events::draw_in(app, frame, rect, index == tab.focus);
        }
        if let Some(rect) = divider {
            frame.render_widget(
                PaneDivider {
                    theme: &self.focused().theme,
                    level: self.focused().color,
                    unicode: self.caps.unicode_box,
                },
                rect,
            );
        }
        if self.tab_bar_height() > 0 {
            let labels = self.tab_labels();
            frame.render_widget(self.tab_bar(&labels), Rect::new(0, 0, self.size.0, 1));
        }
    }

    /// Prepare every visible pane for the frame about to be drawn.
    pub(crate) fn prepare_frame(&mut self) {
        self.sync_views();
        let active = self.active;
        for app in &mut self.tabs[active].panes {
            app.prepare_frame();
        }
    }

    /// Highlight code just outside the viewport of the focused pane while the
    /// session is idle.
    pub(crate) fn realize_ahead(&mut self) -> bool {
        self.focused_mut().realize_ahead()
    }

    /// Reap the link openers every view may have spawned.
    pub(crate) fn reap_children(&mut self) {
        for tab in &mut self.tabs {
            for app in &mut tab.panes {
                app.reap_children();
            }
        }
    }

    /// React to a terminal resize: every view is resized to its own pane.
    pub(crate) fn resize(&mut self, cols: u16, rows: u16) {
        self.size = (cols.max(1), rows.max(1));
        self.sync_views();
    }

    /// Push the current geometry and view counts into every view.
    ///
    /// Called before every frame rather than only after a resize, because
    /// opening, closing and switching all change what a pane is given.
    fn sync_views(&mut self) {
        let body = self.body();
        let tabs = self.tabs.len();
        for tab in &mut self.tabs {
            let (rects, _) = tab.geometry(body);
            let panes = tab.panes.len();
            for (app, rect) in tab.panes.iter_mut().zip(rects) {
                app.set_view_context(tabs, panes);
                app.resize(rect.width, rect.height);
            }
        }
    }

    // -- input ------------------------------------------------------------

    /// Dispatch one terminal event.
    pub(crate) fn handle_event(&mut self, event: crossterm::event::Event) {
        use crossterm::event::Event;
        match event {
            Event::Key(key) => {
                self.focused_mut().handle_key(&key);
                self.settle();
            }
            Event::Mouse(mouse) => {
                self.handle_mouse(&mouse);
                self.settle();
            }
            Event::Resize(cols, rows) => self.resize(cols, rows),
            Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
        }
    }

    /// A mouse event, routed to the pane it happened in.
    ///
    /// A click focuses that pane; scrolling does not, so the wheel moves the
    /// document under the pointer without taking the keyboard away from the
    /// one being read.
    fn handle_mouse(&mut self, event: &crossterm::event::MouseEvent) {
        use crossterm::event::{MouseButton, MouseEventKind};
        let click = matches!(event.kind, MouseEventKind::Down(MouseButton::Left));
        if self.tab_bar_height() > 0 && event.row == 0 {
            if click {
                if let Some(index) = self.tab_at(event.column) {
                    self.goto_tab(index);
                }
            }
            return;
        }
        let (rects, _) = self.tabs[self.active].geometry(self.body());
        let Some(index) = rects
            .iter()
            .position(|r| contains(*r, event.column, event.row))
        else {
            return; // the divider column belongs to neither pane
        };
        if click && self.tabs[self.active].focus != index {
            self.tabs[self.active].focused_mut().blur();
            self.tabs[self.active].focus = index;
        }
        let rect = rects[index];
        // The view knows nothing about where it is on the screen: its own
        // click handling counts from its top-left corner.
        let local = crossterm::event::MouseEvent {
            column: event.column - rect.x,
            row: event.row - rect.y,
            ..*event
        };
        self.tabs[self.active].panes[index].handle_mouse(&local);
    }

    /// The tab whose label covers `column` in the tab bar.
    fn tab_at(&self, column: u16) -> Option<usize> {
        let labels = self.tab_labels();
        let column = usize::from(column);
        self.tab_bar(&labels)
            .spans(usize::from(self.size.0))
            .into_iter()
            .position(|(start, width)| column >= start && column < start + width)
    }

    // -- requests ---------------------------------------------------------

    /// Carry out what the focused view asked for, then bring the session back
    /// into a consistent state.
    fn settle(&mut self) {
        if let Some(request) = self.focused_mut().take_request() {
            self.perform(request);
        }
        if !self.quit && self.focused().should_quit() {
            self.close_focused(true);
        }
        self.propagate_config();
        self.sync_views();
    }

    fn perform(&mut self, request: Request) {
        match request {
            Request::QuitAll => self.quit = true,
            Request::Close => self.close_focused(false),
            Request::FocusOtherPane => self.focus_other_pane(),
            Request::NextTab => self.cycle_tab(1),
            Request::PreviousTab => self.cycle_tab(-1),
            Request::GotoTab(index) => self.goto_tab(index),
            Request::Open { target, path } => self.open(target, &path),
        }
    }

    /// Hand a `:` change on to every other view.
    fn propagate_config(&mut self) {
        let current = self.focused().config().clone();
        if current == self.config {
            return;
        }
        self.config = current;
        let config = self.config.clone();
        for (t, tab) in self.tabs.iter_mut().enumerate() {
            for (p, app) in tab.panes.iter_mut().enumerate() {
                if t == self.active && p == tab.focus {
                    continue; // the view that was typed in is already there
                }
                app.adopt_config(config.clone());
            }
        }
    }

    fn focus_other_pane(&mut self) {
        let tab = &mut self.tabs[self.active];
        if tab.panes.len() < 2 {
            tab.focused_mut()
                .set_message("no other pane (`:open side-by-side <path>` makes one)");
            return;
        }
        tab.focused_mut().blur();
        tab.focus = (tab.focus + 1) % tab.panes.len();
    }

    fn cycle_tab(&mut self, delta: isize) {
        let count = self.tabs.len();
        if count < 2 {
            self.focused_mut()
                .set_message("only one tab (`:open tab <path>` makes another)");
            return;
        }
        let next = (self.active as isize + delta).rem_euclid(count as isize);
        self.focused_mut().blur();
        self.active = next as usize;
    }

    fn goto_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            if index != self.active {
                self.focused_mut().blur();
            }
            self.active = index;
        } else {
            self.focused_mut()
                .set_message(format!("no tab {}", index + 1));
        }
    }

    /// Close the focused pane, or the focused tab when it is the only pane.
    ///
    /// `may_quit` is false for `:close`, which never ends the session — it is
    /// the command for closing *a* document, and `:q` is the one for closing
    /// the last.
    fn close_focused(&mut self, may_quit: bool) {
        let tab = &mut self.tabs[self.active];
        if tab.panes.len() > 1 {
            tab.panes.remove(tab.focus);
            tab.focus = 0;
            return;
        }
        if self.tabs.len() > 1 {
            self.tabs.remove(self.active);
            self.active = self.active.min(self.tabs.len() - 1);
            return;
        }
        if may_quit {
            self.quit = true;
        } else {
            self.focused_mut()
                .set_message("this is the last document (`:q` leaves)");
        }
    }

    // -- opening ----------------------------------------------------------

    /// Open `path`, or report why it could not be opened.
    fn open(&mut self, target: OpenTarget, path: &str) {
        let near = self.focused().filename().to_string();
        let resolved = paths::resolve(path, Some(&near));
        if let Some(reason) = self.no_room_for(target) {
            self.focused_mut().set_message(reason);
            return;
        }
        let app = match self.load(&resolved) {
            Ok(app) => app,
            Err(message) => {
                self.focused_mut().set_message(message);
                return;
            }
        };
        let name = app.filename().to_string();
        self.focused_mut().blur();
        match target {
            OpenTarget::Tab => {
                self.tabs.push(Tab::single(app));
                self.active = self.tabs.len() - 1;
            }
            OpenTarget::SideBySide | OpenTarget::Stacked => {
                let split = if target == OpenTarget::SideBySide {
                    Split::SideBySide
                } else {
                    Split::Stacked
                };
                let tab = &mut self.tabs[self.active];
                tab.split = split;
                if tab.panes.len() > 1 {
                    // Both panes are taken, so the new document replaces the
                    // one the reader is *not* looking at.
                    let other = (tab.focus + 1) % tab.panes.len();
                    tab.panes[other] = app;
                    tab.focus = other;
                } else {
                    tab.panes.push(app);
                    tab.focus = tab.panes.len() - 1;
                }
            }
        }
        self.sync_views();
        self.focused_mut().set_message(format!("opened {name}"));
    }

    /// Why `target` does not fit on this terminal, if it does not.
    fn no_room_for(&self, target: OpenTarget) -> Option<String> {
        let body = self.body();
        // Opening in a tab costs the tab bar's row, which any terminal has.
        match target {
            OpenTarget::Tab => None,
            OpenTarget::SideBySide if body.width < 2 * MIN_PANE_WIDTH + 1 => Some(format!(
                "too narrow to split: needs {} columns, has {}",
                2 * MIN_PANE_WIDTH + 1,
                body.width
            )),
            OpenTarget::Stacked if body.height < 2 * MIN_PANE_HEIGHT => Some(format!(
                "too short to split: needs {} rows, has {}",
                2 * MIN_PANE_HEIGHT,
                body.height
            )),
            _ => None,
        }
    }

    /// Read, parse and lay out a document, built exactly like the first one.
    fn load(&self, path: &Path) -> Result<App, String> {
        let bytes =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
        };
        // `:open` detects the format exactly the way the command line does —
        // the same call, the same session-wide `--format` — so a Markdown
        // document and a YAML one can sit side by side in one split.
        let source = crate::document::SourceDocument::new(path.display().to_string(), text);
        let doc = crate::document::load(self.format, source)
            .map_err(|error| error.report())?
            .model;
        let color = crate::app::color_level(self.config.color, &self.caps);
        let theme = crate::app::resolve_theme(&self.config.theme, color);
        let diagrams =
            crate::app::diagram_provider(&doc, &self.config, &self.caps, usize::from(self.size.0));
        Ok(App::new(
            doc,
            self.config.clone(),
            self.keymap.clone(),
            AppEnv {
                caps: self.caps.clone(),
                theme,
                color,
                diagrams,
            },
            AppOptions {
                filename: path.display().to_string(),
                size: self.size,
                width_override: self.width_override,
                debug: self.debug,
            },
        ))
    }

    /// The content rectangle of each visible pane, for the image and
    /// hyperlink passes the event loop runs after ratatui.
    pub(crate) fn pane_contents(&self) -> Vec<(&App, Rect)> {
        self.panes()
            .into_iter()
            .map(|(app, rect)| {
                let content = areas(app, rect).content;
                (app, content)
            })
            .collect()
    }
}

/// Whether `rect` covers the cell at `(column, row)`.
fn contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::fs;
    use std::path::PathBuf;

    /// A workspace over one in-memory document at a comfortable size.
    fn workspace(size: (u16, u16)) -> Workspace {
        let app = testing::app_sized("# One\n\nBody text.\n", size);
        Workspace::new(
            app,
            Config::default(),
            KeyMap::with_defaults(),
            Capabilities::default(),
            None,
            false,
            FormatRequest::Auto,
        )
    }

    /// A directory holding two documents to open, removed with the test.
    struct Docs(PathBuf);

    impl Docs {
        fn new(name: &str) -> Docs {
            let dir =
                std::env::temp_dir().join(format!("diple-workspace-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            fs::write(dir.join("second.md"), "# Second\n\nMore text.\n").expect("write");
            fs::write(dir.join("third.md"), "# Third\n\nEven more.\n").expect("write");
            Docs(dir)
        }

        fn path(&self, name: &str) -> String {
            self.0.join(name).display().to_string()
        }
    }

    impl Drop for Docs {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn command(ws: &mut Workspace, line: &str) {
        ws.focused_mut().run_command(line);
        ws.settle();
    }

    #[test]
    fn opening_side_by_side_splits_the_body_and_focuses_the_new_document() {
        let docs = Docs::new("split");
        let mut ws = workspace((100, 24));
        command(
            &mut ws,
            &format!("open side-by-side {}", docs.path("second.md")),
        );

        assert_eq!(ws.panes().len(), 2, "both documents are visible");
        assert!(ws.focused().filename().ends_with("second.md"));
        let (left, right) = (ws.panes()[0].1, ws.panes()[1].1);
        assert_eq!(left.width + right.width + 1, 100, "a column for the rule");
        assert_eq!(left.height, 24, "no tab bar with a single tab");
        assert_eq!(ws.focused().size(), (right.width, right.height));

        // A third document takes the pane the reader is not looking at.
        command(
            &mut ws,
            &format!("open side-by-side {}", docs.path("third.md")),
        );
        assert_eq!(ws.panes().len(), 2, "never more than two panes");
        assert!(ws.focused().filename().ends_with("third.md"));
        assert!(ws.panes()[1].0.filename().ends_with("second.md"));
    }

    #[test]
    fn opening_stacked_shares_the_rows() {
        let docs = Docs::new("stacked");
        let mut ws = workspace((80, 24));
        command(&mut ws, &format!("open stacked {}", docs.path("second.md")));
        let (top, bottom) = (ws.panes()[0].1, ws.panes()[1].1);
        assert_eq!(top.width, 80);
        assert_eq!(top.height + bottom.height, 24);
        assert_eq!(bottom.y, top.height);
    }

    #[test]
    fn opening_a_tab_adds_the_tab_bar_and_takes_a_row_from_every_document() {
        let docs = Docs::new("tab");
        let mut ws = workspace((80, 24));
        assert_eq!(ws.tab_bar_height(), 0, "one tab needs no bar");
        command(&mut ws, &format!("open tab {}", docs.path("second.md")));
        assert_eq!(ws.tab_bar_height(), 1);
        assert_eq!(ws.panes()[0].1, Rect::new(0, 1, 80, 23));
        assert_eq!(
            ws.tabs[0].panes[0].size(),
            (80, 23),
            "the tab that is not shown is resized too, so switching to it is instant"
        );
    }

    #[test]
    fn tabs_cycle_in_both_directions_and_by_number() {
        let docs = Docs::new("cycle");
        let mut ws = workspace((80, 24));
        command(&mut ws, &format!("open tab {}", docs.path("second.md")));
        command(&mut ws, &format!("open tab {}", docs.path("third.md")));
        assert_eq!(ws.active, 2);

        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        ws.handle_event(crossterm::event::Event::Key(ctrl('n')));
        assert_eq!(ws.active, 0, "the last tab wraps to the first");
        ws.handle_event(crossterm::event::Event::Key(ctrl('p')));
        assert_eq!(ws.active, 2, "and back again");

        ws.handle_event(crossterm::event::Event::Key(KeyEvent::new(
            KeyCode::Char('2'),
            KeyModifiers::ALT,
        )));
        assert_eq!(ws.active, 1, "Alt-2 is the second tab");
    }

    #[test]
    fn the_focus_key_moves_between_the_two_panes() {
        let docs = Docs::new("focus");
        let mut ws = workspace((100, 24));
        command(
            &mut ws,
            &format!("open side-by-side {}", docs.path("second.md")),
        );
        assert_eq!(ws.tabs[0].focus, 1);
        ws.handle_event(crossterm::event::Event::Key(KeyEvent::new(
            KeyCode::Char('w'),
            KeyModifiers::CONTROL,
        )));
        assert_eq!(ws.tabs[0].focus, 0);
    }

    #[test]
    fn closing_walks_back_out_pane_then_tab_then_session() {
        let docs = Docs::new("close");
        let mut ws = workspace((100, 24));
        command(&mut ws, &format!("open tab {}", docs.path("second.md")));
        command(
            &mut ws,
            &format!("open side-by-side {}", docs.path("third.md")),
        );
        assert_eq!((ws.tabs.len(), ws.panes().len()), (2, 2));

        command(&mut ws, "q");
        assert_eq!(ws.panes().len(), 1, "the pane goes first");
        assert!(!ws.should_quit());
        command(&mut ws, "q");
        assert_eq!(ws.tabs.len(), 1, "then the tab");
        assert!(!ws.should_quit());
        command(&mut ws, "q");
        assert!(ws.should_quit(), "and the last one leaves");
    }

    #[test]
    fn close_never_ends_the_session_and_quit_all_always_does() {
        let mut ws = workspace((80, 24));
        command(&mut ws, "close");
        assert!(!ws.should_quit());
        assert_eq!(
            ws.focused().message(),
            Some("this is the last document (`:q` leaves)")
        );
        command(&mut ws, "qa");
        assert!(ws.should_quit());
    }

    #[test]
    fn a_view_that_loses_the_keyboard_gives_up_its_prompt() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let docs = Docs::new("blur");
        let mut ws = workspace((100, 24));
        ws.focused_mut().enable_mouse_for_test();
        command(
            &mut ws,
            &format!("open side-by-side {}", docs.path("second.md")),
        );
        ws.focused_mut().enable_mouse_for_test();
        // The `:` line has the keys, so only the mouse can move the focus —
        // which is exactly the case this is about.
        ws.handle_event(crossterm::event::Event::Key(KeyEvent::new(
            KeyCode::Char(':'),
            KeyModifiers::NONE,
        )));
        assert_eq!(ws.focused().mode(), crate::app::state::Mode::Command);
        ws.handle_event(crossterm::event::Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 3,
            modifiers: KeyModifiers::NONE,
        }));
        assert_eq!(ws.tabs[0].focus, 0);
        assert_eq!(
            ws.tabs[0].panes[1].mode(),
            crate::app::state::Mode::Normal,
            "a `:` line in a pane nobody is typing in would reserve a row for nothing"
        );
    }

    #[test]
    fn a_setting_changed_in_one_view_reaches_every_other_one() {
        let docs = Docs::new("config");
        let mut ws = workspace((100, 24));
        command(
            &mut ws,
            &format!("open side-by-side {}", docs.path("second.md")),
        );
        command(&mut ws, "line_numbers = true");
        assert!(
            ws.tabs[0].panes.iter().all(|p| p.config().line_numbers),
            "a setting belongs to the session, not to one document"
        );
    }

    #[test]
    fn a_document_that_cannot_be_read_reports_it_and_changes_nothing() {
        let mut ws = workspace((100, 24));
        command(&mut ws, "open tab /no/such/file.md");
        assert_eq!(ws.tabs.len(), 1);
        let message = ws.focused().message().unwrap_or_default().to_string();
        assert!(
            message.starts_with("cannot read /no/such/file.md"),
            "{message}"
        );
    }

    #[test]
    fn a_terminal_with_no_room_refuses_the_split_rather_than_halving_it() {
        let docs = Docs::new("narrow");
        let mut ws = workspace((40, 24));
        command(
            &mut ws,
            &format!("open side-by-side {}", docs.path("second.md")),
        );
        assert_eq!(ws.panes().len(), 1);
        assert!(ws
            .focused()
            .message()
            .unwrap_or_default()
            .starts_with("too narrow to split"));

        let mut ws = workspace((80, 8));
        command(&mut ws, &format!("open stacked {}", docs.path("second.md")));
        assert_eq!(ws.panes().len(), 1);
        assert!(ws
            .focused()
            .message()
            .unwrap_or_default()
            .starts_with("too short to split"));
    }

    #[test]
    fn a_click_focuses_the_pane_it_landed_in_and_a_click_on_the_bar_switches_tab() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let docs = Docs::new("mouse");
        let mut ws = workspace((100, 24));
        ws.focused_mut().enable_mouse_for_test();
        command(
            &mut ws,
            &format!("open side-by-side {}", docs.path("second.md")),
        );
        assert_eq!(ws.tabs[0].focus, 1);

        let click = |column, row| {
            crossterm::event::Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            })
        };
        ws.handle_event(click(3, 3));
        assert_eq!(ws.tabs[0].focus, 0, "a click moves the keyboard there");

        command(&mut ws, &format!("open tab {}", docs.path("third.md")));
        assert_eq!(ws.active, 1);
        ws.handle_event(click(1, 0));
        assert_eq!(ws.active, 0, "the first label selects the first tab");
    }
}
