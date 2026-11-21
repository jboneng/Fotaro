//! The Import dialog, laid out like LrC's: Source on the left, the photos found in
//! the middle, Copy | Move | Add on top, File Handling / File Renaming / Apply During
//! Import / Destination on the right, and the Import button at the bottom.

use catalog::{DateFolders, FileKind, ImportMode, RENAME_PRESETS, RENAME_TOKENS};
use eframe::egui::{self, Align, Color32, Layout, RichText, Stroke};

use super::theme;
use crate::app::{Command, FotaroApp};
use crate::jobs::Job;

const CELL: f32 = 132.0;

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).strong().color(theme::TEXT_STRONG));
    ui.separator();
}

fn weak(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::TEXT_WEAK)
}

pub fn window(app: &mut FotaroApp, ctx: &egui::Context) {
    if app.session.as_ref().is_none_or(|s| s.import.is_none()) {
        return;
    }
    let screen = ctx.content_rect();
    let size = egui::vec2((screen.width() * 0.92).min(1500.0), (screen.height() * 0.88).min(950.0));
    let mut commands: Vec<Command> = Vec::new();
    let modal = egui::Modal::new(egui::Id::unique("import_dialog")).show(ctx, |ui| {
        ui.set_width(size.x);
        ui.set_height(size.y);
        top_bar(app, ui);
        ui.separator();
        let body_height = size.y - 90.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(egui::vec2(230.0, body_height), Layout::top_down(Align::Min), |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("import_source")
                    .show(ui, |ui| source_panel(app, ui, &mut commands));
            });
            ui.separator();
            let center_width = ui.available_width() - 330.0;
            ui.allocate_ui_with_layout(
                egui::vec2(center_width, body_height),
                Layout::top_down(Align::Min),
                |ui| {
                    photos_panel(app, ui);
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(egui::vec2(318.0, body_height), Layout::top_down(Align::Min), |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("import_settings")
                    .show(ui, |ui| settings_panel(app, ui));
            });
        });
        ui.separator();
        bottom_bar(app, ui, &mut commands);
    });
    // Like LrC, only Cancel or Esc close it: a stray click outside must not throw away
    // the photos you ticked.
    let _ = modal;
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        commands.push(Command::CloseImport);
    }
    for c in commands {
        app.run(c);
    }
}

fn top_bar(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let source = app.session.as_ref().and_then(|s| s.import.as_ref()).and_then(|d| d.source.clone());
    let mode = &mut app.ui.import.settings.mode;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Import").size(18.0).color(theme::TEXT_STRONG));
        ui.add_space(12.0);
        ui.label(weak("From"));
        ui.label(source.map_or_else(|| "(choose a source)".into(), |p| p.display().to_string()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for (m, name, tip) in [
                (ImportMode::Add, "Add", "Catalog the photos where they are"),
                (
                    ImportMode::Move,
                    "Move",
                    "Move the photos to the destination (originals are deleted after copying)",
                ),
                (ImportMode::Copy, "Copy", "Copy the photos to the destination"),
            ] {
                let on = *mode == m;
                let text = RichText::new(name).size(15.0).color(if on {
                    theme::TEXT_STRONG
                } else {
                    theme::TEXT_WEAK
                });
                if ui.add(egui::Button::new(text).selected(on)).on_hover_text(tip).clicked() {
                    *mode = m;
                }
            }
        });
    });
}

