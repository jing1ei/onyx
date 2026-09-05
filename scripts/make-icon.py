#!/usr/bin/env python3
"""Generate the Onyx app icon source (obsidian slab + champagne mark).

Produces `src-tauri/icons/source.png` (1024x1024, RGBA), which is then fed to
`npx @tauri-apps/cli icon` to produce every platform icon.

    python3 scripts/make-icon.py

Design: an obsidian (near-black, faintly graded) squircle carrying a champagne
ring split into two arcs -- deck A on the left, deck B on the right -- with a
champagne playhead bar running through the middle.
"""

from __future__ import annotations

import pathlib

from PIL import Image, ImageDraw, ImageFilter

SIZE = 1024
SS = 4  # supersampling factor
N = SIZE * SS

OBSIDIAN_TOP = (26, 28, 33)
OBSIDIAN_BOTTOM = (7, 8, 10)
CHAMPAGNE_LIGHT = (242, 226, 194)
CHAMPAGNE_DARK = (198, 165, 105)


def lerp(a: tuple[int, int, int], b: tuple[int, int, int], t: float) -> tuple[int, int, int]:
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(3))  # type: ignore[return-value]


def vertical_gradient(
    size: int, top: tuple[int, int, int], bottom: tuple[int, int, int]
) -> Image.Image:
    strip = Image.new("RGB", (1, size))
    px = strip.load()
    for y in range(size):
        px[0, y] = lerp(top, bottom, y / (size - 1))
    return strip.resize((size, size), Image.NEAREST)


def squircle_mask(size: int, radius: int) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=255)
    return mask


def main() -> None:
    out = pathlib.Path(__file__).resolve().parent.parent / "src-tauri" / "icons" / "source.png"

    # --- obsidian slab ----------------------------------------------------
    slab = vertical_gradient(N, OBSIDIAN_TOP, OBSIDIAN_BOTTOM).convert("RGBA")
    slab.putalpha(squircle_mask(N, radius=int(N * 0.22)))

    # A soft diagonal sheen so the "obsidian" reads as polished stone.
    sheen = Image.new("L", (N, N), 0)
    ImageDraw.Draw(sheen).polygon(
        [(0, int(N * 0.62)), (N, int(N * 0.06)), (N, 0), (0, 0)], fill=16
    )
    sheen = sheen.filter(ImageFilter.GaussianBlur(N * 0.05))
    slab.alpha_composite(Image.merge("RGBA", (*[Image.new("L", (N, N), 255)] * 3, sheen)))
    slab.putalpha(squircle_mask(N, radius=int(N * 0.22)))

    # --- champagne mark ---------------------------------------------------
    mark = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    ink = Image.new("L", (N, N), 0)
    draw = ImageDraw.Draw(ink)

    cx = cy = N / 2
    r = N * 0.285
    thickness = int(N * 0.062)
    box = (cx - r, cy - r, cx + r, cy + r)
    gap = 11  # degrees of clear space at 12 and 6 o'clock

    # Right arc (deck B) at full strength, left arc (deck A) slightly recessed.
    draw.arc(box, start=-90 + gap, end=90 - gap, fill=255, width=thickness)
    draw.arc(box, start=90 + gap, end=270 - gap, fill=170, width=thickness)

    # Playhead through the middle.
    bar_w = int(N * 0.042)
    bar_h = N * 0.215
    draw.rounded_rectangle(
        (cx - bar_w / 2, cy - bar_h, cx + bar_w / 2, cy + bar_h),
        radius=bar_w / 2,
        fill=255,
    )

    gradient = vertical_gradient(N, CHAMPAGNE_LIGHT, CHAMPAGNE_DARK).convert("RGBA")
    gradient.putalpha(ink)
    mark.alpha_composite(gradient)

    icon = Image.alpha_composite(slab, mark).resize((SIZE, SIZE), Image.LANCZOS)
    out.parent.mkdir(parents=True, exist_ok=True)
    icon.save(out)
    print(f"wrote {out} ({SIZE}x{SIZE})")


if __name__ == "__main__":
    main()
