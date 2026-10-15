//! The launcher window (egui) plus the hotkey, clipboard watcher and
//! background workers that feed it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use eframe::egui::{self, Color32, FontId, Key, Modifiers, Pos2, Rect, Sense, Vec2};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use ashdash_core::theme::{Rgba, Theme};
use ashdash_core::{apps, Action, Engine, Icon, Item, Mods, Outcome};

const VISIBLE_ROWS: usize = 9;
const ROW_HEIGHT: f32 = 48.0;

fn c(v: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(v[0], v[1], v[2], v[3])
}

enum IconState {
    Pending,
    Ready(Option<PathBuf>),
}

struct LauncherApp {
    engine: Arc<Engine>,
    theme: Theme,

    query: String,
    sent_query: String,
    items: Vec<Item>,
    selected: usize,
    scroll_to_selected: bool,

    visible: bool,
    shown_at: Instant,
    want_focus: bool,
    last_height: f32,
    last_pointer: Option<Pos2>,

    status: Option<String>,
    confirming: Option<String>,
    large_type: Option<String>,

    generation: u64,
    req_tx: Sender<(u64, String)>,
    res_rx: Receiver<(u64, Vec<Item>)>,
    toggle_rx: Receiver<()>,
    outcome_rx: Receiver<Outcome>,
    outcome_tx: Sender<Outcome>,

    icons: HashMap<PathBuf, IconState>,
    icon_tx: Sender<(PathBuf, Option<PathBuf>)>,
    icon_rx: Receiver<(PathBuf, Option<PathBuf>)>,

    _hotkey_manager: Option<GlobalHotKeyManager>,
}

pub fn run(engine: Arc<Engine>) -> Result<()> {
    let theme = match &engine.config.theme {
        Some(p) => Theme::load(p).unwrap_or_else(|e| {
            eprintln!("ashdash: could not load theme {}: {e:#}", p.display());
            Theme::default()
        }),
        None => Theme::default(),
    };

    let viewport = egui::ViewportBuilder::default()
        .with_title("Ashdash")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_resizable(false)
        .with_visible(false)
        .with_inner_size([theme.window_width, header_height(&theme)]);

    let options = eframe::NativeOptions {
        viewport,
        event_loop_builder: Some(Box::new(|builder| {
            // Menu-bar style app: no Dock icon, never steals the menu bar.
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Accessory);
        })),
        ..Default::default()
    };

    eframe::run_native(
        "Ashdash",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(LauncherApp::new(cc.egui_ctx.clone(), engine, theme)))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}

fn query_id() -> egui::Id {
    egui::Id::new("ashdash-query")
}

fn header_height(t: &Theme) -> f32 {
    t.window_padding * 2.0 + t.search_size * 1.5
}

