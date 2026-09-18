//! Backend selection implementing the Mermaid fallback matrix.
//!
//! | condition | result |
//! |---|---|
//! | native renderer supports the diagram | native terminal rendering |
//! | native unsupported, `mmdc` available, image protocol supported | `mmdc` + terminal image |
//! | native unsupported, `mmdc` available, no image protocol | source rendering + external-open action |
//! | `mmdc` unavailable | source rendering |
//! | render process fails | source rendering + non-fatal warning |
//!
//! The first four rows are the pure decision table in [`decide`]; the last row
//! is applied at render time by [`AutoRenderer`] (and by every explicit
//! backend), because a failure can only be observed while rendering.
//!
//! `[mermaid] backend` and `[mermaid] images` are explicit overrides that
//! bypass the automatic decision — see [`select_backend`].

use std::time::Duration;

use crate::config::schema::{ImageMode, MermaidBackend, MermaidConfig};
use crate::document::markdown::ast::MermaidBlock;

use super::detect::diagram_kind;
use super::image::ImageData;
use super::mmdc::MmdcRunner;
use super::parser::parse;
use super::terminal::{self, RenderOptions};

/// Marker the app shows for a diagram that could not be rendered.
pub const UNRENDERABLE_MARKER: &str = "[Mermaid diagram could not be rendered]";

/// Terminal image protocol available to the caller.
///
/// This mirrors `terminal::capabilities::ImageSupport` without depending on it:
/// the Mermaid module must stay independent of `terminal`, `layout` and
/// `render`. The integrator maps `terminal::Capabilities` onto
/// [`RenderEnvironment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageCapability {
    /// No inline image protocol.
    #[default]
    None,
    /// Kitty graphics protocol.
    Kitty,
    /// DEC Sixel.
    Sixel,
    /// iTerm2 inline images (OSC 1337).
    Iterm2,
}

impl ImageCapability {
    /// `true` for anything but [`ImageCapability::None`].
    #[must_use]
    pub const fn is_supported(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Everything the Mermaid module needs to know about the render target.
///
/// The integrator builds this from `terminal::capabilities::Capabilities`
/// (Workstream A) plus the current viewport width, e.g.
///
/// ```ignore
/// RenderEnvironment {
///     images: match caps.images { ImageSupport::Kitty => ImageCapability::Kitty, .. },
///     mmdc_available: mermaid::mmdc::find_executable(&cfg.mermaid.mmdc_command).is_some(),
///     unicode_box: caps.unicode_box,
///     width_cells: viewport_width,
///     cell_pixels: (8, 16),
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderEnvironment {
    /// Inline image protocol supported by the terminal.
    pub images: ImageCapability,
    /// Whether the configured `mmdc` command was found.
    pub mmdc_available: bool,
    /// Whether box-drawing characters may be used (`false` ⇒ ASCII fallback).
    pub unicode_box: bool,
    /// Available width in terminal columns.
    pub width_cells: usize,
    /// Pixel size of one character cell, `(width, height)`.
    pub cell_pixels: (u16, u16),
}

impl Default for RenderEnvironment {
    fn default() -> Self {
        Self {
            images: ImageCapability::None,
            mmdc_available: false,
            unicode_box: true,
            width_cells: 80,
            cell_pixels: (8, 16),
        }
    }
}

/// What a backend produced for one Mermaid block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MermaidOutput {
    /// Natively rendered diagram lines, all padded to the same width.
    Text(Vec<String>),
    /// A rendered image plus the `(cols, rows)` cell box it should occupy.
    /// Encoding into a terminal protocol is Workstream A's job.
    Image(ImageData, (u16, u16)),
    /// The diagram source, to be shown verbatim.
    Source(Vec<String>),
}

/// A backend result: the output plus a non-fatal warning channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MermaidRender {
    /// The rendered output.
    pub output: MermaidOutput,
    /// Non-fatal diagnostic. When `Some`, the app shows
    /// [`UNRENDERABLE_MARKER`] together with this message; it never aborts the
    /// surrounding document.
    pub warning: Option<String>,
    /// `true` for the matrix row "no image protocol": the app may offer an
    /// external-open action for the diagram.
    pub external_open: bool,
}

