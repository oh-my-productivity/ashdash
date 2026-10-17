#!/usr/bin/env python3
"""Sample colour tokens from the V4 sidebar mockups (general-dark.jpg / general-light.jpg).

Regions are given in the coordinate space of the 2212x1659 view that the designer looked at;
they are scaled to the real pixel size. For "bg" regions the value is the per-channel median.
For "ink" regions the value is the per-channel median of the pixels that are at least half as far
from the region's median as the farthest pixel (the text / line core; damps JPEG overshoot).

Usage: python3 sample_tokens.py            (prints a table, writes sampled.json)
Requires Pillow (pip install pillow).
"""
import json
import os
import statistics
import sys

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
VIEW_W, VIEW_H = 2212.0, 1659.0

# name: (kind, (x0, y0, x1, y1)) in view coordinates
REGIONS = {
    "backdrop (outside window)": ("bg", (10, 1620, 70, 1650)),
    "title bar": ("bg", (300, 110, 900, 130)),
    "sidebar bg": ("bg", (150, 1000, 450, 1300)),
    "content bg (header)": ("bg", (1250, 170, 1400, 215)),
    "row bg, label column": ("bg", (700, 900, 900, 960)),
    "row bg, value column": ("bg", (1700, 880, 1900, 960)),
    "search field bg": ("bg", (345, 190, 395, 225)),
    "control fill (dropdown)": ("bg", (1058, 560, 1078, 605)),
    "control fill (button)": ("bg", (1056, 1112, 1076, 1155)),
    "stepper value cell": ("bg", (1160, 770, 1240, 785)),
    "status strip bg": ("bg", (1000, 1560, 1150, 1580)),
    "card bg (reload)": ("bg", (900, 1360, 1000, 1380)),
    "toggle track (on)": ("bg", (1062, 880, 1100, 900)),
    "toggle knob": ("bg", (1124, 882, 1142, 900)),
    "recorder outline": ("ink", (1040, 400, 1060, 430)),
    "active nav bar": ("bg", (85, 285, 90, 330)),
    "text: page title": ("ink", (555, 195, 770, 235)),
    "text: row label": ("ink", (585, 375, 890, 395)),
    "text: nav active": ("ink", (123, 296, 240, 318)),
    "text: nav inactive": ("ink", (123, 385, 228, 410)),
    "text: nav shortcut": ("ink", (415, 385, 455, 410)),
    "text: key (mono, under label)": ("ink", (585, 425, 675, 445)),
    "text: subtitle": ("ink", (553, 252, 700, 276)),
    "text: control (dropdown value)": ("ink", (1085, 570, 1250, 595)),
    "text: status label": ("ink", (575, 1512, 665, 1538)),
    "text: status value": ("ink", (705, 1512, 905, 1538)),
    "text: placeholder (search)": ("ink", (138, 196, 330, 222)),
    "status light: ok": ("bg", (536, 1518, 548, 1530)),
    "status light: attention": ("bg", (1439, 1518, 1451, 1530)),
    "hairline: row border": ("ink", (1500, 496, 1520, 514)),
    "hairline: sidebar divider": ("ink", (506, 800, 518, 820)),
    "hairline: status strip top": ("ink", (1000, 1442, 1020, 1458)),
    "border: dropdown (top edge)": ("ink", (1200, 541, 1220, 553)),
    "border: card": ("ink", (700, 1235, 720, 1247)),
    "warning glyph": ("ink", (1836, 1505, 1876, 1540)),
}


def region_pixels(img, box):
    sx, sy = img.width / VIEW_W, img.height / VIEW_H
    x0, y0, x1, y1 = (int(box[0] * sx), int(box[1] * sy), int(box[2] * sx), int(box[3] * sy))
    crop = img.crop((x0, y0, x1, y1)).convert("RGB")
    return list(crop.getdata())


def median_rgb(px):
    return tuple(int(statistics.median(p[i] for p in px)) for i in range(3))


def dist(a, b):
    return sum((a[i] - b[i]) ** 2 for i in range(3))


def sample(img, kind, box):
    px = region_pixels(img, box)
    med = median_rgb(px)
    if kind == "bg":
        return med
    dmax = max(dist(p, med) for p in px)
    core = [p for p in px if dist(p, med) >= 0.5 * dmax]
    return median_rgb(core)


def hexc(c):
    return "#%02X%02X%02X" % c


def main():
    out = {}
    for mode in ("dark", "light"):
        path = os.path.join(ROOT, "general-%s.jpg" % mode)
        img = Image.open(path)
        out[mode] = {name: hexc(sample(img, k, b)) for name, (k, b) in REGIONS.items()}
    with open(os.path.join(HERE, "sampled.json"), "w") as f:
        json.dump(out, f, indent=2)
    w = max(len(n) for n in REGIONS)
    print("%-*s  %-8s  %-8s" % (w, "region", "dark", "light"))
    for n in REGIONS:
        print("%-*s  %-8s  %-8s" % (w, n, out["dark"][n], out["light"][n]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
