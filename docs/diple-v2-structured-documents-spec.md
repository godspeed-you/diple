# diple v2.0 - Structured Documents

**Specification status:** Implementation specification  
**Target release:** diple 2.0  
**Scope:** JSON and YAML as first-class document formats; architecture prepared for PDF as the next format class  
**Baseline:** diple 1.2.0, repository state reviewed on 2026-08-31  
**Repository:** https://github.com/godspeed-you/diple

---

## 0. Purpose of this specification

This document specifies the next architectural and product step for diple.

diple 1.x is an interactive terminal reader for Markdown. Its defining property is not merely that Markdown is rendered with colors and formatting. Its defining property is that diple understands the document as a structure: headings form a hierarchy, sections can be folded, search operates on semantic content, the table of contents represents the actual document hierarchy, and navigation stays meaningful independently of terminal line wrapping.

diple 2.0 generalizes that idea.

The product shall no longer be defined as "`less` for Markdown documents". It shall become:

> **`less` for structured documents.**

The first additional structured formats are:

- JSON
- YAML

This is not a request to add syntax highlighting for JSON and YAML. It is not a request to turn diple into `jq`, `yq`, an editor, a formatter, or a generic file browser.

The intent is that users can open a Markdown document, a JSON object, or a YAML configuration and interact with all three using the same mental model:

- understand the structure;
- move through semantic units rather than display lines;
- collapse and expand parts of the document;
- search content and reveal the path to a match;
- inspect the current structural location;
- keep the interaction stable when the terminal is resized;
- use the same pager, tab, split, configuration, theme, mouse, and terminal capability behavior that already defines diple.

The implementation must preserve that intent throughout the codebase. Format support is successful only when JSON and YAML feel native to diple rather than bolted onto it.

This specification is intentionally detailed enough to be used as the primary implementation contract for an autonomous coding agent. Ambiguities should be resolved according to the product principles defined here, not by choosing the easiest rendering shortcut.

---

## 1. Baseline and current architecture

At the time this specification was written, diple is version 1.2.0.

The current repository already contains several architectural properties that should be preserved:

- `src/document/ast.rs` defines the document AST and dense pre-order `NodeId` values.
- `src/document/parser.rs` parses Markdown into a derived `Document`.
- `src/document/sections.rs` derives heading-based sections, table-of-contents entries, and `FoldState`.
- `src/document/search.rs` builds a document-order search index over semantic node text.
- application, layout, rendering, CLI, configuration, Mermaid, and terminal concerns are already separated into their own modules.
- the current `Document` type contains Markdown-specific derived state such as heading sections, anchors, footnotes, and links.
- current folding is section-oriented: the heading remains visible while its body is hidden.
- search can reveal matches hidden inside collapsed sections.
- the current README explicitly positions Markdown as a structured document rather than colored text.

This architecture is a strong starting point, but the current `Document` and `NodeKind` types are specifically shaped around Markdown. diple 2.0 must generalize the format boundary without destroying the good semantic separation already present.

### 1.1 Required compatibility principle

The existing Markdown behavior is the compatibility baseline.

The JSON/YAML work MUST NOT regress:

- Markdown parsing;
- Markdown rendering;
- heading navigation;
- heading sibling navigation;
- folding behavior;
- search;
- table of contents;
- links;
- tables;
- code blocks and syntax highlighting;
- Mermaid behavior;
- tabs and splits;
- configuration;
- terminal capability detection;
- non-interactive output;
- startup behavior for existing Markdown workloads.

The correct architectural change is therefore not "replace the Markdown AST with a generic lowest-common-denominator tree". The correct change is to introduce a format-neutral document interface above format-specific semantic models.

---

## 2. Product definition

### 2.1 New product statement

The preferred short positioning for diple 2.0 is:

> `less` for structured documents.

A longer description may read:

> diple is an interactive terminal reader for structured documents. Markdown, JSON, and YAML are treated as documents rather than colored text: navigation, folding, search, outlines, paths, and layout operate on semantic structure.

The exact README wording may be refined during implementation, but the product meaning must remain equivalent.

### 2.2 What "structured document" means in diple

A structured document is content for which diple can derive meaningful semantic units and relationships.

Examples:

- Markdown: headings, sections, paragraphs, lists, tables, code blocks, links.
- JSON: objects, arrays, members, array items, scalar values.
- YAML: documents, mappings, sequences, entries, scalar values, anchors, aliases, tags, comments.

A structured document does NOT need to be a tree in every future format.

This distinction is important because PDF is explicitly expected to be a likely next supported format. A PDF may expose:

- pages;
- text blocks;
- paragraphs;
- headings inferred from layout;
- bookmarks/outlines;
- links;
- images;
- tables;
- coordinates.

For this reason, the common architecture must describe **capabilities and navigation semantics**, not assume that every future format is a JSON-like tree.

### 2.3 Core user promise

For all supported formats, diple should answer the same question:

> "How do I understand and move through this document quickly without leaving the terminal?"

The answer should be consistent:

- open it with `diple`;
- see a readable semantic representation;
- navigate by meaningful structure;
- collapse what is not relevant;
- search everything;
- understand where the cursor is;
- open another document beside it if useful;
- exit without terminal damage.

---

## 3. Scope

### 3.1 In scope for diple 2.0

diple 2.0 SHALL implement first-class reading support for:

1. Markdown - existing behavior retained.
2. JSON.
3. YAML 1.2, with practical compatibility for common real-world YAML.

The release SHALL include:

- automatic format detection;
- explicit format override;
- semantic rendering for JSON and YAML;
- structural cursor/navigation;
- folding of JSON/YAML containers;
- outline/tree sidebar for JSON/YAML;
- full-text search across keys, scalar values, and relevant YAML metadata;
- automatic expansion of ancestors when jumping to a search result;
- current path display;
- multi-document YAML support;
- anchors and aliases represented without unsafe recursive expansion;
- tags represented visibly;
- comments preserved and rendered for YAML;
- useful parse-error presentation;
- non-interactive output behavior;
- configuration options required by the new formats;
- complete key-hint/help integration;
- tests, snapshots, fuzzing strategy, benchmarks, and documentation;
- architecture that intentionally leaves a clean extension point for PDF.

### 3.2 Explicitly out of scope

diple 2.0 SHALL NOT become:

- a JSON query language;
- a YAML query language;
- a replacement for `jq`;
- a replacement for `yq`;
- a document editor;
- a formatter that rewrites files;
- a schema validator;
- a Kubernetes-aware tool;
- an OpenAPI-aware tool;
- a JSON Schema-aware tool;
- a YAML templating engine;
- a Helm renderer;
- a source-code IDE;
- a general file manager.

The following commands or concepts are explicitly out of scope:

```text
:set .spec.replicas = 4
.foo.bar[]
select(...)
map(...)
del(...)
```

A user may pipe the output of `jq`, `yq`, `kubectl`, `helm`, or another processor into diple. diple's responsibility begins at reading and understanding the resulting document.

---

## 4. Product principles

The following principles are normative. When implementation choices conflict, choose the option that best preserves these principles.

### P1 - Semantics before decoration

JSON/YAML support is not complete merely because the text is pretty-printed and colored.

A format is first-class only when diple understands:

- hierarchy;
- foldable units;
- navigation units;
- search text;
- current location;
- structural relationships.

### P2 - One interaction model, format-specific meaning

The same keys should perform analogous actions across formats.

Examples:

- `Enter` toggles the semantic unit under the cursor.
- `za`, `zc`, `zo` operate on the current foldable unit.
- `zM`, `zR` collapse/expand all foldable units.
- `/`, `n`, `N` search the semantic document.
- the outline represents the document hierarchy appropriate to the format.

The underlying meaning may differ:

- Markdown foldable unit = section.
- JSON/YAML foldable unit = object/mapping/array/sequence node.
- future PDF foldable unit might not exist or might apply only to inferred/outline sections.

Commands must therefore be capability-aware.

### P3 - Never destroy source meaning for presentation convenience

For a reader, fidelity matters.

The viewer must not silently:

- reorder JSON object members;
- reorder YAML mappings;
- discard YAML comments;
- expand aliases into misleading duplicated trees;
- change a YAML scalar's apparent type;
- merge YAML keys in a way that hides what the source actually contains;
- normalize away tags or anchors;
- pretend an invalid document is valid.

The visual representation may be reformatted for terminal readability, but semantic source facts must remain discoverable and truthful.

### P4 - Preserve the Markdown experience

The generalization must not make Markdown feel like a special case forced through an impoverished generic tree.

Markdown keeps Markdown-native concepts.

### P5 - The viewport is not the document

