//! Characterization tests: the Markdown reading experience as it is.
//!
//! These tests exist to make a regression *observable* while the document
//! layer is generalized over several formats. They deliberately assert
//! behaviour a reader would notice — which line the viewport lands on after a
//! heading jump, what a fold hides, what a search reveals — rather than the
//! internal shape of whatever produced it. A refactor that keeps the reader's
//! experience intact keeps these green; one that does not, does not.
//!
//! The narrower unit tests next to each algorithm stay where they are; this
//! module is the cross-cutting net above them.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::app::state::{App, Mode};
use crate::config::Action;
use crate::document::NodeKind;
use crate::render::primitives::LineKind;
use crate::testing::{app_sized, key, key_mod};

const DOC: &str = "\
# Title

Intro paragraph about needles.

## Alpha

Alpha body with a needle in it.

### Alpha One

Deep body.

## Beta

Beta body.

[link](https://example.invalid/)

## Gamma

Gamma body.
";

fn open() -> App {
    app_sized(DOC, (60, 14))
}

/// Text of the line the viewport starts at.
fn top_text(app: &App) -> String {
    app.tree()
        .lines
        .get(app.top_line())
        .map(|l| l.to_text().trim_end().to_string())
        .unwrap_or_default()
}

/// Every visible (laid-out) line, as plain text.
fn visible(app: &App) -> Vec<String> {
    app.tree()
        .lines
        .iter()
        .map(|l| l.to_text().trim_end().to_string())
        .collect()
}

fn heading_texts(app: &App) -> Vec<String> {
    app.tree()
        .heading_lines()
        .iter()
        .filter_map(|(line, _, _)| app.tree().lines.get(*line))
        .map(|l| l.to_text().trim_end().to_string())
        .collect()
}

#[test]
fn headings_are_laid_out_as_headings() {
    let app = open();
    let kinds: Vec<&LineKind> = app
        .tree()
        .lines
        .iter()
        .map(|l| &l.kind)
        .filter(|k| matches!(k, LineKind::Heading(_)))
        .collect();
    assert!(
        !kinds.is_empty(),
        "a document with five headings lays out heading lines"
    );
    assert_eq!(
        heading_texts(&app).len(),
        5,
        "one heading entry per heading, collapsed markers aside"
    );
}

#[test]
fn next_and_previous_heading_walk_the_document() {
    let mut app = open();
    // `]` from the top lands on the first heading below the cursor.
    app.apply(Action::NextHeading);
    let first = top_text(&app);
    app.apply(Action::NextHeading);
    let second = top_text(&app);
    assert_ne!(first, second, "`]` moves on");

    app.apply(Action::PreviousHeading);
    assert_eq!(top_text(&app), first, "`[` comes back");
}

#[test]
fn same_level_navigation_skips_the_nested_heading() {
    let mut app = open();
    // Land on "Alpha" (an H2), then `}` must reach "Beta", not "Alpha One".
    app.apply(Action::NextHeading);
    let alpha_line = app.top_line();
    app.apply(Action::NextHeadingSameLevel);
    let after = app.top_line();
    assert!(after > alpha_line, "`}}` moved forward");
    let text: String = visible(&app)[after..]
        .iter()
        .take(4)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Beta"),
        "`}}` skips the H3 and lands near Beta, got:\n{text}"
    );
}

#[test]
fn collapsing_a_section_hides_its_body_and_its_children() {
    let mut app = open();
    let before = visible(&app).join("\n");
    assert!(before.contains("Alpha One"));

    // Put the cursor on "Alpha" and collapse it.
    app.apply(Action::NextHeading);
    app.apply(Action::CollapseFold);

    let after = visible(&app).join("\n");
    assert!(after.contains("Alpha"), "the heading itself stays visible");
    assert!(
        !after.contains("Alpha body"),
        "the body is hidden:\n{after}"
    );
    assert!(
        !after.contains("Alpha One"),
        "the nested section is hidden too:\n{after}"
    );
    assert!(after.contains("Beta"), "siblings are untouched");

    app.apply(Action::ExpandFold);
    assert!(
        visible(&app).join("\n").contains("Alpha One"),
        "`zo` undoes"
    );
}

#[test]
fn collapse_all_then_expand_all_round_trips() {
    let mut app = open();
    let expanded = visible(&app).join("\n");

    app.apply(Action::CollapseAll);
    let collapsed = visible(&app).join("\n");
    assert!(
        collapsed.contains("Title"),
        "the outermost heading survives"
    );
    assert!(!collapsed.contains("Gamma body"), "zM hides the bodies");
    assert!(
        !collapsed.contains("Gamma"),
        "collapsing the H1 hides the H2s nested under it:\n{collapsed}"
    );

    app.apply(Action::ExpandAll);
    assert_eq!(
        visible(&app).join("\n"),
        expanded,
        "zR restores the document"
    );
}

#[test]
fn enter_toggles_the_section_under_the_cursor() {
    let mut app = open();
    app.apply(Action::NextHeading);
    app.apply(Action::Activate);
    assert!(!visible(&app).join("\n").contains("Alpha body"));
    app.apply(Action::Activate);
    assert!(visible(&app).join("\n").contains("Alpha body"));
}

