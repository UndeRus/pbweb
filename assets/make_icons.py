#!/usr/bin/env python3
"""Generate PocketBook launcher icons for PBWeb.

Target: e-ink friendly, high contrast, 8-bit BMP.
- pbweb.bmp   : normal state (black on white)
- pbweb_f.bmp : focused/pressed state (inverted)
- pbweb-preview.png : large preview for README (not for device)

Convention (see jjrrw174/PocketBook-Desktop-and-App-Customizations):
  /mnt/ext1/applications/icons/pbweb.bmp (+ pbweb_f.bmp),
  registered in /system/config/desktop/view.json as U_pbweb.
"""
import os
from PIL import Image, ImageDraw, ImageOps

W, H = 106, 128
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)))


def draw_icon(draw: ImageDraw.ImageDraw) -> None:
    # frame
    draw.rounded_rectangle([2, 2, W - 3, H - 3], radius=14, outline=0, width=5)
    # wifi mark: dot + 3 arcs opening upwards (PIL angles: 0=east, clockwise)
    cx, dot_y = W // 2, 56
    draw.ellipse([cx - 5, dot_y - 5, cx + 5, dot_y + 5], fill=0)
    for r in (15, 26, 37):
        draw.arc(
            [cx - r, dot_y - r, cx + r, dot_y + r],
            start=200,
            end=340,
            fill=0,
            width=6,
        )
    # open book: two pages + spine + text lines
    top, bot = 84, 114
    lx0, lx1 = 16, 50
    rx0, rx1 = 54, 88
    draw.rectangle([lx0, top, lx1, bot], outline=0, width=3)
    draw.rectangle([rx0, top, rx1, bot], outline=0, width=3)
    draw.line([W // 2, top - 2, W // 2, bot + 2], fill=0, width=3)
    for yy in (92, 99, 106):
        draw.line([lx0 + 6, yy, lx1 - 6, yy], fill=0, width=2)
        draw.line([rx0 + 6, yy, rx1 - 6, yy], fill=0, width=2)


def main() -> None:
    img = Image.new("L", (W, H), 255)
    draw_icon(ImageDraw.Draw(img))
    img.save(os.path.join(OUT, "pbweb.bmp"))

    focused = ImageOps.invert(img.convert("L"))
    focused.save(os.path.join(OUT, "pbweb_f.bmp"))

    big = img.resize((W * 4, H * 4), Image.NEAREST)
    big.save(os.path.join(OUT, "pbweb-preview.png"))

    for name in ("pbweb.bmp", "pbweb_f.bmp", "pbweb-preview.png"):
        p = os.path.join(OUT, name)
        with Image.open(p) as im:
            print(f"{name}: mode={im.mode} size={im.size} bytes={os.path.getsize(p)}")


if __name__ == "__main__":
    main()
