# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

PDF-X is a Rust port of Mozilla's PDF.js, structured as a **platform-agnostic, composable core library** that is embedded by native, web, and WASM viewer/editor shells.

### Technical Goal (primary, explicitly unbundled)

Earlier drafts fused several goals into one (PDF.js fidelity + progressive loading + immutable viewer + collaborative editor). Those goals constrain each other, so they are unbundled here into one primary objective with explicit positioning:

> **A platform-agnostic, composable PDF core library** that (1) loads **progressively with bounded memory on every source** — local or network, (2) is **WASM-clean** (no baked-in async runtime or threading), and (3) ships as a **viewer first** but is **editor-ready by construction**: the mutation/delta path is wired into object resolution from day one, so adding editing later is a feature, not a refactor.

- **Primary objective**: a fast, faithful, progressive, composable, WASM-clean **viewer core**.
- **Deferred but architecturally preserved**: concrete editor commands and collaboration. The foundation must not preclude them.
- **Editor scope is open**: v1 ships viewer-only, but the delta/resolution seam is the foundation, so editor features slot in without refactoring the viewer.

### Key Design Principles
- Maintain architectural fidelity to PDF.js while implementing idiomatic Rust
- **Composable core**: one sync, runtime-free library at the bottom; platform shells (native/web/wasm) on top provide I/O, async, threading, and UI
- **Progressive by default**: bounded memory on every source (local and network) — large local PDFs are NOT loaded whole
- **Editor-ready, not editor-first**: the `delta → base` object-resolution path is live from day one (delta may be empty), so editing is added by populating the delta

## PDF.js Reference Submodule

The `pdf.js/` directory contains the original PDF.js codebase as a git submodule. This serves as the **primary reference implementation** for:

- Architecture patterns and module structure
- Progressive loading algorithms (see `pdf.js/src/core/`)
- Chunked data handling from various sources
- Incremental parsing logic
- Page tree traversal and lazy rendering

**When implementing new features**: Always reference the corresponding JavaScript implementation in `pdf.js/src/` to understand the algorithm and data flow before writing Rust code.

### PDF.js Source Structure

- `pdf.js/src/core/`: Core parsing layer (stream handling, PDF object parsing, xref tables, fonts, images)
- `pdf.js/src/display/`: Rendering layer (canvas API, page rendering)
- `pdf.js/src/shared/`: Shared utilities between worker and main thread

## Architecture: Composable Core + Shells

The project is a **composable core library** (`pdf-x-core`) at the bottom, used by platform shells (`tauri-app`, future web/wasm shells) on top. The dependency direction is strict: the core never depends on a shell concern.

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

### The Three Foundational Decisions

These three decisions are load-bearing and must be respected by all code. They are what make the goal (progressive + WASM-clean + editor-ready) simultaneously achievable.

#### A. The core is pure synchronous Rust; all I/O and async live in shell adapters

The core must not hard-depend on, or internally drive, an async runtime (tokio/async-std/smol), must not `.await`, and must not spawn or assume OS threads. Concretely: **no baked-in runtime or threading.**

- The core defines a **synchronous, pull-based data protocol**: when it needs bytes it does not have, it returns a *value* — `Need::Range(pos, len)` — not a thrown exception and not a future.
- The **shell owns the load-and-retry loop** and fulfills those requests with platform-appropriate I/O: blocking file read on native, async `fetch` on web, in-memory bytes in tests.

| Concern | Lives in… | Examples |
|---|---|---|
| Async runtime, `.await`, tasks | **Shell** | tokio (native), promises (web), none (tests) |
| Thread spawning, parallelism | **Shell** | parallel page render over snapshots |
| Blocking I/O, network, filesystem | **Shell** | `fs::read`, `fetch` |
| Parsing, resolution, interpretation, the `Need::Range` protocol, the doc store, caching | **Core** | plain sync `fn`, owned data |

What this rule does **not** mean:
- It does **not** forbid multithreading. A native shell may render pages in parallel over immutable `ResolvedPage` snapshots. The core provides thread-safe owned data; the shell orchestrates threads.
- It does **not** forbid `Send + Sync`. The opposite — core public types should be `Send + Sync` (owned data, no `Rc`/`RefCell` in the public API) so a native shell can move them across threads while the same types also run on single-threaded WASM.
- It does **not** forbid caching/mutation. The `DocStore` mutates and caches freely — it just does not spawn threads or drive a runtime to do so.

#### B. A central DocStore is the single resolution point: `delta → base`, always

