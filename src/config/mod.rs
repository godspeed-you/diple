//! Configuration: schema, loader (XDG + `--config` + env), and keybinding
//! resolution.

pub mod actions;
pub mod keys;
pub mod loader;
pub mod schema;
pub mod settings;

pub use actions::Action;
pub use keys::{Key, KeyMap, KeyMatch};
pub use loader::{load, LoadedConfig};
pub use schema::{
    CodeConfig, ColorMode, Config, ConfigError, FormatMode, ImageMode, KeyBinding, LinksConfig,
    MermaidBackend, MermaidConfig, Osc8Mode, PathMode, StructuredConfig, TableConfig, TableMode,
    Theme,
};

use crate::cli::CliArgs;

impl Config {
    /// Apply `DIPLE_*` environment overrides and CLI overrides on top of
    /// this configuration. Precedence: defaults < file < env < CLI.
    pub fn merged(&self, cli: &CliArgs) -> Result<Config, ConfigError> {
        self.merged_with_env(cli, |k| std::env::var(k).ok())
    }

    /// [`Config::merged`] with an injectable environment (for tests).
    pub fn merged_with_env(
        &self,
        cli: &CliArgs,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Config, ConfigError> {
        let mut merged = self.clone();

        // Environment overrides.
        if let Some(theme) = env("DIPLE_THEME") {
            merged.theme = Theme::parse(&theme);
        }
        if let Some(backend) = env("DIPLE_MERMAID") {
            merged.mermaid.backend =
                MermaidBackend::parse(&backend).ok_or_else(|| ConfigError::Env {
                    var: "DIPLE_MERMAID".to_string(),
                    value: backend.clone(),
                    expected: MermaidBackend::expected().trim().to_string(),
                })?;
        }

        // CLI overrides.
        if let Some(format) = cli.format {
            merged.format = format;
        }
        if let Some(indent) = cli.structured_indent {
            merged.structured.indent = indent;
        }
        if let Some(path) = cli.path {
            merged.structured.path = path;
        }
        if let Some(theme) = &cli.theme {
            merged.theme = Theme::parse(theme);
        }
        if let Some(color) = cli.color {
            merged.color = color;
        }
        if let Some(mouse) = cli.mouse_override() {
            merged.mouse = mouse;
        }
        if let Some(toc) = cli.toc_override() {
            merged.toc = toc;
        }
        if let Some(key_hints) = cli.key_hints_override() {
            merged.key_hints = key_hints;
        }
        if let Some(ln) = cli.line_numbers_override() {
            merged.line_numbers = ln;
        }
        if let Some(wrap) = cli.wrap_override() {
            merged.wrap = wrap;
        }
        if let Some(max_width) = cli.max_width {
            merged.max_width = max_width;
        }
        if let Some(center) = cli.center_override() {
            merged.center = center;
        }
        if let Some(backend) = cli.mermaid {
            merged.mermaid.backend = backend;
        }
        if let Some(images) = cli.mermaid_images {
            merged.mermaid.images = images;
        }
        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CliArgs;
    use clap::Parser;

    fn cli(args: &[&str]) -> CliArgs {
        CliArgs::try_parse_from(std::iter::once("diple").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn cli_overrides_config() {
        let mut base = Config {
            theme: Theme::Dark,
            mouse: true,
            ..Config::default()
        };
        base.mermaid.backend = MermaidBackend::Terminal;
        let merged = base
            .merged_with_env(
                &cli(&["--theme", "light", "--no-mouse", "--mermaid", "source"]),
                |_| None,
            )
            .unwrap();
        assert_eq!(merged.theme, Theme::Light);
        assert!(!merged.mouse);
        assert_eq!(merged.mermaid.backend, MermaidBackend::Source);
        // Untouched values survive.
        assert_eq!(merged.table.max_column_width, 60);
    }

    #[test]
    fn cli_overrides_the_width_limit_and_centring() {
        let base = Config {
            max_width: 80,
            center: true,
            ..Config::default()
        };
        let merged = base
            .merged_with_env(&cli(&["--max-width", "140", "--no-center"]), |_| None)
            .unwrap();
        assert_eq!(merged.max_width, 140);
        assert!(!merged.center);

        // Unset flags leave the configured values alone.
        let merged = base.merged_with_env(&cli(&[]), |_| None).unwrap();
        assert_eq!(merged.max_width, 80);
        assert!(merged.center);
    }

    #[test]
    fn env_overrides_config_but_cli_wins() {
        let base = Config::default();
        let env = |k: &str| match k {
            "DIPLE_THEME" => Some("dark".to_string()),
            "DIPLE_MERMAID" => Some("mmdc".to_string()),
            _ => None,
        };
        let merged = base.merged_with_env(&cli(&[]), env).unwrap();
        assert_eq!(merged.theme, Theme::Dark);
        assert_eq!(merged.mermaid.backend, MermaidBackend::Mmdc);

        let merged = base
            .merged_with_env(&cli(&["--theme", "light", "--mermaid", "source"]), env)
            .unwrap();
        assert_eq!(merged.theme, Theme::Light, "CLI beats env");
        assert_eq!(merged.mermaid.backend, MermaidBackend::Source);
    }

    #[test]
    fn invalid_env_value_is_reported() {
        let base = Config::default();
        let err = base
            .merged_with_env(&cli(&[]), |k| {
                (k == "DIPLE_MERMAID").then(|| "webgl".to_string())
            })
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("DIPLE_MERMAID"), "{msg}");
        assert!(msg.contains("webgl"), "{msg}");
        assert!(msg.contains("terminal"), "{msg}");
    }

    #[test]
    fn key_hints_follows_the_same_precedence_as_toc() {
        let base = Config {
            key_hints: true,
            ..Config::default()
        };
        assert!(!Config::default().key_hints, "off by default");

        let merged = base.merged_with_env(&cli(&[]), |_| None).unwrap();
        assert!(merged.key_hints, "the file value survives");

        let merged = base
            .merged_with_env(&cli(&["--no-key-hints"]), |_| None)
            .unwrap();
        assert!(!merged.key_hints, "the flag beats the file");

        let merged = Config::default()
            .merged_with_env(&cli(&["--key-hints"]), |_| None)
            .unwrap();
        assert!(merged.key_hints);

        let merged = Config::default()
            .merged_with_env(&cli(&["--key-hints", "--no-key-hints"]), |_| None)
            .unwrap();
        assert!(!merged.key_hints, "the later flag wins");
    }

    /// `--format` is the last word on how a document is read, and the file
    /// may still set the default for a session that does not pass it.
    #[test]
    fn the_format_flag_beats_the_configured_default() {
        use crate::document::{DocumentKind, FormatRequest};

        let base = Config {
            format: FormatMode::Yaml,
            ..Config::default()
        };
        let merged = base.merged_with_env(&cli(&[]), |_| None).unwrap();
        assert_eq!(
            merged.format.request(),
            FormatRequest::Fixed(DocumentKind::Yaml),
            "the file decides when the flag is absent"
        );

        let merged = base
            .merged_with_env(&cli(&["--format", "json"]), |_| None)
            .unwrap();
        assert_eq!(
            merged.format.request(),
            FormatRequest::Fixed(DocumentKind::Json)
        );

        let merged = base
            .merged_with_env(&cli(&["--format", "auto"]), |_| None)
            .unwrap();
        assert_eq!(merged.format.request(), FormatRequest::Auto);
    }

    #[test]
    fn the_structured_flags_override_the_structured_section() {
        let mut base = Config::default();
        base.structured.indent = 4;
        base.structured.path = PathMode::Never;

        let merged = base.merged_with_env(&cli(&[]), |_| None).unwrap();
        assert_eq!(merged.structured.indent, 4);
        assert_eq!(merged.structured.path, PathMode::Never);
        assert!(merged.structured.show_indices, "untouched keys survive");

        let merged = base
            .merged_with_env(
                &cli(&["--structured-indent", "8", "--path", "always"]),
                |_| None,
            )
            .unwrap();
        assert_eq!(merged.structured.indent, 8);
        assert_eq!(merged.structured.path, PathMode::Always);
    }

    #[test]
    fn no_overrides_is_identity() {
        let base = Config::default();
        let merged = base.merged_with_env(&cli(&[]), |_| None).unwrap();
        assert_eq!(merged, base);
    }
}
