# diple 2.0 — implementation status

**Resumed from:** commit `b4cf2c4`
**State described:** everything up to and including `f7f7eef` — this file is
the commit after it, on `main`, 2026-09-18
**Suite:** 747 tests, all passing (as of `df27dc4`, 2026-09-24)

---

## Resuming: what is left

**This section is the only part of this file that is about the future. Delete
it when the item below is done.**

### Two independent audits ran; fix, then audit a third time

**First audit** (2026-09-24, at `128fc52`): AC-09, AC-10, AC-17 and AC-18
failed, plus smaller defects. Fixed in `fb90aba`..`34b6fa7`.

**Second audit** (2026-09-24, at `34b6fa7`): AC-09, AC-10 and AC-18 now
VERIFIED. **AC-06** and **AC-01** failed on stdin — Markdown lists with a colon
in an item, and an introduction over a list, were detected as YAML (D11) —
and **AC-17** was only partly fixed: a YAML stream cut mid-key, a JSON keyword
cut short, broken JSON after a member name and over-deep JSON still fell back
to Markdown. Smaller findings: a cancelled search moved the cursor, a
Latin-1 character in JSON errors, file names and OSC 8 targets not fully
sanitised, keys with quotes, braces or zero-width characters, quoted YAML
keys that pass for other scalars in the path, the outline dropping instead of
replacing controls, decoded tag escapes, two doc contradictions, and a stdin
slowdown from the speculative YAML parse. All fixed in `9730cee`..`d779346`,
each with a test.

**Run the audit a third time**, with the same brief — a fresh agent, §7 as a
claim — pressing on AC-06, AC-17 and AC-01 on stdin: try hard for prose the
detection now claims, and for truncations it still lets through. The 1.2.0
binary is built from `git worktree add <dir> 8147fdc`.

Any mandatory criterion that is not VERIFIED means the release is not
complete.

### What not to redo

The previous handover cost a session's worth of work by describing states that
had already been superseded. To avoid repeating that: §3 lists what the old
handover got wrong, §5 lists every defect execution found and how it was
resolved, and §8 lists what was left undone **on purpose** with the reasoning.
Read those three before changing anything, and check any statement here
against the code before acting on it — including this one.

**Normative source of truth:** `docs/diple-v2-structured-documents-spec.md`.
This file records *decisions, state and evidence*, never requirements. Where
the two disagree, the spec wins.

---

## Status vocabulary

Used consistently below. A requirement counts as complete only when it is
**Verified**.

| State | Meaning |
| --- | --- |
| Not started | No meaningful implementation exists. |
| Implemented | Code exists that appears to implement it. Says nothing about whether it compiles or works. |
| Compiling | It compiles in its intended build path. Says nothing about behaviour. |
| Verified | The observable behaviour has been exercised successfully by automated and/or end-to-end tests. |

---

## 1. Where the pieces are

```
src/document/
  mod.rs            format-neutral vocabulary + module map
  source.rs         SourceDocument (Rc<str>-backed), SourceSpan, line_col
  format.rs         DocumentKind, FormatRequest, extension table, JSON sniff
  capabilities.rs   DocumentCapabilities (+ MARKDOWN / STRUCTURED presets)
  folds.rs          FoldId, FoldState
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
src/util/text.rs           terminal-escape sanitisation
src/layout/structured.rs   semantic rows for JSON/YAML
```

Start at `src/document/model.rs` to understand how anything above the boundary
works; `structured/ast.rs` explains the node model and why it is flat;
`layout/structured.rs` explains why rows come from nodes; `yaml.rs` explains
what a YAML *reader* has to keep.

---

## 2. Architectural decisions

These resolve the choices spec §33 delegates to implementation, plus a few the
spec leaves open. They are wired through the model, the layout engine and the
tests; do not revisit one silently.

### D1 — `DocumentModel` is an enum, not a trait

`document/model.rs`. Markdown keeps its own AST; JSON and YAML share
`StructuredDocument`. The application above the boundary only ever asks
capability-shaped questions (`fold_at`, `next_structural`, `outline`, `path`,
`is_hidden`, `reveal`, …). Two escape hatches are deliberate: `as_markdown()`
for links, anchors and Mermaid, `as_structured()` for the structured-only
renderer. Spec §7.3 prefers the enum; §29 is why the interface is
capability-shaped rather than "every document is a tree".

### D2 — `[` / `]` visit containers only (spec §33.1)

Structured navigation stops on **mappings and sequences**, never scalars — the
exact analogue of Markdown headings, i.e. the foldable structure. A scalar stop
would make `]` indistinguishable from `j` on a large array. `[` from a body row
lands on the enclosing container, mirroring Markdown's "back goes to the
heading you are under".

### D3 — `{` / `}` are sibling navigation, climbing out of a finished branch

`next_sibling_or_uncle` / `previous_sibling_or_parent`. Matches Markdown's
"same or higher level" reading.

### D4 — parent/first-child actions bind to `H` / `L`

Required to exist by spec §12.4. `h`/`l` are horizontal scrolling and `H`/`L`
were unbound, so nothing was removed or repurposed (§26.3). `Alt-` was avoided
because `Esc`-prefixed terminals report it unreliably.

