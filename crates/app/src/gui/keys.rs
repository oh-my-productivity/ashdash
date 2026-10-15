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
    number: Option<usize>,
}

impl LauncherApp {
    pub(super) fn handle_keys(&mut self, ctx: &egui::Context) {
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
            if self.large_type.is_some() {
                self.large_type = None;
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
        if keys.large
            && let Some(item) = self.items.get(self.selected)
        {
            self.large_type = Some(item.large_type.clone().unwrap_or_else(|| item.title.clone()));
        }
        if let Some(n) = keys.number
            && n < self.items.len().min(VISIBLE_ROWS)
        {
            self.activate(ctx, n, Mods::NONE);
            return;
        }
        if let Some(mods) = keys.enter {
            if self.large_type.is_some() {
                self.large_type = None;
            } else {
                self.activate(ctx, self.selected, mods);
            }
        }
    }
}
