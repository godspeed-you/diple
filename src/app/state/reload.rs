//! Reading the document again from its file, without losing the reader's
//! place.
//!
//! # In place, not a new view
//!
//! A reload swaps the document under an existing [`App`] rather than building
//! a new one. Everything that belongs to the *view* — the sidebars, the
//! settings typed at `:`, the mouse, a half-typed command line, the help
//! overlay — therefore simply stays, and only what is keyed to the document
//! has to be carried over: the viewport anchor, the cursor, the collapsed
//! folds, the current search match, the outline selection and the Mermaid
//! source toggles. [`Relocation`] says where each of those went.
//!
//! # A failed reload changes nothing
//!
//! A file that cannot be read, or a JSON file caught halfway through an edit,
//! leaves the version on screen exactly as it was and says why in the status
//! line. That is what makes `watch` safe to leave on while editing: the
//! reader sees the last version that parsed until the next one does.
//!
//! # The format stays
//!
//! The document is read again as the format it was read as the first time,
//! never detected afresh. Content detection on a file in the middle of an
//! edit could otherwise turn a broken JSON file into a Markdown document.

use super::{App, Mode};
use crate::app::source_file::Change;
use crate::document::{DocumentModel, FormatRequest, Relocation, SourceDocument};
use crate::layout::Layout;

impl App {
    /// The file the document was read from, if it came from one.
    pub(crate) fn source_path(&self) -> Option<&std::path::Path> {
        self.source.as_ref().map(|source| source.path())
    }

    /// Read the document again from its file (`R`, `:reload`).
    ///
    /// On failure the document on screen is untouched and the reason is
    /// returned, worded for the status line.
    pub(crate) fn reload(&mut self) -> Result<(), String> {
        let Some(source) = self.source.as_mut() else {
            return Err("stdin cannot be reloaded: there is no file to read again".to_string());
        };
        // The status line already names the file, so the messages do not:
        // a path repeated there pushed the reason itself off a narrow line.
        let text = source.read().map_err(|e| format!("not reloaded: {e}"))?;
        let request = FormatRequest::Fixed(self.doc.kind());
        let doc = crate::document::load(request, SourceDocument::new(self.filename.clone(), text))
            .map_err(|error| {
                // Short on purpose. The status line gives way to the JSON or
                // YAML path when a message would crowd it out, and the reader
                // needs to know *that* the screen is stale more than the
                // parser's prose; where to look is enough to fix the file.
                let at = error
                    .position
                    .map(|p| format!(" at {}:{}", p.line, p.column))
                    .unwrap_or_default();
                format!("not reloaded: invalid {}{at}", error.format.label())
            })?
            .model;
        self.replace_document(doc);
        Ok(())
    }

    /// Look at the file once for `watch`, and read it again if it changed.
    ///
    /// Returns `true` when something on screen changed — the document, or a
    /// message about it.
    ///
    /// Nothing happens while the reader is typing at the `/` or the `:`
    /// prompt: the search preview holds fold and viewport state keyed to the
    /// document, and a document changing under a half-typed command would be
    /// a surprise. The change is not lost; it is picked up at the first check
    /// after the prompt closes.
    pub(crate) fn watch_tick(&mut self) -> bool {
        if matches!(self.mode, Mode::Search | Mode::Command) {
            return false;
        }
        let Some(source) = self.source.as_mut() else {
            return false;
        };
        match source.check() {
            Change::None | Change::Settling => false,
            Change::Vanished => {
                self.set_message("file is gone; showing the last version until it is back");
                true
            }
            Change::Changed => {
                match self.reload() {
                    Ok(()) => self.set_message("file changed, reloaded"),
                    Err(message) => self.set_message(message),
                }
                true
            }
        }
    }

