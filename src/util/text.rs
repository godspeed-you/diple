//! Making document text safe to put on a terminal.
//!
//! diple decides what escape sequences the terminal sees; the document does
//! not. A JSON string, a YAML comment, a Markdown paragraph or a parse-error
//! excerpt can all contain an ESC, an OSC introducer, a BEL or a bidirectional
//! override, and a reader who opens an untrusted file must not thereby hand it
//! control of the screen — or of what the screen appears to say.
//!
//! The policy:
//!
//! * `\t`, `\n` and `\r` become a space. Line structure is the layout
//!   engine's decision, and a document that could emit a carriage return
//!   could overwrite the line it was drawn on.
//! * every other C0 control, DEL, and the C1 range become `U+FFFD`. They are
//!   shown rather than dropped, because a reader inspecting a suspicious file
//!   wants to see that something is there.
//! * the bidirectional overrides, embeddings, isolates and marks become
//!   `U+FFFD`. They do not move the cursor, but they can make text read in an
//!   order other than the one it is written in, which for a document viewer
//!   is the same class of problem.
//!
//! Everything else — all ordinary Unicode, combining marks, emoji, CJK — is
//! left exactly as it was.

use std::borrow::Cow;

/// What an unprintable character is replaced with.
pub const REPLACEMENT: char = '\u{fffd}';

/// Whether `text` contains anything the terminal must not see verbatim.
///
/// The fast path: almost every span is clean, and a byte scan settles it
/// without allocating. Only the bidi controls need the character scan, and
/// they all start with the same two lead bytes.
pub fn needs_sanitizing(text: &str) -> bool {
    text.bytes()
        .any(|b| b < 0x20 || b == 0x7f || b == 0xc2 || b == 0xe2)
        && text.chars().any(is_unsafe)
}

/// Whether a character must not reach the terminal as itself.
fn is_unsafe(c: char) -> bool {
    matches!(c,
        '\u{0}'..='\u{1f}'
        | '\u{7f}'..='\u{9f}'
        // Bidirectional marks, embeddings, overrides and isolates.
        | '\u{200e}' | '\u{200f}'
        | '\u{202a}'..='\u{202e}'
        | '\u{2066}'..='\u{2069}')
}

/// What a character is replaced with, or `None` when it is safe as it is.
fn replacement(c: char) -> Option<char> {
    match c {
        '\t' | '\n' | '\r' => Some(' '),
        c if is_unsafe(c) => Some(REPLACEMENT),
        _ => None,
    }
}

/// Replace every character the terminal must not see, borrowing when there is
/// nothing to replace.
pub fn sanitized(text: &str) -> Cow<'_, str> {
    if !needs_sanitizing(text) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match replacement(c) {
            Some(r) => out.push(r),
            None => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// [`sanitized`], always owned.
pub fn sanitize(text: &str) -> String {
    sanitized(text).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_is_borrowed_unchanged() {
        for text in ["", "hello", "Grüße", "本 🎵", "a-b_c/d:e"] {
            assert!(!needs_sanitizing(text), "{text:?}");
            assert!(matches!(sanitized(text), Cow::Borrowed(_)), "{text:?}");
            assert_eq!(sanitize(text), text);
        }
    }

    #[test]
    fn escape_sequences_cannot_survive() {
        let hostile = "\u{1b}[31mred\u{1b}]0;title\u{7}\u{1b}]8;;http://x\u{1b}\\link";
        let clean = sanitize(hostile);
        assert!(!clean.contains('\u{1b}'), "{clean:?}");
        assert!(!clean.contains('\u{7}'), "{clean:?}");
        // The text is still readable; only the controls went.
        assert!(clean.contains("red"));
        assert!(clean.contains("link"));
    }

    #[test]
    fn line_structure_belongs_to_the_layout_engine() {
        assert_eq!(sanitize("a\tb"), "a b");
        assert_eq!(sanitize("a\nb"), "a b");
        assert_eq!(sanitize("overwrite\rme"), "overwrite me");
    }

    #[test]
    fn c1_controls_and_del_are_replaced() {
        assert_eq!(sanitize("a\u{7f}b"), "a\u{fffd}b");
        assert_eq!(
            sanitize("a\u{9b}b"),
            "a\u{fffd}b",
            "CSI as a single C1 byte"
        );
        assert_eq!(sanitize("a\u{0}b"), "a\u{fffd}b");
    }

    #[test]
    fn bidirectional_overrides_cannot_reorder_what_is_shown() {
        for c in ['\u{202e}', '\u{202d}', '\u{2066}', '\u{2069}', '\u{200f}'] {
            let text = format!("safe{c}evil");
            assert!(needs_sanitizing(&text), "{c:?}");
            assert_eq!(sanitize(&text), "safe\u{fffd}evil", "{c:?}");
        }
        // Zero-width joiners are ordinary text and must survive.
        assert!(!needs_sanitizing("a\u{200d}b"));
        assert_eq!(sanitize("\u{200b}"), "\u{200b}");
    }

    #[test]
    fn sanitizing_is_idempotent() {
        let once = sanitize("\u{1b}[0m\tx\u{202e}");
        assert_eq!(sanitize(&once), once);
    }
}
