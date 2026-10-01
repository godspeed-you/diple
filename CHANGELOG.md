# Changelog

All notable changes to diple are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/1.1.0/) and the project uses
[semantic versioning](https://semver.org/).

`scripts/release.sh` and the CI `release:prepare` job cut the release notes from
the `## [<version>]` section matching the tag they are given, so every release
must have its own section here **before** it is tagged. Keep `## [Unreleased]`
at the top for work that has not shipped yet.

## [Unreleased]

## [2.0.0] - 2026-09-25 — Structured Documents

diple now supports JSON and YAML as first-class document formats alongside
Markdown. Navigation, folding, search and the outline operate on the document's
semantic structure instead of merely highlighting its syntax.

### Highlights

- Open JSON and YAML files directly or pipe them into diple. The format is
  detected automatically and can be selected explicitly with
  `--format <auto|markdown|json|yaml>`.
- Navigate and fold objects, arrays, mappings and sequences using the familiar
  structure keys. `H` and `L` move to the parent or first child.
- See the selected node's path in the status line and browse the complete
  document structure with the outline (`t`).
- Search covers keys, values and YAML metadata. Hidden matches are revealed
  without unnecessarily expanding unrelated branches.
- YAML comments, anchors, tags, directives and multi-document streams are
  preserved. JSON is parsed strictly while retaining source order, duplicate
  keys and the original spelling of numbers.
- Structured documents also work with tabs, splits, `:open` and non-interactive
  output.
- New settings control format detection, indentation, path display, array
  indices and collapsed summaries. Existing defaults work without additional
  configuration.

### Fixed

- Markdown content is sanitised before reaching the terminal, preventing
  embedded control and escape sequences from affecting the terminal.
- Cancelling or changing a search now restores folds, cursor and viewport
  correctly.
- Invalid or truncated JSON and YAML produce useful parse errors instead of
  being silently displayed as Markdown when the input clearly indicates a
  structured format.
- Improved rendering of unusual keys, YAML tags and quoted values, JSON files
  with a UTF-8 byte-order mark, and whitespace in Markdown.
- More conservative YAML detection prevents Markdown containing colons, lists
  or front matter from being misclassified.

### Compatibility

Existing Markdown workflows, configuration files and key bindings remain
compatible. No command-line option or configuration key was removed or changed
in meaning.

### Breaking changes

- The public Rust API now exposes the format-neutral `DocumentModel` instead of
  the previous Markdown-specific `Document`. Users of `document::parse` must
  migrate to the new `load` API and can use `as_markdown()` where access to the
  Markdown representation is required.
- The minimum supported Rust version is now 1.81 instead of 1.80.

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
