#!/usr/bin/env python3
"""Final palette for the V4 sidebar settings page + WCAG 2.x contrast check.

The palette is the sampled values (tools/sample_tokens.py -> sampled.json) rounded to a clean set.
Where a rounded or sampled value would fail AA it is adjusted and marked "adj".
Usage: python3 contrast.py            (prints markdown tables, exits 1 if any required pair fails)
"""
import sys

PALETTE = {
    "dark": {
        "titlebar": "#2A2A2A",
        "sidebar": "#1A1A1A",
        "content": "#1C1C1C",
        "status_bg": "#1C1C1C",
        "card": "#1C1C1C",
        "field": "#282828",
        "button": "#303030",
        "hairline": "#2A2A2A",
        "stroke_control": "#6A6A6A",
        "text": "#F0F0F0",
        "dim": "#9A9A9A",
        "key": "#8A8A8A",
        "accent": "#FF5F1F",
        "accent_text": "#FF5F1F",
        "ok": "#1FBF63",
        "attention": "#E8962E",
        "error": "#FF6B6B",
    },
    "light": {
        "titlebar": "#EBEAE6",
        "sidebar": "#EAE8E4",
        "content": "#F7F6F4",
        "status_bg": "#F2F1ED",
        "card": "#F7F6F4",
        "field": "#FFFFFF",
        "button": "#FFFFFF",
        "hairline": "#DEDDD9",
        "stroke_control": "#8C8C8C",
        "text": "#111111",
        "dim": "#5F5F5F",
        "key": "#6B6B6B",
        "accent": "#E5470A",
        "accent_text": "#B53A04",
        "ok": "#1A9455",
        "attention": "#B86A00",
        "error": "#C62828",
    },
}

# (foreground token, background token, minimum ratio, what it is)
PAIRS = [
    ("text", "content", 4.5, "page title, row labels, values"),
    ("text", "sidebar", 4.5, "active nav item"),
    ("text", "field", 4.5, "dropdown / search field value"),
    ("text", "button", 4.5, "button label"),
    ("text", "status_bg", 4.5, "status label"),
    ("text", "titlebar", 4.5, "window title"),
    ("dim", "content", 4.5, "subtitle, captions, status value"),
    ("dim", "sidebar", 4.5, "inactive nav item, shortcuts"),
    ("dim", "field", 4.5, "placeholder 'Go to a setting...'"),
    ("dim", "status_bg", 4.5, "status value"),
    ("dim", "card", 4.5, "card caption"),
    ("key", "content", 4.5, "config key under a label (mono)"),
    ("accent_text", "content", 4.5, "orange text (recording prompt, links)"),
    ("accent_text", "sidebar", 4.5, "orange text on sidebar"),
    ("accent", "content", 3.0, "toggle track, recorder outline, focus ring"),
    ("accent", "sidebar", 3.0, "active nav bar"),
    ("stroke_control", "content", 3.0, "control outline (non-text, WCAG 1.4.11)"),
    ("ok", "status_bg", 3.0, "status light ok"),
    ("attention", "status_bg", 3.0, "status light / glyph attention"),
    ("attention", "card", 3.0, "attention card light"),
    ("error", "content", 4.5, "validation error text and glyph"),
    ("error", "field", 4.5, "validation error text inside a field"),
]
# informational only (decorative): not required to pass
INFO = [("hairline", "content", "row grid / dividers (decorative)")]


def lin(c):
    c /= 255.0
    return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4


def lum(h):
    r, g, b = (int(h[i:i + 2], 16) for i in (1, 3, 5))
    return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)


def ratio(a, b):
    la, lb = lum(a), lum(b)
    hi, lo = max(la, lb), min(la, lb)
    return (hi + 0.05) / (lo + 0.05)


def main():
    bad = 0
    for mode in ("dark", "light"):
        p = PALETTE[mode]
        print("\n### %s\n" % mode)
        print("| foreground | background | ratio | need | use | result |")
        print("|---|---|---|---|---|---|")
        for fg, bg, need, use in PAIRS:
            r = ratio(p[fg], p[bg])
            ok = r >= need
            bad += 0 if ok else 1
            print("| %s `%s` | %s `%s` | %.2f | %.1f | %s | %s |" % (fg, p[fg], bg, p[bg], r, need, use, "pass" if ok else "FAIL"))
        for fg, bg, use in INFO:
            print("| %s `%s` | %s `%s` | %.2f | n/a | %s | info |" % (fg, p[fg], bg, p[bg], ratio(p[fg], p[bg]), use))
    print("\nfailures: %d" % bad)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