One `DocStore` owns the progressively-loaded base (+ its caches) **and** the `DeltaLayer`. **Every** object lookup — including the viewer's — goes through `store.resolve(ref)` = `delta.or_else(base)`, from day one, even with an empty delta.

- Caches live **inside** the store and are invalidated by delta mutations. Nothing outside the store caches object lookups, so there is no stale-cache class of bugs.
- The store mutates freely for caching and editing. Because WASM is single-threaded and native shells serialize store access, there is no aliasing problem — no `BaseObjectFetcher` function-pointer dodge, no `&mut XRef` threaded through every render call.
- Bonus: a delta override is *local*, so `delta → base` resolution skips a remote fetch for overridden objects. Editing and progressive loading cooperate rather than fight.

This is what "editor-ready by construction" means: the resolution seam **is** the foundation, so the editor is added by populating the delta with zero viewer refactor.

#### C. Resolve-then-interpret: the progressive contract is owned by the resolver, never seen by the interpreter

Progressive loading is a hard requirement on every source, but the complexity is quarantined to one phase:

- **Resolve phase**: walks a page's object graph on demand, emits `Need::Range` requests, may suspend/resume. It owns **all** the progressive-loading complexity. Resolution is naturally idempotent (re-walking a graph), so it is safe to retry.
- **Interpret phase**: content-stream → `Device` operations, a **pure synchronous function over a fully-resolved, immutable `ResolvedPage`**. It never blocks, never sees missing data, and never needs to be restartable.

This deletes the old "interpretation may fail mid-stream and must cleanly restart" invariant — an undeclared, easy-to-violate hazard. Only the resolver can fail on missing data; interpretation is a pure function: testable, parallelizable, WASM-safe.

`ResolvedPage` is immutable, self-contained, `Send + Sync + 'static`. So native shells render pages in parallel off snapshots, while the single-threaded store keeps being edited. Edits mutate the store; the next render resolves a fresh snapshot. This snapshot model is also the natural basis for future collaboration (a snapshot ≈ a diff against base).

**Fast first paint is preserved**: for a linearized PDF, page 1's object graph sits at the front of the file, so the resolver fetches just those objects and the interpreter renders page 1 before the rest downloads. Linearization is what makes resolve-then-interpret give progressive display — the two belong together.

## Development Commands

Since this is an early-stage Rust project, the standard Cargo workflow applies:

```bash
# Build
cargo build
cargo build --release

# Test
cargo test
cargo test --lib          # Library tests only
cargo test <test_name>    # Single test

# Check without building
cargo check

# Format and lint
cargo fmt
cargo clippy
```

## **CRITICAL RULE: Always Check References Before Implementing**

This is a **NON-NEGOTIABLE** development practice that MUST be followed for ALL features:

**Before implementing ANY feature, algorithm, or conversion, you MUST:**

1. **Search PDF.js source code** (`pdf.js/src/`) for the corresponding implementation
2. **Check hayro libraries** (`../hayro/`) for existing implementations
3. **Copy the exact algorithm** from the reference, adapting it to Rust
4. **Add code comments** with the reference file path

**Why This Matters:**

When implementing features without consulting references, you introduce bugs that could have been avoided. For example:

❌ **WRONG**: Implementing CMYK to RGB conversion using a naive formula
```rust
// Naive, incorrect implementation
let c = 255.0 - chunk[0] as f32;
let r = (c * k / 255.0) as u8;  // Produces wrong colors
```

✅ **CORRECT**: Using PDF.js's proven polynomial coefficients
```rust
// Reference: pdf.js/src/core/colorspace.js - DeviceCmykCS.#toRgb
// Uses polynomial coefficients derived from US Web Coated (SWOP) colorspace
let r = 255.0 +
    c * (-4.387332384609988 * c + 54.48615194189176 * m + ...) +
    m * (1.7149763477362134 * m - 5.6096736904047315 * y + ...) +
    y * (-2.5217340131683033 * y - 21.248923337353073 * k + ...) +
    k * (-21.86122147463605 * k - 189.48180835922747);
```

**Reference Checking Workflow:**

```bash
# 1. Search PDF.js for the feature
grep -r "CMYK\|cmyk" ../pdf.js/src/core/

# 2. Find the specific implementation
#    - ../pdf.js/src/core/colorspace.js for color conversions
#    - ../pdf.js/src/core/image.js for image handling
#    - ../pdf.js/src/core/evaluator.js for content stream operators

# 3. Check hayro libraries
ls ../hayro/hayro-*/src/
```

