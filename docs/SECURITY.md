# Ashdash security review and threat model

Status: wave 1, written by WP6 against commit `8efb202` (main) plus the code in this branch's tests.
Scope: `crates/core` (engine, workflows, data stores) and the places `crates/app` calls it. The GUI
itself (window, hotkey) is not reviewed here beyond what it feeds into the core.

Every finding has an ID (`SEC-nn`). Findings that can be demonstrated have a regression test in
`crates/core/tests/` marked `#[ignore = "known: SEC-nn ..."]`; un-ignore it when the fix lands.
Run the open ones with `cargo test -p ashdash-core --tests -- --ignored --no-fail-fast`
(they are expected to FAIL while the finding is open; a few allocate 200-300 MB or take 6-14 s).
Tests named `documents_*` pin intentional, reference-compatible behaviour so that nobody changes it by accident.

The code under review belongs to other work packages and is changing concurrently, so nothing here
was patched: each entry gives a suggested fix and an owner hint.

## 1. Assets, actors, trust boundaries

Assets: the user's account (anything the user can run), clipboard history (passwords, tokens), query
history (`usage.json`), snippets, the workflow directory (which is executable code), files the user
opens through the launcher.

The launcher runs as the logged-in user with no sandbox. Whoever can make it run a command already
has the user's privileges, so the question for each path is *who controls the bytes that reach it*.

| Boundary | Who controls the data | Trusted? |
|---|---|---|
| Typed query | the user (but see paste: web pages, chat) | yes for intent, no for content |
| Installed workflow (`info.plist`, scripts) | its author | **code the user chose to run**; see 3 |
| `.alfredworkflow` archive being imported | whoever sent/hosted it | archive structure untrusted, scripts trusted only after import |
| Script Filter output (JSON/XML) | the script, which may relay web APIs, file names, mail subjects | **untrusted data** |
| Clipboard contents | any app, any web page | untrusted data |
| App bundles (`Info.plist`, icons) | their publishers | untrusted data |
| `config.json`, `snippets.json`, `usage.json`, `clipboard.json` | the user / local malware | local files, treated as untrusted when parsed |
| Future `ashdash://` URLs | any web page or app | **untrusted caller**; see 6 |

Out of scope: malware already running as the user (it can edit workflows directly), kernel/OS bugs,
physical access, network attackers (the core makes no network connections itself).

## 2. Findings

Ratings: Critical = remote/one-click code execution or data theft with no user mistake;
High = code execution or crash reachable from untrusted *data* the product is designed to handle;
Medium = needs a hostile workflow/archive or a local foothold, or leaks sensitive data;
Low = hardening, robustness, privacy details.

