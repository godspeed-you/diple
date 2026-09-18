//! The key hints sidebar.
//!
//! The mirror image of [`crate::app::toc`]: a sidebar on the *right* edge
//! listing the keyboard commands that are available **right now**. It is not a
//! static copy of the help overlay — the groups and rows are selected from the
//! current [`Mode`] and from the cursor context ([`HintContext`]), so a reader
//! only ever sees keys that would actually do something if pressed.
//!
//! Every key label is taken from the live [`KeyMap`] via
//! [`KeyMap::bindings_for`], exactly as the help overlay does, so custom
//! `[keys]` bindings are reflected. An action the user has unbound produces no
//! row at all rather than a row with an empty key.

use crate::app::state::Mode;
use crate::config::actions::Action;
use crate::config::keys::KeyMap;
use crate::document::{DocumentCapabilities, DocumentKind};
use crate::render::terminal::{HintGroup, HintRow};

/// Default sidebar width in columns, capped to a third of the screen.
pub(crate) const DEFAULT_WIDTH: u16 = 30;

/// Minimum document width kept free of sidebars (narrow terminals
/// must still render readably). Below it the hints sidebar hides itself; when
/// both sidebars are open the hints go first, because the TOC is navigation
/// and the hints are only discoverability.
pub(crate) const MIN_DOCUMENT_WIDTH: u16 = 40;

/// Sidebar visibility.
///
/// Deliberately far simpler than [`crate::app::toc::TocState`]: the hints
/// sidebar has no selection and no scroll offset — it never scrolls, it fits
/// itself (see [`crate::render::terminal::fit_hint_groups`]).
#[derive(Debug, Clone, Default)]
pub(crate) struct HintsState {
    /// Whether the sidebar is drawn.
    pub(crate) open: bool,
}

impl HintsState {
    /// Sidebar width for a screen of `total` columns.
    pub(crate) fn width(&self, total: u16) -> u16 {
        DEFAULT_WIDTH
            .min(total / 3)
            .max(if total >= 16 { 16 } else { total })
    }
}

/// Everything the selector needs to know about the current context.
///
/// Kept as plain data so the rules can be unit-tested without an [`crate::app::state::App`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HintContext {
    /// The current interaction mode.
    pub(crate) mode: Mode,
    /// The current view can actually scroll horizontally.
    pub(crate) can_scroll_horizontally: bool,
    /// What the document being read supports.
    ///
    /// This is what makes the sidebar honest across formats: a key whose
    /// action means nothing for this document is not offered, rather than
    /// offered and then answered with a status message.
    pub(crate) capabilities: DocumentCapabilities,
    /// Which format the document was read as, which decides the wording — a
    /// Markdown reader looks for headings, a YAML reader for structure.
    pub(crate) format: DocumentKind,
    /// The visible region contains at least one link.
    pub(crate) link_in_view: bool,
    /// The cursor is on a structural row (so `Enter` folds instead of
    /// following a link).
    pub(crate) cursor_on_structural: bool,
    /// The cursor is at or near a Mermaid diagram.
    pub(crate) near_diagram: bool,
    /// A search is active and has matches.
    pub(crate) search_active: bool,
    /// The terminal reports mouse events at all, so the toggle means
    /// something.
    pub(crate) mouse_available: bool,
    /// diple is currently asking for those events, so the terminal cannot
    /// select text with the mouse.
    pub(crate) mouse_on: bool,
    /// How many tabs are open.
    pub(crate) tabs: usize,
    /// How many panes the tab being shown has.
    pub(crate) panes: usize,
}

impl Default for HintContext {
    fn default() -> Self {
        Self {
            mode: Mode::Normal,
            can_scroll_horizontally: false,
            capabilities: DocumentCapabilities::MARKDOWN,
            format: DocumentKind::Markdown,
            link_in_view: false,
            cursor_on_structural: false,
            near_diagram: false,
            search_active: false,
            mouse_available: false,
            mouse_on: false,
            tabs: 1,
            panes: 1,
        }
    }
}

/// Build one row, or `None` when the action is unbound.
fn row(keys: &KeyMap, action: Action, label: &str) -> Option<HintRow> {
    let bindings = keys.bindings_for(action);
    if bindings.is_empty() {
        return None;
    }
    Some(HintRow {
        keys: bindings.join("/"),
        label: label.to_string(),
        action,
    })
}

