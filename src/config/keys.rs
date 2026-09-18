//! Key-spec parsing and multi-key binding resolution.
//!
//! A key spec is a whitespace-separated list of tokens. Each token is either
//! a named key (`enter`, `esc`, `space`, `tab`, `pgdn`, `f1`, …), a
//! modifier chord (`ctrl-d`, `shift-tab`, `alt-x`), a single character
//! (`q`, `/`), or a run of characters that forms a multi-key sequence
//! (`za`, `zM`).

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::config::actions::Action;
use crate::config::schema::KeyBinding;

/// A normalized key: code + modifiers (SHIFT folded into the char).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    /// Key code.
    pub code: KeyCode,
    /// Modifiers (never contains SHIFT for character keys).
    pub mods: KeyModifiers,
}

impl Key {
    /// Plain character key.
    pub fn char(c: char) -> Key {
        Key {
            code: KeyCode::Char(c),
            mods: KeyModifiers::NONE,
        }
    }

    /// Character with CTRL.
    pub fn ctrl(c: char) -> Key {
        Key {
            code: KeyCode::Char(c),
            mods: KeyModifiers::CONTROL,
        }
    }

    /// Modifier-less special key.
    pub fn code(code: KeyCode) -> Key {
        Key {
            code,
            mods: KeyModifiers::NONE,
        }
    }

    /// Normalize a crossterm event: fold SHIFT into characters, map
    /// `Shift-Tab` to `BackTab`, drop the SHIFT modifier on BackTab.
    pub fn from_event(event: &KeyEvent) -> Key {
        let mut code = event.code;
        let mut mods = event.modifiers;
        if code == KeyCode::Tab && mods.contains(KeyModifiers::SHIFT) {
            code = KeyCode::BackTab;
        }
        if matches!(code, KeyCode::Char(_) | KeyCode::BackTab) {
            mods.remove(KeyModifiers::SHIFT);
        }
        Key { code, mods }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.contains(KeyModifiers::CONTROL) {
            write!(f, "ctrl-")?;
        }
        if self.mods.contains(KeyModifiers::ALT) {
            write!(f, "alt-")?;
        }
        if self.mods.contains(KeyModifiers::SHIFT) && !matches!(self.code, KeyCode::Char(_)) {
            write!(f, "shift-")?;
        }
        match self.code {
            KeyCode::Char(' ') => write!(f, "space"),
            KeyCode::Char(c) => write!(f, "{c}"),
            KeyCode::Enter => write!(f, "enter"),
            KeyCode::Esc => write!(f, "esc"),
            KeyCode::Tab => write!(f, "tab"),
            KeyCode::BackTab => write!(f, "shift-tab"),
            KeyCode::Backspace => write!(f, "backspace"),
            KeyCode::Up => write!(f, "up"),
            KeyCode::Down => write!(f, "down"),
            KeyCode::Left => write!(f, "left"),
            KeyCode::Right => write!(f, "right"),
            KeyCode::PageDown => write!(f, "pgdn"),
            KeyCode::PageUp => write!(f, "pgup"),
            KeyCode::Home => write!(f, "home"),
            KeyCode::End => write!(f, "end"),
            KeyCode::Insert => write!(f, "insert"),
            KeyCode::Delete => write!(f, "del"),
            KeyCode::F(n) => write!(f, "f{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// Error produced for an unparsable key spec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid key spec `{spec}`: {reason}")]
pub struct KeySpecError {
    /// The spec as written.
    pub spec: String,
    /// Why it failed.
    pub reason: String,
}

fn named_key(token: &str) -> Option<Key> {
    let key = match token {
        "enter" | "return" | "cr" => Key::code(KeyCode::Enter),
        "esc" | "escape" => Key::code(KeyCode::Esc),
        "space" => Key::char(' '),
        "tab" => Key::code(KeyCode::Tab),
        "backtab" => Key::code(KeyCode::BackTab),
        "backspace" | "bs" => Key::code(KeyCode::Backspace),
        "up" => Key::code(KeyCode::Up),
        "down" => Key::code(KeyCode::Down),
        "left" => Key::code(KeyCode::Left),
        "right" => Key::code(KeyCode::Right),
        "pgdn" | "pagedown" | "page-down" => Key::code(KeyCode::PageDown),
        "pgup" | "pageup" | "page-up" => Key::code(KeyCode::PageUp),
        "home" => Key::code(KeyCode::Home),
        "end" => Key::code(KeyCode::End),
        "insert" | "ins" => Key::code(KeyCode::Insert),
        "delete" | "del" => Key::code(KeyCode::Delete),
        _ => {
            let n: u8 = token.strip_prefix('f')?.parse().ok()?;
            if (1..=24).contains(&n) {
                Key::code(KeyCode::F(n))
            } else {
                return None;
            }
        }
    };
    Some(key)
}

fn modifier(token: &str) -> Option<KeyModifiers> {
    match token {
        "ctrl" | "control" | "c" => Some(KeyModifiers::CONTROL),
        "alt" | "meta" | "m" | "a" => Some(KeyModifiers::ALT),
        "shift" | "s" => Some(KeyModifiers::SHIFT),
        _ => None,
    }
}

/// Parse one token into exactly one key, if possible.
fn parse_token_single(token: &str) -> Option<Key> {
    let lower = token.to_lowercase();
    if let Some(key) = named_key(&lower) {
        return Some(key);
    }
    let mut chars = token.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(Key::char(c));
    }
    // Modifier chord: `ctrl-d`, `alt+enter`, `ctrl-shift-f5`.
    if token.len() > 1 && token[1..].contains(['-', '+']) {
        let parts: Vec<&str> = lower.split(['-', '+']).collect();
        let (last, mods_parts) = parts.split_last()?;
        if mods_parts.is_empty() {
            return None;
        }
        let mut mods = KeyModifiers::NONE;
        for p in mods_parts {
            mods |= modifier(p)?;
        }
        let mut base = if let Some(k) = named_key(last) {
            k
        } else {
            let mut cs = last.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => Key::char(c),
                _ => return None,
            }
        };
        base.mods |= mods;
        // Normalize like from_event.
        if base.code == KeyCode::Tab && base.mods.contains(KeyModifiers::SHIFT) {
            base.code = KeyCode::BackTab;
        }
        if let KeyCode::Char(c) = base.code {
            if base.mods.contains(KeyModifiers::SHIFT) {
                if c.is_alphabetic() {
                    base.code = KeyCode::Char(c.to_uppercase().next().unwrap_or(c));
                }
                base.mods.remove(KeyModifiers::SHIFT);
            }
        }
        if base.code == KeyCode::BackTab {
            base.mods.remove(KeyModifiers::SHIFT);
        }
        return Some(base);
    }
    None
}