**What to Reference:**

| Feature | PDF.js Location | Hayro Library |
|---------|-----------------|---------------|
| Color spaces (CMYK, Lab, etc.) | `core/colorspace.js` | N/A |
| Image decoding | `core/image.js`, `core/jpg.js` | N/A |
| Content stream operators | `core/evaluator.js` | N/A |
| Font handling | `core/font.js`, `core/cff_parser.js` | `hayro-font/` |
| JPEG2000 decoding | N/A | `hayro-jpeg2000/` |
| JBIG2 decoding | N/A | `hayro-jbig2/` |

**Consequences of Not Checking References:**

- **Incorrect implementations** that produce wrong results
- **Wasted time** debugging issues already solved in PDF.js
- **Incompatibility** with PDF files that work in PDF.js
- **Technical debt** that must be fixed later

**This rule applies to:**
- All color space conversions (CMYK, Lab, ICC-based, etc.)
- All image format handling (JPEG, PNG, JPEG2000, JBIG2, etc.)
- All content stream operators (text, paths, images, etc.)
- All font parsing (Type1, TrueType, CFF, etc.)
- All encryption algorithms (RC4, AES-128, AES-256, etc.)
- All compression algorithms (Flate, ASCIIHex, ASCII85, etc.)

**Remember**: PDF.js has 10+ years of bug fixes and real-world testing. Their implementations are proven to work with thousands of PDF files. Always leverage this knowledge.

## Progressive Loading Implementation Notes

Progressive loading is the **core differentiator** of this project. Key concepts from PDF.js to replicate:

1. **Chunked Loading**: Load PDF data in chunks (typically 64KB), regardless of source
2. **Source Abstraction**: Unified interface for network, filesystem, and other sources
3. **Range Requests**: Network loader uses HTTP Range headers; filesystem loader uses seek + read
4. **Incremental Parsing**: Parse PDF objects as chunks arrive, don't wait for complete file
5. **Lazy Page Loading**: Parse page content streams only when requested, not during initial document load
6. **Linearized PDF Support**: Recognize and optimize for linearized PDFs (fast web view)

The Data Source Layer must provide a trait/interface that all loaders implement, enabling the parser to work identically whether loading from network, disk, or memory.

Reference `pdf.js/src/core/stream.js` and `pdf.js/src/core/chunked_stream.js` for implementation patterns.

### **CRITICAL RULE: Progressive Loading via a Pull-Based Protocol**

This is a **NON-NEGOTIABLE** architectural principle that MUST be followed in all code:

**Load as little PDF data as possible. The core never blocks for data and never loads the whole file. When data is missing, the core returns a `Need::Range(pos, len)` value; the shell fulfills it and re-drives the operation.**

This is the value-returning refinement of PDF.js's exception-driven loading. The earlier formulation — a `PDFError::DataMissing` exception permeating every layer, with `&mut XRef` threaded through every call — violates the goals of extensibility, performance, and correctness (it is not WASM-clean, not parallelizable, and rests on an undeclared "every layer must be restartable" invariant). The **goal** (progressive, bounded memory, on every source) is unchanged; only the **mechanism** is replaced, per the resolve-then-interpret decision (C above).

```rust
// ✅ CORRECT — core returns a data *request*; shell owns the load+retry loop
loop {
    match resolver.step() {
        Ok(resolved) => break resolved,                 // fully-resolved, in-memory model
        Need::Range(pos, len) => provider.fill(pos, len)?, // shell: blocking read / fetch / Vec
    }
}
// Interpretation then runs as a pure sync fn over `resolved` — never sees missing data.
```

```rust
// ❌ WRONG — loading all data upfront (NEVER DO THIS)
let all_data = stream.get_all_bytes()?; // Violates progressive loading
parser.parse_xref(&all_data)?;
```

```rust
// ❌ WRONG — interpretation that can fail mid-stream on missing data
// Interpretation must run over a fully-resolved page (decision C). Only the resolver may
// emit Need::Range; the interpreter must be a pure function that never blocks.
```

**Implementation Requirements:**

1. **Never preload data**: no `read_all()`, `load_complete()`, or whole-file patterns
2. **Express missing data as a value**: core operations return `Need::Range { position, length }` (or the legacy `PDFError::DataMissing` in paths not yet migrated), not a future and not a thrown exception that callers must `.await`
3. **Quarantine the contract to the resolver**: only the resolve phase may request data; the interpret phase is pure and never blocks
4. **Shell owns the loop**: retry/load loops live in shell adapters, never in the core
5. **Minimal chunk requests**: request only the specific byte range needed
6. **No buffering layers that hide progressivity**: caches belong inside the `DocStore`, not scattered across layers

