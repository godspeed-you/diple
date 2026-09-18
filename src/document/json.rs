//! The JSON backend.
//!
//! # Why diple parses JSON itself
//!
//! A reader has requirements a data binder does not. Members must stay in
//! **source order**, because that is the order the producer chose and the
//! order a human scans. Duplicate keys must stay **duplicated**, because a
//! DOM that keeps only the last one silently deletes part of the document
//! being read. Numbers must keep their **lexical form**, because `1e6` is not
//! improved by being shown as `1000000`. And every node needs a **source
//! span**, for error excerpts and for anything a future source view wants.
//!
//! A `Value`-style DOM gives up the first three and a deserializer gives up
//! all four, so the parser is written here: about three hundred lines of
//! RFC 8259, no dependency, and the model built directly as it goes.
//!
//! # Strictness
//!
//! Strict RFC 8259 and nothing else: no comments, no trailing commas, no
//! single quotes, no unquoted keys, no `NaN`. A file that needs those is not
//! JSON, and quietly accepting it would make diple disagree with every other
//! tool in the pipeline about what the document says.
//!
//! # Depth
//!
//! The parser is iterative and carries an explicit stack, so nesting costs
//! heap rather than call frames; past [`MAX_DEPTH`] it reports an ordinary
//! parse error. `[[[[…]]]]` from an untrusted source is a message, not a
//! crash.

use super::error::DocumentError;
use super::format::DocumentKind;
use super::source::{SourceDocument, SourceSpan};
use super::structured::ast::{
    NodeRelation, ScalarKind, ScalarValue, StructuredDocument, StructuredKey, MAX_DEPTH,
};
use super::structured::Builder;

/// Parse strict JSON into the structured model.
pub fn parse(source: &SourceDocument) -> Result<StructuredDocument, DocumentError> {
    let mut parser = Parser::new(source);
    parser.run()?;
    Ok(parser.into_document(source.clone()))
}

/// Whether `text` is valid JSON — used by format detection, which must not
/// commit to JSON for something that will fail to parse a moment later.
pub fn is_valid(text: &str) -> bool {
    let source = SourceDocument::new("<probe>", text);
    Parser::new(&source).run().is_ok()
}

/// What a container frame needs to remember.
#[derive(Debug, Clone, Copy)]
enum Frame {
    Object,
    Array { next_index: usize },
}

struct Parser<'a> {
    text: &'a [u8],
    source: &'a SourceDocument,
    pos: usize,
    builder: Builder,
    frames: Vec<Frame>,
}

impl<'a> Parser<'a> {
    fn new(source: &'a SourceDocument) -> Self {
        let text = source.text().as_bytes();
        let mut builder = Builder::new(DocumentKind::Json);
        // One node per ~16 bytes is a fair guess for object-heavy JSON and
        // saves most of the reallocations on a large document.
        builder.reserve(text.len() / 16);
        Self {
            text,
            source,
            pos: 0,
            builder,
            frames: Vec::new(),
        }
    }

    fn into_document(self, source: SourceDocument) -> StructuredDocument {
        self.builder.finish(source)
    }

    // ---- scanning --------------------------------------------------------

    fn peek(&self) -> Option<u8> {
        self.text.get(self.pos).copied()
    }

