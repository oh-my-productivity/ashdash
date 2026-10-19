#!/usr/bin/env python3
"""Pixel check of the launcher-look captures: the window edge must be our own border colour (or transparent), never light.

    edge-check.py [--max-lum N] [--report FILE] SHOTS_DIR      check every <state>-<n>.png and <state>-with-shadow.png
    edge-check.py --selftest                                   a few synthetic images (also what CI runs first)

Why: on macOS 26 the owner saw a white border around the panel (docs/design/launcher-look.md, "The white border"). The only edge the
launcher may have is the theme border (1 device pixel, #242628 by default). Two kinds of capture are checked:

* `<state>-<n>.png`, `screencapture -o` (no shadow): the outermost ring of pixels (all four edges, corners included) must be fully
  transparent or have a luminance (composited over black) of at most N (default 80); the second ring is reported.
* `<state>-with-shadow.png` (the window as the compositor draws it): the window is the box of opaque pixels. Its outermost two rings
  must equal the window-only capture (a step of more than 12 per channel means the compositor drew something over the edge: on
  macOS 26 a light "Liquid Glass" rim) and not be light; the 1 pixel ring just OUTSIDE the border, corners included, must be dark or
  transparent too (a shadow is dark; a halo is not).

Pure Python 3 (no Pillow): the CI runners do not promise it. Reads 8-bit RGB / RGBA / gray / gray+alpha PNGs, not interlaced.
Exit status: 0 all clear, 1 a light edge pixel (or no capture at all), 2 usage / unreadable image.
"""

import glob
import os
import re
import struct
import sys
import tempfile
import zlib

MAX_LUM_DEFAULT = 80


class Image:
    """`rows[y]` is `bytes` of RGBA quadruples, left to right."""

    def __init__(self, width, height, rows):
        self.w, self.h, self.rows = width, height, rows

    def px(self, x, y):
        i = 4 * x
        r = self.rows[y]
        return r[i], r[i + 1], r[i + 2], r[i + 3]


def read_png(path):
    with open(path, "rb") as f:
        data = f.read()
    return decode_png(data, path)


def decode_png(data, name="<memory>"):
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{name}: not a PNG")
    pos, idat, hdr = 8, [], None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            hdr = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat.append(body)
        elif kind == b"IEND":
            break
    if hdr is None:
        raise ValueError(f"{name}: no IHDR")
    w, h, depth, ctype, _, _, interlace = hdr
    if depth != 8 or interlace != 0 or ctype not in (0, 2, 4, 6):
        raise ValueError(f"{name}: unsupported PNG (depth {depth}, colour type {ctype}, interlace {interlace})")
    bpp = {0: 1, 2: 3, 4: 2, 6: 4}[ctype]
    raw = zlib.decompress(b"".join(idat))
    stride = w * bpp
    rows, prev = [], bytearray(stride)
    p = 0
    for _ in range(h):
        ft = raw[p]
        cur = bytearray(raw[p + 1 : p + 1 + stride])
        p += 1 + stride
        if ft == 1:
            for i in range(bpp, stride):
                cur[i] = (cur[i] + cur[i - bpp]) & 255
        elif ft == 2:
            for i in range(stride):
                cur[i] = (cur[i] + prev[i]) & 255
        elif ft == 3:
            for i in range(stride):
                left = cur[i - bpp] if i >= bpp else 0
                cur[i] = (cur[i] + ((left + prev[i]) >> 1)) & 255
        elif ft == 4:
            for i in range(stride):
                a = cur[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                cur[i] = (cur[i] + pr) & 255
        elif ft != 0:
            raise ValueError(f"{name}: bad PNG filter {ft}")
        prev = cur
        out = bytearray(4 * w)
        if ctype == 6:
            out[:] = cur
        elif ctype == 2:
            for x in range(w):
                out[4 * x : 4 * x + 3] = cur[3 * x : 3 * x + 3]
                out[4 * x + 3] = 255
        elif ctype == 0:
            for x in range(w):
                out[4 * x : 4 * x + 3] = bytes((cur[x],)) * 3
                out[4 * x + 3] = 255
        else:
            for x in range(w):
                out[4 * x : 4 * x + 3] = bytes((cur[2 * x],)) * 3
                out[4 * x + 3] = cur[2 * x + 1]
        rows.append(bytes(out))
    return Image(w, h, rows)


def write_png(path, width, height, pixel):
    """Test helper: RGBA PNG, `pixel(x, y)` -> (r, g, b, a)."""
    raw = bytearray()
    for y in range(height):
        raw.append(0)
        for x in range(width):
            raw += bytes(pixel(x, y))

    def chunk(kind, body):
        c = struct.pack(">I", len(body)) + kind + body
        return c + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(bytes(raw))))
        f.write(chunk(b"IEND", b""))


