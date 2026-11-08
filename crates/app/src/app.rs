//! The egui application: UI state and rendering.

use std::path::PathBuf;

use eframe::egui;
use serde::{Deserialize, Serialize};

use catalog::Photo;

/// State persisted between runs (via eframe's `persistence` feature).
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    library_root: Option<PathBuf>,
}

#[derive(Default)]
pub struct PhotoLibraryApp {
    settings: Settings,
    photos: Vec<Photo>,
    selected: Option<usize>,
    status: String,
}

impl PhotoLibraryApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();

        let mut app = Self { settings, ..Default::default() };
        if app.settings.library_root.is_some() {
            app.rescan();
        } else {
            app.status = "Open a folder to get started.".into();
        }
        app
    }

    fn pick_folder(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            self.settings.library_root = Some(dir);
            self.rescan();
        }
    }

    // TODO: move scanning off the UI thread once libraries get large.
    fn rescan(&mut self) {
        let Some(root) = self.settings.library_root.clone() else { return };
        self.selected = None;
        match catalog::scan(&root) {
            Ok(photos) => {
                self.status = format!("{} photos in {}", photos.len(), root.display());
                self.photos = photos;
            }
            Err(err) => {
                self.status = format!("Scan failed: {err:#}");
                self.photos.clear();
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open folder…").clicked() {
                    self.pick_folder();
                }
                let can_rescan = self.settings.library_root.is_some();
                if ui.add_enabled(can_rescan, egui::Button::new("Rescan")).clicked() {
                    self.rescan();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }

    fn photo_list(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show_rows(
            ui,
            ui.text_style_height(&egui::TextStyle::Body),
            self.photos.len(),
            |ui, range| {
                for i in range {
                    let name = &self.photos[i].file_name;
                    if ui.selectable_label(self.selected == Some(i), name).clicked() {
                        self.selected = Some(i);
                    }
                }
            },
        );
    }

    fn details(&self, ui: &mut egui::Ui) {
        match self.selected.and_then(|i| self.photos.get(i)) {
            Some(photo) => {
                ui.heading(&photo.file_name);
                ui.label(photo.path.display().to_string());
                ui.label(format!("{:.1} KB", photo.size_bytes as f64 / 1024.0));
                // TODO: render the image / thumbnail grid here.
            }
            None => {
                ui.centered_and_justified(|ui| ui.weak("Select a photo"));
            }
        }
    }
}

impl eframe::App for PhotoLibraryApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("menu").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| ui.label(&self.status));
        egui::Panel::left("photos")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| self.photo_list(ui));
        egui::CentralPanel::default().show(ui, |ui| self.details(ui));
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }
}