    fn bump(&mut self) {
        self.pos += 1;
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// JSON whitespace: space, tab, LF, CR. Nothing else, and in particular
    /// no comments.
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.bump();
        }
    }

    fn error(&self, at: usize, message: impl Into<String>) -> DocumentError {
        DocumentError::at(self.source, DocumentKind::Json, at, message)
    }

    fn expected(&self, what: &str) -> DocumentError {
        let found = match self.peek() {
            Some(b) => format!("found {}", describe(b)),
            None => "reached the end of the input".to_string(),
        };
        self.error(self.pos, format!("expected {what}, {found}"))
    }

    fn depth_error(&self) -> DocumentError {
        self.error(
            self.pos,
            format!("nested more than {MAX_DEPTH} levels deep; diple refuses to go further"),
        )
    }

    // ---- the state machine ----------------------------------------------

    fn run(&mut self) -> Result<(), DocumentError> {
        self.builder.begin_document(true, Vec::new());
        self.skip_ws();
        if self.peek().is_none() {
            return Err(self.error(self.pos, "the document is empty"));
        }
        self.parse_values()?;
        self.skip_ws();
        if self.pos < self.text.len() {
            return Err(self.error(
                self.pos,
                "trailing content after the value; JSON holds exactly one document",
            ));
        }
        Ok(())
    }

    /// Read one value, then everything that follows it, without recursing.
    fn parse_values(&mut self) -> Result<(), DocumentError> {
        let mut relation = NodeRelation::Root { document: 0 };
        'value: loop {
            self.skip_ws();
            let start = self.pos;
            let opened = match self.peek() {
                Some(b'{') => {
                    self.bump();
                    self.builder
                        .open_mapping(relation, SourceSpan::new(start, self.pos), None)
                        .map_err(|_| self.depth_error())?;
                    self.frames.push(Frame::Object);
                    true
                }
                Some(b'[') => {
                    self.bump();
                    self.builder
                        .open_sequence(relation, SourceSpan::new(start, self.pos), None)
                        .map_err(|_| self.depth_error())?;
                    self.frames.push(Frame::Array { next_index: 0 });
                    true
                }
                Some(_) => {
                    self.scalar(relation)?;
                    false
                }
                None => return Err(self.expected("a value")),
            };

            if opened {
                self.skip_ws();
                let (close, is_object) = match self.frames.last() {
                    Some(Frame::Object) => (b'}', true),
                    _ => (b']', false),
                };
                if self.eat(close) {
                    self.close_container();
                } else {
                    relation = if is_object {
                        self.object_key()?
                    } else {
                        NodeRelation::SequenceItem { index: 0 }
                    };
                    continue 'value;
                }
            }

            // The value is complete: close containers that end here, or take
            // the next member of the innermost one.
            loop {
                self.skip_ws();
                let Some(frame) = self.frames.last().copied() else {
                    return Ok(());
                };
                match frame {
                    Frame::Object => {
                        if self.eat(b',') {
                            relation = self.object_key()?;
                            continue 'value;
                        }
                        if self.eat(b'}') {
                            self.close_container();
                            continue;
                        }
                        return Err(self.expected("`,` or `}`"));
                    }
                    Frame::Array { next_index } => {
                        if self.eat(b',') {
                            let index = next_index + 1;
                            if let Some(Frame::Array { next_index }) = self.frames.last_mut() {
                                *next_index = index;
                            }
                            relation = NodeRelation::SequenceItem { index };
                            continue 'value;
                        }
                        if self.eat(b']') {
                            self.close_container();
                            continue;
                        }
                        return Err(self.expected("`,` or `]`"));
                    }
                }
            }
        }
    }

    fn close_container(&mut self) {
        if let Some(id) = self.builder.current_container() {
            self.builder.set_span_end(id, self.pos);
        }
        self.builder.close();
        self.frames.pop();
    }

    /// Read `"key" :` and return the relation the value will carry.
    fn object_key(&mut self) -> Result<NodeRelation, DocumentError> {
        self.skip_ws();
        let start = self.pos;
        if self.peek() != Some(b'"') {
            return Err(self.expected("a `\"`-quoted member name"));
        }
        let text = self.string()?;
        let span = SourceSpan::new(start, self.pos);
        self.skip_ws();
        if !self.eat(b':') {
            return Err(self.expected("`:` after the member name"));
        }
        Ok(NodeRelation::MappingEntry {
            key: StructuredKey {
                text,
                span,
                complex: false,
            },
        })
    }

    fn scalar(&mut self, relation: NodeRelation) -> Result<(), DocumentError> {
        let start = self.pos;
        let value = match self.peek() {
            Some(b'"') => {
                let text = self.string()?;
                ScalarValue::string(text)
            }
            Some(b't') => {
                self.keyword("true")?;
                ScalarValue::plain("true", ScalarKind::Boolean)
            }
            Some(b'f') => {
                self.keyword("false")?;
                ScalarValue::plain("false", ScalarKind::Boolean)
            }
            Some(b'n') => {
                self.keyword("null")?;
                ScalarValue::plain("null", ScalarKind::Null)
            }
            Some(b'-' | b'0'..=b'9') => {
                let text = self.number()?;
                ScalarValue::plain(text, ScalarKind::Number)
            }
            _ => return Err(self.expected("a value")),
        };
        self.builder
            .scalar(relation, value, SourceSpan::new(start, self.pos), None);
        Ok(())
    }

    fn keyword(&mut self, word: &str) -> Result<(), DocumentError> {
        let end = self.pos + word.len();
        if self.text.get(self.pos..end) == Some(word.as_bytes()) {
            self.pos = end;
            Ok(())
        } else {
            Err(self.expected(&format!("`{word}`")))
        }
    }

    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?`, returned verbatim.
    fn number(&mut self) -> Result<String, DocumentError> {
        let start = self.pos;
        self.eat(b'-');
        match self.peek() {
            Some(b'0') => self.bump(),
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.bump();
                }
            }
            _ => return Err(self.expected("a digit")),
        }
        if self.eat(b'.') {
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.expected("a digit after the decimal point"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.expected("a digit in the exponent"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.bump();
            }
        }
        // The slice is ASCII by construction, so this cannot fail.
        Ok(String::from_utf8_lossy(&self.text[start..self.pos]).into_owned())
    }

    /// A `"`-quoted string, decoded.
    fn string(&mut self) -> Result<String, DocumentError> {
        let open = self.pos;
        if !self.eat(b'"') {
            return Err(self.expected("a `\"`"));
        }
        let mut out = String::new();
        let mut run_start = self.pos;
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error(open, "unterminated string"));
            };
            match byte {
                b'"' => {
                    out.push_str(&self.slice(run_start, self.pos));
                    self.bump();
                    return Ok(out);
                }
                b'\\' => {
                    out.push_str(&self.slice(run_start, self.pos));
                    self.bump();
                    self.escape(&mut out)?;
                    run_start = self.pos;
                }
                0x00..=0x1f => {
                    return Err(self.error(
                        self.pos,
                        format!("{} must be escaped inside a string", describe(byte)),
                    ));
                }
                _ => self.bump(),
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), DocumentError> {
        let at = self.pos;
        let Some(byte) = self.peek() else {
            return Err(self.error(at, "unterminated escape"));
        };
        self.bump();
        let c = match byte {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.unicode_escape(at, out),
            other => {
                return Err(self.error(at, format!("unknown escape `\\{}`", describe_raw(other))))
            }
        };
        out.push(c);
        Ok(())
    }

    /// `\uXXXX`, joining a surrogate pair when one follows.
    fn unicode_escape(&mut self, at: usize, out: &mut String) -> Result<(), DocumentError> {
        let first = self.hex4(at)?;
        // A high surrogate is only meaningful with its low half; a lone one is
        // shown as the replacement character rather than refused, because a
        // reader is better served by seeing the rest of the document.
        if (0xd800..0xdc00).contains(&first) {
            if self.text.get(self.pos..self.pos + 2) == Some(b"\\u") {
                let save = self.pos;
                self.pos += 2;
                let second = self.hex4(at)?;
                if (0xdc00..0xe000).contains(&second) {
                    let combined =
                        0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
                    out.push(char::from_u32(combined).unwrap_or(char::REPLACEMENT_CHARACTER));
                    return Ok(());
                }
                self.pos = save;
            }
            out.push(char::REPLACEMENT_CHARACTER);
            return Ok(());
        }
        out.push(char::from_u32(first).unwrap_or(char::REPLACEMENT_CHARACTER));
        Ok(())
    }

    fn hex4(&mut self, at: usize) -> Result<u32, DocumentError> {
        let Some(digits) = self.text.get(self.pos..self.pos + 4) else {
            return Err(self.error(at, "`\\u` needs four hexadecimal digits"));
        };
        let mut value = 0u32;
        for byte in digits {
            let digit = match byte {
                b'0'..=b'9' => u32::from(byte - b'0'),
                b'a'..=b'f' => u32::from(byte - b'a') + 10,
                b'A'..=b'F' => u32::from(byte - b'A') + 10,
                _ => return Err(self.error(at, "`\\u` needs four hexadecimal digits")),
            };
            value = value * 16 + digit;
        }
        self.pos += 4;
        Ok(value)
    }

    /// The source between two offsets. The parser only ever slices at
    /// character boundaries, so the lossy conversion never changes anything.
    fn slice(&self, from: usize, to: usize) -> String {
        String::from_utf8_lossy(&self.text[from..to]).into_owned()
    }
}

