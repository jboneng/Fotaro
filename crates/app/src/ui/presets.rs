//! The Develop module's Presets and Snapshots panels, and their dialogs: New Preset,
//! Copy Settings (the groups Paste sets), Rename.

use std::collections::BTreeSet;

use eframe::egui::{self, RichText};
use imaging::develop::{SettingsGroup, changed_groups};

use super::develop::weak;
use super::theme;
use crate::app::{Command, FotaroApp};
use crate::develop::SnapshotEdit;
use crate::presets::{BUILT_IN, PresetFile, USER};

/// What the panels' dialogs are showing.
#[derive(Default)]
pub struct PresetsUi {
    pub filter: String,
    /// Folders folded shut.
    pub closed: BTreeSet<String>,
    pub new_preset: Option<NewPreset>,
    /// Copy Settings: the groups ticked.
    pub copy: Option<Vec<SettingsGroup>>,
    pub rename: Option<(RenameTarget, String)>,
    /// This frame: the settings of the preset or snapshot under the pointer (the
    /// Navigator shows them).
    pub hover: Option<imaging::develop::DevelopSettings>,
}

pub struct NewPreset {
    pub name: String,
    pub folder: String,
    /// A new folder's name, when "New Folder" is chosen.
    pub new_folder: Option<String>,
    pub groups: Vec<SettingsGroup>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenameTarget {
    Preset(usize),
    Snapshot(i64),
}

/// Something a click in the panels asks for, done after drawing them.
enum Ask {
    Apply(usize),
    Update(usize),
    Delete(usize),
    Reveal(usize),
    NewPreset,
    Import(bool),
    ImportLightroom,
    TakeSnapshot,
    ApplySnapshot(i64),
    EditSnapshot(i64, SnapshotEdit),
}

/// LrC's Presets panel: folders of presets; a click applies one.
pub fn presets_panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let has_photo = app.session.as_ref().is_some_and(|s| s.develop.photo.is_some());
    let mut ask = None;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.presets_ui.filter)
                .hint_text("Filter presets")
                .desired_width(ui.available_width() - 30.0),
        );
        ui.menu_button("+", |ui| {
            if ui.add_enabled(has_photo, egui::Button::new("Create Preset…")).clicked() {
                // key: develop.presets.create
                ask = Some(Ask::NewPreset);
            }
            ui.separator();
            if ui
                .button("Import Presets…") // key: develop.presets.import
                .on_hover_text("Lightroom presets: .xmp and .lrtemplate files")
                .clicked()
            {
                ask = Some(Ask::Import(false));
            }
            if ui.button("Import Folder of Presets…").clicked() {
                // key: develop.presets.import_folder
                ask = Some(Ask::Import(true));
            }
            if crate::presets::lightroom_folder().is_some()
                && ui
                    .button("Import from Lightroom") // key: develop.presets.import_lightroom
                    .on_hover_text("Every develop preset Lightroom has on this computer")
                    .clicked()
            {
                ask = Some(Ask::ImportLightroom);
            }
        });
    });
    ui.add_space(2.0);
    let filter = app.presets_ui.filter.to_lowercase();
    for folder in app.presets.folders() {
        let items: Vec<usize> = app
            .presets
            .presets
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                p.folder == folder && (filter.is_empty() || p.name().to_lowercase().contains(&filter))
            })
            .map(|(i, _)| i)
            .collect();
        if items.is_empty() && (!filter.is_empty() || folder != USER) {
            continue;
        }
        let open = !app.presets_ui.closed.contains(&folder) || !filter.is_empty();
        let arrow = if open { "▾" } else { "▸" };
        let head = super::sidebars::source_row(
            ui,
            2.0,
            &format!("{arrow} {folder}"),
            Some(items.len() as i64),
            false,
        );
        if head.clicked() {
            if open {
                app.presets_ui.closed.insert(folder.clone());
            } else {
                app.presets_ui.closed.remove(&folder);
            }
        }
        if !open {
            continue;
        }
        if items.is_empty() {
            ui.label(weak("    Create Preset… (+) saves one here").size(11.5));
        }
        for i in items {
            let p = &app.presets.presets[i];
            let mut resp = super::sidebars::source_row(ui, 20.0, p.name(), None, false);
            let own = p.path.is_some();
            let mut tip = groups_text(&p.file.groups);
            if p.file.lightroom.is_some() {
                tip = format!("From Lightroom. {tip}");
                if !p.file.unsupported.is_empty() {
                    tip += &format!("\nNot carried over: {}", p.file.unsupported.join(", "));
                }
            }
            resp = resp.on_hover_text(tip);
            // The Navigator shows the photo with it (not while a menu or dialog is open).
            if resp.hovered()
                && let Some(s) = &app.session
                && s.develop.photo.is_some()
            {
                let d = &s.develop;
                app.presets_ui.hover = Some(p.file.apply(&d.settings, d.kelvin()));
            }
            if resp.clicked() && has_photo {
                ask = Some(Ask::Apply(i));
            }
            resp.context_menu(|ui| {
                if ui
                    .add_enabled(own && has_photo, egui::Button::new("Update with Current Settings")) // no hotkey: a preset's own menu
                    .clicked()
                {
                    ask = Some(Ask::Update(i));
                    ui.close();
                }
                if ui.add_enabled(own, egui::Button::new("Rename…")).clicked() {
                    // no hotkey: a preset's own menu
                    app.presets_ui.rename =
                        Some((RenameTarget::Preset(i), app.presets.presets[i].name().to_string()));
                    ui.close();
                }
                if ui.add_enabled(own, egui::Button::new("Show in Explorer")).clicked() {
                    // no hotkey: a preset's own menu
                    ask = Some(Ask::Reveal(i));
                    ui.close();
                }
                ui.separator();
                if ui.add_enabled(own, egui::Button::new("Delete")).clicked() {
                    // no hotkey: a preset's own menu
                    ask = Some(Ask::Delete(i));
                    ui.close();
                }
            });
        }
    }
    if let Some(ask) = ask {
        run(app, ask);
    }
}

