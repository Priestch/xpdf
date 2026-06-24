# PDF-X Architecture

> This is the detailed design document. For the always-loaded operating rules, see
> [`../CLAUDE.md`](../CLAUDE.md). For the current foundation migration, see
> [`phase-0-foundation-spec.md`](phase-0-foundation-spec.md).

## Technical Goal (primary, explicitly unbundled)

Earlier drafts fused several goals into one (PDF.js fidelity + progressive loading +
immutable viewer + collaborative editor). Those goals constrain each other, so they are
unbundled into one primary objective with explicit positioning:

> **A platform-agnostic, composable PDF core library** that (1) loads **progressively with
> bounded memory on every source** — local or network, (2) is **WASM-clean** (no baked-in
> async runtime or threading), and (3) ships as a **viewer first** but is **editor-ready by
> construction**: the mutation/delta path is wired into object resolution from day one, so
> adding editing later is a feature, not a refactor.

- **Primary objective**: a fast, faithful, progressive, composable, WASM-clean **viewer core**.
- **Deferred but architecturally preserved**: concrete editor commands and collaboration.
  The foundation must not preclude them.
- **Editor scope is open**: v1 ships viewer-only, but the delta/resolution seam is the
  foundation, so editor features slot in without refactoring the viewer.

## Composable Core + Shells

The project is a **composable core library** (`pdf-x-core`) at the bottom, used by platform
shells (`tauri-app`, future web/wasm shells) on top. The dependency direction is strict: the
core never depends on a shell concern.

```
                  Shells (platform-specific I/O, async, threading, UI)
        native (tokio / blocking)    web/wasm (fetch, canvas)    tests (Vec<u8>)
                  \                        |                        /
                   \   fulfill Need::Range requests + drive loop   /
                    ╔══════════════════════════════════════════════╗
                    ║   pdf-x-core  (sync, runtime-free)           ║
                    ║  DocStore (delta → base) ── Resolver      ║
                    ║  Parser · XRef · Crypto · Fonts              ║
                    ║  Interpreter ──> Device trait                ║
                    ╚══════════════════════════════════════════════╝
        Device backends as separate crates/adapters: tiny-skia (now), vello/sitro, canvas-2d (wasm)
```

## The Three Foundational Decisions

These are load-bearing and make the goal (progressive + WASM-clean + editor-ready)
simultaneously achievable.

### A. The core is pure synchronous Rust; all I/O and async live in shell adapters

The core must not hard-depend on, or internally drive, an async runtime (tokio/async-std/smol),
must not `.await`, and must not spawn or assume OS threads. Concretely: **no baked-in runtime
or threading.**

- The core defines a **synchronous, pull-based data protocol**: when it needs bytes it does
  not have, it returns a *value* — `Need::Range(pos, len)` — not a thrown exception and not a
  future.
- The **shell owns the load-and-retry loop** and fulfills those requests with
  platform-appropriate I/O: blocking file read on native, async `fetch` on web, in-memory bytes
  in tests.

| Concern | Lives in… | Examples |
|---|---|---|
| Async runtime, `.await`, tasks | **Shell** | tokio (native), promises (web), none (tests) |
| Thread spawning, parallelism | **Shell** | parallel page render over snapshots |
| Blocking I/O, network, filesystem | **Shell** | `fs::read`, `fetch` |
| Parsing, resolution, interpretation, the `Need::Range` protocol, the doc store, caching | **Core** | plain sync `fn`, owned data |

What this rule does **not** mean:
- It does **not** forbid multithreading. A native shell may render pages in parallel over
  immutable `ResolvedPage` snapshots. The core provides thread-safe owned data; the shell
  orchestrates threads.
- It does **not** forbid `Send + Sync`. The opposite — core public types should be
  `Send + Sync` (owned data, no `Rc`/`RefCell` in the public API) so a native shell can move
  them across threads while the same types also run on single-threaded WASM.
- It does **not** forbid caching/mutation. The `DocStore` mutates and caches freely — it just
  does not spawn threads or drive a runtime to do so.

### B. A central DocStore is the single resolution point: `delta → base`, always

One `DocStore` owns the progressively-loaded base (+ its caches) **and** the `DeltaLayer`.
**Every** object lookup goes through `store.resolve(ref)` = `delta.or_else(base)`, from day
one, even with an empty delta.

- Caches live **inside** the store and are invalidated by delta mutations. Nothing outside the
  store caches object lookups, so there is no stale-cache class of bugs.
- The store mutates freely for caching and editing. Because WASM is single-threaded and native
  shells serialize store access, there is no aliasing problem — no `&mut XRef` threaded through
  every render call.
