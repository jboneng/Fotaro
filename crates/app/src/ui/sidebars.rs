//! Left (Navigator, Catalog, Folders, Collections) and right (Histogram,
//! Keywording, Keyword List, Metadata) panel stacks.

use catalog::{Flag, Source};
use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Sense};
use imaging::PreviewSize;

use super::panels::Side;
use super::theme;
use crate::app::{Command, FieldValue, FotaroApp, MetadataPreset, View, ZoomLevel};
use crate::platform;
use catalog::IptcField;

const LEFT: &[&str] = &["Navigator", "Catalog", "Folders", "Collections"];
pub const RIGHT: &[&str] = &["Histogram", "Keywording", "Keyword List", "Metadata"];

fn weak(text: impl Into<String>) -> RichText {
    RichText::new(text).color(theme::TEXT_WEAK)
}

/// A full-width selectable row with a right-aligned count, like LrC's panel rows.
pub(super) fn source_row(
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

pub fn left(app: &mut FotaroApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut stack = std::mem::take(&mut app.ui.left);
        stack.section(ui, Side::Left, "Navigator", LEFT, true, |ui| {
            super::layout::timed("navigator", || navigator(app, ui, None))
        });
        stack.section(ui, Side::Left, "Catalog", LEFT, true, |ui| catalog_section(app, ui));
        stack.section(ui, Side::Left, "Folders", LEFT, true, |ui| {
            super::layout::timed("folders", || folders(app, ui))
        });
        stack.section(ui, Side::Left, "Collections", LEFT, true, |ui| {
            super::layout::timed("collections", || super::collections::panel(app, ui))
        });
        app.ui.left = stack;
    });
}

/// LrC's Navigator: the active photo, the zoom level picker (FIT FILL 1:1 2:1 3:1), and
/// while zoomed a rectangle showing the visible area; click or drag to move it.
/// `shown`: the image to show instead of the photo's thumbnail (Develop's live render).
pub(super) fn navigator(app: &mut FotaroApp, ui: &mut egui::Ui, shown: Option<egui::TextureHandle>) {
    let fit_selected = !(app.zoom.zoomed && matches!(app.view, View::Loupe | View::Compare));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for level in ZoomLevel::ALL.iter().rev() {
                let on = !fit_selected && app.ui.zoom_level == *level;
                if zoom_choice(ui, level.label(), on).clicked() {
                    app.ui.zoom_level = *level;
                    if app.session.as_ref().and_then(|s| s.active).is_some() {
                        app.zoom.zoomed = true;
                        app.view = View::Loupe;
                    }
                }
            }
            if zoom_choice(ui, "FIT", fit_selected).clicked() {
                app.zoom.zoomed = false;
            }
        });
    });

    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, width * 0.66), Sense::click_and_drag());
    ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x22, 0x22, 0x22));
    let Some(s) = &mut app.session else { return };
    let Some(photo) = s.active_photo().cloned() else { return };
    // The thumbnail is about the Navigator's size; the 2048 px preview would alias.
    let Some(tex) = shown.or_else(|| s.thumbs.get(photo.id)).or_else(|| s.previews.get(photo.id)) else {
        if !photo.missing {
            s.request(&photo, PreviewSize::Thumb);
        }
        return;
    };
    let size = super::views::rotated_size(tex.size_vec2(), photo.rotation);
    let img_rect = super::views::fit_rect(size, rect.shrink(4.0));
    super::views::paint_image(ui.painter(), &tex, img_rect, photo.rotation, Color32::WHITE);

    // Click / drag: zoom in (if needed) and center the Loupe on that point.
    if let Some(p) = resp.interact_pointer_pos()
        && (resp.clicked() || resp.dragged())
    {
        let q = ((p - img_rect.min) / img_rect.size()).clamp(egui::Vec2::ZERO, egui::Vec2::splat(1.0));
        app.zoom.center = q;
        app.zoom.anchor = None;
        app.zoom.zoomed = true;
        app.view = View::Loupe;
    }
    // The visible area, from the Loupe's last frame.
    if app.zoom.zoomed
        && matches!(app.view, View::Loupe | View::Compare)
        && let Some(v) = app.zoom.visible
    {
        let r = Rect::from_min_max(
            img_rect.min + v.min.to_vec2() * img_rect.size(),
            img_rect.min + v.max.to_vec2() * img_rect.size(),
        );
        ui.painter().rect_filled(r, 0.0, Color32::from_white_alpha(18));
        ui.painter().rect_stroke(r, 0.0, egui::Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Inside);
    }
}

