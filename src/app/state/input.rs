//! Key and mouse input, and the [`Action`] dispatcher they both feed
//!
//! The boundary here is *events in, one `Action` out, then dispatch*. Modal
//! keys are consumed first — the `/` prompt, the TOC pane and the help
//! overlay each read the raw event before the key map ever sees it — and only
//! in [`Mode::Normal`] does the key map get a chance to resolve a binding,
//! possibly after buffering a prefix.
//!
//! [`App::apply`] is the single place where an [`Action`] becomes an effect,
//! whatever produced it: a key, a mouse click on a hint row, or a caller. It
//! is deliberately a flat, exhaustive `match`, so the compiler reports a new
//! action variant that nobody handles. Most arms are one call into
//! [`navigate`](super::navigate) or [`sidebars`](super::sidebars), which is
//! the cost of this split: reading what an action *does* takes a hop into
//! another file.
//!
//! Mouse handling is here for the same reason — a click is resolved to a
//! screen region and then turned into the same actions a key produces, so
//! there is exactly one behaviour to keep consistent.

use super::navigate::FoldOp;
use super::{App, HelpKind, Mode};
use crate::app::workspace::Request;
use crate::config::actions::Action;
use crate::config::keys::KeyMatch;
use crate::config::settings;
use crate::render::primitives::LineKind;
use crate::render::terminal::{HintLine, KeyHintsSidebar};

impl App {
    // -- key handling -----------------------------------------------------

    /// Feed one key event. Sub-modes consume keys before the key map.
    pub(crate) fn handle_key(&mut self, event: &crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
        if event.kind == KeyEventKind::Release {
            return;
        }
        self.clear_message();

        // Ctrl-C always quits, whatever the mode — and whatever else is
        // open: it is the panic key, not a way to close one document.
        if event.code == KeyCode::Char('c') && event.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            self.request(Request::QuitAll);
            return;
        }

        // Alt-1 … Alt-9 select a tab by its number. Handled here rather than
        // as nine bindable actions: it is one gesture with an argument, and
        // `[keys]` maps names to actions, not to arguments.
        if event.modifiers.contains(KeyModifiers::ALT) {
            if let KeyCode::Char(c @ '1'..='9') = event.code {
                if matches!(self.mode, Mode::Normal | Mode::Message) {
                    self.mode = Mode::Normal;
                    let index = c as usize - '1' as usize;
                    self.request(Request::GotoTab(index));
                    return;
                }
            }
        }

        if self.mode == Mode::Search {
            self.handle_search_key(event);
            return;
        }
        if self.mode == Mode::Command {
            self.handle_command_key(event);
            return;
        }
        if self.mode == Mode::Message {
            self.mode = Mode::Normal;
        }