### D5 — a document root is not a foldable unit

`structured/build.rs`. A structured root wraps everything, so making it a fold
target would turn `zM` from "show me the shape of this file" into "show me one
brace". Spec §11.2 lists a document root as *optional*. Consequence: `za` on a
top-level entry with a scalar value reports that there is nothing to fold,
which is honest.

### D6 — no closing-delimiter rows

`layout/structured.rs`. JSON renders `key: {` and indents; there is no `}` row.
A closing brace is a cursor stop that stands for nothing, which spec §7.5.1
warns against, and it would double the row count of deeply nested data. The
collapsed forms (`{4 members}`, `[3 items]`) and the explicit `{}` / `[]` for
empty containers are implemented as §11.3 and §15.7 specify.

### D7 — one node produces one *contiguous* run of rows

`RenderTree::line_index_for` walks forward from a node's first line while the
node id matches, so a split run would break the viewport anchor. D6 is what
keeps runs contiguous. A block scalar and a comment block are several rows of
*one* node, which is fine.

### D8 — YAML is rendered YAML-shaped, JSON JSON-shaped

JSON: braces, `[0]: value`, every string quoted. YAML: indentation, `[0]` (or
`-` when `structured.show_indices = false`), quotes only where the source had
them. A single-document YAML stream shows no root row and its entries start at
the left margin (spec §5.2); JSON always shows its root brace (spec §5.1); a
multi-document YAML stream gets a `--- Document N` row per root.
`StructuredDocument::shows_root_row()` / `has_row()` own this rule so that the
layout engine and the navigation guards cannot disagree about it.

### D9 — diple parses JSON itself

`document/json.rs`, no new dependency. `serde_json` cannot give source order
*and* duplicate keys *and* lexical number forms *and* spans, all of which a
reader needs (spec §9.2–§9.4, §22.1). Duplicate keys are kept — spec §9.3's
preferred behaviour, so **no documented limitation is needed**.

### D10 — YAML uses `granit-parser` (spec §22.2)

MIT OR Apache-2.0, pure Rust, `unsafe` forbidden, yaml-test-suite tested, two
small transitive crates. Two passes over the source are needed: the event
stream identifies an anchor by a numeric id and a reader has to see
`&defaults`, so a token pass collects anchor names (ids are assigned in source
order) plus `%TAG`/`%YAML`, which are only visible at the token level. Both
passes run with `parser_options()`, which raises granit's nesting limits to
diple's `MAX_DEPTH` so that the two backends refuse deep input at the same
depth and with the same message.

### D11 — content detection is conservative (spec §6.5, AC-06)

A YAML stream is claimed only when **every** root is a non-empty collection
*and* at least one structural signal is present (several documents, a
directive, an anchor/alias/tag, a nested container, or a top-level mapping with
≥2 entries). The "every root is a collection" clause is what keeps Markdown
with YAML front matter as Markdown — its body parses as a bare scalar. JSON is
claimed only when the first token is `{` or `[` *and* strict parsing succeeds,
so a bare `42` on stdin stays Markdown.

Narrowed after the second audit found Markdown lists claimed as YAML: "a
container inside a container" was a signal, and `- Fast: written in Rust` is
one. The signals are now a mapping under a mapping key, a mapping of two or
more entries at depth > 0, multiple documents, directives, anchors, aliases
and tags, or a flat top-level mapping of two or more entries whose keys have
no spaces and whose values are not all phrases.

### D12 — a *stated* format that fails to parse is an error; a *guess* falls back

`document/load.rs`. `--format` and a recognised suffix are statements
(spec §17.1); anonymous stdin is a guess (§17.2).

One exception, added after the independent audit found AC-17 failing on it:
a guess that parses cleanly until the input runs out *inside a value* — an
unclosed string, bracket or quote, flagged as `DocumentError::incomplete` by
both parsers — is an error, because §6.6 names exactly that case. For YAML the
lines before the open construct must also be a stream detection would have
claimed, since prose opens quotes it never closes (`'Tis the season`).

Widened after the second audit: a YAML break on the last line (a key cut in
half) counts as running out; an unclosed flow collection needs only a
collection before it; and JSON that fails after a member name, a comma, or
at the depth limit is `DocumentError::committed` and reported as broken
JSON. A large input is probed on its first 64 KiB before a full YAML parse.

### D13 — search fields, not just nodes

`Match` carries a `MatchField` (`Body`, `Label`, `Value`, `Comment`, `Tag`,
`Anchor`) so a hit in a key and a hit in its value can both live on one row and
highlight the right run (spec §14.2). Markdown uses `Body` throughout. A node's
comments are **one** field joined by newlines, so a per-row offset stays
meaningful; the layout engine tracks the base offset per rendered row.

### D14 — escape sanitisation at one choke point

