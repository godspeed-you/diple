//! The `:` command line: parsing, completion and the line being typed.
//!
//! Everything here is a pure function of the typed text and the settings
//! table in [`crate::config::settings`] — no terminal, no application state.
//! [`crate::app::state::App`] owns the buffer and executes what
//! [`parse`] returns, which keeps the grammar testable on its own and stops
//! the command line from growing a second idea of what a setting is.
//!
//! The one exception is `:open`, whose last argument is a path: only the
//! filesystem knows what may be typed there, so that completion is delegated
//! to [`crate::app::paths`].

use crate::app::paths;
use crate::config::settings;

/// Where `:open` puts the document it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenTarget {
    /// A new tab, which becomes the active one.
    Tab,
    /// Beside the current document, left and right.
    SideBySide,
    /// Above and below the current document.
    Stacked,
}

impl OpenTarget {
    /// The spellings accepted after `:open`, in completion order.
    ///
    /// The first of each row is the canonical one — what completion produces
    /// and what the help lists; the rest are the words readers of other
    /// pagers and of vim already have in their fingers.
    const SPELLINGS: &'static [(&'static str, OpenTarget)] = &[
        ("side-by-side", OpenTarget::SideBySide),
        ("stacked", OpenTarget::Stacked),
        ("tab", OpenTarget::Tab),
        ("vsplit", OpenTarget::SideBySide),
        ("split", OpenTarget::Stacked),
    ];

    /// The canonical names, for completion and error messages.
    pub(crate) fn names() -> Vec<&'static str> {
        Self::SPELLINGS
            .iter()
            .take(3)
            .map(|(name, _)| *name)
            .collect()
    }

    fn parse(word: &str) -> Option<OpenTarget> {
        Self::SPELLINGS
            .iter()
            .find(|(name, _)| *name == word)
            .map(|(_, target)| *target)
    }
}

/// What `:open` says when it was not given both of its arguments.
const OPEN_USAGE: &str = "usage: open <side-by-side|stacked|tab> <path>";

/// The command words `:` knows, beside every setting name.
///
/// Completed like a setting name, but they take no `=`, so completion gives
/// them a space instead of a separator.
const COMMANDS: &[&str] = &["open", "close", "reload", "help", "quit", "qall"];

/// What a typed line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    /// Nothing was typed.
    Empty,
    /// Show every setting with its values and default (`:help`).
    Help,
    /// Close the focused view; the last one leaves (`:q`, `:quit`).
    Quit,
    /// Leave, whatever is open (`:qa`, `:qall`).
    QuitAll,
    /// Close the focused view, but never leave (`:close`).
    Close,
    /// Open another document (`:open tab notes.md`).
    Open(OpenTarget, String),
    /// Read the focused document again from its file (`:reload`, `:e`).
    Reload,
    /// A command that was recognised but not usable as typed; the string is
    /// the message to show.
    Invalid(String),
    /// Report the current value of a key (`:center`).
    Show(String),
    /// Assign a value (`:center = false`, `:center false`).
    Set(String, String),
    /// Not a command at all.
    Unknown(String),
}

/// Parse a typed line.
///
/// The separator may be `=` or plain whitespace, because both read naturally:
/// `:center = true` mirrors the configuration file, `:theme crt` mirrors every
/// other pager. A leading `set` is accepted for the muscle memory it comes
/// from, and ignored.
pub(crate) fn parse(line: &str) -> Command {
    let line = line.trim();
    if line.is_empty() {
        return Command::Empty;
    }
    let line = line.strip_prefix("set ").map_or(line, str::trim);
    match line {
        "help" | "h" | "?" => return Command::Help,
        "q" | "quit" => return Command::Quit,
        "qa" | "qall" | "quitall" => return Command::QuitAll,
        "close" => return Command::Close,
        // vim's `:e` with no file name reads the current one again; diple has
        // no other use for the word, so the short spelling is free to take.
        "reload" | "e" | "edit" => return Command::Reload,
        _ => {}
    }
    if let Some(rest) = strip_word(line, "open") {
        return parse_open(rest);
    }

    let (key, value) = split(line);
    let key = key.to_string();
    match value {
        // A bare key is a question, not an assignment: `:center` says what
        // centring is currently doing rather than turning it on.
        None if settings::get(&key).is_some() => Command::Show(key),
        None => Command::Unknown(key),
        Some(value) => Command::Set(key, value.to_string()),
    }
}

