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
| SEC-17 | Low | App icon path from `CFBundleIconFile` (and workflow / Script Filter icon paths) not confined | `apps.rs`, `workflow/filter.rs`, `confine.rs` | `icon_file_name_is_confined_to_resources`, `item_icons_are_confined_to_the_workflow_directory`, `workflow_icon_png_symlink_falls_back_to_the_glyph`, `workflow_roots_refuse_hard_links_but_app_roots_do_not`, `confine::tests` | Fixed (core-followups); app icons now use the bundle as root (wave 3 review, 9c); hard-linked files in workflow folders refused (W4-FIX, 9d) |
| SEC-18 | Medium | Privacy: `usage.json` stores every executed query in clear text; `config.json` and the `.tmp` files are not forced to 0600; clipboard history has no concealed-type filter | `usage.rs`, `config.rs`, `clipboard.rs` | `usage_file_is_private` covers the mode only; `concealed_transient_and_ignored_app_copies_never_reach_rows_or_disk` | Fixed in `usage.rs`/`config.rs` (FIX-B) and the clipboard watcher (WP2, re-verified in core-followups). Opt-out and clear now exist (wave 3): `learn_usage` in `config.json` (applied live through `Usage::set_enabled`), the Preferences > General switch, and "Forget what I picked..." (`Usage::clear`, via the `clear-usage` socket verb to the running launcher, or directly when none runs). **Turning learning off does not delete an existing `usage.json`**: it stays on disk (clear-text queries, mode 0600) until Forget is pressed or the file is deleted; the preferences hint says earlier picks "stop affecting the order", not that they are gone. Residual: no setting to stop clipboard recording (SEC-19 filters only) |
| SEC-19 | Low | Snippet / clipboard text can show secrets in the result list (first line of the expansion, large type) | `providers/snippets.rs`, `providers/clipboard.rs`, `engine.rs` | `crates/core/tests/clipboard_privacy.rs`, `clipboard_rows_and_their_queries_are_not_written_to_usage` | Fixed for what the core controls (core-followups); screen-share masking preference is UI work, see section 9b |
| SEC-20 | Low | `Engine::run_node` starts a workflow at any object and is not an external entry point | `engine.rs` | `run_node_*` in `engine_e2e.rs` | Design constraint (core-followups), see 9b; now keyed by workflow id (9c) |
| SEC-30 | Medium | Import prompt said "Runs no scripts and no terminal commands" for a workflow whose only object is Write File (append to `~/.zshenv`, write a LaunchAgent plist = persistence) | `workflow/trust.rs`, `gui/importprompt.rs` | `write_file_is_reported_and_is_never_passive`, `write_file_targets_are_classified_like_the_executor_resolves_them` (trust.rs); `a_workflow_that_only_writes_a_file_is_not_called_harmless`, `the_harmless_sentence_is_only_for_workflows_that_do_nothing_else` (importprompt.rs) | Fixed (FIX-TRUST), see 9c |
| SEC-31 | Medium | Every import path previewed the package and then re-opened the same path to install (TOCTOU: the file could change while the prompt was up) | `workflow/package.rs`, `engine.rs`, `gui/glue.rs`, `mac/opendocs.rs`, `main.rs`, `gui/prefs/workflows.rs` | `the_installed_bytes_are_the_previewed_bytes_even_if_the_file_is_swapped` (import_staging.rs); `the_launcher_prompt_installs_the_copy_it_showed`, `the_cli_installs_the_staged_copy_or_nothing`, `finder_import_installs_the_copy_the_dialog_described`, `installing_from_the_prefs_prompt_uses_the_staged_copy` | Fixed (FIX-TRUST), see 9c |
| SEC-32 | Medium | A held Return (key repeat) satisfied "press again to confirm" for destructive actions and the import prompt's guard | `gui/keys.rs`, `gui/mod.rs`, `gui/importprompt.rs` | `a_held_return_never_answers_the_question` (mod.rs), `a_held_return_never_installs_by_repeating`, `a_first_press_is_fresh_and_its_repeats_are_not` (keys.rs) | Fixed (FIX-TRUST), see 9c |
| SEC-33 | Low | Importing a package whose bundle id matches an installed workflow replaced it silently (outside the prefs window) and it inherited the saved settings (API tokens) | `workflow/package.rs`, `gui/importprompt.rs` | `the_preview_reports_what_would_be_replaced_and_inherited` (import_staging.rs), `a_replacement_is_announced_with_the_installed_name_and_version` | Fixed by disclosure; inheritance kept on purpose (reference-compatible), see 9c |
| SEC-34 | Low | Prefs import prompt showed package text raw; Unicode format characters (bidi override, zero-width) survived the launcher's sanitising | `gui/importprompt.rs`, `gui/prefs/workflows.rs` | `format_characters_are_stripped_and_long_names_are_capped`, `the_prefs_prompt_is_cleaned_shows_the_author_and_never_calls_a_file_writer_harmless` | Fixed (FIX-TRUST), see 9c |
| SEC-35 | Medium | Open File with an empty file field (= the incoming argument) or a `{placeholder}` path was invisible to the trust summary, so a workflow that launches an app or script shipped in its own package was called passive | `workflow/trust.rs`, `gui/importprompt.rs` | `open_file_with_an_empty_or_dynamic_field_is_never_passive` (trust.rs), `files_opened_at_run_time_are_listed_and_not_called_harmless` (importprompt.rs) | Fixed (FIX-R2), see 9c |
| SEC-36 | Medium | Write File with "relative to the workflow folder" could overwrite the workflow's own `info.plist`/scripts after the prompt and was listed as harmless "own folders" | `workflow/trust.rs`, `workflow/exec.rs`, `gui/importprompt.rs` | `write_file_never_rewrites_the_workflows_own_info_plist` (workflow/tests.rs), `writing_into_its_own_installed_folder_is_a_separate_warning` (importprompt.rs) | Fixed for `info.plist` and disclosed for the rest (FIX-R2), see 9c |
| SEC-37 | Low | The trust summary could not say that a Script Filter's items (`arg`, `variables`, `quicklookurl`) steer later Run Script / Open URL / Open File / Write File / Terminal steps, so such a workflow could look passive | `workflow/trust.rs`, `gui/importprompt.rs` | `script_filter_results_steering_later_steps_are_flagged`, `script_filter_into_harmless_steps_or_unwired_acting_steps_is_not_flagged` (trust.rs); `script_filter_steering_is_stated` (importprompt.rs) | Fixed by disclosure (W4-FIX), see 9d |
| SEC-38 | Low | `ashdash import` ignored unknown `--` flags (`--yess` silently meant "ask") and the preferences drop zone accepted only `.alfredworkflow`, so the same package was handled differently per path | `main.rs`, `gui/prefs/app.rs`, `gui/prefs/workflows.rs`, `workflow/package.rs` | `import_rejects_unknown_flags_and_wrong_file_counts` (main.rs), `the_drop_zone_accepts_every_workflow_extension_the_launcher_does`, `every_reference_package_extension_is_recognised` | Fixed (W4-FIX), see 9d |
| SEC-39 | Low | `write_workflow_value` trusted its caller to check the variable: the preferences window did not, so any key could be stored in `workflow-config/<bundle id>.json` and reach the workflow's scripts as an environment variable | `settings_view.rs` | `write_workflow_value_only_accepts_variables_on_the_sheet` | Fixed (W4-FIX), see 9d |
| SEC-R4-01 | Medium | Open File `openwith` and Open URL `browser` (and "Search the web" `browser`) were not read by the trust summary, and a relative literal `sourcefile` went to `open` unresolved (the launcher's working directory): a workflow that launches an app shipped in its own package was called passive ("Runs no scripts...") | `workflow/trust.rs`, `workflow/exec.rs`, `gui/importprompt.rs` | `open_file_openwith_and_open_url_browser_are_launched_applications`, `a_relative_literal_file_is_a_file_shipped_with_the_workflow` (trust.rs); `relative_open_targets_and_apps_resolve_against_the_workflow_folder` (exec.rs); `a_launched_application_is_a_capability_line_and_a_shipped_one_is_called_out`, `the_harmless_sentence_is_only_for_workflows_that_do_nothing_else` (importprompt.rs) | Fixed (W4-R1), see 9e |
| SEC-R4-02 | Medium | Write File could still rewrite the workflow's `info.plist` through a package symlink (`notes.txt -> info.plist`): the refusal compared path text and the parent folder only, and `perform` followed links | `workflow/exec.rs` | `write_file_does_not_follow_a_package_symlink_to_info_plist`, `write_file_still_writes_through_a_link_to_an_ordinary_file` (exec.rs) | Fixed (W4-R1), see 9e |
| SEC-R4-04 | Medium | File Filter ran `mdfind` with no timeout and no output cap (a types-only predicate matched 430 572 files, 58.7 MB, 18 s) on the single query worker | `workflow/filefilter.rs`, `workflow/filter.rs`, `providers/workflow.rs` | `mdfind_goes_through_the_bounded_runner_with_the_search_budget`, `mdfind_output_is_capped_before_strings_are_built`, `an_empty_argument_with_a_types_only_predicate_stays_bounded` (filefilter.rs); `a_file_filter_runs_through_the_engines_os_with_the_configured_budget` (providers/workflow.rs) | Fixed (W4-R1), see 9e |
| SEC-R4-09 | Low | A cross-volume install removed the old version before a long copy, and `load_all` read the hidden `.import-*` folders, so a reload or crash could show a phantom duplicate; leftovers were never swept | `workflow/package.rs`, `workflow/model.rs` | `load_all_skips_dot_directories_so_a_swap_in_progress_is_not_a_second_workflow`, `the_old_version_stays_visible_until_the_new_copy_is_complete`, `interrupted_installs_are_swept_and_a_lost_workflow_is_put_back`, `staging_a_package_sweeps_the_workflows_folder`, `a_failed_same_volume_install_gives_the_staged_tree_back` (package.rs) | Fixed (W4-R1), see 9e |
| SEC-R4-10 | Low | `config set web_searches '<array>'` compared the number of bad entries, so a new list with a different bad entry passed when an old bad entry existed | `settings_view.rs` | `replacing_the_whole_web_search_list_checks_every_entry_not_the_count` | Fixed (W4-R1), see 9e |
| SEC-R4-14 | Low | The spawn guard test matched only the one-line form `Command::new("name")`; `system_program`'s behaviour for a missing tool was undocumented | `tests/system_programs.rs`, `actions.rs` | `no_bare_system_tool_names_are_spawned`, `the_scan_sees_calls_the_old_line_match_missed`, `every_known_tool_has_one_absolute_path_and_never_falls_back_to_path` | Tightened and documented (W4-R1), see 9e |
| SEC-50 | Medium | A ~2 MB property list of nested `<array>` elements (a theme opened from Finder, `ashdash theme`, an app's `Info.plist`) overflowed the stack inside the `plist` crate and aborted the launcher | `theme.rs`, `apps.rs`, `mac/opendocs.rs` | `a_deeply_nested_theme_is_refused_not_a_stack_overflow`, `nesting_up_to_the_cap_parses_and_one_more_level_does_not`, `binary_plists_are_checked_too`, `every_kind_of_value_passes_the_probe_in_both_encodings` (theme.rs); `a_hostile_info_plist_is_ignored_not_fatal` (apps.rs); `a_deeply_nested_theme_is_refused_not_fatal` (opendocs.rs) | Fixed (W4-R2), see 9e; workflow `info.plist` and the value bound added by W4-R3 (SEC-R5-01, SEC-R5-02, 9g) |
| SEC-51 | Medium | `ashdash://search` typed attacker text into the launcher *and ran it*, so a web page could start a workflow's Script Filter with `{query}` chosen by the page, with no consent dialog | `urlscheme.rs`, `events.rs`, `gui/glue.rs`, `gui/mod.rs` | `search_from_a_url_never_becomes_a_query` (urlscheme.rs); the GUI side is on-screen only (QA) | Fixed (W4-R2): the URL only fills the field, see 9e and section 6 |
| SEC-52 | Low | URL consent dialog: one dialog per link (a page could stack dozens, each living up to 60 s); a timeout was stored as a permanent "deny"; `url-trust.json` was read once, so "delete the entry to be asked again" needed a restart and a hand edit was overwritten | `urlscheme.rs` | `only_one_question_per_bundle_is_open_at_a_time`, `the_claim_is_released_when_the_question_fails`, `a_timeout_or_dismissed_dialog_is_not_a_decision`, `the_dialog_result_maps_to_an_answer`, `edits_to_the_consent_file_are_seen_without_a_restart` | Fixed (W4-R2), see 9e |
| SEC-53 | Medium | Snippet auto-paste went through the pasteboard (`pbcopy`, no marker type), so a snippet holding a token or password was recorded in `clipboard.json` in clear text | `clipboard/{command,history,watch,filter}.rs` | `a_snippet_pasted_by_ashdash_is_not_recorded_but_the_users_own_copies_are`, `an_old_note_does_not_hide_a_later_copy`, `markers_still_win_over_the_own_paste_note` (watch.rs); `snippet_pastes_are_noted_for_the_watcher_but_history_pastes_are_not`, `a_failed_snippet_copy_leaves_no_note_behind` (command.rs) | Fixed (W4-R2); live pasteboard not run, see 9e |
| SEC-54 | Low | `urlscheme::parse` sliced the URL at a byte offset: `ashdash:/é` panicked the dispatch thread | `urlscheme.rs` | `multibyte_text_anywhere_never_panics_the_parser` | Fixed (W4-R2) |
| SEC-55 | Low | The IPC connection counter was decremented after the handler returned: sixteen panicking handlers locked the socket out until restart | `ipc.rs` | `panicking_handlers_do_not_use_up_the_connection_slots`, `a_connection_slot_is_released_when_its_guard_drops_even_while_unwinding` | Fixed (W4-R2) |
| SEC-56 | Low | AppleScript `search` / `run trigger` skipped the length caps of the socket and the URL scheme; Finder import messages showed raw file names (bidi, zero-width); a theme was read three times (check vs. copy) | `scripting.rs`, `mac/opendocs.rs` | `script_commands_apply_the_same_length_caps_as_the_other_entry_points`; `finder_messages_clean_file_names_like_the_import_prompt`, `theme_install_reads_the_file_once_and_installs_what_it_validated` | Fixed (W4-R2) |

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
- `copy_to_clipboard` uses `pbcopy` with stdin; no shell. `copy_to_clipboard_transient` (workflow Copy to Clipboard with "transient" ticked, SEC-R5-10) runs `osascript -l JavaScript` with the text as an argument after `--`.
- `Command::new` is always given a program path or fixed name; `PATH` hijacking of `open`, `osascript`,
  `pbcopy`, `mdfind`, `sips`, `pmset` was possible only if the launcher's own environment is hostile. Fixed: every such tool
  is started by absolute path (`actions::system_command`, guarded by `tests/system_programs.rs`; see the fix logs below).
- `ASHDASH_CONFIRM=1` in the CLI only affects the dev CLI.
- CLI `import`/`trigger` take paths and ids from argv; they are as trusted as the shell user.

## 6. `ashdash://` URL scheme (design requirements; implemented in `urlscheme.rs`)

A registered URL scheme lets any web page trigger actions with one click, so it must be designed as a
hostile-input boundary from day one:

1. Allow-list of verbs: `search`, `show`, `trigger` (external trigger of an *enabled* workflow), plus the harmless `reload`, `theme` (by name from the themes folder) and `prefs` (opens the preferences window). No verbs for
   import-and-run, run-script, system commands, clipboard read, or file open.
   **`search` does not search** (W4-R2, SEC-51): it shows the launcher with the text in the field (`AppEvent::Prefill`) and runs *no* query, so no workflow keyword or Script
   Filter sees text chosen by a web page until the user edits the field (a keystroke then runs the query as usual; the footer is set to "Text from a link: edit it to search" - the footer text is code-only, not seen on screen).
   The CLI socket and AppleScript `search` are local, same-user controls and still run the query (`AppEvent::Search`).
2. `trigger` must require that the workflow's External Trigger object opts in to URL invocation, and a
   per-workflow, user-visible allow setting (default: ask once, remember per bundle id). At most one question per bundle id is on
   screen (further links for it are dropped, W4-R2); only "Allow" and "Don't Allow" are stored, a timeout or dismissed dialog is
   not (the next link asks again); `url-trust.json` is re-read when its modification time or size changes, and `set` merges over the
   current file, so deleting an entry takes effect in a running launcher and a hand edit is not overwritten (a file that does not parse keeps the last good decisions in memory and is moved to `url-trust.json.bad` before the next write, SEC-R5-03). Residual: different
   bundle ids can each have their own dialog open (bounded by the 6-per-10-s limiter and the number of installed workflows with an External Trigger).
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
  harmless. Open File/Launch Files that run an app were still only visible as "files outside its folder" (a gap closed by SEC-35 in the re-review fixes).
- **SEC-31** `workflow::stage_package(&Paths, file)` opens the package **once**: it extracts into a private (0700) directory
  `<data>/import-staging/.import-*` (normally the same volume as the workflows folder, so the install is a rename; on `EXDEV` it is copied beside the destination first, see FIX-R2), parses and describes
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
`Workflow::trust_summary()` and the import prompt. SEC-20: design constraint. SEC-30, 31, 32, 34: fixed (FIX-TRUST); SEC-33: fixed by disclosure; SEC-35, 36: fixed (FIX-R2); SEC-37..39: fixed (W4-FIX); SEC-R4-01, 02, 04, 09, 10, 14: fixed or tightened (W4-R1; the `SEC-R4-nn` ids are the issue numbers of `docs/reviews/wave4.md`). Nothing is without a status.

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
  (Wave 3: the hotkey registry still read the folders itself on the window thread when it re-registered; fixed in 9d.)
- **Web-search URLs (item 8)** `websearch::validate_url_template` is the single rule (http/https only, a host, no userinfo,
  only `{query}`); `providers::registry_checked` skips web searches that fail it, for hand edits, `ashdash config set` and the
  preferences window alike, and `ApplyReport.warnings` / the launcher status list each skipped entry. `prefs_model` calls the
  same function.
- **SEC-17 (item 10)** `apps.rs::icns_path` passes the *bundle* as the confinement root and `Contents/Resources/<name>` as the
  relative path, so a `Resources` that is a symlink to another folder no longer exposes that folder's images.
  `confine::confined_file_strict` additionally refuses a root that is itself a symlink (not used for app bundles or workflow
  folders: both may legitimately be links). Hard links were not detected in wave 3; see 9d.
- **Config writers (item 11)** `Config::edit_locked(paths, |cfg| ...)` / `config::ConfigLock` take an exclusive `flock` on
  `<data>/run/config.lock` (0600) around read-modify-write; `Store::edit` (prefs), `ashdash config set` and `install_theme`
  use it. Reads are strict (an unreadable file is never replaced by defaults) and nothing is written when nothing changed.
  (Wave 3: keys this build does not know were dropped by a save; fixed in 9d.) A file from a newer config version is
  refused. Writers that bypass the lock (an editor, `jq`) are not coordinated.
- **PATH (item 13)** `actions::system_program` maps `open`, `osascript`, `osacompile`, `ditto`, `qlmanage`, `sips`, `mdfind`,
  `unzip`, `tar`, `pbcopy`, `pbpaste`, `pmset` and `screencapture` to fixed `/usr/bin` (`/usr/sbin`) paths in `actions::build`, in
  the `engine.rs` open/reveal/copy/osascript wrappers and in the icon conversion. The other spawn sites (`system.rs`,
  `workflow/filefilter.rs`, `clipboard/restore.rs`, ...) were moved to `actions::system_command` by FIX-R2 (below).
- **Script Filter reruns (item 12)** `schedule::RERUN_CEILING` (5 minutes): reruns of one request stop that long after its first
  run; typing (a new request) starts a new period. Mouse or arrow-key activity does not extend it.


## 9c. Fix log (wave 3 re-review, FIX-R2, branch `fix/w3-r2`)

- **SEC-35 (N1)** `TrustSummary.opens_at_run_time` lists Open File / Launch Files targets that are produced at run time: an empty
  file field (the executor then opens the incoming argument; shown as "(the incoming argument)") and any path with a
  `{placeholder}`; literal paths outside the folder stay in `external_files`. Such a workflow is never passive and the prompt says
  "Opens files chosen at run time: ... (this can be an app or script shipped inside the package)". The old test that pinned the
  empty field as harmless was corrected, not weakened. Residual: what a Script Filter or script *returns* is still invisible (the summary now says so when a Script Filter feeds an acting step: SEC-37).
- **SEC-36 (N2)** A Write File object in the workflow's own installed folder (`relativepathmode` 2) now has its own prompt line
  ("Can modify its own files (scripts, settings) after you install it") and is no longer listed under the data/cache line. *Decision
  on the executor:* the executor refuses to write the workflow's own `info.plist` (compared after resolving `.`/`..`, ignoring case,
  and by real location when both exist), whatever the path is spelled like, in every mode, with a warning; this closes "add
  objects after the prompt". (The wave 4 review found that a package symlink got around the textual comparison; SEC-R4-02 below
  resolves the file itself and checks the opened file's identity.) Other files in the folder stay writable (the reference launcher semantics; a workflow may keep a settings file there).
  Residual: a script file the workflow already runs can still be rewritten, and a Run Script can write anywhere anyway, so this is
  a disclosure plus one hard rule, not a sandbox.
- **Cross-volume install (N3)** *(order changed by SEC-R4-09, see 9e)* `package::install_with` falls back, on `EXDEV`, to copying the staged tree (modes, links)
  into a hidden folder *beside the destination*, fsyncing files and directories, and renaming that into place; the copy is removed
  on any failure and the previous version is restored as before. Tested with an injected rename (`install_across_volumes_copies_next_to_the_destination`,
  `a_failed_cross_volume_install_keeps_the_old_version_and_leaves_nothing_behind`).
- **Reload leftovers (N4)** workflow hotkeys are re-read after a successful launcher-hotkey rebind (the old key is free) and only
  once `Engine::reload_pending()` is false (a hotkey fired before the background refresh would find no workflow); the reload gates
  release themselves if the worker panics (`GateRelease`), so `reload_pending()` cannot stay true.
- **Bare tool names (N5)** `actions::system_command(name)`; `system.rs` (Lock/Sleep/Log out/..), `filefilter.rs`, `clipboard/restore.rs`,
  `clipboard_watch.rs`, `mac/opendocs.rs` and `prefs/dialogs.rs` use it. `crates/core/tests/system_programs.rs` scans both crates and fails
  on a new `Command::new("osascript")` (or any other name in the table) outside `actions.rs`. `zip` (tests only) is not in the table. (Tightened by SEC-R4-14, see 9e.)
- **Web search URLs (N6)** `settings_view::set` (and so `ashdash config set`) refuses a value that makes a web search invalid (changed by
  SEC-R4-10: every entry of a new list is checked, not the count); an entry that is already bad never blocks unrelated edits or its own repair. A search skipped at start-up is kept in
  `Engine::startup_warnings()` and shown as a "settings" problem (status file + one notification), like a live reload.
- **`clean` (N7)** blank-looking characters (U+3164, U+115F/1160, U+FFA0, U+2800, U+17B4/17B5, U+034F, U+2065) are shown as spaces and
  collapse; a field made only of them reads as empty ("not stated" / "(unnamed)"). Variation selectors are left alone (emoji).
- **Test hygiene** `trash_refuses_a_symlink_that_points_at_a_protected_place` creates `$HOME/Library` itself.

## 9d. Fix log (wave 4, W4-FIX, branch `wp/w4-fix`): wave 3 leftovers

- **Unknown config keys (SEC-18 follow-up)** `Config::save` (and so `edit_locked`, the preferences window and `ashdash config set`) merges the
  typed settings over the file's current, migrated JSON: keys `Config` has no field for survive at the top level and inside nested objects
  (`clipboard`, `file_search`, `bookmarks`); migrated keys (`clipboard_history_size`) do not come back. Arrays (web searches, root
  lists) are replaced as a whole, so unknown keys *inside array elements* are still dropped. A file from a newer config version is
  still refused. Tests: `unknown_keys_survive_a_locked_edit`, `migrated_keys_do_not_come_back_after_a_save`.
- **SEC-37** `TrustSummary.results_steer_later_steps` is true when a Script Filter reaches, along the wires and at any depth, a Run Script,
  Run Script output, NSAppleScript, Script Filter, Open URL, Open File, Launch Files, Write File or Terminal Command object. It makes
  `is_passive()` false and the prompt says "Its results can change what later steps do". A Script Filter followed only by
  notifications/clipboard output is not flagged. Still static: what the items actually contain is unknown until run time.
- **Hard links (SEC-17 follow-up)** `confine::confined_workflow_file` is `confined_file` plus "link count must be 1": a regular file in a
  workflow folder with more than one hard link is treated as missing (icon falls back to the glyph). It is used for workflow and Script
  Filter icons (`workflow/filter.rs`). *Decision:* a hard link cannot be told apart from the original by path, and no zip can carry
  one into the folder, so the rule costs nothing legitimate; the price is that a file hard-linked *inside* the folder is refused too.
  System and app-bundle roots (`apps.rs::icns_path`) keep `confined_file`: they are not writable by the user and installers may
  hard-link there. Residual: the check is on the file's state at lookup time (a link made between the check and the read is a
  race for the image loader; creating one needs local access to the same volume).
