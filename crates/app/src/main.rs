mod clipboard_watch;
mod gui;

use std::path::PathBuf;
use std::sync::Arc;

use ashdash_core::theme::Theme;
use ashdash_core::{Config, Engine, Mods, Outcome, Paths};
use anyhow::{Context, Result, bail};

const HELP: &str = "\
ashdash — a keyboard launcher for macOS

USAGE:
  ashdash                         start the launcher (default hotkey: Option+Shift+Space)
  ashdash query <text>            print the ranked results for <text>
  ashdash run <text> [index]      run result <index> (default 0) for <text>
  ashdash import <file>           install an .alfredworkflow package
  ashdash workflows               list installed workflows
  ashdash trigger <bundle-id> <trigger-id> [argument]
                                      fire a workflow's External Trigger
  ashdash theme <file>            parse an .alfredappearance file and print it
  ashdash paths                   print data / config locations
  ashdash config path             print the settings file location
  ashdash config get [key]        print all settings, or one by dotted path (clipboard.max_entries)
  ashdash config set <key> <value>
                                      change one setting (JSON values; lists may be comma-separated)
  ashdash config keys             list every setting name
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = Paths::system();
    let config = Config::load(&paths);

    match args.first().map(String::as_str) {
        None => {
            let engine = Arc::new(Engine::new(paths, config));
            gui::run(engine)
        }
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            Ok(())
        }
        Some("paths") => {
            println!("data:      {}", paths.data.display());
            println!("config:    {}", paths.config_file().display());
            println!("workflows: {}", paths.workflows_dir().display());
            println!("snippets:  {}", paths.snippets_file().display());
            println!("cache:     {}", paths.cache.display());
            Ok(())
        }
        Some("config") => config_command(&paths, &args[1..]),
        Some("query") => {
            let text = args[1..].join(" ");
            let engine = Engine::new(paths, config);
            for (i, it) in engine.query(&text).iter().enumerate() {
                println!(
                    "{i:>2}  {:<40}  {}{}",
                    it.title,
                    it.subtitle,
                    if it.valid { "" } else { "  [not actionable]" }
                );
            }
            Ok(())
        }
        Some("run") => {
            let Some(text) = args.get(1) else {
                bail!("usage: ashdash run <text> [index]")
            };
            let idx: usize = match args.get(2) {
                Some(s) => s.parse().context("index must be a number")?,
                None => 0,
            };
            let engine = Engine::new(paths, config);
            let items = engine.query(text);
            let item = items
                .get(idx)
                .with_context(|| format!("no result #{idx} for '{text}'"))?;
            println!("running: {} — {}", item.title, item.subtitle);
            let mut outcome = engine.execute(item, text, Mods::NONE, false);
            if let Outcome::Confirm(msg) = &outcome {
                eprintln!("{msg} (re-run with ASHDASH_CONFIRM=1 to confirm)");
                if std::env::var_os("ASHDASH_CONFIRM").is_some() {
                    outcome = engine.execute(item, text, Mods::NONE, true);
                }
            }
            report(outcome)
        }
        Some("import") => {
            let Some(file) = args.get(1) else {
                bail!("usage: ashdash import <file.alfredworkflow>")
            };
            let engine = Engine::new(paths, config);
            let dest = engine.import_workflow(&PathBuf::from(file))?;
            println!("installed to {}", dest.display());
            Ok(())
        }
        Some("workflows") => {
            let engine = Engine::new(paths, config);
            for (name, bundle, disabled) in engine.workflow_summaries() {
                println!("{name}  [{bundle}]{}", if disabled { "  (disabled)" } else { "" });
            }
            Ok(())
        }
        Some("trigger") => {
            let (Some(bundle), Some(trigger)) = (args.get(1), args.get(2)) else {
                bail!("usage: ashdash trigger <bundle-id> <trigger-id> [argument]")
            };
            let engine = Engine::new(paths, config);
            report(engine.run_trigger(bundle, trigger, args.get(3).map(String::as_str).unwrap_or("")))
        }
        Some("theme") => {
            let Some(file) = args.get(1) else {
                bail!("usage: ashdash theme <file>")
            };
            println!("{:#?}", Theme::load(&PathBuf::from(file))?);
            Ok(())
        }
        Some(other) => bail!("unknown command '{other}'\n\n{HELP}"),
    }
}

/// `ashdash config path|get|set|keys`.
fn config_command(paths: &Paths, args: &[String]) -> Result<()> {
    use ashdash_core::settings_view;
    const USAGE: &str = "usage: ashdash config path | get [key] | set <key> <value> | keys";
    match args.first().map(String::as_str) {
        Some("path") => {
            println!("{}", paths.config_file().display());
            Ok(())
        }
        Some("get") => {
            let config = Config::load_strict(paths)?;
            match settings_view::get(&config, args.get(1).map(String::as_str).unwrap_or(""))? {
                serde_json::Value::String(s) => println!("{s}"),
                v => println!("{}", serde_json::to_string_pretty(&v)?),
            }
            Ok(())
        }
        Some("keys") => {
            for k in settings_view::keys(&Config::load_strict(paths)?) {
                println!("{k}");
            }
            Ok(())
        }
        Some("set") => {
            let (Some(key), Some(value)) = (args.get(1), args.get(2)) else {
                bail!("{USAGE}")
            };
            if args.len() > 3 {
                bail!("{USAGE}\n(quote values that contain spaces)");
            }
            // Strict load: never overwrite a settings file we could not read.
            let mut config = Config::load_strict(paths)?;
            settings_view::set(&mut config, key, value)?;
            config.save(paths).context("saving the settings")?;
            println!("{key} = {}", settings_view::get(&config, key)?);
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

fn report(o: Outcome) -> Result<()> {
    match o {
        Outcome::Done => Ok(()),
        Outcome::LargeType(t) => {
            println!("{t}");
            Ok(())
        }
        Outcome::Confirm(m) => bail!("{m}"),
        Outcome::Error(e) => bail!("{e}"),
    }
}
