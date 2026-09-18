//! Shared test-support vocabulary for the crate's unit tests.
//!
//! # Why this module exists
//!
//! Almost every `#[cfg(test)]` block in `diple` needs the same three or four
//! values: a parsed [`Document`], a [`Theme`], a [`LayoutOptions`] and — in
//! `app` — a fully wired [`App`]. Spelled out by hand at every call site,
//! those literals are what makes the suite rot: adding a field to [`AppEnv`]
//! or [`AppOptions`] means editing every `App::new` literal, which is exactly
//! what happened when the key hints sidebar was added.
//!
//! # Why it is not a cargo feature
//!
//! It is declared as `#[cfg(test)] pub(crate) mod testing;` in `lib.rs`, so it
//! is compiled only when the library is built *as a test target*. A cargo
//! feature would be enableable by any downstream `--features` flag and would
//! ship these helpers in a release build; `cfg(test)` cannot be turned on from
//! outside. Verified with
//! `cargo build --release && nm -C target/release/diple | grep -i testing`,
//! which finds nothing.
//!
//! # Boundary
//!
//! Integration tests in `tests/` link the library as an *external* crate,
//! built without `cfg(test)`, so they cannot see this module. Their shared
//! helpers live in `tests/common/mod.rs`.
//!
//! # How to use it
//!
//! Reach for these helpers when a test needs a *plausible* value. A test that
//! is about an unusual [`Config`], an exotic capability set or a hand-built
//! [`RenderLine`](crate::render::primitives::RenderLine) should keep building
//! that value visibly at the call site — the point of the helper is to hide
//! the boilerplate, never the thing under test.

// A shared vocabulary is allowed to offer a word this particular test binary
// happens not to use; `dead_code` here would only push tests back to writing
// the literal out by hand.
#![allow(dead_code)]

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::diagrams::DiagramProvider;
use crate::app::state::{App, AppEnv, AppOptions};
use crate::config::keys::KeyMap;
use crate::config::Config;
use crate::document::markdown::{parse, Document};
use crate::document::source::SourceDocument;
use crate::document::{json, yaml, DocumentModel};
use crate::layout::{Layout, LayoutOptions};
use crate::render::primitives::RenderTree;
use crate::render::theme::{ColorLevel, Theme};
use crate::terminal::capabilities::{detect_from, Capabilities, CapabilityOverrides, TerminalSize};

/// Parse Markdown into a [`Document`].
pub(crate) fn doc(src: &str) -> Document {
    parse(src)
}

/// Parse Markdown into a [`DocumentModel`] — what everything above the format
/// boundary actually takes.
pub(crate) fn model(src: &str) -> DocumentModel {
    DocumentModel::markdown(parse(src))
}

/// Parse JSON into a [`DocumentModel`], panicking on invalid input.
pub(crate) fn json_model(src: &str) -> DocumentModel {
    DocumentModel::structured(
        json::parse(&SourceDocument::new("test.json", src)).expect("valid JSON"),
    )
}

/// Parse YAML into a [`DocumentModel`], panicking on invalid input.
pub(crate) fn yaml_model(src: &str) -> DocumentModel {
    DocumentModel::structured(
        yaml::parse(&SourceDocument::new("test.yaml", src)).expect("valid YAML"),
    )
}

/// The theme every layout helper here uses.
///
/// `Theme::dark()` is the built-in default and is deterministic, so tests that
/// only need *a* theme should take this one rather than pick their own.
pub(crate) fn theme() -> Theme {
    Theme::dark()
}

/// Default layout options at `width`, for the given theme.
///
/// Separate from [`render`] because a test that needs to tweak one option
/// still wants the rest of the defaults.
pub(crate) fn options(width: usize, theme: &Theme) -> LayoutOptions<'_> {
    LayoutOptions::new(width, theme)
}

/// Parse and lay out `src` at `width` with default options.
pub(crate) fn render(src: &str, width: usize) -> RenderTree {
    let document = model(src);
    let theme = theme();
    Layout::build(&document, &options(width, &theme))
}

/// Parse, lay out and serialise `src` at `width` — what the reader sees on the
/// non-interactive path.
pub(crate) fn plain(src: &str, width: usize) -> String {
    render(src, width).to_plain_text()
}

