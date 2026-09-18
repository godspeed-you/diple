//! The settable configuration surface, as data.
//!
//! One table describes every key the `:` command line accepts: its name, what
//! values it takes, its default, and one line of help. Reading a value,
//! writing a value, completing a half-typed one and printing the help all come
//! from that single table, so a key can never be settable but undocumented, or
//! completable but unsettable.
//!
//! The names are the ones the configuration file uses, with a dot for the
//! section: `center`, `table.mode`, `code.tab_width`. Sections themselves are
//! not settable — there is nothing to assign to a table of keys.

use super::schema::{
    ColorMode, Config, FormatMode, ImageMode, MermaidBackend, Osc8Mode, PathMode, TableMode, Theme,
};

/// What a setting accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `true` or `false`.
    Bool,
    /// A whole number within an inclusive range.
    Number {
        /// Smallest accepted value.
        min: u64,
        /// Largest accepted value.
        max: u64,
    },
    /// One of a fixed set of words.
    Choice(&'static [&'static str]),
    /// A choice that also accepts anything else — the theme name, which may
    /// name a theme this build does not know.
    ChoiceOrName(&'static [&'static str]),
    /// Free text, e.g. a command name.
    Text,
}

impl Kind {
    /// The accepted values, spelled out for the help and for completion.
    pub fn values(self) -> Vec<&'static str> {
        match self {
            Kind::Bool => vec!["true", "false"],
            Kind::Choice(v) | Kind::ChoiceOrName(v) => v.to_vec(),
            Kind::Number { .. } | Kind::Text => Vec::new(),
        }
    }

    /// How the accepted values read in the help column.
    pub fn describe(self) -> String {
        match self {
            Kind::Bool => "true | false".to_string(),
            Kind::Choice(v) => v.join(" | "),
            Kind::ChoiceOrName(v) => format!("{} | <name>", v.join(" | ")),
            Kind::Number { min, max } if max == u64::from(u16::MAX) => format!("{min}.."),
            Kind::Number { min, max } => format!("{min}..{max}"),
            Kind::Text => "<text>".to_string(),
        }
    }
}

/// One settable key.
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    /// Dotted name, as in the configuration file.
    pub name: &'static str,
    /// What it accepts.
    pub kind: Kind,
    /// The built-in default, as it would be written in the file.
    pub default: &'static str,
    /// One line: what the key does.
    pub help: &'static str,
}

/// Whether changing a key needs more than a redraw.
///
/// The command line applies this: a key that feeds the layout forces a
/// rebuild, and one that feeds the palette rebuilds the theme. Nothing else
/// in the program has to know which key is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Only the next frame differs.
    Redraw,
    /// The document must be laid out again.
    Relayout,
    /// The resolved theme and colour level must be rebuilt.
    Palette,
    /// The sidebars follow the new value.
    Sidebar,
    /// Mouse reporting follows the new value.
    Mouse,
}

const BOOL: Kind = Kind::Bool;