/// Build one row for an opposed pair of actions (`j`/`k`, `]`/`[`, `zc`/`zo`).
///
/// A key-hints panel reads far better — and leaves room for far more groups —
/// when the pairs share a line. The two halves are still resolved
/// independently from the live key map: if the user unbound one of them the
/// row collapses to the half that is still bound *and* to that half's own
/// label, so the sidebar never claims a key that would do nothing. If both are
/// unbound there is no row at all.
///
/// The row carries the first action as its click target.
fn pair(
    keys: &KeyMap,
    (first, first_label): (Action, &str),
    (second, second_label): (Action, &str),
    both_label: &str,
) -> Option<HintRow> {
    // Only the primary binding of each half: the combined form has to stay
    // narrow enough for the label beside it. The help overlay (`?`) remains
    // the complete list.
    let a = keys.bindings_for(first).into_iter().next();
    let b = keys.bindings_for(second).into_iter().next();
    match (a, b) {
        (Some(a), Some(b)) => Some(HintRow {
            keys: format!("{a}/{b}"),
            label: both_label.to_string(),
            action: first,
        }),
        (Some(a), None) => Some(HintRow {
            keys: a,
            label: first_label.to_string(),
            action: first,
        }),
        (None, Some(b)) => Some(HintRow {
            keys: b,
            label: second_label.to_string(),
            action: second,
        }),
        (None, None) => None,
    }
}

/// A group, or `None` when every one of its actions is unbound.
fn group(title: &'static str, priority: u8, rows: Vec<Option<HintRow>>) -> Option<HintGroup> {
    let rows: Vec<HintRow> = rows.into_iter().flatten().collect();
    if rows.is_empty() {
        None
    } else {
        Some(HintGroup {
            title,
            rows,
            priority,
        })
    }
}

/// The groups to show for `ctx`, in display order.
///
/// Drop priorities (higher is dropped first when the terminal is short):
/// `Move` 0, `View` 1, `Headings` 2, `Fold` 3, `Search` 4, `Links` 5,
/// `Diagram` 6, `Documents` 7.
///
/// The ordering is deliberate: what distinguishes diple from `less` is
/// semantic heading navigation and folding, so those outrank the generic
/// pager features. `Move` survives longest because it is what the reader
/// needs every second, and `View` next because it holds `q`, `?` and the key
/// that closes the sidebar again — the way *out*.
pub(crate) fn groups(ctx: &HintContext, keys: &KeyMap) -> Vec<HintGroup> {
    match ctx.mode {
        Mode::Search => search_groups(keys),
        Mode::Command => command_groups(),
        Mode::Toc => toc_groups(keys),
        Mode::Help => help_groups(keys),
        Mode::Normal | Mode::Message => normal_groups(ctx, keys),
    }
}

