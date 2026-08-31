# Phase 0 — Foundation Spec

> Status: **mostly landed (2026-07)**. Target interfaces for decisions A/B/C are live on the
> viewer path (`DocStore`, `ResolvedPage`, pure `interpret` / `interpret_with_scale`, device
> adapter crate). This document remains the design reference; §4 steps 1–5 are implemented at
> the level described in `docs/architecture.md` → *Current Project Status*. Residual Phase-0
> work is progressive-loading production-path cleanup and inherited page-tree seams, not a
> greenfield rewrite.

## 1. Purpose

Turn `pdf-x-core` into the architecture the docs describe:

- **A** — a sync, runtime-free core with a pull-based data protocol (shells own I/O/async/threading)
- **B** — a central `DocStore` that resolves objects `delta → base`, live from day one
- **C** — resolve-then-interpret: the resolver owns progressive loading; the interpreter is a pure
  function over an immutable `ResolvedPage`

### Governing constraint

**The end-to-end viewer must stay green after every step.** No big-bang rewrite. Each step is
independently shippable: `cargo test --features rendering` passes, and
`PDFDocument::render_page_to_image` still produces correct pixels.

## 2. Target interfaces

Types are proposed signatures. Existing crate types (`PDFObject`, `Ref`, `DeltaLayer`, `Device`,
`GraphicsState`, `PDFWriter`) are reused.

### 2.1 The data protocol (decision A)

The core never blocks, never `.await`s, and never loads the whole file. When it needs bytes it does
not have, it returns a **value**, and the **shell** fulfills it and re-drives the operation.

```rust
/// A request from the core for bytes it does not yet have locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Fetch `[pos .. pos+len)` and make it available, then re-drive the operation.
    Range { pos: u64, len: usize },
}

/// Shell-provided byte access. Implementations live OUTSIDE the core:
///   - `FileByteSource`      (native: blocking pread)            [shell]
///   - `HttpByteSource`      (web/native: async fetch driven by shell loop) [shell]
///   - `MemoryByteSource`    (tests, embedded data)              [shell/core-test]
///
/// `get` MUST be cheap and MUST NOT block internally. If the range is not already
/// available, return `Err(Need::Range)`; the caller (shell loop) loads it and retries.
pub trait ByteSource {
    fn get(&mut self, pos: u64, len: usize) -> Result<&[u8], Need>;
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool { self.len() == 0 }
}
```

The shell owns the loop. Because resolution is **idempotent and cached** (re-walking a graph only
re-fetches uncached objects), restart-from-top on `Need::Range` is safe — this is why the
restartability hazard lives only in resolution, never in interpretation.

```rust
// Shell side (native): blocking. Web/wasm: same shape, driven by promises/event loop.
loop {
    match doc_store.resolve_page(0) {
        Ok(page)   => break page,
        Err(Need::Range { pos, len }) => source.load(pos, len)?, // platform I/O
    }
}
```

> **Open question Q1 (§5):** restartable-resolve (above, simpler) vs. an explicit coroutine
> state machine for the resolver. We default to restartable-resolve; revisit if redundant
> re-walking shows up in profiles.

### 2.2 `DocStore` — the single resolution point (decision B)

One type owns the progressively-loaded base (+ caches) **and** the `DeltaLayer`. Every object lookup
goes through it. The delta is empty in v1; the seam is live regardless.

```rust
pub struct DocStore {
    base: BaseCache,            // evolved XRef: progressively-loaded, cached objects
    delta: DeltaLayer,          // edits; empty in v1
    source: Box<dyn ByteSource>,
}

impl DocStore {
    /// The only object-resolution path. delta → base. May emit Need::Range for base misses.
    pub fn resolve(&mut self, id: Ref) -> Result<PDFObject, Need>;

    /// Resolve a whole page to an immutable, I/O-free model. Idempotent + cached.
    pub fn resolve_page(&mut self, index: usize) -> Result<ResolvedPage, Need>;

    /// Local edit surface (commands write here). Never serialized/merged.
    pub fn delta_mut(&mut self) -> &mut DeltaLayer;
}
```

`BaseCache` is the existing `XRef` evolved: same job (resolve objects, cache, load progressively),
now spoken to exclusively through `DocStore`, and delta-aware. The legacy `&mut XRef` threaded
through render/extract calls is replaced by `&mut DocStore`.

### 2.3 `ResolvedPage` — the immutable render model (decision C)

