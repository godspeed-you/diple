//! Colours, styles and themes, plus the colour-depth downgrade chain
//! (TrueColor → 256 → 16 → none).
//!
//! # Boundary type
//!
//! [`ColorLevel`] is intentionally a *local copy* of the colour level that
//! `terminal::capabilities::Capabilities` reports. `render` must not depend on
//! the terminal workstream's types; converting one into the other is the
//! integrator's job (a trivial `match`).

use std::fmt;

/// Colour depth of the output device.
///
/// Local mirror of the terminal capability enum (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum ColorLevel {
    /// Monochrome: only bold/underline/reverse survive.
    None,
    /// The 16 ANSI colours.
    Ansi16,
    /// The xterm 256-colour palette.
    Ansi256,
    /// 24-bit colour.
    #[default]
    TrueColor,
}

impl ColorLevel {
    /// Canonical string form (`none`, `ansi16`, `ansi256`, `truecolor`).
    pub fn as_str(self) -> &'static str {
        match self {
            ColorLevel::None => "none",
            ColorLevel::Ansi16 => "ansi16",
            ColorLevel::Ansi256 => "ansi256",
            ColorLevel::TrueColor => "truecolor",
        }
    }
}

impl fmt::Display for ColorLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A colour, either 24-bit or a palette index (0..=15 are the ANSI colours).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// 24-bit RGB.
    Rgb(u8, u8, u8),
    /// Palette index into the xterm 256-colour table.
    Indexed(u8),
}

impl Color {
    /// Approximate RGB value of this colour.
    pub fn to_rgb(self) -> (u8, u8, u8) {
        match self {
            Color::Rgb(r, g, b) => (r, g, b),
            Color::Indexed(i) => indexed_to_rgb(i),
        }
    }

    /// Reduce this colour to the given level.
    pub fn downgrade(self, level: ColorLevel) -> Option<Color> {
        match level {
            ColorLevel::None => None,
            ColorLevel::TrueColor => Some(self),
            ColorLevel::Ansi256 => Some(match self {
                Color::Indexed(i) => Color::Indexed(i),
                Color::Rgb(r, g, b) => Color::Indexed(rgb_to_ansi256(r, g, b)),
            }),
            ColorLevel::Ansi16 => {
                let (r, g, b) = self.to_rgb();
                Some(Color::Indexed(rgb_to_ansi16(r, g, b)))
            }
        }
    }
}

/// The xterm 6×6×6 cube levels.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn nearest_cube(v: u8) -> (usize, u8) {
    let mut best = 0usize;
    let mut best_d = u32::MAX;
    for (i, c) in CUBE.iter().enumerate() {
        let d = (v as i32 - *c as i32).unsigned_abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    (best, CUBE[best])
}

/// Map 24-bit RGB to the closest xterm-256 index (colour cube or grey ramp).
pub fn rgb_to_ansi256(r: u8, g: u8, b: u8) -> u8 {
    let (ri, rq) = nearest_cube(r);
    let (gi, gq) = nearest_cube(g);
    let (bi, bq) = nearest_cube(b);
    let cube_index = 16 + 36 * ri as u16 + 6 * gi as u16 + bi as u16;
    let cube_dist = dist((r, g, b), (rq, gq, bq));

    // Grey ramp 232..=255: 8, 18, …, 238.
    let avg = (r as u32 + g as u32 + b as u32) / 3;
    let step = ((avg as i32 - 8).clamp(0, 238) as f32 / 10.0).round() as i32;
    let step = step.clamp(0, 23) as u8;
    let grey = 8 + 10 * step as u16;
    let grey = grey.min(238) as u8;
    let grey_dist = dist((r, g, b), (grey, grey, grey));

    if grey_dist < cube_dist {
        232 + step
    } else {
        cube_index.min(255) as u8
    }
}

const ANSI16: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (170, 0, 0),
    (0, 170, 0),
    (170, 85, 0),
    (0, 0, 170),
    (170, 0, 170),
    (0, 170, 170),
    (170, 170, 170),
    (85, 85, 85),
    (255, 85, 85),
    (85, 255, 85),
    (255, 255, 85),
    (85, 85, 255),
    (255, 85, 255),
    (85, 255, 255),
    (255, 255, 255),
];

/// Map 24-bit RGB to the nearest of the 16 ANSI colours.
pub fn rgb_to_ansi16(r: u8, g: u8, b: u8) -> u8 {
    let mut best = 0u8;
    let mut best_d = u32::MAX;
    for (i, c) in ANSI16.iter().enumerate() {
        let d = dist((r, g, b), *c);
        if d < best_d {
            best_d = d;
            best = i as u8;
        }
    }
    best
}

