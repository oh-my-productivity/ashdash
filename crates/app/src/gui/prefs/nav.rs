//! The sidebar's entries: which page each shows, its Cmd+digit shortcut, and the name filter
//! of the "Go to a setting…" field. Pure, so it is tested without a window.

use eframe::egui::{Key, Modifiers};

use crate::prefs_model::Page;

/// One sidebar entry. The old Features page is shown as three entries (it has the sections of
/// all three); the other pages map one to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Nav {
    #[default]
    General,
    Search,
    Clipboard,
    Snippets,
    Workflows,
    Permissions,
    About,
}

/// The parts of the Features page an entry shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeaturesPart {
    Search,
    Clipboard,
    Snippets,
}

impl Nav {
    pub const ALL: [Nav; 7] = [
        Nav::General,
        Nav::Search,
        Nav::Clipboard,
        Nav::Snippets,
        Nav::Workflows,
        Nav::Permissions,
        Nav::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Nav::General => "General",
            Nav::Search => "Search",
            Nav::Clipboard => "Clipboard",
            Nav::Snippets => "Snippets",
            Nav::Workflows => "Workflows",
            Nav::Permissions => "Permissions",
            Nav::About => "About",
        }
    }

    /// What the page header says under the title: the settings on it, nothing promotional.
    pub fn subtitle(self) -> &'static str {
        match self {
            Nav::General => "Shortcut · theme · results · start-up",
            Nav::Search => "Features · files · bookmarks · web searches",
            Nav::Clipboard => "History limits · ignored apps",
            Nav::Snippets => "Snippets · collections · import",
            Nav::Workflows => "Import · installed · per-workflow settings",
            Nav::Permissions => "Accessibility · automation · disk access",
            Nav::About => "Version · licence · folders",
        }
    }

    /// The window page whose data this entry needs loaded.
    pub fn page(self) -> Page {
        match self {
            Nav::General => Page::General,
            Nav::Search | Nav::Clipboard | Nav::Snippets => Page::Features,
            Nav::Workflows => Page::Workflows,
            Nav::Permissions => Page::Permissions,
            Nav::About => Page::About,
        }
    }

    pub fn features_part(self) -> Option<FeaturesPart> {
        match self {
            Nav::Search => Some(FeaturesPart::Search),
            Nav::Clipboard => Some(FeaturesPart::Clipboard),
            Nav::Snippets => Some(FeaturesPart::Snippets),
            _ => None,
        }
    }

    /// 1-based position, which is also the Cmd+digit shortcut.
    pub fn number(self) -> usize {
        Nav::ALL.iter().position(|n| *n == self).map_or(0, |i| i + 1)
    }

    /// The shortcut as drawn in the sidebar.
    pub fn shortcut_label(self) -> String {
        format!("⌘{}", self.number())
    }

    pub fn shortcut_key(self) -> Key {
        const KEYS: [Key; 9] = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        KEYS[self.number().saturating_sub(1).min(KEYS.len() - 1)]
    }

    /// The entry that Cmd + `key` selects, if any.
    pub fn from_shortcut(mods: Modifiers, key: Key) -> Option<Nav> {
        if !mods.matches_exact(Modifiers::COMMAND) {
            return None;
        }
        Nav::ALL.into_iter().find(|n| n.shortcut_key() == key)
    }
}

/// Entries whose name contains `query` (case-insensitive, surrounding blanks ignored); all of
/// them for an empty query.
pub fn filter(query: &str) -> Vec<Nav> {
    let q = query.trim().to_lowercase();
    Nav::ALL
        .into_iter()
        .filter(|n| q.is_empty() || n.title().to_lowercase().contains(&q))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_are_cmd_1_to_n_in_sidebar_order() {
        for (i, n) in Nav::ALL.into_iter().enumerate() {
            assert_eq!(n.number(), i + 1);
            assert_eq!(n.shortcut_label(), format!("⌘{}", i + 1));
            assert_eq!(Nav::from_shortcut(Modifiers::COMMAND, n.shortcut_key()), Some(n));
        }
    }

    #[test]
    fn permissions_is_cmd_6_and_about_moved_to_cmd_7() {
        assert_eq!(Nav::Permissions.shortcut_label(), "⌘6");
        assert_eq!(Nav::About.shortcut_label(), "⌘7");
        assert_eq!(
            Nav::from_shortcut(Modifiers::COMMAND, Key::Num6),
            Some(Nav::Permissions)
        );
        assert_eq!(Nav::from_shortcut(Modifiers::COMMAND, Key::Num7), Some(Nav::About));
    }

    #[test]
    fn only_a_plain_cmd_digit_selects() {
        assert_eq!(Nav::from_shortcut(Modifiers::NONE, Key::Num2), None);
        assert_eq!(Nav::from_shortcut(Modifiers::ALT, Key::Num2), None);
        assert_eq!(
            Nav::from_shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::Num2),
            None
        );
        assert_eq!(Nav::from_shortcut(Modifiers::COMMAND, Key::Num9), None);
        assert_eq!(Nav::from_shortcut(Modifiers::COMMAND, Key::A), None);
    }

    #[test]
    fn filter_matches_names_only_and_ignores_case_and_blanks() {
        assert_eq!(filter(""), Nav::ALL.to_vec());
        assert_eq!(filter("  "), Nav::ALL.to_vec());
        assert_eq!(filter("CLIP"), vec![Nav::Clipboard]);
        assert_eq!(filter(" work "), vec![Nav::Workflows]);
        assert_eq!(
            filter("s"),
            vec![Nav::Search, Nav::Snippets, Nav::Workflows, Nav::Permissions]
        );
        assert_eq!(filter("perm"), vec![Nav::Permissions]);
        assert!(filter("zzz").is_empty());
    }

    #[test]
    fn every_window_page_is_reachable_and_the_features_page_is_split_in_three() {
        for page in Page::ALL {
            assert!(Nav::ALL.iter().any(|n| n.page() == page), "{page:?}");
        }
        let parts: Vec<_> = Nav::ALL.iter().filter_map(|n| n.features_part()).collect();
        assert_eq!(
            parts,
            vec![FeaturesPart::Search, FeaturesPart::Clipboard, FeaturesPart::Snippets]
        );
    }

    #[test]
    fn subtitles_are_plain_descriptions() {
        for n in Nav::ALL {
            let s = n.subtitle().to_lowercase();
            assert!(!s.is_empty());
            assert!(!s.contains("simple") && !s.contains("faster"), "{s}");
        }
    }
}