- **Hotkey registry off the window thread** `hotkeys::build_snapshot(roots)` is the only reader of the folders; `WorkflowHotkeys::
  refresh_in_background` runs it on a thread, publishes an `Arc<Snapshot>` through a small cell (older scans never replace newer ones)
  and sends `AppEvent::HotkeysRefreshed`; the window thread then calls `apply`, which only plans and registers from the snapshot.
  `Automation::reload_hotkeys` no longer touches the disk. Test: `the_lookup_path_reads_only_the_snapshot_not_the_folders`,
  `a_slow_older_refresh_never_replaces_a_newer_snapshot`, `the_background_refresh_publishes_then_tells_the_host`.
- **SEC-39** `write_workflow_value` refuses a variable that is not on the workflow's configuration sheet.
- **SEC-38** `ashdash import` accepts `--yes` and `--`; any other option is an error with the usage line and exit code 2; exactly
  one file. `workflow::WORKFLOW_PACKAGE_EXTENSIONS` / `is_workflow_package` are the one list used by the launcher, Finder opens, the
  preferences drop zone and the preferences file chooser (the chooser filter now offers all four).


## 9e. Fix log (wave 4 review fixes, W4-R1, branch `fix/w4-r1`): items 1, 2, 4, 9, 10, 14 of `docs/reviews/wave4.md`

