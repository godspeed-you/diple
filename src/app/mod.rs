//! Interactive application: state, event loop, navigation, search UI and TOC.
//!
//! Owned by Workstream D. [`Action`], the bindable-action vocabulary, lives in
//! [`crate::config::actions`] and is re-exported here so that the Phase 1
//! interface contract paths keep resolving.
//!
//! * `state` — [`App`], the layout cache and every action handler,
//! * [`events`] — the crossterm event loop and the draw pass (including image
//!   and OSC 8 painting, which ratatui cannot do),
//! * `adapters` — the conversions between the workstream boundary enums,
//! * `diagrams` — the Mermaid → layout adapter with its render cache and
//!   image registry,
//! * `workspace` — the open documents: tabs, the two panes of a split and
//!   the focus between them,
//! * `paths` — filesystem completion and resolution for `:open`,
//! * `toc` / `search_ui` — sidebar and search prompt state,
//! * `hints` — the right-hand key hints sidebar and its context rules.

pub(crate) mod adapters;
pub(crate) mod command;
pub(crate) mod diagrams;
pub mod events;
pub(crate) mod hints;
pub(crate) mod paths;
pub(crate) mod search_ui;
pub(crate) mod state;
pub(crate) mod toc;
pub(crate) mod workspace;

pub use crate::config::actions::{self, Action};
pub use adapters::{color_level, render_environment, resolve_theme};
pub use diagrams::DiagramProvider;
pub use state::{App, AppEnv, AppOptions, Mode};
pub use workspace::Workspace;

/// Build the diagram provider for `doc`.
///
/// Documents without a Mermaid block skip every bit of Mermaid work,
/// including the `mmdc` `PATH` probe — that shortcut is the startup budget,
/// and it has to hold for a document opened with `:open` just as it does for
/// the one named on the command line.
pub fn diagram_provider(
    doc: &crate::document::DocumentModel,
    config: &crate::config::Config,
    caps: &crate::terminal::Capabilities,
    width: usize,
) -> DiagramProvider {
    use crate::document::markdown::NodeKind;
    // Only Markdown has Mermaid fences, so a JSON or YAML document skips the
    // probe for the same reason a Markdown document without a diagram does.
    let has_diagrams = doc.as_markdown().is_some_and(|markdown| {
        markdown
            .walk()
            .any(|node| matches!(node.kind, NodeKind::Mermaid(_)))
    });
    if !has_diagrams {
        return DiagramProvider::source_only();
    }
    let env = render_environment(config, caps, width, true);
    DiagramProvider::new(crate::mermaid::select_backend(&config.mermaid, &env))
}