def lum(p):
    """Perceived brightness 0..255 of an RGBA pixel composited over black (a transparent pixel is black)."""
    r, g, b, a = p
    return (0.299 * r + 0.587 * g + 0.114 * b) * a / 255.0


def ring(img, x0, y0, x1, y1):
    """Coordinates of the one-pixel ring of the rectangle x0..=x1, y0..=y1, clipped to the image."""
    pts = []
    for x in range(x0, x1 + 1):
        pts += [(x, y0), (x, y1)]
    for y in range(y0 + 1, y1):
        pts += [(x0, y), (x1, y)]
    return [(x, y) for (x, y) in pts if 0 <= x < img.w and 0 <= y < img.h]


def worst(img, pts):
    """(brightest luminance, its (x, y), its pixel) over `pts`; (0, None, None) when empty."""
    best = (0.0, None, None)
    for x, y in pts:
        p = img.px(x, y)
        v = lum(p)
        if v > best[0]:
            best = (v, (x, y), p)
    return best


def describe(pts_img, pts):
    from collections import Counter

    c = Counter(pts_img.px(x, y) for x, y in pts)
    return ", ".join(f"{n}x rgba{p}" for p, n in c.most_common(3))


def opaque_box(img, min_alpha=250):
    """(x0, y0, x1, y1), inclusive, of the pixels that are (nearly) opaque: the window inside a capture that has a shadow margin."""
    xs, ys = [], []
    for y in range(img.h):
        row = img.rows[y]
        hit = [x for x in range(img.w) if row[4 * x + 3] >= min_alpha]
        if hit:
            xs += [hit[0], hit[-1]]
            ys.append(y)
    if not xs:
        return None
    return min(xs), ys[0], max(xs), ys[-1]


def check_plain(path, max_lum, out):
    img = read_png(path)
    bad = 0
    r0 = ring(img, 0, 0, img.w - 1, img.h - 1)
    r1 = ring(img, 1, 1, img.w - 2, img.h - 2)
    v0, at0, p0 = worst(img, r0)
    v1, at1, p1 = worst(img, r1)
    name = os.path.basename(path)
    out.append(f"{name} {img.w}x{img.h}: outer ring [{describe(img, r0)}] brightest {v0:.0f}; second ring [{describe(img, r1)}] brightest {v1:.0f}")
    corners = [(0, 0), (img.w - 1, 0), (0, img.h - 1), (img.w - 1, img.h - 1)]
    out.append(f"  corners: {[img.px(x, y) for x, y in corners]}")
    if v0 > max_lum:
        out.append(f"  FAIL {name}: light pixel in the outermost ring at {at0}: rgba{p0} (luminance {v0:.0f} > {max_lum})")
        bad += 1
    return bad, img


MAX_STEP = 12  # per channel: how far a composited edge pixel may differ from the same pixel of the window-only capture


def check_shadow(path, ref, max_lum, out):
    """The capture WITH the window as the compositor draws it (shadow, and on macOS 26 the system's edge rim).

    The window is found as the box of opaque pixels. Its outermost two rings must equal the window-only capture `ref` (within
    MAX_STEP per channel: a system rim adds light there) and not be light; the ring just outside must be dark or transparent."""
    img = read_png(path)
    name = os.path.basename(path)
    bad = 0
    out.append(f"{name} {img.w}x{img.h} (window {ref.w}x{ref.h})")
    v, at, p = worst(img, ring(img, 0, 0, img.w - 1, img.h - 1))
    out.append(f"  capture's own outer ring: brightest {v:.0f}")
    if v > max_lum:
        out.append(f"  FAIL {name}: light pixel on the capture's outer ring at {at}: rgba{p}")
        bad += 1
    box = opaque_box(img)
    if box is None:
        out.append(f"  FAIL {name}: no opaque pixel: the window is not in the capture")
        return bad + 1
    x0, y0, x1, y1 = box
    if (x1 - x0 + 1, y1 - y0 + 1) != (ref.w, ref.h):
        out.append(f"  FAIL {name}: the opaque box is {x1 - x0 + 1}x{y1 - y0 + 1} but the window-only capture is {ref.w}x{ref.h}")
        return bad + 1
    out.append(f"  window at +{x0},+{y0}; margins left {x0}, right {img.w - 1 - x1}, top {y0}, bottom {img.h - 1 - y1}")
    for depth in (0, 1):
        got = ring(img, x0 + depth, y0 + depth, x1 - depth, y1 - depth)
        step, at, gp = 0, None, None
        for x, y in got:
            e = ref.px(x - x0, y - y0)
            g = img.px(x, y)
            d = max(abs(e[i] - g[i]) for i in range(3))
            if d > step:
                step, at, gp = d, (x - x0, y - y0), g
        v, vat, vp = worst(img, got)
        what = "outermost" if depth == 0 else "second"
        out.append(f"  {what} ring of the window as composited: [{describe(img, got)}] brightest {v:.0f}, largest step from the window-only capture {step}")
        if step > MAX_STEP:
            out.append(
                f"  FAIL {name}: the compositor lightened the {what} ring: window pixel {at} is rgba{gp}, the window-only capture has "
                f"rgba{ref.px(at[0], at[1])} (step {step} > {MAX_STEP}): a system rim"
            )
            bad += 1
        if v > max_lum:
            out.append(f"  FAIL {name}: light pixel in the {what} ring at {vat}: rgba{vp} (luminance {v:.0f} > {max_lum})")
            bad += 1
    outside = ring(img, x0 - 1, y0 - 1, x1 + 1, y1 + 1)
    v, at, p = worst(img, outside)
    out.append(f"  1 px outside the border (corners included): [{describe(img, outside)}] brightest {v:.0f}")
    if v > max_lum:
        out.append(f"  FAIL {name}: light pixel just outside the border at {at}: rgba{p} (luminance {v:.0f} > {max_lum})")
        bad += 1
    return bad