fn groups_text(groups: &[SettingsGroup]) -> String {
    let names: Vec<&str> = groups.iter().map(|g| g.label()).collect();
    if names.is_empty() { "Sets nothing".into() } else { format!("Sets: {}", names.join(", ")) }
}

/// LrC's Snapshots panel: settings kept under a name; a click goes back to them.
pub fn snapshots_panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &app.session else { return };
    let d = &s.develop;
    let mut ask = None;
    let mut hover = None;
    ui.horizontal(|ui| {
        ui.label(
            weak(format!("{} snapshot{}", d.snapshots.len(), if d.snapshots.len() == 1 { "" } else { "s" }))
                .size(12.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(d.photo.is_some(), egui::Button::new("+")) // key: NewSnapshot
                .on_hover_text("Keep the current settings as a snapshot (Ctrl+N)")
                .clicked()
            {
                ask = Some(Ask::TakeSnapshot);
            }
        });
    });
    for snap in &d.snapshots {
        let resp = super::sidebars::source_row(ui, 8.0, &snap.name, None, false);
        let resp = resp.on_hover_text("Click to go back to these settings");
        if resp.hovered() {
            let s = snap.settings.as_deref().and_then(imaging::develop::DevelopSettings::from_json);
            hover = Some(s.unwrap_or_else(|| d.defaults()));
        }
        if resp.clicked() {
            ask = Some(Ask::ApplySnapshot(snap.id));
        }
        resp.context_menu(|ui| {
            if ui.button("Update with Current Settings").clicked() {
                // no hotkey: a snapshot's own menu
                ask = Some(Ask::EditSnapshot(snap.id, SnapshotEdit::Update));
                ui.close();
            }
            if ui.button("Rename…").clicked() {
                // no hotkey: a snapshot's own menu
                ask = Some(Ask::EditSnapshot(snap.id, SnapshotEdit::Rename(snap.name.clone())));
                ui.close();
            }
            ui.separator();
            if ui.button("Delete").clicked() {
                // no hotkey: a snapshot's own menu
                ask = Some(Ask::EditSnapshot(snap.id, SnapshotEdit::Delete));
                ui.close();
            }
        });
    }
    if hover.is_some() {
        app.presets_ui.hover = hover;
    }
    if let Some(ask) = ask {
        run(app, ask);
    }
}

