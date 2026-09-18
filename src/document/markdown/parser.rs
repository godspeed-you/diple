//! Markdown → [`Document`] using `pulldown-cmark`'s event stream.
//!
//! The builder is a stack machine: block containers (root, quote, list item,
//! footnote, …) live on `containers`; inline containers (emphasis, links, …)
//! live on `inline_stack`. Every node — including nested ones — receives a
//! unique pre-order [`NodeId`] at its start event, which makes
//! `Document::node(id)` and the section lookup work for nested nodes.
//!
//! The parser never panics on input: unknown or unbalanced events degrade to
//! plain text or are ignored.

use std::ops::Range;

use pulldown_cmark::{
    Alignment as PdAlignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use super::anchors::AnchorIndex;
use super::ast::{
    inlines_to_text, Alignment, CodeBlock, Document, Footnote, Heading, Image, Inline, Inlines,
    List, ListItem, MermaidBlock, Node, NodeId, NodeKind, SourceSpan, Table,
};
use super::links::Link;
use crate::document::search::SearchIndex;
use crate::document::source::SourceDocument;
use super::sections;

/// Parser options used by diple.
pub fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_HEADING_ATTRIBUTES
}

/// Parse Markdown source into a fully derived [`Document`].
pub fn parse(source: &str) -> Document {
    parse_source(SourceDocument::new("<document>", source))
}

/// Parse a named source, keeping it on the document.
///
/// The source outlives the parse for the same reasons it does in the
/// structured backends: error excerpts, source spans and a future source
/// view all need the bytes the document was made of.
pub fn parse_source(source: SourceDocument) -> Document {
    let mut builder = Builder::new();
    for (event, range) in Parser::new_ext(source.text(), options()).into_offset_iter() {
        builder.event(event, range);
    }
    let mut doc = builder.finish();
    sections::build(&mut doc);
    doc.search = super::search::build(&doc);
    doc.source = source;
    doc
}

enum Container {
    Root {
        nodes: Vec<Node>,
    },
    Quote {
        id: NodeId,
        start: usize,
        nodes: Vec<Node>,
    },
    List {
        id: NodeId,
        start: usize,
        ordered: bool,
        first: Option<u64>,
        items: Vec<ListItem>,
    },
    Item {
        checked: Option<bool>,
        blocks: Vec<Node>,
    },
    FootnoteDef {
        id: NodeId,
        start: usize,
        label: String,
        blocks: Vec<Node>,
    },
    Paragraph {
        id: NodeId,
        start: usize,
        end: usize,
        implicit: bool,
        inlines: Inlines,
    },
    Heading {
        id: NodeId,
        start: usize,
        level: u8,
        explicit_id: Option<String>,
        inlines: Inlines,
    },
    Code {
        id: NodeId,
        start: usize,
        language: Option<String>,
        code: String,
    },
    Html {
        id: NodeId,
        start: usize,
        text: String,
    },
    Table {
        id: NodeId,
        start: usize,
        alignments: Vec<Alignment>,
        header: Vec<Inlines>,
        rows: Vec<Vec<Inlines>>,
        in_head: bool,
    },
    TableRow {
        cells: Vec<Inlines>,
    },
    TableCell {
        inlines: Inlines,
    },
    /// Unknown block-level tag: children are hoisted into the parent.
    Transparent {
        nodes: Vec<Node>,
    },
}

enum InlineFrame {
    Emph(Inlines),
    Strong(Inlines),
    Strike(Inlines),
    Link {
        url: String,
        title: Option<String>,
        link_id: usize,
        inlines: Inlines,
    },
    Image {
        url: String,
        inlines: Inlines,
    },
    /// Unknown inline tag (superscript, …): content is hoisted.
    Transparent(Inlines),
}

impl InlineFrame {
    fn inlines_mut(&mut self) -> &mut Inlines {
        match self {
            InlineFrame::Emph(i)
            | InlineFrame::Strong(i)
            | InlineFrame::Strike(i)
            | InlineFrame::Transparent(i) => i,
            InlineFrame::Link { inlines, .. } | InlineFrame::Image { inlines, .. } => inlines,
        }
    }
}

struct Builder {
    next_id: NodeId,
    containers: Vec<Container>,
    inline_stack: Vec<InlineFrame>,
    /// Ids of currently open nodes, outermost first.
    open_ids: Vec<NodeId>,
    links: Vec<Link>,
    anchors: AnchorIndex,
    footnotes: Vec<Footnote>,
    title: Option<String>,
    top_level: Vec<NodeId>,
}