| ID | Sev | Title | Owner area | Test | Status |
|---|---|---|---|---|---|
| SEC-01 | High | `osascript` option injection via notification text and Terminal commands | `engine.rs` | `notification_text_cannot_inject_applescript` | Fixed (FIX-B) |
| SEC-02 | High | Script Filter variables can set `PATH`, `BASH_ENV`, `DYLD_*` for downstream scripts | `workflow/exec.rs`, `script.rs` | `script_filter_variables_cannot_inject_loader_env`, `..._replace_path` | Fixed (FIX-B), rule in 9 |
| SEC-03 | Medium | Default `{query}` escaping does not stop `; \| & ' < >` newline injection | `workflow/script.rs` | `documents_default_mask_leaves_shell_metacharacters_live` (documented) | Open by design; `Workflow::trust_summary()` hook |
| SEC-04 | Low | `reveal` lacks the leading-`-` guard of the other `open` wrappers | `engine.rs` | `reveal_refuses_option_like_paths` | Fixed (FIX-B) |
| SEC-05 | Medium | Timeout kills only the direct child; backgrounded grandchildren orphaned; pipe readers block after exit | `workflow/script.rs` | `timeout_kills_grandchildren`, `backgrounded_grandchild_does_not_stall_the_script_filter` | Fixed (FIX-B) |
| SEC-06 | Medium | Script output read without a size limit | `workflow/script.rs` | `huge_script_output_is_bounded` | Fixed (FIX-B) |
| SEC-07 | Medium | Zip extraction has no size / entry-count limit (decompression bomb) | `workflow/package.rs` | `zip_bomb_is_rejected` | Fixed (FIX-B) |
| SEC-08 | High | `calc::eval` recurses without depth limit: pasting `((((...` aborts the process | `calc.rs` | `calc_deep_nesting_does_not_overflow_the_stack` | Fixed (FIX-B) |
| SEC-09 | Low | `fuzzy::score` recursion depth grows with query word count | `fuzzy.rs` | `fuzzy_pathologically_long_input_neither_crashes_nor_stalls` | Fixed (FIX-B) |
| SEC-10 | Low | Failed import leaks its `.import-<pid>` staging directory | `workflow/package.rs` | `failed_import_cleans_up_staging_dir` | Fixed (FIX-B) |
| SEC-11 | Medium | Import staging dir is per-process and install is remove-then-rename (races, non-atomic) | `workflow/package.rs` | `concurrent_imports_do_not_interfere` | Fixed (FIX-B) |
| SEC-12 | Medium | Workflow data/cache dir derived from the unsanitised bundle id (`..` escapes) | `workflow/script.rs` | `bundle_id_cannot_steer_workflow_data_dir` | Fixed (FIX-B) |
| SEC-13 | Medium | Terminal Command and `openurl` splice `{query}` unescaped; any URL scheme is opened | `workflow/exec.rs` | (design finding, see text) | Open by design; `trust_summary()` hook |
| SEC-14 | Low | Scripts inherit the launcher's whole environment | `workflow/script.rs` | `documents_scripts_inherit_parent_environment` (documented) | Open by design (documented) |
| SEC-15 | Low | Temporary script files are created 0644 | `workflow/script.rs` | `temp_script_file_is_private` | Fixed (FIX-B) |
| SEC-16 | Medium | Stdin-mode query is written before the timeout loop: an unread pipe defeats the timeout | `workflow/script.rs` | `unread_stdin_cannot_defeat_the_timeout` | Fixed (FIX-B) |
| SEC-17 | Low | App icon path from `CFBundleIconFile` (and workflow / Script Filter icon paths) not confined | `apps.rs`, `workflow/filter.rs`, `confine.rs` | `icon_file_name_is_confined_to_resources`, `item_icons_are_confined_to_the_workflow_directory`, `workflow_icon_png_symlink_falls_back_to_the_glyph`, `confine::tests` | Fixed (core-followups); app icons now use the bundle as root (wave 3 review, 9c) |
| SEC-18 | Medium | Privacy: `usage.json` stores every executed query in clear text; `config.json` and the `.tmp` files are not forced to 0600; clipboard history has no concealed-type filter | `usage.rs`, `config.rs`, `clipboard.rs` | `usage_file_is_private` covers the mode only; `concealed_transient_and_ignored_app_copies_never_reach_rows_or_disk` | Fixed in `usage.rs`/`config.rs` (FIX-B) and the clipboard watcher (WP2, re-verified in core-followups). Opt-out and clear now exist (wave 3): `learn_usage` in `config.json` (applied live through `Usage::set_enabled`), the Preferences > General switch, and "Forget what I picked..." (`Usage::clear`, via the `clear-usage` socket verb to the running launcher, or directly when none runs). **Turning learning off does not delete an existing `usage.json`**: it stays on disk (clear-text queries, mode 0600) until Forget is pressed or the file is deleted; the preferences hint says earlier picks "stop affecting the order", not that they are gone. Residual: no setting to stop clipboard recording (SEC-19 filters only) |
| SEC-19 | Low | Snippet / clipboard text can show secrets in the result list (first line of the expansion, large type) | `providers/snippets.rs`, `providers/clipboard.rs`, `engine.rs` | `crates/core/tests/clipboard_privacy.rs`, `clipboard_rows_and_their_queries_are_not_written_to_usage` | Fixed for what the core controls (core-followups); screen-share masking preference is UI work, see section 9b |
| SEC-20 | Low | `Engine::run_node` starts a workflow at any object and is not an external entry point | `engine.rs` | `run_node_*` in `engine_e2e.rs` | Design constraint (core-followups), see 9b; now keyed by workflow id (9c) |
| SEC-30 | Medium | Import prompt said "Runs no scripts and no terminal commands" for a workflow whose only object is Write File (append to `~/.zshenv`, write a LaunchAgent plist = persistence) | `workflow/trust.rs`, `gui/importprompt.rs` | `write_file_is_reported_and_is_never_passive`, `write_file_targets_are_classified_like_the_executor_resolves_them` (trust.rs); `a_workflow_that_only_writes_a_file_is_not_called_harmless`, `the_harmless_sentence_is_only_for_workflows_that_do_nothing_else` (importprompt.rs) | Fixed (FIX-TRUST), see 9c |
| SEC-31 | Medium | Every import path previewed the package and then re-opened the same path to install (TOCTOU: the file could change while the prompt was up) | `workflow/package.rs`, `engine.rs`, `gui/glue.rs`, `mac/opendocs.rs`, `main.rs`, `gui/prefs/workflows.rs` | `the_installed_bytes_are_the_previewed_bytes_even_if_the_file_is_swapped` (import_staging.rs); `the_launcher_prompt_installs_the_copy_it_showed`, `the_cli_installs_the_staged_copy_or_nothing`, `finder_import_installs_the_copy_the_dialog_described`, `installing_from_the_prefs_prompt_uses_the_staged_copy` | Fixed (FIX-TRUST), see 9c |
| SEC-32 | Medium | A held Return (key repeat) satisfied "press again to confirm" for destructive actions and the import prompt's guard | `gui/keys.rs`, `gui/mod.rs`, `gui/importprompt.rs` | `a_held_return_never_answers_the_question` (mod.rs), `a_held_return_never_installs_by_repeating`, `a_first_press_is_fresh_and_its_repeats_are_not` (keys.rs) | Fixed (FIX-TRUST), see 9c |
| SEC-33 | Low | Importing a package whose bundle id matches an installed workflow replaced it silently (outside the prefs window) and it inherited the saved settings (API tokens) | `workflow/package.rs`, `gui/importprompt.rs` | `the_preview_reports_what_would_be_replaced_and_inherited` (import_staging.rs), `a_replacement_is_announced_with_the_installed_name_and_version` | Fixed by disclosure; inheritance kept on purpose (reference-compatible), see 9c |
| SEC-34 | Low | Prefs import prompt showed package text raw; Unicode format characters (bidi override, zero-width) survived the launcher's sanitising | `gui/importprompt.rs`, `gui/prefs/workflows.rs` | `format_characters_are_stripped_and_long_names_are_capped`, `the_prefs_prompt_is_cleaned_shows_the_author_and_never_calls_a_file_writer_harmless` | Fixed (FIX-TRUST), see 9c |

No Critical finding: there is no network listener, no URL handler yet, and untrusted *archives* cannot
write outside the workflows directory (verified, see 4.1). The two Highs that touch untrusted data
(SEC-01, SEC-02) and the crash SEC-08 should be fixed before the first public build.

### SEC-01 High: `osascript` option injection

`engine.rs::osascript_args` builds `osascript -e <line>... <arg>...`. A first argument that begins with
`-e` (or another option letter) is parsed by `osascript` as more script text, not as `argv`. The
`property x : (do shell script ...)` form is evaluated at compile time, before `on run`.
Reached by: `notify(title, text)` (workflow notification objects: text is often `{query}` or a Script
Filter item argument, i.e. untrusted) and `run_in_terminal(cmd)` (Terminal Command objects and the `>` prefix).

Repro (verified on this machine; the test only `touch`es a temp file):
`notify("t", "-eproperty zz : (do shell script \"touch /tmp/x\")")`.
Also verified that `osascript -e ... -- "-eabc"` treats the argument as data.

Fix: put `"--"` between the `-e` lines and the arguments in `osascript_args`, and add an
`argv[0]`-style sentinel only if `--` proves insufficient. Keep the unit test un-ignored afterwards.

### SEC-02 High: variables are an environment injection channel

