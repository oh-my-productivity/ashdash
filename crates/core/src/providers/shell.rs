//! `> command` — run in Terminal.

use super::{Provider, ProviderOutput, QueryCtx};
use crate::item::{Action, Icon, Item};

pub struct ShellProvider;

impl Provider for ShellProvider {
    fn id(&self) -> &'static str {
        "shell"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let Some(cmd) = ctx.query.strip_prefix('>') else {
            return ProviderOutput::none();
        };
        let cmd = cmd.trim();
        let it = Item::new(
            "shell",
            if cmd.is_empty() {
                "Run a command in Terminal".to_string()
            } else {
                format!("Run in Terminal: {cmd}")
            },
        )
        .subtitle("↩ to run")
        .icon(Icon::Glyph(">"))
        .valid(!cmd.is_empty())
        .action(Action::Shell(cmd.to_string()));
        ProviderOutput::exclusive(vec![it])
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;
    use crate::item::Action;

    #[test]
    fn shell_prefix_is_exclusive_and_beats_everything() {
        let (e, root) = engine("shell");
        let r = e.query("> ls -l");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].action, Action::Shell("ls -l".into()));
        assert!(r[0].valid);
        // bare prefix: shown but not actionable
        let r = e.query(">");
        assert_eq!(r.len(), 1);
        assert!(!r[0].valid);
        let _ = std::fs::remove_dir_all(root);
    }
}
