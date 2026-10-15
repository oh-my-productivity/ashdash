//! Running Script objects: interpreter table, query escaping, environment,
//! and process execution with a timeout.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
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

struct Interpreter {
    program: &'static str,
    pre_args: &'static [&'static str],
    ext: &'static str,
}

/// Script types. Values 0 (bash) and 5 (zsh) are confirmed by the bundled
/// templates; the others follow the reference launcher's documented language list and are
/// unverified against the binary.
fn interpreter(kind: i64) -> Option<Interpreter> {
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
fn escape_query(q: &str, mask: i64) -> String {
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

fn script_env(wf: &Workflow, paths: &Paths, vars: &BTreeMap<String, String>) -> Vec<(String, String)> {
    let key = wf.data_key();
    let data = paths.data.join("workflow-data").join(&key);
    let cache = paths.cache.join("workflow-cache").join(&key);
    let _ = std::fs::create_dir_all(&data);
    let _ = std::fs::create_dir_all(&cache);
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    env.insert("alfred_version".into(), "5.0".into());
    env.insert("alfred_workflow_name".into(), wf.name.clone());
    env.insert("alfred_workflow_bundleid".into(), wf.bundle_id.clone());
    env.insert("alfred_workflow_uid".into(), wf.id.clone());
    env.insert("alfred_workflow_data".into(), data.display().to_string());
    env.insert("alfred_workflow_cache".into(), cache.display().to_string());
    for (k, v) in &wf.variables {
        env.insert(k.clone(), v.clone());
    }
    for (k, v) in vars {
        env.insert(k.clone(), v.clone());
    }
    env.into_iter().collect()
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
        let dir = paths.cache.join("scripts");
        std::fs::create_dir_all(&dir)?;
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let file = dir.join(format!("{}-{}.{}", std::process::id(), n, interp.ext));
        std::fs::write(&file, script)?;
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
    let result = run_with_timeout(&mut cmd, if argtype == 2 { Some(query) } else { None }, timeout);
    if let Some(f) = cleanup {
        let _ = std::fs::remove_file(f);
    }
    result
}

fn run_with_timeout(cmd: &mut Command, stdin: Option<&str>, timeout: Duration) -> Result<ScriptOutput> {
    let mut child = cmd
        .spawn()
        .with_context(|| format!("starting {:?}", cmd.get_program()))?;
    if let (Some(text), Some(mut si)) = (stdin, child.stdin.take()) {
        let _ = si.write_all(text.as_bytes());
    }
    let mut so = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let mut se = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("script timed out after {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    Ok(ScriptOutput {
        stdout,
        stderr,
        success: status.success(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_escaping_follows_mask() {
        assert_eq!(escape_query("a\"b$c d", 4 | 32), "a\\\"b\\$c d");
        assert_eq!(escape_query("a b", 16), "a\\ b");
    }
}
