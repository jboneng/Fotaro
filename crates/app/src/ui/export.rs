//! The Export dialog, laid out like LrC's: a preset list on the left, one column of
//! collapsible sections on the right (Export Location, File Naming, File Settings,
//! Image Sizing, Output Sharpening, Metadata, Watermarking, Post-Processing) and
//! Export / Cancel at the bottom.

use catalog::{ExistingFiles, MetadataLevel, RENAME_PRESETS, RENAME_TOKENS};
use eframe::egui::{self, Align, Layout, RichText};
use imaging::export::{
    Anchor, ColorSpace, FileFormat, ResizeMode, SharpenAmount, SharpenFor, WatermarkStyle,
};

use super::theme;
use crate::app::{Command, PhotoLibraryApp};
use crate::export::{
    BUILTIN_PRESETS, ExportPrefs, ExportPreset, ExportTo, PresetId, preset_prefs, unique_preset_name,
};

/// What the dialog exports, and a sample photo for the file name preview.
pub struct ExportDialog {
    pub photo_ids: Vec<i64>,
    /// File stem, extension and capture time of the first photo.
    pub sample: Option<(String, String, i64)>,
    /// The user preset last clicked (what Remove removes).
    pub selected: Option<String>,
    /// Add: the new preset's name being typed.
    pub new_preset: Option<String>,
}

impl ExportDialog {
    pub fn new(photo_ids: Vec<i64>, sample: Option<(String, String, i64)>) -> Self {
        ExportDialog { photo_ids, sample, selected: None, new_preset: None }
    }
}

fn weak(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::TEXT_WEAK)
}

fn section(ui: &mut egui::Ui, title: &str, summary: String, body: impl FnOnce(&mut egui::Ui)) {
    let id = ui.make_persistent_id(("export_section", title));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
        .show_header(ui, |ui| {
            ui.label(RichText::new(title).strong().color(theme::TEXT_STRONG));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Clear of the (floating) scroll bar.
                ui.add_space(14.0);
                ui.label(weak(summary))
            });
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

fn combo<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    width: f32,
    value: &mut T,
    all: &[T],
    label: fn(T) -> &'static str,
) {
    egui::ComboBox::from_id_salt(id).width(width).selected_text(label(*value)).show_ui(ui, |ui| {
        for v in all {
            ui.selectable_value(value, *v, label(*v));
        }
    });
}

/// File ▸ Export with Preset, and the same in the photo context menu.
pub fn preset_menu(ui: &mut egui::Ui, app: &mut PhotoLibraryApp) {
    let mut chosen = None;
    for (i, (name, _)) in BUILTIN_PRESETS.iter().enumerate() {
        if ui.button(*name).clicked() {
            chosen = Some(PresetId::Builtin(i));
        }
    }
    if !app.ui.export_presets.is_empty() {
        ui.separator();
        for p in &app.ui.export_presets {
            if ui.button(&p.name).clicked() {
                chosen = Some(PresetId::User(p.name.clone()));
            }
        }
    }
    if let Some(id) = chosen {
        app.run(Command::ExportWithPreset(id));
        ui.close();
    }
}