Navigation state must be based on semantic IDs/locations, not physical terminal rows.

Terminal resizing can change wrapping and layout, but it must not change:

- which semantic node is selected;
- which node is collapsed;
- which search result is active;
- the current path;
- logical navigation order.

### P6 - Graceful degradation remains mandatory

JSON/YAML must work over:

- SSH;
- tmux;
- terminals without true color;
- terminals without Unicode;
- non-interactive stdout.

No new feature may require an image protocol or graphical terminal.

### P7 - Source pipelines are a primary use case

The following must feel natural:

```bash
diple config.yaml
diple response.json
cat response.json | diple
curl -s https://example.invalid/api | diple
kubectl get deployment nginx -o yaml | diple
docker inspect container | diple
git show HEAD:config.yaml | diple
```

### P8 - Future formats must not require another architectural reset

The 2.0 architecture must make it possible to add a substantially different format - especially PDF - without replacing the application, navigation, search, tab, split, and terminal layers again.

---

## 5. User experience

### 5.1 JSON example

Source:

```json
{
  "metadata": {
    "name": "nginx",
    "labels": {
      "app": "frontend"
    }
  },
  "spec": {
    "replicas": 3,
    "containers": [
      {
        "name": "nginx",
        "image": "nginx:1.27"
      }
    ]
  }
}
```

Default expanded presentation should be conceptually equivalent to:

```text
{
  metadata: {
    name: "nginx"
    labels: {
      app: "frontend"
    }
  }
  spec: {
    replicas: 3
    containers: [
      [0]: {
        name: "nginx"
        image: "nginx:1.27"
      }
    ]
  }
}
```

The exact glyphs, punctuation, tree guides, and colors are theme/layout decisions. The semantic requirements are:

- keys and values are visually distinguishable;
- nesting is immediately visible;
- objects and arrays are foldable;
- array indices are visible or discoverable;
- scalar type is visually distinguishable;
- source order is retained;
- the cursor can identify one semantic row/node;
- the current path is available.

A collapsed `spec` object should be conceptually equivalent to:

```text
spec: {3 members}
```

A collapsed array:

```text
containers: [1 item]
```

An empty object and empty array should remain explicit:

```text
metadata: {}
containers: []
```

### 5.2 YAML example

Source:

```yaml
apiVersion: apps/v1
kind: Deployment

metadata:
  name: nginx
  labels:
    app: frontend

spec:
  replicas: 3
  template:
    spec:
      containers:
        - name: nginx
          image: nginx:1.27
```

diple should display the document in a semantic tree while preserving a recognizable YAML reading experience.

The viewer should not unnecessarily add JSON punctuation to YAML. The preferred representation remains mapping/sequence oriented:

```text
apiVersion: apps/v1
kind: Deployment

metadata:
  name: nginx
  labels:
    app: frontend

spec:
  replicas: 3
  template:
    spec:
      containers:
        [0]
          name: nginx
          image: nginx:1.27
```

Collapsed:

```text
spec:  {2 entries}
```

Exact compact markers may be refined, but JSON and YAML should remain visually related without becoming identical.

### 5.3 Path display

For JSON and YAML, diple SHALL expose the current semantic path.

Preferred visual form:

```text
spec › template › spec › containers › [0] › image
```

ASCII fallback:

```text
spec > template > spec > containers > [0] > image
```

The path should normally appear in the status area rather than consume document rows.

Requirements:

- update as the semantic cursor moves;
- use array/sequence indices;
- disambiguate keys that contain separators;
- provide a canonical copyable representation internally;
- remain stable across terminal resize;
- reflect the selected node, not merely the top visible line.

#### 5.3.1 Canonical internal path

diple SHALL define an internal path type rather than store a preformatted string.

Conceptual form:

```rust
enum PathSegment {
    Key(String),
    Index(usize),
    Document(usize),
}
```

The UI renderer may produce a human-friendly breadcrumb.

A future command that copies the path may expose JSON Pointer or another canonical syntax, but diple 2.0 does not require a query language.

### 5.4 Status line

The status line for structured data should provide compact orientation.

Example:

```text
deployment.yaml  YAML  42%   spec > template > spec > containers > [0]
```

Potential additional information when space permits:

- current node type;
- child count;
- YAML document number;
- search match `3/17`.

The layout must degrade cleanly on narrow terminals.

---

## 6. Format detection

Format detection is important because stdin is a first-class workflow.

### 6.1 CLI option

Add:

```text
--format <auto|markdown|json|yaml>
```

Default:

```text
auto
```

The configuration system MAY support a default format mode, but `auto` should remain the normal global default.

Explicit `--format` always wins over extension and content detection.

### 6.2 File extension hints

Recognized extensions:

Markdown:

```text
.md
.markdown
.mdown
.mkd
```

JSON:

```text
.json
.jsonc     # see JSONC rule below
```

YAML:

```text
.yaml
.yml
```

#### JSONC rule

Standard diple 2.0 JSON parsing SHALL be strict JSON.

`.jsonc` MAY be recognized only if a comment-capable JSONC parser is intentionally implemented. If not implemented, `.jsonc` must not be silently treated as strict JSON and then produce confusing errors. The simplest compliant behavior is:

- do not advertise JSONC support;
- treat `.jsonc` as unknown/Markdown unless `--format json` is specified;
- document that JSON comments are not part of JSON support.

JSONC support is not required by this specification.

### 6.3 Detection precedence

For named files:

1. explicit `--format`;
2. recognized extension;
3. content detection;
4. Markdown fallback.

For stdin / unknown filename:

1. explicit `--format`;
2. strong content detection;
3. Markdown fallback.

### 6.4 JSON content detection

JSON may be auto-detected when:

- the first non-whitespace token is `{` or `[`;
- strict JSON parsing succeeds.

Scalar JSON such as:

```text
42
"hello"
true
null
```

must NOT be auto-detected from anonymous stdin because that would steal ordinary textual input from Markdown behavior.

A file with a `.json` extension may contain a scalar JSON root and must still be parsed as JSON.

### 6.5 YAML content detection

YAML is difficult to auto-detect because many ordinary text files and Markdown fragments are valid YAML scalars or sequences.

diple SHALL use conservative detection.

Anonymous input may be auto-detected as YAML only when:

1. YAML parsing succeeds; and
2. at least one strong structured-YAML signal is present.

A strong signal is structure that prose and Markdown do not produce. Strong signals are:

- a `%YAML` or `%TAG` directive;
- a mapping nested under a mapping key;
- a mapping of two or more entries inside a sequence or mapping;
- anchor, alias, or explicit tag syntax.

The following are valid YAML structure but are **not** strong signals, because ordinary Markdown produces them:

- a sequence of scalars, or of one-entry mappings (`- Fast: written in Rust`);
- a mapping containing a sequence (`Pros:` over a list);
- a flat mapping of scalars, however many entries (`Status: done` over `Owner: alice`);
- an explicit document marker `---` or multiple documents (`---` is also a Markdown thematic break, and YAML front matter is a document).

Input whose first content line is indented (an indented Markdown code block) is not auto-detected as YAML.

*Revised 2026-09-24:* the first 2.0 draft listed "at least two mapping entries", "mapping containing a sequence", "nested sequence containing mappings" (without requiring more than one entry per mapping), an explicit `---` marker and "multiple YAML documents" as strong signals. Three independent audits showed each of them claiming ordinary Markdown on stdin, which AC-06 forbids; they were narrowed or withdrawn in favour of AC-06.

The following anonymous input should default to Markdown rather than YAML:

```text
hello
```

```text
- one
- two
```

```text
title: hello
```

unless stronger YAML structure exists.

This conservative policy intentionally protects existing Markdown/stdin behavior.

Users can always force YAML:

```bash
some-command | diple --format yaml
```

The common output of tools such as `kubectl -o yaml` contains enough nested mapping structure to be detected automatically.

### 6.6 Detection result visibility

The detected format shall be visible in the status line or document metadata area.

If auto-detection chooses YAML for stdin and parsing later fails due to incomplete streaming input, the user must receive a useful error rather than silent fallback after partial interpretation.

Detection can only recognise incomplete input where the parser can: a JSON value, string, keyword or container left open at the end of the input, a JSON object that fails after its first member name, and a YAML quoted scalar or flow collection left open after lines that are themselves confidently YAML (§6.5). YAML cut at a point where the remainder still parses — or fails in a way prose also fails, such as a key without its colon on the last line — cannot be told apart from Markdown and falls back; `--format yaml` or a `.yaml` name makes such input an error. (*Revised 2026-09-24*, after a broader rule refused ordinary notes with exit 1.)