impl Builder {
    fn new() -> Self {
        Self {
            next_id: 0,
            containers: vec![Container::Root { nodes: Vec::new() }],
            inline_stack: Vec::new(),
            open_ids: Vec::new(),
            links: Vec::new(),
            anchors: AnchorIndex::new(),
            footnotes: Vec::new(),
            title: None,
            top_level: Vec::new(),
        }
    }

    /// Allocate a node id (pre-order) and record its top-level ancestor.
    fn alloc(&mut self) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        let top = self.open_ids.first().copied().unwrap_or(id);
        self.top_level.push(top);
        id
    }

    fn open(&mut self, container: Container, id: Option<NodeId>) {
        if let Some(id) = id {
            self.open_ids.push(id);
        }
        self.containers.push(container);
    }

    fn close_id(&mut self) {
        self.open_ids.pop();
    }

    /// Id of the innermost node that owns inline content (for links).
    fn current_node_id(&self) -> NodeId {
        self.open_ids.last().copied().unwrap_or(0)
    }

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.start(tag, range),
            Event::End(tag) => self.end(tag, range),
            Event::Text(t) => self.text(&t, range),
            Event::Code(c) => self.inline(Inline::Code(c.into_string()), range),
            Event::InlineMath(m) | Event::DisplayMath(m) => {
                self.inline(Inline::Code(m.into_string()), range)
            }
            Event::Html(h) => self.html(&h, range),
            Event::InlineHtml(h) => self.inline(Inline::Text(h.into_string()), range),
            Event::FootnoteReference(label) => {
                self.inline(Inline::FootnoteRef(label.into_string()), range)
            }
            Event::SoftBreak => self.inline(Inline::SoftBreak, range),
            Event::HardBreak => self.inline(Inline::HardBreak, range),
            Event::Rule => {
                self.close_implicit_paragraph();
                let id = self.alloc();
                self.push_node(Node {
                    id,
                    kind: NodeKind::HorizontalRule,
                    span: span(range),
                });
            }
            Event::TaskListMarker(done) => {
                if let Some(Container::Item { checked, .. }) = self.containers.last_mut() {
                    *checked = Some(done);
                }
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        let start = range.start;
        match tag {
            Tag::Paragraph => {
                self.close_implicit_paragraph();
                let id = self.alloc();
                self.open(
                    Container::Paragraph {
                        id,
                        start,
                        end: range.end,
                        implicit: false,
                        inlines: Vec::new(),
                    },
                    Some(id),
                );
            }
            Tag::Heading {
                level,
                id: explicit,
                ..
            } => {
                self.close_implicit_paragraph();
                let id = self.alloc();
                self.open(
                    Container::Heading {
                        id,
                        start,
                        level: heading_level(level),
                        explicit_id: explicit.map(|s| s.into_string()),
                        inlines: Vec::new(),
                    },
                    Some(id),
                );
            }
            Tag::BlockQuote(_) => {
                self.close_implicit_paragraph();
                let id = self.alloc();
                self.open(
                    Container::Quote {
                        id,
                        start,
                        nodes: Vec::new(),
                    },
                    Some(id),
                );
            }
            Tag::CodeBlock(kind) => {
                self.close_implicit_paragraph();
                let language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(|s| s.to_string())
                    }
                    CodeBlockKind::Indented => None,
                };
                let id = self.alloc();
                self.open(
                    Container::Code {
                        id,
                        start,
                        language,
                        code: String::new(),
                    },
                    Some(id),
                );
            }
            Tag::HtmlBlock => {
                self.close_implicit_paragraph();
                let id = self.alloc();
                self.open(
                    Container::Html {
                        id,
                        start,
                        text: String::new(),
                    },
                    Some(id),
                );
            }
            Tag::List(first) => {
                self.close_implicit_paragraph();
                let id = self.alloc();
                self.open(
                    Container::List {
                        id,
                        start,
                        ordered: first.is_some(),
                        first,
                        items: Vec::new(),
                    },
                    Some(id),
                );
            }
            Tag::Item => {
                self.close_implicit_paragraph();
                self.open(
                    Container::Item {
                        checked: None,
                        blocks: Vec::new(),
                    },
                    None,
                );
            }
            Tag::FootnoteDefinition(label) => {
                self.close_implicit_paragraph();
                let id = self.alloc();
                self.open(
                    Container::FootnoteDef {
                        id,
                        start,
                        label: label.into_string(),
                        blocks: Vec::new(),
                    },
                    Some(id),
                );
            }
            Tag::Table(alignments) => {
                self.close_implicit_paragraph();
                let id = self.alloc();
                self.open(
                    Container::Table {
                        id,
                        start,
                        alignments: alignments.iter().map(|a| alignment(*a)).collect(),
                        header: Vec::new(),
                        rows: Vec::new(),
                        in_head: false,
                    },
                    Some(id),
                );
            }
            Tag::TableHead => {
                if let Some(Container::Table { in_head, .. }) = self.containers.last_mut() {
                    *in_head = true;
                }
            }
            Tag::TableRow => self.open(Container::TableRow { cells: Vec::new() }, None),
            Tag::TableCell => self.open(
                Container::TableCell {
                    inlines: Vec::new(),
                },
                None,
            ),
            Tag::Emphasis => self.inline_stack.push(InlineFrame::Emph(Vec::new())),
            Tag::Strong => self.inline_stack.push(InlineFrame::Strong(Vec::new())),
            Tag::Strikethrough => self.inline_stack.push(InlineFrame::Strike(Vec::new())),
            Tag::Superscript | Tag::Subscript => {
                self.inline_stack.push(InlineFrame::Transparent(Vec::new()))
            }
            Tag::Link {
                dest_url, title, ..
            } => {
                self.ensure_inline_target(range.clone());
                let url = dest_url.into_string();
                let title = non_empty(title.into_string());
                let link_id = self.links.len();
                self.links.push(Link {
                    id: link_id,
                    kind: Link::classify(&url),
                    url: url.clone(),
                    text: String::new(),
                    title: title.clone(),
                    node: self.current_node_id(),
                });
                self.inline_stack.push(InlineFrame::Link {
                    url,
                    title,
                    link_id,
                    inlines: Vec::new(),
                });
            }
            Tag::Image {
                dest_url, title, ..
            } => {
                self.ensure_inline_target(range);
                let _ = title;
                self.inline_stack.push(InlineFrame::Image {
                    url: dest_url.into_string(),
                    inlines: Vec::new(),
                });
            }
            Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => {
                self.close_implicit_paragraph();
                self.open(Container::Transparent { nodes: Vec::new() }, None);
            }
        }
    }

    fn end(&mut self, tag: TagEnd, range: Range<usize>) {
        match tag {
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image => self.end_inline(),
            TagEnd::TableHead => {
                if let Some(Container::Table { in_head, .. }) = self.containers.last_mut() {
                    *in_head = false;
                }
            }
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::FootnoteDefinition
            | TagEnd::Table
            | TagEnd::TableRow
            | TagEnd::TableCell
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => self.end_block(range.end),
        }
    }

    fn end_inline(&mut self) {
        let Some(frame) = self.inline_stack.pop() else {
            return;
        };
        let inline = match frame {
            InlineFrame::Emph(i) => Inline::Emph(i),
            InlineFrame::Strong(i) => Inline::Strong(i),
            InlineFrame::Strike(i) => Inline::Strike(i),
            InlineFrame::Transparent(i) => {
                for item in i {
                    self.push_inline(item);
                }
                return;
            }
            InlineFrame::Link {
                url,
                title,
                link_id,
                inlines,
            } => {
                if let Some(link) = self.links.get_mut(link_id) {
                    link.text = inlines_to_text(&inlines);
                }
                Inline::Link {
                    inlines,
                    url,
                    title,
                    id: link_id,
                }
            }
            InlineFrame::Image { url, inlines } => Inline::Image {
                alt: inlines_to_text(&inlines),
                url,
            },
        };
        self.push_inline(inline);
    }

    /// Close the innermost block container and attach it to its parent.
    fn end_block(&mut self, end: usize) {
        // A block end while an implicit paragraph is open (tight list item).
        if matches!(
            self.containers.last(),
            Some(Container::Paragraph { implicit: true, .. })
        ) {
            self.close_implicit_paragraph();
        }
        // Unterminated inline frames (should not happen) are flushed as text.
        while !self.inline_stack.is_empty() {
            self.end_inline();
        }
        let Some(container) = self.containers.pop() else {
            return;
        };
        match container {
            Container::Root { nodes } => {
                // Never pop the root.
                self.containers.push(Container::Root { nodes });
            }
            Container::Quote { id, start, nodes } => {
                self.close_id();
                self.push_node(Node {
                    id,
                    kind: NodeKind::Quote(nodes),
                    span: sp(start, end),
                });
            }
            Container::List {
                id,
                start,
                ordered,
                first,
                items,
            } => {
                self.close_id();
                self.push_node(Node {
                    id,
                    kind: NodeKind::List(List {
                        ordered,
                        start: first,
                        items,
                    }),
                    span: sp(start, end),
                });
            }
            Container::Item { checked, blocks } => {
                if let Some(Container::List { items, .. }) = self.containers.last_mut() {
                    items.push(ListItem { checked, blocks });
                } else {
                    // Orphan item: hoist its blocks.
                    for b in blocks {
                        self.push_node(b);
                    }
                }
            }
            Container::FootnoteDef {
                id,
                start,
                label,
                blocks,
            } => {
                self.close_id();
                let footnote = Footnote { label, blocks };
                self.footnotes.push(footnote.clone());
                self.push_node(Node {
                    id,
                    kind: NodeKind::FootnoteDefinition(footnote),
                    span: sp(start, end),
                });
            }
            Container::Paragraph {
                id,
                start,
                end: implicit_end,
                implicit,
                inlines,
            } => {
                self.close_id();
                let end = if implicit {
                    implicit_end.max(start)
                } else {
                    end
                };
                let kind = match inlines.as_slice() {
                    [Inline::Image { alt, url }] => NodeKind::Image(Image {
                        alt: alt.clone(),
                        url: url.clone(),
                        title: None,
                    }),
                    _ => NodeKind::Paragraph(inlines),
                };
                self.push_node(Node {
                    id,
                    kind,
                    span: sp(start, end),
                });
            }
            Container::Heading {
                id,
                start,
                level,
                explicit_id,
                inlines,
            } => {
                self.close_id();
                let text = inlines_to_text(&inlines).trim().to_string();
                let base = explicit_id.as_deref().unwrap_or(&text);
                let anchor = self.anchors.insert(base, id);
                if level == 1 && self.title.is_none() {
                    self.title = Some(text.clone());
                }
                self.push_node(Node {
                    id,
                    kind: NodeKind::Heading(Heading {
                        level,
                        inlines,
                        text,
                        anchor,
                    }),
                    span: sp(start, end),
                });
            }
            Container::Code {
                id,
                start,
                language,
                code,
            } => {
                self.close_id();
                let is_mermaid = language
                    .as_deref()
                    .is_some_and(|l| l.eq_ignore_ascii_case("mermaid"));
                let kind = if is_mermaid {
                    NodeKind::Mermaid(MermaidBlock { source: code })
                } else {
                    NodeKind::CodeBlock(CodeBlock { language, code })
                };
                self.push_node(Node {
                    id,
                    kind,
                    span: sp(start, end),
                });
            }
            Container::Html { id, start, text } => {
                self.close_id();
                self.push_node(Node {
                    id,
                    kind: NodeKind::Html(text),
                    span: sp(start, end),
                });
            }
            Container::Table {
                id,
                start,
                alignments,
                header,
                mut rows,
                ..
            } => {
                self.close_id();
                let columns = alignments.len().max(header.len());
                let mut header = header;
                header.resize_with(columns, Vec::new);
                for row in &mut rows {
                    row.resize_with(columns, Vec::new);
                }
                let mut alignments = alignments;
                alignments.resize(columns, Alignment::None);
                self.push_node(Node {
                    id,
                    kind: NodeKind::Table(Table {
                        alignments,
                        header,
                        rows,
                    }),
                    span: sp(start, end),
                });
            }
            Container::TableRow { cells } => {
                if let Some(Container::Table { rows, .. }) = self.containers.last_mut() {
                    rows.push(cells);
                }
            }
            Container::TableCell { inlines } => match self.containers.last_mut() {
                Some(Container::TableRow { cells }) => cells.push(inlines),
                Some(Container::Table {
                    header,
                    in_head: true,
                    ..
                }) => header.push(inlines),
                Some(Container::Table { rows, .. }) => {
                    // Cell outside a row: start a new row.
                    rows.push(vec![inlines]);
                }
                _ => {}
            },
            Container::Transparent { nodes } => {
                for n in nodes {
                    self.push_node(n);
                }
            }
        }
    }

    /// Attach a finished node to the innermost block container.
    fn push_node(&mut self, node: Node) {
        for container in self.containers.iter_mut().rev() {
            match container {
                Container::Root { nodes }
                | Container::Quote { nodes, .. }
                | Container::Transparent { nodes } => {
                    nodes.push(node);
                    return;
                }
                Container::Item { blocks, .. } | Container::FootnoteDef { blocks, .. } => {
                    blocks.push(node);
                    return;
                }
                _ => continue,
            }
        }
    }

    fn text(&mut self, text: &str, range: Range<usize>) {
        match self.containers.last_mut() {
            Some(Container::Code { code, .. }) => code.push_str(text),
            Some(Container::Html { text: html, .. }) => html.push_str(text),
            _ => self.inline(Inline::Text(text.to_string()), range),
        }
    }

    fn html(&mut self, html: &str, range: Range<usize>) {
        match self.containers.last_mut() {
            Some(Container::Html { text, .. }) => text.push_str(html),
            Some(Container::Code { code, .. }) => code.push_str(html),
            _ => self.inline(Inline::Text(html.to_string()), range),
        }
    }

    /// Add an inline to the current inline target, creating an implicit
    /// paragraph when inline content appears directly inside a container
    /// (tight list items).
    fn inline(&mut self, inline: Inline, range: Range<usize>) {
        self.ensure_inline_target(range);
        self.push_inline(inline);
    }

    fn push_inline(&mut self, inline: Inline) {
        if let Some(frame) = self.inline_stack.last_mut() {
            frame.inlines_mut().push(inline);
            return;
        }
        if let Some(
            Container::Paragraph { inlines, .. }
            | Container::Heading { inlines, .. }
            | Container::TableCell { inlines },
        ) = self.containers.last_mut()
        {
            inlines.push(inline);
        }
        // Inline content anywhere else (e.g. inside a code container) is
        // dropped rather than panicking.
    }

    fn ensure_inline_target(&mut self, range: Range<usize>) {
        match self.containers.last_mut() {
            Some(Container::Paragraph {
                end,
                implicit: true,
                ..
            }) => {
                *end = (*end).max(range.end);
            }
            Some(
                Container::Paragraph { .. }
                | Container::Heading { .. }
                | Container::TableCell { .. },
            ) => {}
            Some(Container::Code { .. } | Container::Html { .. }) => {}
            _ => {
                let id = self.alloc();
                self.open(
                    Container::Paragraph {
                        id,
                        start: range.start,
                        end: range.end,
                        implicit: true,
                        inlines: Vec::new(),
                    },
                    Some(id),
                );
            }
        }
    }

    fn close_implicit_paragraph(&mut self) {
        if let Some(Container::Paragraph {
            implicit: true,
            end,
            ..
        }) = self.containers.last()
        {
            let end = *end;
            while !self.inline_stack.is_empty() {
                self.end_inline();
            }
            // Temporarily mark as explicit so end_block does not recurse.
            if let Some(Container::Paragraph { implicit, .. }) = self.containers.last_mut() {
                *implicit = false;
            }
            self.end_block(end);
        }
    }

    fn finish(mut self) -> Document {
        // Close anything left open (malformed input).
        while self.containers.len() > 1 {
            let end = self.next_end();
            self.end_block(end);
        }
        let nodes = match self.containers.pop() {
            Some(Container::Root { nodes }) => nodes,
            _ => Vec::new(),
        };
        Document {
            source: SourceDocument::default(),
            nodes,
            sections: Vec::new(),
            anchors: self.anchors,
            footnotes: self.footnotes,
            title: self.title,
            links: self.links,
            top_level: self.top_level,
            node_section: Vec::new(),
            search: SearchIndex::default(),
        }
    }

    fn next_end(&self) -> usize {
        match self.containers.last() {
            Some(Container::Paragraph { end, .. }) => *end,
            _ => 0,
        }
    }
}

