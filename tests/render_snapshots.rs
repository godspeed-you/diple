//! Golden snapshot tests: every fixture is laid out at 40, 80 and 120 columns
//! and compared against a committed plain-text snapshot.
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
use diple::document::{load, DocumentModel, FoldId, FoldState, FormatRequest, SourceDocument};
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
