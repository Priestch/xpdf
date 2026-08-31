#!/usr/bin/env python3
"""Accuracy comparison: pdf-x vs pdftotext (Poppler).

Full-text 3-gram Jaccard for speed; per-page LCS for first 20 pages.

Usage:
    python3 scripts/text_accuracy.py [pdf-path ...]
"""
import difflib
import subprocess
import sys
import os

def run(cmd):
    r = subprocess.run(cmd, capture_output=True)
    return r.stdout.decode("utf-8", errors="replace") if r.returncode == 0 else ""

def normalize(s):
    return "".join(c.lower() for c in s if not c.isspace())

def ngrams(s, n=3):
    if len(s) < n:
        return set()
    return {s[i:i+n] for i in range(len(s) - n + 1)}

def jaccard(a, b):
    if not a and not b:
        return 1.0
    if not a or not b:
        return 0.0
    return len(a & b) / len(a | b)

def lcs_sim(a, b):
    if not a and not b:
        return 1.0
    if not a or not b:
        return 0.0
    return difflib.SequenceMatcher(None, a, b, autojunk=False).ratio()

def first_diff(a, b, ctx=50):
    for i, (ca, cb) in enumerate(zip(a, b)):
        if ca != cb:
            start = max(0, i - ctx)
            return i, a[start:i+ctx], b[start:i+ctx]
    if len(a) != len(b):
        short = min(len(a), len(b))
        return short, a[short:short+ctx], b[short:short+ctx]
    return -1, "", ""

def main():
    pdfs = sys.argv[1:] or [
        "pdf.js/test/pdfs/tracemonkey.pdf",
        "pdf.js/test/pdfs/freeculture.pdf",
    ]

    pdfx_bin = os.path.join(os.path.dirname(__file__), "..", "target", "release", "examples", "extract_all_text")

    for pdf in pdfs:
        basename = os.path.basename(pdf)

        r = subprocess.run(["pdfinfo", pdf], capture_output=True)
        pages = 0
        for line in r.stdout.decode().splitlines():
            if line.startswith("Pages:"):
                pages = int(line.split(":")[1].strip())

        # Full text
        ours_full = run([pdfx_bin, pdf, "all"])
        ref_full = run(["pdftotext", pdf, "-"])
        ref_layout = run(["pdftotext", "-layout", pdf, "-"])

        ours_norm = normalize(ours_full)
        ref_norm = normalize(ref_full)
        ref_layout_norm = normalize(ref_layout)

        sim_jac = jaccard(ngrams(ours_norm), ngrams(ref_norm))
        sim_jac_layout = jaccard(ngrams(ours_norm), ngrams(ref_layout_norm))

        print(f"\n=== {basename} ({pages} pages) ===")
        print(f"3-gram Jaccard (raw):    {sim_jac*100:.2f}%")
        print(f"3-gram Jaccard (layout): {sim_jac_layout*100:.2f}%")
        print(f"NWchars: ours={len(ours_norm)} ref_raw={len(ref_norm)} ref_layout={len(ref_layout_norm)}")

        # Per-page LCS (first 20 pages)
        ref_pages = ref_full.split("\f")
        max_page = min(pages, 20)
        print(f"\n{'Page':>5} {'LCS%':>8} {'NW(ours)':>8} {'NW(ref)':>8}")
        print("-" * 45)
        worst_pages = []
        for i in range(max_page):
            ours_page = run([pdfx_bin, pdf, f"page={i}"])
            ref_page = ref_pages[i] if i < len(ref_pages) else ""
            ours_pn = normalize(ours_page)
            ref_pn = normalize(ref_page)
            sim = lcs_sim(ours_pn, ref_pn)
            print(f"{i+1:>5} {sim*100:>7.2f}% {len(ours_pn):>8} {len(ref_pn):>8}")
            if sim < 0.95:
                worst_pages.append((i, sim, ours_pn, ref_pn))

        # Show details for worst pages
        for page_idx, sim, ours_pn, ref_pn in worst_pages[:3]:
            pos, ours_ctx, ref_ctx = first_diff(ours_pn, ref_pn)
            if pos >= 0:
                print(f"\n--- Page {page_idx+1} (LCS {sim*100:.1f}%) first diff at char {pos} ---")
                print(f"  ours: ...{ours_ctx}...")
                print(f"  ref:  ...{ref_ctx}...")

if __name__ == "__main__":
    main()