diple reads stdin to completion before semantic parsing; it is not a live streaming YAML parser in 2.0.

---

## 7. Architectural model

### 7.1 Required architecture

The implementation shall introduce three conceptual layers:

```text
Input
  |
  v
Format detection
  |
  +----------------+----------------+----------------+
  |                |                |
Markdown backend   JSON backend     YAML backend
  |                |                |
  +----------------+----------------+----------------+
                   |
                   v
        Document session interface
                   |
       +-----------+-----------+
       |           |           |
     Search     Navigation    Layout
       |           |           |
       +-----------+-----------+
                   |
                 Render
```

The common layer is not required to erase all format-specific types.

A backend may retain its native semantic model and expose common operations through a stable interface.

### 7.2 Do not make one giant universal `NodeKind`

The current Markdown AST is expressive because `NodeKind` contains Markdown concepts such as:

- Heading
- Paragraph
- List
- Table
- CodeBlock
- Mermaid
- Image
- FootnoteDefinition

It would be a design mistake to add:

- JsonObject
- JsonArray
- YamlMapping
- YamlSequence
- PdfPage
- PdfTextBlock
- PdfImage
- ...

to the same enum indefinitely.

Instead, diple should separate:

1. format-native document representation;
2. common navigation/inspection interface.

### 7.3 Proposed top-level types

Names may be adapted to repository style, but the shape should remain equivalent.

```rust
pub enum DocumentKind {
    Markdown,
    Json,
    Yaml,
}

pub struct LoadedDocument {
    pub format: DocumentKind,
    pub source: SourceDocument,
    pub model: DocumentModel,
}
```

`DocumentModel` may be an enum:

```rust
pub enum DocumentModel {
    Markdown(MarkdownDocument),
    Structured(StructuredDocument),
}
```

or a trait-backed abstraction if that produces cleaner ownership and dispatch.

For 2.0, an enum is likely preferable:

- simpler ownership;
- easier exhaustive matching;
- no unnecessary dynamic dispatch;
- visible format capabilities;
- easier snapshots and testing.

The architectural goal is not trait purity. It is a stable format boundary.

### 7.4 Common semantic identity

Every interactable semantic unit must have a stable ID for the lifetime of the loaded document.

The existing dense pre-order `NodeId` model is good and should be retained or generalized.

Requirements:

- IDs are stable after parsing;
- IDs do not depend on terminal width;
- IDs are unique within the loaded document;
- pre-order/document order is deterministic;
- fold state uses IDs for foldable units;
- search results reference semantic IDs;
- layout rows reference semantic IDs;
- cursor state references semantic IDs.

### 7.5 Structured document model

JSON and YAML should share a structured-data model where practical.

Conceptual representation:

```rust
pub struct StructuredDocument {
    pub documents: Vec<StructuredRoot>,
    pub nodes: Vec<StructuredNode>,
    pub roots: Vec<StructuredNodeId>,
    pub outline: StructuredOutline,
    pub search: SearchIndex,
}

pub struct StructuredNode {
    pub id: StructuredNodeId,
    pub parent: Option<StructuredNodeId>,
    pub relation: NodeRelation,
    pub kind: StructuredNodeKind,
    pub span: Option<SourceSpan>,
    pub path: StructuredPath,
    pub metadata: StructuredMetadata,
}

pub enum NodeRelation {
    Root,
    MappingEntry { key: StructuredKey },
    SequenceItem { index: usize },
}

pub enum StructuredNodeKind {
    Mapping { children: Vec<StructuredNodeId> },
    Sequence { children: Vec<StructuredNodeId> },
    Scalar(ScalarValue),
    Alias(AliasRef),
}
```

This is conceptual, not mandatory Rust syntax.

#### 7.5.1 Why mapping entries should not necessarily be separate nodes

For rendering and navigation, a mapping entry such as:

```yaml
replicas: 3
```

is often best represented as one semantic row whose value node is scalar.

However, a mapping entry whose value is a mapping:

```yaml
metadata:
  name: nginx
```

requires the key `metadata` to act as the visible label of the foldable child container.

The implementation may model entries explicitly or attach their key/relation to the child value node.

The visible interaction must satisfy:

- cursor can select `metadata`;
- `metadata` can be folded because its value is a container;
- path includes `metadata`;
- search can match the key;
- the value retains its semantic type.

Avoid an AST shape that produces visually meaningless extra "entry" cursor stops.

### 7.6 Source model

Retain the original source string/bytes for the lifetime of a loaded document unless memory constraints force a future redesign.

Reasons:

- parse error excerpts;
- YAML comment and scalar presentation;
- source spans;
- future source-view toggles;
- fidelity for tagged/anchored values;
- debugging;
- future PDF-like backends may also need native source handles.

For Markdown, the existing source-span semantics may remain.

For JSON/YAML, `SourceSpan` should become format-neutral in documentation. The existing comment "Byte range in the original Markdown source" should be generalized to "original textual source".

---

## 8. Capability model

### 8.1 Motivation

Not every format supports the same structural features.

Markdown:

- hierarchy: yes, through headings/sections;
- folding: yes;
- outline: yes;
- links: yes;
- paths: section path possible but not currently required;
- pages: no.

JSON/YAML:

- hierarchy: yes;
- folding: yes;
- outline: yes;
- links: generally no semantic hyperlinks;
- paths: yes;
- pages: no.

Future PDF:

- hierarchy: maybe;
- folding: maybe limited;
- outline: maybe native/inferred;
- links: yes;
- paths: likely page/block rather than key path;
- pages: yes.

Therefore commands and UI hints must be capability-aware.

### 8.2 Capability set

Introduce an internal capability description conceptually equivalent to:

```rust
pub struct DocumentCapabilities {
    pub hierarchy_navigation: bool,
    pub sibling_navigation: bool,
    pub folding: bool,
    pub outline: bool,
    pub semantic_path: bool,
    pub links: bool,
    pub pages: bool,
    pub source_view: bool,
}
```

A bitflag or methods may be preferable.

The key-hints sidebar and help context should query capabilities rather than assume Markdown.

### 8.3 Behavior when a capability is absent

A key binding whose action is not meaningful should:

- not crash;
- not alter unrelated state;
- normally be omitted from context-sensitive key hints;
- optionally show a brief status message if manually invoked.

Example future PDF without folding:

```text
No foldable structure in this document
```

---

## 9. JSON semantics

### 9.1 JSON standard

Support strict JSON as defined by the JSON data model:

- object;
- array;
- string;
- number;
- boolean;
- null.

The viewer is read-only and does not need serialization round-tripping.

### 9.2 Object member order

Object members MUST be displayed in source order.

Although JSON object semantics do not require ordering, humans inspect configuration and API responses in the order emitted by the producer. Reordering damages readability.

If `serde_json::Value` is used, ensure a source-order-preserving map representation is enabled or build the diple structured model directly during parsing.

### 9.3 Duplicate object keys

JSON parsers often silently keep only the last duplicate key. A reader should not silently erase source content.

diple SHOULD preserve duplicate object members if the selected parser makes this feasible.

If the initial parser choice cannot preserve duplicates, this limitation must be explicitly tested, documented, and treated as a known fidelity limitation rather than silently assumed correct.

Preferred behavior:

```json
{
  "x": 1,
  "x": 2
}
```

renders both entries in source order.

Paths to duplicate keys need internal disambiguation even if the human breadcrumb still displays `x`.

This is one reason a direct event/parser-based model may be preferable to a plain map DOM.

### 9.4 Numbers

Preserve textual number representation where possible.

Examples:

```text
1
1.0
1e6
-0
```

A reader should not needlessly rewrite `1e6` as `1000000.0`.

If the parser loses lexical form, retaining source spans is preferred so display can use source text.

### 9.5 Strings

Display decoded string content while remaining truthful about control characters.

Requirements:

- do not emit raw terminal control sequences from document strings;
- sanitize control characters before rendering;
- represent embedded newlines visibly/wrapped safely;
- preserve Unicode;
- never allow file content to inject terminal escape codes.

Security requirement: untrusted JSON/YAML content must not be able to emit arbitrary ANSI/OSC sequences through diple's renderer.

### 9.6 Root scalars

A `.json` file or `--format json` input may contain a scalar root.

It should render as a one-node document with search support and a root path.

---

## 10. YAML semantics

### 10.1 Version and compatibility

Target YAML 1.2 semantics.

Real-world compatibility is important. The selected parser should tolerate common YAML 1.1-era syntax where doing so is safe and well-defined, but diple should not invent a private YAML dialect.

### 10.2 Parser requirements

The YAML parser must provide or enable:

