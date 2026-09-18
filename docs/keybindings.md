# diple — Keybinding Reference

All bindings are configurable in the `[keys]` section of the configuration file
(see [configuration.md](configuration.md)). Press `?` or `F1` inside diple for
the same list with your own overrides applied.

Keys follow `less` and Vim conventions where practical.

## One interaction model, format-specific meaning

No key changes its job between formats; what the document *is* changes what
its structure is. In Markdown the structure is the headings and their
sections, in JSON and YAML it is the containers — objects, arrays, mappings
and sequences. Scalars are not stops, so `]` on a long array is not another
`j`.

| Key | Markdown | JSON/YAML |
|---|---|---|
| `Enter` | toggle the section under the cursor, or open the selected link | toggle the container under the cursor |
| `[` `]` | previous / next heading | previous / next container |
| `{` `}` | previous / next heading at the same or a higher level | previous / next sibling, climbing out of a finished branch |
| `H` `L` | out to the parent heading / in to the first child heading | out to the enclosing node / in to the first node inside |
| `za` `zc` `zo` | section fold | container fold |
| `zM` `zR` | all sections | all containers |
| `t` | table of contents | structure outline, one entry per node |
| `/` `n` `N` | search the prose | search keys, values, YAML comments, tags and anchors |
| `Tab` `Shift-Tab` `o` | select and open links | — (JSON and YAML have no links) |

Every key that works in Markdown works exactly as it did before 2.0; `H` and
`L` were unbound and are the only additions.

## Paging and scrolling

| Keys | Action name | Description |
|---|---|---|
| `j`, `↓` | `scroll_down` | scroll down one line |
| `k`, `↑` | `scroll_up` | scroll up one line |
| `PgDn`, `Space` | `page_down` | page down |
| `PgUp`, `b` | `page_up` | page up |
| `Ctrl-D` | `half_page_down` | half page down |
| `Ctrl-U` | `half_page_up` | half page up |
| `h`, `←` | `scroll_left` | scroll left (wide tables, unwrapped code, the TOC) |
| `l`, `→` | `scroll_right` | scroll right |
| `g` | `top` | top of document |
| `G` | `bottom` | bottom of document |

## Search

| Keys | Action name | Description |
|---|---|---|
| `/` | `search` | open the search prompt |
| `n` | `next_search` | next search result |
| `N` | `previous_search` | previous search result |

Search runs over document content, not over rendered terminal lines. Jumping to
a match inside a collapsed section expands the collapsed ancestors of the match
automatically. In JSON and YAML the searched content is every field of a node —
its key or `[index]`, its value, and a YAML node's comments, tags and anchor
names — so `/nginx` finds the key `nginx` and the image `nginx:1.27` alike.

## Structure navigation

| Keys | Action name | Description |
|---|---|---|
| `]` | `next_heading` | next heading (Markdown) or container (JSON/YAML) |
| `[` | `previous_heading` | previous heading or container |
| `}` | `next_heading_same_level` | next sibling at the same or a higher level |
| `{` | `previous_heading_same_level` | previous sibling at the same or a higher level |
| `H` | `parent_node` | move out to the enclosing node |
| `L` | `first_child` | move in to the first node inside |

Structural jumps are semantic: they target the node, not a line number, and
they remain correct after a terminal resize.

`[` from a row that is not itself structural lands on the node it belongs to —
the heading of the paragraph you are in, or the container of the entry you are
on — which is the same "back goes to what I am under" rule in both cases.
`}` and `{` stay within one level while there are siblings left and then climb
out of the finished branch, matching Markdown's "same or higher level".

`H` and `L` are the tree axis: `h` and `l` scroll sideways, and their shifted
forms move one level of the hierarchy, which is the same axis one step up.
In Markdown, `H` goes from a body node to its heading and from a heading to
its parent heading, and `L` goes to the first child heading. Neither key was
bound before 2.0, so nothing changed meaning.

## Folding

