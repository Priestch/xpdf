#!/usr/bin/env python3
"""Smoke-scan real-world PDFs with pdf-x: crash / blank / very sparse pages.

Designed for safety on large book corpora:
  - default timeout 45s per file (was 180s)
  - skip PDFs larger than --max-mb (default 80)
  - kill hung render children on timeout
  - dark-count in C-speed (numpy), not pure-Python per-pixel loops
  - --compare-dark is OFF by default (mutool doubles cost)

Examples:
  python3 scripts/scan_real_pdfs.py ~/Downloads --limit 30
  python3 scripts/scan_real_pdfs.py ~/Books -r --max-mb 40 --timeout 30
  python3 scripts/scan_real_pdfs.py a.pdf --compare-dark

Exit code 0 if no hard failures (crashes/timeouts). Blank pages are reported
but non-fatal unless --fail-blank is set.
"""
from __future__ import annotations

import argparse
import os
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_BIN = ROOT / "target/release/examples/render_page_png"

# Safety defaults for book-sized corpora (override via flags).
DEFAULT_TIMEOUT_S = 45
DEFAULT_MAX_MB = 80.0


def find_bin() -> Path:
    env = os.environ.get("PDFX_RENDER_BIN")
    if env and Path(env).is_file():
        return Path(env)
    if DEFAULT_BIN.is_file():
        return DEFAULT_BIN
    debug = ROOT / "target/debug/examples/render_page_png"
    return debug if debug.is_file() else DEFAULT_BIN


def collect_pdfs(paths: list[Path], recursive: bool) -> list[Path]:
    out: list[Path] = []
    for p in paths:
        p = p.expanduser().resolve()
        if p.is_file() and p.suffix.lower() == ".pdf":
            out.append(p)
        elif p.is_dir():
            if recursive:
                out.extend(sorted(p.rglob("*.pdf")))
            else:
                out.extend(sorted(p.glob("*.pdf")))
    seen: set[Path] = set()
    uniq: list[Path] = []
    for p in out:
        if p not in seen:
            seen.add(p)
            uniq.append(p)
    return uniq


def dark_from_raw(raw: bytes, thr: int = 200) -> tuple[int, int, int, int]:
    """Return (w, h, dark_count, pixel_count). Uses numpy when available."""
    if len(raw) < 8:
        return 0, 0, 0, 0
    w, h = struct.unpack("<II", raw[:8])
    n = w * h
    need = 8 + n * 4
    if n <= 0 or len(raw) < need:
        return w, h, 0, max(n, 0)
    # Guard absurd dimensions (corrupt header / giant MediaBox) before allocating.
    if n > 50_000_000:  # ~50 Mpx ≈ 200 MB RGBA
        return w, h, -1, n
    try:
        import numpy as np

        arr = np.frombuffer(raw, dtype=np.uint8, offset=8, count=n * 4).reshape(n, 4)
        dark = int((arr[:, :3] < thr).any(axis=1).sum())
        return w, h, dark, n
    except Exception:
        # Fallback: sample every 16th pixel to avoid multi-second pure-Python loops.
        dark = 0
        step = 16 * 4
        for i in range(0, n * 4, step):
            if raw[8 + i] < thr or raw[8 + i + 1] < thr or raw[8 + i + 2] < thr:
                dark += 1
        return w, h, dark * 16, n


def mutool_dark(
    pdf: Path, page: int, scale: float, password: str | None, thr: int = 200, timeout: int = 30
) -> int | None:
    try:
        from PIL import Image
        import numpy as np
    except ImportError:
        return None
    dpi = max(1, int(round(72 * scale)))
    with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as f:
        out = Path(f.name)
    try:
        cmd = ["mutool", "draw", "-o", str(out), "-F", "png", "-r", str(dpi)]
        if password:
            cmd.extend(["-p", password])
        cmd.extend([str(pdf), str(page + 1)])
        subprocess.run(cmd, capture_output=True, timeout=timeout)
        if not out.is_file() or out.stat().st_size == 0:
            return None
        arr = np.array(Image.open(out).convert("RGBA"))
        return int((arr[..., :3] < thr).any(axis=-1).sum())
    except Exception:
        return None
    finally:
        out.unlink(missing_ok=True)


