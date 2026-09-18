# diple 2.0 — implementation status and handover

**Status:** work in progress. The document layer is written and self-consistent;
the application layer has not been ported to it yet, so **the crate does not
compile at this commit**. Everything needed to finish is below.

**Normative source of truth:** `docs/diple-v2-structured-documents-spec.md`.
This file records *decisions and progress*, never requirements. Where the two
disagree, the spec wins.

---

## 0. How to build and test

The repository's `rust-toolchain.toml` says `stable`. On the machine this work
was done on, the installed `stable` toolchain is a stale 1.71.1 that cannot
even parse the registry, so every command was run with an explicit toolchain:

```bash
cargo +1.90.0 check --all-targets
cargo +1.90.0 test --all-targets --all-features
cargo +1.90.0 fmt --check
cargo +1.90.0 clippy --all-targets --all-features -- -D warnings
```

Use whatever toolchain is ≥ the MSRV; `rustup update stable` would also fix it.

**MSRV was raised 1.80 → 1.81** (`Cargo.toml`, `clippy.toml`), deliberately and
on its own, because `granit-parser` requires it — spec §10.2 asks for exactly
that decision to be made explicitly rather than smuggled in. CI, packaging docs
and the README still need to be checked for a hard-coded 1.80.

---

## 1. What is done

### Commit `test: pin the Markdown reading experience before the 2.0 refactor`

`src/app/state/characterization.rs` — 15 cross-cutting tests over `App` that
assert what a *reader* notices: heading navigation, same-level navigation,
section folding, `zM`/`zR`, `Enter`, search ordering, search reveal through
collapsed ancestors, the table of contents, resize anchoring, links, and that
Markdown keeps its own node kinds. These are the regression net for the port.
They were green before any refactoring started.

### The document layer (this commit)

```
src/document/
  mod.rs            format-neutral vocabulary + module map
  source.rs         SourceDocument (Rc<str>-backed), SourceSpan, line_col
  format.rs         DocumentKind, FormatRequest, extension table, JSON sniff
  capabilities.rs   DocumentCapabilities (+ MARKDOWN / STRUCTURED presets)
  folds.rs          FoldId, FoldState  (moved out of markdown/sections.rs)
  outline.rs        OutlineEntry, scalar preview trimming
  path.rs           DocumentPath, PathSegment, breadcrumb + JSON Pointer
  search.rs         SearchIndex, SearchIndexBuilder, Match, MatchField
  error.rs          DocumentError with source excerpt and caret
  model.rs          DocumentModel  ← THE FORMAT BOUNDARY
  load.rs           load(FormatRequest, SourceDocument) -> LoadedDocument
  markdown/         ast, parser, sections, anchors, links, search
  structured/       ast, build (shared JSON+YAML model and builder)
  json.rs           strict RFC 8259 parser, written here on purpose
  yaml.rs           granit-parser adapter
src/util/text.rs    terminal-escape sanitisation
src/layout/structured.rs   semantic rows for JSON/YAML
```

All of the above carries its own unit tests. None of it has been *run* yet,
because the crate does not link until the app layer is ported — expect the
usual crop of small fixes on first compile.

---

## 2. Architectural decisions already taken

These resolve the choices spec §33 explicitly delegates to implementation, plus
a few the spec leaves open. **Do not silently revisit them; they are wired
through the model, the layout engine and the tests.**

### D1 — `DocumentModel` is an enum, not a trait

`document/model.rs`. Markdown keeps its own AST; JSON and YAML share
`StructuredDocument`. The application above the boundary only ever asks
capability-shaped questions (`fold_at`, `next_structural`, `outline`, `path`,
`is_hidden`, `reveal`, …). Two escape hatches exist and are deliberate:
`as_markdown()` for links, anchors and Mermaid, `as_structured()` for the
structured-only renderer. Spec §7.3 prefers the enum; §29 is why the interface
is capability-shaped rather than "every document is a tree".

### D2 — `[` / `]` visit containers only (spec §33.1)

Structured navigation stops on **mappings and sequences**, never scalars. This
is the exact analogue of Markdown headings: the foldable structure. A scalar
stop would make `]` indistinguishable from `j` on a large array. Symmetric,
stable under folding, and implemented as `DocumentModel::next_structural` /
`previous_structural`. `[` from a body row lands on the enclosing container
(`enclosing_structural`), mirroring Markdown's "back goes to the heading you
are under".

