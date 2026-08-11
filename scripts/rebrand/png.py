"""Minimal pure-Python PNG read/write (non-interlaced) + HSL helpers."""
import struct, zlib, colorsys

def _chunks(data):
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a png"
    i = 8
    out = []
    while i < len(data):
        (ln,) = struct.unpack(">I", data[i:i+4])
        typ = data[i+4:i+8]
        body = data[i+8:i+8+ln]
        out.append((typ, body))
        i += 12 + ln
    return out

def _pack(chunks):
    out = b"\x89PNG\r\n\x1a\n"
    for typ, body in chunks:
        out += struct.pack(">I", len(body)) + typ + body
        out += struct.pack(">I", zlib.crc32(typ + body) & 0xFFFFFFFF)
    return out

def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p-a), abs(p-b), abs(p-c)
    if pa <= pb and pa <= pc: return a
    if pb <= pc: return b
    return c

def _unfilter(raw, w, h, bpp, stride):
    out = bytearray()
    prev = bytearray(stride)
    i = 0
    for _ in range(h):
        ft = raw[i]; i += 1
        line = bytearray(raw[i:i+stride]); i += stride
        if ft == 1:
            for x in range(bpp, stride): line[x] = (line[x] + line[x-bpp]) & 255
        elif ft == 2:
            for x in range(stride): line[x] = (line[x] + prev[x]) & 255
        elif ft == 3:
            for x in range(stride):
                a = line[x-bpp] if x >= bpp else 0
                line[x] = (line[x] + ((a + prev[x]) >> 1)) & 255
        elif ft == 4:
            for x in range(stride):
                a = line[x-bpp] if x >= bpp else 0
                c = prev[x-bpp] if x >= bpp else 0
                line[x] = (line[x] + _paeth(a, prev[x], c)) & 255
        out += line
        prev = line
    return out

class PNG:
    def __init__(self, path):
        self.path = path
        data = open(path, "rb").read()
        self.chunks = _chunks(data)
        ihdr = dict(self.chunks)[b"IHDR"]
        (self.w, self.h, self.depth, self.ctype, _, _, self.interlace) = struct.unpack(">IIBBBBB", ihdr)
        assert self.interlace == 0, f"{path}: interlaced unsupported"
        assert self.depth == 8, f"{path}: bit depth {self.depth} unsupported"
        self.nch = {0:1, 2:3, 3:1, 4:2, 6:4}[self.ctype]
        idat = b"".join(b for t, b in self.chunks if t == b"IDAT")
        raw = zlib.decompress(idat)
        bpp = self.nch
        self.stride = self.w * bpp
        self.pixels = _unfilter(raw, self.w, self.h, bpp, self.stride)
        self.plte = dict(self.chunks).get(b"PLTE")

    def write(self, path):
        raw = bytearray()
        for y in range(self.h):
            raw.append(0)
            raw += self.pixels[y*self.stride:(y+1)*self.stride]
        comp = zlib.compress(bytes(raw), 9)
        new = []
        seen_idat = False
        for typ, body in self.chunks:
            if typ == b"IDAT":
                if not seen_idat:
                    new.append((b"IDAT", comp)); seen_idat = True
                continue
            if typ == b"PLTE" and self.plte is not None:
                new.append((b"PLTE", bytes(self.plte))); continue
            new.append((typ, body))
        open(path, "wb").write(_pack(new))
