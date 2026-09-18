//! Golden snapshot tests: every fixture is laid out at 40, 80 and 120 columns
//! and compared against a committed plain-text snapshot.
//!
//! The Markdown fixtures come first, then the structured (JSON and YAML) ones
//! with the views a structured document has that a Markdown one does not —
//! collapsed containers, the outline, a revealed search match, a split.
//!
//! The plain-text serialisation is `RenderTree::to_plain_text`, which is also
//! what `--color never` output is built from.
//!
//! Two kinds of test live here, and the difference matters:
//!
//! * the **snapshots** pin the exact appearance of a fixture at a width;
//! * the **invariants** pin properties that must hold for every fixture at
//!   every width. A snapshot diff of 65 plain-text lines is a thing humans
//!   review badly and `cargo insta accept` makes it cheaper to say yes than
//!   to read — which is how literal `###` markers once got baked into
//!   accepted snapshots. An invariant fails with a pointer at the offending
//!   line instead.

use diple::document::markdown::{self, parse};
use diple::document::{load, DocumentModel, FoldId, FormatRequest, SourceDocument};
use diple::layout::{Layout, LayoutOptions};
use diple::render::ansi::to_ansi_text;
use diple::render::primitives::LineKind;
use diple::render::theme::{ColorLevel, Theme};

const WIDTHS: [usize; 3] = [40, 80, 120];

/// Fixtures whose rendering does not depend on the terminal width, with the
/// widths that render identically.
///
/// The first width of each group is the one that carries a snapshot; the rest
/// are covered by [`width_invariant_fixtures_are_identical_at_every_width`],
/// which is a stronger claim than three byte-identical snapshot files (those
/// merely happened to agree; this asserts that they must).
const WIDTH_INVARIANT: [(&str, &[usize]); 5] = [
    ("code-blocks", &[80, 120]),
    ("malformed", &[80, 120]),
    ("mermaid", &[40, 80, 120]),
    ("narrow-table", &[40, 80, 120]),
    ("nested-lists", &[80, 120]),
];

/// `true` if `(name, width)` is covered by a width-invariance group and is
/// not the representative width that carries the snapshot.
fn covered_by_width_invariance(name: &str, width: usize) -> bool {
    WIDTH_INVARIANT.iter().any(|(fixture, widths)| {
        *fixture == name && widths.first() != Some(&width) && widths.contains(&width)
    })
}

fn fixtures() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("fixtures dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("fixture")
            .to_string();
        let source = std::fs::read_to_string(&path).expect("read fixture");
        out.push((name, source));
    }
    out.sort();
    assert!(!out.is_empty(), "no fixtures");
    out
}

fn render(source: &str, width: usize) -> String {
    let doc = DocumentModel::markdown(parse(source));
    let theme = Theme::dark();
    Layout::build(&doc, &LayoutOptions::new(width, &theme)).to_plain_text()
}

#[test]
fn fixtures_render_deterministically_at_every_width() {
    let theme = Theme::dark();
    for (name, source) in fixtures() {
        let model = DocumentModel::markdown(parse(&source));
        let doc = model.as_markdown().expect("markdown");
        for width in WIDTHS {
            let opts = LayoutOptions::new(width, &theme);
            let tree = Layout::build(&model, &opts);
            // Sanity: every line belongs to a real node.
            for line in &tree.lines {
                assert!(doc.node(line.node).is_some(), "{name}: bad node id");
            }
            if covered_by_width_invariance(&name, width) {
                continue;
            }
            insta::assert_snapshot!(format!("{name}-{width}"), tree.to_plain_text());
        }
    }
}

/// Some fixtures are narrow enough — or fixed-width enough — that the
/// terminal width makes no difference to them. That is a claim about the
/// layout engine (a diagram, a table that already fits, code that is not
/// wrapped by default), so it is asserted rather than left implicit in three
/// snapshot files that happen to be byte-identical.
#[test]
fn width_invariant_fixtures_are_identical_at_every_width() {
    for (name, widths) in WIDTH_INVARIANT {
        let source = fixtures()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| s)
            .unwrap_or_else(|| panic!("fixture {name} is gone: update WIDTH_INVARIANT"));
        let (first, rest) = widths.split_first().expect("a representative width");
        let reference = render(&source, *first);
        for width in rest {
            assert_eq!(
                render(&source, *width),
                reference,
                "{name} renders differently at {width} than at {first}; if that is \
                 intended, drop {width} from WIDTH_INVARIANT and add its snapshot"
            );
        }
    }
}