fn zoom_choice(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let color = if on { theme::TEXT_STRONG } else { theme::TEXT_WEAK };
    ui.add(egui::Label::new(egui::RichText::new(text).size(11.0).color(color)).sense(Sense::click()))
}

fn catalog_section(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &app.session else { return };
    let (all, prev) = s.counts;
    let quick = s.quick_count;
    let source = s.source;
    if source_row(ui, 4.0, "All Photographs", Some(all), source == Source::AllPhotographs).clicked() {
        app.run(Command::SetSource(Source::AllPhotographs));
    }
    let resp = source_row(ui, 4.0, "Quick Collection +", Some(quick), source == Source::QuickCollection)
        .on_hover_text("B adds or removes the selected photos; drag photos here too");
    if resp.clicked() {
        app.run(Command::SetSource(Source::QuickCollection));
    }
    if let Some(photos) = super::collections::accept_photos(&resp, ui) {
        app.run(Command::QuickCollectionAdd(photos));
    }
    if source_row(ui, 4.0, "Previous Import", Some(prev), source == Source::PreviousImport).clicked() {
        app.run(Command::SetSource(Source::PreviousImport));
    }
}

fn folders(app: &mut FotaroApp, ui: &mut egui::Ui) {
    // key: library.folders.add
    if ui.button("+  Add Folder…").on_hover_text("Import ▸ Add: catalog photos where they are").clicked()
    {
        app.run(Command::OpenImport(Some(catalog::ImportMode::Add)));
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

    // A filter: the folders whose names have the text, with the folders above them.
    let filter_id = egui::Id::unique("folder_filter");
    let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    if folders.len() > 6 || !filter.is_empty() {
        ui.add(
            egui::TextEdit::singleline(&mut filter).hint_text("Filter Folders").desired_width(f32::INFINITY),
        );
        ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
        ui.add_space(2.0);
    }
    let needle = filter.trim().to_lowercase();
    let mut shown = vec![needle.is_empty(); tree.nodes.len()];
    if !needle.is_empty() {
        // Children come after their parents: mark matches, then their parents, from the end.
        for n in (0..tree.nodes.len()).rev() {
            let node = &tree.nodes[n];
            let hit =
                node.name.to_lowercase().contains(&needle) || node.prefix.to_lowercase().contains(&needle);
            shown[n] = hit || node.children.iter().any(|c| shown[*c]);
        }
    }

    let selected = match s.source {
        Source::Folder { id, .. } => Some(id),
        _ => None,
    };
    let include_subfolders = app.ui.include_subfolders;
    let mut commands = Vec::new();
    let mut toggles: Vec<(Option<i64>, std::path::PathBuf)> = Vec::new();
    let mut none_shown = true;

    for (key, name, tops) in &tree.volumes {
        if !tops.iter().any(|t| shown[*t]) {
            continue;
        }
        none_shown = false;
        // Volume header: name and free space.
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).color(theme::TEXT_STRONG).strong()).on_hover_text(key.as_str());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(free) = s.free_space.get(key) {
                    ui.label(weak(format!("{} free", platform::human_bytes(free))).size(10.5));
                }
            });
        });

        let mut stack: Vec<(usize, usize)> = tops.iter().rev().map(|&n| (n, 0)).collect();
        while let Some((n, depth)) = stack.pop() {
            if !shown[n] {
                continue;
            }
            let node = &tree.nodes[n];
            let folder = node.folder.map(|i| &folders[i]);
            let has_kids = node.children.iter().any(|c| shown[*c]);
            let expanded = !needle.is_empty()
                || match folder {
                    Some(f) => !s.collapsed_folders.contains(&f.id),
                    None => !s.collapsed_groups.contains(&node.path),
                };
            let count = match folder {
                Some(f) if !include_subfolders => f.photo_count,
                _ => node.total,
            };
            let missing = folder.is_some_and(|f| s.missing_folders.contains(&f.id));
            let row = FolderRow {
                depth,
                kids: has_kids.then_some(expanded),
                prefix: &node.prefix,
                name: &node.name,
                count,
                selected: folder.is_some_and(|f| selected == Some(f.id)),
                group: folder.is_none(),
                missing,
            };
            let (resp, on_triangle) = folder_row(ui, &row);
            let tip = match (folder, missing) {
                (_, true) => format!("{}\nThis folder could not be found.", node.path.display()),
                (None, _) => {
                    format!("{}\nNot a catalog folder: the folders under it are.", node.path.display())
                }
                (Some(_), false) => node.path.display().to_string(),
            };
            let resp = resp.on_hover_text(tip);
            let toggle = (folder.map(|f| f.id), node.path.clone());
            if resp.clicked() {
                match folder {
                    Some(f) if !(on_triangle && has_kids) => {
                        commands.push(Command::SetSource(Source::Folder { id: f.id, include_subfolders }));
                    }
                    _ if has_kids => toggles.push(toggle.clone()),
                    _ => {}
                }
            }
            if resp.double_clicked() && has_kids && folder.is_some() {
                toggles.push(toggle);
            }
            match folder {
                Some(f) => folder_menu(&resp, f, &node.name, missing, &mut commands),
                None => {
                    resp.context_menu(|ui| {
                        // no hotkey: a folder's own menu
                        if ui.button(platform::REVEAL_LABEL).clicked() {
                            commands.push(Command::Reveal(node.path.clone()));
                            ui.close();
                        }
                    });
                }
            }
            if has_kids && expanded {
                for &k in node.children.iter().rev() {
                    stack.push((k, depth + 1));
                }
            }
        }
        ui.add_space(4.0);
    }
    if none_shown {
        ui.label(weak("No folder by that name."));
    }

    for (id, path) in toggles {
        match id {
            Some(id) => {
                if !s.collapsed_folders.remove(&id) {
                    s.collapsed_folders.insert(id);
                }
            }
            None => {
                if !s.collapsed_groups.remove(&path) {
                    s.collapsed_groups.insert(path);
                }
            }
        }
    }
    for c in commands {
        app.run(c);
    }
}

