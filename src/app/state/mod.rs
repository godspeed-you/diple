//! Application state: the document, its layout cache, the semantic viewport
//! anchor and every action handler.
//!
//! # Semantic anchoring
//!
//! The viewport is never stored as a raw line number. [`App`] keeps an anchor
//! `(NodeId, offset)` — the node at the top of the screen plus the line offset
//! inside that node — and derives `top_line` from it after every re-layout.
//! Resizing, folding, searching and toggling a diagram therefore all keep the
//! same content at the top of the screen.
//!
//! # Layout caching
//!
//! [`App::ensure_layout`] only rebuilds the [`RenderTree`] when an input
//! actually changed (width, fold state, theme, search matches, render
//! options). Scrolling, link cycling, TOC selection and help scrolling never
//! trigger a re-layout.

//!
//! # Layout of this module
//!
//! The state is one inherent `impl App` split over five files along the lines
//! of what a key press actually does. Rust lets an inherent impl live in the
//! submodules of the module that defines the type, and private fields stay
//! visible to descendants, so the split costs no accessors and no widened
//! API — only the reader's jump between files.
//!
//! * this file — the value types ([`Mode`], [`AppOptions`], [`AppEnv`]), the
//!   [`App`] struct itself, its constructor, every accessor, the viewport
//!   geometry, scrolling and the transient status message,
//! * `layout_cache` — the performance contract: when the render
//!   tree is rebuilt, when it is spliced, and what is deliberately *not* a
//!   layout input,
//! * `input` — key and mouse events and the [`Action`](crate::app::Action)
//!   dispatcher they feed,
//! * `navigate` — everything that moves the cursor through the document:
//!   folding, heading jumps, search driving and links,
//! * `sidebars` — the TOC and key-hints panes and the Mermaid source
//!   toggle, the three things that live beside the document rather than in
//!   it.

#[cfg(test)]
mod characterization;
mod input;
mod layout_cache;
mod navigate;
mod sidebars;

use std::process::Child;

use crate::app::command::CommandState;
use crate::app::diagrams::DiagramProvider;
use crate::app::hints::HintsState;
use crate::app::search_ui::SearchState;
use crate::app::toc::TocState;
use crate::app::workspace::Request;
use crate::config::keys::KeyMap;
use crate::config::schema::Osc8Mode;
use crate::config::Config;
use crate::document::{
    DocumentCapabilities, DocumentKind, DocumentModel, DocumentPath, FoldId, FoldState, LinkId,
    NodeId,
};
use crate::layout::Layout;
use crate::render::primitives::RenderTree;
use crate::render::theme::{ColorLevel, Theme};
use crate::terminal::capabilities::Capabilities;
use crate::util::viewport::max_top_line;
use layout_cache::BuildKey;

/// Lines of context kept above a heading after a heading jump, so that a jump
/// preserves enough surrounding context.
pub(crate) const HEADING_CONTEXT: usize = 2;

/// What the help overlay is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpKind {
    /// Keys and what they do (`?`).
    Keys,
    /// Settings, their values and their defaults (`:help`).
    Settings,
}

/// Interaction modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Normal pager mode.
    Normal,
    /// The `/` search prompt has keyboard focus.
    Search,
    /// The `:` command line has keyboard focus.
    Command,
    /// The TOC sidebar has keyboard focus.
    Toc,
    /// The help overlay is shown.
    Help,
    /// A transient message is shown; any key returns to [`Mode::Normal`].
    Message,
}

/// Everything needed to construct an [`App`] that is not the document or the
/// configuration.
#[derive(Debug, Clone)]
pub struct AppOptions {
    /// Name shown in the status bar.
    pub filename: String,
    /// Terminal size in `(columns, rows)`.
    pub size: (u16, u16),
    /// `--width`: pin the layout width regardless of the terminal size.
    pub width_override: Option<u16>,
    /// Emit `--debug` timings on stderr.
    pub debug: bool,
}

