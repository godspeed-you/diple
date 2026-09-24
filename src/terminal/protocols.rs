//! Escape-sequence encoders: OSC 8 hyperlinks, Kitty graphics, iTerm2 inline
//! images, Sixel and tmux passthrough.
//!
//! Every function here is pure — it takes data and returns a `String` — so the
//! encoders are unit-testable without a terminal. Nothing in this module reads
//! the environment or writes to a stream; the caller decides *whether* to emit
//! based on [`Capabilities`](super::capabilities::Capabilities).

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

use super::capabilities::ImageSupport;

/// ESC.
const ESC: char = '\u{1b}';
/// String terminator (`ESC \`).
const ST: &str = "\u{1b}\\";
/// BEL, the alternative OSC terminator.
const BEL: char = '\u{7}';

/// Maximum base64 payload per Kitty transmission chunk (protocol limit).
pub(crate) const KITTY_CHUNK: usize = 4096;

// ---------------------------------------------------------------------------
// tmux passthrough
// ---------------------------------------------------------------------------

/// Wrap a sequence so tmux forwards it to the outer terminal.
///
/// tmux only does this when `set -g allow-passthrough on` is configured; the
/// wrapper is `ESC P tmux; … ESC \` with every embedded `ESC` doubled.
pub(crate) fn tmux_passthrough(sequence: &str) -> String {
    let mut out = String::with_capacity(sequence.len() + 16);
    out.push_str("\u{1b}Ptmux;");
    for ch in sequence.chars() {
        if ch == ESC {
            out.push(ESC);
        }
        out.push(ch);
    }
    out.push_str(ST);
    out
}

/// Apply [`tmux_passthrough`] only when `wrap` is true.
pub(crate) fn maybe_tmux(sequence: String, wrap: bool) -> String {
    if wrap {
        tmux_passthrough(&sequence)
    } else {
        sequence
    }
}

// ---------------------------------------------------------------------------
// OSC 8 hyperlinks
// ---------------------------------------------------------------------------

/// Start an OSC 8 hyperlink: `ESC ] 8 ; ; URL ESC \`.
///
/// The link text follows and must be closed with [`osc8_end`]. Control
/// characters are stripped from `url` so a malicious document cannot inject
/// escape sequences.
pub(crate) fn osc8_start(url: &str) -> String {
    format!("\u{1b}]8;;{}{ST}", sanitize_url(url))
}

/// Start an OSC 8 hyperlink carrying an `id=` parameter, which lets terminals
/// highlight all cells belonging to the same link.
#[allow(dead_code)]
pub(crate) fn osc8_start_with_id(url: &str, id: &str) -> String {
    format!(
        "\u{1b}]8;id={};{}{ST}",
        sanitize_param(id),
        sanitize_url(url)
    )
}

/// End an OSC 8 hyperlink.
pub(crate) fn osc8_end() -> String {
    format!("\u{1b}]8;;{ST}")
}

/// A complete hyperlink: start, text, end.
#[allow(dead_code)]
pub(crate) fn osc8_link(url: &str, text: &str) -> String {
    format!("{}{}{}", osc8_start(url), text, osc8_end())
}

/// Drop what must not reach the terminal inside an OSC 8 target: controls,
/// which could end the sequence, and bidirectional overrides, which could make
/// the target a terminal shows on hover read as a different address.
fn sanitize_url(url: &str) -> String {
    url.chars()
        .filter(|&c| crate::util::text::replacement(c).is_none() && c != ';')
        .collect()
}

#[allow(dead_code)]
fn sanitize_param(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() && *c != ';' && *c != ':')
        .collect()
}

// ---------------------------------------------------------------------------
// Image payloads
// ---------------------------------------------------------------------------

/// A raw 8-bit RGB image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RgbImage {
    /// Width in pixels.
    pub(crate) width: u32,
    /// Height in pixels.
    pub(crate) height: u32,
    /// `width * height * 3` bytes, row-major.
    pub(crate) pixels: Vec<u8>,
}

impl RgbImage {
    /// Create an image, returning [`None`] if `pixels` has the wrong length.
    pub(crate) fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<RgbImage> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(3)?;
        (pixels.len() == expected).then_some(RgbImage {
            width,
            height,
            pixels,
        })
    }

    /// The pixel at `(x, y)` as `(r, g, b)`.
    #[allow(dead_code)]
    pub(crate) fn pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y as usize) * (self.width as usize) + x as usize) * 3;
        Some((
            *self.pixels.get(i)?,
            *self.pixels.get(i + 1)?,
            *self.pixels.get(i + 2)?,
        ))
    }
}

