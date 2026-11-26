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

pub(crate) const LEFT: &[&str] = &["Navigator", "Catalog", "Folders", "Collections"];
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
    let ctx = ui.ctx().clone();
    let name_of = |i: usize| crate::app::folder_name(&folders[i].path);
    let kids_of = |i: usize| tree.children.get(&Some(folders[i].id)).map_or(&[][..], Vec::as_slice);

    // A filter: the folders whose names (or places) have the text, with those above them.
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
    let filtering = !needle.is_empty();
    let mut shown = vec![!filtering; folders.len()];
    if filtering {
        // Folders are sorted by path: children after their parents.
        for i in (0..folders.len()).rev() {
            let caption = tree.captions.get(&i).map(|c| c.join(" ").to_lowercase()).unwrap_or_default();
            let hit = name_of(i).to_lowercase().contains(&needle) || caption.contains(&needle);
            shown[i] = hit || kids_of(i).iter().any(|k| shown[*k]);
        }
    }

    let selected = match s.source {
        Source::Folder { id, .. } => Some(id),
        _ => None,
    };
    let index: std::collections::HashMap<i64, usize> =
        folders.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
    let key_of = |i: usize| folders[i].path.to_string_lossy().into_owned();

    // A folder chosen elsewhere (Show Parent Folder, an import, a key) is shown: the
    // folders above it open, and the panel scrolls to it.
    let (last_id, clicked_id) = (egui::Id::unique("folders_last"), egui::Id::unique("folders_clicked"));
    let mut reveal = ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::unique(REVEAL)).unwrap_or(false));
    let last: Option<i64> = ctx.data(|d| d.get_temp(last_id)).flatten();
    let clicked: Option<i64> = ctx.data(|d| d.get_temp(clicked_id)).flatten();
    if selected != last {
        reveal |= selected.is_some() && selected != clicked;
        ctx.data_mut(|d| d.insert_temp(last_id, selected));
    }
    let mut scroll_to = None;
    if reveal && let Some(mut at) = selected.and_then(|id| index.get(&id).copied()) {
        scroll_to = Some(folders[at].id);
        let (volume, _) = platform::volume_of(&folders[at].path);
        app.ui.closed_volumes.remove(&volume);
        while let Some(p) = folders[at].parent_id.and_then(|p| index.get(&p).copied()) {
            app.ui.closed_folders.remove(&key_of(p));
            at = p;
        }
    }

    let include_subfolders = app.ui.include_subfolders;
    let mut commands = Vec::new();
    let mut toggles: Vec<String> = Vec::new();
    let mut set_all: Option<(usize, bool)> = None;
    let mut toggle_volume: Option<String> = None;
    let mut rows: Vec<i64> = Vec::new();
    let mut none_shown = true;

    for (key, name, tops) in &tree.volumes {
        if !tops.iter().any(|t| shown[*t]) {
            continue;
        }
        none_shown = false;
        let open = filtering || !app.ui.closed_volumes.contains(key);
        let network = key.starts_with(r"\\");
        if volume_header(ui, name, key, network, open, s.free_space.get(key)).clicked() {
            toggle_volume = Some(key.clone());
        }
        if !open {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = tops.iter().rev().map(|&i| (i, 0)).collect();
        while let Some((i, depth)) = stack.pop() {
            if !shown[i] {
                continue;
            }
            let f = &folders[i];
            let kids = kids_of(i);
            let has_kids = kids.iter().any(|k| shown[*k]);
            let expanded = filtering || !app.ui.closed_folders.contains(&key_of(i));
            let count = if include_subfolders { tree.totals[&f.id] } else { f.photo_count };
            let missing = s.missing_folders.contains(&f.id);
            let name = name_of(i);
            let row = FolderRow {
                depth,
                kids: has_kids.then_some(expanded),
                name: &name,
                caption: tree.captions.get(&i).map(Vec::as_slice).unwrap_or_default(),
                count,
                selected: selected == Some(f.id),
                missing,
            };
            let (resp, on_triangle) = folder_row(ui, &row);
            if scroll_to == Some(f.id) {
                resp.scroll_to_me(Some(Align::Center));
            }
            rows.push(f.id);
            let tip = if missing {
                format!("{}\nThis folder could not be found.", f.path.display())
            } else {
                f.path.display().to_string()
            };
            let resp = resp.on_hover_text(tip);
            if resp.clicked() {
                if on_triangle && has_kids {
                    toggles.push(key_of(i));
                } else {
                    ctx.data_mut(|d| d.insert_temp(clicked_id, Some(f.id)));
                    commands.push(Command::SetSource(Source::Folder { id: f.id, include_subfolders }));
                }
            }
            if resp.double_clicked() && has_kids {
                toggles.push(key_of(i));
            }
            if let Some(all) = folder_menu(&resp, f, &name, missing, !kids.is_empty(), &mut commands) {
                set_all = Some((i, all));
            }
            if has_kids && expanded {
                for &k in kids.iter().rev() {
                    stack.push((k, depth + 1));
                }
            }
        }
        ui.add_space(4.0);
    }
    if none_shown {
        ui.label(weak("No folder by that name."));
    }
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(ROWS), rows));

    if let Some(key) = toggle_volume
        && !app.ui.closed_volumes.remove(&key)
    {
        app.ui.closed_volumes.insert(key);
    }
    for key in toggles {
        if !app.ui.closed_folders.remove(&key) {
            app.ui.closed_folders.insert(key);
        }
    }
    if let Some((i, open)) = set_all {
        // The folder and every folder under it.
        let mut todo = vec![i];
        while let Some(at) = todo.pop() {
            if open {
                app.ui.closed_folders.remove(&key_of(at));
            } else if !kids_of(at).is_empty() {
                app.ui.closed_folders.insert(key_of(at));
            }
            todo.extend_from_slice(kids_of(at));
        }
    }
    for c in commands {
        app.run(c);
    }
}

