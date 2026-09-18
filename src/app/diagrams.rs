//! Diagram provider: adapts a [`MermaidRenderer`] to the layout engine's
//! [`DiagramSource`] trait, caches results and keeps the image registry.
//!
//! Responsibilities:
//!
//! * translate [`MermaidOutput`] into [`DiagramContent`],
//! * cache per `(node, width)` so scrolling and re-layout never re-invoke
//!   `mmdc`,
//! * keep the encoded image payloads in a registry keyed by the opaque
//!   [`crate::render::primitives::ImageRef::id`] so the renderer can fetch
//!   them at draw time,
//! * collect non-fatal warnings; a diagram that cannot be rendered shows
//!   [`UNRENDERABLE_MARKER`] and never blocks the document,
//! * remember which diagrams the user switched to source view (`s`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::document::markdown::MermaidBlock;
use crate::document::NodeId;
use crate::layout::{DiagramContent, DiagramSource};
use crate::mermaid::{MermaidOutput, MermaidRenderer, UNRENDERABLE_MARKER};
use crate::terminal::protocols::ImageSource;

/// Image ids handed to the layout engine start here so they can never collide
/// with the `NodeId`-based ids that plain Markdown images use.
const IMAGE_ID_BASE: usize = 1 << 32;

/// A registered diagram image, ready to be encoded at draw time.
#[derive(Debug, Clone)]
pub(crate) struct DiagramImage {
    /// Encoded payload (PNG bytes from `mmdc`).
    pub(crate) source: ImageSource,
    /// Reserved width in columns.
    pub(crate) cols: u16,
    /// Reserved height in rows.
    pub(crate) rows: u16,
}

#[derive(Default)]
struct Inner {
    cache: HashMap<(NodeId, usize), DiagramContent>,
    images: HashMap<usize, DiagramImage>,
    force_source: HashSet<NodeId>,
    warnings: Vec<String>,
    seen_warnings: HashSet<String>,
    next_id: usize,
}

/// Diagram provider used by [`crate::app::App`].
pub struct DiagramProvider {
    renderer: Box<dyn MermaidRenderer>,
    inner: RefCell<Inner>,
}

impl std::fmt::Debug for DiagramProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.borrow();
        f.debug_struct("DiagramProvider")
            .field("cached", &inner.cache.len())
            .field("images", &inner.images.len())
            .field("source_view", &inner.force_source.len())
            .finish()
    }
}

impl DiagramProvider {
    /// Wrap a selected Mermaid backend.
    pub fn new(renderer: Box<dyn MermaidRenderer>) -> Self {
        Self {
            renderer,
            inner: RefCell::new(Inner::default()),
        }
    }

    /// A provider that always shows the diagram source (no Mermaid work).
    pub fn source_only() -> Self {
        Self::new(Box::new(crate::mermaid::SourceRenderer))
    }

    /// Whether `node` is currently displayed as Mermaid source.
    #[cfg(test)]
    pub(crate) fn shows_source(&self, node: NodeId) -> bool {
        self.inner.borrow().force_source.contains(&node)
    }

    /// Toggle between the rendered diagram and its source for `node`.
    ///
    /// Returns the new state (`true` = source shown). The cached renders for
    /// the node are dropped so the next layout picks up the change.
    pub(crate) fn toggle_source(&self, node: NodeId) -> bool {
        let mut inner = self.inner.borrow_mut();
        let now_source = if inner.force_source.contains(&node) {
            inner.force_source.remove(&node);
            false
        } else {
            inner.force_source.insert(node);
            true
        };
        inner.cache.retain(|(n, _), _| *n != node);
        now_source
    }

    /// Look up a registered image by the id carried in an `ImageRef`.
    pub(crate) fn image(&self, id: usize) -> Option<DiagramImage> {
        self.inner.borrow().images.get(&id).cloned()
    }

    /// Take the pending non-fatal warnings (each distinct message is only
    /// ever reported once).
    pub(crate) fn take_warnings(&self) -> Vec<String> {
        std::mem::take(&mut self.inner.borrow_mut().warnings)
    }

    /// Number of cached `(node, width)` renders — used by tests to prove that
    /// scrolling and re-layout do not re-invoke the backend.
    #[cfg(test)]
    pub(crate) fn cache_len(&self) -> usize {
        self.inner.borrow().cache.len()
    }