/// What a Folders panel row shows.
struct FolderRow<'a> {
    depth: usize,
    /// Has folders under it: expanded or not.
    kids: Option<bool>,
    /// Folders above it with nothing else in them (dimmer, shortened first when narrow).
    prefix: &'a str,
    name: &'a str,
    count: i64,
    selected: bool,
    /// Not a catalog folder: only shows where the ones under it are.
    group: bool,
    missing: bool,
}

/// A Folders panel row: a triangle (in its own column, so names line up), the name
/// shortened to fit before the count, and the count. Also returns whether a click was on
/// the triangle.
fn folder_row(ui: &mut egui::Ui, row: &FolderRow) -> (egui::Response, bool) {
    let height = 19.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
    let painter = ui.painter_at(rect);
    if row.selected {
        painter.rect_filled(rect, 0.0, Color32::from_rgb(0x55, 0x55, 0x55));
    } else if resp.hovered() {
        painter.rect_filled(rect, 0.0, Color32::from_rgb(0x3d, 0x3d, 0x3d));
    }
    let x = rect.left() + 4.0 + row.depth as f32 * 12.0;
    let tri = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(12.0, height));
    if let Some(open) = row.kids {
        let c = tri.center();
        let points = if open {
            vec![c + egui::vec2(-3.5, -2.0), c + egui::vec2(3.5, -2.0), c + egui::vec2(0.0, 2.5)]
        } else {
            vec![c + egui::vec2(-2.0, -3.5), c + egui::vec2(2.5, 0.0), c + egui::vec2(-2.0, 3.5)]
        };
        painter.add(egui::Shape::convex_polygon(points, theme::TEXT_WEAK, egui::Stroke::NONE));
    }
    // The count, then the name in what is left.
    let small = egui::FontId::proportional(11.5);
    let count = painter.text(
        rect.right_center() - egui::vec2(4.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        row.count.to_string(),
        small,
        theme::TEXT_WEAK,
    );
    let font = egui::FontId::proportional(12.5);
    let room = (count.left() - 8.0 - (tri.right() + 2.0)).max(10.0);
    let name_color = if row.group {
        theme::TEXT_WEAK
    } else if row.selected {
        theme::TEXT_STRONG
    } else {
        theme::TEXT
    };
    let width = |text: &str| {
        ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x)
    };
    let name = if row.missing { format!("{}  ?", row.name) } else { row.name.to_owned() };
    // The prefix whole if it fits, else its end, else none.
    let mut prefix = if row.prefix.is_empty() { String::new() } else { format!("{} / ", row.prefix) };
    while !prefix.is_empty() && width(&prefix) + width(&name) > room {
        let parts: Vec<&str> =
            prefix.trim_start_matches("… / ").trim_end_matches(" / ").split(" / ").collect();
        prefix = if parts.len() > 1 { format!("… / {} / ", parts[1..].join(" / ")) } else { String::new() };
    }
    let mut job = egui::text::LayoutJob::default();
    let dim = theme::TEXT_WEAK.gamma_multiply(0.85);
    job.append(&prefix, 0.0, egui::TextFormat::simple(font.clone(), dim));
    job.append(&name, 0.0, egui::TextFormat::simple(font.clone(), name_color));
    job.wrap = egui::text::TextWrapping {
        max_width: room,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
    let at = egui::pos2(tri.right() + 2.0, rect.center().y - galley.size().y / 2.0);
    painter.galley(at, galley, name_color);
    if row.missing {
        // LrC greys out folders it can't find (e.g. an unplugged drive).
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(90));
    }
    let on_triangle = resp.interact_pointer_pos().is_some_and(|p| p.x < tri.right() + 2.0);
    (resp, on_triangle)
}

