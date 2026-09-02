# CLAUDE.md

Operating guidance for Claude Code working in this repository. Detailed design lives in
[`docs/architecture.md`](docs/architecture.md); the current foundation migration in
[`docs/phase-0-foundation-spec.md`](docs/phase-0-foundation-spec.md).

## Project Overview

PDF-X is a Rust port of Mozilla's PDF.js: a **platform-agnostic, composable core library**
embedded by native, web, and WASM viewer/editor shells.

> **Technical goal** (unbundled): a composable PDF core that (1) loads **progressively with
> bounded memory on every source**, (2) is **WASM-clean** (no baked-in async runtime or
> threading), and (3) ships **viewer-first but editor-ready by construction** — the
> `delta → base` resolution path is live from day one. Editor commands and collaboration are
> deferred but must not be precluded. v1 is viewer-only.

## Non-Negotiable Rules

### 1. Always check the references before implementing

Before writing **any** feature/algorithm/conversion, find the corresponding implementation in
**pdf.js** (`pdf.js/src/`) or **hayro** (`../hayro/`), copy the exact algorithm, adapt to Rust,
and cite the source file in a comment. PDF.js has 10+ years of bug fixes; reinventing its
algorithms introduces avoidable bugs (e.g. CMYK→RGB must use pdf.js's `colorspace.js`
polynomial coefficients, not a naive formula).

| Feature | PDF.js | Hayro |
|---------|--------|-------|
| Color spaces (CMYK, Lab, ICC) | `core/colorspace.js` | — |
| Image decoding | `core/image.js`, `core/jpg.js` | — |
| Content-stream operators | `core/evaluator.js` | — |
| Font parsing (Type1/TrueType/CFF) | `core/font.js`, `core/cff_parser.js` | `hayro-font/` |
| JPEG2000 / JBIG2 | — | `hayro-jpeg2000/` / `hayro-jbig2/` |

Applies to: all color conversions, image formats, content-stream operators, font parsing,
encryption (RC4/AES), and compression (Flate/ASCIIHex/ASCII85).

### 2. Composable core: sync and runtime-free; shells own I/O

The **core** (`pdf-x-core`) never `.await`s, never spawns/assumes threads, and depends on no
async runtime. Missing data is returned as a **value** (`Need::Range(pos, len)`), and the
**shell** owns the load-and-retry loop (`drive`).

| Lives in **Shell** | Lives in **Core** |
|---|---|
| async runtime, `.await`, tasks, thread spawning, blocking I/O (`fs::read`, `fetch`) | parsing, resolution, interpretation, `Need::Range`, `DocStore`, caching |

Multithreading is allowed — but orchestrated by the shell, over immutable `Send + Sync` core
data. Device backends (tiny-skia, vello, canvas-2d) are adapter crates, not core hard-deps.

### 3. Progressive loading: never load the whole file

Load as little as possible. **No** `read_all()`/`load_complete()`/whole-file patterns. Only the
**resolver** may emit `Need::Range`; **interpretation** runs over a fully-resolved,
immutable `ResolvedPage` and never blocks. External crates that require full-file buffers are
rejected; isolated decoders (`flate2`, `png`) are fine.

### 4. Resolve through DocStore; interpret over ResolvedPage

All object resolution goes through `DocStore::resolve` (`delta → base`) — never read objects
directly from the base `XRef`. Interpretation is a pure function over a `ResolvedPage` (no
`DocStore`, no `Need`). This is the resolve-then-interpret boundary (decision C); it makes
interpretation restartable-safe, parallelizable, and WASM-clean.

> **Current state** (Phase 0 complete): `Page::render` and `extract_text` route through a
> `DocStore`; `/Contents` and nested resources are delta-aware (tested). `render_page_to_image`
> resolves to a `ResolvedPage` snapshot then calls the **pure** `interpret(&ResolvedPage,
> &mut Device)` — no `DocStore`, no fetches (decision C, live end-to-end). `drive` owns the
> `open_file` progressive-load loop (decision A).

## Development Commands

```bash
cargo build                       # build
cargo test --lib                  # library tests
cargo test <name>                 # single test
cargo test --test rendering_tests --features pdf-x-core/rendering   # viewer/render tests
cargo check                       # type-check without building
cargo fmt && cargo clippy         # format + lint
```

Rendering is feature-gated; pass `--features pdf-x-core/rendering` (or `--features rendering`
in `pdf-x-core`) for render/viewer tests.

## Working Conventions

- Keep the viewer green: every change passes `cargo test --lib` and the rendering tests, with
  byte-identical render output for behavior-preserving refactors.
- Follow Rust idioms; don't translate JavaScript patterns directly.
- `expect`/`assert` messages describe the **violated expectation**, never the happy path.
- Incremental updates (PDF spec §7.5.6) are the default save mechanism.

## Key PDF Concepts

- **xref table**: object-number → byte-offset index.
- **Incremental updates**: append-only xref sections — changes can be appended to the original
  file without rewriting it (the basis of delta persistence).
- **Page tree**: hierarchical (not always a flat array).
- **Content streams**: compressed instruction streams defining page graphics.
- **Linearized PDF**: reorganized with hint tables for fast progressive display.
- **Object graph**: objects reference each other indirectly; the delta must track/fix-up these
  refs when objects are modified or deleted.

## Standard Font Selection Strategies

See [`docs/standard-font-strategies.md`](docs/standard-font-strategies.md) for
hayro / pdf.js / pdfium / xpdf standard-font substitution strategies.
## References

- PDF 1.7 spec (ISO 32000-1) — §7.5.6 (Incremental Updates) is crucial for the delta layer.
- `pdf.js/` submodule — primary reference implementation.
- `../hayro/` — font/image-decode libraries.
- `/home/gaopeng/Enter/pdfium-lib/build/wasm32/pdfium/` — pdfium source (WASM build checkout).
- `docs/architecture.md` — full architecture & delta-layer design.
- `docs/phase-0-foundation-spec.md` — current foundation migration steps & acceptance criteria.