    fn convert(&self, inner: &mut Inner, source: &str, width: usize) -> DiagramContent {
        let block = MermaidBlock {
            source: source.to_string(),
        };
        let render = self.renderer.render(&block, width.max(1));
        // Record the warning first, then still dispatch on the output: the
        // Mermaid backend returns `MermaidOutput::Source(..)` *together* with
        // the warning, and throwing that away leaves the diagram unreachable
        // on the non-interactive path, where there is no `s` key.
        let note = render.warning.map(|warning| {
            let message = format!("{UNRENDERABLE_MARKER} {warning}");
            if inner.seen_warnings.insert(message.clone()) {
                inner.warnings.push(message.clone());
            }
            message
        });
        match render.output {
            MermaidOutput::Text(lines) => DiagramContent::Lines(lines),
            MermaidOutput::Source(_) => match note {
                // Show the marker *and* the source below it.
                Some(note) => DiagramContent::SourceWithNote(note),
                None => DiagramContent::Source,
            },
            MermaidOutput::Image(data, (cols, rows)) => {
                let id = IMAGE_ID_BASE + inner.next_id;
                inner.next_id += 1;
                inner.images.insert(
                    id,
                    DiagramImage {
                        source: ImageSource::Png(data.png),
                        cols: cols.max(1),
                        rows: rows.max(1),
                    },
                );
                DiagramContent::Image {
                    id,
                    cols: cols.max(1),
                    rows: rows.max(1),
                }
            }
        }
    }
}

impl DiagramSource for DiagramProvider {
    fn diagram(&self, node: NodeId, source: &str, width: usize) -> DiagramContent {
        let mut inner = self.inner.borrow_mut();
        if inner.force_source.contains(&node) {
            return DiagramContent::Source;
        }
        if let Some(hit) = inner.cache.get(&(node, width)) {
            return hit.clone();
        }
        let content = self.convert(&mut inner, source, width);
        inner.cache.insert((node, width), content.clone());
        content
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mermaid::{ImageData, MermaidRender};
    use std::cell::Cell;

    struct Counting {
        calls: Cell<usize>,
    }

    impl MermaidRenderer for Counting {
        fn render(&self, _block: &MermaidBlock, _width: usize) -> MermaidRender {
            self.calls.set(self.calls.get() + 1);
            MermaidRender::ok(MermaidOutput::Text(vec!["A --> B".to_string()]))
        }
    }

    struct Failing;

    impl MermaidRenderer for Failing {
        fn render(&self, block: &MermaidBlock, _width: usize) -> MermaidRender {
            MermaidRender::fallback(&block.source, "unsupported diagram kind")
        }
    }

    struct Imaging;

    impl MermaidRenderer for Imaging {
        fn render(&self, _block: &MermaidBlock, _width: usize) -> MermaidRender {
            let png = vec![1u8, 2, 3];
            MermaidRender::ok(MermaidOutput::Image(
                ImageData {
                    png,
                    width_px: 40,
                    height_px: 40,
                },
                (10, 5),
            ))
        }
    }

    #[test]
    fn results_are_cached_per_node_and_width() {
        let counting = Counting {
            calls: Cell::new(0),
        };
        let p = DiagramProvider::new(Box::new(counting));
        for _ in 0..5 {
            let _ = p.diagram(3, "graph LR\nA-->B", 80);
        }
        assert_eq!(p.cache_len(), 1);
        let _ = p.diagram(3, "graph LR\nA-->B", 40);
        assert_eq!(p.cache_len(), 2);
    }

    #[test]
    fn a_warning_shows_the_marker_once() {
        let p = DiagramProvider::new(Box::new(Failing));
        let out = p.diagram(1, "sequenceDiagram\nA->>B: hi", 80);
        // The marker is kept, but the source the backend returned must be
        // rendered with it instead of being discarded.
        let DiagramContent::SourceWithNote(note) = &out else {
            panic!("expected marker + source, got {out:?}");
        };
        assert!(note.starts_with(UNRENDERABLE_MARKER), "{note}");
        assert!(note.contains("unsupported diagram kind"), "{note}");
        let warnings = p.take_warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains(UNRENDERABLE_MARKER));
        // Same diagram again: cached, and the warning is not repeated.
        let _ = p.diagram(1, "sequenceDiagram\nA->>B: hi", 80);
        assert!(p.take_warnings().is_empty());
    }

    #[test]
    fn images_are_registered_and_ids_never_collide_with_node_ids() {
        let p = DiagramProvider::new(Box::new(Imaging));
        let out = p.diagram(2, "graph LR\nA-->B", 80);
        let DiagramContent::Image { id, cols, rows } = out else {
            panic!("expected an image, got {out:?}");
        };
        assert!(id >= IMAGE_ID_BASE);
        assert_eq!((cols, rows), (10, 5));
        let img = p.image(id).expect("registered");
        assert_eq!(img.cols, 10);
        assert_eq!(img.source, ImageSource::Png(vec![1, 2, 3]));
        assert!(p.image(2).is_none(), "node ids are not image ids");
    }

    #[test]
    fn toggling_source_invalidates_the_cache() {
        let p = DiagramProvider::new(Box::new(Counting {
            calls: Cell::new(0),
        }));
        let _ = p.diagram(4, "graph LR\nA-->B", 80);
        assert_eq!(p.cache_len(), 1);
        assert!(p.toggle_source(4));
        assert_eq!(p.cache_len(), 0);
        assert_eq!(p.diagram(4, "graph LR\nA-->B", 80), DiagramContent::Source);
        assert!(!p.toggle_source(4));
        assert!(matches!(
            p.diagram(4, "graph LR\nA-->B", 80),
            DiagramContent::Lines(_)
        ));
    }
}
