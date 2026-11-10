//! External Editing settings (LrC's Preferences ▸ External Editing): where NegPy is,
//! and the file NegPy exports for the catalog.

use eframe::egui::{self, Align, Layout, RichText};

use super::theme;
use crate::app::PhotoLibraryApp;
use crate::editors::{EditColorSpace, EditFormat, RESULT_SUFFIX};

fn weak(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::TEXT_WEAK)
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([120.0, 20.0], egui::Label::new(weak(label)).halign(Align::Max));
        add(ui);
    });
}

pub fn window(app: &mut PhotoLibraryApp, ctx: &egui::Context) {
    if !app.edit_settings {
        return;
    }
    if app.negpy.is_none() {
        app.negpy = Some(crate::editors::detect(app.ui.edit_in.negpy_location.as_deref()));
    }
    let mut close = false;
    let mut location_changed = false;
    let width = 600.0_f32.min(ctx.content_rect().width() - 40.0);
    egui::Modal::new(egui::Id::unique("edit_in_settings")).show(ctx, |ui| {
        ui.set_width(width);
        ui.label(RichText::new("External Editing").size(18.0).color(theme::TEXT_STRONG));
        ui.separator();
        ui.label(RichText::new("NegPy").strong().color(theme::TEXT_STRONG));
        let p = &mut app.ui.edit_in;
        row(ui, "Application:", |ui| match &app.negpy {
            Some(Ok(n)) => {
                ui.label(RichText::new(n.describe()).color(theme::TEXT_STRONG));
            }
            Some(Err(why)) => {
                ui.label(RichText::new(why).color(theme::MISSING));
            }
            None => {}
        });
        row(ui, "", |ui| {
            let how = if p.negpy_location.is_some() { "Chosen by you" } else { "Found automatically" };
            ui.label(weak(how));
            if ui.button("Choose Folder…").on_hover_text("A NegPy source checkout, or the folder NegPy is installed in").clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title("NegPy Folder").pick_folder()
            {
                p.negpy_location = Some(dir);
                location_changed = true;
            }
            if ui.button("Choose Program…").clicked()
                && let Some(f) = rfd::FileDialog::new().set_title("NegPy Program").pick_file()
            {
                p.negpy_location = Some(f);
                location_changed = true;
            }
            if ui.add_enabled(p.negpy_location.is_some(), egui::Button::new("Find Automatically")).clicked() {
                p.negpy_location = None;
                location_changed = true;
            }
        });
        ui.add_space(6.0);
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
            egui::ComboBox::from_id_salt("edit_color").selected_text(p.color_space.label()).show_ui(ui, |ui| {
                for c in EditColorSpace::ALL {
                    ui.selectable_value(&mut p.color_space, c, c.label());
                }
            });
        });
        row(ui, "", |ui| {
            ui.label(weak(format!(
                "NegPy saves its export next to the original as \"<name>_{RESULT_SUFFIX}\" ({}); it is added to \
                 the catalog and stacked with the original. Exporting again in NegPy updates it.",
                p.format.label()
            )));
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
    if location_changed {
        app.negpy = None;
    }
    if close {
        app.edit_settings = false;
    }
}

/// Photo ▸ Edit In, and the same in the photo context menu.
pub fn menu(ui: &mut egui::Ui, app: &mut PhotoLibraryApp) {
    let has_catalog = app.session.is_some();
    let shortcut = super::layout::shortcut_for(super::shortcuts::Action::EditIn);
    let button = egui::Button::new("Edit in NegPy…").shortcut_text(shortcut.unwrap_or_default());
    let r = ui.add_enabled(has_catalog, button);
    let r = match &app.negpy {
        Some(Err(why)) => r.on_hover_text(why.clone()),
        Some(Ok(n)) => r.on_hover_text(n.describe()),
        None => r,
    };
    if r.clicked() {
        app.run(crate::app::Command::EditInNegPy);
        ui.close();
    }
    let _ = ui
        .add_enabled(false, egui::Button::new("Edit in Luminar…"))
        .on_disabled_hover_text("Not yet supported");
    ui.separator();
    if ui.button("External Editing Settings…").clicked() {
        app.run(crate::app::Command::OpenEditSettings);
        ui.close();
    }
}
