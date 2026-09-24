//! The YAML backend.
//!
//! # What a YAML reader has to keep
//!
//! YAML carries more of the author's intent than its data model does, and a
//! reader that throws that away is lying about the file it is showing:
//!
//! * **comments** are the document's prose and must stay visible and
//!   searchable, even though they are not YAML data;
//! * **anchors and aliases** are references. An alias is shown as a reference,
//!   never expanded into a copy of the anchored subtree — that would be both
//!   untrue to the source and, for a document designed to be hostile, an
//!   exponential amount of work;
//! * **merge keys** (`<<: *defaults`) stay as the entry the author wrote.
//!   diple reads a document; it does not resolve a configuration;
//! * **tags** (`!!timestamp`, `!MyType`) stay visible, and searchable;
//! * **directives** (`%YAML`, `%TAG`) stay visible above their document;
//! * **scalar style** is kept, so a `|` block still reads as a block and a
//!   quoted string is still distinguishable from a plain one;
//! * **entry order** is source order, and **every document** of a stream is a
//!   root of its own.
//!
//! # The parser
//!
//! [`granit_parser`] is a pure-Rust YAML 1.2 event parser with comments,
//! scalar styles, tags and byte spans — everything above. Events are consumed
//! iteratively into [`super::structured::Builder`], so nesting costs heap
//! rather than call frames.
//!
//! Two passes are needed over the source, for one reason: the event stream
//! identifies an anchor by a numeric id rather than by its name, and a reader
//! has to see `&defaults`. A token pass collects the anchor names in source
//! order — the parser assigns ids in exactly that order — along with the
//! `%TAG` and `%YAML` directives, which are likewise only visible at the token
//! level.

use granit_parser::{
    ErrorKind, Event, Marker, Options, Parser, ScalarStyle as YamlStyle, Scanner, StrInput,
    TokenType,
};

use super::error::DocumentError;
use super::format::DocumentKind;
use super::source::{SourceDocument, SourceSpan};
use super::structured::ast::{
    Directive, NodeMeta, NodeRelation, ScalarKind, ScalarStyle, ScalarValue, StructuredDocument,
    StructuredKey, StructuredNodeKind, MAX_DEPTH,
};
use super::structured::Builder;
use super::NodeId;

/// Parse a YAML stream into the structured model.
pub fn parse(source: &SourceDocument) -> Result<StructuredDocument, DocumentError> {
    let prelude = Prelude::scan(source.text())?;
    Loader::new(source, prelude).run()
}

/// The parser settings a reader needs.
///
/// granit defaults its nesting limits to 255, which is a sensible ceiling for
/// a deserializer but not the one diple promises: [`MAX_DEPTH`] is the single
/// place where "too deep to be worth showing" is decided, and the JSON backend
/// already answers to it. Raising granit's limits to the same number makes the
/// two backends refuse the same documents, and [`Loader::scan_error`] then
/// gives granit's depth refusal the same wording as diple's own.
///
/// Comment emission is granit's default, but it is stated here because
/// everything in this module exists to keep comments.
fn parser_options() -> Options {
    granit_parser::options! {
        emit_comments: true,
        flow_nesting_limit: MAX_DEPTH,
        block_nesting_limit: MAX_DEPTH,
    }
}

// ---------------------------------------------------------------------------
// Pass 1: the token-level facts the event stream does not carry.
// ---------------------------------------------------------------------------

/// Anchor names and directives, in source order.
#[derive(Debug, Default)]
struct Prelude {
    /// The *n*-th anchor definition in the source; the parser numbers anchors
    /// from 1 in the same order, so anchor id `n` is `anchors[n - 1]`.
    anchors: Vec<String>,
    /// Directives with the byte offset they were written at.
    directives: Vec<(usize, Directive)>,
}

impl Prelude {
    fn scan(text: &str) -> Result<Prelude, DocumentError> {
        let mut prelude = Prelude::default();
        for token in Scanner::with_options(StrInput::new(text), parser_options()) {
            // A scan error here is reported by the event pass, with the
            // position and message the reader should see; this pass only
            // gathers what it can.
            let Ok(token) = token else { break };
            let (span, kind) = token.into_parts();
            let at = span.start.byte_offset().unwrap_or(0);
            match kind {
                TokenType::Anchor(name) => prelude.anchors.push(name.into_owned()),
                TokenType::VersionDirective(major, minor) => {
                    prelude
                        .directives
                        .push((at, Directive::Version { major, minor }));
                }
                TokenType::TagDirective(handle, prefix) => {
                    prelude.directives.push((
                        at,
                        Directive::Tag {
                            handle: handle.into_owned(),
                            prefix: prefix.into_owned(),
                        },
                    ));
                }
                TokenType::StreamEnd => break,
                _ => {}
            }
        }
        Ok(prelude)
    }

    /// The name of the anchor the parser gave `id`.
    fn anchor(&self, id: usize) -> Option<&str> {
        id.checked_sub(1)
            .and_then(|i| self.anchors.get(i))
            .map(String::as_str)
    }

    /// Every directive written before `byte`, removed from the pool so that
    /// each is claimed by exactly one document.
    fn take_before(&mut self, byte: usize) -> Vec<Directive> {
        let keep = self.directives.split_off(
            self.directives
                .iter()
                .position(|(at, _)| *at >= byte)
                .unwrap_or(self.directives.len()),
        );
        let taken = std::mem::replace(&mut self.directives, keep);
        taken.into_iter().map(|(_, d)| d).collect()
    }
}

// ---------------------------------------------------------------------------
// Pass 2: events into the structured model.
// ---------------------------------------------------------------------------

/// Where the loader is inside a mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Key,
    Value,
}

/// What kind of row the node being created will render as. Only comment
/// attachment cares, and only because a block collection shares its first
/// line with its first entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Leaf,
    Container,
}

/// One open container.
#[derive(Debug)]
struct Frame {
    node: NodeId,
    mapping: bool,
    next_index: usize,
    expect: Expect,
    pending_key: Option<StructuredKey>,
}

struct Loader<'a> {
    source: &'a SourceDocument,
    text: &'a str,
    prelude: Prelude,
    builder: Builder,
    frames: Vec<Frame>,
    /// Comments written above the node that is about to be created, each with
    /// the byte offset it was written at — deciding which node a comment
    /// describes needs to know whether it stands above a row or inside it.
    pending_above: Vec<(usize, usize)>,
    /// A same-line comment that belongs to the entry currently being read —
    /// `metadata: # note` arrives before the mapping the note describes.
    pending_right: Option<usize>,
    /// The node a trailing same-line comment attaches to.
    last_node: Option<NodeId>,
    /// While a collection is being used as a mapping key, how deep we are
    /// inside it and where it started.
    complex_key: Option<(usize, usize)>,
}

impl<'a> Loader<'a> {
    fn new(source: &'a SourceDocument, prelude: Prelude) -> Self {
        let text = source.text();
        let mut builder = Builder::new(DocumentKind::Yaml);
        builder.reserve(text.len() / 24);
        Self {
            source,
            text,
            prelude,
            builder,
            frames: Vec::new(),
            pending_above: Vec::new(),
            pending_right: None,
            last_node: None,
            complex_key: None,
        }
    }

