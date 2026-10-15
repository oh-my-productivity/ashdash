//! File search (Spotlight with a directory-walk fallback), scope/exclusion
//! rules, kind filters, recency ranking and path navigation.
//!
//! A search is a stream: a backend feeds hits to a [`Ranker`], which applies
//! the scope while reading and keeps only the best `limit` (bounded heap), so
//! the result cap is applied after ranking. Spotlight runs under the scope's
//! [`SearchBudget`] (wall-clock timeout and output cap; the process is killed
//! and the hits read so far are used); the directory-walk fallback, used only
//! when `mdfind` cannot be run at all, is bounded by time and entries.

use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use crate::actions::{Cmd, Os, OutputBudget};
use crate::fuzzy;

pub fn tilde(path: &Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

pub fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        return dirs::home_dir().unwrap_or_default();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return dirs::home_dir().unwrap_or_default().join(rest);
    }
    PathBuf::from(s)
}

// ---------------------------------------------------------------- scope

/// Where file search looks and what it ignores. Constructed by the caller so
/// that user settings can be wired in later.
#[derive(Debug, Clone, PartialEq)]
pub struct FileSearchScope {
    /// Search roots (default: the home folder). Results outside are dropped.
    pub roots: Vec<PathBuf>,
    /// Folders (and everything below them) that are never searched.
    pub excluded_paths: Vec<PathBuf>,
    /// Any path component with one of these names excludes the result
    /// (checked relative to the root, so a root inside `target/` still works).
    pub excluded_names: Vec<String>,
    /// Results whose full path contains one of these strings are dropped.
    pub excluded_substrings: Vec<String>,
    /// Include dot-files and dot-folders.
    pub include_hidden: bool,
    /// Wall-clock and size limits for one search.
    pub budget: SearchBudget,
}

/// How much one search may cost. A search runs on the launcher's single
/// query worker, so every limit here is a promise that typing the next
/// character is never blocked for longer than `mdfind_timeout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchBudget {
    /// `mdfind` is killed after this long; hits found so far are kept.
    pub mdfind_timeout: Duration,
    /// `mdfind` output beyond this many bytes is discarded (process killed).
    pub mdfind_max_bytes: usize,
    /// The directory-walk fallback stops after this long...
    pub walk_timeout: Duration,
    /// ...or after visiting this many entries.
    pub walk_max_entries: usize,
}

impl Default for SearchBudget {
    fn default() -> Self {
        SearchBudget {
            mdfind_timeout: Duration::from_secs(4),
            mdfind_max_bytes: 8 << 20,
            walk_timeout: Duration::from_secs(2),
            walk_max_entries: 200_000,
        }
    }
}

impl SearchBudget {
    fn output_budget(&self) -> OutputBudget {
        OutputBudget {
            timeout: self.mdfind_timeout,
            max_bytes: self.mdfind_max_bytes,
        }
    }
}

impl FileSearchScope {
    /// The built-in noise lists: `node_modules`, `target`, Go's `pkg/mod`.
    pub fn new(roots: Vec<PathBuf>, excluded_paths: Vec<PathBuf>) -> FileSearchScope {
        FileSearchScope {
            roots,
            excluded_paths,
            excluded_names: vec!["node_modules".into(), "target".into()],
            excluded_substrings: vec!["/pkg/mod/".into()],
            include_hidden: false,
            budget: SearchBudget::default(),
        }
    }

    pub fn with_budget(mut self, budget: SearchBudget) -> FileSearchScope {
        self.budget = budget;
        self
    }

    /// Search `home`, skipping `~/Library` and the noise lists.
    pub fn home_default(home: &Path) -> FileSearchScope {
        FileSearchScope::new(vec![home.to_path_buf()], vec![home.join("Library")])
    }

    pub fn allows(&self, p: &Path) -> bool {
        // The most specific root decides what "relative" means.
        let Some(root) = self
            .roots
            .iter()
            .filter(|r| p.starts_with(r))
            .max_by_key(|r| r.components().count())
        else {
            return false;
        };
        if self.excluded_paths.iter().any(|e| p.starts_with(e)) {
            return false;
        }
        let full = p.to_string_lossy();
        if self.excluded_substrings.iter().any(|s| full.contains(s.as_str())) {
            return false;
        }
        let rel = p.strip_prefix(root).unwrap_or(p);
        rel.components().all(|c| {
            let n = c.as_os_str().to_string_lossy();
            (self.include_hidden || !n.starts_with('.')) && !self.excluded_names.iter().any(|x| *x == n)
        })
    }
}

impl Default for FileSearchScope {
    fn default() -> Self {
        FileSearchScope::home_default(&dirs::home_dir().unwrap_or_default())
    }
}

// ---------------------------------------------------------------- query

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Match file names (`open`, `find`, `'`).
    Name,
    /// Match file contents (`in`).
    Content,
}

/// A `kind:` filter, mapped to a UTI for Spotlight and to extensions for the
/// plain directory walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindFilter {
    pub name: String,
    pub uti: Option<&'static str>,
    pub exts: Vec<String>,
}

const FOLDER_UTI: &str = "public.folder";

/// (aliases, UTI, extensions)
type KindRow = (&'static [&'static str], &'static str, &'static [&'static str]);
const KINDS: &[KindRow] = &[
    (&["pdf"], "com.adobe.pdf", &["pdf"]),
    (
        &["image", "images", "picture", "pictures", "photo", "photos"],
        "public.image",
        &[
            "png", "jpg", "jpeg", "gif", "heic", "heif", "tif", "tiff", "bmp", "webp", "svg",
        ],
    ),
    (&["folder", "folders", "dir", "directory"], FOLDER_UTI, &[]),
    (
        &["music", "audio", "song", "songs"],
        "public.audio",
        &["mp3", "m4a", "aac", "wav", "flac", "aiff", "aif", "ogg", "opus"],
    ),
    (
        &["video", "videos", "movie", "movies"],
        "public.movie",
        &["mp4", "mov", "m4v", "avi", "mkv", "webm"],
    ),
    (&["text"], "public.text", &["txt", "md", "markdown", "rtf", "log"]),
    (
        &["presentation", "slides"],
        "public.presentation",
        &["ppt", "pptx", "key"],
    ),
    (&["spreadsheet"], "public.spreadsheet", &["xls", "xlsx", "numbers"]),
    (
        &["app", "apps", "application", "applications"],
        "com.apple.application-bundle",
        &["app"],
    ),
    (
        &["archive", "archives", "zip"],
        "public.archive",
        &["zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar"],
    ),
    (
        &["code", "source"],
        "public.source-code",
        &[
            "rs", "py", "js", "ts", "c", "h", "cpp", "go", "java", "swift", "rb", "sh",
        ],
    ),
    (&["markdown", "md"], "net.daringfireball.markdown", &["md", "markdown"]),
    (&["html"], "public.html", &["html", "htm"]),
    (&["json"], "public.json", &["json"]),
];

impl KindFilter {
    /// `pdf`, `images`, `folder`, ... or any plain extension (`rs`, `.toml`).
    pub fn parse(value: &str) -> Option<KindFilter> {
        let v = value.trim().trim_start_matches('.').to_lowercase();
        if v.is_empty() {
            return None;
        }
        if let Some((_, uti, exts)) = KINDS.iter().find(|(names, _, _)| names.contains(&v.as_str())) {
            return Some(KindFilter {
                name: v,
                uti: Some(uti),
                exts: exts.iter().map(|e| e.to_string()).collect(),
            });
        }
        if v.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '_') {
            return Some(KindFilter {
                name: v.clone(),
                uti: None,
                exts: vec![v],
            });
        }
        None
    }

    pub fn is_folder(&self) -> bool {
        self.uti == Some(FOLDER_UTI)
    }

    /// Offline check used by the directory-walk backend.
    pub fn matches_path(&self, p: &Path) -> bool {
        if self.is_folder() {
            return p.is_dir();
        }
        p.extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .is_some_and(|e| self.exts.contains(&e))
    }
}

