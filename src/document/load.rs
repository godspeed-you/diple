//! Opening a document: deciding the format, then parsing it.
//!
//! # The policy
//!
//! ```text
//! --format <kind>   →  that format, and a parse error is an error
//! recognised suffix →  that format, and a parse error is an error
//! otherwise         →  content detection, then Markdown
//! ```
//!
//! One exception to the fallback: input that is structured up to the point
//! where it stops — a JSON container or a YAML stream that parses cleanly
//! until the input runs out mid-value — was clearly meant as that format,
//! and is most likely a pipeline that was cut short. Showing it as Markdown
//! would hide that, so it is an error like a stated format's (spec §6.6,
//! §17.2).
//!
//! The asymmetry is deliberate. When the user or the filename *said* what the
//! document is, a parse failure is a fact the reader needs — `config.yaml`
//! that will not parse must say so at the line it broke on, not silently open
//! as Markdown and look almost right. When nothing said, diple is guessing,
//! and a guess that does not pan out simply falls back.
//!
//! # Why detection is conservative
//!
//! `cat notes.md | diple` has no filename, and almost any text is a valid
//! YAML scalar. Content detection therefore only claims a document it is
//! confident about: a JSON object or array that parses strictly, or a YAML
//! stream whose every root is a non-empty collection *and* which shows a
//! structural signal prose does not produce. Everything else stays Markdown,
//! because stealing a reader's prose is worse than asking for `--format`.

use super::error::DocumentError;
use super::format::{self, DocumentKind, FormatRequest};
use super::markdown;
use super::model::DocumentModel;
use super::source::SourceDocument;
use super::{json, yaml};

/// A document that is ready to be read.
#[derive(Debug, Clone)]
pub struct LoadedDocument {
    /// The format it was read as.
    pub format: DocumentKind,
    /// Whether that format was chosen by detection rather than stated.
    pub detected: bool,
    /// The parsed model.
    pub model: DocumentModel,
}

/// Load `source`, honouring an explicit `--format` and otherwise working the
/// format out.
pub fn load(
    request: FormatRequest,
    source: SourceDocument,
) -> Result<LoadedDocument, DocumentError> {
    if let Some(kind) = request.fixed() {
        return parse_as(kind, source, false);
    }
    if let Some(kind) = format::from_extension(source.name()) {
        return parse_as(kind, source, false);
    }
    detect(source)
}

/// Parse `source` as a stated format; a failure is reported rather than
/// worked around.
fn parse_as(
    kind: DocumentKind,
    source: SourceDocument,
    detected: bool,
) -> Result<LoadedDocument, DocumentError> {
    let model = match kind {
        DocumentKind::Markdown => DocumentModel::markdown(markdown::parse_source(source)),
        DocumentKind::Json => DocumentModel::structured(json::parse(&source)?),
        DocumentKind::Yaml => DocumentModel::structured(yaml::parse(&source)?),
    };
    Ok(LoadedDocument {
        format: kind,
        detected,
        model,
    })
}

/// Work the format out from the content, falling back to Markdown — unless
/// the content is structured input that was cut short.
fn detect(source: SourceDocument) -> Result<LoadedDocument, DocumentError> {
    // JSON first: the `{`/`[` test is cheap and decisive, and a document that
    // starts that way and parses strictly is not prose. A bare JSON scalar
    // deliberately does not qualify — `42` and `"hello"` are ordinary text.
    if format::looks_like_json_container(source.text()) {
        match json::parse(&source) {
            Ok(document) => {
                return Ok(LoadedDocument {
                    format: DocumentKind::Json,
                    detected: true,
                    model: DocumentModel::structured(document),
                })
            }
            // Valid JSON up to the end — `{"a": [1, 2`, `{"a": tr` — or
            // input that had already read a member name or a comma before it
            // broke: `{"broken": }` is a broken JSON document. `{not json`
            // and `[a link](url)` fail before either and stay Markdown.
            Err(error) if error.incomplete => return Err(truncated(error)),
            Err(error) if error.committed => return Err(broken(error)),
            Err(_) => {}
        }
    }
    if !starts_indented(source.text())
        && !has_indented_code_block(source.text())
        && probably_yaml(&source)
    {
        match yaml::parse(&source) {
            Ok(document) if yaml::is_confidently_yaml(&document) => {
                return Ok(LoadedDocument {
                    format: DocumentKind::Yaml,
                    detected: true,
                    model: DocumentModel::structured(document),
                });
            }
            Err(error) if yaml_cut_short(&source, &error) => return Err(truncated(error)),
            _ => {}
        }
    }
    Ok(LoadedDocument {
        format: DocumentKind::Markdown,
        detected: true,
        model: DocumentModel::markdown(markdown::parse_source(source)),
    })
}