Variables returned by a Script Filter item (or set by Script/Argument objects) become process
environment of every later Run Script. They are data from outside the workflow (web API, file names).
`BASH_ENV`, `ENV`, `PATH`, `DYLD_INSERT_LIBRARIES`, `PYTHONPATH`, `PERL5OPT`, `RUBYOPT`, `NODE_OPTIONS`,
`LD_*`, `SHELLOPTS`, `BASHOPTS` give code execution in a later script that never evaluates the data.
Tests prove `BASH_ENV` and `PATH` take effect.

Fix: in `script_env`, drop (or refuse with a warning) a deny-list of loader/interpreter variables when
they come from *dynamic* variables (script or item level); allow them only when the workflow author sets
them statically in `info.plist`. Never let dynamic variables override `alfred_*` or `PATH`/`HOME`/`USER`.
The reference launcher itself exports all variables; deviating for this deny-list is a deliberate, documented compatibility cost.

### SEC-03 Medium: `{query}` substitution into script source

Argument modes: argv (`scriptargtype 1`) and stdin (`2`) are safe (tests pass hostile text through both
literally). "with input as {query}" (`0`) pastes text into source. The default mask 102 escapes `` ` `` `"` `$`
and `\` only, so inside double quotes it is safe, but unquoted or single-quoted use is injectable:
`;`, `|`, `&`, `'`, `<`, `>` and newline stay live (pinned by `documents_default_mask_leaves_shell_metacharacters_live`).
This matches the reference launcher's behaviour, so it is a UI problem more than an engine one:
- the workflow importer should warn when a script uses `{query}` in mode 0 (see 3);
- the editor/inspector should prefer argv, show a "unsafe unless quoted" badge for mode 0;
- the engine may add an opt-in "escape everything" mask for new workflows (single-quote wrapping).

Bit meanings of the mask (1/8/16) are unverified guesses in `escape_query` (its own comment says so).

### SEC-04 Low: `reveal` without `-` guard

`open_path`/`open_url` call `no_dash`; `reveal` runs `open -R <path>` without it, so a path such as
`-n` or `--args` is read as an option. Mostly harmless, but inconsistent. Fix: call `no_dash` (or prefix `./`).

### SEC-05 Medium: process lifecycle

(a) On timeout only `child.kill()` runs; `bash -c 'sleep 300 & wait'` leaves `sleep` running forever
(reparented to launchd). Fix: spawn in a new process group (`CommandExt::process_group(0)`) and `killpg(-pgid, SIGKILL)`.
(b) After the interpreter exits, the code joins the stdout/stderr reader threads. A backgrounded
grandchild that inherited the pipe keeps `read_to_end` blocking until it exits (6 s in the test; forever for
a daemon), so a Script Filter that legitimately starts a helper stalls the launcher up to its next
process kill. Fix: after the main child exits, give the readers a short grace period, then close/abandon
the pipes (read with a deadline, or `poll`); also kill the group.
(c) Zombies: `child.wait()` is always reached, so no zombies were found for the direct child.

### SEC-06 Medium: unbounded output

`read_to_end` into a `Vec`, then `from_utf8_lossy`, then JSON parse: 64 MiB of output peaks at about
200 MB RSS (estimate from the buffer copies, not instrumented). A buggy or hostile script filter can freeze or OOM the launcher.
Fix: cap each stream (suggest 4 MiB stdout / 256 KiB stderr), kill the process group on overflow and
surface an error item. Also cap number of items (e.g. 1,000) and per-field length.

### SEC-07 Medium: zip bomb

`ZipArchive::extract` has no limit on total uncompressed size, entry count or ratio. Path escaping IS handled
(see 4.1). Fix: iterate entries manually: reject > N entries (e.g. 2,000), > 100 MB total declared size,
and enforce the limit again while copying (declared sizes can lie); use `enclosed_name()` and refuse symlink
entries whose target leaves the tree; extract with `O_NOFOLLOW`-style checks.

### SEC-08 High: calculator stack overflow

The recursive-descent parser (`expr -> term -> unary -> power -> atom`) has no depth limit. A run of 10,000
`(` (or 10,000 `^` terms, or `sqrt(` x 10,000) overflows a 2 MiB stack; a stack overflow aborts the
whole process and cannot be caught. Reachable from any text pasted into the launcher because the calculator
provider evaluates every query that `looks_like_expression`. Measured with a debug build on a 2 MiB stack: parens/power/calls crash at 10,000 levels
(fine at 1,000); negation survives 10,000 and crashes at 100,000.
Fix: carry a depth counter in `Parser`, return `None` beyond ~200; also cap query length passed to `eval`
(e.g. 512 chars). Provider-level cap is the cheapest first step.

### SEC-09 Low: fuzzy recursion

`fuzzy::score` recurses per word (`words_prefix_match`, `initials_match`); ~30,000 query words, or
30,000 combining sequences, overflow a 2 MiB stack. Needs an absurd paste; the same cap on query length
as SEC-08 fixes it. WP5 is rewriting this module: carry the cap into the new implementation.

### SEC-10, SEC-11 import robustness

