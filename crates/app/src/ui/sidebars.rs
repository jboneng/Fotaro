//! Left (Navigator, Catalog, Folders, Collections) and right (Histogram,
//! Keywording, Keyword List, Metadata) panel stacks.

use catalog::{Flag, Folder, Source};
use eframe::egui::{self, Align, Color32, Layout, RichText, Sense};
use imaging::PreviewSize;

use super::panels::Side;
use super::theme;
use crate::app::{Command, MetadataPreset, PhotoLibraryApp};
use crate::platform;

const LEFT: &[&str] = &["Navigator", "Catalog", "Folders", "Collections"];
const RIGHT: &[&str] = &["Histogram", "Keywording", "Keyword List", "Metadata"];

fn weak(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::TEXT_WEAK)
}

/// A full-width selectable row with a right-aligned count, like LrC's panel rows.
fn source_row(
    ui: &mut egui::Ui,
    indent: f32,
    label: &str,
    count: Option<i64>,
    selected: bool,
) -> egui::Response {
    let height = 19.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x55, 0x55, 0x55));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x3d, 0x3d, 0x3d));
    }
    let color = if selected { theme::TEXT_STRONG } else { theme::TEXT };
    ui.painter().text(
        rect.left_center() + egui::vec2(indent, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.5),
        color,
    );
    if let Some(n) = count {
        ui.painter().text(
            rect.right_center() - egui::vec2(4.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            n.to_string(),
            egui::FontId::proportional(11.5),
            theme::TEXT_WEAK,
        );
    }
    resp
}

pub fn left(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut stack = std::mem::take(&mut app.ui.left);
        stack.section(ui, Side::Left, "Navigator", LEFT, true, |ui| {
            super::layout::timed("navigator", || navigator(app, ui))
        });
        stack.section(ui, Side::Left, "Catalog", LEFT, true, |ui| catalog_section(app, ui));
        stack.section(ui, Side::Left, "Folders", LEFT, true, |ui| {
            super::layout::timed("folders", || folders(app, ui))
        });
        stack.section(ui, Side::Left, "Collections", LEFT, true, |ui| {
            ui.horizontal(|ui| {
                ui.label(weak("No collections yet."));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("+").on_hover_text("Create Collection…").clicked() {
                        app.run(Command::Planned("Collections", "Phase 4"));
                    }
                });
            });
        });
        app.ui.left = stack;
    });
}