Everything the interpreter needs, fully resolved, no further I/O possible. This is the contract that
makes interpretation a pure function: testable in isolation, parallelizable, WASM-safe.

```rust
/// A fully-resolved, immutable, self-contained page. Send + Sync + 'static so a native
/// shell can render it on another thread while the DocStore keeps being edited.
pub struct ResolvedPage {
    pub media_box: Rect,
    pub crop_box: Rect,
    pub rotation: i32,                 // /Rotate, normalized
    pub ops: Vec<Op>,                  // decoded content-stream operations
    pub resources: ResolvedResources,  // fonts / images / color spaces / xobjects, resolved
}

pub struct ResolvedResources {
    pub fonts:       IndexMap<Name, ResolvedFont>,
    pub images:      IndexMap<Name, DecodedImage>,
    pub color_spaces: IndexMap<Name, ResolvedColorSpace>,
    pub xobjects:    IndexMap<Name, ResolvedXobject>,
    // ... patterns, shadings, ext_gstate, as needed
}

/// Pure interpretation. No I/O, no DocStore, no Need. Drives a Device.
pub fn interpret(page: &ResolvedPage, device: &mut dyn Device);
```

### 2.4 `Device` boundary — rendering backends leave the core (decision A)

`Device` stays as the core's rendering abstraction (it already exists). The **backends** become
separate crates / feature-gated adapters, not hard dependencies of the core:

- `pdf-x-device-tinyskia` (current `SkiaDevice`) — CPU
- future: `pdf-x-device-vello` / `sitro`, `pdf-x-device-canvas2d` (WASM)

Core `Cargo.toml` consequence: `tiny-skia`, `rustybuzz`, `ttf-parser`, `reqwest`, `tokio` move out
of non-optional core deps. The `rendering` feature today bundles a backend *into* the core; Phase 0
splits "interpretation" (core) from "backend" (adapter crate).

### 2.5 Shell vs core boundary (dependency direction)

| Concern | Owner |
|---|---|
| `Need` / `ByteSource` trait, `DocStore`, resolver, `ResolvedPage`, `interpret`, `Device` trait, `DeltaLayer`, `PDFWriter`, parser/xref/crypto/fonts | **core** |
| `ByteSource` impls (file/http/memory), async runtime + drive loop, thread spawning, `Device` backends, UI | **shell** |

## 3. Correctness strategy — "make the implementation right"

Direct answer to *"should we port tests from pdf.js, hayro, or other projects?"*:

**Do not port test *code*; reuse test *corpora, manifests, and expectations* — and use external
tools as oracles.** Different layers need different correctness signals.

### 3.1 The corpus (reuse, don't rewrite)

- **`hayro/hayro-tests/` already vendored the pdf.js corpus.** It ships `manifest_pdfjs.json`,
  `manifest_pdfbox.json`, `manifest_corpus.json`, `blacklist_pdfjs.txt`, and ~340 PDFs. This is the
  canonical, feature-tagged, battle-tested corpus. **Mirror it** into our test corpus rather than
  re-fetching from the (currently uninitialized) `pdf.js` submodule.
- Initialize the `pdf.js` submodule as the reference *implementation* (for differential comparison
  and expected-output extraction), not as a test-code source.
- Add the **Isartor test suite** (PDF Association, ~170 files, one per spec clause with documented
  expected results) for ISO 32000 *spec-compliance* signal.

### 3.2 Per-layer oracles

| Layer | What "right" means | Oracle / technique |
|---|---|---|
| Parser / xref / object streams | Structure correct | **qpdf `--check`** on our output; parse every corpus file without panic |
| Encryption (RC4/AES) | Decrypt matches reference | Decrypt corpus files, compare to unencrypted equivalents; vectors from pdf.js |
| Writer (incremental update, compact, linearization) | Output is spec-valid & editable | **qpdf `--check`** + reopen in ourselves, pdf.js, and a reader; round-trip equality |
| Interpretation (graphics/text) | Pixels/text match reference | **Reference-image diff** at 72 DPI with mutool as the default oracle; documented exceptions use PDFium/hayro/spec behavior when mutool disagrees; **differential text** vs `mutool draw -t` / pdf.js |
| Progressive loading (our differentiator) | Minimal, bounded fetching | **Property tests**: assert exact `Need::Range` set per operation; assert page-1 render fetches only page-1 objects; no whole-file reads |
| `DocStore` delta semantics | Override/delete correctness | **Property tests**: `delta → base` laws; delete cascades; cache invalidation on edit |
| Robustness (corrupt input) | Never panic, graceful degrade | **Fuzz** the parser/lexer with `cargo-fuzz`; curate the corrupt-PDF subset |

