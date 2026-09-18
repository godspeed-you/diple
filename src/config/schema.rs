//! Configuration schema, with serde defaults for every key, plus the precise
//! [`ConfigError`] required for invalid values.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::document::FormatRequest;

/// Top-level configuration (`~/.config/diple/config.toml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// How a document is read: `auto`, `markdown`, `json`, `yaml`.
    ///
    /// First in the file because it decides what the document *is*; every
    /// other key decides how it looks.
    pub format: FormatMode,
    /// Colour theme: `auto`, `dark`, `light` or a named theme.
    pub theme: Theme,
    /// Colour output: `auto`, `always`, `never`.
    pub color: ColorMode,
    /// Mouse support.
    pub mouse: bool,
    /// Show the TOC sidebar on startup.
    pub toc: bool,
    /// Show the key hints sidebar on startup.
    pub key_hints: bool,
    /// Show document line numbers.
    pub line_numbers: bool,
    /// Wrap prose to the terminal width.
    pub wrap: bool,
    /// Maximum line width in columns before wrapping (`0`: the full width).
    ///
    /// Long lines are hard to read; a limit keeps the measure comfortable on
    /// a wide terminal, which is why the default is `160` rather than `0`.
    /// The document is laid out at `min(max_width, available)`, so the limit
    /// never widens anything and does nothing on a narrower terminal.
    pub max_width: u16,
    /// Centre the document in the columns the sidebars leave over.
    ///
    /// Only the document moves: the TOC keeps the left edge and the key hints
    /// the right one, so with a `max_width` narrower than the terminal the
    /// sidebars sit outside the text rather than beside it. On by default, so
    /// the columns the default `max_width` gives up become two margins rather
    /// than a pile of empty space on the right.
    pub center: bool,
    /// Table rendering options.
    pub table: TableConfig,
    /// Code block rendering options.
    pub code: CodeConfig,
    /// Link handling options.
    pub links: LinksConfig,
    /// Mermaid rendering options.
    pub mermaid: MermaidConfig,
    /// Structured-document (JSON, YAML) rendering options.
    pub structured: StructuredConfig,
    /// Keybinding overrides: action name → key spec(s).
    pub keys: BTreeMap<String, KeyBinding>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            format: FormatMode::Auto,
            theme: Theme::Auto,
            color: ColorMode::Auto,
            mouse: true,
            toc: false,
            key_hints: false,
            line_numbers: false,
            wrap: true,
            max_width: 160,
            center: true,
            table: TableConfig::default(),
            code: CodeConfig::default(),
            links: LinksConfig::default(),
            mermaid: MermaidConfig::default(),
            structured: StructuredConfig::default(),
            keys: BTreeMap::new(),
        }
    }
}

/// `[table]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TableConfig {
    /// Layout mode.
    pub mode: TableMode,
    /// Maximum column width in cells.
    pub max_column_width: usize,
}

impl Default for TableConfig {
    fn default() -> Self {
        Self {
            mode: TableMode::Auto,
            max_column_width: 60,
        }
    }
}

/// `[code]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CodeConfig {
    /// Wrap long code lines instead of horizontal scrolling.
    pub wrap: bool,
    /// Show line numbers in code blocks.
    pub line_numbers: bool,
    /// Tab expansion width.
    pub tab_width: u8,
}

impl Default for CodeConfig {
    fn default() -> Self {
        Self {
            wrap: false,
            line_numbers: false,
            tab_width: 4,
        }
    }
}

/// `[links]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LinksConfig {
    /// Command used to open external links.
    pub opener: String,
    /// OSC 8 hyperlink emission.
    pub osc8: Osc8Mode,
}

impl Default for LinksConfig {
    fn default() -> Self {
        Self {
            opener: "xdg-open".to_string(),
            osc8: Osc8Mode::Auto,
        }
    }
}

/// `[mermaid]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MermaidConfig {
    /// Rendering backend.
    pub backend: MermaidBackend,
    /// Image protocol usage for diagrams.
    pub images: ImageMode,
    /// Mermaid CLI executable.
    pub mmdc_command: String,
}

