//! Small shared helpers that do not belong to a specific layer.
//!
//! * [`text`] — making document-controlled text safe to put on a terminal,
//! * [`unicode`] — grapheme- and width-correct string measurement and slicing
//!   (used by `layout`, `render` and `mermaid` alike),
//! * [`viewport`] — vertical windowing and horizontal slicing of a render
//!   tree (used by `app` and `render`).

pub mod text;
pub mod unicode;
pub mod viewport;

/// The character-cell size in pixels assumed when the terminal does not report
/// one, in `(width, height)`.
///
/// A conventional 8x16 cell. This is the single definition: the capability
/// probe ([`crate::terminal::capabilities::Capabilities::cell_size`]), the
/// image protocols (`terminal::protocols::cell_pixels`) and the
/// diagram image geometry ([`crate::mermaid::image::ImageData::cell_size`])
/// all fall back to it, and they must agree.
pub const DEFAULT_CELL_PIXELS: (u16, u16) = (8, 16);