impl Default for AppOptions {
    fn default() -> Self {
        Self {
            filename: "<stdin>".to_string(),
            size: (80, 24),
            width_override: None,
            debug: false,
        }
    }
}

/// The rendering environment an [`App`] runs in: what the terminal can do,
/// the resolved theme and the diagram backend.
///
/// Grouped into one value so [`App::new`] stays readable.
#[derive(Debug)]
pub struct AppEnv {
    /// Detected terminal capabilities.
    pub caps: Capabilities,
    /// Resolved (and colour-downgraded) theme.
    pub theme: Theme,
    /// Effective colour level.
    pub color: ColorLevel,
    /// Mermaid renderer with its cache and image registry.
    pub diagrams: DiagramProvider,
}

/// The interactive application state.
pub struct App {
    /// The parsed document, whatever format it came from.
    pub(crate) doc: DocumentModel,
    /// Which foldable units are collapsed.
    pub(crate) folds: FoldState,
    /// Effective configuration.
    pub(crate) config: Config,
    /// Active key bindings.
    keymap: KeyMap,
    /// Detected terminal capabilities.
    pub(crate) caps: Capabilities,
    /// Resolved (and colour-downgraded) theme.
    pub(crate) theme: Theme,
    /// Effective colour level.
    pub(crate) color: ColorLevel,
    /// Mermaid renderer, diagram cache and image registry.
    pub(crate) diagrams: DiagramProvider,
    /// Search prompt and result set.
    pub(crate) search: SearchState,
    /// The folds as they were when the search prompt opened. The preview
    /// reveals each match it passes through; only the one the reader accepts
    /// with `Enter` may keep its path open (spec AC-09).
    pub(crate) search_folds: Option<FoldState>,
    pub(crate) command: CommandState,
    /// TOC sidebar state.
    pub(crate) toc: TocState,
    /// Key hints sidebar state (the right-hand mirror of `App::toc`).
    pub(crate) hints: HintsState,

    layout: Layout,
    tree: RenderTree,
    built: Option<BuildKey>,
    diagram_generation: u64,
    relayouts: u64,

    size: (u16, u16),
    width_override: Option<u16>,
    anchor: (NodeId, usize),
    cursor: NodeId,
    top_line: usize,
    h_offset: usize,
    painted: Option<(usize, usize)>,
    painted_query: String,
    selected_link: Option<LinkId>,
    mode: Mode,
    help_kind: HelpKind,
    mouse_on: bool,
    message: Option<String>,
    pending: String,
    help_scroll: usize,
    filename: String,
    debug: bool,
    quit: bool,
    children: Vec<Child>,
    /// What this view is asking the workspace to do, if anything.
    ///
    /// An [`App`] only ever knows its own document, so the actions that are
    /// *about* the set of open documents — opening one, switching tabs,
    /// leaving for good — are recorded here and carried out by
    /// [`crate::app::workspace::Workspace`], which owns that set. Taking the
    /// request is what executes it, so it can never be executed twice.
    request: Option<Request>,
    /// How many tabs are open, and how many panes the active tab has.
    ///
    /// Pushed in by the workspace before every frame. The document does not
    /// need it, but the status bar and the key hints do: a key that switches
    /// panes is only worth offering while there is a second pane.
    views: (usize, usize),
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("filename", &self.filename)
            .field("mode", &self.mode)
            .field("top_line", &self.top_line)
            .field("anchor", &self.anchor)
            .field("lines", &self.tree.len())
            .finish()
    }
}