/// How a byte reads in an error message.
fn describe(byte: u8) -> String {
    match byte {
        b'\n' => "a line break".to_string(),
        b'\t' => "a tab".to_string(),
        b'\r' => "a carriage return".to_string(),
        0x00..=0x1f | 0x7f => format!("the control character 0x{byte:02x}"),
        _ => format!("`{}`", describe_raw(byte)),
    }
}

fn describe_raw(byte: u8) -> String {
    char::from_u32(u32::from(byte))
        .filter(|c| !c.is_control())
        .map_or_else(|| format!("0x{byte:02x}"), |c| c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::search::MatchField;
    use crate::document::structured::StructuredNodeKind;

    fn doc(src: &str) -> StructuredDocument {
        parse(SourceDocument::new("t.json", src)).expect(src)
    }

    fn err(src: &str) -> DocumentError {
        parse(SourceDocument::new("t.json", src)).expect_err(src)
    }

    /// The document as `depth:label kind value` lines — a compact way to
    /// assert the whole shape at once.
    fn shape(d: &StructuredDocument) -> Vec<String> {
        d.nodes()
            .iter()
            .map(|n| {
                let what = match &n.kind {
                    StructuredNodeKind::Mapping => "{}".to_string(),
                    StructuredNodeKind::Sequence => "[]".to_string(),
                    StructuredNodeKind::Scalar(v) => format!("{:?} {}", v.kind, v.display()),
                    StructuredNodeKind::Alias { name } => format!("*{name}"),
                };
                format!("{}{} {what}", "  ".repeat(n.depth), d.label(n.id))
            })
            .collect()
    }

    #[test]
    fn objects_arrays_and_scalars_build_the_expected_tree() {
        let d = doc(r#"{"m":{"n":"x"},"a":[1,true,null],"e":{},"l":[]}"#);
        assert_eq!(
            shape(&d),
            [
                "root {}",
                "  m {}",
                "    n String x",
                "  a []",
                "    [0] Number 1",
                "    [1] Boolean true",
                "    [2] Null null",
                "  e {}",
                "  l []",
            ]
        );
        assert_eq!(d.node(4).unwrap().child_count, 0, "an empty object");
        assert_eq!(d.collapsed_summary(1).as_deref(), Some("{1 member}"));
        assert_eq!(d.collapsed_summary(3).as_deref(), Some("[3 items]"));
        assert_eq!(d.collapsed_summary(7).as_deref(), Some("{0 members}"));
    }

    #[test]
    fn member_order_is_source_order_not_sorted() {
        let d = doc(r#"{"z":1,"a":2,"m":3}"#);
        let keys: Vec<String> = d.children(0).into_iter().map(|id| d.label(id)).collect();
        assert_eq!(keys, ["z", "a", "m"]);
    }

    #[test]
    fn duplicate_members_are_both_kept() {
        // A map-shaped parser would silently drop the first one, deleting
        // part of the document a reader opened diple to look at.
        let d = doc(r#"{"x":1,"x":2}"#);
        assert_eq!(d.children(0).len(), 2);
        assert_eq!(
            shape(&d),
            ["root {}", "  x Number 1", "  x Number 2"]
        );
        // The human path is the same for both; the node id is what tells them
        // apart, and the canonical path is what a reader can copy.
        assert_eq!(d.path(1).breadcrumb(false), "x");
        assert_eq!(d.path(2).breadcrumb(false), "x");
        assert_ne!(d.node(1).unwrap().span, d.node(2).unwrap().span);
    }

    #[test]
    fn numbers_keep_the_form_they_were_written_in() {
        let d = doc(r#"[1, 1.0, 1e6, -0, 2.5E-3, 1234567890123456789012345]"#);
        let shown: Vec<&str> = d
            .children(0)
            .into_iter()
            .filter_map(|id| d.node(id)?.scalar().map(|v| v.display()))
            .collect();
        assert_eq!(
            shown,
            ["1", "1.0", "1e6", "-0", "2.5E-3", "1234567890123456789012345"]
        );
    }

    #[test]
    fn strings_are_decoded_including_surrogate_pairs() {
        let d = doc(r#"{"a":"tab\there","b":"ü","c":"𝄞","d":"quote\"slash\/"}"#);
        let values: Vec<&str> = d
            .children(0)
            .into_iter()
            .filter_map(|id| d.node(id)?.scalar().map(|v| v.text.as_str()))
            .collect();
        assert_eq!(values, ["tab\there", "ü", "𝄞", "quote\"slash/"]);
    }

    #[test]
    fn a_lone_surrogate_becomes_the_replacement_character() {
        let d = doc(r#"["\uD834"]"#);
        assert_eq!(d.node(1).unwrap().scalar().unwrap().text, "\u{fffd}");
    }

    #[test]
    fn a_scalar_root_is_a_one_node_document() {
        for (src, kind) in [
            ("42", ScalarKind::Number),
            (r#""hello""#, ScalarKind::String),
            ("true", ScalarKind::Boolean),
            ("null", ScalarKind::Null),
        ] {
            let d = doc(src);
            assert_eq!(d.node_count(), 1, "{src}");
            assert_eq!(d.node(0).unwrap().scalar().unwrap().kind, kind, "{src}");
            assert_eq!(d.path(0).canonical(), "/", "{src}");
            assert_eq!(d.fold_count(), 0, "a scalar root has nothing to fold");
            assert_eq!(d.first_semantic(), Some(0));
        }
    }

    #[test]
    fn unicode_survives_everywhere() {
        let d = doc(r#"{"キー":"値 🎵","ü":["ß"]}"#);
        assert_eq!(d.label(1), "キー");
        assert_eq!(d.node(1).unwrap().scalar().unwrap().text, "値 🎵");
        assert_eq!(d.path(4).breadcrumb(false), "ü > [0]");
    }

    #[test]
    fn keys_and_values_are_both_searchable_and_tell_each_other_apart() {
        let d = doc(r#"{"image":"image:1.27"}"#);
        let hits = d.search_index().find("image", false);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].field, MatchField::Label);
        assert_eq!(hits[1].field, MatchField::Value);
        assert_eq!(hits[0].node, hits[1].node, "one row, two fields");
    }

    #[test]
    fn spans_point_back_into_the_source() {
        let src = r#"{"a": 42}"#;
        let d = doc(src);
        let value = d.node(1).unwrap();
        assert_eq!(value.span.slice(src), Some("42"));
        assert_eq!(value.key().unwrap().span.slice(src), Some("\"a\""));
        assert_eq!(d.node(0).unwrap().span.slice(src), Some(src));
    }

    #[test]
    fn deep_nesting_is_an_error_rather_than_a_crash() {
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 10), "]".repeat(MAX_DEPTH + 10));
        let error = err(&deep);
        assert!(error.message.contains("nested more than"), "{error}");
        // Just inside the limit still parses.
        let ok = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(SourceDocument::new("t.json", &ok)).is_ok());
    }

    #[test]
    fn malformed_input_says_where_and_what() {
        for (src, needle) in [
            ("", "empty"),
            ("{", "expected"),
            (r#"{"a"}"#, "`:`"),
            (r#"{"a":}"#, "a value"),
            (r#"{"a":1,}"#, "member name"),
            (r#"[1,]"#, "a value"),
            (r#"{'a':1}"#, "member name"),
            (r#"{"a":1}{"b":2}"#, "trailing content"),
            (r#"{"a":01}"#, "`,` or `}`"),
            (r#"{"a":1.}"#, "decimal point"),
            (r#"{"a":1e}"#, "exponent"),
            (r#"{"a":"unterminated}"#, "unterminated string"),
            (r#"{"a":"\q"}"#, "unknown escape"),
            (r#"{"a":"\u00"}"#, "hexadecimal"),
            ("{\"a\":\"line\nbreak\"}", "must be escaped"),
            ("nul", "`null`"),
            ("// a comment\n{}", "a value"),
        ] {
            let error = err(src);
            assert!(
                error.message.contains(needle),
                "{src:?}: expected {needle:?} in {:?}",
                error.message
            );
            assert!(error.position.is_some(), "{src:?} has a position");
            assert_eq!(error.format, DocumentKind::Json);
        }
    }

    #[test]
    fn an_error_report_quotes_the_line() {
        let error = err("{\n  \"a\": 1,\n  \"b\"\n}\n");
        let report = error.report();
        assert!(report.starts_with("JSON parse error\nt.json:"), "{report}");
        assert!(report.contains('^'), "{report}");
    }

    #[test]
    fn validity_probing_agrees_with_parsing() {
        for src in ["{}", "[]", "1", r#"{"a":[1,{"b":null}]}"#, "  \n {} \n "] {
            assert!(is_valid(src), "{src}");
        }
        for src in ["", "{", "{,}", "nope", "{} {}", "'x'"] {
            assert!(!is_valid(src), "{src}");
        }
    }

    #[test]
    fn whitespace_between_every_token_is_accepted() {
        let d = doc(" \n\t{ \"a\" : [ 1 , 2 ] , \"b\" : { } } \r\n");
        assert_eq!(d.node_count(), 5);
        assert_eq!(d.children(0).len(), 2);
    }
}
