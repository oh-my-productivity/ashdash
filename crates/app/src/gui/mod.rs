//! The launcher window (egui) plus the hotkey, clipboard watcher and
//! background workers that feed it.

mod geometry;
mod glue;
pub(crate) mod importprompt;
mod keynames;
mod keys;
mod largetype;
mod mac;
mod panel;
pub mod prefs;
mod quicklook;
mod results;
mod rowhint;
mod schedule;
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
use ashdash_core::{Engine, Item, Mods, Outcome};
use anyhow::{Result, anyhow};
use eframe::egui::{self, Color32, FontId, Pos2, Vec2};
use global_hotkey::GlobalHotKeyManager;

pub(crate) use window::wake_then_repaint;

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
    /// Frames drawn since the last `show()` (0 until the first fresh frame; see `reveal_if_drawn`).
    frames_since_show: u32,
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
    res_rx: Receiver<workers::QueryResult>,
    toggle_rx: Receiver<()>,
    outcome_rx: Receiver<Outcome>,
    outcome_tx: Sender<Outcome>,

    icons: HashMap<PathBuf, IconState>,
    icon_tx: Sender<(PathBuf, Option<PathBuf>)>,
    icon_rx: Receiver<(PathBuf, Option<PathBuf>)>,

    _hotkey_manager: Option<GlobalHotKeyManager>,
    /// Live reload of settings, workflows and snippets (see `prefs::reload`).
    hot_reload: prefs::reload::HotReload,
    /// CLI socket, URL scheme and workflow-hotkey events (see `crate::events`).
    automation: crate::events::Automation,
    /// Actions panel, buffer, import prompt (see `glue.rs`).
    glue: glue::Glue,
}

