//! The legacy XML Script Filter format, converted to the JSON shape so the
//! rest of the pipeline handles one representation.
//!
//! ```xml
//! <?xml version="1.0"?>
//! <output>
//!   <rerun>1</rerun>
//!   <variables><variable name="k">v</variable></variables>
//!   <items>
//!     <item uid="u" arg="a" valid="yes" autocomplete="x" type="file">
//!       <title>T</title><subtitle>S</subtitle><subtitle mod="cmd">S2</subtitle>
//!       <icon type="fileicon">~/Desktop</icon>
//!       <text type="copy">c</text><text type="largetype">l</text>
//!       <variables><variable name="k">v</variable></variables>
//!     </item>
//!   </items>
//! </output>
//! ```
//!
//! Both the `<output>` wrapper (with `<rerun>` and `<variables>`) and a bare
//! `<items>` root are accepted.
//! The parser is deliberately small: elements, attributes, text, CDATA,
//! comments, processing instructions and the five predefined plus numeric
//! entities. It does not validate and does not resolve DTD entities.

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value, json};

const MAX_DEPTH: usize = 64;

#[derive(Debug, Default)]
struct Elem {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Elem>,
    text: String,
}

impl Elem {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
    fn child(&self, name: &str) -> Option<&Elem> {
        self.children.iter().find(|c| c.name == name)
    }
}

fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn parse_tag(inner: &str) -> Result<(Elem, bool)> {
    let self_closing = inner.ends_with('/');
    let inner = inner.trim_end_matches('/').trim();
    let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
    let mut el = Elem {
        name: inner[..name_end].to_string(),
        ..Default::default()
    };
    if el.name.is_empty() {
        bail!("empty tag name");
    }
    let mut rest = inner[name_end..].trim_start();
    while !rest.is_empty() {
        let eq = rest
            .find('=')
            .ok_or_else(|| anyhow!("attribute without value in <{}>", el.name))?;
        let key = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let quote = after
            .chars()
            .next()
            .filter(|c| *c == '"' || *c == '\'')
            .ok_or_else(|| anyhow!("unquoted attribute '{key}'"))?;
        let body = &after[1..];
        let close = body
            .find(quote)
            .ok_or_else(|| anyhow!("unterminated attribute '{key}'"))?;
        el.attrs.push((key, decode(&body[..close])));
        rest = body[close + 1..].trim_start();
    }
    Ok((el, self_closing))
}

fn parse(src: &str) -> Result<Elem> {
    let mut stack: Vec<Elem> = vec![Elem::default()]; // synthetic document root
    let mut i = 0;
    let b = src.as_bytes();
    while i < b.len() {
        if b[i] != b'<' {
            let next = src[i..].find('<').map(|n| i + n).unwrap_or(src.len());
            let top = stack.last_mut().expect("stack never empty");
            top.text.push_str(&decode(&src[i..next]));
            i = next;
            continue;
        }
        let rest = &src[i..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").ok_or_else(|| anyhow!("unterminated comment"))?;
            i += end + 3;
        } else if rest.starts_with("<![CDATA[") {
            let end = rest.find("]]>").ok_or_else(|| anyhow!("unterminated CDATA"))?;
            stack
                .last_mut()
                .expect("stack never empty")
                .text
                .push_str(&rest[9..end]);
            i += end + 3;
        } else if rest.starts_with("<?") {
            let end = rest
                .find("?>")
                .ok_or_else(|| anyhow!("unterminated processing instruction"))?;
            i += end + 2;
        } else if rest.starts_with("<!") {
            let end = rest.find('>').ok_or_else(|| anyhow!("unterminated declaration"))?;
            i += end + 1;
        } else if rest.starts_with("</") {
            let end = rest.find('>').ok_or_else(|| anyhow!("unterminated end tag"))?;
            let name = rest[2..end].trim();
            if stack.len() < 2 {
                bail!("unexpected </{name}>");
            }
            let done = stack.pop().expect("checked above");
            if done.name != name {
                bail!("mismatched </{name}>, expected </{}>", done.name);
            }
            stack.last_mut().expect("document root remains").children.push(done);
            i += end + 1;
        } else {
            let end = rest.find('>').ok_or_else(|| anyhow!("unterminated tag"))?;
            let (el, self_closing) = parse_tag(&rest[1..end])?;
            if self_closing {
                stack.last_mut().expect("stack never empty").children.push(el);
            } else {
                if stack.len() > MAX_DEPTH {
                    bail!("XML nested too deeply");
                }
                stack.push(el);
            }
            i += end + 1;
        }
    }
    if stack.len() != 1 {
        bail!("unclosed <{}>", stack.last().map(|e| e.name.as_str()).unwrap_or("?"));
    }
    let mut doc = stack.pop().expect("document root");
    if doc.children.len() == 1 {
        Ok(doc.children.remove(0))
    } else if doc.children.is_empty() {
        bail!("no root element")
    } else {
        Ok(doc)
    }
}

