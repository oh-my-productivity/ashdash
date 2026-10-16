//! The channel through which every automation entry point (CLI socket, URL
//! scheme, AppleScript, workflow hotkeys) talks to the launcher window.
//!
//! Producers hold an [`EventSender`]; the window owns the [`Automation`] and
//! drains it once per frame (`LauncherApp::poll_automation`). Nothing here
//! touches UI state directly, so the producers are testable with a plain
//! [`EventSink`] fake.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};

use ashdash_core::Outcome;
use eframe::egui;
use global_hotkey::GlobalHotKeyManager;

/// Something the launcher window should do on behalf of an automation caller.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// Show the launcher, like the global hotkey.
    Show,
    /// Show the launcher with this text typed into the search field.
    Search(String),
    /// Re-scan applications, workflows and snippets; re-register workflow hotkeys.
    Reload,
    /// The background scan of the workflow folders finished: register the hotkeys it found.
    HotkeysRefreshed,
    Quit,
    /// Open the preferences window (a separate process).
    Preferences,
    /// Apply an `.alfredappearance` file.
    Theme(std::path::PathBuf),
    /// Present the result of something that ran without the window (a hotkey,
    /// a URL): Large Type text, or an error/confirmation message.
    Outcome(Outcome),
}

/// Where producers send events. Implemented by [`EventSender`] and by test fakes.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: AppEvent);
}

#[derive(Clone)]
pub struct EventSender {
    tx: Sender<AppEvent>,
    ctx: Arc<OnceLock<egui::Context>>,
}

impl EventSink for EventSender {
    fn emit(&self, event: AppEvent) {
        // A closed receiver means the window is gone (quitting); nothing to do.
        let _ = self.tx.send(event);
        if let Some(ctx) = self.ctx.get() {
            ctx.request_repaint();
        }
    }
}

/// The receiving end, created with the sender and handed to [`Automation::new`].
pub struct Inbox {
    rx: Receiver<AppEvent>,
    ctx: Arc<OnceLock<egui::Context>>,
}

/// Build the producer and consumer halves.
pub fn channel() -> (EventSender, Inbox) {
    let (tx, rx) = mpsc::channel();
    let ctx = Arc::new(OnceLock::new());
    (
        EventSender {
            tx,
            ctx: Arc::clone(&ctx),
        },
        Inbox { rx, ctx },
    )
}

/// The window-side half: the inbox plus the workflow-hotkey registrations and
/// the macOS URL handler. Owned by `LauncherApp`.
pub struct Automation {
    inbox: Inbox,
    hotkeys: crate::hotkeys::WorkflowHotkeys,
    url: Option<Arc<crate::urlscheme::UrlDispatcher>>,
    /// Held so the Apple Event handler outlives `attach`.
    _url_handler: Option<crate::urlscheme::AppleEventHandler>,
}

impl Automation {
    pub fn new(
        inbox: Inbox,
        hotkeys: crate::hotkeys::WorkflowHotkeys,
        url: Option<Arc<crate::urlscheme::UrlDispatcher>>,
    ) -> Self {
        // Early registration catches the URL that launched the app, which
        // AppKit may deliver before the window exists. AppKit installs its own
        // handler during launch, so `attach` registers again afterwards.
        let _url_handler = url.clone().and_then(crate::urlscheme::install_apple_event_handler);
        Automation {
            inbox,
            hotkeys,
            url,
            _url_handler,
        }
    }

    /// Called once from the window constructor (main thread): lets producers
    /// wake the UI, installs the macOS URL handler and registers the hotkeys.
    pub fn attach(&mut self, ctx: &egui::Context, _manager: Option<&GlobalHotKeyManager>) {
        let _ = self.inbox.ctx.set(ctx.clone());
        self._url_handler = self.url.clone().and_then(crate::urlscheme::install_apple_event_handler);
        self.hotkeys.refresh_in_background();
    }

    /// Re-read the workflow folders for hotkeys. The scan runs on a background thread; the
    /// registrations follow when [`AppEvent::HotkeysRefreshed`] arrives
    /// ([`apply_hotkeys`](Self::apply_hotkeys)). Nothing here touches the disk.
    pub fn reload_hotkeys(&mut self, _manager: Option<&GlobalHotKeyManager>) {
        self.hotkeys.refresh_in_background();
    }

    /// Register the hotkeys of the latest scan (main thread only; reads no folders).
    pub fn apply_hotkeys(&mut self, manager: Option<&GlobalHotKeyManager>) {
        self.hotkeys.apply(manager);
    }

    /// Follow a live settings change: the workflow folders the next
    /// [`reload_hotkeys`](Self::reload_hotkeys) scans.
    pub fn set_workflow_dirs(&mut self, paths: &ashdash_core::Paths, dirs: &[std::path::PathBuf]) {
        self.hotkeys.set_extra_dirs(paths, dirs);
    }

    /// The launcher hotkey now in force (workflow hotkeys may not take it).
    pub fn set_launcher_hotkey(&mut self, hotkey: Option<global_hotkey::hotkey::HotKey>) {
        self.hotkeys.set_launcher_hotkey(hotkey);
    }

    pub fn try_recv(&self) -> Option<AppEvent> {
        self.inbox.rx.try_recv().ok()
    }
}

/// A sink that stores events, for tests.
#[cfg(test)]
#[derive(Default)]
pub struct VecSink(pub std::sync::Mutex<Vec<AppEvent>>);

#[cfg(test)]
impl EventSink for VecSink {
    fn emit(&self, event: AppEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[cfg(test)]
impl VecSink {
    pub fn take(&self) -> Vec<AppEvent> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}