impl App {
    /// Build the application state and lay the document out once.
    pub fn new(
        doc: DocumentModel,
        config: Config,
        keymap: KeyMap,
        env: AppEnv,
        opts: AppOptions,
    ) -> App {
        let AppEnv {
            caps,
            theme,
            color,
            diagrams,
        } = env;
        let folds = doc.fold_state();
        let mut toc = TocState::new();
        // Deriving the outline is the one thing a lazily built sidebar cannot
        // put off: a session that starts with it open has to have something
        // to draw.
        if config.toc {
            toc.ensure(&doc);
            toc.open = !toc.is_empty();
        }
        let hints = HintsState {
            open: config.key_hints,
        };
        let anchor = (doc.first_semantic().unwrap_or(0), 0);
        let mut app = App {
            doc,
            folds,
            config,
            keymap,
            caps,
            theme,
            color,
            diagrams,
            search: SearchState::default(),
            search_folds: None,
            command: CommandState::default(),
            toc,
            hints,
            layout: Layout::new(),
            tree: RenderTree::default(),
            built: None,
            diagram_generation: 0,
            relayouts: 0,
            size: opts.size,
            width_override: opts.width_override,
            anchor,
            cursor: anchor.0,
            top_line: 0,
            h_offset: 0,
            painted: None,
            painted_query: String::new(),
            selected_link: None,
            mode: Mode::Normal,
            help_kind: HelpKind::Keys,
            mouse_on: false, // set below, once `config` and `caps` are owned
            message: None,
            pending: String::new(),
            help_scroll: 0,
            filename: opts.filename,
            debug: opts.debug,
            quit: false,
            children: Vec::new(),
            request: None,
            views: (1, 1),
        };
        app.mouse_on = app.config.mouse && app.caps.mouse;
        app.ensure_layout();
        app
    }

    // -- accessors --------------------------------------------------------

    /// The current render tree.
    pub(crate) fn tree(&self) -> &RenderTree {
        &self.tree
    }

    /// Index of the first visible line.
    pub(crate) fn top_line(&self) -> usize {
        self.top_line
    }

    /// Horizontal scroll offset in columns.
    pub(crate) fn h_offset(&self) -> usize {
        self.h_offset
    }

    /// The semantic viewport anchor `(node, line offset within the node)`.
    #[cfg(test)]
    pub(crate) fn anchor(&self) -> (NodeId, usize) {
        self.anchor
    }

    /// The current interaction mode.
    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    /// The transient message shown in the status line, if any.
    pub(crate) fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// The pending multi-key prefix (e.g. `z`), shown in the status line.
    pub(crate) fn pending(&self) -> &str {
        &self.pending
    }

    /// First visible help line (the overlay scrolls).
    pub(crate) fn help_scroll(&self) -> usize {
        self.help_scroll
    }

    /// The currently selected link, if any.
    pub(crate) fn selected_link(&self) -> Option<LinkId> {
        self.selected_link
    }

    /// Document name shown in the status bar.
    pub(crate) fn filename(&self) -> &str {
        &self.filename
    }

    /// Terminal size in `(columns, rows)`.
    pub(crate) fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Whether the app was asked to quit.
    ///
    /// With more than one document open this means "close this view"; the
    /// workspace decides whether that also ends the session.
    pub(crate) fn should_quit(&self) -> bool {
        self.quit
    }

    /// Take the pending workspace request, if there is one.
    pub(crate) fn take_request(&mut self) -> Option<Request> {
        self.request.take()
    }

    /// Ask the workspace for something only it can do.
    ///
    /// The last request of one key press wins; a key produces at most one.
    pub(crate) fn request(&mut self, request: Request) {
        self.request = Some(request);
    }

    /// The effective configuration, so the workspace can notice a `:` command
    /// that changed it and pass the change on to the other views.
    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// Adopt a configuration another view changed.
    ///
    /// A setting is a property of the session, not of one document: `:theme
    /// crt` in the left pane has to reach the right one too. Everything a
    /// setting can touch is re-derived here, which is the same work
    /// `apply_effect` does for the view that was typed in.
    pub(crate) fn adopt_config(&mut self, config: Config) {
        if self.config == config {
            return;
        }
        self.config = config;
        self.color = crate::app::color_level(self.config.color, &self.caps);
        self.theme = crate::app::resolve_theme(&self.config.theme, self.color);
        self.toc.open = self.config.toc && !self.toc.is_empty();
        self.hints.open = self.config.key_hints;
        if self.mode == Mode::Toc && !self.toc.open {
            self.mode = Mode::Normal;
        }
        self.mouse_on = self.config.mouse && self.caps.mouse;
        self.invalidate();
        self.ensure_layout();
    }