fn dist(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let dr = a.0 as i32 - b.0 as i32;
    let dg = a.1 as i32 - b.1 as i32;
    let db = a.2 as i32 - b.2 as i32;
    (dr * dr + dg * dg + db * db) as u32
}

/// RGB approximation of an xterm-256 palette index.
pub fn indexed_to_rgb(i: u8) -> (u8, u8, u8) {
    match i {
        0..=15 => ANSI16[i as usize],
        16..=231 => {
            let v = i as usize - 16;
            (CUBE[v / 36], CUBE[(v / 6) % 6], CUBE[v % 6])
        }
        _ => {
            let v = 8 + 10 * (i as u16 - 232);
            let v = v.min(255) as u8;
            (v, v, v)
        }
    }
}

/// A visual style. All fields are optional/off by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    /// Foreground colour.
    pub fg: Option<Color>,
    /// Background colour.
    pub bg: Option<Color>,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underline.
    pub underline: bool,
    /// Dim / faint.
    pub dim: bool,
    /// Strikethrough.
    pub strikethrough: bool,
    /// Reverse video (additive extension; used as the monochrome substitute
    /// for background colours).
    pub reverse: bool,
}

impl Style {
    /// The empty style.
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            bold: false,
            italic: false,
            underline: false,
            dim: false,
            strikethrough: false,
            reverse: false,
        }
    }

    /// With a foreground colour.
    pub const fn fg(mut self, c: Color) -> Self {
        self.fg = Some(c);
        self
    }

    /// With a background colour.
    pub const fn bg(mut self, c: Color) -> Self {
        self.bg = Some(c);
        self
    }

    /// Bold.
    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Italic.
    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// Underlined.
    pub const fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    /// Dim.
    pub const fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    /// Struck through.
    pub const fn strikethrough(mut self) -> Self {
        self.strikethrough = true;
        self
    }

    /// Reverse video.
    pub const fn reverse(mut self) -> Self {
        self.reverse = true;
        self
    }

    /// Overlay `other` on top of `self`: colours and set flags of `other`
    /// win, everything else is inherited.
    pub fn patch(self, other: Style) -> Style {
        Style {
            fg: other.fg.or(self.fg),
            bg: other.bg.or(self.bg),
            bold: self.bold || other.bold,
            italic: self.italic || other.italic,
            underline: self.underline || other.underline,
            dim: self.dim || other.dim,
            strikethrough: self.strikethrough || other.strikethrough,
            reverse: self.reverse || other.reverse,
        }
    }

    /// Reduce this style to the given colour level. At [`ColorLevel::None`]
    /// colours are dropped and a background colour becomes reverse video.
    pub fn downgrade(self, level: ColorLevel) -> Style {
        if level == ColorLevel::None {
            return Style {
                fg: None,
                bg: None,
                dim: false,
                reverse: self.reverse || self.bg.is_some(),
                ..self
            };
        }
        Style {
            fg: self.fg.and_then(|c| c.downgrade(level)),
            bg: self.bg.and_then(|c| c.downgrade(level)),
            ..self
        }
    }
}

/// A complete set of element styles.
///
/// Named themes may be added later (`Theme::builtin` returns `None` for
/// unknown names, callers fall back to `auto`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// Theme name (`dark`, `light`, …).
    pub name: String,
    /// Painted over the whole screen before anything else, for a theme whose
    /// point is the colour of the screen itself. Empty in the themes that let
    /// the terminal's own background through, which is all of them but `crt`.
    pub screen: Style,
    /// Whether this theme is designed for a dark background (also selects the
    /// syntect highlighting theme).
    pub dark: bool,
    /// Whether code blocks are coloured by the syntax highlighter.
    ///
    /// `false` draws them in [`Theme::code`] alone, for a theme whose palette
    /// is the whole point and which a highlighter's dozen hues would break.
    pub syntax: bool,
    /// Styles for heading levels 1..=6.
    pub heading: [Style; 6],
    /// Body text.
    pub text: Style,
    /// `*emphasis*`.
    pub emph: Style,
    /// `**strong**`.
    pub strong: Style,
    /// `~~strikethrough~~`.
    pub strike: Style,
    /// `` `inline code` `` and code block default foreground.
    pub code: Style,
    /// Background applied to code block lines.
    pub code_bg: Style,
    /// Blockquote text.
    pub quote: Style,
    /// Blockquote gutter (`▌`).
    pub quote_gutter: Style,
    /// Link text.
    pub link: Style,
    /// Currently selected link.
    pub link_selected: Style,
    /// Table borders.
    pub table_border: Style,
    /// Table header cells.
    pub table_header: Style,
    /// List bullets and numbers.
    pub list_marker: Style,
    /// Completed task checkbox.
    pub task_done: Style,
    /// Search match highlight.
    pub search_match: Style,
    /// The currently selected search match.
    pub search_current: Style,
    /// Status bar.
    pub status_bar: Style,
    /// TOC entries.
    pub toc: Style,
    /// Selected TOC entry.
    pub toc_selected: Style,
    /// Fold markers `▶`/`▼`.
    pub fold_marker: Style,
    /// Diagram lines.
    pub diagram: Style,
    /// Warnings and placeholders (`[image: …]`, unrenderable diagrams).
    pub warning: Style,
    /// Semantic roles of a structured document (JSON, YAML).
    pub structured: StructuredStyles,
}

