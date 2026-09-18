//! The document layer: what diple reads, in whatever format it comes in.
//!
//! # The shape of this module
//!
//! The layer is split into a **format-neutral vocabulary** that the rest of
//! the program speaks, and **backends** that each keep their own semantic
//! model:
//!
//! ```text
//!                       Input (file or stdin)
//!                               |
//!                         format detection
//!                               |
//!             +-----------------+-----------------+
//!             |                 |                 |
//!        markdown::          json::            yaml::
//!        Document        ---> StructuredDocument <---
//!             |                       |
//!             +---------- DocumentModel ----------+
//!                               |
//!               capabilities · folds · outline
//!               search · paths · navigation
//! ```
//!
//! Everything above [`DocumentModel`] — the pager, the layout engine, the
//! renderer, the CLI — asks what a document *can do* rather than assuming it
//! is Markdown. Everything below it keeps the vocabulary its format actually
//! has: Markdown has headings, footnotes, links and Mermaid fences; JSON and
//! YAML have mappings, sequences, scalars, anchors, tags and comments. The
//! shared structured model exists because JSON and YAML genuinely agree, not
//! because every format must.
//!
//! # What the vocabulary is
//!
//! * [`NodeId`] — a semantic unit, numbered densely in document order and
//!   independent of terminal geometry. A node may render to many terminal
//!   rows; those rows never become its identity.
//! * [`FoldState`] — which foldable units are collapsed, over a forest the
//!   backend describes. Markdown folds sections; structured data folds
//!   containers.
//! * [`SearchIndex`] / [`Match`] — literal full-text search over everything
//!   the reader can see, in reading order, with the field a hit landed in.
//! * [`OutlineEntry`] — one line of the outline: the table of contents for
//!   Markdown, the structure tree for JSON and YAML.
//! * [`DocumentPath`] — where a node sits, for the formats that have a path.
//! * [`DocumentCapabilities`] — what any of the above means for this format.
//!
//! Nothing in this module knows about widths, colours or terminals.

pub mod capabilities;
pub mod error;
pub mod folds;
pub mod format;
pub mod json;
pub mod load;
pub mod markdown;
pub mod model;
pub mod outline;
pub mod path;
pub mod search;
pub mod source;
pub mod structured;
pub mod yaml;

/// Identifier of a semantic node within a document.
///
/// Ids are dense (`0..node_count`), assigned in document order, and stable for
/// the lifetime of a loaded document. Every backend keeps that contract: it is
/// what lets the viewport anchor, the cursor, the fold state, the search
/// results and the rendered rows all name the same thing without knowing which
/// format produced it.
pub type NodeId = usize;

/// Identifier of a followable link within a document.
///
/// Part of the neutral vocabulary rather than of Markdown, because links are
/// a *capability* ([`DocumentCapabilities::links`]) that other formats have
/// too — a PDF's annotations, an HTML document's anchors — and the renderer
/// that draws a selected link should not have to know which format produced
/// it.
pub type LinkId = usize;

pub use capabilities::DocumentCapabilities;
pub use error::{DocumentError, Position};
pub use folds::{FoldId, FoldState};
pub use format::{DocumentKind, FormatRequest};
pub use load::{load, LoadedDocument};
pub use model::DocumentModel;
pub use outline::{Outline, OutlineEntry};
pub use path::{DocumentPath, PathSegment};
pub use search::{Match, MatchField, SearchIndex};
pub use source::{SourceDocument, SourceSpan};
