//! External Editing settings (LrC's Preferences ▸ External Editing): where NegPy and
//! Luminar Neo are, which one Ctrl+E opens, and the file NegPy exports for the catalog.

use std::path::PathBuf;

use eframe::egui::{self, Align, Layout, RichText};

use super::theme;
use crate::app::{Command, FotaroApp};
use crate::editors::{EditColorSpace, EditFormat, Editor, RESULT_SUFFIX, darktable, luminar};

fn weak(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::TEXT_WEAK)
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([120.0, 20.0], egui::Label::new(weak(label)).halign(Align::Max));
        add(ui);
    });
}

/// Look for the editors not looked for yet.
fn refresh(app: &mut FotaroApp) {
    for e in Editor::ALL {
        if !app.editors.contains_key(&e) {
            let found = crate::editors::find(e, &app.ui.edit_in);
            app.editors.insert(e, found);
        }
    }
}

/// "Application:" with what was found, and the buttons to choose another location.
/// Returns true when the location changed.
fn location(
    ui: &mut egui::Ui,
    found: Option<&Result<crate::editors::Found, String>>,
    editor: Editor,
    place: &mut Option<PathBuf>,
) -> bool {
    let mut changed = false;
    row(ui, "Application:", |ui| match found {
        Some(Ok(f)) => {
            ui.label(RichText::new(f.describe()).color(theme::TEXT_STRONG));
        }
        Some(Err(why)) => {
            ui.label(RichText::new(why).color(theme::MISSING));
        }
        None => {}
    });
    row(ui, "", |ui| {
        ui.label(weak(if place.is_some() { "Chosen by you" } else { "Found automatically" }));
        let folder_hint = match editor {
            Editor::NegPy => "A NegPy source checkout, or the folder NegPy is installed in",
            Editor::Luminar => "The folder Luminar Neo is installed in",
            Editor::Darktable => "The folder darktable is installed in (with darktable-cli)",
        };
        if ui.button("Choose Folder…").on_hover_text(folder_hint).clicked()
            && let Some(dir) = rfd::FileDialog::new().set_title(editor.label()).pick_folder()
        {
            *place = Some(dir);
            changed = true;
        }
        if ui.button("Choose Program…").clicked()
            && let Some(f) = rfd::FileDialog::new().set_title(editor.label()).pick_file()
        {
            *place = Some(f);
            changed = true;
        }
        if ui.add_enabled(place.is_some(), egui::Button::new("Find Automatically")).clicked() {
            *place = None;
            changed = true;
        }
    });
    changed
}

