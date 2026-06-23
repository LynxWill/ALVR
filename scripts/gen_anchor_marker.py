#!/usr/bin/env python3
"""Generate a printable Original-ArUco marker for the ALVR_Lynx anchor.

Matches aruco-rs `DICTIONARY_ARUCO` (5x5 data + 1-cell black border = 7x7).
Bit convention (verified against the detector): bit 1 = WHITE cell, MSB = the
top-left inner cell, row-major. The printed 7x7 black-bordered square equals
MARKER_SIZE_CM, which must match `MARKER_SIZE_M` in camera.rs. A white quiet
zone is added around it (required for detection); a ruler aids print scaling.

Usage:  python scripts/gen_anchor_marker.py            # id 0, 15cm
        python scripts/gen_anchor_marker.py --id 1 --size-cm 25
"""
import argparse
from PIL import Image, ImageDraw, ImageFont

# aruco-rs DICTIONARY_ARUCO.code_list (25-bit codes).
# id 0 is a poor fiducial (5/25 white, near-symmetric) -> ArUco confuses its
# 0/180 rotation near fronto-parallel views (floor marker overhead) and the
# in-plane yaw flips. The 964/1010/781/872 ids are rotation-robust picks
# (min Hamming between their 4 orientations = 14, balanced 12/25 white).
CODES = {
    0: 0x1084210, 1: 0x1084217, 2: 0x1084209, 3: 0x108420e,
    964: 0x0e742f0, 1010: 0x0e73a09, 781: 0x0e841d7, 872: 0x0eba530,
}

GRID = 7          # full marker incl. border
INNER = 5
DPI = 300


def marker_cells(code: int):
    """Return a 7x7 array of 0/255 (black/white) for the given 25-bit code."""
    cells = [[0] * GRID for _ in range(GRID)]  # border defaults to black
    for k in range(INNER * INNER):
        bit = (code >> (INNER * INNER - 1 - k)) & 1
        r, c = k // INNER, k % INNER
        cells[r + 1][c + 1] = 255 if bit else 0
    return cells


def render(marker_id: int, size_cm: float, out: str):
    code = CODES[marker_id]
    cells = marker_cells(code)

    px_per_cm = DPI / 2.54
    marker_px = int(round(size_cm * px_per_cm))
    cell_px = marker_px // GRID
    marker_px = cell_px * GRID                      # snap to whole cells
    quiet = cell_px * 2                             # white quiet zone (2 cells)
    ruler_h = int(round(1.2 * px_per_cm))
    margin = quiet

    W = marker_px + 2 * margin
    H = marker_px + 2 * margin + ruler_h
    img = Image.new("L", (W, H), 255)
    draw = ImageDraw.Draw(img)

    ox, oy = margin, margin
    for r in range(GRID):
        for c in range(GRID):
            if cells[r][c] == 0:
                draw.rectangle(
                    [ox + c * cell_px, oy + r * cell_px,
                     ox + (c + 1) * cell_px - 1, oy + (r + 1) * cell_px - 1],
                    fill=0,
                )

    # Ruler: a black bar exactly spanning the 7x7 marker width (= size_cm).
    ry = marker_px + 2 * margin + ruler_h // 3
    draw.line([(ox, ry), (ox + marker_px, ry)], fill=0, width=3)
    for x in (ox, ox + marker_px):
        draw.line([(x, ry - 10), (x, ry + 10)], fill=0, width=3)
    try:
        font = ImageFont.truetype("arial.ttf", int(0.5 * px_per_cm))
    except Exception:
        font = ImageFont.load_default()
    label = f"ALVR_Lynx ArUco id={marker_id}  marker={size_cm:.1f}cm (full square incl. border)"
    draw.text((ox, ry + 14), label, fill=0, font=font)

    img.save(out, dpi=(DPI, DPI))
    print(f"wrote {out}  ({W}x{H}px @ {DPI}dpi, marker {marker_px}px = {size_cm}cm, cell {cell_px}px)")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--id", type=int, default=964)
    ap.add_argument("--size-cm", type=float, default=15.0)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    out = a.out or f"build/ALVR_Lynx_ArUco_id{a.id}_{a.size_cm:.0f}cm.png"
    render(a.id, a.size_cm, out)