/// Where the Folders panel keeps its rows in order (for Next / Previous Folder), and the
/// wish to show the selected folder.
const ROWS: &str = "folders_rows";
const REVEAL: &str = "folders_reveal";

/// The folders shown in the Folders panel last time it was drawn, top to bottom.
pub fn folder_rows(ctx: &egui::Context) -> Vec<i64> {
    ctx.data(|d| d.get_temp(egui::Id::unique(ROWS))).unwrap_or_default()
}

/// Show the selected folder in the Folders panel (open what is above it, scroll to it).
pub fn reveal_folder(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(REVEAL), true));
}

/// A volume's header: its icon, name, free space, and a bar of how full it is. Clicking
/// it opens or closes it.
fn volume_header(
    ui: &mut egui::Ui,
    name: &str,
    key: &str,
    network: bool,
    open: bool,
    space: Option<(u64, u64)>,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 27.0), Sense::click());
    let painter = ui.painter_at(rect);
    let bg = if resp.hovered() {
        Color32::from_rgb(0x3a, 0x3a, 0x3a)
    } else {
        Color32::from_rgb(0x2e, 0x2e, 0x2e)
    };
    painter.rect_filled(rect, 2.0, bg);
    let mid = rect.top() + 11.0;
    let tri = egui::pos2(rect.left() + 9.0, mid);
    let dir = if open { super::icons::Dir::Down } else { super::icons::Dir::Right };
    super::icons::triangle(&painter, tri, 3.5, dir, theme::TEXT_WEAK);
    let icon = egui::pos2(rect.left() + 25.0, mid);
    if network {
        super::icons::network_drive(&painter, icon, 5.5, theme::TEXT);
    } else {
        super::icons::drive(&painter, icon, 5.5, theme::TEXT);
    }
    let free_text = space.map(|(free, _)| format!("{} free", platform::human_bytes(free)));
    let free = free_text.as_ref().map(|t| {
        painter.text(
            egui::pos2(rect.right() - 6.0, mid),
            egui::Align2::RIGHT_CENTER,
            t,
            egui::FontId::proportional(10.5),
            theme::TEXT_WEAK,
        )
    });
    let room = free.map_or(rect.right(), |r| r.left()) - 8.0 - (icon.x + 10.0);
    let galley =
        one_line(ui, &[(name, theme::TEXT_STRONG)], egui::FontId::proportional(12.5), room.max(10.0));
    painter.galley(egui::pos2(icon.x + 10.0, mid - galley.size().y / 2.0), galley, theme::TEXT_STRONG);
    // How full: grey, amber under a tenth free, red under 3 %.
    if let Some((free, total)) = space.filter(|(_, t)| *t > 0) {
        let left = free as f32 / total as f32;
        let bar = egui::Rect::from_min_max(
            egui::pos2(icon.x - 6.0, rect.bottom() - 6.0),
            egui::pos2(rect.right() - 6.0, rect.bottom() - 3.5),
        );
        painter.rect_filled(bar, 1.0, Color32::from_rgb(0x22, 0x22, 0x22));
        let used = egui::Rect::from_min_max(
            bar.min,
            egui::pos2(bar.left() + bar.width() * (1.0 - left), bar.bottom()),
        );
        let color = if left < 0.03 {
            Color32::from_rgb(0xc8, 0x4a, 0x3a)
        } else if left < 0.10 {
            Color32::from_rgb(0xd0, 0x9a, 0x2a)
        } else {
            Color32::from_rgb(0x78, 0x78, 0x78)
        };
        painter.rect_filled(used, 1.0, color);
    }
    resp.on_hover_text(match space {
        Some((free, total)) if total > 0 => {
            format!("{key}\n{} free of {}", platform::human_bytes(free), platform::human_bytes(total))
        }
        _ => key.to_owned(),
    })
}