def scan_one(
    bin_path: Path,
    pdf: Path,
    page: int,
    scale: float,
    password: str | None,
    compare_dark: bool,
    timeout: int,
    max_mb: float,
) -> dict:
    result = {
        "pdf": str(pdf),
        "status": "OK",
        "w": 0,
        "h": 0,
        "dark": 0,
        "pixels": 0,
        "err": "",
        "mut_dark": None,
        "collapse": False,
    }

    try:
        size_mb = pdf.stat().st_size / (1024 * 1024)
    except OSError as e:
        result["status"] = "BAD_OUT"
        result["err"] = str(e)
        return result

    if max_mb > 0 and size_mb > max_mb:
        result["status"] = "SKIP_LARGE"
        result["err"] = f"{size_mb:.1f} MB > --max-mb {max_mb:g}"
        return result

    fd, raw_name = tempfile.mkstemp(suffix=".raw")
    os.close(fd)
    raw_path = Path(raw_name)
    cmd = [str(bin_path), str(pdf), str(page), str(scale), str(raw_path)]
    if password:
        cmd.append(password)
    env = os.environ.copy()
    if password:
        env["PDFX_PASSWORD"] = password

    try:
        # start_new_session so timeout can kill the whole process group if needed
        r = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=env,
            timeout=timeout,
            start_new_session=True,
        )
    except subprocess.TimeoutExpired:
        raw_path.unlink(missing_ok=True)
        result["status"] = "TIMEOUT"
        result["err"] = f"render exceeded {timeout}s"
        return result
    except Exception as e:
        raw_path.unlink(missing_ok=True)
        result["status"] = "CRASH"
        result["err"] = str(e)
        return result

    if r.returncode != 0:
        err = (r.stderr or r.stdout or "").strip()
        if r.returncode == 2 and "password" in err.lower():
            result["status"] = "NEED_PASSWORD"
        else:
            result["status"] = "CRASH"
        result["err"] = err[-300:]
        raw_path.unlink(missing_ok=True)
        return result

    try:
        data = raw_path.read_bytes()
        w, h, dark, n = dark_from_raw(data)
        result.update(w=w, h=h, dark=max(dark, 0), pixels=n)
        if dark < 0:
            result["status"] = "HUGE_PAGE"
            result["err"] = f"pixmap {w}x{h} too large to analyze"
        elif n > 0 and dark == 0:
            result["status"] = "BLANK"
        elif n > 1000 and dark < max(20, n * 0.0005):
            result["status"] = "SPARSE"
        if compare_dark and result["status"] in ("OK", "SPARSE", "BLANK"):
            md = mutool_dark(pdf, page, scale, password, timeout=min(30, timeout))
            result["mut_dark"] = md
            if md is not None and md > 500 and dark < md * 0.15:
                result["collapse"] = True
                if result["status"] == "OK":
                    result["status"] = "DARK_COLLAPSE"
    except Exception as e:
        result["status"] = "BAD_OUT"
        result["err"] = str(e)
    finally:
        raw_path.unlink(missing_ok=True)
    return result


