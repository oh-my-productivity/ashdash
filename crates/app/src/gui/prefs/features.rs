//! Features page: what Ashdash searches and how.

use ashdash_core::format_ids::SNIPPETS_EXT;
use std::path::Path;

use ashdash_core::bookmarks::ALL_SOURCE_IDS;
use ashdash_core::settings_view::{SnippetLibrary, SnippetLoc};
use ashdash_core::{Config, Paths};
use eframe::egui::{self, RichText};

use super::app::{PrefsApp, Purpose};
use super::kit;
use super::nav::FeaturesPart;
use super::style::{self, Palette};
use super::widgets::{Msg, RowAction, confirm_button, hint, is_error, section, show_msg, string_list, warn_text};
use crate::prefs_model::{self as model, ClipboardForm, FileList, SnippetDraft, WebSearchDraft};

#[derive(Default)]
pub struct State {
    providers_msg: Option<Msg>,
    // file search
    file_inputs: [String; 4],
    file_msgs: [Option<Msg>; 4],
    files_msg: Option<Msg>,
    // bookmarks
    bookmarks_msg: Option<Msg>,
    // clipboard
    clip_form: Option<ClipboardForm>,
    clip_msg: Option<Msg>,
    ignored_input: String,
    // web searches
    ws_edit: Option<(Option<usize>, WebSearchDraft)>,
    ws_msg: Option<Msg>,
    confirm_reset_ws: bool,
    // snippets
    library: Option<SnippetLibrary>,
    sn_edit: Option<(Option<SnippetLoc>, SnippetDraft)>,
    sn_msg: Option<Msg>,
    sn_input: String,
    new_collection: String,
    confirm_delete: Option<SnippetLoc>,
}

impl State {
    pub fn refresh(&mut self, paths: &Paths, cfg: &Config) {
        self.library = Some(SnippetLibrary::load(&paths.snippets_file()));
        self.clip_form = Some(ClipboardForm::from_settings(&cfg.clipboard));
    }

    /// Snippet and collection names, keywords and the text being edited (for the font fallback).
    pub fn user_texts(&self) -> impl Iterator<Item = &str> {
        let library = self.library.iter().flat_map(|l| {
            l.entries()
                .into_iter()
                .flat_map(|(_, s)| [s.name.as_str(), s.keyword.as_str(), s.collection.as_str()])
        });
        library.chain(
            self.sn_edit
                .iter()
                .flat_map(|(_, d)| [d.name.as_str(), d.keyword.as_str(), d.text.as_str()]),
        )
    }

    /// `config.json` changed on disk: forms that mirror it start over.
    pub fn config_changed(&mut self) {
        self.clip_form = None;
        self.ws_edit = None;
    }
}

impl PrefsApp {
    pub(super) fn features_page(&mut self, ui: &mut egui::Ui, part: FeaturesPart) {
        if self.features.clip_form.is_none() {
            self.features.clip_form = Some(ClipboardForm::from_settings(&self.cfg.clipboard));
        }
        match part {
            FeaturesPart::Search => {
                self.providers_section(ui);
                self.file_search_section(ui);
                self.bookmarks_section(ui);
                self.web_searches_section(ui);
            }
            FeaturesPart::Clipboard => self.clipboard_section(ui),
            FeaturesPart::Snippets => self.snippets_section(ui),
        }
    }

    // ---------------------------------------------------------------- providers