fn source_panel(app: &mut FotaroApp, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
    let Some(s) = &mut app.session else { return };
    let Some(d) = &mut s.import else { return };
    let volumes = d.volumes.lock().unwrap().clone();
    heading(ui, "Source");
    match &volumes {
        None => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(weak("Looking for drives…"));
            });
        }
        Some(vols) => {
            // LrC picks an inserted camera card automatically.
            if d.source.is_none()
                && let Some(card) = vols.iter().find(|v| v.card)
            {
                d.recent_started = true; // the card wins over the last source
                commands.push(Command::ImportSource(card.path.join("DCIM")));
            }
            if vols.iter().any(|v| v.card) {
                ui.label(weak("Devices").size(11.0));
                for v in vols.iter().filter(|v| v.card) {
                    let dcim = v.path.join("DCIM");
                    let on = d.source.as_deref() == Some(dcim.as_path());
                    if ui.selectable_label(on, format!("📷 {} (card)", v.name)).clicked() {
                        commands.push(Command::ImportSource(dcim));
                    }
                }
                ui.add_space(4.0);
            }
            ui.label(weak("Drives").size(11.0));
            for v in vols.iter().filter(|v| !v.card) {
                let on = d.source.as_deref() == Some(v.path.as_path());
                if ui.selectable_label(on, &v.name).on_hover_text("Import from the whole drive").clicked() {
                    commands.push(Command::ImportSource(v.path.clone()));
                }
            }
        }
    }
    ui.add_space(8.0);
    if let Some(choice) = d.picking_source.as_ref().and_then(|p| p.poll()) {
        d.picking_source = None;
        if let Some(dir) = choice {
            commands.push(Command::ImportSource(dir));
        }
    }
    if ui.add_enabled(d.picking_source.is_none(), egui::Button::new("Choose Folder…")).clicked() {
        d.picking_source = Some(crate::platform::pick_folder(ui.ctx(), "Import from Folder"));
    }
    // Checked in the background: `is_dir` on a busy network share can block for seconds.
    let recent = d.recent.get().cloned().flatten();
    // Without a card, start from the last source.
    if !d.recent_started
        && let Some(last) = &recent
    {
        d.recent_started = true;
        if d.source.is_none() {
            commands.push(Command::ImportSource(last.clone()));
        }
    }
    if let Some(last) = recent {
        ui.add_space(4.0);
        ui.label(weak("Recent").size(11.0));
        let name =
            last.file_name().map_or_else(|| last.display().to_string(), |n| n.to_string_lossy().into());
        if ui
            .selectable_label(d.source.as_ref() == Some(&last), name)
            .on_hover_text(last.display().to_string())
            .clicked()
        {
            commands.push(Command::ImportSource(last));
        }
    }
    ui.add_space(8.0);
    if ui.checkbox(&mut app.ui.import.include_subfolders, "Include Subfolders").changed()
        && let Some(src) = app.session.as_ref().and_then(|s| s.import.as_ref()).and_then(|d| d.source.clone())
    {
        commands.push(Command::ImportSource(src));
    }
}