/// A snapshot's name for now: the date and time, as LrC names them.
fn snapshot_name() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Ctrl+N in Develop: a new snapshot, then name it.
/// The + menu's items by their keys (see `app_keys`).
pub fn run_menu(app: &mut FotaroApp, what: &str) {
    let has_photo = app.session.as_ref().is_some_and(|s| s.develop.photo.is_some());
    let ask = match what {
        "create" if has_photo => Ask::NewPreset,
        "import" => Ask::Import(false),
        "import_folder" => Ask::Import(true),
        "import_lightroom" if crate::presets::lightroom_folder().is_some() => Ask::ImportLightroom,
        _ => return,
    };
    run(app, ask);
}

pub fn take_snapshot(app: &mut FotaroApp) {
    run(app, Ask::TakeSnapshot);
}

fn run(app: &mut FotaroApp, ask: Ask) {
    match ask {
        Ask::Apply(i) => {
            let Some(p) = app.presets.presets.get(i).cloned() else { return };
            app.develop_with(|d, c| {
                let s = p.file.apply(&d.settings, d.kelvin());
                d.apply(c, s, &format!("Preset: {}", p.name()))
            });
        }
        Ask::Update(i) => {
            let Some(s) = app.session.as_ref().map(|s| s.develop.settings.clone()) else { return };
            match app.presets.update(i, &s) {
                Ok(()) => app.notify("Preset updated"),
                Err(e) => app.error = Some(format!("{e:#}")),
            }
        }
        Ask::Delete(i) => {
            if let Err(e) = app.presets.delete(i) {
                app.error = Some(format!("{e:#}"));
            }
        }
        Ask::Reveal(i) => {
            if let Some(path) = app.presets.presets.get(i).and_then(|p| p.path.clone()) {
                app.run(Command::Reveal(path));
            }
        }
        Ask::NewPreset => {
            let Some(s) = &app.session else { return };
            let d = &s.develop;
            let mut groups = changed_groups(&d.settings, d.is_raw());
            groups.retain(|g| !g.per_photo());
            app.presets_ui.new_preset = Some(NewPreset {
                name: "Untitled Preset".into(),
                folder: USER.into(),
                new_folder: None,
                groups,
            });
        }
        Ask::Import(folder) => {
            let picked: Vec<std::path::PathBuf> = if folder {
                rfd::FileDialog::new()
                    .set_title("Import Folder of Presets")
                    .pick_folder()
                    .into_iter()
                    .collect()
            } else {
                rfd::FileDialog::new()
                    .set_title("Import Presets")
                    .add_filter("Lightroom presets", &["xmp", "lrtemplate"])
                    .pick_files()
                    .unwrap_or_default()
            };
            if !picked.is_empty() {
                import(app, &picked);
            }
        }
        Ask::ImportLightroom => {
            if let Some(dir) = crate::presets::lightroom_folder() {
                import(app, &[dir]);
            }
        }
        Ask::TakeSnapshot => {
            let name = snapshot_name();
            app.develop_with(|d, c| d.take_snapshot(c, &name));
            // Name it right away (the date stays if left as it is).
            let id = app
                .session
                .as_ref()
                .and_then(|s| s.develop.snapshots.iter().find(|x| x.name == name).map(|x| x.id));
            if let Some(id) = id {
                app.presets_ui.rename = Some((RenameTarget::Snapshot(id), name));
            }
        }
        Ask::ApplySnapshot(id) => app.develop_with(|d, c| d.apply_snapshot(c, id)),
        Ask::EditSnapshot(id, SnapshotEdit::Rename(name)) => {
            app.presets_ui.rename = Some((RenameTarget::Snapshot(id), name));
        }
        Ask::EditSnapshot(id, edit) => app.develop_with(|d, c| d.edit_snapshot(c, id, edit)),
    }
}