    /// Give up the keyboard.
    ///
    /// A prompt or an overlay belongs to the view being *typed in*: a `:`
    /// line left open in a pane nobody is looking at would keep a row of that
    /// pane reserved for a prompt that cannot be typed into. The TOC sidebar
    /// stays open, because it is a sidebar rather than a mode — only the
    /// focus inside it is given back to the document.
    pub(crate) fn blur(&mut self) {
        match self.mode {
            Mode::Command => {
                self.command.open();
                self.mode = Mode::Normal;
            }
            Mode::Search | Mode::Help | Mode::Toc | Mode::Message => {
                self.help_scroll = 0;
                self.mode = Mode::Normal;
            }
            Mode::Normal => {}
        }
        self.clear_message();
    }

    /// Tell this view how many tabs and panes the session has.
    pub(crate) fn set_view_context(&mut self, tabs: usize, panes: usize) {
        self.views = (tabs.max(1), panes.max(1));
    }

    /// How many tabs are open, and how many panes the active tab has.
    pub(crate) fn views(&self) -> (usize, usize) {
        self.views
    }

    /// Pretend the terminal reports mouse events, for tests that click.
    #[cfg(test)]
    pub(crate) fn enable_mouse_for_test(&mut self) {
        self.caps.mouse = true;
        self.mouse_on = true;
    }

    /// What the help overlay is currently showing.
    pub(crate) fn help_kind(&self) -> HelpKind {
        self.help_kind
    }

    /// Whether diple is currently asking the terminal for mouse events.
    ///
    /// The configured value at startup, and whatever [`Action::ToggleMouse`]
    /// last set afterwards. The event loop mirrors it onto the terminal, so
    /// this is also what the terminal is doing.
    pub(crate) fn mouse_on(&self) -> bool {
        self.mouse_on
    }

    /// Whether OSC 8 hyperlinks may be emitted.
    pub(crate) fn osc8_enabled(&self) -> bool {
        match self.config.links.osc8 {
            Osc8Mode::Never => false,
            Osc8Mode::Always => true,
            Osc8Mode::Auto => self.caps.osc8,
        }
    }

    /// Widths of the two sidebars as `(toc, hints)`, in columns.
    ///
    /// The single source of truth for the horizontal split, shared by
    /// [`App::content_width`], `events::areas` and the mouse
    /// handler, so the width the document is laid out at is always exactly
    /// the width it is drawn at.
    ///
    /// Narrow terminals: the hints sidebar is only shown while at
    /// least [`crate::app::hints::MIN_DOCUMENT_WIDTH`] columns are left for
    /// the document. When both sidebars are open and there is not room for
    /// both, the hints go first — the TOC is navigation, the hints are
    /// discoverability.
    pub(crate) fn sidebar_widths(&self) -> (u16, u16) {
        let total = self.size.0.max(1);
        let toc = if self.toc.open && total > 12 {
            self.toc.width(total)
        } else {
            0
        };
        let hints = if self.hints.open {
            let want = self.hints.width(total);
            let rest = total.saturating_sub(toc).saturating_sub(want);
            if rest >= crate::app::hints::MIN_DOCUMENT_WIDTH {
                want
            } else {
                0
            }
        } else {
            0
        };
        (toc, hints)
    }

    /// Columns the TOC sidebar has for its entries: its width less the
    /// border it draws on its right edge. Zero when the sidebar is closed.
    pub(crate) fn toc_inner_width(&self) -> usize {
        usize::from(self.sidebar_widths().0.saturating_sub(1))
    }

