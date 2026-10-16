mod automation;
mod clipboard_watch;
mod events;
mod gui;
mod hotkeys;
mod ipc;
mod mac;
mod prefs_model;
mod scripting;
mod urlscheme;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ashdash_core::theme::Theme;
use ashdash_core::{Config, Engine, Mods, Outcome, Paths};
use anyhow::{Context, Result, bail};

const HELP: &str = "\
ashdash — a keyboard launcher for macOS

USAGE:
  ashdash                         start the launcher (default hotkey: Option+Shift+Space)
  ashdash query <text>            print the ranked results for <text>
  ashdash run <text> [index]      run result <index> (default 0) for <text>
  ashdash import [--yes] <file>   install an .alfredworkflow package after showing what it can
                                      do and asking (--yes: skip the question)
  ashdash workflows               list installed workflows
  ashdash trigger <bundle-id> <trigger-id> [argument]
                                      fire a workflow's External Trigger (through the
                                      running launcher when there is one)
  ashdash theme <file>            validate an .alfredappearance file; apply it to the
                                      running launcher, or print it when none runs
  ashdash show                    show the launcher (starts it if needed)
  ashdash search <text>           show the launcher with <text> typed (starts it if needed)
  ashdash reload                  re-scan apps, workflows, snippets and workflow hotkeys
  ashdash prefs                   open the preferences window
  ashdash quit                    quit the running launcher
  ashdash login-item enable|disable|status
                                      launch at login (needs the Ashdash.app bundle)
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
        None => run_launcher(paths, config),
        Some("-h" | "--help" | "help") => {
            print!("{HELP}");
            Ok(())
        }
        Some("prefs") => gui::prefs::open_window(),
        // The window itself (its own process); `ashdash prefs` starts this detached.
        Some("prefs-window") => gui::prefs::run(paths),
        Some("login-item") => mac::loginitem::cli(args.get(1).map(String::as_str)),
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
            let yes = args[1..].iter().any(|a| a == "--yes");
            let Some(file) = args[1..].iter().find(|a| !a.starts_with("--")) else {
                bail!("usage: ashdash import [--yes] <file.alfredworkflow>")
            };
            let file = PathBuf::from(file);
            // A workflow is code that will run as you: show what it can do and ask first. The
            // package is opened once; what is shown is the staged copy that gets installed.
            let staged = ashdash_core::workflow::stage_package(&paths, &file)?;
            let sock = socket_path(&paths);
            let dest = gui::importprompt::cli_import(staged, yes, gui::importprompt::ask_yes_no, |staged| {
                Engine::new(paths, config).install_staged(staged)
            })?;
            println!("installed to {}", dest.display());
            // Best effort: let a running launcher pick up the new workflow and its hotkeys.
            let _ = ipc::send(&sock, &ipc::Request::Reload, CONTROL_TIMEOUT);
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
            let arg = args.get(3).cloned().unwrap_or_default();
            let req = ipc::Request::Trigger {
                bundle: bundle.clone(),
                id: trigger.clone(),
                arg: arg.clone(),
            };
            match ipc::send(&socket_path(&paths), &req, TRIGGER_TIMEOUT) {
                Ok(resp) => report_response(resp),
                // No launcher running: run it in this process, as before.
                Err(ipc::ClientError::NotRunning) => {
                    let engine = Engine::new(paths, config);
                    report(engine.run_trigger(bundle, trigger, &arg))
                }
                Err(e) => Err(e.into()),
            }
        }
        Some("theme") => {
            let Some(file) = args.get(1) else {
                bail!("usage: ashdash theme <file>")
            };
            let file = std::fs::canonicalize(file).with_context(|| format!("cannot read {file}"))?;
            let theme = Theme::load(&file)?;
            match ipc::send(&socket_path(&paths), &ipc::Request::Theme { file }, CONTROL_TIMEOUT) {
                Ok(resp) => report_response(resp),
                Err(ipc::ClientError::NotRunning) => {
                    eprintln!("ashdash is not running; the theme was only parsed:");
                    println!("{theme:#?}");
                    Ok(())
                }
                Err(e) => Err(e.into()),
            }
        }
        Some("show") => control(&paths, ipc::Request::Show, true),
        Some("search") => {
            if args.len() < 2 {
                bail!("usage: ashdash search <text>")
            }
            control(
                &paths,
                ipc::Request::Search {
                    text: args[1..].join(" "),
                },
                true,
            )
        }
        Some("reload") => control(&paths, ipc::Request::Reload, false),
        Some("quit") => control(&paths, ipc::Request::Quit, false),
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

const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
/// Workflows may run scripts for up to a minute.
const TRIGGER_TIMEOUT: Duration = Duration::from_secs(130);

fn socket_path(paths: &Paths) -> PathBuf {
    ipc::socket_path(paths, ipc::current_uid())
}

/// Start the launcher: single instance, control socket, URL handler, hotkeys, window.
fn run_launcher(paths: Paths, config: Config) -> Result<()> {
    let uid = ipc::current_uid();
    let sock = ipc::socket_path(&paths, uid);
    let primary = match ipc::acquire(&sock, uid) {
        Ok(ipc::Acquired::Primary(p)) => Some(p),
        Ok(ipc::Acquired::AlreadyRunning) => {
            // A second `ashdash` behaves like pressing the hotkey in the first.
            let _ = ipc::send(&sock, &ipc::Request::Show, CONTROL_TIMEOUT);
            eprintln!("ashdash: already running");
            return Ok(());
        }
        Err(e) => {
            // Not fatal: the launcher still works, just without `ashdash show` and friends.
            eprintln!("ashdash: control socket unavailable: {e}");
            None
        }
    };
    let engine = Arc::new(Engine::new(paths, config));
    // Double-clicked .alfredworkflow / .alfredappearance / .alfredsnippets files.
    mac::opendocs::install(Arc::clone(&engine));
    let started = automation::start(&engine, primary);
    gui::run(engine, started.automation)
}

/// Send `req` to the running launcher, starting one first when `start` is set
/// and none runs. Without a launcher and without `start` the command is a no-op.
fn control(paths: &Paths, req: ipc::Request, start: bool) -> Result<()> {
    let sock = socket_path(paths);
    match ipc::send(&sock, &req, CONTROL_TIMEOUT) {
        Ok(resp) => return report_response(resp),
        Err(ipc::ClientError::NotRunning) if !start => {
            eprintln!("ashdash is not running");
            return Ok(());
        }
        Err(ipc::ClientError::NotRunning) => {}
        Err(e) => return Err(e.into()),
    }
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    Command::new(std::env::current_exe()?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("could not start the launcher")?;
    if !ipc::wait_until_up(&sock, Duration::from_secs(15)) {
        bail!("the launcher did not start within 15 s");
    }
    report_response(ipc::send(&sock, &req, CONTROL_TIMEOUT)?)
}

fn report_response(r: ipc::Response) -> Result<()> {
    if !r.ok {
        bail!("{}", r.error);
    }
    if r.large || !r.message.is_empty() {
        println!("{}", r.message);
    }
    Ok(())
}

fn report(o: Outcome) -> Result<()> {
    match o {
        Outcome::Done => Ok(()),
        Outcome::LargeType(t) => {
            println!("{t}");
            Ok(())
        }
        Outcome::ShowLauncher(q) => {
            eprintln!("ashdash: the workflow wants the launcher open with '{q}' (no launcher is running)");
            Ok(())
        }
        Outcome::Confirm(m) => bail!("{m}"),
        Outcome::Error(e) => bail!("{e}"),
    }
}