### 3.3 Test categories we will own (not portable from anywhere)

- **Property tests** for the novel architecture: progressive-minimality, resolve-purity (interpret
  never returns `Need`), delta-override laws. These exist in no other project.
- **Round-trip tests** for the editor/writer: apply delta → `PDFWriter` → reopen in ourselves +
  qpdf → assert change visible and file valid.
- **Fuzz targets** for the parser, lexer, crypto, and font decoders.

### 3.4 What we explicitly avoid

- Porting pdf.js **JavaScript unit tests** — they assert against pdf.js's API surface, not ours.
  Extract their *expected outputs* (text strings, reference images), not their call sequences.
- Hand-asserting pixels for hundreds of files — use reference-image diffing + a blacklist (the same
  pattern `hayro-tests/blacklist_pdfjs.txt` already uses).

## 4. Migration steps (dependency-ordered, viewer always green)

Each step ends with `cargo test --features rendering` passing and `render_page_to_image` correct.

**Step 1 — Data-access boundary (A skeleton), no removals.**
- Add `Need`, `ByteSource` trait, `MemoryByteSource` (sync, for tests).
- Add a shell-side driver loop helper.
- Acceptance: a unit test resolves a page from an in-memory source via the new loop, *alongside* the
  existing path. Nothing deleted.

**Step 2 — Introduce `DocStore` (B), behavior-preserving.**
- `DocStore` wraps existing `XRef` + empty `DeltaLayer`; `resolve()` delegates to XRef (delta empty).
- Migrate `Page::render` / `extract_text` / `render_page_to_image` to call `DocStore::resolve`
  instead of raw `XRef`. `PDFDocument` exposes `doc_store_mut()`.
- Acceptance: identical render output before/after (byte-equal PNGs on the corpus subset). Delta path
  is live but a no-op.

**Step 3 — Migrate progressive loading to the protocol (A full).**
- Base object access goes through `ByteSource`; missing data returns `Need::Range` instead of the
  inline `PDFError::DataMissing`. The shell owns the retry loop.
- `DataMissing` becomes the legacy form, retained only behind a compat shim until call sites migrate.
- Acceptance: same corpus renders identically; a property test asserts the `Need::Range` set for a
  known page; no whole-file reads.

**Step 4 — Establish `ResolvedPage` (C).**
- Split each page: `resolve_page` produces a `ResolvedPage` (decoded ops + resolved resources);
  `interpret(&ResolvedPage, &mut Device)` is a pure fn with no `DocStore`/`Need` access.
- Acceptance: `interpret` takes only `&ResolvedPage` + `&mut dyn Device`; a unit test interprets a
  hand-built `ResolvedPage` with no source attached.

**Step 5 — Extract rendering/runtime behind traits (A deps).**
- Move `tiny-skia`/`rustybuzz`/`ttf-parser` into a `pdf-x-device-tinyskia` adapter crate; move
  `reqwest`/`tokio` into shell adapters. Core compiles with none of them as hard deps.
- Acceptance: `cargo build -p pdf-x-core --no-default-features` compiles a runtime-free core; a WASM
  build of the core links no native runtime.

## 5. Open design questions (resolve before/within the relevant step)

- **Q1** Restartable-resolve vs. coroutine resolver (§2.1). Default: restartable.
- **Q2** `ResolvedPage` granularity — pre-decode all images/fonts at resolve time (max purity, more
  memory) vs. lazy-but-pinned handles (less memory, needs care to stay I/O-free). Lean pre-decode,
  with a cache budget.
- **Q3** Device-backend packaging — separate workspace crates vs. feature-gated modules in core.
  Lean separate crates (`pdf-x-device-*`) to keep the core dependency-light.
- **Q4** Where the decoded-`Op` representation lives (reuse existing `content_stream::Op`/`Operation`
  vs. a resolve-time normalized form). Audit `content_stream.rs` in Step 4.
- **Q5** Corpus vendoring — mirror `hayro-tests/pdfs` + manifests into our own `tests/corpus/`, or
  path-depend on hayro's. Lean vendoring our own copy for independence; decide in Step 1.

## 6. Out of scope for Phase 0

