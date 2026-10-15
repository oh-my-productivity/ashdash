//! Result list painting and app-icon loading.

use ashdash_core::theme::Theme;
use ashdash_core::{Icon, Mods, apps};
use eframe::egui::{self, FontId, Pos2, Rect, Sense, Vec2};

use super::{IconState, LauncherApp, ROW_HEIGHT, VISIBLE_ROWS, c};

impl LauncherApp {
    pub(super) fn draw_results(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, t: &Theme) {
        let width = ui.available_width();
        let mut clicked: Option<usize> = None;
        egui::ScrollArea::vertical()
            .max_height(VISIBLE_ROWS as f32 * ROW_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                for idx in 0..self.items.len() {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_HEIGHT), Sense::click());
                    if self.scroll_to_selected && idx == self.selected {
                        ui.scroll_to_rect(rect, None);
                    }
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let sel = idx == self.selected;
                    // Only a real mouse movement may steal the selection from the keyboard.
                    if resp.hovered() {
                        let pos = ctx.input(|i| i.pointer.hover_pos());
                        if let (Some(p), Some(prev)) = (pos, self.last_pointer)
                            && p.distance(prev) > 2.0
                        {
                            self.selected = idx;
                        }
                    }
                    if resp.clicked() {
                        clicked = Some(idx);
                    }
                    let item = self.items[idx].clone();
                    let painter = ui.painter_at(rect);
                    if sel {
                        painter.rect_filled(rect, t.result_roundness, c(t.result_selected_background));
                    }
                    let icon_size = t.icon_size.min(ROW_HEIGHT - 6.0);
                    let icon_rect = Rect::from_min_size(
                        Pos2::new(rect.left() + 8.0, rect.center().y - icon_size / 2.0),
                        Vec2::splat(icon_size),
                    );
                    match self.icon_uri(ctx, &item.icon) {
                        Some(uri) => {
                            egui::Image::new(uri)
                                .fit_to_exact_size(Vec2::splat(icon_size))
                                .paint_at(ui, icon_rect);
                        }
                        None => {
                            let glyph = match &item.icon {
                                Icon::Glyph(g) => *g,
                                Icon::App(_) => "▣",
                                _ => "•",
                            };
                            painter.text(
                                icon_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                glyph,
                                FontId::proportional(icon_size * 0.55),
                                c(if sel { t.result_text_selected } else { t.result_subtext }),
                            );
                        }
                    }
                    let text_x = icon_rect.right() + 10.0;
                    let has_sub = !item.subtitle.is_empty();
                    let title_y = if has_sub {
                        rect.top() + 6.0
                    } else {
                        rect.center().y - t.result_text_size * 0.6
                    };
                    let dim = if item.valid { 1.0 } else { 0.6 };
                    painter.text(
                        Pos2::new(text_x, title_y),
                        egui::Align2::LEFT_TOP,
                        &item.title,
                        FontId::proportional(t.result_text_size),
                        c(if sel { t.result_text_selected } else { t.result_text }).gamma_multiply(dim),
                    );
                    if has_sub {
                        painter.text(
                            Pos2::new(text_x, rect.top() + 6.0 + t.result_text_size + 3.0),
                            egui::Align2::LEFT_TOP,
                            &item.subtitle,
                            FontId::proportional(t.result_subtext_size),
                            c(if sel {
                                t.result_subtext_selected
                            } else {
                                t.result_subtext
                            }),
                        );
                    }
                    if idx < 9 {
                        painter.text(
                            Pos2::new(rect.right() - 12.0, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            format!("⌘{}", idx + 1),
                            FontId::proportional(t.result_subtext_size + 2.0),
                            c(if sel {
                                t.result_subtext_selected
                            } else {
                                t.result_subtext
                            }),
                        );
                    }
                }
            });
        self.last_pointer = ctx.input(|i| i.pointer.hover_pos()).or(self.last_pointer);
        self.scroll_to_selected = false;
        if let Some(i) = clicked {
            let mods = ctx.input(|i| Mods {
                cmd: i.modifiers.mac_cmd,
                alt: i.modifiers.alt,
                ctrl: i.modifiers.ctrl,
                shift: i.modifiers.shift,
                fn_key: false,
            });
            self.activate(ctx, i, mods);
        }
    }

    pub(super) fn icon_uri(&mut self, ctx: &egui::Context, icon: &Icon) -> Option<String> {
        match icon {
            Icon::Image(p) => Some(format!("file://{}", p.display())),
            Icon::App(app) => {
                match self.icons.get(app) {
                    Some(IconState::Ready(Some(png))) => return Some(format!("file://{}", png.display())),
                    Some(_) => return None,
                    None => {}
                }
                self.icons.insert(app.clone(), IconState::Pending);
                let (tx, app, cache, ctx) = (
                    self.icon_tx.clone(),
                    app.clone(),
                    self.engine.paths.icon_cache(),
                    ctx.clone(),
                );
                std::thread::spawn(move || {
                    let png = apps::app_icon_png(&app, &cache);
                    let _ = tx.send((app, png));
                    ctx.request_repaint();
                });
                None
            }
            _ => None,
        }
    }
}