The ids `SEC-R4-nn` are the issue numbers of the review (so they cannot collide with ids the parallel W4-R2 branch picks).

- **SEC-R4-01 (review 1, MUST)** `TrustSummary.launched_apps` lists the application field of Open File (`openwith`), Open URL and "Search the web"
  (`browser`) objects as `{app, shipped}`; `shipped` is true for a literal path that points into the workflow's own folder (a relative path, or an
  absolute one under the workflow directory). `opens_shipped_files` lists Open File / Launch Files targets that are literal relative paths. Both make
  `is_passive()` false; the prompt adds "Launches an application: payload.app (shipped with the workflow)" and "Opens files shipped with the workflow: ...".
  A bare app name (`Safari`) is listed but not called shipped; an application field counts as a path when it has a `/` or ends in `.app`.
  *Decision on the executor:* a relative literal is not left to `open` (which would use the launcher's working directory): the executor resolves a relative
  `sourcefile`, Launch Files path, `openwith` path or `browser` path against the workflow's own folder (after `~` expansion; lexically normalised), so the
  classifier and the executor agree on what a relative path means (the reference launcher runs workflows with the workflow folder as the working directory). An app *name*
  is passed on as written. Residual: a name that happens to be an app shipped elsewhere on the machine, and what a Script Filter returns, stay invisible (SEC-37 discloses the latter).
- **SEC-R4-02 (review 2)** `exec.rs::is_info_plist_of` also canonicalises the *final path* (so `notes.txt -> info.plist` is refused with the usual warning), and
  `WriteFile` carries `protect: Option<PathBuf>` (the workflow's `info.plist`): `perform` opens the target without truncating, compares the opened file's
  device and inode with the protected file and refuses in every mode (overwrite, append, prepend) before writing a byte. That also covers hard links to
  `info.plist` and a link made between the executor's check and the write. *Decision:* `perform` still follows a final symlink in general (a dotfile that
  is a link into a dotfiles repository keeps working; test `write_file_still_writes_through_a_link_to_an_ordinary_file`), because a no-follow open would break
  that reference-compatible use; only the workflow's own `info.plist` is protected. SEC-36's "by real location" wording is now true; a script that is already part of the package can
  still be rewritten (disclosed in the prompt as "Can modify its own files").
- **SEC-R4-04 (review 4)** File Filter's `mdfind` goes through `Os::output_bounded` with the file-search budget from `config.file_search` (time limit
  `mdfind_timeout_ms`, output limit `mdfind_max_bytes`), additionally capped at 200 hits x 1 KiB (about 200 KB), so a types-only predicate (an empty argument) is
  killed after a few hundred kilobytes instead of printing for seconds. *Wording corrected (W4-R3, SEC-R5-06):* it is **bounded**, not cheap - see below. At most 200 paths are built, straight from the bytes (non-UTF-8 names survive), and a path cut by the kill is
  dropped. `WorkflowProvider` takes the engine's `Os` (`registry_for` wires it); `Workflow::keyword_results` (no `Os` argument) keeps the real OS and the default budget for callers outside the engine.
  Residual: the search still runs on the single query worker, bounded by the budget (default 4 s) rather than gone.
