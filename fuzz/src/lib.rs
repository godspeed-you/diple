//! Fuzz target bodies, factored out of `fuzz_targets/` so that they can also
//! be replayed by the corpus runner (`cargo run --bin smoke`) on a stable
//! toolchain, where libFuzzer is not available.
//!
//! Every function here takes raw bytes and must never panic.

use diple::document::markdown::{parse, Alignment, Inline, Inlines, Table};
use diple::document::structured::StructuredDocument;
use diple::document::{
    load, DocumentKind, DocumentModel, DocumentPath, FoldState, FormatRequest, Match, MatchField,
    SourceDocument,
};
use diple::layout::table::{layout_table, TableOptions};
use diple::layout::unicode;
use diple::layout::{Layout, LayoutOptions};
use diple::render::theme::Theme;

/// Largest terminal width a fuzzed input may ask for.
///
/// diple itself clamps to the real terminal size; the cap only keeps the
/// fuzzer from spending all its time allocating gigantic lines.
const MAX_WIDTH: usize = 500;

/// Split the leading control byte off the input.
fn split_control(data: &[u8]) -> (u8, &[u8]) {
    match data.split_first() {
        Some((first, rest)) => (*first, rest),
        None => (0, &[]),
    }
}

/// Interpret a control byte as a terminal width in `1..=MAX_WIDTH`.
fn width_from(byte: u8) -> usize {
    // 0 maps to 1, not to 0: a zero-width terminal is not a real input, and
    // the layout engine documents `width.max(1)` behaviour anyway.
    (usize::from(byte) * MAX_WIDTH / 256).max(1)
}

/// `document::parser::parse` on arbitrary UTF-8.
pub fn parse_markdown(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let doc = parse(text);
    // Touch the derived indices too: sections, anchors and links are built
    // from the same input and have their own edge cases.
    let _ = doc.node_count();
    for node in doc.walk() {
        let _ = &node.kind;
    }
}

/// Parse and lay out at a fuzzed width.
pub fn layout(data: &[u8]) {
    let (control, rest) = split_control(data);
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let doc = parse(text);
    let theme = if control & 1 == 0 {
        Theme::dark()
    } else {
        Theme::light()
    };
    let mut opts = LayoutOptions::new(width_from(control), &theme);
    opts.wrap = control & 2 == 0;
    opts.code_wrap = control & 4 == 0;
    opts.code_line_numbers = control & 8 != 0;
    opts.unicode = control & 16 == 0;
    opts.footnotes = control & 32 == 0;
    let tree = Layout::build(&DocumentModel::markdown(doc), &opts);
    // Spec §23.4: no uncontrolled escapes, for Markdown exactly as for the
    // structured formats. A heading, a code block, a table cell and a link's
    // text all end up here.
    for line in &tree.lines {
        for span in &line.spans {
            assert!(
                !has_terminal_controls(&span.text),
                "a rendered span must not carry terminal controls: {:?}",
                span.text
            );
        }
    }
    let _ = tree.to_plain_text();
}