- source spans;
- mapping and sequence events;
- scalar style information if available;
- anchors;
- aliases;
- tags;
- document boundaries;
- comments;
- useful syntax errors;
- protection against pathological parser behavior.

A low-level event parser is preferable to deserializing directly into application structs because diple is a source reader, not a data binder.

At the time of writing, `serde_yaml` is deprecated and no longer maintained. It should not be introduced as the foundation of new diple functionality.

A maintained low-level YAML parser with comment and span support should be selected. `granit-parser` is a strong candidate as of 2026-08-31, but it currently declares MSRV 1.81 while diple 1.2.0 declares Rust 1.80. If it is chosen, the implementation SHALL either:

- intentionally raise diple's MSRV to 1.81 and update packaging/documentation/tests; or
- select another maintained parser compatible with the chosen MSRV.

Do not hide an MSRV change inside an unrelated dependency update.

### 10.3 Comments

YAML comments are part of the human document even though they are not part of the YAML data model.

They MUST remain visible.

Examples:

```yaml
# Production replicas. Keep in sync with capacity plan.
replicas: 3
```

and:

```yaml
replicas: 3  # minimum for HA
```

The viewer should associate comments with nearby nodes when the parser provides placement metadata.

Comments should:

- render dimmer or with comment styling;
- be searchable;
- remain visible when their associated node is visible;
- collapse with the subtree they belong to;
- not become separate noisy navigation stops unless there is no better representation.

Freestanding document comments may be rendered as non-selectable or selectable informational rows depending on implementation practicality.

### 10.4 Mapping order

Preserve source order.

### 10.5 Sequences

Sequence items receive stable zero-based path indices:

```text
containers > [0] > image
```

Display may use `[0]`, `0`, or a themed marker, but the index must be discoverable.

### 10.6 Anchors

Example:

```yaml
defaults: &defaults
  retries: 3
  timeout: 5

service:
  <<: *defaults
```

Anchors must be visible at their definition.

Preferred rendering:

```text
defaults: &defaults
  retries: 3
  timeout: 5

service:
  <<: *defaults
```

Do not eagerly duplicate the anchored subtree beneath every alias.

Reasons:

- source fidelity;
- avoids exponential alias expansion;
- avoids misleading users about actual source;
- protects against YAML alias bombs.

### 10.7 Aliases

Aliases are semantic references.

An alias node should be searchable by alias name and visibly distinguishable.

Potential interaction:

- selecting `*defaults` and pressing `Enter` MAY jump to the anchor definition if the current interaction design can do so without conflicting with folding.
- if `Enter` is already needed to toggle the current foldable container, an explicit link-like selection action MAY be used.

Anchor jumping is desirable but not mandatory for 2.0 if it would destabilize the interaction model.

However, aliases MUST NOT be transparently expanded into duplicate child nodes.

### 10.8 Merge keys

YAML merge-key syntax such as:

```yaml
<<: *defaults
```

must be shown as source structure.

diple must not silently materialize merged keys as if they appeared literally in the mapping.

This is a reader, not a resolved configuration engine.

### 10.9 Tags

Explicit tags must be visible.

Example:

```yaml
date: !!timestamp 2026-08-31
custom: !MyType value
```

The renderer must not discard `!!timestamp` or `!MyType`.

Search should include the tag text.

### 10.10 Scalar styles

YAML has meaningful presentation styles:

- plain;
- single-quoted;
- double-quoted;
- literal block `|`;
- folded block `>`.

diple need not reproduce exact whitespace byte-for-byte in semantic view, but it should preserve distinctions important for reading.

Block scalars should be rendered as multi-line content, indented beneath the key, rather than compressed into a single escaped line.

Example:

```yaml
script: |
  echo hello
  echo world
```

should remain visually multi-line.

### 10.11 Multi-document YAML

Support:

```yaml
---
kind: ConfigMap
...
---
kind: Deployment
...
```

Each YAML document becomes a root in the structured model.

The outline should show document roots:

```text
Document 1
  kind
  metadata
  data
Document 2
  kind
  metadata
  spec
```

The path should include document identity when multiple roots exist:

```text
Document 2 > spec > template > spec
```

Navigation commands should move across document boundaries naturally in document order.

`zM` and `zR` apply across all YAML documents in the active diple document.

### 10.12 Directives

YAML directives such as `%YAML` and `%TAG` must not disappear.

They may be rendered as document metadata rows above the relevant root.

---

## 11. Folding model

### 11.1 Generalization

The existing `FoldState` is section-indexed.

diple 2.0 should introduce a format-neutral fold-state abstraction whose units are semantic fold targets.

Markdown continues to fold sections.

Structured data folds container nodes.

Conceptual representation:

```rust
pub type FoldId = usize;

pub struct FoldTarget {
    pub id: FoldId,
    pub semantic_node: SemanticId,
    pub parent: Option<FoldId>,
}
```

A simpler backend-specific fold map is also acceptable if the application can interact through common operations.

### 11.2 Foldable structured nodes

Foldable:

- JSON object;
- JSON array;
- YAML mapping;
- YAML sequence;
- optional YAML document root if useful.

Not foldable:

- scalar values;
- aliases unless represented as expandable references;
- standalone comments;
- tags themselves.

### 11.3 Collapsed summary

Collapsed containers must show enough information to remain meaningful.

Examples:

```text
metadata: {4 members}
containers: [3 items]
labels: {0 members}
```

YAML:

```text
metadata:  {4 entries}
containers:  [3 items]
```

Pluralization should be correct.

Optional compact type/value summary is allowed:

```text
containers: [3 items]  nginx, sidecar, exporter
```

but this is not required and should not add expensive scanning.

### 11.4 Existing fold commands

Retain:

```text
Enter
za
zc
zo
zM
zR
```

Their semantics become:

- Markdown: current section.
- JSON/YAML: current foldable container associated with the cursor row.

If the cursor is on a scalar child inside a container, `za`/`zc`/`zo` should act on the nearest intuitive fold target. Preferred behavior is the node represented by the current row; if the row is not foldable, act on its parent container only if that behavior is already consistent with diple's Markdown cursor model. Otherwise show "No foldable node here".

The behavior must be deterministic and documented.

### 11.5 Search reveal

When jumping to a search match, all collapsed ancestors required to make the matched row visible SHALL be expanded.

Other unrelated folds remain unchanged.

This mirrors current Markdown behavior and is a core product consistency requirement.

---

## 12. Navigation

### 12.1 Physical scrolling

Existing:

```text
j k ↓ ↑
Space b PgDn PgUp
g G
h l ← →
```

continues to control viewport movement.

Semantic cursor movement and viewport scrolling must remain conceptually separate even if the current implementation combines them.

### 12.2 Structural next/previous

Existing heading navigation keys:

```text
[ ]
```

shall become "previous/next primary structural node" for formats that support hierarchy navigation.

Markdown:

- previous/next heading.

JSON/YAML:

- previous/next sibling-or-document-order structural node according to the rules below.

The exact mapping must prioritize backward compatibility. Existing Markdown meaning cannot change.

#### Preferred structured-data semantics

For JSON/YAML:

```text
[  previous visible container/key node
]  next visible container/key node
```

Scalar rows may be included if doing so produces a more intuitive reading flow. The implementation team must settle this with snapshots and documented tests.

The design goal is that `[`/`]` jump between meaningful rows, not terminal lines.

### 12.3 Sibling navigation

Existing:

```text
{ }
```

currently moves between headings at the same or a higher level.

For JSON/YAML, interpret as sibling navigation:

```text
{  previous sibling
}  next sibling
```

If no sibling exists, climbing to an ancestor's next/previous sibling is acceptable if it matches the "same or higher level" Markdown mental model.

The chosen rule must be symmetric and test-covered.

### 12.4 Parent/child navigation

Structured documents benefit strongly from tree navigation.

Add optional default bindings:

```text
Alt-h or H?   parent
Alt-l or L?   first child
```

However, do not casually consume common existing pager keys.

This specification REQUIRES parent/child actions to exist as commands in the command/action model, but does not mandate a specific default key if keyspace conflicts are significant.

They must be:

- bindable in `[keys]`;
- listed in `:help`;
- exposed by key hints when relevant.

Suggested action names:

```text
parent_node
first_child
```

Sibling actions should also have explicit internal command names even if `{`/`}` map to them.

### 12.5 Outline navigation

The `t` sidebar remains the primary hierarchy overview.

Selecting an outline entry should move the document to that semantic node.

The outline should reflect current fold state visually but must not hide outline entries merely because content is folded.

---

## 13. Outline / table of contents

### 13.1 Naming