pub fn window(app: &mut PhotoLibraryApp, ctx: &egui::Context) {
    let Some(d) = app.session.as_mut().and_then(|s| s.export.as_mut()) else { return };
    let count = d.photo_ids.len();
    let sample = d.sample.clone();
    let mut commands: Vec<Command> = Vec::new();
    let screen = ctx.content_rect();
    let size = egui::vec2(860.0_f32.min(screen.width() - 40.0), (screen.height() * 0.85).min(860.0));
    let preset_w = 210.0_f32.min(size.x * 0.3);
    egui::Modal::new(egui::Id::unique("export_dialog")).show(ctx, |ui| {
        ui.set_width(size.x);
        ui.label(
            RichText::new(format!("Export {count} File{}", if count == 1 { "" } else { "s" }))
                .size(18.0)
                .color(theme::TEXT_STRONG),
        );
        ui.separator();
        let body_h = size.y - 90.0;
        // A fixed-height body: the vertical separator and the presets' bottom-aligned
        // buttons fill the height they're given, and a Modal would otherwise grow to
        // fit them, frame after frame.
        ui.allocate_ui_with_layout(egui::vec2(size.x, body_h), Layout::left_to_right(Align::Min), |ui| {
            ui.set_height(body_h);
            ui.allocate_ui_with_layout(egui::vec2(preset_w, body_h), Layout::top_down(Align::Min), |ui| {
                ui.set_width(preset_w);
                ui.set_min_height(body_h);
                presets(ui, &mut app.ui.export, &mut app.ui.export_presets, d, body_h);
            });
            ui.separator();
            ui.vertical(|ui| {
                let width = ui.available_width();
                egui::ScrollArea::vertical().id_salt("export_settings").max_height(body_h).show(ui, |ui| {
                    ui.set_width(width);
                    settings(&mut app.ui.export, sample.as_ref(), ui);
                });
            });
        });
        ui.separator();
        let p = &app.ui.export;
        ui.horizontal(|ui| {
            match p.problem() {
                Some(problem) => ui.label(RichText::new(problem).color(theme::MISSING)),
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
    // As with Import, only Cancel or Esc close it (Esc first leaves a preset name edit).
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        match app.session.as_mut().and_then(|s| s.export.as_mut()) {
            Some(d) if d.new_preset.is_some() => d.new_preset = None,
            _ => commands.push(Command::CloseExport),
        }
    }
    for c in commands {
        app.run(c);
    }
}

enum PresetAction {
    Update(usize),
    Delete(usize),
}

/// The preset column: built-in and user presets, Add and Remove.
fn presets(
    ui: &mut egui::Ui,
    p: &mut ExportPrefs,
    user: &mut Vec<ExportPreset>,
    d: &mut ExportDialog,
    height: f32,
) {
    ui.label(RichText::new("Preset:").strong().color(theme::TEXT_STRONG));
    let mut action = None;
    egui::ScrollArea::vertical().id_salt("export_presets").max_height(height - 70.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::CollapsingHeader::new("Built-in").default_open(true).show(ui, |ui| {
            for (i, (name, _)) in BUILTIN_PRESETS.iter().enumerate() {
                let Some(probe) = preset_prefs(p, user, &PresetId::Builtin(i)) else { continue };
                let active = probe == *p;
                let r = ui.selectable_label(active, *name).on_hover_text(probe.summary());
                if r.clicked() {
                    *p = probe;
                    d.selected = None;
                }
            }
        });
        egui::CollapsingHeader::new("User Presets").default_open(true).show(ui, |ui| {
            if user.is_empty() && d.new_preset.is_none() {
                ui.label(weak("Add saves the current settings as a preset."));
            }
            for (i, u) in user.iter().enumerate() {
                let active = u.prefs == *p || d.selected.as_deref() == Some(u.name.as_str());
                let r = ui.selectable_label(active, &u.name).on_hover_text(u.prefs.summary());
                if r.clicked() {
                    *p = u.prefs.clone();
                    d.selected = Some(u.name.clone());
                }
                r.context_menu(|ui| {
                    if ui.button("Update with Current Settings").clicked() {
                        action = Some(PresetAction::Update(i));
                        ui.close();
                    }
                    if ui.button("Delete").clicked() {
                        action = Some(PresetAction::Delete(i));
                        ui.close();
                    }
                });
            }
            let mut create = None;
            if let Some(name) = &mut d.new_preset {
                let r =
                    ui.add(egui::TextEdit::singleline(name).hint_text("Preset name").desired_width(170.0));
                if !r.has_focus() && !r.lost_focus() && name.is_empty() {
                    r.request_focus();
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let mut cancel = false;
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() || enter {
                        create = Some(name.clone());
                    }
                    cancel = ui.button("Cancel").clicked();
                });
                if cancel {
                    d.new_preset = None;
                }
            }
            if let Some(name) = create {
                let name = unique_preset_name(user, &name);
                user.push(ExportPreset { name: name.clone(), prefs: p.clone() });
                d.selected = Some(name);
                d.new_preset = None;
            }
        });
    });
    match action {
        Some(PresetAction::Update(i)) => user[i].prefs = p.clone(),
        Some(PresetAction::Delete(i)) => {
            let gone = user.remove(i);
            if d.selected.as_deref() == Some(gone.name.as_str()) {
                d.selected = None;
            }
        }
        None => {}
    }
    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        ui.horizontal(|ui| {
            if ui.button("Add").on_hover_text("Save the current settings as a new preset").clicked() {
                d.new_preset = Some(String::new());
            }
            let selected = d.selected.clone().and_then(|n| user.iter().position(|u| u.name == n));
            if ui
                .add_enabled(selected.is_some(), egui::Button::new("Remove"))
                .on_hover_text("Delete the selected user preset")
                .clicked()
                && let Some(i) = selected
            {
                user.remove(i);
                d.selected = None;
            }
        });
    });
}

