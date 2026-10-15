//! Calculator: `= expr` or anything that looks like an expression.

use super::{Provider, ProviderOutput, QueryCtx};
use crate::calc;
use crate::item::{Action, Icon, Item};

pub struct CalcProvider;

impl Provider for CalcProvider {
    fn id(&self) -> &'static str {
        "calc"
    }

    fn query(&self, ctx: &QueryCtx) -> ProviderOutput {
        let q = ctx.query;
        let expr = q
            .strip_prefix('=')
            .map(str::trim)
            .or_else(|| calc::looks_like_expression(q).then_some(q));
        let Some(v) = expr.and_then(calc::eval) else {
            return ProviderOutput::none();
        };
        let s = calc::format(v);
        let mut it = Item::new(format!("calc:{s}"), s.clone())
            .subtitle(format!("{} = {s} — ↩ to copy", expr.unwrap_or("")))
            .icon(Icon::Glyph("="))
            .action(Action::Copy(s.clone()))
            .score(5000.0);
        it.large_type = Some(s.clone());
        it.copy_text = Some(s);
        ProviderOutput::ranked(vec![it])
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::engine;

    #[test]
    fn result_outranks_apps_and_is_not_exclusive() {
        let (e, root) = engine("calc");
        let r = e.query("2+3*4");
        assert_eq!(r[0].title, "14");
        // merged with other results and followed by fallbacks, so not exclusive
        assert!(r.last().unwrap().id.starts_with("fallback:"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn equals_prefix_forces_calculation() {
        let (e, root) = engine("calc-eq");
        assert_eq!(e.query("= sqrt(81)")[0].title, "9");
        let _ = std::fs::remove_dir_all(root);
    }
}
