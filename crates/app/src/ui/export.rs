//! The Export dialog, laid out like LrC's: one column of collapsible sections
//! (Export Location, File Naming, File Settings, Image Sizing, Metadata,
//! Post-Processing) and Export / Cancel at the bottom.

use catalog::{ExistingFiles, MetadataLevel, RENAME_PRESETS, RENAME_TOKENS};
use eframe::egui::{self, Align, Layout, RichText};
use imaging::export::{FileFormat, ResizeMode};

use super::theme;
use crate::app::{Command, PhotoLibraryApp};
use crate::export::{ExportPrefs, ExportTo};

/// What the dialog exports, and a sample photo for the file name preview.
pub struct ExportDialog {
    pub photo_ids: Vec<i64>,
    /// File stem, extension and capture time of the first photo.
    pub sample: Option<(String, String, i64)>,
}

fn weak(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::TEXT_WEAK)
}

fn section(ui: &mut egui::Ui, title: &str, summary: String, body: impl FnOnce(&mut egui::Ui)) {
    let id = ui.make_persistent_id(("export_section", title));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            ui.label(RichText::new(title).strong().color(theme::TEXT_STRONG));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(weak(summary)));
        })
        .body(|ui| {
            ui.add_space(4.0);
            body(ui);
            ui.add_space(6.0);
        });
    ui.separator();
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 20.0], egui::Label::new(weak(label)).halign(Align::Max));
        add(ui);
    });
}

pub fn window(app: &mut PhotoLibraryApp, ctx: &egui::Context) {
    let Some(d) = app.session.as_ref().and_then(|s| s.export.as_ref()) else { return };
    let count = d.photo_ids.len();
    let sample = d.sample.clone();
    let mut commands: Vec<Command> = Vec::new();
    let screen = ctx.content_rect();
    let size = egui::vec2(640.0_f32.min(screen.width() - 40.0), (screen.height() * 0.85).min(820.0));
    egui::Modal::new(egui::Id::unique("export_dialog")).show(ctx, |ui| {
        ui.set_width(size.x);
        ui.label(
            RichText::new(format!("Export {count} File{}", if count == 1 { "" } else { "s" }))
                .size(18.0)
                .color(theme::TEXT_STRONG),
        );
        ui.separator();
        egui::ScrollArea::vertical().id_salt("export_settings").max_height(size.y - 90.0).show(ui, |ui| {
            ui.set_width(size.x - 16.0);
            settings(&mut app.ui.export, sample.as_ref(), ui);
        });
        ui.separator();
        let p = &app.ui.export;
        ui.horizontal(|ui| {
            match p.problem() {
                Some(problem) => ui.label(RichText::new(problem).color(theme::STAR)),
                None => ui.label(weak(p.summary())),
            };
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ok = p.problem().is_none() && count > 0;
                if ui.add_enabled(ok, egui::Button::new(RichText::new("Export").size(15.0))).clicked() {
                    commands.push(Command::StartExport);
                }
                if ui.button("Cancel").clicked() {
                    commands.push(Command::CloseExport);
                }
            });
        });
    });
    // As with Import, only Cancel or Esc close it.
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        commands.push(Command::CloseExport);
    }
    for c in commands {
        app.run(c);
    }
}