**When Evaluating External Crates:**

Before using any external Rust crate for PDF functionality, you must evaluate whether it supports progressive loading:

1. ✅ **Acceptable: Isolated operation crates**
   - Stream decoders (e.g., `flate2` for FlateDecode)
   - Image decoders (e.g., `png`, `jpeg-decoder`)
   - Compression algorithms that work on already-loaded data
   - Utility functions for parsing small data structures
   - These are acceptable because they operate on data you've already explicitly loaded

2. ❌ **Reject: Full-file processing crates**
   - Any crate requiring `Arc<[u8]>`, `Vec<u8>`, or similar for the entire PDF
   - Crates with `load()`, `open(path)`, `from_file()` APIs that load complete files
   - Crates that use `std::fs::read()` or equivalent in their examples
   - Crates with internal buffering that loads large file portions without your control

3. ✅ **Acceptable: Learning from source code**
   - You CAN read external crate source code to understand algorithms
   - You CAN copy/adapt implementation patterns and utility functions
   - You CAN use their approach to solving specific problems
   - You CANNOT use them as dependencies if they violate progressive loading

**Examples:**

```rust
// ✅ GOOD - Using flate2 for stream decompression
// (You already loaded this stream data progressively)
use flate2::read::ZlibDecoder;
let decompressed = decode_flate(&already_loaded_stream_data)?;

// ❌ BAD - Using hypothetical full-file crate
use some_pdf_crate::Pdf;
let data = std::fs::read(path)?;  // Loads entire file
let pdf = Pdf::new(Arc::new(data))?;  // Requires full data

// ✅ GOOD - Learning from external crate source
// Copy algorithm pattern, adapt to progressive loading
fn parse_xref_field(stream: &mut ChunkedStream, offset: usize, width: u8)
    -> Result<u32, PDFError>
{
    // This pattern learned from external crate source
    // But adapted to use YOUR chunked stream with DataMissing errors
    let bytes = stream.get_bytes(offset, width as usize)?;  // Can throw DataMissing
    Ok(match width {
        0 => 0,
        1 => bytes[0] as u32,
        2 => u16::from_be_bytes([bytes[0], bytes[1]]) as u32,
        3 => u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]),
        4 => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        _ => return Err(PDFError::InvalidData("Invalid field width".into())),
    })
}
```

**Progressive Loading is Non-Negotiable:**

This rule exists to enable:
- **Fast first-page display**: Start rendering before full PDF download completes
- **Low memory usage**: Only load needed portions, critical for large PDFs
- **Network efficiency**: HTTP range requests only fetch required data
- **Responsive UI**: No blocking on full file load

If a feature cannot be implemented with progressive loading, it should be redesigned or deferred. **External crates are valuable for learning algorithms and patterns, but the core parsing infrastructure must support progressive loading.**

## Delta Layer Architecture

The **Delta Layer** enables editing while keeping the base PDF untouched, and is **wired into object resolution from day one** (decision B). v1 ships with an empty delta; editor features are added by populating it.

### Core Design Principles

1. **Single resolution point**: object lookup always goes through `DocStore::resolve(ref)` = `delta → base`. The delta is checked first (cheap, local) and may even *avoid* a remote fetch for overridden objects.
2. **State-diff ≠ undo-log**: the `DeltaLayer` stores the **mergeable, serializable object-level state diff** (modified / new / deleted objects). The **command/undo history is a separate, local-only layer** — it is never serialized, merged, or transported. Conflating the two (as an earlier draft did) makes collaboration and session-resume ill-defined.
3. **Command Pattern (local only)**: edits are executed as reversible commands for natural undo/redo within one session; they are not the unit of merge or persistence.
4. **Incremental Persistence**: changes are saved as PDF incremental updates (appended to the original file, per PDF spec §7.5.6).
5. **Reference-graph awareness** *(open design point)*: deleting or renumbering an object must fix up dangling `Ref`s elsewhere; the data model must account for this, not just a `HashSet` of deleted ids.

### Delta Layer Structure

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

The delta layer enables these common PDF operations:

**Easy (Low-Hanging Fruit)**:
- ✅ **Annotations**: Highlight, text markup, sticky notes, freehand drawing
- ✅ **Page operations**: Rotation, deletion, reordering (small delta)
- ✅ **Form filling**: Store field values, checkboxes, signatures
- ✅ **Document merging**: Copy page objects + update catalog
- ✅ **Metadata editing**: Modify document info, properties