pub fn window(app: &mut FotaroApp, ctx: &egui::Context) {
    if !app.edit_settings {
        return;
    }
    refresh(app);
    let mut close = false;
    let mut changed = Vec::new();
    let width = 640.0_f32.min(ctx.content_rect().width() - 40.0);
    let editors = &app.editors;
    let p = &mut app.ui.edit_in;
    egui::Modal::new(egui::Id::unique("edit_in_settings")).show(ctx, |ui| {
        ui.set_width(width);
        ui.label(RichText::new("External Editing").size(18.0).color(theme::TEXT_STRONG));
        ui.separator();
        row(ui, "Ctrl+E opens:", |ui| {
            for e in Editor::ALL {
                ui.radio_value(&mut p.primary, e, e.label());
            }
        });
        row(ui, "Ctrl+Alt+E opens:", |ui| {
            for e in Editor::ALL {
                ui.radio_value(&mut p.secondary, e, e.label());
            }
        });
        ui.add_space(6.0);

        ui.label(RichText::new("NegPy").strong().color(theme::TEXT_STRONG));
        if location(ui, editors.get(&Editor::NegPy), Editor::NegPy, &mut p.negpy_location) {
            changed.push(Editor::NegPy);
        }
        row(ui, "File Format:", |ui| {
            egui::ComboBox::from_id_salt("edit_format").selected_text(p.format.label()).show_ui(ui, |ui| {
                for f in EditFormat::ALL {
                    ui.selectable_value(&mut p.format, f, f.label());
                }
            });
            if p.format.has_bit_depth() {
                ui.label(weak("Bit Depth:"));
                egui::ComboBox::from_id_salt("edit_bits")
                    .selected_text(if p.sixteen_bit { "16 bits/component" } else { "8 bits/component" })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut p.sixteen_bit, false, "8 bits/component");
                        ui.selectable_value(&mut p.sixteen_bit, true, "16 bits/component");
                    });
            } else {
                ui.label(weak("Quality:"));
                ui.add(egui::Slider::new(&mut p.quality, 1..=100));
            }
        });
        row(ui, "Color Space:", |ui| {
            egui::ComboBox::from_id_salt("edit_color").selected_text(p.color_space.label()).show_ui(
                ui,
                |ui| {
                    for c in EditColorSpace::ALL {
                        ui.selectable_value(&mut p.color_space, c, c.label());
                    }
                },
            );
        });
        row(ui, "", |ui| {
            ui.label(weak(format!(
                "NegPy saves its export next to the original as \"<name>_{RESULT_SUFFIX}\" ({}). Exporting \
                 again in NegPy updates it.",
                p.format.label()
            )));
        });
        ui.add_space(6.0);

        ui.label(RichText::new("Luminar Neo").strong().color(theme::TEXT_STRONG));
        if location(ui, editors.get(&Editor::Luminar), Editor::Luminar, &mut p.luminar_location) {
            changed.push(Editor::Luminar);
        }
        row(ui, "", |ui| {
            ui.label(weak(format!(
                "Luminar Neo opens the original, one photo at a time. Apply saves the edit next to the \
                 original as \"<name>{}.tif\".",
                luminar::RESULT_SUFFIX
            )));
        });
        ui.add_space(6.0);

        ui.label(RichText::new("darktable").strong().color(theme::TEXT_STRONG));
        if location(ui, editors.get(&Editor::Darktable), Editor::Darktable, &mut p.darktable_location) {
            changed.push(Editor::Darktable);
        }
        row(ui, "Bit Depth:", |ui| {
            egui::ComboBox::from_id_salt("darktable_bits")
                .selected_text(if p.darktable_sixteen_bit { "16 bits/component" } else { "8 bits/component" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut p.darktable_sixteen_bit, false, "8 bits/component");
                    ui.selectable_value(&mut p.darktable_sixteen_bit, true, "16 bits/component");
                });
            ui.label(weak("Color Space:"));
            egui::ComboBox::from_id_salt("darktable_color").selected_text(p.darktable_color_space.label()).show_ui(
                ui,
                |ui| {
                    for c in EditColorSpace::ALL {
                        ui.selectable_value(&mut p.darktable_color_space, c, c.label());
                    }
                },
            );
        });
        row(ui, "", |ui| {
            ui.label(weak(format!(
                "darktable opens the originals; edit, then close darktable. Each photo you changed is \n                 rendered as \"<name>{}.tif\" (TIFF) next to the original.",
                darktable::RESULT_SUFFIX
            )));
        });
        ui.add_space(4.0);
        row(ui, "", |ui| {
            ui.label(weak("Edits are added to the catalog with the original's metadata, stacked below it."));
        });
        ui.separator();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Close").clicked() {
                close = true;
            }
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }
    for e in changed {
        app.editors.remove(&e);
    }
    if close {
        app.edit_settings = false;
    }
}

/// Photo ▸ Edit In, and the same in the photo context menu.
pub fn menu(ui: &mut egui::Ui, app: &mut FotaroApp) {
    let has_catalog = app.session.is_some();
    let primary = app.ui.edit_in.primary;
    for editor in Editor::ALL {
        let action = if editor == primary {
            Some(super::shortcuts::Action::EditIn)
        } else if editor == app.ui.edit_in.secondary {
            Some(super::shortcuts::Action::EditInOther)
        } else {
            None
        };
        let shortcut = action.and_then(super::layout::shortcut_for).unwrap_or_default();
        let button = egui::Button::new(format!("Edit in {}…", editor.label())).shortcut_text(shortcut);
        let r = ui.add_enabled(has_catalog, button);
        let r = match app.editors.get(&editor) {
            Some(Err(why)) => r.on_hover_text(why.clone()),
            Some(Ok(f)) => r.on_hover_text(f.describe()),
            None => r,
        };
        if r.clicked() {
            app.run(Command::EditIn(editor));
            ui.close();
        }
    }
    ui.separator();
    if ui.button("External Editing Settings…").clicked() {
        app.run(Command::OpenEditSettings);
        ui.close();
    }
}