/// Parse a key spec into a sequence of keys.
pub fn parse_key_spec(spec: &str) -> Result<Vec<Key>, KeySpecError> {
    let err = |reason: &str| KeySpecError {
        spec: spec.to_string(),
        reason: reason.to_string(),
    };
    if spec.trim().is_empty() {
        return Err(err("empty key spec"));
    }
    let mut keys = Vec::new();
    for token in spec.split_whitespace() {
        if let Some(key) = parse_token_single(token) {
            keys.push(key);
        } else if token.contains(['-', '+']) && token.len() > 1 {
            return Err(err("unknown key or modifier name"));
        } else {
            // Multi-character sequence: one key per character (`za`, `zM`).
            keys.extend(token.chars().map(Key::char));
        }
    }
    if keys.is_empty() {
        Err(err("no keys in spec"))
    } else {
        Ok(keys)
    }
}

/// Result of feeding one key event into a [`KeyMap`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMatch {
    /// A complete binding matched.
    Action(Action),
    /// The keys so far are a prefix of at least one binding.
    Pending,
    /// No binding matches.
    None,
}

/// Resolves key event sequences to actions, handling multi-key prefixes.
#[derive(Debug, Clone)]
pub struct KeyMap {
    bindings: Vec<(Vec<Key>, Action)>,
    pending: Vec<Key>,
}

impl Default for KeyMap {
    fn default() -> Self {
        Self::with_defaults()
    }
}

