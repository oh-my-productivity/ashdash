//! Background threads (query worker, clipboard watcher) and the per-frame
//! polling that drains their channels into the UI state.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use ashdash_core::{Engine, Item, Outcome};
use eframe::egui;

use super::{IconState, LauncherApp};

/// Query worker: always answers the newest request, dropping stale ones.
pub(super) fn spawn_query_worker(
    engine: Arc<Engine>,
    ctx: egui::Context,
    req_rx: Receiver<(u64, String)>,
    res_tx: Sender<(u64, Vec<Item>)>,
) {
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

/// Watch the system pasteboard and record changes in the history.
pub(super) fn spawn_clipboard_watcher(engine: Arc<Engine>) {
    crate::clipboard_watch::spawn(engine);
}

impl LauncherApp {
    pub(super) fn poll_background(&mut self, ctx: &egui::Context) {
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
}