def check_dir(shots, max_lum, out):
    bad, seen = 0, 0
    states = sorted({re.sub(r"-(\d+|with-shadow|screen)\.png$", "", os.path.basename(f)) for f in glob.glob(os.path.join(shots, "*.png"))})
    for state in states:
        plain = sorted(glob.glob(os.path.join(shots, f"{state}-[0-9].png")))
        first = None
        for f in plain:
            b, img = check_plain(f, max_lum, out)
            bad += b
            seen += 1
            first = first or img
        shadow = os.path.join(shots, f"{state}-with-shadow.png")
        if os.path.exists(shadow):
            seen += 1
            if first is None:
                out.append(f"FAIL {state}: with-shadow capture but no plain capture to locate the window")
                bad += 1
            else:
                bad += check_shadow(shadow, first, max_lum, out)
    if seen == 0:
        out.append(f"FAIL: no captures in {shots}")
        bad += 1
    return bad


def selftest():
    d = tempfile.mkdtemp(prefix="edge-check.")
    border, fill = (36, 38, 40, 255), (6, 9, 11, 255)

    def window(x, y, w=40, h=20, rim=None):
        edge = x in (0, w - 1) or y in (0, h - 1)
        return border if edge else fill

    def plain(rim):
        def px(x, y):
            if rim and (x in (0, 39) or y in (0, 19)):
                return rim
            return window(x, y)

        return px

    def shadowed(margin, rim_outside, inner=window):
        def px(x, y):
            wx, wy = x - margin, y - margin
            if 0 <= wx < 40 and 0 <= wy < 20:
                return inner(wx, wy)
            if rim_outside and -1 <= wx <= 40 and -1 <= wy <= 20:
                return rim_outside
            return (0, 0, 0, 40)

        return px

    def run(name, plain_px, shadow_px, expect_bad):
        sub = os.path.join(d, name)
        os.makedirs(sub)
        write_png(os.path.join(sub, "empty-1.png"), 40, 20, plain_px)
        write_png(os.path.join(sub, "empty-with-shadow.png"), 40 + 20, 20 + 20, shadow_px)
        out = []
        bad = check_dir(sub, MAX_LUM_DEFAULT, out)
        assert (bad > 0) == expect_bad, f"{name}: bad={bad}\n" + "\n".join(out)

    run("clean", plain(None), shadowed(10, None), False)
    run("white-edge", plain((250, 250, 250, 255)), shadowed(10, None, plain((250, 250, 250, 255))), True)
    run("white-halo", plain(None), shadowed(10, (240, 240, 240, 200)), True)
    run("system-rim", plain(None), shadowed(10, None, lambda x, y: (58, 60, 61, 255) if x in (0, 39) or y in (0, 19) else window(x, y)), True)
    sub = os.path.join(d, "empty-dir")
    os.makedirs(sub)
    assert check_dir(sub, MAX_LUM_DEFAULT, []) == 1
    print("edge-check: selftest ok")


def main(argv):
    max_lum, report, args = MAX_LUM_DEFAULT, None, []
    it = iter(argv)
    for a in it:
        if a == "--selftest":
            selftest()
            return 0
        if a == "--max-lum":
            max_lum = float(next(it))
        elif a == "--report":
            report = next(it)
        elif a.startswith("-"):
            print(__doc__, file=sys.stderr)
            return 2
        else:
            args.append(a)
    if len(args) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    out = []
    try:
        bad = check_dir(args[0], max_lum, out)
    except (ValueError, OSError, zlib.error) as e:
        print(f"edge-check: {e}", file=sys.stderr)
        return 2
    out.append(f"edge-check: {'FAILED: ' + str(bad) + ' problem(s)' if bad else 'ok'} (max luminance {max_lum:g})")
    text = "\n".join(out)
    print(text)
    if report:
        with open(report, "w") as f:
            f.write(text + "\n")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