#[test]
fn search_finds_matches_in_document_order_and_scrolls_to_them() {
    let mut app = open();
    app.apply(Action::Search);
    assert_eq!(app.mode(), Mode::Search);
    for c in "needle".chars() {
        app.handle_key(&key(c));
    }
    app.handle_key(&key_mod(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.mode(), Mode::Normal);

    let first = app.top_line();
    app.apply(Action::NextSearch);
    assert_ne!(app.top_line(), first, "`n` moves to the next match");
    app.apply(Action::PreviousSearch);
    assert_eq!(app.top_line(), first, "`N` comes back");
}

#[test]
fn search_reveals_a_match_hidden_inside_a_collapsed_section() {
    let mut app = open();
    app.apply(Action::CollapseAll);
    assert!(!visible(&app).join("\n").contains("Alpha body"));

    app.apply(Action::Search);
    // "Deep" occurs only in the H3 two levels down, so reaching it needs the
    // whole ancestor chain expanded and nothing else.
    for c in "Deep".chars() {
        app.handle_key(&key(c));
    }
    app.handle_key(&key_mod(KeyCode::Enter, KeyModifiers::NONE));

    let shown = visible(&app).join("\n");
    assert!(
        shown.contains("Deep body"),
        "every collapsed ancestor of a match is expanded:\n{shown}"
    );
    assert!(
        !shown.contains("Beta body"),
        "unrelated folds stay collapsed:\n{shown}"
    );
}

#[test]
fn the_toc_lists_every_heading_and_jumps_to_one() {
    let mut app = open();
    app.apply(Action::ToggleToc);
    assert_eq!(app.mode(), Mode::Toc);
    let entries: Vec<String> = app.toc.entries.iter().map(|e| e.text.clone()).collect();
    assert_eq!(entries, ["Title", "Alpha", "Alpha One", "Beta", "Gamma"]);
    let depths: Vec<usize> = app.toc.entries.iter().map(|e| e.depth).collect();
    assert_eq!(depths, [0, 1, 2, 1, 1], "depth is nesting, not level");

    // Jump to "Beta".
    for _ in 0..3 {
        app.apply(Action::ScrollDown);
    }
    app.apply(Action::Activate);
    let near: String = visible(&app)[app.top_line()..]
        .iter()
        .take(5)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(near.contains("Beta"), "the TOC jumped to Beta:\n{near}");
}

#[test]
fn resizing_keeps_the_same_content_at_the_top() {
    let mut app = open();
    for _ in 0..3 {
        app.apply(Action::NextHeading);
    }
    let anchored = app.anchor().0;
    app.resize(40, 20);
    assert_eq!(
        app.anchor().0,
        anchored,
        "the anchored node survives a resize"
    );
    app.resize(120, 30);
    assert_eq!(app.anchor().0, anchored);
}

#[test]
fn folding_survives_a_resize() {
    let mut app = open();
    app.apply(Action::NextHeading);
    app.apply(Action::CollapseFold);
    app.resize(40, 20);
    assert!(
        !visible(&app).join("\n").contains("Alpha body"),
        "the fold is semantic, not a line range"
    );
    app.resize(100, 30);
    assert!(!visible(&app).join("\n").contains("Alpha body"));
}

#[test]
fn links_are_selectable_and_reported() {
    let mut app = open();
    app.apply(Action::Bottom);
    app.apply(Action::NextLink);
    assert!(app.selected_link().is_some(), "tab selects a link");
    let message = app.message().unwrap_or_default().to_string();
    assert!(
        message.contains("example.invalid"),
        "the status line names the destination, got {message:?}"
    );
}

#[test]
fn the_document_title_comes_from_the_first_h1() {
    let app = open();
    assert_eq!(app.doc.title.as_deref(), Some("Title"));
}

#[test]
fn a_document_without_headings_still_renders_and_reports_the_absence() {
    let mut app = app_sized("just text\n\nmore text\n", (40, 10));
    assert!(app.tree().len() > 0);
    app.apply(Action::NextHeading);
    assert_eq!(app.message(), Some("document has no headings"));
    app.apply(Action::ToggleToc);
    assert_eq!(app.message(), Some("document has no headings"));
    assert_eq!(app.mode(), Mode::Normal, "the TOC did not open");
}

#[test]
fn markdown_keeps_its_own_node_kinds() {
    // The generalization must not flatten Markdown into a lowest common
    // denominator: a table stays a table, a code block stays a code block.
    let app = app_sized(
        "# H\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn x() {}\n```\n",
        (60, 20),
    );
    let kinds: Vec<String> = app
        .doc
        .walk()
        .map(|n| match &n.kind {
            NodeKind::Heading(_) => "heading".to_string(),
            NodeKind::Table(_) => "table".to_string(),
            NodeKind::CodeBlock(_) => "code".to_string(),
            other => format!("{other:?}"),
        })
        .collect();
    assert!(kinds.contains(&"heading".to_string()));
    assert!(kinds.contains(&"table".to_string()));
    assert!(kinds.contains(&"code".to_string()));
    assert!(
        app.tree()
            .lines
            .iter()
            .any(|l| matches!(l.kind, LineKind::TableRow)),
        "the table is laid out as a table"
    );
}
