"""Hue-shift the Handy pink brand icons to Handyer burnt orange.

Anchor mapping: #faa2ca (hsl 333, 90%, 81%) -> #e2620e (hsl 24, 88%, 47%).
Only pixels in the pink/magenta hue band are touched; neutrals (white, black,
gray) and the monochrome tray template icons pass through untouched.
"""
import sys, os, colorsys, glob
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from png import PNG

SRC_H, SRC_L, SRC_S = 332.7 / 360.0, 0.808, 0.898
DST_H, DST_L, DST_S = 23.8 / 360.0, 0.471, 0.883

HUE_DELTA = (DST_H - SRC_H) % 1.0
SAT_MUL = DST_S / SRC_S
# Pink band in normalised hue: 280deg .. 360deg
BAND_LO, BAND_HI = 280 / 360.0, 1.0


def map_l(l):
    """Piecewise linear through (0,0), (SRC_L, DST_L), (1,1)."""
    if l <= SRC_L:
        return l * (DST_L / SRC_L)
    return DST_L + (l - SRC_L) * (1.0 - DST_L) / (1.0 - SRC_L)


_cache = {}


def convert(r, g, b):
    key = (r, g, b)
    hit = _cache.get(key)
    if hit is not None:
        return hit
    mx, mn = max(r, g, b), min(r, g, b)
    if mx - mn <= 6:  # neutral: leave exactly as-is
        _cache[key] = key
        return key
    h, l, s = colorsys.rgb_to_hls(r / 255.0, g / 255.0, b / 255.0)
    if not (BAND_LO <= h < BAND_HI):  # not pink/magenta: leave alone
        _cache[key] = key
        return key
    h2 = (h + HUE_DELTA) % 1.0
    s2 = min(1.0, s * SAT_MUL)
    l2 = map_l(l)
    r2, g2, b2 = colorsys.hls_to_rgb(h2, l2, s2)
    out = (round(r2 * 255), round(g2 * 255), round(b2 * 255))
    _cache[key] = out
    return out


def process(path, out_path):
    im = PNG(path)
    if im.ctype in (0, 4):  # grayscale: no chroma to shift
        return "skip(gray)"
    changed = 0
    if im.ctype == 3:  # palette: remap PLTE only
        p = bytearray(im.plte)
        for i in range(0, len(p), 3):
            n = convert(p[i], p[i + 1], p[i + 2])
            if n != (p[i], p[i + 1], p[i + 2]):
                changed += 1
            p[i], p[i + 1], p[i + 2] = n
        im.plte = p
    else:
        px = im.pixels
        n = im.nch
        for i in range(0, len(px) - n + 1, n):
            if n == 4 and px[i + 3] == 0:
                continue
            new = convert(px[i], px[i + 1], px[i + 2])
            if new != (px[i], px[i + 1], px[i + 2]):
                px[i], px[i + 1], px[i + 2] = new
                changed += 1
    if not changed:
        return "skip(no pink)"
    im.write(out_path)
    return f"recolored ({changed} entries)"


if __name__ == "__main__":
    root, outdir = sys.argv[1], sys.argv[2]
    targets = sorted(glob.glob(root + "/src-tauri/icons/**/*.png", recursive=True))
    targets += [
        root + "/src-tauri/resources/" + f
        for f in ("handy.png", "handy_warning.png", "recording.png", "transcribing.png")
    ]
    for p in targets:
        rel = os.path.relpath(p, root)
        dst = os.path.join(outdir, rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        print(f"{rel:60s} {process(p, dst)}", flush=True)
