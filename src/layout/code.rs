//! Code block layout: syntect highlighting, gutter, line numbers, tab
//! expansion, and wrap vs. horizontal-scroll modes.
//!
//! The syntax and theme sets are loaded exactly once per process
//! ([`std::sync::OnceLock`]) because loading them dominates the startup budget
//! Highlighting results are cached per `(node, theme, tab width)` so that a
//! resize which only changes the available width never re-highlights (only
//! re-wraps, and not even that when wrapping is off).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::document::markdown::CodeBlock;
use crate::document::{Match, NodeId};
use crate::layout::inline::{push_span, Piece};
use crate::render::primitives::StyledSpan;
use crate::render::theme::{Color, Style, Theme};
use crate::util::unicode;

/// Options that influence code block layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeOptions {
    /// Soft-wrap long lines (otherwise the line is emitted at full width and
    /// the viewport scrolls horizontally).
    pub wrap: bool,
    /// Render a line-number gutter.
    pub line_numbers: bool,
    /// Tab expansion width.
    pub tab_width: usize,
    /// Unicode box drawing available (ASCII gutter otherwise).
    pub unicode: bool,
    /// Defer syntax highlighting: emit plain code styling for a block that is
    /// not in the cache yet and let the caller realize it later
    /// ([`crate::render::primitives::PendingCode`]).
    ///
    /// Highlighting is what dominates the startup budget — `fancy-regex`
    /// compiles a syntax definition's patterns on first use, once per language
    /// per process — and it never influences how many lines a block occupies
    /// or how wide they are.
    pub lazy: bool,
}

impl Default for CodeOptions {
    fn default() -> Self {
        Self {
            wrap: false,
            line_numbers: false,
            tab_width: 4,
            unicode: true,
            lazy: false,
        }
    }
}

/// Indent of soft-wrapped continuation lines inside a code block.
const CONTINUATION_INDENT: usize = 2;

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

/// Map a fence info string to a syntect lookup token.
///
/// Unknown languages simply fall through and end up as plain text — never an
/// error.
pub fn language_token(language: &str) -> &str {
    let lang = language.trim();
    let lang = lang.split_whitespace().next().unwrap_or(lang);
    match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rs",
        "sh" | "bash" | "zsh" | "shell" | "console" | "shell-session" => "sh",
        "python" | "py" | "python3" => "py",
        "javascript" | "js" | "node" | "mjs" => "js",
        "typescript" | "ts" | "tsx" => "ts",
        "yaml" | "yml" => "yaml",
        "toml" | "tml" => "toml",
        "json" | "jsonc" => "json",
        "markdown" | "md" | "mdown" => "md",
        "c" | "h" => "c",
        "cpp" | "c++" | "cxx" | "hpp" => "cpp",
        "go" | "golang" => "go",
        "java" => "java",
        "ruby" | "rb" => "rb",
        "php" => "php",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" | "sass" => "scss",
        "xml" | "svg" => "xml",
        "sql" => "sql",
        "diff" | "patch" => "diff",
        "ini" | "cfg" | "conf" => "ini",
        "make" | "makefile" => "make",
        "perl" | "pl" => "pl",
        "lua" => "lua",
        "erlang" | "erl" => "erl",
        "haskell" | "hs" => "hs",
        "objc" | "objective-c" => "m",
        "cs" | "csharp" | "c#" => "cs",
        "tex" | "latex" => "tex",
        "text" | "plain" | "txt" | "" => "txt",
        _ => "",
    }
}

/// A logical (unwrapped) highlighted code line.
type HighlightedLine = Vec<(Style, String)>;

/// Cache key for highlighting results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    node: NodeId,
    dark: bool,
    syntax: bool,
    tab_width: usize,
}

/// Cache of syntect highlighting results owned by the layout engine.
///
/// Keyed by node, theme polarity and tab width — none of which depend on the
/// terminal width, so re-layout after a resize reuses the cached highlighting.
#[derive(Debug, Default)]
pub struct CodeCache {
    map: RefCell<HashMap<CacheKey, Vec<HighlightedLine>>>,
}