impl LauncherApp {
    fn new(ctx: egui::Context, engine: Arc<Engine>, theme: Theme) -> Self {
        // Query worker: always answers the newest request, dropping stale ones.
        let (req_tx, req_rx) = channel::<(u64, String)>();
        let (res_tx, res_rx) = channel::<(u64, Vec<Item>)>();
        {
            let engine = Arc::clone(&engine);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                while let Ok(mut req) = req_rx.recv() {
                    while let Ok(newer) = req_rx.try_recv() {
                        req = newer;
                    }
                    let items = engine.query(&req.1);
                    if res_tx.send((req.0, items)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }

        // Global hotkey.
        let (toggle_tx, toggle_rx) = channel::<()>();
        let mut manager = None;
        let mut hotkey_ok = false;
        match register_hotkey(&engine.config.hotkey) {
            Ok((m, id)) => {
                let ctx = ctx.clone();
                GlobalHotKeyEvent::set_event_handler(Some(move |ev: GlobalHotKeyEvent| {
                    if ev.id == id && ev.state == HotKeyState::Pressed {
                        let _ = toggle_tx.send(());
                        ctx.request_repaint();
                    }
                }));
                manager = Some(m);
                hotkey_ok = true;
                eprintln!("ashdash: press {} to open (Esc to close, Cmd+Q to quit)", engine.config.hotkey);
            }
            Err(e) => eprintln!("ashdash: could not register hotkey '{}': {e:#}", engine.config.hotkey),
        }

        spawn_clipboard_watcher(Arc::clone(&engine));

        let (icon_tx, icon_rx) = channel();
        let (outcome_tx, outcome_rx) = channel();
        let mut app = LauncherApp {
            engine,
            theme,
            query: String::new(),
            sent_query: String::new(),
            items: Vec::new(),
            selected: 0,
            scroll_to_selected: false,
            visible: false,
            shown_at: Instant::now(),
            want_focus: false,
            last_height: 0.0,
            last_pointer: None,
            status: None,
            confirming: None,
            large_type: None,
            generation: 0,
            req_tx,
            res_rx,
            toggle_rx,
            outcome_rx,
            outcome_tx,
            icons: HashMap::new(),
            icon_tx,
            icon_rx,
            _hotkey_manager: manager,
        };
        if !hotkey_ok {
            // Without a hotkey the window would be unreachable; show it now.
            app.show(&ctx);
        }
        app
    }

    fn show(&mut self, ctx: &egui::Context) {
        self.visible = true;
        self.shown_at = Instant::now();
        self.want_focus = true;
        self.query.clear();
        self.sent_query.clear();
        self.items.clear();
        self.selected = 0;
        self.large_type = None;
        self.confirming = None;
        self.status = None;
        self.generation += 1;
        self.last_height = 0.0;
        self.last_pointer = None;
        let width = self.theme.window_width;
        let monitor = ctx.input(|i| i.viewport().monitor_size);
        if let Some(m) = monitor {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(
                ((m.x - width) / 2.0).max(0.0),
                m.y * 0.2,
            )));
        } else if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) {
            ctx.send_viewport_cmd(cmd);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn hide(&mut self, ctx: &egui::Context) {
        if !self.visible {
            return;
        }
        self.visible = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        mac::hide_app();
    }

    fn request_query(&mut self) {
        self.generation += 1;
        self.sent_query = self.query.clone();
        if self.query.trim().is_empty() {
            self.items.clear();
            self.selected = 0;
            return;
        }
        let _ = self.req_tx.send((self.generation, self.query.clone()));
    }

    fn activate(&mut self, ctx: &egui::Context, index: usize, mods: Mods) {
        let Some(item) = self.items.get(index).cloned() else { return };
        if !item.valid {
            return;
        }
        if let Action::System(cmd) = &item.action {
            if cmd.needs_confirmation() && self.confirming.as_deref() != Some(item.id.as_str()) {
                self.confirming = Some(item.id.clone());
                self.status = Some(format!("{}? Press Return again to confirm.", cmd.title()));
                return;
            }
        }
        let query = self.query.clone();
        self.hide(ctx);
        let engine = Arc::clone(&self.engine);
        let tx = self.outcome_tx.clone();
        let ctx2 = ctx.clone();
        std::thread::spawn(move || {
            let out = engine.execute(&item, &query, mods, true);
            let _ = tx.send(out);
            ctx2.request_repaint();
        });
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        while self.toggle_rx.try_recv().is_ok() {
            if self.visible {
                self.hide(ctx);
            } else {
                self.show(ctx);
            }
        }
        while let Ok((generation, items)) = self.res_rx.try_recv() {
            if generation == self.generation {
                self.items = items;
                self.selected = self.selected.min(self.items.len().saturating_sub(1));
                self.scroll_to_selected = true;
            }
        }
        while let Ok((app, png)) = self.icon_rx.try_recv() {
            self.icons.insert(app, IconState::Ready(png));
        }
        while let Ok(outcome) = self.outcome_rx.try_recv() {
            match outcome {
                Outcome::Done => {}
                Outcome::LargeType(t) => {
                    self.show(ctx);
                    self.large_type = Some(t);
                }
                Outcome::Confirm(m) | Outcome::Error(m) => {
                    self.show(ctx);
                    self.status = Some(m);
                }
            }
        }
    }

    fn icon_uri(&mut self, ctx: &egui::Context, icon: &Icon) -> Option<String> {
        match icon {
            Icon::Image(p) => Some(format!("file://{}", p.display())),
            Icon::App(app) => {
                match self.icons.get(app) {
                    Some(IconState::Ready(Some(png))) => return Some(format!("file://{}", png.display())),
                    Some(_) => return None,
                    None => {}
                }
                self.icons.insert(app.clone(), IconState::Pending);
                let (tx, app, cache, ctx) = (self.icon_tx.clone(), app.clone(), self.engine.paths.icon_cache(), ctx.clone());
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

impl eframe::App for LauncherApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_background(ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.visible {
            return;
        }

        // Close when the window loses focus (after a short grace period while
        // the OS hands focus over).
        let focused = ctx.input(|i| i.viewport().focused);
        if focused == Some(false) && self.shown_at.elapsed() > Duration::from_millis(400) {
            self.hide(&ctx);
            return;
        }

        self.handle_keys(&ctx);
        if !self.visible {
            return;
        }

        let t = self.theme.clone();
        let rows = self.items.len().min(VISIBLE_ROWS);
        let large = self.large_type.is_some();
        let body_h = if large { 220.0 } else { rows as f32 * ROW_HEIGHT };
        let status_h = if self.status.is_some() { 26.0 } else { 0.0 };
        let sep = if body_h > 0.0 { 9.0 } else { 0.0 };
        let total_h = header_height(&t) + sep + body_h + status_h;
        if (total_h - self.last_height).abs() > 0.5 {
            self.last_height = total_h;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(t.window_width, total_h)));
        }

        let frame = egui::Frame::new()
            .fill(c(t.window_color))
            .stroke(egui::Stroke::new(1.0, c(t.window_border)))
            .corner_radius(t.window_roundness)
            .inner_margin(egui::Margin::same(t.window_padding as i8));

        frame.show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            // Search field
            egui::Frame::new()
                .fill(c(t.search_background))
                .corner_radius(t.search_roundness)
                .inner_margin(egui::Margin::symmetric(8, 2))
                .show(ui, |ui| {
                    let edit = egui::TextEdit::singleline(&mut self.query)
                        .font(FontId::proportional(t.search_size))
                        .text_color(c(t.search_text))
                        .hint_text(egui::RichText::new("Ashdash").color(c(t.search_text).gamma_multiply(0.35)))
                        .frame(egui::Frame::NONE)
                        .id(query_id())
                        .desired_width(f32::INFINITY);
                    let resp = ui.add(edit);
                    if self.want_focus || !resp.has_focus() {
                        resp.request_focus();
                        self.want_focus = false;
                    }
                    if resp.changed() {
                        self.confirming = None;
                        self.status = None;
                        self.large_type = None;
                        self.selected = 0;
                        self.request_query();
                    }
                });

            if let Some(text) = self.large_type.clone() {
                ui.add_space(9.0);
                egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(text).size(32.0).color(c(t.result_text_selected))).wrap());
                });
            } else if rows > 0 {
                ui.add_space(9.0);
                self.draw_results(ui, &ctx, &t);
            }

            if let Some(msg) = &self.status {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(msg).size(t.result_subtext_size + 1.0).color(c(t.result_subtext_selected)));
            }
        });
    }
}

