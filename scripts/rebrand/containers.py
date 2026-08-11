"""Recolor the PNG sub-images inside icon.icns and icon.ico.

Legacy icns entries (is32/il32/s8mk/l8mk — RLE24 + masks, unused on macOS 10.7+)
are passed through untouched; the app's minimum is 10.15.
"""
import struct, sys, os, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import recolor

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


def recolor_png_bytes(body):
    if body[:8] != PNG_MAGIC:
        return body, False
    with tempfile.TemporaryDirectory() as td:
        src, dst = os.path.join(td, "i.png"), os.path.join(td, "o.png")
        open(src, "wb").write(body)
        res = recolor.process(src, dst)
        if res.startswith("skip") or not os.path.exists(dst):
            return body, False
        return open(dst, "rb").read(), True


def do_icns(path, out):
    d = open(path, "rb").read()
    assert d[:4] == b"icns"
    parts, i, n = [], 8, 0
    while i < len(d):
        t = d[i:i + 4]
        (ln,) = struct.unpack(">I", d[i + 4:i + 8])
        body = d[i + 8:i + ln]
        new, ch = recolor_png_bytes(body)
        n += ch
        parts.append(t + struct.pack(">I", len(new) + 8) + new)
        i += ln
    blob = b"".join(parts)
    open(out, "wb").write(b"icns" + struct.pack(">I", len(blob) + 8) + blob)
    return n


def do_ico(path, out):
    d = open(path, "rb").read()
    res, typ, cnt = struct.unpack("<HHH", d[:6])
    entries, bodies, n = [], [], 0
    for k in range(cnt):
        o = 6 + k * 16
        hdr = bytearray(d[o:o + 16])
        sz, off = struct.unpack("<II", d[o + 8:o + 16])
        new, ch = recolor_png_bytes(d[off:off + sz])
        n += ch
        entries.append(hdr)
        bodies.append(new)
    off = 6 + cnt * 16
    for hdr, body in zip(entries, bodies):
        struct.pack_into("<II", hdr, 8, len(body), off)
        off += len(body)
    out_b = struct.pack("<HHH", res, typ, cnt) + b"".join(bytes(h) for h in entries) + b"".join(bodies)
    open(out, "wb").write(out_b)
    return n


if __name__ == "__main__":
    root, outdir = sys.argv[1], sys.argv[2]
    os.makedirs(os.path.join(outdir, "src-tauri/icons"), exist_ok=True)
    print("icon.icns: recolored", do_icns(root + "/src-tauri/icons/icon.icns",
                                          outdir + "/src-tauri/icons/icon.icns"), "sub-images")
    print("icon.ico:  recolored", do_ico(root + "/src-tauri/icons/icon.ico",
                                         outdir + "/src-tauri/icons/icon.ico"), "sub-images")