/// The palette a structured document is drawn with.
///
/// These are *roles*, not colours: a theme decides what a key or a null looks
/// like, and the layout engine never reaches for a colour of its own. Colour
/// is also never the only signal — strings are quoted, nulls and booleans are
/// spelled, tags are written as the source wrote them — so the distinctions
/// survive `NO_COLOR` and a monochrome terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredStyles {
    /// A mapping key or object member name.
    pub key: Style,
    /// A sequence index label (`[0]`).
    pub index: Style,
    /// A string value.
    pub string: Style,
    /// A numeric value, in whatever form it was written.
    pub number: Style,
    /// `true` / `false`.
    pub boolean: Style,
    /// `null`, `~`, or an empty YAML value.
    pub null: Style,
    /// Braces, brackets, colons and sequence markers.
    pub punctuation: Style,
    /// A YAML comment.
    pub comment: Style,
    /// An explicit tag (`!!timestamp`).
    pub tag: Style,
    /// An anchor definition (`&name`).
    pub anchor: Style,
    /// An alias reference (`*name`).
    pub alias: Style,
    /// The `<<` merge key, which diple shows but never performs.
    pub merge_key: Style,
    /// A `%YAML` or `%TAG` directive.
    pub directive: Style,
    /// The `{4 entries}` summary of a collapsed container.
    pub folded: Style,
    /// The semantic path in the status line.
    pub path: Style,
    /// The row the semantic cursor is on.
    pub cursor_row: Style,
}

impl StructuredStyles {
    /// Derive the structured palette from the roles a theme already defines.
    ///
    /// Deriving rather than spelling sixteen more colours out per theme means
    /// a theme is coherent by construction — a JSON string is the colour that
    /// theme already uses for code, a comment the colour it uses for a
    /// blockquote — and that a theme added later gets a usable palette
    /// without knowing this type exists. A theme that wants something else
    /// may still overwrite the field after building itself.
    pub fn derived(theme: &Theme) -> StructuredStyles {
        /// The colour of a role without its emphasis: a key on every row must
        /// not be as loud as a heading once a page.
        fn hue(style: Style) -> Style {
            Style {
                fg: style.fg,
                ..Style::new()
            }
        }
        StructuredStyles {
            key: hue(theme.heading[0]),
            index: hue(theme.list_marker),
            string: hue(theme.code),
            number: hue(theme.heading[2]),
            boolean: hue(theme.heading[4]),
            null: theme.quote_gutter,
            punctuation: theme.table_border,
            comment: theme.quote,
            tag: hue(theme.heading[5]),
            anchor: hue(theme.link),
            alias: theme.link,
            merge_key: hue(theme.warning),
            directive: theme.quote_gutter,
            folded: Style {
                fg: theme.quote_gutter.fg,
                italic: true,
                ..Style::new()
            },
            path: hue(theme.toc),
            // The cursor row borrows the selection background the theme
            // already uses for the outline; a theme without one falls back to
            // reverse video, which every terminal has.
            cursor_row: match theme.toc_selected.bg {
                Some(bg) => Style::new().bg(bg),
                None => Style::new().reverse(),
            },
        }
    }
}