fn photos_panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let skip = app.ui.import.settings.skip_duplicates;
    let Some(s) = &mut app.session else { return };
    let Some(d) = &mut s.import else { return };
    let Some(candidates) = d.candidates.clone() else {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            if d.source.is_some() {
                ui.spinner();
                ui.label(weak("Finding photos…"));
            } else {
                ui.label(weak("Choose a card, drive or folder on the left."));
            }
        });
        return;
    };
    if let Some(e) = &d.error {
        ui.label(RichText::new(e).color(theme::MISSING));
    }
    let dupes = candidates.iter().filter(|c| c.duplicate).count();
    ui.horizontal(|ui| {
        if ui.button("Check All").clicked() {
            d.checked.iter_mut().for_each(|c| *c = true);
        }
        if ui.button("Uncheck All").clicked() {
            d.checked.iter_mut().for_each(|c| *c = false);
        }
        ui.label(weak(format!("{} photos found", candidates.len())));
        if dupes > 0 {
            let note = if skip { "already in the catalog (skipped)" } else { "already in the catalog" };
            ui.label(weak(format!("· {dupes} {note}")));
        }
    });
    if candidates.is_empty() {
        ui.label(weak("No photos here."));
        return;
    }
    let cols = ((ui.available_width() / CELL).floor() as usize).max(1);
    let rows = candidates.len().div_ceil(cols);
    let mut requests = Vec::new();
    egui::ScrollArea::vertical().id_salt("import_grid").auto_shrink([false, false]).show_rows(
        ui,
        CELL + 4.0,
        rows,
        |ui, range| {
            d.thumbs.begin_frame();
            for row in range {
                ui.horizontal(|ui| {
                    for col in 0..cols {
                        let i = row * cols + col;
                        let Some(c) = candidates.get(i) else { break };
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(CELL - 4.0, CELL), egui::Sense::click());
                        let skipped = skip && c.duplicate;
                        let on = d.checked[i] && !skipped;
                        let bg = if on { Color32::from_gray(0x5a) } else { Color32::from_gray(0x38) };
                        ui.painter().rect_filled(rect, 2.0, bg);
                        let img_area = rect.shrink2(egui::vec2(8.0, 18.0)).translate(egui::vec2(0.0, -4.0));
                        let key = d.thumb_key(i);
                        match d.thumbs.get(key) {
                            Some(tex) => {
                                let r = super::views::fit_rect(tex.size_vec2(), img_area);
                                let tint = if on { Color32::WHITE } else { Color32::from_white_alpha(90) };
                                super::views::paint_image(ui.painter(), &tex, r, 0, tint);
                            }
                            None => {
                                if d.thumbs.should_request(key) {
                                    requests.push((key, c.path.clone(), c.kind));
                                }
                            }
                        }
                        // Checkbox (top left), name and kind (bottom).
                        let check = egui::Rect::from_min_size(
                            rect.min + egui::vec2(5.0, 5.0),
                            egui::vec2(12.0, 12.0),
                        );
                        ui.painter().rect_stroke(
                            check,
                            2.0,
                            Stroke::new(1.0, theme::TEXT),
                            egui::StrokeKind::Inside,
                        );
                        if on {
                            ui.painter().rect_filled(check.shrink(3.0), 1.0, theme::TEXT_STRONG);
                        }
                        let name = c.path.file_name().unwrap_or_default().to_string_lossy();
                        let label = if skipped { "Already in catalog".to_string() } else { name.to_string() };
                        ui.painter().text(
                            rect.center_bottom() - egui::vec2(0.0, 7.0),
                            egui::Align2::CENTER_CENTER,
                            label,
                            egui::FontId::proportional(10.5),
                            if on { theme::TEXT_STRONG } else { theme::TEXT_WEAK },
                        );
                        if c.kind == FileKind::Raw {
                            ui.painter().text(
                                rect.right_top() + egui::vec2(-5.0, 5.0),
                                egui::Align2::RIGHT_TOP,
                                "RAW",
                                egui::FontId::proportional(9.0),
                                theme::TEXT_WEAK,
                            );
                        }
                        let resp = resp.on_hover_text(c.path.display().to_string());
                        if resp.clicked() && !skipped {
                            d.checked[i] ^= true;
                        }
                    }
                });
            }
        },
    );
    // Visible cells first (the visible lane is last-in, first-out).
    for (key, path, kind) in requests {
        s.jobs.submit_import_thumb(Job::ImportThumb { key, path, kind });
    }
}

