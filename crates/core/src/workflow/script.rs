//! Running Script objects: interpreter table, query escaping, environment,
//! and process execution with a timeout.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use super::{Node, Workflow};
use crate::config::Paths;

#[derive(Debug)]
pub struct ScriptOutput {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

pub(super) struct Interpreter {
    pub(super) program: &'static str,
    pre_args: &'static [&'static str],
    ext: &'static str,
}

/// Script types. Values 0 (bash) and 5 (zsh) are confirmed by the bundled
/// templates; the others follow the reference launcher's documented language list and are
/// unverified against the binary.
pub(super) fn interpreter(kind: i64) -> Option<Interpreter> {
    Some(match kind {
        0 => Interpreter {
            program: "/bin/bash",
            pre_args: &[],
            ext: "sh",
        },
        1 => Interpreter {
            program: "/usr/bin/php",
            pre_args: &[],
            ext: "php",
        },
        2 => Interpreter {
            program: "/usr/bin/ruby",
            pre_args: &[],
            ext: "rb",
        },
        3 => Interpreter {
            program: "/usr/bin/python3",
            pre_args: &[],
            ext: "py",
        },
        4 => Interpreter {
            program: "/usr/bin/perl",
            pre_args: &[],
            ext: "pl",
        },
        5 => Interpreter {
            program: "/bin/zsh",
            pre_args: &[],
            ext: "zsh",
        },
        6 => Interpreter {
            program: "/usr/bin/osascript",
            pre_args: &[],
            ext: "applescript",
        },
        7 => Interpreter {
            program: "/usr/bin/osascript",
            pre_args: &["-l", "JavaScript"],
            ext: "js",
        },
        _ => return None,
    })
}

