//! Showing, hiding and positioning the launcher window, viewport sizing and
//! the global hotkey.

use std::str::FromStr;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use anyhow::{Result, anyhow};
use ashdash_core::Engine;
use eframe::egui::{self, Pos2, Vec2};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use std::time::Duration;

use super::statusitem::MenuCmd;
use super::{LauncherApp, geometry, mac, style, trace};

/// A woken window is made opaque after this long even if no frame was drawn (safety net, never expected).
const REVEAL_DEADLINE: Duration = Duration::from_millis(300);

/// Register the configured global hotkey. Returns the manager (which must be
/// kept alive), the toggle channel and the registration error, if any.
pub(super) fn install_hotkey(
    engine: &Engine,
    ctx: &egui::Context,
) -> (Option<GlobalHotKeyManager>, Receiver<()>, Option<String>) {
    let (toggle_tx, toggle_rx) = channel::<()>();
    let mut manager = None;
    let mut error = None;
    // Installed even when the registration below fails: the handler can only be set once per
    // process, and live reload (gui/prefs/reload.rs) may register a working hotkey later.
    {
        let ctx = ctx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |ev: GlobalHotKeyEvent| {
            trace::trace(|| "hotkey event".into());
            // Workflow hotkeys share this process-wide handler (crate::hotkeys).
            crate::hotkeys::route(&ev);
            if ev.id == super::prefs::hotkey::main_id() && ev.state == HotKeyState::Pressed {
                let _ = toggle_tx.send(());
                wake_then_repaint(&ctx);
            }
        }));
    }
    match register_hotkey(&engine.config.hotkey) {
        Ok((m, id)) => {
            super::prefs::hotkey::set_main_id(id);
            manager = Some(m);
            eprintln!(
                "ashdash: press {} to open (Esc to close, Cmd+Q to quit)",
                engine.config.hotkey
            );
        }
        Err(e) => {
            eprintln!("ashdash: could not register hotkey '{}': {e:#}", engine.config.hotkey);
            error = Some(format!("{e:#}"));
        }
    }
    (manager, toggle_rx, error)
}

fn register_hotkey(spec: &str) -> Result<(GlobalHotKeyManager, u32)> {
    let manager = GlobalHotKeyManager::new().map_err(|e| anyhow!("{e}"))?;
    let hotkey = HotKey::from_str(spec).map_err(|e| anyhow!("invalid hotkey: {e}"))?;
    manager.register(hotkey).map_err(|e| anyhow!("{e}"))?;
    Ok((manager, hotkey.id()))
}

/// Ask the UI loop to run now. Used by whoever asks for the launcher from outside it (hotkey handler,
/// IPC / URL / AppleScript events): a hidden window is first made "visible but transparent"
/// ([`mac::wake_hidden_launcher`]) because eframe repaints invisible windows only every 100 ms.
/// Does nothing special when the window is already up.
pub(crate) fn wake_then_repaint(ctx: &egui::Context) {
    let ctx = ctx.clone();
    mac::wake_hidden_launcher(super::WINDOW_TITLE, move || ctx.request_repaint());
}

impl LauncherApp {
    /// Called at the start of every frame: lifts the "woken but transparent" state of the window once the
    /// first frame with the fresh content has been drawn. If the request turned out to be a hide, the window
    /// stays transparent and is ordered out by `hide()`; the next `show()` reveals it.
    pub(super) fn reveal_if_drawn(&mut self) {
        if mac::is_cloaked()
            && self.visible
            && (self.frames_since_show > 0 || self.shown_at.elapsed() > REVEAL_DEADLINE)
        {
            trace::trace(|| "reveal".into());
            mac::reveal(super::WINDOW_TITLE);
        }
    }

    pub(super) fn show(&mut self, ctx: &egui::Context) {
        trace::trace(|| "show".into());
        self.visible = true;
        self.frames_since_show = 0;
        self.focus_seen = None;
        self.focus_watch.reset();
        self.shown_at = Instant::now();
        self.want_focus = true;
        self.query.clear();
        self.sent_query.clear();
        self.items.clear();
        self.selected = 0;
        self.large_type = None;
        self.confirming = None;
        self.status = None;
        self.glue.reset();
        self.cancel_query();
        self.last_height = 0.0;
        self.last_pointer = None;
        self.pointer_moved = false;
        self.overlay = None;
        mac::style_launcher_window(super::WINDOW_TITLE, &style::window_style(&self.theme));
        self.position_launcher(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    pub(super) fn hide(&mut self, ctx: &egui::Context) {
        if !self.visible {
            return;
        }
        trace::trace(|| "hide".into());
        self.visible = false;
        self.glue.reset();
        self.cancel_query();
        self.quicklook.close();
        self.quicklook_for = None;
        self.overlay = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        mac::hide_app();
    }

    /// Put the launcher on the monitor that contains the mouse.
    pub(super) fn position_launcher(&self, ctx: &egui::Context) {
        let width = f64::from(self.theme.window_width);
        let target = mac::screens_and_mouse()
            .and_then(|(monitors, mouse)| geometry::launcher_position(&monitors, mouse, width, 0.2));
        if let Some((x, y)) = target {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(x as f32, y as f32)));
        } else if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) {
            ctx.send_viewport_cmd(cmd);
        }
    }

    /// Handle menu-bar status item clicks.
    pub(super) fn poll_menu(&mut self, ctx: &egui::Context) {
        while let Ok(cmd) = self.menu_rx.try_recv() {
            match cmd {
                MenuCmd::Show => {
                    if !self.visible {
                        self.show(ctx);
                    }
                }
                MenuCmd::Reload => {
                    let engine = std::sync::Arc::clone(&self.engine);
                    std::thread::spawn(move || engine.reload());
                    self.automation.reload_hotkeys(self._hotkey_manager.as_ref());
                }
                MenuCmd::Preferences => super::prefs::open_or_report(),
                MenuCmd::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
    }

    /// Resize the viewport when the wanted height changed. The very first
    /// resize after startup can be dropped by the OS, so while the real
    /// content height still differs, the command is repeated a few times.
    pub(super) fn sync_height(&mut self, ctx: &egui::Context, total_h: f32) {
        let changed = (total_h - self.last_height).abs() > 0.5;
        if changed {
            self.last_height = total_h;
            self.resize_retries = 0;
        }
        let actual = ctx.content_rect().height();
        let stuck = !changed && (actual - total_h).abs() > 1.5 && self.resize_retries < 10;
        if changed || stuck {
            if stuck {
                self.resize_retries += 1;
                ctx.request_repaint_after(std::time::Duration::from_millis(80));
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(
                self.theme.window_width,
                total_h,
            )));
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
    }
}