impl MermaidRender {
    /// A plain result without a warning.
    #[must_use]
    pub fn ok(output: MermaidOutput) -> Self {
        Self {
            output,
            warning: None,
            external_open: false,
        }
    }

    /// Source fallback carrying a non-fatal warning.
    #[must_use]
    pub fn fallback(source: &str, warning: impl Into<String>) -> Self {
        Self {
            output: MermaidOutput::Source(source_lines(source)),
            warning: Some(warning.into()),
            external_open: false,
        }
    }
}

/// Splits a diagram source into display lines (trailing whitespace removed).
#[must_use]
pub fn source_lines(source: &str) -> Vec<String> {
    source
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>()
}

/// Renders a Mermaid block.
pub trait MermaidRenderer {
    /// Renders `block` for a viewport `width` columns wide.
    ///
    /// Implementations never panic and never fail: an unrenderable diagram
    /// yields [`MermaidOutput::Source`] plus a warning.
    fn render(&self, block: &MermaidBlock, width: usize) -> MermaidRender;
}

// ---------------------------------------------------------------------------
// The decision table
// ---------------------------------------------------------------------------

/// Inputs of the decision table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Situation {
    /// The native renderer can draw this diagram.
    pub native_supported: bool,
    /// `mmdc` is available.
    pub mmdc_available: bool,
    /// A terminal image protocol may be used.
    pub image_protocol: bool,
}

/// Result of the decision table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// Native terminal rendering.
    Native,
    /// `mmdc` render shown as a terminal image.
    MmdcImage,
    /// Source rendering, plus an optional external-open action.
    SourceWithExternalOpen,
    /// Plain source rendering.
    Source,
}

/// The fallback matrix as a pure function.
///
/// Row 5 ("render process fails ⇒ source rendering + non-fatal warning") is not
/// representable here: it is applied by the renderers when a strategy fails.
#[must_use]
pub const fn decide(s: Situation) -> Strategy {
    if s.native_supported {
        Strategy::Native
    } else if s.mmdc_available {
        if s.image_protocol {
            Strategy::MmdcImage
        } else {
            Strategy::SourceWithExternalOpen
        }
    } else {
        Strategy::Source
    }
}

// ---------------------------------------------------------------------------
// Backends
// ---------------------------------------------------------------------------

/// Shows the diagram source verbatim (`backend = "source"`).
#[derive(Debug, Default, Clone)]
pub struct SourceRenderer;

impl MermaidRenderer for SourceRenderer {
    fn render(&self, block: &MermaidBlock, _width: usize) -> MermaidRender {
        MermaidRender::ok(MermaidOutput::Source(source_lines(&block.source)))
    }
}

/// Native terminal rendering (`backend = "terminal"`).
#[derive(Debug, Clone)]
pub struct TerminalRenderer {
    unicode_box: bool,
    max_label_width: usize,
}

impl TerminalRenderer {
    /// Creates a native renderer for the given environment.
    #[must_use]
    pub fn new(env: &RenderEnvironment) -> Self {
        Self {
            unicode_box: env.unicode_box,
            max_label_width: 32,
        }
    }

    fn try_render(&self, source: &str, width: usize) -> Result<Vec<String>, String> {
        let diagram = parse(source).map_err(|e| e.to_string())?;
        let opts = RenderOptions {
            width_cells: width,
            unicode_box: self.unicode_box,
            max_label_width: self.max_label_width,
        };
        terminal::render(&diagram, &opts).map_err(|e| e.to_string())
    }
}

