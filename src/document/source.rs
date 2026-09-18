//! The textual source a document was parsed from, and positions inside it.
//!
//! diple is a reader, not a data binder: the source text outlives the parse
//! so that a parse error can quote the offending line, a YAML scalar can be
//! shown in its source spelling, and a future source view has something to
//! show. Every format keeps its [`SourceDocument`]; every semantic node may
//! point back into it with a [`SourceSpan`].

/// Byte range in the original textual source.
///
/// Half-open: `start` is inclusive, `end` exclusive. An empty span
/// (`start == end`) is meaningful — YAML spells an implicit null that way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceSpan {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl SourceSpan {
    /// A span covering `start..end`.
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Whether the span covers no bytes.
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    /// The text this span covers, when it lies on character boundaries of
    /// `source`.
    pub fn slice<'a>(&self, source: &'a str) -> Option<&'a str> {
        source.get(self.start..self.end)
    }
}

/// A named source text, kept for the lifetime of a loaded document.
///
/// The text is reference-counted because format detection has to try a parse
/// before it knows which backend will keep the source: sharing it means a
/// rejected attempt costs nothing and the accepted one costs a pointer, rather
/// than a copy of a document that may be tens of megabytes. Nothing here
/// crosses a thread, so `Rc` is the right count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceDocument {
    name: std::rc::Rc<str>,
    text: std::rc::Rc<str>,
}

impl SourceDocument {
    /// A source named `name` (a path, or `<stdin>`).
    pub fn new(name: impl AsRef<str>, text: impl AsRef<str>) -> Self {
        Self {
            name: name.as_ref().into(),
            text: text.as_ref().into(),
        }
    }

    /// The display name (path or `<stdin>`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The whole source text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 1-based line and column (in characters) of a byte offset.
    ///
    /// An offset past the end resolves to the position just after the last
    /// character, which is where a "unexpected end of input" caret belongs.
    pub fn line_col(&self, byte: usize) -> (usize, usize) {
        let byte = byte.min(self.text.len());
        let mut line = 1usize;
        let mut line_start = 0usize;
        for (idx, ch) in self.text.char_indices() {
            if idx >= byte {
                break;
            }
            if ch == '\n' {
                line += 1;
                line_start = idx + ch.len_utf8();
            }
        }
        let col = self.text[line_start..byte.max(line_start)].chars().count() + 1;
        (line, col)
    }

    /// The 1-based line `line`, without its terminator.
    pub fn line(&self, line: usize) -> Option<&str> {
        if line == 0 {
            return None;
        }
        self.text.lines().nth(line - 1)
    }

    /// Number of lines, counting a trailing partial line.
    pub fn line_count(&self) -> usize {
        self.text.lines().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_are_one_based_and_count_characters() {
        let src = SourceDocument::new("x.yaml", "a: 1\nbü: 2\n");
        assert_eq!(src.line_col(0), (1, 1));
        assert_eq!(src.line_col(3), (1, 4));
        assert_eq!(src.line_col(5), (2, 1));
        // `ü` is two bytes but one column.
        assert_eq!(src.line_col(8), (2, 3));
        assert_eq!(src.line(2), Some("bü: 2"));
        assert_eq!(src.line(3), None);
        assert_eq!(src.line(0), None);
    }

    #[test]
    fn an_offset_past_the_end_lands_after_the_last_character() {
        let src = SourceDocument::new("x", "ab");
        assert_eq!(src.line_col(99), (1, 3));
    }

    #[test]
    fn spans_slice_the_source() {
        let src = SourceDocument::new("x", "hello world");
        assert_eq!(SourceSpan::new(6, 11).slice(src.text()), Some("world"));
        assert!(SourceSpan::new(0, 0).is_empty());
        assert_eq!(SourceSpan::new(1, 4).len(), 3);
        // A span that cuts a character reports nothing rather than panicking.
        let src = SourceDocument::new("x", "ü");
        assert_eq!(SourceSpan::new(0, 1).slice(src.text()), None);
    }
}