/// A catalog folder's menu: the same items and order as LrC's Folders panel.
fn folder_menu(
    resp: &egui::Response,
    f: &catalog::Folder,
    name: &str,
    missing: bool,
    commands: &mut Vec<Command>,
) {
    resp.context_menu(|ui| {
        let mut item = |ui: &mut egui::Ui, text: &str, enabled: bool, cmd: Command| {
            // no hotkey: a folder's own menu
            if ui.add_enabled(enabled, egui::Button::new(text)).clicked() {
                commands.push(cmd);
                ui.close();
            }
        };
        if missing {
            item(ui, "Find Missing Folder…", true, Command::LocateFolder(f.id));
            ui.separator();
        }
        item(ui, &format!("Create Folder inside \"{name}\"…"), !missing, Command::CreateFolderIn(f.id));
        item(ui, "Rename…", !missing, Command::RenameFolder(f.id));
        item(ui, "Remove…", true, Command::RemoveFolder(f.id));
        ui.separator();
        item(ui, "Synchronize Folder…", !missing, Command::SyncFolder(f.id));
        if !missing {
            item(ui, "Update Folder Location…", true, Command::LocateFolder(f.id));
        }
        item(ui, "Show Parent Folder", f.parent_id.is_none(), Command::ShowParentFolder(f.id));
        ui.separator();
        item(ui, platform::REVEAL_LABEL, !missing, Command::Reveal(f.path.clone()));
    });
}

pub fn right(app: &mut FotaroApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut stack = std::mem::take(&mut app.ui.right);
        stack.section(ui, Side::Right, "Histogram", RIGHT, true, |ui| {
            super::layout::timed("histogram", || histogram(app, ui))
        });
        stack.section(ui, Side::Right, "Keywording", RIGHT, true, |ui| super::keywords::keywording(app, ui));
        stack.section(ui, Side::Right, "Keyword List", RIGHT, true, |ui| {
            super::layout::timed("keyword list", || super::keywords::keyword_list(app, ui))
        });
        stack.section(ui, Side::Right, "Metadata", RIGHT, true, |ui| {
            super::layout::timed("metadata", || metadata(app, ui))
        });
        app.ui.right = stack;
    });
}