/// Raw Markdown syntax must never reach the reader.
///
/// Two leaks are checked at every width, for every fixture:
///
/// * a line the layout attributes to a heading must not start with `#` —
///   literal `###` markers leaked once already and were accepted into the
///   snapshots;
/// * outside code blocks and diagrams, no line may carry an unconsumed `**`,
///   `__` or backtick run.
///
/// `malformed.md` deliberately contains spans that are never closed, and an
/// unclosed marker is *meant* to survive as text; those lines are the only
/// exemption, and they are matched by content so that a new leak elsewhere in
/// that fixture still fails.
#[test]
fn no_markdown_marker_leaks_into_rendered_output() {
    let theme = Theme::dark();
    for (name, source) in fixtures() {
        let model = DocumentModel::markdown(parse(&source));
        for width in WIDTHS {
            let tree = Layout::build(&model, &LayoutOptions::new(width, &theme));
            for (idx, line) in tree.lines.iter().enumerate() {
                let text = line.to_text();
                if matches!(line.kind, LineKind::Heading(_)) {
                    assert!(
                        !text.trim_start().starts_with('#'),
                        "{name}@{width} line {idx}: heading leaks its Markdown marker: {text:?}"
                    );
                }
                if matches!(line.kind, LineKind::Code | LineKind::Diagram) {
                    continue;
                }
                // A deliberately unclosed span in `malformed.md` is text, not
                // a leak.
                if text.contains("Unclosed") {
                    continue;
                }
                for marker in ["**", "__", "`"] {
                    assert!(
                        !text.contains(marker),
                        "{name}@{width} line {idx}: unconsumed {marker:?} in {text:?}"
                    );
                }
            }
        }
    }
}

/// Collapsing every section leaves exactly the top-level headings, each on a
/// single line behind a `▶` marker, and nothing of any body.
///
/// This replaces two snapshots that each held a single line (`▶ Project Foo`
/// and `▶ Nested Lists`) — a snapshot in name only, with no diff worth
/// reviewing. Asserted directly it also covers every other fixture.
#[test]
fn collapsed_sections_render_only_their_headings() {
    let theme = Theme::dark();
    let mut checked = 0usize;
    for (name, source) in fixtures() {
        let model = DocumentModel::markdown(parse(&source));
        let doc = model.as_markdown().expect("markdown");
        if doc.sections.is_empty() || doc.sections[0].heading != 0 {
            // A fixture with content above its first heading would keep that
            // content visible; not what this test is about.
            continue;
        }
        let mut folds = model.fold_state();
        folds.collapse_all();
        let mut opts = LayoutOptions::new(80, &theme).with_folds(&folds);
        // The trailing footnote section is appended by the layout engine and
        // is not part of any foldable section; it is covered by its own
        // fixture snapshots and would only add noise here.
        opts.footnotes = false;
        let tree = Layout::build(&model, &opts);

        let top_level: Vec<&str> = doc
            .sections
            .iter()
            .filter(|s| s.parent.is_none())
            .filter_map(|s| doc.node(s.heading))
            .filter_map(|n| match &n.kind {
                markdown::NodeKind::Heading(h) => Some(h.text.as_str()),
                _ => None,
            })
            .collect();
        let want: String = top_level.iter().map(|t| format!("▶ {t}\n")).collect();
        assert_eq!(tree.to_plain_text(), want, "{name}: collapsed rendering");
        assert!(
            tree.lines
                .iter()
                .all(|l| matches!(l.kind, LineKind::FoldedMarker)),
            "{name}: every line is a fold marker"
        );
        checked += 1;
    }
    assert!(
        checked >= 2,
        "at least two fixtures exercised, got {checked}"
    );
}

#[test]
fn ascii_fallback_and_code_options() {
    let theme = Theme::light();
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/code-blocks.md"),
    )
    .expect("fixture");
    let doc = DocumentModel::markdown(parse(&source));
    let mut opts = LayoutOptions::new(80, &theme);
    opts.unicode = false;
    opts.code_line_numbers = true;
    opts.code_wrap = true;
    let tree = Layout::build(&doc, &opts);
    insta::assert_snapshot!("code-blocks-ascii-numbered-80", tree.to_plain_text());
}

