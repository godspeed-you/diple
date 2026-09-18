//! The Markdown backend.
//!
//! Markdown keeps its own semantic model — headings, sections, anchors,
//! footnotes, links, tables, code blocks, Mermaid fences — rather than being
//! flattened into the key/value tree JSON and YAML share. That is the point
//! of the format boundary: the common layer describes what a document *can
//! do*, and each backend keeps the vocabulary its format actually has.
//!
//! The pipeline is `Markdown text → [`parser::parse`] → [`Document`]`. The
//! document owns everything that is independent of terminal geometry: the
//! block/inline AST, the section hierarchy used for folding and navigation,
//! heading anchors, the ordered link list and the search index.

pub mod anchors;
pub mod ast;
pub mod links;
pub mod parser;
pub mod search;
pub mod sections;

pub use anchors::AnchorIndex;
pub use ast::{
    inlines_to_text, Alignment, CodeBlock, Document, Footnote, Heading, Image, Inline, Inlines,
    LinkId, List, ListItem, MermaidBlock, Node, NodeKind, Table, Walk,
};
pub use links::{Link, LinkKind};
pub use parser::{parse, parse_source};
pub use sections::{Section, SectionId};