/// Default bindings, including link cycling, help, mermaid source, half-page
/// and horizontal scrolling.
const DEFAULT_BINDINGS: &[(Action, &[&str])] = &[
    (Action::Quit, &["q", "ctrl-c"]),
    (Action::Cancel, &["esc"]),
    (Action::ScrollDown, &["j", "down"]),
    (Action::ScrollUp, &["k", "up"]),
    (Action::PageDown, &["pgdn", "space"]),
    (Action::PageUp, &["pgup", "b"]),
    (Action::HalfPageDown, &["ctrl-d"]),
    (Action::HalfPageUp, &["ctrl-u"]),
    (Action::ScrollLeft, &["h", "left"]),
    (Action::ScrollRight, &["l", "right"]),
    (Action::Top, &["g"]),
    (Action::Bottom, &["G"]),
    (Action::Search, &["/"]),
    (Action::NextSearch, &["n"]),
    (Action::PreviousSearch, &["N"]),
    (Action::NextHeading, &["]"]),
    (Action::PreviousHeading, &["["]),
    (Action::NextHeadingSameLevel, &["}"]),
    (Action::PreviousHeadingSameLevel, &["{"]),
    (Action::ToggleToc, &["t"]),
    (Action::ToggleKeyHints, &["K"]),
    (Action::ToggleMouse, &["m"]),
    (Action::FocusOtherPane, &["ctrl-w"]),
    (Action::NextTab, &["ctrl-n"]),
    (Action::PreviousTab, &["ctrl-p"]),
    (Action::Activate, &["enter"]),
    (Action::OpenLink, &["o"]),
    (Action::NextLink, &["tab"]),
    (Action::PreviousLink, &["shift-tab"]),
    (Action::ToggleFold, &["za"]),
    (Action::CollapseFold, &["zc"]),
    (Action::ExpandFold, &["zo"]),
    (Action::CollapseAll, &["zM"]),
    (Action::ExpandAll, &["zR"]),
    (Action::Help, &["?", "f1"]),
    (Action::CommandPrompt, &[":"]),
    (Action::ToggleMermaidSource, &["s"]),
    // `h` and `l` scroll sideways; their shifted forms move through the
    // hierarchy, which is the same axis one level up. Neither key was bound,
    // so no existing binding changes meaning.
    (Action::ParentNode, &["H"]),
    (Action::FirstChild, &["L"]),
];

impl KeyMap {
    /// The default key map.
    pub fn with_defaults() -> Self {
        let mut bindings = Vec::new();
        for (action, specs) in DEFAULT_BINDINGS {
            for spec in *specs {
                if let Ok(keys) = parse_key_spec(spec) {
                    bindings.push((keys, *action));
                }
            }
        }
        Self {
            bindings,
            pending: Vec::new(),
        }
    }

    /// Defaults plus `[keys]` overrides. An entry replaces *all* default
    /// bindings for its action. Errors carry the offending action and value.
    pub fn from_overrides<'a, I>(overrides: I) -> Result<Self, KeyOverrideError>
    where
        I: IntoIterator<Item = (&'a str, &'a KeyBinding)>,
    {
        let mut map = Self::with_defaults();
        for (name, binding) in overrides {
            let Some(action) = Action::from_name(name) else {
                return Err(KeyOverrideError {
                    action: name.to_string(),
                    value: binding.specs().join(", "),
                    reason: format!(
                        "unknown action `{name}` (expected one of: {})",
                        Action::ALL
                            .iter()
                            .map(|a| a.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                });
            };
            map.bindings.retain(|(_, a)| *a != action);
            for spec in binding.specs() {
                let keys = parse_key_spec(spec).map_err(|e| KeyOverrideError {
                    action: name.to_string(),
                    value: spec.to_string(),
                    reason: e.to_string(),
                })?;
                map.bindings.push((keys, action));
            }
        }
        Ok(map)
    }

    /// Feed one key event; resolves multi-key sequences.
    pub fn feed(&mut self, event: &KeyEvent) -> KeyMatch {
        if event.kind == KeyEventKind::Release {
            return if self.pending.is_empty() {
                KeyMatch::None
            } else {
                KeyMatch::Pending
            };
        }
        let key = Key::from_event(event);
        self.pending.push(key);
        match self.lookup() {
            KeyMatch::None if self.pending.len() > 1 => {
                // Failed sequence: retry with just the new key.
                self.pending = vec![key];
                let result = self.lookup();
                if result == KeyMatch::None {
                    self.pending.clear();
                }
                result
            }
            KeyMatch::None => {
                self.pending.clear();
                KeyMatch::None
            }
            other => other,
        }
    }

    fn lookup(&mut self) -> KeyMatch {
        let mut prefix = false;
        for (keys, action) in &self.bindings {
            if keys.as_slice() == self.pending.as_slice() {
                self.pending.clear();
                return KeyMatch::Action(*action);
            }
            if keys.len() > self.pending.len() && keys.starts_with(&self.pending) {
                prefix = true;
            }
        }
        if prefix {
            KeyMatch::Pending
        } else {
            KeyMatch::None
        }
    }

    /// Discard a pending multi-key prefix.
    pub fn reset(&mut self) {
        self.pending.clear();
    }

    /// The pending prefix (for the status line).
    pub fn pending(&self) -> &[Key] {
        &self.pending
    }

    /// Display strings of the bindings for an action (help overlay).
    pub fn bindings_for(&self, action: Action) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|(_, a)| *a == action)
            .map(|(keys, _)| keys.iter().map(Key::to_string).collect::<Vec<_>>().join(""))
            .collect()
    }
}

