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
    Ok(detect(source))
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

/// Work the format out from the content, falling back to Markdown.
fn detect(source: SourceDocument) -> LoadedDocument {
    // JSON first: the `{`/`[` test is cheap and decisive, and a document that
    // starts that way and parses strictly is not prose. A bare JSON scalar
    // deliberately does not qualify — `42` and `"hello"` are ordinary text.
    if format::looks_like_json_container(source.text()) {
        if let Ok(document) = json::parse(&source) {
            return LoadedDocument {
                format: DocumentKind::Json,
                detected: true,
                model: DocumentModel::structured(document),
            };
        }
    }
    if let Ok(document) = yaml::parse(&source) {
        if yaml::is_confidently_yaml(&document) {
            return LoadedDocument {
                format: DocumentKind::Yaml,
                detected: true,
                model: DocumentModel::structured(document),
            };
        }
    }
    LoadedDocument {
        format: DocumentKind::Markdown,
        detected: true,
        model: DocumentModel::markdown(markdown::parse_source(source)),
    }
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
        let d = open("<stdin>", "{not json at all");
        assert_eq!(d.format, DocumentKind::Markdown);
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