impl CodeCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of cached blocks.
    ///
    /// Cache occupancy is an implementation detail with no caller outside the
    /// tests that assert the cache is actually reused, so it is test-only.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.borrow().len()
    }

    /// Whether the cache is empty.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.map.borrow().is_empty()
    }

    /// Drop all cached highlighting (e.g. on a theme change).
    pub fn clear(&self) {
        self.map.borrow_mut().clear();
    }

    fn get_or_highlight(
        &self,
        node: NodeId,
        block: &CodeBlock,
        theme: &Theme,
        tab_width: usize,
    ) -> Vec<HighlightedLine> {
        let key = CacheKey {
            node,
            dark: theme.dark,
            syntax: theme.syntax,
            tab_width,
        };
        if let Some(hit) = self.map.borrow().get(&key) {
            return hit.clone();
        }
        let lines = highlight(&block.code, block.language.as_deref(), theme, tab_width);
        self.map.borrow_mut().insert(key, lines.clone());
        lines
    }

    /// The cached highlighting of a block, without computing it.
    fn peek(&self, node: NodeId, theme: &Theme, tab_width: usize) -> Option<Vec<HighlightedLine>> {
        let key = CacheKey {
            node,
            dark: theme.dark,
            syntax: theme.syntax,
            tab_width,
        };
        let hit = self.map.borrow().get(&key).cloned();
        hit
    }
}

/// The un-highlighted form of a block: one run per line, tabs expanded.
///
/// This is exactly what [`highlight`] returns for a fence without a language,
/// and — crucially — it has the same number of lines and the same text as the
/// highlighted form, so laying a block out plain and highlighting it later
/// cannot move a single line.
fn plain_lines(code: &str, theme: &Theme, tab_width: usize) -> Vec<HighlightedLine> {
    code.lines()
        .map(|l| vec![(theme.code, unicode::expand_tabs(l, tab_width))])
        .collect()
}

fn convert_color(c: syntect::highlighting::Color) -> Option<Color> {
    if c.a == 0 {
        None
    } else {
        Some(Color::Rgb(c.r, c.g, c.b))
    }
}