/// `line` without a leading `word`, when it starts with that word followed by
/// whitespace or nothing at all.
///
/// `:opener` must not parse as `:open er`, which is why this asks for the
/// separator rather than for a prefix.
fn strip_word<'a>(line: &'a str, word: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(word)?;
    if rest.is_empty() {
        return Some(rest);
    }
    rest.starts_with(char::is_whitespace)
        .then(|| rest.trim_start())
}

/// Parse what follows `:open`.
///
/// The path is everything after the target word, untrimmed at its end only by
/// the outer trim: a file name may contain spaces, so it is not a token.
fn parse_open(rest: &str) -> Command {
    let (word, path) = match rest.split_once(char::is_whitespace) {
        Some((word, path)) => (word, path.trim()),
        None => (rest, ""),
    };
    let Some(target) = OpenTarget::parse(word) else {
        return Command::Invalid(OPEN_USAGE.to_string());
    };
    if path.is_empty() {
        return Command::Invalid(OPEN_USAGE.to_string());
    }
    Command::Open(target, path.to_string())
}

/// Split a line into its key and, if there is one, its value.
fn split(line: &str) -> (&str, Option<&str>) {
    if let Some((key, value)) = line.split_once('=') {
        return (key.trim(), Some(value.trim()));
    }
    match line.split_once(char::is_whitespace) {
        Some((key, value)) => (key.trim(), Some(value.trim())),
        None => (line, None),
    }
}

/// The result of pressing Tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Completion {
    /// The line after completing as far as it is unambiguous.
    pub(crate) line: String,
    /// What is still possible, when more than one thing is.
    pub(crate) candidates: Vec<String>,
}

/// Complete the last token of `line`.
///
/// Completion never guesses past the point where the answer stops being
/// unique: it extends the token by the longest prefix every candidate shares
/// and hands the rest back as candidates for the status line. A key that
/// completes to exactly one match gains its separator, so the next keystroke
/// is the value rather than a space.
pub(crate) fn complete(line: &str) -> Completion {
    let unchanged = |candidates: Vec<String>| Completion {
        line: line.to_string(),
        candidates,
    };
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];

    // `:open` has a grammar of its own — a target word and then a path — so
    // it takes over completion as soon as its name is typed.
    if let Some(rest) = strip_word(trimmed, "open") {
        return complete_open(indent, rest);
    }

    // A value is being typed once a separator is there, even with nothing
    // after it: `:theme ` completes the theme names, not the key names.
    let (key, value) = split_for_completion(trimmed);
    let Some(partial) = value else {
        // Command words complete beside the setting names: from the reader's
        // side `:close` and `:center` are both just things `:` accepts, and
        // one list is what makes them discoverable.
        let commands: Vec<&'static str> = COMMANDS
            .iter()
            .copied()
            .filter(|name| name.starts_with(key))
            .collect();
        let mut names = settings::matching(key);
        names.extend(commands.iter().copied());
        if names.is_empty() {
            return unchanged(Vec::new());
        }
        let shared = common_prefix(&names);
        // A unique setting gains its separator, because the next keystroke is
        // its value. A unique command gains a space: it takes arguments, or
        // nothing at all, but never an `=`.
        let line = match (names.len(), commands.len()) {
            (1, 1) => format!("{indent}{shared} "),
            (1, _) => format!("{indent}{shared} = "),
            _ => format!("{indent}{shared}"),
        };
        return Completion {
            line,
            candidates: if names.len() == 1 {
                Vec::new()
            } else {
                names.iter().map(|n| (*n).to_string()).collect()
            },
        };
    };

    let Some(setting) = settings::get(key) else {
        return unchanged(Vec::new());
    };
    let values: Vec<&str> = setting
        .kind
        .values()
        .into_iter()
        .filter(|v| v.starts_with(partial))
        .collect();
    if values.is_empty() {
        // Free text and numbers have nothing to offer, so say what the key
        // takes instead of completing to nothing.
        return unchanged(vec![setting.kind.describe()]);
    }
    let shared = common_prefix(&values);
    Completion {
        line: format!("{indent}{key} = {shared}"),
        candidates: if values.len() == 1 {
            Vec::new()
        } else {
            values.iter().map(|v| (*v).to_string()).collect()
        },
    }
}