/// Error for an invalid `[keys]` override; the loader adds the file path.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid binding for `keys.{action}` = `{value}`: {reason}")]
pub struct KeyOverrideError {
    /// Action name as written in the config.
    pub action: String,
    /// Offending value.
    pub value: String,
    /// Explanation, includes the expected form.
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn parse_single_keys() {
        assert_eq!(parse_key_spec("q").unwrap(), vec![Key::char('q')]);
        assert_eq!(parse_key_spec("/").unwrap(), vec![Key::char('/')]);
        assert_eq!(parse_key_spec("?").unwrap(), vec![Key::char('?')]);
        assert_eq!(parse_key_spec("-").unwrap(), vec![Key::char('-')]);
        assert_eq!(parse_key_spec("G").unwrap(), vec![Key::char('G')]);
        assert_eq!(
            parse_key_spec("enter").unwrap(),
            vec![Key::code(KeyCode::Enter)]
        );
        assert_eq!(
            parse_key_spec("esc").unwrap(),
            vec![Key::code(KeyCode::Esc)]
        );
        assert_eq!(parse_key_spec("space").unwrap(), vec![Key::char(' ')]);
        assert_eq!(
            parse_key_spec("tab").unwrap(),
            vec![Key::code(KeyCode::Tab)]
        );
        assert_eq!(
            parse_key_spec("pgdn").unwrap(),
            vec![Key::code(KeyCode::PageDown)]
        );
        assert_eq!(parse_key_spec("up").unwrap(), vec![Key::code(KeyCode::Up)]);
        assert_eq!(
            parse_key_spec("down").unwrap(),
            vec![Key::code(KeyCode::Down)]
        );
        assert_eq!(
            parse_key_spec("left").unwrap(),
            vec![Key::code(KeyCode::Left)]
        );
        assert_eq!(
            parse_key_spec("right").unwrap(),
            vec![Key::code(KeyCode::Right)]
        );
        assert_eq!(
            parse_key_spec("f1").unwrap(),
            vec![Key::code(KeyCode::F(1))]
        );
        assert_eq!(
            parse_key_spec("f12").unwrap(),
            vec![Key::code(KeyCode::F(12))]
        );
    }

    #[test]
    fn parse_modified_keys() {
        assert_eq!(parse_key_spec("ctrl-d").unwrap(), vec![Key::ctrl('d')]);
        assert_eq!(
            parse_key_spec("shift-tab").unwrap(),
            vec![Key::code(KeyCode::BackTab)],
            "shift-tab normalizes to BackTab without SHIFT"
        );
        assert_eq!(
            parse_key_spec("alt-enter").unwrap(),
            vec![Key {
                code: KeyCode::Enter,
                mods: KeyModifiers::ALT
            }]
        );
        assert_eq!(parse_key_spec("shift-g").unwrap(), vec![Key::char('G')]);
        assert_eq!(
            parse_key_spec("ctrl-shift-f5").unwrap(),
            vec![Key {
                code: KeyCode::F(5),
                mods: KeyModifiers::CONTROL | KeyModifiers::SHIFT
            }]
        );
    }

    #[test]
    fn parse_sequences() {
        assert_eq!(
            parse_key_spec("za").unwrap(),
            vec![Key::char('z'), Key::char('a')]
        );
        assert_eq!(
            parse_key_spec("zM").unwrap(),
            vec![Key::char('z'), Key::char('M')]
        );
        assert_eq!(
            parse_key_spec("g g").unwrap(),
            vec![Key::char('g'), Key::char('g')]
        );
        assert_eq!(
            parse_key_spec("z enter").unwrap(),
            vec![Key::char('z'), Key::code(KeyCode::Enter)]
        );
    }

    #[test]
    fn parse_errors() {
        assert!(parse_key_spec("").is_err());
        assert!(parse_key_spec("   ").is_err());
        assert!(parse_key_spec("ctrl-").is_err());
        assert!(parse_key_spec("hyper-x").is_err());
        assert!(parse_key_spec("ctrl-doesnotexist").is_err());
        let e = parse_key_spec("ctrl-doesnotexist").unwrap_err();
        assert!(e.to_string().contains("ctrl-doesnotexist"));
    }