impl MermaidRenderer for TerminalRenderer {
    fn render(&self, block: &MermaidBlock, width: usize) -> MermaidRender {
        match self.try_render(&block.source, width) {
            Ok(lines) => MermaidRender::ok(MermaidOutput::Text(lines)),
            Err(message) => MermaidRender::fallback(&block.source, message),
        }
    }
}

/// `mmdc` rendering shown as a terminal image (`backend = "mmdc"`).
#[derive(Debug)]
pub struct MmdcRenderer {
    runner: MmdcRunner,
    cell_pixels: (u16, u16),
    /// `false` when `images = "never"` or no protocol is available.
    images_allowed: bool,
}

impl MmdcRenderer {
    /// Creates an `mmdc` backend.
    #[must_use]
    pub fn new(cfg: &MermaidConfig, env: &RenderEnvironment, images_allowed: bool) -> Self {
        Self {
            runner: MmdcRunner::new(cfg.mmdc_command.clone()),
            cell_pixels: env.cell_pixels,
            images_allowed,
        }
    }

    /// Overrides the subprocess timeout (used by tests).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.runner = self.runner.with_timeout(timeout);
        self
    }

    fn render_image(&self, source: &str, width: usize) -> Result<MermaidRender, String> {
        let cell_w = if self.cell_pixels.0 == 0 {
            8
        } else {
            self.cell_pixels.0
        };
        let width_px = u32::try_from(width.max(1))
            .unwrap_or(80)
            .saturating_mul(u32::from(cell_w))
            .clamp(200, 4000);
        let data = self
            .runner
            .render(source, width_px)
            .map_err(|e| e.to_string())?;
        let cells = data.cell_size(self.cell_pixels, width);
        Ok(MermaidRender::ok(MermaidOutput::Image(data, cells)))
    }
}

impl MermaidRenderer for MmdcRenderer {
    fn render(&self, block: &MermaidBlock, width: usize) -> MermaidRender {
        if !self.images_allowed {
            // Row 3 of the matrix: `mmdc` is there but the image cannot be shown.
            let mut r = MermaidRender::fallback(
                &block.source,
                "no terminal image protocol available for the mmdc-rendered diagram",
            );
            r.external_open = true;
            return r;
        }
        match self.render_image(&block.source, width) {
            Ok(render) => render,
            Err(message) => MermaidRender::fallback(&block.source, message),
        }
    }
}

/// Automatic backend (`backend = "auto"`): applies [`decide`] per diagram and
/// falls back to source rendering with a non-fatal warning whenever a step
/// fails (row 5 of the matrix).
#[derive(Debug)]
pub struct AutoRenderer {
    native: TerminalRenderer,
    mmdc: MmdcRenderer,
    mmdc_available: bool,
    image_protocol: bool,
}

impl AutoRenderer {
    /// Creates the automatic backend.
    #[must_use]
    pub fn new(cfg: &MermaidConfig, env: &RenderEnvironment) -> Self {
        let image_protocol = image_protocol_allowed(cfg, env);
        Self {
            native: TerminalRenderer::new(env),
            mmdc: MmdcRenderer::new(cfg, env, image_protocol),
            mmdc_available: env.mmdc_available,
            image_protocol,
        }
    }

    /// The strategy this backend would pick for `source`.
    #[must_use]
    pub fn strategy_for(&self, source: &str) -> Strategy {
        decide(Situation {
            native_supported: diagram_kind(source).natively_supported(),
            mmdc_available: self.mmdc_available,
            image_protocol: self.image_protocol,
        })
    }
}

