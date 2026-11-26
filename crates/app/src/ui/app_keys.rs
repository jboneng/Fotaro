//! Hot keys for what the app does itself, without a panel's row being drawn: the menu
//! items that are not actions, the Library toolbar's sort, the Library Filter bar, the
//! Library panels' buttons, and the Presets panel's + menu.
//!
//! [`items`] lists them for the hot keys list; [`run`] does one.

use catalog::{FileKind, Flag, NumOp, SortKey, TextField, TextOp};
use eframe::egui;

use super::shortcuts::Scope;
use crate::app::{Command, FotaroApp, Lights, MetadataPreset};

const FILE: &str = "Menu ▸ File";
const EDIT: &str = "Menu ▸ Edit";
const LIBRARY: &str = "Menu ▸ Library";
const PHOTO: &str = "Menu ▸ Photo";
const METADATA: &str = "Menu ▸ Metadata";
const VIEW: &str = "Menu ▸ View";
const HELP: &str = "Menu ▸ Help";
const SORT: &str = "Library ▸ Toolbar";
const FILTER: &str = "Library ▸ Filter Bar";
const PANELS: &str = "Library ▸ Panels";
const PRESETS: &str = "Develop ▸ Presets";

const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

/// Every item: id, name, section, where it works.
pub fn items() -> Vec<(String, String, &'static str, Scope)> {
    let mut out = Vec::new();
    let mut add = |id: String, name: String, section: &'static str, scope: Scope| {
        out.push((id, name, section, scope));
    };
    for (id, name, section) in [
        ("menu.file.default_catalog", "Open or Create the Default Catalog", FILE),
        ("menu.file.new_catalog", "New Catalog…", FILE),
        ("menu.file.open_catalog", "Open Catalog…", FILE),
        ("menu.file.exit", "Exit", FILE),
        ("menu.edit.external_editing", "External Editing Settings…", EDIT),
        ("menu.library.sync_folder", "Synchronize Folder…", LIBRARY),
        ("menu.library.subfolders", "Show Photos in Subfolders", LIBRARY),
        ("menu.library.new_collection", "New Collection…", LIBRARY),
        ("menu.library.new_smart", "New Smart Collection…", LIBRARY),
        ("menu.library.new_set", "New Collection Set…", LIBRARY),
        ("menu.library.separate_jpeg", "Treat JPEG Files Next to Raw Files as Separate Photos", LIBRARY),
        ("menu.library.build_previews", "Build Standard-Sized Previews", LIBRARY),
        ("menu.photo.reveal", crate::platform::REVEAL_LABEL, PHOTO),
        ("menu.metadata.read", "Read Metadata from Files", METADATA),
        ("menu.metadata.auto_xmp", "Automatically Write Changes into XMP", METADATA),
        ("menu.view.lights.on", "Lights On", VIEW),
        ("menu.view.lights.dim", "Lights Dim", VIEW),
        ("menu.view.lights.off", "Lights Off", VIEW),
        ("menu.help.check_updates", "Check for Updates…", HELP),
        ("menu.help.auto_updates", "Check for Updates Automatically", HELP),
        ("menu.help.about", "About", HELP),
        ("jobs.stop_import", "Stop the Import", FILE),
        ("jobs.stop_export", "Stop the Export", FILE),
    ] {
        add(id.into(), name.into(), section, Scope::All);
    }
    let lib = Scope::Library;
    add("library.sort.reverse".into(), "Sort: Reverse the Direction".into(), SORT, lib);
    for k in SortKey::ALL {
        add(format!("library.sort.{k:?}"), format!("Sort by {}", k.label()), SORT, lib);
    }
    for (id, name) in
        [("text", "Text"), ("attribute", "Attribute"), ("metadata", "Metadata"), ("none", "None")]
    {
        add(format!("library.filter.{id}"), format!("Library Filter: {name}"), FILTER, lib);
    }
    for t in TextField::ALL {
        add(format!("library.filter.text_field.{t:?}"), format!("Text Filter in {}", t.label()), FILTER, lib);
    }
    for o in TextOp::ALL {
        add(format!("library.filter.text_op.{o:?}"), format!("Text Filter: {}", o.label()), FILTER, lib);
    }
    add("library.filter.text.clear".into(), "Text Filter: Clear".into(), FILTER, lib);
    for (f, name) in [(Flag::Picked, "Flagged"), (Flag::Unflagged, "Unflagged"), (Flag::Rejected, "Rejected")]
    {
        add(format!("library.filter.flag.{f:?}"), format!("Attribute Filter: {name}"), FILTER, lib);
    }
    for o in NumOp::ALL {
        add(format!("library.filter.rating_op.{o:?}"), format!("Rating Filter: {}", o.label()), FILTER, lib);
    }
    for n in 1..=5 {
        add(format!("library.filter.rating.{n}"), format!("Rating Filter: {n} Stars"), FILTER, lib);
    }
    for l in LABELS {
        add(format!("library.filter.label.{l}"), format!("Color Filter: {l}"), FILTER, lib);
    }
    add("library.filter.label.none".into(), "Color Filter: No Label".into(), FILTER, lib);
    add("library.filter.kind.raw".into(), "Kind Filter: Raw".into(), FILTER, lib);
    add("library.filter.kind.other".into(), "Kind Filter: Other".into(), FILTER, lib);
    add("library.folders.add".into(), "Add Folder…".into(), PANELS, lib);
    for (id, name) in [
        ("next", "Next Folder"),
        ("previous", "Previous Folder"),
        ("expand_all", "Expand All Folders"),
        ("collapse_all", "Collapse All Folders"),
        ("reveal", "Show the Selected Folder"),
    ] {
        add(format!("library.folders.{id}"), name.into(), PANELS, lib);
    }
    add("library.keywords.create".into(), "Create Keyword Tag…".into(), PANELS, lib);
    for (p, name) in
        [(MetadataPreset::Default, "Default"), (MetadataPreset::Exif, "EXIF"), (MetadataPreset::Iptc, "IPTC")]
    {
        add(format!("library.metadata.{p:?}"), format!("Metadata Panel: {name}"), PANELS, lib);
    }
    for (id, name) in [
        ("create", "Create Preset…"),
        ("import", "Import Presets…"),
        ("import_folder", "Import Folder of Presets…"),
        ("import_lightroom", "Import Presets from Lightroom"),
    ] {
        add(format!("develop.presets.{id}"), name.into(), PRESETS, Scope::Develop);
    }
    out
}