SEC-10: any error after extraction starts leaves `workflows/.import-<pid>` behind (and the next import
removes it only if the pid matches). Fix: a drop-guard that removes the staging dir.
SEC-11: the staging name uses only the pid. Two imports in one process (batch drop, GUI + CLI trigger
handler) share it: `remove_dir_all` in one deletes the other's files, or the installed workflow is a
mixture of both archives (demonstrated: import fails with ENOENT). The install is
`remove_dir_all(dest)` then `rename(tmp, dest)`, so a failure between the two (disk full, permissions) loses
the user's previous version, and a running workflow sees a half-installed tree. Fix: unique staging dir
(`tempfile`-style random suffix, created with `create_dir`, mode 0700), validate fully, then
swap: `rename(dest, dest.old)`, `rename(tmp, dest)`, `remove_dir_all(dest.old)`, restoring on failure.
TOCTOU note: the check-then-use window is on a directory only the user can write; the real exposure is the
workflow dir being writable by other processes of the same user, which is out of scope. Re-validating
symlinks after extraction (already done by `Workflow::load`'s reads) is sufficient.

### SEC-12 Medium: bundle id used as a path component

`script_env` builds `paths.data/workflow-data/<bundle id or id>`. `import_package` sanitises the *install*
directory name, but the bundle id inside `info.plist` is used raw at run time. `bundleid` = `../../../x` makes
the engine `create_dir_all` outside the data root and hands that path to scripts as `alfred_workflow_data`.
Fix: sanitise with the same function as the import, in one shared helper (`workflow::safe_key`).

### SEC-13 Medium: Terminal Command and URL objects

`Effect::Terminal(render(text, query, vars))` substitutes `{query}` with no escaping into text that Terminal
executes as a shell line. The reference launcher offers escaping options for this object; the importer ignores them. Any
untrusted item argument therefore is code in Terminal (after SEC-01 is fixed it is still a shell line).
`openurl` percent-encodes the query only when `utf8` is set, and `open_url` accepts any scheme: a Script Filter
returning `file:///Applications/Foo.app`, `x-apple.systempreferences:`, `ssh://`, `smb://` launches those
handlers. Fix: honour the Terminal object's escape settings (default: single-quote wrapping), and give the
UI an allow-list of schemes (`http`, `https`, `mailto`, plus those the workflow declares) with a one-time
confirmation for others.

### SEC-14 Low: environment inheritance

Scripts run with the launcher's full environment (documented by test). A GUI app launched from Finder has a
small environment, but a launcher started from a shell exposes everything in it (cloud credentials, tokens).
Fix: start from a minimal allow-list (`HOME`, `USER`, `LOGNAME`, `SHELL`, `TMPDIR`, `LANG`, `LC_*`, a fixed
`PATH`) plus workflow variables, as the reference launcher does for GUI-launched apps. Compatibility cost: low.

### SEC-15 Low: temp script permissions

`paths.cache/scripts/<pid>-<n>.<ext>` is written with `fs::write` (0644 under the default umask) and
contains the substituted `{query}`, which can be a clipboard secret. Fix: `OpenOptions::mode(0o600)` +
`create_new(true)`, and create the directory 0700. The file name is predictable (pid + counter), so `create_new`
also removes the symlink-pre-plant risk.

### SEC-16 Medium: stdin-mode deadlock

See the table. Fix: write stdin from its own thread, after the timeout supervision starts; ignore `BrokenPipe`.

### SEC-17 Low: icon path

`icns_path` joins `CFBundleIconFile` to `Contents/Resources`. A hostile bundle can name `../../../etc/x` or an
absolute path, and `sips` then parses an arbitrary file as an image (information leak is limited to
the converted PNG; the exposure is parser bugs). Fix: reject names containing `/` or `..`, and `canonicalize`
the result to verify it stays inside the bundle. WP5 owns this file.

### SEC-18 Medium: stored data privacy

- `usage.json` keeps every query the user has executed (lower-cased), with item ids. People type
  passwords, partial secrets and private names into launchers. It is 0600 (verified) but never expires.
  Fix: store a hash of the query or only prefixes up to N chars, cap the table size, add "clear learning data".
- `write_private` sets 0600 on the `.tmp` it creates, but an already-existing `.tmp` (crash leftover) keeps its old mode,
  and the temp name is shared by writers in different threads (clipboard watcher and `Engine`). Fix: unique tmp names,
  `fchmod` after open.
- `Config::save` uses `fs::write` (default umask, 0644). The file lists directories and web searches; low value, but
  should use `write_private` for consistency. The data directory itself is created with default permissions; create it 0700.
- Clipboard history: concealed/transient pasteboard types are not filtered (WP2's work item; verify that
  `org.nspasteboard.ConcealedType` entries never reach disk) and `push` rewrites the whole file synchronously under the mutex.

### SEC-19 Low: secrets in the UI

The snippet provider shows the first line of the *expanded* text in the subtitle, and `{clipboard}` expansion
means a copied password appears in the result list and in large type. Clipboard rows show text of any entry.
Fix: mask subtitles for snippets containing `{clipboard}`, add a "hide clipboard contents when screen is shared" preference.

## 3. Trust model for workflows (what the UI must tell the user)

A workflow is a program. Importing one is equivalent to installing an application: its scripts run with
the user's full privileges, without a sandbox, whenever its keyword or hotkey fires, and *Script Filters run
on every keystroke after the keyword*. Nothing in the engine can make an untrusted workflow safe, so the
product must:

1. On import, show a confirmation that states the name, bundle id, author (if present), the list of
   objects that execute code (Run Script, Script Filter, Terminal Command, scriptfile references) and the
   interpreters in use, with "Import", "Inspect first" and "Cancel". Highlight scripts that use
   `{query}` substitution (SEC-03), network tools (`curl`, `nc`), `osascript`, `sudo`, `rm -rf`.
2. Not execute anything at import time (current behaviour: verified, import only extracts and parses).
3. Treat imported workflows as disabled until the user enables them, if the confirmation is skipped by
   a drag-and-drop shortcut.
4. Show where data comes from: item arguments originating from Script Filters are untrusted (SEC-01/02/13).
5. Never auto-update a workflow's scripts silently; show a diff.
6. Not run workflows found in `workflow_dirs` of a *different* user or world-writable directories; warn if the directory is.
7. Make a kill switch visible (disable workflow, kill running scripts).

**Status after wave 3.** Points 1 and 2 are implemented for every install path: the launcher window, Finder
(`opendocs.rs`), `ashdash import [--yes]` and the preferences window all show `prompt_text` built from
`Workflow::trust_summary()` (scripts and interpreters, `{query}`-in-source, Terminal commands, Write File targets, clipboard
replacement, URL schemes, files outside the folder, external triggers, replacement and saved-settings notices; SEC-30..34)
and install the staged copy that was shown (SEC-31). "Inspect first" does not exist as a separate button (the text is the
inspection; read the files in the staging copy or the package yourself). Not implemented: point 3 (imports are enabled at once),
point 5 (no diff for replaced scripts; the prompt only says "This replaces ..."), point 6 (no check of directory ownership or
permissions of `workflow_dirs`) and highlighting of `curl`/`sudo`/`rm -rf` patterns. Point 7: Preferences > Workflows has the
enable/disable checkbox and Remove. **Bypasses of the prompt, by design:** a workflow folder placed in `workflows/` by hand and
every folder listed in `workflow_dirs` are loaded without any prompt, and since `config.json` is watched, adding a
`workflow_dirs` entry (editing the file, `ashdash config set workflow_dirs`) activates that folder's keywords, hotkeys and
External Triggers in the running launcher at once. There is no URL, socket or AppleScript verb that imports a workflow
or edits `workflow_dirs`.

Quarantine: files extracted from an archive that carries the `com.apple.quarantine` attribute should keep it
(do not strip xattrs) so Gatekeeper still checks bundled binaries on first launch.

## 4. What was verified to be safe

### 4.1 Archive extraction

The importer rejects or neutralises, with tests in `import_security.rs`: `../` entries; absolute
paths; very long, odd or BiDi/emoji entry names such as `con:/aux`, `a//b///c` and 1000-deep paths (no panic, nothing outside); symlink
entries pointing outside; chains of relative symlinks whose composition leaves the tree; a symlinked
`info.plist` that would read an outside file; a pre-planted symlink at the predictable staging path;
hostile `bundleid` values (cannot steer the install destination); empty/missing/broken plists. After every
accepted import the installed tree contains no symlink resolving outside itself. Extraction relies on the
`zip` crate's `extract`, which sanitises names; keep that crate current (it is a parser of untrusted data).

### 4.2 Argument passing

`open_url`/`open_path` refuse arguments starting with `-` and empty strings before spawning; scripts with
argv or stdin modes receive text literally; the script runner never invokes a shell on user text itself
(`Command::new(interp).arg(file)`); the temp script file is removed after each run; graph walking is
bounded (`MAX_GRAPH_STEPS`) so cycles, long chains and wide fan-out finish quickly with a warning.
System commands: `needs_confirmation` is exactly {Restart, ShutDown, LogOut, EmptyTrash}; confirmation
cannot be bypassed with the Cmd modifier; invalid items do nothing; destructive ones are not learned.
No test ever executes a destructive command.

### 4.3 File permissions

`usage.json` and `clipboard.json` are 0600 (tests). `snippets.json` is only read by the core.

### 4.4 Robustness

Corrupt `usage.json`, `clipboard.json`, `config.json` and arbitrary JSON/bytes in them never panic
(`fuzz.rs`, `engine_e2e.rs`); config from a newer version is not overwritten. Script Filter output of
arbitrary bytes, wrong JSON types, truncated and mangled JSON never panics.

## 5. Other items reviewed without a finding

- `mdfind -onlyin <home> -name <q>`: `q` is its own argv element after `-name`, so a leading `-` is data.
- `copy_to_clipboard` uses `pbcopy` with stdin; no shell.
- `Command::new` is always given a program path or fixed name; `PATH` hijacking of `open`, `osascript`,
  `pbcopy`, `mdfind`, `sips`, `pmset` is possible only if the launcher's own environment is hostile.
  Fix if desired: use absolute paths (`/usr/bin/open`, ...). Low.
- `ASHDASH_CONFIRM=1` in the CLI only affects the dev CLI.
- CLI `import`/`trigger` take paths and ids from argv; they are as trusted as the shell user.

## 6. `ashdash://` URL scheme (design requirements; implemented in `urlscheme.rs`)

A registered URL scheme lets any web page trigger actions with one click, so it must be designed as a
hostile-input boundary from day one:

1. Allow-list of verbs: `search`, `show`, `trigger` (external trigger of an *enabled* workflow), plus the harmless `reload`, `theme` (by name from the themes folder) and `prefs` (opens the preferences window). No verbs for
   import-and-run, run-script, system commands, clipboard read, or file open.
2. `trigger` must require that the workflow's External Trigger object opts in to URL invocation, and a
   per-workflow, user-visible allow setting (default: ask once, remember per bundle id).
3. The argument is data: pass it through argv/stdin, never as `{query}` source substitution, and never
   through the mode-0 path. Cap its length (e.g. 4 KiB) before it reaches the engine (SEC-08/09).
4. Parse with a real URL parser; reject userinfo, fragments with control characters, invalid UTF-8, repeated
   keys; percent-decode exactly once.
5. Rate-limit (a page can fire it in a loop) and never bring the app to the foreground differently from a user hotkey.
6. Never accept a URL that installs a workflow (`ashdash://import?url=`): download-and-install is a drive-by
   code-execution primitive (SEC-13 plus 3).
7. Log invocations (workflow id, verb, time), not the argument.
8. The CLI `trigger` command and the URL handler must go through the same `Engine::run_trigger` check.

## 7. Resource exhaustion summary

| Resource | Today | Needed |
|---|---|---|
| Script stdout/stderr | stdout 8 MiB, stderr 256 KiB, group killed (SEC-06, fixed) | ok |
| Script runtime | 10 s filter / 60 s action; whole process group killed (SEC-05/16, fixed) | ok |
| Concurrent script filters | the GUI worker drops stale *results*, but a started script is not cancelled and (unverified) a slow filter delays the next query | cancel stale runs; one in flight per workflow |
| Archive size / entries | 2,000 entries / 100 MiB / ratio ceiling (SEC-07, fixed) | ok |
| Graph traversal | bounded (256 steps) | ok |
| Query length | `calc` 512 chars + depth 200, `fuzzy` 256 chars (SEC-08/09, fixed) | a single cap at the engine entry point would be tidier |
| Clipboard history | max entries, text entries capped at 1 MiB (`MAX_TEXT_BYTES`), images size-capped; whole file still rewritten per copy | async writes |
| Script Filter item count | capped at 5,000 (core-followups) | ok |
| Icon cache | 500 files LRU, concurrent `sips` limited (WP5, done) | ok |

## 8. Suggested fix order

1. SEC-01 (`--`), SEC-08 (+ length cap), SEC-02 deny-list: small, high value.
2. SEC-05/06/16 together (one rewrite of `run_with_timeout`).
3. SEC-07/10/11 together (one rewrite of `import_package`), SEC-12.
4. SEC-18/15/14 privacy and hygiene.
5. Import confirmation UI (3) before workflows are advertised to users.

## 9. Fix log (wave 2, FIX-B, branch `fix/security`)

All `known: SEC-nn` regression tests for the fixed findings are un-ignored and pass.

- **SEC-01/04** `engine.rs`: `osascript -e ... -- <args>` (`osascript_command`), `reveal` refuses a leading `-`.
  Script objects pass the query only after the script/program (argv) or on stdin; unit tests feed
  `-eabc`, `-e return 1`, `--`, `-l JavaScript` to bash, zsh, python3, AppleScript and JXA scripts.
- **SEC-08/09** `calc.rs`: parser depth limit 200 (`expr`/`unary` entries) and 512-character input cap, both return
  `None`. `fuzzy.rs`: queries over 256 characters match nothing; initials matching is now iterative (a `u128` set of
  consumed query prefixes; the recursive search was also exponential) and ignores candidate words past the 1024th.
- **SEC-02** `workflow/script.rs::script_env`. Decision: *workflow-author variables stay as they are*
  (`info.plist` variables, configuration defaults and user-saved values are exported verbatim, even `PATH`);
  *dynamic variables* (anything that arrives through the variable set at run time: Script Filter output, scripts,
  Arg and Utility objects) are exported unless they are in the protected set, start with `alfred_`, or are not valid
  environment entries (empty name, `=` or NUL). Protected set (case-insensitive): `PATH HOME USER LOGNAME SHELL TMPDIR
  BASH_ENV ENV IFS SHELLOPTS BASHOPTS PS4 PROMPT_COMMAND CDPATH GLOBIGNORE FPATH ZDOTDIR NODE_OPTIONS NODE_PATH RUBYOPT
  RUBYLIB PERL5OPT PERL5LIB PERLLIB PHPRC PHP_INI_SCAN_DIR GEM_HOME GEM_PATH CLASSPATH JAVA_TOOL_OPTIONS _JAVA_OPTIONS
  JDK_JAVA_OPTIONS GCONV_PATH LOCPATH NLSPATH TERMINFO HOSTALIASES MALLOC_CONF` and the prefixes `DYLD_ LD_ PYTHON BASH_`.
  A dynamic variable whose value equals the author's value for the same name passes (the author's variables travel
  through the same variable set). The variable still exists for `{var:...}` templating; only the process environment is
  filtered. `ASHDASH_WORKFLOW_DEBUG=1` logs each dropped name. Compatibility cost: a workflow that deliberately sets
  `PATH` or `PYTHON*` from a Script Filter must set it statically instead.