/// Image data handed to an encoder: either PNG bytes or raw RGB.
///
/// Conversions between the two are performed lazily and only when the selected
/// protocol needs the other form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImageSource {
    /// Encoded PNG bytes (what `mmdc` produces).
    Png(Vec<u8>),
    /// Raw RGB pixels.
    #[allow(dead_code)]
    Rgb(RgbImage),
}

impl ImageSource {
    /// The PNG representation, encoding raw RGB when necessary.
    ///
    /// Returns [`None`] if encoding fails; callers then skip the image.
    pub(crate) fn to_png(&self) -> Option<Vec<u8>> {
        match self {
            ImageSource::Png(bytes) => Some(bytes.clone()),
            ImageSource::Rgb(img) => encode_png(img),
        }
    }

    /// The RGB representation, decoding PNG when necessary.
    pub(crate) fn to_rgb(&self) -> Option<RgbImage> {
        match self {
            ImageSource::Rgb(img) => Some(img.clone()),
            ImageSource::Png(bytes) => decode_png(bytes),
        }
    }

    /// Pixel dimensions, if they can be determined.
    #[allow(dead_code)]
    pub(crate) fn dimensions(&self) -> Option<(u32, u32)> {
        match self {
            ImageSource::Rgb(img) => Some((img.width, img.height)),
            ImageSource::Png(bytes) => png_dimensions(bytes),
        }
    }
}

fn encode_png(img: &RgbImage) -> Option<Vec<u8>> {
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&img.pixels, img.width, img.height, ExtendedColorType::Rgb8)
        .ok()?;
    Some(out)
}

fn decode_png(bytes: &[u8]) -> Option<RgbImage> {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgb = decoded.to_rgb8();
    let (width, height) = (rgb.width(), rgb.height());
    RgbImage::new(width, height, rgb.into_raw())
}

/// Read `width`/`height` straight from a PNG `IHDR` chunk.
#[allow(dead_code)]
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes.get(..8)? != SIGNATURE {
        return None;
    }
    if bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    Some((w, h))
}

// ---------------------------------------------------------------------------
// Cell geometry
// ---------------------------------------------------------------------------

/// The cell size assumed when the terminal does not report one.
///
/// Re-exported from [`crate::util`] so every fallback agrees.
pub(crate) use crate::util::DEFAULT_CELL_PIXELS;

/// Query the terminal's cell size in pixels, falling back to
/// [`DEFAULT_CELL_PIXELS`] when `window_size()` is unavailable or reports zero.
#[allow(dead_code)]
pub(crate) fn cell_pixels() -> (u16, u16) {
    match crossterm::terminal::window_size() {
        Ok(ws) if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 => {
            (ws.width / ws.columns, ws.height / ws.rows)
        }
        _ => DEFAULT_CELL_PIXELS,
    }
}

// ---------------------------------------------------------------------------
// Kitty graphics protocol
// ---------------------------------------------------------------------------

/// Encode PNG bytes as a Kitty graphics transmit-and-display command.
///
/// The payload is base64 and split into [`KITTY_CHUNK`]-byte chunks; every
/// chunk but the last carries `m=1`, the last `m=0`. The first chunk carries
/// the format (`f=100`, PNG), the action (`a=T`, transmit and display) and the
/// placement size in cells (`c=`/`r=`).
pub(crate) fn kitty_image(png: &[u8], cols: u16, rows: u16) -> String {
    let data = BASE64.encode(png);
    let mut out = String::with_capacity(data.len() + 64);
    let chunks: Vec<&str> = split_chunks(&data, KITTY_CHUNK);
    let last = chunks.len().saturating_sub(1);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i != last);
        out.push_str("\u{1b}_G");
        if i == 0 {
            out.push_str(&format!("f=100,a=T,c={cols},r={rows},m={more}"));
        } else {
            out.push_str(&format!("m={more}"));
        }
        out.push(';');
        out.push_str(chunk);
        out.push_str(ST);
    }
    out
}