| Keys | Action name | Description |
|---|---|---|
| `Enter` | `activate` | toggle the section or container under the cursor (or open a selected link) |
| `za` | `toggle_fold` | toggle the current section or container |
| `zc` | `collapse_fold` | collapse it |
| `zo` | `expand_fold` | expand it |
| `zM` | `collapse_all` | collapse everything foldable |
| `zR` | `expand_all` | expand everything foldable |

A collapsed section shows `▶ Heading`; an expanded one shows `▼ Heading`.
Fold state lives for the current session only.

In JSON and YAML every container is foldable and nothing else is: a scalar has
nothing inside it, an alias is a reference rather than a subtree, and the
document root is not a fold target — collapsing it would replace the shape of
the file with a single brace. A collapsed container keeps its own row and says
what it holds in place of its contents: `{4 members}` for a JSON object,
`{4 entries}` for a YAML mapping, `[3 items]` for either kind of sequence. An
empty container is drawn `{}` or `[]` whether or not it is folded. `za` on a
top-level scalar entry therefore reports that there is nothing to fold.

## Links

| Keys | Action name | Description |
|---|---|---|
| `Tab` | `next_link` | select the next link |
| `Shift-Tab` | `previous_link` | select the previous link |
| `o` | `open_link` | open the selected link with the configured opener |
| `Enter` | `activate` | open the selected link, or follow an internal `#anchor` |

Links are a Markdown capability; in a JSON or YAML document the link keys do
nothing and the key hints leave them out, while `Enter` still folds.

Internal links (`[text](#anchor)`) jump within the document. External links are
handed to `links.opener` (default `xdg-open`). Where the terminal supports OSC 8,
links are also emitted as native terminal hyperlinks.

## View

| Keys | Action name | Description |
|---|---|---|
| `t` | `toggle_toc` | toggle the outline sidebar (table of contents) |
| `K` | `toggle_key_hints` | toggle the key hints sidebar |
| `m` | `toggle_mouse` | toggle mouse reporting (off: select text with the mouse) |
| `:` | `command_prompt` | open the command line to change a setting |
| `s` | `toggle_mermaid_source` | toggle Mermaid source view for the diagram at the cursor |
| `?`, `F1` | `help` | show the help overlay |
| `Esc` | `cancel` | close the overlay, prompt or sidebar |
| `q`, `Ctrl-C` | `quit` | close the focused document; the last one quits |

## Documents

| Keys | Action name | Description |
|---|---|---|
| `Ctrl-W` | `focus_other_pane` | move the keyboard to the other pane of a split |
| `Ctrl-N` | `next_tab` | show the next tab |
| `Ctrl-P` | `previous_tab` | show the previous tab |
| `Alt-1` … `Alt-9` | — | show that tab by its number |

`:open` (below) is what puts a second document on screen. Two panes share the
body left and right (`side-by-side`) or above and below (`stacked`); the pane
with the keyboard is the one whose status bar marks its file name. A tab bar
appears on the top row as soon as a second tab is open and prints each tab's
number in front of its name — that number is its `Alt` key.

`Alt-1` … `Alt-9` is one gesture with an argument rather than nine actions, so
it is the only binding `[keys]` cannot change. Everything else here is a
normal action name.

`q` and `:q` close the focused pane, then the focused tab, and only leave when
the last document is closed. `Ctrl-C` always leaves at once.

For Markdown the sidebar holds one entry per heading. For JSON and YAML it
holds one entry per node — containers by their key or `[index]`, scalars as
`key: value` with the value trimmed to 32 columns — so the outline is the
document's shape rather than a selection someone guessed at. It is built the
first time the sidebar is opened.

Inside the TOC sidebar, `j`/`k` move the selection and `Enter` jumps to the
heading or node; the section currently shown in the document is marked. A jump keeps
the focus in the sidebar, so `j`/`k` go on walking the outline and further
jumps need no reopening; `Esc` or `t` hands the keys back to the document.