- Bonus: a delta override is *local*, so `delta → base` resolution skips a remote fetch for
  overridden objects. Editing and progressive loading cooperate rather than fight.

This is what "editor-ready by construction" means: the resolution seam **is** the foundation,
so the editor is added by populating the delta with zero viewer refactor.

### C. Resolve-then-interpret: the progressive contract is owned by the resolver, never seen by the interpreter

Progressive loading is a hard requirement on every source, but the complexity is quarantined to
one phase:

- **Resolve phase**: walks a page's object graph on demand, emits `Need::Range` requests, may
  suspend/resume. It owns **all** the progressive-loading complexity. Resolution is naturally
  idempotent (re-walking a graph), so it is safe to retry.
- **Interpret phase**: content-stream → `Device` operations, a **pure synchronous function over
  a fully-resolved, immutable `ResolvedPage`**. It never blocks, never sees missing data, and
  never needs to be restartable.

This deletes the old "interpretation may fail mid-stream and must cleanly restart" invariant —
an undeclared, easy-to-violate hazard. Only the resolver can fail on missing data;
interpretation is a pure function: testable, parallelizable, WASM-safe.

`ResolvedPage` is immutable, self-contained, `Send + Sync + 'static`. So native shells render
pages in parallel off snapshots, while the single-threaded store keeps being edited. Edits
mutate the store; the next render resolves a fresh snapshot. This snapshot model is also the
natural basis for future collaboration (a snapshot ≈ a diff against base).

**Fast first paint is preserved**: for a linearized PDF, page 1's object graph sits at the front
of the file, so the resolver fetches just those objects and the interpreter renders page 1
before the rest downloads. Linearization is what makes resolve-then-interpret give progressive
display — the two belong together.

## Progressive Loading

Progressive loading is the **core differentiator**. Concepts replicated from PDF.js:

1. **Chunked Loading**: Load PDF data in chunks (typically 64KB), regardless of source
2. **Source Abstraction**: Unified interface for network, filesystem, and other sources
3. **Range Requests**: Network loader uses HTTP Range headers; filesystem loader uses seek + read
4. **Incremental Parsing**: Parse PDF objects as chunks arrive, don't wait for complete file
5. **Lazy Page Loading**: Parse page content streams only when requested
6. **Linearized PDF Support**: Recognize and optimize for linearized PDFs (fast web view)

The core never blocks for data and never loads the whole file. When data is missing, the core
returns a `Need::Range(pos, len)` value; the shell fulfills it and re-drives the operation
(`drive`). This is the value-returning refinement of PDF.js's exception-driven loading.

```rust
// ✅ Core returns a data *request*; shell owns the load+retry loop.
loop {
    match resolver.step() {
        Ok(resolved) => break resolved,
        Need::Range(pos, len) => provider.fill(pos, len)?, // shell I/O
    }
}
// Interpretation then runs as a pure sync fn over `resolved` — never sees missing data.
```

```rust
// ❌ NEVER: loading all data upfront.
let all_data = stream.get_all_bytes()?;
// ❌ NEVER: interpretation that can fail mid-stream on missing data — interpret only over a
//           fully-resolved ResolvedPage (decision C).
```

Reference `pdf.js/src/core/stream.js` and `pdf.js/src/core/chunked_stream.js` for patterns.

### Evaluating External Crates

Before using any external Rust crate for PDF functionality, evaluate whether it respects
progressive loading:

- ✅ **Acceptable: isolated-operation crates** — stream decoders (`flate2`), image decoders
  (`png`, `jpeg-decoder`), utilities that operate on data you've already loaded.
- ❌ **Reject: full-file crates** — anything requiring `Arc<[u8]>`/`Vec<u8>` for the entire PDF,
  or `load()`/`open(path)`/`from_file()` APIs, or internal buffering that loads large portions
  without your control.
- ✅ **Acceptable: learning from source** — you may read external crate source to understand
  algorithms and adapt patterns, but not use them as dependencies if they violate progressive
  loading.

## Delta Layer Architecture

The **Delta Layer** enables editing while keeping the base PDF untouched, and is wired into
object resolution from day one (decision B). v1 ships with an empty delta; editor features are
added by populating it.

### Core Design Principles

1. **Single resolution point**: object lookup always goes through `DocStore::resolve(ref)` =
   `delta → base`. The delta is checked first (cheap, local) and may even *avoid* a remote
   fetch for overridden objects.
2. **State-diff ≠ undo-log**: the `DeltaLayer` stores the **mergeable, serializable
   object-level state diff** (modified / new / deleted objects). The **command/undo history is
   a separate, local-only layer** — it is never serialized, merged, or transported. Conflating
   the two makes collaboration and session-resume ill-defined.