The existing user-facing concept may continue to be called "table of contents" for Markdown.

For structured data, "outline" is more accurate.

Internally, the component should become format-neutral.

Possible UI label:

```text
Outline
```

The `t` key can continue to toggle it for all formats.

Documentation may say:

> `t` toggles the document outline (table of contents for Markdown).

### 13.2 JSON outline

Example:

```text
root
  metadata
    name
    labels
      app
  spec
    replicas
    containers
      [0]
        name
        image
```

To avoid noise in very large objects, the outline MAY default to containers plus their immediately useful scalar children.

However, a consistent complete-tree outline is preferable if performance and readability remain acceptable.

The implementation SHALL define a deterministic policy.

Recommended default:

- show all mapping/object keys;
- show array/sequence indices;
- show scalar value previews only when short;
- trim previews to sidebar width;
- never allow a huge scalar to dominate outline width.

### 13.3 YAML outline

Same structural policy as JSON, plus:

- document roots for multi-document YAML;
- anchors may be annotated;
- comments do not receive outline entries by default;
- directives do not receive outline entries by default.

### 13.4 Outline performance

Opening the outline must not require reparsing the document.

Outline information should be derived during/after initial parse and retained.

---

## 14. Search

### 14.1 Unified search contract

The current `SearchIndex` design - pre-order semantic entries with text and stable node references - is conceptually appropriate and should be generalized.

Search remains literal full-text search by default.

No JSONPath/YAMLPath query language is introduced.

### 14.2 JSON searchable text

Index:

- object keys;
- string values;
- number lexical representation;
- `true`;
- `false`;
- `null`;
- optionally path representation if enabled by future setting.

A match in a key and a match in its scalar value should both navigate to the same semantic row if that is how the row is rendered.

The match range must still support highlighting the correct visible text region.

This may require richer search result metadata than only `(NodeId, start, end)`.

Conceptual:

```rust
enum MatchField {
    Label,
    Value,
    Comment,
    Tag,
    Anchor,
}
```

### 14.3 YAML searchable text

Index:

- mapping keys;
- scalar values;
- comments;
- anchor names;
- alias names;
- tags;
- directives if rendered.

### 14.4 Hidden match behavior

Jumping to a hidden match expands required ancestors.

### 14.5 Search result ordering

Results are in semantic document/source order.

### 14.6 Search state across format types

Tabs/splits already support multiple documents. Each pane/document should retain its own search state as it does today or as the current architecture dictates.

No global cross-document search is required.

---

## 15. Rendering

### 15.1 Render from semantics, not pretty-printed source

Do not implement JSON/YAML support by:

1. pretty-printing the input to a string;
2. syntax-highlighting it as a code block;
3. pretending line-based folding is semantic.

That would undermine resizing, folding, cursor identity, path awareness, and search reveal.

Render layout rows from semantic nodes.

### 15.2 Visual hierarchy

The renderer should use:

- indentation;
- optional tree guides;
- fold indicators;
- key/value differentiation;
- scalar type styling;
- comment styling;
- selected-row styling;
- search-match styling.

Unicode tree guides must have ASCII fallback.

Example Unicode:

```text
metadata
├─ name: nginx
└─ labels
   └─ app: frontend
```

ASCII fallback:

```text
metadata
|- name: nginx
`- labels
   `- app: frontend
```

The exact tree drawing is optional. Indentation alone is acceptable if it better matches diple's current aesthetics.

### 15.3 Scalar colors

Themes should expose semantic roles rather than hard-coded colors:

- structured key;
- string;
- number;
- boolean;
- null;
- comment;
- tag;
- anchor;
- alias;
- punctuation/delimiter;
- path;
- folded summary.

Existing themes must receive sensible defaults.

Do not require users to update existing config files.

### 15.4 Long keys

Long keys should wrap or horizontally scroll according to diple's existing layout policy.

Key/value association must remain visually clear after wrapping.

### 15.5 Long scalar values

Strings may be extremely long.

They must obey wrap/no-wrap configuration and horizontal scrolling behavior.

A long scalar should not allocate one rendered row per byte or otherwise create pathological layout behavior.

### 15.6 Multi-line YAML scalars

Render as child lines associated with the scalar row.

Search matches within the content must scroll to the correct rendered line.

### 15.7 Empty/null values

Differentiate clearly:

```text
null
""
{}
[]
```

YAML null spellings may be displayed in their source spelling when spans permit.

### 15.8 Source fidelity mode

A dedicated raw/source view is not required for JSON/YAML 2.0.

However, architecture should retain enough source information to add one later.

The Mermaid `s` source toggle is Mermaid-specific and should not be overloaded without an intentional future design.

---

## 16. CLI and configuration

### 16.1 New CLI options

Required:

```text
--format <auto|markdown|json|yaml>
```

Recommended structured-data options:

```text
--structured-indent <N>
--path <auto|always|never>
```

Avoid adding many format-specific switches unless they solve a clear reading problem.

### 16.2 Configuration

Suggested:

```toml
format = "auto"

[structured]
indent = 2
path = "auto"
show_indices = true
collapsed_summary = true
```

If existing configuration conventions prefer flat keys, follow repository style.

Defaults must produce good behavior without configuration.

### 16.3 Runtime commands

`:help` must list new settings/actions.

Runtime setting changes that affect layout only should re-render immediately.

Changing format after parsing is not required as a normal runtime setting. If a `:format` command is implemented, it must reparse from retained source atomically and restore safe cursor state.

### 16.4 `:open`

`:open` must detect JSON/YAML exactly like initial document loading.

Examples:

```text
:open tab deployment.yaml
:open side-by-side response.json
```

A Markdown document and YAML document may coexist in one split/session.

Global theme/settings continue to behave consistently.

---

## 17. Parse errors

### 17.1 Named file with explicit/extension format

If `config.yaml` is invalid YAML, do not fall back to Markdown.

Show a structured error.

Example:

```text
YAML parse error
config.yaml:18:9

  16 | spec:
  17 |   containers:
> 18 |      - name: nginx
     |         ^
  19 |     image: nginx:1.27

unexpected indentation
```

Exact parser wording may differ.

Requirements:

- file/source name;
- format;
- line and column when available;
- short source excerpt;
- caret/span when available;
- useful error message;
- terminal safely restored if error occurs after TUI initialization.

### 17.2 Auto-detected anonymous input

If high-confidence format detection chooses JSON/YAML and parsing fails, report the parse error.

If detection confidence was only tentative, fallback to Markdown may occur only before the format has been committed and only when doing so cannot hide a clearly intended structured document.

Keep detection code deterministic and testable.

### 17.3 Non-interactive mode

When stdout is not a terminal:

- valid documents render plain semantic text and exit 0;
- parse errors go to stderr;
- parse errors return non-zero.

---

## 18. Non-interactive rendering

Current diple behavior prints rendered plain text when stdout is not a terminal.

JSON/YAML shall behave sensibly in pipelines.

Example:

```bash
diple response.json | head -20
```

Output should be a readable textual representation of the semantic document.

Requirements:

- no ANSI unless explicitly forced by existing color policy;
- no cursor control;
- no interactive status line;
- deterministic output;
- source order preserved;
- reasonable indentation;
- comments retained for YAML;
- folded state is irrelevant - non-interactive output is fully expanded.

Do not silently output minified JSON.

---

## 19. Performance and scalability

### 19.1 Preserve startup character

diple currently has explicit startup-performance attention in the repository. New format support must retain the expectation that opening normal files feels immediate.

Performance shall be benchmarked separately for:

- Markdown;
- JSON;
- YAML.

### 19.2 Benchmark fixtures

Add representative fixtures:

#### Small

- 5-20 KiB JSON API response.
- 5-20 KiB YAML configuration.
- existing Markdown README fixture.

#### Medium

- 1 MiB JSON document with nested objects/arrays.
- 1 MiB YAML Kubernetes-like configuration corpus.

#### Large

- 10-50 MiB JSON log/export style document.
- 10-50 MiB YAML generated configuration where parser limits allow.

### 19.3 Performance goals

Do not regress Markdown startup budgets.

For JSON/YAML, set and enforce practical benchmark budgets on the existing CI/reference benchmark environment.

Rather than choosing meaningless universal millisecond numbers in this specification, implementation SHALL:

1. measure parser-only time;
2. measure model construction;
3. measure search-index construction;
4. measure first layout/frame;
5. record p50/p95 where the existing benchmark harness supports it;
6. add regression thresholds based on measured release-candidate behavior.

The architecture should avoid needless duplicated trees.

### 19.4 Lazy work

Possible lazy work:

- expensive scalar preview formatting;
- outline value previews;
- syntax-like token coloring of very large scalar blocks.

Not safely lazy:

- semantic hierarchy needed for navigation;
- fold relationships;
- basic search index if search currently expects eager indexing.

If search indexing dominates large-file startup, it may become lazy in a later optimization, but correctness comes first.

---

## 20. Resource and security limits

Structured-data parsers operate on untrusted input.

### 20.1 Depth

Protect against excessive nesting causing stack overflow.

Tree walking, rendering, search indexing, path construction, and fold reveal must avoid unbounded recursive Rust call stacks where malicious depth can crash the process.

Prefer iterative traversal for deep structures.

### 20.2 YAML aliases

Never recursively materialize aliases.

Apply parser/library safety limits where available.

### 20.3 Huge scalars

Avoid accidental quadratic copying.

Search indexing and lowercase mappings should be reviewed for large scalar memory behavior.

The existing search implementation creates normalized copies per indexed node. For large JSON/YAML values, consider whether this remains acceptable or whether a more memory-conscious representation is warranted.

Do not prematurely optimize at the cost of correctness, but add large-scalar tests.

### 20.4 Terminal escape sanitization

All document-controlled textual content must be sanitized before terminal rendering.

Particularly test:

- ESC;
- OSC introducers;
- C0 control characters;
- embedded carriage returns;
- bidi/control Unicode where relevant.

diple controls terminal escape sequences. The document does not.

---

## 21. Refactoring plan

This section specifies the intended architectural direction. Exact file names may change.

### 21.1 Phase A - establish format boundary

Refactor current Markdown loading so application code no longer directly assumes `document::Document` means Markdown.

Potential module structure:

```text
src/
  document/
    mod.rs
    source.rs
    format.rs
    capabilities.rs

    markdown/
      mod.rs
      ast.rs
      parser.rs
      sections.rs
      anchors.rs
      links.rs

    structured/
      mod.rs
      ast.rs
      outline.rs
      path.rs
      folds.rs

    json/
      mod.rs
      parser.rs

    yaml/
      mod.rs
      parser.rs

    search/
      mod.rs
```

A less disruptive structure is acceptable, but format ownership must become obvious.

### 21.2 Phase B - generalize application-facing document operations

Identify every place in:

- `app`;
- `layout`;
- `render`;
- `cli`;
- key hints/help

that directly matches Markdown node types.

Introduce format-aware adapters/common operations for:

- title/name;
- capabilities;
- semantic cursor targets;
- fold target at cursor;
- next/previous structural target;
- sibling navigation;
- outline entries;
- search entries/results;
- rendered blocks/rows;
- current path;
- link operations.

Do not create a "god trait" with dozens of mutable methods if an enum and helper functions are simpler.

### 21.3 Phase C - JSON backend

Implement JSON first.

Reasons:

- smaller semantic surface;
- no comments;
- no anchors/aliases;
- no multi-document stream;
- allows the new structured navigation/folding/rendering model to stabilize.

JSON acceptance must be complete before YAML-specific complexity is layered on top.

### 21.4 Phase D - YAML backend

Add YAML using the same structured model where semantics match, extending metadata for YAML-only constructs.

Do not force YAML anchors/tags/comments into JSON concepts if that loses fidelity.

### 21.5 Phase E - product/documentation integration

Update:

- README;
- man page;
- generated shell completions if `--format` changes;
- `docs/configuration.md`;
- `docs/keybindings.md`;
- troubleshooting;
- package descriptions;
- Cargo metadata description/keywords;
- changelog.

Suggested package description:

```text
Interactive terminal reader for structured documents
```

Keywords may include:

```text
markdown
json
yaml
pager
terminal
```

Subject to crates.io keyword count limits.

---

## 22. Dependency strategy

### 22.1 JSON

A maintained strict JSON parser is required.

`serde_json` is a reasonable choice if it can satisfy:

- order preservation;
- number/source fidelity requirements;
- duplicate-key policy or documented limitation.

If duplicate-key/source-span fidelity cannot be achieved with the chosen DOM path, consider an event/deserializer visitor that builds diple nodes directly.

### 22.2 YAML

Do not add deprecated `serde_yaml`.

Prefer a maintained YAML 1.2 parser capable of emitting spans and comments.

As of the specification date, `granit-parser` is a strong candidate because it is a low-level pure-Rust YAML 1.2 parser with comment/style/span support.

If its Rust 1.81 MSRV is accepted, update:

```toml
rust-version = "1.81"
```

and all documentation/CI assumptions consistently.

The implementation agent must verify the exact current crate API and license before integration.

### 22.3 Dependency minimization

Do not add a large generic framework merely to parse two formats.

diple should continue to have understandable dependencies and a practical CLI binary size.

---

## 23. Testing strategy

### 23.1 Unit tests

Add unit tests for:

#### Detection

- extension detection;
- explicit override;
- JSON stdin detection;
- YAML high-confidence detection;
- Markdown fallback;
- ambiguous Markdown list vs YAML list;
- invalid structured input.

#### JSON AST/model

- object;
- array;
- nested combinations;
- scalars;
- empty containers;
- root scalar;
- ordering;
- duplicate keys according to selected policy;
- escaped strings;
- Unicode;
- deep nesting;
- huge numbers.

#### YAML AST/model

- mappings;
- sequences;
- nested values;
- comments;
- anchors;
- aliases;
- merge keys;
- tags;
- block scalars;
- quoted/plain scalars;
- empty values;
- multiple documents;
- directives;
- Unicode;
- deep nesting;
- parser errors.

#### Paths

- simple keys;
- nested keys;
- array indices;
- keys containing dots;
- keys containing breadcrumb separators;
- duplicate keys;
- multi-document roots.

#### Folding

- collapse/expand;
- parent fold hides children;
- child fold state survives parent collapse/expand;
- collapse all;
- expand all;
- search reveal;
- multi-document folds.

#### Search

- key matches;
- value matches;
- comments;
- tags;
- anchors;
- aliases;
- case-insensitive Unicode behavior;
- correct visible match location after wrapping.

### 23.2 Snapshot tests

Snapshot representative terminal layouts for:

- JSON expanded;
- JSON partially collapsed;
- JSON narrow terminal;
- JSON no Unicode;
- JSON no color;
- YAML expanded;
- YAML comments;
- YAML anchors/aliases;
- YAML block scalar;
- multi-document YAML;
- outline open;
- key hints open;
- search match hidden then revealed;
- split Markdown + YAML;
- split JSON + JSON.

Use multiple terminal widths, including narrow widths likely to expose wrapping bugs.

### 23.3 Integration tests

Command-level tests:

```bash
diple file.json
diple file.yaml
diple --format json -
diple --format yaml -
cat file.json | diple
cat file.yaml | diple
diple file.json | head
```

Verify:

- exit codes;
- stderr on errors;
- no terminal escape leakage in non-color non-interactive output;
- config precedence;
- `--format` help/completion/man generation.

### 23.4 Property/fuzz tests

The existing repository already has a fuzz area.

Add fuzz targets for:

- format detection;
- JSON parser adapter/model construction;
- YAML parser adapter/model construction;
- structured layout;
- structured path formatting;
- folding/reveal traversal.

Critical invariant:

> Arbitrary input must not panic, hang, recurse until stack overflow, or emit uncontrolled terminal escapes.

### 23.5 Conformance corpus

YAML has enough edge cases that handwritten tests are insufficient.

The implementation SHOULD integrate or periodically validate against a YAML conformance corpus compatible with the selected parser.

diple does not need to own parser conformance if the parser library already does, but diple must test its adapter on representative difficult cases.

---

## 24. Accessibility and terminal compatibility

### 24.1 Color is never the only type signal

String, number, boolean, null, key, tag, alias, and comment distinctions must remain understandable without color.

Use punctuation, labels, or textual form in addition to color.

### 24.2 ASCII mode

Paths:

```text
a > b > [0]
```

Tree guides and fold indicators need ASCII fallbacks.

### 24.3 Narrow terminals

At 40-60 columns:

- path may truncate from the left while retaining the current node;
- outline width must remain bounded;
- selected key/value must remain readable;
- status information should prioritize filename, format, progress, and current leaf path.

### 24.4 Copy/select mode

Existing behavior that hands mouse selection back to the terminal must continue to work with structured rendering.

---

## 25. Documentation requirements

### 25.1 README

Update opening description and examples.

Suggested usage block:

```bash
diple README.md
diple deployment.yaml
diple response.json

kubectl get deployment nginx -o yaml | diple
curl -s https://api.example.com/state | diple
```

README must explain the product distinction:

> diple does not merely syntax-highlight structured formats. It parses their structure so folding, navigation, search, and the outline operate on semantic nodes.

### 25.2 Keybindings

Document format-dependent semantics.

Example table:

| Key | Markdown | JSON/YAML |
| --- | --- | --- |
| `Enter` | toggle section / open link | toggle container |
| `[` `]` | previous/next heading | previous/next structural node |
| `{` `}` | same/higher heading | sibling navigation |
| `za` `zc` `zo` | section fold | container fold |
| `zM` `zR` | all sections | all containers |
| `t` | table of contents | structure outline |

Exact structured navigation semantics must match implementation.

### 25.3 Configuration docs

Document all new options and defaults.

### 25.4 Troubleshooting

Include:

- wrong auto-detected format;
- forcing `--format`;
- invalid YAML;
- YAML feature/parser limitations if any;
- ambiguous stdin;
- large-file behavior.

---

## 26. Compatibility and migration

### 26.1 CLI compatibility

Existing commands without `--format` remain valid.

```bash
diple README.md
cat README.md | diple
```

must behave as before.

### 26.2 Configuration compatibility

Existing config files remain valid without modification.

New settings have defaults.

Unknown new enum values must produce the same quality of config error as existing settings.

### 26.3 Keybinding compatibility

Do not remove or repurpose a key in Markdown mode.

A key may gain an analogous structured-data meaning.

### 26.4 Public Rust API

If diple exposes library APIs through `docs.rs`, treat current public AST types carefully.

Because 2.0 is a major version, breaking API changes are permitted, but they should still be intentional and documented.

Prefer clean long-term API boundaries over preserving an accidental Markdown-specific public type under a misleading generic name.

---

## 27. Acceptance criteria

diple 2.0 is complete only when all criteria below are met.

### AC-01 - Markdown remains first-class

All existing Markdown functionality and tests pass with no intentional UX regression.

### AC-02 - JSON opens natively

```bash
diple file.json
```

opens a semantic, navigable, foldable document.

### AC-03 - JSON stdin works

```bash
cat file.json | diple
```

auto-detects normal object/array JSON.

### AC-04 - YAML opens natively

```bash
diple file.yaml
```

opens a semantic, navigable, foldable document.

### AC-05 - Common YAML pipelines work

Typical nested output such as:

```bash
kubectl get deployment nginx -o yaml | diple
```

is automatically recognized as YAML without requiring `--format yaml`.

### AC-06 - Ambiguous text remains Markdown

Simple prose and Markdown lists piped to diple are not stolen by permissive YAML parsing.

### AC-07 - Explicit override works

```bash
diple --format yaml -
diple --format json file.data
diple --format markdown config.yaml
```

behave according to explicit selection.

### AC-08 - Folding is semantic

Container folds survive terminal resizing and never depend on display line numbers.

### AC-09 - Search reveals hidden matches

Searching for a value in a collapsed nested structure opens only the ancestor path required to reveal it.

### AC-10 - Path orientation works

The selected structured node exposes a correct path including sequence/array indices and YAML document number where needed.

### AC-11 - Outline works

`t` shows a navigable semantic outline for JSON and YAML.

### AC-12 - YAML comments survive

Comments remain visible and searchable.

### AC-13 - YAML aliases are safe

Aliases do not cause recursive materialization or exponential expansion.

### AC-14 - YAML metadata survives

Anchors, aliases, tags, directives, and multi-document boundaries are not silently erased.

### AC-15 - Source order survives

JSON members and YAML entries render in source order.

### AC-16 - Non-interactive mode is useful

Structured documents piped through diple render deterministic readable plain text.

### AC-17 - Parse errors are useful

Invalid JSON/YAML produces line/column/source context when available and exits safely.

### AC-18 - No terminal injection

Document text containing terminal escape/control sequences cannot execute arbitrary terminal control behavior.

### AC-19 - Tabs and splits are format-independent

Markdown, JSON, and YAML can be mixed across tabs and panes.

### AC-20 - Help is context-aware

Key hints and `:help` correctly describe available structured-document actions.

### AC-21 - Packaging is complete

Man pages, completion generation, package metadata, README, configuration docs, keybindings docs, and changelog reflect 2.0.

### AC-22 - Tests and fuzzing cover the new parsers

No new parser/model path is shipped without regression tests and fuzz coverage.

### AC-23 - Future PDF support is not architecturally blocked

The application-facing document interface is capability-oriented enough that a paged, partially hierarchical PDF backend can be added without another rewrite of app navigation/search/tab/split state.

---

## 28. Definition of done for implementation work

The implementation agent should not stop after "JSON renders" or "YAML parser integrated".

The work is done only when:

1. architecture is generalized;
2. JSON is complete;
3. YAML is complete;
4. Markdown remains stable;
5. all interactive commands behave consistently;
6. tests pass;
7. new tests are added;
8. fuzz targets build;
9. benchmarks exist;
10. docs are updated;
11. packaging metadata is updated;
12. `cargo fmt` passes;
13. `cargo clippy` passes under project policy;
14. `cargo test` passes;
15. release build succeeds for supported targets;
16. generated man/completion artifacts still work;
17. no temporary compatibility hacks remain undocumented.

Do not leave "TODO: implement YAML comments later" or equivalent incomplete semantic support in a release advertised as YAML support.

---

## 29. Future format: PDF

PDF is deliberately not part of the diple 2.0 implementation scope, but it is the next format specifically anticipated by this architecture.

This section is normative only insofar as 2.0 must not block it.

### 29.1 Why PDF fits the product

PDF appears superficially unrelated to JSON/YAML, but it fits the same product promise:

> A structured document should be understandable and navigable in the terminal without flattening it into meaningless lines.

A terminal PDF experience could offer:

- extracted/reflowed text;
- page navigation;
- PDF outline/bookmark navigation;
- links;
- headings when available/inferred;
- search;
- images where terminal capability allows;
- page thumbnails or images in capable terminals;
- text fallback everywhere else.

That would make diple meaningfully broader than a programmer-only config viewer while retaining the same reader identity.

### 29.2 Why PDF must not be forced into the JSON/YAML tree

A PDF's primary structure may be:

```text
Document
  Page 1
    text blocks
    image
    links
  Page 2
    ...
```

Its semantic reading structure may instead be:

```text
Outline
  Introduction
  Architecture
  Appendix
```

Those structures are not necessarily the same.

Therefore diple 2.0 must not define the common document interface as:

```rust
fn root_tree() -> Tree
```

and assume every operation comes from that tree.

It should expose capabilities and navigation models.

### 29.3 Anticipated PDF capabilities

A future PDF backend may report:

```text
hierarchy_navigation = maybe
sibling_navigation   = maybe
folding              = maybe/no
outline              = yes if bookmarks/inference exists
semantic_path        = page/block-oriented
links                = yes
pages                = yes
source_view           = page image / extracted text mode
```

### 29.4 Expected future PDF modes

A future design should likely distinguish:

#### Reflow mode

Extract text and semantic blocks, then render them to terminal width.

Benefits:

- readable over SSH;
- searchable;
- copyable;
- terminal-native.

#### Page mode

Render the PDF page as an image where the terminal supports images.

Benefits:

- layout fidelity;
- diagrams;
- typography;
- forms.

Fallback:

- reflow mode.

The existing terminal capability architecture and Mermaid image fallback philosophy are useful precedents.

### 29.5 Native PDF outline

When a PDF includes bookmarks/outlines, they map naturally to diple's outline sidebar.

This is another reason the 2.0 "TOC" implementation should become a format-neutral outline component.

### 29.6 PDF search

Search results need:

- page number;
- semantic block/span;
- ability to move viewport/page to the match.

This is another reason generalized search results should not assume every result is a Markdown node with one contiguous rendered line.

### 29.7 PDF pages

The capability model should reserve the possibility of:

```text
next_page
previous_page
page_number
page_count
```

No key bindings are specified in 2.0.

### 29.8 Scanned PDFs and OCR

Scanned PDFs require OCR to become searchable/reflowable.

A future first PDF release should not necessarily bundle OCR.

Possible future policy:

- native text PDFs: full support;
- scanned PDFs: page-image viewing plus clear "no text layer" status;
- optional external OCR integration later.

Do not contaminate 2.0 JSON/YAML work with OCR dependencies.

### 29.9 PDF implementation boundary

No PDF library should be added in diple 2.0 merely to prove extensibility.

The architecture is considered PDF-ready if a future backend can plug in without changing the fundamental ownership of:

- app session;
- tabs;
- splits;
- search UI;
- outline UI;
- key hints;
- terminal lifecycle;
- viewport/cursor abstraction.