// ---------------------------------------------------------------------------
// Structured documents (JSON and YAML)
// ---------------------------------------------------------------------------
//
// The same contract as above, for the formats added in 2.0: a committed
// plain-text snapshot per (fixture, width, view). What each of these has to
// show is a *semantic* rendering rather than a pretty-printed copy of the
// file — nesting by indentation, source order kept, keys distinguishable from
// values, a collapsed container standing for its contents with a summary, and
// an ASCII fallback that says the same things without box drawing.
//
// Widths: 80 is the reference, 40 and 56 are the narrow terminals §24.3 warns
// about, where an indented deep key has almost no room left for its value.

/// Load a structured fixture the way the pager does: by name, so the
/// extension picks the format.
fn structured_fixture(name: &str) -> DocumentModel {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path).expect("read fixture");
    load(FormatRequest::Auto, SourceDocument::new(name, &text))
        .unwrap_or_else(|e| panic!("{name} must parse:\n{}", e.report()))
        .model
}

/// Lay a structured fixture out at `width`, everything expanded.
fn render_structured(model: &DocumentModel, width: usize) -> String {
    let theme = Theme::dark();
    Layout::build(model, &LayoutOptions::new(width, &theme)).to_plain_text()
}

/// The fold whose container row carries the mapping key `key`.
///
/// Folds are addressed by what the reader sees rather than by index, so a
/// fixture gaining an entry does not silently move these snapshots to a
/// different container.
fn fold_with_key(model: &DocumentModel, key: &str) -> FoldId {
    let doc = model.as_structured().expect("structured");
    (0..model.fold_parents().len())
        .find(|fold| {
            model
                .fold_node(*fold)
                .and_then(|node| doc.node(node))
                .and_then(|node| node.key())
                .is_some_and(|k| k.text == key)
        })
        .unwrap_or_else(|| panic!("no foldable container keyed {key:?}"))
}

/// A JSON document, expanded, at the reference width and at two narrow ones.
#[test]
fn json_renders_expanded() {
    let model = structured_fixture("nested.json");
    for width in [40usize, 56, 80] {
        insta::assert_snapshot!(
            format!("nested-json-{width}"),
            render_structured(&model, width)
        );
    }
    let arrays = structured_fixture("array-of-objects.json");
    insta::assert_snapshot!("array-of-objects-json-40", render_structured(&arrays, 40));
    insta::assert_snapshot!("array-of-objects-json-80", render_structured(&arrays, 80));

    // Escapes, CJK, emoji and a non-ASCII key: the width arithmetic of the
    // structured layout has to agree with the Markdown engine's.
    let unicode = structured_fixture("unicode-escapes.json");
    insta::assert_snapshot!("unicode-escapes-json-40", render_structured(&unicode, 40));
    insta::assert_snapshot!("unicode-escapes-json-80", render_structured(&unicode, 80));

    // Twenty-four levels at 40 columns: the point where indentation alone
    // would leave nothing for the value.
    let deep = structured_fixture("deeply-nested.json");
    insta::assert_snapshot!("deeply-nested-json-40", render_structured(&deep, 40));
}

/// A partially collapsed JSON document: two containers stand for their
/// contents with a summary, everything else is still expanded.
#[test]
fn json_renders_partially_collapsed() {
    let model = structured_fixture("nested.json");
    let theme = Theme::dark();
    let mut folds = model.fold_state();
    folds.collapse(fold_with_key(&model, "repository"));
    folds.collapse(fold_with_key(&model, "profile"));
    let opts = LayoutOptions::new(80, &theme).with_folds(&folds);
    let text = Layout::build(&model, &opts).to_plain_text();

    assert!(
        !text.contains("codegen-units"),
        "a collapsed container still shows its contents:\n{text}"
    );
    insta::assert_snapshot!("nested-json-collapsed-80", text);
}

/// The ASCII fallback: no box drawing, no `▼`/`▶`, and the same information.
#[test]
fn json_renders_without_unicode() {
    let model = structured_fixture("nested.json");
    let theme = Theme::dark();
    let mut folds = model.fold_state();
    folds.collapse(fold_with_key(&model, "repository"));
    let mut opts = LayoutOptions::new(80, &theme).with_folds(&folds);
    opts.unicode = false;
    let text = Layout::build(&model, &opts).to_plain_text();

    for marker in ['▼', '▶', '│', '─'] {
        assert!(
            !text.contains(marker),
            "{marker:?} survived the ASCII fallback:\n{text}"
        );
    }
    assert!(text.is_ascii() || text.contains("http"), "{text}");
    insta::assert_snapshot!("nested-json-ascii-80", text);
}