/// A parsed file-search request.
#[derive(Debug, Clone, PartialEq)]
pub struct FileQuery {
    pub mode: SearchMode,
    /// Search words with `kind:` tokens removed.
    pub terms: String,
    /// Any of these kinds may match (empty = all kinds).
    pub kinds: Vec<KindFilter>,
}

impl FileQuery {
    /// Split `kind:pdf` style tokens (anywhere in the text) from the search words.
    pub fn parse(mode: SearchMode, text: &str) -> FileQuery {
        let mut words = Vec::new();
        let mut kinds = Vec::new();
        for tok in text.split_whitespace() {
            let lower = tok.to_lowercase();
            if let Some(vals) = lower.strip_prefix("kind:") {
                kinds.extend(vals.split(',').filter_map(KindFilter::parse));
            } else {
                words.push(tok);
            }
        }
        FileQuery {
            mode,
            terms: words.join(" "),
            kinds,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.kinds.is_empty()
    }
}

// ---------------------------------------------------------------- backends

/// One raw search hit, before scope filtering and ranking.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: PathBuf,
    /// `kMDItemLastUsedDate` (Spotlight) or the modification time (walk).
    pub last_used: Option<SystemTime>,
}

/// Whether a backend looked at everything it was asked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Completeness {
    #[default]
    Complete,
    /// A time, size or entry limit stopped the search early.
    Partial,
}

pub trait FileBackend {
    /// Feed every candidate file for `q` inside `scope.roots` to `sink`, as it
    /// is read. Backends apply the `kinds` filter themselves; scope
    /// exclusions, ranking and the result cap are the sink's business, so a
    /// huge result set never has to be held in memory.
    fn find(&self, scope: &FileSearchScope, q: &FileQuery, sink: &mut dyn FnMut(Hit)) -> Result<Completeness>;
}

fn md_term(t: &str) -> String {
    t.chars()
        .filter(|c| !matches!(c, '"' | '\\' | '*') && !c.is_control())
        .collect()
}

/// The Spotlight query string for `q`, or `None` when it has no criteria.
/// Search words are sanitised: quotes, backslashes and wildcards are removed.
pub fn spotlight_query(q: &FileQuery) -> Option<String> {
    let mut parts: Vec<String> = q
        .terms
        .split_whitespace()
        .map(md_term)
        .filter(|t| !t.is_empty())
        .map(|t| match q.mode {
            SearchMode::Name => format!("kMDItemFSName == \"*{t}*\"cd"),
            SearchMode::Content => format!("kMDItemTextContent == \"{t}*\"cd"),
        })
        .collect();
    let kinds: Vec<String> = q
        .kinds
        .iter()
        .map(|k| match (k.uti, k.exts.first()) {
            (Some(uti), _) => format!("kMDItemContentTypeTree == \"{uti}\""),
            (None, Some(ext)) => format!("kMDItemFSName == \"*.{}\"cd", md_term(ext)),
            (None, None) => "kMDItemFSName == \"*\"".to_string(),
        })
        .collect();
    if !kinds.is_empty() {
        parts.push(format!("({})", kinds.join(" || ")));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" && "))
    }
}

/// Spotlight through `mdfind`, run via the injectable [`Os`] under the
/// scope's [`SearchBudget`] (wall-clock timeout, output byte cap).
pub struct SpotlightBackend<'a> {
    pub os: &'a dyn Os,
}

impl FileBackend for SpotlightBackend<'_> {
    fn find(&self, scope: &FileSearchScope, q: &FileQuery, sink: &mut dyn FnMut(Hit)) -> Result<Completeness> {
        let Some(query) = spotlight_query(q) else {
            return Ok(Completeness::Complete);
        };
        let mut cmd = Cmd::new("mdfind").arg("-0").arg("-attr").arg("kMDItemLastUsedDate");
        for r in &scope.roots {
            cmd = cmd.arg("-onlyin").arg(r);
        }
        let out = self.os.output_bounded(&cmd.arg(query), &scope.budget.output_budget())?;
        let partial = out.truncated || out.timed_out;
        // A killed process may have been cut in the middle of a record.
        for_each_mdfind(&out.bytes, partial, sink);
        Ok(if partial {
            Completeness::Partial
        } else {
            Completeness::Complete
        })
    }
}

const ATTR_MARK: &str = " kMDItemLastUsedDate = ";

/// Parse `mdfind -0 -attr kMDItemLastUsedDate` output (plain lines also work).
pub fn parse_mdfind(out: &[u8]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for_each_mdfind(out, false, &mut |h| hits.push(h));
    hits
}

/// Stream the records of `mdfind` output to `sink`. With `drop_last` the final
/// record is skipped (it was cut off when the process was killed).
pub fn for_each_mdfind(out: &[u8], drop_last: bool, sink: &mut dyn FnMut(Hit)) {
    let sep = if out.contains(&0) { 0 } else { b'\n' };
    let mut records: Vec<&[u8]> = out.split(|b| *b == sep).collect();
    // `split` yields a trailing empty slice after a final separator; a
    // non-empty last slice is an unterminated (possibly cut) record.
    let last_unterminated = records.last().is_some_and(|r| !r.is_empty());
    if drop_last && last_unterminated {
        records.pop();
    }
    for r in records.into_iter().filter(|r| !r.is_empty()) {
        let rec = String::from_utf8_lossy(r);
        sink(match rec.rfind(ATTR_MARK) {
            Some(i) => {
                // mdfind pads the attribute column with spaces
                let mut path = &rec[..i];
                for _ in 0..2 {
                    path = path.strip_suffix(' ').unwrap_or(path);
                }
                Hit {
                    path: PathBuf::from(path),
                    last_used: parse_md_date(rec[i + ATTR_MARK.len()..].trim()),
                }
            }
            None => Hit {
                path: PathBuf::from(rec.trim_end_matches('\n').to_string()),
                last_used: None,
            },
        });
    }
}