/// A synthetic key press of a character, with no modifiers.
pub(crate) fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// A synthetic key press of an arbitrary code and modifier set.
pub(crate) fn key_mod(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

/// Environment variables as the capability detector wants them.
pub(crate) fn env_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// Capabilities detected from a synthetic environment.
///
/// This is the *pure* detector ([`detect_from`]), so it never reads the
/// developer's own environment and never queries a terminal.
pub(crate) fn caps(pairs: &[(&str, &str)]) -> Capabilities {
    detect_from(
        &env_map(pairs),
        TerminalSize::default(),
        &CapabilityOverrides::default(),
    )
}

/// Builder for a test [`App`].
///
/// Every field has a plausible default; a test overrides only the ones it is
/// actually about. This is the single place that knows the shape of
/// [`AppEnv`] and [`AppOptions`], so a new field on either is a one-line
/// change here rather than an edit in every test that builds an `App`.
pub(crate) struct AppBuilder {
    doc: DocumentModel,
    config: Config,
    keymap: KeyMap,
    caps: Capabilities,
    theme: Theme,
    color: ColorLevel,
    diagrams: DiagramProvider,
    options: AppOptions,
}

impl AppBuilder {
    /// A builder over the parsed `src`, with default configuration, the
    /// default keymap, no terminal capabilities and no diagram backend.
    pub(crate) fn new(src: &str) -> Self {
        Self::over(model(src), "test.md")
    }

    /// A builder over an already-built document of any format.
    pub(crate) fn over(doc: DocumentModel, filename: &str) -> Self {
        Self {
            doc,
            config: Config::default(),
            keymap: KeyMap::with_defaults(),
            caps: Capabilities::default(),
            theme: theme(),
            color: ColorLevel::None,
            diagrams: DiagramProvider::source_only(),
            options: AppOptions {
                filename: filename.to_string(),
                size: (80, 24),
                width_override: None,
                debug: false,
            },
        }
    }

    /// Terminal size in `(columns, rows)`.
    pub(crate) fn size(mut self, size: (u16, u16)) -> Self {
        self.options.size = size;
        self
    }

    /// Detected terminal capabilities.
    pub(crate) fn caps(mut self, caps: Capabilities) -> Self {
        self.caps = caps;
        self
    }

    /// Diagram backend.
    pub(crate) fn diagrams(mut self, diagrams: DiagramProvider) -> Self {
        self.diagrams = diagrams;
        self
    }

    /// Effective configuration.
    pub(crate) fn config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    /// Active key bindings.
    pub(crate) fn keymap(mut self, keymap: KeyMap) -> Self {
        self.keymap = keymap;
        self
    }

    /// Colour level and, with it, the theme the app renders through.
    pub(crate) fn color(mut self, color: ColorLevel) -> Self {
        self.color = color;
        self
    }

    /// `--width`: pin the layout width regardless of the terminal size.
    pub(crate) fn width_override(mut self, width: Option<u16>) -> Self {
        self.options.width_override = width;
        self
    }

    /// Build the application.
    pub(crate) fn build(self) -> App {
        App::new(
            self.doc,
            self.config,
            self.keymap,
            AppEnv {
                caps: self.caps,
                theme: self.theme,
                color: self.color,
                diagrams: self.diagrams,
            },
            self.options,
        )
    }
}

/// An [`App`] over `src` at the default 80×24 size.
pub(crate) fn app(src: &str) -> App {
    AppBuilder::new(src).build()
}

/// An [`App`] over `src` at a given terminal size.
pub(crate) fn app_sized(src: &str, size: (u16, u16)) -> App {
    AppBuilder::new(src).size(size).build()
}

/// An [`App`] over JSON `src` at the default 80×24 size.
pub(crate) fn json_app(src: &str) -> App {
    AppBuilder::over(json_model(src), "test.json").build()
}

/// An [`App`] over YAML `src` at the default 80×24 size.
pub(crate) fn yaml_app(src: &str) -> App {
    AppBuilder::over(yaml_model(src), "test.yaml").build()
}
