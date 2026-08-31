#!/usr/bin/env python3
"""Render a PDF page with pdf-x (and optionally mutool) for visual inspection.

Examples:
  python3 scripts/view_page.py document.pdf
  python3 scripts/view_page.py document.pdf --page 0 --scale 1.5
  python3 scripts/view_page.py secret.pdf --password testpw --compare
  python3 scripts/view_page.py a.pdf b.pdf c.pdf --out-dir /tmp/views

Output:
  PNG next to the PDF (or under --out-dir). With --compare, also writes
  mutool PNG and a side-by-side composite ``*_side.png``.
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


def find_render_bin() -> Path:
    env = os.environ.get("PDFX_RENDER_BIN")
    if env:
        p = Path(env)
        if p.is_file():
            return p
    if DEFAULT_BIN.is_file():
        return DEFAULT_BIN
    debug = ROOT / "target/debug/examples/render_page_png"
    if debug.is_file():
        return debug
    return DEFAULT_BIN


def ensure_render_bin(bin_path: Path) -> Path:
    if bin_path.is_file():
        return bin_path
    print(f"render binary not found: {bin_path}", file=sys.stderr)
    print(
        "Build with:\n"
        "  cargo build --release -p pdf-x-core --example render_page_png "
        "--features pdf-x-core/rendering",
        file=sys.stderr,
    )
    sys.exit(2)


def raw_to_png(raw_path: Path, png_path: Path) -> tuple[int, int]:
    try:
        from PIL import Image
    except ImportError as e:
        raise SystemExit("Pillow required: pip install Pillow") from e

    data = raw_path.read_bytes()
    if len(data) < 8:
        raise RuntimeError(f"raw too short: {raw_path}")
    w, h = struct.unpack("<II", data[:8])
    need = 8 + w * h * 4
    if len(data) < need:
        raise RuntimeError(f"raw truncated: have {len(data)} need {need}")
    img = Image.frombytes("RGBA", (w, h), data[8:need])
    png_path.parent.mkdir(parents=True, exist_ok=True)
    img.save(png_path)
    return w, h


def render_pdfx(
    bin_path: Path,
    pdf: Path,
    page: int,
    scale: float,
    password: str | None,
) -> Path:
    fd, raw_name = tempfile.mkstemp(suffix=".raw")
    os.close(fd)
    raw_path = Path(raw_name)
    cmd = [str(bin_path), str(pdf), str(page), str(scale), str(raw_path)]
    if password:
        cmd.append(password)
    env = os.environ.copy()
    if password and "PDFX_PASSWORD" not in env:
        env["PDFX_PASSWORD"] = password
    try:
        r = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=env,
            timeout=120,
            start_new_session=True,
        )
    except subprocess.TimeoutExpired as e:
        raw_path.unlink(missing_ok=True)
        raise RuntimeError("pdf-x render timed out after 120s") from e
    if r.returncode != 0:
        raw_path.unlink(missing_ok=True)
        err = (r.stderr or r.stdout or "").strip()
        raise RuntimeError(f"pdf-x render failed ({r.returncode}): {err[-500:]}")
    return raw_path


def render_mutool(pdf: Path, page: int, scale: float, password: str | None, out_png: Path) -> None:
    # 72 DPI * scale
    dpi = max(1, int(round(72 * scale)))
    out_png.parent.mkdir(parents=True, exist_ok=True)
    cmd = ["mutool", "draw", "-o", str(out_png), "-F", "png", "-r", str(dpi)]
    if password:
        cmd.extend(["-p", password])
    cmd.extend([str(pdf), str(page + 1)])  # mutool is 1-based
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0 or not out_png.is_file() or out_png.stat().st_size == 0:
        err = (r.stderr or r.stdout or "").strip()
        raise RuntimeError(f"mutool failed: {err[-300:]}")


def side_by_side(ours: Path, mut: Path, out: Path) -> None:
    from PIL import Image

    a = Image.open(ours).convert("RGBA")
    b = Image.open(mut).convert("RGBA")
    h = max(a.height, b.height)
    w = a.width + b.width + 8
    canvas = Image.new("RGBA", (w, h), (40, 40, 40, 255))
    canvas.paste(a, (0, 0))
    canvas.paste(b, (a.width + 8, 0))
    canvas.save(out)


def dark_count_png(png: Path, thr: int = 200) -> int:
    from PIL import Image
    import numpy as np

    arr = np.array(Image.open(png).convert("RGBA"))
    return int((arr[..., :3] < thr).any(axis=-1).sum())


def process_one(
    bin_path: Path,
    pdf: Path,
    page: int,
    scale: float,
    password: str | None,
    out_dir: Path | None,
    compare: bool,
    open_result: bool,
) -> Path:
    if not pdf.is_file():
        raise FileNotFoundError(pdf)

    dest_dir = out_dir if out_dir else pdf.parent
    dest_dir.mkdir(parents=True, exist_ok=True)
    stem = f"{pdf.stem}_p{page}_s{scale:g}"
    ours_png = dest_dir / f"{stem}_pdfx.png"

    raw = render_pdfx(bin_path, pdf, page, scale, password)
    try:
        w, h = raw_to_png(raw, ours_png)
    finally:
        raw.unlink(missing_ok=True)

    dark = dark_count_png(ours_png)
    print(f"pdf-x: {ours_png}  ({w}x{h}, dark≈{dark})")

    if compare:
        mut_png = dest_dir / f"{stem}_mutool.png"
        try:
            render_mutool(pdf, page, scale, password, mut_png)
            md = dark_count_png(mut_png)
            print(f"mutool: {mut_png}  (dark≈{md})")
            side = dest_dir / f"{stem}_side.png"
            side_by_side(ours_png, mut_png, side)
            print(f"side-by-side: {side}  (left=pdf-x, right=mutool)")
        except Exception as e:
            print(f"compare skipped: {e}", file=sys.stderr)

    if open_result:
        # Best-effort open on Linux
        for opener in ("xdg-open", "eog", "feh"):
            if subprocess.call(["which", opener], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) == 0:
                subprocess.Popen([opener, str(ours_png)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                break

    return ours_png


def main() -> None:
    ap = argparse.ArgumentParser(description="Render PDF page(s) with pdf-x for visual inspection")
    ap.add_argument("pdfs", nargs="+", type=Path, help="PDF file(s)")
    ap.add_argument("--page", type=int, default=0, help="0-based page index (default 0)")
    ap.add_argument("--scale", type=float, default=1.0, help="render scale (default 1.0 ≈ 72 DPI)")
    ap.add_argument("--password", default=None, help="user password for encrypted PDFs")
    ap.add_argument("--out-dir", type=Path, default=None, help="output directory (default: beside PDF)")
    ap.add_argument("--compare", action="store_true", help="also render mutool and side-by-side PNG")
    ap.add_argument("--open", action="store_true", help="open result with system viewer")
    ap.add_argument("--bin", type=Path, default=None, help="path to render_page_png binary")
    args = ap.parse_args()

    bin_path = ensure_render_bin(args.bin or find_render_bin())
    print(f"using {bin_path}")

    failed = 0
    for pdf in args.pdfs:
        try:
            process_one(
                bin_path,
                pdf.expanduser().resolve(),
                args.page,
                args.scale,
                args.password,
                args.out_dir,
                args.compare,
                args.open,
            )
        except Exception as e:
            failed += 1
            print(f"FAIL {pdf}: {e}", file=sys.stderr)

    if failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