/// Complete `:open`, whose two arguments are a target word and a path.
///
/// `rest` is everything after the command name, already trimmed on the left.
fn complete_open(indent: &str, rest: &str) -> Completion {
    let targets = OpenTarget::names();
    let Some((word, partial)) = rest.split_once(char::is_whitespace) else {
        // Still typing the target.
        let matches: Vec<&str> = targets
            .iter()
            .copied()
            .filter(|name| name.starts_with(rest))
            .collect();
        if matches.is_empty() {
            // Nothing matches what is typed, so say what the word may be
            // rather than completing to nothing.
            return Completion {
                line: format!("{indent}open {rest}"),
                candidates: targets.iter().map(|n| (*n).to_string()).collect(),
            };
        }
        let shared = common_prefix(&matches);
        return Completion {
            line: if matches.len() == 1 {
                format!("{indent}open {shared} ")
            } else {
                format!("{indent}open {shared}")
            },
            candidates: if matches.len() == 1 {
                Vec::new()
            } else {
                matches.iter().map(|n| (*n).to_string()).collect()
            },
        };
    };
    if !targets.contains(&word) && OpenTarget::parse(word).is_none() {
        return Completion {
            line: format!("{indent}open {rest}"),
            candidates: targets.iter().map(|n| (*n).to_string()).collect(),
        };
    }
    let (completed, candidates) = paths::complete(partial.trim_start());
    Completion {
        line: format!("{indent}open {word} {completed}"),
        candidates,
    }
}

/// Like [`split`], but a trailing separator counts as an empty value so that
/// completion switches from keys to values as soon as one is typed.
fn split_for_completion(line: &str) -> (&str, Option<&str>) {
    let line = line.strip_prefix("set ").map_or(line, str::trim_start);
    if let Some((key, value)) = line.split_once('=') {
        return (key.trim(), Some(value.trim_start()));
    }
    match line.split_once(char::is_whitespace) {
        Some((key, value)) => (key.trim(), Some(value.trim_start())),
        None => (line, None),
    }
}

/// The longest prefix every candidate shares.
fn common_prefix(items: &[&str]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let mut end = first.len();
    for item in &items[1..] {
        end = end.min(
            first
                .char_indices()
                .zip(item.char_indices())
                .take_while(|((_, a), (_, b))| a == b)
                .map(|((i, c), _)| i + c.len_utf8())
                .last()
                .unwrap_or(0),
        );
    }
    first[..end].to_string()
}

/// The `:` line as it is being typed.
#[derive(Debug, Clone, Default)]
pub(crate) struct CommandState {
    /// The text after the `:`.
    pub(crate) line: String,
    /// Completion candidates from the last Tab, shown until the next key.
    pub(crate) candidates: Vec<String>,
}

impl CommandState {
    /// Start a new, empty line.
    pub(crate) fn open(&mut self) {
        self.line.clear();
        self.candidates.clear();
    }

    /// The prompt as drawn, including the leading `:`.
    pub(crate) fn prompt(&self) -> String {
        format!(":{}", self.line)
    }

    /// Type one character.
    pub(crate) fn push(&mut self, c: char) {
        self.line.push(c);
        self.candidates.clear();
    }

    /// Delete the character before the cursor. Returns `false` when the line
    /// was already empty, which closes the prompt the way it opened.
    pub(crate) fn backspace(&mut self) -> bool {
        self.candidates.clear();
        self.line.pop().is_some()
    }