### D3 — `{` / `}` are sibling navigation, climbing out of a finished branch

`next_sibling_or_uncle` / `previous_sibling_or_parent`. Matches Markdown's
"same or higher level" reading.

### D4 — parent/first-child actions bind to `H` / `L`

Required to exist by spec §12.4; the default keys are free in diple's keyspace
(`h`/`l` are horizontal scrolling, `H`/`L` are unbound) and avoid `Alt-`, which
`Esc`-prefixed terminals report unreliably. **Not yet added to `Action`** — see
§3.

### D5 — a document root is not a foldable unit

`structured/build.rs`. A structured root always wraps everything, so making it
a fold target would turn `zM` from "show me the shape of this file" into "show
me one brace". Markdown's analogue — the document as a whole — is likewise not
a section, and `za` before the first heading already reports "no section here".
Consequence: `za` on a *top-level* entry with a scalar value reports that there
is nothing to fold, which is honest.

### D6 — no closing-delimiter rows

`layout/structured.rs`. JSON renders `key: {` and indents; there is no `}` row.
A closing brace is a cursor stop that stands for nothing, which spec §7.5.1
explicitly warns against, and it would double the row count of deeply nested
data. Spec §5.1 calls glyphs a layout decision; the collapsed forms
(`{4 members}`, `[3 items]`) and the explicit `{}` / `[]` for empty containers
are implemented as specified (§11.3, §15.7).

### D7 — one node produces one *contiguous* run of rows

`RenderTree::line_index_for` walks forward from a node's first line while the
node id matches, so a split run would break the viewport anchor. D6 is what
keeps runs contiguous. A block scalar and a comment block are several rows of
*one* node, which is fine.

### D8 — YAML is rendered YAML-shaped, JSON JSON-shaped

JSON: braces, `[0]: value`, every string quoted. YAML: indentation, `[0]` (or
`-` when `structured.show_indices = false`), quotes only where the source had
them. A single-document YAML stream shows no root row and its entries start at
the left margin (spec §5.2); JSON always shows its root brace (spec §5.1);
a multi-document YAML stream gets a `--- Document N` row per root.

### D9 — diple parses JSON itself

`document/json.rs`, ~350 lines, no new dependency. `serde_json` cannot give
source order *and* duplicate keys *and* lexical number forms *and* spans, all
of which a reader needs (spec §9.2–§9.4, §22.1). Duplicate keys are kept
(spec §9.3 preferred behaviour — **no documented limitation needed**).

### D10 — YAML uses `granit-parser` 1.3 (spec §22.2)

MIT OR Apache-2.0, pure Rust, `unsafe` forbidden, yaml-test-suite tested, two
small transitive crates (`arraydeque`, `smallvec`), actively maintained.
Two passes over the source are needed: the event stream identifies an anchor by
a numeric id, and a reader has to see `&defaults`, so a token pass collects
anchor names (ids are assigned in source order) plus `%TAG`/`%YAML`, which are
only visible at the token level.

### D11 — content detection is conservative (spec §6.5, AC-06)

A YAML stream is claimed only when **every** root is a non-empty collection
*and* at least one structural signal is present (several documents, a
directive, an anchor/alias/tag, a nested container, or a top-level mapping with
≥2 entries). The "every root is a collection" clause is what keeps Markdown
with YAML front matter as Markdown — its body parses as a bare scalar.
JSON is claimed only when the first token is `{` or `[` *and* strict parsing
succeeds, so a bare `42` on stdin stays Markdown.

### D12 — a *stated* format that fails to parse is an error; a *guess* falls back

`document/load.rs`. `--format` and a recognised suffix are statements
(spec §17.1); anonymous stdin is a guess (§17.2).

### D13 — search fields, not just nodes

`Match` gained a `MatchField` (`Body`, `Label`, `Value`, `Comment`, `Tag`,
`Anchor`) so a hit in a key and a hit in its value can both live on one row and
highlight the right run (spec §14.2). Markdown uses `Body` throughout.
A node's comments are **one** field joined by newlines, so a per-row offset
stays meaningful; the layout engine tracks the base offset per rendered row.