- **SEC-05/06/16** `run_with_timeout`: every script runs in its own process group; timeout or output overflow kills the
  group with `killpg(SIGKILL)` while the child is still unreaped (no pid reuse). stdout is capped at 8 MiB (over it the
  script is killed and the caller gets `script output exceeded 8 MiB; the script was stopped`, shown as an error item);
  stderr keeps 256 KiB and drains the rest. Reader threads are abandoned 300 ms after the direct child exits, so a
  backgrounded helper holding the pipe no longer stalls the launcher (it is *not* killed on a normal exit: workflows
  legitimately start helpers). Stdin text is written by its own thread.
- **SEC-15** temp scripts: `scripts/` is 0700, files are created `O_EXCL` with mode 0600 and a pid/counter/nanosecond name.
- **SEC-12** `workflow::script::safe_key` (ASCII alphanumerics, `.`, `-`, `_`; never `.`/`..`/hidden; at most 128 bytes) is the
  single sanitiser for the run-time data/cache dirs and the install directory name.
- **SEC-07/10/11** `workflow/package.rs`: manual extraction with at most 2,000 entries, 100 MiB expanded (checked from
  the headers and again while copying), an overall ratio ceiling (200:1 above 16 MiB), unsafe names and duplicate entries
  rejected, set-uid/world-write bits dropped, symlinks created last and only when relative and resolving inside the
  tree (both lexically and after canonicalisation). Staging dir: unique, 0700, `create_dir` (never follows a pre-planted
  link), removed by a drop guard on every error. Install: old version renamed aside, new tree renamed in, old removed;
  the old version is restored if the second rename fails; two imports of the same bundle id retry instead of mixing.