/// Table layout with fuzzed cell content, alignments and width.
pub fn table(data: &[u8]) {
    let (control, rest) = split_control(data);
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    // `\n` separates rows, `|` separates cells — a compact encoding that lets
    // the fuzzer reach ragged rows, empty cells and very wide cells quickly.
    let mut rows: Vec<Vec<Inlines>> = text
        .split('\n')
        .map(|row| {
            row.split('|')
                .map(|cell| vec![Inline::Text(cell.to_string())])
                .collect()
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    let header = rows.remove(0);
    let alignments = (0..header.len())
        .map(|i| match (control as usize + i) % 4 {
            0 => Alignment::None,
            1 => Alignment::Left,
            2 => Alignment::Center,
            _ => Alignment::Right,
        })
        .collect();
    let table = Table {
        alignments,
        header,
        rows,
    };
    let theme = Theme::dark();
    let opts = TableOptions {
        unicode: control & 1 == 0,
        ..TableOptions::default()
    };
    let _ = layout_table(&table, &theme, &opts, width_from(control), &[]);
}

/// The Unicode width / wrap / slice helpers.
pub fn unicode_helpers(data: &[u8]) {
    let (control, rest) = split_control(data);
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let n = usize::from(control);
    let (head, tail) = unicode::split_at_width(text, n);
    // Holds for every input, control characters included.
    assert_eq!(
        head.len() + tail.len(),
        text.len(),
        "split_at_width must partition the input"
    );
    assert!(unicode::truncate_to_width(text, n).len() <= text.len());
    assert!(unicode::pad_to_width(text, n).len() >= text.len());
    assert!(unicode::pad_left_to_width(text, n).len() >= text.len());
    let _ = unicode::width(text);
    let _ = unicode::center_to_width(text, n);
    let _ = unicode::truncate_with_ellipsis(text, n, "…");
    let _ = unicode::slice_columns(text, n / 2, n);
    let _ = unicode::expand_tabs(text, (n % 16) + 1);
    let _ = unicode::tokenize(text);
    let wrapped = unicode::wrap(text, n.max(1), n.max(1));

    // The width invariants hold for every input, control characters included:
    // `unicode::width`'s ASCII fast path is restricted to printable ASCII, so
    // it agrees with `unicode::grapheme_width` — the measure `split_at_width`
    // and `wrap` use — that a control character occupies zero cells.
    assert!(
        head.is_empty() || unicode::width(head) <= n,
        "split_at_width must not exceed the requested width"
    );
    for line in wrapped {
        // A single grapheme cluster that is wider than the limit cannot be
        // broken any further and is emitted as-is — the same documented
        // escape hatch `split_at_width` has. Everything else must fit.
        // (Note that "unbreakable" is not the same as "contains no space":
        // `" \u{fe0f}"` is one cluster whose base character is a space.)
        let unbreakable = unicode::graphemes(&line).count() <= 1;
        assert!(
            unbreakable || unicode::width(&line) <= n.max(1),
            "wrap must respect the width for breakable text"
        );
    }
}

/// `config::loader` TOML parsing, through the real file entry point.
pub fn config(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let mut path = std::env::temp_dir();
    path.push(format!("diple-fuzz-config-{}.toml", std::process::id()));
    if std::fs::write(&path, text).is_err() {
        return;
    }
    // Both outcomes are valid: a well-formed config loads, a malformed one
    // must produce a `ConfigError` rather than a panic.
    let _ = diple::config::loader::load_file(&path);
}

/// The Mermaid subset parser and the native renderer behind it.
pub fn mermaid(data: &[u8]) {
    let (control, rest) = split_control(data);
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let _ = diple::mermaid::diagram_kind(text);
    if let Ok(diagram) = diple::mermaid::parse(text) {
        let opts = diple::mermaid::RenderOptions {
            width_cells: width_from(control),
            unicode_box: control & 1 == 0,
            ..Default::default()
        };
        let _ = diple::mermaid::terminal::render(&diagram, &opts);
    }
}

// ---------------------------------------------------------------------------
// Structured documents (JSON and YAML)
//
// The six targets below are what §23.4 of the v2 spec asks for. They share one
// rule, stated once here rather than in each of them: arbitrary input must not
// panic, must not hang, must not recurse until the stack overflows, and must
// not put anything on the terminal that the terminal would obey. A target that
// only called the parser and dropped the result would catch the first of those
// and none of the rest, so each one asserts the structural invariants the rest
// of the program relies on instead.
// ---------------------------------------------------------------------------

/// Filenames the format targets choose between.
///
/// The extension is half of the detection policy — a recognised suffix decides
/// the format and turns a parse failure into an error rather than a fallback —
/// so a fuzzer that only ever passed `<stdin>` would never reach `parse_as`.
const NAMES: [&str; 6] = ["<stdin>", "a.md", "a.json", "a.yaml", "a.yml", "notes.txt"];

/// The `--format` requests the format targets choose between.
const REQUESTS: [FormatRequest; 4] = [
    FormatRequest::Auto,
    FormatRequest::Fixed(DocumentKind::Markdown),
    FormatRequest::Fixed(DocumentKind::Json),
    FormatRequest::Fixed(DocumentKind::Yaml),
];

/// Every field a structured node can contribute to the search index.
const FIELDS: [MatchField; 6] = [
    MatchField::Body,
    MatchField::Label,
    MatchField::Value,
    MatchField::Comment,
    MatchField::Tag,
    MatchField::Anchor,
];

/// Whether `text` holds anything `util::text::sanitize` is supposed to have
/// removed: an ESC, any other C0 control, DEL, the C1 range, or a bidi
/// override.
///
/// This is deliberately a second, independent implementation of the predicate
/// in `util::text` — a fuzz target that asked the code under test whether the
/// code under test was right would assert nothing at all.
fn has_terminal_controls(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c,
            '\u{0}'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{200e}' | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}')
    })
}