fn navigator(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, width * 0.66), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x22, 0x22, 0x22));
    let Some(s) = &mut app.session else { return };
    let Some(photo) = s.active_photo().cloned() else { return };
    let tex = s.previews.get(photo.id).or_else(|| s.thumbs.get(photo.id));
    match tex {
        Some(tex) => {
            let img_rect = super::views::fit_rect(tex.size_vec2(), rect.shrink(4.0));
            ui.painter().image(
                tex.id(),
                img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => s.request(&photo, PreviewSize::Thumb),
    }
}

fn catalog_section(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let Some(s) = &app.session else { return };
    let (all, prev) = s.counts;
    let source = s.source;
    if source_row(ui, 4.0, "All Photographs", Some(all), source == Source::AllPhotographs).clicked() {
        app.run(Command::SetSource(Source::AllPhotographs));
    }
    if source_row(ui, 4.0, "Quick Collection +", Some(0), false).clicked() {
        app.run(Command::Planned("Quick Collection", "Phase 4"));
    }
    if source_row(ui, 4.0, "Previous Import", Some(prev), source == Source::PreviousImport).clicked() {
        app.run(Command::SetSource(Source::PreviousImport));
    }
}

fn folders(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    if ui.button("+  Add Folder…").clicked() {
        app.run(Command::Action(super::shortcuts::Action::Import));
    }
    ui.add_space(4.0);
    let Some(s) = &mut app.session else { return };
    if s.folders.is_empty() {
        ui.label(weak("Add a folder of photos to get started."));
        return;
    }

    // The tree structure is computed once per catalog change (see `FolderTree`).
    let tree = &s.folder_tree;
    let folders = &s.folders;

    let selected = match s.source {
        Source::Folder { id, .. } => Some(id),
        _ => None,
    };
    let include_subfolders = app.ui.include_subfolders;
    let mut commands = Vec::new();
    let mut toggles = Vec::new();

    for (key, name, roots) in &tree.volumes {
        // Volume header: name and free space.
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).color(theme::TEXT_STRONG).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(free) = s.free_space.get(key) {
                    ui.label(weak(format!("{} free", platform::human_bytes(free))).size(10.5));
                }
            });
        });

        let mut stack: Vec<(&Folder, usize)> = roots.iter().rev().map(|&i| (&folders[i], 0)).collect();
        while let Some((f, depth)) = stack.pop() {
            let kids = tree.children.get(&Some(f.id));
            let has_kids = kids.is_some_and(|k| !k.is_empty());
            let expanded = !s.collapsed_folders.contains(&f.id);
            let name = f
                .path
                .file_name()
                .map_or_else(|| f.path.display().to_string(), |n| n.to_string_lossy().into_owned());
            let tri = match (has_kids, expanded) {
                (false, _) => "   ",
                (true, true) => "▼ ",
                (true, false) => "▶ ",
            };
            let indent = 4.0 + depth as f32 * 14.0;
            let count = if include_subfolders { tree.totals[&f.id] } else { f.photo_count };
            let resp = source_row(ui, indent, &format!("{tri}{name}"), Some(count), selected == Some(f.id));
            let resp = resp.on_hover_text(f.path.display().to_string());
            if resp.clicked() {
                let tri_hit =
                    resp.interact_pointer_pos().is_some_and(|p| p.x < resp.rect.left() + indent + 12.0);
                if tri_hit && has_kids {
                    toggles.push(f.id);
                } else {
                    commands.push(Command::SetSource(Source::Folder { id: f.id, include_subfolders }));
                }
            }
            if resp.double_clicked() && has_kids {
                toggles.push(f.id);
            }
            resp.context_menu(|ui| {
                if ui.button("Synchronize Folder…").clicked() {
                    commands.push(Command::SyncFolder(f.id));
                    ui.close();
                }
                if ui.button(platform::REVEAL_LABEL).clicked() {
                    commands.push(Command::Reveal(f.path.clone()));
                    ui.close();
                }
                ui.separator();
                let _ = ui
                    .add_enabled(false, egui::Button::new("Remove…"))
                    .on_disabled_hover_text("Arrives in Phase 1");
            });
            if has_kids && expanded {
                for &k in kids.into_iter().flatten().rev() {
                    stack.push((&folders[k], depth + 1));
                }
            }
        }
        ui.add_space(4.0);
    }

    for id in toggles {
        if !s.collapsed_folders.remove(&id) {
            s.collapsed_folders.insert(id);
        }
    }
    for c in commands {
        app.run(c);
    }
}

pub fn right(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut stack = std::mem::take(&mut app.ui.right);
        stack.section(ui, Side::Right, "Histogram", RIGHT, true, |ui| {
            super::layout::timed("histogram", || histogram(app, ui))
        });
        stack.section(ui, Side::Right, "Keywording", RIGHT, false, |ui| {
            ui.label(weak("Keyword tags arrive in Phase 3."));
        });
        stack.section(ui, Side::Right, "Keyword List", RIGHT, false, |ui| {
            ui.label(weak("Keyword list arrives in Phase 3."));
        });
        stack.section(ui, Side::Right, "Metadata", RIGHT, true, |ui| {
            super::layout::timed("metadata", || metadata(app, ui))
        });
        app.ui.right = stack;
    });
}

fn histogram(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 90.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(0x22, 0x22, 0x22));
    let Some(s) = &mut app.session else { return };
    let Some(id) = s.active else { return };

    if let Some(h) = s.histograms.get(&id) {
        // Ignore the extreme bins when scaling so clipped highlights don't flatten the curve.
        let peak = h.iter().flat_map(|c| c[1..255].iter()).copied().max().unwrap_or(1).max(1) as f32;
        let colors = [
            Color32::from_rgba_unmultiplied(220, 70, 70, 110),
            Color32::from_rgba_unmultiplied(80, 200, 80, 110),
            Color32::from_rgba_unmultiplied(80, 120, 230, 110),
        ];
        for (channel, color) in h.iter().zip(colors) {
            let mut points = vec![rect.left_bottom()];
            for (i, &v) in channel.iter().enumerate() {
                let x = rect.left() + rect.width() * i as f32 / 255.0;
                let y = rect.bottom() - rect.height() * (v as f32 / peak).min(1.0) * 0.95;
                points.push(egui::pos2(x, y));
            }
            points.push(rect.right_bottom());
            // Draw as thin vertical bars (robust for non-convex shapes).
            for w in points.windows(2) {
                painter.line_segment(
                    [egui::pos2(w[1].x, rect.bottom()), w[1]],
                    egui::Stroke::new(rect.width() / 255.0 + 0.5, color),
                );
            }
        }
    }

    // Exposure summary under the histogram, as in LrC.
    if let Some(m) = s.active_metadata() {
        let mut parts = Vec::new();
        if let Some(iso) = m.iso {
            parts.push(format!("ISO {iso:.0}"));
        }
        if let Some(f) = m.focal_len {
            parts.push(format!("{f:.0} mm"));
        }
        if let Some(a) = m.aperture {
            parts.push(format!("f / {a:.1}"));
        }
        if let Some(t) = m.shutter {
            parts.push(format_shutter(t));
        }
        if !parts.is_empty() {
            ui.vertical_centered(|ui| ui.label(RichText::new(parts.join("    ")).size(11.0)));
        }
    }
}