---

## 30. Recommended implementation sequence

This sequence is not an MVP plan. Every step is part of the finished 2.0 release. The sequence minimizes architectural risk.

### Step 1 - characterization tests

Before refactoring:

- expand tests around current Markdown navigation/folding/search;
- capture snapshots for key existing behavior;
- ensure regressions are observable.

### Step 2 - input/source + format abstraction

Introduce:

- source identity;
- format detection;
- explicit `--format`;
- format enum;
- loader boundary.

Markdown still behaves identically.

### Step 3 - application-facing capability abstraction

Remove direct Markdown assumptions from common app/UI code.

Do not implement JSON yet until Markdown passes through the new boundary unchanged.

### Step 4 - structured AST/model

Implement:

- node IDs;
- parent/children;
- relation/key/index;
- path;
- generic structured fold state;
- outline;
- search adapter.

### Step 5 - JSON parser

Complete JSON including:

- source order;
- scalar rendering;
- arrays;
- fold;
- search;
- path;
- outline;
- errors;
- stdin detection;
- non-interactive output.

### Step 6 - structured UX stabilization

Exercise large and deeply nested JSON.

Fix navigation semantics before YAML adds additional complexity.

### Step 7 - YAML parser

Add:

- mappings/sequences;
- comments;
- anchors/aliases;
- tags;
- directives;
- scalar styles;
- multi-document streams.

### Step 8 - integration/polish

Complete:

- themes;
- key hints;
- help;
- configuration;
- docs;
- package metadata;
- man page;
- completion;
- snapshots;
- benchmarks;
- fuzzing.

### Step 9 - release validation

Run all release and packaging workflows.

Test manually at minimum:

```text
local modern terminal
tmux
SSH session
NO_COLOR
ASCII/non-Unicode fallback if supported by test harness
stdin
non-interactive pipe
narrow terminal
wide terminal
```

---

## 31. Design invariants for code review

Reviewers and implementation agents should repeatedly verify the following.

#### Invariant 1

A semantic selection has an ID independent of rendered row position.

#### Invariant 2

Resizing the terminal never reparses simply to determine structure.

#### Invariant 3

Folding hides semantic descendants, not arbitrary line ranges.

#### Invariant 4

Search indexes semantic content and can reveal hidden ancestors.

#### Invariant 5

Source order is preserved.

#### Invariant 6

Markdown-native concepts remain Markdown-native.

#### Invariant 7

YAML source constructs are not erased merely because they are absent from JSON's data model.

#### Invariant 8

The common application layer asks what a document can do rather than assuming every document has Markdown headings.

#### Invariant 9

Untrusted document strings cannot write terminal control sequences.

#### Invariant 10

No implementation shortcut turns diple into a colored `cat`.

---

## 32. Example interaction scenarios

### 32.1 Inspect Kubernetes YAML

```bash
kubectl get deployment nginx -o yaml | diple
```

Expected experience:

1. diple detects structured YAML.
2. status shows `YAML`.
3. cursor begins at the first meaningful root entry.
4. user opens outline with `t`.
5. user jumps to `spec`.
6. user collapses `metadata`.
7. user searches `/image:`.
8. diple reveals the `containers` ancestry if collapsed.
9. status path shows:

```text
spec > template > spec > containers > [0] > image
```

10. terminal resize retains the same selected semantic node.

### 32.2 Inspect API JSON

```bash
curl -s https://example.invalid/api/v1/state | diple
```

Expected:

- auto-detect JSON object/array;
- long arrays can be collapsed;
- `zM` creates a compact overview;
- opening selected branches creates an exploratory tree-reader workflow;
- search finds both property names and values.

### 32.3 Compare Markdown and configuration

```text
diple README.md
:open side-by-side deployment.yaml
```

Expected:

- left pane remains normal Markdown;
- right pane is YAML;
- `Ctrl-W` moves focus;
- `t` produces Markdown TOC on left and YAML outline on right;
- global theme changes both;
- format-specific key hints change with focus.

### 32.4 Ambiguous stdin

```bash
printf '%s\n' '- one' '- two' | diple
```

Expected:

- Markdown, not YAML.

```bash
printf '%s\n' 'metadata:' '  name: nginx' 'spec:' '  replicas: 3' | diple
```

Expected:

- YAML.

### 32.5 Invalid YAML

```bash
diple broken.yaml
```

Expected:

- useful source-positioned error;
- no raw Rust panic;
- no damaged terminal;
- non-zero exit status when no interactive document can be opened.

---

## 33. Open implementation choices that must be resolved in code, not left ambiguous

The following choices are intentionally delegated to implementation because they depend on existing app/layout constraints. They MUST be decided, documented, and tested before release.

### 33.1 Structured `[` / `]` exact traversal

Choose one:

- every visible semantic row;
- only container/key rows;
- another deterministic semantic traversal.

Criteria:

- feels useful on both JSON and YAML;
- not dominated by punctuation/scalars;
- symmetric;
- stable under folding.

### 33.2 Parent/child default bindings

Actions are required; exact defaults may be chosen after auditing current keyspace.

### 33.3 Duplicate JSON keys

Preferred: preserve.

If parser constraints make this disproportionately complex, explicitly document the limitation and create an issue before release rather than silently ignoring it.

### 33.4 Exact YAML comment attachment rules

Follow parser placement metadata where possible. Snapshot difficult cases.

### 33.5 Exact tree guide aesthetic

Must fit existing diple themes and graceful-degradation design.

These are implementation choices, not permission to omit the features themselves.

---

## 34. Suggested internal API sketch

This sketch exists to make the intended separation concrete. It is not a requirement to copy the names verbatim.

```rust
pub enum DocumentModel {
    Markdown(markdown::Document),
    Structured(structured::Document),
}

impl DocumentModel {
    pub fn kind(&self) -> DocumentKind;
    pub fn capabilities(&self) -> DocumentCapabilities;

    pub fn title(&self) -> Option<&str>;

    pub fn search_index(&self) -> &SearchIndex;
    pub fn outline(&self) -> Outline<'_>;

    pub fn first_semantic_id(&self) -> Option<SemanticId>;
    pub fn next_semantic(&self, from: SemanticId, folds: &FoldState)
        -> Option<SemanticId>;
    pub fn previous_semantic(&self, from: SemanticId, folds: &FoldState)
        -> Option<SemanticId>;

    pub fn parent(&self, from: SemanticId) -> Option<SemanticId>;
    pub fn first_child(&self, from: SemanticId) -> Option<SemanticId>;
    pub fn next_sibling(&self, from: SemanticId) -> Option<SemanticId>;
    pub fn previous_sibling(&self, from: SemanticId) -> Option<SemanticId>;

    pub fn fold_target(&self, at: SemanticId) -> Option<FoldId>;
    pub fn reveal(&self, at: SemanticId, folds: &mut FoldState);

    pub fn path(&self, at: SemanticId) -> Option<DocumentPath<'_>>;
}
```

Rendering should consume a view/layout representation rather than require every backend to expose the same AST node enum.

Potential:

```rust
pub struct SemanticRow {
    pub semantic_id: SemanticId,
    pub depth: usize,
    pub role: RowRole,
    pub fragments: Vec<StyledFragment>,
    pub fold: Option<FoldMarker>,
}
```

A layout phase can turn semantic rows into wrapped terminal rows while retaining the semantic ID on every wrapped fragment.

This explicitly preserves the key invariant:

> one semantic node may render to many terminal rows, but those terminal rows do not become the identity of the node.

---

## 35. Release naming and versioning

Because this change broadens the core product identity and likely changes public Rust APIs, `2.0.0` is the appropriate release target.

Suggested changelog headline:

```text
## 2.0.0 - Structured Documents

diple is now `less` for structured documents. JSON and YAML join Markdown as
first-class semantic document formats with navigation, folding, search, paths,
and outlines.
```

This release should be presented as a product evolution, not as two isolated parsers.

---

## 36. Final product intent

The defining test for this release is not:

> "Can diple display JSON and YAML?"

Many terminal tools can do that.

The defining test is:

> "Does opening a 2,000-line Kubernetes YAML or a large nested API response in diple feel like entering and navigating the document's structure rather than scrolling through colored text?"

If the answer is yes, the extension belongs in diple.

If the implementation merely pretty-prints structured data, the work has missed the purpose of the release.

diple 2.0 should make these commands feel like one coherent product:

```bash
diple README.md
diple deployment.yaml
diple response.json
```

The format changes.

The interaction philosophy does not.

And the architecture produced by this release should make the next step possible:

```bash
diple architecture.pdf
```

without having to reinvent diple again.