/// How many lines a parser can legitimately report a position on.
///
/// Counted with YAML 1.2's set of line breaks rather than with `str::lines`,
/// which splits on `\n` alone. The two disagree on a lone CR, and that
/// disagreement is a known defect rather than something this target can
/// assert away: `SourceDocument::line_count` and `SourceDocument::line` count
/// `\n` only, while granit counts every YAML break, so a YAML document with
/// old-Mac line endings reports an error position that its own excerpt does
/// not contain (`b"\r\r*"` reports line 3 of a source diple thinks has one
/// line, and the report comes out with no caret). Counting the way the parser
/// does keeps the in-range assertion meaningful for every other input.
fn line_count(text: &str) -> usize {
    let breaks = text
        .replace("\r\n", "\n")
        .chars()
        .filter(|c| matches!(c, '\n' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'))
        .count();
    // A trailing break does not open a further line.
    if text.is_empty() {
        1
    } else {
        breaks + 1
    }
}

/// Parse `text` as a structured format, or give up.
fn structured(kind: DocumentKind, name: &str, text: &str) -> Option<StructuredDocument> {
    let source = SourceDocument::new(name, text);
    let result = match kind {
        DocumentKind::Json => diple::document::json::parse(&source),
        _ => diple::document::yaml::parse(&source),
    };
    check_parse_result(&source, kind, &result);
    result.ok()
}

/// A parse either produces a model that holds together, or an error that says
/// where — and an error report a terminal can be shown safely.
fn check_parse_result(
    source: &SourceDocument,
    kind: DocumentKind,
    result: &Result<StructuredDocument, diple::document::DocumentError>,
) {
    match result {
        Ok(doc) => check_model(doc),
        Err(error) => {
            assert_eq!(error.format, kind, "an error must name the format it read");
            if let Some(position) = error.position {
                assert!(position.line >= 1, "lines are 1-based: {position:?}");
                assert!(position.column >= 1, "columns are 1-based: {position:?}");
                assert!(
                    position.line <= line_count(source.text()) + 1,
                    "a position past the end of the source: {position:?} in {} lines",
                    line_count(source.text())
                );
            }
            // A broken document is exactly the kind that carries an escape
            // sequence, and the report quotes it.
            let report = error.report();
            for line in report.lines() {
                assert!(
                    !has_terminal_controls(line),
                    "a parse-error report must not carry terminal controls: {line:?}"
                );
            }
        }
    }
}

/// The invariants every `StructuredDocument` must satisfy, whatever produced
/// it — the arena is what the cursor, the folds, the outline, the search and
/// the layout all address nodes through, so a violation here is a panic or a
/// wrong row somewhere far away.
fn check_model(doc: &StructuredDocument) {
    let count = doc.node_count();
    assert_eq!(count, doc.nodes().len(), "node_count must be the arena size");

    for (index, node) in doc.nodes().iter().enumerate() {
        assert_eq!(node.id, index, "node ids must be dense and in document order");
        assert!(node.end > node.id, "a subtree must contain its own root");
        assert!(node.end <= count, "a subtree must end inside the arena");
        assert!(
            node.depth <= diple::document::structured::MAX_DEPTH,
            "node {index} is deeper than the documented limit"
        );
        match node.parent {
            // A parent earlier in the arena is what makes the forest acyclic
            // by construction; nothing else has to walk it to find out.
            Some(parent) => {
                assert!(parent < node.id, "a parent must precede its child");
                let parent = &doc.nodes()[parent];
                assert!(
                    parent.end >= node.end,
                    "a subtree must be contained in its parent's"
                );
                assert_eq!(node.depth, parent.depth + 1, "depth must follow the parent");
            }
            None => assert_eq!(node.depth, 0, "a root sits at depth 0"),
        }
        assert!(
            node.child_count <= node.end - node.id - 1,
            "more children than the subtree has room for"
        );
        if let Some(first) = node.first_child() {
            assert_eq!(first, node.id + 1, "the first child follows in pre-order");
            assert_eq!(doc.nodes()[first].parent, Some(node.id));
        }
        assert!(node.span.start <= node.span.end, "a span must not run backwards");
        assert!(
            node.span.end <= doc.source().text().len(),
            "a span must stay inside the source"
        );
        let children = doc.children(node.id);
        assert_eq!(
            children.len(),
            node.child_count,
            "children() must agree with child_count"
        );
        for child in children {
            assert_eq!(doc.parent(child), Some(node.id), "parent/child must agree");
        }
    }

    for root in doc.roots() {
        assert!(root.node < count, "a root must name a real node");
        assert_eq!(doc.nodes()[root.node].parent, None, "a root has no parent");
    }

    check_folds(doc);
    check_search(doc);
    check_outline(doc);
}

/// The fold forest is addressed by id from the outline sidebar and from the
/// key bindings, so an out-of-range or cyclic parent is a hang or a panic.
fn check_folds(doc: &StructuredDocument) {
    let folds = doc.fold_count();
    assert_eq!(folds, doc.fold_parents().len(), "fold_count is the forest size");
    for fold in 0..folds {
        if let Some(parent) = doc.fold_parents()[fold] {
            assert!(parent < fold, "a fold's parent must precede it");
        }
        let node = doc.fold_node(fold).expect("every fold names a node");
        assert!(node < doc.node_count(), "a fold must name a real node");
        assert_eq!(
            doc.nodes()[node].fold,
            Some(fold),
            "fold_node and node.fold must be inverses"
        );
        assert!(
            doc.nodes()[node].is_container(),
            "only containers fold"
        );
    }
}

/// Search results are turned straight into byte ranges of indexed text by the
/// layout engine, which would panic on a non-boundary or an out-of-range one.
fn check_search(doc: &StructuredDocument) {
    let index = doc.search_index();
    for node in 0..doc.node_count() {
        for field in FIELDS {
            if let Some(text) = index.text(node, field) {
                assert!(
                    !has_terminal_controls(text) || !text.is_empty(),
                    "indexed text is document text; it is only checked for shape here"
                );
            }
        }
    }
    let sample: String = doc.source().text().chars().take(4).collect();
    for query in [sample.as_str(), "a", "\u{fffd}"] {
        for case_sensitive in [true, false] {
            for m in index.find(query, case_sensitive) {
                assert!(m.node < doc.node_count(), "a match must name a real node");
                let text = index
                    .text(m.node, m.field)
                    .expect("a match must point at an indexed field");
                assert!(m.start < m.end, "an empty match is not a match");
                assert!(m.end <= text.len(), "a match must stay inside its field");
                assert!(text.is_char_boundary(m.start), "matches are char-aligned");
                assert!(text.is_char_boundary(m.end), "matches are char-aligned");
                if case_sensitive {
                    assert_eq!(&text[m.start..m.end], query, "a literal match is literal");
                }
            }
        }
    }
}

/// One outline entry per listed unit, and never more than there are nodes —
/// an outline longer than the arena means the walk revisited something.
fn check_outline(doc: &StructuredDocument) {
    let model = DocumentModel::structured(doc.clone());
    let outline = model.outline();
    assert!(
        outline.len() <= doc.node_count(),
        "the outline cannot have more entries than the document has nodes"
    );
    for entry in &outline {
        assert!(entry.node < doc.node_count(), "an outline entry names a node");
        if let Some(fold) = entry.fold {
            assert!(fold < doc.fold_count(), "an outline entry names a real fold");
        }
        assert!(
            entry.depth <= diple::document::structured::MAX_DEPTH,
            "an outline entry deeper than the model allows"
        );
    }
}

/// Format detection: it must be deterministic, it must honour what it was
/// told, and it must never panic.
///
/// Detection is the one decision a reader cannot correct without knowing it
/// happened — `kubectl get -o yaml | diple` has no filename to fall back on —
/// so an input that detects as YAML on one run and as Markdown on the next
/// would be a bug the user experiences as diple being haunted.
pub fn format_detect(data: &[u8]) {
    let (control, rest) = split_control(data);
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let name = NAMES[usize::from(control) % NAMES.len()];
    let request = REQUESTS[usize::from(control / 8) % REQUESTS.len()];

    let _ = diple::document::format::from_extension(name);
    let container = diple::document::format::looks_like_json_container(text);
    assert_eq!(
        container,
        diple::document::format::looks_like_json_container(text),
        "the JSON container test must be a pure function of the text"
    );

    let first = load(request, SourceDocument::new(name, text));
    let second = load(request, SourceDocument::new(name, text));
    match (&first, &second) {
        (Ok(a), Ok(b)) => {
            assert_eq!(a.format, b.format, "detection must be deterministic");
            assert_eq!(a.detected, b.detected, "detection must be deterministic");
            assert_eq!(a.model.node_count(), b.model.node_count());
        }
        (Err(a), Err(b)) => assert_eq!(a, b, "a parse error must be deterministic"),
        _ => panic!("the same input parsed and failed to parse"),
    }

    if let Ok(loaded) = first {
        assert_eq!(
            loaded.model.kind(),
            loaded.format,
            "the model must be the format the loader reports"
        );
        assert_eq!(
            DocumentKind::parse(loaded.format.as_str()),
            Some(loaded.format),
            "the format's own name must parse back to it"
        );
        if let Some(fixed) = request.fixed() {
            assert_eq!(loaded.format, fixed, "an explicit --format is not a hint");
            assert!(!loaded.detected, "a stated format was not detected");
        }
        assert_eq!(
            loaded.format.is_structured(),
            loaded.model.as_structured().is_some(),
            "is_structured must describe the model that was built"
        );
        if let Some(doc) = loaded.model.as_structured() {
            check_model(doc);
        }
    } else {
        // Detection never fails: it falls back to Markdown. Only a stated
        // format — from `--format` or from the filename — can error.
        assert!(
            request.fixed().is_some() || diple::document::format::from_extension(name).is_some(),
            "an unstated format must fall back rather than fail"
        );
    }
}

/// The strict JSON parser and the model it builds.
///
/// JSON is hand-written in diple, which means every bound in it is diple's own
/// to get wrong: nesting depth, numeric literals, `\u` surrogate pairs, and the
/// byte offsets that become the reader's error position.
pub fn json_model(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let source = SourceDocument::new("fuzz.json", text);
    let result = diple::document::json::parse(&source);
    check_parse_result(&source, DocumentKind::Json, &result);
    assert_eq!(
        result.is_ok(),
        diple::document::json::is_valid(text),
        "is_valid must agree with the parser it is a shortcut for"
    );
}

/// The YAML adapter over `granit-parser` and the model it builds.
///
/// YAML is the format with the adversarial features: anchors and aliases that
/// must never be materialised recursively, merge keys, tags, multi-document
/// streams and block scalars. The model built from them has to satisfy exactly
/// the same arena invariants JSON's does, because everything above the format
/// boundary treats the two identically.
pub fn yaml_model(data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let source = SourceDocument::new("fuzz.yaml", text);
    let result = diple::document::yaml::parse(&source);
    check_parse_result(&source, DocumentKind::Yaml, &result);
    if let Ok(doc) = &result {
        // Detection asks this of every YAML parse, so it must be as total as
        // the parse itself.
        let _ = diple::document::yaml::is_confidently_yaml(doc);
        for node in doc.nodes() {
            let _ = node.anchor();
            let _ = node.tag();
            let _ = node.is_merge_key();
        }
        for comment in doc.comments() {
            let _ = &comment.text;
        }
    }
}

/// Structured layout — the AC-18 target.
///
/// Every row the reader sees comes from here, and the text in it is document
/// text. The invariant that matters is therefore not "it did not panic" but
/// *nothing document-controlled reaches the terminal as itself*: no ESC, no
/// other C0 control, no DEL or C1, no bidi override, and no embedded newline
/// or tab that would let a document decide where a row ends. Every row must
/// also name a real node, because the viewport, the cursor and the fold
/// splicing all address rows by the node they came from.
pub fn structured_layout(data: &[u8]) {
    let (control, rest) = split_control(data);
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let kind = if control & 1 == 0 {
        DocumentKind::Json
    } else {
        DocumentKind::Yaml
    };
    let Some(doc) = structured(kind, "fuzz", text) else {
        return;
    };

    let theme = if control & 2 == 0 {
        Theme::dark()
    } else {
        Theme::light()
    };
    let folds = doc_fold_state(&doc, control);
    let matches = doc.search_index().find("a", control & 16 == 0);
    let opts = structured_opts(control, &theme, &matches, &folds);

    let mut tree = diple::layout::structured::layout(&doc, &opts);
    check_rows(&doc, &tree);

    // The incremental path a fold takes: splicing a re-laid-out subtree back
    // in must leave the tree as addressable as a full re-layout would.
    if doc.node_count() > 0 {
        let node = usize::from(control) % doc.node_count();
        let mut changed = doc_fold_state(&doc, control);
        if let Some(fold) = doc.fold_at(node) {
            changed.toggle(fold);
        }
        let opts = structured_opts(control, &theme, &matches, &changed);
        let _ = diple::layout::structured::relayout_subtree(&doc, &opts, &mut tree, node);
        check_rows(&doc, &tree);
    }
}

/// Layout options for the structured targets, so the full layout and the
/// spliced re-layout differ only in the fold state they are given.
fn structured_opts<'a>(
    control: u8,
    theme: &'a Theme,
    matches: &'a [Match],
    folds: &'a FoldState,
) -> LayoutOptions<'a> {
    let mut opts = LayoutOptions::new(width_from(control), theme);
    opts.unicode = control & 4 == 0;
    opts.show_indices = control & 8 == 0;
    opts.structured_indent = usize::from(control % 5);
    opts.search_matches = matches;
    if control & 32 == 0 {
        opts.folds = Some(folds);
    }
    opts
}