/// Whether the first line with content on it — past blank lines, comments,
/// directives and `---` — is indented. A YAML stream starts at the margin;
/// Markdown that starts indented is a code block (`# Config` over four
/// indented lines of configuration), which YAML would read as a nested
/// mapping under nothing.
fn starts_indented(text: &str) -> bool {
    text.lines()
        .find(|line| {
            let t = line.trim_start();
            !(t.is_empty() || t.starts_with('#') || t.starts_with('%') || t.starts_with("---"))
        })
        .is_some_and(|line| line.starts_with([' ', '\t']))
}

/// Whether a line after a blank line is indented four or more columns deeper
/// than the line before the blank — a Markdown indented code block
/// (`Example config:` over an indented example). YAML nests two or so
/// columns at a time and does not open a level after a blank line.
fn has_indented_code_block(text: &str) -> bool {
    let indent = |line: &str| line.len() - line.trim_start_matches([' ', '\t']).len();
    let mut previous: Option<usize> = None;
    let mut blank = false;
    for line in text.lines() {
        if line.trim().is_empty() {
            blank = true;
            continue;
        }
        let here = indent(line);
        if blank && previous.is_some_and(|p| here >= p + 4) {
            return true;
        }
        previous = Some(here);
        blank = false;
    }
    false
}

/// How much of a large input the YAML probe reads first.
const PROBE_BYTES: usize = 64 * 1024;

/// A cheap first look before parsing a large input as YAML: its first
/// [`PROBE_BYTES`], cut at a line break. A prefix that parses and is *not*
/// YAML detection would claim settles it — a five-megabyte Markdown list is
/// not worth parsing as YAML to the end to find that out. Anything else —
/// a confident prefix, or one the cut left unparseable — earns the full parse.
fn probably_yaml(source: &SourceDocument) -> bool {
    let text = source.text();
    if text.len() <= PROBE_BYTES {
        return true;
    }
    // Searched as bytes: `PROBE_BYTES` may fall inside a multi-byte
    // character, and a `\n` byte is always a character boundary of its own.
    let Some(cut) = text.as_bytes()[..PROBE_BYTES]
        .iter()
        .rposition(|&b| b == b'\n')
    else {
        return true;
    };
    let prefix = SourceDocument::new(source.name(), &text[..cut + 1]);
    match yaml::parse(&prefix) {
        Ok(doc) => yaml::is_confidently_yaml(&doc),
        Err(_) => true,
    }
}

/// Whether a YAML parse failure is a YAML stream that was cut short: the
/// lines before the failing one are a stream detection would claim, and the
/// parser ran out inside an unclosed quote or bracket. Prose fails too (`'Tis the season` never closes its
/// quote), which is why what came before has to be confidently YAML; an open
/// flow collection needs only a mapping or sequence before it, because prose
/// does not write `b: [1, 2`.
fn yaml_cut_short(source: &SourceDocument, error: &DocumentError) -> bool {
    // Only a construct the parser saw left open. A break on the last line
    // was tried as a signal too, and refused ordinary notes — `Name: Alice`
    // over `Role: Admin` over a closing `Thanks!` — with exit 1; and so did
    // accepting an open bracket after any mapping (`Status: draft` over
    // `[TODO: fill in`). What comes before has to be confidently YAML.
    if !error.incomplete {
        return false;
    }
    let Some(position) = error.position else {
        return false;
    };
    // The parser may point past the construct it could not finish (a quoted
    // scalar cut at a line break is reported where the input ends), so walk
    // back from its line to the last one that starts a parseable prefix.
    let text = source.text();
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    let line = position.line.min(starts.len());
    (1..line)
        .rev()
        .take(TRUNCATED_LINES)
        .map(|l| starts[l])
        .filter(|&end| end > 0)
        .find_map(|end| yaml::parse(&SourceDocument::new(source.name(), &text[..end])).ok())
        .is_some_and(|doc| yaml::is_confidently_yaml(&doc))
}