/// Text in one line at most `width` wide, ending in "…" when it does not fit.
fn one_line(
    ui: &egui::Ui,
    parts: &[(&str, Color32)],
    font: egui::FontId,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    for (text, color) in parts {
        job.append(text, 0.0, egui::TextFormat::simple(font.clone(), *color));
    }
    job.wrap = egui::text::TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.ctx().fonts_mut(|f| f.layout_job(job))
}

/// What a Folders panel row shows.
struct FolderRow<'a> {
    depth: usize,
    /// Has folders under it: expanded or not.
    kids: Option<bool>,
    name: &'a str,
    /// A top-level folder's place (the folders between its volume and it); empty: none.
    caption: &'a [String],
    count: i64,
    selected: bool,
    missing: bool,
}

/// A Folders panel row: a triangle (in its own column, so names line up), the folder's
/// icon and name, the count, and for a top-level folder a dim line saying where it is,
/// cut from the left when narrow. Also returns whether a click was on the triangle.
fn folder_row(ui: &mut egui::Ui, row: &FolderRow) -> (egui::Response, bool) {
    let height = if row.caption.is_empty() { 20.0 } else { 34.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
    let painter = ui.painter_at(rect);
    if row.selected {
        painter.rect_filled(rect, 2.0, Color32::from_rgb(0x55, 0x55, 0x55));
    } else if resp.hovered() {
        painter.rect_filled(rect, 2.0, Color32::from_rgb(0x3d, 0x3d, 0x3d));
    }
    let mid = rect.top() + 10.0;
    let x = rect.left() + 4.0 + row.depth as f32 * 14.0;
    let tri = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(12.0, 20.0));
    if let Some(open) = row.kids {
        let dir = if open { super::icons::Dir::Down } else { super::icons::Dir::Right };
        super::icons::triangle(&painter, egui::pos2(tri.center().x, mid), 3.5, dir, theme::TEXT_WEAK);
    }
    let icon = egui::pos2(tri.right() + 8.0, mid);
    let icon_color = if row.selected { theme::TEXT_STRONG } else { theme::TEXT_WEAK };
    super::icons::folder(&painter, icon, 5.5, icon_color, row.selected);
    if row.missing {
        super::icons::missing_badge(&painter, icon + egui::vec2(5.0, 3.5), theme::MISSING);
    }
    let count = painter.text(
        egui::pos2(rect.right() - 6.0, mid),
        egui::Align2::RIGHT_CENTER,
        row.count.to_string(),
        egui::FontId::proportional(11.5),
        theme::TEXT_WEAK,
    );
    let text_x = icon.x + 10.0;
    let room = (count.left() - 8.0 - text_x).max(10.0);
    let color = if row.missing {
        theme::TEXT_WEAK
    } else if row.selected {
        theme::TEXT_STRONG
    } else {
        theme::TEXT
    };
    let name = one_line(ui, &[(row.name, color)], egui::FontId::proportional(12.5), room);
    painter.galley(egui::pos2(text_x, mid - name.size().y / 2.0), name, color);
    if !row.caption.is_empty() {
        // Where it is: whole if it fits, else its end ("… › real").
        let font = egui::FontId::proportional(10.5);
        let room = rect.right() - 6.0 - text_x;
        let width = |t: &str| {
            ui.ctx().fonts_mut(|f| f.layout_no_wrap(t.to_owned(), font.clone(), Color32::WHITE).size().x)
        };
        let mut from = 0;
        let mut text = row.caption.join(" › ");
        while from + 1 < row.caption.len() && width(&text) > room {
            from += 1;
            text = format!("… › {}", row.caption[from..].join(" › "));
        }
        let dim = theme::TEXT_WEAK.gamma_multiply(0.9);
        let caption = one_line(ui, &[(&text, dim)], font, room);
        painter.galley(egui::pos2(text_x, rect.top() + 19.0), caption, dim);
    }
    let on_triangle = resp.interact_pointer_pos().is_some_and(|p| p.x < tri.right() + 2.0);
    (resp, on_triangle)
}

/// A catalog folder's menu: the same items and order as LrC's Folders panel, then
/// Expand All / Collapse All for it and the folders under it (returned: open or not).
fn folder_menu(
    resp: &egui::Response,
    f: &catalog::Folder,
    name: &str,
    missing: bool,
    has_kids: bool,
    commands: &mut Vec<Command>,
) -> Option<bool> {
    let mut all = None;
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
        if has_kids {
            ui.separator();
            for (text, open) in [("Expand All", true), ("Collapse All", false)] {
                // no hotkey: a folder's own menu (Library ▸ Panels has the keys for all folders)
                if ui.button(text).clicked() {
                    all = Some(open);
                    ui.close();
                }
            }
        }
    });
    all
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