/// Every settable key, in help-display order: the top-level keys first, then
/// the sections, each in the order the configuration file writes them.
pub const ALL: &[Setting] = &[
    Setting {
        name: "format",
        kind: Kind::Choice(&["auto", "markdown", "json", "yaml"]),
        default: "auto",
        help: "how a document is read; the open one stays as it was parsed",
    },
    Setting {
        name: "theme",
        kind: Kind::ChoiceOrName(&["auto", "dark", "light", "crt", "cyberpunk"]),
        default: "auto",
        help: "colour theme; auto follows the terminal background",
    },
    Setting {
        name: "color",
        kind: Kind::Choice(&["auto", "always", "never"]),
        default: "auto",
        help: "emit colour: auto detects, never renders plain",
    },
    Setting {
        name: "mouse",
        kind: BOOL,
        default: "true",
        help: "report mouse events; off lets the terminal select text",
    },
    Setting {
        name: "toc",
        kind: BOOL,
        default: "false",
        help: "the table-of-contents sidebar",
    },
    Setting {
        name: "key_hints",
        kind: BOOL,
        default: "false",
        help: "the key hints sidebar",
    },
    Setting {
        name: "line_numbers",
        kind: BOOL,
        default: "false",
        help: "document line numbers",
    },
    Setting {
        name: "wrap",
        kind: BOOL,
        default: "true",
        help: "reflow paragraphs to the available width",
    },
    Setting {
        name: "max_width",
        kind: Kind::Number {
            min: 0,
            max: u16::MAX as u64,
        },
        default: "160",
        help: "cap the line width in columns; 0 is the full width",
    },
    Setting {
        name: "center",
        kind: BOOL,
        default: "true",
        help: "centre the document between the sidebars",
    },
    Setting {
        name: "table.mode",
        kind: Kind::Choice(&["auto", "wrap", "scroll", "compact"]),
        default: "auto",
        help: "how tables use the width they are given",
    },
    Setting {
        name: "table.max_column_width",
        kind: Kind::Number { min: 3, max: 4096 },
        default: "60",
        help: "widest a single table column may become, in cells",
    },
    Setting {
        name: "code.wrap",
        kind: BOOL,
        default: "false",
        help: "wrap long code lines instead of scrolling them",
    },
    Setting {
        name: "code.line_numbers",
        kind: BOOL,
        default: "false",
        help: "line numbers inside code blocks",
    },
    Setting {
        name: "code.tab_width",
        kind: Kind::Number { min: 1, max: 16 },
        default: "4",
        help: "columns a tab expands to inside code",
    },
    Setting {
        name: "links.opener",
        kind: Kind::Text,
        default: "xdg-open",
        help: "command run to open an external link",
    },
    Setting {
        name: "links.osc8",
        kind: Kind::Choice(&["auto", "always", "never"]),
        default: "auto",
        help: "emit links as native terminal hyperlinks",
    },
    Setting {
        name: "mermaid.backend",
        kind: Kind::Choice(&["auto", "terminal", "mmdc", "source"]),
        default: "auto",
        help: "how Mermaid diagrams are rendered",
    },
    Setting {
        name: "mermaid.images",
        kind: Kind::Choice(&["auto", "always", "never"]),
        default: "auto",
        help: "use the terminal image protocol for diagrams",
    },
    Setting {
        name: "mermaid.mmdc_command",
        kind: Kind::Text,
        default: "mmdc",
        help: "the Mermaid CLI executable",
    },
    Setting {
        name: "structured.indent",
        kind: Kind::Number { min: 1, max: 16 },
        default: "2",
        help: "columns one JSON/YAML nesting level is indented by",
    },
    Setting {
        name: "structured.path",
        kind: Kind::Choice(&["auto", "always", "never"]),
        default: "auto",
        help: "show the path of the selected node",
    },
    Setting {
        name: "structured.show_indices",
        kind: BOOL,
        default: "true",
        help: "number sequence items as [0], [1], [2]",
    },
    Setting {
        name: "structured.collapsed_summary",
        kind: BOOL,
        default: "true",
        help: "say what a collapsed container holds",
    },
];

/// Look a key up by its exact name.
pub fn get(name: &str) -> Option<&'static Setting> {
    ALL.iter().find(|s| s.name == name)
}

/// Every key name starting with `prefix`, in help order.
pub fn matching(prefix: &str) -> Vec<&'static str> {
    ALL.iter()
        .map(|s| s.name)
        .filter(|n| n.starts_with(prefix))
        .collect()
}

/// The current value of `name`, written the way the key accepts it.
pub fn read(config: &Config, name: &str) -> Option<String> {
    let v = match name {
        "format" => config.format.as_str().to_string(),
        "theme" => config.theme.as_str().to_string(),
        "color" => config.color.as_str().to_string(),
        "mouse" => config.mouse.to_string(),
        "toc" => config.toc.to_string(),
        "key_hints" => config.key_hints.to_string(),
        "line_numbers" => config.line_numbers.to_string(),
        "wrap" => config.wrap.to_string(),
        "max_width" => config.max_width.to_string(),
        "center" => config.center.to_string(),
        "table.mode" => config.table.mode.as_str().to_string(),
        "table.max_column_width" => config.table.max_column_width.to_string(),
        "code.wrap" => config.code.wrap.to_string(),
        "code.line_numbers" => config.code.line_numbers.to_string(),
        "code.tab_width" => config.code.tab_width.to_string(),
        "links.opener" => config.links.opener.clone(),
        "links.osc8" => config.links.osc8.as_str().to_string(),
        "mermaid.backend" => config.mermaid.backend.as_str().to_string(),
        "mermaid.images" => config.mermaid.images.as_str().to_string(),
        "mermaid.mmdc_command" => config.mermaid.mmdc_command.clone(),
        "structured.indent" => config.structured.indent.to_string(),
        "structured.path" => config.structured.path.as_str().to_string(),
        "structured.show_indices" => config.structured.show_indices.to_string(),
        "structured.collapsed_summary" => config.structured.collapsed_summary.to_string(),
        _ => return None,
    };
    Some(v)
}

