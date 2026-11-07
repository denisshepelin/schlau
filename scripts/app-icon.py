# /// script
# requires-python = ">=3.11"
# dependencies = ["pillow", "numpy"]
# ///
"""Builds the macOS app icon from the artwork in assets/icon/schlau-fuchs.png.

The artwork is a rounded square painted on a dark backdrop. The square is cut out with a superellipse
(Apple's continuous corner, closely enough), placed on Apple's icon grid (824 px of shape on a 1024 px
canvas) with a soft shadow, and written as assets/icon/schlau-1024.png and assets/icon/schlau.icns.

    uv run scripts/app-icon.py
"""

import subprocess
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "assets/icon/schlau-fuchs.png"
PNG = ROOT / "assets/icon/schlau-1024.png"
ICNS = ROOT / "assets/icon/schlau.icns"

# Where the rounded square sits in the artwork, and how square its corners are.
SHAPE_LO, SHAPE_HI, EXPONENT = 86.0, 1168.0, 4.6
CANVAS, SHAPE = 1024, 824
SHADOW_OFFSET, SHADOW_BLUR, SHADOW_ALPHA = 10, 14, 0.35
ICONSET = [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)]


def cut_out(artwork: Image.Image) -> Image.Image:
    rgb = np.asarray(artwork.convert("RGB"))
    centre, radius = (SHAPE_LO + SHAPE_HI) / 2, (SHAPE_HI - SHAPE_LO) / 2
    ys, xs = np.mgrid[0 : rgb.shape[0], 0 : rgb.shape[1]] + 0.5
    f = np.abs((xs - centre) / radius) ** EXPONENT + np.abs((ys - centre) / radius) ** EXPONENT
    alpha = np.clip(0.5 + (1 - f) * radius / EXPONENT, 0, 1)
    rgba = np.dstack([rgb, (alpha * 255).round().astype(np.uint8)])
    box = tuple(int(v) for v in (SHAPE_LO, SHAPE_LO, SHAPE_HI, SHAPE_HI))
    return Image.fromarray(rgba, "RGBA").crop(box)


def on_grid(shape: Image.Image) -> Image.Image:
    shape = shape.resize((SHAPE, SHAPE), Image.Resampling.LANCZOS)
    margin = (CANVAS - SHAPE) // 2
    shadow_alpha = Image.new("L", (CANVAS, CANVAS), 0)
    shadow_alpha.paste(shape.getchannel("A"), (margin, margin + SHADOW_OFFSET))
    shadow_alpha = shadow_alpha.filter(ImageFilter.GaussianBlur(SHADOW_BLUR)).point(lambda a: round(a * SHADOW_ALPHA))
    canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    canvas.putalpha(shadow_alpha)
    canvas.alpha_composite(shape, (margin, margin))
    return canvas


def write_icns(icon: Image.Image) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "schlau.iconset"
        iconset.mkdir()
        for size, scale in ICONSET:
            name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
            icon.resize((size * scale, size * scale), Image.Resampling.LANCZOS).save(iconset / name)
        subprocess.run(["iconutil", "--convert", "icns", "--output", str(ICNS), str(iconset)], check=True)


def main() -> None:
    icon = on_grid(cut_out(Image.open(SOURCE)))
    icon.save(PNG, optimize=True)
    write_icns(icon)
    print(PNG.relative_to(ROOT), ICNS.relative_to(ROOT))


if __name__ == "__main__":
    main()
