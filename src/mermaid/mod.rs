//! Mermaid diagram support: subset parser, native terminal layout, `mmdc`
//! integration and backend selection.
//!
//! Owned by Workstream E.
//!
//! # Pipeline
//!
//! ```text
//! MermaidBlock ──▶ detect::diagram_kind ──▶ select::decide
//!                                              │
//!                    ┌─────────────────────────┼─────────────────────────┐
//!                    ▼                         ▼                         ▼
//!            parser::parse                mmdc::MmdcRunner        source fallback
//!            terminal::render             image::ImageData        + non-fatal warning
//! ```
//!
//! # Invariants
//!
//! * Nothing in this module panics: the subset parser is a fuzz target
//!   and every external failure (missing `mmdc`, timeout, bad PNG)
//!   becomes a [`MermaidRender`] with a `warning`, never an abort.
//! * The module is independent of `layout`, `render` and `terminal`; the
//!   terminal's capabilities enter through [`RenderEnvironment`], and image
//!   protocol encoding is left to Workstream A.
//!
//! # Example
//!
//! ```
//! use diple::config::schema::MermaidConfig;
//! use diple::document::markdown::ast::MermaidBlock;
//! use diple::mermaid::{select_backend, MermaidOutput, RenderEnvironment};
//!
//! let cfg = MermaidConfig::default();
//! let env = RenderEnvironment::default();
//! let renderer = select_backend(&cfg, &env);
//! let block = MermaidBlock { source: "graph LR\n A --> B\n".into() };
//! match renderer.render(&block, 80).output {
//!     MermaidOutput::Text(lines) => assert!(!lines.is_empty()),
//!     other => panic!("unexpected {other:?}"),
//! }
//! ```

pub mod ast;
pub mod detect;
mod hash;
pub mod image;
pub mod mmdc;
pub mod parser;
pub mod select;
pub mod terminal;

pub use ast::{
    ArrowKind, Diagram, DiagramEdge, DiagramNode, EdgeStyle, NodeShape, Orientation, Subgraph,
};
pub use detect::{diagram_kind, DiagramKind};
pub use image::{ImageData, ImageDecodeError};
pub use mmdc::{cache_key, find_executable, MmdcError, MmdcRunner};
pub use parser::{parse, MermaidParseError};
pub use select::{
    decide, select_backend, AutoRenderer, ImageCapability, MermaidOutput, MermaidRender,
    MermaidRenderer, MmdcRenderer, RenderEnvironment, Situation, SourceRenderer, Strategy,
    TerminalRenderer, UNRENDERABLE_MARKER,
};
pub use terminal::{NativeError, RenderOptions};
