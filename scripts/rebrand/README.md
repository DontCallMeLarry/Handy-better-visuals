# Rebrand recolor toolkit

Hue-shifts the upstream pink brand assets (`#faa2ca`) to Handyer burnt orange
(`#e2620e`). Pure Python 3 stdlib — no PIL/ImageMagick. Only pixels in the
pink/magenta hue band are touched; neutrals (the white hand, black outline)
and the monochrome tray template icons pass through byte-identical.

Re-run after merging upstream whenever icon art changes:

```bash
OUT=$(mktemp -d)
python3 scripts/rebrand/recolor.py . "$OUT"      # all icons/**/*.png + resources/*.png
python3 scripts/rebrand/containers.py . "$OUT"   # PNGs inside icon.icns / icon.ico
rsync -a "$OUT"/src-tauri/ src-tauri/
```

- `png.py` — minimal PNG codec (unfilter/refilter, palette support)
- `recolor.py` — the HSL band transform; anchor mapping and lightness curve at the top
- `containers.py` — rewrites the PNG sub-images inside `.icns`/`.ico`, fixing offsets

Legacy `is32`/`il32` icns entries (pre-10.7 RLE24) are passed through; the app's
minimum is 10.15. Hex accents in TSX/CSS are not covered here — grep for the old
hex if upstream adds new ones (`theme.css` is the palette source of truth).