- **SEC-R4-09 (review 9)** `load_all` skips folders whose name starts with `.` (so `.import-*` staging, copy and previous-version folders are never loaded as a second
  copy). Install order: the complete new tree is put beside the destination first (a rename on the same volume; on `EXDEV` the copy with modes and links, fsynced),
  and only then is the old version moved aside and the tree renamed into place (two renames, microseconds), so the old version stays visible and runnable for the whole
  copy. Leftover names carry the start time (`.import-copy-<secs>-...`, `.import-old-<secs>-<pid>-<n>-<attempt>-<installed folder name>`).
  `sweep_interrupted_installs` runs when a package is staged and on every `load_all` of a folder: a leftover older than an hour is removed, except that a previous
  version (`.import-old-*`) whose installed folder is *missing* (a crash between the two renames) is put back; staging folders older than a day are removed as before.
  Residual: a previous version left behind after a *successful* swap (its removal failed) is restored if the user later deletes the installed folder and an hour has passed; younger leftovers (a swap in another process) are never touched.
- **SEC-R4-10 (review 10)** `settings_view::set`: for `web_searches` (the whole list) every entry of the new value must pass the URL rule; for `web_searches.N.key` the entry N must. An old bad entry that the edit does not touch still blocks nothing.
- **SEC-R4-14 (review 14)** `tests/system_programs.rs` now scans `crates/core/src` **and** `crates/app/src` (it already did; a minimum file count is asserted) for every
  `Command::new(` whatever the line breaks, spacing or path prefix (`std::process::Command::new`), skipping whole-line comments and `actions.rs`. An absolute literal and `system_program(...)` pass;
  a bare tool name from the table fails; every other bare literal or expression must be listed in the test's `KNOWN_SPAWNS` with a reason, and a listed entry that
  matches nothing fails (stale). Stated limits: a text scan, not a parse; no data flow through variables, macros, benches, `tests/` or `build.rs`; it cannot tell test code from production code.
  *Decision on `system_program`:* a known tool is always its absolute path; if the file is missing the spawn fails with an explicit "starting ..." error and there is **no `PATH` fallback**;
  only a name outside the table is passed on unchanged (documented on the function; the table is checked to exist on macOS by `every_known_tool_has_one_absolute_path_and_never_falls_back_to_path`).
