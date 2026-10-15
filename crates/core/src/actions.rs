//! Universal Actions: operations offered for a result (files, folders, text,
//! URLs) and for the multi-select [`Buffer`].
//!
//! Every action is a plain-data [`UniversalAction`] carried by an
//! [`Item`](crate::item::Item) as `Action::Universal`, so `Engine::execute`
//! runs them like any other result. Nothing here touches the OS directly:
//! [`run`] turns an action into one or more [`Cmd`]s and hands them to an
//! [`Os`]. The real implementation is [`RealOs`]; tests use a recording fake
//! and assert the exact commands.
//!
//! # Security
//! * Paths and URLs reach child processes only as separate `argv` entries,
//!   never through a shell and never spliced into AppleScript source.
//! * Every path must be absolute and must not start with `-` (so it can never
//!   be read as an option); see [`safe_path`].
//! * Move to Trash resolves the *parent* directory (so a symlink is trashed
//!   itself, not its target), refuses top-level and system locations and the
//!   home folder, and always needs confirmation.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::engine::Outcome;
use crate::files;
use crate::fuzzy;
use crate::item::{Action, Icon, Item};
use crate::websearch::percent_encode;

// ------------------------------------------------------------------ Os trait

/// A command line: program + separate arguments (+ optional stdin text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub stdin: Option<String>,
}

impl Cmd {
    pub fn new(program: &str) -> Cmd {
        Cmd {
            program: program.into(),
            args: Vec::new(),
            stdin: None,
        }
    }
    pub fn arg(mut self, a: impl Into<OsString>) -> Cmd {
        self.args.push(a.into());
        self
    }
    pub fn args<I, S>(mut self, it: I) -> Cmd
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(it.into_iter().map(Into::into));
        self
    }
    pub fn stdin(mut self, text: impl Into<String>) -> Cmd {
        self.stdin = Some(text.into());
        self
    }
    /// `osascript` with the script lines as `-e` options and `argv` after them.
    pub fn osascript<P: Into<OsString>>(lines: &[&str], argv: impl IntoIterator<Item = P>) -> Cmd {
        let mut c = Cmd::new("osascript");
        for l in lines {
            c = c.arg("-e").arg(*l);
        }
        c.args(argv)
    }
}

/// Everything that reaches the operating system. Injectable so tests never
/// touch the real machine.
pub trait Os: Send + Sync {
    /// Run to completion; a non-zero exit is an error carrying stderr.
    fn run(&self, cmd: &Cmd) -> Result<()>;
    /// Run to completion and return stdout; a non-zero exit is an error.
    fn output(&self, cmd: &Cmd) -> Result<Vec<u8>>;
    /// Start without waiting (e.g. Quick Look, which blocks until closed).
    fn spawn(&self, cmd: &Cmd) -> Result<()>;
}

/// The real thing: spawns processes with no shell involved.
pub struct RealOs;

fn build(cmd: &Cmd) -> Command {
    let mut c = Command::new(&cmd.program);
    c.args(&cmd.args);
    c
}