fn import(app: &mut FotaroApp, paths: &[std::path::PathBuf]) {
    let r = app.presets.import(paths, USER);
    for (p, why) in &r.skipped {
        log::info!("preset not imported: {}: {why}", p.display());
    }
    let mut text = format!("Imported {} preset{}", r.imported, if r.imported == 1 { "" } else { "s" });
    if r.partial > 0 {
        text += &format!("; {} with settings we can't apply (see their tooltips)", r.partial);
    }
    if !r.skipped.is_empty() {
        text += &format!("; {} files skipped", r.skipped.len());
    }
    app.notify(text);
}

/// The panels' dialogs (New Preset, Copy Settings, Rename), drawn each frame in Develop.
pub fn dialogs(app: &mut FotaroApp, ctx: &egui::Context) {
    new_preset_dialog(app, ctx);
    copy_dialog(app, ctx);
    rename_dialog(app, ctx);
}

// hotkeys: none (a dialog's fields)
/// Checkboxes for the groups, under their panels' names, with Check All / None.
fn group_ticks(ui: &mut egui::Ui, ticked: &mut Vec<SettingsGroup>) {
    ui.horizontal(|ui| {
        if ui.button("Check All").clicked() {
            *ticked = SettingsGroup::ALL.to_vec();
        }
        if ui.button("Check None").clicked() {
            ticked.clear();
        }
    });
    ui.add_space(4.0);
    ui.columns(2, |cols| {
        let mut panel = "";
        for g in SettingsGroup::ALL {
            // Basic, Color and Detail on the left; Effects, Geometry, Local on the right.
            let col = usize::from(matches!(g.panel(), "Effects" | "Geometry" | "Local"));
            let ui = &mut cols[col];
            if g.panel() != panel {
                panel = g.panel();
                ui.add_space(4.0);
                ui.label(RichText::new(panel).strong().color(theme::TEXT_STRONG));
            }
            let mut on = ticked.contains(&g);
            if ui.checkbox(&mut on, g.label()).changed() {
                if on {
                    ticked.push(g);
                } else {
                    ticked.retain(|x| *x != g);
                }
            }
        }
    });
    ticked.sort();
}