impl LauncherApp {
    fn handle_keys(&mut self, ctx: &egui::Context) {
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
        let keys = ctx.input_mut(|i| {
            let m = i.modifiers;
            let mods = Mods { cmd: m.mac_cmd, alt: m.alt, ctrl: m.ctrl, shift: m.shift, fn_key: false };
            let mut k = Keys { up: false, down: false, enter: None, tab: false, esc: false, large: false, quit: false, number: None };
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
            for (n, key) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9]
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
        if keys.tab {
            if let Some(a) = self.items.get(self.selected).and_then(|i| i.autocomplete.clone()) {
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
        }
        if keys.large {
            if let Some(item) = self.items.get(self.selected) {
                self.large_type = Some(item.large_type.clone().unwrap_or_else(|| item.title.clone()));
            }
        }
        if let Some(n) = keys.number {
            if n < self.items.len().min(VISIBLE_ROWS) {
                self.activate(ctx, n, Mods::NONE);
                return;
            }
        }
        if let Some(mods) = keys.enter {
            if self.large_type.is_some() {
                self.large_type = None;
            } else {
                self.activate(ctx, self.selected, mods);
            }
        }
    }

    fn draw_results(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, t: &Theme) {
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
                        if let (Some(p), Some(prev)) = (pos, self.last_pointer) {
                            if p.distance(prev) > 2.0 {
                                self.selected = idx;
                            }
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
                            egui::Image::new(uri).fit_to_exact_size(Vec2::splat(icon_size)).paint_at(ui, icon_rect);
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
                    let title_y = if has_sub { rect.top() + 6.0 } else { rect.center().y - t.result_text_size * 0.6 };
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
                            c(if sel { t.result_subtext_selected } else { t.result_subtext }),
                        );
                    }
                    if idx < 9 {
                        painter.text(
                            Pos2::new(rect.right() - 12.0, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            format!("⌘{}", idx + 1),
                            FontId::proportional(t.result_subtext_size + 2.0),
                            c(if sel { t.result_subtext_selected } else { t.result_subtext }),
                        );
                    }
                }
            });
        self.last_pointer = ctx.input(|i| i.pointer.hover_pos()).or(self.last_pointer);
        self.scroll_to_selected = false;
        if let Some(i) = clicked {
            let mods = ctx.input(|i| Mods { cmd: i.modifiers.mac_cmd, alt: i.modifiers.alt, ctrl: i.modifiers.ctrl, shift: i.modifiers.shift, fn_key: false });
            self.activate(ctx, i, mods);
        }
    }
}

fn register_hotkey(spec: &str) -> Result<(GlobalHotKeyManager, u32)> {
    let manager = GlobalHotKeyManager::new().map_err(|e| anyhow!("{e}"))?;
    let hotkey = HotKey::from_str(spec).map_err(|e| anyhow!("invalid hotkey: {e}"))?;
    manager.register(hotkey).map_err(|e| anyhow!("{e}"))?;
    Ok((manager, hotkey.id()))
}

/// Poll the system clipboard and record changes in the history.
fn spawn_clipboard_watcher(engine: Arc<Engine>) {
    std::thread::spawn(move || {
        let Ok(mut board) = arboard::Clipboard::new() else { return };
        let history = engine.clipboard();
        let mut last = board.get_text().ok();
        loop {
            std::thread::sleep(Duration::from_millis(600));
            let now = board.get_text().ok();
            if now != last {
                if let Some(text) = &now {
                    history.lock().unwrap().push(text);
                }
                last = now;
            }
        }
    });
}

mod mac {
    /// Hide the whole application so macOS returns focus to the previous app.
    pub fn hide_app() {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSApplication;
        if let Some(mtm) = MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm).hide(None);
        }
    }
}