impl MermaidRenderer for AutoRenderer {
    fn render(&self, block: &MermaidBlock, width: usize) -> MermaidRender {
        match self.strategy_for(&block.source) {
            Strategy::Native => {
                let r = self.native.render(block, width);
                if r.warning.is_none() {
                    return r;
                }
                // Row 5: the native attempt failed. Try `mmdc` if it can show
                // something, otherwise keep the source fallback + warning.
                if self.mmdc_available && self.image_protocol {
                    let m = self.mmdc.render(block, width);
                    if m.warning.is_none() {
                        return m;
                    }
                }
                let mut r = r;
                r.external_open = self.mmdc_available && !self.image_protocol;
                r
            }
            Strategy::MmdcImage => self.mmdc.render(block, width),
            Strategy::SourceWithExternalOpen => {
                let mut r = MermaidRender::fallback(
                    &block.source,
                    "this diagram type needs mmdc, but the terminal has no image protocol",
                );
                r.external_open = true;
                r
            }
            Strategy::Source => MermaidRender::fallback(
                &block.source,
                "unsupported diagram type and mmdc is not available",
            ),
        }
    }
}

/// Resolves `[mermaid] images` against the detected protocol.
fn image_protocol_allowed(cfg: &MermaidConfig, env: &RenderEnvironment) -> bool {
    match cfg.images {
        ImageMode::Never => false,
        ImageMode::Always => true,
        ImageMode::Auto => env.images.is_supported(),
    }
}