/// Why a value was refused. The message is shown as typed, so it names the
/// key and what it would have accepted instead.
pub fn write(config: &mut Config, name: &str, value: &str) -> Result<Effect, String> {
    let Some(setting) = get(name) else {
        return Err(format!("unknown setting: {name}"));
    };
    let bad = || format!("{name} takes {}", setting.kind.describe());
    let number = |max: u64| -> Result<u64, String> {
        let n: u64 = value.parse().map_err(|_| bad())?;
        match setting.kind {
            Kind::Number { min, .. } if n < min || n > max => Err(bad()),
            _ => Ok(n),
        }
    };
    let boolean = || -> Result<bool, String> {
        match value {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(bad()),
        }
    };

    let effect = match name {
        // A document is parsed once, from the format that was asked for when
        // it was opened (spec §16.3: changing the format afterwards is not a
        // runtime setting). Writing this key records what the *next* session
        // should read, so nothing on screen has to change.
        "format" => {
            config.format = FormatMode::parse(value).ok_or_else(bad)?;
            Effect::Redraw
        }
        "theme" => {
            config.theme = Theme::parse(value);
            Effect::Palette
        }
        "color" => {
            config.color = ColorMode::parse(value).ok_or_else(bad)?;
            Effect::Palette
        }
        "mouse" => {
            config.mouse = boolean()?;
            Effect::Mouse
        }
        "toc" => {
            config.toc = boolean()?;
            Effect::Sidebar
        }
        "key_hints" => {
            config.key_hints = boolean()?;
            Effect::Sidebar
        }
        "line_numbers" => {
            config.line_numbers = boolean()?;
            Effect::Relayout
        }
        "wrap" => {
            config.wrap = boolean()?;
            Effect::Relayout
        }
        "max_width" => {
            config.max_width = u16::try_from(number(u64::from(u16::MAX))?).map_err(|_| bad())?;
            Effect::Relayout
        }
        "center" => {
            config.center = boolean()?;
            Effect::Relayout
        }
        "table.mode" => {
            config.table.mode = TableMode::parse(value).ok_or_else(bad)?;
            Effect::Relayout
        }
        "table.max_column_width" => {
            config.table.max_column_width = usize::try_from(number(4096)?).map_err(|_| bad())?;
            Effect::Relayout
        }
        "code.wrap" => {
            config.code.wrap = boolean()?;
            Effect::Relayout
        }
        "code.line_numbers" => {
            config.code.line_numbers = boolean()?;
            Effect::Relayout
        }
        "code.tab_width" => {
            config.code.tab_width = u8::try_from(number(16)?).map_err(|_| bad())?;
            Effect::Relayout
        }
        "links.opener" => {
            if value.is_empty() {
                return Err(bad());
            }
            config.links.opener = value.to_string();
            Effect::Redraw
        }
        "links.osc8" => {
            config.links.osc8 = Osc8Mode::parse(value).ok_or_else(bad)?;
            Effect::Redraw
        }
        "mermaid.backend" => {
            config.mermaid.backend = MermaidBackend::parse(value).ok_or_else(bad)?;
            Effect::Relayout
        }
        "mermaid.images" => {
            config.mermaid.images = ImageMode::parse(value).ok_or_else(bad)?;
            Effect::Relayout
        }
        "mermaid.mmdc_command" => {
            if value.is_empty() {
                return Err(bad());
            }
            config.mermaid.mmdc_command = value.to_string();
            Effect::Relayout
        }
        "structured.indent" => {
            config.structured.indent = u8::try_from(number(16)?).map_err(|_| bad())?;
            Effect::Relayout
        }
        "structured.path" => {
            config.structured.path = PathMode::parse(value).ok_or_else(bad)?;
            Effect::Redraw
        }
        "structured.show_indices" => {
            config.structured.show_indices = boolean()?;
            Effect::Relayout
        }
        "structured.collapsed_summary" => {
            config.structured.collapsed_summary = boolean()?;
            Effect::Relayout
        }
        _ => return Err(format!("unknown setting: {name}")),
    };
    Ok(effect)
}