### D14 — escape sanitisation at one choke point

`util/text.rs` + `StyledSpan::new`. Every piece of document text reaches the
terminal through that constructor. C0/C1/DEL become U+FFFD, `\t`/`\n`/`\r`
become a space (line structure is the layout engine's decision), and the
bidirectional overrides/isolates/marks become U+FFFD. Ordinary Unicode is
untouched and clean text is not copied. Satisfies spec §9.5, §20.4, AC-18.

### D15 — the outline is complete, and derived on demand

Every node gets an entry, with a trimmed scalar preview (`outline::preview`,
32 columns). A filtered "interesting nodes only" outline would be a rule the
reader cannot predict. `DocumentModel::outline()` computes it from the parsed
model — never by reparsing (spec §13.4) — so `TocState` should build it lazily
on first open (see §3).

### D16 — the theme's structured palette is *derived*

`StructuredStyles::derived(&Theme)` maps existing roles (heading hues, code,
quote, link, …) onto the sixteen structured roles, and each built-in theme ends
its constructor with `.with_derived_structured()`. A theme added later gets a
coherent palette for free, and no existing config file needs changing
(spec §15.3). `Theme::styles_mut` destructures `StructuredStyles` exhaustively,
so a new role is a compile error until it is added to the downgrade chain.

### D17 — `SourceDocument` is `Rc<str>`-backed

Detection has to attempt a parse before it knows which backend keeps the
source. Reference counting makes a rejected attempt free and the accepted one a
pointer copy, instead of copying a document that may be tens of megabytes.
Nothing here crosses a thread.

---

## 3. What remains, in order

### Step A — port the application layer (this is the blocker)

Compile errors are currently confined to `src/app/`. The inventory:

| File | What to change |
| --- | --- |
| `app/state/mod.rs` | `doc: Document` → `doc: DocumentModel`; `FoldState::new(&doc)` → `doc.fold_state()`; `SearchIndex::build(&doc)` → `doc.search_index().clone()` (or borrow); `current_section()` → `current_fold() -> Option<FoldId>` |
| `app/state/navigate.rs` | `fold_current` on `FoldId`; `jump_heading` → `jump_structural` driven by `model.next_structural`/`previous_structural` + `tree.first_line_of`; `jump_heading_same_level` → `model.next_sibling`/`previous_sibling`; `reveal_node` → `model.reveal`; link handling behind `as_markdown()` |
| `app/state/layout_cache.rs` | `layout.layout(&self.doc, …)`; `splice_section` → a fold-shaped splice that dispatches to `Layout::relayout_nodes` (Markdown) or `structured::relayout_subtree`; `visible_ancestor_line` via `model.parent` |
| `app/state/sidebars.rs` | `toc_jump` via `OutlineEntry.node`; `nearest_diagram` behind `as_markdown()`; `toggle_toc` via `model.has_outline()` |
| `app/state/input.rs` | new actions; mouse fold-toggle via `model.fold_at(node)` instead of matching `LineKind` |
| `app/toc.rs` | `TocState` over `OutlineEntry`, built **lazily** on first open (a 1M-node JSON must not pay for an outline nobody opened) |
| `app/hints.rs` | `HintContext` gains the document's `DocumentCapabilities`; groups drop what the format cannot do, and "Headings" becomes format-dependent wording |
| `app/events.rs` | status line shows format label + `DocumentPath` breadcrumb (`breadcrumb_within`, left-truncating); cursor-row highlight for structured documents only, painted at draw time like search marks — **never** a layout input |
| `app/workspace.rs` | `load()` via `document::load` with the session's `FormatRequest`; surface `DocumentError` as a status message |
| `app/mod.rs` | `diagram_provider(&DocumentModel, …)` — no diagrams for structured documents |

Two helpers still need writing:

* `RenderTree::splice_lines(start, old_len, lines) -> bool` — the line-range
  splice `layout/structured.rs::relayout_subtree` calls. Model it on
  `splice_nodes`: drop the removed `first_line` entries, splice, recompute
  `node_offset` over the new range, shift `first_line`/`headings`/`spans`/
  `pending`/`tail` by the delta, re-scan the new range for structural rows,
  recompute `max_width`.
* `LayoutOptions` needs `structured_indent: usize` and `show_indices: bool`
  (used by `layout/structured.rs`), **and both must be added to
  `LayoutFingerprint`** or the layout cache silently goes stale.
* `Layout::layout` / `realize` / a new `relayout_fold` take `&DocumentModel`
  and dispatch; `realize` is a no-op for structured documents.

### Step B — CLI and configuration

* `--format <auto|markdown|json|yaml>` on `CliArgs`, threaded into
  `document::load` and into `Workspace` so `:open` detects identically (§16.4).
* `--structured-indent <N>`, `--path <auto|always|never>`.
* `[structured]` config section: `indent = 2`, `path = "auto"`,
  `show_indices = true`, `collapsed_summary = true`, plus a top-level
  `format = "auto"`. Add each to `config/settings.rs::ALL` (name, kind,
  default, help) *and* to `read`/`write` — the table is checked against
  `Config::default()` by an existing test.
* `main.rs`: `read_input` → `SourceDocument`; `document::load`; a
  `DocumentError` goes to stderr with a non-zero exit (§17.3); `print_plain`
  takes the model.

### Step C — actions and keys

Add to `config/actions.rs` (and `ALL`, `name`, `description`) and to
`DEFAULT_BINDINGS`: `parent_node` (`H`), `first_child` (`L`). Consider renaming
the four heading actions' *descriptions* to be format-neutral while keeping
their names for `[keys]` compatibility — §26.3 forbids removing or repurposing
a key, and none of this does.

### Step D — tests

Fixtures to add under `tests/fixtures/`: nested JSON, a JSON array of objects,
a Kubernetes-like YAML, multi-document YAML, YAML with comments/anchors/tags/
block scalars, invalid JSON, invalid YAML, empty and minimal files, and a large
generated document for the benches. `tests/common/mod.rs::fixture_names()`
currently filters `*.md` — it needs a format-aware sibling.

Snapshot coverage the spec asks for (§23.2): JSON expanded / partially
collapsed / narrow / no-Unicode / no-colour, YAML expanded / comments /
anchors / block scalar, multi-document YAML, outline open, key hints open,
search match hidden then revealed, split Markdown+YAML, split JSON+JSON.

CLI tests (§23.3): `diple file.json`, `--format json -`, `cat … | diple`,
`diple file.json | head`, exit codes, stderr on parse errors, no escape leakage.

Fuzz targets (§23.4): format detection, the JSON parser, the YAML adapter,
structured layout, path formatting, fold reveal.

Benches (§19.2): small/medium/large JSON and YAML alongside the Markdown ones.

### Step E — documentation and packaging

README (new positioning, `--format`, supported formats), `docs/keybindings.md`
(the Markdown/JSON-YAML table from §25.2, plus `H`/`L`), `docs/configuration.md`
(`[structured]`), a troubleshooting section (§25.4), `CHANGELOG.md` 2.0.0
headline (§35), Cargo `description`/`keywords`, the Debian/RPM
`extended-description`/`summary`, and `cli/man_sections.rs`.

### Step F — compliance audit

Re-read the spec end to end and check every normative requirement and every
AC-01…AC-23 against implementation, tests and docs.

---

## 4. Things that will bite

* **`LayoutFingerprint`** must list every `LayoutOptions` field. The type's own
  doc comment says so; the layout cache breaks silently otherwise.
* **`Theme::styles_mut`** destructures exhaustively on purpose — a new style
  field is a compile error until it is downgraded too.
* **`RenderTree` invariants:** `realize` must not change line count or width;
  `mark_search`/`clear_search` must not change text, width or line count;
  `splice_nodes` assumes `node_spans()` tiles `0..tail_start()` contiguously.
  There is an `assert_index_is_consistent` in `render/primitives.rs` tests.
* **`granit_parser::Event` and `Placement` are `#[non_exhaustive]`** — the
  adapter has catch-all arms, deliberately.
* **Anchor ids** are correlated to names positionally (the *n*-th `Anchor`
  token is anchor id *n*). If granit ever changes that, the YAML anchor tests
  fail loudly, which is the point.
* `tests/snapshots/` will need re-accepting only if Markdown rendering actually
  changed. It should not have — the sanitisation change turns a literal tab in
  prose into a space, which is the one place to check.