/// Highlight code into logical lines of `(style, text)` runs.
///
/// Tabs are expanded first, so the returned text is what will be displayed.
/// An unknown language, a missing syntect theme or a highlighting error all
/// degrade to unstyled plain text.
pub fn highlight(
    code: &str,
    language: Option<&str>,
    theme: &Theme,
    tab_width: usize,
) -> Vec<HighlightedLine> {
    let expanded: Vec<String> = code
        .lines()
        .map(|l| unicode::expand_tabs(l, tab_width))
        .collect();
    let plain = || -> Vec<HighlightedLine> {
        expanded
            .iter()
            .map(|l| vec![(theme.code, l.clone())])
            .collect()
    };

    let token = language.map(language_token).unwrap_or("");
    if !theme.syntax || token.is_empty() || token == "txt" {
        return plain();
    }
    let ps = syntax_set();
    let Some(syntax) = ps
        .find_syntax_by_token(token)
        .or_else(|| ps.find_syntax_by_extension(token))
    else {
        return plain();
    };
    let ts = theme_set();
    let theme_name = if theme.dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let Some(sy_theme) = ts
        .themes
        .get(theme_name)
        .or_else(|| ts.themes.values().next())
    else {
        return plain();
    };

    let mut h = HighlightLines::new(syntax, sy_theme);
    let mut out = Vec::with_capacity(expanded.len());
    for line in &expanded {
        let with_nl = format!("{line}\n");
        match h.highlight_line(&with_nl, ps) {
            Ok(ranges) => {
                let mut runs: HighlightedLine = Vec::new();
                for (style, text) in ranges {
                    let text = text.trim_end_matches('\n');
                    if text.is_empty() {
                        continue;
                    }
                    let mut s = Style::new();
                    s.fg = convert_color(style.foreground);
                    s.bold = style.font_style.contains(FontStyle::BOLD);
                    s.italic = style.font_style.contains(FontStyle::ITALIC);
                    s.underline = style.font_style.contains(FontStyle::UNDERLINE);
                    runs.push((s, text.to_string()));
                }
                out.push(runs);
            }
            Err(_) => out.push(vec![(theme.code, line.clone())]),
        }
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

/// The result of laying out one code block.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeLayout {
    /// The block's lines, gutter included.
    pub lines: Vec<Vec<StyledSpan>>,
    /// `true` when [`CodeOptions::lazy`] deferred the highlighting, so the
    /// lines carry plain code styling and the caller should record the block
    /// as pending.
    pub deferred: bool,
}

/// Lay out a code block into lines of spans.
///
/// Returns lines that already contain the gutter. In no-wrap mode lines may be
/// wider than `width`; the caller reports the true width so the viewport can
/// scroll horizontally.
pub fn layout_code(
    node: NodeId,
    block: &CodeBlock,
    theme: &Theme,
    opts: &CodeOptions,
    width: usize,
    matches: &[Match],
    cache: &CodeCache,
) -> CodeLayout {
    let width = width.max(4);
    let tab_width = opts.tab_width.max(1);
    // Deferring is only sound when the plain form has exactly as many lines as
    // the highlighted one. That holds for every non-empty block: both are one
    // entry per `code.lines()` item. An empty block is the single exception
    // (highlighting emits one empty line) and is highlighted right away, which
    // costs nothing because there is no line to run a regex over.
    let mut deferred = false;
    let highlighted = match cache.peek(node, theme, tab_width) {
        Some(hit) => hit,
        None if opts.lazy && block.code.lines().next().is_some() => {
            deferred = true;
            plain_lines(&block.code, theme, tab_width)
        }
        None => cache.get_or_highlight(node, block, theme, tab_width),
    };
    let bg = theme.code_bg;
    let gutter_style = theme.code_bg.patch(theme.quote_gutter);

    let total = highlighted.len();
    let num_width = if opts.line_numbers {
        total.to_string().len()
    } else {
        0
    };
    let bar = if opts.unicode { "▏" } else { "|" };
    let gutter_width = if opts.line_numbers {
        num_width + 3 // "NN │ "
    } else {
        2 // "▏ "
    };
    let avail = width.saturating_sub(gutter_width).max(1);

    // Byte offset of every logical line in the original code (for search
    // match mapping). Tab expansion shifts offsets, so lines containing tabs
    // are not search-highlighted.
    let mut offsets = Vec::with_capacity(total);
    let mut off = 0usize;
    for line in block.code.lines() {
        offsets.push((off, line.contains('\t')));
        off += line.len() + 1;
    }

    let mut out = Vec::with_capacity(total);
    for (idx, runs) in highlighted.iter().enumerate() {
        let (line_off, has_tab) = offsets.get(idx).copied().unwrap_or((0, true));
        // Build pieces with plain-text offsets for search highlighting.
        let mut pieces = Vec::with_capacity(runs.len());
        let mut rel = line_off;
        for (style, text) in runs {
            pieces.push(Piece {
                text: text.clone(),
                style: bg.patch(*style),
                link: None,
                search: false,
                plain_start: rel,
                hard_break: false,
            });
            rel += text.len();
        }
        let pieces = if has_tab || matches.is_empty() {
            pieces
        } else {
            crate::layout::inline::apply_matches(pieces, matches)
        };
        let pieces: Vec<Piece> = pieces
            .into_iter()
            .map(|mut p| {
                if p.search {
                    p.style = p.style.patch(theme.search_match);
                }
                p
            })
            .collect();

        let content_lines: Vec<Vec<StyledSpan>> = if opts.wrap {
            wrap_code_line(&pieces, avail)
        } else {
            let mut line: Vec<StyledSpan> = Vec::new();
            for p in &pieces {
                push_span(&mut line, &p.text, p.style, None, p.search);
            }
            vec![line]
        };

        for (sub, mut content) in content_lines.into_iter().enumerate() {
            let mut line: Vec<StyledSpan> = Vec::new();
            if opts.line_numbers {
                let label = if sub == 0 {
                    unicode::pad_left_to_width(&(idx + 1).to_string(), num_width)
                } else {
                    " ".repeat(num_width)
                };
                push_span(&mut line, &label, gutter_style, None, false);
                push_span(&mut line, &format!(" {bar} "), gutter_style, None, false);
            } else {
                push_span(&mut line, &format!("{bar} "), gutter_style, None, false);
            }
            if sub > 0 {
                push_span(&mut line, &" ".repeat(CONTINUATION_INDENT), bg, None, false);
            }
            line.append(&mut content);
            // Pad to the full width so the code background forms a block.
            let w: usize = line.iter().map(StyledSpan::width).sum();
            if w < width {
                push_span(&mut line, &" ".repeat(width - w), bg, None, false);
            }
            out.push(line);
        }
    }
    CodeLayout {
        lines: out,
        deferred,
    }
}

fn wrap_code_line(pieces: &[Piece], avail: usize) -> Vec<Vec<StyledSpan>> {
    // Code is wrapped by hard breaking at the available width (never at word
    // boundaries) so that indentation and alignment stay predictable.
    let cont = avail.saturating_sub(CONTINUATION_INDENT).max(1);
    let mut lines: Vec<Vec<StyledSpan>> = Vec::new();
    let mut cur: Vec<StyledSpan> = Vec::new();
    let mut cur_w = 0usize;
    let mut limit = avail;
    for p in pieces {
        let mut rest = p.text.as_str();
        loop {
            let room = limit.saturating_sub(cur_w);
            if unicode::width(rest) <= room {
                push_span(&mut cur, rest, p.style, None, p.search);
                cur_w += unicode::width(rest);
                break;
            }
            let (head, tail) = unicode::split_at_width(rest, room);
            let (head, tail) = if head.is_empty() && cur.is_empty() {
                let end = unicode::graphemes(rest)
                    .next()
                    .map(str::len)
                    .unwrap_or(rest.len());
                rest.split_at(end.min(rest.len()))
            } else {
                (head, tail)
            };
            push_span(&mut cur, head, p.style, None, p.search);
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
            limit = cont;
            rest = tail;
            if rest.is_empty() {
                break;
            }
        }
    }
    lines.push(cur);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::markdown::{parse, NodeKind};

    fn block(src: &str) -> (NodeId, CodeBlock) {
        let doc = parse(src);
        for n in doc.walk() {
            if let NodeKind::CodeBlock(c) = &n.kind {
                return (n.id, c.clone());
            }
        }
        panic!("no code block in {src:?}");
    }

    fn text(laid: &CodeLayout) -> Vec<String> {
        let lines = &laid.lines;
        lines
            .iter()
            .map(|l| {
                l.iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn language_aliases() {
        assert_eq!(language_token("rs"), "rs");
        assert_eq!(language_token("Rust"), "rs");
        assert_eq!(language_token("bash"), "sh");
        assert_eq!(language_token("yml"), "yaml");
        assert_eq!(language_token("brainfuck"), "");
        assert_eq!(language_token("rust,ignore should_panic"), "");
        assert_eq!(language_token("rust ignore"), "rs");
    }

    #[test]
    fn unknown_language_is_plain_not_an_error() {
        let (id, b) = block("```wobbly\nlet x = 1;\n```\n");
        let cache = CodeCache::new();
        let lines = layout_code(
            id,
            &b,
            &Theme::dark(),
            &CodeOptions::default(),
            40,
            &[],
            &cache,
        );
        assert_eq!(text(&lines), ["▏ let x = 1;"]);
    }

    #[test]
    fn rust_is_highlighted() {
        let (id, b) = block("```rust\nfn main() {}\n```\n");
        let cache = CodeCache::new();
        let lines = layout_code(
            id,
            &b,
            &Theme::dark(),
            &CodeOptions::default(),
            40,
            &[],
            &cache,
        );
        assert_eq!(text(&lines), ["▏ fn main() {}"]);
        let colored = lines.lines[0]
            .iter()
            .filter(|s| s.style.fg.is_some())
            .count();
        assert!(colored > 1, "expected several highlight colours");
    }

    #[test]
    fn line_numbers_gutter() {
        let (id, b) = block("```\na\nb\n```\n");
        let opts = CodeOptions {
            line_numbers: true,
            ..CodeOptions::default()
        };
        let cache = CodeCache::new();
        let lines = layout_code(id, &b, &Theme::dark(), &opts, 40, &[], &cache);
        assert_eq!(text(&lines), ["1 ▏ a", "2 ▏ b"]);
    }

    #[test]
    fn no_wrap_emits_full_width_lines() {
        let long = "x".repeat(100);
        let (id, b) = block(&format!("```\n{long}\n```\n"));
        let cache = CodeCache::new();
        let lines = layout_code(
            id,
            &b,
            &Theme::dark(),
            &CodeOptions::default(),
            20,
            &[],
            &cache,
        );
        assert_eq!(lines.lines.len(), 1);
        let w: usize = lines.lines[0].iter().map(StyledSpan::width).sum();
        assert_eq!(w, 102, "true width is reported for horizontal scrolling");
    }

    #[test]
    fn wrap_mode_breaks_with_continuation_indent() {
        let long = "x".repeat(50);
        let (id, b) = block(&format!("```\n{long}\n```\n"));
        let opts = CodeOptions {
            wrap: true,
            ..CodeOptions::default()
        };
        let cache = CodeCache::new();
        let lines = layout_code(id, &b, &Theme::dark(), &opts, 20, &[], &cache);
        assert!(lines.lines.len() > 2);
        for l in &lines.lines {
            let w: usize = l.iter().map(StyledSpan::width).sum();
            assert_eq!(w, 20, "wrapped lines are padded to the block width");
        }
        assert!(text(&lines)[1].starts_with("▏   "), "continuation indent");
        let joined: String = text(&lines)
            .iter()
            .map(|l| l.trim_start_matches('▏').trim().to_string())
            .collect();
        assert_eq!(joined, long);
    }

    #[test]
    fn tabs_are_expanded() {
        let (id, b) = block("```\n\tindented\n```\n");
        let opts = CodeOptions {
            tab_width: 4,
            ..CodeOptions::default()
        };
        let cache = CodeCache::new();
        let lines = layout_code(id, &b, &Theme::dark(), &opts, 40, &[], &cache);
        assert_eq!(text(&lines), ["▏     indented"]);
        let opts8 = CodeOptions {
            tab_width: 8,
            ..CodeOptions::default()
        };
        let lines = layout_code(id, &b, &Theme::dark(), &opts8, 40, &[], &cache);
        assert_eq!(text(&lines), ["▏         indented"]);
    }

    #[test]
    fn ascii_fallback_gutter() {
        let (id, b) = block("```\na\n```\n");
        let opts = CodeOptions {
            unicode: false,
            ..CodeOptions::default()
        };
        let cache = CodeCache::new();
        let lines = layout_code(id, &b, &Theme::dark(), &opts, 40, &[], &cache);
        assert_eq!(text(&lines), ["| a"]);
    }

    /// A warm cache must never change what is rendered. The cache is keyed by
    /// block and theme but not by width, so the risk it carries is serving a
    /// 40-column highlighting to a 120-column request, or dark spans to a
    /// light theme; both are asserted against a cold render.
    ///
    /// The `len()` assertions are kept deliberately: "width is not part of
    /// the key, the theme is" is a memory-and-latency design decision with no
    /// other observable, and equal output alone would also hold for a cache
    /// that stored one entry per width.
    #[test]
    fn a_warm_cache_renders_exactly_what_a_cold_one_does() {
        let (id, b) = block("```rust\nfn main() { let x = 1; }\n```\n");
        let o = CodeOptions::default();
        let warm = CodeCache::new();

        let _ = layout_code(id, &b, &Theme::dark(), &o, 40, &[], &warm);
        assert_eq!(warm.len(), 1);

        for (label, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
            for width in [40usize, 120] {
                let cold = layout_code(id, &b, &theme, &o, width, &[], &CodeCache::new());
                let hot = layout_code(id, &b, &theme, &o, width, &[], &warm);
                assert_eq!(text(&cold), text(&hot), "{label}@{width}: text");
                assert_eq!(cold.lines, hot.lines, "{label}@{width}: spans and styles");
            }
        }

        assert_eq!(warm.len(), 2, "one entry per theme, not per width");
        warm.clear();
        assert!(warm.is_empty());
    }

    #[test]
    fn empty_and_zero_width_do_not_panic() {
        let (id, b) = block("```\n```\n");
        let cache = CodeCache::new();
        let lines = layout_code(
            id,
            &b,
            &Theme::dark(),
            &CodeOptions::default(),
            0,
            &[],
            &cache,
        );
        assert!(lines.lines.len() <= 1);
    }
}
