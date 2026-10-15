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

use super::{LauncherApp, geometry, mac, style, trace};

/// Register the configured global hotkey. Returns the manager (which must be
/// kept alive), the toggle channel and whether registration succeeded.
pub(super) fn install_hotkey(
    engine: &Engine,
    ctx: &egui::Context,
) -> (Option<GlobalHotKeyManager>, Receiver<()>, bool) {
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
            eprintln!(
                "ashdash: press {} to open (Esc to close, Cmd+Q to quit)",
                engine.config.hotkey
            );
        }
        Err(e) => eprintln!("ashdash: could not register hotkey '{}': {e:#}", engine.config.hotkey),
    }
    (manager, toggle_rx, hotkey_ok)
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
        let width = self.theme.window_width;
        mac::style_launcher_window(super::WINDOW_TITLE, &style::window_style(&self.theme));
        let target = mac::screens_and_mouse()
            .and_then(|(monitors, mouse)| geometry::launcher_position(&monitors, mouse, f64::from(width), 0.2));
        if let Some((x, y)) = target {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(x as f32, y as f32)));
        } else if let Some(cmd) = egui::ViewportCommand::center_on_screen(ctx) {
            ctx.send_viewport_cmd(cmd);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    pub(super) fn hide(&mut self, ctx: &egui::Context) {
        if !self.visible {
            return;
        }
        trace::trace(|| "hide".into());
        self.visible = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        mac::hide_app();
    }

    /// Resize the viewport when the wanted height changed.
    pub(super) fn sync_height(&mut self, ctx: &egui::Context, total_h: f32) {
        if (total_h - self.last_height).abs() > 0.5 {
            self.last_height = total_h;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(Vec2::new(
                self.theme.window_width,
                total_h,
            )));
        }
    }
}
