//! Result list painting and app-icon loading.

use std::sync::Arc;

use ashdash_core::theme::Theme;
use ashdash_core::{Icon, Item, Mods, apps, first_fallback};
use eframe::egui::text::{LayoutJob, TextWrapping};
use eframe::egui::{self, Color32, FontId, Galley, Painter, Pos2, Rect, Sense, Stroke, Vec2};

use super::rowcache::{RowCache, RowKey};
use super::rowtext::SubKind;
use super::{IconState, LauncherApp, ROW_HEIGHT, VISIBLE_ROWS, c, keynames, rowfonts, rowhint, rowtext};

/// The hover fill of a row that is not selected: white at alpha 10 (`#FFFFFF0A`), 1.06:1 against the panel. A derived constant, not a
/// theme key.
const HOVER_FILL: Color32 = Color32::from_rgba_premultiplied(10, 10, 10, 10);

/// One line of `text` that is cut with `…` where it would pass `max_width`.
fn one_line(painter: &Painter, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping {
        max_width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

/// The laid-out text of one result row: what [`RowCache`] keeps from frame to frame.
#[derive(Clone)]
pub(super) struct RowText {
    title: Arc<Galley>,
    sub: Option<Arc<Galley>>,
}

/// What decides how a row's text is laid out, besides the item itself.
struct RowLook<'a> {
    selected: bool,
    /// The row is a fallback search (its subtitle only shows when selected).
    fallback: bool,
    /// The modifier hint that replaces the subtitle while a modifier is held on the selected row.
    held_hint: Option<&'a str>,
    title_color: Color32,
    sub_color: Color32,
    max_width: f32,
    pixels_per_point: f32,
    font_epoch: u32,
}

/// The key of a row's layout: the item's id and texts, the width, the selection and everything else that changes a glyph or a position.
fn row_key(item: &Item, t: &Theme, look: &RowLook) -> RowKey {
    RowKey::of(|h| {
        h.str(&item.id)
            .str(&item.title)
            .str(&item.subtitle)
            .str(look.held_hint.unwrap_or(""))
            .bool(look.held_hint.is_some())
            .bool(look.selected)
            .bool(look.fallback)
            .f32(look.max_width)
            .f32(look.pixels_per_point)
            .u32(look.font_epoch)
            .f32(t.result_text_size)
            .f32(t.result_subtext_size)
            .bytes(&look.title_color.to_array())
            .bytes(&look.sub_color.to_array());
    })
}

/// The title and subtitle galleys of a row, from the cache when the row looks the same as in the last frame.
fn row_text(
    cache: &mut RowCache<RowText>,
    painter: &Painter,
    t: &Theme,
    home: &str,
    item: &Item,
    look: &RowLook,
) -> RowText {
    let key = row_key(item, t, look);
    cache.get_or_insert_with(key, || {
        let title = one_line(
            painter,
            &item.title,
            FontId::new(t.result_text_size, rowfonts::text_family()),
            look.title_color,
            look.max_width,
        );
        // the subtitle: the modifier hint while a modifier is held on the selected row; a fallback row shows its URL only when
        // selected (so the user sees what Return opens)
        let subtitle = look.held_hint.unwrap_or(&item.subtitle);
        let sub = (!subtitle.is_empty() && (look.selected || !look.fallback))
            .then(|| subtitle_galley(painter, t, home, subtitle, look.sub_color, look.max_width));
        RowText { title, sub }
    })
}

/// The subtitle laid out in one line of at most `max_w`: Inter for plain text; Geist Mono for a path (home as `~`, middle elided) or a
/// URL (without scheme and `www.`, cut at the end).
fn subtitle_galley(
    painter: &Painter,
    t: &Theme,
    home: &str,
    subtitle: &str,
    color: Color32,
    max_w: f32,
) -> Arc<Galley> {
    let (kind, text) = rowtext::subtitle_text(subtitle, home);
    match kind {
        SubKind::Plain => one_line(
            painter,
            &text,
            FontId::new(t.result_subtext_size, rowfonts::text_family()),
            color,
            max_w,
        ),
        SubKind::Url | SubKind::Path => {
            let font = FontId::new(
                t.result_subtext_size - rowtext::MONO_SIZE_DELTA,
                rowfonts::mono_family(),
            );
            let text = if kind == SubKind::Path {
                let measure = |s: &str| painter.layout_no_wrap(s.to_owned(), font.clone(), color).size().x <= max_w;
                rowtext::elide_path(&text, measure)
            } else {
                text
            };
            one_line(painter, &text, font, color, max_w)
        }
    }
}

impl LauncherApp {
    pub(super) fn draw_results(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, t: &Theme) {
        let width = ui.available_width();
        let mut clicked: Option<usize> = None;
        let held = ctx.input(|i| Mods {
            cmd: i.modifiers.mac_cmd,
            alt: i.modifiers.alt,
            ctrl: i.modifiers.ctrl,
            shift: i.modifiers.shift,
            fn_key: false,
        });
        let ppp = ctx.pixels_per_point();
        if let (Some(p), Some(prev)) = (ctx.input(|i| i.pointer.hover_pos()), self.last_pointer)
            && p.distance(prev) > 2.0
        {
            self.pointer_moved = true;
        }
        // the hairline goes above the first fallback row, when at least one real hit is above it
        let separator_row = first_fallback(&self.items).filter(|&i| i > 0);
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
                    let sel = idx == self.selected;
                    let (icon, valid, is_fallback) = {
                        let item = &self.items[idx];
                        (item.icon.clone(), item.valid, item.is_fallback())
                    };
                    let painter = ui.painter_at(rect);
                    if sel {
                        painter.rect_filled(rect, t.result_roundness, c(t.result_selected_background));
                    } else if resp.hovered() && self.pointer_moved {
                        // a faint hover row for a pointer the keyboard selection has moved away from
                        painter.rect_filled(rect, t.result_roundness, HOVER_FILL);
                    }
                    if Some(idx) == separator_row && !sel {
                        let (x0, x1, y, w) = rowtext::separator_line(rect.left(), rect.right(), rect.top(), ppp);
                        painter.line_segment([Pos2::new(x0, y), Pos2::new(x1, y)], Stroke::new(w, c(t.separator)));
                    }
                    let icon_size = t.icon_size.min(ROW_HEIGHT - 6.0);
                    let icon_rect = Rect::from_min_size(
                        Pos2::new(rect.left() + 8.0, rect.center().y - icon_size / 2.0),
                        Vec2::splat(icon_size),
                    );
                    match self.icon_uri(ctx, &icon) {
                        Some(uri) => {
                            egui::Image::new(uri)
                                .fit_to_exact_size(Vec2::splat(icon_size))
                                .paint_at(ui, icon_rect);
                        }
                        None => {
                            let glyph = match &icon {
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
                    let has_hint = idx < rowtext::HINT_ROWS;
                    let text_x = icon_rect.right() + rowtext::ICON_GAP;
                    let max_w = rowtext::text_max_width(text_x, rect.right(), has_hint);
                    let dim = if valid { 1.0 } else { 0.6 };
                    let title_color = if sel {
                        c(t.result_text_selected)
                    } else if is_fallback {
                        c(t.result_text).gamma_multiply(rowtext::FALLBACK_TITLE_ALPHA)
                    } else {
                        c(t.result_text)
                    }
                    .gamma_multiply(dim);
                    let sub_color = c(if sel {
                        t.result_subtext_selected
                    } else {
                        t.result_subtext
                    });
                    let held_hint = (sel && !held.is_none())
                        .then(|| rowhint::mod_hint(&self.items[idx], held))
                        .flatten();
                    let look = RowLook {
                        selected: sel,
                        fallback: is_fallback,
                        held_hint: held_hint.as_deref(),
                        title_color,
                        sub_color,
                        max_width: max_w,
                        pixels_per_point: ppp,
                        font_epoch: self.font_epoch,
                    };
                    let RowText { title, sub } =
                        row_text(&mut self.rows, &painter, t, &self.home, &self.items[idx], &look);
                    let (title_y, sub_y) =
                        rowtext::block_tops(rect.center().y, title.size().y, sub.as_ref().map(|g| g.size().y));
                    painter.galley(Pos2::new(text_x, title_y), title, title_color);
                    if let (Some(galley), Some(y)) = (sub, sub_y) {
                        painter.galley(Pos2::new(text_x, y), galley, sub_color);
                    }
                    if has_hint {
                        painter.text(
                            Pos2::new(rect.right() - rowtext::HINT_MARGIN_R, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            format!("⌘{}", idx + 1),
                            FontId::new(t.result_subtext_size, rowfonts::mono_family()),
                            sub_color,
                        );
                    }
                }
            });
        self.rows.end_frame();
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

    /// A one-row notice shown instead of results while the query is empty
    /// (currently: the global hotkey could not be registered).
    pub(super) fn startup_hint(&self) -> Option<String> {
        let err = self.hotkey_error.as_ref()?;
        if !self.query.is_empty() {
            return None;
        }
        Some(hotkey_hint(&self.engine, err))
    }

    pub(super) fn draw_hint_row(&self, ui: &mut egui::Ui, t: &Theme, text: &str) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.text(
            Pos2::new(rect.left() + 8.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "⚠",
            FontId::proportional(t.icon_size.min(ROW_HEIGHT - 6.0) * 0.55),
            c(t.result_subtext),
        );
        let galley = painter.layout(
            text.to_owned(),
            FontId::proportional(t.result_subtext_size + 1.0),
            c(t.result_text),
            rect.width() - 48.0,
        );
        let pos = Pos2::new(rect.left() + 40.0, rect.center().y - galley.size().y / 2.0);
        painter.galley(pos, galley, c(t.result_text));
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

/// The notice for a launcher hotkey that could not be registered. Names the hotkey in force
/// *now* (a live reload may have changed it), not the one from start-up.
fn hotkey_hint(engine: &ashdash_core::Engine, err: &str) -> String {
    let raw = engine.config_now().hotkey;
    let shown = keynames::display_hotkey(&raw).unwrap_or(raw);
    format!(
        "Hotkey {shown} is unavailable ({}). Change \"hotkey\" in config.json.",
        short_error(err)
    )
}

/// First sentence-ish part of a library error (some append "please report this to ...").
fn short_error(e: &str) -> &str {
    e.split(", if you").next().unwrap_or(e).trim()
}

#[cfg(test)]
mod tests {
    use ashdash_core::Item;
    use ashdash_core::theme::Theme;
    use eframe::egui::Color32;

    use super::{RowLook, hotkey_hint, row_key, short_error};

    fn look() -> RowLook<'static> {
        RowLook {
            selected: false,
            fallback: false,
            held_hint: None,
            title_color: Color32::WHITE,
            sub_color: Color32::GRAY,
            max_width: 442.0,
            pixels_per_point: 2.0,
            font_epoch: 0,
        }
    }

    #[test]
    fn a_rows_layout_key_follows_every_input_of_its_text_and_nothing_else() {
        let t = Theme::default();
        let mut item = Item::new("app:safari", "Safari");
        item.subtitle = "/Applications/Safari.app".into();
        let base = row_key(&item, &t, &look());
        assert_eq!(
            base,
            row_key(&item.clone(), &t, &look()),
            "the same row in the next frame"
        );

        let mut other = item.clone();
        other.title = "Safari Technology Preview".into();
        assert_ne!(base, row_key(&other, &t, &look()), "title");
        other = item.clone();
        other.subtitle.push('x');
        assert_ne!(base, row_key(&other, &t, &look()), "subtitle");
        other = item.clone();
        other.id = "app:other".into();
        assert_ne!(base, row_key(&other, &t, &look()), "id");

        let changed = |f: &dyn Fn(&mut RowLook)| {
            let mut l = look();
            f(&mut l);
            row_key(&item, &t, &l)
        };
        assert_ne!(base, changed(&|l| l.selected = true), "selection");
        assert_ne!(base, changed(&|l| l.fallback = true), "fallback");
        assert_ne!(base, changed(&|l| l.max_width = 441.5), "width");
        assert_ne!(base, changed(&|l| l.pixels_per_point = 1.0), "screen scale");
        assert_ne!(base, changed(&|l| l.font_epoch = 1), "a font was added");
        assert_ne!(base, changed(&|l| l.title_color = Color32::RED), "title colour");
        assert_ne!(base, changed(&|l| l.sub_color = Color32::RED), "subtitle colour");
        assert_ne!(
            base,
            changed(&|l| l.held_hint = Some("⌘ Reveal in Finder")),
            "a modifier hint"
        );
        assert_ne!(
            changed(&|l| l.held_hint = Some("")),
            base,
            "an empty hint hides the subtitle, which is not the same as no hint"
        );
        let bigger = Theme {
            result_text_size: t.result_text_size + 1.0,
            ..t.clone()
        };
        assert_ne!(base, row_key(&item, &bigger, &look()), "text size");
        let recoloured = Theme {
            result_text: [1, 2, 3, 255],
            ..t
        };
        assert_eq!(
            base,
            row_key(&item, &recoloured, &look()),
            "the theme colour reaches the key through the colours in the look"
        );
    }

    struct NoOs;
    impl ashdash_core::actions::Os for NoOs {
        fn run(&self, _: &ashdash_core::actions::Cmd) -> anyhow::Result<()> {
            panic!("nothing may run")
        }
        fn output(&self, _: &ashdash_core::actions::Cmd) -> anyhow::Result<Vec<u8>> {
            Ok(Vec::new())
        }
        fn spawn(&self, _: &ashdash_core::actions::Cmd) -> anyhow::Result<Box<dyn ashdash_core::actions::Process>> {
            panic!("nothing may run")
        }
    }

    #[test]
    fn the_hotkey_hint_names_the_hotkey_now_in_force() {
        let root = std::env::temp_dir().join(format!("oa-hint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let cfg = ashdash_core::Config {
            app_dirs: vec![],
            hotkey: "ctrl+alt+KeyJ".into(),
            ..Default::default()
        };
        let e = ashdash_core::Engine::with_env(
            ashdash_core::Paths::under(&root),
            cfg.clone(),
            Box::new(NoOs),
            root.join("home"),
        );
        let first = hotkey_hint(&e, "taken");
        let mut later = cfg;
        later.hotkey = "ctrl+alt+KeyK".into();
        e.apply_config(later);
        let second = hotkey_hint(&e, "taken");
        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(first, second, "the hint kept showing the start-up hotkey");
        assert!(second.contains('K') && !second.contains('J'), "{second}");
    }

    #[test]
    fn trims_library_boilerplate() {
        assert_eq!(
            short_error("invalid hotkey: Couldn't recognize \"x\" as a valid key, if you feel like it, report"),
            "invalid hotkey: Couldn't recognize \"x\" as a valid key"
        );
        assert_eq!(short_error("already registered"), "already registered");
    }
}