    fn run(mut self) -> Result<StructuredDocument, DocumentError> {
        for next in Parser::new_from_str_with_options(self.text, parser_options()) {
            let (event, span) = match next {
                Ok(pair) => pair,
                Err(error) => return Err(self.scan_error(&error)),
            };
            let start = span.start.byte_offset().unwrap_or(0);
            let end = span.end.byte_offset().unwrap_or(start);
            let range = SourceSpan::new(start, end);

            // A collection used as a mapping key is consumed as source text:
            // there is no honest way to show `[1, 2]: x` other than as it was
            // written, and a key subtree would be a set of cursor stops that
            // are not entries of anything.
            if let Some((depth, key_start)) = self.complex_key {
                match event {
                    Event::MappingStart(..) | Event::SequenceStart(..) => {
                        self.complex_key = Some((depth + 1, key_start));
                    }
                    Event::MappingEnd | Event::SequenceEnd => {
                        if depth == 1 {
                            self.complex_key = None;
                            self.finish_complex_key(key_start, end);
                        } else {
                            self.complex_key = Some((depth - 1, key_start));
                        }
                    }
                    _ => {}
                }
                continue;
            }

            match event {
                Event::StreamStart => {}
                Event::StreamEnd => break,
                Event::DocumentStart(explicit, _) => {
                    let directives = self.prelude.take_before(end.max(start));
                    self.builder.begin_document(explicit, directives);
                }
                Event::DocumentEnd => {
                    self.flush_free_comments();
                }
                Event::Comment(text, placement) => {
                    self.comment(text.as_ref(), range, placement);
                }
                Event::Scalar(value, style, anchor, tag) => {
                    let scalar = self.scalar_value(&value, style, tag.as_deref(), range);
                    if self.expecting_key() {
                        self.set_scalar_key(scalar, anchor, range);
                    } else {
                        let meta = self.take_meta(anchor, tag.as_deref(), Row::Leaf);
                        let relation = self.take_relation();
                        let id = self.builder.scalar(relation, scalar, range, meta);
                        self.node_created(id);
                        self.value_completed();
                    }
                }
                Event::Alias(anchor_id) => {
                    let name = self
                        .prelude
                        .anchor(anchor_id)
                        .map(str::to_string)
                        .unwrap_or_else(|| alias_name_from_source(self.text, range));
                    if self.expecting_key() {
                        self.set_key(StructuredKey {
                            text: format!("*{name}"),
                            span: range,
                            // Shown as the source wrote it, never quoted.
                            complex: true,
                            style: ScalarStyle::Plain,
                            prefix: 0,
                        });
                    } else {
                        let meta = self.take_meta(0, None, Row::Leaf);
                        let relation = self.take_relation();
                        let id = self.builder.alias(relation, name, range, meta);
                        self.node_created(id);
                        self.value_completed();
                    }
                }
                Event::MappingStart(_, anchor, tag) => {
                    if self.expecting_key() {
                        self.complex_key = Some((1, start));
                        continue;
                    }
                    let meta = self.take_meta(anchor, tag.as_deref(), Row::Container);
                    let relation = self.take_relation();
                    let id = self
                        .builder
                        .open_mapping(relation, range, meta)
                        .map_err(|_| self.depth_error(&span.start))?;
                    self.node_created(id);
                    self.frames.push(Frame {
                        node: id,
                        mapping: true,
                        next_index: 0,
                        expect: Expect::Key,
                        pending_key: None,
                    });
                }
                Event::SequenceStart(_, anchor, tag) => {
                    if self.expecting_key() {
                        self.complex_key = Some((1, start));
                        continue;
                    }
                    let meta = self.take_meta(anchor, tag.as_deref(), Row::Container);
                    let relation = self.take_relation();
                    let id = self
                        .builder
                        .open_sequence(relation, range, meta)
                        .map_err(|_| self.depth_error(&span.start))?;
                    self.node_created(id);
                    self.frames.push(Frame {
                        node: id,
                        mapping: false,
                        next_index: 0,
                        expect: Expect::Value,
                        pending_key: None,
                    });
                }
                Event::MappingEnd | Event::SequenceEnd => {
                    if let Some(frame) = self.frames.pop() {
                        self.builder.set_span_end(frame.node, end);
                        self.builder.close();
                        self.last_node = Some(frame.node);
                    }
                    self.value_completed();
                }
                // `Event` is `#[non_exhaustive]`: an event granit adds later
                // is presentation diple does not yet show, never structure it
                // must not lose, so ignoring it is the right default.
                _ => {}
            }
        }
        self.flush_free_comments();
        Ok(self.builder.finish(self.source.clone()))
    }

    // ---- relations -------------------------------------------------------

    fn expecting_key(&self) -> bool {
        self.frames
            .last()
            .is_some_and(|f| f.mapping && f.expect == Expect::Key)
    }

    /// The relation the value now being read will carry.
    fn take_relation(&mut self) -> NodeRelation {
        match self.frames.last_mut() {
            None => NodeRelation::Root { document: 0 },
            Some(frame) if frame.mapping => {
                let key = frame
                    .pending_key
                    .take()
                    .unwrap_or_else(|| StructuredKey::plain("", SourceSpan::default()));
                NodeRelation::MappingEntry { key }
            }
            Some(frame) => {
                let index = frame.next_index;
                frame.next_index += 1;
                NodeRelation::SequenceItem { index }
            }
        }
    }