- **SEC-18** `config.rs::write_private`: unique `O_EXCL` 0600 temp file + rename (stale `.tmp` files and concurrent
  writers can no longer leak an old mode or collide), new directories 0700; `Config::save` uses it (config.json is 0600,
  the data dir is created 0700). `usage.rs`: queries over 100 characters are not stored (the pick still counts
  globally), ids over 200 bytes are not recorded, at most 2,000 queries and 50 ids per query are kept, `Usage::set_enabled`
  is the opt-out switch (off: nothing is recorded, nothing is boosted), `Usage::clear` wipes memory and file.
  *Done afterwards (wave 3):* the `learn_usage` config field calls `set_enabled` at start-up and on every live reload,
  and "Forget what I picked..." / the `clear-usage` verb wipe the data; the clipboard concealed-type filter is WP2's.
  Disabling learning leaves the old `usage.json` on disk until it is cleared.
- **SEC-03/13/14** are reference-compatible behaviours and unchanged. `Workflow::trust_summary()` (`workflow/trust.rs`)
  returns a `TrustSummary` (serialisable): script-running object count, interpreters used, scripts that paste `{query}`
  into source, Open URL schemes (`{dynamic}` when not literal, `(none)` when absent), Terminal Command templates, files
  outside the workflow directory, and External Trigger count. The import confirmation (section 3) should be built on it.

## 9b. Fix log (wave 3, CORE follow-ups, branch `wp/core-followups`)