3. **Command Pattern (local only)**: edits are executed as reversible commands for natural
   undo/redo within one session; they are not the unit of merge or persistence.
4. **Incremental Persistence**: changes are saved as PDF incremental updates (appended to the
   original file, per PDF spec §7.5.6).
5. **Reference-graph awareness** *(open design point)*: deleting or renumbering an object must
   fix up dangling `Ref`s elsewhere; the data model must account for this, not just a `HashSet`
   of deleted ids.

### Structure

```rust
/// The mergeable, serializable state diff. This is what persists and what
/// collaboration would merge. It contains NO command history.
pub struct DeltaLayer {
    modified_objects: HashMap<ObjectId, PDFObject>, // overrides base
    new_objects: Vec<DeltaObject>,                  // annotations, pages, form values
    deleted_objects: HashSet<ObjectId>,             // marked for removal
    next_obj_num: u32,
}

/// Local-only undo/redo log. Lives in the session/editor, NOT in the delta.
/// Never serialized, never merged across users.
pub struct CommandHistory {
    undo_stack: Vec<Box<dyn Command>>,
    redo_stack: Vec<Box<dyn Command>>,
}

pub trait Command {
    fn execute(&mut self, delta: &mut DeltaLayer) -> Result<(), PDFError>;
    fn undo(&mut self, delta: &mut DeltaLayer) -> Result<(), PDFError>;
    fn redo(&mut self, delta: &mut DeltaLayer) -> Result<(), PDFError>;
}
```

### Object Resolution (in the DocStore)

The `DocStore` is the only place object resolution happens; both viewer and editor go through it:

```rust
impl DocStore {
    fn resolve(&mut self, id: ObjectId) -> PDFResult<PDFObject> {
        // 1. Delta override first (local, may avoid a remote fetch)
        if let Some(obj) = self.delta.get_modified(id) {
            return Ok(obj.clone());
        }
        if self.delta.is_deleted(id) {
            return Err(PDFError::ObjectDeleted(id));
        }
        // 2. Fall back to base (progressive loading, may emit Need::Range)
        self.base.get_object(id)
    }
}
```

### Supported Edit Operations

**Easy**: annotations (highlight, text markup, sticky notes, drawing); page operations
(rotation, deletion, reordering); form filling; document merging; metadata editing.

**Moderate**: text overlay; image replacement; redaction; content-stream modification.

**Hard (significant challenge)**: true text editing — reposition characters, reflow paragraphs.
PDF stores character positions absolutely, not as flowing text; this requires a full text-layout
engine. Many commercial editors avoid it.

### Saving Changes

**Incremental Update (recommended)** — append changes to the end of the original PDF (new xref
table + modified/new objects, original data untouched). Fast, preserves the original file,
spec-compliant; the file grows with each save.

**Full Rewrite (compact)** — rewrite the entire PDF with changes applied (removes deleted
objects, compacts size, single xref table). Clean and compact; slower; loses incremental
history.

### Example: Adding an Annotation

```rust
// User adds a highlight annotation on page 5
let command = AddAnnotationCommand {
    page_number: 5,
    annotation: Annotation::highlight(100, 200, 300, 50, Color::Yellow),
};

// Execute against the store's delta (local undo log records it)
command.execute(store.delta_mut())?;

// The delta now contains:
// 1. New annotation object (e.g., 123 0 obj)
// 2. Modified page object: /Annots [ ... 123 0 R ]

// Rendering page 5 — resolve-then-interpret (decisions B + C):
let resolved: ResolvedPage = store.resolve_page(5)?; // delta → base; may emit Need::Range
interpreter.render(&resolved, &mut device)?;         // pure sync fn over immutable model

// Undo (local session only)
command.undo(store.delta_mut())?;
```

### Advantages Over a Traditional Editor

| Aspect | Traditional Editor | PDF-X Hybrid |
|--------|-------------------|--------------|
| **Startup time** | Slow (parse all) | Fast (progressive) |
| **Memory usage** | High (all objects) | Low (viewed pages + delta) |
| **Save time** | Slow (serialize all) | Fast (append delta) |
| **Undo/redo** | Complex object tracking | Natural command pattern |
| **Progressive loading** | ❌ Not possible | ✅ Maintained |
| **Network efficiency** | ❌ Full download | ✅ Range requests |

### Collaboration Potential

Because the **mergeable state-diff is separated from the local undo-log**, collaboration is
well-defined:

- The object-level `DeltaLayer` (state diff) is serializable and mergeable — the unit
  transported and merged across users.