    /// A value finished: a mapping goes back to expecting a key.
    fn value_completed(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            if frame.mapping {
                frame.expect = Expect::Key;
            }
        }
    }

    fn set_key(&mut self, key: StructuredKey) {
        if let Some(frame) = self.frames.last_mut() {
            frame.pending_key = Some(key);
            frame.expect = Expect::Value;
        }
    }

    /// A scalar used as a key. An anchor or tag on the key itself is shown as
    /// part of the key text rather than dropped.
    fn set_scalar_key(&mut self, scalar: ScalarValue, anchor: usize, span: SourceSpan) {
        let mut text = scalar.text;
        let mut prefix = 0;
        if let Some(name) = self.prelude.anchor(anchor) {
            let anchor = format!("&{name} ");
            prefix = anchor.len();
            text = anchor + &text;
        }
        self.set_key(StructuredKey {
            text,
            span,
            complex: false,
            style: scalar.style,
            prefix,
        });
    }

    /// A collection used as a key: shown as the source wrote it.
    fn finish_complex_key(&mut self, start: usize, end: usize) {
        let text = self
            .text
            .get(start..end)
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        self.set_key(StructuredKey {
            text,
            span: SourceSpan::new(start, end),
            complex: true,
            style: ScalarStyle::Plain,
            prefix: 0,
        });
    }

    // ---- metadata --------------------------------------------------------

    fn node_created(&mut self, id: NodeId) {
        self.last_node = Some(id);
    }

    /// Collect the metadata the node about to be created carries.
    ///
    /// Must be called before [`Loader::take_relation`], which consumes the
    /// pending key that says whether this entry is a merge key.
    fn take_meta(
        &mut self,
        anchor: usize,
        tag: Option<&granit_parser::Tag>,
        row: Row,
    ) -> Option<NodeMeta> {
        let merge_key = self
            .frames
            .last()
            .and_then(|f| f.pending_key.as_ref())
            .is_some_and(|k| k.text == "<<");
        let claimed = self.comments_claimed_above(row);
        let above: Vec<_> = self
            .pending_above
            .drain(..claimed)
            .map(|(id, _)| id)
            .collect();
        let meta = NodeMeta {
            anchor: self.prelude.anchor(anchor).map(str::to_string),
            tag: tag.map(tag_text),
            above,
            right: self.pending_right.take(),
            merge_key,
        };
        (!meta.is_empty()).then_some(meta)
    }

    /// How many of the pending own-line comments the node about to be created
    /// describes. They are in source order, so it is always a prefix; the rest
    /// wait for the row that follows.
    ///
    /// A block collection begins where its first entry begins, so the naive
    /// answer — "the next node built" — hands
    ///
    /// ```yaml
    /// # Production replicas.
    /// replicas: 3
    /// ```
    ///
    /// to the enclosing mapping instead of to `replicas`, which puts the
    /// comment on a row the reader is not looking at and collapses it with the
    /// wrong subtree. A comment belongs to the row it stands above:
    ///
    /// * a leaf always takes it — the leaf *is* a row;
    /// * a document root never takes it. A root is the document, not a row
    ///   within it, so the comment passes down to the first entry;
    /// * a keyed container takes those written above the key that names it,
    ///   and only those. A comment written *after* that key, as in `foo:` /
    ///   `# why` / `bar: 1`, stands above `bar` and waits for it — and it does
    ///   so even when the same key also had comments above it, which is the
    ///   case an all-or-nothing answer got wrong: `# about foo` and `# why`
    ///   were pending together, the first of them stood above `foo`, and both
    ///   were handed to `foo`.
    ///
    /// A container that is a sequence item has no key, but its `- ` does start
    /// a row of its own, so it takes the comments.
    fn comments_claimed_above(&self, row: Row) -> usize {
        if row == Row::Leaf {
            return self.pending_above.len();
        }
        let Some(frame) = self.frames.last() else {
            return 0;
        };
        match &frame.pending_key {
            Some(key) => self
                .pending_above
                .partition_point(|(_, at)| *at < key.span.start),
            None => self.pending_above.len(),
        }
    }

    fn comment(&mut self, text: &str, span: SourceSpan, placement: granit_parser::Placement) {
        use granit_parser::Placement;
        let id = self.builder.comment(text, span);
        match placement {
            // A same-line comment belongs to the entry whose key has just been
            // read when there is one — `metadata: # note` — and otherwise to
            // the node that was just built.
            Placement::Right => {
                if self
                    .frames
                    .last()
                    .is_some_and(|f| f.mapping && f.expect == Expect::Value)
                {
                    self.pending_right = Some(id);
                } else if let Some(node) = self.last_node {
                    self.builder.attach_meta(node, |m| m.right = Some(id));
                } else {
                    self.pending_above.push((id, span.start));
                }
            }
            Placement::Last => self.builder.trailing_comment(id),
            // An own-line comment describes what follows it, so it waits for
            // the next node and then collapses and moves with it. That is
            // also the safe default for a placement granit adds later.
            Placement::Above | Placement::Free | _ => self.pending_above.push((id, span.start)),
        }
    }

    /// Comments that never found a node to describe stay in the document as
    /// trailing text rather than disappearing.
    fn flush_free_comments(&mut self) {
        for (id, _) in std::mem::take(&mut self.pending_above) {
            self.builder.trailing_comment(id);
        }
        if let Some(id) = self.pending_right.take() {
            self.builder.trailing_comment(id);
        }
    }

    // ---- scalars ---------------------------------------------------------

    fn scalar_value(
        &self,
        value: &str,
        style: YamlStyle,
        tag: Option<&granit_parser::Tag>,
        span: SourceSpan,
    ) -> ScalarValue {
        let style = match style {
            YamlStyle::Plain => ScalarStyle::Plain,
            YamlStyle::SingleQuoted => ScalarStyle::SingleQuoted,
            YamlStyle::DoubleQuoted => ScalarStyle::DoubleQuoted,
            YamlStyle::Literal => ScalarStyle::Literal,
            YamlStyle::Folded => ScalarStyle::Folded,
        };
        // An empty span means the value was not written at all — `key:` with
        // nothing after it. The parser reports that as `~`; the reader should
        // see what the author wrote, which is nothing.
        if style == ScalarStyle::Plain && span.is_empty() {
            return ScalarValue {
                text: "null".to_string(),
                kind: ScalarKind::Null,
                style,
                source: Some(String::new()),
                block_header: None,
            };
        }
        let kind = tag
            .and_then(core_schema_kind)
            .unwrap_or_else(|| resolve_kind(value, style));
        ScalarValue {
            text: value.to_string(),
            kind,
            style,
            source: None,
            block_header: style
                .is_block()
                .then(|| block_header(self.text, span.start))
                .flatten(),
        }
    }

    // ---- errors ----------------------------------------------------------

    /// A parser failure, in diple's words.
    ///
    /// granit's own depth refusal is reworded: "recursion limit exceeded" is
    /// about granit's internals, and a reader hitting it has hit exactly the
    /// same wall the JSON backend describes as nesting past [`MAX_DEPTH`].
    fn scan_error(&self, error: &granit_parser::ScanError) -> DocumentError {
        let marker = error.marker();
        if matches!(error.kind(), ErrorKind::RecursionLimitExceeded) {
            return self.depth_error(marker);
        }
        let report = DocumentError::at(
            self.source,
            DocumentKind::Yaml,
            Self::byte_of(marker),
            error.info(),
        );
        // A quote or a flow collection still open when the stream ends.
        //
        // A multi-line quoted scalar cut at a line break (`head -n`) is
        // reported as bad indentation where the input ends rather than as an
        // unclosed quote; at the very end it is the same thing.
        let at_the_end = Self::byte_of(marker) >= self.source.text().trim_end().len();
        match error.kind() {
            ErrorKind::UnclosedQuotedScalar | ErrorKind::UnclosedFlowCollection { .. } => {
                report.ran_out()
            }
            // Anything else that breaks exactly where the input ends — an
            // escape, an alias or an item left without its rest — ran out
            // as well; detection still asks what came before.
            _ if at_the_end => report.ran_out(),
            _ => report,
        }
    }

    fn depth_error(&self, marker: &Marker) -> DocumentError {
        DocumentError::at(
            self.source,
            DocumentKind::Yaml,
            Self::byte_of(marker),
            format!("nested more than {MAX_DEPTH} levels deep; diple refuses to go further"),
        )
    }

    /// Where a parser marker is, as a byte offset.
    ///
    /// The line and column are recomputed from that offset rather than taken
    /// from the marker, because YAML counts a lone `\r` as a line break and
    /// [`SourceDocument`] — which is format-neutral, and is what prints the
    /// excerpt — does not. Taking the parser's line number would point at a
    /// line the excerpt cannot show, and the caret would vanish.
    fn byte_of(marker: &Marker) -> usize {
        marker.byte_offset().unwrap_or(marker.index())
    }
}

/// A tag as the source wrote it: the handle it was written with plus its
/// suffix, so `!!timestamp` and `!MyType` read back the way they were typed.
/// A verbatim tag has no handle and keeps its `!<…>`, without which
/// `!<tag:x> v` would read as the plain text `tag:x v`.
///
/// The parser decodes `%XX` escapes in the suffix; a decoded character that
/// cannot be shown as itself (a control, a bidi override, a space) is written
/// back as the escape the source used, so `!t%1b%07tag` reads as typed.
fn tag_text(tag: &granit_parser::Tag) -> String {
    let suffix = reencode_tag(tag.suffix());
    if tag.original_handle().is_empty() {
        return format!("!<{}{suffix}>", tag.handle());
    }
    format!("{}{suffix}", tag.original_handle())
}