## 9f. Fix log (wave 4 review, W4-R2, branch `fix/w4-r2`)

- **SEC-50 plist depth** `theme::parse_plist` is the one way the launcher parses a property list from an untrusted file (themes: `Theme::load`/`from_bytes`;
  app `Info.plist`: `apps::icns_path`). It first runs the bytes through `plist`'s event-driven serde deserializer with a probe type that
  fails at nesting depth 64 (`MAX_PLIST_DEPTH`, the workflow parser's cap), so its own recursion is bounded; only then is the tree built.
  Works for XML, binary and ASCII input. Files are read through `theme::read_limited` (themes 2 MiB, `Info.plist` 4 MiB). Measured before the
  fix: a 1.9 MB nested file aborted the test binary ("has overflowed its stack"). Not covered: a binary plist nested >64 levels *and* large enough to
  matter was only tested at 70 levels (written by the library, which cannot write much deeper). *Correction (review `wave4b.md`, item 1):* an earlier version of this entry said the
  workflow importer's `plist::Value::from_file(info.plist)` ran "after the capped workflow parser"; that was wrong (the 64-level cap is in the Script Filter XML parser, `xml.rs`, which never sees
  `info.plist`), and a 2.9 KB package aborted the process at staging. Fixed in 9g (SEC-R5-01).