- The command/undo history is **local-only** and never merged; undo stacks diverge after a
  merge, which is correct.
- Snapshots of `ResolvedPage` against the base double as natural diff points
  (snapshot ≈ diff against base).
- Conflict resolution / CRDT patterns apply to the state diff, not to commands.

## Implementation Roadmap

**Phase 0: Architectural Foundation (current focus)** — `DocStore` as the single `delta → base`
resolution point; pull-based `Need::Range` protocol owned by shells; resolve-then-interpret
(`ResolvedPage`); runtime/threading/rendering backends moved behind traits.

**Phase 1: Viewer Core (ships first)** — content-stream interpretation over `ResolvedPage`;
font/image handling; text extraction; linearized fast first paint.

**Phase 2: Delta Foundation** — `DeltaLayer` populated through the store's resolution path;
basic incremental writer (exists); reference-graph fix-up on delete/renumber.

**Phase 3: Editor Features** — annotation commands; page rotate/delete/reorder; form values;
local command/undo history.

**Phase 4: Advanced Operations** — content-stream modification, image replacement, redaction.

**Phase 5: Collaboration (future)** — delta serialization format, merge algorithms, conflict
resolution over the state diff.

## Current Project Status

The viewer pipeline runs end-to-end: `PDFDocument::open` → xref/parser → `Page::render` →
`ContentStreamEvaluator` → `RenderingContext` → `SkiaDevice` → RGBA pixels, exposed through the
Tauri app.

**Implemented**: core PDF parsing (document structure, xref, objects); encryption/decryption
(RC4, AES-128, AES-256); annotation parsing + outline extraction; content-stream interpretation
+ tiny-skia rendering (`render_page_to_image`); chunked data sources
(file / HTTP / async HTTP / memory); delta scaffolding (`DeltaLayer`, `Command` trait,
`RotatePageCommand`, `PDFWriter::write_incremental_update`).

**Phase 0 progress** (tracked in `phase-0-foundation-spec.md`):

- ✅ **Pull-based protocol** (decision A): `Need`, `ByteSource`/`Loadable`, `MemoryByteSource`,
  `BaseStreamSource`, `drive`, `StepErr` (`DataMissing → Need` boundary). Tested; minimality
  proven. `open_file`'s parse site now drives `XRef::parse` through `drive` (resetting
  `stream_pos` to `startxref` per retry — `parse` is idempotent under `set-if-None` entry
  guards). A `MAX_DRIVE_STEPS` cap guards against runaway loops.
  ⚠️ Known related bug (out of scope, affects the legacy path equally): on a *partially*-loaded
  stream `XRef::parse`/lexer can loop instead of returning `DataMissing`. It doesn't manifest
  in normal use because `open_file` preloads the xref region before parsing.
- ✅ **`DocStore`** (decision B): `delta → base` resolution authority, borrowed over
  `(&mut XRef, &mut DeltaLayer)`. `get_page_dict`, `resolve_page`, and `Page::render`
  (via `render_page_to_image`) and `extract_text` (via `extract_text_from_page`) route through
  it, including nested resources. ✅ `Page::render` and `extract_text` are delta-aware for both
  `/Contents` and nested resources (fonts/xobjects/ext-gstate) — `RenderingContext` resolves
  through a `PageResolver`, so a delta override of any resource object is honored (a Form
  XObject override changes rendered output — tested). Inherited page-tree properties
  (MediaBox/Rotate/Resources from parent nodes) still resolve through the base `XRef`.
- ✅ **Resolve-then-interpret** (decision C): `ResolvedPage` carries an `objects` snapshot
  (every object reachable from `/Resources`, captured by `resolve_page`). `rendering::interpret`
  is a **pure** function — it runs `RenderingContext` over a `SnapshotPageResolver` with no
  live `DocStore`, no `Need`, no fetches beyond the frozen snapshot (tested: a Form XObject
  renders from the snapshot). ⚠️ `render_page_to_image` still uses the live `RenderingContext`
  path; it is not yet rewired to `resolve_page → interpret`.
- 🔄 **Progressive loading** uses the legacy exception form on production paths (see above).
- 🔄 **Core still feature-flags shell concerns**: rendering (`tiny-skia`) and HTTP
  (`reqwest`/`tokio`) are pulled into the core; these must move behind traits/adapters.

> **Note on decision B wording**: "every object lookup goes through `DocStore`" is the
> *target*. Today it is true for page-tree traversal (`get_page_dict`), `resolve_page`, and
> `/Contents` in `Page::render`; nested resources and `extract_text` are not yet routed through.
