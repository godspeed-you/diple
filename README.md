# diple

**`less` for structured documents.**

[![CI](https://github.com/godspeed-you/diple/actions/workflows/ci.yml/badge.svg)](https://github.com/godspeed-you/diple/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/godspeed-you/diple?sort=semver)](https://github.com/godspeed-you/diple/releases/latest)
[![License: MIT](https://img.shields.io/github/license/godspeed-you/diple)](LICENSE)
[![Rust 1.81+](https://img.shields.io/badge/rust-1.81%2B-orange?logo=rust)](rust-toolchain.toml)

An interactive terminal reader for structured documents. Markdown, JSON and
YAML are all first-class: semantic navigation, collapsible structure,
terminal-aware tables, syntax-highlighted code and Mermaid diagrams.

diple does not merely syntax-highlight structured formats. It parses their
structure, so folding, navigation, search and the outline operate on semantic
nodes — a JSON object folds as an object, a YAML mapping entry is one unit, a
Markdown section is one section. The consequence is the same for all three:
the document model, not the rendered text, is what you move through, so it
stays correct when the terminal is resized.

```bash
diple README.md
diple deployment.yaml
diple response.json

kubectl get deployment nginx -o yaml | diple
curl -s https://api.example.com/state | diple
```

![diple in action](docs/demo.gif)

## Features

- **Interactive pager** — never dumps into your shell scrollback
- **Three formats, one interaction model** — Markdown, JSON and YAML, detected
  from the file name and the content, or stated with `--format`
- **Semantic navigation** — jump by heading in Markdown and by container in
  JSON/YAML, never by line number; `H`/`L` move out to the parent and in to
  the first child
- **Collapsible structure** — fold any section, object, array or mapping,
  `zM`/`zR` for the whole document; a collapsed container says what it holds
  (`{4 members}`, `[3 items]`)
- **Full-text search** — searches document content, including YAML keys,
  comments, tags and anchors, and expands collapsed ancestors of a match
- **Outline** — sidebar reflecting the real hierarchy: headings for Markdown,
  the node tree with value previews for JSON and YAML
- **Where am I** — the path of the selected node (`spec › containers › [0] ›
  image`) in the status line for JSON and YAML
- **Key hints sidebar** — `K` shows, on the right, the commands available right
  now, following the mode and the cursor context
- **Command line** — `:` changes any setting while running, with completion and
  a `:help` listing every key, its values and its default
- **Several documents at once** — `:open` puts another document side by side,
  above and below, or in a tab of its own, with `Tab` completion for the path
- **Terminal-aware tables** — column widths computed from content and terminal
  width, with wrapping or horizontal scrolling
- **Syntax highlighting** — fenced code blocks, optional line numbers
- **YAML as it was written** — comments, anchors, tags, block scalars and
  multi-document streams survive; an alias is shown as `*name` rather than
  expanded, and a merge key (`<<`) is shown rather than performed
- **JSON as it was written** — strict RFC 8259, source order, duplicate keys
  kept and numbers left in their own spelling (`1e6` stays `1e6`)
- **Links** — keyboard selection, opening via `xdg-open`, OSC 8 hyperlinks
  where supported
- **Mermaid diagrams** — rendered natively in the terminal, as images via
  `mmdc` where the terminal supports it, with a deterministic source fallback
- **Graceful degradation** — works over SSH, in tmux, without true color,
  without Unicode and without image support

## Installation

### Packages

```bash
# Debian / Ubuntu
sudo apt install ./diple_2.0.0_amd64.deb

# Fedora / RHEL
sudo dnf install ./diple-2.0.0.x86_64.rpm

# Arch Linux
cd packaging/arch && makepkg -si
```

### From source

```bash
cargo install --path .
```

Requires Rust 1.81 or newer and a C compiler. The C compiler is needed for
the oniguruma regex engine, which diple uses by default because it is what
makes the startup budget reachable — a syntax definition is compiled on first
use, and that cost lands on the first frame. If you cannot provide a C
toolchain, build with the pure-Rust engine instead:

```bash
cargo install --path . --no-default-features --features syntax-fancy
```

Both produce byte-identical output; the pure-Rust engine is simply slower to
show the first frame (p50 73 ms versus 17 ms in a 100x24 terminal). For
`*-linux-musl` targets the C compiler must be a real musl compiler
(`musl-tools`), because the host's glibc `cc` emits `_FORTIFY_SOURCE` symbols
that musl does not provide:

```bash
CC_x86_64_unknown_linux_musl=musl-gcc \
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
cargo build --release --target x86_64-unknown-linux-musl
```

Optional: `mmdc` (`npm install -g @mermaid-js/mermaid-cli`) for Mermaid
diagrams that the built-in renderer does not cover.

## Usage

```bash
diple README.md            # read a file
diple deployment.yaml      # …of any supported format
diple response.json
cat README.md | diple      # read from stdin
diple < README.md          # read from a redirect
git show HEAD:README.md | diple
kubectl get deployment nginx -o yaml | diple
curl -s https://api.example.com/state | diple
diple --format json message.txt   # state the format yourself
```

When output is not a terminal, diple prints the rendered document as plain
text and exits — fully expanded and in source order, so `diple response.json |
head -20` and CI usage behave sensibly.

### Supported formats

| Format | Recognised by name | Semantic units |
|---|---|---|
| Markdown | `.md` `.markdown` `.mdown` `.mkd` | headings, sections, lists, tables, code blocks, links |
| JSON | `.json` | objects, arrays, members, items, scalars |
| YAML | `.yaml` `.yml` | documents, mappings, sequences, entries, scalars, anchors, aliases, tags, comments |

The format is decided in this order, and the first rule that applies wins:

1. **`--format <auto|markdown|json|yaml>`** — final; it beats both the file
   name and the content. `diple --format markdown config.yaml` reads a YAML
   file as prose, `diple --format json file.data` insists on JSON.
2. **The file name**, per the table above.
3. **The content**, but only a confident guess: a document that opens with `{`
   or `[` and parses as strict JSON, or a YAML stream whose roots are all
   non-empty collections with at least one signal prose does not produce — a
   mapping nested under a key or a mapping of several entries inside a list
   (with identifier-like keys), an anchor some alias uses, a `!!` tag or a
   `%YAML` directive. A Markdown list whose items contain a
   colon, `Status: done` over `Owner: alice`, and lists separated by `---`
   all parse as YAML too, and stay Markdown; so does a flat YAML file piped
   in, which needs `--format yaml`. This is what makes
   `kubectl get … -o yaml | diple` work with nothing to configure.
4. **Markdown**, the fallback. Prose and anything else ambiguous stay Markdown.

A format *stated* by `--format` or by the file name is binding: a `config.yaml`
that does not parse is reported as a YAML error and diple exits non-zero
rather than opening it as Markdown and looking almost right. A format merely
*guessed* from the content of anonymous input falls back to Markdown instead,
since nothing had claimed it — unless the input is unmistakably JSON or YAML
that was cut short or broken, as a cut-off pipeline leaves it. That is
reported as the error it is; `--format markdown` reads it as text.

JSON with comments is not a format diple reads, so `.jsonc` is deliberately
not recognised.

### Key bindings

Structure keys mean the same thing in every format; what counts as structure
follows the document. In Markdown that is the headings, in JSON and YAML the
containers — objects, arrays, mappings and sequences.

| Key | Action |
|---|---|
| `j` `k` `↓` `↑` | scroll |
| `Space` `b` `PgDn` `PgUp` | page |
| `g` `G` | top / bottom |
| `h` `l` `←` `→` | scroll horizontally |
| `/` `n` `N` | search, next, previous |
| `[` `]` | previous / next heading, or container |
| `{` `}` | previous / next sibling at the same or a higher level |
| `H` `L` | out to the enclosing node / in to the first node inside |
| `Enter` | toggle the section or container under the cursor, or open the selected link |
| `za` `zc` `zo` | toggle / collapse / expand the current section or container |
| `zM` `zR` | collapse / expand everything |
| `Tab` `Shift-Tab` `o` | select links, open the selected one (Markdown) |
| `t` | toggle the outline (table of contents) |
| `K` | toggle the key hints sidebar |
| `m` | hand the mouse back to the terminal, to select text with it |
| `:` | command line: `:center = false`, `:theme crt`, `:help` |
| `s` | toggle Mermaid source view |
| `Ctrl-W` | focus the other pane of a split |
| `Ctrl-N` `Ctrl-P` | next / previous tab |
| `Alt-1` … `Alt-9` | tab by number |
| `?` | help |
| `q` | close the document; the last one quits |

Full list, including how to rebind: [docs/keybindings.md](docs/keybindings.md).

### Several documents at once

```text
:open side-by-side ../notes.md   # beside the current document
:open stacked CHANGELOG.md       # above and below it
:open tab docs/configuration.md  # in a tab of its own
:close                           # close this document, keep the session
```

`Tab` completes the target and then the path; a relative path that is not in
the working directory is looked for next to the document that is open.
`Ctrl-W` moves the keyboard between the two panes of a split, `Ctrl-N` and
`Ctrl-P` walk the tabs, and `Alt-1` … `Alt-9` pick one by the number the tab
bar prints. A tab bar appears on the top row only once a second tab is open.
Settings apply to the whole session: `:theme crt` in one pane changes both.

### Options

```text
diple [OPTIONS] [FILE]

  --format <auto|markdown|json|yaml>
  --structured-indent <COLUMNS>      --path <auto|always|never>
  --theme <auto|dark|light|NAME>     --color <auto|always|never>
  --width <COLUMNS>                  --max-width <COLUMNS>
  --center / --no-center             --mouse / --no-mouse
  --toc / --no-toc                   --key-hints / --no-key-hints
  --line-numbers / --no-line-numbers
  --wrap / --no-wrap
  --mermaid <auto|terminal|mmdc|source>
  --mermaid-images <auto|always|never>
  --config <PATH> / --no-config
  --print-capabilities  --check-config  --debug
  -h, --help  -V, --version
```

## Configuration

`~/.config/diple/config.toml`:

```toml
format = "auto"  # auto | markdown | json | yaml
theme = "auto"   # auto | dark | light | crt | cyberpunk
mouse = true
toc = false
key_hints = false
max_width = 160
center = true

[structured]     # JSON and YAML only; Markdown ignores this section
indent = 2
path = "auto"

[table]
mode = "auto"

[code]
line_numbers = false

[mermaid]
backend = "auto"

[keys]
quit = "q"
next_heading = "]"
```

Command-line options override the configuration file. Full reference:
[docs/configuration.md](docs/configuration.md).

Validate a configuration without opening a document:

```bash
diple --check-config
```

## Using diple as a Git pager

```bash
git config core.pager diple
git show HEAD:README.md | diple
```

## Documentation

- [Configuration reference](docs/configuration.md)
- [Keybinding reference](docs/keybindings.md)
- [Mermaid behavior](docs/mermaid.md)
- [Terminal compatibility checklist](docs/terminal-compatibility-checklist.md)
- `man diple`

## Troubleshooting

**The document opened as the wrong format.** The status line names the format
diple decided on. A known extension settles it; otherwise the content has to
make a confident case, so prose, a Markdown list and a single-line `key:
value` all stay Markdown rather than being claimed by a permissive parser —
while a `.data` file that really is a JSON object is read as one. State the
format when the guess is not the one you wanted: `diple --format yaml -`,
`diple --format json message.txt`. `--format` beats both the file name and the
content, so it also works the other way round: `--format markdown config.yaml`
reads a YAML file as prose.

**A YAML or JSON file will not open.** A format stated by `--format` or by the
file name is binding, so a parse error is reported — with the file name, the
line and column, the offending source and a caret — and diple exits non-zero
instead of falling back to Markdown. Fix the document, or read it as Markdown
with `--format markdown` to look at it as text.

**YAML does not do what my program does with it.** diple reads a document, it
does not resolve a configuration: an alias is shown as `*name` and never
expanded, a merge key (`<<: *defaults`) is shown rather than performed, and
`%TAG` handles are not substituted. Types follow the YAML 1.2 core schema
only, so `yes` is a string, not a boolean, and a timestamp is a string. A
collection used as a mapping key (`? …`, `[1, 2]: x`) is shown as it was
written rather than as a subtree of its own.

**JSON with comments or trailing commas is refused.** diple is strict RFC 8259
on purpose, and `.jsonc` is not recognised, so such a file falls back to
Markdown. Duplicate keys are the opposite case: both are kept and shown, since
dropping one would silently delete part of the document.

**A big file takes a moment.** diple reads the whole input and parses it once;
there is no size limit and no truncation, and the outline and the syntax
highlighting are computed only when they are needed. Nesting is capped at 1024
levels, which is reported as an error rather than a crash.

**Colors look wrong or are missing.** Run `diple --print-capabilities`; it
reports what was detected and the evidence for each decision. `NO_COLOR` and a
non-terminal stdout always disable color. Force with `--color always`.

**Mermaid diagrams show as source.** See
[docs/mermaid.md](docs/mermaid.md#troubleshooting). Inside tmux, image
protocols need `set -g allow-passthrough on`.

**Box drawing shows as question marks.** Your locale is not UTF-8; diple falls
back to ASCII automatically, so check `LANG`/`LC_ALL` if you expected Unicode.

**Tables are cut off.** Scroll horizontally with `h`/`l`, or set
`[table] mode = "wrap"` to wrap cells instead.

**The terminal looks broken after a crash.** diple restores the terminal on
every exit path including panics; if something still slipped through, `reset`
fixes it — and please report it: diple treats terminal
corruption as a release blocker.

## Development

```bash
cargo test                 # unit, integration and snapshot tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
cargo bench
```

Snapshot tests use [insta](https://insta.rs/); review changes with
`cargo insta review`.

## License

MIT — see [LICENSE](LICENSE).