fn normal_groups(ctx: &HintContext, keys: &KeyMap) -> Vec<HintGroup> {
    let mut move_rows = vec![
        pair(
            keys,
            (Action::ScrollDown, "scroll down"),
            (Action::ScrollUp, "scroll up"),
            "scroll",
        ),
        pair(
            keys,
            (Action::PageDown, "page down"),
            (Action::PageUp, "page up"),
            "page",
        ),
        pair(
            keys,
            (Action::HalfPageDown, "half page down"),
            (Action::HalfPageUp, "half page up"),
            "half page",
        ),
        pair(
            keys,
            (Action::Top, "top"),
            (Action::Bottom, "bottom"),
            "top/bottom",
        ),
    ];
    if ctx.can_scroll_horizontally {
        move_rows.push(pair(
            keys,
            (Action::ScrollLeft, "left"),
            (Action::ScrollRight, "right"),
            "left/right",
        ));
    }

    let mut search_rows = vec![row(keys, Action::Search, "search")];
    if ctx.search_active {
        search_rows.push(pair(
            keys,
            (Action::NextSearch, "next match"),
            (Action::PreviousSearch, "prev match"),
            "next/prev match",
        ));
    }

    // The words differ because the thing does: a Markdown reader folds a
    // section, a JSON or YAML reader folds a container.
    let (unit, units) = match ctx.format {
        DocumentKind::Markdown => ("section", "all sections"),
        _ => ("container", "all containers"),
    };
    let fold = if ctx.capabilities.folding {
        let mut fold_rows = vec![
            row(keys, Action::ToggleFold, &format!("toggle {unit}")),
            pair(
                keys,
                (Action::CollapseFold, "collapse"),
                (Action::ExpandFold, "expand"),
                "collapse/expand",
            ),
            pair(
                keys,
                (Action::CollapseAll, "collapse all"),
                (Action::ExpandAll, "expand all"),
                units,
            ),
        ];
        // `Enter` folds when the cursor is on a structural row and follows a
        // link otherwise — exactly what `App::activate` does, so the row
        // appears in one group or the other, never in both. Its label says
        // what is context-dependent about it: unlike `za`, which always acts
        // on the unit the cursor is *inside*, `Enter` acts on the one the
        // cursor is *on*.
        if ctx.cursor_on_structural {
            fold_rows.insert(0, row(keys, Action::Activate, "fold at cursor"));
        }
        group("Fold", 3, fold_rows)
    } else {
        None
    };

    let links = if ctx.capabilities.links && ctx.link_in_view {
        let mut rows = vec![
            pair(
                keys,
                (Action::NextLink, "next link"),
                (Action::PreviousLink, "prev link"),
                "select",
            ),
            row(keys, Action::OpenLink, "open"),
        ];
        if !ctx.cursor_on_structural {
            rows.push(row(keys, Action::Activate, "follow at cursor"));
        }
        group("Links", 5, rows)
    } else {
        None
    };

    // Markdown headings, JSON and YAML containers: the same two keys, named
    // after whatever the document being read actually has.
    let structure = if ctx.capabilities.hierarchy_navigation {
        let (next, previous) = match ctx.format {
            DocumentKind::Markdown => ("next heading", "prev heading"),
            _ => ("next node", "prev node"),
        };
        let mut rows = vec![pair(
            keys,
            (Action::NextHeading, next),
            (Action::PreviousHeading, previous),
            "next/prev",
        )];
        if ctx.capabilities.sibling_navigation {
            rows.push(pair(
                keys,
                (Action::NextHeadingSameLevel, "next sibling"),
                (Action::PreviousHeadingSameLevel, "prev sibling"),
                "same level",
            ));
            rows.push(pair(
                keys,
                (Action::ParentNode, "out"),
                (Action::FirstChild, "in"),
                "out/in",
            ));
        }
        let title = match ctx.format {
            DocumentKind::Markdown => "Headings",
            _ => "Structure",
        };
        group(title, 2, rows)
    } else {
        None
    };

    let diagram = if ctx.near_diagram {
        group(
            "Diagram",
            6,
            vec![row(keys, Action::ToggleMermaidSource, "source/render")],
        )
    } else {
        None
    };

    // Only worth a group once there is a second document: with one open,
    // every key in it would report that there is nothing to switch to.
    let documents = if ctx.tabs > 1 || ctx.panes > 1 {
        let mut rows = Vec::new();
        if ctx.panes > 1 {
            rows.push(row(keys, Action::FocusOtherPane, "other pane"));
        }
        if ctx.tabs > 1 {
            rows.push(pair(
                keys,
                (Action::NextTab, "next tab"),
                (Action::PreviousTab, "prev tab"),
                "next/prev tab",
            ));
            rows.push(Some(HintRow {
                keys: "alt-1…9".to_string(),
                label: "tab by number".to_string(),
                action: Action::NextTab,
            }));
        }
        group("Documents", 7, rows)
    } else {
        None
    };

    let mut view_rows = vec![
        row(
            keys,
            Action::ToggleToc,
            if ctx.format == DocumentKind::Markdown {
                "contents"
            } else {
                "outline"
            },
        ),
        row(keys, Action::ToggleKeyHints, "hide hints"),
    ];
    // Only worth offering where the terminal reports mouse events at all, and
    // the label says what the key gets you rather than what it sets.
    if ctx.mouse_available {
        let label = if ctx.mouse_on {
            "select text"
        } else {
            "mouse back on"
        };
        view_rows.push(row(keys, Action::ToggleMouse, label));
    }
    view_rows.push(row(keys, Action::Help, "help"));
    // The same key, and a label that says what it will actually do: with a
    // second document open it closes this one rather than the session.
    let quit_label = if ctx.tabs > 1 || ctx.panes > 1 {
        "close document"
    } else {
        "quit"
    };
    view_rows.push(row(keys, Action::Quit, quit_label));

    [
        group("Move", 0, move_rows),
        structure,
        fold,
        group("Search", 4, search_rows),
        links,
        diagram,
        documents,
        group("View", 1, view_rows),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// While the `/` prompt is open the key map is bypassed entirely
/// ([`crate::app::state::App::handle_key`] routes to the prompt handler), so
/// the labels here are the only ones in the sidebar that are not looked up —
/// they are not bindable actions but the prompt's fixed editing keys.
fn search_groups(_keys: &KeyMap) -> Vec<HintGroup> {
    vec![HintGroup {
        title: "Search",
        priority: 0,
        rows: vec![
            HintRow {
                keys: "…".to_string(),
                label: "type pattern".to_string(),
                action: Action::Search,
            },
            HintRow {
                keys: "backspace".to_string(),
                label: "delete".to_string(),
                action: Action::Search,
            },
            HintRow {
                keys: "enter".to_string(),
                label: "confirm".to_string(),
                action: Action::Search,
            },
            HintRow {
                keys: "esc".to_string(),
                label: "cancel".to_string(),
                action: Action::Cancel,
            },
        ],
    }]
}

/// Like [`search_groups`]: while the `:` line has the keys, the key map is
/// bypassed, so these labels name the line editor's own keys rather than
/// bindable actions.
fn command_groups() -> Vec<HintGroup> {
    let row = |keys: &str, label: &str, action| HintRow {
        keys: keys.to_string(),
        label: label.to_string(),
        action,
    };
    vec![HintGroup {
        title: "Command",
        priority: 0,
        rows: vec![
            row("…", "key = value", Action::CommandPrompt),
            row("tab", "complete", Action::CommandPrompt),
            row("enter", "apply", Action::CommandPrompt),
            row(":help", "all settings", Action::CommandPrompt),
            row("esc", "cancel", Action::Cancel),
        ],
    }]
}

fn toc_groups(keys: &KeyMap) -> Vec<HintGroup> {
    [group(
        "Contents",
        0,
        vec![
            pair(
                keys,
                (Action::ScrollDown, "next entry"),
                (Action::ScrollUp, "prev entry"),
                "move",
            ),
            row(keys, Action::Activate, "jump"),
            row(keys, Action::Cancel, "close"),
            row(keys, Action::ToggleToc, "close"),
        ],
    )]
    .into_iter()
    .flatten()
    .collect()
}

fn help_groups(keys: &KeyMap) -> Vec<HintGroup> {
    [group(
        "Help",
        0,
        vec![
            pair(
                keys,
                (Action::ScrollDown, "scroll down"),
                (Action::ScrollUp, "scroll up"),
                "scroll",
            ),
            pair(
                keys,
                (Action::PageDown, "page down"),
                (Action::PageUp, "page up"),
                "page",
            ),
            row(keys, Action::Cancel, "close"),
            row(keys, Action::Quit, "close"),
        ],
    )]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::KeyBinding;
    use std::collections::BTreeMap;

    fn titles(groups: &[HintGroup]) -> Vec<&str> {
        groups.iter().map(|g| g.title).collect()
    }

    fn labels(groups: &[HintGroup], title: &str) -> Vec<String> {
        groups
            .iter()
            .find(|g| g.title == title)
            .map(|g| g.rows.iter().map(|r| r.label.clone()).collect())
            .unwrap_or_default()
    }

    fn keys_for(groups: &[HintGroup], action: Action) -> Option<String> {
        groups
            .iter()
            .flat_map(|g| g.rows.iter())
            .find(|r| r.action == action)
            .map(|r| r.keys.clone())
    }

    #[test]
    fn normal_mode_shows_the_core_groups_in_priority_order() {
        let map = KeyMap::with_defaults();
        let g = groups(&HintContext::default(), &map);
        assert_eq!(
            titles(&g),
            vec!["Move", "Headings", "Fold", "Search", "View"]
        );
        // The two things that distinguish diple from `less` outrank the
        // generic pager features, so they survive a short terminal.
        let by_title = |t: &str| g.iter().find(|x| x.title == t).map(|x| x.priority);
        assert_eq!(by_title("Move"), Some(0));
        assert_eq!(by_title("View"), Some(1));
        assert_eq!(by_title("Headings"), Some(2));
        assert_eq!(by_title("Fold"), Some(3));
        assert_eq!(by_title("Search"), Some(4));
        assert_eq!(keys_for(&g, Action::ToggleKeyHints).as_deref(), Some("K"));
    }

    #[test]
    fn opposed_pairs_share_one_row() {
        let map = KeyMap::with_defaults();
        let g = groups(&HintContext::default(), &map);
        let move_rows: Vec<(String, String)> = g
            .iter()
            .find(|x| x.title == "Move")
            .map(|x| {
                x.rows
                    .iter()
                    .map(|r| (r.keys.clone(), r.label.clone()))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            move_rows,
            vec![
                ("j/k".to_string(), "scroll".to_string()),
                ("pgdn/pgup".to_string(), "page".to_string()),
                ("ctrl-d/ctrl-u".to_string(), "half page".to_string()),
                ("g/G".to_string(), "top/bottom".to_string()),
            ],
            "four rows, not eight"
        );
        assert_eq!(
            labels(&g, "Headings"),
            vec![
                "next/prev".to_string(),
                "same level".to_string(),
                "out/in".to_string(),
            ],
            "parent/child join the pairs spec §12.4 asks the hints to offer"
        );
        assert_eq!(
            g.iter()
                .find(|x| x.title == "Headings")
                .map(|x| x.rows[0].keys.clone()),
            Some("]/[".to_string())
        );
    }

    #[test]
    fn a_pair_with_one_half_unbound_collapses_to_the_bound_half() {
        let mut overrides = BTreeMap::new();
        overrides.insert("scroll_up".to_string(), KeyBinding::Many(Vec::new()));
        overrides.insert("collapse_fold".to_string(), KeyBinding::Many(Vec::new()));
        let map = KeyMap::from_overrides(overrides.iter().map(|(k, v)| (k.as_str(), v)))
            .expect("valid overrides");
        let g = groups(&HintContext::default(), &map);
        let scroll = g
            .iter()
            .flat_map(|x| x.rows.iter())
            .find(|r| r.action == Action::ScrollDown)
            .expect("the bound half survives");
        assert_eq!(scroll.keys, "j", "only the half that still works");
        assert_eq!(scroll.label, "scroll down", "with its own label");
        // The other direction: the *second* half is the survivor.
        let expand = g
            .iter()
            .flat_map(|x| x.rows.iter())
            .find(|r| r.action == Action::ExpandFold)
            .expect("expand survives");
        assert_eq!(expand.keys, "zo");
        assert_eq!(expand.label, "expand");
    }

    #[test]
    fn a_pair_with_both_halves_unbound_produces_no_row() {
        let mut overrides = BTreeMap::new();
        overrides.insert("scroll_left".to_string(), KeyBinding::Many(Vec::new()));
        overrides.insert("scroll_right".to_string(), KeyBinding::Many(Vec::new()));
        let map = KeyMap::from_overrides(overrides.iter().map(|(k, v)| (k.as_str(), v)))
            .expect("valid overrides");
        let g = groups(
            &HintContext {
                can_scroll_horizontally: true,
                ..HintContext::default()
            },
            &map,
        );
        assert_eq!(
            labels(&g, "Move"),
            vec!["scroll", "page", "half page", "top/bottom"],
            "no left/right row at all"
        );
    }

    #[test]
    fn horizontal_scrolling_appears_only_when_it_can_scroll() {
        let map = KeyMap::with_defaults();
        let plain = groups(&HintContext::default(), &map);
        assert!(keys_for(&plain, Action::ScrollLeft).is_none());
        let wide = groups(
            &HintContext {
                can_scroll_horizontally: true,
                ..HintContext::default()
            },
            &map,
        );
        assert_eq!(keys_for(&wide, Action::ScrollLeft).as_deref(), Some("h/l"));
    }

    #[test]
    fn link_rows_appear_only_when_a_link_is_in_view() {
        let map = KeyMap::with_defaults();
        assert!(!titles(&groups(&HintContext::default(), &map)).contains(&"Links"));
        let g = groups(
            &HintContext {
                link_in_view: true,
                ..HintContext::default()
            },
            &map,
        );
        assert!(titles(&g).contains(&"Links"));
        assert_eq!(
            keys_for(&g, Action::NextLink).as_deref(),
            Some("tab/shift-tab")
        );
        assert!(labels(&g, "Links").contains(&"follow at cursor".to_string()));
    }

    #[test]
    fn enter_is_distinguishable_from_za_and_never_shown_twice() {
        let map = KeyMap::with_defaults();
        let g = groups(
            &HintContext {
                cursor_on_structural: true,
                link_in_view: true,
                ..HintContext::default()
            },
            &map,
        );
        let activate: Vec<&HintRow> = g
            .iter()
            .flat_map(|x| x.rows.iter())
            .filter(|r| r.action == Action::Activate)
            .collect();
        assert_eq!(activate.len(), 1, "one Enter row only");
        assert_eq!(activate[0].label, "fold at cursor");
        // `za` acts on the section the cursor is inside; `Enter` on the
        // heading it is on. The two must not read the same.
        let toggle = g
            .iter()
            .flat_map(|x| x.rows.iter())
            .find(|r| r.action == Action::ToggleFold)
            .expect("za");
        assert_eq!(toggle.label, "toggle section");
        assert_ne!(toggle.label, activate[0].label);
        assert!(!labels(&g, "Links").contains(&"follow at cursor".to_string()));
    }

    #[test]
    fn diagram_and_search_rows_are_contextual() {
        let map = KeyMap::with_defaults();
        let plain = groups(&HintContext::default(), &map);
        assert!(!titles(&plain).contains(&"Diagram"));
        assert!(keys_for(&plain, Action::NextSearch).is_none());

        let ctx = HintContext {
            near_diagram: true,
            search_active: true,
            ..HintContext::default()
        };
        let g = groups(&ctx, &map);
        assert!(titles(&g).contains(&"Diagram"));
        assert_eq!(keys_for(&g, Action::NextSearch).as_deref(), Some("n/N"));
        assert!(labels(&g, "Search").contains(&"next/prev match".to_string()));
    }

    #[test]
    fn each_mode_produces_its_own_groups() {
        let map = KeyMap::with_defaults();
        let g = groups(
            &HintContext {
                mode: Mode::Search,
                ..HintContext::default()
            },
            &map,
        );
        assert_eq!(titles(&g), vec!["Search"]);
        let labels: Vec<&str> = g[0].rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["type pattern", "delete", "confirm", "cancel"],
            "nothing unreachable while the prompt is open"
        );

        let g = groups(
            &HintContext {
                mode: Mode::Toc,
                ..HintContext::default()
            },
            &map,
        );
        assert_eq!(titles(&g), vec!["Contents"]);
        assert!(keys_for(&g, Action::Activate).is_some());
        assert_eq!(keys_for(&g, Action::ScrollDown).as_deref(), Some("j/k"));

        let g = groups(
            &HintContext {
                mode: Mode::Help,
                ..HintContext::default()
            },
            &map,
        );
        assert_eq!(titles(&g), vec!["Help"]);
        assert!(keys_for(&g, Action::Quit).is_some());
    }

    #[test]
    fn custom_bindings_change_the_labels() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "toggle_key_hints".to_string(),
            KeyBinding::One("f2".to_string()),
        );
        overrides.insert(
            "scroll_down".to_string(),
            KeyBinding::Many(vec!["ctrl-n".to_string()]),
        );
        let map = KeyMap::from_overrides(overrides.iter().map(|(k, v)| (k.as_str(), v)))
            .expect("valid overrides");
        let g = groups(&HintContext::default(), &map);
        assert_eq!(keys_for(&g, Action::ToggleKeyHints).as_deref(), Some("f2"));
        assert_eq!(
            keys_for(&g, Action::ScrollDown).as_deref(),
            Some("ctrl-n/k")
        );
    }

    #[test]
    fn an_unbound_action_is_omitted_rather_than_shown_empty() {
        let mut overrides = BTreeMap::new();
        // An empty list unbinds the action entirely.
        overrides.insert("toggle_fold".to_string(), KeyBinding::Many(Vec::new()));
        overrides.insert(
            "toggle_mermaid_source".to_string(),
            KeyBinding::Many(Vec::new()),
        );
        let map = KeyMap::from_overrides(overrides.iter().map(|(k, v)| (k.as_str(), v)))
            .expect("valid overrides");
        let g = groups(
            &HintContext {
                near_diagram: true,
                ..HintContext::default()
            },
            &map,
        );
        assert!(keys_for(&g, Action::ToggleFold).is_none());
        assert!(
            g.iter()
                .flat_map(|x| x.rows.iter())
                .all(|r| !r.keys.is_empty()),
            "no empty key labels"
        );
        assert!(
            !titles(&g).contains(&"Diagram"),
            "a group whose every action is unbound disappears"
        );
    }

    #[test]
    fn width_is_bounded() {
        let hints = HintsState { open: true };
        assert_eq!(hints.width(120), DEFAULT_WIDTH);
        assert_eq!(hints.width(60), 20);
        assert!(hints.width(10) <= 10);
    }
}