fn variables_of(e: &Elem) -> Map<String, Value> {
    let mut m = Map::new();
    if let Some(vs) = e.child("variables") {
        for v in vs.children.iter().filter(|c| c.name == "variable") {
            if let Some(n) = v.attr("name") {
                m.insert(n.to_string(), Value::String(v.text.trim().to_string()));
            }
        }
    }
    m
}

fn yes(s: &str) -> bool {
    !matches!(s.trim().to_ascii_lowercase().as_str(), "no" | "false" | "0")
}

fn item_to_json(it: &Elem) -> Value {
    let mut o = Map::new();
    for key in ["uid", "type", "autocomplete"] {
        if let Some(v) = it.attr(key) {
            o.insert(key.into(), json!(v));
        }
    }
    if let Some(v) = it.attr("arg") {
        o.insert("arg".into(), json!(v));
    }
    if let Some(v) = it.attr("valid") {
        o.insert("valid".into(), json!(yes(v)));
    }
    let mut mods = Map::new();
    let mut text = Map::new();
    for c in &it.children {
        let t = c.text.trim();
        match c.name.as_str() {
            "title" => {
                o.insert("title".into(), json!(t));
            }
            "subtitle" => match c.attr("mod") {
                Some(m) => {
                    mods.entry(m.to_string())
                        .or_insert_with(|| json!({}))
                        .as_object_mut()
                        .expect("object")
                        .insert("subtitle".into(), json!(t));
                }
                None => {
                    o.insert("subtitle".into(), json!(t));
                }
            },
            "arg" => {
                o.insert("arg".into(), json!(t));
            }
            "valid" => {
                o.insert("valid".into(), json!(yes(t)));
            }
            "autocomplete" => {
                o.insert("autocomplete".into(), json!(t));
            }
            "icon" => {
                let mut icon = json!({"path": t});
                if let Some(ty) = c.attr("type") {
                    icon["type"] = json!(ty);
                }
                o.insert("icon".into(), icon);
            }
            "text" => {
                if let Some(ty) = c.attr("type") {
                    text.insert(ty.to_string(), json!(t));
                }
            }
            "quicklookurl" => {
                o.insert("quicklookurl".into(), json!(t));
            }
            _ => {}
        }
    }
    if !mods.is_empty() {
        o.insert("mods".into(), Value::Object(mods));
    }
    if !text.is_empty() {
        o.insert("text".into(), Value::Object(text));
    }
    let vars = variables_of(it);
    if !vars.is_empty() {
        o.insert("variables".into(), Value::Object(vars));
    }
    Value::Object(o)
}

