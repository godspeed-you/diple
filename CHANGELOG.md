# Changelog

All notable changes to diple are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/1.1.0/) and the project uses
[semantic versioning](https://semver.org/).

`scripts/release.sh` and the CI `release:prepare` job cut the release notes from
the `## [<version>]` section matching the tag they are given, so every release
must have its own section here **before** it is tagged. Keep `## [Unreleased]`
at the top for work that has not shipped yet.

## [Unreleased]

## [2.0.0] - 2026-09-18 — Structured Documents

diple is now `less` for structured documents. JSON and YAML join Markdown as
first-class semantic document formats with navigation, folding, search, paths
and outlines.

This is one reader that learned what a document can be, not two parsers bolted
to the side of a Markdown viewer. diple does not syntax-highlight JSON and
YAML: it parses them, and everything the reader already did — `]` to the next
piece of structure, `za` to fold it away, `/` to search, `t` for the outline —
now operates on whichever semantic nodes the open document has. Markdown keeps
every key, every setting and every behaviour it had in 1.2.

### Added

- **JSON and YAML as document formats.** `diple response.json` and `diple
  deployment.yaml` open natively, and so do `kubectl get deployment nginx -o
  yaml | diple` and `curl -s … | diple`. The format is decided by `--format`
  first, then the file name (`.json`, `.yaml`, `.yml`), then a confident look
  at the content, and finally Markdown — which is where prose, a bare scalar
  and anything ambiguous stay, because a reader's prose must never be claimed
  by a permissive parser. A format *stated* by `--format` or by the file name
  is binding: a `config.yaml` that does not parse is a YAML error naming the
  line, the column and the offending source, with a non-zero exit, rather than
  a Markdown document that looks almost right. A format merely *guessed* from
  anonymous input falls back to Markdown, since nothing had claimed it.
- **Structure navigation in JSON and YAML.** `]` and `[` walk the containers —
  objects, arrays, mappings and sequences — exactly as they walk headings in
  Markdown; scalars are not stops, so `]` on a long array is not another `j`.
  `}` and `{` move between siblings and climb out of a finished branch, the
  same "same or higher level" rule Markdown has always used.
- **`H` and `L`** (`parent_node`, `first_child`) move out to the enclosing node
  and in to the first node inside. `h` and `l` scroll sideways; their shifted
  forms move the same axis one level up. Both keys were unbound before, so no
  existing binding changed meaning, and both are rebindable in `[keys]` and
  listed in `?` and the key hints like every other action.
- **Semantic folding for structured documents.** Every container folds with
  `za`, `zc`, `zo`, `Enter`, `zM` and `zR`, and a collapsed one says what it
  holds: `{4 members}`, `{4 entries}`, `[3 items]`. The document root is not a
  fold target — collapsing it would replace the shape of the file with a single
  brace.
- **The path of the selected node** in the status line — `spec › containers ›
  [0] › image` — so a deeply nested value is never anonymous. `--path
  <auto|always|never>` and `structured.path` decide when it is shown; the
  status line also names the format it read.
- **An outline for structured documents.** `t` opens one entry per node, with
  scalar values previewed and trimmed, built the first time it is opened.
- **Search over every field of a node.** A hit may be in a key, a value, or a
  YAML comment, tag or anchor name, and the right run is highlighted; jumping
  to a hidden match still expands the collapsed ancestors that hide it.
- **YAML read as a reader needs it**, not as a deserializer leaves it:
  comments, anchors, tags, `%YAML`/`%TAG` directives, block, flow and quoted
  scalar styles and multi-document streams all survive into the view. An alias
  is shown as `*name` and never expanded — the honest reading, and the one that
  makes an alias bomb cost what the source costs — and a merge key (`<<`) is
  shown rather than performed. Types follow the YAML 1.2 core schema.
- **JSON read strictly** — RFC 8259 and nothing else, parsed by diple itself so
  that source order, duplicate keys, the lexical spelling of numbers (`1e6`
  stays `1e6`) and byte spans all survive. Duplicate keys are kept and both
  shown: a DOM that keeps the last one silently deletes part of the document.
  Comments and trailing commas are refused, and `.jsonc` is deliberately not a
  recognised extension.
- **New options.** `--format <auto|markdown|json|yaml>`, `--structured-indent
  <COLUMNS>`, `--path <auto|always|never>`, the top-level `format` key and a
  `[structured]` section (`indent = 2`, `path = "auto"`, `show_indices = true`,
  `collapsed_summary = true`). Every one has a default that needs no
  configuration, and all of them are settable at `:` and completed by `Tab`.
- **Long keys and scalars follow `wrap`** like any other content: a value too
  wide for the terminal wraps with its continuation indented past its key, so
  the pair still reads as a pair, and `--no-wrap` leaves the row full width for
  `h`/`l`. A row whose own indentation has already filled the terminal is left
  long either way, because wrapping it would give mostly blank margin. A
  quoted scalar is escaped the way its own dialect escapes, so what is between
  the quotes is never ambiguous and an embedded newline is visible rather than
  flattened to a space.
- **A `DOCUMENT FORMATS` section in the man page**, and the shell completions
  now offer the new options and their values.

### Changed

- The product is an interactive terminal reader for **structured documents**;
  the crate description, keywords and the Debian, RPM and Arch package
  descriptions say so. `tui` and `mermaid` gave way to `json` and `yaml` in the
  crates.io keywords, which are capped at five.
- Non-interactive output covers the new formats: a piped JSON or YAML document
  is written as plain text, fully expanded and in source order, so `diple
  response.json | head -20` is readable and reproducible. Minified JSON is
  never printed back.
- Tabs and splits are format-independent: `:open` detects a format exactly the
  way the command line does, so a Markdown document and a YAML document can
  sit side by side in one session.
- The help overlay and the key hints follow the open document's capabilities —
  the link keys are offered where links exist, the structure keys where
  structure does, and an action that cannot apply says so rather than doing
  nothing.

### Fixed

- **A Markdown document can no longer send escape sequences to the
  terminal.** Text in headings, paragraphs, lists, tables, quotes, footnotes
  and code blocks reached the output unfiltered, so a hostile file could set
  the window title, recolour the screen or — through the text of a link in a
  terminal with OSC 8 hyperlinks — reset the terminal outright. Markdown text
  now goes through the same sanitising JSON and YAML already did: controls,
  C1 characters and bidirectional overrides are drawn as `U+FFFD`, in the
  pager and in piped output alike. 1.x behaved the same way.

- **A search opens only the path to the match it lands on.** The prompt
  searches as you type and revealed every match it passed through on the way,
  so `/spec` in a collapsed Kubernetes manifest left `metadata` open because
  `sp` had matched `namespace` — and `Esc` left it open too. The folds are now
  put back before each new preview and when the search is cancelled. A match
  on a container's own key no longer opens that container either: its key row
  is visible while it is collapsed, so only its ancestors open.

- **A key that would read as something else is quoted**, in the document and
  in the path alike. `{"a: b": "c"}` showed `a: b: "c"`, an empty key showed
  as nothing, a line break in a key became an invisible space, and the path
  `a.b › x › y › [0]` could not say whether `x › y` was one key or two, or
  whether `[0]` was an index or a key. Such keys are now shown quoted and
  escaped — `"a: b": "c"`, `"x › y"`, `"[0]"` — while `metadata:` stays as it
  is. A YAML key keeps the quotes its source gave it.

- **Piped JSON or YAML that was cut short is an error, not Markdown.**
  `curl … | diple` on a response that ended early showed the half-document as
  garbled Markdown and exited 0. Anonymous input that parses cleanly as JSON,
  or as a YAML stream detection would have claimed, until the input runs out
  inside a value now gets the parse error a stated format gets, with a hint
  that `--format markdown` reads it as text. Prose that merely fails to parse
  as either still opens as Markdown.

### Breaking

- **The public Rust API changed.** The Markdown-specific AST is no longer the
  document: `diple::document` now exposes a format-neutral `DocumentModel`
  (Markdown or structured) with the vocabulary the application actually uses —
  folds, outline, path, search, capabilities — plus `SourceDocument`,
  `FormatRequest` and a `load` entry point. Code that reached for the old
  `document::parse` and the Markdown `Document` type must move to
  `DocumentModel` and its `as_markdown()` escape hatch. The alternative was to
  keep a Markdown-only type under a generic-sounding name, which would be a
  worse long-term boundary than a major-version break.
- **The minimum supported Rust version is now 1.81** (was 1.80), because the
  YAML parser diple builds on requires it. It was raised deliberately and on
  its own.
- No key, configuration key or command-line option was removed, renamed or
  given a different meaning. Existing configuration files stay valid, existing
  bindings keep doing what they did, and `diple README.md` and `cat README.md |
  diple` behave exactly as before.

## [1.2.0] - 2026-08-26

Several documents in one session: side by side, stacked or in tabs.

### Added

- **Several documents in one session.** `:open <side-by-side|stacked|tab>
  <path>` opens another document beside the current one, above and below it,
  or in a tab of its own. `Tab` completes the target and then the path, and a
  relative path that is not in the working directory is looked for next to the
  document that is already open. `vsplit` and `split` are accepted for the two
  split targets.
- **Navigation between the open documents.** `Ctrl-W` moves the keyboard to
  the other pane of a split, `Ctrl-N` and `Ctrl-P` walk the tabs, and `Alt-1` …
  `Alt-9` select a tab by the number the new tab bar prints in front of its
  name. The tab bar only appears once a second tab is open, and clicking a
  label selects that tab; clicking into a pane moves the keyboard there, while
  the wheel scrolls whichever pane the pointer is over. `focus_other_pane`,
  `next_tab` and `previous_tab` are bindable in `[keys]` like every other
  action.
- **`:close`**, which closes the focused document but never ends the session,
  and **`:qa`**, which ends it whatever is open.

### Changed

- `q` and `:q` now close the focused document, and only leave when it is the
  last one open — `Ctrl-C` still ends the session immediately. With a single
  document, which is every session that never runs `:open`, nothing about
  either key changes.
- A setting typed at `:` applies to every open document rather than only to
  the pane it was typed in: a setting is a property of the session.

## [1.1.0] - 2026-08-25

A command line for changing settings while reading, two themes, and the
reading defaults the width limit was added for.

### Added

- **A `:` command line for changing settings while reading.** Every key the
  configuration file has is settable at runtime under the same name, dotted for
  a section: `:center = false`, `:theme crt`, `:table.mode compact`. The
  separator may be `=` or a space, a key on its own reports its current value,
  and `Tab` completes — the key first, then the value once a separator is
  typed, filling in as much as is unambiguous and listing the rest in the
  status line. `:help` shows every setting with the values it accepts and the
  default it started from, `:q` quits, `Esc` leaves without applying. Changes
  last for the session; the configuration file is not written.

- **A `crt` theme.** An early-nineties film's idea of a computer: phosphor
  green on a screen the theme paints itself, amber for anything alarming, and
  contrast carried by brightness and reversed video rather than by hue.
  Emphasis is underlined instead of slanted and code blocks are not syntax
  coloured, because neither was a thing a terminal of that era could do — and a
  dozen highlighter hues would undo the two colours the theme is built from.
  Select it with `theme = "crt"`, `--theme crt` or `:theme crt`.

- **A `cyberpunk` theme.** A netrunner console: cyan on near-black, with
  crimson kept for the chrome and the alarms — table borders, list and fold
  markers, warnings, the current search match — so anything red on the screen
  is something worth looking at. It paints its own background like `crt` but
  keeps italics and syntax colouring, being a bitmapped console rather than a
  monochrome tube. Select it with `theme = "cyberpunk"`, `--theme cyberpunk` or
  `:theme cyberpunk`.

### Fixed

- **The line width limit and centring are now on by default.** `max_width` and
  `center` shipped as `0` and `false`, so a fresh installation still laid every
  document out across the full terminal — the very reading problem the two
  settings were added to solve, left switched off. They now default to
  `max_width = 160` and `center = true`, so a wide terminal gets a comfortable
  measure in the middle of the screen without any configuration. Nothing
  changes on a terminal of 160 columns or fewer, since the limit only ever
  narrows. Set `max_width = 0` and `center = false` (or pass `--max-width 0
  --no-center`) for the previous behaviour.

- **The table of contents sizes itself to its headings.** The sidebar was a
  flat 28 columns wide whatever it held, so headings of any length were cut
  off with an ellipsis and there was no way to see the rest of them. It is now
  as wide as its widest entry needs, bounded by 40 columns — the same number
  as the minimum document width — and by a third of the screen. Documents with
  short headings get a narrower sidebar and hand the columns back to the text.
  What still does not fit scrolls: while the sidebar has the focus, `h`/`l`
  (`←`/`→`) move the outline sideways rather than the document, and the key
  hints offer them only while something is cut off.

- **The mouse can select text in the document again.** diple asked the
  terminal for the whole of `EnableMouseCapture`, which includes drag
  reporting (`1002`) and any-motion reporting (`1003`) — events it never
  handled and threw away, but which cost the terminal its own text selection,
  because a drag forwarded to diple is a drag the terminal cannot select with.
  It now asks only for button presses and releases in SGR encoding (`1000`
  and `1006`), which is exactly what the wheel and the clickable sidebars
  need. Where a terminal still reserves plain dragging for the application,
  `m` (`toggle_mouse`) hands the mouse back entirely: dragging selects and
  copies as it does in any other program, and `m` again restores the wheel and
  the clickable sidebars. The key hints show it while the terminal reports a
  mouse at all.

## [1.0.0] - 2026-08-24

First stable release.

### Fixed

- **A signal no longer leaves the terminal unusable.** `SIGTERM`, `SIGHUP`,
  `SIGINT` and `SIGQUIT` killed diple mid-frame, so the shell that came back
  was still in raw mode on the alternate screen with the cursor hidden and
  mouse reporting on — unusable until `reset`. They now run the same
  restoration as every other exit path and then re-raise with the default
  disposition, so the exit status still reports the signal. Ctrl-C and panics
  were never affected.

### Added

- `max_width` caps the line width before wrapping, and `center` puts the
  document in the middle of the screen with equal margins on both sides.
  Available as `--max-width <COLUMNS>` and `--center` / `--no-center` too. The
  sidebars keep the screen edges: a centred document has the table of contents
  to its left and the key hints to its right, outside the text. Piped output
  honours `max_width` but is never padded.

### Changed

- A jump from the table of contents (`Enter`) keeps the focus in the sidebar
  instead of returning to the document, so `j`/`k` go on walking the outline
  and several headings can be visited in a row. `Esc` or `t` leaves the
  sidebar.

- **The project is now called `diple`.** The binary, the crate and the
  configuration all follow: run `diple`, configure it in
  `~/.config/diple/config.toml` and override it with `DIPLE_*` environment
  variables. There is no compatibility shim — `mdless`, `~/.config/mdless/`
  and `MDLESS_*` are gone. Move your configuration file and rename your
  environment variables when upgrading from 0.2.0.

## [0.2.0] - 2026-08-23

Feature-complete for 1.0, released as a minor version because four of the
release checks cannot be performed without real hardware and root, and are
therefore still open:

- the terminal matrix (GNOME Terminal, Konsole, Kitty, Alacritty,
  WezTerm, tmux, SSH, macOS, terminals without true colour or images) — see
  `docs/terminal-compatibility-checklist.md`
- that diagram images actually appear in an image-capable terminal
- installing and purging the `.deb` and `.rpm` as root
- resizing a multi-megabyte document, which still re-lays out the whole
  document (144 ms at 1 MB, 561 ms at 4 MB)

`1.0.0` is reserved for the release in which those are verified.

### Changed

- **Building from source now requires a C compiler.** The default syntax
  regex engine is oniguruma, because it is what makes the startup budget
  reachable: measured in a 100x24 terminal, time to first frame is p50 17 ms
  against p50 73 ms with the previous pure-Rust engine, where the budget
  requires p50 < 30 ms. Behaviour is unaffected — glibc, static musl and the
  pure-Rust build produce byte-identical output including highlighting.
  Environments without a C toolchain can build the previous engine with
  `--no-default-features --features syntax-fancy`; for `*-linux-musl` the
  compiler must be a real musl compiler (`musl-tools`).

### Added

- Interactive terminal Markdown reader: the document is rendered as structure,
  not as coloured text, and read in an alternate screen that leaves no output
  in the shell's scrollback.
- Semantic navigation: heading-to-heading jumps, a table-of-contents sidebar
  and a viewport anchored to `(node, offset)` so position survives resizing,
  folding and re-layout.
- Key hints sidebar (`K`, `key_hints`, `--key-hints`/`--no-key-hints`): a
  right-hand list of the commands available right now, grouped and labelled,
  following the mode and the cursor context, with labels read from the live
  key map so custom bindings are shown.
- Collapsible sections with per-section and document-wide fold commands;
  searching reveals a match inside a collapsed section.
- Incremental full-text search with wrap-around and match highlighting.
- Terminal-aware rendering: width-driven table layout with scroll, wrap and
  compact modes, syntax-highlighted code blocks, nested lists, blockquotes,
  footnotes and task lists.
- Link interaction: selection, in-document anchor jumps, external opening and
  OSC 8 hyperlinks where the terminal supports them.
- Mermaid support: a built-in renderer for the supported flowchart subset,
  `mmdc` integration, terminal image protocols (Kitty, Sixel, iTerm2) and a
  source fallback that is always reachable, including on the non-interactive
  path.
- Graceful degradation for colour depth, Unicode box drawing, mouse, images,
  OSC 8 and a missing `mmdc`, with `--print-capabilities` to explain every
  decision.
- Configuration file (`~/.config/mdless/config.toml`) with rebindable keys,
  `--check-config` validation reporting path, line, key, value and expected
  form, and `MDLESS_*` environment overrides.
- Non-interactive output for pipes and CI, honouring `--color always`,
  `--color never` and `--width`.
- Packaging: `.deb`, `.rpm`, an Arch `PKGBUILD`, standalone Linux tarballs, a
  man page and bash/zsh/fish completions.

[Unreleased]: https://github.com/godspeed-you/diple/compare/v2.0.0...main
[2.0.0]: https://github.com/godspeed-you/diple/releases/tag/v2.0.0
[1.2.0]: https://github.com/godspeed-you/diple/releases/tag/v1.2.0
[1.1.0]: https://github.com/godspeed-you/diple/releases/tag/v1.1.0
[1.0.0]: https://github.com/godspeed-you/diple/releases/tag/v1.0.0
[0.2.0]: https://github.com/godspeed-you/diple/releases/tag/v0.2.0