**Moderate Complexity**:
- ⚠️ **Text overlay**: Add new text on top of existing content (easy)
- ⚠️ **Image replacement**: Swap content object references
- ⚠️ **Redaction**: Overlay black rectangles + delete underlying text objects
- ⚠️ **Content stream modification**: Edit page graphics operations

**Hard (Significant Challenge)**:
- ❌ **True text editing**: Reposition characters, reflow paragraphs
  - PDF stores character positions absolutely, not as flowing text
  - Requires implementing a text layout engine
  - Many commercial PDF editors avoid this or use crude workarounds

### Saving Changes

The delta layer supports two save strategies:

**1. Incremental Update (Recommended)**
```rust
fn save_incremental() -> Result<(), PDFError> {
    // Append changes to end of original PDF
    // - New xref table
    // - Modified/new objects
    // - Original data untouched
    pdf_writer.append_incremental_update(&delta_layer)?;
}
```
- ✅ Fast (only writes delta)
- ✅ Preserves original file
- ✅ PDF specification compliant
- ❌ File grows with each save

**2. Full Rewrite (Compact)**
```rust
fn save_compact() -> Result<(), PDFError> {
    // Rewrite entire PDF with changes applied
    // - Removes deleted objects
    // - Compacts file size
    // - Single xref table
    pdf_writer.rewrite_pdf(&viewer, &delta_layer)?;
}
```
- ✅ Produces clean, compact file
- ❌ Slower (rewrites everything)
- ❌ Loses incremental history

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

### Advantages Over Traditional Editor

| Aspect | Traditional Editor | PDF-X Hybrid |
|--------|-------------------|--------------|
| **Startup time** | Slow (parse all) | Fast (progressive) |
| **Memory usage** | High (all objects) | Low (viewed pages + delta) |
| **Save time** | Slow (serialize all) | Fast (append delta) |
| **Undo/redo** | Complex object tracking | Natural command pattern |
| **Progressive loading** | ❌ Not possible | ✅ Maintained |
| **Network efficiency** | ❌ Full download | ✅ Range requests |

### Collaboration Potential

Because the **mergeable state-diff is separated from the local undo-log**, collaboration is well-defined:

- The object-level `DeltaLayer` (state diff) is serializable and mergeable — this is the unit transported and merged across users.
- The command/undo history is **local-only** and never merged; undo stacks are expected to diverge after a merge, which is correct.
- Snapshots of `ResolvedPage` against the base double as natural diff points (snapshot ≈ diff against base).
- Conflict resolution / CRDT patterns apply to the state diff, not to commands.

### Implementation Roadmap

**Phase 0: Architectural Foundation (current focus)**
- Introduce `DocStore` as the single `delta → base` resolution point
- Migrate progressive loading to the pull-based `Need::Range` protocol owned by shells
- Establish resolve-then-interpret: `ResolvedPage` as the immutable, `Send + Sync` render model
- Move runtime/threading/rendering-backend concerns out of the core behind traits

**Phase 1: Viewer Core (ships first)**
- Complete content-stream interpretation over `ResolvedPage`
- Font and image handling; text extraction
- Linearized-PDF fast first paint through the resolver

**Phase 2: Delta Foundation (already scaffolded; wire it in)**
- `DeltaLayer` state diff populated through the store's resolution path
- Basic incremental PDF writer (exists)
- Reference-graph fix-up on delete/renumber

**Phase 3: Editor Features (populate the delta)**
- Annotation commands; page rotate/delete/reorder; form field values
- Local command/undo history

**Phase 4: Advanced Operations**
- Content-stream modification, image replacement, redaction

**Phase 5: Collaboration** (Future)
- Delta serialization format, merge algorithms, conflict resolution over the state diff



## Current Project Status

The viewer pipeline (layers now expressed as core phases) is implemented end-to-end: `PDFDocument::open` → xref/parser → `Page::render` → `ContentStreamEvaluator` → `RenderingContext` → `SkiaDevice` → RGBA pixels, exposed through the Tauri app.

**Implemented**:
- ✅ Core PDF parsing (document structure, xref, objects)
- ✅ Encryption/decryption (RC4, AES-128, AES-256)
- ✅ Annotation parsing, outline extraction with page indices
- ✅ Content-stream interpretation + tiny-skia rendering (`render_page_to_image`)
- ✅ Chunked data sources (file / HTTP / async HTTP / memory) — the loader machinery exists
- ✅ Delta scaffolding (`DeltaLayer`, `Command` trait, `RotatePageCommand`, `PDFWriter::write_incremental_update`)