`util/text.rs`, applied by `StyledSpan::new` for structured text and by
`layout::inline::push_span` for Markdown, which assembles its spans without
the constructor; the OSC 8 writer in `app/events.rs` applies it again because
it prints past the backend. (Until `fb90aba` only the first of the three
existed, which is how Markdown text escaped it — the audit's AC-18 finding.)
A replaced character is measured as the one cell its replacement takes. C0/C1/DEL become U+FFFD, `\t`/`\n`/`\r`
become a space, and the bidirectional overrides/isolates/marks become U+FFFD.
Ordinary Unicode is untouched and clean text is not copied. Satisfies spec
§9.5, §20.4, AC-18.

### D15 — the outline is complete, and derived on demand

Every node gets an entry, with a trimmed scalar preview (32 columns). A
filtered "interesting nodes only" outline would be a rule the reader cannot
predict. `DocumentModel::outline()` computes it from the parsed model — never
by reparsing (spec §13.4) — and `TocState` builds it lazily on first open, so a
1M-node JSON does not pay for an outline nobody opened.

### D16 — the theme's structured palette is *derived*

`StructuredStyles::derived(&Theme)` maps existing roles onto the sixteen
structured roles, and each built-in theme ends its constructor with
`.with_derived_structured()`. A theme added later gets a coherent palette for
free, and no existing config file needs changing (spec §15.3).
`Theme::styles_mut` destructures `StructuredStyles` exhaustively, so a new role
is a compile error until it is added to the downgrade chain.

### D17 — `SourceDocument` is `Rc<str>`-backed

Detection has to attempt a parse before it knows which backend keeps the
source. Reference counting makes a rejected attempt free and the accepted one a
pointer copy, instead of copying a document that may be tens of megabytes.
Nothing here crosses a thread.

### D18 — a jump targets a node's *landmark* row, not its first row

`RenderTree::landmark_line_of`. A node owns the blank spacer line in front of
it, so scrolling to `first_line_of` put every heading jump one row lower on
screen than 1.x did. The structural index is in line order with ascending node
ids, so the lookup is a binary search rather than a scan — which matters for a
document whose every container is a structural row. See §5 for the regression
this fixed.

### D19 — comments after the last node are drawn, not only indexed

`layout/structured.rs::trailing_comments`. A YAML comment following the final
entry has no node to hang from. It is attributed to the last root for the
viewport anchor and the search highlight, and rendered at the left margin,
because spec §10.3 says comments stay visible and AC-12 is checked by a reader,
not by a search index.

### D20 — `structured.path = "always"` outranks the progress counters

`render/terminal.rs::StatusBar::text`. Under `auto` the counters win a narrow
line and the breadcrumb is dropped (spec §24.3 ranks filename, format and
progress first). `always` is the reader saying orientation matters more, so
there the counters go instead. `never` suppresses the breadcrumb outright.

### D21 — a quoted scalar is escaped the way its dialect escapes

`layout/structured.rs`. Adding quotes around decoded content produced
`"she said "hello" and left"`, which a reader cannot parse by eye, and a
newline flattened to a space was invisible. JSON and YAML's double-quoted
style re-escape backslashes, quotes and control characters; YAML's
single-quoted style doubles its own quote and leaves a backslash alone,
because that is all that style has. Escaping moves the bytes, so the search
offsets are moved with them. `DocumentPath::canonical` is the exception and
stays unescaped: it is the internal copyable form (§5.3.1) and never reaches
the terminal.

### D22 — structured rows wrap, unless the indentation has eaten the terminal

Spec §15.4 and §15.5 make wrapping the reader's setting, not the format's.
A row wraps at the viewport width, breaking at a space where there is one,
with the continuation indented one level past the entry so a key and its
value still read as a pair. Continuation rows carry the same node id, so a
wrapped value stays one semantic unit for the cursor, the anchor and the
fold splice (D7). The exception is `MIN_WRAP_CONTENT`: when a node sits so
deep that fewer than sixteen columns are left, the row is emitted long and
scrolls horizontally, because wrapping there would turn a deeply nested
document into pages of blank left margin. No continuation *marker* is added
— §15.2 says indentation alone is acceptable, and every real row carries a
`key:`, an index or a `#` that a continuation does not, so the two are
already distinguishable without spending a column or clashing with the fold
glyphs.

### D23 — a comment belongs to the row it stands above

`yaml.rs`. A block collection's start event has a zero-width span at its
*first entry*, so a container and its first child appear to begin together
and something has to arbitrate. A keyed container claims exactly the run of
comments written before its own key; the rest wait for the entry they stand
above. A leaf claims what is above it, and a document root claims nothing,
because a root is the document rather than a row. This is what makes a
comment collapse with the subtree it belongs to (§10.3) rather than with its
parent. Spec §33.4 delegates the exact rule; this is it.

---

## 3. Corrections to the previous handover

The handover this run inherited mixed observations from several stages of the
earlier session. These statements were **false at `b4cf2c4`** and have been
corrected here rather than carried forward:

| Stale claim | Reality at `b4cf2c4` |
| --- | --- |
| "`Action::ParentNode`/`FirstChild` not yet added" (D4) | They existed, were in the `ALL` table, bound to `H`/`L`, dispatched and described. |
| "`structured_indent`/`show_indices` **must be added** to `LayoutFingerprint`" | Both were already in the fingerprint. The only `LayoutOptions` field absent is `diagrams`, deliberately and documented. |
| "`RenderTree::splice_lines` still needs writing" | Already implemented, and already called by `structured::relayout_subtree`. |
| "`Layout::relayout_fold` needs writing" | Already implemented and dispatching Markdown → `relayout_nodes`, structured → `relayout_subtree`. |
| "the installed stable toolchain is a stale 1.71.1, use `cargo +1.90.0`" | The machine's stable is 1.94.0. Plain `cargo` is correct; no explicit toolchain is needed. |
| "roughly a hundred errors remain" | 73 (`70` lib test + `3` bin test) plus the bench targets. |
| "Step A′ … almost all in `#[cfg(test)]` modules; nothing needs a decision" | Broadly right, but `src/main.rs` also did not compile, and repairing the suite surfaced three genuine behaviour bugs (§5). |

Statements that were **true** and remain so: the application layer was ported
(`App::doc` is a `DocumentModel`, `TocState` is format-neutral and lazy,
`HintContext` carries capabilities, `Workspace` carries a `FormatRequest`); the
status line showed neither format nor breadcrumb; the structured cursor row was
not highlighted; nothing set the `[structured]` configuration; and no CLI flag
or config key for any of it existed.

---

## 4. What this run did

1. **Restored the build.** Ported every `#[cfg(test)]` module, the integration
   tests, the benches and `src/main.rs` to the v2 API. No test was deleted,
   ignored or weakened.
2. **Ran the suite for the first time** — the first point at which any of the
   structured code could be *observed*. 597 passed, 12 failed.
3. **Diagnosed all twelve** against the spec rather than against whichever side
   was easier to change (§5).
4. **Finished the port's debts**: the status line's format label and path
   breadcrumb, the structured cursor-row highlight, and the configuration
   wiring for `structured_indent` / `show_indices`.
5. **Added the missing edges**: `--format`, `--structured-indent`, `--path`,
   the top-level `format` setting and the `[structured]` config section, and
   routed `main.rs` through `document::load`.
6. **Built the corpus and the coverage**: 18 JSON/YAML fixtures, a structured
   integration suite, six new fuzz targets, JSON/YAML benchmarks, structured
   snapshots.
7. **Rewrote the public documentation and packaging** for the 2.0 product.

---

## 5. What execution found

### The first run

All twelve first-run failures are recorded here because several of them explain
a design decision above.

| Failure | Verdict | Resolution |
| --- | --- | --- |
| `next_and_previous_heading_walk_the_document` | **Real Markdown regression.** Confirmed by running the test at `155632a`, before the refactor, where it passed. | Heading jumps landed one row early: the port replaced `heading_lines()` (the heading's own row) with `first_line_of` (the blank spacer the node owns). Fixed by D18. |
| `a_setting_takes_hold_where_it_is_used` | **Real bug from the lazy outline.** | `:set toc = true` asked `toc.is_empty()` before the entries were derived, so it always said yes and the sidebar never opened. `Effect::Sidebar` and `App::new` now derive first. |
| `comments_survive_attached_to_what_they_describe` | **Real YAML bug.** | A block collection's start event has a zero-width span at its *first entry*, so a comment above `replicas:` was consumed by the implicit root mapping. Attachment now asks which row the comment stands above. |
| `a_comment_with_nothing_after_it_is_still_kept` | **Real YAML bug.** | Free comments were stored but never indexed, violating §10.3 "searchable". Now indexed against the node they follow — and, separately, D19 made them visible. |
| `anchors_are_visible_and_aliases_are_references_not_copies` | **Real YAML bug.** | `take_relation()` ran before `take_meta()` and consumed the pending key, so `merge_key` was never set and `<<` was silently un-marked, contradicting §10.8. Ordering fixed at all four call sites. |
| `deep_nesting_is_an_error_rather_than_a_crash` | **Real YAML bug.** | granit's default nesting limit is 255 while JSON accepted to `MAX_DEPTH` = 1024, so the two backends disagreed. Both passes now run with diple's limit, and granit's recursion error is reworded into diple's. |
| `folds_cover_the_containers_and_nothing_else` | Wrong prediction. | The tests assumed a foldable root; D5 says otherwise, and §11.2 makes a root fold optional. Fold indices corrected, plus positive assertions that the ids are dense. |
| `a_collapsed_container_keeps_its_own_row_and_hides_the_rest` | Wrong prediction. | Same off-by-one fold index. |
| `the_outline_is_the_whole_tree_with_short_previews` | Wrong prediction. | `outline[0].fold` is `None` because the root is not foldable. |
| `every_row_carries_the_node_it_came_from` | Wrong prediction. | A single-document YAML root deliberately has no row (D8). The blanket assertion was replaced with the actual contract, `first_line_of(n).is_some() == doc.has_row(n)`, plus a D7 contiguity check. |
| `opposed_pairs_share_one_row` | Superseded by the spec. | `H`/`L` legitimately form a third opposed pair, which §12.4 asks the hints to offer. |
| `unicode_survives_everywhere` | Wrong prediction. | The test named node 4 in a 4-node document. Rewritten to reach the item through `first_child` rather than by a guessed id. |

Where a test's prediction was wrong it was corrected to the behaviour the spec
requires, never to whatever the implementation happened to do.

### What the suite could not see

A green suite is not the same as a correct one. Six further defects came out
of fuzzing, of reading generated snapshots, and of driving the binary through
a pseudo-terminal — none of which any passing test had been looking at.

| Defect | Found by | Resolution |
| --- | --- | --- |
| A key containing an ESC wrote to the terminal through the status line. Every other piece of document text is sanitised by `StyledSpan::new`, but the status bar renders the breadcrumb itself — so the one exit that bypassed the choke point was the one a document controls by naming a key. | `structured_layout` fuzz target | `PathSegment::label` sanitises (D21). **An AC-18 breach.** |
| YAML counts a lone carriage return as a line break and `SourceDocument` does not, so a parse error reported a line the excerpt could not show — and with no line to point at, the caret vanished. | `yaml_model` fuzz target | The adapter hands `DocumentError` a byte offset and lets it derive the position, so the number and the excerpt cannot disagree. AC-17. |
| A quoted scalar was ambiguous and its newlines invisible. | reading a snapshot | D21. |
| `|+`, `|-`, `>-` and `|2` all rendered as a bare `\|` or `>`, so "keep the trailing newlines" and "strip them" looked identical. | reading a snapshot | The scalar remembers the header it was written with; the adapter recovers it from the line that introduces the scalar. §10.10, P3. |
| A comment written inside a mapping above its first entry was hoisted out to the mapping, so it collapsed with the wrong subtree. | reading a snapshot | D23. |
| `structured.collapsed_summary` was in the schema, the settings registry, the man page and the configuration reference, and nothing read it. | driving the binary | Wired through `LayoutOptions`. A setting that settles nothing is worse than no setting. |
| Structured rows ignored the `wrap` setting entirely, so `wrap = true` and `--no-wrap` were indistinguishable for JSON and YAML. | driving the binary at 44 columns | D22. §15.4, §15.5. |
| Selecting an outline entry scrolled to the node's first line while `]` scrolls to its landmark row, so the two ways of reaching one heading disagreed; and selecting a rowless YAML root did nothing at all. | driving the binary | Both go through `goto_node`; a rowless root goes to the document's first entry. |

---

## 6. Validation

Every gate below was run from a clean tree at `f6e8ba7`.

| Gate | Result |
| --- | --- |
| `cargo fmt --check` | clean |
| `cargo check --all-targets --all-features` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean |
| `cargo test --all-targets --all-features` | **731 passed, 0 failed, 2 ignored** |
| `cargo doc --no-deps` | 0 warnings |
| `cargo bench --no-run` | builds |
| `cargo +nightly fuzz build` | all 12 targets build |
| `cargo check --release --target …` | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-musl` and `x86_64-unknown-linux-musl` (pure-Rust regex) all clean. The musl build *with* the oniguruma engine cannot be checked here — `onig_sys` needs a musl C toolchain this machine does not have — which is an environment limit, not a code one; CI installs it. |
| man page (`--generate-man`, `man --warnings`) | 0 warnings; CI's own option sweep passes |
| shell completions (bash/zsh/fish) | generated, `bash -n` clean, include the new flags |

Test counts by target: 653 unit, 38 CLI integration, 19 snapshot, 19
structured-corpus, 2 fixture smoke.

### Fuzzing

Six new targets, each run to `DONE` with no crash and no artefact:

| Target | Executions |
| --- | --- |
| `format_detect` | 17,059 + 7,035 |
| `json_model` | 114,149 + 10,628 |
| `yaml_model` | 18,399 + 7,636 |
| `structured_layout` | 118,445 + 10,767 |
| `structured_path` | 43,318 + 10,897 |
| `fold_reveal` | 54,893 + 36,052 |

`structured_layout` is what found the unsanitised breadcrumb (§5);
`yaml_model` found the carriage-return position defect.

### Performance

Measured on an idle machine. The question §19.4 asks is whether anything
is accidentally quadratic; throughput answers it directly.

| | 12 KiB | 1 MiB | 12 MiB |
| --- | --- | --- | --- |
| `json/layout@80` | 26.7 MiB/s | 23.3 MiB/s | 24.5 MiB/s |
| `yaml/layout@80` | 26.6 MiB/s | 25.7 MiB/s | 25.6 MiB/s |
| `json/load` | 266 µs | 24.7 ms | 477 ms |

Throughput is flat across three orders of magnitude: the scaling is
linear. Two shapes that could have been quadratic are not — 16 × 512 KiB
scalars index at the same rate per byte as ordinary documents (§20.3's
concern does not materialise), and a 128-deep document is flat in load,
layout, path and reveal, which is the iterative traversal §20.1 asks for
doing its job. Navigation and folding stay flat at every size.

Against the 1.2.0 binary, on the same machine, best of nine:

| Markdown document | 1.2.0 | 2.0.0 |
| --- | --- | --- |
| 1 KB README | 0.00 s | 0.00 s |
| 100 KB | 0.01 s | 0.01 s |
| 1 MB | 0.12 s | 0.13 s |
| 10 MB | 1.28 s | 1.49 s |

That table predates `2d87517`, and its explanation — sanitisation — was wrong:
the audit measured the slowdown with Markdown text not sanitised at all. The
cost was the Markdown search index, built eagerly at parse time (190 ms →
370 ms and about 100 MB more on 10 MB) although piped output never searches.
It is built on first use now. Re-measured with `hyperfine` on 2026-09-24,
after the AC-18 fix added Markdown sanitising: 10 MB 1.53 s ± 0.03 against
1.2.0's 1.59 s ± 0.29, 1 MB 158 ms ± 10 against 151 ms ± 2, same peak memory —
within noise, with the sanitising measured separately at about 2 %.

And the three formats are in the same class as each other on an 11 MB
document: Markdown 3.1 s / 490 MB, YAML 3.2 s / 722 MB, JSON 4.4 s /
599 MB. diple lays a document out in full before its first frame, which
is 1.x architecture; the structured formats did not make it worse.

### Manual behavioural verification

Driven through a real pseudo-terminal, not merely started:

* **Markdown** — `README.md`: scrolling, heading navigation, search,
  outline, links, splits.
* **JSON** — nested objects and arrays, empty `{}`/`[]`, `1e6` kept
  lexically, Unicode keys and values, duplicate keys in source order, a
  root scalar.
* **YAML** — a Kubernetes-like manifest: comments above, beside and after
  the last entry; `&defaults` and `*defaults`; `<<:` shown, not
  performed; `!!timestamp`; `|`, `|-`, `|+` and `>-` block scalars;
  `%YAML` and `%TAG` directives; multi-document streams.
* **Interaction** — `]`/`[`/`{`/`}`, `H`/`L`, `za`/`zM`/`zR`, `/` with
  reveal through collapsed ancestors, `t` outline with jump,
  `:open side-by-side` with a Markdown pane beside a YAML pane.
* **Status line** — format label and live breadcrumb, left-truncating on
  narrow terminals; `structured.path = never` suppresses it.
* **Runtime settings** — `:set structured.indent = 6`,
  `show_indices = false` and `collapsed_summary = false` each change the
  screen immediately.
* **Errors** — invalid JSON and YAML report file, line, column, an
  excerpt and a caret, and exit 1; invalid CLI values exit 2; a missing
  file exits 1; nothing panics and the terminal is never left in raw
  mode.
* **Degradation** — `NO_COLOR`, ASCII fallback, narrow widths,
  non-interactive pipes, a closed pipe.
* **Hostile input** — a 12-level YAML alias bomb renders in 0.01 s using
  4.6 MB (no exponential expansion); 5000-deep JSON is refused with a
  clean error rather than a stack overflow; a document containing ESC,
  OSC, BEL and CR emits none of them, in the document *or* the status
  line.

## 7. Spec 2.0 compliance matrix

The authoritative completion checklist. "Verified" means the observable
behaviour was exercised, not that the types exist.

| AC | Requirement | State | Evidence |
| --- | --- | --- | --- |
| AC-01 | Markdown remains first-class | **Fixed after 2nd audit, awaiting re-audit** | 2nd audit: 8400 file/stdin comparisons byte-identical to 1.2.0 except stdin Markdown lists detected as YAML (fixed with AC-06) and two documented sanitising differences. Before that: `app/state/characterization.rs` (15 cross-cutting tests pinned before the refactor, all green); `tests/render_snapshots.rs` with unchanged committed snapshots; the Markdown unit suite; manual paging/search/outline/links/splits. One real regression was found and fixed (D18, §5). |
| AC-02 | JSON opens natively | **Verified** | `document/json.rs`, `layout/structured.rs`, `tests/structured_fixtures.rs`; manual `diple api.json` with folding, navigation and outline. |
| AC-03 | JSON stdin auto-detects | **Verified** | `document/format.rs` + `load.rs` tests; `tests/cli_integration.rs`; manual `cat api.json \| diple`. |
| AC-04 | YAML opens natively | **Verified** | `document/yaml.rs` (30 tests), `layout/structured.rs`, `tests/structured_fixtures.rs`; manual `diple k8s.yaml`. |
| AC-05 | Common YAML pipelines work | **Verified** | D11 detection tests; manual `diple < detect.yaml` on `kubectl`-shaped nested mapping output, recognised without `--format`. |
| AC-06 | Ambiguous text stays Markdown | **Fixed after 2nd audit, awaiting re-audit** | `9730cee`: `prose_and_markdown_stay_markdown` covers the audit's lists and prose. Before that: Detection tests for prose, a Markdown list, `title: hello`, a bare `42` and two sentence-valued lines such as `Q: why?` / `A: because.` (D11, `df27dc4`); `ambiguous_stdin_stays_markdown` at the command line; manual `printf -- '- one\n- two\n' \| diple` renders bullets, not a sequence. |
| AC-07 | Explicit override works | **Verified** | `cli/args.rs` tests; `an_explicit_format_overrides_name_and_content`; manual `--format markdown config.yaml`, `--format yaml -`, `--format json file.data`. |
| AC-08 | Folding is semantic | **Verified** | `folding_never_depends_on_the_width`; `folds_cover_the_containers_and_nothing_else`; `structured_fixtures` collapse/reveal sweep; manual resize with folds held. |
| AC-09 | Search reveals hidden matches | **Verified (2nd audit)** | `88faa4c`: `a_search_keeps_only_the_folds_its_accepted_match_needs`, `cancelling_a_search_closes_what_its_preview_opened`, `a_match_on_a_container_key_opens_only_its_ancestors`. Before that: `search_reveals_a_match_hidden_inside_a_collapsed_section` (Markdown) and the structured counterpart leaving unrelated folds collapsed; manual `zM` then `/` opening exactly the ancestor path. |
| AC-10 | Path orientation works | **Verified (2nd audit)** | `5483a7a`: `the_breadcrumb_disambiguates_keys_that_read_as_something_else`, `keys_are_quoted_where_bare_they_would_mislead`. Before that: `document/path.rs` tests (keys with separators, indices, duplicates, multi-document roots); `render/terminal.rs` breadcrumb tests; manual live breadcrumb `spec › template › spec › containers › [0] › image`. |
| AC-11 | Outline works | **Verified** | `app/toc.rs` tests, `document/outline.rs`, the `structured_fixtures` outline sweep; manual `t` on YAML showing the complete tree with previews and jumping. |
| AC-12 | YAML comments survive | **Verified** | Attachment tests above/beside/trailing/first-inside-a-mapping (D23), `MatchField::Comment` search, `a_comment_collapses_with_the_subtree_it_belongs_to`, `a_comment_after_the_last_entry_is_still_drawn` (D19); `comments-yaml` snapshots at two widths; manual rendering. |
| AC-13 | YAML aliases are safe | **Verified** | `an_alias_to_a_container_stays_a_single_node`; `tests/fixtures/anchors.yaml`; manual 12-level alias bomb: 0.01 s, 4.6 MB. |
| AC-14 | YAML metadata survives | **Verified** | Anchor, alias, merge-key, tag, directive and multi-document tests; `anchors_aliases_tags_and_merge_keys_are_all_visible`; block headers kept as written, including chomping (§5); `anchors-yaml`, `block-scalars-yaml` and `multi-document-yaml` snapshots; manual rendering of all six. |
| AC-15 | Source order survives | **Verified** | `duplicate-keys.json` kept in source order (D9); ordering assertions in the JSON and YAML suites; manual `{"x":1,"a":2,"x":3}`. |
| AC-16 | Non-interactive mode is useful | **Verified** | `tests/cli_integration.rs` piping tests, including a structured document surviving a closed pipe; `plain_output_matches_the_render_snapshots_byte_for_byte`; manual `diple api.json \| head`, deterministic, fully expanded, no ANSI. |
| AC-17 | Parse errors are useful | **Fixed after 2nd audit, awaiting re-audit** | `9730cee`: the audit's truncation sweep now falls back only on cuts in the first line; `broken_json_that_committed_to_being_json_is_an_error`. Before that: `71d1c84`: truncated anonymous input is an error (D12), `truncated_structured_input_is_an_error`, `structured_input_cut_short_is_an_error`; `0af2963`: `:open` reports on one line. Before that: `document/error.rs` tests; invalid fixtures asserting an in-range 1-based line/column and a caret; `an_invalid_structured_document_reports_and_exits_non_zero` and `a_guess_that_does_not_parse_falls_back_to_markdown` at the command line; a regression test for the carriage-return position defect fuzzing found (§5); manual invalid JSON and YAML with exit 1 and no panic. |
| AC-18 | No terminal injection | **Verified (2nd audit)** | `fb90aba`: `markdown_text_cannot_carry_terminal_controls`, the Markdown `layout` fuzz target now asserts §23.4 with a seed carrying escapes in every construct, and the OSC 8 injection replayed in a pseudo-terminal is clean. Before that: `util/text.rs` tests (D14); `path.rs`'s hostile-key test (D21); the `structured_layout` fuzz target asserts no rendered span contains ESC, a C0 control or a bidi override, across 129k executions; `no_structured_fixture_leaks_an_escape` at the command line; manual ESC/OSC/BEL/CR document emits none, in the document or the status line. Fuzzing found and closed one breach here — see §5. |
| AC-19 | Tabs and splits are format-independent | **Verified** | `app/workspace.rs` tests; manual `:open side-by-side` with Markdown left and YAML right, `Ctrl-W` moving focus. |
| AC-20 | Help is context-aware | **Verified** | `e4dd82e`: `?` is filtered by capabilities (`help_offers_only_what_the_document_can_do`) — until then only the key hints were; `app/hints.rs` capability-driven group tests; `:help` settings registry test; `--help` and the man page list the new flags and actions. |
| AC-21 | Packaging is complete | **Verified** | `Cargo.toml` 2.0.0 with the new description and five keywords; Debian/RPM/Arch metadata; README, `docs/configuration.md`, `docs/keybindings.md`, troubleshooting, `CHANGELOG.md` 2.0.0; man page lints with 0 warnings and documents `H`/`L`; completions include the new flags. |
| AC-22 | Tests and fuzzing cover the new parsers | **Verified** | Unit suites for JSON, YAML, the structured model, paths, folds, the outline and detection; the structured integration corpus; six new fuzz targets (`format_detect`, `json_model`, `yaml_model`, `structured_layout`, `structured_path`, `fold_reveal`), built, seeded and smoke-run. |
| AC-23 | Future PDF is not architecturally blocked | **Verified (by design review)** | The application layer asks `DocumentCapabilities`, never "give me the tree": `DocumentModel` exposes `capabilities()`, `outline()`, `path()`, `fold_at()`, `next_structural()` and friends, and the key hints and help omit what a format cannot do. `capabilities.rs` already carries `pages` and `source_view`, unused by all three current backends. Adding a paged, partially hierarchical backend is one new enum variant plus its capability set; no app, tab, split, search, outline or terminal ownership changes. This is a structural criterion, so the evidence is the interface and its capability tests, not a PDF backend. |

Three mandatory criteria wait for a third audit (see "Resuming" above);
every other criterion was independently VERIFIED on 2026-09-24.

Three criteria needed substantial remediation rather than merely confirming:
**AC-18**, where fuzzing found a real escape-injection path through the status
line; **AC-17**, where a lone carriage return made an error point at a line
that could not be shown; and **AC-01**, where the port had introduced a
Markdown heading-jump regression that the characterization tests caught on
their first execution.

---

## 8. Deliberately not done

Out of scope for 2.0, recorded so nobody mistakes them for oversights.

* **`:format` as a runtime reformat.** Spec §16.3 makes it optional. A document
  is parsed once; `:open` reuses the session's `FormatRequest`. The setting is
  in the registry and its help says so.
* **Alias → anchor jumping** (§10.7). Explicitly "desirable but not mandatory
  for 2.0 if it would destabilise the interaction model"; `Enter` is already
  the fold toggle.
* **JSONC.** Spec §6.2 makes it optional and asks that `.jsonc` not be silently
  treated as strict JSON. It is not recognised, and the man page says so.
* **A raw/source view for JSON and YAML** (§15.8). Not required; the source is
  retained so it can be added later.
* **A PDF backend** (§29.9). Explicitly forbidden in 2.0.
* **Lazy search indexing** (§19.4). Only warranted if indexing dominates
  large-file startup; the benchmarks do not show that.
* **Showing fold state in the outline** (§12.5). The mandatory half of that
  sentence — folding never removes an outline entry — is implemented and
  tested. The other half is a *should*, and every marker considered either
  collided with a glyph the sidebar already uses (`▸` is the current entry)
  or cost a column on every row. Left undone rather than done badly; the
  outline's own entries already say what the structure is.

---

## 9. Known limitations

* **`md` and `yml` as `--format` values.** `FormatRequest::parse` accepts them,
  but the CLI enum and the config enum do not, so they are undocumented. The
  canonical spellings are `markdown` and `yaml`.
* **`structured.indent = 0`** is clamped to 1 rather than rejected at load
  time. A range error in `config/loader.rs::validate` would read better.
* **A comment in a YAML document with no nodes at all** has nothing to index
  against and is kept as text only.
* **An explicit YAML indent indicator (`|2`)** keeps its header, but the
  content lines render one column further right than the source, because the
  parser hands back the extra leading spaces as content and the layout adds
  its own indent on top.
* **Two more wall-clock assertions** remain from 1.x, with generous budgets:
  `app/state/layout_cache.rs` (50 ms per relayout) and
  `document/markdown/anchors.rs` (500 ms). Neither has been seen to flake;
  both are candidates for the same treatment as `layout::tests::deterministic`.
* **What a PDF backend would still have to change** (the audit's AC-23
  caveats, none a blocker): `SourceDocument` is UTF-8 text and `main.rs`
  decodes lossily, so a binary format needs a byte-level source; link
  following goes through `as_markdown()`; layout is dispatched per
  `DocumentModel` variant. All three are loader- or layout-level, not changes
  to the app, tab, split or search code.

---

## 10. Execution report for the resumed run

Recorded so a later run can be compared against this one. Nothing here
influenced an implementation decision.

| | |
| --- | --- |
| Start | 2026-09-18 15:50 CEST |
| End | 2026-09-18 17:45 CEST |
| Active runtime | ≈1 h 55 min, continuous |
| Human interventions | two, both at the very start: switching the model to Opus 5, and `git pull` — the resumed work was not yet in the local clone |
| Sub-agents | 9 |
| Peak parallel sub-agents | 3 |
| Starting commit | `b4cf2c4` |
| Final commit | `f6e8ba7` |
| Commits created | 14 |

Phases, in order: capture the starting point → reconcile the handover against
the code → restore the build → first execution → diagnose what it found →
finish the port's debts → CLI and configuration → fixtures, fuzzing,
benchmarks, snapshots → documentation and packaging → fix what fuzzing,
snapshot review and driving the binary found → validate.

Test counts: **0 → 731**. Not because 731 tests were written — most existed
already — but because the suite did not compile at `b4cf2c4` and so had never
run. Its first execution was 597 passing and 12 failing; the 12 are §5.

Resource notes: a full `cargo test --all-targets --all-features` takes about
18 s from cold and under 2 s warm. `target/` grew to 5.4 GB and `fuzz/target/`
to a similar size; free disk stayed above 600 GB throughout. Benchmarks were
first measured while three sub-agents were compiling, which made them look
superlinear; re-measured on an idle machine they are flat (§6). Measure
performance on a quiet machine or do not measure it.