The sidebar is as wide as its widest entry, up to 40 columns and never more
than a third of the screen. A heading longer than that is not truncated for
good: while the sidebar has the focus, `h`/`l` (or `←`/`→`) scroll the outline
sideways instead of the document, and the key hints offer them only while
something is actually cut off. Closing and reopening the sidebar returns it to
the left edge of the outline.

## The command line

`:` opens a command line for changing a setting while diple is running. The key
is the one the configuration file uses, dotted for a section, and the separator
is `=` or a space — `:center = false`, `:theme crt`, `:table.mode compact` all
work, as does a leading `set` for the muscle memory it comes from.

| Input | Effect |
|---|---|
| `:center` | report what `center` is currently set to |
| `:center = false` | set it, and lay the document out again |
| `Tab` | complete the key, or the value once a separator is typed |
| `:help` | every setting, its accepted values and its default |
| `:open side-by-side PATH` | open a document beside this one |
| `:open stacked PATH` | open a document above and below this one |
| `:open tab PATH` | open a document in a tab of its own |
| `:close` | close the focused document, never the session |
| `:q` | close the focused document; the last one quits |
| `:qa` | quit, whatever is open |
| `Esc` | leave the line without applying it |

`:open` completes its target and then its path with `Tab`, directory by
directory. A relative path is looked for in the working directory first and
then next to the document that is open, so `:open tab notes.md` works while
reading `docs/index.md`. `vsplit` and `split` are accepted for `side-by-side`
and `stacked`. A split is refused, with a message, on a terminal that cannot
give both panes a readable width — the document is not halved into two
unreadable columns.

A setting typed at `:` applies to every open document, not only to the pane it
was typed in.

Completion stops where the answer stops being unique: it fills in the longest
prefix every candidate shares and lists the rest in the status line. A key that
completes to exactly one match gains its ` = ` too, so the next keystroke is
already the value.

Changes apply immediately and last for the session — the configuration file is
never written, so a change that turns out badly is undone by restarting.

The key hints sidebar (`K`) is drawn on the right-hand edge and lists, grouped
and labelled, the commands that are available *right now* — it is not a static
copy of the help overlay. It follows the mode (normal, search prompt, TOC,
help) and the cursor context: horizontal scrolling appears only when the view
can actually scroll sideways, the link keys only when a link is visible, the
Mermaid source toggle only at or near a diagram, and `n`/`N` only while a
search has matches. Every key label is read from the live key map, so a
rebound key shows its new binding and an unbound action is left out entirely.
Where the mouse is enabled, clicking a row runs that command.

Opposed key pairs share a line (`j/k scroll`, `]/[ next/prev`, `zc/zo
collapse/expand`); if you unbind one half, the row collapses to the half that
still works.

The sidebar takes its width from the document area. In a terminal too narrow
to leave at least 40 columns for the document it hides itself, and if both
sidebars are open and only one fits, the table of contents wins. When the
groups are taller than the screen, the blank rows between them go first, then
whole groups in this order: Documents, Diagram, Links, Search, Fold, Headings — heading
navigation and folding outrank the generic pager rows, and `Move` and `View`
(which holds `q`, `?` and `K` itself) are the last to go.

## Key specification syntax

Values in `[keys]` use this syntax:

- single characters: `q`, `/`, `?`, `G`
- named keys: `enter`, `esc`, `space`, `tab`, `shift-tab`, `backtab`, `pgup`,
  `pgdn`, `up`, `down`, `left`, `right`, `home`, `end`, `f1` … `f24`
- modifiers: `ctrl-d`, `alt-enter`, `ctrl-shift-f5`
- multi-key sequences: written together (`za`, `zM`) or separated by spaces
  (`g g`, `z enter`)

A `[keys]` entry replaces **all** default bindings for that action. To bind
several keys to one action, use a list:

```toml
[keys]
quit = ["q", "ctrl-q"]
toggle_fold = "f t"
```

If a multi-key sequence fails to match, the last key is retried on its own — so
after a stray `z`, pressing `q` still quits.
