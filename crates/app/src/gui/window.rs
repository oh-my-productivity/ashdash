//! Showing, hiding and positioning the launcher window, viewport sizing and
//! the global hotkey.

use std::str::FromStr;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use ashdash_core::Engine;
use anyhow::{Result, anyhow};
use eframe::egui::{self, Pos2, Vec2};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use super::statusitem::MenuCmd;
use super::{LauncherApp, geometry, mac, style, trace};

/// Register the configured global hotkey. Returns the manager (which must be
/// kept alive), the toggle channel and the registration error, if any.
pub(super) fn install_hotkey(
    engine: &Engine,
    ctx: &egui::Context,
) -> (Option<GlobalHotKeyManager>, Receiver<()>, Option<String>) {
    let (toggle_tx, toggle_rx) = channel::<()>();
    let mut manager = None;
    let mut error = None;
    match register_hotkey(&engine.config.hotkey) {
        Ok((m, id)) => {
            let ctx = ctx.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |ev: GlobalHotKeyEvent| {
                // Workflow hotkeys share this process-wide handler (crate::hotkeys).
                crate::hotkeys::route(&ev);
                if ev.id == id && ev.state == HotKeyState::Pressed {
                    let _ = toggle_tx.send(());
                    ctx.request_repaint();
                }
            }));
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

impl LauncherApp {
    pub(super) fn show(&mut self, ctx: &egui::Context) {
        trace::trace(|| "show".into());
        self.visible = true;
        self.focus_seen = None;
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
