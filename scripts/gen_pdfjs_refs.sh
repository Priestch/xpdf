#!/usr/bin/env bash
# Generate pdf.js operator list reference snapshots for cross-comparison.
# Usage: bash scripts/gen_pdfjs_refs.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SNAP="$ROOT/pdf-x-core/tests/snapshots"
FIX="$ROOT/tests/fixtures/pdfs"
HAYRO="$ROOT/hayro/hayro-tests/pdfs/custom"
NODE="$ROOT/scripts/pdfjs_oplist.mjs"

mkdir -p "$SNAP"

dump() {
  local pdf="$1" name="$2" page="${3:-0}"
  local out="$SNAP/pdfjs_${name}.txt"
  echo "  $name <- $pdf (page $page)"
  node "$NODE" "$pdf" "$page" > "$out"
}

echo "Generating pdf.js reference snapshots..."

# Original fixtures (from tests/fixtures/pdfs/)
dump "$FIX/basicapi.pdf" basicapi 0
dump "$FIX/tracemonkey.pdf" tracemonkey 0
dump "$FIX/simpletype3font.pdf" type3_simple 0

# Hayro custom fixtures
dump "$HAYRO/font_type3_filled_glyphs.pdf" type3_filled 0
dump "$HAYRO/font_cid_1.pdf" cid_1 0
dump "$HAYRO/pattern_shading_type0_7.pdf" shading_axial 0
dump "$HAYRO/pattern_shading_type2_1.pdf" shading_radial 0
dump "$HAYRO/pattern_tiling_simple.pdf" tiling_simple 0
dump "$HAYRO/fireworks.pdf" fireworks 0
dump "$HAYRO/mask_alpha.pdf" mask_alpha 0
dump "$HAYRO/image_cmyk_jpg.pdf" cmyk_jpg 0
dump "$HAYRO/image_inline_2.pdf" inline_img 0

echo "Done. $(ls "$SNAP"/pdfjs_*.txt | wc -l) reference files generated."