    /// Complete the line in place, keeping any remaining candidates.
    pub(crate) fn complete(&mut self) {
        let done = complete(&self.line);
        self.line = done.line;
        self.candidates = done.candidates;
    }

    /// The candidates as one status line, cut to a sensible length.
    pub(crate) fn candidate_hint(&self) -> Option<String> {
        if self.candidates.is_empty() {
            return None;
        }
        const SHOWN: usize = 6;
        let mut hint = self
            .candidates
            .iter()
            .take(SHOWN)
            .cloned()
            .collect::<Vec<_>>()
            .join("  ");
        if self.candidates.len() > SHOWN {
            hint.push_str(&format!("  … +{}", self.candidates.len() - SHOWN));
        }
        Some(hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_separators_assign_and_a_bare_key_asks() {
        assert_eq!(
            parse("center = false"),
            Command::Set("center".into(), "false".into())
        );
        assert_eq!(
            parse("center false"),
            Command::Set("center".into(), "false".into())
        );
        assert_eq!(
            parse("set max_width=100"),
            Command::Set("max_width".into(), "100".into())
        );
        assert_eq!(parse("  center  "), Command::Show("center".into()));
        assert_eq!(parse("nonsense"), Command::Unknown("nonsense".into()));
        assert_eq!(parse("   "), Command::Empty);
        assert_eq!(parse("help"), Command::Help);
        assert_eq!(parse("?"), Command::Help);
        assert_eq!(parse("q"), Command::Quit);
    }

    /// A value may contain spaces and an `=`: an opener is a command line,
    /// not a word.
    #[test]
    fn a_value_keeps_what_it_contains() {
        assert_eq!(
            parse("links.opener = my-open --arg=1"),
            Command::Set("links.opener".into(), "my-open --arg=1".into())
        );
    }

    #[test]
    fn keys_complete_to_the_shared_prefix_and_then_to_the_separator() {
        // Several matches: only the shared prefix, and the rest is offered.
        let c = complete("c");
        assert_eq!(c.line, "c");
        assert!(c.candidates.contains(&"center".to_string()));
        assert!(c.candidates.contains(&"color".to_string()));

        let c = complete("ce");
        assert_eq!(c.line, "center = ", "unique keys gain the separator");
        assert!(c.candidates.is_empty());

        let c = complete("code.");
        assert_eq!(c.line, "code.", "the shared prefix is already typed");
        assert_eq!(c.candidates.len(), 3);

        assert_eq!(complete("zzz").line, "zzz", "nothing to complete");
    }

    #[test]
    fn values_complete_once_a_separator_is_there() {
        let c = complete("theme ");
        assert_eq!(c.line, "theme = ");
        assert_eq!(
            c.candidates,
            vec!["auto", "dark", "light", "crt", "cyberpunk"]
        );

        // Two themes start with `c`, so completion stops at what they share.
        let c = complete("theme = c");
        assert_eq!(c.line, "theme = c");
        assert_eq!(c.candidates, vec!["crt", "cyberpunk"]);
        let c = complete("theme = cy");
        assert_eq!(c.line, "theme = cyberpunk");
        assert!(c.candidates.is_empty());

        let c = complete("theme = d");
        assert_eq!(c.line, "theme = dark");
        assert!(c.candidates.is_empty());

        let c = complete("center = ");
        assert_eq!(c.line, "center = ");
        assert_eq!(c.candidates, vec!["true", "false"]);

        let c = complete("center = t");
        assert_eq!(c.line, "center = true");

        // A key that takes free text says so rather than completing.
        let c = complete("links.opener = ");
        assert_eq!(c.line, "links.opener = ");
        assert_eq!(c.candidates, vec!["<text>"]);
    }

    #[test]
    fn the_state_tracks_typing_and_drops_stale_candidates() {
        let mut state = CommandState::default();
        state.open();
        for c in "ce".chars() {
            state.push(c);
        }
        state.complete();
        assert_eq!(state.prompt(), ":center = ");
        state.complete();
        assert_eq!(state.candidates, vec!["true", "false"]);
        state.push('t');
        assert!(
            state.candidates.is_empty(),
            "typing invalidates the last Tab"
        );
        state.complete();
        assert_eq!(state.line, "center = true");

        while state.backspace() {}
        assert!(!state.backspace(), "an empty line reports it");
    }

    #[test]
    fn open_takes_a_target_and_a_path_and_says_so_when_it_does_not() {
        assert_eq!(
            parse("open tab notes.md"),
            Command::Open(OpenTarget::Tab, "notes.md".into())
        );
        assert_eq!(
            parse("open side-by-side ../a b.md"),
            Command::Open(OpenTarget::SideBySide, "../a b.md".into()),
            "a path may contain spaces"
        );
        assert_eq!(
            parse("open vsplit x.md"),
            Command::Open(OpenTarget::SideBySide, "x.md".into()),
            "vim's word for it works too"
        );
        assert_eq!(
            parse("open split x.md"),
            Command::Open(OpenTarget::Stacked, "x.md".into())
        );
        assert!(matches!(parse("open tab"), Command::Invalid(_)), "no path");
        assert!(
            matches!(parse("open x.md"), Command::Invalid(_)),
            "no target"
        );
        assert!(matches!(parse("open"), Command::Invalid(_)));
        // A setting whose name merely starts with `open` is still a setting.
        assert_eq!(
            parse("links.opener = xdg-open"),
            Command::Set("links.opener".into(), "xdg-open".into())
        );
    }

    #[test]
    fn the_words_that_close_things_are_told_apart() {
        assert_eq!(parse("close"), Command::Close);
        assert_eq!(parse("qa"), Command::QuitAll);
        assert_eq!(parse("qall"), Command::QuitAll);
        assert_eq!(parse("q"), Command::Quit);
    }

    #[test]
    fn reload_has_vims_spellings_too() {
        assert_eq!(parse("reload"), Command::Reload);
        assert_eq!(parse("e"), Command::Reload);
        assert_eq!(parse("edit"), Command::Reload);
        assert_eq!(complete("rel").line, "reload ");
    }

    #[test]
    fn commands_complete_beside_the_settings_and_take_a_space_not_a_separator() {
        let c = complete("op");
        assert_eq!(c.line, "open ", "a unique command gains a space");
        assert!(c.candidates.is_empty());

        let c = complete("clo");
        assert_eq!(c.line, "close ");

        // `c` matches settings and a command; both are offered.
        let c = complete("c");
        assert!(c.candidates.contains(&"center".to_string()));
        assert!(c.candidates.contains(&"close".to_string()));

        let c = complete("open ");
        assert_eq!(c.line, "open ", "the targets share no prefix");
        assert_eq!(c.candidates, vec!["side-by-side", "stacked", "tab"]);

        let c = complete("open ta");
        assert_eq!(c.line, "open tab ", "a unique target gains its space");
        assert!(c.candidates.is_empty());

        let c = complete("open s");
        assert_eq!(c.line, "open s");
        assert_eq!(c.candidates, vec!["side-by-side", "stacked"]);

        let c = complete("open nonsense ");
        assert_eq!(
            c.candidates,
            vec!["side-by-side", "stacked", "tab"],
            "a target nobody knows is answered with the ones that exist"
        );
    }

    #[test]
    fn the_last_argument_of_open_completes_against_the_filesystem() {
        let dir = std::env::temp_dir().join(format!("diple-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("readme.md"), "# r").expect("write");
        let base = dir.display().to_string();

        let c = complete(&format!("open tab {base}/re"));
        assert_eq!(c.line, format!("open tab {base}/readme.md"));
        assert!(c.candidates.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_candidate_hint_is_bounded() {
        let mut state = CommandState::default();
        state.complete();
        let hint = state.candidate_hint().expect("every key matches");
        assert!(hint.contains("theme"));
        assert!(hint.contains("… +"), "a long list is summarised: {hint}");
    }
}
