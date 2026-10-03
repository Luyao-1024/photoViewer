#!/usr/bin/env python3
"""Generate the PNG icons from the same artwork as the symbolic SVGs.

Two independent jobs live here, each rendering the artwork its way:

* The app icon is drawn directly with PIL, to avoid relying on rsvg-convert /
  ImageMagick SVG parsing (which is finicky for small hand-written SVGs). The
  artwork mirrors data/icons/photo-viewer-symbolic.svg at a 16x16 base; we
  scale up with high-quality resampling for 64 and 128 px outputs.
* The cloud sync badges are real SVG artwork, so they are rasterised with
  librsvg (through GObject introspection) and downsampled from a 4x render.
  They ship at BADGE_SIZE, not at the 18 px they are displayed at: the badge
  sits next to vector toolbar icons, and an 18 px bitmap was visibly soft on a
  HiDPI display. 72 px lets GTK filter it down for either scale factor.

Run from this directory: `cd data/icons && python3 _generate.py`.
"""
from PIL import Image, ImageDraw

BASE = 16
# Color palette from the SVG (RGB tuples)
FRAME_OUTER = (0x3a, 0x3a, 0x3a)
FRAME_INNER = (0xf0, 0xf0, 0xf0)
SUN = (0xff, 0xd7, 0x00)
MOUNTAIN = (0x5f, 0xb8, 0x78)

# Cloud sync badge: source SVG -> (luminance written into the LA PNG). The
# viewer picks the light or dark variant from the Adw style manager, so both
# have to exist and they must stay pixel-identical apart from that luminance.
BADGE_SIZE = 72
BADGE_SUPERSAMPLE = 4
BADGE_SOURCE_COLOR = "#222222"
CLOUD_BADGES = {
    "gnome-cloud-white.png": ("gnome-cloud-symbolic.svg", 0xFF),
    "gnome-cloud-off-white.png": ("gnome-cloud-off-symbolic.svg", 0xFF),
    "gnome-cloud-dark.png": ("gnome-cloud-symbolic.svg", 0x22),
    "gnome-cloud-off-dark.png": ("gnome-cloud-off-symbolic.svg", 0x22),
}


def render_badge(svg_name: str, luminance: int) -> Image.Image:
    """Rasterise a badge SVG into an LA image of BADGE_SIZE.

    librsvg renders the vector at 4x and PIL downsamples with LANCZOS, so the
    hairline stroke keeps a clean edge instead of the stepped one a direct
    small-size render gives.
    """
    import cairo
    import gi

    gi.require_version("Rsvg", "2.0")
    from gi.repository import Rsvg

    svg = open(svg_name, encoding="utf-8").read()
    svg = svg.replace(BADGE_SOURCE_COLOR, f"#{luminance:02x}{luminance:02x}{luminance:02x}")
    handle = Rsvg.Handle().new_from_data(svg.encode())

    big = BADGE_SIZE * BADGE_SUPERSAMPLE
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, big, big)
    ctx = cairo.Context(surface)
    viewport = Rsvg.Rectangle()
    viewport.x, viewport.y = 0.0, 0.0
    viewport.width, viewport.height = float(big), float(big)
    handle.render_document(ctx, viewport)
    surface.flush()
    big_img = Image.frombuffer(
        "RGBA", (big, big), surface.get_data(), "raw", "BGRA", 0, 1
    ).copy()
    img = big_img.resize((BADGE_SIZE, BADGE_SIZE), Image.LANCZOS)
    # LA keeps the shape in alpha and the tone in luminance, which is how the
    # existing badges are stored.
    return img.convert("LA")


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

    for out, (svg_name, luminance) in CLOUD_BADGES.items():
        render_badge(svg_name, luminance).save(out, "PNG")
        print(f"wrote {out}")


if __name__ == "__main__":
    main()