        match self.keymap.feed(event) {
            KeyMatch::Action(action) => {
                self.pending.clear();
                self.apply(action);
            }
            KeyMatch::Pending => {
                self.pending = self
                    .keymap
                    .pending()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("");
            }
            KeyMatch::None => {
                self.pending.clear();
                // Unhandled keys are ignored silently, except the sub-mode
                // conveniences below.
                if self.mode == Mode::Toc {
                    self.handle_toc_key(event);
                } else if self.mode == Mode::Help {
                    self.handle_help_key(event);
                }
            }
        }
    }

    fn handle_search_key(&mut self, event: &crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        match event.code {
            KeyCode::Esc => {
                self.restore_search_folds();
                self.search_folds = None;
                self.search.query = self.search.saved.clone();
                self.search.refresh(self.doc.search_index());
                self.mode = Mode::Normal;
                self.prepare_frame();
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                self.search_folds = None;
                if self.search.has_matches() {
                    self.goto_current_match();
                } else if !self.search.query.is_empty() {
                    self.set_message(format!("pattern not found: {}", self.search.query));
                }
            }
            KeyCode::Backspace => {
                self.search.query.pop();
                self.refresh_search_preview();
            }
            KeyCode::Char(c)
                if !event
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.search.query.push(c);
                self.refresh_search_preview();
            }
            _ => {}
        }
    }

    /// The `:` line editor.
    ///
    /// Like the search prompt, this bypasses the key map entirely: while a
    /// line is being typed every printable key is text, not a binding.
    fn handle_command_key(&mut self, event: &crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        match event.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.command.open();
            }
            KeyCode::Enter => {
                self.mode = Mode::Normal;
                let line = self.command.line.clone();
                self.run_command(&line);
                self.command.open();
            }
            KeyCode::Tab => self.command.complete(),
            KeyCode::Backspace => {
                // Backspacing past the `:` leaves, so the key that opened the
                // prompt is also the one that closes an empty one.
                if !self.command.backspace() {
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Char(c)
                if !event
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.command.push(c);
            }
            _ => {}
        }
    }

    /// Execute one typed command line.
    pub(crate) fn run_command(&mut self, line: &str) {
        use crate::app::command::{parse, Command};
        match parse(line) {
            Command::Empty => {}
            Command::Quit => self.quit = true,
            Command::Close => self.request(Request::Close),
            Command::QuitAll => {
                self.quit = true;
                self.request(Request::QuitAll);
            }
            Command::Open(target, path) => self.request(Request::Open { target, path }),
            Command::Invalid(message) => self.set_message(message),
            Command::Help => {
                self.help_kind = HelpKind::Settings;
                self.help_scroll = 0;
                self.mode = Mode::Help;
            }
            Command::Show(key) => match settings::read(&self.config, &key) {
                Some(value) => self.set_message(format!("{key} = {value}")),
                None => self.set_message(format!("unknown setting: {key}")),
            },
            Command::Unknown(key) => self.unknown_setting(&key),
            Command::Set(key, value) => match settings::write(&mut self.config, &key, &value) {
                Ok(effect) => {
                    self.apply_effect(effect);
                    let shown = settings::read(&self.config, &key).unwrap_or(value);
                    self.set_message(format!("{key} = {shown}"));
                }
                Err(message) if settings::get(&key).is_none() => {
                    // The same wrong key reads the same whether or not a
                    // value was typed after it.
                    let _ = message;
                    self.unknown_setting(&key);
                }
                Err(message) => self.set_message(message),
            },
        }
    }

    /// One wording for a name no setting has, wherever it was typed.
    fn unknown_setting(&mut self, key: &str) {
        self.set_message(format!("unknown setting: {key} (`:help` lists them)"));
    }

    /// Bring the rest of the application in line with a changed setting.
    ///
    /// Everything a key can reach is re-derived here rather than at the call
    /// site, so a new setting only has to name its [`Effect`] to take hold.
    fn apply_effect(&mut self, effect: settings::Effect) {
        use settings::Effect;
        match effect {
            Effect::Redraw => {}
            Effect::Relayout => self.invalidate(),
            Effect::Palette => {
                self.color = crate::app::color_level(self.config.color, &self.caps);
                self.theme = crate::app::resolve_theme(&self.config.theme, self.color);
                // The syntax highlighting is baked into the tree with the old
                // palette, so the tree has to go with it.
                self.invalidate();
            }
            Effect::Sidebar => {
                if self.toc.open != self.config.toc {
                    // The outline is derived lazily, so asking whether it is
                    // empty before building it would always say yes and the
                    // setting would never take hold.
                    if self.config.toc {
                        self.toc.ensure(&self.doc);
                    }
                    self.toc.open = self.config.toc && !self.toc.is_empty();
                    self.toc.h_scroll = 0;
                }
                self.hints.open = self.config.key_hints;
                if self.mode == Mode::Toc && !self.toc.open {
                    self.mode = Mode::Normal;
                }
                self.invalidate();
            }
            Effect::Mouse => self.mouse_on = self.config.mouse && self.caps.mouse,
        }
        self.ensure_layout();
    }

    fn handle_toc_key(&mut self, event: &crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        let height = self.content_height();
        match event.code {
            KeyCode::Char('j') | KeyCode::Down => self.toc.move_selection(1, height),
            KeyCode::Char('k') | KeyCode::Up => self.toc.move_selection(-1, height),
            KeyCode::Home => self.toc.select(0, height),
            KeyCode::End => self.toc.select(usize::MAX, height),
            _ => {}
        }
    }

    fn handle_help_key(&mut self, event: &crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        match event.code {
            KeyCode::Char('j') | KeyCode::Down => self.help_scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => {
                self.help_scroll = self.help_scroll.saturating_sub(1)
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.help_scroll = 0;
            }
            _ => {}
        }
    }

    // -- actions ----------------------------------------------------------

    /// Execute one [`Action`]. Every variant does something real.
    pub(crate) fn apply(&mut self, action: Action) {
        let height = self.content_height();
        match action {
            Action::Quit => match self.mode {
                Mode::Help | Mode::Toc => {
                    self.close_overlays();
                }
                _ => self.quit = true,
            },
            Action::Cancel => self.close_overlays(),
            Action::ScrollDown => match self.mode {
                Mode::Toc => self.toc.move_selection(1, height),
                Mode::Help => self.help_scroll += 1,
                _ => self.scroll_by(1),
            },
            Action::ScrollUp => match self.mode {
                Mode::Toc => self.toc.move_selection(-1, height),
                Mode::Help => self.help_scroll = self.help_scroll.saturating_sub(1),
                _ => self.scroll_by(-1),
            },
            Action::PageDown => match self.mode {
                Mode::Toc => self.toc.move_selection(height as isize, height),
                Mode::Help => self.help_scroll += height,
                _ => self.scroll_by(height as isize),
            },
            Action::PageUp => match self.mode {
                Mode::Toc => self.toc.move_selection(-(height as isize), height),
                Mode::Help => self.help_scroll = self.help_scroll.saturating_sub(height),
                _ => self.scroll_by(-(height as isize)),
            },
            Action::HalfPageDown => self.scroll_by((height / 2).max(1) as isize),
            Action::HalfPageUp => self.scroll_by(-((height / 2).max(1) as isize)),
            // In the sidebar the arrows belong to the sidebar: the reader is
            // looking at a heading that does not fit, not at the document.
            Action::ScrollLeft => match self.mode {
                Mode::Toc => self.toc.scroll_h(-8, self.toc_inner_width()),
                _ => self.scroll_h(-8),
            },
            Action::ScrollRight => match self.mode {
                Mode::Toc => self.toc.scroll_h(8, self.toc_inner_width()),
                _ => self.scroll_h(8),
            },
            Action::Top => match self.mode {
                Mode::Toc => self.toc.select(0, height),
                Mode::Help => self.help_scroll = 0,
                _ => self.scroll_to(0),
            },
            Action::Bottom => match self.mode {
                Mode::Toc => self.toc.select(usize::MAX, height),
                _ => self.scroll_to(usize::MAX),
            },
            Action::Search => self.open_search(),
            Action::NextSearch => self.cycle_search(true),
            Action::PreviousSearch => self.cycle_search(false),
            Action::NextHeading => self.jump_structural(true),
            Action::PreviousHeading => self.jump_structural(false),
            Action::NextHeadingSameLevel => self.jump_sibling(true),
            Action::PreviousHeadingSameLevel => self.jump_sibling(false),
            Action::ToggleToc => self.toggle_toc(),
            Action::ToggleKeyHints => self.toggle_key_hints(),
            Action::ToggleMouse => self.toggle_mouse(),
            Action::FocusOtherPane => self.request(Request::FocusOtherPane),
            Action::NextTab => self.request(Request::NextTab),
            Action::PreviousTab => self.request(Request::PreviousTab),
            Action::Activate => self.activate(),
            Action::OpenLink => self.open_selected_link(),
            Action::NextLink => self.cycle_link(true),
            Action::PreviousLink => self.cycle_link(false),
            Action::ToggleFold => self.fold_current(FoldOp::Toggle),
            Action::CollapseFold => self.fold_current(FoldOp::Collapse),
            Action::ExpandFold => self.fold_current(FoldOp::Expand),
            Action::CollapseAll => {
                self.folds.collapse_all();
                self.after_fold_change();
                let (_, units) = self.doc.kind().fold_unit();
                self.set_message(format!("all {units} collapsed"));
            }
            Action::ExpandAll => {
                self.folds.expand_all();
                self.after_fold_change();
                let (_, units) = self.doc.kind().fold_unit();
                self.set_message(format!("all {units} expanded"));
            }
            Action::CommandPrompt => {
                self.command.open();
                self.mode = Mode::Command;
                self.clear_message();
            }
            Action::Help => {
                self.mode = if self.mode == Mode::Help {
                    Mode::Normal
                } else {
                    Mode::Help
                };
                self.help_kind = HelpKind::Keys;
                self.help_scroll = 0;
            }
            Action::ToggleMermaidSource => self.toggle_mermaid_source(),
            Action::ParentNode => self.jump_parent(),
            Action::FirstChild => self.jump_first_child(),
        }
        self.ensure_layout();
    }

    fn close_overlays(&mut self) {
        match self.mode {
            Mode::Help => {
                self.mode = Mode::Normal;
                self.help_scroll = 0;
            }
            Mode::Toc => {
                self.mode = Mode::Normal;
                self.toc.open = false;
                self.invalidate();
            }
            Mode::Search | Mode::Command => {
                self.mode = Mode::Normal;
            }
            _ => {
                if self.search.has_matches() {
                    self.search.clear();
                    self.prepare_frame();
                } else {
                    self.selected_link = None;
                }
            }
        }
        self.clear_message();
    }

    // -- mouse ------------------------------------------------------------

    /// Handle a mouse event; ignored unless `mouse` is configured and the
    /// terminal supports it.
    pub(crate) fn handle_mouse(&mut self, event: &crossterm::event::MouseEvent) {
        use crossterm::event::MouseEventKind;
        if !self.mouse_on {
            return;
        }
        match event.kind {
            MouseEventKind::ScrollDown => self.scroll_by(3),
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::ScrollLeft => self.scroll_h(-4),
            MouseEventKind::ScrollRight => self.scroll_h(4),
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                self.click(event.column, event.row)
            }
            _ => return,
        }
        self.ensure_layout();
    }

    fn click(&mut self, column: u16, row: u16) {
        let (sidebar, hints) = self.sidebar_widths();
        if usize::from(row) >= self.content_height() {
            return; // status/prompt row
        }
        if hints > 0 && column >= self.size.0.max(1).saturating_sub(hints) {
            // The hints sidebar is a column of buttons: the row the reader
            // clicked names its own action, so run it. Titles and the blank
            // separators are inert.
            if let Some(action) = self.hint_row_action(row) {
                self.apply(action);
            }
            return;
        }
        if sidebar > 0 && column < sidebar {
            let index = self.toc.scroll + usize::from(row);
            if index < self.toc.len() {
                let height = self.content_height();
                self.toc.select(index, height);
                self.mode = Mode::Toc;
                self.toc_jump();
            }
            return;
        }
        // Clicks in the centring margin belong to no line.
        let margin = self.content_margin();
        let column = usize::from(column).saturating_sub(usize::from(sidebar));
        if column < margin || column >= margin + self.content_width() {
            return;
        }
        let line_index = self.top_line + usize::from(row);
        let Some(line) = self.tree.lines.get(line_index) else {
            return;
        };
        let node = line.node;
        let kind = line.kind.clone();
        // Column inside the document, accounting for the sidebar, the
        // centring margin and the h-scroll.
        let target_col = self.h_offset + column - margin;
        let mut col = 0usize;
        let mut clicked_link = None;
        for span in &line.spans {
            let width = span.width();
            if target_col < col + width {
                clicked_link = span.link;
                break;
            }
            col += width;
        }
        if let Some(id) = clicked_link {
            self.selected_link = Some(id);
            if let Some(link) = self.link(id) {
                self.set_message(format!("link: {}", link.url));
            }
            return;
        }
        // Clicking a row that stands for a foldable unit toggles it: a
        // Markdown heading, a JSON or YAML container, or the single row a
        // collapsed one leaves behind.
        let foldable_row = matches!(
            kind,
            LineKind::Heading(_) | LineKind::Structural(_) | LineKind::FoldedMarker
        );
        if foldable_row {
            if let Some(fold) = self.doc.fold_at(node) {
                self.folds.toggle(fold);
                if let Some(own) = self.doc.fold_node(fold) {
                    self.anchor = (own, 0);
                    self.cursor = own;
                }
                self.after_one_fold(fold);
            }
        }
    }

    /// The action a click on hints-sidebar row `row` triggers, if any.
    ///
    /// Recomputed from the current state rather than remembered from the last
    /// frame: the sidebar's content is a pure function of the state, so the
    /// two can never disagree.
    fn hint_row_action(&self, row: u16) -> Option<Action> {
        let (_, width) = self.sidebar_widths();
        if width == 0 {
            return None;
        }
        let groups = self.hint_groups();
        let inner = usize::from(width.saturating_sub(1)).max(1);
        let rows = KeyHintsSidebar {
            groups: &groups,
            theme: &self.theme,
            level: self.color,
            unicode: self.caps.unicode_box,
        }
        .rows(inner, self.content_height());
        match rows.get(usize::from(row)).map(|(_, kind)| *kind) {
            Some(HintLine::Row(action)) => Some(action),
            _ => None,
        }
    }

    // -- help -------------------------------------------------------------

    /// Help entries with the *actual* current bindings, so custom keybindings
    /// are reflected — for the actions the open document can use. A JSON
    /// document has no links to follow and no diagrams to toggle, and help
    /// that offers them is help about some other document (spec AC-20).
    pub(crate) fn help_entries(&self) -> Vec<(String, String)> {
        let caps = self.doc.capabilities();
        let markdown = self.doc.as_markdown().is_some();
        Action::ALL
            .iter()
            .filter(|action| match action {
                Action::OpenLink | Action::NextLink | Action::PreviousLink => caps.links,
                Action::ToggleMermaidSource => markdown,
                Action::NextHeading | Action::PreviousHeading => caps.hierarchy_navigation,
                Action::NextHeadingSameLevel
                | Action::PreviousHeadingSameLevel
                | Action::ParentNode
                | Action::FirstChild => caps.sibling_navigation,
                Action::ToggleFold
                | Action::CollapseFold
                | Action::ExpandFold
                | Action::CollapseAll
                | Action::ExpandAll => caps.folding,
                Action::ToggleToc => caps.outline,
                _ => true,
            })
            .map(|action| {
                let keys = self.keymap.bindings_for(*action);
                let keys = if keys.is_empty() {
                    "(unbound)".to_string()
                } else {
                    keys.join(", ")
                };
                (keys, action.description().to_string())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::test_support::*;
    use crate::config::keys::KeyMap;
    use crate::config::schema::KeyBinding;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    #[test]
    fn clicking_a_hint_row_runs_its_action() {
        let mut a = with_mouse(app_with(DOC, (120, 24)));
        a.apply(Action::ToggleKeyHints);
        let (_, hints) = a.sidebar_widths();
        assert!(hints > 0);
        let column = a.size().0 - hints + 2; // past the left border
        let groups = a.hint_groups();
        let rows = KeyHintsSidebar {
            groups: &groups,
            theme: &a.theme,
            level: a.color,
            unicode: a.caps.unicode_box,
        }
        .rows(usize::from(hints - 1), a.content_height());
        let toc_row = rows
            .iter()
            .position(|(_, k)| *k == HintLine::Row(Action::ToggleToc))
            .expect("the `contents` row is on screen");

        a.handle_mouse(&crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column,
            row: toc_row as u16,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert!(a.toc.open, "clicking the `contents` row opened the TOC");

        // A group title is inert.
        a.handle_mouse(&crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column,
            row: 0,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
        assert_eq!(a.mode(), Mode::Toc, "the title row did nothing");
    }

    #[test]
    fn help_lists_custom_bindings() {
        let binding = KeyBinding::One("x".to_string());
        let keymap = KeyMap::from_overrides([("quit", &binding)]).expect("valid override");
        let mut a = crate::testing::AppBuilder::new(DOC).keymap(keymap).build();
        let entries = a.help_entries();
        let quit = entries
            .iter()
            .find(|(_, d)| d == Action::Quit.description())
            .expect("quit entry");
        assert_eq!(quit.0, "x", "help shows the override, not the default");
        a.apply(Action::Help);
        assert_eq!(a.mode(), Mode::Help);
        // `q` closes the overlay rather than quitting when it is bound away.
        key(&mut a, 'x');
        assert!(!a.should_quit(), "quit closes the overlay first");
        assert_eq!(a.mode(), Mode::Normal);
    }

    #[test]
    fn unknown_keys_are_ignored_and_prefixes_are_shown() {
        let mut a = app();
        let before = a.top_line();
        key(&mut a, 'Z');
        assert_eq!(a.top_line(), before);
        assert_eq!(a.pending(), "");
        key(&mut a, 'z');
        assert_eq!(a.pending(), "z", "pending prefix is displayed");
        key(&mut a, 'M');
        assert_eq!(a.pending(), "");
        assert!(a.tree().to_plain_text().contains('▶') || a.tree().len() < 20);
    }

    #[test]
    fn ctrl_c_quits_from_any_mode() {
        let mut a = app();
        a.apply(Action::Help);
        a.handle_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(a.should_quit());
    }

    #[test]
    fn mouse_wheel_scrolls_when_enabled() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mut a = with_mouse(app());
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        a.handle_mouse(&wheel);
        assert_eq!(a.top_line(), 3);

        // Reporting off: the events belong to the terminal, which is what
        // makes selecting text with the mouse work again.
        a.apply(Action::ToggleMouse);
        assert!(!a.mouse_on());
        a.handle_mouse(&wheel);
        assert_eq!(a.top_line(), 3, "the wheel is the terminal's now");

        a.apply(Action::ToggleMouse);
        assert!(a.mouse_on());
        a.apply(Action::Top);
        let before = a.tree().len();
        let heading_row = a
            .tree()
            .heading_lines()
            .first()
            .map(|(line, _, _)| *line as u16)
            .expect("a heading");
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: heading_row,
            modifiers: KeyModifiers::NONE,
        };
        a.handle_mouse(&click);
        assert!(a.tree().len() < before, "clicking the H1 collapsed it");
    }

    #[test]
    fn clicks_land_on_the_document_through_the_centring_margin() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

        let cfg = crate::config::Config {
            max_width: 60,
            center: true,
            mouse: true,
            ..crate::config::Config::default()
        };
        let mut a = crate::testing::AppBuilder::new(DOC)
            .size((100, 24))
            .config(cfg)
            .build();
        a = with_mouse(a);
        let margin = a.content_margin();
        assert_eq!(margin, 20);

        let heading_row = a
            .tree()
            .heading_lines()
            .first()
            .map(|(line, _, _)| *line as u16)
            .expect("a heading");
        let click = |column: u16| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: heading_row,
            modifiers: KeyModifiers::NONE,
        };

        // Inside the left margin: no line lives there.
        let before = a.tree().len();
        a.handle_mouse(&click(margin as u16 - 1));
        assert_eq!(a.tree().len(), before, "the margin swallowed the click");

        // The first document column is the first cell after the margin.
        a.handle_mouse(&click(margin as u16));
        assert!(a.tree().len() < before, "clicking the heading collapsed it");

        // And the right margin is just as inert.
        let before = a.tree().len();
        a.handle_mouse(&click((margin + a.content_width()) as u16));
        assert_eq!(a.tree().len(), before);
    }

    #[test]
    fn help_overlay_scrolls_and_closes() {
        let mut a = app();
        a.apply(Action::Help);
        assert_eq!(a.mode(), Mode::Help);
        a.apply(Action::ScrollDown);
        a.apply(Action::ScrollDown);
        assert_eq!(a.help_scroll(), 2);
        assert_eq!(a.top_line(), 0, "the document did not scroll");
        a.apply(Action::ScrollUp);
        assert_eq!(a.help_scroll(), 1);
        a.apply(Action::Cancel);
        assert_eq!(a.mode(), Mode::Normal);
        assert_eq!(a.help_scroll(), 0);
    }

    /// The `:` line is typed, not bound: every printable key is text, Tab
    /// completes it, and Enter applies it to the live configuration.
    #[test]
    fn the_command_line_types_completes_and_applies() {
        let mut a = app_with(DOC, (120, 24));
        assert!(a.config.center, "the default this test then turns off");

        key(&mut a, ':');
        assert_eq!(a.mode(), Mode::Command);
        // `j` is a binding in normal mode and a letter in this one.
        for c in "ce".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Tab);
        assert_eq!(a.command.prompt(), ":center = ", "completed and separated");
        for c in "false".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Enter);

        assert_eq!(a.mode(), Mode::Normal);
        assert!(!a.config.center, "the setting took hold");
        assert_eq!(a.content_margin(), 0, "and the layout followed it");
        assert_eq!(a.message(), Some("center = false"));
        assert!(
            a.command.line.is_empty(),
            "the line is cleared for next time"
        );
    }

    /// Reading is not writing: a bare key reports, an unknown key says so and
    /// a refused value leaves the configuration alone.
    #[test]
    fn the_command_line_reports_and_refuses() {
        let mut a = app_with(DOC, (120, 24));

        a.run_command("max_width");
        assert_eq!(a.message(), Some("max_width = 160"));

        a.run_command("max_width = wide");
        assert_eq!(a.message(), Some("max_width takes 0.."));
        assert_eq!(a.config.max_width, 160, "a refused value changes nothing");

        a.run_command("centre = true");
        assert_eq!(
            a.message(),
            Some("unknown setting: centre (`:help` lists them)")
        );

        a.run_command("max_width = 100");
        assert_eq!(a.config.max_width, 100);
        assert_eq!(a.content_width(), 100, "the document was laid out again");
    }

    /// A setting is not just a field: the parts of the program it feeds have
    /// to follow it, which is what `Effect` is for.
    #[test]
    fn a_setting_takes_hold_where_it_is_used() {
        let mut a = with_mouse(app_with(DOC, (120, 24)));

        a.run_command("theme = crt");
        assert_eq!(a.theme.name, "crt", "the palette was rebuilt");

        a.run_command("toc = true");
        assert!(a.toc.open, "the sidebar opened");
        assert!(a.sidebar_widths().0 > 0);
        a.run_command("toc = false");
        assert!(!a.toc.open);

        a.run_command("mouse = false");
        assert!(!a.mouse_on(), "reporting followed the setting");
        a.run_command("mouse = true");
        assert!(a.mouse_on());

        a.run_command("wrap = false");
        assert!(!a.config.wrap);
    }

    /// `:help` shows the settings, `?` the keys, and both use the same
    /// overlay — so the one that was asked for must be the one drawn.
    #[test]
    fn the_two_helps_do_not_collide() {
        let mut a = app_with(DOC, (120, 24));
        a.run_command("help");
        assert_eq!(a.mode(), Mode::Help);
        assert_eq!(a.help_kind(), HelpKind::Settings);

        a.apply(Action::Cancel);
        a.apply(Action::Help);
        assert_eq!(a.help_kind(), HelpKind::Keys);
        let keys: Vec<String> = a.help_entries().into_iter().map(|(k, _)| k).collect();
        assert!(keys.iter().any(|k| k == ":"), "`:` is a documented key");
    }

    /// Backspacing past the `:` leaves the prompt, and Esc abandons whatever
    /// was typed without applying any of it.
    #[test]
    fn the_command_line_can_be_left_without_applying() {
        let mut a = app_with(DOC, (120, 24));
        key(&mut a, ':');
        code(&mut a, KeyCode::Backspace);
        assert_eq!(a.mode(), Mode::Normal, "an empty line closes");

        key(&mut a, ':');
        for c in "center = false".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.mode(), Mode::Normal);
        assert!(a.config.center, "Esc applied nothing");
        assert!(a.command.line.is_empty());
    }

    /// `zM`/`zR` report what they folded in the same word the key hints
    /// beside them use, which depends on the format.
    #[test]
    fn help_offers_only_what_the_document_can_do() {
        let described =
            |a: &App| -> Vec<String> { a.help_entries().into_iter().map(|(_, d)| d).collect() };
        let markdown = described(&app());
        let json = described(&crate::testing::json_app(r#"{"a": [1]}"#));
        for action in [
            Action::OpenLink,
            Action::NextLink,
            Action::ToggleMermaidSource,
        ] {
            let d = action.description().to_string();
            assert!(markdown.contains(&d), "Markdown offers {d}");
            assert!(!json.contains(&d), "JSON does not offer {d}");
        }
        for action in [Action::ToggleFold, Action::ParentNode, Action::ToggleToc] {
            assert!(json.contains(&action.description().to_string()));
        }
    }

    #[test]
    fn fold_all_messages_name_what_the_format_folds() {
        let mut a = app();
        a.apply(Action::CollapseAll);
        assert_eq!(a.message(), Some("all sections collapsed"));
        a.apply(Action::ExpandAll);
        assert_eq!(a.message(), Some("all sections expanded"));

        for mut a in [
            crate::testing::json_app(r#"{"a": {"b": 1}, "c": [1, 2]}"#),
            crate::testing::yaml_app("a:\n  b: 1\nc:\n  - 1\n  - 2\n"),
        ] {
            a.apply(Action::CollapseAll);
            assert_eq!(a.message(), Some("all containers collapsed"));
            a.apply(Action::ExpandAll);
            assert_eq!(a.message(), Some("all containers expanded"));
        }
    }
}