- **SEC-51 `ashdash://search`** see section 6, item 1. New event `AppEvent::Prefill`; `LauncherApp::show_with_prefill` shows the window, sets the field and a footer note, and does not call `request_query`.
- **SEC-52 consent dialog** `Prompter::confirm` returns `Answer::{Allow, Deny, Unanswered}`; `dialog_answer` maps the `osascript` result (empty button after "giving up after 60", a
  dismissed dialog, or a failed `osascript` are `Unanswered`, which is reported and never stored). `UrlDispatcher::begin_asking` allows one open question per bundle id; `TrustStore::{refresh, lookup, set}` re-read by (mtime, length).
- **SEC-53 snippet pastes** `ClipCommand::PasteText` calls `ClipboardHistory::note_own_paste(text)` before the pasteboard write (and `forget_own_paste` if the write fails); `Watcher::step` skips a text change whose hash matches a
  note (`SkipReason::OwnPaste`), once per note and only for 10 s (`OWN_PASTE_TTL`). Marker types still take precedence. *Decision:* matching by content rather than by change count, because the change count after `pbcopy` is not known to the core, and a
  marker type cannot be added by `pbcopy`. *Residual:* if the user copies the identical text within 10 s of a snippet paste, that one copy is not recorded; pasting an existing history entry (`Paste`) is not noted (it is already in the history). Not run against a real pasteboard (QA item: paste a snippet, check `ashdash query "clip ..."`).