/// `2025-10-24 20:06:16 +0000`; `(null)` and anything else give `None`.
pub fn parse_md_date(s: &str) -> Option<SystemTime> {
    let mut it = s.split_whitespace();
    let (date, time, tz) = (it.next()?, it.next()?, it.next().unwrap_or("+0000"));
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':').map(|x| x.parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let off = if tz.len() == 5 && (tz.starts_with('+') || tz.starts_with('-')) {
        let n = tz[1..].parse::<i64>().ok()?;
        let secs = (n / 100) * 3600 + (n % 100) * 60;
        if tz.starts_with('-') { -secs } else { secs }
    } else {
        0
    };
    // days since 1970-01-01 (civil-from-days inverse, Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss - off;
    u64::try_from(secs).ok().map(|s| UNIX_EPOCH + Duration::from_secs(s))
}

/// Plain directory walk: the fallback when Spotlight is unavailable, and the
/// backend the unit tests run against temp trees. Bounded by entries and time.
#[derive(Default)]
pub struct WalkBackend {
    /// Stop after visiting this many entries (0 = default of 200 000).
    pub max_entries: usize,
    /// Stop after this long (`None` = no time limit).
    pub max_duration: Option<Duration>,
}

impl WalkBackend {
    pub fn from_budget(b: &SearchBudget) -> WalkBackend {
        WalkBackend {
            max_entries: b.walk_max_entries,
            max_duration: Some(b.walk_timeout),
        }
    }
}

impl FileBackend for WalkBackend {
    fn find(&self, scope: &FileSearchScope, q: &FileQuery, sink: &mut dyn FnMut(Hit)) -> Result<Completeness> {
        if q.is_empty() {
            return Ok(Completeness::Complete);
        }
        let cap = if self.max_entries == 0 {
            200_000
        } else {
            self.max_entries
        };
        let deadline = self.max_duration.map(|d| Instant::now() + d);
        let words: Vec<String> = q.terms.split_whitespace().map(str::to_lowercase).collect();
        let mut seen = 0usize;
        for root in &scope.roots {
            let walk = walkdir::WalkDir::new(root)
                .max_depth(12)
                .into_iter()
                .filter_entry(|e| e.depth() == 0 || scope.allows(e.path()));
            for entry in walk.filter_map(|e| e.ok()) {
                seen += 1;
                // Content mode can read a megabyte per entry, so look at the clock often.
                let tick = if q.mode == SearchMode::Content { 0x0f } else { 0xff };
                if seen > cap || (seen & tick == 0 && deadline.is_some_and(|d| Instant::now() >= d)) {
                    return Ok(Completeness::Partial);
                }
                if entry.depth() == 0 {
                    continue;
                }
                let path = entry.path();
                if !q.kinds.is_empty() && !q.kinds.iter().any(|k| k.matches_path(path)) {
                    continue;
                }
                let matched = match q.mode {
                    SearchMode::Name => {
                        let name = entry.file_name().to_string_lossy().to_lowercase();
                        words.iter().all(|w| name.contains(w.as_str()))
                    }
                    SearchMode::Content => entry.file_type().is_file() && file_contains(path, &words),
                };
                if matched {
                    let last_used = entry.metadata().ok().and_then(|m| m.modified().ok());
                    sink(Hit {
                        path: path.to_path_buf(),
                        last_used,
                    });
                }
            }
        }
        Ok(Completeness::Complete)
    }
}

/// Case-insensitive check that a small text file holds every word.
fn file_contains(p: &Path, words: &[String]) -> bool {
    use std::io::Read;
    const MAX: u64 = 1 << 20;
    let Ok(f) = std::fs::File::open(p) else { return false };
    let mut buf = Vec::new();
    if f.take(MAX).read_to_end(&mut buf).is_err() {
        return false;
    }
    let Ok(text) = String::from_utf8(buf) else { return false };
    let text = text.to_lowercase();
    words.iter().all(|w| text.contains(w.as_str()))
}

// ---------------------------------------------------------------- ranking

const RECENT_MAX_BOOST: f64 = 60.0;
const RECENT_HALF_LIFE_DAYS: f64 = 14.0;

/// Up to 60 points for something used just now, halving every two weeks.
pub fn recency_boost(last_used: Option<SystemTime>, now: SystemTime) -> f64 {
    let Some(t) = last_used else { return 0.0 };
    let age_days = now.duration_since(t).unwrap_or_default().as_secs_f64() / 86_400.0;
    RECENT_MAX_BOOST * 0.5f64.powf(age_days / RECENT_HALF_LIFE_DAYS)
}

/// A ranked candidate; "greater" means "shown earlier".
struct Scored {
    score: f64,
    path: PathBuf,
}

