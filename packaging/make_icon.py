"""Generate PlaneUI's macOS app icon with Pillow."""

from pathlib import Path

from PIL import Image, ImageDraw

SIZE = 1024
SCALE = 2
image = Image.new("RGBA", (SIZE * SCALE, SIZE * SCALE))
mask = Image.new("L", image.size)
ImageDraw.Draw(mask).rounded_rectangle(
    (100 * SCALE, 100 * SCALE, 924 * SCALE - 1, 924 * SCALE - 1),
    radius=185 * SCALE,
    fill=255,
)
draw = ImageDraw.Draw(image)
for y in range(image.height):
    progress = min(1, max(0, (y / SCALE - 100) / 824))
    color = tuple(
        round(top + (bottom - top) * progress)
        for top, bottom in zip((75, 133, 194), (30, 64, 110))
    )
    draw.line((0, y, image.width, y), fill=(*color, 255))
image.putalpha(mask)
draw = ImageDraw.Draw(image)
for rectangle in ((256, 270, 486, 754), (518, 270, 768, 496), (518, 528, 768, 754)):
    draw.rounded_rectangle(
        tuple(coordinate * SCALE for coordinate in rectangle),
        radius=28 * SCALE,
        fill=(243, 248, 255, 255),
    )
# cargo-packager only accepts a 1024 px PNG named as the @2x of the 512 px one.
for filename, size in (
    ("icon@2x.png", SIZE),
    ("icon-512.png", 512),
):
    image.resize((size, size), Image.Resampling.LANCZOS).save(
        Path(__file__).with_name(filename)
    )