/// Convert an XML Script Filter document to the JSON object shape
/// (`{"items": [...], "variables": {...}, "rerun": n, ...}`).
pub(super) fn to_json(src: &str) -> Result<Value> {
    let root = parse(src)?;
    let (container, items_el) = if root.name == "items" {
        (&root, &root)
    } else {
        let items = root
            .child("items")
            .ok_or_else(|| anyhow!("XML script filter has no <items> element"))?;
        (&root, items)
    };
    let mut out = Map::new();
    out.insert(
        "items".into(),
        Value::Array(
            items_el
                .children
                .iter()
                .filter(|c| c.name == "item")
                .map(item_to_json)
                .collect(),
        ),
    );
    if container.name != "items" {
        let vars = variables_of(container);
        if !vars.is_empty() {
            out.insert("variables".into(), Value::Object(vars));
        }
        if let Some(r) = container.child("rerun").and_then(|r| r.text.trim().parse::<f64>().ok()) {
            out.insert("rerun".into(), json!(r));
        }
        if let Some(s) = container.child("skipknowledge") {
            out.insert("skipknowledge".into(), json!(yes(s.text.trim())));
        }
        if let Some(c) = container.child("cache") {
            let seconds = c
                .attr("seconds")
                .map(str::to_string)
                .or_else(|| c.child("seconds").map(|s| s.text.trim().to_string()))
                .and_then(|s| s.parse::<u64>().ok());
            if let Some(seconds) = seconds {
                let loose = c
                    .attr("loosereload")
                    .map(str::to_string)
                    .or_else(|| c.child("loosereload").map(|s| s.text.trim().to_string()))
                    .is_some_and(|s| yes(&s) && !s.is_empty());
                out.insert("cache".into(), json!({"seconds": seconds, "loosereload": loose}));
            }
        }
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_template_shape_with_variables_and_rerun() {
        let v = to_json(
            r#"<?xml version="1.0"?>
<output>
  <rerun>1</rerun>
  <variables>
    <variable name="counter">3</variable>
  </variables>
  <items>
    <item arg="Script run 3 times">
      <title>Variable Counter</title>
      <subtitle>run 3 times</subtitle>
    </item>
  </items>
</output>"#,
        )
        .unwrap();
        assert_eq!(v["rerun"], json!(1.0));
        assert_eq!(v["variables"]["counter"], "3");
        assert_eq!(v["items"][0]["arg"], "Script run 3 times");
        assert_eq!(v["items"][0]["title"], "Variable Counter");
    }

    #[test]
    fn legacy_items_root_with_attributes_mods_icons_text() {
        let v = to_json(
            r#"<items>
  <item uid="u1" arg="a&amp;b" valid="no" autocomplete="ac" type="file">
    <title>Caf&#233; &lt;x&gt;</title>
    <subtitle>plain</subtitle>
    <subtitle mod="cmd">with cmd</subtitle>
    <icon type="fileicon">~/Desktop</icon>
    <text type="copy">cp</text>
    <text type="largetype"><![CDATA[a < b]]></text>
    <variables><variable name="k">v</variable></variables>
  </item>
  <item><title>Second</title><icon>img.png</icon></item>
</items>"#,
        )
        .unwrap();
        let it = &v["items"][0];
        assert_eq!(it["uid"], "u1");
        assert_eq!(it["arg"], "a&b");
        assert_eq!(it["valid"], false);
        assert_eq!(it["autocomplete"], "ac");
        assert_eq!(it["type"], "file");
        assert_eq!(it["title"], "Café <x>");
        assert_eq!(it["subtitle"], "plain");
        assert_eq!(it["mods"]["cmd"]["subtitle"], "with cmd");
        assert_eq!(it["icon"], json!({"path": "~/Desktop", "type": "fileicon"}));
        assert_eq!(it["text"]["copy"], "cp");
        assert_eq!(it["text"]["largetype"], "a < b");
        assert_eq!(it["variables"]["k"], "v");
        assert_eq!(v["items"][1]["icon"], json!({"path": "img.png"}));
        assert!(v.get("variables").is_none());
    }

    #[test]
    fn bad_documents_are_errors_not_panics() {
        for bad in [
            "",
            "<items>",
            "<items></oops>",
            "<items><item></items>",
            "<a b=c></a>",
            "<!-- x",
            "<output/>",
        ] {
            assert!(to_json(bad).is_err(), "{bad:?}");
        }
        let deep = "<a>".repeat(200);
        assert!(to_json(&deep).is_err());
    }

    #[test]
    fn entities_decode_and_stray_ampersands_survive() {
        assert_eq!(decode("a &amp; b &#x41; &#66; &bogus; & c"), "a & b A B &bogus; & c");
    }
}