/// `--color never`: the SGR serialisation at [`ColorLevel::None`] is the
/// plain text, byte for byte, with no escape sequence anywhere in it.
#[test]
fn json_renders_without_colour() {
    let model = structured_fixture("nested.json");
    let theme = Theme::dark();
    let tree = Layout::build(&model, &LayoutOptions::new(80, &theme));
    let text = to_ansi_text(&tree, ColorLevel::None);

    assert!(
        !text.contains('\u{1b}'),
        "an escape leaked into --color never"
    );
    assert_eq!(
        text,
        tree.to_plain_text(),
        "the two serialisations disagree"
    );
    insta::assert_snapshot!("nested-json-nocolor-80", text);
}

/// YAML, expanded, including the narrow widths where a deeply indented
/// container key and its value compete for the same columns.
#[test]
fn yaml_renders_expanded() {
    let model = structured_fixture("k8s-deployment.yaml");
    for width in [40usize, 56, 80] {
        insta::assert_snapshot!(
            format!("k8s-deployment-yaml-{width}"),
            render_structured(&model, width)
        );
    }
}

/// Comments are content: they stay, above their entry and beside it, and they
/// stay in source order.
#[test]
fn yaml_renders_comments() {
    let model = structured_fixture("comments.yaml");
    let text = render_structured(&model, 80);
    assert!(
        text.contains("Every comment in this file must survive parsing"),
        "a comment was dropped:\n{text}"
    );
    insta::assert_snapshot!("comments-yaml-80", text);
    insta::assert_snapshot!("comments-yaml-40", render_structured(&model, 40));
}

/// Anchors and aliases are shown as references. An alias must never be
/// expanded into a copy of what it points at.
#[test]
fn yaml_renders_anchors_and_aliases() {
    let model = structured_fixture("anchors.yaml");
    let text = render_structured(&model, 80);
    assert!(
        text.contains("&defaults"),
        "the anchor is not shown:\n{text}"
    );
    assert!(
        text.contains("*defaults"),
        "the alias is not shown:\n{text}"
    );
    assert_eq!(
        text.matches("adapter").count(),
        1,
        "an alias was expanded into a copy of its anchor:\n{text}"
    );
    insta::assert_snapshot!("anchors-yaml-80", text);
    insta::assert_snapshot!("anchors-yaml-40", render_structured(&model, 40));
}

/// Block scalars keep their style and their line structure.
#[test]
fn yaml_renders_block_scalars() {
    let model = structured_fixture("block-scalars.yaml");
    insta::assert_snapshot!("block-scalars-yaml-80", render_structured(&model, 80));
    insta::assert_snapshot!("block-scalars-yaml-40", render_structured(&model, 40));
}

/// A stream is three documents, each a root of its own, in source order.
#[test]
fn yaml_renders_a_multi_document_stream() {
    let model = structured_fixture("multi-document.yaml");
    let text = render_structured(&model, 80);
    assert_eq!(
        model.as_structured().expect("structured").roots().len(),
        3,
        "the stream lost a document"
    );
    for kind in ["ConfigMap", "Service", "Secret"] {
        assert!(text.contains(kind), "{kind} is missing:\n{text}");
    }
    insta::assert_snapshot!("multi-document-yaml-80", text);
    insta::assert_snapshot!("multi-document-yaml-40", render_structured(&model, 40));
}

/// The outline sidebar's content, for both formats.
///
/// The sidebar widget itself is `app`-internal; what is pinned here is what it
/// is given — one entry per node, in document order, nested by depth — which
/// is the part a structured document decides.
#[test]
fn the_outline_of_a_structured_document() {
    for name in ["nested.json", "k8s-deployment.yaml"] {
        let model = structured_fixture(name);
        let text: String = model
            .outline()
            .iter()
            .map(|entry| {
                format!(
                    "{}{}{}\n",
                    "  ".repeat(entry.depth),
                    entry.text,
                    if entry.fold.is_some() { " [fold]" } else { "" }
                )
            })
            .collect();
        let stem = name.replace('.', "-");
        insta::assert_snapshot!(format!("{stem}-outline"), text);
    }
}