fn settings(p: &mut ExportPrefs, sample: Option<&(String, String, i64)>, ui: &mut egui::Ui) {
    let location = match p.export_to {
        ExportTo::SpecificFolder => p.folder.display().to_string(),
        ExportTo::SameFolder => "Same folder as original".to_owned(),
    };
    section(ui, "Export Location", location, |ui| {
        row(ui, "Export To:", |ui| {
            combo(
                ui,
                "export_to",
                260.0,
                &mut p.export_to,
                &[ExportTo::SpecificFolder, ExportTo::SameFolder],
                ExportTo::label,
            );
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
            ui.add_enabled(p.add_to_catalog, egui::Checkbox::new(&mut p.add_to_stack, "Add to Stack"))
                .on_hover_text("Each export joins its original's stack (role Export)");
        });
        row(ui, "Existing Files:", |ui| {
            combo(ui, "export_existing", 300.0, &mut p.existing, &ExistingFiles::ALL, ExistingFiles::label);
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

    section(ui, "File Settings", p.file_settings(), |ui| file_settings(p, ui));

    let rendered = p.image.format != FileFormat::Original;
    if rendered {
        let sizing = p.sizing().unwrap_or_else(|| "Full size".to_owned());
        section(ui, "Image Sizing", sizing, |ui| image_sizing(p, ui));

        let i = &mut p.image;
        let sharpening = if i.sharpen {
            format!("{}, {}", i.sharpen_for.label(), i.sharpen_amount.label())
        } else {
            "Off".to_owned()
        };
        section(ui, "Output Sharpening", sharpening, |ui| {
            row(ui, "", |ui| {
                ui.checkbox(&mut i.sharpen, "Sharpen For:");
                ui.add_enabled_ui(i.sharpen, |ui| {
                    combo(
                        ui,
                        "export_sharpen_for",
                        140.0,
                        &mut i.sharpen_for,
                        &SharpenFor::ALL,
                        SharpenFor::label,
                    );
                    ui.label(weak("Amount:"));
                    combo(
                        ui,
                        "export_sharpen_amount",
                        110.0,
                        &mut i.sharpen_amount,
                        &SharpenAmount::ALL,
                        SharpenAmount::label,
                    );
                });
            });
            row(ui, "", |ui| {
                ui.label(weak("Paper sharpening adapts to the Resolution set under Image Sizing."));
            });
        });
    }

    let mut meta_summary = p.metadata.label().to_owned();
    if p.remove_location && p.metadata.location() {
        meta_summary += ", no location";
    }
    section(ui, "Metadata", meta_summary, |ui| {
        row(ui, "Include:", |ui| {
            combo(ui, "export_metadata", 260.0, &mut p.metadata, &MetadataLevel::ALL, MetadataLevel::label);
        });
        row(ui, "", |ui| {
            ui.add_enabled(
                p.metadata.location(),
                egui::Checkbox::new(&mut p.remove_location, "Remove Location Info"),
            )
            .on_disabled_hover_text("This level leaves the location out anyway");
        });
        row(ui, "", |ui| {
            let note = if rendered {
                "Keywords are written with their hierarchy (Lightroom style)."
            } else {
                "Originals keep everything in the file itself; these settings apply to the XMP sidecar."
            };
            ui.label(weak(note));
        });
    });

    if rendered {
        let w = &mut p.image.watermark;
        let summary = if w.enabled { w.style.label().to_owned() } else { "No watermark".to_owned() };
        section(ui, "Watermarking", summary, |ui| watermarking(w, ui));
    }

    let reveal = if p.reveal { crate::platform::REVEAL_LABEL } else { "Do nothing" };
    section(ui, "Post-Processing", reveal.to_owned(), |ui| {
        row(ui, "After Export:", |ui| {
            ui.checkbox(&mut p.reveal, crate::platform::REVEAL_LABEL);
        });
    });
}

fn file_settings(p: &mut ExportPrefs, ui: &mut egui::Ui) {
    let i = &mut p.image;
    row(ui, "Image Format:", |ui| {
        combo(ui, "export_format", 110.0, &mut i.format, &FileFormat::ALL, FileFormat::label);
        if i.format != FileFormat::Original {
            ui.label(weak("Color Space:"));
            combo(ui, "export_color_space", 150.0, &mut i.color_space, &ColorSpace::ALL, ColorSpace::label);
        }
    });
    let bit_depth = |ui: &mut egui::Ui, sixteen: &mut bool| {
        row(ui, "Bit Depth:", |ui| {
            egui::ComboBox::from_id_salt("export_bit_depth")
                .width(150.0)
                .selected_text(if *sixteen { "16 bits/component" } else { "8 bits/component" })
                .show_ui(ui, |ui| {
                    ui.selectable_value(sixteen, false, "8 bits/component");
                    ui.selectable_value(sixteen, true, "16 bits/component");
                });
        });
    };
    match i.format {
        FileFormat::Jpeg => {
            row(ui, "Quality:", |ui| {
                ui.add_enabled(!i.limit_size, egui::Slider::new(&mut i.jpeg_quality, 0..=100));
            });
            row(ui, "", |ui| {
                ui.checkbox(&mut i.limit_size, "Limit File Size To:");
                ui.add_enabled(
                    i.limit_size,
                    egui::DragValue::new(&mut i.limit_kb).range(10..=100_000).suffix(" K"),
                );
            });
        }
        FileFormat::Tiff => {
            row(ui, "Compression:", |ui| {
                ui.radio_value(&mut i.tiff_zip, false, "None");
                ui.radio_value(&mut i.tiff_zip, true, "ZIP");
            });
            bit_depth(ui, &mut i.sixteen_bit);
        }
        FileFormat::Png => bit_depth(ui, &mut i.sixteen_bit),
        FileFormat::Original => row(ui, "", |ui| {
            ui.label(weak("The original file, unchanged, with an XMP sidecar holding the catalog metadata"));
        }),
    }
    if i.sixteen() {
        row(ui, "", |ui| {
            ui.label(weak(
                "16 bits keep the precision of 16-bit scans and TIFFs. RAW files export the camera's \
                 full-size preview, as shown in the Library.",
            ));
        });
    }
    if i.color_space == ColorSpace::ProPhotoRgb && !i.sixteen() && i.format != FileFormat::Original {
        row(ui, "", |ui| {
            ui.label(weak("ProPhoto RGB is best used with 16 bits/component (TIFF or PNG)."));
        });
    }
}

fn image_sizing(p: &mut ExportPrefs, ui: &mut egui::Ui) {
    let i = &mut p.image;
    row(ui, "", |ui| {
        ui.checkbox(&mut i.resize, "Resize to Fit:");
        ui.add_enabled_ui(i.resize, |ui| {
            combo(ui, "export_resize", 130.0, &mut i.resize_mode, &ResizeMode::ALL, ResizeMode::label);
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
            ui.add(egui::DragValue::new(&mut i.megapixels).range(0.1..=500.0).speed(0.1).suffix(" MP"));
        }),
        ResizeMode::Percentage => row(ui, "", |ui| {
            ui.add(egui::DragValue::new(&mut i.percent).range(1..=400).suffix(" %"));
        }),
    });
    row(ui, "Resolution:", |ui| {
        ui.add(egui::DragValue::new(&mut i.ppi).range(1..=4800).suffix(" pixels per inch"));
    });
}

fn watermarking(w: &mut imaging::export::Watermark, ui: &mut egui::Ui) {
    row(ui, "", |ui| {
        ui.checkbox(&mut w.enabled, "Watermark:");
        ui.add_enabled_ui(w.enabled, |ui| {
            combo(ui, "export_watermark", 220.0, &mut w.style, &WatermarkStyle::ALL, WatermarkStyle::label);
        });
    });
    ui.add_enabled_ui(w.enabled, |ui| {
        match w.style {
            WatermarkStyle::Copyright => row(ui, "", |ui| {
                ui.label(weak("Each photo's Copyright metadata; photos without one get no watermark."));
            }),
            WatermarkStyle::Text => row(ui, "Text:", |ui| {
                ui.add(egui::TextEdit::singleline(&mut w.text).hint_text("© Your Name").desired_width(300.0));
            }),
            WatermarkStyle::Graphic => row(ui, "Graphic:", |ui| {
                let label = if w.graphic.as_os_str().is_empty() {
                    "(choose a PNG or JPEG)".to_owned()
                } else {
                    w.graphic.file_name().unwrap_or_default().to_string_lossy().into_owned()
                };
                ui.label(RichText::new(label).color(theme::TEXT_STRONG))
                    .on_hover_text(w.graphic.display().to_string());
                if ui.button("Choose…").clicked()
                    && let Some(f) = rfd::FileDialog::new()
                        .set_title("Watermark Graphic")
                        .add_filter("Images", &["png", "jpg", "jpeg"])
                        .pick_file()
                {
                    w.graphic = f;
                }
            }),
        }
        if w.style != WatermarkStyle::Graphic {
            row(ui, "Color:", |ui| {
                ui.radio_value(&mut w.black, false, "White");
                ui.radio_value(&mut w.black, true, "Black");
                ui.checkbox(&mut w.shadow, "Shadow");
            });
        }
        row(ui, "Opacity:", |ui| {
            ui.add(egui::Slider::new(&mut w.opacity, 1..=100).suffix(" %"));
        });
        row(ui, "Size:", |ui| {
            ui.add(egui::Slider::new(&mut w.size, 1..=100).suffix(" % of width"));
        });
        row(ui, "Inset:", |ui| {
            ui.add(egui::Slider::new(&mut w.inset, 0..=25).suffix(" %"));
        });
        row(ui, "Anchor:", |ui| {
            egui::Grid::new("export_watermark_anchor").spacing([2.0, 2.0]).show(ui, |ui| {
                for (n, a) in Anchor::ALL.into_iter().enumerate() {
                    ui.radio_value(&mut w.anchor, a, "").on_hover_text(a.label());
                    if n % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
        });
    });
}