**Phase 0 progress** (foundation; tracked in `docs/phase-0-foundation-spec.md`):
- ✅ **Pull-based protocol** (decision A): `Need`, `ByteSource`/`Loadable`, `MemoryByteSource`,
  `BaseStreamSource`, `drive`, `StepErr` (`DataMissing → Need` boundary). Tested; minimality proven.
  ⚠️ `drive` has **no production caller yet** — `open_file`'s parse site still uses
  `retry_on_data_missing!` because `XRef::parse` is not restartable (the migration hung; see
  the note in `document.rs`). A `MAX_DRIVE_STEPS` cap guards against the hang class.
- ✅ **`DocStore`** (decision B): `delta → base` resolution authority, borrowed over
  `(&mut XRef, &mut DeltaLayer)`. `get_page_dict`, `resolve_page`, and `Page::render`
  (via `render_page_to_image`) route through it.
  ⚠️ **The gate is partially enforced**: `Page::render` is delta-aware for `/Contents`
  (a delta override changes rendered output — tested), but nested resource fetches
  (fonts/xobjects/ext-gstate) still go through the base `XRef`, so deep `/Resources`
  overrides aren't yet honored. `extract_text` still bypasses `DocStore` entirely.
- 🔄 **Resolve-then-interpret** (decision C): `ResolvedPage` type + `DocStore::resolve_page`
  (delta-aware, tested incl. a delta-override seam proof) + a pure-signature
  `rendering::interpret` stub. ⚠️ The render pipeline is **not** rewired to consume
  `ResolvedPage` yet (`RenderingContext` still resolves lazily); this is the Step 4a rewire.
- 🔄 **Progressive loading uses the legacy exception form** on production paths (see above).
- 🔄 **Core still feature-flags shell concerns**: rendering (`tiny-skia`) and HTTP
  (`reqwest`/`tokio`) are pulled into the core; these must move behind traits/adapters.

> **Note on decision B wording**: "every object lookup goes through `DocStore`" is the
> *target*. Today it is true only for page-tree traversal (`get_page_dict`) and
> `resolve_page`; the render/extract hot paths bypass it until the Step 4a rewire.

**Priorities** (viewer ships first, editor-ready by construction):

1. **Phase 0 — Foundation**: enforce `DocStore` on the render path (Step 4a rewire),
   restartable `parse` + migrate `open_file` to `drive`, move runtime/rendering behind traits
2. **Phase 1 — Viewer Core**: complete interpretation over `ResolvedPage`, fonts/images, text extraction, linearized fast first paint
3. **Phase 2+ — Editor**: populate the delta (annotations, page ops, forms) once the resolution seam is real

**Development Guidelines**:
- Reference PDF.js implementations before writing new components
- Follow Rust idioms (no direct translation of JavaScript patterns)
- Keep the core sync, runtime-free, and `Send + Sync`; put I/O/async/threading/rendering backends in shells and adapters
- Resolution always goes through the `DocStore` (`delta → base`) — never read objects directly from the base xref
- Interpretation runs only over a fully-resolved `ResolvedPage`; only the resolver may emit `Need::Range`
- Test progressive loading with network resources (not just local files) and with a sync in-memory driver
- Incremental updates are the default save mechanism

## Key PDF Concepts

- **Cross-reference (xref) table**: Index mapping object numbers to byte offsets in file
- **Incremental updates**: PDFs can have multiple xref sections (append-only updates)
  - **Critical for delta layer**: Changes can be appended to original file without rewriting it
  - New xref table points to modified objects and original unchanged objects
  - Enables fast saves and preserves original document data
- **Page tree**: Hierarchical structure storing pages (not always flat array)
- **Content streams**: Compressed instruction streams defining page graphics
- **Linearized PDF**: Reorganized PDF with hint tables for progressive display
- **Object graph**: PDF objects can reference each other (direct and indirect references)
  - Delta layer must track and update these references when objects are modified

## Reference Documentation

- PDF 1.7 Specification (ISO 32000-1): Official PDF format specification
  - **Section 7.5.6**: Incremental Updates (crucial for delta layer persistence)
- PDF.js source code in `pdf.js/` submodule: Working reference implementation
- [pdf-rs](https://github.com/pdf-rs/pdf): Existing Rust PDF library for patterns