/// How far back from a failure [`yaml_cut_short`] looks for the start of the
/// construct that was left open: a quoted scalar or flow collection longer
/// than this is not a cut the reader will recognise anyway.
const TRUNCATED_LINES: usize = 16;

/// A parse error in input that is clearly that format, with the reason
/// diple did not fall back to Markdown.
fn broken(mut error: DocumentError) -> DocumentError {
    error.message = format!(
        "{} — the input looks like {} that does not parse; \
         pass `--format markdown` to read it as text",
        error.message,
        error.format.label()
    );
    error
}

/// A parse error, with the reason diple did not fall back to Markdown.
fn truncated(mut error: DocumentError) -> DocumentError {
    // Detection decided the input ran out, even where the parser saw only a
    // key without its colon; the error says so like the parser's own would.
    error.incomplete = true;
    error.message = format!(
        "{} — the input looks like {} that ends early; \
         pass `--format markdown` to read it as text",
        error.message,
        error.format.label()
    );
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(name: &str, text: &str) -> LoadedDocument {
        load(FormatRequest::Auto, SourceDocument::new(name, text)).expect(text)
    }

    fn forced(kind: &str, name: &str, text: &str) -> Result<LoadedDocument, DocumentError> {
        load(
            FormatRequest::parse(kind).expect(kind),
            SourceDocument::new(name, text),
        )
    }

    const YAML: &str = "metadata:\n  name: nginx\nspec:\n  replicas: 3\n";
    const JSON: &str = r#"{"metadata": {"name": "nginx"}}"#;
    const MD: &str = "# Title\n\nSome prose.\n";

    #[test]
    fn a_recognised_suffix_decides_the_format() {
        assert_eq!(open("README.md", MD).format, DocumentKind::Markdown);
        assert_eq!(open("r.json", JSON).format, DocumentKind::Json);
        assert_eq!(open("d.yaml", YAML).format, DocumentKind::Yaml);
        assert_eq!(open("d.yml", YAML).format, DocumentKind::Yaml);
        // A suffix beats the content: this file says it is Markdown.
        assert_eq!(open("notes.md", YAML).format, DocumentKind::Markdown);
        // And a `.json` file may hold a bare scalar, which content detection
        // would never claim.
        let scalar = open("n.json", "42");
        assert_eq!(scalar.format, DocumentKind::Json);
        assert_eq!(scalar.model.node_count(), 1);
    }

    #[test]
    fn an_explicit_format_beats_everything() {
        assert_eq!(
            forced("yaml", "<stdin>", "a: 1\n").unwrap().format,
            DocumentKind::Yaml
        );
        assert_eq!(
            forced("markdown", "config.yaml", YAML).unwrap().format,
            DocumentKind::Markdown
        );
        assert_eq!(
            forced("json", "file.data", JSON).unwrap().format,
            DocumentKind::Json
        );
        assert!(!forced("json", "x", JSON).unwrap().detected);
    }

    #[test]
    fn a_stated_format_that_does_not_parse_is_an_error_not_a_fallback() {
        let error = forced("yaml", "<stdin>", "a: 1\n b: 2\n  c: 3\n").unwrap_err();
        assert_eq!(error.format, DocumentKind::Yaml);
        let error = load(
            FormatRequest::Auto,
            SourceDocument::new("broken.json", "{\"a\":}"),
        )
        .unwrap_err();
        assert_eq!(error.format, DocumentKind::Json);
        assert!(error.position.is_some());
        // Markdown has no parse errors: every byte sequence is some document.
        assert!(forced("markdown", "x.md", "**unbalanced").is_ok());
    }

    #[test]
    fn anonymous_json_and_yaml_are_detected() {
        let d = open("<stdin>", JSON);
        assert_eq!(d.format, DocumentKind::Json);
        assert!(d.detected);
        assert_eq!(open("<stdin>", "[1, 2, 3]").format, DocumentKind::Json);
        assert_eq!(open("<stdin>", YAML).format, DocumentKind::Yaml);
        // What `kubectl -o yaml` produces.
        let kube = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: nginx\nspec:\n  replicas: 3\n";
        assert_eq!(open("<stdin>", kube).format, DocumentKind::Yaml);
    }

    #[test]
    fn anonymous_prose_stays_markdown() {
        for text in [
            "hello\n",
            "- one\n- two\n",
            "title: hello\n",
            MD,
            "42",
            "\"hello\"",
            "true",
            "null",
            "",
            "---\ntitle: Post\n---\n\nProse with **bold**.\n",
            "Note: this is important.\nAlso: check that.\n",
            "Q: why?\nA: because.\n",
        ] {
            assert_eq!(
                open("<stdin>", text).format,
                DocumentKind::Markdown,
                "{text:?}"
            );
        }
    }

    #[test]
    fn broken_anonymous_input_falls_back_rather_than_failing() {
        // Nothing said this was JSON, so a `{` that does not parse is just a
        // Markdown document that starts with a brace.
        for text in [
            "{not json at all",
            "[a link](https://example.com) to start with\n",
            "[1, 2] is a list, {x} a set\n",
            "'Tis the season\n",
            "\"Unfinished quote\n",
            "Note: this is important.\n\"And this",
            // Key-value lines over a closing line of prose, and a bracketed
            // title: notes and Markdown, not broken YAML or JSON.
            "Name: Alice\nRole: Admin\n\nThanks!\n",
            "# ADR 1\n\nStatus: Accepted\nDate: 2024-03-01\n\n## Context\n\nWe need X.\n",
            "[2024, the year] in review\n",
            "[1, Smith et al.] showed this.\n",
            "# Config\n\n    server:\n      port: 80\n",
            "# Setup\n\nExample config:\n\n    server:\n      port: 80\n",
            "Example:\n\n    server:\n      port: 80\n",
            // An open bracket after lines that are not confidently YAML.
            "a: 1\nb: [1, 2",
            "- buy milk\n- call mom\n\n[later: the rest of the list\n",
            "Status: draft\nOwner: me\n\n[TODO: fill in the rest",
            "Note: see below\n\n{placeholder text",
            // An HTML entity or a `!` at the start of a list item, and a
            // wrapped item whose lines both hold a colon.
            "- &copy; 2024 ACME\n- All rights reserved\n",
            "- !important: read this\n- then that\n",
            "- Fix: the cache is cleared on restart. See also the\n  release notes: they explain it.\n",
            "- Stack-Footprint: Keycloak, NATS, Postgres gleichzeitig\n  Gegenmassnahme: vorbefuelltes Compose-Profil.\n",
        ] {
            let d = load(FormatRequest::Auto, SourceDocument::new("<stdin>", text))
                .unwrap_or_else(|e| panic!("{text:?}: {e:?}"));
            assert_eq!(d.format, DocumentKind::Markdown, "{text:?}");
        }
    }

    /// Regression: the probe sliced the text at a byte count, which panicked
    /// when a multi-byte character straddled it.
    #[test]
    fn a_large_input_with_a_multibyte_character_at_the_probe_edge_opens() {
        for pad in 0..4 {
            let mut text = "x".repeat(PROBE_BYTES - 2 + pad);
            while text.len() < PROBE_BYTES + 100 {
                text.push_str("Grüße 日本語 ");
            }
            text.push('\n');
            let d = load(FormatRequest::Auto, SourceDocument::new("<stdin>", &text)).unwrap();
            assert_eq!(d.format, DocumentKind::Markdown);
        }
        let mut yaml = String::from("a:\n  b: 1\n");
        while yaml.len() < PROBE_BYTES + 100 {
            yaml.push_str("  k: \"日本語\"\n");
        }
        assert!(load(FormatRequest::Auto, SourceDocument::new("<stdin>", &yaml)).is_ok());
    }

    /// The probe settles a large input on its first 64 KiB: a Markdown list
    /// far past it stays Markdown, and a large YAML stream is still read in
    /// full once its start is confidently YAML.
    #[test]
    fn the_probe_decides_a_large_input_on_its_start() {
        let mut list = String::new();
        while list.len() < 4 * PROBE_BYTES {
            list.push_str("- Item: a note with a colon in it\n");
        }
        let d = load(FormatRequest::Auto, SourceDocument::new("<stdin>", &list)).unwrap();
        assert_eq!(d.format, DocumentKind::Markdown);

        let mut yaml = String::from("items:\n");
        while yaml.len() < 4 * PROBE_BYTES {
            yaml.push_str("  - name: web\n    image: nginx\n");
        }
        let d = load(FormatRequest::Auto, SourceDocument::new("<stdin>", &yaml)).unwrap();
        assert_eq!(d.format, DocumentKind::Yaml);
        assert!(
            d.model.node_count() > 10_000,
            "read to the end, not just the probe"
        );
    }

    /// Spec §17.2: input that had already read a member name or a comma is a
    /// broken JSON document, not Markdown — and neither is a thousand
    /// opening brackets.
    #[test]
    fn broken_json_that_committed_to_being_json_is_an_error() {
        let deep = format!("{}{}", "[".repeat(5000), "]".repeat(5000));
        for text in [r#"{"broken": }"#, r#"[{"a": 1}, oops]"#, deep.as_str()] {
            let error =
                load(FormatRequest::Auto, SourceDocument::new("<stdin>", text)).expect_err(text);
            assert_eq!(error.format, DocumentKind::Json);
            assert!(error.message.contains("--format markdown"), "{error:?}");
        }
    }

    /// Spec §6.6: structured input cut short is an error, not Markdown that
    /// looks almost right.
    #[test]
    fn truncated_structured_input_is_an_error() {
        for (text, kind) in [
            (r#"{"a": [1, 2"#, DocumentKind::Json),
            ("[\n  {\"name\": \"x\"},\n  {\"name\": ", DocumentKind::Json),
            (
                "apiVersion: v1\nkind: Pod\nmetadata:\n  name: web\nspec:\n  image: \"nginx:1.",
                DocumentKind::Yaml,
            ),
            ("metadata:\n  name: web\nflow: {x: 1, y:", DocumentKind::Yaml),
            (r#"{"a": tr"#, DocumentKind::Json),
            // A multi-line quoted value cut at a line break, as `head -n`
            // leaves it.
            (
                "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: c\ndata:\n  note: \"first line\n",
                DocumentKind::Yaml,
            ),
        ] {
            let error =
                load(FormatRequest::Auto, SourceDocument::new("<stdin>", text)).expect_err(text);
            assert_eq!(error.format, kind, "{text:?}");
            assert!(error.message.contains("--format markdown"), "{error:?}");
        }
    }

    #[test]
    fn the_model_matches_the_format_it_reports() {
        assert!(open("a.md", MD).model.as_markdown().is_some());
        assert!(open("a.json", JSON).model.as_structured().is_some());
        assert!(open("a.yaml", YAML).model.as_structured().is_some());
        assert_eq!(
            open("a.yaml", YAML).model.kind(),
            DocumentKind::Yaml,
            "the structured model remembers which dialect it is"
        );
    }

    #[test]
    fn the_source_stays_with_the_document() {
        for (name, text) in [("a.md", MD), ("a.json", JSON), ("a.yaml", YAML)] {
            let d = open(name, text);
            assert_eq!(d.model.source().name(), name);
            assert_eq!(d.model.source().text(), text);
        }
    }
}