    /// Put `doc` in place of the current document, keeping the reader where
    /// they were.
    pub(crate) fn replace_document(&mut self, doc: DocumentModel) {
        let relocation = Relocation::new(&self.doc, &doc);

        // What the reader had, in terms of the old document.
        let (anchor, offset) = self.anchor;
        let cursor = self.cursor;
        let top_line = self.top_line;
        let collapsed: Vec<_> = (0..self.folds.len())
            .filter(|fold| self.folds.is_collapsed(*fold))
            .collect();
        let toc_node = self.toc.selected_node();
        let search_node = self.search.current_match().map(|m| m.node);

        let diagrams =
            crate::app::diagram_provider(&doc, &self.config, &self.caps, usize::from(self.size.0));
        diagrams.inherit(&self.diagrams, &self.doc, &doc, &relocation);
        self.diagrams = diagrams;
        self.diagram_generation += 1;
        self.doc = doc;
        // The highlighting cache is keyed by node, and node numbers now mean
        // something else.
        self.layout = Layout::new();

        self.folds = self.doc.fold_state();
        for fold in collapsed.into_iter().filter_map(|f| relocation.fold(f)) {
            self.folds.collapse(fold);
        }

        // A selected link is an index into the old document's link list.
        self.selected_link = None;
        // Only ever set while the `/` prompt is open, which a reload waits
        // for; cleared anyway, because they describe the old document.
        self.search_folds = None;
        self.search_origin = None;
        if !self.search.query.is_empty() {
            self.search.refresh(self.doc.search_index());
            if let Some(node) = search_node.and_then(|n| relocation.node(n)) {
                self.search.select_near(node);
            }
        }

        self.toc.invalidate();
        if self.toc.open {
            self.toc.ensure(&self.doc);
            let height = self.content_height();
            if self.toc.is_empty() {
                self.toc.open = false;
                if self.mode == Mode::Toc {
                    self.mode = Mode::Normal;
                }
            } else {
                let index = toc_node
                    .and_then(|n| relocation.node(n))
                    .and_then(|n| self.toc.index_of_node(n))
                    .unwrap_or(self.toc.selected);
                self.toc.select(index.min(self.toc.len() - 1), height);
            }
        }

        let anchored = relocation.node(anchor);
        self.anchor = (anchored.unwrap_or(0), offset);
        self.cursor = relocation
            .node(cursor)
            .or_else(|| self.doc.first_semantic())
            .unwrap_or(0);
        self.invalidate();
        self.ensure_layout();
        if anchored.is_none() {
            // Nothing in common with the old document — another format, or
            // nothing left of it: stay at the same line rather than jumping
            // to the top.
            self.scroll_to(top_line);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::app::state::test_support::*;
    use crate::app::state::Mode;
    use crate::config::actions::Action;
    use crate::testing;
    use std::fs;
    use std::path::PathBuf;

    /// An app reading `text` from a real file it can read again.
    fn app_on_file(
        name: &str,
        text: &str,
        size: (u16, u16),
    ) -> (tempfile::TempDir, PathBuf, crate::app::App) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(name);
        fs::write(&path, text).expect("write");
        let doc = match name.rsplit('.').next() {
            Some("json") => testing::json_model(text),
            Some("yaml") => testing::yaml_model(text),
            _ => testing::model(text),
        };
        let app = testing::AppBuilder::over(doc, name)
            .size(size)
            .source(&path)
            .build();
        (dir, path, app)
    }

    fn top_text(app: &crate::app::App) -> String {
        app.tree()
            .visible_slice(app.top_line(), 1)
            .first()
            .map(|line| line.spans.iter().map(|s| s.text.as_str()).collect())
            .unwrap_or_default()
    }

    fn long_doc(prefix: &str) -> String {
        let mut text = String::from(prefix);
        for section in ["One", "Two", "Three", "Four"] {
            text.push_str(&format!("# {section}\n\n"));
            for n in 0..6 {
                text.push_str(&format!("{section} paragraph {n}.\n\n"));
            }
        }
        text
    }

    #[test]
    fn stdin_cannot_be_reloaded_and_says_so() {
        let mut a = app();
        let before = a.tree().len();
        let error = a.reload().expect_err("no file behind it");
        assert!(error.contains("cannot be reloaded"), "{error}");
        assert_eq!(a.tree().len(), before, "nothing changed");
        assert!(!a.watch_tick(), "and there is nothing to watch");
    }

    #[test]
    fn reloading_shows_the_new_text_and_keeps_the_reader_on_the_same_line_of_text() {
        let (_dir, path, mut a) = app_on_file("doc.md", &long_doc(""), (60, 10));
        // Read down to the middle of section Three.
        while !top_text(&a).contains("Three paragraph 2") {
            a.apply(Action::ScrollDown);
        }
        // Someone adds a paragraph to section One, above the reader.
        fs::write(&path, long_doc("A new introduction.\n\n")).expect("write");
        a.reload().expect("reload");
        assert!(
            top_text(&a).contains("Three paragraph 2"),
            "the same text is at the top: {:?}",
            top_text(&a)
        );
    }

    #[test]
    fn collapsed_sections_stay_collapsed_and_new_ones_open() {
        let (_dir, path, mut a) = app_on_file("doc.md", "# A\n\na\n\n# B\n\nb\n", (60, 20));
        a.apply(Action::CollapseAll);
        fs::write(&path, "# New\n\nn\n\n# A\n\na\n\n# B\n\nb\n").expect("write");
        a.reload().expect("reload");
        assert!(!a.folds.is_collapsed(0), "a new section starts open");
        assert!(a.folds.is_collapsed(1), "A");
        assert!(a.folds.is_collapsed(2), "B");
    }

    #[test]
    fn a_search_runs_again_over_the_new_text() {
        let (_dir, path, mut a) = app_on_file("doc.md", "# A\n\nneedle\n", (60, 20));
        key(&mut a, '/');
        for c in "needle".chars() {
            key(&mut a, c);
        }
        code(&mut a, crossterm::event::KeyCode::Enter);
        assert_eq!(a.search.matches.len(), 1);
        fs::write(&path, "# A\n\nneedle\n\nanother needle\n").expect("write");
        a.reload().expect("reload");
        assert_eq!(a.search.matches.len(), 2, "the query was run again");
    }

    #[test]
    fn a_file_that_no_longer_parses_leaves_the_old_version_on_screen() {
        let (_dir, path, mut a) = app_on_file("data.json", r#"{"a": 1}"#, (60, 20));
        let before = a.doc.node_count();
        fs::write(&path, r#"{"a": 1, "b": "#).expect("write");
        let error = a.reload().expect_err("broken JSON");
        assert!(
            error.starts_with("not reloaded: invalid JSON at 1:"),
            "{error}"
        );
        assert_eq!(a.doc.node_count(), before);

        fs::write(&path, r#"{"a": 1, "b": 2}"#).expect("write");
        a.reload().expect("fixed");
        assert_eq!(a.doc.node_count(), before + 1);
    }

    #[test]
    fn a_reload_keeps_the_format_the_document_was_read_as() {
        // A `.txt` file read as YAML must not turn into Markdown when it is
        // read again, whatever detection would say about the new content.
        let (_dir, path, mut a) = app_on_file("notes.yaml", "a: 1\n", (60, 20));
        fs::write(&path, "# just a comment\n").expect("write");
        a.reload().expect("valid YAML");
        assert!(a.doc.as_structured().is_some());
    }

    #[test]
    fn a_json_cursor_follows_its_key() {
        let (_dir, path, mut a) = app_on_file("data.json", r#"{"a": 1, "b": 2}"#, (60, 20));
        let breadcrumb = |a: &crate::app::App| a.cursor_path().map(|p| p.breadcrumb(false));
        a.cursor = 2; // 0 root, 1 a, 2 b
        assert_eq!(breadcrumb(&a).as_deref(), Some("b"));
        fs::write(&path, r#"{"new": 0, "a": 1, "b": 2}"#).expect("write");
        a.reload().expect("reload");
        assert_eq!(
            breadcrumb(&a).as_deref(),
            Some("b"),
            "not `a`, which is node 2 now"
        );
    }

    #[test]
    fn watching_reloads_once_the_change_has_settled() {
        let (_dir, path, mut a) = app_on_file("doc.md", "# A\n", (60, 20));
        assert!(!a.watch_tick(), "unchanged");
        fs::write(&path, "# A\n\n# B\n").expect("write");
        assert!(!a.watch_tick(), "first sight: wait for the write to finish");
        assert!(a.watch_tick(), "settled: reloaded");
        assert_eq!(a.message(), Some("file changed, reloaded"));
        assert_eq!(a.folds.len(), 2);
        assert!(!a.watch_tick(), "and only once");
    }

    #[test]
    fn watching_waits_while_a_prompt_is_open() {
        let (_dir, path, mut a) = app_on_file("doc.md", "# A\n", (60, 20));
        fs::write(&path, "# A\n\n# B\n").expect("write");
        key(&mut a, ':');
        assert_eq!(a.mode(), Mode::Command);
        assert!(!a.watch_tick());
        assert!(!a.watch_tick());
        assert_eq!(a.folds.len(), 1, "not under the reader's typing");
        code(&mut a, crossterm::event::KeyCode::Esc);
        a.watch_tick();
        assert!(a.watch_tick());
        assert_eq!(a.folds.len(), 2, "picked up once the prompt closed");
    }

    #[test]
    fn a_vanished_file_is_reported_and_its_return_reloads() {
        let (_dir, path, mut a) = app_on_file("doc.md", "# A\n", (60, 20));
        fs::remove_file(&path).expect("remove");
        a.watch_tick();
        assert!(a.watch_tick());
        assert!(a.message().is_some_and(|m| m.contains("is gone")));
        assert_eq!(a.folds.len(), 1, "the last version stays");
        fs::write(&path, "# A\n\n# B\n").expect("write");
        a.watch_tick();
        assert!(a.watch_tick());
        assert_eq!(a.folds.len(), 2);
    }

    #[test]
    fn the_outline_follows_the_new_document() {
        let (_dir, path, mut a) = app_on_file("doc.md", "# A\n\n# B\n", (80, 20));
        a.apply(Action::ToggleToc);
        assert_eq!(a.toc.len(), 2);
        fs::write(&path, "# A\n\n# B\n\n# C\n").expect("write");
        a.reload().expect("reload");
        assert_eq!(a.toc.len(), 3, "derived again, not left stale");
        assert!(a.toc.open);
    }
}
