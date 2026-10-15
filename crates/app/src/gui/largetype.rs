//! Large Type: pure font fitting for the full-screen overlay.
//!
//! Real glyph metrics are only known to the renderer, so the fit uses a
//! deliberately pessimistic average advance (`CHAR_W` em per character) and
//! line height. It simulates greedy word wrapping, hard-breaking words that
//! are longer than a line.

use super::trace;

use eframe::egui::{self, Align, Color32, FontId, Pos2, Rect as Rect2, text::LayoutJob};

use super::geometry::{self, Rect};
use super::{LauncherApp, mac, style};

impl LauncherApp {
    /// Stretch the window over the monitor under the mouse.
    fn enter_overlay(&mut self, ctx: &egui::Context) {
        let rect = mac::screens_and_mouse()
            .and_then(|(monitors, mouse)| geometry::pick_monitor(&monitors, mouse).map(|i| monitors[i]))
            .unwrap_or(Rect::new(0.0, 0.0, 1440.0, 900.0));
        trace::trace(|| format!("large type overlay on {rect:?}"));
        self.overlay = Some(rect);
        let mut ws = style::window_style(&self.theme);
        ws.corner_radius = 0.0;
        mac::style_launcher_window(super::WINDOW_TITLE, &ws);
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(
            rect.x as f32,
            rect.y as f32,
        )));
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::Vec2::new(
            rect.w as f32,
            rect.h as f32,
        )));
    }

    /// Back to the normal launcher window.
    pub(super) fn leave_overlay(&mut self, ctx: &egui::Context) {
        self.overlay = None;
        mac::style_launcher_window(super::WINDOW_TITLE, &style::window_style(&self.theme));
        self.last_height = 0.0; // force sync_height to resize
        self.position_launcher(ctx);
    }

    /// Leave Large Type: back to the results if there were any, else hide
    /// (Large Type triggered by an action while the launcher was hidden).
    pub(super) fn close_large_type(&mut self, ctx: &egui::Context) {
        self.large_type = None;
        if self.query.is_empty() && self.items.is_empty() {
            self.hide(ctx);
        }
    }

    pub(super) fn draw_large_type(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(text) = self.large_type.clone() else {
            return;
        };
        let Some(target) = self.overlay else {
            self.enter_overlay(ctx);
            return;
        };
        // Wait until the OS applied the new size, otherwise the first frame is clipped.
        let size = ctx.content_rect().size();
        if (f64::from(size.x) - target.w).abs() > 2.0 || (f64::from(size.y) - target.h).abs() > 2.0 {
            ctx.request_repaint();
            return;
        }
        let rect = ctx.content_rect();
        let translucent = style::window_style(&self.theme).vibrancy.is_some();
        let dim = Color32::from_black_alpha(if translucent { 70 } else { 220 });
        ui.painter().rect_filled(rect, 0.0, dim);

        let (max_w, max_h) = (rect.width() * 0.9, rect.height() * 0.85);
        let font_size = fit_font_size(&text, max_w, max_h);
        let text_for_scroll = text.clone();
        let mut job = LayoutJob::simple(text, FontId::proportional(font_size), Color32::WHITE, max_w);
        job.halign = Align::Center;
        let galley = ui.painter().layout_job(job);
        if needs_scroll(galley.size().y, max_h) {
            // Even the smallest size does not fit: the text scrolls (wheel, trackpad, arrows, Page keys).
            let area = Rect2::from_center_size(rect.center(), egui::vec2(max_w, rect.height() * 0.92));
            let delta = std::mem::take(&mut self.glue.large_scroll);
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if delta != 0.0 {
                        ui.scroll_with_delta(egui::vec2(0.0, -delta));
                    }
                    ui.set_width(max_w);
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Label::new(egui::RichText::new(&text_for_scroll).size(font_size).color(Color32::WHITE)).wrap());
                    });
                });
            });
        } else {
            let top = (rect.center().y - galley.size().y / 2.0).max(rect.top() + 8.0);
            ui.painter()
                .galley(Pos2::new(rect.center().x, top), galley, Color32::WHITE);
        }

        if ctx.input(|i| i.pointer.any_click()) {
            self.close_large_type(ctx);
        }
    }
}

/// The laid-out text is taller than the room for it, so it has to scroll.
pub fn needs_scroll(text_height: f32, available: f32) -> bool {
    text_height > available
}

/// Average glyph advance as a fraction of the font size (proportional sans
/// fonts are around 0.5; bold/wide text and capitals are wider).
pub const CHAR_W: f32 = 0.6;
/// Line height as a fraction of the font size.
pub const LINE_H: f32 = 1.25;
pub const MIN_SIZE: f32 = 18.0;
pub const MAX_SIZE: f32 = 360.0;

