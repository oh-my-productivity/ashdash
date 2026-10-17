# Fonts used by the preferences window

Static TrueType files (not variable fonts), embedded with `include_bytes!` in
`crates/app/src/gui/prefs/fonts.rs`. The launcher window does not use them.
Both families are under the SIL Open Font License 1.1; the licence texts sit next to the files.

| File | Family / weight | Version | Source archive | SHA-256 of the archive | SHA-256 of the file |
|---|---|---|---|---|---|
| `Inter-Regular.ttf` | Inter Regular | 4.1 | https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip (`extras/ttf/Inter-Regular.ttf`) | `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e` | `40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82` |
| `Inter-Medium.ttf` | Inter Medium | 4.1 | same archive (`extras/ttf/Inter-Medium.ttf`) | same | `97ad806f526e41546d46365bb3a393145f75b7b1568913db74549ad8b8dba872` |
| `GeistMono-Regular.ttf` | Geist Mono Regular | 1.8.1 | https://github.com/vercel/geist-font/releases/download/v1.8.1/geist-font-v1.8.1.zip (`geist-font/GeistMono/ttf/GeistMono-Regular.ttf`) | `70f42c1452fb6df12ea183e20a65999157354f91d11dac18b4c713e383808ec2` | `90f1ada2b0e62e15e4268fd00636c6fb8665ba51556becfead428839f433d454` |

| Licence file | Copied from | SHA-256 |
|---|---|---|
| `OFL-Inter.txt` | `LICENSE.txt` in the Inter archive | `262481e844521b326f5ecd053e59b98c8b2da78c8ee1bdbb6e8174305e54935a` |
| `OFL-GeistMono.txt` | `geist-font/OFL.txt` in the Geist archive | `c683bfbcc7e087f5d37a54ef628f10387c451a83ddc459b151403a164ac46c90` |

Downloaded 2026-10-17 from the official GitHub releases. A test in `fonts.rs` checks that the three files are TrueType and within the size budget; the hashes above are the record to compare with.

Why these two: Inter is a screen-oriented UI face with wide symbol coverage (arrows, command-key glyphs);
Geist Mono has clear digits for the stepper and the config keys. Both are OFL, so redistribution inside the
binary is allowed provided the licence text ships with it (see `NOTICE`).