    /// Columns left over for the document once both sidebars took theirs.
    ///
    /// This is the space the document is centred in, not necessarily the
    /// width it is laid out at — see [`App::content_width`].
    pub(crate) fn content_available(&self) -> usize {
        let total = self.size.0.max(1);
        let (toc, hints) = self.sidebar_widths();
        usize::from(total.saturating_sub(toc).saturating_sub(hints)).max(1)
    }

    /// Width of the document area in columns (excludes both sidebars).
    ///
    /// Both `--width` and `[max_width]` only ever narrow: the layout width is
    /// the smallest of the available columns and whatever limits are set, so
    /// a limit wider than the terminal is a no-op rather than a horizontal
    /// scroll nobody asked for.
    pub(crate) fn content_width(&self) -> usize {
        let available = self.content_available();
        let mut width = available;
        if let Some(w) = self.width_override {
            width = width.min(usize::from(w.max(1)));
        }
        if self.config.max_width > 0 {
            width = width.min(usize::from(self.config.max_width));
        }
        width.max(1)
    }

    /// Columns of empty space left of the document.
    ///
    /// Zero unless `center` is on and the document is narrower than the space
    /// it has. The leftover column of an odd remainder goes to the right, so
    /// the two margins differ by at most one cell.
    pub(crate) fn content_margin(&self) -> usize {
        if !self.config.center {
            return 0;
        }
        self.content_available()
            .saturating_sub(self.content_width())
            / 2
    }

    /// Height of the document area in rows (excludes status and prompt rows).
    pub(crate) fn content_height(&self) -> usize {
        usize::from(self.size.1)
            .saturating_sub(usize::from(self.chrome_rows()))
            .max(1)
    }

    /// Number of rows reserved at the bottom for the status bar and prompt.
    pub(crate) fn chrome_rows(&self) -> u16 {
        if matches!(self.mode, Mode::Search | Mode::Command) {
            2
        } else {
            1
        }
    }

    /// Scroll percentage for the status bar, computed from the render tree.
    pub(crate) fn percent(&self) -> u8 {
        crate::util::viewport::percent(self.top_line, self.content_height(), self.tree.len())
    }

    /// 1-based number of the *last* line visible in the viewport.
    ///
    /// This is what the status bar shows next to the percentage, because
    /// [`App::percent`] is also computed from the bottom line — `less` reports
    /// the percentage of the bottom line displayed, and mixing a bottom-line
    /// percentage with a top-line counter produced the nonsensical `31% 1/60`.
    pub(crate) fn bottom_line(&self) -> usize {
        if self.tree.is_empty() {
            0
        } else {
            (self.top_line + self.content_height().max(1)).min(self.tree.len())
        }
    }

    /// 1-based number of the line at the top of the viewport.
    #[cfg(test)]
    pub(crate) fn current_line(&self) -> usize {
        if self.tree.is_empty() {
            0
        } else {
            self.top_line + 1
        }
    }

    /// Line index of the semantic cursor.
    ///
    /// The cursor is stored as a [`NodeId`], not as a line, so it survives
    /// re-layout, folding and resizing. Plain scrolling moves it to
    /// the node at the top of the viewport; heading, TOC, search and anchor
    /// jumps place it on their target even when the viewport had to be clamped
    /// at the end of the document.
    pub(crate) fn cursor_line(&self) -> usize {
        self.tree
            .first_line_of(self.cursor)
            .unwrap_or_else(|| self.top_line.min(self.tree.len().saturating_sub(1)))
    }

    /// The node at the cursor.
    pub(crate) fn cursor_node(&self) -> Option<NodeId> {
        if self.tree.is_empty() {
            return None;
        }
        if self.tree.first_line_of(self.cursor).is_some() {
            Some(self.cursor)
        } else {
            self.tree.node_at(self.top_line)
        }
    }

