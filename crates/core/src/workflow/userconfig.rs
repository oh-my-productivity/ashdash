//! Workflow configuration sheet (`userconfigurationconfig` in `info.plist`).
//!
//! Each entry declares one workflow variable with a UI control (text field,
//! checkbox, popup, slider, file picker, text area) and a default. Entries
//! become workflow variables, so they reach scripts as environment variables
//! and templates as `{var:name}`.
//!
//! The per-entry layout is `variable`, `label`, `description`, `type` and
//! `config.default`; `variablesdontexport` lists variables left out of exports.

use serde_json::Value;

use super::model::stringify;

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigVar {
    pub variable: String,
    pub label: String,
    pub description: String,
    /// `textfield`, `textarea`, `checkbox`, `popupbutton`, `slider`, `filepicker`, ...
    pub kind: String,
    /// Default as the string scripts see (checkboxes are "1"/"0").
    pub default: String,
}

fn value_string(v: &Value) -> String {
    match v {
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => stringify(other),
    }
}

pub(super) fn parse(v: Option<&Value>) -> Vec<ConfigVar> {
    let mut out = Vec::new();
    for e in v.and_then(Value::as_array).into_iter().flatten() {
        let Some(variable) = e.get("variable").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
            continue;
        };
        let text = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let cfg = e.get("config");
        // Sliders call it `defaultvalue`, everything else `default`.
        let default = cfg
            .and_then(|c| c.get("default").or_else(|| c.get("defaultvalue")))
            .map(value_string)
            .unwrap_or_default();
        out.push(ConfigVar {
            variable: variable.to_string(),
            label: text("label"),
            description: text("description"),
            kind: text("type"),
            default,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_are_stringified_per_control_type() {
        let v = json!([
            {"variable": "name", "type": "textfield", "label": "Name", "config": {"default": "Ann"}},
            {"variable": "loud", "type": "checkbox", "config": {"default": true}},
            {"variable": "quiet", "type": "checkbox", "config": {"default": false}},
            {"variable": "level", "type": "slider", "config": {"defaultvalue": 3.0}},
            {"variable": "ratio", "type": "slider", "config": {"defaultvalue": 0.5}},
            {"variable": "", "type": "textfield"},
            {"type": "textfield"},
            {"variable": "bare", "type": "textarea"}
        ]);
        let cfg = parse(Some(&v));
        let d: Vec<(&str, &str)> = cfg.iter().map(|c| (c.variable.as_str(), c.default.as_str())).collect();
        assert_eq!(
            d,
            vec![
                ("name", "Ann"),
                ("loud", "1"),
                ("quiet", "0"),
                ("level", "3"),
                ("ratio", "0.5"),
                ("bare", "")
            ]
        );
        assert_eq!(cfg[0].label, "Name");
    }
}