fn settings(p: &mut ExportPrefs, sample: Option<&(String, String, i64)>, ui: &mut egui::Ui) {
    let location = match p.export_to {
        ExportTo::SpecificFolder => p.folder.display().to_string(),
        ExportTo::SameFolder => "Same folder as original".to_owned(),
    };
    section(ui, "Export Location", location, |ui| {
        row(ui, "Export To:", |ui| {
            egui::ComboBox::from_id_salt("export_to")
                .width(260.0)
                .selected_text(p.export_to.label())
                .show_ui(ui, |ui| {
                    for t in [ExportTo::SpecificFolder, ExportTo::SameFolder] {
                        ui.selectable_value(&mut p.export_to, t, t.label());
                    }
                });
        });
        if p.export_to == ExportTo::SpecificFolder {
            row(ui, "Folder:", |ui| {
                let label = if p.folder.as_os_str().is_empty() {
                    "(choose a folder)".into()
                } else {
                    p.folder.display().to_string()
                };
                ui.label(RichText::new(label).color(theme::TEXT_STRONG));
                if ui.button("Choose…").clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_title("Export To").pick_folder()
                {
                    p.folder = dir;
                }
            });
        }
        row(ui, "", |ui| {
            ui.checkbox(&mut p.subfolder_on, "Put in Subfolder:");
            ui.add_enabled(p.subfolder_on, egui::TextEdit::singleline(&mut p.subfolder).desired_width(200.0));
        });
        row(ui, "", |ui| {
            ui.checkbox(&mut p.add_to_catalog, "Add to This Catalog");
            let can_stack = p.export_to == ExportTo::SameFolder && !p.subfolder_on;
            ui.add_enabled(
                can_stack && p.add_to_catalog,
                egui::Checkbox::new(&mut p.add_to_stack, "Add to Stack"),
            )
            .on_disabled_hover_text("Stacking needs exporting to the original's folder, without a subfolder");
        });
        row(ui, "Existing Files:", |ui| {
            egui::ComboBox::from_id_salt("export_existing")
                .width(300.0)
                .selected_text(p.existing.label())
                .show_ui(ui, |ui| {
                    for e in ExistingFiles::ALL {
                        ui.selectable_value(&mut p.existing, e, e.label());
                    }
                });
        });
    });

    let naming = if p.rename_on { p.template.clone() } else { "Filename".to_owned() };
    section(ui, "File Naming", naming, |ui| {
        row(ui, "", |ui| {
            ui.checkbox(&mut p.rename_on, "Rename To:");
            let current = if !p.rename_on {
                "Filename"
            } else {
                RENAME_PRESETS.iter().find(|(_, t)| *t == p.template).map_or("Custom", |(n, _)| *n)
            };
            ui.add_enabled_ui(p.rename_on, |ui| {
                egui::ComboBox::from_id_salt("export_rename").width(240.0).selected_text(current).show_ui(
                    ui,
                    |ui| {
                        if ui.selectable_label(p.template == "{Filename}", "Filename").clicked() {
                            p.template = "{Filename}".into();
                        }
                        for (name, template) in RENAME_PRESETS {
                            if ui.selectable_label(current == *name, *name).clicked() {
                                p.template = (*template).to_owned();
                            }
                        }
                    },
                );
            });
        });
        if p.rename_on {
            row(ui, "Template:", |ui| {
                ui.add(egui::TextEdit::singleline(&mut p.template).desired_width(300.0))
                    .on_hover_text(format!("Tokens: {RENAME_TOKENS}"));
            });
            row(ui, "Custom Text:", |ui| {
                ui.add(egui::TextEdit::singleline(&mut p.custom_text).desired_width(160.0));
                ui.label(weak("Start Number:"));
                ui.add(egui::DragValue::new(&mut p.start_number).range(0..=999_999));
            });
        }
        if let Some((stem, ext, time)) = sample {
            let ext = p.image.format.extension().map_or_else(|| ext.clone(), str::to_owned);
            let name = if p.rename_on {
                catalog::import::render_template(
                    &p.template,
                    stem,
                    p.custom_text.trim(),
                    p.start_number,
                    *time,
                )
            } else {
                Ok(stem.clone())
            };
            row(ui, "Example:", |ui| {
                match name {
                    Ok(n) => ui.label(format!("{n}.{ext}")),
                    Err(e) => ui.label(RichText::new(e.to_string()).color(theme::MISSING)),
                };
            });
        }
    });

    section(ui, "File Settings", p.image.format.label().to_owned(), |ui| {
        let i = &mut p.image;
        row(ui, "Image Format:", |ui| {
            egui::ComboBox::from_id_salt("export_format").selected_text(i.format.label()).show_ui(ui, |ui| {
                for f in FileFormat::ALL {
                    ui.selectable_value(&mut i.format, f, f.label());
                }
            });
            if i.format != FileFormat::Original {
                ui.label(weak("Color Space: sRGB"));
            }
        });
        match i.format {
            FileFormat::Jpeg => row(ui, "Quality:", |ui| {
                ui.add(egui::Slider::new(&mut i.jpeg_quality, 0..=100));
            }),
            FileFormat::Tiff => row(ui, "Compression:", |ui| {
                ui.radio_value(&mut i.tiff_zip, false, "None");
                ui.radio_value(&mut i.tiff_zip, true, "ZIP");
                ui.label(weak("Bit Depth: 8 bits/component"));
            }),
            FileFormat::Png => row(ui, "", |ui| {
                ui.label(weak("8 bits/component, lossless"));
            }),
            FileFormat::Original => row(ui, "", |ui| {
                ui.label(weak(
                    "The original file, unchanged, with an XMP sidecar holding the catalog metadata",
                ));
            }),
        }
    });

    let rendered = p.image.format != FileFormat::Original;
    if rendered {
        let sizing = if p.image.resize {
            p.summary().split_once(", ").map(|(_, s)| s.to_owned()).unwrap_or_default()
        } else {
            "Full size".to_owned()
        };
        section(ui, "Image Sizing", sizing, |ui| {
            let i = &mut p.image;
            row(ui, "", |ui| {
                ui.checkbox(&mut i.resize, "Resize to Fit:");
                ui.add_enabled_ui(i.resize, |ui| {
                    egui::ComboBox::from_id_salt("export_resize")
                        .selected_text(i.resize_mode.label())
                        .show_ui(ui, |ui| {
                            for m in ResizeMode::ALL {
                                ui.selectable_value(&mut i.resize_mode, m, m.label());
                            }
                        });
                    ui.checkbox(&mut i.dont_enlarge, "Don't Enlarge");
                });
            });
            ui.add_enabled_ui(i.resize, |ui| match i.resize_mode {
                ResizeMode::WidthHeight | ResizeMode::Dimensions => row(ui, "W:", |ui| {
                    ui.add(egui::DragValue::new(&mut i.width).range(0..=65_000).suffix(" px"));
                    ui.label(weak("H:"));
                    ui.add(egui::DragValue::new(&mut i.height).range(0..=65_000).suffix(" px"));
                }),
                ResizeMode::LongEdge | ResizeMode::ShortEdge => row(ui, "", |ui| {
                    ui.add(egui::DragValue::new(&mut i.edge).range(1..=65_000).suffix(" px"));
                }),
                ResizeMode::Megapixels => row(ui, "", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut i.megapixels).range(0.1..=500.0).speed(0.1).suffix(" MP"),
                    );
                }),
                ResizeMode::Percentage => row(ui, "", |ui| {
                    ui.add(egui::DragValue::new(&mut i.percent).range(1..=400).suffix(" %"));
                }),
            });
            row(ui, "Resolution:", |ui| {
                ui.add(egui::DragValue::new(&mut i.ppi).range(1..=4800).suffix(" pixels per inch"));
            });
        });
    }

    section(ui, "Metadata", p.metadata.label().to_owned(), |ui| {
        row(ui, "Include:", |ui| {
            egui::ComboBox::from_id_salt("export_metadata")
                .width(260.0)
                .selected_text(p.metadata.label())
                .show_ui(ui, |ui| {
                    for m in MetadataLevel::ALL {
                        ui.selectable_value(&mut p.metadata, m, m.label());
                    }
                });
        });
        row(ui, "", |ui| {
            ui.label(weak("Keywords are written with their hierarchy (Lightroom style)."));
        });
    });

    let reveal = if p.reveal { crate::platform::REVEAL_LABEL } else { "Do nothing" };
    section(ui, "Post-Processing", reveal.to_owned(), |ui| {
        row(ui, "After Export:", |ui| {
            ui.checkbox(&mut p.reveal, crate::platform::REVEAL_LABEL);
        });
    });
}