impl Default for StructuredStyles {
    /// A palette with no colours of its own.
    ///
    /// Every built-in theme replaces it through
    /// [`Theme::with_derived_structured`] immediately after building itself;
    /// the default exists so that the field can be written before the rest of
    /// the theme is known, and so a hand-built `Theme` in a test is still
    /// legal.
    fn default() -> Self {
        StructuredStyles {
            key: Style::new(),
            index: Style::new(),
            string: Style::new(),
            number: Style::new(),
            boolean: Style::new(),
            null: Style::new(),
            punctuation: Style::new(),
            comment: Style::new(),
            tag: Style::new(),
            anchor: Style::new(),
            alias: Style::new(),
            merge_key: Style::new(),
            directive: Style::new(),
            folded: Style::new(),
            path: Style::new(),
            cursor_row: Style::new().reverse(),
        }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

impl Default for Theme {
    fn default() -> Self {
        Theme::dark()
    }
}

impl Theme {
    /// Fill in [`Theme::structured`] from this theme's own roles.
    ///
    /// Every built-in theme ends its constructor with this call, so a theme is
    /// never shipped with a structured palette that disagrees with the rest of
    /// it, and adding a theme costs no extra colours.
    #[must_use]
    pub fn with_derived_structured(mut self) -> Theme {
        self.structured = StructuredStyles::derived(&self);
        self
    }

    /// The built-in dark theme.
    pub fn dark() -> Theme {
        let base = rgb(0x1a, 0x1b, 0x26);
        Theme {
            name: "dark".into(),
            screen: Style::new(),
            dark: true,
            syntax: true,
            heading: [
                Style::new().fg(rgb(0x82, 0xaa, 0xff)).bold(),
                Style::new().fg(rgb(0x9e, 0xce, 0x6a)).bold(),
                Style::new().fg(rgb(0xff, 0xcb, 0x6b)).bold(),
                Style::new().fg(rgb(0x89, 0xdd, 0xff)).bold(),
                Style::new().fg(rgb(0xc7, 0x92, 0xea)),
                Style::new().fg(rgb(0xf7, 0x8c, 0x6c)),
            ],
            text: Style::new(),
            emph: Style::new().italic(),
            strong: Style::new().bold(),
            strike: Style::new().strikethrough().dim(),
            code: Style::new().fg(rgb(0xc3, 0xe8, 0x8d)),
            code_bg: Style::new().bg(rgb(0x24, 0x28, 0x32)),
            quote: Style::new().fg(rgb(0xa9, 0xb1, 0xd6)).italic(),
            quote_gutter: Style::new().fg(rgb(0x56, 0x5f, 0x89)),
            link: Style::new().fg(rgb(0x7d, 0xcf, 0xff)).underline(),
            link_selected: Style::new().fg(base).bg(rgb(0x7d, 0xcf, 0xff)).bold(),
            table_border: Style::new().fg(rgb(0x56, 0x5f, 0x89)),
            table_header: Style::new().fg(rgb(0xff, 0xcb, 0x6b)).bold(),
            list_marker: Style::new().fg(rgb(0x82, 0xaa, 0xff)),
            task_done: Style::new().fg(rgb(0x9e, 0xce, 0x6a)),
            search_match: Style::new().fg(base).bg(rgb(0xe0, 0xaf, 0x68)),
            search_current: Style::new().fg(base).bg(rgb(0xff, 0x9e, 0x64)).bold(),
            status_bar: Style::new()
                .fg(rgb(0xc0, 0xca, 0xf5))
                .bg(rgb(0x2a, 0x2f, 0x3a)),
            toc: Style::new().fg(rgb(0xa9, 0xb1, 0xd6)),
            toc_selected: Style::new()
                .fg(rgb(0xff, 0xff, 0xff))
                .bg(rgb(0x36, 0x4a, 0x82))
                .bold(),
            fold_marker: Style::new().fg(rgb(0xff, 0x9e, 0x64)),
            diagram: Style::new().fg(rgb(0x89, 0xdd, 0xff)),
            warning: Style::new().fg(rgb(0xff, 0x9e, 0x64)).bold(),
            structured: StructuredStyles::default(),
        }
        .with_derived_structured()
    }

    /// The built-in light theme.
    pub fn light() -> Theme {
        let base = rgb(0xff, 0xff, 0xff);
        Theme {
            name: "light".into(),
            screen: Style::new(),
            dark: false,
            syntax: true,
            heading: [
                Style::new().fg(rgb(0x1a, 0x4f, 0xa0)).bold(),
                Style::new().fg(rgb(0x2a, 0x6b, 0x2a)).bold(),
                Style::new().fg(rgb(0x8a, 0x5a, 0x00)).bold(),
                Style::new().fg(rgb(0x0d, 0x6b, 0x7a)).bold(),
                Style::new().fg(rgb(0x6b, 0x30, 0x9a)),
                Style::new().fg(rgb(0xa0, 0x40, 0x20)),
            ],
            text: Style::new(),
            emph: Style::new().italic(),
            strong: Style::new().bold(),
            strike: Style::new().strikethrough().dim(),
            code: Style::new().fg(rgb(0x8a, 0x1a, 0x50)),
            code_bg: Style::new().bg(rgb(0xf0, 0xf0, 0xf0)),
            quote: Style::new().fg(rgb(0x4a, 0x4a, 0x4a)).italic(),
            quote_gutter: Style::new().fg(rgb(0x9a, 0x9a, 0x9a)),
            link: Style::new().fg(rgb(0x0a, 0x50, 0xc0)).underline(),
            link_selected: Style::new().fg(base).bg(rgb(0x0a, 0x50, 0xc0)).bold(),
            table_border: Style::new().fg(rgb(0x9a, 0x9a, 0x9a)),
            table_header: Style::new().fg(rgb(0x8a, 0x5a, 0x00)).bold(),
            list_marker: Style::new().fg(rgb(0x1a, 0x4f, 0xa0)),
            task_done: Style::new().fg(rgb(0x2a, 0x6b, 0x2a)),
            search_match: Style::new()
                .fg(rgb(0x00, 0x00, 0x00))
                .bg(rgb(0xff, 0xe0, 0x80)),
            search_current: Style::new()
                .fg(rgb(0x00, 0x00, 0x00))
                .bg(rgb(0xff, 0xa5, 0x40))
                .bold(),
            status_bar: Style::new()
                .fg(rgb(0x20, 0x20, 0x20))
                .bg(rgb(0xdd, 0xdd, 0xdd)),
            toc: Style::new().fg(rgb(0x30, 0x30, 0x30)),
            toc_selected: Style::new().fg(base).bg(rgb(0x1a, 0x4f, 0xa0)).bold(),
            fold_marker: Style::new().fg(rgb(0xa0, 0x40, 0x20)),
            diagram: Style::new().fg(rgb(0x0d, 0x6b, 0x7a)),
            warning: Style::new().fg(rgb(0xa0, 0x40, 0x20)).bold(),
            structured: StructuredStyles::default(),
        }
        .with_derived_structured()
    }

    /// The netrunner-console theme: cyan on black, crimson for anything that
    /// wants attention.
    ///
    /// Where [`Theme::crt`] imitates one monitor, this imitates one interface:
    /// a dark panel grid lit by two hues that never blend. Cyan carries the
    /// content — headings, links, prose — and crimson carries the chrome and
    /// the alarms: borders, list markers, warnings, the current search match.
    /// The two are kept apart on purpose, so anything red on the screen is
    /// something the reader is meant to look at.
    ///
    /// Unlike `crt` this is not a monochrome tube: it paints its own screen
    /// but keeps italics and syntax colouring, because the console it comes
    /// from is a bitmapped one.
    pub fn cyberpunk() -> Theme {
        // The panel behind everything: black with just enough blue in it to
        // read as a screen rather than as an absence.
        let base = rgb(0x05, 0x0a, 0x0e);
        let cyan = rgb(0x22, 0xe8, 0xe8);
        let cyan_bright = rgb(0x9c, 0xff, 0xff);
        let cyan_dim = rgb(0x14, 0x8a, 0x92);
        let crimson = rgb(0xe6, 0x00, 0x3c);
        let crimson_bright = rgb(0xff, 0x3d, 0x6e);
        let magenta = rgb(0xd6, 0x4c, 0xff);
        let amber = rgb(0xff, 0xb8, 0x2e);
        let body = rgb(0x8f, 0xd4, 0xd4);
        Theme {
            name: "cyberpunk".into(),
            screen: Style::new().fg(body).bg(base),
            dark: true,
            syntax: true,
            heading: [
                Style::new().fg(cyan_bright).bold(),
                Style::new().fg(cyan).bold(),
                Style::new().fg(crimson_bright).bold(),
                Style::new().fg(cyan_dim).bold(),
                Style::new().fg(magenta),
                Style::new().fg(cyan_dim),
            ],
            text: Style::new().fg(body),
            emph: Style::new().fg(cyan).italic(),
            strong: Style::new().fg(cyan_bright).bold(),
            strike: Style::new().fg(cyan_dim).strikethrough(),
            code: Style::new().fg(amber),
            code_bg: Style::new().bg(rgb(0x0a, 0x14, 0x1a)),
            quote: Style::new().fg(cyan_dim).italic(),
            quote_gutter: Style::new().fg(crimson),
            link: Style::new().fg(cyan).underline(),
            link_selected: Style::new().fg(base).bg(cyan).bold(),
            table_border: Style::new().fg(crimson),
            table_header: Style::new().fg(crimson_bright).bold(),
            list_marker: Style::new().fg(crimson),
            task_done: Style::new().fg(cyan),
            search_match: Style::new().fg(base).bg(cyan_dim),
            search_current: Style::new().fg(base).bg(crimson_bright).bold(),
            status_bar: Style::new().fg(cyan).bg(rgb(0x0d, 0x1a, 0x1f)),
            toc: Style::new().fg(cyan_dim),
            toc_selected: Style::new().fg(base).bg(cyan).bold(),
            fold_marker: Style::new().fg(crimson),
            diagram: Style::new().fg(cyan),
            warning: Style::new().fg(crimson_bright).bold(),
            structured: StructuredStyles::default(),
        }
        .with_derived_structured()
    }

    /// The phosphor-terminal theme: an early-nineties film's idea of a
    /// computer.
    ///
    /// Two colours, because a monitor of that era had two: P1 phosphor green
    /// for everything, and amber for anything that would have been meant to
    /// alarm the audience. Contrast is carried by brightness and by reversing
    /// the beam, the way it was on a monochrome tube, rather than by hue —
    /// which is why the headings step down through five greens instead of
    /// picking five colours, and why emphasis is underlined rather than
    /// italic: a character generator drawing an 8×14 cell had no italics.
    pub fn crt() -> Theme {
        // The tube: not quite black, because the phosphor never fully
        // stopped glowing between refreshes.
        let base = rgb(0x00, 0x14, 0x08);
        let bright = rgb(0xcc, 0xff, 0xcc);
        let green = rgb(0x33, 0xff, 0x33);
        let mid = rgb(0x2b, 0xcc, 0x2b);
        let dim = rgb(0x1d, 0x8a, 0x1d);
        let amber = rgb(0xff, 0xb0, 0x00);
        Theme {
            name: "crt".into(),
            // The one theme that paints the screen: the tube is the point.
            screen: Style::new().fg(green).bg(base),
            dark: true,
            // A terminal of that era coloured nothing by grammar, and a dozen
            // syntax hues would undo the two-phosphor palette in one code
            // block.
            syntax: false,
            heading: [
                Style::new().fg(bright).bold().underline(),
                Style::new().fg(bright).bold(),
                Style::new().fg(green).bold(),
                Style::new().fg(green),
                Style::new().fg(mid),
                Style::new().fg(dim),
            ],
            text: Style::new().fg(green),
            emph: Style::new().fg(green).underline(),
            strong: Style::new().fg(bright).bold(),
            strike: Style::new().fg(dim).strikethrough(),
            code: Style::new().fg(amber),
            code_bg: Style::new().bg(rgb(0x00, 0x22, 0x0e)),
            quote: Style::new().fg(mid),
            quote_gutter: Style::new().fg(dim),
            link: Style::new().fg(bright).underline(),
            link_selected: Style::new().fg(base).bg(bright).bold(),
            table_border: Style::new().fg(dim),
            table_header: Style::new().fg(bright).bold(),
            list_marker: Style::new().fg(green),
            task_done: Style::new().fg(amber),
            search_match: Style::new().fg(base).bg(mid),
            search_current: Style::new().fg(base).bg(green).bold(),
            status_bar: Style::new().fg(base).bg(green).bold(),
            toc: Style::new().fg(mid),
            toc_selected: Style::new().fg(base).bg(green).bold(),
            fold_marker: Style::new().fg(amber),
            diagram: Style::new().fg(green),
            warning: Style::new().fg(amber).bold(),
            structured: StructuredStyles::default(),
        }
        .with_derived_structured()
    }

    /// A built-in theme by name (`dark`, `light`), or `None`.
    pub fn builtin(name: &str) -> Option<Theme> {
        match name {
            "dark" => Some(Theme::dark()),
            "light" => Some(Theme::light()),
            "crt" => Some(Theme::crt()),
            "cyberpunk" => Some(Theme::cyberpunk()),
            _ => None,
        }
    }

    /// Resolve `auto`: use trivially available environment hints
    /// (`COLORFGBG`, `TERM_PROGRAM`), default to dark.
    pub fn auto() -> Theme {
        if detect_light_background() {
            Theme::light()
        } else {
            Theme::dark()
        }
    }

    /// Resolve a theme name (`auto`, `dark`, `light`, or an unknown custom
    /// name which falls back to `auto`).
    pub fn resolve(name: &str) -> Theme {
        match name {
            "auto" => Theme::auto(),
            other => Theme::builtin(other).unwrap_or_else(Theme::auto),
        }
    }

    /// A copy of this theme with every style reduced to `level`.
    pub fn downgraded(&self, level: ColorLevel) -> Theme {
        let mut t = self.clone();
        for style in t.styles_mut() {
            *style = style.downgrade(level);
        }
        t
    }

    fn styles_mut(&mut self) -> Vec<&mut Style> {
        let Theme {
            name: _,
            dark: _,
            syntax: _,
            screen,
            heading,
            text,
            emph,
            strong,
            strike,
            code,
            code_bg,
            quote,
            quote_gutter,
            link,
            link_selected,
            table_border,
            table_header,
            list_marker,
            task_done,
            search_match,
            search_current,
            status_bar,
            toc,
            toc_selected,
            fold_marker,
            diagram,
            warning,
            structured,
        } = self;
        let StructuredStyles {
            key,
            index,
            string,
            number,
            boolean,
            null,
            punctuation,
            comment,
            tag,
            anchor,
            alias,
            merge_key,
            directive,
            folded,
            path,
            cursor_row,
        } = structured;
        let mut v: Vec<&mut Style> = heading.iter_mut().collect();
        v.extend([
            key,
            index,
            string,
            number,
            boolean,
            null,
            punctuation,
            comment,
            tag,
            anchor,
            alias,
            merge_key,
            directive,
            folded,
            path,
            cursor_row,
        ]);
        v.extend([
            screen,
            text,
            emph,
            strong,
            strike,
            code,
            code_bg,
            quote,
            quote_gutter,
            link,
            link_selected,
            table_border,
            table_header,
            list_marker,
            task_done,
            search_match,
            search_current,
            status_bar,
            toc,
            toc_selected,
            fold_marker,
            diagram,
            warning,
        ]);
        v
    }

    /// Style for a heading level (levels outside 1..=6 clamp).
    pub fn heading(&self, level: u8) -> Style {
        let idx = (level.clamp(1, 6) - 1) as usize;
        self.heading.get(idx).copied().unwrap_or_default()
    }
}

/// Trivial light-background detection from environment variables.
fn detect_light_background() -> bool {
    if let Ok(fgbg) = std::env::var("COLORFGBG") {
        // Format "fg;bg" (sometimes "fg;default;bg"); a low bg index means a
        // dark background, 7/15 mean a light one.
        if let Some(bg) = fgbg.rsplit(';').next() {
            if let Ok(idx) = bg.trim().parse::<u32>() {
                return matches!(idx, 7 | 15);
            }
        }
    }
    if let Ok(prog) = std::env::var("TERM_PROGRAM") {
        if prog == "Apple_Terminal" {
            // Apple Terminal's factory default profile is light.
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truecolor_to_256_cube_and_grey() {
        assert_eq!(rgb_to_ansi256(0, 0, 0), 16);
        assert_eq!(rgb_to_ansi256(255, 255, 255), 231);
        assert_eq!(rgb_to_ansi256(255, 0, 0), 196);
        // Mid grey lands on the grey ramp.
        let g = rgb_to_ansi256(0x80, 0x80, 0x80);
        assert!((232..=255).contains(&g), "got {g}");
    }

    #[test]
    fn truecolor_to_16() {
        assert_eq!(rgb_to_ansi16(0, 0, 0), 0);
        assert_eq!(
            rgb_to_ansi16(250, 10, 10),
            1,
            "dark red is nearer to the normal red"
        );
        assert_eq!(rgb_to_ansi16(255, 120, 120), 9);
        assert_eq!(rgb_to_ansi16(250, 250, 250), 15);
        assert_eq!(rgb_to_ansi16(10, 160, 10), 2);
    }

    #[test]
    fn style_downgrade_chain() {
        let s = Style::new()
            .fg(rgb(0x82, 0xaa, 0xff))
            .bg(rgb(0, 0, 0))
            .bold();
        let t256 = s.downgrade(ColorLevel::Ansi256);
        assert!(matches!(t256.fg, Some(Color::Indexed(_))));
        assert!(t256.bold);
        let t16 = s.downgrade(ColorLevel::Ansi16);
        assert!(matches!(t16.fg, Some(Color::Indexed(i)) if i < 16));
        let mono = s.downgrade(ColorLevel::None);
        assert_eq!(mono.fg, None);
        assert_eq!(mono.bg, None);
        assert!(mono.bold);
        assert!(mono.reverse, "background becomes reverse video");
    }

    #[test]
    fn theme_downgrade_touches_every_style() {
        let t = Theme::dark().downgraded(ColorLevel::Ansi16);
        assert!(matches!(t.heading(1).fg, Some(Color::Indexed(i)) if i < 16));
        assert!(matches!(t.link.fg, Some(Color::Indexed(i)) if i < 16));
        assert!(matches!(t.warning.fg, Some(Color::Indexed(i)) if i < 16));
        let mono = Theme::light().downgraded(ColorLevel::None);
        assert_eq!(mono.code_bg.fg, None);
        assert_eq!(mono.code_bg.bg, None);
        assert!(mono.search_match.reverse);
    }

    #[test]
    fn indexed_roundtrip_is_sane() {
        for i in 0u8..=255 {
            let (r, g, b) = indexed_to_rgb(i);
            let back = rgb_to_ansi256(r, g, b);
            let (r2, g2, b2) = indexed_to_rgb(back);
            assert!(dist((r, g, b), (r2, g2, b2)) < 400, "index {i}");
        }
    }

    /// The point of `crt` is that it looks like one monitor, not like a
    /// palette: every colour it draws with is either phosphor green or the
    /// amber alert colour, and it paints its own screen.
    #[test]
    fn the_crt_theme_is_two_phosphors_on_a_painted_screen() {
        let theme = Theme::builtin("crt").expect("a built-in theme");
        assert_eq!(theme.name, "crt");
        assert!(theme.dark, "it selects the dark syntax highlighting");
        assert_eq!(theme.screen.bg, Some(rgb(0x00, 0x14, 0x08)));

        let mut theme = theme;
        let amber = rgb(0xff, 0xb0, 0x00);
        for style in theme.styles_mut() {
            for colour in [style.fg, style.bg].into_iter().flatten() {
                let Color::Rgb(r, g, b) = colour else {
                    panic!("the palette is defined in RGB");
                };
                if colour == amber {
                    continue;
                }
                assert!(
                    g >= r && g >= b,
                    "green never loses to another channel: {colour:?}"
                );
            }
        }
    }

    /// The themes that do not own the screen must not paint it, or every
    /// terminal's own background would be overwritten by an approximation of
    /// itself.
    #[test]
    fn only_the_screen_owning_themes_paint_it() {
        assert_eq!(Theme::dark().screen.bg, None);
        assert_eq!(Theme::light().screen.bg, None);
        assert!(Theme::crt().screen.bg.is_some());
        assert!(Theme::cyberpunk().screen.bg.is_some());
    }

    /// `cyberpunk` divides its two hues by job: cyan is content, crimson is
    /// chrome and alarm. The split is the whole design, so it is pinned —
    /// a heading that turns red, or a warning that turns cyan, would make
    /// red stop meaning "look here".
    #[test]
    fn the_cyberpunk_theme_keeps_content_and_alarm_apart() {
        let theme = Theme::builtin("cyberpunk").expect("a built-in theme");
        assert!(
            theme.dark && theme.syntax,
            "a bitmapped console, not a tube"
        );

        let reddish = |style: Style| match style.fg {
            Some(Color::Rgb(r, g, b)) => r > g && r > b,
            _ => false,
        };
        for (what, style) in [
            ("body text", theme.text),
            ("links", theme.link),
            ("the first heading", theme.heading[0]),
            ("emphasis", theme.emph),
        ] {
            assert!(!reddish(style), "{what} must not compete with the alarms");
        }
        for (what, style) in [
            ("warnings", theme.warning),
            ("table borders", theme.table_border),
            ("list markers", theme.list_marker),
            ("fold markers", theme.fold_marker),
        ] {
            assert!(reddish(style), "{what} carry the crimson");
        }
    }

    #[test]
    fn resolve_names() {
        assert_eq!(Theme::resolve("dark").name, "dark");
        assert_eq!(Theme::resolve("light").name, "light");
        assert_eq!(Theme::resolve("crt").name, "crt");
        assert_eq!(Theme::resolve("cyberpunk").name, "cyberpunk");
        assert!(Theme::builtin("nope").is_none());
    }
}
