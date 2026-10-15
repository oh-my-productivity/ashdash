//! Menu-bar status item (NSStatusItem) with Show / Reload / Preferences / Quit.
//!
//! Done with plain objc2 instead of `tray-icon`: no extra dependency, and no
//! second owner of the main-thread event loop. Menu clicks are forwarded to
//! the egui app over a channel (`MenuCmd`) and handled in `LauncherApp::logic`.

use std::sync::mpsc::Sender;

use eframe::egui;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_foundation::NSString;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCmd {
    Show,
    Reload,
    Quit,
}

struct Ivars {
    tx: Sender<MenuCmd>,
    ctx: egui::Context,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AshdashMenuTarget"]
    #[ivars = Ivars]
    struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(showLauncher:))]
        fn show_launcher(&self, _sender: Option<&AnyObject>) {
            self.send(MenuCmd::Show);
        }

        #[unsafe(method(reloadIndex:))]
        fn reload_index(&self, _sender: Option<&AnyObject>) {
            self.send(MenuCmd::Reload);
        }

        #[unsafe(method(quitLauncher:))]
        fn quit_launcher(&self, _sender: Option<&AnyObject>) {
            self.send(MenuCmd::Quit);
        }
    }

    unsafe impl NSObjectProtocol for MenuTarget {}
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, ivars: Ivars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    fn send(&self, cmd: MenuCmd) {
        let iv = self.ivars();
        let _ = iv.tx.send(cmd);
        iv.ctx.request_repaint();
    }
}

/// Keeps the status item and its action target alive.
pub struct StatusItem {
    _item: Retained<NSStatusItem>,
    _target: Retained<MenuTarget>,
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<objc2::runtime::Sel>,
    target: &MenuTarget,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(""),
        )
    };
    if action.is_some() {
        unsafe { item.setTarget(Some(target)) };
    }
    item
}

/// Install the status item. Returns `None` when not on the main thread.
pub fn install(tx: Sender<MenuCmd>, ctx: egui::Context) -> Option<StatusItem> {
    let mtm = MainThreadMarker::new()?;
    let target = MenuTarget::new(mtm, Ivars { tx, ctx });

    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    menu.addItem(&menu_item(mtm, "Show Ashdash", Some(sel!(showLauncher:)), &target));
    menu.addItem(&menu_item(mtm, "Reload", Some(sel!(reloadIndex:)), &target));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    // TODO(prefs-window): enable once the preferences window exists.
    let prefs = menu_item(mtm, "Preferences…", None, &target);
    prefs.setEnabled(false);
    menu.addItem(&prefs);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item(mtm, "Quit Ashdash", Some(sel!(quitLauncher:)), &target));

    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("magnifyingglass"),
            Some(&NSString::from_str("Ashdash")),
        );
        match symbol {
            Some(img) => {
                img.setTemplate(true);
                button.setImage(Some(&img));
            }
            None => button.setTitle(&NSString::from_str("A")),
        }
    }
    item.setMenu(Some(&menu));
    Some(StatusItem {
        _item: item,
        _target: target,
    })
}