/// Escape `{query}` for substitution into script source. Bit meanings for
/// 2/4/32/64 are inferred from the default mask (102) and the reference launcher's forum
/// guidance; 1/8/16 are brackets/semicolons/spaces in unverified order.
pub(super) fn escape_query(q: &str, mask: i64) -> String {
    let mut out = String::with_capacity(q.len());
    for ch in q.chars() {
        let esc = match ch {
            '`' => mask & 2 != 0,
            '"' => mask & 4 != 0,
            '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' => mask & 1 != 0,
            ';' => mask & 8 != 0,
            ' ' => mask & 16 != 0,
            '$' => mask & 32 != 0,
            '\\' => mask & 64 != 0,
            _ => false,
        };
        if esc {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A name that is safe as a single path component: ASCII alphanumerics, `.`, `-` and `_` only,
/// never `.`/`..`/hidden, at most 128 bytes. Used for every directory derived from a bundle id
/// (workflow data/cache dirs at run time, the install directory on import; SEC-12).
pub(super) fn safe_key(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .take(128)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.starts_with('.') {
        s.replace_range(..1, "_");
    }
    if s.is_empty() {
        s.push('_');
    }
    s
}

/// Variables that decide *which code runs* (loader, shell and interpreter start-up hooks) or who
/// the process thinks it is. They may be set by the workflow author, but never by dynamic
/// variables, which are data from outside the workflow (Script Filter output, scripts, the
/// previous object's `variables`): SEC-02. Matching is case-insensitive.
///
/// The rule applied by [`script_env`]: a dynamic variable is dropped when it is listed here, when
/// it starts with `alfred_` (the engine owns that namespace), or when its name/value cannot be an
/// environment entry at all - unless its value is exactly the author's own value (the author's
/// variables travel through the same variable set, so this keeps them working). Everything else
/// is exported as before, as the reference launcher does.
pub(super) fn is_protected_env_name(name: &str) -> bool {
    const EXACT: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "TMPDIR",
        "BASH_ENV",
        "ENV",
        "IFS",
        "SHELLOPTS",
        "BASHOPTS",
        "PS4",
        "PROMPT_COMMAND",
        "CDPATH",
        "GLOBIGNORE",
        "FPATH",
        "ZDOTDIR",
        "NODE_OPTIONS",
        "NODE_PATH",
        "RUBYOPT",
        "RUBYLIB",
        "PERL5OPT",
        "PERL5LIB",
        "PERLLIB",
        "PHPRC",
        "PHP_INI_SCAN_DIR",
        "GEM_HOME",
        "GEM_PATH",
        "CLASSPATH",
        "JAVA_TOOL_OPTIONS",
        "_JAVA_OPTIONS",
        "JDK_JAVA_OPTIONS",
        "GCONV_PATH",
        "LOCPATH",
        "NLSPATH",
        "TERMINFO",
        "HOSTALIASES",
        "MALLOC_CONF",
    ];
    const PREFIXES: &[&str] = &["DYLD_", "LD_", "PYTHON", "BASH_", "ALFRED_"];
    let u = name.to_ascii_uppercase();
    EXACT.contains(&u.as_str()) || PREFIXES.iter().any(|p| u.starts_with(p))
}

fn valid_env_entry(k: &str, v: &str) -> bool {
    !k.is_empty() && !k.contains(['=', '\0']) && !v.contains('\0')
}

fn script_env(wf: &Workflow, paths: &Paths, vars: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let key = safe_key(&wf.data_key());
    let data = paths.data.join("workflow-data").join(&key);
    let cache = paths.cache.join("workflow-cache").join(&key);
    let _ = std::fs::create_dir_all(&data);
    let _ = std::fs::create_dir_all(&cache);
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    env.insert("alfred_version".into(), "5.0".into());
    env.insert("alfred_workflow_name".into(), wf.name.clone());
    env.insert("alfred_workflow_bundleid".into(), wf.bundle_id.clone());
    env.insert("alfred_workflow_uid".into(), wf.id.clone());
    env.insert("alfred_workflow_version".into(), wf.version.clone());
    env.insert("alfred_workflow_data".into(), data.display().to_string());
    env.insert("alfred_workflow_cache".into(), cache.display().to_string());
    let base = wf.base_vars(paths);
    for (k, v) in &base {
        if valid_env_entry(k, v) {
            env.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in vars {
        if base.get(k) == Some(v) {
            continue; // the author's own value, already exported above
        }
        if !valid_env_entry(k, v) || is_protected_env_name(k) {
            super::exec::debug_log(&format!("not exporting dynamic variable '{k}' to scripts"));
            continue;
        }
        env.insert(k.clone(), v.clone());
    }
    env.into_iter().collect()
}

/// Create a private (0600, exclusive) script file in the cache's private (0700) `scripts` dir.
/// The name is unpredictable enough, and `create_new` never follows a pre-planted symlink (SEC-15).
fn write_temp_script(paths: &Paths, ext: &str, content: &str) -> Result<PathBuf> {
    let dir = paths.cache.join("scripts");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir)?;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    let mut last = None;
    for _ in 0..16 {
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let file = dir.join(format!("{}-{n}-{nanos:x}.{ext}", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file)
        {
            Ok(mut f) => {
                if let Err(e) = f.write_all(content.as_bytes()) {
                    let _ = std::fs::remove_file(&file);
                    return Err(e.into());
                }
                return Ok(file);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e.into()),
        }
    }
    Err(anyhow!(
        "cannot create a temporary script file: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

pub(super) fn run_node_script(
    node: &Node,
    wf: &Workflow,
    paths: &Paths,
    query: &str,
    vars: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<ScriptOutput> {
    let kind = node.i("type");
    let argtype = node.i("scriptargtype");
    let mut script = node.s("script");
    let scriptfile = node.s("scriptfile");
    let env = script_env(wf, paths, vars);

    let mut cleanup: Option<PathBuf> = None;
    let (program, args): (PathBuf, Vec<String>) = if script.trim().is_empty() && !scriptfile.is_empty() {
        let p = if Path::new(&scriptfile).is_absolute() {
            PathBuf::from(&scriptfile)
        } else {
            wf.dir.join(&scriptfile)
        };
        (p, vec![])
    } else {
        let interp = interpreter(kind).ok_or_else(|| anyhow!("unsupported script type {kind}"))?;
        if argtype == 0 {
            script = script.replace("{query}", &escape_query(query, node.i("escaping")));
        }
        let file = write_temp_script(paths, interp.ext, &script)?;
        cleanup = Some(file.clone());
        let mut a: Vec<String> = interp.pre_args.iter().map(|s| s.to_string()).collect();
        a.push(file.display().to_string());
        (PathBuf::from(interp.program), a)
    };

    let mut cmd = Command::new(&program);
    cmd.args(&args);
    if argtype == 1 || (argtype == 0 && cleanup.is_none()) {
        cmd.arg(query);
    }
    cmd.current_dir(&wf.dir)
        .envs(env)
        .stdin(if argtype == 2 { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The query is only ever an argv element *after* the program/script (or stdin): no
    // interpreter can read it as an option (SEC-01).
    let result = run_with_timeout(&mut cmd, if argtype == 2 { Some(query) } else { None }, timeout);
    if let Some(f) = cleanup {
        let _ = std::fs::remove_file(f);
    }
    result
}

/// Run AppleScript source with `osascript`. A script that only defines
/// `on alfred_script(q)` (the reference launcher's Run NSAppleScript template) gets a `run`
/// handler appended that calls it with the argument.
pub(super) fn run_applescript(
    source: &str,
    arg: &str,
    wf: &Workflow,
    paths: &Paths,
    vars: &BTreeMap<String, String>,
    timeout: Duration,
) -> Result<ScriptOutput> {
    let mut src = source.to_string();
    if src.contains("alfred_script") && !src.contains("on run") {
        src.push_str("\non run argv\n\treturn alfred_script(item 1 of argv)\nend run\n");
    }
    let file = write_temp_script(paths, "applescript", &src)?;
    let mut cmd = Command::new("/usr/bin/osascript");
    cmd.arg(&file)
        .arg(arg)
        .current_dir(&wf.dir)
        .envs(script_env(wf, paths, vars))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let result = run_with_timeout(&mut cmd, None, timeout);
    let _ = std::fs::remove_file(file);
    result
}

/// Run a one-off AppleScript snippet (`osascript -e`), e.g. a Music command.
pub fn run_osascript(source: &str) -> Result<String> {
    let mut cmd = Command::new("/usr/bin/osascript");
    cmd.arg("-e")
        .arg(source)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = run_with_timeout(&mut cmd, None, Duration::from_secs(30))?;
    if out.success {
        Ok(out.stdout.trim_end().to_string())
    } else {
        bail!("{}", osascript_failure(&out.stderr))
    }
}

/// The message for a failed `osascript`, with the Automation hint for a refused Apple event.
fn osascript_failure(stderr: &str) -> String {
    format!(
        "osascript failed: {}",
        crate::engine::with_automation_hint(stderr.trim())
    )
}

/// Largest stdout a script may produce; more kills the script and is an error (SEC-06).
pub const MAX_STDOUT_BYTES: usize = 8 * 1024 * 1024;
/// stderr beyond this is read and discarded (the script must not block on a full pipe).
const MAX_STDERR_BYTES: usize = 256 * 1024;
/// How long the pipes may keep delivering after the interpreter itself has exited (SEC-05).
const READER_GRACE: Duration = Duration::from_millis(300);

#[derive(Default)]
struct Capture {
    buf: Vec<u8>,
    overflow: bool,
    done: bool,
}

fn lock(c: &Mutex<Capture>) -> MutexGuard<'_, Capture> {
    c.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read `r` on its own thread into a capped buffer. With `stop_on_overflow` the thread stops reading
/// at the cap (the caller kills the process); otherwise it keeps draining and discards the rest.
fn spawn_reader<R: Read + Send + 'static>(mut r: R, cap: usize, stop_on_overflow: bool) -> Arc<Mutex<Capture>> {
    let shared = Arc::new(Mutex::new(Capture::default()));
    let s = Arc::clone(&shared);
    std::thread::spawn(move || {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match r.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let mut c = lock(&s);
                    let room = cap.saturating_sub(c.buf.len());
                    c.buf.extend_from_slice(&chunk[..n.min(room)]);
                    if n > room {
                        c.overflow = true;
                        if stop_on_overflow {
                            break;
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        lock(&s).done = true;
    });
    shared
}

unsafe extern "C" {
    fn killpg(pgrp: i32, sig: i32) -> i32;
}

/// SIGKILL the child's whole process group. Only call while the child is still unreaped: its pid
/// (= the group id) cannot be reused before `wait`.
fn kill_group(child: &mut Child) {
    // SAFETY: plain syscall wrapper; the id is our own child's process group.
    unsafe {
        killpg(child.id() as i32, 9);
    }
    let _ = child.kill();
}

/// Run `cmd` in its own process group with a wall-clock `timeout`. On timeout or when stdout exceeds
/// [`MAX_STDOUT_BYTES`] the whole group is killed. Pipes are read on separate threads and are
/// abandoned shortly after the direct child exits, so a background helper that inherited them
/// cannot stall the caller. `stdin` text is written by its own thread, so an unread pipe cannot
/// defeat the timeout (SEC-05/06/16).
fn run_with_timeout(cmd: &mut Command, stdin: Option<&str>, timeout: Duration) -> Result<ScriptOutput> {
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("starting {:?}", cmd.get_program()))?;
    if let (Some(text), Some(mut si)) = (stdin.map(str::to_owned), child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = si.write_all(text.as_bytes()); // EPIPE once the script is gone: fine
        });
    }
    let so = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let se = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
    let out = spawn_reader(so, MAX_STDOUT_BYTES, true);
    let err = spawn_reader(se, MAX_STDERR_BYTES, false);
    let overflow_error = || {
        anyhow!(
            "script output exceeded {} MiB; the script was stopped",
            MAX_STDOUT_BYTES >> 20
        )
    };
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if lock(&out).overflow {
            kill_group(&mut child);
            let _ = child.wait();
            return Err(overflow_error());
        }
        if start.elapsed() > timeout {
            kill_group(&mut child);
            let _ = child.wait();
            bail!("script timed out after {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    // The interpreter is gone; let the pipes drain briefly, then take what there is.
    let grace = Instant::now();
    while grace.elapsed() < READER_GRACE && !(lock(&out).done && lock(&err).done) {
        std::thread::sleep(Duration::from_millis(1));
    }
    let (stdout, o) = {
        let c = lock(&out);
        (String::from_utf8_lossy(&c.buf).into_owned(), c.overflow)
    };
    if o {
        return Err(overflow_error());
    }
    let stderr = String::from_utf8_lossy(&lock(&err).buf).into_owned();
    Ok(ScriptOutput {
        stdout,
        stderr,
        success: status.success(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_denied_automation_request_says_where_to_allow_it() {
        let raw = "49:75: execution error: Not authorized to send Apple events to Terminal. (-1743)";
        let msg = osascript_failure(raw);
        assert!(msg.starts_with("osascript failed: "), "{msg}");
        assert!(msg.contains(raw), "the original text stays: {msg}");
        assert!(
            msg.contains("Privacy & Security > Automation") && msg.contains(crate::brand::NAME),
            "{msg}"
        );
        // other failures are unchanged
        assert_eq!(
            osascript_failure("1:2: syntax error (-2741)"),
            "osascript failed: 1:2: syntax error (-2741)"
        );
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("oa-script-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sh(script: &str) -> Command {
        let mut c = Command::new("/bin/bash");
        c.arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    #[test]
    fn query_escaping_follows_mask() {
        assert_eq!(escape_query("a\"b$c d", 4 | 32), "a\\\"b\\$c d");
        assert_eq!(escape_query("a b", 16), "a\\ b");
    }

    #[test]
    fn safe_key_is_a_single_harmless_path_component() {
        assert_eq!(safe_key("com.example.wf-1_x"), "com.example.wf-1_x");
        assert_eq!(safe_key("../../x"), "_._.._x");
        assert_eq!(safe_key(".."), "_.");
        assert_eq!(safe_key("."), "_");
        assert_eq!(safe_key(""), "_");
        assert_eq!(safe_key("a/b\\c d\0e"), "a_b_c_d_e");
        assert_eq!(safe_key(&"x".repeat(1000)).len(), 128);
        for raw in ["..", "../..", "/etc", "a/../b", "~", "\u{202e}x"] {
            let k = safe_key(raw);
            assert!(!k.contains('/') && k != ".." && k != ".", "{raw:?} -> {k:?}");
        }
    }

    #[test]
    fn loader_and_shell_variables_are_protected() {
        for n in [
            "PATH",
            "path",
            "BASH_ENV",
            "ENV",
            "IFS",
            "DYLD_INSERT_LIBRARIES",
            "dyld_library_path",
            "LD_PRELOAD",
            "PYTHONPATH",
            "PYTHONSTARTUP",
            "RUBYOPT",
            "PERL5OPT",
            "NODE_OPTIONS",
            "SHELLOPTS",
            "BASH_FUNC_x%%",
            "HOME",
            "alfred_workflow_data",
        ] {
            assert!(is_protected_env_name(n), "{n}");
        }
        for n in [
            "greeting",
            "query",
            "my_path",
            "LANGUAGE_X",
            "api_key",
            "ENVIRONMENT",
            "USERNAME",
        ] {
            assert!(!is_protected_env_name(n), "{n}");
        }
    }

    fn env_of(wf_vars: serde_json::Value, dynamic: &[(&str, &str)]) -> BTreeMap<String, String> {
        let root = tmp("env");
        let doc = json!({"name": "E", "bundleid": "t.env", "variables": wf_vars, "objects": [], "connections": {}});
        let wf = Workflow::from_json(&root, "e", &doc).unwrap();
        let paths = Paths::under(&root);
        let mut vars = wf.base_vars(&paths);
        vars.extend(dynamic.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let env = script_env(&wf, &paths, &vars).into_iter().collect();
        let _ = std::fs::remove_dir_all(root);
        env
    }

    #[test]
    fn dynamic_variables_cannot_set_protected_names_but_authors_can() {
        let env = env_of(
            json!({"PYTHONIOENCODING": "utf-8", "greeting": "hi"}),
            &[
                ("BASH_ENV", "/tmp/evil"),
                ("PATH", "/tmp/bin"),
                ("DYLD_INSERT_LIBRARIES", "/tmp/x.dylib"),
                ("alfred_workflow_data", "/etc"),
                ("greeting", "overridden"),
                ("fresh", "1"),
                ("bad=name", "x"),
                ("nul", "a\0b"),
                ("", "empty"),
            ],
        );
        for n in ["BASH_ENV", "DYLD_INSERT_LIBRARIES", "bad=name", "nul", ""] {
            assert!(!env.contains_key(n), "{n} leaked");
        }
        assert!(env.get("PATH").is_none_or(|p| p != "/tmp/bin"));
        assert_ne!(env["alfred_workflow_data"], "/etc");
        // ordinary dynamic variables still work, and may still override the author's ordinary ones
        assert_eq!(env["greeting"], "overridden");
        assert_eq!(env["fresh"], "1");
        // the author's own loader-ish variable is kept (it is code the user installed)
        assert_eq!(env["PYTHONIOENCODING"], "utf-8");
    }

    #[test]
    fn author_defined_protected_variable_is_exported() {
        let env = env_of(json!({"PATH": "/opt/tools:/usr/bin"}), &[]);
        assert_eq!(env["PATH"], "/opt/tools:/usr/bin");
        // ... and a dynamic attempt to change it is ignored
        let env = env_of(json!({"PATH": "/opt/tools:/usr/bin"}), &[("PATH", "/tmp/evil")]);
        assert_eq!(env["PATH"], "/opt/tools:/usr/bin");
    }

    #[test]
    fn temp_script_files_are_private_and_exclusive() {
        let root = tmp("tmpfile");
        let paths = Paths::under(&root);
        let f = write_temp_script(&paths, "sh", "echo hi").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&f), 0o600);
        assert_eq!(mode(f.parent().unwrap()), 0o700);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "echo hi");
        let g = write_temp_script(&paths, "sh", "x").unwrap();
        assert_ne!(f, g);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn timeout_kills_the_whole_process_group_quickly() {
        let root = tmp("pgrp");
        let pidfile = root.join("pid");
        let mut c = sh(&format!("sleep 300 & echo $! > '{}'; wait", pidfile.display()));
        let start = Instant::now();
        let err = run_with_timeout(&mut c, None, Duration::from_millis(400)).unwrap_err();
        assert!(format!("{err:#}").contains("timed out"), "{err:#}");
        assert!(start.elapsed() < Duration::from_secs(5));
        let pid = std::fs::read_to_string(&pidfile).unwrap().trim().to_string();
        std::thread::sleep(Duration::from_millis(100));
        let alive = Command::new("kill")
            .args(["-0", &pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        if alive {
            let _ = Command::new("kill").args(["-9", &pid]).status();
        }
        assert!(!alive, "grandchild {pid} survived");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn output_over_the_cap_is_an_error_and_stops_the_script() {
        let mut c = sh("head -c 20000000 /dev/zero; sleep 30");
        let start = Instant::now();
        let err = run_with_timeout(&mut c, None, Duration::from_secs(20)).unwrap_err();
        assert!(format!("{err:#}").contains("exceeded 8 MiB"), "{err:#}");
        assert!(start.elapsed() < Duration::from_secs(10), "{:?}", start.elapsed());
    }

    #[test]
    fn output_just_under_the_cap_and_noisy_stderr_are_fine() {
        let mut c = sh("head -c 1000000 /dev/zero | tr '\\0' a; head -c 2000000 /dev/zero >&2; echo done");
        let out = run_with_timeout(&mut c, None, Duration::from_secs(20)).unwrap();
        assert!(out.success && out.stdout.ends_with("done\n") && out.stdout.len() == 1_000_005);
        assert_eq!(out.stderr.len(), MAX_STDERR_BYTES);
    }

    #[test]
    fn exited_script_with_a_lingering_pipe_holder_returns_promptly() {
        let mut c = sh("sleep 5 & echo hello");
        let start = Instant::now();
        let out = run_with_timeout(&mut c, None, Duration::from_secs(20)).unwrap();
        assert_eq!(out.stdout.trim(), "hello");
        assert!(start.elapsed() < Duration::from_secs(3), "{:?}", start.elapsed());
    }

    #[test]
    fn stdin_is_delivered_and_a_never_reading_script_still_times_out() {
        let mut c = Command::new("/bin/cat");
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = run_with_timeout(&mut c, Some("héllo"), Duration::from_secs(5)).unwrap();
        assert_eq!(out.stdout, "héllo");

        let mut c = sh("sleep 30");
        c.stdin(Stdio::piped());
        let big = "x".repeat(4 << 20);
        let start = Instant::now();
        assert!(run_with_timeout(&mut c, Some(&big), Duration::from_millis(500)).is_err());
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }

    /// SEC-01 for scripts: a query that looks like an interpreter option stays data.
    #[test]
    fn option_like_queries_are_passed_as_plain_arguments() {
        let root = tmp("optq");
        let paths = Paths::under(&root);
        for (kind, script, expect_prefix) in [
            (0, "printf '%s' \"$1\"", ""),
            (5, "printf '%s' \"$1\"", ""),
            (3, "import sys\nprint(sys.argv[1], end='')", ""),
            (6, "on run argv\nreturn item 1 of argv\nend run", ""),
            (7, "function run(argv) { return argv[0]; }", ""),
        ] {
            let doc = json!({"name": "O", "bundleid": "t.optq", "connections": {}, "objects": [
                {"uid": "S", "type": "alfred.workflow.action.script", "config": {
                    "type": kind, "scriptargtype": 1, "script": script}}]});
            let wf = Workflow::from_json(&root, "o", &doc).unwrap();
            let node = wf.node("S").unwrap();
            for q in ["-eabc", "-e return 1", "--", "-", "-l JavaScript"] {
                let out = run_node_script(node, &wf, &paths, q, &BTreeMap::new(), Duration::from_secs(20)).unwrap();
                assert!(out.success, "type {kind} q={q:?}: {}", out.stderr);
                assert_eq!(
                    out.stdout.trim_end_matches('\n'),
                    format!("{expect_prefix}{q}"),
                    "type {kind}"
                );
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