impl PartialEq for Scored {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Scored {}
impl PartialOrd for Scored {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Scored {
    fn cmp(&self, o: &Self) -> Ordering {
        // higher score first; equal scores: the lexicographically smaller path first
        self.score.total_cmp(&o.score).then_with(|| o.path.cmp(&self.path))
    }
}

/// Everything in `words` is a (case-insensitive) substring of `name`.
fn all_words_in(words: &[String], name: &str) -> bool {
    let name = name.to_lowercase();
    words.iter().all(|w| name.contains(w.as_str()))
}

/// Below every `fuzzy` score (the weakest is a late substring match, > 0).
const WORDS_IN_NAME_SCORE: f64 = 300.0;

/// Scope-filters, deduplicates and ranks hits as they stream in, keeping only
/// the best `limit` (a bounded heap): memory stays small however large the
/// result set is, and a hit is judged on scope and score, not on where
/// Spotlight happened to list it.
///
/// Name matches are scored with the launcher's fuzzy matcher; when that
/// rejects the name (Spotlight matches each word anywhere, in any order, so
/// `report annual` finds `Annual Report.pdf`) a name containing every word
/// still ranks, below any fuzzy match. Shallower paths win ties and a recency
/// boost is added; content matches are ordered by recency and depth.
pub struct Ranker<'a> {
    scope: &'a FileSearchScope,
    mode: SearchMode,
    fuzzy: Option<fuzzy::Query>,
    words: Vec<String>,
    limit: usize,
    now: SystemTime,
    seen: HashSet<PathBuf>,
    heap: BinaryHeap<Reverse<Scored>>,
}

impl<'a> Ranker<'a> {
    pub fn new(scope: &'a FileSearchScope, q: &FileQuery, limit: usize, now: SystemTime) -> Ranker<'a> {
        let by_name = q.mode == SearchMode::Name && !q.terms.is_empty();
        Ranker {
            scope,
            mode: q.mode,
            fuzzy: by_name.then(|| fuzzy::Query::new(&q.terms)),
            words: q.terms.split_whitespace().map(str::to_lowercase).collect(),
            limit,
            now,
            seen: HashSet::new(),
            heap: BinaryHeap::new(),
        }
    }

    pub fn offer(&mut self, h: Hit) {
        if self.limit == 0 || !self.scope.allows(&h.path) || !self.seen.insert(h.path.clone()) {
            return;
        }
        let depth = h.path.components().count() as f64 * 0.1;
        let base = match (&self.fuzzy, self.mode) {
            (Some(fq), SearchMode::Name) => {
                let Some(name) = h.path.file_name().map(|n| n.to_string_lossy()) else {
                    return;
                };
                match fq.score(&fuzzy::Candidate::new(&name)) {
                    Some(s) => s,
                    None if all_words_in(&self.words, &name) => WORDS_IN_NAME_SCORE,
                    None => return,
                }
            }
            _ => 0.0,
        };
        let entry = Scored {
            score: base - depth + recency_boost(h.last_used, self.now),
            path: h.path,
        };
        if self.heap.len() < self.limit {
            self.heap.push(Reverse(entry));
        } else if self.heap.peek().is_some_and(|worst| entry > worst.0) {
            self.heap.pop();
            self.heap.push(Reverse(entry));
        }
    }

    /// The best paths, best first.
    pub fn finish(self) -> Vec<PathBuf> {
        // ascending order of `Reverse<Scored>` is descending order of `Scored`
        self.heap.into_sorted_vec().into_iter().map(|r| r.0.path).collect()
    }
}

/// Scope-filter, deduplicate and rank raw hits (see [`Ranker`]).
pub fn rank(hits: Vec<Hit>, scope: &FileSearchScope, q: &FileQuery, limit: usize, now: SystemTime) -> Vec<PathBuf> {
    let mut r = Ranker::new(scope, q, limit, now);
    hits.into_iter().for_each(|h| r.offer(h));
    r.finish()
}

/// The result of one search, and whether it saw everything.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchOutcome {
    pub paths: Vec<PathBuf>,
    pub completeness: Completeness,
}

/// Search with an explicit backend (used by tests and by [`search_files`]).
pub fn search_with_outcome(
    backend: &dyn FileBackend,
    scope: &FileSearchScope,
    q: &FileQuery,
    limit: usize,
    now: SystemTime,
) -> Result<SearchOutcome> {
    let mut ranker = Ranker::new(scope, q, limit, now);
    let completeness = backend.find(scope, q, &mut |h| ranker.offer(h))?;
    Ok(SearchOutcome {
        paths: ranker.finish(),
        completeness,
    })
}

pub fn search_with(
    backend: &dyn FileBackend,
    scope: &FileSearchScope,
    q: &FileQuery,
    limit: usize,
    now: SystemTime,
) -> Result<Vec<PathBuf>> {
    search_with_outcome(backend, scope, q, limit, now).map(|o| o.paths)
}

/// Spotlight first; only if `mdfind` itself fails to run (not when it merely
/// times out or finds nothing) a directory walk bounded by the scope's budget.
pub fn search_files_outcome(os: &dyn Os, scope: &FileSearchScope, q: &FileQuery, limit: usize) -> SearchOutcome {
    if q.is_empty() {
        return SearchOutcome::default();
    }
    let now = SystemTime::now();
    search_with_outcome(&SpotlightBackend { os }, scope, q, limit, now)
        .or_else(|_| search_with_outcome(&WalkBackend::from_budget(&scope.budget), scope, q, limit, now))
        .unwrap_or_default()
}

pub fn search_files(os: &dyn Os, scope: &FileSearchScope, q: &FileQuery, limit: usize) -> Vec<PathBuf> {
    search_files_outcome(os, scope, q, limit).paths
}

/// List a directory for path-navigation queries such as `~/Doc` or `/usr/l`.
pub fn browse(query: &str, limit: usize) -> Vec<PathBuf> {
    let full = expand_tilde(query);
    let (dir, prefix) = if query.ends_with('/') {
        (full.clone(), String::new())
    } else {
        (
            full.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/")),
            full.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
        )
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let show_hidden = prefix.starts_with('.');
    let mut v: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let n = p
                .file_name()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            (show_hidden || !n.starts_with('.')) && n.starts_with(&prefix.to_lowercase())
        })
        .collect();
    v.sort_by_key(|p| p.file_name().map(|s| s.to_string_lossy().to_lowercase()));
    v.truncate(limit);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::testing::{FakeOs, How};
    use crate::actions::{Os, OutputBudget};

    /// Temp tree (canonical root; removed on drop):
    /// docs/{Annual Report.pdf, report-draft.txt("quarterly numbers"), notes.md, blob.bin}
    /// pics/{photo.png, sub/}, apps/Foo.app/, plus noise that must never show:
    /// node_modules/, target/, .hidden/, Library/, go/pkg/mod/ — each holding a "report" file.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(tag: &str) -> Tree {
            let raw = std::env::temp_dir().join(format!("oa-files-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&raw);
            std::fs::create_dir_all(&raw).unwrap();
            let root = raw.canonicalize().unwrap();
            let w = |rel: &str, body: &[u8]| {
                let f = root.join(rel);
                std::fs::create_dir_all(f.parent().unwrap()).unwrap();
                std::fs::write(f, body).unwrap();
            };
            w("docs/Annual Report.pdf", b"%PDF");
            w("docs/report-draft.txt", b"Quarterly numbers and Totals");
            w("docs/notes.md", b"# notes\nnothing here");
            w("docs/blob.bin", &[0xff, 0xfe, 0x00, 0x71, 0x75]);
            w("pics/photo.png", b"png");
            std::fs::create_dir_all(root.join("pics/sub")).unwrap();
            std::fs::create_dir_all(root.join("apps/Foo.app")).unwrap();
            for noisy in [
                "node_modules/pkg/report.pdf",
                "target/report.txt",
                ".hidden/report.pdf",
                "Library/report.pdf",
                "go/pkg/mod/x/report.pdf",
            ] {
                w(noisy, b"quarterly");
            }
            Tree { root }
        }

        fn p(&self, rel: &str) -> PathBuf {
            self.root.join(rel)
        }

        fn scope(&self) -> FileSearchScope {
            FileSearchScope::new(vec![self.root.clone()], vec![self.root.join("Library")])
        }

        fn rel(&self, v: &[PathBuf]) -> Vec<String> {
            let mut r: Vec<_> = v
                .iter()
                .map(|p| p.strip_prefix(&self.root).unwrap().display().to_string())
                .collect();
            r.sort();
            r
        }

        fn walk(&self, mode: SearchMode, text: &str) -> Vec<String> {
            let q = FileQuery::parse(mode, text);
            let v = search_with(&WalkBackend::default(), &self.scope(), &q, 50, SystemTime::now()).unwrap();
            self.rel(&v)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn browse_lists_matching_children() {
        let t = Tree::new("browse");
        std::fs::create_dir_all(t.p("alpha")).unwrap();
        std::fs::create_dir_all(t.p("alps")).unwrap();
        let q = format!("{}/al", t.root.display());
        let names: Vec<_> = browse(&q, 10)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["alpha", "alps"]);
    }

    #[test]
    fn tilde_roundtrip() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~/x"), home.join("x"));
        assert_eq!(tilde(&home.join("x")), "~/x");
    }