    #[test]
    fn feed_single_and_sequence() {
        let mut map = KeyMap::with_defaults();
        assert_eq!(
            map.feed(&ev(KeyCode::Char('q'), KeyModifiers::NONE)),
            KeyMatch::Action(Action::Quit)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('z'), KeyModifiers::NONE)),
            KeyMatch::Pending
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('a'), KeyModifiers::NONE)),
            KeyMatch::Action(Action::ToggleFold)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('z'), KeyModifiers::NONE)),
            KeyMatch::Pending
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('M'), KeyModifiers::SHIFT)),
            KeyMatch::Action(Action::CollapseAll),
            "shifted char events normalize"
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('x'), KeyModifiers::NONE)),
            KeyMatch::None
        );
        assert!(map.pending().is_empty());
    }

    #[test]
    fn failed_sequence_retries_last_key() {
        let mut map = KeyMap::with_defaults();
        assert_eq!(
            map.feed(&ev(KeyCode::Char('z'), KeyModifiers::NONE)),
            KeyMatch::Pending
        );
        // `zq` is not bound; `q` alone is → quit fires.
        assert_eq!(
            map.feed(&ev(KeyCode::Char('q'), KeyModifiers::NONE)),
            KeyMatch::Action(Action::Quit)
        );
    }

    #[test]
    fn special_keys_resolve() {
        let mut map = KeyMap::with_defaults();
        assert_eq!(
            map.feed(&ev(KeyCode::PageDown, KeyModifiers::NONE)),
            KeyMatch::Action(Action::PageDown)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char(' '), KeyModifiers::NONE)),
            KeyMatch::Action(Action::PageDown)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            KeyMatch::Action(Action::HalfPageDown)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            KeyMatch::Action(Action::Quit)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::BackTab, KeyModifiers::SHIFT)),
            KeyMatch::Action(Action::PreviousLink)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Tab, KeyModifiers::SHIFT)),
            KeyMatch::Action(Action::PreviousLink)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Tab, KeyModifiers::NONE)),
            KeyMatch::Action(Action::NextLink)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Up, KeyModifiers::NONE)),
            KeyMatch::Action(Action::ScrollUp)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::F(1), KeyModifiers::NONE)),
            KeyMatch::Action(Action::Help)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Esc, KeyModifiers::NONE)),
            KeyMatch::Action(Action::Cancel)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            KeyMatch::Action(Action::Bottom)
        );
    }

    #[test]
    fn overrides_replace_defaults() {
        use crate::config::schema::KeyBinding;
        let quit = KeyBinding::Many(vec!["x".into(), "ctrl-q".into()]);
        let fold = KeyBinding::One("f t".into());
        let overrides = [("quit", &quit), ("toggle_fold", &fold)];
        let mut map = KeyMap::from_overrides(overrides.iter().map(|(n, b)| (*n, *b))).unwrap();
        assert_eq!(
            map.feed(&ev(KeyCode::Char('q'), KeyModifiers::NONE)),
            KeyMatch::None,
            "default replaced"
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('x'), KeyModifiers::NONE)),
            KeyMatch::Action(Action::Quit)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            KeyMatch::Action(Action::Quit)
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('f'), KeyModifiers::NONE)),
            KeyMatch::Pending
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('t'), KeyModifiers::NONE)),
            KeyMatch::Action(Action::ToggleFold)
        );
        // `za` no longer bound: `z` is still a prefix for zc/zo/zM/zR.
        assert_eq!(
            map.feed(&ev(KeyCode::Char('z'), KeyModifiers::NONE)),
            KeyMatch::Pending
        );
        assert_eq!(
            map.feed(&ev(KeyCode::Char('a'), KeyModifiers::NONE)),
            KeyMatch::None
        );
    }

    #[test]
    fn override_errors() {
        use crate::config::schema::KeyBinding;
        let b = KeyBinding::One("q".into());
        let err = KeyMap::from_overrides([("does_not_exist", &b)]).unwrap_err();
        assert!(err.to_string().contains("does_not_exist"));
        assert!(err.reason.contains("expected one of"));
        let bad = KeyBinding::One("hyper-x".into());
        let err = KeyMap::from_overrides([("quit", &bad)]).unwrap_err();
        assert_eq!(err.value, "hyper-x");
    }

    #[test]
    fn bindings_for_display() {
        let map = KeyMap::with_defaults();
        assert_eq!(
            map.bindings_for(Action::Quit),
            vec!["q".to_string(), "ctrl-c".to_string()]
        );
        assert_eq!(map.bindings_for(Action::ToggleFold), vec!["za".to_string()]);
        assert_eq!(
            map.bindings_for(Action::PreviousLink),
            vec!["shift-tab".to_string()]
        );
    }

    #[test]
    fn all_actions_have_default_bindings() {
        let map = KeyMap::with_defaults();
        for &a in Action::ALL {
            assert!(!map.bindings_for(a).is_empty(), "{a:?} unbound");
        }
    }
}