- **SEC-17** new `crates/core/src/confine.rs::confined_file(root, candidate)`: no `..`/root/prefix components, no symlink
  anywhere below the root (also links that stay inside), a regular file whose canonical path is inside the canonical root;
  the root itself may be a symlink (`/tmp`). Used for
  (a) `apps.rs::icns_path`: `CFBundleIconFile` containing `/`, `\` or NUL is refused, the rest must be a regular,
  non-symlink file in `Contents/Resources`; (b) `workflow/filter.rs::resolve_icon` (Script Filter / List Filter `icon.path`,
  relative or absolute) and `workflow_icon` (`icon.png`): the image must live inside the workflow directory. Compatibility
  cost: a Script Filter that points icons at its own cache/data directory or at a system path shows no icon (the item
  still works). If that matters, widen the root list to include the workflow's data and cache directories (needs `Paths`
  in `resolve_icon`); do not allow arbitrary absolute paths. `fileicon`/`filetype` icon types never touch the file system.
- **SEC-19** (a) Concealed / transient / auto-generated pasteboard types and ignored apps never reach `clipboard.json`
  or any row (re-verified end to end with a fake pasteboard: `clipboard_privacy.rs`). (b) Clipboard text only appears
  behind the `clip` / `clipboard` keyword: ordinary queries, `snip`, `open`, `bm` never include it. (c) A snippet whose
  expansion depends on the clipboard (directly, with an index, or through `{snippet:..}`; decided by expanding twice with
  different clipboards) shows "expands using the clipboard" instead of the expanded first line in its subtitle; Return,
  Cmd+Return, Large Type and copy still use the real expansion (explicit requests, and an existing test pins them). (d) `Engine::execute` no longer records clipboard actions in `usage.json`: the typed
  query (`clip hunter2-corr`) is stored in clear text there and clipboard rows are ordered by recency anyway.
  *Left to the UI:* a "hide clipboard contents while the screen is shared" preference; Large Type of a plain `clip` row
  still shows the entry (the user asked for it explicitly).
- **SEC-20** `Engine::run_node(bundle, node_uid, arg, vars)` runs from an arbitrary object of an enabled workflow and
  records nothing. It exists for the user's own hotkeys (`crates/app/src/hotkeys.rs`, specs built from the installed
  workflows). **It must never be reachable from `ashdash://`, the control socket or the CLI**, which may only use
  `Engine::run_trigger` (External Trigger objects that opt in). Today `ipc.rs`, `urlscheme.rs` and `automation.rs` call only
  `run_trigger` (checked with grep). Keep it that way when adding verbs.
- **Bookmarks** `bookmarks/url.rs`: the allow-list is `http` and `https` only. `ftp:` was dropped (no current macOS browser
  browses it and `open ftp://...` goes to whatever handler is registered); `file:`, `javascript:`, `data:`, `chrome:`,
  `ssh:`, `smb:`, `vnc:`, `x-apple.*:`, `tel:` and anything else were already refused, now pinned by tests.
- **Resource limits** `Config.file_search.{mdfind_timeout_ms, mdfind_max_bytes, walk_timeout_ms, walk_max_entries}`
  (defaults 4000 ms / 8 MiB / 2000 ms / 200,000 = the previous hard-coded `SearchBudget`). Values are clamped when read
  (200..30,000 ms, 64 KiB..256 MiB, 100..30,000 ms, 1,000..10,000,000), so neither a hand edit nor `0` can remove a limit;
  `ashdash config set` rejects out-of-range values because the round trip no longer returns what was stored. The Script
  Filter item count is capped at 5,000 (output size was already capped at 8 MiB).
- **Fuzzing** `crates/core/tests/fuzz.rs`: hostile values for every Config v2 section and field (wrong types, extremes, junk),
  `Config::load`/`load_strict` + save/load round trip, an `Engine` built from such settings answering queries with a null OS
  and a temp home, and `settings_view::set` with arbitrary dotted paths and values (total, atomic, result loadable).
  No defect found: the budget clamps are the only behaviour that had to be added.
- **Re-measure** (`cargo bench -p ashdash-core --bench report`, release, 5,000 synthetic apps, this machine, idle):
  engine query p95 0.42 ms overall (0.12-0.43 ms per query), warm-cache start to first query 6.2 ms, cold start 251 ms,
  idle CPU 0, real scan of 81 apps 14.7 ms. The debug-build ceiling test `query_latency_p95_at_5000_apps_stays_under_the_ceiling`
  was flaky because a single p95 over 120 samples is dominated by scheduler noise when the whole suite runs in parallel;
  it now warms up and takes the best of five independent rounds (a real regression slows every round) against the
  unchanged 25 ms ceiling and prints all rounds.

## 9c. Fix log (wave 3 review, FIX-TRUST, branch `fix/w3-trust`)

- **SEC-30** `TrustSummary` (`workflow/trust.rs`) gained `file_writes` (one `FileWriteInfo` per Write File object: the file
  name template as written, write kind overwrite/append/prepend, where it lands, whether it is decided at run time) and
  `copies_to_clipboard`; `is_passive()` is true only when the workflow can do none of the things the summary reports.
  Classification follows the executor (`exec.rs`): a relative path lands in the workflow's data, cache or own folder
  (`relativepathmode`); an absolute or `~` path, a path with `..`, or **any path with a `{placeholder}`** (a query or variable
  can bring in `/` or `..`) is "anywhere". The prompt (`importprompt::capability_lines`, now the only wording, also used by the
  workflow list and the prefs window) says "Runs no scripts and no terminal commands." only for a passive workflow; otherwise
  it leads with "Runs no scripts or Terminal commands, but it can do the following:" and lists the writes (outside / in its own
  folders), clipboard replacement, URL schemes, external files and triggers. *Decision:* Copy to Clipboard is listed (it can
  replace what the user later pastes into a terminal); Reveal in Finder (`action.revealfile`) is **not**: it only selects a file
  in Finder and neither opens, runs nor changes anything. Notification, Large Type and the other output objects are likewise
  harmless. Open File/Launch Files that run an app are still only visible as "files outside its folder" (unchanged).
- **SEC-31** `workflow::stage_package(&Paths, file)` opens the package **once**: it extracts into a private (0700) directory
  `<data>/import-staging/.import-*` (same volume as the workflows folder, so the install is a rename), parses and describes
  it (`StagedPackage::preview()`); `StagedPackage::install()` moves exactly that tree into place and the file is never read
  again. Dropping a `StagedPackage` (Esc, hide, a newer prompt, "no") deletes the copy; copies older than 24 h from a crashed
  run are swept when the next one is staged; if the copy has vanished, install fails with "open the file again" instead of
  installing something else. All four paths use it (`Engine::stage_workflow` / `install_staged`): launcher prompt, Finder
  dialog, `ashdash import`, prefs window. `import_package(file, dir)` remains the unconfirmed one-shot for tests and trusted
  scripts; `Engine::import_workflow` is stage + install. `settings_view::preview_package` was merged into this single
  implementation. Residual: the staged tree lives in the user's own data folder, so a same-user process can still modify it
  while the prompt is up (it could equally edit the installed workflow afterwards).
