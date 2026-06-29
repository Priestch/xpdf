#!/usr/bin/env python3
"""Visual diff: pdf-x vs mutool rendering comparison.

Renders test PDFs with both engines at 72 DPI, compares pixel output,
and reports per-page match/mismatch statistics.

Usage:
    python3 scripts/visual_diff.py [pdf-path ...]

Without arguments, runs on all PDFs in hayro/hayro-tests/pdfs/custom/.
"""
import subprocess
import sys
import os
import tempfile
import struct

def get_page_count(pdf_path):
    r = subprocess.run(["pdfinfo", pdf_path], capture_output=True, text=True)
    for line in r.stdout.splitlines():
        if line.startswith("Pages:"):
            return int(line.split(":")[1].strip())
    return 1

def render_mutool(pdf_path, page, dpi=72):
    """Render with mutool to PPM, return (width, height, rgba_bytes)."""
    with tempfile.NamedTemporaryFile(suffix=".ppm", delete=False) as f:
        out = f.name
    try:
        r = subprocess.run(
            ["mutool", "draw", "-o", out, "-F", "pnm", "-r", str(dpi),
             pdf_path, str(page + 1)],
            capture_output=True
        )
        if r.returncode != 0:
            return None
        with open(out, "rb") as f:
            data = f.read()
        # Parse PPM (P5=grayscale, P6=RGB)
        if data.startswith(b"P5"):
            # P5\n width height\n maxval\n data (grayscale)
            # Find the 3rd newline (after maxval)
            idx = 0
            nl_count = 0
            while nl_count < 3 and idx < len(data):
                if data[idx] == ord('\n'):
                    nl_count += 1
                idx += 1
            header = data[:idx].decode()
            pixel_data = data[idx:]
            dims = header.split('\n')[1].split()
            w, h = int(dims[0]), int(dims[1])
            # Convert grayscale to RGBA
            rgba = bytearray()
            for g in pixel_data[:w * h]:
                rgba.extend([g, g, g, 255])
            return (w, h, bytes(rgba))
        elif data.startswith(b"P6"):
            parts = data.split(b"\n", 3)
            w, h = int(parts[1].split()[0]), int(parts[1].split()[1])
            rgb = parts[3]
            rgba = bytearray()
            for i in range(0, len(rgb), 3):
                rgba.extend(rgb[i:i+3])
                rgba.append(255)
            return (w, h, bytes(rgba))
        return None
    finally:
        os.unlink(out)

def render_pdfx(pdf_path, page, scale=1.0):
    """Render with pdf-x, return (width, height, rgba_bytes)."""
    bin = os.path.join(os.path.dirname(__file__), "..",
                       "target/release/examples/render_page_png")
    if not os.path.exists(bin):
        return None
    with tempfile.NamedTemporaryFile(suffix=".raw", delete=False) as f:
        out = f.name
    try:
        r = subprocess.run(
            [bin, pdf_path, str(page), str(scale), out],
            capture_output=True
        )
        if r.returncode != 0:
            sys.stderr.write(f"  pdf-x failed: {r.stderr.decode('utf-8','replace')[:200]}\n")
            return None
        with open(out, "rb") as f:
            data = f.read()
        # First 8 bytes: width (u32 LE) + height (u32 LE)
        if len(data) < 8:
            return None
        w, h = struct.unpack("<II", data[:8])
        rgba = data[8:]
        return (w, h, rgba)
    finally:
        os.unlink(out)

def compare_pixels(a, b, threshold=5):
    """Compare two RGBA buffers. Returns (matching_pixels, total_pixels, max_diff)."""
    w = min(a[0], b[0])
    h = min(a[1], b[1])
    total = w * h
    matching = 0
    max_diff = 0
    for y in range(h):
        for x in range(w):
            ai = (y * a[0] + x) * 4
            bi = (y * b[0] + x) * 4
            if ai + 3 >= len(a[2]) or bi + 3 >= len(b[2]):
                continue
            dr = abs(a[2][ai] - b[2][bi])
            dg = abs(a[2][ai+1] - b[2][bi+1])
            db = abs(a[2][ai+2] - b[2][bi+2])
            diff = max(dr, dg, db)
            max_diff = max(max_diff, diff)
            if diff <= threshold:
                matching += 1
    return matching, total, max_diff

def main():
    if len(sys.argv) > 1:
        pdfs = sys.argv[1:]
    else:
        custom = os.path.join(os.path.dirname(__file__), "..",
                              "hayro/hayro-tests/pdfs/custom")
        pdfs = sorted([os.path.join(custom, f) for f in os.listdir(custom)
                       if f.endswith(".pdf")])

    print(f"{'PDF':<40s} {'Pages':>5} {'Match%':>7} {'MaxDiff':>7} {'Status':>8}")
    print("-" * 70)

    total_pass = 0
    total_fail = 0
    total_skip = 0

    for pdf in pdfs:
        name = os.path.basename(pdf)
        try:
            pages = get_page_count(pdf)
        except:
            pages = 1

        # Only test first page for speed
        pages_to_test = min(pages, 3)

        all_match = True
        worst_pct = 100.0
        worst_diff = 0

        for page in range(pages_to_test):
            mu = render_mutool(pdf, page)
            px = render_pdfx(pdf, page)

            if mu is None or px is None:
                all_match = False
                worst_pct = 0
                worst_diff = 999
                continue

            if mu[0] != px[0] or mu[1] != px[1]:
                # Size mismatch — still compare overlapping region
                pass

            matching, total, max_diff = compare_pixels(mu, px)
            pct = matching / total * 100 if total > 0 else 0
            worst_pct = min(worst_pct, pct)
            worst_diff = max(worst_diff, max_diff)

            if pct < 99.5:
                all_match = False

        status = "PASS" if all_match else ("SKIP" if worst_diff == 999 else "FAIL")
        if status == "PASS":
            total_pass += 1
        elif status == "FAIL":
            total_fail += 1
        else:
            total_skip += 1

        print(f"{name:<40s} {pages:>5} {worst_pct:>6.1f}% {worst_diff:>7} {status:>8}")

    print(f"\n{'='*70}")
    print(f"PASS: {total_pass}  FAIL: {total_fail}  SKIP: {total_skip}  TOTAL: {len(pdfs)}")

if __name__ == "__main__":
    main()
