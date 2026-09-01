// Dump pdf.js operator list for cross-comparison with pdf-x RecordingDevice snapshots.
// Usage: node scripts/pdfjs_oplist.mjs <pdf-path> [page-index]

import fs from "fs";
import path from "path";
import { fileURLToPath } from "url";
import { getDocument, OPS } from "pdfjs-dist/legacy/build/pdf.mjs";

// Build reverse OPS map: number -> name
const FN_NAME = new Map();
for (const [name, val] of Object.entries(OPS)) {
  FN_NAME.set(val, name);
}

const ROOT = path.dirname(fileURLToPath(import.meta.url));
const CMAP_DIR = path.resolve(ROOT, "../pdf.js/external/bcmaps");
const FONT_DIR = path.resolve(ROOT, "../pdf.js/external/standard_fonts");

function fmt(v) {
  if (v === null || v === undefined) return "null";
  if (typeof v === "number") return v.toFixed(6);
  if (typeof v === "boolean") return v ? "true" : "false";
  if (typeof v === "string") return v;
  if (Array.isArray(v)) {
    if (v.length <= 20) {
      return "[" + v.map(fmt).join(", ") + "]";
    }
    return "[" + v.map(fmt).join(", ") + "]"; // still show all for correctness
  }
  if (typeof v === "object") {
    // Image data objects: summarize
    if (v.data || v.bitmap) {
      const w = v.width || "?";
      const h = v.height || "?";
      const kind = v.kind || "img";
      return `{width=${w}, height=${h}, kind=${kind}}`;
    }
    // Generic object
    const parts = [];
    for (const [k, val] of Object.entries(v)) {
      if (k === "data" || k === "bitmap") continue;
      parts.push(`${k}=${fmt(val)}`);
    }
    return `{${parts.join(", ")}}`;
  }
  return String(v);
}

function formatOp(fnId, args) {
  const name = FN_NAME.get(fnId) || `UNKNOWN(${fnId})`;
  if (!args || args.length === 0) return name;

  // Special formatting for certain ops
  if (fnId === OPS.constructPath && args.length >= 2) {
    // args: [ops_array, coords_array, minMax?]
    const ops = args[0];
    const coords = args[1];
    return `constructPath ops=[${ops}] coords=[${coords.map(c => typeof c === "number" ? c.toFixed(6) : String(c)).join(",")}]`;
  }

  if (
    fnId === OPS.paintImageXObject ||
    fnId === OPS.paintInlineImageXObject ||
    fnId === OPS.paintImageMaskXObject
  ) {
    if (args.length >= 2 && args[1]?.width) {
      const imgName = fmt(args[0]);
      const img = args[1];
      return `${name} ${imgName} ${img.width}x${img.height}`;
    }
  }

  // showText / showSpacedText: just note the glyph count
  if (fnId === OPS.showText && Array.isArray(args[0])) {
    const glyphs = args[0].length;
    return `showText glyphs=${glyphs}`;
  }
  if (fnId === OPS.showSpacedText && Array.isArray(args[0])) {
    const items = args[0].length;
    return `showSpacedText items=${items}`;
  }

  return `${name} ${args.map(fmt).join(" ")}`;
}

async function main() {
  const pdfPath = process.argv[2];
  if (!pdfPath) {
    console.error("Usage: node scripts/pdfjs_oplist.mjs <pdf-path> [page-index]");
    process.exit(1);
  }
  const pageIndex = parseInt(process.argv[3] || "0", 10);

  const data = new Uint8Array(fs.readFileSync(pdfPath));
  const doc = await getDocument({
    data,
    cMapUrl: CMAP_DIR + "/",
    cMapPacked: true,
    standardFontDataUrl: FONT_DIR + "/",
    verbosity: 0, // Errors only
  }).promise;

  const page = await doc.getPage(pageIndex + 1); // pdf.js is 1-based
  const opList = await page.getOperatorList();

  for (let i = 0; i < opList.fnArray.length; i++) {
    console.log(formatOp(opList.fnArray[i], opList.argsArray[i]));
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