impl Default for MermaidConfig {
    fn default() -> Self {
        Self {
            backend: MermaidBackend::Auto,
            images: ImageMode::Auto,
            mmdc_command: "mmdc".to_string(),
        }
    }
}

/// `[structured]` section — how JSON and YAML documents are presented.
///
/// Markdown ignores every key here: they describe a nesting tree, which is
/// what a structured document has and prose does not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StructuredConfig {
    /// Columns one nesting level is indented by.
    ///
    /// Two is the convention both formats already write themselves, and it
    /// keeps a deeply nested document inside the measure; eight would push a
    /// Kubernetes manifest off the right of the screen.
    pub indent: u8,
    /// When the path of the selected node is shown.
    pub path: PathMode,
    /// Show `[0]`-style indices on sequence items.
    pub show_indices: bool,
    /// Summarise what a collapsed container holds (`{3 members}`).
    pub collapsed_summary: bool,
}

impl Default for StructuredConfig {
    fn default() -> Self {
        Self {
            indent: 2,
            path: PathMode::Auto,
            show_indices: true,
            collapsed_summary: true,
        }
    }
}

/// A `[keys]` value: one key spec or a list of alternatives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyBinding {
    /// A single key spec, e.g. `"za"`.
    One(String),
    /// Alternative key specs, e.g. `["q", "ctrl-c"]`.
    Many(Vec<String>),
}