fn histogram(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else {
        paint_histogram(ui, None);
        return;
    };
    paint_histogram(ui, s.active.and_then(|id| s.histograms.get(&id)).map(|h| &**h));
    exposure_summary(app, ui);
}

/// The histogram graph (R, G, B overlaid), or an empty box.
pub(super) fn paint_histogram(ui: &mut egui::Ui, h: Option<&crate::jobs::Histogram>) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 90.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(0x22, 0x22, 0x22));
    if let Some(h) = h {
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
}

/// The exposure summary under the histogram, as in LrC ("ISO 100  50 mm  f / 2.8  1/250").
pub(super) fn exposure_summary(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
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

fn metadata(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let preset = &mut app.ui.metadata_preset;
    let name = |p: MetadataPreset| match p {
        MetadataPreset::Default => "Default",
        MetadataPreset::Exif => "EXIF",
        MetadataPreset::Iptc => "IPTC",
    };
    ui.horizontal(|ui| {
        ui.label(weak("Preset"));
        egui::ComboBox::from_id_salt("metadata_preset").selected_text(name(*preset)).show_ui(ui, |ui| {
            for p in [MetadataPreset::Default, MetadataPreset::Exif, MetadataPreset::Iptc] {
                // key: library.metadata.*
                ui.selectable_value(preset, p, name(p));
            }
        });
    });
    ui.add_space(4.0);
    let preset = *preset;
    let view = app.view;

    let Some(s) = &mut app.session else { return };
    let Some(photo) = s.active_photo().cloned() else {
        ui.label(weak("No photo selected"));
        return;
    };
    let m = s.active_metadata().unwrap_or_default();
    let selection = s.selection_meta(view).clone();
    let n = selection.ids.len();
    if n > 1 {
        ui.label(weak(format!("Editing {n} photos")).size(11.0));
    }

    // Rows are either read-only text or an editable IPTC field.
    enum Row {
        Text(&'static str, String),
        Edit(IptcField),
    }
    let mut rows: Vec<Row> = Vec::new();
    let text = |k: &'static str, v: String| Row::Text(k, v);
    rows.push(text("File Name", photo.file_name.clone()));
    if let Some(copy) = &photo.virtual_copy {
        rows.push(text("Copy Name", copy.clone()));
    }
    if preset != MetadataPreset::Iptc {
        rows.push(text(
            "Folder",
            photo
                .path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ));
    }
    if photo.missing {
        rows.push(text("Status", "File is missing".into()));
    }
    match preset {
        MetadataPreset::Default => {
            rows.extend(IptcField::ALL.map(Row::Edit));
            rows.push(text(
                "Rating",
                match photo.rating {
                    0 => "None".into(),
                    1 => "1 Star".into(),
                    n => format!("{n} Stars"),
                },
            ));
            rows.push(text(
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
            rows.push(text(
                "Pick",
                match photo.flag {
                    Flag::Picked => "Flagged",
                    Flag::Rejected => "Rejected",
                    Flag::Unflagged => "Unflagged",
                }
                .into(),
            ));
            rows.push(text("Capture Time", photo.capture_time.map(format_capture_time).unwrap_or_default()));
            if let Some(d) = dimensions(&m, photo.rotation) {
                rows.push(text("Dimensions", d));
            }
            rows.push(text(
                "Camera",
                [m.make.clone(), m.model.clone()].into_iter().flatten().collect::<Vec<_>>().join(" "),
            ));
            if let Some(gps) = format_gps(&m) {
                rows.push(text("GPS", gps));
            }
        }
        MetadataPreset::Exif => {
            rows.push(text("File Size", platform::human_bytes(photo.size_bytes)));
            rows.push(text(
                "File Type",
                photo.path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default(),
            ));
            if let Some(d) = dimensions(&m, photo.rotation) {
                rows.push(text("Dimensions", d));
            }
            rows.push(text(
                "Date Time Original",
                photo.capture_time.map(format_capture_time).unwrap_or_default(),
            ));
            let exposure = match (m.shutter, m.aperture) {
                (Some(t), Some(a)) => format!("{} at ƒ / {a:.1}", format_shutter(t)),
                (Some(t), None) => format_shutter(t),
                _ => String::new(),
            };
            rows.push(text("Exposure", exposure));
            rows.push(text("Focal Length", m.focal_len.map(|f| format!("{f:.0} mm")).unwrap_or_default()));
            rows.push(text("ISO Speed Rating", m.iso.map(|i| format!("ISO {i:.0}")).unwrap_or_default()));
            rows.push(text("Make", m.make.clone().unwrap_or_default()));
            rows.push(text("Model", m.model.clone().unwrap_or_default()));
            rows.push(text("Lens", m.lens.clone().unwrap_or_default()));
            if let Some(gps) = format_gps(&m) {
                rows.push(text("GPS", gps));
            }
            if let Some(alt) = m.gps_alt.filter(|_| m.gps_lat.is_some()) {
                rows.push(text("Altitude", format!("{alt:.0} m")));
            }
        }
        MetadataPreset::Iptc => rows.extend(IptcField::ALL.map(Row::Edit)),
    }

    let mut commands = Vec::new();
    egui::Grid::new("metadata_grid").num_columns(2).spacing([10.0, 3.0]).striped(false).show(ui, |ui| {
        for row in rows {
            match row {
                Row::Text(k, v) => {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(weak(k).size(11.5)));
                    ui.label(RichText::new(v).size(11.5));
                }
                Row::Edit(field) => {
                    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                        ui.add_space(2.0);
                        ui.label(weak(field.label()).size(11.5))
                    });
                    let value =
                        selection.iptc.get(&field).cloned().unwrap_or(FieldValue::Same(String::new()));
                    let (current, hint) = match value {
                        FieldValue::Same(v) => (v, ""),
                        FieldValue::Mixed => (String::new(), "< mixed >"),
                    };
                    let id = egui::Id::unique(("iptc", field));
                    let multiline = field == IptcField::Caption;
                    let edited = super::keywords::commit_on_blur(ui, id, &current, |t| {
                        let edit = if multiline {
                            egui::TextEdit::multiline(t).desired_rows(2)
                        } else {
                            egui::TextEdit::singleline(t)
                        };
                        edit.hint_text(hint).desired_width(f32::INFINITY).font(egui::TextStyle::Small)
                    });
                    if let Some((_, value)) = edited {
                        commands.push(Command::SetIptc { field, value });
                    }
                }
            }
            ui.end_row();
        }
    });
    for c in commands {
        app.run(c);
    }
}

/// "59°54'50" N 10°45'8" E", as LrC writes positions.
fn format_gps(m: &catalog::PhotoMetadata) -> Option<String> {
    let (lat, lon) = (m.gps_lat?, m.gps_lon?);
    let dms = |v: f64, pos: char, neg: char| {
        let a = v.abs();
        let total = (a * 3600.0).round() as u64;
        let (d, m, s) = (total / 3600, total % 3600 / 60, total % 60);
        format!("{d}°{m}'{s}\" {}", if v < 0.0 { neg } else { pos })
    };
    Some(format!("{} {}", dms(lat, 'N', 'S'), dms(lon, 'E', 'W')))
}

/// "6032 x 4032" as displayed: swapped when the photo is rotated a quarter turn.
pub(super) fn dimensions(m: &catalog::PhotoMetadata, rotation: u8) -> Option<String> {
    let (w, h) = (m.width?, m.height?);
    let (w, h) = if rotation % 2 == 1 { (h, w) } else { (w, h) };
    Some(format!("{w} x {h}"))
}

#[cfg(test)]
mod gps_tests {
    #[test]
    fn positions_read_like_lightroom() {
        let m =
            catalog::PhotoMetadata { gps_lat: Some(59.9139), gps_lon: Some(-10.7522), ..Default::default() };
        assert_eq!(super::format_gps(&m).as_deref(), Some("59°54'50\" N 10°45'8\" W"));
        assert_eq!(super::format_gps(&catalog::PhotoMetadata::default()), None);
    }
}
