//! Which format a source is, and how diple works that out.
//!
//! Detection matters because a pipeline is a first-class way to open a
//! document: `kubectl get deploy -o yaml | diple` has no filename to look at.
//! The policy is deliberately conservative — Markdown is the fallback and
//! ambiguous text stays Markdown, because stealing prose from the reader is
//! worse than asking for `--format yaml` on the rare file that needs it.

use std::path::Path;

/// A document format diple can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DocumentKind {
    /// CommonMark with GitHub extensions.
    #[default]
    Markdown,
    /// Strict JSON.
    Json,
    /// YAML 1.2.
    Yaml,
}

impl DocumentKind {
    /// The lowercase name used by `--format` and the configuration.
    pub fn as_str(self) -> &'static str {
        match self {
            DocumentKind::Markdown => "markdown",
            DocumentKind::Json => "json",
            DocumentKind::Yaml => "yaml",
        }
    }

    /// The name shown in the status line.
    pub fn label(self) -> &'static str {
        match self {
            DocumentKind::Markdown => "Markdown",
            DocumentKind::Json => "JSON",
            DocumentKind::Yaml => "YAML",
        }
    }

    /// Whether this format is read through the structured document model.
    pub fn is_structured(self) -> bool {
        matches!(self, DocumentKind::Json | DocumentKind::Yaml)
    }

    /// Parse a `--format`/config value (`markdown`, `json`, `yaml`).
    pub fn parse(value: &str) -> Option<DocumentKind> {
        match value {
            "markdown" | "md" => Some(DocumentKind::Markdown),
            "json" => Some(DocumentKind::Json),
            "yaml" | "yml" => Some(DocumentKind::Yaml),
            _ => None,
        }
    }
}

/// What the user asked for: a fixed format, or "work it out".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormatRequest {
    /// Extension, then content, then Markdown.
    #[default]
    Auto,
    /// An explicit `--format`; detection is skipped entirely.
    Fixed(DocumentKind),
}

impl FormatRequest {
    /// Parse a `--format` value (`auto`, `markdown`, `json`, `yaml`).
    pub fn parse(value: &str) -> Option<FormatRequest> {
        match value {
            "auto" => Some(FormatRequest::Auto),
            other => DocumentKind::parse(other).map(FormatRequest::Fixed),
        }
    }

    /// The canonical spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            FormatRequest::Auto => "auto",
            FormatRequest::Fixed(kind) => kind.as_str(),
        }
    }

    /// The explicitly requested format, if any.
    pub fn fixed(self) -> Option<DocumentKind> {
        match self {
            FormatRequest::Auto => None,
            FormatRequest::Fixed(kind) => Some(kind),
        }
    }
}

/// The accepted `--format` values, in help order.
pub const FORMAT_VALUES: &[&str] = &["auto", "markdown", "json", "yaml"];

/// The format a filename's extension announces, if diple recognises it.
///
/// `.jsonc` is deliberately absent: diple's JSON is strict JSON, and quietly
/// treating a commented file as strict JSON would only produce a confusing
/// parse error at the first `//`. `--format json` still forces it.
pub fn from_extension(name: &str) -> Option<DocumentKind> {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" | "mdown" | "mkd" => Some(DocumentKind::Markdown),
        "json" => Some(DocumentKind::Json),
        "yaml" | "yml" => Some(DocumentKind::Yaml),
        _ => None,
    }
}

/// Whether `text` opens with a JSON object or array.
///
/// A bare scalar (`42`, `"hello"`, `true`, `null`) is valid JSON but is also
/// ordinary prose, so it never triggers content detection — a `.json`
/// extension or `--format json` is what makes a scalar root a JSON document.
pub fn looks_like_json_container(text: &str) -> bool {
    matches!(
        text.trim_start_matches(|c: char| c.is_whitespace())
            .as_bytes()
            .first(),
        Some(b'{') | Some(b'[')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_formats() {
        assert_eq!(from_extension("README.md"), Some(DocumentKind::Markdown));
        assert_eq!(from_extension("a/b.markdown"), Some(DocumentKind::Markdown));
        assert_eq!(from_extension("x.MKD"), Some(DocumentKind::Markdown));
        assert_eq!(from_extension("r.json"), Some(DocumentKind::Json));
        assert_eq!(from_extension("d.yaml"), Some(DocumentKind::Yaml));
        assert_eq!(from_extension("d.yml"), Some(DocumentKind::Yaml));
        // Not recognised: no extension, an unknown one, and JSONC.
        assert_eq!(from_extension("Makefile"), None);
        assert_eq!(from_extension("data.txt"), None);
        assert_eq!(from_extension("tsconfig.jsonc"), None);
        assert_eq!(from_extension("<stdin>"), None);
    }

    #[test]
    fn format_values_round_trip() {
        for value in FORMAT_VALUES {
            let request = FormatRequest::parse(value).expect(value);
            assert_eq!(request.as_str(), *value);
        }
        assert_eq!(FormatRequest::parse("nope"), None);
        assert_eq!(FormatRequest::Auto.fixed(), None);
        assert_eq!(
            FormatRequest::parse("json").and_then(FormatRequest::fixed),
            Some(DocumentKind::Json)
        );
        assert_eq!(DocumentKind::parse("yml"), Some(DocumentKind::Yaml));
    }

    #[test]
    fn only_a_json_container_opens_the_content_detection_door() {
        assert!(looks_like_json_container("{\"a\":1}"));
        assert!(looks_like_json_container("\n  [1, 2]"));
        assert!(!looks_like_json_container("42"));
        assert!(!looks_like_json_container("\"hello\""));
        assert!(!looks_like_json_container("true"));
        assert!(!looks_like_json_container("null"));
        assert!(!looks_like_json_container(""));
        assert!(!looks_like_json_container("# Heading"));
    }

    #[test]
    fn labels_and_structure_flags() {
        assert_eq!(DocumentKind::Markdown.label(), "Markdown");
        assert_eq!(DocumentKind::Json.label(), "JSON");
        assert_eq!(DocumentKind::Yaml.label(), "YAML");
        assert!(!DocumentKind::Markdown.is_structured());
        assert!(DocumentKind::Json.is_structured());
        assert!(DocumentKind::Yaml.is_structured());
        assert_eq!(DocumentKind::default(), DocumentKind::Markdown);
    }
}
