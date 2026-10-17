# Settings V4 "Instrument" with sidebar: implementation spec

Source images: `general-dark.jpg` (primary), `general-light.jpg`, `reference-v4.jpg` (the V4 the owner picked). Target: egui 0.36.2, preferences window 760 x 560 pt (outer, native title bar). Scale in the images: 2.70 view-px per pt (window 2054 px = 760 pt).
Colours were **sampled by script** (`tools/sample_tokens.py` -> `tools/sampled.json`), rounded to the palette below, and checked with `tools/contrast.py` (full table: `contrast.md`, 0 failures). Where a sampled value failed AA it was darkened or lightened and is marked **adj**. Items marked **new** are not in the image (the image has no error state, hover or focus ring).

## 1. Colour tokens

| token | dark | light | sampled (dark / light) | egui 0.36 field |
|---|---|---|---|---|
| titlebar | `#2A2A2A` | `#EBEAE6` | `#2C2B2B` / `#EBEAE7` | drawn by the window (`kit::title_bar`, `Palette::titlebar`); the native title is hidden |
| sidebar | `#1A1A1A` | `#EAE8E4` | `#1A191A` / `#EAE9E5` | `panel_fill` (on the sidebar `SidePanel` frame) |
| content (window, card, status strip) | `#1C1C1C` | `#F7F6F4` | `#1C1C1C` / `#F7F6F4` | `window_fill`, `widgets.noninteractive.bg_fill` |
| status strip | `#1C1C1C` | `#F2F1ED` | `#1C1C1C` / `#F2F1ED` | own `Frame::fill` |
| field (search, dropdown, stepper cell) | `#282828` | `#FFFFFF` | `#282828` / `#FEFEFE` | `extreme_bg_color`, `text_edit_bg_color`, `widgets.inactive.weak_bg_fill` (dropdown) |
| button | `#303030` | `#FFFFFF` | `#313030` / `#FEFDFC` | `widgets.inactive.bg_fill` / `weak_bg_fill` |
| hover **new** | `#383838` | `#F0EEEA` | n/a | `widgets.hovered.bg_fill` / `weak_bg_fill`; nav hover `#232323` / `#E3E1DC` |
| hairline (row grid, dividers, card border) | `#2A2A2A` | `#DEDDD9` | `#2A2A2A` / `#DFDDDA` (mean of 4) | `widgets.noninteractive.bg_stroke`, `window_stroke` |
| stroke_control **adj** | `#6A6A6A` | `#8C8C8C` | image ~1.2:1 | `widgets.inactive.bg_stroke` (see Q1) |
| text | `#F0F0F0` | `#111111` | `#FAFAFA` / `#070606` | `override_text_color`, `widgets.*.fg_stroke.color` |
| dim | `#9A9A9A` | `#5F5F5F` | `#969596` / `#646362` **adj** | `weak_text_color` (set `weak_text_alpha` 1.0) |
| key (mono key under label) | `#8A8A8A` | `#6B6B6B` | `#8D8C8D` / `#747372` **adj** | custom `TextFormat.color` |
| accent (signal orange) | `#FF5F1F` | `#E5470A` | toggle `#FD631F` / `#EC5314` | `selection.bg_fill` (at 35 % alpha), `selection.stroke`, `hyperlink_color` for dark |
| accent_text | `#FF5F1F` | `#B53A04` **adj** | n/a | `hyperlink_color`, `widgets.active.fg_stroke` on orange text |
| ok (status light) | `#1FBF63` | `#1A9455` **adj** | `#17BB63` / `#42B56F` | custom |
| attention | `#E8962E` | `#B86A00` **adj** | `#E09928` / `#F79307` | status lights and card outlines (`Palette::warn`) |
| ok_text | `#1FBF63` | `#157A47` **adj** | n/a | text that says "ok" (`✓ Saved.`), 4.5:1 on content, field and status strip (the light's `ok` only needs 3:1) |
| warn_text | `#E8962E` | `#8A5200` **adj** | n/a | amber *text* (login reason, confirmations); `warn_fg_color`. Light `warn` stays the 3:1 light colour |
| error **new** | `#FF6B6B` | `#C62828` | n/a | `error_fg_color` |

Contrast (computed): text on any surface 11.6 to 18.9; dim on sidebar/content/field 5.2 to 6.4; key 4.9 (both); accent_text 4.8 to 5.7; accent 3.3 to 5.7 against content and sidebar (needed 3.0); stroke_control 3.1 to 3.2; ok/attention 3.4 to 7.2; error 5.2 to 6.1. Hairlines are 1.2:1 by design and carry no information alone (decorative).

## 2. Type

Fonts, both OFL 1.1, static files embedded with `include_bytes!`: **Inter** (Regular, Medium) for UI text, because it has tabular figures and a large x-height that stays legible at 11 pt; **Geist Mono** (Regular) for keys, numbers and times, because it is the mono V4 was drawn with (the image shows a slashed zero; not verified against the font file). Fallback mono: JetBrains Mono. Egui has no real small caps: use `to_uppercase()` plus `TextFormat.extra_letter_spacing`. Not verified: that the embedded files contain `⌘ ⌥ ⇧ ↵ ⚠`; the implementer must check and, if a glyph is missing, keep Inter second in the `Monospace` family list so egui falls back.

| style | font / size / weight | tracking (`extra_letter_spacing`) | colour | where |
|---|---|---|---|---|
| page title | Inter 17 Medium, upper | 3.0 pt (0.18 em) | text | "GENERAL" |
| label / nav | Inter 11 Medium, upper | 1.5 pt (0.14 em) | text; nav inactive dim | row labels, nav items, RELOAD |
| subtitle | Inter 10 Regular, upper | 1.2 pt | dim | page subtitle |
| key | Geist Mono 11 | 0 | key | `hotkey`, `max_results` |
| body | Inter 13 Regular | 0 | text | dropdown, buttons, card text |
| mono caption | Geist Mono 11 | 0 | dim | captions, config path, `⌘1` |
| number | Geist Mono 20, tabular | 0 | text | stepper value |
| status label / value | Inter 10 Medium upper / Inter 12 | 1.4 pt / 0 | text / dim | status strip |

## 3. Geometry

Grid 4 pt (steps 4, 8, 12, 16, 24). Title bar 28 pt (second pass; the traffic lights live in it). Sidebar 160 pt wide; nav item 34 pt tall; content padding 20 pt; row grid label column **188 pt** (so "RESULTS PER SEARCH" stays on one line; a test measures it), value column the rest; row min height 60 pt, cell padding 12 pt; recorder 42 pt tall; controls 28 pt tall; stepper 32 x 172 pt; toggle 38 x 20 pt (knob 16); status strip **36 pt** (not 48: the page has to fit 560 pt); status light 8 x 8 pt square. Radii: controls, cards, recorder 4 pt (`widgets.*.corner_radius`), toggle pill, status light 0, window 10 pt (`window_corner_radius`). Strokes: hairline 1 pt, control 1 pt, recorder (recording) 1.5 pt accent, focus ring 2 pt accent with 2 pt gap, active nav bar 3 pt wide. Spacing: `item_spacing` (8, 8), `button_padding` (12, 6), `interact_size.y` 28, `window_margin` 0 (panels draw their own frames), `combo_width` 190.

## 4. Components

- **Sidebar item.** 34 pt row, 16 pt left inset, label (nav style) left, `⌘1`..`⌘7` mono caption right, 14 pt right inset. Active: 3 pt accent bar on the left edge full row height, text colour `text` (inactive `dim`); the bar is the shape cue so the state is not colour only. Hover: row fill hover. A search field "Go to a setting..." with `⌘K` sits above the list (field fill, 28 pt, 8 pt margin); a 1 pt hairline separates sidebar and content. Order: GENERAL, SEARCH, CLIPBOARD, SNIPPETS, WORKFLOWS, PERMISSIONS, ABOUT.
- **Row.** Two columns split by a hairline; rows split by hairlines; the whole grid has a 1 pt hairline border. Left: label (label style) with the config key (key style) 4 pt under it; rows with no config key (Launch at login) show none and put "macOS login item · not in config.json" under the control. Right: the control, 16 pt inset.
- **Recorder (one box).** Idle: field fill, control stroke, the current shortcut centred, large, mono. Click, Return or Space on the focused box starts recording. Recording: 1.5 pt accent outline, "Press the new shortcut…" left, "was ⌥⇧Space" right-aligned in dim mono (on a second line when the box is too narrow for both). The keys that work while recording are said in the row's label cell, under the key: "Esc cancels · ⌫ default" (Backspace alone = back to the default shortcut, the keyboard route to "reset"). A refused combination (no Cmd/Option/Ctrl) shows the row error state with the reason; nothing else is shown while idle. The combination being recorded never switches pages (Cmd+1 ... are ignored while recording).
- **Permissions row.** Square status light + word (Granted / Not granted / Asked on first use / Unknown) + what needs it + "Open System Settings". Only a refusal is amber; "asked on first use" and "unknown" are the neutral outlined light. Probing is silent (see `mac/permissions.rs`).
- **Own title bar.** `ViewportBuilder::with_fullsize_content_view(true).with_title_shown(false).with_titlebar_shown(false)`: the native bar becomes transparent and only the traffic lights stay. `kit::title_bar` draws "Ashdash" centred (Inter Medium 13, `text` on `titlebar`) in the top 28 pt, a hairline under it, drags the window (`ViewportCommand::StartDrag`) and zooms on double click. Nothing else is drawn in that strip.
- **Dropdown / button.** Button fill, 1 pt control stroke, body text, 4 pt radius; dropdown has a chevron at the right. Hover: hover fill. Pressed: `widgets.active` with accent stroke.
- **Stepper.** `[ - ][ 30 ][ + ]` in one 1 pt frame, value cell field fill, number style; dividers hairline.
- **Toggle.** Pill 38 x 20; on: accent fill, white knob; off: field fill, control stroke, `dim` knob. No text label (the row label names it).
- **Card (RELOAD, attention).** Content fill, hairline border, 4 pt radius, 12 pt padding; icon or status light left, text, then a button right.
- **Status light.** 8 pt square, filled `ok` / `attention` / `error`; always followed by its text (`CONFIG applied 12:14:02`), so colour is never the only cue. Cells that need attention also get a painted warning triangle after the value. When the window is too narrow the strip first drops the "STATUS" caption, then the values (the cell names and triangles stay); it is measured, not guessed, so it never clips.
- **Focus ring (new).** egui draws no ring on buttons: when `response.has_focus()` paint `rect.expand(2)` with `StrokeKind::Outside`, 2 pt `accent`, same radius + 2. Text edits keep `selection.stroke`.
- **Validation error (new).** Control gets a 1.5 pt `error` outline; a painted warning triangle (drawn with the painter, not a font glyph) 12 pt and message in 11 pt `error` Inter below the control, e.g. "Not a valid shortcut. Use at least one modifier." The row label and key stay unchanged.

## 5. Not in the images

Hover, disabled (`disabled_alpha` 0.5), error and focus states are defined here, not drawn. Other pages (Search, Clipboard, ...) reuse rows, cards and the sidebar unchanged.

## 6. Owner-approved choices (second pass)

These differ from the mockup on purpose and are final unless the owner says otherwise.

- **Stronger control outline than the image.** Controls (buttons, dropdown, stepper, recorder, search field) use `stroke_control` (`#6A6A6A` dark / `#8C8C8C` light, 3:1) instead of the image's near-invisible border. Hairlines of the grid stay 1.2:1.
- **Amber warnings.** Warning lights, card outlines and amber text use the `attention` / `warn_text` tokens (amber), not the image's orange-red triangle; errors are red.
- **Full-width status strip.** The strip spans the whole window under the sidebar, as in the mockup, and is 36 pt high.
- **Only `config.json` at the top right** of a page header, not the full path.
- **No static help text.** A line of help appears only when it says something now: an error, a refusal, a disabled reason in one line. Confirmations ("Saved.") disappear after six seconds.
- **RELOAD and attention cards** sit side by side (one column when there is no attention card), the attention card has no title and a "Fix… ↵" button; Return with nothing focused follows it to the Permissions page.

## 7. What still differs from the mockup (verified in the VM lab, see `impl-*.png`)

- Dropdown arrow is egui's filled triangle, not the thin chevron; the stepper `−` and `+` are drawn smaller than in the image.
- Mono font for the RELOAD caption is not used (Inter), and the card text wraps to three lines at 760 pt because the cards share one row.
- "Launch at login" shows its macOS note on the same line as the toggle (mockup: below it) and, when the binary is not inside an `.app`, an amber one-line reason.
- The status strip has an extra LAUNCHER cell, and at 760 pt with the DISK ACCESS cell it drops the "STATUS" caption; at 640 pt it drops the values.
- The title bar is 28 pt and flat; the window corner radius, shadow and the vignette of the mockup are the system's.
- The Snippets and Workflows pages still use egui's grid, small buttons and path fields in places (they were not redesigned); the Search page's web-search editor too.
- `⌫` in the recorder note is drawn by the fallback font and looks smaller than the other glyphs.
- Not drawn in the images, defined here and checked in the lab: focus ring (seen on a nav item and on the recorder), row error state with triangle (seen on the shortcut row), Permissions page.