// hotkeys: none (a dialog)
fn new_preset_dialog(app: &mut FotaroApp, ctx: &egui::Context) {
    let Some(np) = &mut app.presets_ui.new_preset else { return };
    let folders: Vec<String> = app.presets.folders().into_iter().filter(|f| f != BUILT_IN).collect();
    let (mut create, mut cancel) = (false, false);
    let modal = egui::Modal::new(egui::Id::unique("new_develop_preset")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.heading("New Develop Preset");
        ui.add_space(6.0);
        egui::Grid::new("new_preset_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
            ui.label("Preset Name:");
            let r = ui.add(egui::TextEdit::singleline(&mut np.name).desired_width(300.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                create = true;
            }
            ui.end_row();
            ui.label("Group:");
            ui.horizontal(|ui| {
                let shown = np.new_folder.as_ref().map_or(np.folder.clone(), |_| "New Folder…".into());
                egui::ComboBox::from_id_salt("preset_folder").selected_text(shown).width(180.0).show_ui(
                    ui,
                    |ui| {
                        for f in &folders {
                            if ui.selectable_label(np.new_folder.is_none() && np.folder == *f, f).clicked() {
                                np.folder = f.clone();
                                np.new_folder = None;
                            }
                        }
                        ui.separator();
                        if ui.selectable_label(np.new_folder.is_some(), "New Folder…").clicked() {
                            np.new_folder = Some(String::new());
                        }
                    },
                );
                if let Some(f) = &mut np.new_folder {
                    ui.add(egui::TextEdit::singleline(f).hint_text("Folder name").desired_width(110.0));
                }
            });
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.label(weak("Settings the preset sets (the rest of a photo's stay as they are):").size(12.0));
        group_ticks(ui, &mut np.groups);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let ok = !np.name.trim().is_empty()
                && !np.groups.is_empty()
                && np.new_folder.as_ref().is_none_or(|f| !f.trim().is_empty());
            if ui.add_enabled(ok, egui::Button::new("Create")).clicked() {
                create = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
    });
    if modal.should_close() {
        cancel = true;
    }
    if create && !np.name.trim().is_empty() && !np.groups.is_empty() {
        let folder =
            np.new_folder.clone().filter(|f| !f.trim().is_empty()).unwrap_or_else(|| np.folder.clone());
        let file = PresetFile {
            name: np.name.trim().to_string(),
            settings: app.session.as_ref().map(|s| s.develop.settings.clone()),
            groups: np.groups.clone(),
            lightroom: None,
            unsupported: Vec::new(),
        };
        app.presets_ui.new_preset = None;
        app.presets_ui.closed.remove(folder.trim());
        match app.presets.add(folder.trim(), file) {
            Ok(()) => app.notify("Preset created"),
            Err(e) => app.error = Some(format!("{e:#}")),
        }
    } else if cancel {
        app.presets_ui.new_preset = None;
    }
}

// hotkeys: none (a dialog)
fn copy_dialog(app: &mut FotaroApp, ctx: &egui::Context) {
    let Some(groups) = &mut app.presets_ui.copy else { return };
    let (mut copy, mut cancel) = (false, false);
    let modal = egui::Modal::new(egui::Id::unique("copy_develop_settings")).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.heading("Copy Settings");
        ui.add_space(4.0);
        ui.label(weak("Paste sets these on another photo; its other settings stay.").size(12.0));
        group_ticks(ui, groups);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!groups.is_empty(), egui::Button::new("Copy")).clicked() {
                copy = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
    });
    if modal.should_close() {
        cancel = true;
    }
    if copy {
        let groups = app.presets_ui.copy.take().unwrap_or_default();
        if let Some(s) = &mut app.session {
            s.develop.clipboard = Some((s.develop.settings.clone(), groups));
        }
        app.notify("Copied settings");
    } else if cancel {
        app.presets_ui.copy = None;
    }
}

// hotkeys: none (a dialog)
fn rename_dialog(app: &mut FotaroApp, ctx: &egui::Context) {
    let Some((target, name)) = &mut app.presets_ui.rename else { return };
    let target = *target;
    let (mut ok, mut cancel) = (false, false);
    let modal = egui::Modal::new(egui::Id::unique("rename_preset_or_snapshot")).show(ctx, |ui| {
        ui.set_width(320.0);
        ui.heading(match target {
            RenameTarget::Preset(_) => "Rename Preset",
            RenameTarget::Snapshot(_) => "Snapshot Name",
        });
        ui.add_space(6.0);
        let r = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            ok = true;
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(!name.trim().is_empty(), egui::Button::new("OK")).clicked() {
                ok = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
    });
    if modal.should_close() {
        cancel = true;
    }
    if ok && !name.trim().is_empty() {
        let name = name.trim().to_string();
        app.presets_ui.rename = None;
        match target {
            RenameTarget::Preset(i) => {
                if let Err(e) = app.presets.rename(i, &name) {
                    app.error = Some(format!("{e:#}"));
                }
            }
            RenameTarget::Snapshot(id) => {
                app.develop_with(|d, c| d.edit_snapshot(c, id, SnapshotEdit::Rename(name)));
            }
        }
    } else if cancel {
        app.presets_ui.rename = None;
    }
}