fn settings_panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let first = app
        .session
        .as_ref()
        .and_then(|s| s.import.as_ref())
        .and_then(|d| d.candidates.as_ref())
        .and_then(|c| c.first().cloned());
    let p = &mut app.ui.import;
    let picking = app.session.as_mut().and_then(|s| s.import.as_mut()).map(|d| &mut d.picking_destination);
    heading(ui, "File Handling");
    ui.checkbox(&mut p.settings.skip_duplicates, "Don't Import Suspected Duplicates")
        .on_hover_text("Same file name and size as a photo already in the catalog");

    if p.settings.mode == ImportMode::Add {
        heading(ui, "Destination");
        ui.label(weak("Add catalogs the photos where they are; nothing is copied or moved."));
    } else {
        heading(ui, "File Renaming");
        ui.checkbox(&mut p.rename_on, "Rename Files");
        if p.rename_on {
            let current = RENAME_PRESETS.iter().find(|(_, t)| *t == p.template).map_or("Custom", |(n, _)| *n);
            egui::ComboBox::from_id_salt("rename_preset").width(260.0).selected_text(current).show_ui(
                ui,
                |ui| {
                    for (name, template) in RENAME_PRESETS {
                        if ui.selectable_label(current == *name, *name).clicked() {
                            p.template = (*template).to_owned();
                        }
                    }
                },
            );
            ui.add(egui::TextEdit::singleline(&mut p.template).desired_width(f32::INFINITY))
                .on_hover_text(format!("Tokens: {RENAME_TOKENS}"));
            ui.horizontal(|ui| {
                ui.label(weak("Custom Text"));
                ui.add(egui::TextEdit::singleline(&mut p.settings.custom_text).desired_width(150.0));
            });
            ui.horizontal(|ui| {
                ui.label(weak("Start Number"));
                ui.add(egui::DragValue::new(&mut p.settings.start_number).range(0..=999_999));
            });
            if let Some(c) = &first {
                let stem = c.path.file_stem().unwrap_or_default().to_string_lossy();
                let ext = c.path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                match catalog::import::render_template(
                    &p.template,
                    &stem,
                    p.settings.custom_text.trim(),
                    p.settings.start_number,
                    c.capture_time.unwrap_or(0),
                ) {
                    Ok(name) => ui.label(weak(format!("Sample: {name}{ext}"))),
                    Err(e) => ui.label(RichText::new(e.to_string()).color(theme::MISSING)),
                };
            }
        }

        heading(ui, "Destination");
        let dest = &mut p.settings.destination;
        ui.horizontal(|ui| {
            let label = if dest.as_os_str().is_empty() {
                "(choose a folder)".to_string()
            } else {
                dest.display().to_string()
            };
            ui.label(RichText::new(label).color(theme::TEXT_STRONG));
        });
        if let Some(slot) = picking {
            if let Some(choice) = slot.as_ref().and_then(|p| p.poll()) {
                *slot = None;
                if let Some(dir) = choice {
                    *dest = dir;
                }
            }
            if ui.add_enabled(slot.is_none(), egui::Button::new("Choose…")).clicked() {
                *slot = Some(crate::platform::pick_folder(ui.ctx(), "Import To"));
            }
        }
        ui.checkbox(&mut p.subfolder_on, "Into Subfolder");
        if p.subfolder_on {
            ui.add(
                egui::TextEdit::singleline(&mut p.subfolder).hint_text("Subfolder name").desired_width(220.0),
            );
        }
        ui.horizontal(|ui| {
            ui.label(weak("Organize"));
            egui::ComboBox::from_id_salt("date_folders")
                .selected_text(p.settings.date_folders.label())
                .show_ui(ui, |ui| {
                    for f in DateFolders::ALL {
                        ui.selectable_value(&mut p.settings.date_folders, f, f.label());
                    }
                });
        });
        if p.settings.mode == ImportMode::Move {
            ui.label(
                RichText::new("Move deletes each original after its copy is checked.").color(theme::STAR),
            );
        }
    }

    heading(ui, "Apply During Import");
    ui.label(weak("Keywords"));
    ui.add(egui::TextEdit::multiline(&mut p.keywords).desired_rows(2).desired_width(f32::INFINITY))
        .on_hover_text("Comma separated; Child < Parent for hierarchy");
}

fn bottom_bar(app: &mut FotaroApp, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
    let prefs = app.ui.import.clone();
    let chosen = app
        .session
        .as_ref()
        .and_then(|s| s.import.as_ref())
        .map(|d| d.chosen(prefs.settings.skip_duplicates))
        .unwrap_or_default();
    let bytes: u64 = chosen.iter().map(|c| c.size).sum();
    // The plan catches bad templates, subfolder names and a missing destination.
    let problem = if chosen.is_empty() {
        Some("Nothing to import".to_string())
    } else {
        catalog::import::plan(&chosen[..1], &prefs.effective()).err().map(|e| e.to_string())
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{} photos / {}", chosen.len(), crate::platform::human_bytes(bytes)))
                .size(14.0),
        );
        if let Some(p) = &problem {
            ui.label(RichText::new(p).color(theme::TEXT_WEAK));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let import =
                ui.add_enabled(problem.is_none(), egui::Button::new(RichText::new("Import").size(15.0)));
            if import.clicked() {
                commands.push(Command::StartImport);
            }
            if ui.button("Cancel").clicked() {
                commands.push(Command::CloseImport);
            }
        });
    });
}