fn reencode_tag(suffix: &str) -> String {
    let mut out = String::with_capacity(suffix.len());
    for c in suffix.chars() {
        if crate::util::text::replacement(c).is_some() || c.is_whitespace() {
            let mut buf = [0u8; 4];
            for byte in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The type a YAML core-schema tag forces, if it is one.
fn core_schema_kind(tag: &granit_parser::Tag) -> Option<ScalarKind> {
    match tag.core_suffix()? {
        "str" => Some(ScalarKind::String),
        "int" | "float" => Some(ScalarKind::Number),
        "bool" => Some(ScalarKind::Boolean),
        "null" => Some(ScalarKind::Null),
        _ => None,
    }
}

/// YAML 1.2 core-schema resolution for a plain scalar. Anything quoted or
/// written as a block is a string whatever it looks like.
pub(crate) fn resolve_kind(value: &str, style: ScalarStyle) -> ScalarKind {
    if style != ScalarStyle::Plain {
        return ScalarKind::String;
    }
    match value {
        "" | "~" | "null" | "Null" | "NULL" => return ScalarKind::Null,
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" => return ScalarKind::Boolean,
        _ => {}
    }
    if is_yaml_number(value) {
        ScalarKind::Number
    } else {
        ScalarKind::String
    }
}

fn is_yaml_number(value: &str) -> bool {
    let body = value.strip_prefix(['-', '+']).unwrap_or(value);
    if body.is_empty() {
        return false;
    }
    if matches!(body, ".inf" | ".Inf" | ".INF") {
        return true;
    }
    if matches!(value, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    if let Some(hex) = body.strip_prefix("0x") {
        return !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    if let Some(oct) = body.strip_prefix("0o") {
        return !oct.is_empty() && oct.bytes().all(|b| (b'0'..=b'7').contains(&b));
    }
    // [0-9]* ( '.' [0-9]* )? ( [eE] [-+]? [0-9]+ )?
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (body, None),
    };
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (mantissa, None),
    };
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    // `Option::is_none_or` would read better but arrived in Rust 1.82;
    // diple's MSRV is 1.81, set by `granit-parser` alone.
    if !digits(int) || matches!(frac, Some(f) if !digits(f)) {
        return false;
    }
    if int.is_empty() && !matches!(frac, Some(f) if !f.is_empty()) {
        return false;
    }
    match exponent {
        None => true,
        Some(e) => {
            let e = e.strip_prefix(['-', '+']).unwrap_or(e);
            !e.is_empty() && digits(e)
        }
    }
}

/// The block header a block scalar was written with — `|`, `|+`, `>-`, `|2-` —
/// read out of the source that starts at `content`.
///
/// Neither the event stream nor the token stream carries it: granit resolves
/// the indent and chomping indicators while scanning and reports only
/// [`YamlStyle::Literal`] or [`YamlStyle::Folded`] together with the already
/// chomped content, whose span begins at the content rather than at the
/// header. The source is therefore the only place left to read it from, and
/// the reader must see it: `|+` and `|-` are different documents.
///
/// The header is the last thing on the line that introduces the scalar, save
/// for a comment; between that line and the content there can only be empty
/// lines, because inside a block scalar every other line is content.
fn block_header(text: &str, content: usize) -> Option<String> {
    let mut line_start = text.get(..content)?.rfind('\n').map_or(0, |nl| nl + 1);
    while line_start > 0 {
        let end = line_start - 1;
        line_start = text.get(..end)?.rfind('\n').map_or(0, |nl| nl + 1);
        let line = text.get(line_start..end)?.trim_end_matches('\r');
        if !line.trim().is_empty() {
            return header_of_line(line);
        }
    }
    None
}

/// The block header written on `line`, if it ends with one.
///
/// The indicator is taken to be the first `|` or `>` that stands at the start
/// of a token and is followed by nothing but an indent indicator, a chomping
/// indicator and possibly a comment — which is what tells the header of
/// `keep: |+  # kept` apart from a `|` inside a key or a comment.
fn header_of_line(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    for (i, byte) in bytes.iter().enumerate() {
        if !matches!(byte, b'|' | b'>') {
            continue;
        }
        if i > 0 && !bytes[i - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = &bytes[i + 1..];
        let indicators = rest
            .iter()
            .take_while(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-'))
            .count();
        let (indicators, tail) = (&rest[..indicators], &rest[indicators..]);
        let digits = indicators.iter().filter(|b| b.is_ascii_digit()).count();
        let signs = indicators.len() - digits;
        let rest_of_line = tail.trim_ascii_start();
        let ends_line = (tail.is_empty() || tail[0].is_ascii_whitespace())
            && (rest_of_line.is_empty() || rest_of_line[0] == b'#');
        if digits <= 1 && signs <= 1 && ends_line {
            return Some(line[i..i + 1 + indicators.len()].to_string());
        }
    }
    None
}

/// The alias name read straight out of the source, for the case where the
/// anchor table could not supply it.
fn alias_name_from_source(text: &str, span: SourceSpan) -> String {
    span.slice(text)
        .map(|s| s.trim_start_matches('*').to_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Content detection
// ---------------------------------------------------------------------------

/// Whether a parsed YAML document is confidently YAML rather than prose that
/// happens to parse.
///
/// Almost any text is a valid YAML scalar, and a Markdown list is a valid YAML
/// sequence, so parsing successfully proves nothing. Detection therefore asks
/// for two things: the stream must be **coherent** — every document's root is
/// a mapping or a sequence, never a bare scalar, which is what rules out prose
/// and a Markdown file with YAML front matter — and it must show at least one
/// **structural signal** that prose does not produce.
///
/// The cost of a false positive is that a reader's Markdown is shown as YAML;
/// the cost of a false negative is typing `--format yaml`. The policy is tuned
/// accordingly.
pub fn is_confidently_yaml(doc: &StructuredDocument) -> bool {
    // An empty document — a trailing `---`, or `---` twice — says nothing
    // either way, and a stream of nothing else is not YAML anyone piped in.
    let documents: Vec<NodeId> = doc
        .roots()
        .iter()
        .map(|root| root.node)
        .filter(|&root| {
            // An empty document reads as a null whose source is empty — but
            // only one with nothing at all in its part of the stream is empty:
            // front matter over a lone `# Heading` is two documents, the
            // second a comment, and that second one is Markdown.
            doc.node(root).is_some_and(|n| {
                let null = matches!(&n.kind, StructuredNodeKind::Scalar(v)
                    if v.source.as_deref().is_some_and(|s| s.trim().is_empty()));
                !(null && document_text_is_blank(doc, root))
            })
        })
        .collect();
    if documents.is_empty() {
        return false;
    }
    let coherent = documents.iter().all(|&root| {
        doc.node(root)
            .is_some_and(|n| n.is_container() && n.child_count > 0)
    });
    if !coherent {
        return false;
    }
    // `%YAML` or `%TAG`.
    if doc.roots().iter().any(|r| !r.directives.is_empty()) {
        return true;
    }
    // Deliberately *not* signals, though YAML has them: several documents
    // (`---` is also a Markdown rule, and front matter is a document) and a
    // flat top-level mapping (`Status: done` over `Owner: alice` is a note).
    // Both were tried, and both claimed ordinary Markdown; a flat YAML file
    // piped in needs `--format yaml`. For the same reason every document of a
    // stream has to show a signal of its own: front matter with a nested
    // `params:` over a Markdown list is two documents, one of them a list.
    let aliased: std::collections::HashSet<&str> = doc
        .nodes()
        .iter()
        .filter_map(|n| match &n.kind {
            StructuredNodeKind::Alias { name } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    documents.iter().all(|&root| {
        let end = doc.node(root).map_or(root, |n| n.end);
        document_has_signal(doc, &doc.nodes()[root..end], &aliased)
    })
}

/// Whether the part of the stream an (empty) document occupies — from its
/// root to the next document's root — holds nothing but blank lines and
/// document markers.
fn document_text_is_blank(doc: &StructuredDocument, root: NodeId) -> bool {
    let text = doc.source().text();
    let start = doc.node(root).map_or(0, |n| n.span.start).min(text.len());
    let end = doc
        .roots()
        .iter()
        .map(|r| r.node)
        .find(|&r| r > root)
        .and_then(|r| doc.node(r))
        .map_or(text.len(), |n| n.span.start)
        .clamp(start, text.len());
    // The null's own span may begin just before its marker line; look at
    // whole lines from there.
    let from = text[..start].rfind('\n').map_or(0, |at| at + 1);
    text[from..end].lines().all(|line| {
        let line = line.trim();
        line.is_empty() || line == "---" || line == "..."
    })
}

/// Whether one document's nodes show structure prose does not write.
fn document_has_signal(
    doc: &StructuredDocument,
    nodes: &[super::structured::StructuredNode],
    aliased: &std::collections::HashSet<&str>,
) -> bool {
    nodes.iter().any(|node| {
        // An anchor some alias refers to. An anchor alone is `- &copy; 2024`,
        // an HTML entity at the start of a list item.
        if node.anchor().is_some_and(|a| aliased.contains(a)) {
            return true;
        }
        // A tag Markdown does not write: a `!!` core tag, a verbatim `!<…>`
        // tag, or any tag on a collection. `- !important: read this` is a
        // local tag on a scalar.
        if let Some(tag) = node.tag() {
            if tag.starts_with("!!") || tag.starts_with("!<") || node.is_container() {
                return true;
            }
        }
        // A mapping inside a container, in a shape a Markdown list cannot
        // take. `- Fast: written in Rust` is a one-entry mapping inside a
        // sequence, and `Pros:` over a list is a sequence inside a mapping —
        // both are ordinary Markdown. What prose does not write is a mapping
        // under a mapping key (`metadata:` over `name: web`), or a mapping of
        // two entries or more inside anything (`- name: x` over `image: y`) —
        // provided no key involved has a space in it. Prose makes keys of
        // phrases: a wrapped list item (`- Fix: …` over `  release notes: …`)
        // or an introduction over an indented example (`Example config:`).
        if matches!(node.kind, StructuredNodeKind::Mapping) && node.depth > 0 {
            let under_a_key = node
                .parent
                .and_then(|p| doc.node(p))
                .is_some_and(|p| matches!(p.kind, StructuredNodeKind::Mapping));
            return (under_a_key || node.child_count >= 2)
                && keys_are_identifiers(doc, node.id)
                && not_all_phrases(doc, node.id);
        }
        false
    })
}

/// Whether a mapping holds something other than phrases: a collection, or a
/// scalar that is a token (`nginx`, `actions/checkout@v4`, `3`). A wrapped
/// list item makes a mapping whose every value is a clause of the sentence
/// (`- Stack-Footprint: Keycloak, NATS, …` over `  Gegenmassnahme: …`).
fn not_all_phrases(doc: &StructuredDocument, mapping: NodeId) -> bool {
    doc.children(mapping).into_iter().any(|child| {
        doc.node(child).is_some_and(|c| match c.scalar() {
            None => true,
            Some(v) => {
                let text = v.text.trim();
                !(v.style == ScalarStyle::Plain && v.kind == ScalarKind::String && is_phrase(text))
            }
        })
    })
}

/// Whether a plain value reads as words rather than as a token: it has a
/// space in it, it ends like a sentence in any common script, or it is
/// written in a script that does not put spaces between words, where the
/// absence of one says nothing.
fn is_phrase(text: &str) -> bool {
    const SENTENCE_ENDS: [char; 14] = [
        '.', '!', '?', '\u{2026}', // Latin
        '\u{3002}', '\u{ff01}', '\u{ff1f}', '\u{ff0e}', // CJK 。！？．
        '\u{589}',  // Armenian ։
        '\u{61f}', '\u{6d4}', // Arabic ؟ ۔
        '\u{964}', '\u{965}',  // Devanagari । ॥
        '\u{1362}', // Ethiopic ።
    ];
    text.contains(char::is_whitespace)
        || text.ends_with(SENTENCE_ENDS)
        || text.chars().any(|c| {
            matches!(c,
                '\u{3040}'..='\u{30ff}'   // Hiragana, Katakana
                | '\u{3400}'..='\u{4dbf}' // CJK extension A
                | '\u{4e00}'..='\u{9fff}' // CJK unified ideographs
                | '\u{0e00}'..='\u{0e7f}') // Thai
        })
}

/// Whether a mapping's own keys, and every key on the way down to it, are
/// free of spaces — the keys configuration uses, not the ones prose makes.
/// Quoted or not: spec §6.5 says no key.
fn keys_are_identifiers(doc: &StructuredDocument, mapping: NodeId) -> bool {
    let plain_phrase = |id: NodeId| {
        doc.node(id)
            .and_then(|n| n.key())
            .is_some_and(|k| !k.complex && k.name().contains(char::is_whitespace))
    };
    if doc.children(mapping).into_iter().any(plain_phrase) {
        return false;
    }
    let mut cur = Some(mapping);
    let mut guard = 0usize;
    while let Some(id) = cur {
        if plain_phrase(id) {
            return false;
        }
        cur = doc.node(id).and_then(|n| n.parent);
        guard += 1;
        if guard > doc.node_count() {
            break;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::search::MatchField;

    fn doc(src: &str) -> StructuredDocument {
        parse(&SourceDocument::new("t.yaml", src)).expect(src)
    }

    fn err(src: &str) -> DocumentError {
        parse(&SourceDocument::new("t.yaml", src)).expect_err(src)
    }

    fn shape(d: &StructuredDocument) -> Vec<String> {
        d.nodes()
            .iter()
            .map(|n| {
                let what = match &n.kind {
                    StructuredNodeKind::Mapping => "{}".to_string(),
                    StructuredNodeKind::Sequence => "[]".to_string(),
                    StructuredNodeKind::Scalar(v) => format!("{:?} {:?}", v.kind, v.display()),
                    StructuredNodeKind::Alias { name } => format!("*{name}"),
                };
                format!("{}{} {what}", "  ".repeat(n.depth), d.label(n.id))
            })
            .collect()
    }

    /// The rows a reader sees, laid out at a width no fixture here reaches.
    ///
    /// The model is only half the claim: a style or a comment that survives
    /// parsing but never reaches a row is still lost to the reader.
    fn rows(src: &str) -> Vec<String> {
        rows_with(src, |_, _| {})
    }

    /// The rows a reader sees after `fold` has had its way with the fold
    /// state — the way a collapsed subtree is checked.
    fn rows_with(
        src: &str,
        fold: impl FnOnce(&crate::document::DocumentModel, &mut crate::document::FoldState),
    ) -> Vec<String> {
        use crate::layout::{Layout, LayoutOptions};
        let model = crate::document::DocumentModel::structured(doc(src));
        let theme = crate::render::theme::Theme::dark();
        let mut folds = model.fold_state();
        fold(&model, &mut folds);
        let opts = LayoutOptions::new(100, &theme).with_folds(&folds);
        Layout::build(&model, &opts)
            .to_plain_text()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The fold over the container reached by the mapping key `key`.
    fn fold_of(model: &crate::document::DocumentModel, key: &str) -> crate::document::FoldId {
        let d = model.as_structured().expect("a structured document");
        (0..model.fold_parents().len())
            .find(|fold| {
                model
                    .fold_node(*fold)
                    .and_then(|node| d.node(node))
                    .and_then(|node| node.key())
                    .is_some_and(|k| k.text == key)
            })
            .unwrap_or_else(|| panic!("no foldable container keyed {key:?}"))
    }

    /// The node that owns the comment whose text contains `needle`.
    fn comment_owner(d: &StructuredDocument, needle: &str) -> String {
        let id = d
            .comments()
            .iter()
            .position(|c| c.text.contains(needle))
            .unwrap_or_else(|| panic!("no comment saying {needle:?}"));
        let node = d
            .nodes()
            .iter()
            .find(|n| n.comments_above().contains(&id) || n.comment_right() == Some(id))
            .unwrap_or_else(|| panic!("comment {needle:?} belongs to no node"));
        d.label(node.id)
    }

    #[test]
    fn mappings_and_sequences_build_the_expected_tree() {
        let d = doc(
            "metadata:\n  name: nginx\n  labels:\n    app: frontend\nports:\n  - 80\n  - 443\n",
        );
        assert_eq!(
            shape(&d),
            [
                "root {}",
                "  metadata {}",
                "    name String \"nginx\"",
                "    labels {}",
                "      app String \"frontend\"",
                "  ports []",
                "    [0] Number \"80\"",
                "    [1] Number \"443\"",
            ]
        );
        assert_eq!(d.path(7).breadcrumb(false), "ports > [1]");
        assert_eq!(d.collapsed_summary(1).as_deref(), Some("{2 entries}"));
        assert_eq!(d.collapsed_summary(5).as_deref(), Some("[2 items]"));
    }

    #[test]
    fn a_sequence_of_mappings_nests_the_way_it_reads() {
        let d = doc("containers:\n  - name: nginx\n    image: nginx:1.27\n  - name: sidecar\n");
        assert_eq!(
            shape(&d),
            [
                "root {}",
                "  containers []",
                "    [0] {}",
                "      name String \"nginx\"",
                "      image String \"nginx:1.27\"",
                "    [1] {}",
                "      name String \"sidecar\"",
            ]
        );
        assert_eq!(d.path(4).breadcrumb(false), "containers > [0] > image");
    }

    #[test]
    fn entry_order_is_source_order() {
        let d = doc("z: 1\na: 2\nm: 3\n");
        let keys: Vec<String> = d.children(0).into_iter().map(|id| d.label(id)).collect();
        assert_eq!(keys, ["z", "a", "m"]);
    }

    #[test]
    fn scalar_types_follow_the_core_schema_and_the_style() {
        let d = doc(
            "a: 3\nb: 3.5\nc: -2\nd: 1e6\ne: true\nf: False\ng: null\nh: ~\ni: text\nj: \"3\"\nk: '3'\nl: 0x1f\nm: .inf\nn: 2026-08-31\n",
        );
        let kinds: Vec<(std::string::String, ScalarKind)> = d
            .children(0)
            .into_iter()
            .filter_map(|id| Some((d.label(id), d.node(id)?.scalar()?.kind)))
            .collect();
        use ScalarKind::*;
        assert_eq!(
            kinds,
            [
                ("a".into(), Number),
                ("b".into(), Number),
                ("c".into(), Number),
                ("d".into(), Number),
                ("e".into(), Boolean),
                ("f".into(), Boolean),
                ("g".into(), Null),
                ("h".into(), Null),
                ("i".into(), String),
                ("j".into(), String),
                ("k".into(), String),
                ("l".into(), Number),
                ("m".into(), Number),
                // A timestamp is a string under the core schema; the YAML 1.1
                // types are not resolved implicitly.
                ("n".into(), String),
            ]
        );
    }

    #[test]
    fn an_empty_value_shows_as_the_author_wrote_it() {
        let d = doc("empty:\nexplicit: null\ntilde: ~\n");
        let shown: Vec<&str> = d
            .children(0)
            .into_iter()
            .filter_map(|id| d.node(id)?.scalar().map(|v| v.display()))
            .collect();
        assert_eq!(shown, ["", "null", "~"]);
        let kinds: Vec<ScalarKind> = d
            .children(0)
            .into_iter()
            .filter_map(|id| d.node(id)?.scalar().map(|v| v.kind))
            .collect();
        assert_eq!(kinds, [ScalarKind::Null; 3]);
    }

    #[test]
    fn block_scalars_keep_their_style_and_their_line_breaks() {
        let d = doc("script: |\n  echo hello\n  echo world\nnote: >\n  one\n  two\n");
        let script = d.node(1).unwrap().scalar().unwrap();
        assert_eq!(script.style, ScalarStyle::Literal);
        assert_eq!(script.text, "echo hello\necho world\n");
        assert!(script.style.is_block());
        let note = d.node(2).unwrap().scalar().unwrap();
        assert_eq!(note.style, ScalarStyle::Folded);
        assert_eq!(note.text, "one two\n");
    }

    #[test]
    fn a_block_scalar_keeps_the_header_the_author_wrote() {
        // `|+` keeps the trailing newlines, `|-` strips them and `|` clips
        // them to one: three different documents, which a bare `|` on every
        // row would present as the same one.
        let src =
            "keep: |+\n  a\n\nstrip: |-\n  b\nclip: |\n  c\nfold: >-\n  d\nindent: |2\n   e\n";
        let d = doc(src);
        let headers: Vec<&str> = d
            .nodes()
            .iter()
            .filter_map(|n| n.scalar()?.block_header.as_deref())
            .collect();
        assert_eq!(headers, ["|+", "|-", "|", ">-", "|2"]);
        let shown: Vec<String> = rows(src).into_iter().filter(|r| r.contains(':')).collect();
        assert_eq!(
            shown,
            [
                "  keep: |+",
                "  strip: |-",
                "  clip: |",
                "  fold: >-",
                "  indent: |2"
            ]
        );
    }

    #[test]
    fn a_block_header_is_found_past_a_comment_and_past_empty_lines() {
        // The header is the last thing on its line but a comment may follow
        // it, and the content may start several lines below it.
        let src = "a: |+ # kept on purpose\n\n\n  body\nb: >-\n  folded\n";
        let d = doc(src);
        let headers: Vec<&str> = d
            .nodes()
            .iter()
            .filter_map(|n| n.scalar()?.block_header.as_deref())
            .collect();
        assert_eq!(headers, ["|+", ">-"]);
        assert_eq!(rows(src)[0], "  a: |+  # kept on purpose");
    }

    #[test]
    fn a_block_header_is_not_confused_with_a_bar_written_elsewhere() {
        let d = doc("a > b: |\n  x\n'c|d': >\n  y\ne: |\n  f | g\n");
        let headers: Vec<&str> = d
            .nodes()
            .iter()
            .filter_map(|n| n.scalar()?.block_header.as_deref())
            .collect();
        assert_eq!(headers, ["|", ">", "|"]);
    }

    #[test]
    fn quoting_styles_survive() {
        let d = doc("a: plain\nb: 'single'\nc: \"double\\ttab\"\n");
        let styles: Vec<ScalarStyle> = d
            .children(0)
            .into_iter()
            .filter_map(|id| d.node(id)?.scalar().map(|v| v.style))
            .collect();
        assert_eq!(
            styles,
            [
                ScalarStyle::Plain,
                ScalarStyle::SingleQuoted,
                ScalarStyle::DoubleQuoted
            ]
        );
        assert_eq!(d.node(3).unwrap().scalar().unwrap().text, "double\ttab");
    }

    #[test]
    fn comments_survive_attached_to_what_they_describe() {
        let d = doc(
            "# Production replicas.\n# Keep in sync with the capacity plan.\nreplicas: 3  # minimum for HA\nother: 1\n",
        );
        let replicas = d.node(1).unwrap();
        assert_eq!(replicas.comments_above().len(), 2);
        assert!(replicas.comment_right().is_some());
        assert_eq!(
            d.comment(replicas.comments_above()[0]).unwrap().text,
            " Production replicas."
        );
        assert_eq!(
            d.comment(replicas.comment_right().unwrap()).unwrap().text,
            " minimum for HA"
        );
        assert!(d.node(2).unwrap().comments_above().is_empty());
        // And they are searchable.
        let hits = d.search_index().find("capacity", false);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].field, MatchField::Comment);
    }

    #[test]
    fn a_comment_after_a_key_belongs_to_that_entry() {
        let d = doc("metadata: # the pod's identity\n  name: nginx\n");
        let metadata = d.node(1).unwrap();
        assert_eq!(d.label(1), "metadata");
        assert_eq!(
            d.comment(metadata.comment_right().unwrap()).unwrap().text,
            " the pod's identity"
        );
    }

    #[test]
    fn a_comment_above_a_keyed_container_belongs_to_the_container() {
        let d = doc("# The ports the pod listens on.\nports:\n  - 80\n");
        let ports = d.node(1).unwrap();
        assert_eq!(d.label(1), "ports");
        assert_eq!(
            d.comment(ports.comments_above()[0]).unwrap().text,
            " The ports the pod listens on."
        );
    }

    #[test]
    fn a_comment_below_a_key_belongs_to_the_entry_it_stands_above() {
        // The mapping `metadata` opens where `name` opens, so the naive
        // answer would hang this comment off `metadata` — a row above the
        // comment's own line.
        let d = doc("metadata:\n  # Chosen by the release tooling.\n  name: nginx\n");
        assert!(d.node(1).unwrap().comments_above().is_empty());
        let name = d.node(2).unwrap();
        assert_eq!(d.label(2), "name");
        assert_eq!(
            d.comment(name.comments_above()[0]).unwrap().text,
            " Chosen by the release tooling."
        );
    }

    #[test]
    fn a_comment_above_a_key_and_one_below_it_go_to_different_rows() {
        // Both comments are pending when the `folds` mapping opens, because a
        // block mapping opens where its first entry opens. Only the one
        // written above the key `folds` describes `folds`.
        let src = "# Folding defaults.\nfolds:\n  # Start collapsed.\n  collapsed: false\n";
        let d = doc(src);
        assert_eq!(comment_owner(&d, "Folding defaults"), "folds");
        assert_eq!(comment_owner(&d, "Start collapsed"), "collapsed");
        assert_eq!(
            rows(src),
            [
                "  # Folding defaults.",
                "\u{25bc} folds:",
                "    # Start collapsed.",
                "    collapsed: false",
            ]
        );
    }

    #[test]
    fn the_first_comment_inside_a_sequence_stays_inside_it() {
        let src = "# The ports.\nports:\n  # The one the proxy uses.\n  - 80\n  - 443\n";
        let d = doc(src);
        assert_eq!(comment_owner(&d, "The ports"), "ports");
        assert_eq!(comment_owner(&d, "the proxy"), "[0]");
        assert_eq!(
            rows(src),
            [
                "  # The ports.",
                "\u{25bc} ports:",
                "    # The one the proxy uses.",
                "    [0] 80",
                "    [1] 443",
            ]
        );
    }

    #[test]
    fn collapsing_a_mapping_hides_the_comment_written_above_its_first_entry() {
        // §10.3: a comment collapses with the subtree it belongs to. Hoisting
        // it to the parent would leave it on screen describing a row that is
        // no longer there.
        let src =
            "# Folding defaults.\nfolds:\n  # Start collapsed.\n  collapsed: false\nother: 1\n";
        let shown = rows_with(src, |model, folds| folds.collapse(fold_of(model, "folds")));
        assert!(
            shown.iter().any(|r| r.contains("Folding defaults")),
            "the comment describing the collapsed row is still shown: {shown:?}"
        );
        assert!(
            !shown.iter().any(|r| r.contains("Start collapsed")),
            "a comment inside the collapsed subtree is still on screen: {shown:?}"
        );
    }

    #[test]
    fn a_comment_collapses_with_the_subtree_it_belongs_to() {
        let d = doc("spec:\n  # How many of these to run.\n  replicas: 3\nother: 1\n");
        let spec = d.node(1).unwrap();
        let owner = d
            .nodes()
            .iter()
            .find(|n| !n.comments_above().is_empty())
            .expect("a node owning the comment");
        // Inside `spec`'s subtree, so collapsing `spec` hides the comment with
        // the entry it describes rather than leaving it stranded.
        assert!((spec.id..spec.end).contains(&owner.id));
        assert!(d.trailing_comments().is_empty());
    }

    #[test]
    fn a_comment_with_nothing_after_it_is_still_kept() {
        let d = doc("a: 1\n# trailing thought\n");
        assert_eq!(d.trailing_comments().len(), 1);
        assert_eq!(d.search_index().find("trailing thought", false).len(), 1);
    }

    #[test]
    fn anchors_are_visible_and_aliases_are_references_not_copies() {
        let d = doc("defaults: &defaults\n  retries: 3\n  timeout: 5\nservice:\n  <<: *defaults\n  name: web\n");
        assert_eq!(d.node(1).unwrap().anchor(), Some("defaults"));
        let alias = d
            .nodes()
            .iter()
            .find(|n| matches!(n.kind, StructuredNodeKind::Alias { .. }))
            .expect("an alias node");
        assert_eq!(
            alias.kind,
            StructuredNodeKind::Alias {
                name: "defaults".to_string()
            }
        );
        assert_eq!(alias.child_count, 0, "the anchored subtree is not copied");
        assert!(alias.is_merge_key(), "`<<` is marked, never performed");
        assert_eq!(d.label(alias.id), "<<");
        // `retries` exists exactly once, under the anchor.
        assert_eq!(d.search_index().find("retries", false).len(), 1);
        // The alias is searchable by name.
        let anchors = d.search_index().find("defaults", false);
        assert!(anchors.iter().any(|m| m.field == MatchField::Anchor));
    }

    #[test]
    fn a_merge_key_is_shown_rather_than_performed() {
        let d = doc("defaults: &defaults\n  retries: 3\n  timeout: 5\nservice:\n  <<: *defaults\n  name: web\n");
        let service = d
            .nodes()
            .iter()
            .find(|n| d.label(n.id) == "service")
            .expect("the service mapping");
        // Exactly what the author wrote: the merge entry and `name`. Nothing
        // from `defaults` has been materialised into it.
        let keys: Vec<String> = d
            .children(service.id)
            .into_iter()
            .map(|id| d.label(id))
            .collect();
        assert_eq!(keys, ["<<", "name"]);
        assert_eq!(service.end - service.id, 3, "no merged subtree");
        assert!(d
            .children(service.id)
            .into_iter()
            .all(|id| d.node(id).is_some_and(|n| n.child_count == 0)));
    }

    #[test]
    fn an_alias_to_a_container_stays_a_single_node() {
        let d = doc("anchor: &big\n  a: 1\n  b: 2\ncopy: *big\n");
        let copy = d
            .nodes()
            .iter()
            .find(|n| d.label(n.id) == "copy")
            .expect("the alias entry");
        assert_eq!(
            copy.kind,
            StructuredNodeKind::Alias {
                name: "big".to_string()
            }
        );
        assert_eq!(copy.child_count, 0);
        assert_eq!(copy.end, copy.id + 1, "the alias has no subtree");
        assert!(!copy.is_merge_key());
        // The anchored entries exist once each, under the anchor.
        assert_eq!(d.node_count(), 5);
        assert_eq!(d.nodes().iter().filter(|n| d.label(n.id) == "b").count(), 1);
    }

    #[test]
    fn an_alias_bomb_costs_what_the_source_costs() {
        // The classic billion-laughs shape: without expansion this is a few
        // dozen nodes, and with it would be 9^9.
        let mut src = String::from("a: &a [x, x, x, x, x, x, x, x, x]\n");
        for (n, prev) in [('b', 'a'), ('c', 'b'), ('d', 'c'), ('e', 'd')] {
            src.push_str(&format!(
                "{n}: &{n} [*{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}]\n"
            ));
        }
        let d = doc(&src);
        assert!(d.node_count() < 100, "{} nodes", d.node_count());
    }

    #[test]
    fn tags_are_kept_in_the_form_they_were_written() {
        let d = doc("%TAG !e! tag:example.com,2000:\n---\na: !!timestamp 2026-08-31\nb: !MyType value\nc: !e!Custom x\nd: !!str 42\n");
        let tags: Vec<Option<&str>> = d
            .children(0)
            .into_iter()
            .filter_map(|id| d.node(id).map(|n| n.tag()))
            .collect();
        assert_eq!(
            tags,
            [
                Some("!!timestamp"),
                Some("!MyType"),
                Some("!e!Custom"),
                Some("!!str")
            ]
        );
        // An explicit `!!str` overrules the core-schema resolution.
        assert_eq!(
            d.node(4).unwrap().scalar().unwrap().kind,
            ScalarKind::String
        );
        assert_eq!(d.search_index().find("MyType", false).len(), 1);
    }

    #[test]
    fn a_tag_is_searchable_as_the_tag_of_its_node() {
        let d = doc("date: !!timestamp 2026-08-31\ncustom: !MyType value\n");
        for (needle, label) in [("!!timestamp", "date"), ("!MyType", "custom")] {
            let hits = d.search_index().find(needle, false);
            assert_eq!(hits.len(), 1, "{needle}");
            assert_eq!(hits[0].field, MatchField::Tag, "{needle}");
            assert_eq!(d.label(hits[0].node), label, "{needle}");
        }
    }

    #[test]
    fn directives_are_kept_above_the_document_they_belong_to() {
        let d = doc("%YAML 1.2\n%TAG !e! tag:example.com,2000:\n---\na: 1\n---\nb: 2\n");
        assert_eq!(d.roots().len(), 2);
        assert_eq!(
            d.roots()[0].directives,
            [
                Directive::Version { major: 1, minor: 2 },
                Directive::Tag {
                    handle: "!e!".into(),
                    prefix: "tag:example.com,2000:".into()
                }
            ]
        );
        assert!(d.roots()[1].directives.is_empty());
        assert_eq!(d.roots()[0].directives[0].text(), "%YAML 1.2");
        assert_eq!(
            d.roots()[1].directives.len(),
            0,
            "a directive belongs to one document only"
        );
    }

    #[test]
    fn every_document_of_a_stream_is_a_root() {
        let d = doc(
            "---\nkind: ConfigMap\ndata:\n  a: 1\n---\nkind: Deployment\nspec:\n  replicas: 3\n",
        );
        assert_eq!(d.roots().len(), 2);
        assert!(d.roots().iter().all(|r| r.explicit));
        assert_eq!(d.label(d.roots()[0].node), "Document 1");
        assert_eq!(d.label(d.roots()[1].node), "Document 2");
        let deep = d.node_count() - 1;
        assert_eq!(
            d.path(deep).breadcrumb(false),
            "Document 2 > spec > replicas"
        );
    }

    #[test]
    fn a_collection_used_as_a_key_is_shown_as_it_was_written() {
        let d = doc("locations:\n  [47.3769, 8.5417]: local\n  [40.7128, -74.0060]: remote\n");
        let keys: Vec<String> = d.children(1).into_iter().map(|id| d.label(id)).collect();
        assert_eq!(keys, ["[47.3769, 8.5417]", "[40.7128, -74.0060]"]);
        assert!(d.node(2).unwrap().key().expect("a key").complex);
        assert_eq!(d.node(2).unwrap().scalar().unwrap().text, "local");
    }

    #[test]
    fn flow_collections_read_like_block_ones() {
        let d = doc("a: {x: 1, y: 2}\nb: [1, 2]\n");
        assert_eq!(
            shape(&d),
            [
                "root {}",
                "  a {}",
                "    x Number \"1\"",
                "    y Number \"2\"",
                "  b []",
                "    [0] Number \"1\"",
                "    [1] Number \"2\"",
            ]
        );
    }

    #[test]
    fn unicode_survives_in_keys_values_and_comments() {
        let d = doc("キー: 値 🎵  # コメント\nGrüße: \"straße\"\n");
        assert_eq!(d.label(1), "キー");
        assert_eq!(d.node(1).unwrap().scalar().unwrap().text, "値 🎵");
        assert_eq!(d.node(2).unwrap().scalar().unwrap().text, "straße");
        assert_eq!(d.search_index().find("コメント", false).len(), 1);
    }

    #[test]
    fn an_empty_document_parses_to_nothing_useful_but_does_not_fail() {
        let d = doc("");
        assert_eq!(d.node_count(), 0);
        assert!(d.is_empty());
        assert_eq!(d.first_semantic(), None);
    }

    #[test]
    fn deep_nesting_is_an_error_rather_than_a_crash() {
        let deep = format!(
            "{}{}",
            "[".repeat(MAX_DEPTH + 10),
            "]".repeat(MAX_DEPTH + 10)
        );
        let error = err(&deep);
        assert!(error.message.contains("nested more than"), "{error}");
        assert!(error.position.is_some(), "{error}");
        // Block nesting is refused the same way, and so is a mapping.
        let blocks: String = (0..MAX_DEPTH + 10)
            .map(|n| format!("{}a:\n", "  ".repeat(n)))
            .collect();
        assert!(err(&blocks).message.contains("nested more than"));
        // And the limit is diple's, not the parser's default: a document at
        // exactly the limit still reads, as it does in JSON.
        let ok = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert_eq!(doc(&ok).node_count(), MAX_DEPTH);
    }

    #[test]
    fn malformed_input_says_where_and_what() {
        for src in [
            "a: 1\n b: 2\n  c: 3\n",
            "[1, 2",
            "a: *undefined\n",
            "{a: 1\n",
            "\"unterminated\n",
        ] {
            let error = err(src);
            assert_eq!(error.format, DocumentKind::Yaml);
            assert!(error.position.is_some(), "{src:?} reports a position");
            assert!(!error.message.is_empty(), "{src:?}");
            let report = error.report();
            assert!(report.starts_with("YAML parse error\n"), "{report}");
        }
    }

    #[test]
    fn a_percent_encoded_tag_reads_back_encoded() {
        let d = parse(&SourceDocument::new("t.yaml", "a: !t%1b%07tag x\n")).unwrap();
        assert_eq!(d.node(1).and_then(|n| n.tag()), Some("!t%1B%07tag"));
    }

    /// A quoted key that bare would be another scalar keeps its quotes in the
    /// path, so `"null":` and `null:` do not share a breadcrumb.
    #[test]
    fn a_quoted_key_that_would_be_another_scalar_keeps_its_quotes_in_the_path() {
        let d = parse(&SourceDocument::new(
            "t.yaml",
            "\"null\": 1\nnull: 2\n'1': 3\n1: 4\n\"plain\": 5\n",
        ))
        .unwrap();
        let crumbs: Vec<String> = (1..d.node_count())
            .map(|n| d.path(n).breadcrumb(false))
            .collect();
        assert_eq!(crumbs, [r#""null""#, "null", r#""1""#, "1", "plain"]);
    }

    // ---- detection -------------------------------------------------------

    fn confident(src: &str) -> bool {
        parse(&SourceDocument::new("<stdin>", src))
            .map(|d| is_confidently_yaml(&d))
            .unwrap_or(false)
    }

    #[test]
    fn structured_configuration_is_recognised() {
        assert!(confident(
            "metadata:\n  name: nginx\nspec:\n  replicas: 3\n"
        ));
        assert!(confident("a: &x 1\nb: *x\n"));
        assert!(confident("---\na:\n  x: 1\n---\nb:\n  y: 2\n"));
        assert!(confident("%YAML 1.2\n---\na: 1\n"));
        assert!(confident("items:\n  - name: a\n    image: b\n"));
        assert!(confident(
            "- hosts: all\n  tasks:\n    - name: x\n      apt: y\n"
        ));
        assert!(confident("services:\n  web:\n    image: nginx\n"));
        assert!(confident("a: !!str x\n"));
    }

    #[test]
    fn prose_and_markdown_stay_markdown() {
        // Every one of these parses as YAML; none of them is a document a
        // reader piped in expecting a structure view.
        for src in [
            "hello\n",
            "- one\n- two\n",
            "title: hello\n",
            "Some prose that happens to parse.\n",
            "# A heading\n\nSome text.\n",
            // Markdown with YAML front matter: the body is a scalar, so the
            // stream is not coherent.
            "---\ntitle: Post\ndate: 2026-08-31\n---\n\nSome **bold** prose.\n",
            // A list whose items have a colon in them, or an introduction
            // over a list: the shapes Markdown writes that YAML also parses
            // as nested collections.
            "items:\n  - name: a\n",
            "Here are the next steps:\n\n- write tests\n- ship it\n",
            "# diple\n\n## Features\n\n- Fast: written in Rust\n- Small binary\n",
            "- Buy milk\n- Call mom: urgent\n",
            "# Links\n\n- Homepage: https://example.com\n",
            "Shopping list:\n- milk\n- eggs\n",
            // Lines of prose that each put a colon after their first word.
            "Note: this is important.\nAlso: check that.\n",
            "Error: file not found\nHint: try again\n",
            // A flat mapping and a multi-document stream are YAML, and are
            // also notes and Markdown separated by rules; they need
            // `--format yaml` when nothing else says so.
            "apiVersion: apps/v1\nkind: Deployment\n",
            "---\na: 1\n---\nb: 2\n",
            "Status: done\nOwner: alice\n",
            "Pros:\n\n- fast\n- small\n\nCons:\n\n- young\n",
            "- one\n- two\n\n---\n\n- three\n- four\n",
            "---\ntitle: Post\n---\n\n# Heading\n\n- a\n- b\n",
        ] {
            assert!(!confident(src), "{src:?} must stay Markdown");
        }
    }

    #[test]
    fn an_unparseable_document_is_never_confident() {
        assert!(!confident("a: 1\n b: 2\n  c: 3\n"));
        assert!(!confident(""));
    }
    /// Found by fuzzing: YAML counts a lone `\r` as a line break and
    /// `SourceDocument` does not, so taking the parser's line number pointed
    /// at a line the excerpt could not show and the caret disappeared.
    #[test]
    fn an_error_after_a_lone_carriage_return_still_points_at_something() {
        let source = SourceDocument::new("t.yaml", "\r\r*");
        let error = parse(&source).expect_err("an anchor with no name");
        let position = error.position.expect("a position");
        assert!(
            position.line >= 1 && position.line <= source.line_count(),
            "{position:?} is inside a {}-line document",
            source.line_count()
        );
        let report = error.report();
        assert!(report.contains('^'), "the caret survived: {report}");
    }
}
