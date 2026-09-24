//! Parse failures, reported the way a reader can act on them.
//!
//! A structured document either parses or it does not, and when it does not
//! the useful answer is *where*. The error therefore carries the source name,
//! the format, a position when the parser gave one, and enough of the source
//! to quote — so the message can be a few lines of the file with a caret
//! under the offending column rather than a sentence about a token.

use std::fmt;

use super::format::DocumentKind;
use super::source::SourceDocument;

/// Lines of context shown above and below the offending line.
const CONTEXT: usize = 2;

/// A position in the source, 1-based as editors count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// 1-based line.
    pub line: usize,
    /// 1-based column, in characters.
    pub column: usize,
}

/// A document could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentError {
    /// Name of the source (a path, or `<stdin>`).
    pub source_name: String,
    /// The format diple was reading it as.
    pub format: DocumentKind,
    /// Where it went wrong, when the parser could say.
    pub position: Option<Position>,
    /// What went wrong, in the parser's words.
    pub message: String,
    /// The lines around `position`, as `(number, text)`.
    excerpt: Vec<(usize, String)>,
    /// The input ran out in the middle of a value — an unclosed string,
    /// bracket or quote — rather than containing something wrong. Content
    /// detection uses it to tell structured input that was cut short from
    /// prose that merely failed to parse (spec §6.6).
    pub incomplete: bool,
}

impl DocumentError {
    /// An error at a byte offset into `source`.
    pub fn at(
        source: &SourceDocument,
        format: DocumentKind,
        byte: usize,
        message: impl Into<String>,
    ) -> Self {
        let (line, column) = source.line_col(byte);
        Self::at_line_col(source, format, line, column, message)
    }

    /// An error at a 1-based line and column, for a parser that reports them
    /// directly.
    pub fn at_line_col(
        source: &SourceDocument,
        format: DocumentKind,
        line: usize,
        column: usize,
        message: impl Into<String>,
    ) -> Self {
        let first = line.saturating_sub(CONTEXT).max(1);
        let last = line.saturating_add(CONTEXT);
        let excerpt = (first..=last)
            .filter_map(|n| source.line(n).map(|text| (n, text.to_string())))
            .collect();
        Self {
            source_name: source.name().to_string(),
            format,
            position: Some(Position { line, column }),
            message: message.into(),
            excerpt,
            incomplete: false,
        }
    }

    /// An error with no position — a parser that only said what, not where.
    pub fn whole(
        source: &SourceDocument,
        format: DocumentKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            source_name: source.name().to_string(),
            format,
            position: None,
            message: message.into(),
            excerpt: Vec::new(),
            incomplete: false,
        }
    }

    /// Mark the error as the input running out (see
    /// [`DocumentError::incomplete`]).
    pub fn ran_out(mut self) -> Self {
        self.incomplete = true;
        self
    }

    /// The full report: heading, position, source excerpt with a caret, and
    /// the parser's message.
    ///
    /// Written for a terminal and for stderr alike — no colour, no control
    /// characters, and every line of the excerpt comes from the source with
    /// its control characters replaced, because a broken document is exactly
    /// the kind that carries an escape sequence.
    pub fn report(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("{} parse error\n", self.format.label()));
        match self.position {
            Some(Position { line, column }) => {
                out.push_str(&format!("{}:{line}:{column}\n", self.source_name));
            }
            None => out.push_str(&format!("{}\n", self.source_name)),
        }
        if !self.excerpt.is_empty() {
            out.push('\n');
            let width = self
                .excerpt
                .iter()
                .map(|(n, _)| n.to_string().len())
                .max()
                .unwrap_or(1);
            for (number, text) in &self.excerpt {
                let here = self.position.is_some_and(|p| p.line == *number);
                let marker = if here { '>' } else { ' ' };
                out.push_str(&format!(
                    "{marker} {number:>width$} | {}\n",
                    sanitize(text),
                    width = width
                ));
                if here {
                    if let Some(p) = self.position {
                        let pad = " ".repeat(width);
                        let caret = " ".repeat(caret_offset(text, p.column));
                        out.push_str(&format!("  {pad} | {caret}^\n"));
                    }
                }
            }
            out.push('\n');
        }
        out.push_str(&self.message);
        out
    }
}

/// How many cells the caret has to be indented to sit under `column`.
fn caret_offset(line: &str, column: usize) -> usize {
    let prefix: String = line.chars().take(column.saturating_sub(1)).collect();
    crate::util::unicode::width(&sanitize(&prefix))
}

/// Replace control characters so that quoting a broken document cannot make
/// the terminal do anything.
fn sanitize(text: &str) -> String {
    crate::util::text::sanitize(text)
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.report())
    }
}

impl std::error::Error for DocumentError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> SourceDocument {
        SourceDocument::new(
            "config.yaml",
            "a: 1\nb: 2\nspec:\n  containers:\n     - name: nginx\n    image: x\nz: 9\n",
        )
    }

    #[test]
    fn the_report_quotes_the_source_and_points_at_the_column() {
        let err = DocumentError::at_line_col(
            &source(),
            DocumentKind::Yaml,
            5,
            6,
            "unexpected indentation",
        );
        let report = err.report();
        assert!(
            report.starts_with("YAML parse error\nconfig.yaml:5:6\n"),
            "{report}"
        );
        assert!(report.contains("> 5 |      - name: nginx"), "{report}");
        assert!(report.contains("  3 | spec:"), "{report}");
        assert!(report.contains("  7 | z: 9"), "{report}");
        assert!(report.ends_with("unexpected indentation"), "{report}");
        // The caret sits under column 6.
        let caret_line = report
            .lines()
            .find(|l| l.contains('^'))
            .expect("a caret line");
        assert_eq!(caret_line.find('^'), Some("    |      ".len() - 1 + 1));
    }

    #[test]
    fn a_byte_offset_resolves_to_a_line_and_column() {
        let src = SourceDocument::new("x.json", "{\n  \"a\": ,\n}\n");
        let err = DocumentError::at(&src, DocumentKind::Json, 9, "unexpected `,`");
        assert_eq!(err.position, Some(Position { line: 2, column: 8 }));
        assert!(err.report().contains("x.json:2:8"));
    }

    #[test]
    fn an_error_without_a_position_still_reads() {
        let err = DocumentError::whole(&source(), DocumentKind::Json, "unexpected end of input");
        let report = err.report();
        assert_eq!(
            report,
            "JSON parse error\nconfig.yaml\nunexpected end of input"
        );
        assert_eq!(err.to_string(), report);
    }

    #[test]
    fn a_control_sequence_in_the_source_cannot_reach_the_terminal() {
        let src = SourceDocument::new("evil.json", "{\n\u{1b}[31mred\u{7}\n}\n");
        let err = DocumentError::at_line_col(&src, DocumentKind::Json, 2, 1, "bad");
        let report = err.report();
        assert!(!report.contains('\u{1b}'), "{report:?}");
        assert!(!report.contains('\u{7}'), "{report:?}");
    }
}