/// The settings help, as `(key, explanation)` rows.
///
/// The explanation carries the three things a reader needs before typing a
/// value: what the key does, what it accepts, and what it started as.
pub fn help_entries() -> Vec<(String, String)> {
    ALL.iter()
        .map(|s| {
            (
                s.name.to_string(),
                // Values first, then the default, then the prose: a narrow
                // overlay truncates from the right, and the two things a
                // reader needs before typing are what a key accepts and what
                // it was.
                format!(
                    "{}  ·  default {}  ·  {}",
                    s.kind.describe(),
                    s.default,
                    s.help
                ),
            )
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The table claims a default for every key. A claim that disagrees with
    /// `Config::default()` is worse than no help at all, so it is checked
    /// rather than trusted — this is the test that fails when a default is
    /// changed and the help is not.
    #[test]
    fn the_documented_defaults_are_the_real_defaults() {
        let config = Config::default();
        for setting in ALL {
            let actual = read(&config, setting.name)
                .unwrap_or_else(|| panic!("{} is listed but unreadable", setting.name));
            assert_eq!(
                actual, setting.default,
                "{} claims a default it does not have",
                setting.name
            );
        }
    }

    /// Every listed value must be accepted, and the round trip must give the
    /// same string back — otherwise completion would offer a value that the
    /// help then displays under another name.
    #[test]
    fn every_offered_value_is_accepted_and_reads_back() {
        let mut config = Config::default();
        for setting in ALL {
            for value in setting.kind.values() {
                write(&mut config, setting.name, value)
                    .unwrap_or_else(|e| panic!("{} = {value}: {e}", setting.name));
                assert_eq!(
                    read(&config, setting.name).as_deref(),
                    Some(value),
                    "{} did not keep {value}",
                    setting.name
                );
            }
        }
    }

    #[test]
    fn numbers_are_bounded_and_nonsense_is_refused() {
        let mut config = Config::default();
        assert!(write(&mut config, "code.tab_width", "0").is_err());
        assert!(write(&mut config, "code.tab_width", "17").is_err());
        assert!(write(&mut config, "code.tab_width", "8").is_ok());
        assert_eq!(config.code.tab_width, 8);

        assert!(write(&mut config, "table.max_column_width", "2").is_err());
        assert!(write(&mut config, "max_width", "-1").is_err());
        assert!(write(&mut config, "max_width", "99999").is_err());
        assert!(write(&mut config, "max_width", "0").is_ok());

        assert!(write(&mut config, "center", "yes").is_err());
        assert!(write(&mut config, "links.opener", "").is_err());
        let message = write(&mut config, "nope", "1").unwrap_err();
        assert_eq!(message, "unknown setting: nope");
    }

    /// An unknown theme name is a custom theme, not an error: the value is
    /// kept so a `[themes]` file this build does not ship can still be named.
    #[test]
    fn the_theme_accepts_names_it_does_not_know() {
        let mut config = Config::default();
        assert_eq!(
            write(&mut config, "theme", "solarized"),
            Ok(Effect::Palette)
        );
        assert_eq!(read(&config, "theme").as_deref(), Some("solarized"));
    }

    /// The structured keys behave like every other key: the values the help
    /// offers are taken, nonsense is refused by name, and the indent is
    /// bounded so that a stray `0` cannot flatten the hierarchy.
    #[test]
    fn structured_keys_take_the_values_they_advertise() {
        let mut config = Config::default();
        assert_eq!(read(&config, "structured.indent").as_deref(), Some("2"));

        assert!(write(&mut config, "structured.indent", "4").is_ok());
        assert_eq!(read(&config, "structured.indent").as_deref(), Some("4"));
        assert!(write(&mut config, "structured.indent", "0").is_err());
        assert!(write(&mut config, "structured.indent", "17").is_err());
        assert!(write(&mut config, "structured.indent", "wide").is_err());
        assert_eq!(
            read(&config, "structured.indent").as_deref(),
            Some("4"),
            "a refused value leaves the old one in place"
        );

        assert!(write(&mut config, "structured.path", "never").is_ok());
        assert_eq!(read(&config, "structured.path").as_deref(), Some("never"));
        let message = write(&mut config, "structured.path", "sometimes").unwrap_err();
        assert!(message.contains("structured.path"), "{message}");
        assert!(message.contains("always"), "{message}");

        assert!(write(&mut config, "structured.show_indices", "false").is_ok());
        assert!(!config.structured.show_indices);
        assert!(write(&mut config, "structured.collapsed_summary", "no").is_err());
    }

    /// `format` is settable and readable, but it describes how a document is
    /// read rather than how it looks, so it may not claim a layout effect.
    #[test]
    fn the_format_key_takes_every_format_name() {
        let mut config = Config::default();
        assert_eq!(read(&config, "format").as_deref(), Some("auto"));
        for value in ["markdown", "json", "yaml", "auto"] {
            assert_eq!(write(&mut config, "format", value), Ok(Effect::Redraw));
            assert_eq!(read(&config, "format").as_deref(), Some(value));
        }
        assert!(write(&mut config, "format", "toml").is_err());
    }

    #[test]
    fn completion_matches_by_prefix() {
        assert_eq!(matching("cent"), vec!["center"]);
        assert_eq!(
            matching("code."),
            vec!["code.wrap", "code.line_numbers", "code.tab_width"]
        );
        assert!(matching("zzz").is_empty());
        assert_eq!(matching("").len(), ALL.len());
    }

    #[test]
    fn the_help_covers_every_key() {
        let entries = help_entries();
        assert_eq!(entries.len(), ALL.len());
        for (entry, setting) in entries.iter().zip(ALL) {
            assert_eq!(entry.0, setting.name);
            assert!(entry.1.contains(setting.default), "{}", setting.name);
        }
    }
}