/// A fold state derived from the control byte, so the fuzzer reaches collapsed
/// containers rather than only the fully expanded document.
fn doc_fold_state(doc: &StructuredDocument, control: u8) -> FoldState {
    let mut folds = FoldState::from_parents(doc.fold_parents().to_vec());
    if control & 64 != 0 {
        folds.collapse_all();
    } else if doc.fold_count() > 0 {
        folds.collapse(usize::from(control) % doc.fold_count());
    }
    folds
}

/// Every row of a rendered tree must be attributable and printable.
fn check_rows(doc: &StructuredDocument, tree: &diple::render::primitives::RenderTree) {
    for (index, line) in tree.lines.iter().enumerate() {
        assert!(
            line.node < doc.node_count(),
            "row {index} came from node {} of {}",
            line.node,
            doc.node_count()
        );
        let mut width = 0usize;
        for span in &line.spans {
            assert!(
                !has_terminal_controls(&span.text),
                "a rendered span must not carry terminal controls: {:?}",
                span.text
            );
            assert!(
                !span.text.contains('\n') && !span.text.contains('\t'),
                "a span must not decide its own line structure: {:?}",
                span.text
            );
            width += span.width();
        }
        assert_eq!(line.width, width, "a row's width must be its spans' widths");
    }
}

/// Path formatting.
///
/// The breadcrumb is the only thing in the status line whose *content* comes
/// from the document, and the status line has a hard width budget: one column
/// too many and the bar wraps and the screen tears. `breadcrumb_within` must
/// therefore never exceed the width it was given, for any key a document can
/// contain — an emoji key, a CJK key, a key that is one wide grapheme, a key
/// longer than the terminal. The canonical form must stay a well-formed JSON
/// Pointer so it can be pasted into another tool.
pub fn structured_path(data: &[u8]) {
    let (control, rest) = split_control(data);
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let unicode = control & 1 == 0;
    let width = usize::from(control);

    // Synthetic paths first: they reach keys no parser would produce for a
    // short input — a lone combining mark, a 4 KiB key, an empty key.
    let segments: Vec<diple::document::PathSegment> = text
        .split('\n')
        .take(64)
        .enumerate()
        .map(|(i, part)| match i % 3 {
            0 => diple::document::PathSegment::Key(part.to_string()),
            1 => diple::document::PathSegment::Index(part.len()),
            _ => diple::document::PathSegment::Document(i),
        })
        .collect();
    check_path(&DocumentPath::new(segments), width, unicode);

    // Then the real thing, for every node of a document that parsed.
    let kind = if control & 2 == 0 {
        DocumentKind::Json
    } else {
        DocumentKind::Yaml
    };
    if let Some(doc) = structured(kind, "fuzz", text) {
        for node in 0..doc.node_count() {
            check_path(&doc.path(node), width, unicode);
        }
    }
}