    fn providers_section(&mut self, ui: &mut egui::Ui) {
        kit::section_title(ui, "Features", Some("providers_disabled"));
        kit::rows(ui, |ui| {
            for (id, label, what) in model::PROVIDER_INFO {
                let mut on = model::provider_enabled(&self.cfg, id);
                kit::row(ui, label, None, false, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if kit::toggle(ui, &mut on, label).changed() {
                            self.features.providers_msg = self
                                .edit(|c| model::set_provider_enabled(c, id, on))
                                .err()
                                .map(Msg::err);
                        }
                        hint(ui, what);
                    });
                });
            }
        });
        show_msg(ui, &self.features.providers_msg);
    }

    // ---------------------------------------------------------------- file search

    fn file_search_section(&mut self, ui: &mut egui::Ui) {
        kit::section_title(ui, "File search (open, find, in, ')", Some("file_search"));
        kit::card(ui, |ui| {
            for (n, list) in FileList::ALL.into_iter().enumerate() {
                if n > 0 {
                    ui.add_space(10.0);
                }
                ui.label(style::caps(list.title(), Palette::of(ui).text));
                let items = list.get(&self.cfg).to_vec();
                let mut input = std::mem::take(&mut self.features.file_inputs[n]);
                let (action, added) = string_list(ui, &format!("fl{n}"), &items, &mut input, list.hint());
                self.features.file_inputs[n] = input;
                match action {
                    RowAction::Remove(i) => {
                        self.features.file_msgs[n] =
                            self.edit(|c| model::remove_list_entry(c, list, i)).err().map(Msg::err);
                    }
                    RowAction::None => {}
                }
                if let Some(text) = added {
                    match self.edit(|c| model::add_list_entry(c, list, &text)) {
                        Ok(()) => {
                            self.features.file_inputs[n].clear();
                            self.features.file_msgs[n] = None;
                        }
                        Err(e) => self.features.file_msgs[n] = Some(Msg::err(e)),
                    }
                }
                show_msg(ui, &self.features.file_msgs[n]);
            }
        });
        ui.add_space(style::m::GAP);
        let err = is_error(&self.features.files_msg);
        kit::rows(ui, |ui| {
            let mut hidden = self.cfg.file_search.include_hidden;
            kit::row(
                ui,
                "Include hidden files",
                Some("file_search.include_hidden"),
                err,
                |ui| {
                    if kit::toggle(ui, &mut hidden, "Include hidden files").changed() {
                        self.features.files_msg =
                            self.edit(|c| model::set_include_hidden(c, hidden)).err().map(Msg::err);
                    }
                },
            );
            let mut icloud = self.cfg.file_search.include_icloud_drive;
            kit::row(
                ui,
                "iCloud Drive",
                Some("file_search.include_icloud_drive"),
                err,
                |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if kit::toggle(ui, &mut icloud, "Keep iCloud Drive searchable").changed() {
                            self.features.files_msg =
                                self.edit(|c| model::set_include_icloud(c, icloud)).err().map(Msg::err);
                        }
                        hint(ui, "Keep iCloud Drive searchable although ~/Library is excluded");
                    });
                },
            );
        });
        show_msg(ui, &self.features.files_msg);
    }

    // ---------------------------------------------------------------- bookmarks

    fn bookmarks_section(&mut self, ui: &mut egui::Ui) {
        kit::section_title(ui, "Bookmarks (bm)", Some("bookmarks.enabled_sources"));
        kit::rows(ui, |ui| {
            for id in ALL_SOURCE_IDS {
                let mut on = model::bookmark_source_enabled(&self.cfg, id);
                let name = model::bookmark_source_label(id);
                kit::row(ui, &name, None, false, |ui| {
                    if kit::toggle(ui, &mut on, &name).changed() {
                        self.features.bookmarks_msg =
                            self.edit(|c| model::set_bookmark_source(c, id, on)).err().map(Msg::err);
                    }
                });
            }
            let mut general = self.cfg.bookmarks.include_in_general_search;
            kit::row(
                ui,
                "In ordinary searches",
                Some("bookmarks.include_in_general_search"),
                false,
                |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if kit::toggle(ui, &mut general, "Also show bookmarks in ordinary searches").changed() {
                            self.features.bookmarks_msg = self
                                .edit(|c| model::set_bookmarks_in_general_search(c, general))
                                .err()
                                .map(Msg::err);
                        }
                        hint(ui, "Also show bookmarks in ordinary searches");
                    });
                },
            );
        });
        ui.add_space(6.0);
        hint(
            ui,
            "Safari bookmarks need Full Disk Access for Ashdash in System Settings > Privacy & Security.",
        );
        show_msg(ui, &self.features.bookmarks_msg);
    }

    // ---------------------------------------------------------------- clipboard

    fn clipboard_section(&mut self, ui: &mut egui::Ui) {
        let mut form = self
            .features
            .clip_form
            .clone()
            .unwrap_or_else(|| ClipboardForm::from_settings(&self.cfg.clipboard));
        let mut apply = false;
        let err = is_error(&self.features.clip_msg);
        kit::section_title(ui, "Clipboard history (clip)", Some("clipboard"));
        kit::rows(ui, |ui| {
            let p = Palette::of(ui);
            kit::row(ui, "Keep at most", Some("clipboard.max_entries"), err, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut form.max_entries)
                            .font(style::mono_font(style::m::BODY))
                            .desired_width(70.0),
                    );
                    ui.label(style::hint_text(&p, "entries"));
                    apply |= r.lost_focus();
                });
            });
            kit::row(ui, "Forget after", Some("clipboard.max_age_days"), err, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut form.max_age_days)
                            .font(style::mono_font(style::m::BODY))
                            .hint_text("never")
                            .desired_width(70.0),
                    );
                    ui.label(style::hint_text(&p, "days"));
                    apply |= r.lost_focus();
                });
            });
            kit::row(ui, "Largest image", Some("clipboard.max_image_bytes"), err, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut form.max_image_mb)
                            .font(style::mono_font(style::m::BODY))
                            .desired_width(70.0),
                    );
                    ui.label(style::hint_text(&p, "MB"));
                    apply |= r.lost_focus();
                });
            });
        });
        if apply && form != ClipboardForm::from_settings(&self.cfg.clipboard) {
            self.features.clip_msg = Some(match self.edit(|c| model::apply_clipboard_form(c, &form)) {
                Ok(()) => Msg::ok("Saved."),
                Err(e) => Msg::err(e),
            });
        }
        self.features.clip_form = Some(form);
        show_msg(ui, &self.features.clip_msg);

        kit::section_title(
            ui,
            "Never record copies from these apps",
            Some("clipboard.ignored_apps"),
        );
        kit::card(ui, |ui| {
            let items = self.cfg.clipboard.ignored_apps.clone();
            let mut input = std::mem::take(&mut self.features.ignored_input);
            let (action, added) = string_list(ui, "ignored", &items, &mut input, "com.example.passwordmanager");
            self.features.ignored_input = input;
            if let RowAction::Remove(i) = action {
                self.features.clip_msg = self.edit(|c| model::remove_ignored_app(c, i)).err().map(Msg::err);
            }
            if let Some(text) = added {
                match self.edit(|c| model::add_ignored_app(c, &text)) {
                    Ok(()) => {
                        self.features.ignored_input.clear();
                        self.features.clip_msg = None;
                    }
                    Err(e) => self.features.clip_msg = Some(Msg::err(e)),
                }
            }
            hint(
                ui,
                "Copies marked concealed or transient by their app (password managers) are never recorded.",
            );
        });
    }

    // ---------------------------------------------------------------- web searches

    fn web_searches_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Web searches", |ui| {
            let searches = self.cfg.web_searches.clone();
            let mut remove = None;
            let mut edit = None;
            egui::Grid::new("ws-grid")
                .num_columns(4)
                .spacing([10.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    for (i, w) in searches.iter().enumerate() {
                        ui.label(RichText::new(&w.keyword).monospace());
                        ui.label(&w.title);
                        ui.label(RichText::new(if w.fallback { "fallback" } else { "" }).weak().small());
                        ui.horizontal(|ui| {
                            if ui.small_button("Edit").clicked() {
                                edit = Some(i);
                            }
                            if ui.small_button("×").on_hover_text("Remove").clicked() {
                                remove = Some(i);
                            }
                        });
                        ui.end_row();
                    }
                });
            if let Some(i) = remove {
                self.features.ws_msg = self.edit(|c| model::remove_web_search(c, i)).err().map(Msg::err);
            }
            if let Some(i) = edit {
                self.features.ws_edit = Some((Some(i), WebSearchDraft::from_search(&searches[i])));
                self.features.ws_msg = None;
            }
            if self.features.ws_edit.is_none() && ui.button("Add web search…").clicked() {
                self.features.ws_edit = Some((None, WebSearchDraft::default()));
                self.features.ws_msg = None;
            }
            if let Some((idx, mut draft)) = self.features.ws_edit.take() {
                let (mut save, mut cancel) = (false, false);
                ui.add_space(6.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    egui::Grid::new("ws-form")
                        .num_columns(2)
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Keyword");
                            ui.add(egui::TextEdit::singleline(&mut draft.keyword).desired_width(140.0));
                            ui.end_row();
                            ui.label("Name");
                            ui.add(egui::TextEdit::singleline(&mut draft.title).desired_width(220.0));
                            ui.end_row();
                            ui.label("URL");
                            ui.add(
                                egui::TextEdit::singleline(&mut draft.url)
                                    .hint_text("https://example.com/search?q={query}")
                                    .desired_width(330.0),
                            );
                            ui.end_row();
                        });
                    ui.checkbox(&mut draft.fallback, "Offer as a fallback when nothing else matches");
                    hint(ui, "{query} is replaced by what you type after the keyword.");
                    ui.horizontal(|ui| {
                        save = ui.button("Save").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
                if cancel {
                    self.features.ws_msg = None;
                } else if save {
                    match self.edit(|c| model::save_web_search(c, idx, &draft)) {
                        Ok(()) => self.features.ws_msg = Some(Msg::ok("Saved.")),
                        Err(e) => {
                            self.features.ws_msg = Some(Msg::err(e));
                            self.features.ws_edit = Some((idx, draft));
                        }
                    }
                } else {
                    self.features.ws_edit = Some((idx, draft));
                }
            }
            show_msg(ui, &self.features.ws_msg);
            if confirm_button(
                ui,
                &mut self.features.confirm_reset_ws,
                "Restore the default searches…",
                "Yes, replace my list with the defaults",
            ) {
                self.features.ws_msg = Some(match self.edit(model::reset_web_searches) {
                    Ok(()) => Msg::ok("Restored."),
                    Err(e) => Msg::err(e),
                });
            }
        });
    }

    // ---------------------------------------------------------------- snippets

    fn snippets_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Snippets (snip)", |ui| {
            if self.features.library.is_none() {
                self.features.library = Some(SnippetLibrary::load(&self.paths.snippets_file()));
            }
            let entries: Vec<(SnippetLoc, ashdash_core::clipboard::Snippet)> = self
                .features
                .library
                .as_ref()
                .map(|l| l.entries().into_iter().map(|(loc, s)| (loc, s.clone())).collect())
                .unwrap_or_default();
            let collections = self
                .features
                .library
                .as_ref()
                .map(|l| l.collection_names())
                .unwrap_or_default();

            if entries.is_empty() {
                hint(ui, "No snippets yet.");
            }
            let mut edit = None;
            let mut delete = None;
            let mut last_collection: Option<&str> = None;
            egui::Grid::new("sn-grid")
                .num_columns(4)
                .spacing([10.0, 4.0])
                .striped(true)
                .show(ui, |ui| {
                    for (loc, s) in &entries {
                        let group = s.collection.as_str();
                        if last_collection != Some(group) {
                            last_collection = Some(group);
                            ui.label(RichText::new(if group.is_empty() { "No collection" } else { group }).strong());
                            ui.end_row();
                        }
                        ui.label(&s.name);
                        ui.label(RichText::new(s.resolved_keyword()).monospace());
                        ui.label(RichText::new(one_line(&s.text, 48)).weak().small());
                        ui.horizontal(|ui| {
                            if ui.small_button("Edit").clicked() {
                                edit = Some(*loc);
                            }
                            if ui.small_button("×").on_hover_text("Delete").clicked() {
                                delete = Some(*loc);
                            }
                        });
                        ui.end_row();
                    }
                });
            if let Some(loc) = edit
                && let Some((_, s)) = entries.iter().find(|(l, _)| *l == loc)
            {
                self.features.sn_edit = Some((Some(loc), SnippetDraft::from_snippet(s)));
                self.features.sn_msg = None;
            }
            if let Some(loc) = delete {
                self.features.confirm_delete = Some(loc);
            }
            if let Some(loc) = self.features.confirm_delete {
                let name = entries
                    .iter()
                    .find(|(l, _)| *l == loc)
                    .map(|(_, s)| s.name.clone())
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label(warn_text(ui, &format!("Delete '{name}'?")));
                    if ui.button("Delete").clicked() {
                        self.features.confirm_delete = None;
                        self.mutate_library(|lib| lib.remove(loc));
                    }
                    if ui.button("Cancel").clicked() {
                        self.features.confirm_delete = None;
                    }
                });
            }

            if self.features.sn_edit.is_none() {
                ui.horizontal(|ui| {
                    if ui.button("New snippet…").clicked() {
                        self.features.sn_edit = Some((None, SnippetDraft::default()));
                        self.features.sn_msg = None;
                    }
                    if ui.button("Import snippets…").clicked() {
                        self.choose_file(Purpose::Snippets, "Choose a snippet collection", &[SNIPPETS_EXT]);
                    }
                });
            }
            if let Some((loc, mut draft)) = self.features.sn_edit.take() {
                let (mut save, mut cancel) = (false, false);
                ui.add_space(6.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    egui::Grid::new("sn-form")
                        .num_columns(2)
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Name");
                            ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(220.0));
                            ui.end_row();
                            ui.label("Keyword");
                            ui.add(
                                egui::TextEdit::singleline(&mut draft.keyword)
                                    .hint_text("optional")
                                    .desired_width(160.0),
                            );
                            ui.end_row();
                            ui.label("Collection");
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("sn-collection")
                                    .selected_text(if draft.collection.is_empty() {
                                        "No collection"
                                    } else {
                                        &draft.collection
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut draft.collection, String::new(), "No collection");
                                        for c in &collections {
                                            ui.selectable_value(&mut draft.collection, c.clone(), c);
                                        }
                                    });
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.features.new_collection)
                                        .hint_text("or a new collection name")
                                        .desired_width(130.0),
                                );
                            });
                            ui.end_row();
                        });
                    ui.label("Text");
                    ui.add(
                        egui::TextEdit::multiline(&mut draft.text)
                            .desired_rows(6)
                            .desired_width(f32::INFINITY),
                    );
                    hint(
                        ui,
                        "Placeholders: {clipboard} {date} {time} {datetime} {cursor} {random:uuid}",
                    );
                    ui.horizontal(|ui| {
                        save = ui.button("Save").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
                if cancel {
                    self.features.sn_msg = None;
                    self.features.new_collection.clear();
                } else if save {
                    let typed = self.features.new_collection.trim().to_string();
                    if !typed.is_empty() {
                        draft.collection = typed;
                    }
                    match self.save_snippet(loc, &draft, &entries) {
                        Ok(()) => {
                            self.features.sn_msg = Some(Msg::ok("Saved."));
                            self.features.new_collection.clear();
                        }
                        Err(e) => {
                            self.features.sn_msg = Some(Msg::err(e));
                            self.features.sn_edit = Some((loc, draft));
                        }
                    }
                } else {
                    self.features.sn_edit = Some((loc, draft));
                }
            }
            show_msg(ui, &self.features.sn_msg);
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.features.sn_input)
                        .hint_text("path of a snippet collection file")
                        .desired_width(220.0),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Import").clicked() || enter) && !self.features.sn_input.trim().is_empty() {
                    let path = Path::new(self.features.sn_input.trim()).to_path_buf();
                    self.import_snippets(&path);
                }
            });
        });
    }

    fn save_snippet(
        &mut self,
        loc: Option<SnippetLoc>,
        draft: &SnippetDraft,
        entries: &[(SnippetLoc, ashdash_core::clipboard::Snippet)],
    ) -> Result<(), String> {
        let snippet = model::validate_snippet(draft, entries, loc)?;
        let file = self.paths.snippets_file();
        let lib = self.features.library.as_mut().ok_or("snippets are not loaded")?;
        match loc {
            Some(l) => lib.replace(l, snippet),
            None => lib.add(snippet),
        }
        let saved = lib.save(&file).map_err(|e| format!("{e:#}"));
        self.features.library = Some(SnippetLibrary::load(&file));
        saved
    }

    fn mutate_library(&mut self, f: impl FnOnce(&mut SnippetLibrary)) {
        let file = self.paths.snippets_file();
        let Some(lib) = self.features.library.as_mut() else {
            return;
        };
        f(lib);
        self.features.sn_msg = Some(match lib.save(&file) {
            Ok(()) => Msg::ok("Saved."),
            Err(e) => Msg::err(format!("{e:#}")),
        });
        self.features.library = Some(SnippetLibrary::load(&file));
    }

    /// Install a snippet collection package as a collection.
    pub(super) fn import_snippets(&mut self, file: &Path) {
        let dir = ashdash_core::clipboard::snippets::collections_dir(&self.paths.snippets_file());
        self.features.sn_msg = Some(match ashdash_core::clipboard::import_snippet_package(file, &dir) {
            Ok(r) => {
                self.features.sn_input.clear();
                self.features.library = Some(SnippetLibrary::load(&self.paths.snippets_file()));
                Msg::ok(format!(
                    "Collection \"{}\": {} imported{}.",
                    r.collection,
                    r.imported,
                    if r.skipped > 0 {
                        format!(", {} skipped", r.skipped)
                    } else {
                        String::new()
                    }
                ))
            }
            Err(e) => Msg::err(format!("Could not import the snippets: {e:#}")),
        });
    }
}

/// First line of `s`, cut to `max` characters.
fn one_line(s: &str, max: usize) -> String {
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() > max || s.lines().count() > 1 {
        let cut: String = first.chars().take(max).collect();
        format!("{cut}…")
    } else {
        first.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_shortens_long_and_multiline_text() {
        assert_eq!(one_line("short", 10), "short");
        assert_eq!(one_line("a\nb", 10), "a…");
        assert_eq!(one_line("abcdefghij", 4), "abcd…");
        assert_eq!(one_line("", 4), "");
    }
}
