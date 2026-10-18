# Settings V4 "Instrument" with sidebar (General page)

AI-generated mockups (image generation from the first V4 pass). Treat them as intent; `SPEC.md` is the implementation spec.

The three mockups were regenerated from the earlier renders with the product name corrected; layout and content are the same, colours differ by a few levels.
The tokens in `SPEC.md` and `tools/sampled.json` were sampled from the earlier renders and are the ones the app uses, so `tools/sample_tokens.py` run on the new images prints slightly different values.

| File | What |
|---|---|
| `general-dark.jpg` | primary: General page, dark, sidebar |
| `general-light.jpg` | light twin |
| `reference-v4.jpg` | the V4 first pass (top tabs) |
| `impl-general-{dark,light}.png` | the real window (`ashdash prefs-window`) in the VM lab, General with the Full Disk Access card, 760 x 560 pt at 2x |
| `impl-permissions-{dark,light}.png` | the real Permissions page with simulated states (Accessibility not granted, Automation asked on first use, Full Disk Access not granted; `ASHDASH_PREFS_FAKE_PERMISSIONS`) |
| `tools/lab-shots.sh` | the script that takes them inside a lab VM (every page, dark/light, 760x560 / 640x480, recorder states, Cmd+K, Tab focus, Return-Fix) |
| `SPEC.md` | tokens (dark + light), type, geometry, components, egui 0.36 mapping |
| `contrast.md` | WCAG ratios for every text/background pair, from `tools/contrast.py` |
| `tools/sample_tokens.py`, `tools/sampled.json` | pixel sampling of the images; `tools/contrast.py` palette + check |

## What changed from V4

- The top tab bar is gone. A 160 pt sidebar holds the "Go to a setting... ⌘K" field and seven small-caps items, `⌘1` to `⌘7` right-aligned in mono. The active item (GENERAL) has a 3 px orange bar on its left edge and brighter text. A hairline separates sidebar and content. The content column is narrower; "RANK RESULTS BY WHAT I PICK" now wraps to two lines.
- The invented lines "BASIC SETTINGS FOR ASHDASH" and "SIMPLE TOOLS FOR A FASTER MAC" are replaced by the subtitle "SHORTCUT · THEME · RESULTS · START-UP" and, right-aligned in dim mono, the real config path `~/Library/Application Support/ashdash/config.json`.
- The theme name reads "Ashdash Dark" (V4 first pass showed it that way as well).
- Everything else follows V4: small-caps labels with the grey mono key under them, bordered two-column grid, plain orange toggles, orange recorder outline, stepper, RELOAD and attention cards, STATUS strip with square lights. Launch at login has no key.

## Flaws still visible in the images

- The RELOAD caption ("config.json applied 12:14:02 · 0 problems") and the attention card text are set in a proportional font with tight tracking; V4 and the spec use mono for the caption. Two regenerations did not fix it.
- Dark: the status lights render as a pale centre inside a coloured square (a thin coloured ring), not a solid fill. The spec says solid fill.
- Dark and light: the attention card and the DISK ACCESS strip item use an orange-red warning triangle, the spec uses the amber `attention` token. The light image's attention light (`#F79307`-like on `#F2F1ED`) is about 2:1 and would fail the 3:1 non-text bar; the spec darkens it.
- Control and card borders are about 1.2:1 against the surface, below the 3:1 non-text bar; the spec adds a `stroke_control` token (see questions).
- The macOS window chrome, drop shadow and backdrop vignette are not part of the design.
- The 760 x 560 size is estimated from the image aspect ratio (1.35), not measured from the app. "Go to a setting..." is a placeholder only; the mockup shows no results list, no open state.
- Text is AI-rendered: spellings were checked on both final images and are correct, but glyph shapes (`⌥⇧`, `↵`) are the generator's.