    /// The foldable unit containing the cursor: the target of the fold
    /// actions, and what the outline's current-entry marker follows.
    ///
    /// For Markdown that is the section the cursor is in; for JSON and YAML
    /// the container it sits in. Asking the document rather than matching on
    /// the format is what makes `za` mean the analogous thing everywhere.
    pub(crate) fn current_fold(&self) -> Option<FoldId> {
        let node = self.cursor_node()?;
        self.doc.fold_at(node)
    }

    /// What this document supports; the key hints and the help overlay ask
    /// before offering an action.
    pub(crate) fn capabilities(&self) -> DocumentCapabilities {
        self.doc.capabilities()
    }

    /// Which format the document was read as, for the status line.
    pub(crate) fn format(&self) -> DocumentKind {
        self.doc.kind()
    }

    /// The semantic path of the node at the cursor, for the formats that have
    /// one.
    pub(crate) fn cursor_path(&self) -> Option<DocumentPath> {
        self.doc.path(self.cursor_node()?)
    }

    // -- scrolling --------------------------------------------------------

    fn max_top(&self) -> usize {
        max_top_line(self.tree.len(), self.content_height())
    }

    fn clamp_h_offset(&mut self) {
        let max_h = self.tree.max_width().saturating_sub(self.content_width());
        self.h_offset = self.h_offset.min(max_h);
    }

    /// Scroll to an absolute line, clamped to the document.
    pub(crate) fn scroll_to(&mut self, line: usize) {
        self.top_line = line.min(self.max_top());
        self.sync_anchor();
    }

    /// Scroll by `delta` lines (negative scrolls up), clamped at both ends.
    pub(crate) fn scroll_by(&mut self, delta: isize) {
        let next = self.top_line as isize + delta;
        let clamped = next.clamp(0, self.max_top() as isize);
        self.top_line = clamped.max(0) as usize;
        self.sync_anchor();
    }

    /// Scroll horizontally by `delta` columns, clamped to the widest line.
    pub(crate) fn scroll_h(&mut self, delta: isize) {
        let max_h = self.tree.max_width().saturating_sub(self.content_width()) as isize;
        let next = (self.h_offset as isize + delta).clamp(0, max_h.max(0));
        self.h_offset = next.max(0) as usize;
    }

    /// Make sure `line` is inside the viewport, scrolling as little as needed.
    pub(crate) fn reveal_line(&mut self, line: usize) {
        let height = self.content_height();
        if line < self.top_line {
            self.scroll_to(line);
        } else if line >= self.top_line + height {
            self.scroll_to(line + 1 - height);
        }
    }

    /// Place `line` `HEADING_CONTEXT` rows below the top of the viewport so
    /// that the preceding context stays visible.
    pub(crate) fn scroll_with_context(&mut self, line: usize) {
        self.scroll_to(line.saturating_sub(HEADING_CONTEXT));
        if let Some(node) = self.tree.node_at(line) {
            self.place_cursor(node);
        }
    }

    // -- messages ---------------------------------------------------------

    /// Show a transient message in the status line.
    pub(crate) fn set_message(&mut self, text: impl Into<String>) {
        self.message = Some(text.into());
    }

    /// Clear the transient message.
    pub(crate) fn clear_message(&mut self) {
        self.message = None;
    }
}

/// Shared fixtures for the tests of every `state` submodule.
///
/// It lives here rather than in each file so that the five test modules keep
/// exercising the *same* document and the same construction path.
#[cfg(test)]
pub(super) mod test_support {
    use super::*;
    use crate::testing;
    use crossterm::event::{KeyCode, KeyModifiers};

    pub(super) const DOC: &str = concat!(
        "Lead paragraph with a [top](https://top.example) link.\n\n",
        "# Title\n\nIntro paragraph with a [link](https://example.com) and more words.\n\n",
        "## Alpha\n\nAlpha body text mentioning needle once.\n\n",
        "### Alpha Child\n\nChild body text.\n\n",
        "## Beta\n\nBeta body text with needle again.\n\n",
        "See [Alpha](#alpha) for details.\n",
    );

