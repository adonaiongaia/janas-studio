#!/usr/bin/env python3
"""Generate the Janas Studio app icon without external image libraries.

Draws a rounded dark tile with two acid-green gate pillars on it. The output is
a 1024x1024 RGBA PNG that `tauri icon` expands into every platform size.
"""

import math
import struct
import sys
import zlib

SIZE = 1024
OUT = sys.argv[1] if len(sys.argv) > 1 else "src-tauri/icons/icon.png"


def _chunk(kind: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + kind
        + payload
        + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
    )


def rounded_alpha(x: int, y: int) -> int:
    radius = 190.0
    cx = min(max(x, radius), SIZE - radius)
    cy = min(max(y, radius), SIZE - radius)
    distance = math.hypot(x - cx, y - cy)
    if distance <= radius:
        return 255
    if distance <= radius + 1.5:
        return int(255 * (radius + 1.5 - distance) / 1.5)
    return 0


def inside_rect(x, y, left, top, right, bottom, radius=26.0):
    cx = min(max(x, left + radius), right - radius)
    cy = min(max(y, top + radius), bottom - radius)
    return math.hypot(x - cx, y - cy) <= radius


def shade(x, y, top, bottom):
    t = y / SIZE
    return tuple(int(top[i] + (bottom[i] - top[i]) * t) for i in range(3))


pixels = bytearray()
# Vertical background gradient, slightly lighter at the top.
bg_top = (22, 27, 36)
bg_bottom = (8, 9, 12)
accent_top = (226, 255, 143)
accent_bottom = (143, 191, 34)
glyph_left = 300
glyph_right = 724
glyph_top = 278
glyph_bottom = 746
bar = 58

for y in range(SIZE):
    pixels.append(0)  # PNG filter: none
    for x in range(SIZE):
        r, g, b = shade(x, y, bg_top, bg_bottom)

        # A soft halo behind the mark.
        halo = math.hypot(x - SIZE / 2, y - SIZE / 2)
        if halo < 440:
            strength = (1.0 - halo / 440.0) ** 2 * 0.14
            r = int(r + (205 - r) * strength)
            g = int(g + (247 - g) * strength)
            b = int(b + (90 - b) * strength)

        mark = False
        # Two pillars.
        if inside_rect(x, y, glyph_left, glyph_top, glyph_left + bar, glyph_bottom):
            mark = True
        if inside_rect(x, y, glyph_right - bar, glyph_top, glyph_right, glyph_bottom):
            mark = True
        # Top lintel and lower bridge: a gate.
        if inside_rect(x, y, glyph_left, glyph_top, glyph_right, glyph_top + bar):
            mark = True
        if inside_rect(x, y, glyph_left, 606, glyph_right, 606 + bar):
            mark = True
        # A dot in the doorway.
        if math.hypot(x - SIZE / 2, y - 645) < 42:
            mark = True

        if mark:
            r, g, b = shade(x, y, accent_top, accent_bottom)

        pixels.extend((r, g, b, rounded_alpha(x, y)))

raw = b"".join(
    [
        b"\x89PNG\r\n\x1a\n",
        _chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)),
        _chunk(b"IDAT", zlib.compress(bytes(pixels), 9)),
        _chunk(b"IEND", b""),
    ]
)

with open(OUT, "wb") as handle:
    handle.write(raw)
print(f"wrote {OUT} ({len(raw)} bytes)")