pub fn run(engine: Arc<Engine>, automation: crate::events::Automation) -> Result<()> {
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
            Ok(Box::new(LauncherApp::new(
                cc.egui_ctx.clone(),
                engine,
                theme,
                automation,
            )))
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
    fn new(ctx: egui::Context, engine: Arc<Engine>, theme: Theme, mut automation: crate::events::Automation) -> Self {
        let (req_tx, req_rx) = channel::<(u64, String)>();
        let (res_tx, res_rx) = channel::<workers::QueryResult>();
        workers::spawn_query_worker(Arc::clone(&engine), ctx.clone(), req_rx, res_tx);

        // Global hotkey.
        let (manager, toggle_rx, hotkey_error) = window::install_hotkey(&engine, &ctx);
        let hotkey_ok = hotkey_error.is_none();
        let hot_reload = prefs::reload::HotReload::new(
            &ctx,
            Arc::clone(&engine),
            hotkey_ok.then(|| engine.config.hotkey.clone()),
        );
        automation.attach(&ctx, manager.as_ref());

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
            frames_since_show: 0,
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
            hot_reload,
            automation,
            glue: glue::Glue::new(),
        };
        if !hotkey_ok {
            // Without a hotkey the window would be unreachable; show it now.
            app.show(&ctx);
        }
        app
    }

    fn request_query(&mut self) {
        if self.panel_open() {
            // the field filters the panel's rows; the engine is not asked
            self.refilter_panel();
            return;
        }
        self.generation += 1;
        self.sent_query = self.query.clone();
        if self.query.trim().is_empty() {
            self.items.clear();
            self.selected = 0;
            // tell the worker to stop repeating the previous query
            let _ = self.req_tx.send((self.generation, String::new()));
            return;
        }
        let _ = self.req_tx.send((self.generation, self.query.clone()));
    }

    fn activate(&mut self, ctx: &egui::Context, index: usize, mods: Mods) {
        self.activate_with(ctx, index, mods, true);
    }

    /// `fresh`: the key press behind this is a new press, not the auto-repeat of a held key.
    /// A repeat never answers a "press again to confirm" question.
    fn activate_with(&mut self, ctx: &egui::Context, index: usize, mods: Mods, fresh: bool) {
        let Some(item) = self.items.get(index).cloned() else {
            return;
        };
        if !item.valid {
            return;
        }
        // Rows the launcher interprets itself (Open With..., Quick Look, Install Workflow...)
        if self.handle_ui_row(ctx, index) {
            return;
        }
        // The engine alone decides what needs confirming (system commands,
        // Move to Trash, whatever the Cmd variant does); we only ask, once.
        let key = confirm_key(&item.id, mods);
        let prompt = self.engine.confirmation_for(&item, mods);
        match confirm_step(self.confirming.as_deref(), &key, prompt, fresh) {
            ConfirmStep::Ask(msg) => {
                self.confirming = Some(key);
                self.status = Some(msg);
                return;
            }
            ConfirmStep::Wait => return,
            ConfirmStep::Run => {}
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

/// What Return does with an item that may need confirming.
#[derive(Debug, PartialEq, Eq)]
enum ConfirmStep {
    /// Run it (confirmed, or nothing to confirm).
    Run,
    /// Show this question and wait for a second Return.
    Ask(String),
    /// The question is already up and this Return is only a key repeat: do nothing.
    Wait,
}

/// Identifies "this item with this action": Cmd+Return can run a different
/// (possibly destructive) action than Return, so a confirmation given for one
/// must not carry over to the other.
fn confirm_key(item_id: &str, mods: Mods) -> String {
    if mods.cmd {
        format!("{item_id}\u{0}cmd")
    } else {
        item_id.to_string()
    }
}

/// `armed` is the key already confirmed-for-asking by a previous Return,
/// `prompt` the engine's question for this item (None = harmless). `fresh` says the key press
/// is a new one: the answering press of a question must be, so a Return that is merely held
/// down (key repeat) can never confirm Shut Down or Move to Trash.
fn confirm_step(armed: Option<&str>, key: &str, prompt: Option<String>, fresh: bool) -> ConfirmStep {
    match prompt {
        Some(question) if armed != Some(key) => ConfirmStep::Ask(question),
        Some(_) if !fresh => ConfirmStep::Wait,
        _ => ConfirmStep::Run,
    }
}

impl LauncherApp {
    /// Events from the CLI socket, URL scheme, AppleScript and workflow hotkeys.
    fn poll_automation(&mut self, ctx: &egui::Context) {
        use crate::events::AppEvent::*;
        while let Some(ev) = self.automation.try_recv() {
            match ev {
                Show if !self.visible => self.show(ctx),
                Show => {}
                Search(text) => self.show_with_query(ctx, text),
                Prefill(text) => self.show_with_prefill(ctx, text),
                Reload => {
                    let engine = Arc::clone(&self.engine);
                    std::thread::spawn(move || engine.reload());
                    self.automation.reload_hotkeys(self._hotkey_manager.as_ref());
                }
                HotkeysRefreshed => self.automation.apply_hotkeys(self._hotkey_manager.as_ref()),
                Preferences => prefs::open_or_report(),
                Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Theme(file) => match ashdash_core::theme::Theme::load(&file) {
                    Ok(t) => self.theme = t,
                    Err(e) => eprintln!("ashdash: theme {}: {e:#}", file.display()),
                },
                Outcome(o) => drop(self.outcome_tx.send(o)),
            }
        }
    }
}

impl eframe::App for LauncherApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_background(ctx);
        self.poll_menu(ctx);
        self.poll_automation(ctx);
        self.reveal_if_drawn();
        self.poll_theme_epoch();
        self.hot_reload.poll(&mut prefs::reload::Host {
            paths: &self.engine.paths,
            manager: &mut self._hotkey_manager,
            hotkey_error: &mut self.hotkey_error,
            theme: &mut self.theme,
            automation: &mut self.automation,
        });
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if !self.visible {
            return;
        }
        self.frames_since_show = self.frames_since_show.saturating_add(1);
        if mac::is_cloaked() {
            // the next frame lifts the transparent state (reveal_if_drawn)
            ctx.request_repaint();
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
        self.apply_cursor_flag(&ctx);
        self.poll_dropped_files(&ctx);

        if self.large_type.is_some() {
            self.draw_large_type(ui, &ctx);
            return;
        }
        if self.overlay.is_some() {
            self.leave_overlay(&ctx);
        }

        let t = self.theme.clone();
        let prompt_h = self.import_prompt_height();
        let rows = if prompt_h.is_some() {
            0
        } else {
            self.items.len().min(VISIBLE_ROWS)
        };
        let hint = if prompt_h.is_some() { None } else { self.startup_hint() };
        let body_h = match (prompt_h, &hint) {
            (Some(h), _) => h,
            (None, Some(_)) => ROW_HEIGHT,
            (None, None) => rows as f32 * ROW_HEIGHT,
        };
        let footer = self.footer_text();
        if self.visible && self.engine.reload_pending() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let status_h = if footer.is_some() { 26.0 } else { 0.0 };
        let sep = if body_h > 0.0 { 9.0 } else { 0.0 };
        let total_h = header_height(&t) + sep + body_h + status_h;
        self.sync_height(&ctx, total_h);

        let frame = egui::Frame::new()
            .fill(c(t.window_color))
            .stroke(egui::Stroke::new(1.0, c(t.window_border)))
            .corner_radius(t.window_roundness)
            .inner_margin(egui::Margin::same(t.window_padding as i8));

        let search_hint = self.search_hint();
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
                        .hint_text(
                            egui::RichText::new(search_hint.clone()).color(c(t.search_text).gamma_multiply(0.35)),
                        )
                        .interactive(prompt_h.is_none())
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

            if prompt_h.is_some() {
                ui.add_space(9.0);
                self.draw_import_prompt(ui, &t);
            } else if let Some(text) = hint {
                ui.add_space(9.0);
                self.draw_hint_row(ui, &t, &text);
            } else if rows > 0 {
                ui.add_space(9.0);
                self.draw_results(ui, &ctx, &t);
            }

            if let Some(msg) = &footer {
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

#[cfg(test)]
mod tests {
    use ashdash_core::Action;
    use ashdash_core::actions::UniversalAction;
    use ashdash_core::config::{Config, Paths};

    use super::*;

    const ASK: &str = "Move \"a.txt\" to Trash? /tmp/a.txt. Press ↩ again to confirm.";

    #[test]
    fn a_harmless_item_runs_at_once() {
        assert_eq!(confirm_step(None, "k", None, true), ConfirmStep::Run);
        assert_eq!(confirm_step(Some("other"), "k", None, true), ConfirmStep::Run);
    }

    #[test]
    fn a_destructive_item_asks_first_and_runs_on_the_second_return() {
        assert_eq!(
            confirm_step(None, "k", Some(ASK.into()), true),
            ConfirmStep::Ask(ASK.into())
        );
        assert_eq!(confirm_step(Some("k"), "k", Some(ASK.into()), true), ConfirmStep::Run);
    }

    #[test]
    fn a_held_return_never_answers_the_question() {
        // the question is up (armed) and Return is still down: the repeats do nothing, however many
        for _ in 0..50 {
            assert_eq!(confirm_step(Some("k"), "k", Some(ASK.into()), false), ConfirmStep::Wait);
        }
        // only a fresh press runs it
        assert_eq!(confirm_step(Some("k"), "k", Some(ASK.into()), true), ConfirmStep::Run);
        // a repeat before anything was asked still asks (harmless) rather than running
        assert_eq!(
            confirm_step(None, "k", Some(ASK.into()), false),
            ConfirmStep::Ask(ASK.into())
        );
        // harmless items are unaffected by key repeat (holding Return on Open repeats Open as before)
        assert_eq!(confirm_step(None, "k", None, false), ConfirmStep::Run);
    }

    #[test]
    fn confirming_one_item_does_not_confirm_another() {
        assert_eq!(
            confirm_step(Some("a"), "b", Some(ASK.into()), true),
            ConfirmStep::Ask(ASK.into())
        );
    }

    #[test]
    fn plain_and_cmd_return_are_confirmed_separately() {
        let cmd = Mods {
            cmd: true,
            ..Mods::NONE
        };
        assert_ne!(confirm_key("file:x", Mods::NONE), confirm_key("file:x", cmd));
        // Return confirmed an Open; Cmd+Return (trash) must still ask
        let armed = confirm_key("file:x", Mods::NONE);
        assert_eq!(
            confirm_step(Some(&armed), &confirm_key("file:x", cmd), Some(ASK.into()), true),
            ConfirmStep::Ask(ASK.into())
        );
    }

    /// The real engine decides; the GUI only asks it. A fake Os keeps this off the machine.
    #[test]
    fn move_to_trash_from_the_actions_list_is_asked_about_through_the_engine() {
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
        let root = std::env::temp_dir().join(format!("oa-gui-confirm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let config = Config {
            app_dirs: vec![],
            ..Config::default()
        };
        let engine = Engine::with_env(Paths::under(&root), config, Box::new(NoOs), root.join("home"));

        let trash = Item::new("act:trash:/tmp/a.txt", "Move to Trash")
            .action(Action::Universal(UniversalAction::Trash(vec!["/tmp/a.txt".into()])));
        let question = engine.confirmation_for(&trash, Mods::NONE).expect("must ask");
        assert!(question.contains("/tmp/a.txt"), "{question}");

        // the sequence activate() follows: ask, then (same key armed) run
        let key = confirm_key(&trash.id, Mods::NONE);
        assert_eq!(
            confirm_step(None, &key, Some(question.clone()), true),
            ConfirmStep::Ask(question.clone())
        );
        assert_eq!(confirm_step(Some(&key), &key, Some(question), true), ConfirmStep::Run);

        // and a harmless action never asks
        let open = Item::new("act:open:/tmp/a.txt", "Open")
            .action(Action::Universal(UniversalAction::Open("/tmp/a.txt".into())));
        assert_eq!(engine.confirmation_for(&open, Mods::NONE), None);
        let _ = std::fs::remove_dir_all(root);
    }
}