/// A flag list or similar: add `value`, or take it out.
fn toggle<T: PartialEq>(list: &mut Vec<T>, value: T) {
    match list.iter().position(|v| *v == value) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(value),
    }
}

/// Do item `id` (its key was pressed).
pub fn run(app: &mut FotaroApp, ctx: &egui::Context, id: &str) {
    let has_catalog = app.session.is_some();
    let rest = |prefix: &str| id.strip_prefix(prefix);
    if let Some(what) = rest("develop.presets.") {
        super::presets::run_menu(app, what);
        return;
    }
    if let Some(what) = rest("library.filter.") {
        filter(app, what);
        return;
    }
    if let Some(what) = rest("library.sort.") {
        let mut sort = app.ui.sort;
        match what {
            "reverse" => sort.descending = !sort.descending,
            key => match SortKey::ALL.into_iter().find(|k| format!("{k:?}") == key) {
                Some(k) => sort.key = k,
                None => return,
            },
        }
        app.run(Command::SetSort(sort));
        return;
    }
    if let Some(what) = rest("library.folders.")
        && what != "add"
    {
        folders(app, ctx, what);
        return;
    }
    if let Some(p) = rest("library.metadata.") {
        for preset in [MetadataPreset::Default, MetadataPreset::Exif, MetadataPreset::Iptc] {
            if format!("{preset:?}") == p {
                app.ui.metadata_preset = preset;
            }
        }
        return;
    }
    let folder = app.session.as_ref().and_then(|s| match s.source {
        catalog::Source::Folder { id, .. } => Some(id),
        _ => None,
    });
    let collection = |kind| Command::CreateCollection { parent: None, kind };
    let cmd = match id {
        "menu.file.default_catalog" => Command::CreateDefaultCatalog,
        "menu.file.new_catalog" => Command::NewCatalog,
        "menu.file.open_catalog" => Command::OpenCatalog,
        "menu.file.exit" => Command::Quit,
        "menu.edit.external_editing" => Command::OpenEditSettings,
        "menu.library.sync_folder" => match folder {
            Some(f) => Command::SyncFolder(f),
            None => return,
        },
        "menu.library.subfolders" => {
            app.ui.include_subfolders = !app.ui.include_subfolders;
            match folder {
                Some(id) => Command::SetSource(catalog::Source::Folder {
                    id,
                    include_subfolders: app.ui.include_subfolders,
                }),
                None => return,
            }
        }
        "menu.library.new_collection" if has_catalog => collection(catalog::CollectionKind::Collection),
        "menu.library.new_smart" if has_catalog => collection(catalog::CollectionKind::Smart),
        "menu.library.new_set" if has_catalog => collection(catalog::CollectionKind::Set),
        "menu.library.separate_jpeg" if has_catalog => Command::ToggleSeparateRawJpeg,
        "menu.library.build_previews" if app.session.as_ref().is_some_and(|s| !s.selected.is_empty()) => {
            Command::BuildPreviews
        }
        "menu.photo.reveal" => match app.session.as_ref().and_then(|s| s.active_photo()) {
            Some(p) => Command::Reveal(p.path.clone()),
            None => return,
        },
        "menu.metadata.read" if has_catalog => Command::ReadMetadata,
        "menu.metadata.auto_xmp" => Command::ToggleAutoWriteXmp,
        "menu.view.lights.on" => Command::SetLights(Lights::On),
        "menu.view.lights.dim" => Command::SetLights(Lights::Dim),
        "menu.view.lights.off" => Command::SetLights(Lights::Off),
        "menu.help.check_updates" => {
            app.updater.check(ctx, true);
            return;
        }
        "menu.help.auto_updates" => {
            app.ui.check_updates = !app.ui.check_updates;
            return;
        }
        "menu.help.about" => {
            app.show_about = true;
            return;
        }
        "jobs.stop_import" => Command::CancelImport,
        "jobs.stop_export" => Command::CancelExport,
        "library.folders.add" => Command::OpenImport(Some(catalog::ImportMode::Add)),
        "library.keywords.create" if has_catalog => Command::CreateKeyword(None),
        _ => return,
    };
    app.run(cmd);
}