/// Number of lines `text` occupies when wrapped to `chars_per_line` columns.
pub fn wrapped_lines(text: &str, chars_per_line: usize) -> usize {
    let cols = chars_per_line.max(1);
    let mut lines = 0;
    for para in text.split('\n') {
        let mut used = 0usize; // columns used on the current line, 0 = fresh line
        let mut para_lines = 1;
        for word in para.split_whitespace() {
            let mut len = word.chars().count();
            // hard-break words longer than a whole line
            while len > cols {
                if used > 0 {
                    para_lines += 1;
                }
                para_lines += 1;
                len -= cols;
                used = 0;
            }
            let need = if used == 0 { len } else { used + 1 + len };
            if need > cols {
                para_lines += 1;
                used = len;
            } else {
                used = need;
            }
        }
        lines += para_lines;
    }
    lines
}

/// Largest font size in `[MIN_SIZE, MAX_SIZE]` (whole points) at which `text`
/// fits into `width` x `height`. Falls back to `MIN_SIZE` when nothing fits
/// (the overlay scrolls in that case).
pub fn fit_font_size(text: &str, width: f32, height: f32) -> f32 {
    if text.trim().is_empty() || width <= 0.0 || height <= 0.0 {
        return MAX_SIZE.min(height.max(MIN_SIZE));
    }
    let fits = |size: f32| {
        let cols = (width / (size * CHAR_W)).floor() as usize;
        cols >= 1 && wrapped_lines(text, cols) as f32 * size * LINE_H <= height
    };
    // `fits` is monotonic in size, so binary search over whole points.
    let (mut lo, mut hi) = (MIN_SIZE as i32, MAX_SIZE as i32);
    if !fits(lo as f32) {
        return MIN_SIZE;
    }
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if fits(mid as f32) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_counts_lines() {
        assert_eq!(wrapped_lines("", 10), 1);
        assert_eq!(wrapped_lines("hello world", 20), 1);
        assert_eq!(wrapped_lines("hello world", 8), 2);
        assert_eq!(wrapped_lines("a\nb\nc", 10), 3);
        assert_eq!(wrapped_lines("aaaa bbbb cccc", 9), 2);
    }

    #[test]
    fn long_words_are_hard_broken() {
        assert_eq!(wrapped_lines("abcdefghij", 4), 3);
        assert_eq!(wrapped_lines("ab abcdefghij", 4), 4);
        assert_eq!(wrapped_lines("x", 0), 1);
    }

    #[test]
    fn short_text_gets_the_largest_size() {
        let s = fit_font_size("Hi", 1800.0, 900.0);
        assert!(s >= 300.0);
    }

    #[test]
    fn longer_text_gets_smaller_text() {
        let short = fit_font_size("hello", 1600.0, 800.0);
        let medium = fit_font_size(&"hello ".repeat(40), 1600.0, 800.0);
        let long = fit_font_size(&"hello ".repeat(400), 1600.0, 800.0);
        assert!(short > medium && medium > long, "{short} {medium} {long}");
    }

    #[test]
    fn chosen_size_fits_and_next_does_not() {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(12);
        let (w, h) = (1500.0, 700.0);
        let s = fit_font_size(&text, w, h);
        let lines_at = |size: f32| wrapped_lines(&text, (w / (size * CHAR_W)) as usize) as f32 * size * LINE_H;
        assert!(lines_at(s) <= h);
        assert!(s >= MAX_SIZE || lines_at(s + 1.0) > h);
    }

    #[test]
    fn text_that_does_not_fit_even_at_the_minimum_size_scrolls() {
        assert!(!needs_scroll(500.0, 700.0));
        assert!(!needs_scroll(700.0, 700.0));
        assert!(needs_scroll(700.5, 700.0));
        // the pessimistic fit gives up (MIN_SIZE) exactly for text that would need scrolling
        let huge = "word ".repeat(20_000);
        assert_eq!(fit_font_size(&huge, 1200.0, 700.0), MIN_SIZE);
        assert!(wrapped_lines(&huge, (1200.0 / (MIN_SIZE * CHAR_W)) as usize) as f32 * MIN_SIZE * LINE_H > 700.0);
    }

    #[test]
    fn result_is_clamped_and_never_panics() {
        assert_eq!(fit_font_size(&"x".repeat(100_000), 800.0, 600.0), MIN_SIZE);
        assert!(fit_font_size("", 800.0, 600.0) >= MIN_SIZE);
        assert!(fit_font_size("abc", 0.0, 0.0) >= MIN_SIZE);
        assert!(fit_font_size("abc", f32::NAN, 100.0) >= MIN_SIZE);
        assert!(fit_font_size("a", 100000.0, 100000.0) <= MAX_SIZE);
    }
}