/// A search match inside a collapsed container: hidden first, then revealed.
///
/// Both halves are in one snapshot because the pair is the claim — the row is
/// absent while its container is collapsed, and revealing the match opens
/// exactly the ancestors needed to show it, leaving the rest collapsed.
#[test]
fn a_search_match_is_revealed_out_of_a_collapsed_container() {
    let model = structured_fixture("nested.json");
    let theme = Theme::dark();
    let query = "symbols";

    let hits = model.search_index().find(query, false);
    let hit = *hits.first().expect("the query must hit");

    let mut folds = model.fold_state();
    folds.collapse_all();
    assert!(
        model.is_hidden(hit.node, &folds),
        "the match is not hidden to begin with"
    );
    let hidden =
        Layout::build(&model, &LayoutOptions::new(80, &theme).with_folds(&folds)).to_plain_text();
    assert!(
        !hidden.contains(query),
        "a collapsed match is still on screen"
    );

    model.reveal(hit.node, &mut folds);
    assert!(!model.is_hidden(hit.node, &folds), "reveal did nothing");
    let revealed =
        Layout::build(&model, &LayoutOptions::new(80, &theme).with_folds(&folds)).to_plain_text();
    assert!(revealed.contains(query), "the revealed match has no row");

    insta::assert_snapshot!(
        "nested-json-search-reveal-80",
        format!("collapsed:\n{hidden}\nrevealed for /{query}:\n{revealed}")
    );
}

/// The two panes of a split, side by side.
///
/// The split *renderer* lives in `app` and is not reachable from an
/// integration test; what a split does to the document layer is give each
/// pane half the terminal, so each pane's content is laid out at that width
/// and the two are composed here. That is the part a wrapping bug shows up
/// in: 40 columns per pane on an 80-column terminal is exactly the narrow
/// case of §24.3.
fn split(left: &DocumentModel, right: &DocumentModel, pane: usize) -> String {
    let left = render_structured(left, pane);
    let right = render_structured(right, pane);
    let mut left = left.lines();
    let mut right = right.lines();
    let mut out = String::new();
    loop {
        match (left.next(), right.next()) {
            (None, None) => break,
            (l, r) => {
                let l = l.unwrap_or("");
                let r = r.unwrap_or("");
                let pad = pane.saturating_sub(diple::layout::unicode::width(l));
                out.push_str(l);
                out.push_str(&" ".repeat(pad));
                out.push_str(" │ ");
                out.push_str(r);
                out.push('\n');
            }
        }
    }
    out
}

/// A split with a Markdown document beside a YAML one: two formats on screen
/// at once, each rendered by its own engine.
#[test]
fn a_split_of_markdown_and_yaml() {
    let markdown = DocumentModel::markdown(parse(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/readme.md"),
        )
        .expect("fixture"),
    ));
    let yaml = structured_fixture("k8s-deployment.yaml");
    insta::assert_snapshot!("split-markdown-yaml-40x2", split(&markdown, &yaml, 40));
}

/// A split of two JSON documents: the same engine twice, at pane width.
#[test]
fn a_split_of_two_json_documents() {
    let left = structured_fixture("nested.json");
    let right = structured_fixture("array-of-objects.json");
    insta::assert_snapshot!("split-json-json-40x2", split(&left, &right, 40));
}

/// Every structured row fits the terminal width, and a narrow terminal
/// really does render differently from a wide one.
///
/// This is the invariant behind the 40- and 56-column snapshots: they exist
/// to catch a wrapping bug, which they can only do if wrapping happens at
/// all. `deeply-nested.json` is the exception and is excluded on purpose —
/// twenty-four levels of indentation leave no columns to wrap into, so what
/// it pins is the behaviour at the point where indentation alone exceeds the
/// terminal, which its snapshot shows.
#[test]
fn structured_rows_fit_the_terminal_width() {
    const FIXTURES: [&str; 6] = [
        "nested.json",
        "array-of-objects.json",
        "unicode-escapes.json",
        "k8s-deployment.yaml",
        "block-scalars.yaml",
        "anchors.yaml",
    ];
    for name in FIXTURES {
        let model = structured_fixture(name);
        let wide = render_structured(&model, 80);
        for width in [40usize, 56, 80] {
            let text = render_structured(&model, width);
            for (idx, line) in text.lines().enumerate() {
                assert!(
                    diple::layout::unicode::width(line) <= width,
                    "{name}@{width} row {idx} is {} columns wide: {line:?}",
                    diple::layout::unicode::width(line)
                );
            }
        }
        // A fixture that already fits in 40 columns has nothing for a narrow
        // terminal to change; it is in this list for the width check above.
        if wide
            .lines()
            .all(|line| diple::layout::unicode::width(line) <= 40)
        {
            continue;
        }
        assert_ne!(
            render_structured(&model, 40),
            wide,
            "{name} renders identically at 40 and 80 columns, so its narrow \
             snapshot cannot catch a wrapping bug"
        );
    }
}