impl Os for RealOs {
    fn run(&self, cmd: &Cmd) -> Result<()> {
        let mut c = build(cmd);
        c.stdout(Stdio::null()).stderr(Stdio::piped());
        c.stdin(if cmd.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = c.spawn().with_context(|| format!("starting {:?}", cmd.program))?;
        if let (Some(text), Some(mut stdin)) = (&cmd.stdin, child.stdin.take()) {
            stdin.write_all(text.as_bytes())?;
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!(
                "{} failed: {}",
                cmd.program.to_string_lossy(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    fn output(&self, cmd: &Cmd) -> Result<Vec<u8>> {
        let out = build(cmd)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .with_context(|| format!("starting {:?}", cmd.program))?;
        if !out.status.success() {
            bail!("{} failed ({})", cmd.program.to_string_lossy(), out.status);
        }
        Ok(out.stdout)
    }

    fn spawn(&self, cmd: &Cmd) -> Result<()> {
        let mut child = build(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("starting {:?}", cmd.program))?;
        // Reap it when it exits so it does not linger as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

// ------------------------------------------------------------------ safety

/// Accept only absolute paths that cannot be mistaken for an option.
pub fn safe_path(p: &Path) -> Result<&Path> {
    let b = p.as_os_str().as_bytes();
    if b.is_empty() || b[0] == b'-' || b.contains(&0) {
        bail!("refusing unsafe path {p:?}");
    }
    if !p.is_absolute() {
        bail!("refusing relative path {p:?}");
    }
    Ok(p)
}

fn safe_paths(ps: &[PathBuf]) -> Result<Vec<&Path>> {
    if ps.is_empty() {
        bail!("nothing selected");
    }
    ps.iter().map(|p| safe_path(p)).collect()
}

fn has_scheme(u: &str) -> bool {
    let Some((scheme, rest)) = u.split_once(':') else {
        return false;
    };
    !rest.is_empty()
        && !u.starts_with('-')
        && !u.chars().any(|c| c.is_whitespace() || c.is_control())
        && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// The path that will be trashed: parent resolved, final component kept (a
/// symlink is trashed itself, never its target). Refuses dangerous targets.
fn trash_target(p: &Path) -> Result<PathBuf> {
    let p = safe_path(p)?;
    std::fs::symlink_metadata(p).with_context(|| format!("{} does not exist", p.display()))?;
    let (Some(name), Some(parent)) = (p.file_name(), p.parent()) else {
        bail!("refusing to trash {}", p.display());
    };
    let target = parent
        .canonicalize()
        .with_context(|| format!("resolving {}", parent.display()))?
        .join(name);
    let top_level = target.parent().is_none_or(|d| d == Path::new("/"));
    let system = ["/System", "/usr", "/bin", "/sbin", "/private/etc", "/private/var/db"]
        .iter()
        .any(|s| target.starts_with(s));
    let home = dirs::home_dir().map(|h| h.canonicalize().unwrap_or(h));
    let is_home_or_above = home.is_some_and(|h| h.starts_with(&target));
    if top_level || system || is_home_or_above {
        bail!("refusing to trash {}", target.display());
    }
    Ok(target)
}

/// `<dir>/<name>.zip`, or `<name> 2.zip`, ... when taken.
fn unique_zip(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.zip"));
    if std::fs::symlink_metadata(&first).is_err() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem} {n}.zip")))
        .find(|p| std::fs::symlink_metadata(p).is_err())
        .expect("unbounded range")
}

// ------------------------------------------------------------------ actions

#[derive(Debug, Clone, PartialEq)]
pub enum UniversalAction {
    Open(PathBuf),
    /// Open `path` with the application bundle `app`.
    OpenWith {
        path: PathBuf,
        app: PathBuf,
    },
    /// Marker for "Open With...": the UI lists apps with [`open_with_items`].
    OpenWithMenu(PathBuf),
    Reveal(Vec<PathBuf>),
    /// Put the files themselves on the clipboard (paste into Finder copies them).
    CopyFile(Vec<PathBuf>),
    QuickLook(Vec<PathBuf>),
    Trash(Vec<PathBuf>),
    /// Zip next to the first item: `ditto` for one item, `tar --format zip` for several.
    Compress(Vec<PathBuf>),
    EmailAttach(Vec<PathBuf>),
    GetInfo(PathBuf),
    /// Open a URL (must carry a scheme).
    OpenUrl(String),
    CopyText(String),
    LargeType(String),
}

impl UniversalAction {
    /// Destructive actions ask first (`Outcome::Confirm`).
    pub fn confirmation(&self) -> Option<String> {
        match self {
            UniversalAction::Trash(ps) => Some(match ps.as_slice() {
                [p] => format!("Move \"{}\" to Trash? Press ↩ again to confirm.", file_name(p)),
                _ => format!("Move {} items to Trash? Press ↩ again to confirm.", ps.len()),
            }),
            _ => None,
        }
    }
}

const SCRIPT_COPY_FILES: &[&str] = &[
    "on run argv",
    "set fs to {}",
    "repeat with p in argv",
    "set end of fs to (POSIX file (contents of p))",
    "end repeat",
    "set the clipboard to fs",
    "end run",
];

const SCRIPT_TRASH: &[&str] = &[
    "on run argv",
    "tell application \"Finder\"",
    "repeat with p in argv",
    "delete ((POSIX file (contents of p)) as alias)",
    "end repeat",
    "end tell",
    "end run",
];

const SCRIPT_EMAIL: &[&str] = &[
    "on run argv",
    "tell application \"Mail\"",
    "set m to make new outgoing message with properties {visible:true}",
    "tell content of m",
    "repeat with p in argv",
    "make new attachment with properties {file name:((POSIX file (contents of p)) as alias)} at after the last paragraph",
    "end repeat",
    "end tell",
    "activate",
    "end tell",
    "end run",
];

const SCRIPT_GET_INFO: &[&str] = &[
    "on run argv",
    "tell application \"Finder\"",
    "activate",
    "open information window of ((POSIX file (item 1 of argv)) as alias)",
    "end tell",
    "end run",
];

/// Exposed so the scripts can be syntax-checked with `osacompile` by hand.
pub fn applescripts() -> [(&'static str, &'static [&'static str]); 4] {
    [
        ("copy-files", SCRIPT_COPY_FILES),
        ("trash", SCRIPT_TRASH),
        ("email", SCRIPT_EMAIL),
        ("get-info", SCRIPT_GET_INFO),
    ]
}

/// Perform an action through `os`. Errors become `Outcome::Error`.
pub fn run(os: &dyn Os, action: &UniversalAction) -> Outcome {
    match execute(os, action) {
        Ok(o) => o,
        Err(e) => Outcome::Error(format!("{e:#}")),
    }
}

fn execute(os: &dyn Os, action: &UniversalAction) -> Result<Outcome> {
    use UniversalAction as A;
    match action {
        A::Open(p) => os.run(&Cmd::new("open").arg(safe_path(p)?))?,
        A::OpenWith { path, app } => os.run(&Cmd::new("open").arg("-a").arg(safe_path(app)?).arg(safe_path(path)?))?,
        A::OpenWithMenu(_) => {}
        A::Reveal(ps) => os.run(&Cmd::new("open").arg("-R").args(safe_paths(ps)?))?,
        A::CopyFile(ps) => os.run(&Cmd::osascript(SCRIPT_COPY_FILES, safe_paths(ps)?))?,
        A::QuickLook(ps) => os.spawn(&Cmd::new("qlmanage").arg("-p").args(safe_paths(ps)?))?,
        A::Trash(ps) => {
            if ps.is_empty() {
                bail!("nothing selected");
            }
            let targets = ps.iter().map(|p| trash_target(p)).collect::<Result<Vec<_>>>()?;
            os.run(&Cmd::osascript(SCRIPT_TRASH, targets))?
        }
        A::Compress(ps) => os.run(&compress_cmd(ps)?)?,
        A::EmailAttach(ps) => os.run(&Cmd::osascript(SCRIPT_EMAIL, safe_paths(ps)?))?,
        A::GetInfo(p) => os.run(&Cmd::osascript(SCRIPT_GET_INFO, [safe_path(p)?]))?,
        A::OpenUrl(u) => {
            if !has_scheme(u) {
                bail!("refusing to open '{u}': not a URL");
            }
            os.run(&Cmd::new("open").arg(u))?
        }
        A::CopyText(t) => os.run(&Cmd::new("pbcopy").stdin(t.as_str()))?,
        A::LargeType(t) => return Ok(Outcome::LargeType(t.clone())),
    }
    Ok(Outcome::Done)
}

fn compress_cmd(ps: &[PathBuf]) -> Result<Cmd> {
    let srcs = safe_paths(ps)?;
    for s in &srcs {
        std::fs::symlink_metadata(s).with_context(|| format!("{} does not exist", s.display()))?;
    }
    let dir = srcs[0].parent().context("cannot compress the root folder")?;
    if srcs.iter().any(|s| dir.starts_with(s)) {
        bail!("the archive would be created inside one of the items being compressed");
    }
    if let [one] = srcs.as_slice() {
        let stem = one.file_name().context("no file name")?.to_string_lossy().to_string();
        let dest = unique_zip(dir, &stem);
        // A folder keeps its own name as the top-level entry; a plain file
        // would otherwise be wrapped in its parent folder's name.
        let is_dir = std::fs::symlink_metadata(one)?.file_type().is_dir();
        let mut cmd = Cmd::new("ditto").args(["-c", "-k", "--sequesterRsrc"]);
        if is_dir {
            cmd = cmd.arg("--keepParent");
        }
        return Ok(cmd.arg(one).arg(dest));
    }
    // `ditto -c` refuses several sources ("Can't archive multiple sources"),
    // so a multi-item archive is built by bsdtar, one `-C <parent> <name>` per item.
    let mut cmd = Cmd::new("tar")
        .args(["--format", "zip", "-cf"])
        .arg(unique_zip(dir, "Archive"));
    for s in srcs {
        let name = s.file_name().context("no file name")?;
        let mut operand = OsString::new();
        if name.as_bytes().first() == Some(&b'-') {
            operand.push("./"); // never let a name be read as an option
        }
        operand.push(name);
        cmd = cmd.arg("-C").arg(s.parent().context("no parent folder")?).arg(operand);
    }
    Ok(cmd)
}

// ------------------------------------------------------------------ building items

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.display().to_string())
}

fn act(
    id: String,
    title: impl Into<String>,
    subtitle: impl Into<String>,
    glyph: &'static str,
    a: UniversalAction,
) -> Item {
    Item::new(id, title)
        .subtitle(subtitle)
        .icon(Icon::Glyph(glyph))
        .action(Action::Universal(a))
}

/// Keep the given order when the UI sorts by score.
fn ordered(mut items: Vec<Item>) -> Vec<Item> {
    let n = items.len() as f64;
    for (i, it) in items.iter_mut().enumerate() {
        it.score = n - i as f64;
    }
    items
}

/// What a result is, as far as actions are concerned.
#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    File(PathBuf),
    Folder(PathBuf),
    Url(String),
    Text(String),
    Other,
}

pub fn item_kind(item: &Item) -> ItemKind {
    if !item.valid {
        return ItemKind::Other;
    }
    match &item.action {
        Action::Open(p) | Action::Reveal(p) => {
            if p.is_dir() && p.extension().is_none_or(|e| e != "app") {
                ItemKind::Folder(p.clone())
            } else {
                ItemKind::File(p.clone())
            }
        }
        Action::OpenUrl(u) => ItemKind::Url(u.clone()),
        Action::Copy(t) => ItemKind::Text(t.clone()),
        Action::Universal(_) | Action::System(_) | Action::Workflow(_) | Action::Shell(_) => ItemKind::Other,
        Action::None => match item.copy_text.as_ref().or(item.large_type.as_ref()) {
            Some(t) => ItemKind::Text(t.clone()),
            None => ItemKind::Other,
        },
    }
}

/// The action list for a result, by kind. Empty for results without actions.
pub fn actions_for(item: &Item) -> Vec<Item> {
    match item_kind(item) {
        ItemKind::File(p) => file_actions(&p, false),
        ItemKind::Folder(p) => file_actions(&p, true),
        ItemKind::Url(u) => url_actions(&u),
        ItemKind::Text(t) => text_actions(&t),
        ItemKind::Other => Vec::new(),
    }
}

/// Actions for text typed or selected by the user (also reachable with the
/// `actions <text>` keyword).
pub fn text_actions(text: &str) -> Vec<Item> {
    let shown = crate::providers::truncate(&text.replace('\n', " "), 60);
    let mut v = vec![act(
        format!("act:google:{text}"),
        "Search Google",
        shown.clone(),
        "⌕",
        UniversalAction::OpenUrl(format!("https://www.google.com/search?q={}", percent_encode(text))),
    )];
    if is_web_url(text) {
        v.insert(
            0,
            act(
                format!("act:url:{text}"),
                "Open URL",
                shown.clone(),
                "↗",
                UniversalAction::OpenUrl(text.to_string()),
            ),
        );
    }
    v.push(act(
        format!("act:copy:{text}"),
        "Copy",
        shown.clone(),
        "⧉",
        UniversalAction::CopyText(text.to_string()),
    ));
    v.push(act(
        format!("act:large:{text}"),
        "Large Type",
        shown,
        "A",
        UniversalAction::LargeType(text.to_string()),
    ));
    ordered(v)
}

fn is_web_url(t: &str) -> bool {
    (t.starts_with("http://") || t.starts_with("https://")) && has_scheme(t)
}

fn url_actions(url: &str) -> Vec<Item> {
    let mut v = vec![act(
        format!("act:url:{url}"),
        "Open URL",
        url,
        "↗",
        UniversalAction::OpenUrl(url.to_string()),
    )];
    v.push(act(
        format!("act:copy:{url}"),
        "Copy URL",
        url,
        "⧉",
        UniversalAction::CopyText(url.to_string()),
    ));
    v.push(act(
        format!("act:large:{url}"),
        "Large Type",
        url,
        "A",
        UniversalAction::LargeType(url.to_string()),
    ));
    v.push(act(
        format!("act:google:{url}"),
        "Search Google",
        url,
        "⌕",
        UniversalAction::OpenUrl(format!("https://www.google.com/search?q={}", percent_encode(url))),
    ));
    ordered(v)
}

/// File actions for a path that is not (yet) a result item, e.g. typed by the user.
pub fn path_actions(p: &Path) -> Vec<Item> {
    match item_kind(&Item::new("path", "path").action(Action::Open(p.to_path_buf()))) {
        ItemKind::Folder(p) => file_actions(&p, true),
        _ => file_actions(p, false),
    }
}

fn file_actions(p: &Path, folder: bool) -> Vec<Item> {
    use UniversalAction as A;
    let sub = files::tilde(p);
    let key = p.display();
    let one = || vec![p.to_path_buf()];
    let mut v = vec![
        act(format!("act:open:{key}"), "Open", sub.clone(), "▶", A::Open(p.into())),
        act(
            format!("act:openwith:{key}"),
            "Open With…",
            sub.clone(),
            "▤",
            A::OpenWithMenu(p.into()),
        ),
        act(
            format!("act:reveal:{key}"),
            "Reveal in Finder",
            sub.clone(),
            "◎",
            A::Reveal(one()),
        ),
        act(
            format!("act:copyfile:{key}"),
            "Copy File",
            sub.clone(),
            "⧉",
            A::CopyFile(one()),
        ),
        act(
            format!("act:copypath:{key}"),
            "Copy Path",
            sub.clone(),
            "⧉",
            A::CopyText(p.display().to_string()),
        ),
        act(
            format!("act:copyname:{key}"),
            "Copy Name",
            file_name(p),
            "⧉",
            A::CopyText(file_name(p)),
        ),
        act(
            format!("act:quicklook:{key}"),
            "Quick Look",
            sub.clone(),
            "◉",
            A::QuickLook(one()),
        ),
        act(
            format!("act:trash:{key}"),
            "Move to Trash",
            sub.clone(),
            "✕",
            A::Trash(one()),
        ),
        act(
            format!("act:compress:{key}"),
            "Compress",
            sub.clone(),
            "▣",
            A::Compress(one()),
        ),
    ];
    if !folder {
        v.push(act(
            format!("act:email:{key}"),
            "Email as Attachment",
            sub.clone(),
            "✉",
            A::EmailAttach(one()),
        ));
    }
    v.push(act(
        format!("act:info:{key}"),
        "Get Info",
        sub,
        "ⓘ",
        A::GetInfo(p.into()),
    ));
    ordered(v)
}

/// Applications to offer for "Open With…", listed through Spotlight and
/// fuzzy-filtered by `filter`. Nested helper apps are skipped.
pub fn open_with_items(os: &dyn Os, app_dirs: &[PathBuf], path: &Path, filter: &str, limit: usize) -> Vec<Item> {
    let mut cmd = Cmd::new("mdfind");
    for d in app_dirs {
        cmd = cmd.arg("-onlyin").arg(d);
    }
    cmd = cmd.arg("kMDItemContentType == \"com.apple.application-bundle\"");
    let Ok(out) = os.output(&cmd) else {
        return Vec::new();
    };
    let mut apps: Vec<(f64, String, PathBuf)> = String::from_utf8_lossy(&out)
        .lines()
        .map(PathBuf::from)
        .filter(|a| a.extension().is_some_and(|e| e == "app") && !a.to_string_lossy().contains(".app/"))
        .filter_map(|a| {
            let name = a.file_stem()?.to_string_lossy().to_string();
            let s = fuzzy::score(filter, &name)?;
            Some((s, name, a))
        })
        .collect();
    apps.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    apps.truncate(limit);
    apps.into_iter()
        .map(|(s, name, app)| {
            Item::new(format!("act:openwith:{}:{}", app.display(), path.display()), name)
                .subtitle(files::tilde(&app))
                .icon(Icon::App(app.clone()))
                .action(Action::Universal(UniversalAction::OpenWith {
                    path: path.to_path_buf(),
                    app,
                }))
                .score(s)
        })
        .collect()
}

// ------------------------------------------------------------------ buffer

/// The multi-select list ("buffer"): files collected from several results
/// that actions then apply to together. Data model only; the UI owns the keys.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Buffer {
    paths: Vec<PathBuf>,
}

impl Buffer {
    pub fn new() -> Buffer {
        Buffer::default()
    }

    /// Add a path (no duplicates). Unsafe paths are rejected.
    pub fn add(&mut self, p: impl Into<PathBuf>) -> Result<bool> {
        let p = p.into();
        safe_path(&p)?;
        if self.paths.contains(&p) {
            return Ok(false);
        }
        self.paths.push(p);
        Ok(true)
    }

    pub fn remove(&mut self, p: &Path) -> bool {
        let n = self.paths.len();
        self.paths.retain(|x| x != p);
        self.paths.len() != n
    }

    /// Add if absent, remove if present; true when the path is now buffered.
    pub fn toggle(&mut self, p: &Path) -> Result<bool> {
        if self.remove(p) {
            Ok(false)
        } else {
            self.add(p).map(|_| true)
        }
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }

    pub fn contains(&self, p: &Path) -> bool {
        self.paths.iter().any(|x| x == p)
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Actions applying to every buffered file. Empty when the buffer is empty.
    pub fn actions(&self) -> Vec<Item> {
        use UniversalAction as A;
        if self.is_empty() {
            return Vec::new();
        }
        let n = self.len();
        let noun = if n == 1 {
            "item".to_string()
        } else {
            format!("{n} items")
        };
        let what = if n == 1 {
            file_name(&self.paths[0])
        } else {
            format!("{n} items")
        };
        let ps = || self.paths.clone();
        let joined = |f: fn(&Path) -> String| self.paths.iter().map(|p| f(p)).collect::<Vec<_>>().join("\n");
        ordered(vec![
            act(
                "act:buf:reveal".into(),
                format!("Reveal {noun} in Finder"),
                what.clone(),
                "◎",
                A::Reveal(ps()),
            ),
            act(
                "act:buf:copyfile".into(),
                format!("Copy {noun}"),
                what.clone(),
                "⧉",
                A::CopyFile(ps()),
            ),
            act(
                "act:buf:copypath".into(),
                "Copy Paths",
                what.clone(),
                "⧉",
                A::CopyText(joined(|p| p.display().to_string())),
            ),
            act(
                "act:buf:copyname".into(),
                "Copy Names",
                what.clone(),
                "⧉",
                A::CopyText(joined(file_name)),
            ),
            act(
                "act:buf:compress".into(),
                format!("Compress {noun}"),
                what.clone(),
                "▣",
                A::Compress(ps()),
            ),
            act(
                "act:buf:email".into(),
                "Email as Attachments",
                what.clone(),
                "✉",
                A::EmailAttach(ps()),
            ),
            act(
                "act:buf:trash".into(),
                format!("Move {noun} to Trash"),
                what,
                "✕",
                A::Trash(ps()),
            ),
        ])
    }
}

// ------------------------------------------------------------------ test support

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum How {
        Run,
        Output,
        Spawn,
    }

    /// Records every command; `output` replies with canned bytes.
    #[derive(Default)]
    pub struct FakeOs {
        pub calls: Mutex<Vec<(How, Cmd)>>,
        pub stdout: Mutex<Vec<u8>>,
        pub fail: Mutex<bool>,
    }

    impl FakeOs {
        pub fn with_output(s: &str) -> FakeOs {
            let f = FakeOs::default();
            *f.stdout.lock().unwrap() = s.as_bytes().to_vec();
            f
        }
        pub fn calls(&self) -> Vec<(How, Cmd)> {
            self.calls.lock().unwrap().clone()
        }
        pub fn cmds(&self) -> Vec<Cmd> {
            self.calls().into_iter().map(|(_, c)| c).collect()
        }
        fn record(&self, how: How, cmd: &Cmd) -> Result<()> {
            self.calls.lock().unwrap().push((how, cmd.clone()));
            if *self.fail.lock().unwrap() {
                bail!("fake failure");
            }
            Ok(())
        }
    }

    impl Os for FakeOs {
        fn run(&self, cmd: &Cmd) -> Result<()> {
            self.record(How::Run, cmd)
        }
        fn output(&self, cmd: &Cmd) -> Result<Vec<u8>> {
            self.record(How::Output, cmd)?;
            Ok(self.stdout.lock().unwrap().clone())
        }
        fn spawn(&self, cmd: &Cmd) -> Result<()> {
            self.record(How::Spawn, cmd)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    /// A temp tree: <root>/docs/report.pdf, <root>/docs/sub/, <root>/-odd.txt
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(tag: &str) -> Tree {
            let root = std::env::temp_dir()
                .join(format!("oa-act-{tag}-{}", std::process::id()))
                .canonicalize_or_self();
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("docs/sub")).unwrap();
            std::fs::write(root.join("docs/report.pdf"), b"x").unwrap();
            std::fs::write(root.join("docs/notes.txt"), b"x").unwrap();
            Tree { root }
        }
        fn p(&self, rel: &str) -> PathBuf {
            self.root.join(rel)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    trait CanonOrSelf {
        fn canonicalize_or_self(self) -> PathBuf;
    }
    impl CanonOrSelf for PathBuf {
        /// macOS temp dirs are symlinks (/var -> /private/var); the root may not exist yet.
        fn canonicalize_or_self(self) -> PathBuf {
            std::fs::create_dir_all(&self).ok();
            self.canonicalize().unwrap_or(self)
        }
    }

    fn file_item(p: &Path) -> Item {
        Item::new(format!("file:{}", p.display()), "x").action(Action::Open(p.to_path_buf()))
    }

    fn titles(v: &[Item]) -> Vec<&str> {
        v.iter().map(|i| i.title.as_str()).collect()
    }

    fn universal(i: &Item) -> &UniversalAction {
        match &i.action {
            Action::Universal(u) => u,
            other => panic!("not a universal action: {other:?}"),
        }
    }

    fn find<'a>(v: &'a [Item], title: &str) -> &'a UniversalAction {
        universal(
            v.iter()
                .find(|i| i.title == title)
                .unwrap_or_else(|| panic!("no {title} in {:?}", titles(v))),
        )
    }

    #[test]
    fn file_gets_the_full_action_list_in_order() {
        let t = Tree::new("file");
        let v = actions_for(&file_item(&t.p("docs/report.pdf")));
        assert_eq!(
            titles(&v),
            [
                "Open",
                "Open With…",
                "Reveal in Finder",
                "Copy File",
                "Copy Path",
                "Copy Name",
                "Quick Look",
                "Move to Trash",
                "Compress",
                "Email as Attachment",
                "Get Info"
            ]
        );
        // scores keep the order when the UI sorts
        assert!(v.windows(2).all(|w| w[0].score > w[1].score));
        assert_eq!(
            find(&v, "Copy Path"),
            &UniversalAction::CopyText(t.p("docs/report.pdf").display().to_string())
        );
        assert_eq!(find(&v, "Copy Name"), &UniversalAction::CopyText("report.pdf".into()));
    }

    #[test]
    fn folder_has_no_email_action() {
        let t = Tree::new("folder");
        let v = actions_for(&file_item(&t.p("docs")));
        assert!(!titles(&v).contains(&"Email as Attachment"));
        assert!(titles(&v).contains(&"Compress"));
        assert_eq!(item_kind(&file_item(&t.p("docs"))), ItemKind::Folder(t.p("docs")));
    }

    #[test]
    fn app_bundles_count_as_files_not_folders() {
        let t = Tree::new("app");
        std::fs::create_dir_all(t.p("Foo.app")).unwrap();
        assert_eq!(item_kind(&file_item(&t.p("Foo.app"))), ItemKind::File(t.p("Foo.app")));
    }

    #[test]
    fn url_and_text_items() {
        let url = Item::new("u", "Site").action(Action::OpenUrl("https://example.com/a b".into()));
        assert_eq!(
            titles(&actions_for(&url)),
            ["Open URL", "Copy URL", "Large Type", "Search Google"]
        );

        let calc = Item::new("c", "14").action(Action::Copy("14".into()));
        let v = actions_for(&calc);
        assert_eq!(titles(&v), ["Search Google", "Copy", "Large Type"]);
        assert_eq!(
            find(&v, "Search Google"),
            &UniversalAction::OpenUrl("https://www.google.com/search?q=14".into())
        );

        // text that is a web URL gains Open URL first
        let v = text_actions("https://rust-lang.org");
        assert_eq!(titles(&v), ["Open URL", "Search Google", "Copy", "Large Type"]);
        // not a URL when it has spaces
        assert!(!titles(&text_actions("https://a b")).contains(&"Open URL"));

        // items that only carry copy/large-type text are text items
        let mut it = Item::new("t", "snip");
        it.copy_text = Some("hello world".into());
        assert_eq!(item_kind(&it), ItemKind::Text("hello world".into()));
        assert_eq!(
            find(&actions_for(&it), "Search Google"),
            &UniversalAction::OpenUrl("https://www.google.com/search?q=hello%20world".into())
        );
    }

    #[test]
    fn invalid_and_unrelated_items_have_no_actions() {
        let t = Tree::new("none");
        assert!(actions_for(&file_item(&t.p("docs")).valid(false)).is_empty());
        assert!(actions_for(&Item::new("x", "x")).is_empty());
        let sys = Item::new("s", "Lock").action(Action::System(crate::system::SystemCommand::Lock));
        assert!(actions_for(&sys).is_empty());
        // actions of actions are not offered
        let a = &actions_for(&file_item(&t.p("docs")))[0];
        assert!(actions_for(a).is_empty());
    }

    // ---- exact commands through the fake Os

    fn run_one(os: &FakeOs, a: UniversalAction) -> Outcome {
        run(os, &a)
    }

    #[test]
    fn open_reveal_open_with_commands() {
        let t = Tree::new("cmds");
        let f = t.p("docs/report.pdf");
        let os = FakeOs::default();
        assert_eq!(run_one(&os, UniversalAction::Open(f.clone())), Outcome::Done);
        run_one(&os, UniversalAction::Reveal(vec![f.clone(), t.p("docs/notes.txt")]));
        run_one(
            &os,
            UniversalAction::OpenWith {
                path: f.clone(),
                app: "/Applications/Preview.app".into(),
            },
        );
        assert_eq!(
            os.calls(),
            vec![
                (How::Run, Cmd::new("open").arg(&f)),
                (How::Run, Cmd::new("open").arg("-R").arg(&f).arg(t.p("docs/notes.txt"))),
                (
                    How::Run,
                    Cmd::new("open").arg("-a").arg("/Applications/Preview.app").arg(&f)
                ),
            ]
        );
    }

    #[test]
    fn open_with_menu_is_a_no_op_marker() {
        let os = FakeOs::default();
        assert_eq!(run_one(&os, UniversalAction::OpenWithMenu("/x".into())), Outcome::Done);
        assert!(os.calls().is_empty());
    }

    #[test]
    fn clipboard_quicklook_info_email_commands() {
        let t = Tree::new("cmds2");
        let f = t.p("docs/report.pdf");
        let os = FakeOs::default();
        run_one(&os, UniversalAction::CopyText("héllo\nwörld".into()));
        run_one(&os, UniversalAction::CopyFile(vec![f.clone()]));
        run_one(&os, UniversalAction::QuickLook(vec![f.clone()]));
        run_one(&os, UniversalAction::GetInfo(f.clone()));
        run_one(
            &os,
            UniversalAction::EmailAttach(vec![f.clone(), t.p("docs/notes.txt")]),
        );
        assert_eq!(
            os.calls(),
            vec![
                (How::Run, Cmd::new("pbcopy").stdin("héllo\nwörld")),
                (How::Run, Cmd::osascript(SCRIPT_COPY_FILES, [&f])),
                (How::Spawn, Cmd::new("qlmanage").arg("-p").arg(&f)),
                (How::Run, Cmd::osascript(SCRIPT_GET_INFO, [&f])),
                (
                    How::Run,
                    Cmd::osascript(SCRIPT_EMAIL, [f.clone(), t.p("docs/notes.txt")])
                ),
            ]
        );
    }

    #[test]
    fn applescript_never_contains_the_paths() {
        // Paths travel as separate argv entries, never inside the script text.
        let weird = PathBuf::from("/tmp/a\" & (do shell script \"rm -rf ~\") & \".txt");
        let os = FakeOs::default();
        run_one(&os, UniversalAction::EmailAttach(vec![weird.clone()]));
        run_one(&os, UniversalAction::GetInfo(weird.clone()));
        run_one(&os, UniversalAction::CopyFile(vec![weird.clone()]));
        for c in os.cmds() {
            let weird_arg: OsString = weird.clone().into_os_string();
            assert!(c.args.contains(&weird_arg), "path must be its own argument");
            let script_args: Vec<_> = c.args.iter().filter(|a| **a != weird_arg).collect();
            assert!(
                script_args
                    .iter()
                    .all(|a| !a.to_string_lossy().contains("do shell script"))
            );
        }
    }

    #[test]
    fn large_type_needs_no_os() {
        let os = FakeOs::default();
        assert_eq!(
            run_one(&os, UniversalAction::LargeType("hi".into())),
            Outcome::LargeType("hi".into())
        );
        assert!(os.calls().is_empty());
    }

    #[test]
    fn open_url_requires_a_scheme() {
        let os = FakeOs::default();
        assert_eq!(
            run_one(&os, UniversalAction::OpenUrl("https://e.com/?q=a%20b".into())),
            Outcome::Done
        );
        for bad in ["/bin/sh", "-a Calculator", "foo bar", "", "x:", "--help:y"] {
            assert!(
                matches!(run_one(&os, UniversalAction::OpenUrl(bad.into())), Outcome::Error(_)),
                "{bad}"
            );
        }
        assert_eq!(os.calls().len(), 1);
        assert_eq!(os.cmds()[0], Cmd::new("open").arg("https://e.com/?q=a%20b"));
    }

    #[test]
    fn rejects_dash_relative_and_empty_paths_everywhere() {
        let os = FakeOs::default();
        let bad = [PathBuf::from("-rf"), PathBuf::from("relative/file"), PathBuf::from("")];
        for b in bad {
            for a in [
                UniversalAction::Open(b.clone()),
                UniversalAction::Reveal(vec![b.clone()]),
                UniversalAction::CopyFile(vec![b.clone()]),
                UniversalAction::QuickLook(vec![b.clone()]),
                UniversalAction::Trash(vec![b.clone()]),
                UniversalAction::Compress(vec![b.clone()]),
                UniversalAction::EmailAttach(vec![b.clone()]),
                UniversalAction::GetInfo(b.clone()),
                UniversalAction::OpenWith {
                    path: b.clone(),
                    app: "/Applications/X.app".into(),
                },
                UniversalAction::OpenWith {
                    path: "/tmp/x".into(),
                    app: b.clone(),
                },
            ] {
                assert!(matches!(run(&os, &a), Outcome::Error(_)), "{a:?}");
            }
        }
        assert!(matches!(run(&os, &UniversalAction::Reveal(vec![])), Outcome::Error(_)));
        assert!(os.calls().is_empty(), "nothing may reach the OS: {:?}", os.calls());
    }

    #[test]
    fn os_failure_becomes_an_error_outcome() {
        let os = FakeOs::default();
        *os.fail.lock().unwrap() = true;
        let out = run(&os, &UniversalAction::Open("/tmp".into()));
        assert!(matches!(out, Outcome::Error(e) if e.contains("fake failure")));
    }

    // ---- trash

    #[test]
    fn trash_needs_confirmation_and_uses_resolved_paths() {
        let t = Tree::new("trash");
        let f = t.p("docs/report.pdf");
        let a = UniversalAction::Trash(vec![f.clone()]);
        assert_eq!(
            a.confirmation().unwrap(),
            "Move \"report.pdf\" to Trash? Press ↩ again to confirm."
        );
        assert!(
            UniversalAction::Trash(vec![f.clone(), t.p("docs/notes.txt")])
                .confirmation()
                .unwrap()
                .starts_with("Move 2 items")
        );

        // a path spelled through ".." and a symlinked parent resolves to the real location
        let link = t.p("link");
        std::os::unix::fs::symlink(t.p("docs"), &link).unwrap();
        let odd = t.p("docs/sub/../../link/report.pdf");
        let os = FakeOs::default();
        assert_eq!(run(&os, &UniversalAction::Trash(vec![odd])), Outcome::Done);
        assert_eq!(os.cmds(), vec![Cmd::osascript(SCRIPT_TRASH, [&f])]);
        // nothing was actually deleted by the fake
        assert!(f.exists());
    }

    #[test]
    fn trashing_a_symlink_targets_the_link_not_its_target() {
        let t = Tree::new("trash-link");
        let link = t.p("shortcut");
        std::os::unix::fs::symlink(t.p("docs"), &link).unwrap();
        let os = FakeOs::default();
        assert_eq!(run(&os, &UniversalAction::Trash(vec![link.clone()])), Outcome::Done);
        assert_eq!(os.cmds(), vec![Cmd::osascript(SCRIPT_TRASH, [&link])]);
    }

    #[test]
    fn trash_refuses_dangerous_or_missing_targets() {
        let t = Tree::new("trash-bad");
        let os = FakeOs::default();
        let home = dirs::home_dir().unwrap();
        for p in [
            PathBuf::from("/"),
            PathBuf::from("/Users"),
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Library"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/tmp"),
            home.clone(),
            home.parent().unwrap().to_path_buf(),
            t.p("docs/.."),
            t.p("does-not-exist"),
        ] {
            assert!(
                matches!(run(&os, &UniversalAction::Trash(vec![p.clone()])), Outcome::Error(_)),
                "{p:?}"
            );
        }
        // one bad item rejects the whole batch before anything runs
        let batch = UniversalAction::Trash(vec![t.p("docs/report.pdf"), PathBuf::from("/Users")]);
        assert!(matches!(run(&os, &batch), Outcome::Error(_)));
        assert!(os.calls().is_empty());
    }

    // ---- compress

    #[test]
    fn compress_single_and_multiple_with_unique_names() {
        let t = Tree::new("zip");
        let f = t.p("docs/report.pdf");
        let os = FakeOs::default();
        run(&os, &UniversalAction::Compress(vec![f.clone()]));
        let base = ["-c", "-k", "--sequesterRsrc"];
        assert_eq!(
            os.cmds()[0],
            Cmd::new("ditto").args(base).arg(&f).arg(t.p("docs/report.pdf.zip"))
        );
        // a folder keeps its name as the top-level entry
        run(&os, &UniversalAction::Compress(vec![t.p("docs/sub")]));
        assert_eq!(
            os.cmds().pop().unwrap(),
            Cmd::new("ditto")
                .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
                .arg(t.p("docs/sub"))
                .arg(t.p("docs/sub.zip"))
        );
        os.calls.lock().unwrap().pop();
        // existing archive: numbered name
        std::fs::write(t.p("docs/report.pdf.zip"), b"z").unwrap();
        run(&os, &UniversalAction::Compress(vec![f.clone()]));
        assert_eq!(
            os.cmds()[1],
            Cmd::new("ditto").args(base).arg(&f).arg(t.p("docs/report.pdf 2.zip"))
        );
        // several items (even from different folders): Archive.zip next to the first
        let n = t.p("docs/notes.txt");
        let p = t.p("pics/photo.png");
        std::fs::create_dir_all(t.p("pics")).unwrap();
        std::fs::write(&p, b"x").unwrap();
        run(&os, &UniversalAction::Compress(vec![n.clone(), p.clone()]));
        assert_eq!(
            os.cmds()[2],
            Cmd::new("tar")
                .args(["--format", "zip", "-cf"])
                .arg(t.p("docs/Archive.zip"))
                .arg("-C")
                .arg(t.p("docs"))
                .arg("notes.txt")
                .arg("-C")
                .arg(t.p("pics"))
                .arg("photo.png")
        );
        // a name that starts with '-' is passed as ./name
        std::fs::write(t.p("docs/-odd.txt"), b"x").unwrap();
        run(&os, &UniversalAction::Compress(vec![n.clone(), t.p("docs/-odd.txt")]));
        let last = os.cmds().pop().unwrap();
        assert_eq!(last.args.last().unwrap(), "./-odd.txt");
    }

    #[test]
    fn compress_refuses_missing_sources_and_self_containing_archives() {
        let t = Tree::new("zip-bad");
        let os = FakeOs::default();
        assert!(matches!(
            run(&os, &UniversalAction::Compress(vec![t.p("nope")])),
            Outcome::Error(_)
        ));
        // archive for docs/sub would land in docs, which is also being compressed
        let both = UniversalAction::Compress(vec![t.p("docs/sub"), t.p("docs")]);
        assert!(matches!(run(&os, &both), Outcome::Error(_)));
        assert!(os.calls().is_empty());
    }

    // ---- open with

    #[test]
    fn open_with_lists_filtered_top_level_apps() {
        let out = "/Applications/Safari.app\n/Applications/Preview.app\n/Applications/Safari.app/Contents/Helper.app\n/Applications/Notes.txt\n";
        let os = FakeOs::with_output(out);
        let dirs = vec![PathBuf::from("/Applications"), PathBuf::from("/System/Applications")];
        let v = open_with_items(&os, &dirs, Path::new("/tmp/x.pdf"), "", 10);
        assert_eq!(
            os.calls(),
            vec![(
                How::Output,
                Cmd::new("mdfind")
                    .args(["-onlyin", "/Applications", "-onlyin", "/System/Applications"])
                    .arg("kMDItemContentType == \"com.apple.application-bundle\"")
            )]
        );
        assert_eq!(titles(&v), ["Preview", "Safari"]);
        assert_eq!(
            universal(&v[0]),
            &UniversalAction::OpenWith {
                path: "/tmp/x.pdf".into(),
                app: "/Applications/Preview.app".into()
            }
        );
        let v = open_with_items(&os, &dirs, Path::new("/tmp/x.pdf"), "saf", 10);
        assert_eq!(titles(&v), ["Safari"]);
        // Spotlight failure: empty, no panic
        *os.fail.lock().unwrap() = true;
        assert!(open_with_items(&os, &dirs, Path::new("/tmp/x.pdf"), "", 10).is_empty());
    }

    // ---- buffer

    #[test]
    fn buffer_add_remove_clear_and_dedupe() {
        let mut b = Buffer::new();
        assert!(b.is_empty() && b.actions().is_empty());
        assert!(b.add("/a/one").unwrap());
        assert!(!b.add("/a/one").unwrap());
        assert!(b.add("/a/two").unwrap());
        assert_eq!(b.len(), 2);
        assert!(b.contains(Path::new("/a/two")));
        assert!(b.add("-bad").is_err() && b.add("rel").is_err());
        assert!(!b.toggle(Path::new("/a/one")).unwrap());
        assert!(b.toggle(Path::new("/a/one")).unwrap());
        assert_eq!(b.paths(), [PathBuf::from("/a/two"), PathBuf::from("/a/one")]);
        assert!(b.remove(Path::new("/a/two")) && !b.remove(Path::new("/a/two")));
        b.clear();
        assert!(b.is_empty());
    }

    #[test]
    fn buffer_actions_apply_to_all_files() {
        let t = Tree::new("buf");
        let (f, n) = (t.p("docs/report.pdf"), t.p("docs/notes.txt"));
        let mut b = Buffer::new();
        b.add(f.clone()).unwrap();
        b.add(n.clone()).unwrap();
        let v = b.actions();
        assert_eq!(
            titles(&v),
            [
                "Reveal 2 items in Finder",
                "Copy 2 items",
                "Copy Paths",
                "Copy Names",
                "Compress 2 items",
                "Email as Attachments",
                "Move 2 items to Trash"
            ]
        );
        assert_eq!(
            find(&v, "Copy Paths"),
            &UniversalAction::CopyText(format!("{}\n{}", f.display(), n.display()))
        );
        assert_eq!(
            find(&v, "Copy Names"),
            &UniversalAction::CopyText("report.pdf\nnotes.txt".into())
        );

        // run them all against the fake and look at the commands
        let os = FakeOs::default();
        for it in &v {
            let Action::Universal(u) = &it.action else {
                unreachable!()
            };
            assert_eq!(run(&os, u), Outcome::Done, "{}", it.title);
        }
        let c = os.cmds();
        assert_eq!(c[0], Cmd::new("open").arg("-R").arg(&f).arg(&n));
        assert_eq!(c[1], Cmd::osascript(SCRIPT_COPY_FILES, [&f, &n]));
        assert_eq!(c[4].program, OsString::from("tar"));
        assert!(c[4].args.contains(&OsString::from("report.pdf")) && c[4].args.contains(&OsString::from("notes.txt")));
        assert_eq!(c[6], Cmd::osascript(SCRIPT_TRASH, [&f, &n]));
        // only trash asks first
        let asks: Vec<_> = v
            .iter()
            .filter(|i| i.action.confirmation().is_some())
            .map(|i| i.title.as_str())
            .collect();
        assert_eq!(asks, ["Move 2 items to Trash"]);
    }

    #[test]
    fn single_item_buffer_wording() {
        let mut b = Buffer::new();
        b.add("/a/one.txt").unwrap();
        let v = b.actions();
        assert_eq!(v[0].title, "Reveal item in Finder");
        assert_eq!(v[0].subtitle, "one.txt");
    }

    // ---- manual macOS checks: `cargo test -p ashdash-core -- --ignored`
    // They only touch temp directories (and read /Applications through Spotlight).

    #[test]
    #[ignore = "needs macOS tools (ditto, tar, unzip)"]
    fn manual_real_compress_creates_valid_zips() {
        let t = Tree::new("real-zip");
        std::fs::write(t.p("-odd.txt"), b"odd").unwrap();
        let list = |zip: &Path| {
            let out = RealOs.output(&Cmd::new("unzip").arg("-Z1").arg(zip)).unwrap();
            String::from_utf8_lossy(&out)
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            run(&RealOs, &UniversalAction::Compress(vec![t.p("docs/report.pdf")])),
            Outcome::Done
        );
        assert!(list(&t.p("docs/report.pdf.zip")).contains(&"report.pdf".to_string()));
        let many = vec![t.p("docs/notes.txt"), t.p("-odd.txt"), t.p("docs/sub")];
        assert_eq!(run(&RealOs, &UniversalAction::Compress(many)), Outcome::Done);
        let names = list(&t.p("docs/Archive.zip"));
        for want in ["notes.txt", "./-odd.txt", "sub/"] {
            assert!(names.iter().any(|n| n == want), "{want} in {names:?}");
        }
    }

    #[test]
    #[ignore = "needs macOS osacompile"]
    fn manual_applescripts_compile() {
        let dir = std::env::temp_dir().join(format!("oa-osac-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, lines) in applescripts() {
            let mut c = Cmd::new("osacompile");
            for l in lines {
                c = c.arg("-e").arg(*l);
            }
            let c = c.arg("-o").arg(dir.join(format!("{name}.scpt")));
            RealOs.run(&c).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[ignore = "needs macOS Spotlight"]
    fn manual_real_open_with_lists_installed_apps() {
        let dirs = vec![PathBuf::from("/Applications"), PathBuf::from("/System/Applications")];
        let v = open_with_items(&RealOs, &dirs, Path::new("/tmp/x.txt"), "", 500);
        assert!(
            v.iter().any(|i| i.title == "Safari" || i.title == "TextEdit"),
            "{:?}",
            titles(&v)
        );
    }

    #[test]
    fn real_os_runs_and_reports_failures() {
        // harmless commands only: proves the thin wrapper works, touches nothing
        assert!(RealOs.run(&Cmd::new("true")).is_ok());
        assert!(RealOs.run(&Cmd::new("false")).is_err());
        assert_eq!(RealOs.output(&Cmd::new("echo").arg("hi")).unwrap(), b"hi\n");
        assert!(RealOs.run(&Cmd::new("cat").stdin("piped")).is_ok());
        assert!(RealOs.spawn(&Cmd::new("true")).is_ok());
        assert!(RealOs.run(&Cmd::new("definitely-not-a-program-xyz")).is_err());
    }
}