/// The Folders panel, by its keys. It shows (with the folder in view) for all but the
/// expand and collapse ones.
fn folders(app: &mut FotaroApp, ctx: &egui::Context, what: &str) {
    let Some(s) = &app.session else { return };
    match what {
        "expand_all" => {
            app.ui.closed_folders.clear();
            app.ui.closed_volumes.clear();
            return;
        }
        "collapse_all" => {
            let parents: std::collections::BTreeSet<i64> =
                s.folders.iter().filter_map(|f| f.parent_id).collect();
            for f in s.folders.iter().filter(|f| parents.contains(&f.id)) {
                app.ui.closed_folders.insert(f.path.to_string_lossy().into_owned());
            }
            return;
        }
        "next" | "previous" => {
            let rows = super::sidebars::folder_rows(ctx);
            let current = match s.source {
                catalog::Source::Folder { id, .. } => rows.iter().position(|r| *r == id),
                _ => None,
            };
            let at = match (current, what == "next") {
                (Some(i), true) => (i + 1).min(rows.len().saturating_sub(1)),
                (Some(i), false) => i.saturating_sub(1),
                (None, _) => 0,
            };
            let Some(&id) = rows.get(at) else { return };
            let include_subfolders = app.ui.include_subfolders;
            app.run(Command::SetSource(catalog::Source::Folder { id, include_subfolders }));
        }
        _ => {}
    }
    app.ui.show_left = true;
    app.ui.left.set_open("Folders", true, super::sidebars::LEFT);
    super::sidebars::reveal_folder(ctx);
}

/// The Library Filter bar, by its keys: the bar shows, with the row the key is in.
fn filter(app: &mut FotaroApp, what: &str) {
    app.ui.show_filter_bar = true;
    let f = &mut app.ui.filter;
    let row = |f: &mut crate::filter::FilterBar, which: fn(&mut crate::filter::FilterBar) -> &mut bool| {
        *which(f) = true;
        f.enabled = true;
    };
    match what {
        "text" => super::filter_bar::pick_mode(f, |f| &mut f.text_on, false),
        "attribute" => super::filter_bar::pick_mode(f, |f| &mut f.attribute_on, false),
        "metadata" => super::filter_bar::pick_mode(f, |f| &mut f.metadata_on, false),
        "none" => f.none(),
        "text.clear" => f.text.clear(),
        "label.none" => {
            toggle(&mut f.labels, None);
            row(f, |f| &mut f.attribute_on);
        }
        "kind.raw" | "kind.other" => {
            toggle(&mut f.kinds, if what == "kind.raw" { FileKind::Raw } else { FileKind::Image });
            row(f, |f| &mut f.attribute_on);
        }
        _ => {
            if let Some(t) = what.strip_prefix("text_field.") {
                if let Some(t) = TextField::ALL.into_iter().find(|v| format!("{v:?}") == t) {
                    f.text_field = t;
                    row(f, |f| &mut f.text_on);
                }
            } else if let Some(o) = what.strip_prefix("text_op.") {
                if let Some(o) = TextOp::ALL.into_iter().find(|v| format!("{v:?}") == o) {
                    f.text_op = o;
                    row(f, |f| &mut f.text_on);
                }
            } else if let Some(o) = what.strip_prefix("rating_op.") {
                if let Some(o) = NumOp::ALL.into_iter().find(|v| format!("{v:?}") == o) {
                    f.rating_op = o;
                    row(f, |f| &mut f.attribute_on);
                }
            } else if let Some(n) = what.strip_prefix("rating.").and_then(|n| n.parse::<u8>().ok()) {
                f.rating = if f.rating == n { 0 } else { n };
                row(f, |f| &mut f.attribute_on);
            } else if let Some(flag) = what.strip_prefix("flag.") {
                for fl in [Flag::Picked, Flag::Unflagged, Flag::Rejected] {
                    if format!("{fl:?}") == flag {
                        toggle(&mut f.flags, fl);
                        row(f, |f| &mut f.attribute_on);
                    }
                }
            } else if let Some(l) = what.strip_prefix("label.")
                && LABELS.contains(&l)
            {
                toggle(&mut f.labels, Some(l.to_owned()));
                row(f, |f| &mut f.attribute_on);
            }
        }
    }
}