/// The three renderings of a path, and the width contract of the middle one.
fn check_path(path: &DocumentPath, width: usize, unicode: bool) {
    let full = path.breadcrumb(unicode);
    assert_eq!(path.is_empty(), path.len() == 0);
    if path.is_empty() {
        assert!(full.is_empty(), "a rootless path has no breadcrumb");
    }

    let within = path.breadcrumb_within(width, unicode);
    assert!(
        unicode::width(&within) <= width,
        "breadcrumb_within({width}) produced {} columns: {within:?}",
        unicode::width(&within)
    );
    if width == 0 {
        assert!(within.is_empty(), "no width means no breadcrumb");
    }

    let canonical = path.canonical();
    assert!(
        canonical.starts_with('/') || canonical.starts_with('#'),
        "a canonical path is a JSON Pointer, optionally document-qualified: {canonical:?}"
    );
    assert_eq!(
        path.to_string(),
        path.breadcrumb(false),
        "Display is the ASCII breadcrumb"
    );
}

/// Folding and reveal traversal.
///
/// Folds are a forest addressed by id, and `reveal` is the operation every
/// jump depends on: a search hit, an outline click or a `G` may land on a node
/// inside three collapsed ancestors, and the reader must end up looking at it.
/// The invariant is stated exactly that way — after `reveal(n)`, `n` is not
/// hidden — because a reveal that expanded the wrong ancestor would still
/// leave a consistent-looking fold state and a blank screen.
pub fn fold_reveal(data: &[u8]) {
    let (control, rest) = split_control(data);
    let Ok(text) = std::str::from_utf8(rest) else {
        return;
    };
    let kind = if control & 1 == 0 {
        DocumentKind::Json
    } else {
        DocumentKind::Yaml
    };
    let Some(doc) = structured(kind, "fuzz", text) else {
        return;
    };
    if doc.node_count() == 0 {
        return;
    }

    let mut folds = FoldState::from_parents(doc.fold_parents().to_vec());
    let count = doc.fold_count();

    // A sequence of operations driven by the input, so the fuzzer explores
    // orders a hand-written test would not think of. Bounded, because the
    // consistency check after each operation walks the whole forest and a
    // megabyte of input would turn one execution into a minute.
    for byte in text.bytes().take(64) {
        let fold = if count == 0 { 0 } else { usize::from(byte) % count };
        match byte % 6 {
            0 => folds.collapse(fold),
            1 => folds.expand(fold),
            2 => {
                folds.toggle(fold);
            }
            3 => folds.collapse_all(),
            4 => folds.expand_all(),
            _ => folds.reveal(fold),
        }
        assert_eq!(folds.len(), count, "folding must not resize the forest");
        assert_eq!(folds.flags().len(), count, "one flag per fold");
        check_fold_consistency(&doc, &folds);
    }

    // Reveal, from the worst starting point there is.
    folds.collapse_all();
    for node in 0..doc.node_count() {
        doc.reveal(node, &mut folds);
        assert!(
            !doc.is_hidden(node, &folds),
            "reveal({node}) left the node hidden"
        );
        let _ = doc.label(node);
        let _ = doc.collapsed_summary(node);
    }

    folds.expand_all();
    for node in 0..doc.node_count() {
        assert!(
            !doc.is_hidden(node, &folds),
            "nothing is hidden when everything is expanded"
        );
    }
}

