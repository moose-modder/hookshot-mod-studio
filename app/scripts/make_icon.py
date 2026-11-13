"""Draws the app's icon (a hook on a dark tile) as a 1024x1024 PNG. Standard library only.

    python app/scripts/make_icon.py app/src-tauri/icons/source.png
    cd app && npx tauri icon src-tauri/icons/source.png
"""

import math
import struct
import sys
import zlib

SIZE = 1024
TILE = (24, 34, 43)
HOOK = (233, 185, 73)


def rounded_rect(x, y, x0, y0, x1, y1, r):
    cx, cy = min(max(x, x0 + r), x1 - r), min(max(y, y0 + r), y1 - r)
    return math.hypot(x - cx, y - cy) - r


def capsule(x, y, ax, ay, bx, by, r):
    dx, dy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(x - (ax + t * dx), y - (ay + t * dy)) - r


def arc(x, y, cx, cy, radius, half_width, a0, a1):
    """A ring segment from angle a0 to a1 (radians, y down), with round ends."""
    a = math.atan2(y - cy, x - cx) % (2 * math.pi)
    if a0 <= a <= a1:
        return abs(math.hypot(x - cx, y - cy) - radius) - half_width
    ends = [(cx + radius * math.cos(b), cy + radius * math.sin(b)) for b in (a0, a1)]
    return min(math.hypot(x - ex, y - ey) for ex, ey in ends) - half_width


def hook(x, y):
    shaft = capsule(x, y, 610, 250, 610, 560, 46)
    bend = arc(x, y, 470, 560, 140, 46, 0.0, math.radians(200))
    eye = abs(math.hypot(x - 610, y - 190) - 62) - 22
    return min(shaft, bend, eye)


def coverage(d):
    return max(0.0, min(1.0, 0.5 - d))


def main(out):
    rows = []
    for y in range(SIZE):
        row = bytearray([0])
        for x in range(SIZE):
            px, py = x + 0.5, y + 0.5
            tile = coverage(rounded_rect(px, py, 48, 48, 976, 976, 210))
            h = coverage(hook(px, py)) * tile
            rgb = [TILE[c] * (1 - h) + HOOK[c] * h for c in range(3)]
            row += bytes([round(v) for v in rgb] + [round(255 * tile)])
        rows.append(bytes(row))
    raw = zlib.compress(b"".join(rows), 9)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    png = (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
           + chunk(b"IDAT", raw) + chunk(b"IEND", b""))
    with open(out, "wb") as f:
        f.write(png)


if __name__ == "__main__":
    main(sys.argv[1])