pub fn format_shutter(seconds: f64) -> String {
    if seconds >= 0.5 {
        format!("{seconds:.1} sec").replace(".0 sec", " sec")
    } else {
        format!("1/{:.0} sec", 1.0 / seconds)
    }
}

pub fn format_capture_time(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.naive_utc().format("%b %-d, %Y %-I:%M:%S %p").to_string())
        .unwrap_or_default()
}

fn metadata(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let preset = &mut app.ui.metadata_preset;
    ui.horizontal(|ui| {
        ui.label(weak("Preset"));
        egui::ComboBox::from_id_salt("metadata_preset")
            .selected_text(match preset {
                MetadataPreset::Default => "Default",
                MetadataPreset::Exif => "EXIF",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(preset, MetadataPreset::Default, "Default");
                ui.selectable_value(preset, MetadataPreset::Exif, "EXIF");
            });
    });
    ui.add_space(4.0);
    let preset = *preset;

    let Some(s) = &mut app.session else { return };
    let Some(photo) = s.active_photo().cloned() else {
        ui.label(weak("No photo selected"));
        return;
    };
    let m = s.active_metadata().unwrap_or_default();

    let mut rows: Vec<(&str, String)> = Vec::new();
    rows.push(("File Name", photo.file_name.clone()));
    rows.push((
        "Folder",
        photo
            .path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ));
    if photo.missing {
        rows.push(("Status", "File is missing".into()));
    }
    match preset {
        MetadataPreset::Default => {
            rows.push((
                "Rating",
                match photo.rating {
                    0 => "None".into(),
                    1 => "1 Star".into(),
                    n => format!("{n} Stars"),
                },
            ));
            rows.push((
                "Label",
                photo
                    .color_label
                    .clone()
                    .map(|l| {
                        let mut l = l;
                        l[..1].make_ascii_uppercase();
                        l
                    })
                    .unwrap_or_default(),
            ));
            rows.push((
                "Pick",
                match photo.flag {
                    Flag::Picked => "Flagged",
                    Flag::Rejected => "Rejected",
                    Flag::Unflagged => "Unflagged",
                }
                .into(),
            ));
            rows.push(("Capture Time", photo.capture_time.map(format_capture_time).unwrap_or_default()));
            if let (Some(w), Some(h)) = (m.width, m.height) {
                rows.push(("Dimensions", format!("{w} x {h}")));
            }
            rows.push((
                "Camera",
                [m.make.clone(), m.model.clone()].into_iter().flatten().collect::<Vec<_>>().join(" "),
            ));
        }
        MetadataPreset::Exif => {
            rows.push(("File Size", platform::human_bytes(photo.size_bytes)));
            rows.push((
                "File Type",
                photo.path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default(),
            ));
            if let (Some(w), Some(h)) = (m.width, m.height) {
                rows.push(("Dimensions", format!("{w} x {h}")));
            }
            rows.push((
                "Date Time Original",
                photo.capture_time.map(format_capture_time).unwrap_or_default(),
            ));
            let exposure = match (m.shutter, m.aperture) {
                (Some(t), Some(a)) => format!("{} at ƒ / {a:.1}", format_shutter(t)),
                (Some(t), None) => format_shutter(t),
                _ => String::new(),
            };
            rows.push(("Exposure", exposure));
            rows.push(("Focal Length", m.focal_len.map(|f| format!("{f:.0} mm")).unwrap_or_default()));
            rows.push(("ISO Speed Rating", m.iso.map(|i| format!("ISO {i:.0}")).unwrap_or_default()));
            rows.push(("Make", m.make.clone().unwrap_or_default()));
            rows.push(("Model", m.model.clone().unwrap_or_default()));
            rows.push(("Lens", m.lens.clone().unwrap_or_default()));
        }
    }

    egui::Grid::new("metadata_grid").num_columns(2).spacing([10.0, 3.0]).striped(false).show(ui, |ui| {
        for (k, v) in rows {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(weak(k).size(11.5)));
            ui.label(RichText::new(v).size(11.5));
            ui.end_row();
        }
    });
}