    /// The detected capabilities in tests report no mouse, so a test that
    /// wants mouse events must say so — and, like the startup path, start
    /// with reporting on.
    pub(super) fn with_mouse(mut app: App) -> App {
        app.caps.mouse = true;
        app.mouse_on = true;
        app
    }

    pub(super) fn app_with(src: &str, size: (u16, u16)) -> App {
        testing::app_sized(src, size)
    }

    pub(super) fn app() -> App {
        app_with(DOC, (80, 12))
    }

    /// The Markdown model behind a test app.
    ///
    /// Tests that assert on headings, anchors or links are asking a
    /// Markdown-specific question, which is exactly what
    /// [`DocumentModel::as_markdown`] is for. Naming it once here keeps the
    /// unwrap out of every call site.
    pub(super) fn md(app: &App) -> &crate::document::markdown::Document {
        app.doc.as_markdown().expect("a Markdown test document")
    }

    pub(super) fn key(app: &mut App, c: char) {
        app.handle_key(&testing::key(c));
    }

    pub(super) fn code(app: &mut App, c: KeyCode) {
        app.handle_key(&testing::key_mod(c, KeyModifiers::NONE));
    }
}

#[cfg(test)]
mod tests {
    use crate::app::state::test_support::*;
    use crate::config::actions::Action;
    use crate::testing;

    #[test]
    fn scrolling_clamps_at_both_ends() {
        let mut a = app();
        for _ in 0..200 {
            key(&mut a, 'j');
        }
        let max = a.tree().len().saturating_sub(a.content_height());
        assert_eq!(a.top_line(), max);
        for _ in 0..500 {
            key(&mut a, 'k');
        }
        assert_eq!(a.top_line(), 0);
    }

    #[test]
    fn max_width_caps_the_layout_width_and_center_splits_the_rest() {
        let mut cfg = crate::config::Config {
            max_width: 60,
            center: false,
            ..crate::config::Config::default()
        };
        let mut a = testing::AppBuilder::new(DOC)
            .size((100, 12))
            .config(cfg.clone())
            .build();
        assert_eq!(a.content_available(), 100);
        assert_eq!(a.content_width(), 60);
        // Turned off: the document keeps the left edge.
        assert_eq!(a.content_margin(), 0);

        cfg.center = true;
        a = testing::AppBuilder::new(DOC)
            .size((100, 12))
            .config(cfg.clone())
            .build();
        // Equal margins; the odd column, if any, goes right.
        assert_eq!(a.content_margin(), 20);
        assert_eq!(
            a.content_available() - a.content_width() - a.content_margin(),
            20
        );

        // An odd remainder differs by one cell, never more.
        let a = testing::AppBuilder::new(DOC)
            .size((101, 12))
            .config(cfg.clone())
            .build();
        assert_eq!(a.content_margin(), 20);
        assert_eq!(
            a.content_available() - a.content_width() - a.content_margin(),
            21
        );

        // A limit wider than the terminal only ever narrows: no margins, no
        // horizontal scroll invented out of nothing.
        cfg.max_width = 500;
        let a = testing::AppBuilder::new(DOC)
            .size((100, 12))
            .config(cfg)
            .build();
        assert_eq!(a.content_width(), 100);
        assert_eq!(a.content_margin(), 0);
    }

    #[test]
    fn max_width_and_width_override_both_narrow() {
        let cfg = crate::config::Config {
            max_width: 70,
            ..crate::config::Config::default()
        };
        let a = testing::AppBuilder::new(DOC)
            .size((100, 12))
            .config(cfg)
            .width_override(Some(50))
            .build();
        assert_eq!(a.content_width(), 50, "the smaller of the two wins");
    }