/// Split a base64 string into chunks of at most `size` bytes.
///
/// base64 is ASCII, so byte slicing is always on a character boundary.
fn split_chunks(data: &str, size: usize) -> Vec<&str> {
    if data.is_empty() {
        return vec![""];
    }
    data.as_bytes()
        .chunks(size.max(1))
        .filter_map(|c| std::str::from_utf8(c).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// iTerm2 inline images
// ---------------------------------------------------------------------------

/// Encode PNG bytes as an iTerm2 inline image (OSC 1337 `File=`).
///
/// Width and height are given in cells (`Ncells`), which WezTerm and iTerm2
/// both understand.
pub(crate) fn iterm2_image(png: &[u8], cols: u16, rows: u16) -> String {
    format!(
        "\u{1b}]1337;File=inline=1;size={};width={}cells;height={}cells;preserveAspectRatio=1:{}{}",
        png.len(),
        cols,
        rows,
        BASE64.encode(png),
        BEL
    )
}

// ---------------------------------------------------------------------------
// Sixel
// ---------------------------------------------------------------------------

/// Sixel colour registers used by [`sixel_image`]: the fixed 6×6×6 cube.
pub(crate) const SIXEL_PALETTE_SIZE: usize = 216;

/// Encode an RGB image as a Sixel sequence.
///
/// # Palette choice
///
/// diple uses the **fixed 6×6×6 RGB cube** (216 colours, well inside the
/// 256-register limit every Sixel terminal provides) rather than an adaptive
/// median-cut palette. Reasons:
///
/// * it is O(pixels) with no extra pass and no allocation per image, which
///   keeps diagram rendering inside the interaction budget;
/// * it is deterministic, so the encoder output is golden-testable;
/// * diple only renders Mermaid diagrams — flat-coloured line art with few
///   distinct colours — where a uniform cube is visually indistinguishable
///   from an adaptive palette.
///
/// Only the registers actually used are emitted.
pub(crate) fn sixel_image(img: &RgbImage) -> String {
    let mut out = String::new();
    // DCS q, then raster attributes: pixel aspect 1:1 and the image size.
    out.push_str("\u{1b}Pq");
    out.push_str(&format!("\"1;1;{};{}", img.width, img.height));

    if img.width == 0 || img.height == 0 {
        out.push_str(ST);
        return out;
    }

    // Map every pixel to a cube index once.
    let mut indexed = vec![0u8; (img.width as usize) * (img.height as usize)];
    let mut used = [false; SIXEL_PALETTE_SIZE];
    for (i, px) in img.pixels.chunks_exact(3).enumerate() {
        let idx = cube_index(px[0], px[1], px[2]);
        if let Some(slot) = indexed.get_mut(i) {
            *slot = idx;
        }
        if let Some(flag) = used.get_mut(idx as usize) {
            *flag = true;
        }
    }

    // Palette definitions (`#n;2;r;g;b`, components in percent).
    for (idx, _) in used.iter().enumerate().filter(|(_, u)| **u) {
        let (r, g, b) = cube_color(idx as u8);
        out.push_str(&format!("#{idx};2;{r};{g};{b}"));
    }

    let width = img.width as usize;
    let height = img.height as usize;
    let bands = height.div_ceil(6);
    let mut row = vec![0u8; width];

    for band in 0..bands {
        let top = band * 6;
        // Which colours occur in this band?
        let mut band_used = [false; SIXEL_PALETTE_SIZE];
        for y in top..(top + 6).min(height) {
            for x in 0..width {
                if let Some(idx) = indexed.get(y * width + x) {
                    if let Some(flag) = band_used.get_mut(*idx as usize) {
                        *flag = true;
                    }
                }
            }
        }
        let colors: Vec<usize> = band_used
            .iter()
            .enumerate()
            .filter(|(_, u)| **u)
            .map(|(i, _)| i)
            .collect();

        for (n, color) in colors.iter().enumerate() {
            for slot in row.iter_mut() {
                *slot = 0;
            }
            for bit in 0..6usize {
                let y = top + bit;
                if y >= height {
                    break;
                }
                for x in 0..width {
                    if indexed.get(y * width + x).map(|v| *v as usize) == Some(*color) {
                        if let Some(slot) = row.get_mut(x) {
                            *slot |= 1 << bit;
                        }
                    }
                }
            }
            out.push_str(&format!("#{color}"));
            write_run_length(&mut out, &row);
            if n + 1 < colors.len() {
                out.push('$'); // graphics carriage return: overlay next colour
            }
        }
        if band + 1 < bands {
            out.push('-'); // graphics newline
        }
    }
    out.push_str(ST);
    out
}

/// Emit one sixel row with run-length compression (`!count char`).
fn write_run_length(out: &mut String, row: &[u8]) {
    let mut run_char = None::<char>;
    let mut run_len = 0usize;
    let flush = |out: &mut String, ch: Option<char>, len: usize| {
        if let Some(ch) = ch {
            if len >= 4 {
                out.push_str(&format!("!{len}"));
                out.push(ch);
            } else {
                for _ in 0..len {
                    out.push(ch);
                }
            }
        }
    };
    for byte in row {
        let ch = sixel_char(*byte);
        if Some(ch) == run_char {
            run_len += 1;
        } else {
            flush(out, run_char, run_len);
            run_char = Some(ch);
            run_len = 1;
        }
    }
    flush(out, run_char, run_len);
}

/// A six-pixel column bitmask as its sixel character (`?` … `~`).
fn sixel_char(bits: u8) -> char {
    char::from(0x3f + (bits & 0x3f))
}

/// Map an RGB triple onto the 6×6×6 colour cube.
fn cube_index(r: u8, g: u8, b: u8) -> u8 {
    let q = |v: u8| ((u16::from(v) * 6) / 256).min(5) as u8;
    q(r) * 36 + q(g) * 6 + q(b)
}

/// The cube colour for an index, as Sixel percentages (0..=100).
fn cube_color(index: u8) -> (u8, u8, u8) {
    let level = |v: u8| ((u16::from(v) * 255 / 5) * 100 / 255) as u8;
    let r = index / 36;
    let g = (index / 6) % 6;
    let b = index % 6;
    (level(r), level(g), level(b))
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

impl ImageSupport {
    /// Encode `source` for this protocol at the given cell footprint.
    ///
    /// Returns [`None`] for [`ImageSupport::None`] and whenever the image
    /// cannot be converted into the form the protocol needs — the caller then
    /// falls back to text.
    ///
    /// The result is *not* tmux-wrapped; callers inside tmux with passthrough
    /// enabled pass it through [`tmux_passthrough`].
    pub(crate) fn encode(&self, source: &ImageSource, cols: u16, rows: u16) -> Option<String> {
        match self {
            ImageSupport::None => None,
            ImageSupport::Kitty => Some(kitty_image(&source.to_png()?, cols, rows)),
            ImageSupport::Iterm2 => Some(iterm2_image(&source.to_png()?, cols, rows)),
            ImageSupport::Sixel => Some(sixel_image(&source.to_rgb()?)),
        }
    }

    /// [`ImageSupport::encode`] plus tmux passthrough wrapping when `tmux` is
    /// true.
    pub(crate) fn encode_for_tmux(
        &self,
        source: &ImageSource,
        cols: u16,
        rows: u16,
        tmux: bool,
    ) -> Option<String> {
        self.encode(source, cols, rows).map(|s| maybe_tmux(s, tmux))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgb: (u8, u8, u8)) -> RgbImage {
        let mut pixels = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..(width * height) {
            pixels.extend_from_slice(&[rgb.0, rgb.1, rgb.2]);
        }
        RgbImage::new(width, height, pixels).unwrap()
    }

    // -- OSC 8 ------------------------------------------------------------

    #[test]
    fn a_link_target_cannot_carry_controls_or_bidi_overrides() {
        let start = osc8_start("https://example.com/\u{202e}gpj.exe\u{1b}\\;x");
        assert!(!start.contains('\u{202e}'), "{start:?}");
        // The introducer and the terminator are the only escapes.
        assert_eq!(start.matches('\u{1b}').count(), 2, "{start:?}");
    }

    #[test]
    fn osc8_roundtrip() {
        assert_eq!(
            osc8_link("https://example.com/a", "text"),
            "\u{1b}]8;;https://example.com/a\u{1b}\\text\u{1b}]8;;\u{1b}\\"
        );
        assert_eq!(osc8_end(), "\u{1b}]8;;\u{1b}\\");
    }

    #[test]
    fn osc8_strips_injected_control_characters() {
        let s = osc8_start("https://x/\u{1b}]0;pwned\u{7};evil");
        assert!(!s.contains('\u{1b}'.to_string().as_str().repeat(2).as_str()));
        assert!(!s[2..].contains('\u{7}'));
        assert!(!s.contains("pwned;"));
    }

    #[test]
    fn osc8_with_id() {
        assert_eq!(
            osc8_start_with_id("https://x", "42"),
            "\u{1b}]8;id=42;https://x\u{1b}\\"
        );
    }

    // -- tmux -------------------------------------------------------------

    #[test]
    fn tmux_doubles_escapes_and_wraps() {
        let inner = "\u{1b}_Ga=T;AAA\u{1b}\\";
        let wrapped = tmux_passthrough(inner);
        assert!(wrapped.starts_with("\u{1b}Ptmux;"));
        assert!(wrapped.ends_with("\u{1b}\\"));
        // Every ESC of the payload is doubled: 2 in, 4 in the body.
        let body = &wrapped["\u{1b}Ptmux;".len()..wrapped.len() - 2];
        assert_eq!(body.matches('\u{1b}').count(), 4);
    }

    #[test]
    fn maybe_tmux_is_identity_when_disabled() {
        assert_eq!(maybe_tmux("x".to_string(), false), "x");
        assert!(maybe_tmux("x".to_string(), true).starts_with("\u{1b}Ptmux;"));
    }

    // -- Kitty ------------------------------------------------------------

    #[test]
    fn kitty_single_chunk_has_m0_and_placement() {
        let s = kitty_image(b"tiny", 10, 5);
        assert!(s.starts_with("\u{1b}_Gf=100,a=T,c=10,r=5,m=0;"));
        assert!(s.ends_with("\u{1b}\\"));
        let payload = s
            .trim_start_matches("\u{1b}_Gf=100,a=T,c=10,r=5,m=0;")
            .trim_end_matches("\u{1b}\\");
        assert_eq!(BASE64.decode(payload).unwrap(), b"tiny");
    }

    #[test]
    fn kitty_chunks_at_the_4096_boundary() {
        // 3072 raw bytes → exactly 4096 base64 characters → one chunk.
        let png = vec![0xABu8; 3072];
        let s = kitty_image(&png, 1, 1);
        assert_eq!(s.matches("\u{1b}_G").count(), 1);
        assert!(s.contains(",m=0;"));

        // One byte more → two chunks: m=1 then m=0.
        let png = vec![0xABu8; 3073];
        let s = kitty_image(&png, 4, 2);
        let parts: Vec<&str> = s.split("\u{1b}_G").filter(|p| !p.is_empty()).collect();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].starts_with("f=100,a=T,c=4,r=2,m=1;"));
        assert!(parts[1].starts_with("m=0;"));

        // Reassembled payload must decode to the original bytes.
        let mut b64 = String::new();
        for part in parts {
            let after = part.split_once(';').unwrap().1;
            b64.push_str(after.trim_end_matches("\u{1b}\\"));
        }
        assert_eq!(BASE64.decode(b64).unwrap(), png);
    }

    #[test]
    fn kitty_chunk_lengths_never_exceed_the_limit() {
        let png = vec![7u8; 20_000];
        let s = kitty_image(&png, 40, 20);
        for part in s.split("\u{1b}_G").filter(|p| !p.is_empty()) {
            let payload = part.split_once(';').unwrap().1.trim_end_matches("\u{1b}\\");
            assert!(payload.len() <= KITTY_CHUNK, "chunk too large");
        }
    }

    #[test]
    fn kitty_handles_empty_input() {
        let s = kitty_image(b"", 1, 1);
        assert_eq!(s, "\u{1b}_Gf=100,a=T,c=1,r=1,m=0;\u{1b}\\");
    }

    // -- iTerm2 -----------------------------------------------------------

    #[test]
    fn iterm2_prefix_and_payload() {
        let png = b"0123456789";
        let s = iterm2_image(png, 12, 6);
        assert!(s.starts_with(
            "\u{1b}]1337;File=inline=1;size=10;width=12cells;height=6cells;preserveAspectRatio=1:"
        ));
        assert!(s.ends_with('\u{7}'));
        let payload = s.rsplit_once(':').unwrap().1.trim_end_matches('\u{7}');
        assert_eq!(BASE64.decode(payload).unwrap(), png);
    }

    // -- Sixel ------------------------------------------------------------

    #[test]
    fn sixel_header_and_terminator() {
        let s = sixel_image(&solid(4, 6, (255, 0, 0)));
        assert!(s.starts_with("\u{1b}Pq\"1;1;4;6"));
        assert!(s.ends_with("\u{1b}\\"));
    }

    #[test]
    fn sixel_solid_red_uses_one_register_and_full_bits() {
        let s = sixel_image(&solid(8, 6, (255, 0, 0)));
        // Pure red maps to cube index 5*36 = 180.
        assert!(s.contains("#180;2;100;0;0"), "{s}");
        // A full band of six pixels is bitmask 0b111111 → '~', run-length 8.
        assert!(s.contains("#180!8~"), "{s}");
    }

    #[test]
    fn sixel_bands_are_separated_by_dashes() {
        // 12 rows → 2 bands → exactly one '-'.
        let s = sixel_image(&solid(2, 12, (0, 0, 255)));
        assert_eq!(s.matches('-').count(), 1);
        // 6 rows → single band → none.
        let s = sixel_image(&solid(2, 6, (0, 0, 255)));
        assert_eq!(s.matches('-').count(), 0);
    }

    #[test]
    fn sixel_two_colours_share_a_band_with_a_carriage_return() {
        let mut pixels = Vec::new();
        for y in 0..6 {
            for _x in 0..2 {
                if y < 3 {
                    pixels.extend_from_slice(&[255, 255, 255]);
                } else {
                    pixels.extend_from_slice(&[0, 0, 0]);
                }
            }
        }
        let img = RgbImage::new(2, 6, pixels).unwrap();
        let s = sixel_image(&img);
        assert_eq!(s.matches('$').count(), 1, "{s}");
        assert!(s.contains("#0;2;0;0;0"));
        assert!(s.contains("#215;2;100;100;100"));
    }

    #[test]
    fn sixel_partial_last_band_only_sets_existing_rows() {
        // 2 rows → bits 0 and 1 → 0b000011 = 3 → char 'B'.
        let s = sixel_image(&solid(3, 2, (255, 255, 255)));
        assert!(s.contains("#215BBB"), "{s}");
    }

    #[test]
    fn sixel_empty_image_is_a_valid_empty_sequence() {
        let img = RgbImage::new(0, 0, Vec::new()).unwrap();
        assert_eq!(sixel_image(&img), "\u{1b}Pq\"1;1;0;0\u{1b}\\");
    }

    #[test]
    fn sixel_char_mapping() {
        assert_eq!(sixel_char(0), '?');
        assert_eq!(sixel_char(0b111111), '~');
        assert_eq!(sixel_char(1), '@');
    }

    #[test]
    fn cube_quantisation() {
        assert_eq!(cube_index(0, 0, 0), 0);
        assert_eq!(cube_index(255, 255, 255), 215);
        assert_eq!(cube_index(255, 0, 0), 180);
        assert_eq!(cube_color(0), (0, 0, 0));
        assert_eq!(cube_color(215), (100, 100, 100));
    }

    // -- dispatch ----------------------------------------------------------

    #[test]
    fn dispatch_selects_the_right_protocol() {
        let src = ImageSource::Rgb(solid(6, 6, (0, 255, 0)));
        assert!(ImageSupport::None.encode(&src, 2, 1).is_none());
        assert!(ImageSupport::Kitty
            .encode(&src, 2, 1)
            .unwrap()
            .starts_with("\u{1b}_G"));
        assert!(ImageSupport::Iterm2
            .encode(&src, 2, 1)
            .unwrap()
            .starts_with("\u{1b}]1337;"));
        assert!(ImageSupport::Sixel
            .encode(&src, 2, 1)
            .unwrap()
            .starts_with("\u{1b}Pq"));
    }

    #[test]
    fn dispatch_wraps_for_tmux() {
        let src = ImageSource::Rgb(solid(6, 6, (0, 255, 0)));
        let s = ImageSupport::Kitty
            .encode_for_tmux(&src, 2, 1, true)
            .unwrap();
        assert!(s.starts_with("\u{1b}Ptmux;"));
        assert!(ImageSupport::None
            .encode_for_tmux(&src, 2, 1, true)
            .is_none());
    }

    #[test]
    fn png_roundtrip_between_representations() {
        let img = solid(5, 3, (10, 200, 30));
        let png = ImageSource::Rgb(img.clone()).to_png().unwrap();
        assert_eq!(png_dimensions(&png), Some((5, 3)));
        let back = ImageSource::Png(png.clone()).to_rgb().unwrap();
        assert_eq!(back, img);
        assert_eq!(ImageSource::Png(png).dimensions(), Some((5, 3)));
    }

    #[test]
    fn invalid_png_degrades_to_none() {
        let src = ImageSource::Png(b"not a png".to_vec());
        assert!(src.to_rgb().is_none());
        assert!(src.dimensions().is_none());
        assert!(ImageSupport::Sixel.encode(&src, 1, 1).is_none());
        // Kitty/iTerm2 pass bytes through unchecked; the terminal ignores junk.
        assert!(ImageSupport::Kitty.encode(&src, 1, 1).is_some());
    }

    #[test]
    fn rgb_image_rejects_wrong_buffer_length() {
        assert!(RgbImage::new(2, 2, vec![0; 11]).is_none());
        assert!(RgbImage::new(2, 2, vec![0; 12]).is_some());
    }
}
