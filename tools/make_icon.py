#!/usr/bin/env python3
"""Generate assets/com_port_watcher.ico (+ .png preview) for the Rust
COM Port Watcher. Same glyph as the runtime tray icon: green rounded
square, white serial plug (body + 3 pins + stem). Drawn at 256x256
with supersampling, saved as a multi-size ICO for toast/AppUserModel
use. Run from the project root: python3 tools/make_icon.py
"""
import os

from PIL import Image, ImageDraw

SS = 4  # supersample factor
S = 256 * SS


def draw(size_px):
    img = Image.new("RGBA", (size_px, size_px), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    s = size_px / 64.0  # glyph coordinates are in the tray icon's 64x64 space
    green = (31, 122, 58, 255)
    white = (255, 255, 255, 255)

    def rect(x0, y0, x1, y1, fill, radius=0):
        if radius:
            d.rounded_rectangle([x0 * s, y0 * s, x1 * s, y1 * s],
                                radius=radius * s, fill=fill)
        else:
            d.rectangle([x0 * s, y0 * s, x1 * s, y1 * s], fill=fill)

    rect(6, 6, 58, 58, green, radius=9)          # rounded square
    rect(22, 18, 42, 34, white)                  # plug body
    for px in (27, 32, 37):                      # pins (green holes)
        rect(px - 2, 24, px + 2, 28, green)
    rect(29, 34, 35, 48, white)                  # stem
    return img


def main():
    out_dir = os.path.join(os.path.dirname(__file__), "..", "assets")
    out_dir = os.path.abspath(out_dir)
    os.makedirs(out_dir, exist_ok=True)

    base = draw(S).resize((256, 256), Image.LANCZOS)
    base.save(os.path.join(out_dir, "com_port_watcher.png"))
    base.save(
        os.path.join(out_dir, "com_port_watcher.ico"),
        format="ICO",
        sizes=[(16, 16), (20, 20), (24, 24), (32, 32),
               (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    print("wrote", os.path.join(out_dir, "com_port_watcher.ico"))


if __name__ == "__main__":
    main()