impl KeyBinding {
    /// The key specs in order.
    pub fn specs(&self) -> Vec<&str> {
        match self {
            KeyBinding::One(s) => vec![s.as_str()],
            KeyBinding::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

/// Theme selection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Theme {
    /// Detect dark/light from the terminal, default dark.
    #[default]
    Auto,
    /// Built-in dark theme.
    Dark,
    /// Built-in light theme.
    Light,
    /// A named custom theme.
    Named(String),
}

impl Theme {
    /// Parse from a config/CLI/env string. Never fails: unknown names are
    /// custom themes.
    pub fn parse(s: &str) -> Theme {
        match s {
            "auto" => Theme::Auto,
            "dark" => Theme::Dark,
            "light" => Theme::Light,
            other => Theme::Named(other.to_string()),
        }
    }

    /// Canonical string form.
    pub fn as_str(&self) -> &str {
        match self {
            Theme::Auto => "auto",
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::Named(n) => n,
        }
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for Theme {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Theme {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Theme::parse(&s))
    }
}

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
        #[serde(rename_all = "lowercase")]
        pub enum $name {
            $($(#[$vmeta])* $variant,)+
        }

        impl $name {
            /// Canonical string form.
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text,)+ }
            }

            /// Parse from a string (as used in config/env/CLI).
            pub fn parse(s: &str) -> Option<Self> {
                match s { $($text => Some($name::$variant),)+ _ => None }
            }

            /// The accepted values, for error messages.
            pub fn expected() -> &'static str {
                concat!("one of: ", $($text, " ",)+)
            }

            /// Every accepted value, for completion and the settings help.
            pub fn values() -> &'static [&'static str] {
                &[$($text,)+]
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum! {
    /// Colour output mode.
    ColorMode {
        /// Colours when stdout is a terminal.
        #[default] Auto => "auto",
        /// Always emit colours.
        Always => "always",
        /// Never emit colours (implies no styles).
        Never => "never",
    }
}

string_enum! {
    /// How a document is read.
    ///
    /// The configuration's counterpart to
    /// [`FormatRequest`](crate::document::FormatRequest): the document layer
    /// owns what the formats *are*, and this enum is how the file and the
    /// `--format` flag name one. [`FormatMode::request`] is the only bridge
    /// between the two, so the spellings cannot drift apart unnoticed.
    FormatMode {
        /// Extension, then content, then Markdown.
        #[default] Auto => "auto",
        /// Always read as Markdown.
        Markdown => "markdown",
        /// Always read as strict JSON.
        Json => "json",
        /// Always read as YAML.
        Yaml => "yaml",
    }
}

impl FormatMode {
    /// What the document layer should do with this setting.
    pub fn request(self) -> FormatRequest {
        // Every `FormatMode` spelling is a `FormatRequest` spelling; the test
        // below holds that true, so the fallback is unreachable rather than a
        // silent second opinion about what `json` means.
        FormatRequest::parse(self.as_str()).unwrap_or(FormatRequest::Auto)
    }
}

string_enum! {
    /// When the path of the selected structured node is shown.
    PathMode {
        /// Show it when there is room and the format has paths.
        #[default] Auto => "auto",
        /// Always show it.
        Always => "always",
        /// Never show it.
        Never => "never",
    }
}

string_enum! {
    /// Table layout mode.
    TableMode {
        /// Choose per table.
        #[default] Auto => "auto",
        /// Wrap cell content.
        Wrap => "wrap",
        /// Horizontal scrolling.
        Scroll => "scroll",
        /// Compact one-line-per-cell layout.
        Compact => "compact",
    }
}

string_enum! {
    /// OSC 8 hyperlink emission.
    Osc8Mode {
        /// Emit when the terminal supports it.
        #[default] Auto => "auto",
        /// Always emit.
        Always => "always",
        /// Never emit.
        Never => "never",
    }
}

string_enum! {
    /// Mermaid backend.
    MermaidBackend {
        /// Deterministic fallback matrix.
        #[default] Auto => "auto",
        /// Native terminal (box drawing) rendering.
        Terminal => "terminal",
        /// Render via the `mmdc` CLI.
        Mmdc => "mmdc",
        /// Show diagram source.
        Source => "source",
    }
}

string_enum! {
    /// Terminal image usage for diagrams.
    ImageMode {
        /// Use images when a protocol is detected.
        #[default] Auto => "auto",
        /// Force image output.
        Always => "always",
        /// Never use images.
        Never => "never",
    }
}

/// Precise configuration error (path, key, value, expected).
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The config file could not be read.
    #[error("{path}: cannot read configuration: {source}")]
    Io {
        /// File path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A value in the file failed to parse or validate.
    #[error("{path}:{line}: invalid value for `{key}`: `{value}` — expected {expected}")]
    Invalid {
        /// File path.
        path: PathBuf,
        /// Dotted key, e.g. `table.mode`.
        key: String,
        /// The offending value as written.
        value: String,
        /// What would have been accepted.
        expected: String,
        /// 1-based line number (0 if unknown).
        line: usize,
    },
    /// An environment variable override failed to validate.
    #[error("environment variable {var}: invalid value `{value}` — expected {expected}")]
    Env {
        /// Variable name (e.g. `DIPLE_MERMAID`).
        var: String,
        /// The offending value.
        value: String,
        /// What would have been accepted.
        expected: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::DocumentKind;

    /// The configuration names the formats the document layer names. A value
    /// the file accepts but `FormatRequest` does not would quietly become
    /// `auto`, which is the one failure mode `--format` exists to prevent.
    #[test]
    fn every_format_value_reaches_the_document_layer() {
        assert_eq!(
            FormatMode::values(),
            crate::document::format::FORMAT_VALUES,
            "the file and `--format` must accept the same words"
        );
        assert_eq!(FormatMode::Auto.request(), FormatRequest::Auto);
        assert_eq!(
            FormatMode::Markdown.request(),
            FormatRequest::Fixed(DocumentKind::Markdown)
        );
        assert_eq!(
            FormatMode::Json.request(),
            FormatRequest::Fixed(DocumentKind::Json)
        );
        assert_eq!(
            FormatMode::Yaml.request(),
            FormatRequest::Fixed(DocumentKind::Yaml)
        );
    }

    /// A file written before 2.0 knows none of these keys, and must still
    /// load into exactly the built-in behaviour (§26.2).
    #[test]
    fn the_structured_defaults_are_the_conventional_ones() {
        let config = Config::default();
        assert_eq!(config.format, FormatMode::Auto);
        assert_eq!(config.structured.indent, 2);
        assert_eq!(config.structured.path, PathMode::Auto);
        assert!(config.structured.show_indices);
        assert!(config.structured.collapsed_summary);
    }
}
