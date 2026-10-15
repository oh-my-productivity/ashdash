//! The launcher window (egui) plus the hotkey, clipboard watcher and
//! background workers that feed it.

mod geometry;
mod keynames;
mod keys;
mod largetype;
mod mac;
mod quicklook;
mod results;
mod rowhint;
mod statusitem;
mod style;
mod trace;
mod window;
mod workers;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use ashdash_core::theme::{Rgba, Theme};
use ashdash_core::{Action, Engine, Item, Mods, Outcome};
use anyhow::{Result, anyhow};
use eframe::egui::{self, Color32, FontId, Pos2, Vec2};
use global_hotkey::GlobalHotKeyManager;

const WINDOW_TITLE: &str = "Ashdash";
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
    resize_retries: u8,
    last_pointer: Option<Pos2>,
    focus_seen: Option<bool>,

    /// Set while the window is stretched over a monitor for Large Type.
    overlay: Option<geometry::Rect>,
    quicklook: quicklook::QuickLook,
    quicklook_for: Option<String>,
    quicklook_was_open: bool,
    shift_tap: quicklook::ShiftTap,
    started: Instant,
    /// Why the global hotkey could not be registered, if it could not.
    hotkey_error: Option<String>,
    menu_rx: Receiver<statusitem::MenuCmd>,
    _status_item: Option<statusitem::StatusItem>,

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
        .with_title(WINDOW_TITLE)
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
        let (req_tx, req_rx) = channel::<(u64, String)>();
        let (res_tx, res_rx) = channel::<(u64, Vec<Item>)>();
        workers::spawn_query_worker(Arc::clone(&engine), ctx.clone(), req_rx, res_tx);

        // Global hotkey.
        let (manager, toggle_rx, hotkey_error) = window::install_hotkey(&engine, &ctx);
        let hotkey_ok = hotkey_error.is_none();

        let (menu_tx, menu_rx) = channel();
        let status_item = statusitem::install(menu_tx, ctx.clone());

        workers::spawn_clipboard_watcher(Arc::clone(&engine));

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
            resize_retries: 0,
            last_pointer: None,
            focus_seen: None,
            overlay: None,
            quicklook: quicklook::QuickLook::default(),
            quicklook_for: None,
            quicklook_was_open: false,
            shift_tap: quicklook::ShiftTap::default(),
            started: Instant::now(),
            hotkey_error,
            menu_rx,
            _status_item: status_item,
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
        let Some(item) = self.items.get(index).cloned() else {
            return;
        };
        if !item.valid {
            return;
        }
        if let Action::System(cmd) = &item.action
            && cmd.needs_confirmation()
            && self.confirming.as_deref() != Some(item.id.as_str())
        {
            self.confirming = Some(item.id.clone());
            self.status = Some(format!("{}? Press Return again to confirm.", cmd.title()));
            return;
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
}

impl eframe::App for LauncherApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_background(ctx);
        self.poll_menu(ctx);
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
        // the OS hands focus over). While a Quick Look preview is up, focus
        // legitimately lives in the preview process.
        let focused = ctx.input(|i| i.viewport().focused);
        if focused != self.focus_seen {
            self.focus_seen = focused;
            let since = self.shown_at.elapsed().as_millis();
            trace::trace(|| format!("focus={focused:?} +{since}ms after show"));
        }
        self.tick_quicklook(&ctx);
        if focused == Some(false) && self.shown_at.elapsed() > Duration::from_millis(400) && !self.quicklook.is_open() {
            self.hide(&ctx);
            return;
        }

        self.handle_keys(&ctx);
        if !self.visible {
            return;
        }

        if self.large_type.is_some() {
            self.draw_large_type(ui, &ctx);
            return;
        }
        if self.overlay.is_some() {
            self.leave_overlay(&ctx);
        }

        let t = self.theme.clone();
        let rows = self.items.len().min(VISIBLE_ROWS);
        let hint = self.startup_hint();
        let body_h = if hint.is_some() {
            ROW_HEIGHT
        } else {
            rows as f32 * ROW_HEIGHT
        };
        let status_h = if self.status.is_some() { 26.0 } else { 0.0 };
        let sep = if body_h > 0.0 { 9.0 } else { 0.0 };
        let total_h = header_height(&t) + sep + body_h + status_h;
        self.sync_height(&ctx, total_h);

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
                        self.selected = 0;
                        trace::trace(|| format!("query={:?}", self.query));
                        self.request_query();
                    }
                });

            if let Some(text) = hint {
                ui.add_space(9.0);
                self.draw_hint_row(ui, &t, &text);
            } else if rows > 0 {
                ui.add_space(9.0);
                self.draw_results(ui, &ctx, &t);
            }

            if let Some(msg) = &self.status {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(msg)
                        .size(t.result_subtext_size + 1.0)
                        .color(c(t.result_subtext_selected)),
                );
            }
        });
    }
}