    #[test]
    fn page_and_half_page_arithmetic() {
        let mut a = app();
        let height = a.content_height();
        a.apply(Action::PageDown);
        assert_eq!(a.top_line(), height.min(a.tree().len() - height));
        a.apply(Action::Top);
        a.apply(Action::HalfPageDown);
        assert_eq!(a.top_line(), (height / 2).max(1));
        a.apply(Action::HalfPageUp);
        assert_eq!(a.top_line(), 0);
    }

    #[test]
    fn bottom_and_top_are_reachable() {
        let mut a = app();
        a.apply(Action::Bottom);
        assert_eq!(a.top_line(), a.tree().len() - a.content_height());
        assert_eq!(a.percent(), 100);
        a.apply(Action::Top);
        assert_eq!(a.top_line(), 0);
    }

    #[test]
    fn horizontal_scroll_is_clamped_to_the_widest_line() {
        let mut a = app_with("| a | b |\n|---|---|\n| 1 | 2 |\n", (20, 10));
        for _ in 0..50 {
            a.apply(Action::ScrollRight);
        }
        let max = a.tree().max_width().saturating_sub(a.content_width());
        assert_eq!(a.h_offset(), max);
        for _ in 0..50 {
            a.apply(Action::ScrollLeft);
        }
        assert_eq!(a.h_offset(), 0);
    }

    #[test]
    fn percentage_matches_the_viewport() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/readme.md"),
        )
        .expect("fixture");
        let a = app_with(&source, (80, 24));
        let expected = crate::util::viewport::percent(0, a.content_height(), a.tree().len());
        assert_eq!(a.percent(), expected);
        assert_eq!(
            expected,
            ((a.content_height() * 100) / a.tree().len()).min(100) as u8,
            "the status bar reports how much of the document is already shown"
        );
    }

    #[test]
    fn percentage_and_position_come_from_the_render_tree() {
        let mut a = app();
        assert_eq!(a.current_line(), 1);
        a.apply(Action::Bottom);
        assert_eq!(a.percent(), 100);
        assert_eq!(a.current_line(), a.top_line() + 1);
    }

    #[test]
    fn the_status_counter_and_the_percentage_describe_the_same_line() {
        // The bar used to mix a bottom-line percentage with a top-line
        // counter, so a freshly opened document read `31%  1/60`.
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/readme.md"),
        )
        .expect("fixture");
        let mut a = app_with(&source, (80, 24));
        assert!(a.tree().len() > a.content_height(), "the doc scrolls");
        for top in [0usize, 3, 10] {
            a.scroll_to(top);
            let bottom = a.bottom_line();
            assert_eq!(
                bottom,
                (a.top_line() + a.content_height()).min(a.tree().len())
            );
            let expected = ((bottom * 100) / a.tree().len()).min(100) as u8;
            assert_eq!(
                a.percent(),
                expected,
                "the counter {bottom}/{} and the percentage must agree",
                a.tree().len()
            );
        }
        a.apply(Action::Bottom);
        assert_eq!(a.bottom_line(), a.tree().len());
        assert_eq!(a.percent(), 100);
    }

    #[test]
    fn osc8_follows_the_capability_and_the_config() {
        let mut a = app();
        a.caps.osc8 = false;
        assert!(!a.osc8_enabled());
        a.caps.osc8 = true;
        assert!(a.osc8_enabled());
        a.config.links.osc8 = crate::config::schema::Osc8Mode::Never;
        assert!(!a.osc8_enabled());
        a.caps.osc8 = false;
        a.config.links.osc8 = crate::config::schema::Osc8Mode::Always;
        assert!(a.osc8_enabled());
    }

    #[test]
    fn empty_document_does_not_break_navigation() {
        let mut a = app_with("", (80, 10));
        for action in Action::ALL {
            a.apply(*action);
            if a.should_quit() {
                a.quit = false;
            }
        }
        assert_eq!(a.top_line(), 0);
    }
}