def main() -> None:
    ap = argparse.ArgumentParser(
        description="Smoke-scan PDFs with pdf-x renderer (safe defaults for large books)"
    )
    ap.add_argument("paths", nargs="+", type=Path, help="PDF files or directories")
    ap.add_argument("--recursive", "-r", action="store_true", help="recurse into directories")
    ap.add_argument("--limit", type=int, default=0, help="max files (0=all)")
    ap.add_argument("--page", type=int, default=0, help="0-based page (default 0)")
    ap.add_argument("--scale", type=float, default=1.0)
    ap.add_argument("--password", default=None)
    ap.add_argument(
        "--timeout",
        type=int,
        default=DEFAULT_TIMEOUT_S,
        help=f"seconds per PDF (default {DEFAULT_TIMEOUT_S}; kill hung renders)",
    )
    ap.add_argument(
        "--max-mb",
        type=float,
        default=DEFAULT_MAX_MB,
        help=f"skip PDFs larger than this many MB (default {DEFAULT_MAX_MB:g}; 0=no skip)",
    )
    ap.add_argument(
        "--compare-dark",
        action="store_true",
        help="also run mutool and flag dark-collapse (slower; off by default)",
    )
    ap.add_argument(
        "--fail-blank",
        action="store_true",
        help="exit non-zero on BLANK/SPARSE/collapse",
    )
    ap.add_argument("--bin", type=Path, default=None)
    args = ap.parse_args()

    bin_path = args.bin or find_bin()
    if not bin_path.is_file():
        print(f"missing render binary: {bin_path}", file=sys.stderr)
        print(
            "cargo build --release -p pdf-x-core --example render_page_png "
            "--features pdf-x-core/rendering",
            file=sys.stderr,
        )
        sys.exit(2)

    pdfs = collect_pdfs(args.paths, args.recursive)
    if args.limit and args.limit > 0:
        pdfs = pdfs[: args.limit]
    if not pdfs:
        print("no PDFs found", file=sys.stderr)
        sys.exit(2)

    print(
        f"scanning {len(pdfs)} PDF(s) with {bin_path} "
        f"(timeout={args.timeout}s max-mb={args.max_mb:g} compare_dark={args.compare_dark})"
    )
    print(f"{'status':14} {'dark':>8} {'mut':>8} {'size':>12} file")
    print("-" * 100)

    counts: dict[str, int] = {}
    problems: list[dict] = []
    for i, pdf in enumerate(pdfs, 1):
        try:
            res = scan_one(
                bin_path,
                pdf,
                args.page,
                args.scale,
                args.password,
                args.compare_dark,
                args.timeout,
                args.max_mb,
            )
        except Exception as e:
            res = {
                "pdf": str(pdf),
                "status": "CRASH",
                "dark": 0,
                "mut_dark": None,
                "w": 0,
                "h": 0,
                "err": str(e),
            }
        counts[res["status"]] = counts.get(res["status"], 0) + 1
        mut = res.get("mut_dark")
        mut_s = f"{mut}" if mut is not None else "-"
        size = f"{res.get('w', 0)}x{res.get('h', 0)}"
        flag = " <<<" if res["status"] not in ("OK", "SKIP_LARGE") else ""
        # Progress every file; flush so long runs don't look hung.
        print(
            f"{res['status']:14} {res.get('dark', 0):8} {mut_s:>8} {size:>12} "
            f"[{i}/{len(pdfs)}] {pdf.name}{flag}",
            flush=True,
        )
        if res["status"] not in ("OK", "SKIP_LARGE"):
            problems.append(res)
            if res.get("err"):
                print(f"    {res['err'][:200]}", flush=True)

    print("-" * 100)
    print("counts:", counts)
    hard = sum(counts.get(k, 0) for k in ("CRASH", "TIMEOUT", "BAD_OUT", "HUGE_PAGE"))
    soft = sum(counts.get(k, 0) for k in ("BLANK", "SPARSE", "DARK_COLLAPSE"))
    skipped = counts.get("SKIP_LARGE", 0) + counts.get("NEED_PASSWORD", 0)
    print(
        f"hard_fail={hard} soft_issue={soft} skipped={skipped} ok={counts.get('OK', 0)}"
    )

    if problems:
        print("\nproblem list:")
        for r in problems:
            print(f"  [{r['status']}] {r['pdf']}")

    if hard:
        sys.exit(1)
    if args.fail_blank and soft:
        sys.exit(1)


if __name__ == "__main__":
    main()
