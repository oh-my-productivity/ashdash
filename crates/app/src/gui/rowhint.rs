//! What a result row says about itself while a modifier key is held
//! (the subtitle is swapped for the modifier's hint). Limited to what `ashdash_core::Item` exposes:
//! `cmd_action` and a workflow's per-modifier overrides.

use ashdash_core::{Action, Item, Mods};

/// Modifier glyphs in Apple's order: ⌃⌥⇧⌘ (plus 🌐 for fn).
pub fn mods_glyphs(m: Mods) -> String {
    let mut s = String::new();
    for (on, g) in [
        (m.ctrl, "⌃"),
        (m.alt, "⌥"),
        (m.shift, "⇧"),
        (m.cmd, "⌘"),
        (m.fn_key, "fn "),
    ] {
        if on {
            s.push_str(g);
        }
    }
    s
}

pub fn describe_action(a: &Action) -> String {
    match a {
        Action::None => "Do nothing".into(),
        Action::Open(p) => format!("Open {}", file_name(p)),
        Action::Reveal(_) => "Reveal in Finder".into(),
        Action::OpenUrl(u) => format!("Open {u}"),
        Action::Copy(_) => "Copy to clipboard".into(),
        Action::System(c) => c.title().to_string(),
        Action::Shell(_) => "Run in Terminal".into(),
        Action::Workflow(_) => "Run workflow action".into(),
        Action::Universal(_) => "Run action".into(),
        Action::Clipboard(c) => describe_clip(c).into(),
    }
}

fn describe_clip(c: &ashdash_core::clipboard::ClipCommand) -> &'static str {
    use ashdash_core::clipboard::ClipCommand as C;
    match c {
        C::Restore { .. } => "Copy to clipboard",
        C::Paste { .. } | C::PasteText { .. } => "Paste into the frontmost app",
        C::Pin { pinned: true, .. } => "Pin",
        C::Pin { pinned: false, .. } => "Unpin",
        C::Delete { .. } => "Delete from history",
        C::Clear | C::ClearAll => "Clear clipboard history",
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name()
        .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The line to show instead of the subtitle while `mods` are held, if the
/// item reacts to them. `None` = keep showing the normal subtitle.
pub fn mod_hint(item: &Item, mods: Mods) -> Option<String> {
    if mods.is_none() {
        return None;
    }
    if let Action::Workflow(wa) = &item.action
        && let Some((_, ov)) = wa.mods.iter().find(|(m, _)| *m == mods)
    {
        return Some(if ov.valid == Some(false) {
            format!("{} Not available", mods_glyphs(mods))
        } else if let Some(sub) = ov.subtitle.as_deref().filter(|s| !s.is_empty()) {
            format!("{} {}", mods_glyphs(mods), sub)
        } else {
            format!("{} Alternate action", mods_glyphs(mods))
        });
    }
    let only_cmd = mods
        == Mods {
            cmd: true,
            ..Mods::NONE
        };
    if only_cmd && let Some(a) = &item.cmd_action {
        return Some(format!("⌘ {}", describe_action(a)));
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use ashdash_core::workflow::{ModOverride, WorkflowAction};

    use super::*;

    const CMD: Mods = Mods {
        cmd: true,
        alt: false,
        ctrl: false,
        shift: false,
        fn_key: false,
    };

    #[test]
    fn glyphs_use_apple_order() {
        let m = Mods {
            cmd: true,
            alt: true,
            shift: true,
            ctrl: true,
            fn_key: false,
        };
        assert_eq!(mods_glyphs(m), "⌃⌥⇧⌘");
        assert_eq!(mods_glyphs(Mods::NONE), "");
    }

    #[test]
    fn cmd_action_is_described() {
        let mut it = Item::new("a", "Safari");
        it.cmd_action = Some(Action::Reveal(PathBuf::from("/Applications/Safari.app")));
        assert_eq!(mod_hint(&it, CMD).as_deref(), Some("⌘ Reveal in Finder"));
        assert_eq!(mod_hint(&it, Mods::NONE), None);
        // other modifiers do not trigger the cmd variant
        let alt = Mods {
            alt: true,
            ..Mods::NONE
        };
        assert_eq!(mod_hint(&it, alt), None);
    }

    #[test]
    fn items_without_variants_have_no_hint() {
        assert_eq!(mod_hint(&Item::new("a", "x"), CMD), None);
    }

    #[test]
    fn workflow_overrides_are_surfaced() {
        let alt = Mods {
            alt: true,
            ..Mods::NONE
        };
        let wa = WorkflowAction {
            workflow: "w".into(),
            from_uid: "u".into(),
            arg: String::new(),
            vars: BTreeMap::new(),
            mods: vec![
                (
                    CMD,
                    ModOverride {
                        arg: Some("x".into()),
                        subtitle: Some("Open the docs".into()),
                        ..Default::default()
                    },
                ),
                (
                    alt,
                    ModOverride {
                        valid: Some(false),
                        ..Default::default()
                    },
                ),
            ],
            ..Default::default()
        };
        let it = Item::new("w", "t").action(Action::Workflow(Box::new(wa)));
        assert_eq!(mod_hint(&it, CMD).as_deref(), Some("⌘ Open the docs"));
        assert_eq!(mod_hint(&it, alt).as_deref(), Some("⌥ Not available"));
    }
}