- **SEC-54..56** multibyte-safe scheme check (`url.get(..prefix.len())`); `ConnectionSlot` drop guard in `ipc.rs`; AppleScript `search` capped at `MAX_SEARCH` (4096 bytes), `run trigger` argument at `MAX_ARG` (4096 bytes) and empty ids refused;
  Finder import notifications and `display_name` use `importprompt::clean` (format characters, blank-looking characters, one line, 240 / 80 characters); theme install reads the file once (`install_theme_with`), validates those bytes and writes the same bytes.
- **`ashdash --version` / `-V` / `version`** prints `ashdash <crate version>` (first word only), exit 0, before the data folder or settings are touched.

## 9g. Fix log (wave 4 round 3, W4-R3, branch `fix/w4-r3`): items 1-10 of `docs/reviews/wave4b.md`

The ids `SEC-R5-nn` are the item numbers of that review.

- **SEC-R5-01 (item 1, MUST)** Every reader of a workflow's `info.plist` goes through `workflow::read_info_plist(dir)` (`model.rs`): `theme::read_limited` (16 MiB,
  `MAX_INFO_PLIST_BYTES`) and `theme::parse_plist` (64 levels, see SEC-R5-02). Used by `Workflow::load` (so by `load_all`, the hotkey registry and the CLI), the importer
  (`stage_in` and its `text` closure) and `settings_view::set_workflow_enabled`; the app scan's `is_background_only` (`index/scan.rs`) uses the same bounded reader for
  an app's `Info.plist`. A package whose `info.plist` nests too deeply is refused at staging with "nested more than 64 levels" and nothing is left in the staging folder; a deep
  `info.plist` already in a workflows root is skipped by `load_all` with the existing "skipping workflow" warning on stderr. Tests: `a_package_with_a_deeply_nested_info_plist_is_refused_not_a_stack_overflow`
  (2.9 KB package, 120 000 levels, 2 MiB test thread; it aborted the process before) and `load_all_skips_a_deep_info_plist_and_keeps_the_rest` (package.rs).
  Not covered: `plist::Value::from_file`/`to_file_xml` are still used on trees the code itself built (`settings_view`, writing back what was just parsed); the `plist_to_json` conversion is recursive but only sees trees that passed the probe.
- **SEC-R5-02 (item 2)** `theme::parse_plist`'s probe also counts the values it visits (keys included) and fails at `MAX_PLIST_VALUES` = 500 000, because a binary plist can refer to one
  object from many places (147 bytes, 10 references per level, 8 levels = 10^8 values at depth 9, never returned). The counter is per call (thread-local, reset on entry). Test:
  `shared_references_cannot_make_the_parse_do_exponential_work` (the 147-byte plist is built in the test; returns an error in well under a second; a 5 s watchdog fails the test instead of hanging it),
  `a_plist_with_a_lot_of_ordinary_values_still_parses` (100 000 values, XML and binary), `real_system_plists_still_parse` (Calculator, Safari, Notes `Info.plist` when present); the reference launcher's bundled templates are loaded by
  `bundled_reference_templates_load_and_report_unsupported_objects` through the same reader. The bound is on values, not on time (the time of a file that stays under it was not measured beyond the 100 000-value test).
- **SEC-R5-03 (item 3)** `TrustStore` (`urlscheme.rs`): a `url-trust.json` that is there but does not parse (not UTF-8, not JSON, not an object; a half-saved hand edit) is no longer read as an
  empty file. `refresh` keeps the last good in-memory map (a remembered "deny" still denies), prints a warning to stderr once per change of the file, and `TrustStore::warning()` (test builds) returns it.
  `set` moves the damaged file aside to `url-trust.json.bad` (`.bad.1`, `.bad.2`... if a copy is already there, so an earlier one is never replaced) before writing the in-memory map plus the new entry.
  A file that does not exist is still an empty map (deleting it forgets everything, as documented). Tests: `a_half_saved_consent_file_keeps_the_remembered_decisions_and_is_not_overwritten`,
  `a_consent_file_that_is_damaged_at_start_is_kept_until_the_first_answer`. Residual: an edit that keeps both modification time and length is not noticed; the warning is on stderr only (no on-screen message).