- **SEC-32** `gui/keys.rs::fresh_press` looks at the raw egui events: a Return/Cmd+number counts as a deliberate answer only if
  one of its key events has `repeat == false`. `confirm_step(..., fresh)` returns `Wait` for a repeat while the question is up,
  `importprompt::prompt_key` ignores repeats for the import prompt (in addition to the 600 ms guard). Harmless actions repeat as
  before; a repeat before anything was asked still only asks. Mouse clicks count as fresh. The original press that selected
  "Install Workflow..." can therefore no longer confirm it by being held.
- **SEC-33** The preview carries `replaces` (name and version of the installed workflow with the same destination folder) and
  `saved_settings` (a `workflow-config/<id>.json` exists). Every prompt says "This replaces "X" 1.0 (installed)." (title
  "Replace workflow ...") and, when values exist, that "Settings saved for this workflow identifier are kept and the new package
  can read them (they may include tokens or passwords)". *Decision:* saved values stay keyed by bundle id and are inherited,
  exactly like the reference launcher (an update must keep the user's configuration); the answer to "a hostile package claims a trusted bundle id" is
  the disclosure, since the package's own scripts could read those files anyway.
- **SEC-34** `importprompt::clean` strips Unicode format characters (category Cf: bidi embeddings/overrides/isolates,
  zero-width space/joiners, BOM, soft hyphen, tag characters; explicit table `is_format_char`, no new dependency) besides control
  characters, collapses whitespace and caps every field (name 80, other fields 160, lists with "+N more"). The prefs window
  renders `prompt_text` (same cleaning, author shown) instead of its own raw labels.

### Status of every finding (checked in core-followups)

SEC-01, 02, 04-12, 15, 16, 17, 18 (opt-out and clear wired in wave 3; old data stays until Forget), 19: fixed. SEC-03, 13, 14: open by design (reference-compatible), mitigated by
`Workflow::trust_summary()` and the import prompt. SEC-20: design constraint. SEC-30, 31, 32, 34: fixed (FIX-TRUST); SEC-33: fixed by disclosure. Nothing is without a status.

## 9c. Fix log (wave 3 review, FIX-LIVE, branch `fix/w3-live`)

Items refer to `docs/reviews/wave3.md`.

- **Hotkeys (item 2, SEC-20)** `Engine::run_node(workflow_id, node_uid, arg, vars)` now finds the workflow by its directory
  identity (`Workflow::id`), not by bundle id: workflows without a bundle id work, and a copy of a bundle id in a second
  `workflow_dirs` root can no longer be run by the other copy's hotkey. `hotkeys::collect` refuses (with a warning) a second
  enabled workflow whose *folder name* is already taken, since those two cannot be told apart. `run_node` documents that
  `node_uid` must come from an installed workflow's own hotkey objects; the grep check of SEC-20 still holds
  (`ipc.rs`, `urlscheme.rs`, `automation.rs` call only `run_trigger`).
- **Live reload (item 7)** `WorkflowHotkeys` follows `workflow_dirs` and the launcher hotkey now in force
  (`set_extra_dirs`, `set_launcher_hotkey`, called from `prefs/reload.rs`); `EngineHost::external_trigger` (the
  `ashdash://trigger` consent lookup) and the start-up hotkey hint read `Engine::config_now()`. Editing `config.json` is
  therefore a live control plane: a new `workflow_dirs` entry registers that folder's hotkeys and external triggers without
  the import prompt. This is not a privilege boundary (a same-user process can already run anything the user can), but the
  import prompt is *not* the only way a workflow becomes active; `workflow_dirs` and `config.json` are another. (Item 15.)
- **No scan on the window thread (item 9)** `Engine::apply_config` returns at once when `workflow_dirs` changed; the rescan
  runs in `Engine::reload_content_background` (`reload_pending()` is true meanwhile, so the "Reloading..." state shows).
  The hotkey registry still reads the folders itself on the window thread when it re-registers.
- **Web-search URLs (item 8)** `websearch::validate_url_template` is the single rule (http/https only, a host, no userinfo,
  only `{query}`); `providers::registry_checked` skips web searches that fail it, for hand edits, `ashdash config set` and the
  preferences window alike, and `ApplyReport.warnings` / the launcher status list each skipped entry. `prefs_model` calls the
  same function.
- **SEC-17 (item 10)** `apps.rs::icns_path` passes the *bundle* as the confinement root and `Contents/Resources/<name>` as the
  relative path, so a `Resources` that is a symlink to another folder no longer exposes that folder's images.
  `confine::confined_file_strict` additionally refuses a root that is itself a symlink (not used for app bundles or workflow
  folders: both may legitimately be links). Hard links to files outside the root are *not* detected; creating one needs prior
  local access to the same volume.
- **Config writers (item 11)** `Config::edit_locked(paths, |cfg| ...)` / `config::ConfigLock` take an exclusive `flock` on
  `<data>/run/config.lock` (0600) around read-modify-write; `Store::edit` (prefs), `ashdash config set` and `install_theme`
  use it. Reads are strict (an unreadable file is never replaced by defaults) and nothing is written when nothing changed.
  Keys this build does not know are still dropped by a save (`Config` has no catch-all); a file from a newer config version is
  refused. Writers that bypass the lock (an editor, `jq`) are not coordinated.
- **PATH (item 13)** `actions::system_program` maps `open`, `osascript`, `osacompile`, `ditto`, `qlmanage`, `sips`, `mdfind`,
  `unzip`, `tar`, `pbcopy`, `pbpaste`, `pmset` and `screencapture` to fixed `/usr/bin` (`/usr/sbin`) paths in `actions::build`, in
  the `engine.rs` open/reveal/copy/osascript wrappers and in the icon conversion. Other spawn sites (`system.rs`,
  `workflow/filefilter.rs`, `clipboard/restore.rs`) are owned by other packages and still use bare names.
- **Script Filter reruns (item 12)** `schedule::RERUN_CEILING` (5 minutes): reruns of one request stop that long after its first
  run; typing (a new request) starts a new period. Mouse or arrow-key activity does not extend it.

