# Font/Text Rendering Root Cause

This note records the current source-based conclusion for Type0/CID and vertical
text rendering. It is based on local source only:

- `hayro/`
- `pdf.js/`
- `../pdfium-lib/build/wasm32/pdfium`
- current `pdf-x-*` implementation

## Reference Implementations

### hayro

`hayro/hayro-interpret/src/font/cid.rs` keeps the Type0 pipeline split into
separate concepts:

- `/Encoding` CMap reads PDF text bytes into character codes.
- CMap lookup maps character code to CID.
- `/CIDToGIDMap` maps CID to glyph id for TrueType fonts; absent map defaults to
  identity.
- `/W`, `/DW`, `/W2`, `/DW2` provide horizontal and vertical advances.
- Vertical origin displacement is computed separately from glyph selection.

Relevant local references:

- `Type0Font::new`: `cid.rs:33-75`
- glyph mapping: `cid.rs:78-102`
- vertical advance: `cid.rs:111-119`
- vertical origin: `cid.rs:134-143`
- CIDToGIDMap parsing: `cid.rs:220-249`
- Identity-V code reading: `cmap.rs:34-55`, `cmap.rs:122-135`
- text loop: `interpret/text.rs:27-40`
- text advance application: `interpret/state.rs:145-175`

### pdf.js

pdf.js follows the same separation:

- `CMap.readCharCode` reads variable-length PDF character codes.
- `Font.charsToGlyphs` converts bytes to glyph records by charcode.
- `_charToGlyph` uses CMap lookup for width/vmetrics code, while ToUnicode is
  only one glyph property.
- Canvas rendering uses glyph width/vmetrics to position vertical text.

Relevant local references:

- CMap byte reading: `pdf.js/src/core/cmap.js:320-335`
- font vertical properties: `pdf.js/src/core/fonts.js:1124-1129`
- glyph creation: `pdf.js/src/core/fonts.js:3502-3597`
- composite byte loop: `pdf.js/src/core/fonts.js:3602-3625`
- CIDToGIDMap stream extraction: `pdf.js/src/core/evaluator.js:3601-3605`
- W/W2/DW/DW2 extraction: `pdf.js/src/core/evaluator.js:4071-4154`
- vertical canvas positioning: `pdf.js/src/display/canvas.js:2815-2823`,
  `pdf.js/src/display/canvas.js:2894-2904`

### PDFium

PDFium is more native-font specific:

- It maps charcode to CID via `CPDF_CIDFont::CIDFromCharCode`.
- It detects vertical writing from the CMap.
- For embedded CID TrueType, it selects a FreeType charmap based on the CID
  coding before glyph lookup.
- It parses `/CIDToGIDMap`, `/W2`, and `/DW2`.
- It computes vertical width and vertical origin separately.

Relevant local references:

- FreeType CID charmap selection: `cpdf_cidfont.cpp:197-220`
- charcode to CID and vertical mode: `cpdf_cidfont.cpp:301-307`
- embedded font CID charmap setup: `cpdf_cidfont.cpp:470-475`
- CIDToGIDMap and vertical metrics loading: `cpdf_cidfont.cpp:484-509`
- vertical width/origin: `cpdf_cidfont.cpp:608-644`
- glyph lookup and vertical GSUB: `cpdf_cidfont.cpp:646-725`
- text object vertical origin/advance: `cpdf_textobject.cpp:48-65`,
  `cpdf_textobject.cpp:231-239`, `cpdf_textobject.cpp:259-305`

## Current pdf-x Problem

Our current renderer still has a Type0 shortcut that is not equivalent to the
three references:

- `RenderingContext::show_text` and `show_spaced_text` call
  `map_text_bytes_for_device` before drawing.
- `map_text_bytes_for_device` may rewrite original PDF text bytes through
  `/ToUnicode`.
- The device then receives bytes that no longer reliably represent the original
  charcode/CID stream.

Relevant local references:

- draw path calls: `pdf-x-core/src/rendering/context.rs:794-806`,
  `pdf-x-core/src/rendering/context.rs:908-920`
- ToUnicode byte remap: `pdf-x-core/src/rendering/context.rs:1195-1214`
- current Type0 setup and experimental CID hooks:
  `pdf-x-core/src/rendering/context.rs:1323-1435`
- tiny-skia raw CID path: `pdf-x-device-tinyskia/src/skia_device.rs:1489-1535`
- tiny-skia vertical raw CID path:
  `pdf-x-device-tinyskia/src/skia_device.rs:2259-2370`

This conflates at least four mappings that must remain separate:

1. PDF text bytes to character code, using font CMap.
2. Character code to CID, using the same CMap.
3. CID to glyph id, using CIDToGIDMap, CFF CID lookup, or a native font charmap.
4. Character code/CID to Unicode, using ToUnicode for text semantics, not as the
   primary rendering key.

## Confirmed Bad Experiment

Unconditionally enabling raw two-byte CID rendering for `/Identity-V` made
`font_vertical.pdf` worse in local comparison. That is expected from the source
analysis: without the complete charcode/CID/gid/metrics model, treating every
Identity-V byte pair as a direct glyph id is not a valid replacement for
PDFium's FreeType CID charmap path or pdf.js/hayro's structured glyph mapping.

## Implementation Direction

The next fix should not tune percentages. It should introduce an explicit Type0
glyph-run model.

Required model:

```text
Type0Glyph {
  code,        // PDF character code read from CMap
  cid,         // CMap lookup result
  gid,         // CIDToGIDMap / CFF CID / font cmap result where available
  unicode,     // ToUnicode result, for text semantics/fallback only
  advance,     // W/DW or W2/DW2
  origin,      // vertical origin displacement
}
```

Short-term steps:

1. Add a Type0 font classifier/trace for failing PDFs: Encoding, vertical flag,
   descendant subtype, CIDToGIDMap kind, W/W2/DW/DW2 availability, current render
   path.
2. Parse Identity-H/V CMap code reading as a first supported Type0 glyph-run
   case.
3. Parse and retain W/W2/DW/DW2 and CIDToGIDMap in one Type0 metadata object.
4. Route Type0 rendering through a glyph-run path instead of rewriting bytes
   through ToUnicode.
5. Keep the current ToUnicode remap as a fallback only for cases not yet covered
   by the Type0 glyph-run path.

Guardrails found during implementation:

- Do not enable the glyph-run path for horizontal CIDFontType2 fonts with a
  `/CIDToGIDMap` stream until the font-char mapping layer is complete.
  `basicapi.pdf` regressed when this was enabled directly.
- Do not enable the new glyph-run path for CIDFontType0/CFF until the CFF CID
  mapping is validated against hayro/PDFium. `font_cid_1.pdf` regressed when it
  was routed through the new path.
- TrueType glyph ids from `/CIDToGIDMap` must be bounds-checked against the font
  glyph count, and glyph id 0 should be treated as `.notdef` rather than a
  drawable outline. This follows the same intent as pdf.js's `hasGlyph` guard.

Native precision note:

PDFium relies on FreeType for CID charmap selection and glyph rasterization. A
tiny-skia outline-fill backend can improve semantic positioning, but exact
native pixel match for CID TrueType and glyph antialiasing will likely require a
FreeType-backed native text renderer. Core can remain WASM-clean by exposing the
glyph-run metadata and keeping font resolution deterministic.
