"""Draws 8gent's app icon: kr8's rainbow "8" (two rings under its ten-stop gradient) and "gent" in
white on a near-black rounded square. Writes src-tauri/icons/source.png (1024 px); `npx tauri icon`
then makes every size from it.   python3 tools/make_icon.py
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

SS = 4                      # draw at 4x, then scale down, for smooth edges
SIZE = 1024 * SS
STOPS = [(0, "#fb3e09"), (0.14, "#fb9401"), (0.28, "#f7c602"), (0.42, "#b6d001"), (0.54, "#57c730"),
         (0.66, "#1ab88b"), (0.76, "#10b2bc"), (0.86, "#0596f6"), (0.94, "#5041f7"), (1, "#911fe3")]
FONT = "/System/Library/Fonts/Supplemental/Arial Rounded Bold.ttf"


def rgb(h):
    return tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))


def gradient_at(t):
    t = max(0.0, min(1.0, t))
    for (a, ca), (b, cb) in zip(STOPS, STOPS[1:]):
        if t <= b:
            k = (t - a) / (b - a) if b > a else 0
            return tuple(round(x + (y - x) * k) for x, y in zip(rgb(ca), rgb(cb)))
    return rgb(STOPS[-1][1])


def main():
    icon = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(icon)
    # macOS icon shape: an 824 px square on the 1024 grid, corner radius about 185
    m, r = 100 * SS, 185 * SS
    draw.rounded_rectangle([m, m, SIZE - m, SIZE - m], radius=r, fill=(10, 10, 14, 255))

    glyph_h = 300 * SS                       # the "8" is 20 x 34 units
    u = glyph_h / 34
    font = ImageFont.truetype(FONT, int(238 * SS))
    text_w = draw.textlength("gent", font=font)
    gap = 10 * SS
    total = 20 * u + gap + text_w
    x0 = (SIZE - total) / 2
    y0 = (SIZE - glyph_h) / 2 - 6 * SS

    # the two rings as a mask, then the gradient painted through it along the tilted axis (4,0)->(16,34)
    mask = Image.new("L", (SIZE, SIZE), 0)
    md = ImageDraw.Draw(mask)
    for cy, radius, stroke in ((9.4, 6.9, 5.0), (23.8, 7.8, 4.4)):
        cx_px, cy_px = x0 + 10 * u, y0 + cy * u
        outer, inner = (radius + stroke / 2) * u, (radius - stroke / 2) * u
        md.ellipse([cx_px - outer, cy_px - outer, cx_px + outer, cy_px + outer], fill=255)
        md.ellipse([cx_px - inner, cy_px - inner, cx_px + inner, cy_px + inner], fill=0)
    # the second ring's hole must not erase the first ring where they overlap: redraw ring 1's band
    ring1 = Image.new("L", (SIZE, SIZE), 0)
    rd = ImageDraw.Draw(ring1)
    cx_px, cy_px = x0 + 10 * u, y0 + 9.4 * u
    rd.ellipse([cx_px - 9.4 * u, cy_px - 9.4 * u, cx_px + 9.4 * u, cy_px + 9.4 * u], fill=255)
    rd.ellipse([cx_px - 4.4 * u, cy_px - 4.4 * u, cx_px + 4.4 * u, cy_px + 4.4 * u], fill=0)
    mask.paste(255, (0, 0), ring1)

    ax, ay, bx, by = 4, 0, 16, 34
    length2 = (bx - ax) ** 2 + (by - ay) ** 2
    strip = Image.new("RGB", (int(20 * u) + 2, int(glyph_h) + 2))
    px = strip.load()
    for yy in range(strip.height):
        for xx in range(strip.width):
            gx, gy = xx / u, yy / u
            px[xx, yy] = gradient_at(((gx - ax) * (bx - ax) + (gy - ay) * (by - ay)) / length2)
    color = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    color.paste(strip, (int(x0), int(y0)))
    icon = Image.composite(color, icon, mask)

    draw = ImageDraw.Draw(icon)
    # the text sits on the baseline the "8" stands on
    draw.text((x0 + 20 * u + gap, y0 + glyph_h), "gent", font=font, fill=(255, 255, 255, 255), anchor="ls")

    out = Path(__file__).resolve().parent.parent / "src-tauri" / "icons" / "source.png"
    icon.resize((1024, 1024), Image.LANCZOS).save(out)
    print(out)


if __name__ == "__main__":
    main()