- **SEC-R5-04 (item 4)** One rule for "is this a path" in the classifier (`trust.rs`) and the executor (`exec.rs`). A target that begins with `scheme:` before any `/` (`target_scheme`: letters, digits, `+ . -`, first
  character a letter; `https://x`, `mailto:a@b.c`, `tel:1`) is a URL: `open_target` returns it unchanged (also when it arrives as the incoming argument of an empty file field) and the summary lists its scheme
  under URL schemes instead of calling it a file shipped with the workflow. An application field is a path (`app_is_path(app, dir)`) when it contains a `/`, or ends in `.app` *and* that bundle exists in the
  workflow folder (`symlink_metadata`); otherwise (`Safari.app`, `Safari`, `com.google.Chrome`) it is a name passed to `open -a` as written. A relative path (`Documents/y`, `payload.app` when shipped) is
  still resolved against the workflow folder. This amends SEC-R4-01 (which called any `*.app` a path). The one disk look in the summary is that `symlink_metadata` on a bare `*.app` field.
  Tests: `urls_given_to_open_file_are_urls_not_files_shipped_with_the_workflow`, `a_bare_app_name_ending_in_dot_app_is_a_name_unless_the_bundle_is_in_the_package` (trust.rs),
  `urls_and_app_names_are_passed_to_open_unchanged` (exec.rs). Not run: a real `open -a` with these forms. Residual: a literal that contains a colon before its first slash (`notes:2024/x`) is taken for a URL.
- **SEC-R5-05 (item 5)** `note_app` expands `~` and normalises `..` lexically before the "is it under the workflow folder" test, so `~/Library/Application Support/ashdash/workflows/<own folder>/payload.app` is
  called shipped, and `<own folder>/../<other>/payload.app` is not. Test: `a_tilde_path_into_the_workflows_own_folder_is_shipped`.
- **SEC-R5-06 (item 6)** Wording only. The File Filter `mdfind` is *bounded by the budget* (4 s by default, `mdfind_timeout_ms`, and 200 hits / about 200 KB of output), not "cheap": `mdfind` spends about 2.3 s
  starting up before it prints, so a types-only predicate (empty argument) still holds the single query worker for that long on a cold run (one run on one machine, `docs/reviews/wave4b.md`; not re-measured here), and
  a slow or hung `mdfind` for up to the timeout. The output cap shortens neither. No code change; lowering `mdfind_timeout_ms` shortens the worst case at the cost of missed results on a slow index.
- **SEC-R5-07 (item 7)** Removing a workflow (`settings_view::remove_workflow`, the preferences window's "Remove...") also removes the `.import-old-*` previous-version leftovers that name that folder
  (`package::remove_swap_leftovers_of`; exact folder-name match, empty or path-like names remove nothing), so the sweep can no longer put the old copy back once the installed folder is gone.
  `.import-copy-*` leftovers name no workflow and are only ever deleted by the sweep, never restored, so they are not touched. This closes the residual in 9e for removals made through the app; a folder
  deleted in Finder still leaves a leftover (hidden `.import-old-*`) that is restored an hour later - delete it with the workflow. Tests: `removing_a_workflow_removes_its_previous_version_leftovers_and_only_those`
  (package.rs), `removing_a_workflow_does_not_let_the_sweep_bring_back_its_previous_version` (tests/prefs_core.rs).
- **SEC-R5-08 (item 10)** `load_all` no longer sweeps. `workflow::sweep_interrupted_installs(own_root)` is called by the engine's content reload for the app's own workflows folder only (first root, built by
  `Engine::reload_job`) and by `stage_package` (as before); roots from `workflow_dirs` (a synced reference launcher folder, a read-only share) are only read. Amends 9e, which said the sweep ran on every `load_all` of a folder.
  Test: `only_the_apps_own_workflows_folder_is_swept_for_interrupted_installs` (tests/prefs_core.rs: a writable and a read-only extra root keep their stale leftovers, the own folder's lost workflow is put back).
- **SEC-R5-09 (item 8)** `Theme::load` (the `theme` setting, `<data>/themes/`, the daemon applying a theme, `ashdash://theme?name=`) reads up to `MAX_INSTALLED_THEME_BYTES` = 8 MiB; `Theme::load_for_install`
  (`ashdash theme <file>` on the command line) and the Finder/`import_theme` paths (`read_limited(.., MAX_THEME_BYTES)`) keep 2 MiB. The probe (SEC-50, SEC-R5-02) bounds nesting and the number of values, so the larger allowance
  adds memory (at most 8 MiB read, then the parsed tree) but no new way to exhaust the stack. Test: `an_installed_theme_may_be_bigger_than_one_offered_for_installation`. Upgrade note: `CHANGELOG.md`, `docs/release-notes/v0.1.0.md`.
- **SEC-R5-10 (item 9)** Copy to Clipboard in a workflow. The reference launcher's own templates carry a `transient` key on `alfred.workflow.output.clipboard` (checked in the installed reference launcher bundles: `Paste as plain text from hotkey`
  and `Paste text clip from hotkey` have `transient: true`, `Clipboard` and `Snippet Triggers` have `false`). It is now honoured: `transient: true` gives `Effect::CopyTransient`, applied by
  `engine::copy_to_clipboard_transient`, which writes the text as `public.utf8-plain-text` together with `org.nspasteboard.TransientType`, so the clipboard watcher (and any other clipboard manager) skips it
  (`concealed_and_transient_entries_are_never_recorded`). Without the key, or with `false`, the copy is a plain `pbcopy` and **is recorded in clipboard history like a copy made by the user**, as the reference launcher does;
  a workflow that copies a secret should tick "transient". The text reaches the JXA as an argument after `--`, never in the script source. `A::CopyText` (the "Copy" universal action on a row: a path, a URL,
  a calculator result) is a user action and stays a plain `pbcopy`, recorded like any user copy. Tests: `copy_to_clipboard_honours_the_transient_box_and_only_that` (exec.rs),
  `a_transient_copy_carries_the_marker_and_the_text_only_as_an_argument` (engine.rs), `A::CopyText` -> `pbcopy` with stdin in the `actions.rs` command table. Checked live once by hand: the pasteboard held
  the text and the `org.nspasteboard.TransientType` type, and the text with quotes and a leading `-e` arrived unchanged (the previous clipboard text was restored); not run in the test suite, which must not touch the real pasteboard.
