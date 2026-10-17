//! Keyboard handling for the launcher window.

use ashdash_core::Mods;
use eframe::egui::{self, Key, Modifiers};

use super::schedule::Target;
use super::{LauncherApp, VISIBLE_ROWS, panel, query_id};

struct Keys {
    up: bool,
    down: bool,
    enter: Option<Mods>,
    /// The Return in `enter` is a new key press, not a key-repeat event of a held key.
    enter_fresh: bool,
    tab: bool,
    esc: bool,
    large: bool,
    quit: bool,
    quicklook: bool,
    page: i32,
    number: Option<usize>,
    /// The Cmd+number in `number` is a new key press (see `enter_fresh`).
    number_fresh: bool,
    /// Option+Up / Option+Down: add to / remove from the buffer.
    buffer_add: bool,
    buffer_remove: bool,
    /// A plain Right / Left Arrow was pressed (not consumed yet: the text
    /// cursor position decides who gets it).
    right: bool,
    left: bool,
}

/// Whether the query field's text cursor is at the start / at the end, with
/// no selection. Without a cursor state (never focused) it sits at the end.
fn cursor_extent(ctx: &egui::Context, text: &str) -> (bool, bool) {
    let len = text.chars().count();
    match egui::TextEdit::load_state(ctx, query_id()).and_then(|s| s.cursor.char_range()) {
        Some(r) => {
            let (a, b) = (r.primary.index.0, r.secondary.index.0);
            (a == 0 && b == 0, a == len && b == len)
        }
        None => (len == 0, true),
    }
}

/// Whether `events` hold a key press of `key` that is not an auto-repeat. A held key keeps
/// producing `pressed` events with `repeat: true`; only the first one is a deliberate press, and
/// only that one may answer a "press again to confirm" question.
pub fn fresh_press(events: &[egui::Event], key: Key) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            egui::Event::Key { key: k, pressed: true, repeat: false, .. } if *k == key
        )
    })
}

/// How far (points) the Large Type text scrolls for the keys pressed this frame.
pub fn large_scroll_delta(up: bool, down: bool, page: i32) -> f32 {
    const LINE: f32 = 48.0;
    const PAGE: f32 = 480.0;
    (i32::from(down) - i32::from(up)) as f32 * LINE + page as f32 * PAGE
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

        let (at_start, at_end) = cursor_extent(ctx, &self.query);
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
                enter_fresh: false,
                tab: false,
                esc: false,
                large: false,
                quit: false,
                quicklook: false,
                page: 0,
                number: None,
                number_fresh: false,
                buffer_add: false,
                buffer_remove: false,
                right: false,
                left: false,
            };
            // Option+arrows first: a plain-arrow shortcut also matches with Option held
            k.buffer_add = i.consume_key(Modifiers::ALT, Key::ArrowUp);
            k.buffer_remove = i.consume_key(Modifiers::ALT, Key::ArrowDown);
            k.right = m.is_none() && i.key_pressed(Key::ArrowRight);
            k.left = m.is_none() && i.key_pressed(Key::ArrowLeft);
            k.up = i.consume_key(Modifiers::NONE, Key::ArrowUp) || i.consume_key(Modifiers::CTRL, Key::P);
            k.down = i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::CTRL, Key::N);
            if i.key_pressed(Key::Enter) {
                // before consume_key removes the events
                k.enter_fresh = fresh_press(&i.events, Key::Enter);
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
                let fresh = fresh_press(&i.events, key);
                if i.consume_key(Modifiers::MAC_CMD, key) {
                    k.number = Some(n);
                    k.number_fresh = fresh;
                }
            }
            k
        });

        if keys.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        // The workflow import prompt takes over the keyboard: Return installs, Esc cancels.
        if let Some(p) = &self.glue.import {
            match super::importprompt::prompt_key(keys.esc, keys.enter, keys.enter_fresh, p.shown_at.elapsed()) {
                super::importprompt::PromptKey::Cancel => self.cancel_import(),
                super::importprompt::PromptKey::Install => self.confirm_import(ctx),
                super::importprompt::PromptKey::Nothing => {}
            }
            return;
        }
        if self.large_type.is_some() {
            self.glue.large_scroll += large_scroll_delta(keys.up, keys.down, keys.page);
        }
        let keys = if self.large_type.is_some() {
            // arrows scroll the text, they do not move the (hidden) selection
            Keys {
                up: false,
                down: false,
                page: 0,
                ..keys
            }
        } else {
            keys
        };
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
            } else if self.panel_open() {
                self.panel_back();
            } else {
                self.hide(ctx);
                return;
            }
        }
        // Universal Actions: Right opens the panel (or the next level), Left goes back,
        // but only where the arrow would not move the text cursor.
        if keys.right
            && self.large_type.is_none()
            && panel::right_opens_panel(at_end, false, true)
            && self.open_actions(ctx)
        {
            ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowRight));
        }
        if keys.left && panel::left_goes_back(at_start, false, self.panel_open()) {
            ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowLeft));
            self.panel_back();
        }
        if keys.buffer_add && self.large_type.is_none() && !self.panel_open() {
            self.buffer_add();
        }
        if keys.buffer_remove && self.large_type.is_none() {
            self.buffer_remove();
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
        if let Some(n) = keys.number {
            // the row the user saw, even if an answer replaced the list in this very frame
            match self.glue.track.number_target(n, &self.items) {
                Target::Row(i) => {
                    self.activate_with(ctx, i, Mods::NONE, keys.number_fresh);
                    return;
                }
                Target::Changed => {
                    self.status = Some(RESULTS_CHANGED.into());
                    return;
                }
                Target::Nothing => {}
            }
        }
        if let Some(mods) = keys.enter {
            if self.large_type.is_some() {
                self.close_large_type(ctx);
            } else {
                // the row that was highlighted in the frame the user saw
                match self.glue.track.enter_target(self.selected, &self.items) {
                    Target::Row(i) => self.activate_with(ctx, i, mods, keys.enter_fresh),
                    Target::Changed => self.status = Some(RESULTS_CHANGED.into()),
                    Target::Nothing => {}
                }
            }
        }
    }
}

/// Shown when a key named a row that the arriving results took away.
const RESULTS_CHANGED: &str = "The results changed under that key press. Check the highlighted row and press it again.";

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key, pressed: bool, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn the_refused_key_message_fits_return_and_cmd_number() {
        assert!(RESULTS_CHANGED.contains("press it again"), "{RESULTS_CHANGED}");
        assert!(!RESULTS_CHANGED.contains('↩'), "Cmd+number is refused with this too");
    }

    #[test]
    fn a_first_press_is_fresh_and_its_repeats_are_not() {
        assert!(fresh_press(&[key(Key::Enter, true, false)], Key::Enter));
        assert!(!fresh_press(&[key(Key::Enter, true, true)], Key::Enter));
        assert!(!fresh_press(
            &[key(Key::Enter, true, true), key(Key::Enter, true, true)],
            Key::Enter
        ));
        // a release, another key, or nothing at all is not a press of this key
        assert!(!fresh_press(&[key(Key::Enter, false, false)], Key::Enter));
        assert!(!fresh_press(&[key(Key::Num1, true, false)], Key::Enter));
        assert!(!fresh_press(&[], Key::Enter));
        // a release-and-press inside one frame still counts as a new press
        assert!(fresh_press(
            &[
                key(Key::Enter, true, true),
                key(Key::Enter, false, false),
                key(Key::Enter, true, false)
            ],
            Key::Enter
        ));
    }
}