    // ---- scope

    #[test]
    fn scope_applies_roots_and_exclusions() {
        let t = Tree::new("scope");
        let s = t.scope();
        assert!(s.allows(&t.p("docs/notes.md")));
        assert!(s.allows(&t.root));
        for no in [
            "node_modules/pkg/report.pdf",
            "target/report.txt",
            ".hidden/report.pdf",
            "Library/report.pdf",
            "go/pkg/mod/x/report.pdf",
        ] {
            assert!(!s.allows(&t.p(no)), "{no}");
        }
        assert!(!s.allows(Path::new("/somewhere/else.txt")), "outside every root");

        // user-configurable: hidden on, custom names/paths
        let mut s2 = t.scope();
        s2.include_hidden = true;
        s2.excluded_names = vec!["docs".into()];
        s2.excluded_paths.push(t.p("pics/sub"));
        assert!(s2.allows(&t.p(".hidden/report.pdf")));
        assert!(!s2.allows(&t.p("docs/notes.md")));
        assert!(!s2.allows(&t.p("pics/sub/x")));
        assert!(s2.allows(&t.p("pics/photo.png")));
    }

    #[test]
    fn exclusions_are_relative_to_the_root() {
        // a root that itself lives inside a "target" or dot folder still works
        let s = FileSearchScope::new(vec![PathBuf::from("/work/target/proj")], vec![]);
        assert!(s.allows(Path::new("/work/target/proj/src/a.rs")));
        assert!(!s.allows(Path::new("/work/target/proj/target/a.rs")));
        let s = FileSearchScope::new(vec![PathBuf::from("/home/u/.cfg")], vec![]);
        assert!(s.allows(Path::new("/home/u/.cfg/a")));
        assert!(!s.allows(Path::new("/home/u/.cfg/.git/a")));
    }

    #[test]
    fn multiple_roots_pick_the_most_specific() {
        let mut s = FileSearchScope::new(vec![PathBuf::from("/a"), PathBuf::from("/a/.dot")], vec![]);
        assert!(s.allows(Path::new("/a/.dot/x")), "second root makes .dot the base");
        assert!(!s.allows(Path::new("/a/.other/x")));
        s.roots.clear();
        assert!(!s.allows(Path::new("/a/x")));
    }

    #[test]
    fn default_scope_is_home_without_library() {
        let s = FileSearchScope::default();
        let home = dirs::home_dir().unwrap();
        assert_eq!(s.roots, vec![home.clone()]);
        assert!(s.allows(&home.join("Documents/a.txt")));
        assert!(!s.allows(&home.join("Library/Caches/a.txt")));
    }

    // ---- name search on a real temp tree

    #[test]
    fn name_search_skips_noise() {
        let t = Tree::new("name");
        assert_eq!(
            t.walk(SearchMode::Name, "report"),
            ["docs/Annual Report.pdf", "docs/report-draft.txt"]
        );
        // all words must match, in any case
        assert_eq!(t.walk(SearchMode::Name, "ANNUAL report"), ["docs/Annual Report.pdf"]);
        assert!(t.walk(SearchMode::Name, "zzz").is_empty());
        assert!(t.walk(SearchMode::Name, "").is_empty());
    }

    #[test]
    fn hidden_files_can_be_included() {
        let t = Tree::new("hidden");
        let mut s = t.scope();
        s.include_hidden = true;
        let q = FileQuery::parse(SearchMode::Name, "report");
        let v = search_with(&WalkBackend::default(), &s, &q, 50, SystemTime::now()).unwrap();
        assert!(t.rel(&v).contains(&".hidden/report.pdf".to_string()));
        assert!(!t.rel(&v).contains(&"node_modules/pkg/report.pdf".to_string()));
    }

    #[test]
    fn limit_is_applied() {
        let t = Tree::new("limit");
        let q = FileQuery::parse(SearchMode::Name, "report");
        let v = search_with(&WalkBackend::default(), &t.scope(), &q, 1, SystemTime::now()).unwrap();
        assert_eq!(v.len(), 1);
    }

    // ---- kind filters

    #[test]
    fn kind_tokens_are_parsed_anywhere() {
        let q = FileQuery::parse(SearchMode::Name, "kind:PDF annual kind:png,jpg report");
        assert_eq!(q.terms, "annual report");
        let names: Vec<_> = q.kinds.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(names, ["pdf", "png", "jpg"]);
        assert_eq!(q.kinds[0].uti, Some("com.adobe.pdf"));
        assert_eq!(q.kinds[1].uti, None, "plain extension has no UTI");
        // aliases and junk
        assert_eq!(KindFilter::parse("images").unwrap().uti, Some("public.image"));
        assert_eq!(KindFilter::parse(".rs").unwrap().exts, ["rs"]);
        assert!(KindFilter::parse("a b").is_none() && KindFilter::parse("").is_none());
        assert!(FileQuery::parse(SearchMode::Name, "kind:").is_empty());
        assert!(!FileQuery::parse(SearchMode::Name, "kind:pdf").is_empty());
    }

    #[test]
    fn kind_filters_select_files() {
        let t = Tree::new("kind");
        assert_eq!(t.walk(SearchMode::Name, "report kind:pdf"), ["docs/Annual Report.pdf"]);
        assert_eq!(t.walk(SearchMode::Name, "kind:image"), ["pics/photo.png"]);
        assert_eq!(t.walk(SearchMode::Name, "kind:md"), ["docs/notes.md"]);
        assert_eq!(
            t.walk(SearchMode::Name, "kind:pdf,png"),
            ["docs/Annual Report.pdf", "pics/photo.png"]
        );
        assert_eq!(
            t.walk(SearchMode::Name, "kind:bin"),
            ["docs/blob.bin"],
            "unknown kind = extension"
        );
        assert!(t.walk(SearchMode::Name, "kind:video").is_empty());
        let folders = t.walk(SearchMode::Name, "kind:folder");
        assert!(folders.contains(&"pics/sub".to_string()) && folders.contains(&"docs".to_string()));
        assert!(!folders.iter().any(|f| f.ends_with(".pdf")));
        assert!(!folders.iter().any(|f| f.contains("node_modules")));
        assert_eq!(t.walk(SearchMode::Name, "kind:app"), ["apps/Foo.app"]);
    }