fn span(range: Range<usize>) -> SourceSpan {
    SourceSpan {
        start: range.start,
        end: range.end,
    }
}

fn sp(start: usize, end: usize) -> SourceSpan {
    SourceSpan {
        start,
        end: end.max(start),
    }
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn alignment(a: PdAlignment) -> Alignment {
    match a {
        PdAlignment::None => Alignment::None,
        PdAlignment::Left => Alignment::Left,
        PdAlignment::Center => Alignment::Center,
        PdAlignment::Right => Alignment::Right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::links::LinkKind;

    fn kinds(doc: &Document) -> Vec<&NodeKind> {
        doc.nodes.iter().map(|n| &n.kind).collect()
    }

    fn text_of(inlines: &[Inline]) -> String {
        inlines_to_text(inlines)
    }

    #[test]
    fn headings_levels_text_anchor_title() {
        let doc = parse("# Title\n\n## Sub *emph* `code`\n\n###### Deep\n");
        assert_eq!(doc.nodes.len(), 3);
        let NodeKind::Heading(h) = &doc.nodes[0].kind else {
            panic!("heading")
        };
        assert_eq!(
            (h.level, h.text.as_str(), h.anchor.as_str()),
            (1, "Title", "title")
        );
        let NodeKind::Heading(h) = &doc.nodes[1].kind else {
            panic!("heading")
        };
        assert_eq!(h.level, 2);
        assert_eq!(h.text, "Sub emph code");
        assert_eq!(h.anchor, "sub-emph-code");
        assert!(matches!(h.inlines[1], Inline::Emph(_)));
        let NodeKind::Heading(h) = &doc.nodes[2].kind else {
            panic!("heading")
        };
        assert_eq!(h.level, 6);
        assert_eq!(doc.title.as_deref(), Some("Title"));
        assert_eq!(doc.anchors.resolve("#deep"), Some(doc.nodes[2].id));
    }

    #[test]
    fn heading_explicit_attribute_and_setext() {
        let doc = parse("Setext\n======\n\n## Custom {#my-id}\n");
        let NodeKind::Heading(h) = &doc.nodes[0].kind else {
            panic!()
        };
        assert_eq!((h.level, h.anchor.as_str()), (1, "setext"));
        let NodeKind::Heading(h) = &doc.nodes[1].kind else {
            panic!()
        };
        assert_eq!(h.text, "Custom");
        assert_eq!(h.anchor, "my-id");
    }

    #[test]
    /// Every inline construct is recognised, carries its payload, and keeps
    /// its source order.
    ///
    /// The constructs are located by *kind*, never by position: adjacent
    /// `Text` runs are an implementation detail the parser is free to
    /// coalesce, so an index into `p` would pin the wrong thing.
    fn paragraph_inline_formatting() {
        let doc =
            parse("plain **bold** *it* ~~gone~~ `c` [l](http://x \"T\") ![a](i.png)\nsoft  \nhard");
        let NodeKind::Paragraph(p) = &doc.nodes[0].kind else {
            panic!("a paragraph")
        };

        // Order of the marked-up constructs, ignoring the plain text between.
        let tag = |i: &Inline| match i {
            Inline::Text(_) => None,
            Inline::Code(_) => Some("code"),
            Inline::Emph(_) => Some("emph"),
            Inline::Strong(_) => Some("strong"),
            Inline::Strike(_) => Some("strike"),
            Inline::Link { .. } => Some("link"),
            Inline::Image { .. } => Some("image"),
            Inline::SoftBreak => Some("soft"),
            Inline::HardBreak => Some("hard"),
            Inline::FootnoteRef(_) => Some("footnote"),
        };
        assert_eq!(
            p.iter().filter_map(tag).collect::<Vec<_>>(),
            ["strong", "emph", "strike", "code", "link", "image", "soft", "hard"]
        );

        // Payloads, by kind.
        let of = |want: &'static str| -> Vec<String> {
            p.iter()
                .filter(|i| tag(i) == Some(want))
                .map(|i| match i {
                    Inline::Code(c) => c.clone(),
                    Inline::Emph(x) | Inline::Strong(x) | Inline::Strike(x) => text_of(x),
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        assert_eq!(of("strong"), ["bold"]);
        assert_eq!(of("emph"), ["it"]);
        assert_eq!(of("strike"), ["gone"]);
        assert_eq!(of("code"), ["c"]);

        let link = p
            .iter()
            .find(|i| matches!(i, Inline::Link { .. }))
            .expect("a link");
        let Inline::Link {
            inlines,
            url,
            title,
            id,
        } = link
        else {
            unreachable!()
        };
        assert_eq!(text_of(inlines), "l");
        assert_eq!(url, "http://x");
        assert_eq!(title.as_deref(), Some("T"));
        assert_eq!(*id, 0);

        let image = p
            .iter()
            .find(|i| matches!(i, Inline::Image { .. }))
            .expect("an image");
        assert!(matches!(image, Inline::Image { alt, url } if alt == "a" && url == "i.png"));

        // The plain text of the paragraph carries the words and none of the
        // markers.
        let plain = text_of(p);
        for word in ["plain", "bold", "it", "gone", "c", "l", "soft", "hard"] {
            assert!(plain.contains(word), "{word:?} missing from {plain:?}");
        }
        for marker in ["**", "~~", "`", "](", "!["] {
            assert!(!plain.contains(marker), "{marker:?} leaked into {plain:?}");
        }

        assert_eq!(doc.links.len(), 1);
        assert_eq!(doc.links[0].text, "l");
        assert_eq!(doc.links[0].node, doc.nodes[0].id);
        assert_eq!(doc.links[0].kind, LinkKind::External);
    }

    #[test]
    fn block_image_is_image_node() {
        let doc = parse("![Logo](logo.png)\n\ntext ![i](x.png)\n");
        assert!(
            matches!(&doc.nodes[0].kind, NodeKind::Image(Image { alt, url, .. }) if alt == "Logo" && url == "logo.png")
        );
        assert!(matches!(&doc.nodes[1].kind, NodeKind::Paragraph(_)));
    }

    #[test]
    fn code_blocks_fenced_indented_and_mermaid() {
        let src = "```rust\nfn main() {}\n```\n\n    indented\n\n```mermaid\ngraph LR\n  A --> B\n```\n\n```\nnolang\n```\n";
        let doc = parse(src);
        let k = kinds(&doc);
        assert!(
            matches!(k[0], NodeKind::CodeBlock(c) if c.language.as_deref() == Some("rust") && c.code == "fn main() {}\n")
        );
        assert!(
            matches!(k[1], NodeKind::CodeBlock(c) if c.language.is_none() && c.code == "indented\n")
        );
        assert!(matches!(k[2], NodeKind::Mermaid(m) if m.source == "graph LR\n  A --> B\n"));
        assert!(matches!(k[3], NodeKind::CodeBlock(c) if c.language.is_none()));
        assert_eq!(
            &src[doc.nodes[0].span.start..doc.nodes[0].span.end],
            "```rust\nfn main() {}\n```"
        );
        assert_eq!(
            &src[doc.nodes[2].span.start..doc.nodes[2].span.end],
            "```mermaid\ngraph LR\n  A --> B\n```"
        );
    }

    #[test]
    fn mermaid_language_case_insensitive_with_info_string() {
        let doc = parse("```Mermaid title=x\ngraph TD\n```\n");
        assert!(matches!(&doc.nodes[0].kind, NodeKind::Mermaid(_)));
    }

    #[test]
    fn blockquote_nested_blocks() {
        let doc = parse("> quote line\n>\n> ## Inner heading\n>\n> - item\n");
        let NodeKind::Quote(children) = &doc.nodes[0].kind else {
            panic!()
        };
        assert_eq!(children.len(), 3);
        assert!(matches!(children[0].kind, NodeKind::Paragraph(_)));
        assert!(matches!(children[1].kind, NodeKind::Heading(_)));
        assert!(matches!(children[2].kind, NodeKind::List(_)));
        // Heading inside quote is not a section.
        assert!(doc.sections.is_empty());
        // But it has an anchor and can be looked up by id.
        assert_eq!(doc.anchors.resolve("inner-heading"), Some(children[1].id));
        assert_eq!(doc.node(children[1].id).map(|n| n.id), Some(children[1].id));
    }

    #[test]
    fn lists_ordered_unordered_nested_tasks() {
        let src = "- a\n- b\n  - b1\n  - b2\n\n3. three\n4. four\n\n- [x] done\n- [ ] todo\n";
        let doc = parse(src);
        let NodeKind::List(ul) = &doc.nodes[0].kind else {
            panic!()
        };
        assert!(!ul.ordered);
        assert_eq!(ul.items.len(), 2);
        assert!(matches!(&ul.items[0].blocks[0].kind, NodeKind::Paragraph(p) if text_of(p) == "a"));
        let NodeKind::List(nested) = &ul.items[1].blocks[1].kind else {
            panic!("nested list")
        };
        assert_eq!(nested.items.len(), 2);
        let NodeKind::List(ol) = &doc.nodes[1].kind else {
            panic!()
        };
        assert!(ol.ordered);
        assert_eq!(ol.start, Some(3));
        let NodeKind::List(tasks) = &doc.nodes[2].kind else {
            panic!()
        };
        assert_eq!(tasks.items[0].checked, Some(true));
        assert_eq!(tasks.items[1].checked, Some(false));
        assert_eq!(ul.items[0].checked, None);
        // Every nested node has a unique id and is reachable.
        let ids: Vec<_> = doc.walk().map(|n| n.id).collect();
        let mut sorted = ids.clone();
        sorted.dedup();
        assert_eq!(ids, sorted);
        assert_eq!(ids, (0..doc.node_count()).collect::<Vec<_>>());
        for id in ids {
            assert_eq!(doc.node(id).map(|n| n.id), Some(id));
        }
    }

    #[test]
    fn loose_list_items_and_code_in_item() {
        let doc = parse("1. first\n\n   para two\n\n   ```sh\n   ls\n   ```\n2. second\n");
        let NodeKind::List(l) = &doc.nodes[0].kind else {
            panic!()
        };
        assert_eq!(l.items[0].blocks.len(), 3);
        assert!(matches!(l.items[0].blocks[2].kind, NodeKind::CodeBlock(_)));
    }

    #[test]
    fn tables_alignment_and_padding() {
        let src = "| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 |\n| x | **y** | z |\n";
        let doc = parse(src);
        let NodeKind::Table(t) = &doc.nodes[0].kind else {
            panic!()
        };
        assert_eq!(
            t.alignments,
            vec![Alignment::Left, Alignment::Center, Alignment::Right]
        );
        assert_eq!(t.header.len(), 3);
        assert_eq!(text_of(&t.header[1]), "b");
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0].len(), 3, "short row padded");
        assert!(matches!(&t.rows[1][1][0], Inline::Strong(_)));
    }

    #[test]
    fn horizontal_rule_and_html() {
        let doc = parse("a\n\n---\n\n<div class=\"x\">\nhi\n</div>\n\nb <span>c</span>\n");
        let k = kinds(&doc);
        assert!(matches!(k[1], NodeKind::HorizontalRule));
        assert!(
            matches!(k[2], NodeKind::Html(h) if h.contains("<div class=\"x\">") && h.contains("</div>"))
        );
        assert!(matches!(k[3], NodeKind::Paragraph(p) if text_of(p) == "b <span>c</span>"));
    }

    #[test]
    fn footnotes() {
        let doc = parse("Text[^1] more.\n\n[^1]: The note\n    continued.\n");
        let NodeKind::Paragraph(p) = &doc.nodes[0].kind else {
            panic!()
        };
        assert!(p.contains(&Inline::FootnoteRef("1".into())));
        assert_eq!(doc.footnotes.len(), 1);
        assert_eq!(doc.footnotes[0].label, "1");
        assert!(
            matches!(&doc.nodes[1].kind, NodeKind::FootnoteDefinition(f) if f.label == "1" && !f.blocks.is_empty())
        );
    }

    #[test]
    fn internal_links_resolve_to_anchors() {
        let doc = parse("# Intro\n\nSee [intro](#intro) and [rel](docs/x.md).\n");
        assert_eq!(doc.links.len(), 2);
        assert_eq!(doc.links[0].kind, LinkKind::Internal("intro".into()));
        assert_eq!(doc.anchors.resolve("intro"), Some(0));
        assert_eq!(doc.links[1].kind, LinkKind::Relative);
    }

    #[test]
    fn spans_cover_source() {
        let src = "# H\n\npara\n\n- li\n";
        let doc = parse(src);
        assert_eq!(
            &src[doc.nodes[0].span.start..doc.nodes[0].span.end],
            "# H\n"
        );
        assert_eq!(
            &src[doc.nodes[1].span.start..doc.nodes[1].span.end],
            "para\n"
        );
        assert_eq!(
            &src[doc.nodes[2].span.start..doc.nodes[2].span.end],
            "- li\n"
        );
        let NodeKind::List(l) = &doc.nodes[2].kind else {
            panic!()
        };
        let s = l.items[0].blocks[0].span;
        assert_eq!(&src[s.start..s.end], "li");
    }

    #[test]
    fn malformed_input_does_not_panic() {
        let samples = [
            "```\nunterminated",
            "| a |\n|---\n| b",
            "> > > deep\n- - - x",
            "[^x]",
            "**unclosed *emph",
            "# \n\n##\n",
            "\u{0}\u{ffff}",
            "<div>\n",
            "- [ ]",
            "1.\n2.",
            "![](",
        ];
        for s in samples {
            let doc = parse(s);
            for n in doc.walk() {
                assert!(doc.node(n.id).is_some());
            }
        }
    }

    #[test]
    fn empty_document() {
        let doc = parse("");
        assert!(doc.nodes.is_empty());
        assert_eq!(doc.node_count(), 0);
        assert!(doc.title.is_none());
    }
}
