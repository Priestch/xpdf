#!/usr/bin/env python3
"""Generate reference PNG goldens from mutool for all test PDFs.

Usage:
    python3 scripts/generate_goldens.py           # all PDFs
    python3 scripts/generate_goldens.py file.pdf  # specific PDFs

Goldens are stored in tests/goldens/ (gitignored). Each PDF page
becomes <name>_p<page>.png. Single-page PDFs use just <name>.png.
"""
import subprocess
import sys
import os

def get_page_count(pdf_path):
    r = subprocess.run(["pdfinfo", pdf_path], capture_output=True, text=True)
    for line in r.stdout.splitlines():
        if line.startswith("Pages:"):
            return int(line.split(":")[1].strip())
    return 1

def render_mutool_png(pdf_path, page, out_path, dpi=72):
    r = subprocess.run(
        ["mutool", "draw", "-o", out_path, "-F", "png", "-r", str(dpi),
         pdf_path, str(page + 1)],
        capture_output=True
    )
    return r.returncode == 0

def main():
    goldens_dir = os.path.join(os.path.dirname(__file__), "..", "tests", "goldens")
    os.makedirs(goldens_dir, exist_ok=True)

    if len(sys.argv) > 1:
        pdfs = sys.argv[1:]
    else:
        custom = os.path.join(os.path.dirname(__file__), "..",
                              "hayro", "hayro-tests", "pdfs", "custom")
        pdfs = sorted([os.path.join(custom, f) for f in os.listdir(custom)
                       if f.endswith(".pdf")])

    print(f"Generating goldens for {len(pdfs)} PDFs in {goldens_dir}")
    generated = 0
    skipped = 0

    for pdf in pdfs:
        name = os.path.splitext(os.path.basename(pdf))[0]
        try:
            pages = min(get_page_count(pdf), 3)  # Cap at 3 pages
        except:
            pages = 1

        for page in range(pages):
            suffix = f"_p{page}" if pages > 1 else ""
            out = os.path.join(goldens_dir, f"{name}{suffix}.png")

            if render_mutool_png(pdf, page, out):
                generated += 1
            else:
                skipped += 1
                print(f"  SKIP: {name} page {page}")

    print(f"\nGenerated: {generated}  Skipped: {skipped}")

if __name__ == "__main__":
    main()