- Editor commands (annotation/page/form) — Phase 2+, once `DocStore` resolution is real.
- Collaboration / merge — Phase 5.
- New color spaces (Lab/ICC/Indexed), Pattern/Shading fills — rendering completeness, post-Phase-0
  *(note: several of these have since landed on the viewer path as structural dual-lag work;
  they remain out of Phase-0's *architectural* acceptance criteria)*.
- True text reflow — not on the roadmap.

## 7. Viewer-green structural fixes (post Step 5, still Phase-0 hygiene)

These keep `cargo test --features rendering` and the hayro custom visual corpus green. They
are not architecture migrations, but they are load-bearing for the dual-lag chase and must not
be regressed:

| Fix | Where | Acceptance signal |
|---|---|---|
| Viewport scale in `interpret_with_scale` | `rendering/mod.rs`, tinyskia `render_resolved_page` | scale=2/4 content fills canvas, not corner |
| XObject / inline **LZWDecode** | `rendering/context.rs` + `decode::decode_lzw` | `pdftc_900k_0974` dual lag ≈ 0; photo mean ≠ 255 |
| Lazy xref recovery on missing entry | `PDFError::ObjectNotInXRef` → `XRef::fetch` → `recover_by_scan` | `issue53.pdf` opens and paints (mutool blank; ours≈hayro) |
| Tolerant Flate fallback | `core/flate_fallback.rs` ← hayro/pdf.js | corrupt content streams recover non-empty bytes |
| Encryption end-to-end | empty-pw auto-auth; `open_with_password`; `rendering` ⇒ `crypto` | AES-256 empty-pw + password fixtures resolve without panic |
| Decryptor install clears object cache | `XRef::set_decryptor` | auth-after-open does not reuse ciphertext snapshots |
| ASCII85Decode correctness | `decode_ascii85`: `>` is a digit; EOD only on `~`; partial groups pad with `'u'` (84) — hayro/pdf.js | `password_2.pdf` body text present; stream 1788 B not 233 |
| FontFile filter chain | `Font::extract_font_stream` uses `apply_filters` (not Flate-only `decode_stream`) | ASCIIHex/A85 Type1 FontFile no longer "Unsupported filter" |
| Multi `/Contents` extract+render | `Page::extract_text` and `Page::render` concatenate decoded streams before parse (like `ResolvedPage` / pdf.js) | `font_truetype_slow_post_lookup` extract/render survive mid-op splits |
| ObjStm/XRef filter arrays | `fetch_compressed` / XRef stream use `apply_filters` (not Name-only `decode_stream`) | Antenna House books under `~/Books` (Creating Web Animations, Wilke) open |
| Stray `]` / bad numbers | `get_object` recovers Null; invalid number after sign → 0 (Adobe-like) | Foxit-edited arrays no longer abort page_count |

**Visual harness**: `python3 scripts/visual_diff.py [--oracle mutool\|hayro] [pdf…]`  
Oracle default mutool; `EXPECTED_DIVERGENCES` holds documented mutool policy/AA gaps (±0.05).  
Strategy: prefer structural bugs (dark-count collapse, crash, missing filter) over 72 DPI AA.

**Corpus smoke (custom)** (2026-07-10):
- render crash: **265/265** (password fixtures need known user passwords)
- extract / concatenated content-stream parse: **265/265** hard errors
- dark-collapse (ours≪mutool, not policy): none remaining after LZW/ASCII85/multi-Contents

### XFAIL triage (custom, 72 DPI, mutool oracle)

Remaining `EXPECTED_DIVERGENCES` entries fall into classes that are **not** missing
draw/decode bugs under the dual-lag criteria (solid-interior match, scale↑→0 lag,
ours−hay high, ink neighborhood r≤1):

| Class | Examples | Signal |
|---|---|---|
| Color-management / Separation policy | `image_lab*`, `image_cmyk_*`, `color_space_separation_*`, `mask_bc`, `path_rendering_10` | ours−hay ≈ 100%; mutool no-ICC or different Lab/CMYK |
| 72 DPI AA / downsample | `pdftc_900k_0319`, `0584`, `image_inline_2`, `color_space_icc_srgb`, `image_mask` | solid interior 100% or only_oracle ink at r≤2; 2× lag shrinks |
| Three-engine disagreement | `font_type3_widths_with_matrix`, tiling step fixtures | dual lag often **negative** (we closer to mutool than hayro) |
| Documented mutool blanks / oddities | `issue53` (mutool blank), `separation_none` | policy, not missing paint |

Do **not** chase these for dual-lag points without new structural evidence
(dark_ours≈0 while oracle has ink; extract/parse hard fail; wrong glyph topology).
