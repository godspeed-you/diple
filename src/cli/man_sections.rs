//! Hand-written roff sections for the man page.
//!
//! `clap_mangen` generates NAME, SYNOPSIS, DESCRIPTION, OPTIONS and VERSION
//! from the [`clap::Command`], so those can never drift from `--help`. The
//! sections a real man page also needs — key bindings, the configuration
//! *file*, environment, files, exit status, examples and cross references —
//! have no counterpart in the clap model and live here.
//!
//! Everything below is derived from the running program:
//!
//! * key bindings from `src/config/keys.rs` and `docs/keybindings.md`
//! * configuration keys and defaults from `src/config/schema.rs`
//! * the file lookup order from `src/config/loader.rs`
//! * environment variables from `src/terminal/capabilities.rs`,
//!   `src/config/loader.rs`, `src/config/mod.rs` and `src/render/theme.rs`
//! * exit codes from `src/main.rs`
//!
//! Roff rules observed here: no content line begins with `.` or `'`, no
//! literal backslash occurs outside a roff escape, and every literal hyphen is
//! written `\-` so that it is not typeset as a hyphenation point.

/// `.SH "DOCUMENT FORMATS"` — what diple reads, how it decides which format
/// a document is, and what it does when the document does not parse.
///
/// Derived from `src/document/format.rs` and `src/document/load.rs`; the
/// presentation keys it names are the `[structured]` section of
/// `src/config/schema.rs`.
pub const DOCUMENT_FORMATS: &str = r#".SH "DOCUMENT FORMATS"
diple reads Markdown, JSON and YAML as documents rather than as coloured
text: folding, navigation, search and the outline operate on semantic nodes,
so a JSON object folds as an object and a YAML mapping entry is one unit.
.PP
The format is decided in this order, and the first entry that applies wins:
.TP
.BI \-\-format " FORMAT"
.BR auto ", " markdown ", " json " or " yaml .
An explicit format is final: it beats the file name and the content alike, so
.B diple \-\-format markdown config.yaml
reads a YAML file as prose and
.B diple \-\-format json file.data
insists on JSON.
.TP
.B The file name
.BR .md ", " .markdown ", " .mdown " and " .mkd
are Markdown,
.B .json
is JSON, and
.BR .yaml " and " .yml
are YAML. JSON with comments is not a format diple reads, so
.B .jsonc
is deliberately not recognised; pass
.B \-\-format json
for a file that is strict JSON under another name.
.TP
.B The content
Only a confident guess is taken: a document opening with
.BR { " or " [
that parses as strict JSON, or a YAML stream whose roots are all non\-empty
collections. This is what makes
.B kubectl get deployment nginx \-o yaml | diple
open as YAML with nothing to configure.
.TP
.B Markdown
The fallback. Prose, Markdown lists and anything else ambiguous stay
Markdown, because a reader's prose must never be claimed by a permissive
parser.
.PP
The format
.I stated
by
.B \-\-format
or by the file name is binding: a
.B config.yaml
that does not parse is reported as a YAML error naming the line, the column
and the offending source, and diple exits non\-zero rather than opening the
file as Markdown and looking almost right. A format only
.I guessed
from content falls back to Markdown instead, since nothing had claimed it.
.PP
JSON and YAML presentation is controlled by
.BR \-\-structured\-indent ", " \-\-path
and the
.B [structured]
section of the configuration file. When standard output is not a terminal the
document is written as plain text, fully expanded and in source order, so
.B diple response.json | head \-20
is readable and reproducible.
"#;

/// `.SH "KEY BINDINGS"` — the default bindings, grouped as in
/// `docs/keybindings.md`.
pub const KEY_BINDINGS: &str = r#".SH "KEY BINDINGS"
diple follows
.BR less (1)
and Vim conventions where practical. Every binding is configurable in the
.B [keys]
section of the configuration file (see
.B CONFIGURATION
below); the action names given in parentheses are the keys of that section.
Press
.B ?
or
.B F1
inside diple for the same list with your own overrides applied.
.SS "Paging and scrolling"
.TP
.BR j ", " Down
Scroll down one line (\fBscroll_down\fR).
.TP
.BR k ", " Up
Scroll up one line (\fBscroll_up\fR).
.TP
.BR PgDn ", " Space
Page down (\fBpage_down\fR).
.TP
.BR PgUp ", " b
Page up (\fBpage_up\fR).
.TP
.B Ctrl\-D
Half page down (\fBhalf_page_down\fR).
.TP
.B Ctrl\-U
Half page up (\fBhalf_page_up\fR).
.TP
.BR h ", " Left
Scroll left (\fBscroll_left\fR). Used for wide tables and unwrapped code.
.TP
.BR l ", " Right
Scroll right (\fBscroll_right\fR).
.TP
.B g
Top of the document (\fBtop\fR).
.TP
.B G
Bottom of the document (\fBbottom\fR).
.SS Search
.TP
.B /
Open the search prompt (\fBsearch\fR). Matching is incremental as you type;
.B Enter
commits the search and
.B Esc
cancels it.
.TP
.B n
Next match (\fBnext_search\fR).
.TP
.B N
Previous match (\fBprevious_search\fR).
.PP
Search runs over the document content, not over rendered terminal lines.
Jumping to a match inside a collapsed section expands that section
automatically.
.SS "Structure navigation"
.TP
.B ]
Next structural node (\fBnext_heading\fR): a Markdown heading, a JSON or YAML
container.
.TP
.B [
Previous structural node (\fBprevious_heading\fR). From a body row this goes
to the node you are inside.
.TP
.B }
Next sibling at the same or a higher level (\fBnext_heading_same_level\fR).
.TP
.B {
Previous sibling at the same or a higher level
(\fBprevious_heading_same_level\fR).
.TP
.B H
The node that contains this one (\fBparent_node\fR).
.TP
.B L
The first node inside this one (\fBfirst_child\fR).
.PP
The action names keep their 1.x spelling so existing \fB[keys]\fR files go on
working. Structural jumps are semantic: they target the node, not a line
number, and they stay correct across a terminal resize.
.SS Folding
.TP
.B Enter
Toggle the section under the cursor, or open the selected link
(\fBactivate\fR).
.TP
.B za
Toggle the current section (\fBtoggle_fold\fR).
.TP
.B zc
Collapse the current section (\fBcollapse_fold\fR).
.TP
.B zo
Expand the current section (\fBexpand_fold\fR).
.TP
.B zM
Collapse all sections (\fBcollapse_all\fR).
.TP
.B zR
Expand all sections (\fBexpand_all\fR).
.PP
A collapsed section is marked with a right\-pointing triangle, an expanded one
with a down\-pointing triangle. Fold state lives for the current session only.
If a multi\-key sequence fails to match, the last key is retried on its own, so
after a stray
.B z
pressing
.B q
still quits.
.SS Links
.TP
.B Tab
Select the next link (\fBnext_link\fR).
.TP
.B Shift\-Tab
Select the previous link (\fBprevious_link\fR).
.TP
.B o
Open the selected link with the configured opener (\fBopen_link\fR).
.TP
.B Enter
Open the selected link, or follow an internal \fB#anchor\fR within the
document (\fBactivate\fR).
.PP
External links are handed to
.B links.opener
(default
.BR xdg\-open (1)).
Where the terminal supports OSC 8, links are additionally emitted as native
terminal hyperlinks.
.SS View
.TP
.B t
Toggle the table\-of\-contents sidebar (\fBtoggle_toc\fR). Inside the sidebar,
.B j
and
.B k
move the selection and
.B Enter
jumps to that heading. The sidebar is as wide as its widest entry, up to 40
columns and never more than a third of the screen;
.B h
and
.B l
scroll the outline sideways when an entry does not fit.
.TP
.B m
Toggle mouse reporting (\fBtoggle_mouse\fR). diple asks the terminal for
button presses only, never for drag or motion reporting, but while it is
listening the terminal cannot use the mouse to select text. Turning reporting
off hands it back: dragging selects and copies as usual, and the wheel and the
clickable sidebars return when reporting is turned back on.
.TP
.B K
Toggle the key hints sidebar on the right\-hand edge
(\fBtoggle_key_hints\fR). It lists the commands available in the current mode
and cursor context \(em horizontal scrolling only where the view can scroll
sideways, the link keys only where a link is visible, the Mermaid source
toggle only at or near a diagram, and \fBn\fR/\fBN\fR only while a search has
matches. Key labels are read from the live key map, so rebound keys are shown
and unbound actions are omitted. The sidebar hides itself when the terminal
cannot spare 40 columns for the document, and yields to the table of contents
when only one of the two fits.
.TP
.B s
Toggle the Mermaid source view for the diagram at the cursor
(\fBtoggle_mermaid_source\fR).
.TP
.BR ? ", " F1
Show the help overlay (\fBhelp\fR).
.TP
.B Esc
Close the overlay, prompt or sidebar (\fBcancel\fR).
.TP
.BR q ", " Ctrl\-C
Close the focused document (\fBquit\fR); the last one open ends the session.
.B Ctrl\-C
always ends it, whatever else is open.
.SS Documents
.TP
.B Ctrl\-W
Move the keyboard to the other pane of a split (\fBfocus_other_pane\fR). The
pane with the keyboard is the one whose status bar name is marked; the other
one keeps reading, it simply does not get the keys.
.TP
.BR Ctrl\-N ", " Ctrl\-P
Show the next or previous tab (\fBnext_tab\fR, \fBprevious_tab\fR). The tab
bar appears on the top row as soon as a second tab is open, and lists every
open document; clicking a label selects it.
.TP
.BR Alt\-1 " \(en " Alt\-9
Show that tab by its number, which is the number the tab bar prints in front
of the file name. This gesture takes an argument rather than naming an action,
so it is the one binding that
.B [keys]
cannot rebind.
.SS "Command line"
.TP
.B :
Open the command line (\fBcommand_prompt\fR) and change a setting for the
running session. The key is the one the configuration file uses, dotted for a
section, and the separator may be \fB=\fR or a space:
.BR ":center = false" ,
.BR ":theme crt" ,
.BR ":table.mode compact" .
A leading
.B set
is accepted and ignored.
.IP
A key on its own reports its current value.
.B Tab
completes the key, and once a separator is typed it completes the value; what
is still possible is listed in the status line.
.B :help
shows every setting with the values it takes and the default it started from,
.B :q
closes the focused document (and leaves when it is the last one),
.B :qa
leaves whatever is open, and
.B Esc
leaves the line without applying it. Changes last for the session; the
configuration file is not written, and a setting applies to every open
document, not only to the one it was typed in.
.IP
.BI ":open " "TARGET PATH"
opens another document.
.B TARGET
is
.BR side\-by\-side ,
.B stacked
or
.BR tab :
beside the current document, above and below it, or in a tab of its own.
.B vsplit
and
.B split
are accepted for the first two.
.B Tab
completes the target and then the path, and a path that does not exist in the
working directory is looked for next to the document that is open.
.B :close
closes the focused document without ever ending the session. A split is
refused rather than halved on a terminal with no room for two readable panes.
"#;

/// `.SH CONFIGURATION` — the configuration *file*, as opposed to the
/// `Configuration options` heading generated from the CLI flags.
pub const CONFIGURATION: &str = r#".SH CONFIGURATION
diple is configured with a TOML file. The file is looked up in this order,
and the first entry that applies wins:
.TP
.B \-\-no\-config
Skip every configuration file and use the built\-in defaults.
.TP
.BI \-\-config " PATH"
Use this file. It is an error if it does not exist.
.TP
.B $DIPLE_CONFIG
Use this file. It is an error if it does not exist.
.TP
.B $XDG_CONFIG_HOME/diple/config.toml
Normally \fB~/.config/diple/config.toml\fR. Silently skipped when absent.
.PP
Values are resolved with the precedence
.PP
.RS 4
built\-in defaults < configuration file < environment < command line
.RE
.PP
so a flag always beats
.BR DIPLE_THEME ,
which always beats the file.
.PP
Validate a file without starting the reader with
.B diple \-\-check\-config
or
.BR "diple \-\-config ./my.toml \-\-check\-config" .
.SS "Complete example, showing every default"
.RS 4
.EX
format = "auto"         # auto | markdown | json | yaml; see DOCUMENT FORMATS
theme = "auto"          # auto | dark | light | crt | cyberpunk
color = "auto"          # auto | always | never
mouse = true            # enable mouse reporting where supported
toc = false             # start with the table\-of\-contents sidebar open
key_hints = false       # start with the key hints sidebar open
line_numbers = false    # document line numbers
wrap = true             # reflow paragraphs to the terminal width
max_width = 160         # cap the line width in columns; 0 = the full width
center = true           # centre the document between the sidebars

[table]
mode = "auto"           # auto | wrap | scroll | compact
max_column_width = 60   # cells; must be >= 3

[code]
wrap = false            # wrap long code lines instead of scrolling
line_numbers = false    # line numbers inside code blocks
tab_width = 4           # tab expansion width; must be >= 1

[links]
opener = "xdg\-open"     # command used to open external links
osc8 = "auto"           # auto | always | never; terminal hyperlinks

[mermaid]
backend = "auto"        # auto | terminal | mmdc | source
images = "auto"         # auto | always | never; image protocol usage
mmdc_command = "mmdc"   # Mermaid CLI executable

[structured]            # JSON and YAML only; Markdown ignores this section
indent = 2              # columns per nesting level; must be >= 1
path = "auto"           # auto | always | never; path of the selected node
show_indices = true     # number sequence items as [0], [1], [2]
collapsed_summary = true # say what a collapsed container holds

[keys]
# Overrides only; every unlisted action keeps its default binding.
# A [keys] entry replaces all default bindings for that action; use a
# list to bind several keys to one action.
quit = ["q", "ctrl\-q"]
search = "/"
next_heading = "]"
previous_heading = "["
parent_node = "H"
first_child = "L"
toggle_toc = "t"
toggle_key_hints = "K"
toggle_fold = "za"
.EE
.RE
.PP
Key specifications are single characters (\fBq\fR, \fB/\fR, \fBG\fR), named
keys (\fBenter\fR, \fBesc\fR, \fBspace\fR, \fBtab\fR, \fBshift\-tab\fR,
\fBpgup\fR, \fBpgdn\fR, \fBup\fR, \fBdown\fR, \fBleft\fR, \fBright\fR,
\fBhome\fR, \fBend\fR, \fBf1\fR through \fBf24\fR), modifier combinations
(\fBctrl\-d\fR, \fBalt\-enter\fR, \fBctrl\-shift\-f5\fR) or multi\-key
sequences written together (\fBza\fR) or separated by spaces (\fBg g\fR).
.SS "Error reporting"
Unknown keys are rejected rather than ignored, so typos surface immediately.
An invalid configuration is reported before anything is rendered, naming the
file, the line, the offending key, the value and what was expected:
.RS 4
.EX
~/.config/diple/config.toml:9: invalid value for `table.mode`: `fancy`
  \(em expected one of: auto, wrap, scroll, compact
.EE
.RE
"#;

/// `.SH "EXIT STATUS"` — mirrors `EXIT_USAGE` / `EXIT_FAILURE` in
/// `src/main.rs`.
pub const EXIT_STATUS: &str = r#".SH "EXIT STATUS"
.TP
.B 0
Success. This also covers
.BR \-\-help ,
.BR \-\-version ,
.BR \-\-check\-config ,
.B \-\-print\-capabilities
and a reader whose output pipe was closed early, as in
.BR "diple doc.md | head" .
.TP
.B 1
Runtime error: the document could not be read or the terminal could not be
driven. A Mermaid diagram that cannot be rendered is never a runtime error;
it falls back to its source and the rest of the document stays readable.
.TP
.B 2
Usage or configuration error: an unknown or malformed command\-line argument,
a configuration file that was requested but cannot be read, or a configuration
value or key binding that failed validation.
"#;

/// `.SH ENVIRONMENT` — every variable actually read by the program.
pub const ENVIRONMENT: &str = r#".SH ENVIRONMENT
.TP
.B DIPLE_CONFIG
Path to the configuration file, used when
.B \-\-config
is not given. It is an error if the file does not exist.
.TP
.B DIPLE_THEME
Overrides
.B theme
from the configuration file. Overridden in turn by
.BR \-\-theme .
.TP
.B DIPLE_MERMAID
Overrides
.BR mermaid.backend :
one of \fBauto\fR, \fBterminal\fR, \fBmmdc\fR or \fBsource\fR. An
unrecognised value is a configuration error. Overridden by
.BR \-\-mermaid .
.TP
.B NO_COLOR
When set to any value, all colour is disabled and styling is reduced to
bold, underline and reverse. See
.IR https://no\-color.org .
.TP
.B CLICOLOR_FORCE
When set to a non\-empty value other than \fB0\fR, colour is emitted even when
standard output is not a terminal. The colour depth is still detected from
.B COLORTERM
and
.BR TERM .
.B NO_COLOR
wins over it.
.TP
.B CLICOLOR
.B CLICOLOR=0
disables colour on a terminal. Other values have no effect.
.TP
.B TERM
Terminal type. Used to identify the terminal, to derive the colour depth, and
to detect a multiplexer.
.B TERM=dumb
and an empty
.B TERM
disable colour, mouse reporting and the interactive reader.
.TP
.B COLORTERM
.B truecolor
or
.B 24bit
selects 24\-bit colour.
.TP
.B COLORFGBG
Used as a hint for the terminal background when
.B theme = \(dqauto\(dq
is in effect.
.TP
.B TERM_PROGRAM
Terminal identity when it is more specific than
.BR TERM ;
also used as a background hint for
.BR "theme = \(dqauto\(dq" .
\fBVTE_VERSION\fR, \fBKITTY_WINDOW_ID\fR and \fBWEZTERM_EXECUTABLE\fR are read
for the same purpose and to detect image and OSC 8 support.
.TP
.B TMUX
Marks a tmux session; together with
.B STY
and a
.BR screen* / tmux*
.BR TERM ,
it selects the multiplexer\-safe behaviour. Inside tmux, image protocols are
not used unless passthrough is enabled.
.TP
.B SSH_TTY
Set by
.BR ssh (1);
together with
.B SSH_CONNECTION
it marks a remote session in
.BR \-\-print\-capabilities .
.TP
.BR LC_ALL ", " LC_CTYPE ", " LANG
The first of these that is set decides whether box\-drawing characters are
used. A locale announcing UTF\-8 enables them; \fBC\fR, \fBPOSIX\fR and every
other value falls back to ASCII drawing. If none is set, diple is
conservative and uses ASCII.
.TP
.B XDG_CONFIG_HOME
Base directory for the default configuration path.
.TP
.B XDG_CACHE_HOME
Base directory for the Mermaid image cache.
.TP
.B PATH
Searched for the
.B mermaid.mmdc_command
executable.
.TP
.B SOURCE_DATE_EPOCH
Read only by
.BR \-\-generate\-man :
when set to a UNIX timestamp, that date is stamped into the generated page, so
that a page built from a Git tag is reproducible. The current date is never
used, so that rebuilding the page yields identical bytes; when the variable is
unset the date slot carries the diple version instead.
.PP
The following variables override capability detection and are intended for
debugging and for terminals that diple does not recognise. They are read
before the configuration and the command line, both of which still win.
.TP
.B DIPLE_COLOR
Force a colour level:
.BR none ,
.BR ansi16 ,
.B 256
or
.BR truecolor .
.TP
.B DIPLE_UNICODE
Force box\-drawing characters on or off, overriding the locale check.
.TP
.B DIPLE_OSC8
Force OSC 8 terminal hyperlinks on or off.
.TP
.B DIPLE_IMAGES
Force an image protocol:
.BR none ,
.B kitty
or
.BR sixel .
.TP
.B DIPLE_MOUSE
Force mouse reporting on or off. It can only narrow the detected value: mouse
reporting is never enabled when the terminal cannot report events.
.TP
.B DIPLE_FORCE_IMAGES
Assume tmux passthrough, and keep image output enabled even when standard
output is not a terminal.
.TP
.B DIPLE_TMUX_PASSTHROUGH
Assume that tmux was configured with
.BR "set \-g allow\-passthrough on" ,
so image protocols may be used inside tmux.
.PP
.B diple \-\-print\-capabilities
prints every detected capability together with the evidence that decided it,
which is the quickest way to see which of these took effect.
"#;

/// `.SH FILES` — user files plus the installed paths.
pub const FILES: &str = r#".SH FILES
.TP
.B ~/.config/diple/config.toml
Default configuration file, or
.B $XDG_CONFIG_HOME/diple/config.toml
when that variable is set. Absent by default.
.TP
.B ~/.cache/diple/mermaid/
Cache of Mermaid diagrams rendered by
.BR mmdc (1),
keyed by diagram source and render width, or
.B $XDG_CACHE_HOME/diple/mermaid/
when that variable is set. It may be deleted at any time.
.TP
.B /usr/bin/diple
The installed executable.
.TP
.B /usr/share/man/man1/diple.1
This manual page.
.TP
.B /usr/share/bash\-completion/completions/diple
Bash completion, generated by
.BR "diple \-\-generate\-completions bash" .
.TP
.B /usr/share/zsh/site\-functions/_diple
Zsh completion.
.TP
.B /usr/share/fish/vendor_completions.d/diple.fish
Fish completion.
.PP
The installed paths above are those of the distribution packages; a build
installed by other means may place them elsewhere.
"#;

/// `.SH EXAMPLES` — every command here was run against the program.
pub const EXAMPLES: &str = r#".SH EXAMPLES
Read a file interactively. The format follows the name:
.RS 4
.EX
diple README.md
diple deployment.yaml
diple response.json
.EE
.RE
.PP
Read structured output straight from the command that produced it:
.RS 4
.EX
kubectl get deployment nginx \-o yaml | diple
curl \-s https://api.example.com/state | diple
.EE
.RE
.PP
Say what a document is when its name does not, or when the name lies:
.RS 4
.EX
diple \-\-format yaml \-
diple \-\-format json file.data
diple \-\-format markdown config.yaml
.EE
.RE
.PP
Widen the nesting indent and pin the path of the selected node on screen:
.RS 4
.EX
diple \-\-structured\-indent 4 \-\-path always deployment.yaml
.EE
.RE
.PP
Read from standard input. diple still runs interactively, reading keys from
.IR /dev/tty :
.RS 4
.EX
cat README.md | diple
git show HEAD:README.md | diple
diple < README.md
.EE
.RE
.PP
When standard output is not a terminal, diple renders the document once and
writes it as plain text, so it composes with other tools:
.RS 4
.EX
diple README.md | head \-40
diple README.md | grep \-n TODO
.EE
.RE
.PP
Open with the table of contents showing, a light theme and a fixed width:
.RS 4
.EX
diple \-\-toc \-\-theme light \-\-width 100 doc.md
.EE
.RE
.PP
Use diple as the pager for Git commands that emit Markdown:
.RS 4
.EX
git config \-\-global core.pager diple
git \-c core.pager=diple show HEAD:README.md
.EE
.RE
.PP
Inspect what diple detected for the current terminal, with the evidence for
each decision:
.RS 4
.EX
diple \-\-print\-capabilities
.EE
.RE
.PP
Check a configuration file before using it:
.RS 4
.EX
diple \-\-check\-config
diple \-\-config ./my.toml \-\-check\-config
.EE
.RE
.PP
Force a Mermaid backend, for instance to compare the built\-in renderer with
the Mermaid CLI, or to read the diagram source instead:
.RS 4
.EX
diple \-\-mermaid terminal diagrams.md
diple \-\-mermaid mmdc \-\-mermaid\-images always diagrams.md
diple \-\-mermaid source diagrams.md
.EE
.RE
"#;

/// `.SH "SEE ALSO"`, with the project URL taken from the crate metadata so it
/// cannot drift from `Cargo.toml`.
pub fn see_also() -> String {
    format!(
        r#".SH "SEE ALSO"
.BR less (1),
.BR man (1),
.BR mmdc (1),
.BR xdg\-open (1)
.PP
Mermaid CLI:
.IR https://github.com/mermaid\-js/mermaid\-cli
.PP
diple home page and issue tracker:
.IR {}
{SEE_ALSO_TAIL}"#,
        escape(env!("CARGO_PKG_REPOSITORY"))
    )
}

/// Escape a run of plain text for use inside a roff line.
///
/// Backslashes become the roff escape for a literal backslash and hyphens are
/// written `\-` so they are typeset as hyphen-minus rather than as a
/// hyphenation point. A leading `.` or `'` is protected with the zero-width
/// `\&` prefix so the line can never be read as a control request.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    if text.starts_with('.') || text.starts_with('\'') {
        out.push_str("\\&");
    }
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\e"),
            '-' => out.push_str("\\-"),
            _ => out.push(ch),
        }
    }
    out
}

const SEE_ALSO_TAIL: &str = r#".PP
The distribution packages also install
.BR README.md ,
.BR configuration.md ,
.B keybindings.md
and
.B mermaid.md
under
.BR /usr/share/doc/diple/ .
"#;

/// Every `.SH` heading this module contributes, in the order they are written.
/// Used by the regression tests that guard against the page silently
/// collapsing back to a bare clap dump.
pub const SECTION_TITLES: &[&str] = &[
    "DOCUMENT FORMATS",
    "KEY BINDINGS",
    "CONFIGURATION",
    "EXIT STATUS",
    "ENVIRONMENT",
    "FILES",
    "EXAMPLES",
    "SEE ALSO",
];

/// The hand-written sections, in the order they belong in the page.
pub fn sections() -> Vec<std::borrow::Cow<'static, str>> {
    use std::borrow::Cow;
    vec![
        Cow::Borrowed(DOCUMENT_FORMATS),
        Cow::Borrowed(KEY_BINDINGS),
        Cow::Borrowed(CONFIGURATION),
        Cow::Borrowed(EXIT_STATUS),
        Cow::Borrowed(ENVIRONMENT),
        Cow::Borrowed(FILES),
        Cow::Borrowed(EXAMPLES),
        Cow::Owned(see_also()),
    ]
}