    // ---- content search

    #[test]
    fn content_search_reads_text_files_only() {
        let t = Tree::new("content");
        assert_eq!(t.walk(SearchMode::Content, "quarterly"), ["docs/report-draft.txt"]);
        assert_eq!(
            t.walk(SearchMode::Content, "TOTALS quarterly"),
            ["docs/report-draft.txt"]
        );
        assert!(t.walk(SearchMode::Content, "quarterly missing").is_empty());
        // binary (non UTF-8) is skipped even if bytes look like the word
        assert!(t.walk(SearchMode::Content, "qu").iter().all(|p| p != "docs/blob.bin"));
        // content mode combines with kind
        assert_eq!(t.walk(SearchMode::Content, "notes kind:md"), ["docs/notes.md"]);
        assert!(t.walk(SearchMode::Content, "quarterly kind:pdf").is_empty());
    }

    // ---- Spotlight command construction and parsing

    #[test]
    fn spotlight_queries() {
        let name = FileQuery::parse(SearchMode::Name, "annual report kind:pdf");
        assert_eq!(
            spotlight_query(&name).unwrap(),
            "kMDItemFSName == \"*annual*\"cd && kMDItemFSName == \"*report*\"cd && (kMDItemContentTypeTree == \"com.adobe.pdf\")"
        );
        let content = FileQuery::parse(SearchMode::Content, "budget kind:rs,folder");
        assert_eq!(
            spotlight_query(&content).unwrap(),
            "kMDItemTextContent == \"budget*\"cd && (kMDItemFSName == \"*.rs\"cd || kMDItemContentTypeTree == \"public.folder\")"
        );
        assert_eq!(
            spotlight_query(&FileQuery::parse(SearchMode::Name, "kind:image")).unwrap(),
            "(kMDItemContentTypeTree == \"public.image\")"
        );
        assert_eq!(spotlight_query(&FileQuery::parse(SearchMode::Name, "")), None);
    }

