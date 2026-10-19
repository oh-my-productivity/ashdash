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
follows `window` > `roundness`; with radius 0 no corner mask is set at all. The native shadow follows the window's shape.

## Choices the reference did not decide

* **Font.** The reference does not show typed text; only the caret height (about 39 pt) is known. The default is the bundled proportional
  font at 34 pt. Change it with `search` > `text` > `size`; the row height follows (`size x 1.25`).
* **Fill.** Taken as `#05080A` at alpha 245 (0.96, the middle of 0.94-0.97). The blur material is the existing dark HUD one.
* **Panel height.** 62 pt is reached from the text size (34 pt) and the existing 9 pt padding; the result is within 1 pt of the reference.
* **Caret blink** is egui's default (0.5 s on, 0.5 s off) and is not a theme value.
* **Footer lines** (for example "Reloading..." while the index refreshes after start-up) still appear under the query row when they apply.
