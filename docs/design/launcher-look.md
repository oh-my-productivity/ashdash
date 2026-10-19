# The launcher's default look

What a user gets with no theme chosen (`Theme::default()` in `crates/core/src/theme.rs`). A theme file overrides every value below; the theme
format keeps all its keys and gains three optional ones (`window` > `borderWidth`, `search` > `caretColor`, `search` > `caretWidth`).
All sizes are in points (pt); on a 2x screen one point is two device pixels.

## The empty state

Only the query row is visible. There is no result list, no icon, no placeholder, no hint row.

| Part | Value | Theme key |
|---|---|---|
| Panel width | 575 pt | `window` > `width` |
| Panel height | about 62 pt (border 2 x 0.5, padding 2 x 9, query row 42.5) | derived: `search` font size x 1.25 + 2 x `paddingVertical` + 2 x border |
| Corners | square (radius 0) | `window` > `roundness` |
| Fill | `#05080A`, alpha 245/255 (about 0.96) over a dark blur | `window` > `color` |
| Border | one device pixel (0.5 pt on 2x), `#242628` | `window` > `borderColor`, `borderWidth` |
| Window padding | 9 pt | `window` > `paddingVertical` |
| Query font size | 34 pt, white | `search` > `text` > `size`, `color` |
| Query background | none (fully transparent) | `search` > `background` |
| Caret | white, 1 pt wide (2 px on 2x), the height of the text line (about 38 pt for 34 pt text), centred in the row | `search` > `caretColor`, `caretWidth` |
| Caret position | 17.5 pt from the panel's left edge (border 0.5 + padding 9 + 8 pt before the text) | |

## With results

The same panel, extended downward: a 9 pt gap under the query row, then the result rows (48 pt each, at most 9 visible), then, only when there
is something to say (a status question, loading, the buffer line), a 26 pt footer line. Width, border and fill do not change; the window is
always exactly as tall as the panel, so no blur shows outside it. Heights are computed in `crates/app/src/gui/style.rs` (`window_height`).

## Native window

The window is transparent with a blur view behind the panel (dark HUD material when the fill is not opaque). The blur view's corner radius
follows `window` > `roundness`; with radius 0 no corner mask is set at all. On macOS 15 and earlier the native shadow follows the window's
shape; on macOS 26 and later there is no system shadow (next section). The only edge of the window is the theme border.

## The white border (macOS 26)

**Symptom.** On macOS 26 ("Tahoe") the owner saw a white or light line around the whole panel instead of the dark `#242628` border.

**Cause.** The window server of macOS 26 paints a light "Liquid Glass" rim over the outer one or two device pixels of every window that has
a system shadow (`NSWindow.hasShadow = YES`; our window set it to YES). The rim is drawn by the compositor, on top of the window's
content, and its brightness follows the desktop behind the window. It is not part of our content: a capture of the window alone
(`screencapture -o`) does not contain it, the capture with the shadow and the screen do. Because our border is one device pixel wide
(0.5 pt on a 2x screen) the rim covers all of it. The same rim is reported for other borderless windows on macOS 26 (Moonlight, World of
Warcraft). Nothing in our drawing was wrong: the egui frame, the clear colour and the premultiplied surface (`CompositeAlphaMode`
`PostMultiplied` on Metal, which Metal treats as premultiplied) give exactly the border colour at the window edge, on macOS 15 and 26.

**Evidence** (CI, 1x virtual display, `launcher-look` suite, `tests/e2e/screenshots/edge-check.py`):

| Window | edge pixel (outer ring) | second ring |
|---|---|---|
| macOS 15, shadow on, composited | `#242628` (36,38,40), as drawn | `#06090b`, as drawn |
| macOS 26, shadow on, composited (screen capture) | `#3a3c3d` (58,60,61): +22 | `#0b0e10`: +5 |
| macOS 26, shadow on, blur view hidden | the same rim | |
| macOS 26, shadow on, window opaque | the same rim | |
| macOS 26, `hasShadow = NO` | `#242628`, exactly as drawn | `#06090b`, as drawn |

The CI desktop is dark; on a light desktop the rim is correspondingly lighter, which is the white line seen on the owner's screen.

**Fix.** `hasShadow` is switched off on macOS 26 and later (`style::system_shadow`, `mac::style_launcher_window`). The window then has
our border and nothing else. We do not draw a replacement shadow: the window stays exactly as large as the panel (no transparent margin
to position, hit-test or clip), and the reference look has none that stands out on a dark desktop. macOS 15 and earlier keep the system
shadow, where it comes without a rim.

**Guard.** The `launcher-look` suite runs on `macos-15` and `macos-26` and fails when the window's two outer rings, as composited, differ
from the window-only capture by more than 12 per channel, when any of them (or the ring just outside the border) has a luminance above
80, or when the outermost ring of a window-only capture is light.

## Choices the reference did not decide

* **Font.** The reference does not show typed text; only the caret height (about 39 pt) is known. The default is the bundled proportional
  font at 34 pt. Change it with `search` > `text` > `size`; the row height follows (`size x 1.25`).
* **Fill.** Taken as `#05080A` at alpha 245 (0.96, the middle of 0.94-0.97). The blur material is the existing dark HUD one.
* **Panel height.** 62 pt is reached from the text size (34 pt) and the existing 9 pt padding; the result is within 1 pt of the reference.
* **Caret blink** is egui's default (0.5 s on, 0.5 s off) and is not a theme value.
* **Footer lines** (for example "Reloading..." while the index refreshes after start-up) still appear under the query row when they apply.
