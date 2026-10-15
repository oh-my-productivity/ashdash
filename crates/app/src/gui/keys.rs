//! Keyboard handling for the launcher window.

use ashdash_core::Mods;
use eframe::egui::{self, Key, Modifiers};

use super::{LauncherApp, VISIBLE_ROWS, query_id};

struct Keys {
    up: bool,
    down: bool,
    enter: Option<Mods>,
    tab: bool,
    esc: bool,
    large: bool,
    quit: bool,
    quicklook: bool,
    page: i32,
    number: Option<usize>,
}

impl LauncherApp {
    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
        // A lone tap of Shift previews the selected file (Quick Look).
        let (shift, other_mods, other_input) = ctx.input(|i| {
            (
                i.modifiers.shift,
                i.modifiers.alt || i.modifiers.ctrl || i.modifiers.mac_cmd,
                i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Key { pressed: true, .. }
                            | egui::Event::Text(_)
                            | egui::Event::PointerButton { pressed: true, .. }
                    )
                }),
            )
        });
        let tapped = self
            .shift_tap
            .update(shift, other_mods, other_input, self.started.elapsed());

        let keys = ctx.input_mut(|i| {
            let m = i.modifiers;
            let mods = Mods {
                cmd: m.mac_cmd,
                alt: m.alt,
                ctrl: m.ctrl,
                shift: m.shift,
                fn_key: false,
            };
            let mut k = Keys {
                up: false,
                down: false,
                enter: None,
                tab: false,
                esc: false,
                large: false,
                quit: false,
                quicklook: false,
                page: 0,
                number: None,
            };
            k.up = i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::CTRL, Key::P);
            k.down = i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::CTRL, Key::N);
            if i.key_pressed(Key::Enter) {
                i.consume_key(m, Key::Enter);
                k.enter = Some(mods);
            }
            k.tab = i.consume_key(Modifiers::NONE, Key::Tab);
            k.esc = i.consume_key(Modifiers::NONE, Key::Escape);
            k.large = i.consume_key(Modifiers::MAC_CMD, Key::L);
            k.quit = i.consume_key(Modifiers::MAC_CMD, Key::Q);
            k.quicklook = i.consume_key(Modifiers::MAC_CMD, Key::Y);
            if i.consume_key(Modifiers::NONE, Key::PageDown) {
                k.page += 1;
            }
            if i.consume_key(Modifiers::NONE, Key::PageUp) {
                k.page -= 1;
            }
            for (n, key) in [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ]
            .into_iter()
            .enumerate()
            {
                if i.consume_key(Modifiers::MAC_CMD, key) {
                    k.number = Some(n);
                }
            }
            k
        });

        if keys.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if keys.esc {
            if self.quicklook.is_open() {
                self.quicklook.close();
            } else if self.large_type.is_some() {
                self.close_large_type(ctx);
                if !self.visible {
                    return;
                }
            } else if self.status.is_some() && self.confirming.is_some() {
                self.confirming = None;
                self.status = None;
            } else if !self.query.is_empty() {
                self.query.clear();
                self.request_query();
            } else {
                self.hide(ctx);
                return;
            }
        }
        if keys.page != 0 && !self.items.is_empty() {
            let step = keys.page as isize * VISIBLE_ROWS as isize;
            self.selected = (self.selected as isize + step).clamp(0, self.items.len() as isize - 1) as usize;
            self.scroll_to_selected = true;
            self.confirming = None;
            self.status = None;
        }
        if (keys.quicklook || tapped) && self.large_type.is_none() {
            self.toggle_quicklook();
        }
        if keys.down && !self.items.is_empty() {
            self.selected = (self.selected + 1).min(self.items.len() - 1);
            self.scroll_to_selected = true;
            self.confirming = None;
            self.status = None;
        }
        if keys.up {
            self.selected = self.selected.saturating_sub(1);
            self.scroll_to_selected = true;
            self.confirming = None;
            self.status = None;
        }
        if keys.tab
            && let Some(a) = self.items.get(self.selected).and_then(|i| i.autocomplete.clone())
        {
            self.query = a;
            self.want_focus = true;
            self.request_query();
            // put the cursor at the end of the new text
            if let Some(mut state) = egui::TextEdit::load_state(ctx, query_id()) {
                let end = egui::text::CCursor::new(self.query.chars().count());
                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                state.store(ctx, query_id());
            }
        }
        if keys.large {
            if self.large_type.is_some() {
                self.close_large_type(ctx);
                if !self.visible {
                    return;
                }
            } else if let Some(item) = self.items.get(self.selected) {
                self.large_type = Some(item.large_type.clone().unwrap_or_else(|| item.title.clone()));
            }
        }
        if let Some(n) = keys.number
            && n < self.items.len().min(VISIBLE_ROWS)
        {
            self.activate(ctx, n, Mods::NONE);
            return;
        }
        if let Some(mods) = keys.enter {
            if self.large_type.is_some() {
                self.close_large_type(ctx);
            } else {
                self.activate(ctx, self.selected, mods);
            }
        }
    }
}
