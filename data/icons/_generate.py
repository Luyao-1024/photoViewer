#!/usr/bin/env python3
"""Generate photo-viewer PNG icons from the same artwork as the symbolic SVG.

Renders directly with PIL to avoid relying on rsvg-convert / ImageMagick SVG
parsing (which is finicky for small hand-written SVGs). The artwork mirrors
data/icons/photo-viewer-symbolic.svg at a 16x16 base; we scale up with
high-quality resampling for 64 and 128 px outputs.
"""
from PIL import Image, ImageDraw

BASE = 16
# Color palette from the SVG (RGB tuples)
FRAME_OUTER = (0x3a, 0x3a, 0x3a)
FRAME_INNER = (0xf0, 0xf0, 0xf0)
SUN = (0xff, 0xd7, 0x00)
MOUNTAIN = (0x5f, 0xb8, 0x78)


def render(size: int) -> Image.Image:
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    s = size / BASE  # scale factor

    def px(x: float) -> float:
        return x * s

    # Outer frame: rounded rect from (1,2) to (15,14), radius 1
    # PIL rounded_rectangle expects integer radius; use size-aware rounding.
    radius = max(1, round(s * 1))
    draw.rounded_rectangle(
        (px(1), px(2), px(15), px(14)),
        radius=radius,
        fill=FRAME_OUTER,
    )
    # Inner photo area: (2.5, 3.5) to (13.5, 12.5)
    draw.rectangle(
        (px(2.5), px(3.5), px(13.5), px(12.5)),
        fill=FRAME_INNER,
    )
    # Sun: circle at (5,6) radius 1
    sun_r = px(1)
    draw.ellipse(
        (px(5) - sun_r, px(6) - sun_r, px(5) + sun_r, px(6) + sun_r),
        fill=SUN,
    )
    # Mountains: piecewise-linear ridge across the lower photo area.
    # Original SVG points: (2.5,11) (5.5,8) (8,10.5) (10.5,7.5) (13.5,11)
    # Drawn as a filled polygon that closes down to the photo bottom.
    ridge = [
        (px(2.5), px(11)),
        (px(5.5), px(8)),
        (px(8.0), px(10.5)),
        (px(10.5), px(7.5)),
        (px(13.5), px(11)),
        (px(13.5), px(12.5)),
        (px(2.5), px(12.5)),
    ]
    draw.polygon(ridge, fill=MOUNTAIN)

    return img


def main() -> None:
    for size in (64, 128):
        out = f"photo-viewer-{size}.png"
        render(size).save(out, "PNG")
        print(f"wrote {out}")


if __name__ == "__main__":
    main()