/// Picks the Mermaid backend for `cfg` and `env`.
///
/// `backend` and `images` are explicit overrides:
///
/// * `backend = "terminal"` — always native; failures fall back to source.
/// * `backend = "mmdc"` — always `mmdc`; with `images = "never"` (or no
///   protocol under `images = "auto"`) this degrades to source rendering plus
///   an external-open hint.
/// * `backend = "source"` — always the raw source, no warning.
/// * `backend = "auto"` — the fallback matrix, with `images` forcing the
///   "image protocol supported" column on (`always`) or off (`never`).
#[must_use]
pub fn select_backend(cfg: &MermaidConfig, env: &RenderEnvironment) -> Box<dyn MermaidRenderer> {
    match cfg.backend {
        MermaidBackend::Source => Box::new(SourceRenderer),
        MermaidBackend::Terminal => Box::new(TerminalRenderer::new(env)),
        MermaidBackend::Mmdc => Box::new(MmdcRenderer::new(
            cfg,
            env,
            image_protocol_allowed(cfg, env),
        )),
        MermaidBackend::Auto => Box::new(AutoRenderer::new(cfg, env)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOWCHART: &str = "graph LR\n    A --> B\n    B --> C\n";
    const SEQUENCE: &str = "sequenceDiagram\n    A->>B: hi\n";

    fn block(src: &str) -> MermaidBlock {
        MermaidBlock {
            source: src.to_string(),
        }
    }

    /// Named `render_env`, not `env`: a bare `env` shadows `std::env`, and the
    /// crate already had two unrelated helpers by that name.
    fn render_env(images: ImageCapability, mmdc: bool) -> RenderEnvironment {
        RenderEnvironment {
            images,
            mmdc_available: mmdc,
            ..RenderEnvironment::default()
        }
    }

    // --- fallback matrix, one test per row --------------------------------

    #[test]
    fn row1_native_supported() {
        assert_eq!(
            decide(Situation {
                native_supported: true,
                mmdc_available: false,
                image_protocol: false
            }),
            Strategy::Native
        );
        // `mmdc`/image availability must not change row 1.
        for mmdc in [false, true] {
            for img in [false, true] {
                assert_eq!(
                    decide(Situation {
                        native_supported: true,
                        mmdc_available: mmdc,
                        image_protocol: img
                    }),
                    Strategy::Native
                );
            }
        }
    }

    #[test]
    fn row2_mmdc_with_image_protocol() {
        assert_eq!(
            decide(Situation {
                native_supported: false,
                mmdc_available: true,
                image_protocol: true
            }),
            Strategy::MmdcImage
        );
    }

    #[test]
    fn row3_mmdc_without_image_protocol() {
        assert_eq!(
            decide(Situation {
                native_supported: false,
                mmdc_available: true,
                image_protocol: false
            }),
            Strategy::SourceWithExternalOpen
        );
    }

    #[test]
    fn row4_mmdc_unavailable() {
        for img in [false, true] {
            assert_eq!(
                decide(Situation {
                    native_supported: false,
                    mmdc_available: false,
                    image_protocol: img
                }),
                Strategy::Source
            );
        }
    }

    #[test]
    fn row5_render_failure_falls_back_with_warning() {
        // A flowchart that the native renderer cannot fit, with no mmdc.
        let cfg = MermaidConfig::default();
        let e = RenderEnvironment {
            width_cells: 8,
            ..render_env(ImageCapability::None, false)
        };
        let r = select_backend(&cfg, &e).render(&block(FLOWCHART), 8);
        assert!(matches!(r.output, MermaidOutput::Source(_)));
        assert!(r.warning.is_some(), "expected a non-fatal warning");
    }

    // --- End-to-end selection ----------------------------------------------

    #[test]
    fn auto_renders_flowchart_natively() {
        let cfg = MermaidConfig::default();
        let r = select_backend(&cfg, &render_env(ImageCapability::None, false))
            .render(&block(FLOWCHART), 80);
        match r.output {
            MermaidOutput::Text(lines) => assert!(lines.iter().any(|l| l.contains('A'))),
            other => panic!("expected native text, got {other:?}"),
        }
        assert!(r.warning.is_none());
    }

    #[test]
    fn auto_without_mmdc_falls_back_to_source() {
        let cfg = MermaidConfig::default();
        let r = select_backend(&cfg, &render_env(ImageCapability::Kitty, false))
            .render(&block(SEQUENCE), 80);
        assert_eq!(r.output, MermaidOutput::Source(source_lines(SEQUENCE)));
        assert!(r.warning.is_some());
        assert!(!r.external_open);
    }

    #[test]
    fn auto_with_mmdc_but_no_protocol_offers_external_open() {
        let cfg = MermaidConfig::default();
        let r = select_backend(&cfg, &render_env(ImageCapability::None, true))
            .render(&block(SEQUENCE), 80);
        assert!(matches!(r.output, MermaidOutput::Source(_)));
        assert!(r.external_open);
        assert!(r.warning.is_some());
    }

    #[test]
    fn auto_strategy_for_each_diagram_kind() {
        let cfg = MermaidConfig::default();
        let auto = AutoRenderer::new(&cfg, &render_env(ImageCapability::Kitty, true));
        assert_eq!(auto.strategy_for(FLOWCHART), Strategy::Native);
        assert_eq!(auto.strategy_for(SEQUENCE), Strategy::MmdcImage);
        assert_eq!(auto.strategy_for("gantt\ntitle x"), Strategy::MmdcImage);
    }

    // --- Overrides ----------------------------------------------------------

    #[test]
    fn backend_source_override() {
        let cfg = MermaidConfig {
            backend: MermaidBackend::Source,
            ..MermaidConfig::default()
        };
        let r = select_backend(&cfg, &render_env(ImageCapability::Kitty, true))
            .render(&block(FLOWCHART), 80);
        assert_eq!(r.output, MermaidOutput::Source(source_lines(FLOWCHART)));
        assert!(r.warning.is_none(), "an explicit choice is not a failure");
    }

    #[test]
    fn backend_terminal_override_ignores_mmdc() {
        let cfg = MermaidConfig {
            backend: MermaidBackend::Terminal,
            ..MermaidConfig::default()
        };
        let e = render_env(ImageCapability::Kitty, true);
        // A non-flowchart cannot be drawn natively → source + warning, never mmdc.
        let r = select_backend(&cfg, &e).render(&block(SEQUENCE), 80);
        assert!(matches!(r.output, MermaidOutput::Source(_)));
        assert!(r.warning.is_some());
        // A flowchart is drawn natively.
        let r = select_backend(&cfg, &e).render(&block(FLOWCHART), 80);
        assert!(matches!(r.output, MermaidOutput::Text(_)));
    }

    #[test]
    fn backend_mmdc_override_without_protocol_degrades() {
        let cfg = MermaidConfig {
            backend: MermaidBackend::Mmdc,
            ..MermaidConfig::default()
        };
        let r = select_backend(&cfg, &render_env(ImageCapability::None, true))
            .render(&block(FLOWCHART), 80);
        assert!(matches!(r.output, MermaidOutput::Source(_)));
        assert!(r.external_open);
    }

    #[test]
    fn images_never_disables_image_output() {
        let cfg = MermaidConfig {
            images: ImageMode::Never,
            ..MermaidConfig::default()
        };
        let auto = AutoRenderer::new(&cfg, &render_env(ImageCapability::Kitty, true));
        assert_eq!(
            auto.strategy_for(SEQUENCE),
            Strategy::SourceWithExternalOpen
        );
    }

    #[test]
    fn images_always_forces_the_image_column() {
        let cfg = MermaidConfig {
            images: ImageMode::Always,
            ..MermaidConfig::default()
        };
        let auto = AutoRenderer::new(&cfg, &render_env(ImageCapability::None, true));
        assert_eq!(auto.strategy_for(SEQUENCE), Strategy::MmdcImage);
    }

    #[test]
    fn mmdc_backend_never_panics_when_binary_is_missing() {
        let cfg = MermaidConfig {
            backend: MermaidBackend::Mmdc,
            mmdc_command: "definitely-not-a-real-binary-xyz".to_string(),
            ..MermaidConfig::default()
        };
        let r = select_backend(&cfg, &render_env(ImageCapability::Kitty, true))
            .render(&block(SEQUENCE), 80);
        assert!(matches!(r.output, MermaidOutput::Source(_)));
        assert!(r.warning.is_some());
    }

    #[test]
    fn ascii_environment_produces_ascii_output() {
        let cfg = MermaidConfig::default();
        let e = RenderEnvironment {
            unicode_box: false,
            ..render_env(ImageCapability::None, false)
        };
        let r = select_backend(&cfg, &e).render(&block(FLOWCHART), 80);
        match r.output {
            MermaidOutput::Text(lines) => {
                assert!(lines.iter().all(|l| l.is_ascii()), "{lines:?}");
            }
            other => panic!("expected text, got {other:?}"),
        }
    }

    /// Exercises the real `mmdc` binary through the backend selector.
    ///
    /// Ignored by default. Run it with:
    ///
    /// ```text
    /// DIPLE_TEST_MMDC=1 cargo test --lib mermaid::select::tests::real_mmdc_produces_an_image -- --ignored
    /// ```
    ///
    /// With the opt-in set, a missing or broken `mmdc` fails the test. In
    /// particular `MermaidOutput::Source` — the selector's fallback when the
    /// CLI errors out — is a failure here, not a skip.
    #[test]
    #[ignore = "requires a working `mmdc`; set DIPLE_TEST_MMDC=1 and pass --ignored"]
    fn real_mmdc_produces_an_image() {
        assert_eq!(
            std::env::var("DIPLE_TEST_MMDC").ok().as_deref(),
            Some("1"),
            "set DIPLE_TEST_MMDC=1 to run this test"
        );
        assert!(
            super::super::mmdc::find_executable("mmdc").is_some(),
            "`mmdc` is not installed"
        );
        let cfg = MermaidConfig {
            backend: MermaidBackend::Mmdc,
            ..MermaidConfig::default()
        };
        let renderer = select_backend(&cfg, &render_env(ImageCapability::Kitty, true));
        let r = renderer.render(&block(SEQUENCE), 80);
        match r.output {
            MermaidOutput::Image(data, (cols, rows)) => {
                assert!(cols > 0 && rows > 0);
                assert!(!data.png.is_empty());
            }
            MermaidOutput::Source(_) => {
                panic!("mmdc is present but not usable: {:?}", r.warning);
            }
            other => panic!("unexpected output {other:?}"),
        }
    }
}