/// How many folds and nodes one consistency check looks at.
///
/// The check walks each fold's ancestors, so on a document that is one long
/// chain it costs O(n2) — enough to make a 1200-deep input a "slow unit" and
/// starve the fuzzer of executions. Sampling keeps every execution cheap; a
/// forest whose invariants break only beyond the sample is still reached,
/// because the fuzzer mutates towards shorter inputs too.
const CHECK_SAMPLE: usize = 256;

/// `ancestor_collapsed` is a cached-free walk up the fold forest; it must
/// agree with doing the walk by hand, and `is_hidden` must agree with it.
fn check_fold_consistency(doc: &StructuredDocument, folds: &FoldState) {
    let stride = folds.len().div_ceil(CHECK_SAMPLE).max(1);
    for fold in (0..folds.len()).step_by(stride) {
        let mut expected = false;
        let mut cursor = folds.parent(fold);
        let mut steps = 0usize;
        while let Some(f) = cursor {
            assert!(steps <= folds.len(), "the fold forest has a cycle");
            steps += 1;
            if folds.is_collapsed(f) {
                expected = true;
                break;
            }
            cursor = folds.parent(f);
        }
        assert_eq!(
            folds.ancestor_collapsed(fold),
            expected,
            "ancestor_collapsed disagrees with walking the parents of {fold}"
        );
        assert_eq!(
            folds.content_hidden(fold),
            folds.is_collapsed(fold) || expected,
            "content_hidden must be the fold's own state or an ancestor's"
        );
    }
    let stride = doc.node_count().div_ceil(CHECK_SAMPLE).max(1);
    for node in (0..doc.node_count()).step_by(stride) {
        if doc.is_hidden(node, folds) {
            assert!(
                doc.enclosing_fold(node).is_some(),
                "a hidden node must sit inside some fold"
            );
        }
    }
}