    #[test]
    fn spotlight_terms_cannot_inject_query_syntax() {
        let q = FileQuery::parse(SearchMode::Name, r#"a"b*c\d ") || kMDItemFSName == "*"#);
        let s = spotlight_query(&q).unwrap();
        // quotes, wildcards and backslashes are stripped from user text
        assert!(s.starts_with("kMDItemFSName == \"*abcd*\"cd && "), "{s}");
        // every quote left in the query belongs to our own syntax: 2 per clause
        let clauses = s.matches(" && ").count() + 1;
        assert_eq!(s.matches('"').count(), clauses * 2, "{s}");
    }

    #[test]
    fn spotlight_backend_runs_exact_command_and_ranks_by_recency() {
        let t = Tree::new("spot");
        let recent = "2999-01-01 00:00:00 +0000";
        let old = "2001-01-01 00:00:00 +0000";
        let out = [
            format!(
                "{}   kMDItemLastUsedDate = {old}",
                t.p("docs/Annual Report.pdf").display()
            ),
            format!(
                "{}   kMDItemLastUsedDate = (null)",
                t.p("docs/report-draft.txt").display()
            ),
            format!(
                "{}   kMDItemLastUsedDate = {recent}",
                t.p("docs/report.final.pdf").display()
            ),
            format!(
                "{}   kMDItemLastUsedDate = {recent}",
                t.p("Library/report.pdf").display()
            ),
            format!(
                "{}   kMDItemLastUsedDate = {recent}",
                t.p("node_modules/pkg/report.pdf").display()
            ),
        ]
        .join("\0");
        let os = FakeOs::with_output(&out);
        let q = FileQuery::parse(SearchMode::Name, "report");
        let found = search_files(&os, &t.scope(), &q, 10);
        assert_eq!(
            os.calls(),
            vec![(
                How::Output,
                Cmd::new("mdfind")
                    .args(["-0", "-attr", "kMDItemLastUsedDate", "-onlyin"])
                    .arg(&t.root)
                    .arg("kMDItemFSName == \"*report*\"cd")
            )]
        );
        // Library / node_modules dropped; the recently used file leads
        assert_eq!(found[0], t.p("docs/report.final.pdf"));
        assert_eq!(found.len(), 3);
        assert!(!found.iter().any(|p| p.starts_with(t.p("Library"))));
    }

    #[test]
    fn spotlight_failure_falls_back_to_walking_the_scope() {
        let t = Tree::new("fallback");
        let os = FakeOs::default();
        *os.fail.lock().unwrap() = true;
        let q = FileQuery::parse(SearchMode::Name, "kind:png");
        assert_eq!(t.rel(&search_files(&os, &t.scope(), &q, 10)), ["pics/photo.png"]);
        // an empty query never reaches the OS
        let os = FakeOs::default();
        assert!(search_files(&os, &t.scope(), &FileQuery::parse(SearchMode::Name, ""), 10).is_empty());
        assert!(os.calls().is_empty());
    }

    #[test]
    fn mdfind_output_with_spaces_and_plain_lines() {
        let hits = parse_mdfind(
            b"/a/My File.txt   kMDItemLastUsedDate = 2025-10-24 20:06:16 +0000\0/b/x   kMDItemLastUsedDate = (null)\0",
        );
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].path, PathBuf::from("/a/My File.txt"));
        assert_eq!(hits[0].last_used, Some(UNIX_EPOCH + Duration::from_secs(1_761_336_376)));
        assert_eq!(hits[1].path, PathBuf::from("/b/x"));
        assert_eq!(hits[1].last_used, None);
        // plain `mdfind` output (one path per line)
        let plain = parse_mdfind(b"/a/b.txt\n/c/d.txt\n");
        assert_eq!(
            plain.iter().map(|h| h.path.clone()).collect::<Vec<_>>(),
            [PathBuf::from("/a/b.txt"), PathBuf::from("/c/d.txt")]
        );
        assert!(parse_mdfind(b"").is_empty());
    }

    #[test]
    fn mdfind_dates() {
        let d = |s| parse_md_date(s).map(|t| t.duration_since(UNIX_EPOCH).unwrap().as_secs());
        assert_eq!(d("1970-01-02 00:00:00 +0000"), Some(86_400));
        assert_eq!(d("2024-02-29 12:00:00 +0000"), Some(1_709_208_000));
        assert_eq!(d("2024-02-29 12:00:00 -0500"), Some(1_709_208_000 + 5 * 3600));
        assert_eq!(d("2024-02-29 12:00:00 +0130"), Some(1_709_208_000 - 5400));
        assert_eq!(d("(null)"), None);
        assert_eq!(d("2024-13-01 00:00:00 +0000"), None);
        assert_eq!(d("1960-01-01 00:00:00 +0000"), None, "before the epoch");
        assert_eq!(d("garbage in"), None);
    }

    // ---- ranking

    fn hit(p: &str, age_days: Option<f64>, now: SystemTime) -> Hit {
        Hit {
            path: PathBuf::from(p),
            last_used: age_days.map(|d| now - Duration::from_secs_f64(d * 86_400.0)),
        }
    }

    #[test]
    fn recent_files_rank_higher_among_equal_matches() {
        let now = SystemTime::now();
        let scope = FileSearchScope::new(vec![PathBuf::from("/r")], vec![]);
        let q = FileQuery::parse(SearchMode::Name, "plan");
        let hits = vec![
            hit("/r/a/plan one.txt", None, now),
            hit("/r/a/plan two.txt", Some(200.0), now),
            hit("/r/a/plan three.txt", Some(0.1), now),
            hit("/r/a/plan three.txt", Some(0.1), now), // duplicate hit
        ];
        let v = rank(hits, &scope, &q, 10, now);
        let names: Vec<_> = v
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["plan three.txt", "plan two.txt", "plan one.txt"]);
    }

    #[test]
    fn an_exact_name_beats_a_merely_recent_one() {
        let now = SystemTime::now();
        let scope = FileSearchScope::new(vec![PathBuf::from("/r")], vec![]);
        let q = FileQuery::parse(SearchMode::Name, "budget");
        let hits = vec![
            hit("/r/old-budget-notes-archive.txt", Some(0.0), now),
            hit("/r/budget", Some(365.0), now),
        ];
        assert_eq!(rank(hits, &scope, &q, 10, now)[0], PathBuf::from("/r/budget"));
    }

    #[test]
    fn recency_boost_decays() {
        let now = SystemTime::now();
        let b = |d: f64| recency_boost(Some(now - Duration::from_secs_f64(d * 86_400.0)), now);
        assert!((b(0.0) - 60.0).abs() < 0.01);
        assert!((b(14.0) - 30.0).abs() < 0.01);
        assert!(b(1.0) > b(10.0) && b(10.0) > b(100.0));
        assert_eq!(recency_boost(None, now), 0.0);
        // a timestamp in the future counts as "just now"
        assert!((recency_boost(Some(now + Duration::from_secs(3600)), now) - 60.0).abs() < 0.01);
    }

    // ---- review fixes: scope before cap, bounded work, multi-word names

    fn plain_lines(paths: impl IntoIterator<Item = String>) -> String {
        paths.into_iter().collect::<Vec<_>>().join("\n") + "\n"
    }

    #[test]
    fn scope_filtering_happens_before_any_cap() {
        // 6 000 noise hits come first in Spotlight order (more than the old 5 000 cut),
        // then the three the user actually wants.
        let t = Tree::new("scope-first");
        let noise = (0..6000).map(|i| t.p(&format!("node_modules/m{i}/report.pdf")).display().to_string());
        let wanted = ["docs/Annual Report.pdf", "docs/report-draft.txt", "pics/report.png"]
            .iter()
            .map(|r| t.p(r).display().to_string());
        let os = FakeOs::with_output(&plain_lines(noise.chain(wanted)));
        let q = FileQuery::parse(SearchMode::Name, "report");
        let found = search_files(&os, &t.scope(), &q, 10);
        assert_eq!(
            t.rel(&found),
            ["docs/Annual Report.pdf", "docs/report-draft.txt", "pics/report.png"]
        );
    }

    #[test]
    fn the_cap_applies_after_ranking_not_in_spotlight_order() {
        let t = Tree::new("rank-then-cap");
        let old = "2001-01-01 00:00:00 +0000";
        let recent = "2999-01-01 00:00:00 +0000";
        let rec = |name: &str, d: &str| format!("{}   kMDItemLastUsedDate = {d}", t.p(name).display());
        // listed first: 300 old files; listed last: one recently used file
        let mut recs: Vec<String> = (0..300).map(|i| rec(&format!("docs/plan {i}.txt"), old)).collect();
        recs.push(rec("docs/plan latest.txt", recent));
        let os = FakeOs::with_output(&recs.join("\0"));
        let q = FileQuery::parse(SearchMode::Name, "plan");
        let found = search_files(&os, &t.scope(), &q, 3);
        assert_eq!(found.len(), 3);
        assert_eq!(
            found[0],
            t.p("docs/plan latest.txt"),
            "the recent file survives the cap"
        );
    }

    #[test]
    fn bounded_heap_matches_a_full_sort() {
        let now = SystemTime::now();
        let scope = FileSearchScope::new(vec![PathBuf::from("/r")], vec![]);
        let q = FileQuery::parse(SearchMode::Name, "doc");
        let hits: Vec<Hit> = (0..200)
            .map(|i| hit(&format!("/r/d{}/doc {i}.txt", i % 7), Some((i * 37 % 100) as f64), now))
            .collect();
        let all = rank(hits.clone(), &scope, &q, usize::MAX, now);
        for limit in [0, 1, 5, 50, 199, 200, 500] {
            let top = rank(hits.clone(), &scope, &q, limit, now);
            assert_eq!(top, all[..limit.min(all.len())], "limit {limit}");
        }
    }

    #[test]
    fn multi_word_queries_keep_hits_whose_words_are_out_of_order() {
        let t = Tree::new("multiword");
        let os = FakeOs::with_output(&plain_lines(
            ["docs/Annual Report.pdf", "docs/report-draft.txt", "docs/notes.md"]
                .iter()
                .map(|r| t.p(r).display().to_string()),
        ));
        let found = |text: &str| {
            let q = FileQuery::parse(SearchMode::Name, text);
            t.rel(&search_files(&os, &t.scope(), &q, 10))
        };
        // Spotlight matches each word anywhere in the name; so do we, in any order
        assert_eq!(found("report annual"), ["docs/Annual Report.pdf"]);
        assert_eq!(found("ann port"), ["docs/Annual Report.pdf"]);
        assert_eq!(found("pdf REPORT"), ["docs/Annual Report.pdf"]);
        // every word must be inside the name
        assert!(found("report missing").is_empty());
        // in-order matches still come first
        let q = FileQuery::parse(SearchMode::Name, "report draft");
        assert_eq!(t.rel(&search_files(&os, &t.scope(), &q, 10)), ["docs/report-draft.txt"]);
    }

    #[test]
    fn a_fuzzy_match_outranks_an_all_words_match() {
        let now = SystemTime::now();
        let scope = FileSearchScope::new(vec![PathBuf::from("/r")], vec![]);
        let q = FileQuery::parse(SearchMode::Name, "annual report");
        let hits = vec![
            hit("/r/report annual.txt", Some(0.0), now),   // only the fallback matches
            hit("/r/Annual Report.txt", Some(300.0), now), // ordered word prefixes
        ];
        let v = rank(hits, &scope, &q, 10, now);
        assert_eq!(
            v,
            [
                PathBuf::from("/r/Annual Report.txt"),
                PathBuf::from("/r/report annual.txt")
            ]
        );
    }

    #[test]
    fn mdfind_gets_the_scopes_budget() {
        let t = Tree::new("budget");
        let os = FakeOs::default();
        let budget = SearchBudget {
            mdfind_timeout: Duration::from_millis(1234),
            mdfind_max_bytes: 4321,
            ..SearchBudget::default()
        };
        let scope = t.scope().with_budget(budget);
        let q = FileQuery::parse(SearchMode::Name, "report");
        search_files(&os, &scope, &q, 5);
        assert_eq!(
            os.budgets.lock().unwrap().as_slice(),
            [OutputBudget {
                timeout: Duration::from_millis(1234),
                max_bytes: 4321
            }]
        );
        // defaults are sane: seconds, megabytes
        let d = SearchBudget::default();
        assert!(d.mdfind_timeout >= Duration::from_secs(1) && d.mdfind_timeout <= Duration::from_secs(10));
        assert!(d.mdfind_max_bytes >= 1 << 20);
    }

    #[test]
    fn a_cut_off_final_record_is_dropped_but_complete_ones_survive() {
        let mut got = Vec::new();
        for_each_mdfind(b"/a/one\0/b/two\0/c/thr", true, &mut |h| got.push(h.path));
        assert_eq!(got, [PathBuf::from("/a/one"), PathBuf::from("/b/two")]);
        // output that ended exactly on a separator loses nothing
        let mut got = Vec::new();
        for_each_mdfind(b"/a/one\0/b/two\0", true, &mut |h| got.push(h.path));
        assert_eq!(got.len(), 2);
        // untruncated output keeps an unterminated final record
        let mut got = Vec::new();
        for_each_mdfind(b"/a/one\0/b/two", false, &mut |h| got.push(h.path));
        assert_eq!(got.len(), 2);
    }

    /// Runs another command in place of the one it is asked to run, through the
    /// real bounded runner: a stand-in for a hung or runaway `mdfind`.
    struct Stand(Cmd);

    impl Os for Stand {
        fn run(&self, _: &Cmd) -> Result<()> {
            unreachable!()
        }
        fn output(&self, _: &Cmd) -> Result<Vec<u8>> {
            unreachable!()
        }
        fn output_bounded(&self, _: &Cmd, b: &OutputBudget) -> Result<crate::actions::BoundedOutput> {
            crate::actions::RealOs.output_bounded(&self.0, b)
        }
        fn spawn(&self, _: &Cmd) -> Result<Box<dyn crate::actions::Process>> {
            unreachable!()
        }
    }

    #[test]
    fn a_hung_mdfind_is_killed_and_the_search_returns_in_time() {
        let t = Tree::new("hung");
        let scope = t.scope().with_budget(SearchBudget {
            mdfind_timeout: Duration::from_millis(300),
            ..SearchBudget::default()
        });
        let q = FileQuery::parse(SearchMode::Name, "report");
        let started = Instant::now();
        let out = search_files_outcome(&Stand(Cmd::new("sleep").arg("30")), &scope, &q, 10);
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert_eq!(out.completeness, Completeness::Partial);
        assert!(out.paths.is_empty());
        // a timeout is not an mdfind failure: no slow directory walk is started on top of it
    }

    #[test]
    fn a_runaway_mdfind_is_cut_at_the_byte_cap_and_keeps_what_it_found() {
        let t = Tree::new("runaway");
        let scope = t.scope().with_budget(SearchBudget {
            mdfind_max_bytes: 50_000,
            ..SearchBudget::default()
        });
        let q = FileQuery::parse(SearchMode::Name, "report");
        let line = t.p("docs/Annual Report.pdf").display().to_string();
        let started = Instant::now();
        let out = search_files_outcome(&Stand(Cmd::new("yes").arg(&line)), &scope, &q, 10);
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        assert_eq!(out.completeness, Completeness::Partial);
        assert_eq!(out.paths, [t.p("docs/Annual Report.pdf")], "deduplicated, in scope");
    }

    #[test]
    fn walk_fallback_stops_at_its_entry_and_time_limits() {
        let t = Tree::new("walk-bound");
        let q = FileQuery::parse(SearchMode::Name, "report");
        let few = WalkBackend {
            max_entries: 3,
            ..WalkBackend::default()
        };
        let mut n = 0;
        let c = few.find(&t.scope(), &q, &mut |_| n += 1).unwrap();
        assert_eq!(c, Completeness::Partial);
        assert!(n <= 3);
        // a deadline in the past stops it (checked every few hundred entries)
        for i in 0..600 {
            std::fs::write(t.p(&format!("docs/f{i}.txt")), b"x").unwrap();
        }
        let timed = WalkBackend {
            max_entries: 0,
            max_duration: Some(Duration::ZERO),
        };
        assert_eq!(timed.find(&t.scope(), &q, &mut |_| {}).unwrap(), Completeness::Partial);
        // unbounded walk completes
        let full = WalkBackend::default();
        assert_eq!(full.find(&t.scope(), &q, &mut |_| {}).unwrap(), Completeness::Complete);
        // a failed mdfind falls back to a walk built from the scope's budget
        let os = FakeOs::default();
        *os.fail.lock().unwrap() = true;
        let scope = t.scope().with_budget(SearchBudget {
            walk_max_entries: 2,
            ..SearchBudget::default()
        });
        assert_eq!(
            search_files_outcome(&os, &scope, &q, 10).completeness,
            Completeness::Partial
        );
    }

    #[test]
    #[ignore = "needs macOS Spotlight"]
    fn manual_real_spotlight_finds_apps_and_parses_output() {
        use crate::actions::RealOs;
        let scope = FileSearchScope::new(
            vec![PathBuf::from("/Applications"), PathBuf::from("/System/Applications")],
            vec![],
        );
        let q = FileQuery::parse(SearchMode::Name, "safari kind:app");
        let v = search_files(&RealOs, &scope, &q, 5);
        assert!(v.iter().any(|p| p.ends_with("Safari.app")), "{v:?}");
        // content search runs and returns without error (result set may be empty)
        let q = FileQuery::parse(SearchMode::Content, "zzzzqqqq-nonexistent");
        assert!(search_files(&RealOs, &scope, &q, 5).is_empty());
    }

    #[test]
    fn shallower_paths_win_ties() {
        let now = SystemTime::now();
        let scope = FileSearchScope::new(vec![PathBuf::from("/r")], vec![]);
        let q = FileQuery::parse(SearchMode::Name, "todo");
        let hits = vec![hit("/r/a/b/c/todo.txt", None, now), hit("/r/todo.txt", None, now)];
        assert_eq!(rank(hits, &scope, &q, 10, now)[0], PathBuf::from("/r/todo.txt"));
    }
}
