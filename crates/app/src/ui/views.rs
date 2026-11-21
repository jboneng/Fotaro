//! Center views (Grid, Loupe), filmstrip, toolbar, filter bar, full-screen preview.

use catalog::{Flag, PhotoRow};
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Stroke, Vec2};
use imaging::PreviewSize;

use super::shortcuts::Action;
use super::sidebars::{format_capture_time, format_shutter};
use super::{icons, theme};
use crate::app::{Command, GridStyle, FotaroApp, Session, View};

const UV: Rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

/// Largest rect with the image's aspect ratio that fits in `bounds`, centered.
pub fn fit_rect(image: Vec2, bounds: Rect) -> Rect {
    if image.x <= 0.0 || image.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.width() / image.x).min(bounds.height() / image.y);
    Rect::from_center_size(bounds.center(), image * scale)
}

/// Size of an image as displayed after `rotation` clockwise quarter turns.
pub fn rotated_size(size: Vec2, rotation: u8) -> Vec2 {
    if rotation % 2 == 1 { egui::vec2(size.y, size.x) } else { size }
}

/// Paint `tex` into `rect` (given in displayed orientation), rotated clockwise by
/// `rotation` quarter turns. Rotation is display-only: previews are never re-rendered.
///
/// The GPU samples textures without mipmaps here, so an image drawn noticeably smaller
/// than its texture aliases (thin bright lines like whiskers turn into dotted lines).
/// Then it is drawn four times with sub-pixel offsets and blended to their average:
/// 2×2 supersampling, cheap on the GPU.
pub fn paint_image(
    painter: &egui::Painter,
    tex: &egui::TextureHandle,
    rect: Rect,
    rotation: u8,
    tint: Color32,
) {
    let ppp = painter.ctx().pixels_per_point();
    let shown_px = rect.size() * ppp;
    let tex_px = rotated_size(tex.size_vec2(), rotation);
    let minification = (tex_px.x / shown_px.x.max(1.0)).max(tex_px.y / shown_px.y.max(1.0));
    if minification < 1.25 || tint.a() < 255 {
        paint_once(painter, tex.id(), rect, rotation, tint);
        return;
    }
    // "Over" blending with alphas 1, 1/2, 1/3, 1/4 leaves each pass weighted 1/4.
    let d = 0.25 / ppp;
    let offsets = [egui::vec2(-d, -d), egui::vec2(d, -d), egui::vec2(-d, d), egui::vec2(d, d)];
    for (i, offset) in offsets.into_iter().enumerate() {
        let alpha = (255.0 / (i as f32 + 1.0)).round() as u8;
        let pass = Color32::from_rgba_premultiplied(
            (u16::from(tint.r()) * u16::from(alpha) / 255) as u8,
            (u16::from(tint.g()) * u16::from(alpha) / 255) as u8,
            (u16::from(tint.b()) * u16::from(alpha) / 255) as u8,
            alpha,
        );
        paint_once(painter, tex.id(), rect.translate(offset), rotation, pass);
    }
}

fn paint_once(painter: &egui::Painter, tex: egui::TextureId, rect: Rect, rotation: u8, tint: Color32) {
    let r = usize::from(rotation % 4);
    if r == 0 {
        painter.image(tex, rect, UV, tint);
        return;
    }
    let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    let mut mesh = egui::Mesh::with_texture(tex);
    for (pos, uv) in corners.into_iter().zip(corner_uvs(rotation)) {
        mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: tint });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// Texture coordinates for the displayed top-left, top-right, bottom-right and
/// bottom-left corners after turning the image `rotation` quarter turns clockwise.
fn corner_uvs(rotation: u8) -> [egui::Pos2; 4] {
    let uvs = [egui::pos2(0.0, 0.0), egui::pos2(1.0, 0.0), egui::pos2(1.0, 1.0), egui::pos2(0.0, 1.0)];
    let r = usize::from(rotation % 4);
    // Turning clockwise brings the source corner one step "behind" to each screen corner.
    std::array::from_fn(|i| uvs[(i + 4 - r) % 4])
}

struct CellState {
    index: usize,
    selected: bool,
    active: bool,
    /// Show LrC's "metadata needs saving to file" badge.
    show_dirty: bool,
}

/// Stars centered on `center`; `size` is roughly the glyph height.
fn paint_stars(painter: &egui::Painter, center: egui::Pos2, rating: u8, size: f32, color: Color32) {
    icons::stars(painter, center, size * 0.42, rating, color);
}

fn paint_flag(painter: &egui::Painter, left_center: egui::Pos2, flag: Flag, size: f32) {
    match flag {
        Flag::Picked => icons::flag(painter, left_center, size, theme::PICK, Color32::from_gray(0x30), false),
        Flag::Rejected => {
            icons::flag(painter, left_center, size, theme::REJECT, Color32::from_gray(0x10), true)
        }
        Flag::Unflagged => {}
    }
}

/// Draw one Grid cell in the current style.
fn paint_cell(
    ui: &egui::Ui,
    rect: Rect,
    photo: &PhotoRow,
    tex: Option<&egui::TextureHandle>,
    failed: bool,
    style: GridStyle,
    st: &CellState,
) {
    let painter = ui.painter_at(rect);
    let mut bg = if st.active {
        theme::CELL_ACTIVE
    } else if st.selected {
        theme::CELL_SELECTED
    } else {
        theme::CELL
    };
    // Colour labels tint the cell (unselected), like LrC.
    let label = photo.color_label.as_deref().and_then(theme::label_color);
    if let (Some(l), false) = (label, st.selected) {
        bg = l.lerp_to_gamma(bg, 0.55);
    }
    let cell = rect.shrink(2.0);
    painter.rect_filled(cell, 2.0, bg);
    if let Some(l) = label {
        painter.rect_stroke(cell, 2.0, Stroke::new(2.0, l), egui::StrokeKind::Inside);
    }

    let (header, footer) = match style {
        GridStyle::Expanded => (22.0, 22.0),
        GridStyle::Compact => (0.0, 20.0),
        GridStyle::Minimal => (0.0, 0.0),
    };
    let text_color = if st.selected { theme::TEXT_STRONG } else { Color32::from_gray(0x20) };

    if style == GridStyle::Compact {
        // Big faint index number behind the photo.
        painter.text(
            cell.left_top() + egui::vec2(6.0, 2.0),
            Align2::LEFT_TOP,
            (st.index + 1).to_string(),
            FontId::proportional(cell.height() * 0.22),
            theme::CELL_INDEX,
        );
    }
    if style == GridStyle::Expanded {
        let y = cell.top() + header / 2.0;
        painter.text(
            egui::pos2(cell.left() + 6.0, y),
            Align2::LEFT_CENTER,
            (st.index + 1).to_string(),
            FontId::proportional(13.0),
            text_color,
        );
        let ext = photo.path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
        painter.text(
            egui::pos2(cell.right() - 6.0, y),
            Align2::RIGHT_CENTER,
            ext,
            FontId::proportional(10.5),
            text_color,
        );
        let name = photo.file_name.rsplit_once('.').map_or(photo.file_name.as_str(), |(n, _)| n);
        painter.text(
            egui::pos2(cell.left() + 30.0, y),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(11.0),
            text_color,
        );
    }

    let margin = (cell.width() * 0.09).max(6.0);
    let image_area = Rect::from_min_max(
        cell.min + egui::vec2(margin, margin * 0.6 + header),
        cell.max - egui::vec2(margin, margin * 0.6 + footer),
    );
    match tex {
        Some(tex) => {
            let r = fit_rect(rotated_size(tex.size_vec2(), photo.rotation), image_area);
            painter.rect_filled(r.translate(egui::vec2(1.5, 1.5)), 0.0, Color32::from_black_alpha(90));
            let tint =
                if photo.flag == Flag::Rejected { Color32::from_white_alpha(90) } else { Color32::WHITE };
            paint_image(&painter, tex, r, photo.rotation, tint);
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(1.0, Color32::from_black_alpha(120)),
                egui::StrokeKind::Outside,
            );
            if photo.virtual_copy.is_some() {
                paint_virtual_copy_badge(&painter, r.left_bottom());
            }
        }
        None => {
            let r = Rect::from_center_size(image_area.center(), image_area.size() * egui::vec2(0.8, 0.6));
            painter.rect_filled(r, 0.0, Color32::from_black_alpha(25));
            if failed || photo.missing {
                painter.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    "⚠",
                    FontId::proportional(18.0),
                    Color32::from_black_alpha(140),
                );
            }
        }
    }

    if footer > 0.0 {
        let y = cell.bottom() - footer / 2.0 - 2.0;
        paint_flag(&painter, egui::pos2(cell.left() + 7.0, y), photo.flag, 13.0);
        paint_stars(
            &painter,
            egui::pos2(cell.center().x, y),
            photo.rating,
            11.0,
            if st.selected { theme::TEXT_STRONG } else { theme::STAR },
        );
    } else if style == GridStyle::Minimal && photo.flag != Flag::Unflagged {
        paint_flag(&painter, cell.left_top() + egui::vec2(5.0, 9.0), photo.flag, 12.0);
    }
    if photo.missing {
        painter.text(
            cell.right_top() + egui::vec2(-6.0, header + 4.0),
            Align2::RIGHT_TOP,
            "!",
            FontId::proportional(15.0),
            theme::MISSING,
        );
    }
    // Quick Collection: a filled dot at the top right (LrC's circle badge).
    if photo.in_quick_collection {
        super::collections::paint_quick_marker(
            &painter,
            cell.right_top() + egui::vec2(-9.0, header + 9.0),
            true,
        );
    }
    // Stacks: how many photos the unit holds (a card behind; two for a group of
    // pictures), what the shown one is when it isn't the original, and a dot when the
    // filter matched another member.
    let unit = photo.unit;
    if unit.is_stack() {
        let mut text = unit.members.to_string();
        if unit.role != catalog::Role::Original {
            text = format!("{text}  {}", member_label(photo));
        }
        let font = FontId::proportional(10.5);
        let galley = painter.layout_no_wrap(text, font, theme::TEXT_STRONG);
        let pos = cell.left_top() + egui::vec2(5.0, header + 5.0);
        let r = Rect::from_min_size(pos, galley.size() + egui::vec2(8.0, 4.0));
        let cards = if unit.pictures > 1 { 2 } else { 1 };
        for i in (1..=cards).rev() {
            let off = 2.5 * i as f32;
            painter.rect_filled(
                r.translate(egui::vec2(off, off)),
                2.0,
                Color32::from_gray(0x50 - 0x10 * i as u8),
            );
        }
        painter.rect_filled(r, 2.0, Color32::from_gray(0x30));
        painter.rect_stroke(r, 2.0, Stroke::new(1.0, theme::TEXT_WEAK), egui::StrokeKind::Inside);
        painter.galley(r.min + egui::vec2(4.0, 2.0), galley, theme::TEXT_STRONG);
        if unit.hidden_match {
            painter.circle_filled(r.right_top() + egui::vec2(-1.0, 1.0), 3.0, theme::MISSING);
        }
    }
    if st.show_dirty && style != GridStyle::Minimal {
        paint_needs_saving(&painter, cell.right_bottom() - egui::vec2(12.0, 10.0 + footer.max(4.0) / 2.0));
    }
    // LrC's "has Develop adjustments" badge, left of the needs-saving one.
    if photo.develop.is_some() && style != GridStyle::Minimal {
        paint_developed(&painter, cell.right_bottom() - egui::vec2(26.0, 10.0 + footer.max(4.0) / 2.0));
    }
}

/// LrC's virtual copy badge: a page corner folded over at the photo's bottom left.
fn paint_virtual_copy_badge(painter: &egui::Painter, corner: egui::Pos2) {
    let s = 14.0;
    let fold = vec![corner, corner + egui::vec2(0.0, -s), corner + egui::vec2(s, 0.0)];
    painter.add(egui::Shape::convex_polygon(
        fold,
        Color32::from_gray(0xe0),
        Stroke::new(1.0, Color32::from_black_alpha(140)),
    ));
}

/// A small page with a down arrow: catalog metadata not yet saved to the sidecar.
/// A small ± box: the photo has Develop settings.
fn paint_developed(painter: &egui::Painter, center: egui::Pos2) {
    let r = Rect::from_center_size(center, egui::vec2(10.0, 10.0));
    let color = theme::TEXT_WEAK;
    painter.rect_stroke(r, 1.5, Stroke::new(1.0, color), egui::StrokeKind::Inside);
    let s = Stroke::new(1.0, color);
    let c = r.center();
    painter.line_segment([c + egui::vec2(-2.5, -1.5), c + egui::vec2(2.5, -1.5)], s);
    painter.line_segment([c + egui::vec2(0.0, -4.0), c + egui::vec2(0.0, 1.0)], s);
    painter.line_segment([c + egui::vec2(-2.5, 2.5), c + egui::vec2(2.5, 2.5)], s);
}

fn paint_needs_saving(painter: &egui::Painter, center: egui::Pos2) {
    let page = Rect::from_center_size(center, egui::vec2(8.0, 10.0));
    let color = theme::TEXT_WEAK;
    painter.rect_stroke(page, 1.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
    let tip = page.center_bottom() - egui::vec2(0.0, 2.0);
    painter.add(egui::Shape::convex_polygon(
        vec![tip, tip + egui::vec2(-2.5, -3.0), tip + egui::vec2(2.5, -3.0)],
        color,
        Stroke::NONE,
    ));
    painter.line_segment(
        [page.center_top() + egui::vec2(0.0, 2.0), tip - egui::vec2(0.0, 2.5)],
        Stroke::new(1.0, color),
    );
}

fn photo_context_menu(resp: &egui::Response, app: &mut FotaroApp, photo: &PhotoRow) {
    resp.context_menu(|ui| {
        if ui.button(crate::platform::REVEAL_LABEL).clicked() {
            app.run(Command::Reveal(photo.path.clone()));
            ui.close();
        }
        ui.separator();
        ui.menu_button("Set Flag", |ui| {
            for (name, flag) in
                [("Flagged", Flag::Picked), ("Unflagged", Flag::Unflagged), ("Rejected", Flag::Rejected)]
            {
                if ui.button(name).clicked() {
                    app.run(Command::Action(Action::SetFlag { flag, advance: false }));
                    ui.close();
                }
            }
        });
        ui.menu_button("Set Rating", |ui| {
            for stars in 0..=5u8 {
                let label = match stars {
                    0 => "None".to_string(),
                    1 => "1 Star".to_string(),
                    n => format!("{n} Stars"),
                };
                if ui.button(label).clicked() {
                    app.run(Command::Action(Action::SetRating { stars, advance: false }));
                    ui.close();
                }
            }
        });
        ui.menu_button("Set Color Label", |ui| {
            for (label, name) in [("red", "Red"), ("yellow", "Yellow"), ("green", "Green"), ("blue", "Blue")]
            {
                if ui.button(name).clicked() {
                    app.run(Command::Action(Action::ColorLabel { label, advance: false }));
                    ui.close();
                }
            }
        });
        ui.separator();
        let qc = if photo.in_quick_collection {
            "Remove from Quick Collection"
        } else {
            "Add to Quick Collection"
        };
        if ui.button(qc).clicked() {
            app.run(Command::Action(Action::QuickCollection));
            ui.close();
        }
        super::collections::add_to_collection_menu(ui, app);
        if app.session.as_ref().is_some_and(|s| matches!(s.source, catalog::Source::Collection { .. }))
            && ui.button("Remove from Collection").clicked()
        {
            app.run(Command::Action(Action::RemoveFromCollection));
            ui.close();
        }
        if ui.button("Create Virtual Copy").clicked() {
            app.run(Command::Action(Action::CreateVirtualCopy));
            ui.close();
        }
        ui.menu_button("Stacking", |ui| {
            for (name, action) in [
                ("Group into Stack", Action::Stack),
                ("Unstack", Action::Unstack),
                ("Collapse / Expand Stack", Action::ToggleStack),
                ("Move to Top of Stack", Action::MoveToTopOfStack),
            ] {
                if ui.button(name).clicked() {
                    app.run(Command::Action(action));
                    ui.close();
                }
            }
        });
        ui.separator();
        for (name, action) in
            [("Rotate Left (CCW)", Action::RotateLeft), ("Rotate Right (CW)", Action::RotateRight)]
        {
            if ui.button(name).clicked() {
                app.run(Command::Action(action));
                ui.close();
            }
        }
        ui.separator();
        ui.menu_button("Edit In", |ui| super::edit_in::menu(ui, app));
        ui.separator();
        ui.menu_button("Export", |ui| {
            for (name, action) in
                [("Export…", Action::Export), ("Export with Previous", Action::ExportWithPrevious)]
            {
                if ui.button(name).clicked() {
                    app.run(Command::Action(action));
                    ui.close();
                }
            }
            ui.menu_button("Export with Preset", |ui| super::export::preset_menu(ui, app));
        });
    });
}

/// Handle a click on a photo (cell or filmstrip frame).
fn handle_click(
    app: &mut FotaroApp,
    resp: &egui::Response,
    photo: &PhotoRow,
    open_loupe_on_double: bool,
) {
    if resp.clicked() || resp.secondary_clicked() {
        let mods = resp.ctx.input(|i| i.modifiers);
        if let Some(s) = &mut app.session {
            if resp.secondary_clicked() && s.selected.contains(&photo.id) {
                s.active = Some(photo.id); // keep multi-selection for the context menu
            } else {
                s.click(photo.id, mods);
            }
        }
    }
    if open_loupe_on_double && resp.double_clicked() {
        app.view = View::Loupe;
    }
}

pub fn grid(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let style = app.ui.grid_style;
    let auto_write = app.ui.auto_write_xmp;
    let cell_w = app.ui.thumb_size.clamp(80.0, 360.0);
    let cell_h = cell_w * if style == GridStyle::Expanded { 1.18 } else { 1.0 };
    let avail = ui.available_width() - 8.0;
    let cols = ((avail / cell_w).floor() as usize).max(1);
    let cell_w = avail / cols as f32;
    app.grid_cols = cols;

    let Some(s) = &mut app.session else { return };
    if s.photos.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("No photos here yet.\nUse File ▸ Import Photos (Add Folder) to add some.")
                    .color(theme::TEXT_WEAK)
                    .size(14.0),
            );
        });
        return;
    }
    let rows = s.photos.len().div_ceil(cols);

    // Keep the active photo visible after keyboard navigation.
    let mut scroll = egui::ScrollArea::vertical().id_salt("grid").auto_shrink([false, false]);
    if s.scroll_to_active {
        s.scroll_to_active = false;
        if let Some(i) = s.active_index() {
            let top = (i / cols) as f32 * cell_h;
            let (offset, view_h) = app.grid_scroll;
            if top < offset {
                scroll = scroll.vertical_scroll_offset(top);
            } else if top + cell_h > offset + view_h {
                scroll = scroll.vertical_scroll_offset(top + cell_h - view_h);
            }
        }
    }

    let mut clicks: Vec<(egui::Response, PhotoRow)> = Vec::new();
    let out = scroll.show_rows(ui, cell_h, rows, |ui, range| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let s: &mut Session = app.session.as_mut().unwrap();
        for row in range {
            let (row_rect, _) = ui.allocate_exact_size(egui::vec2(avail, cell_h), Sense::hover());
            for col in 0..cols {
                let i = row * cols + col;
                let Some(photo) = s.photos.get(i).cloned() else { break };
                let rect = Rect::from_min_size(
                    row_rect.min + egui::vec2(4.0 + col as f32 * cell_w, 0.0),
                    egui::vec2(cell_w, cell_h),
                );
                let tex = s.thumbs.get(photo.id);
                if tex.is_none() && !photo.missing {
                    s.request(&photo, PreviewSize::Thumb);
                }
                let st = CellState {
                    index: i,
                    selected: s.selected.contains(&photo.id),
                    active: s.active == Some(photo.id),
                    show_dirty: photo.metadata_dirty && !auto_write,
                };
                paint_cell(ui, rect, &photo, tex.as_ref(), s.thumbs.is_failed(photo.id), style, &st);
                let resp = ui.interact(rect, egui::Id::unique(("cell", photo.id)), Sense::click_and_drag());
                // Drag photos onto a collection or the Quick Collection.
                if resp.drag_started() {
                    let dragged =
                        if s.selected.contains(&photo.id) { s.targets(View::Grid) } else { vec![photo.id] };
                    resp.dnd_set_drag_payload(super::collections::PhotoDrag(dragged));
                }
                let resp =
                    if photo.missing { resp.on_hover_text("The file could not be found.") } else { resp };
                clicks.push((resp, photo));
            }
        }
    });
    app.grid_scroll = (out.state.clamped_offset().y, out.inner_rect.height());
    app.grid_rows_visible = ((out.inner_rect.height() / cell_h).floor() as usize).max(1);

    for (resp, photo) in clicks {
        handle_click(app, &resp, &photo, true);
        photo_context_menu(&resp, app, &photo);
    }
    // While dragging photos, show how many are being moved.
    if let Some(drag) = egui::DragAndDrop::payload::<super::collections::PhotoDrag>(ui.ctx())
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        let painter =
            ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("photo_drag")));
        let n = drag.0.len();
        let text = if n == 1 { "1 photo".to_string() } else { format!("{n} photos") };
        let galley = painter.layout_no_wrap(text, FontId::proportional(12.0), theme::TEXT_STRONG);
        let r = Rect::from_min_size(pos + egui::vec2(14.0, 10.0), galley.size() + egui::vec2(12.0, 6.0));
        painter.rect_filled(r, 3.0, Color32::from_black_alpha(200));
        painter.galley(r.min + egui::vec2(6.0, 3.0), galley, theme::TEXT_STRONG);
    }
}

fn info_overlay(app: &mut FotaroApp, ui: &egui::Ui, rect: Rect, photo: &PhotoRow) {
    if app.ui.info_overlay == 0 {
        return;
    }
    let Some(s) = &mut app.session else { return };
    let m = s.active_metadata().unwrap_or_default();
    let line2 = match app.ui.info_overlay {
        1 => {
            let dims = super::sidebars::dimensions(&m, photo.rotation).unwrap_or_default();
            format!("{}    {dims}", photo.capture_time.map(format_capture_time).unwrap_or_default())
        }
        _ => {
            let mut parts = Vec::new();
            if let Some(t) = m.shutter {
                parts.push(format_shutter(t));
            }
            if let Some(a) = m.aperture {
                parts.push(format!("ƒ / {a:.1}"));
            }
            if let Some(i) = m.iso {
                parts.push(format!("ISO {i:.0}"));
            }
            if let Some(f) = m.focal_len {
                parts.push(format!("{f:.0} mm"));
            }
            if let Some(l) = &m.lens {
                parts.push(l.clone());
            }
            parts.join("   ")
        }
    };
    let painter = ui.painter_at(rect);
    let pos = rect.left_top() + egui::vec2(14.0, 10.0);
    for (i, (text, size)) in
        [(photo.file_name.as_str(), 15.0), (line2.as_str(), 12.0)].into_iter().enumerate()
    {
        let p = pos + egui::vec2(0.0, i as f32 * 20.0);
        painter.text(
            p + egui::vec2(1.0, 1.0),
            Align2::LEFT_TOP,
            text,
            FontId::proportional(size),
            Color32::from_black_alpha(160),
        );
        painter.text(p, Align2::LEFT_TOP, text, FontId::proportional(size), theme::TEXT_STRONG);
    }
}

/// The best loaded image for `photo` at fit size: the standard preview, else the
/// thumbnail while it loads. Requests whatever is missing.
fn fit_texture(s: &mut Session, photo: &PhotoRow) -> Option<egui::TextureHandle> {
    if let Some(t) = s.previews.get(photo.id) {
        return Some(t);
    }
    if !photo.missing {
        s.request(photo, PreviewSize::Standard);
    }
    let thumb = s.thumbs.get(photo.id);
    if thumb.is_none() && !photo.missing {
        s.request(photo, PreviewSize::Thumb);
    }
    thumb
}

fn centered_message(ui: &egui::Ui, rect: Rect, photo: &PhotoRow) {
    let msg = if photo.missing { "The file could not be found." } else { "Loading…" };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        msg,
        FontId::proportional(14.0),
        theme::TEXT_WEAK,
    );
}

/// Draw the best available image for `photo` fitted into `rect` (full screen preview).
fn draw_preview(s: &mut Session, ui: &egui::Ui, photo: &PhotoRow, rect: Rect) {
    match fit_texture(s, photo) {
        Some(tex) => {
            let r = fit_rect(rotated_size(tex.size_vec2(), photo.rotation), rect);
            paint_image(ui.painter(), &tex, r, photo.rotation, Color32::WHITE);
        }
        None => centered_message(ui, rect, photo),
    }
}

/// Keep the zoomed view inside the photo: `center` is the image point (0..1) at the
/// view's center and `visible` the fraction of the photo the view spans. Axes where
/// the whole photo fits are centered.
pub fn clamp_center(center: Vec2, visible: Vec2) -> Vec2 {
    let axis = |c: f32, v: f32| if v >= 1.0 { 0.5 } else { c.clamp(v / 2.0, 1.0 - v / 2.0) };
    egui::vec2(axis(center.x, visible.x), axis(center.y, visible.y))
}

/// Draw `photo` in `rect` with the shared zoom state (Loupe, and both Compare panes, so
/// their zoom and position stay linked). `primary` panes record where they drew the
/// photo for the Navigator and for Z; `zoom_on_click` panes toggle zoom when clicked.
fn photo_pane(
    app: &mut FotaroApp,
    ui: &mut egui::Ui,
    rect: Rect,
    photo: &PhotoRow,
    salt: &str,
    primary: bool,
    zoom_on_click: bool,
) -> egui::Response {
    let resp = ui.interact(rect, egui::Id::unique(("photo_pane", salt)), Sense::click_and_drag());
    let ppp = ui.ctx().pixels_per_point();
    let zoom_level = app.ui.zoom_level;
    let Some(s) = &mut app.session else { return resp };
    let zoomed = app.zoom.zoomed && !photo.missing;

    // Zoomed: the 1:1 preview, showing the standard one enlarged until it arrives.
    let full = if zoomed { s.full.get(photo.id) } else { None };
    if zoomed && full.is_none() {
        s.request(photo, PreviewSize::Full);
    }
    let loading_full = zoomed && full.is_none();
    let Some(tex) = full.or_else(|| fit_texture(s, photo)) else {
        centered_message(ui, rect, photo);
        if primary {
            app.zoom.image_rect = None;
        }
        return resp;
    };

    // Size of the photo in pixels as displayed (file orientation, then catalog rotation).
    let native = s.photo_dims(photo.id).unwrap_or_else(|| tex.size_vec2());
    let img_px = rotated_size(native, photo.rotation);
    let view = if zoomed { rect } else { rect.shrink(18.0) };
    let fit = (view.width() / img_px.x).min(view.height() / img_px.y);
    let fill = (view.width() / img_px.x).max(view.height() / img_px.y);
    // Small photos never zoom below Fit.
    let scale = if zoomed { zoom_level.scale(fill, ppp).max(fit) } else { fit };
    let size = img_px * scale;

    let z = &mut app.zoom;
    if zoomed {
        if let Some((q, offset)) = z.anchor.take() {
            z.center = q - offset / size;
        }
        if resp.dragged() {
            z.center -= resp.drag_delta() / size;
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta);
            if scroll != Vec2::ZERO {
                z.center -= scroll / size;
            }
        }
        z.center = clamp_center(z.center, view.size() / size);
    }
    let img_rect = if zoomed {
        Rect::from_min_size(view.center() - size * z.center, size)
    } else {
        Rect::from_center_size(view.center(), size)
    };
    if primary {
        z.image_rect = Some(img_rect);
        z.view_rect = Some(view);
        z.visible = zoomed.then(|| {
            let v = view.intersect(img_rect);
            Rect::from_min_max(
                ((v.min - img_rect.min) / size).to_pos2(),
                ((v.max - img_rect.min) / size).to_pos2(),
            )
        });
    }

    let painter = ui.painter().with_clip_rect(rect);
    paint_image(&painter, &tex, img_rect, photo.rotation, Color32::WHITE);
    if loading_full {
        painter.text(
            rect.left_bottom() + egui::vec2(12.0, -12.0),
            Align2::LEFT_BOTTOM,
            "Loading…",
            FontId::proportional(12.0),
            theme::TEXT_STRONG,
        );
    }

    // LrC: a click toggles between Fit and the zoom level, at the clicked point.
    if zoom_on_click && resp.clicked() && !photo.missing {
        if zoomed {
            z.zoomed = false;
        } else if let Some(p) = resp.interact_pointer_pos() {
            z.zoom_in_at_in(p, img_rect, view);
        }
    }
    if zoomed {
        let icon = if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab };
        resp.clone().on_hover_cursor(icon);
    } else if zoom_on_click && resp.hover_pos().is_some_and(|p| img_rect.contains(p)) && !photo.missing {
        resp.clone().on_hover_cursor(egui::CursorIcon::ZoomIn);
    }
    resp
}

pub fn loupe(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    let Some(s) = &mut app.session else { return };
    let Some(photo) = s.active_photo().cloned() else {
        app.view = View::Grid;
        return;
    };
    // Prefetch neighbours first: the visible lane is LIFO, so the current photo wins.
    let neighbours: Vec<PhotoRow> = s
        .active_index()
        .map(|i| [i + 1, i.wrapping_sub(1)].iter().filter_map(|&j| s.photos.get(j).cloned()).collect())
        .unwrap_or_default();
    for p in neighbours.iter().filter(|p| !p.missing) {
        s.request(p, PreviewSize::Standard);
    }
    // While checking focus across a burst, the next photo's 1:1 is usually wanted too.
    if app.zoom.zoomed
        && s.full.get(photo.id).is_some()
        && let Some(next) = neighbours.first().filter(|p| !p.missing)
    {
        s.request(next, PreviewSize::Full);
    }
    let resp = photo_pane(app, ui, rect, &photo, "loupe", true, true);
    info_overlay(app, ui, rect, &photo);
    photo_context_menu(&resp, app, &photo);
}

/// LrC's Compare view: the Select (left) and a Candidate (right), zoom and position
/// linked. Rating and flagging go to the active pane (white frame); click the other
/// pane to activate it. ←/→ step the Candidate, ↑ makes it the Select, ↓ swaps.
pub fn compare(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    if let Some((original, other)) = app.member_compare.clone() {
        member_compare(app, ui, rect, &original, &other);
        return;
    }
    let Some(s) = &mut app.session else { return };
    let Some(mut pair) = app.compare else {
        app.view = View::Grid;
        return;
    };
    // The filmstrip (or anything else changing the active photo) picks a new Candidate.
    if let Some(a) = s.active
        && a != pair.select
        && a != pair.candidate
    {
        pair.candidate = a;
    }
    let (Some(&si), Some(&ci)) = (s.index_of.get(&pair.select), s.index_of.get(&pair.candidate)) else {
        app.compare = None;
        app.view = View::Grid;
        return;
    };
    app.compare = Some(pair);
    let (select, candidate) = (s.photos[si].clone(), s.photos[ci].clone());
    let active = s.active;

    let gap = 6.0;
    let half = (rect.width() - gap) / 2.0;
    let panes = [
        (Rect::from_min_size(rect.min, egui::vec2(half, rect.height())), select, "Select"),
        (
            Rect::from_min_size(rect.min + egui::vec2(half + gap, 0.0), egui::vec2(half, rect.height())),
            candidate,
            "Candidate",
        ),
    ];
    for (r, photo, label) in panes {
        let is_active = active == Some(photo.id);
        let header = Rect::from_min_size(r.min, egui::vec2(r.width(), 24.0));
        let body = Rect::from_min_max(egui::pos2(r.left(), header.bottom()), r.max);
        let painter = ui.painter();
        painter.rect_filled(header, 0.0, Color32::from_gray(if is_active { 0x50 } else { 0x38 }));
        let name = match &photo.virtual_copy {
            Some(copy) => format!("{}   {}  ({copy})", label, photo.file_name),
            None => format!("{}   {}", label, photo.file_name),
        };
        painter.text(
            header.left_center() + egui::vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(12.0),
            if is_active { theme::TEXT_STRONG } else { theme::TEXT },
        );
        paint_stars(painter, header.right_center() - egui::vec2(50.0, 0.0), photo.rating, 10.0, theme::STAR);
        paint_flag(painter, header.right_center() - egui::vec2(96.0, 0.0), photo.flag, 11.0);
        let resp = photo_pane(app, ui, body, &photo, label, is_active, is_active);
        if is_active {
            ui.painter().rect_stroke(
                body,
                0.0,
                Stroke::new(1.5, theme::TEXT_STRONG),
                egui::StrokeKind::Inside,
            );
        }
        if resp.clicked()
            && !is_active
            && let Some(s) = &mut app.session
        {
            s.active = Some(photo.id);
        }
        photo_context_menu(&resp, app, &photo);
    }
    if let Some(s) = &mut app.session {
        // Both photos are the selection, as in LrC.
        s.selected = [pair.select, pair.candidate].into_iter().collect();
    }
}

/// Columns for laying out `n` photos of aspect `aspect` in `area` as large as possible.
pub fn survey_columns(n: usize, area: Vec2, aspect: f32) -> usize {
    (1..=n.max(1))
        .max_by(|&a, &b| {
            let size = |cols: usize| {
                let rows = n.div_ceil(cols) as f32;
                let (w, h) = (area.x / cols as f32, area.y / rows);
                (w / aspect).min(h)
            };
            size(a).total_cmp(&size(b))
        })
        .unwrap_or(1)
}

/// LrC's Survey view: every selected photo, as large as fits. Click one to make it
/// active (rating and flagging apply to it); ✕ takes a photo out of the survey.
pub fn survey(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    let Some(s) = &mut app.session else { return };
    let photos: Vec<PhotoRow> = s.photos.iter().filter(|p| s.selected.contains(&p.id)).cloned().collect();
    if photos.is_empty() {
        app.view = View::Grid;
        return;
    }
    if s.active.is_none_or(|a| !s.selected.contains(&a)) {
        s.active = Some(photos[0].id);
    }
    let area = rect.shrink(10.0);
    let cols = survey_columns(photos.len(), area.size(), 1.5);
    let rows = photos.len().div_ceil(cols);
    let cell = egui::vec2(area.width() / cols as f32, area.height() / rows as f32);
    // Big cells use the standard previews; many small ones the thumbnails.
    let use_previews = cell.x > 400.0 && photos.len() <= 8;
    let mut drop = None;
    let mut activate = None;
    let mut menus = Vec::new();
    for (i, photo) in photos.iter().enumerate() {
        let (row, col) = (i / cols, i % cols);
        let r = Rect::from_min_size(area.min + egui::vec2(col as f32 * cell.x, row as f32 * cell.y), cell)
            .shrink(6.0);
        let is_active = s.active == Some(photo.id);
        let resp = ui.interact(r, egui::Id::unique(("survey", photo.id)), Sense::click());
        let image_area = Rect::from_min_max(r.min, r.max - egui::vec2(0.0, 22.0)).shrink(4.0);
        let tex = if use_previews { fit_texture(s, photo) } else { s.thumbs.get(photo.id) };
        match tex {
            Some(tex) => {
                let ir = fit_rect(rotated_size(tex.size_vec2(), photo.rotation), image_area);
                paint_image(ui.painter(), &tex, ir, photo.rotation, Color32::WHITE);
                if is_active {
                    ui.painter().rect_stroke(
                        ir.expand(3.0),
                        0.0,
                        Stroke::new(2.0, theme::TEXT_STRONG),
                        egui::StrokeKind::Outside,
                    );
                }
            }
            None if !photo.missing => s.request(photo, PreviewSize::Thumb),
            None => {}
        }
        let y = r.bottom() - 11.0;
        paint_flag(ui.painter(), egui::pos2(r.left() + 6.0, y), photo.flag, 11.0);
        paint_stars(ui.painter(), egui::pos2(r.center().x, y), photo.rating, 10.0, theme::STAR);
        // ✕ on hover takes the photo out of the survey (deselects it), as in LrC.
        let x = Rect::from_center_size(r.right_top() + egui::vec2(-12.0, 12.0), egui::vec2(18.0, 18.0));
        let over_x = resp.hover_pos().is_some_and(|p| x.contains(p));
        if resp.hovered() {
            ui.painter().rect_filled(x, 3.0, Color32::from_black_alpha(if over_x { 220 } else { 140 }));
            ui.painter().text(
                x.center(),
                Align2::CENTER_CENTER,
                "✕",
                FontId::proportional(12.0),
                theme::TEXT_STRONG,
            );
        }
        if resp.clicked() {
            if over_x { drop = Some(photo.id) } else { activate = Some(photo.id) }
        }
        menus.push((resp, photo.clone()));
    }
    if let Some(id) = drop {
        s.selected.remove(&id);
        if s.active == Some(id) {
            s.active = photos.iter().map(|p| p.id).find(|p| s.selected.contains(p));
        }
    }
    if let Some(id) = activate {
        s.active = Some(id);
    }
    for (resp, photo) in menus {
        photo_context_menu(&resp, app, &photo);
    }
}

pub fn fullscreen(app: &mut FotaroApp, ctx: &egui::Context, full: Rect) {
    egui::Area::new(egui::Id::unique("fullscreen_preview"))
        .order(egui::Order::Foreground)
        .fixed_pos(full.min)
        .show(ctx, |ui| {
            let resp = ui.allocate_rect(full, Sense::click());
            ui.painter().rect_filled(full, 0.0, Color32::BLACK);
            if let Some(s) = &mut app.session
                && let Some(photo) = s.active_photo().cloned()
            {
                draw_preview(s, ui, &photo, full);
            }
            if resp.clicked() {
                app.run(Command::Action(Action::FullScreenPreview));
            }
        });
}

pub fn filmstrip(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    // Header: source breadcrumb, counts and the active file name.
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let grid = ui.add(egui::Button::new("▦").frame(false)).on_hover_text("Grid (G)");
        if grid.clicked() {
            app.view = View::Grid;
        }
        ui.add_space(6.0);
        let mut text = format!("{}  /  {} photos", s.source_label(), s.photos.len());
        if s.selected.len() > 1 {
            text.push_str(&format!("  /  {} selected", s.selected.len()));
        }
        if let Some(p) = s.active_photo() {
            text.push_str(&format!("  /  {}", p.file_name));
        }
        ui.label(RichText::new(text).size(11.5).color(theme::TEXT));
        // The version strip is hidden: a way back to it for a photo in a stack.
        let members = s.active_photo().map_or(0, |p| p.unit.members);
        if members > 1 && !app.ui.show_version_strip {
            ui.add_space(8.0);
            let b =
                ui.small_button(format!("Stack: {members} ▴")).on_hover_text("Show the version strip (S)");
            if b.clicked() {
                app.ui.show_version_strip = true;
            }
        }
    });

    let height = (ui.available_height() - 4.0).max(30.0);
    let item = height * 1.25;
    let n = s.photos.len();
    let mut scroll = egui::ScrollArea::horizontal().id_salt("filmstrip").auto_shrink([false, false]);
    if s.scroll_to_active
        && app.view == View::Loupe
        && let Some(i) = s.active_index()
    {
        scroll = scroll
            .horizontal_scroll_offset((i as f32 * item - ui.available_width() / 2.0 + item / 2.0).max(0.0));
    }

    let mut clicks = Vec::new();
    scroll.show_viewport(ui, |ui, viewport| {
        ui.set_min_size(egui::vec2(n as f32 * item, height));
        let s: &mut Session = app.session.as_mut().unwrap();
        let first = (viewport.min.x / item).floor().max(0.0) as usize;
        let last = ((viewport.max.x / item).ceil() as usize).min(n);
        let origin = ui.max_rect().min;
        for i in first..last {
            let photo = s.photos[i].clone();
            let rect =
                Rect::from_min_size(origin + egui::vec2(i as f32 * item, 0.0), egui::vec2(item, height))
                    .shrink(2.0);
            let selected = s.selected.contains(&photo.id);
            let active = s.active == Some(photo.id);
            let bg = if active {
                theme::CELL_ACTIVE
            } else if selected {
                theme::CELL_SELECTED
            } else {
                Color32::from_rgb(0x3a, 0x3a, 0x3a)
            };
            ui.painter().rect_filled(rect, 1.0, bg);
            match s.thumbs.get(photo.id) {
                Some(tex) => {
                    let tint = if photo.flag == Flag::Rejected {
                        Color32::from_white_alpha(90)
                    } else {
                        Color32::WHITE
                    };
                    let r = fit_rect(rotated_size(tex.size_vec2(), photo.rotation), rect.shrink(5.0));
                    paint_image(ui.painter(), &tex, r, photo.rotation, tint);
                }
                None if !photo.missing => s.request(&photo, PreviewSize::Thumb),
                None => {}
            }
            paint_flag(ui.painter(), rect.left_top() + egui::vec2(4.0, 8.0), photo.flag, 10.0);
            paint_stars(
                ui.painter(),
                rect.center_bottom() - egui::vec2(0.0, 6.0),
                photo.rating,
                8.0,
                theme::TEXT_STRONG,
            );
            let resp = ui.interact(rect, egui::Id::unique(("film", photo.id)), Sense::click());
            clicks.push((resp, photo));
        }
    });
    for (resp, photo) in clicks {
        handle_click(app, &resp, &photo, true);
        photo_context_menu(&resp, app, &photo);
    }
}

pub fn toolbar(app: &mut FotaroApp, ui: &mut egui::Ui) {
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let views = [
            (View::Grid, "▦", 15.0, "Grid (G)", Action::GridView),
            (View::Loupe, "▢", 15.0, "Loupe (E)", Action::LoupeView),
            (View::Compare, "XY", 12.0, "Compare (C)", Action::CompareView),
            (View::Survey, "▦▦", 12.0, "Survey (N)", Action::SurveyView),
        ];
        for (v, icon, size, tip, action) in views {
            if ui
                .add(egui::Button::new(RichText::new(icon).size(size)).selected(app.view == v))
                .on_hover_text(tip)
                .clicked()
            {
                app.run(Command::Action(action));
            }
        }
        ui.separator();
        ui.label(RichText::new("Sort:").color(theme::TEXT_WEAK));
        let mut sort = app.ui.sort;
        let dir = if sort.descending { "Z↑A" } else { "A↓Z" };
        if ui
            .add(egui::Button::new(RichText::new(dir).size(10.5)).frame(false))
            .on_hover_text("Reverse sort direction")
            .clicked()
        {
            sort.descending = !sort.descending;
        }
        egui::ComboBox::from_id_salt("sort_key").selected_text(sort.key.label()).width(110.0).show_ui(
            ui,
            |ui| {
                for key in catalog::SortKey::ALL {
                    ui.selectable_value(&mut sort.key, key, key.label());
                }
            },
        );
        if sort != app.ui.sort {
            app.run(Command::SetSort(sort));
        }
        ui.separator();

        // Flag / rating / label controls for the active photo (or selection).
        let (flag, rating, label) = app
            .session
            .as_ref()
            .and_then(|s| s.active_photo())
            .map_or((Flag::Unflagged, 0, None), |p| (p.flag, p.rating, p.color_label.clone()));
        for (target, fill, tip) in [
            (Flag::Picked, theme::PICK, "Flag as Pick (P)"),
            (Flag::Rejected, theme::REJECT, "Set as Rejected (X)"),
        ] {
            let (resp, rect) = icons::icon_button(ui, egui::vec2(22.0, 20.0), flag == target);
            icons::flag(
                ui.painter(),
                rect.left_center() + egui::vec2(7.0, 0.0),
                12.0,
                fill,
                Color32::from_gray(0x15),
                target == Flag::Rejected,
            );
            if resp.on_hover_text(tip).clicked() {
                let f = if flag == target { Flag::Unflagged } else { target };
                app.run(Command::Action(Action::SetFlag { flag: f, advance: false }));
            }
        }
        ui.add_space(4.0);
        for star in 1..=5u8 {
            let (resp, rect) = icons::icon_button(ui, egui::vec2(15.0, 20.0), false);
            let lit = star <= rating;
            let color = if lit { theme::TEXT_STRONG } else { theme::TEXT_WEAK };
            icons::star(ui.painter(), rect.center(), 5.5, lit, color);
            if resp.on_hover_text(format!("Set rating to {star} ({star})")).clicked() {
                let stars = if star == rating { 0 } else { star };
                app.run(Command::Action(Action::SetRating { stars, advance: false }));
            }
        }
        ui.add_space(4.0);
        for (name, key) in [("red", 6), ("yellow", 7), ("green", 8), ("blue", 9)] {
            let c = theme::label_color(name).unwrap();
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::click());
            ui.painter().rect_filled(rect, 1.0, c);
            if label.as_deref() == Some(name) {
                ui.painter().rect_stroke(
                    rect.expand(1.5),
                    1.0,
                    Stroke::new(1.0, theme::TEXT_STRONG),
                    egui::StrokeKind::Outside,
                );
            }
            if resp.on_hover_text(format!("Toggle {name} label ({key})")).clicked() {
                app.run(Command::Action(Action::ColorLabel { label: name, advance: false }));
            }
        }

        if app.view == View::Grid {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut app.ui.thumb_size, 80.0..=360.0).show_value(false));
                ui.label(RichText::new("Thumbnails").color(theme::TEXT_WEAK));
            });
        }
    });
}

/// What a member of a stack is, for badges and the version strip: "Original", "IR",
/// "IR mask", "JPEG", "NegPy", "Luminar", "Copy 1", "Export"…
pub fn member_label(photo: &PhotoRow) -> String {
    let stem = photo.path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ext = photo.path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
    match photo.unit.role {
        catalog::Role::Original => "Original".into(),
        catalog::Role::Pass if stem.ends_with("_ir_valid") => "IR mask".into(),
        catalog::Role::Pass if stem.ends_with("_ir") => "IR".into(),
        catalog::Role::Pass if ext == "JPG" || ext == "JPEG" => "JPEG".into(),
        catalog::Role::Pass => ext,
        catalog::Role::Edit if stem.contains("_negpy") => "NegPy".into(),
        catalog::Role::Edit if stem.contains("_luminarneo-edit") => "Luminar".into(),
        catalog::Role::Edit if stem.contains("_darktable") => "darktable".into(),
        catalog::Role::Edit => "Edit".into(),
        catalog::Role::Copy => photo.virtual_copy.clone().unwrap_or_else(|| "Copy".into()),
        catalog::Role::Export => "Export".into(),
    }
}

/// Compare with Original: a picture's original (left) and the member that was shown
/// (right), zoom and position linked. Esc or G leaves.
fn member_compare(
    app: &mut FotaroApp,
    ui: &mut egui::Ui,
    rect: Rect,
    original: &PhotoRow,
    other: &PhotoRow,
) {
    let gap = 6.0;
    let half = (rect.width() - gap) / 2.0;
    let panes = [
        (Rect::from_min_size(rect.min, egui::vec2(half, rect.height())), original, "Before"),
        (
            Rect::from_min_size(rect.min + egui::vec2(half + gap, 0.0), egui::vec2(half, rect.height())),
            other,
            "After",
        ),
    ];
    for (r, photo, salt) in panes {
        let header = Rect::from_min_size(r.min, egui::vec2(r.width(), 24.0));
        let body = Rect::from_min_max(egui::pos2(r.left(), header.bottom()), r.max);
        let painter = ui.painter();
        painter.rect_filled(header, 0.0, Color32::from_gray(0x38));
        painter.text(
            header.left_center() + egui::vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            format!("{}   {}", member_label(photo), photo.file_name),
            FontId::proportional(12.0),
            theme::TEXT_STRONG,
        );
        photo_pane(app, ui, body, photo, salt, salt == "After", true);
    }
}

/// The version strip (above the filmstrip): every member of the active unit, picture by
/// picture, with its role. Click shows a member (the grid cell keeps its photo);
/// double-click or Shift+S makes it the one shown for the stack.
pub fn version_strip(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let members: Vec<catalog::Member> = s.strip_members().to_vec();
    let shown = s.active_photo().map(|p| p.id);
    let mut actions = Vec::new();
    let mut view = None;
    // The panel's height, measured before the row (a row is only as tall as a line).
    let height = (ui.available_height() - 8.0).max(40.0);
    ui.horizontal(|ui| {
        ui.set_min_height(height);
        ui.add_space(6.0);
        let tile = egui::vec2(height * 1.1, height);
        let mut last_picture = None;
        for m in &members {
            let photo = &m.photo;
            if last_picture.is_some_and(|p| p != photo.unit.picture_id) {
                // Between the pictures of a group.
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, height), Sense::hover());
                ui.painter().vline(r.center().x, r.y_range().shrink(6.0), Stroke::new(1.0, theme::TEXT_WEAK));
            }
            last_picture = Some(photo.unit.picture_id);
            let (rect, resp) = ui.allocate_exact_size(tile, Sense::click());
            let painter = ui.painter_at(rect);
            let is_shown = shown == Some(photo.id);
            let bg = if is_shown { theme::CELL_ACTIVE } else { Color32::from_rgb(0x3a, 0x3a, 0x3a) };
            painter.rect_filled(rect.shrink(2.0), 2.0, bg);
            let label_h = 15.0;
            let image_rect = Rect::from_min_max(
                rect.min + egui::vec2(5.0, 5.0),
                rect.max - egui::vec2(5.0, label_h + 2.0),
            );
            let tint = if m.picture_active { Color32::WHITE } else { Color32::from_white_alpha(150) };
            match s.thumbs.get(photo.id) {
                Some(tex) => {
                    let r = fit_rect(rotated_size(tex.size_vec2(), photo.rotation), image_rect);
                    paint_image(&painter, &tex, r, photo.rotation, tint);
                }
                None if !photo.missing => s.request(photo, PreviewSize::Thumb),
                None => {}
            }
            let mut label = member_label(photo);
            if m.active && m.picture_active {
                label = format!("● {label}");
            }
            painter.text(
                egui::pos2(rect.center().x, rect.bottom() - label_h / 2.0 - 2.0),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(10.5),
                if is_shown { Color32::from_gray(0x20) } else { theme::TEXT },
            );
            let resp = resp.on_hover_text(format!(
                "{}\nClick: show it   Double-click / Shift+S: show it for the stack",
                photo.file_name
            ));
            if resp.double_clicked() {
                view = Some(photo.clone());
                actions.push(Action::MoveToTopOfStack);
            } else if resp.clicked() {
                view = Some(photo.clone());
            }
            resp.context_menu(|ui| {
                if ui.button("Show for the Stack (Shift+S)").clicked() {
                    view = Some(photo.clone());
                    actions.push(Action::MoveToTopOfStack);
                    ui.close();
                }
                if ui.button("Compare with Original").clicked() {
                    view = Some(photo.clone());
                    actions.push(Action::CompareWithOriginal);
                    ui.close();
                }
                if ui.button("Detach from Picture").clicked() {
                    view = Some(photo.clone());
                    actions.push(Action::DetachFromPicture);
                    ui.close();
                }
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if ui.button("Hide").on_hover_text("Hide the version strip (S)").clicked() {
                actions.push(Action::ToggleStack);
            }
            if ui.button("Compare with Original").clicked() {
                actions.push(Action::CompareWithOriginal);
            }
        });
    });
    if let Some(photo) = view
        && let Some(s) = &mut app.session
    {
        s.view_member(photo);
    }
    for a in actions {
        app.run(Command::Action(a));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn rotation_maps_source_corners_clockwise() {
        // Unrotated: identity.
        assert_eq!(corner_uvs(0), [pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0), pos2(0.0, 1.0)]);
        // 90° clockwise: the source's bottom-left ends up top-left, its top-left top-right.
        assert_eq!(corner_uvs(1), [pos2(0.0, 1.0), pos2(0.0, 0.0), pos2(1.0, 0.0), pos2(1.0, 1.0)]);
        // 180°: diagonally opposite corners; 4 turns are back to identity.
        assert_eq!(corner_uvs(2)[0], pos2(1.0, 1.0));
        assert_eq!(corner_uvs(3)[0], pos2(1.0, 0.0), "90° CCW: source top-right becomes top-left");
        assert_eq!(corner_uvs(4), corner_uvs(0));
        assert_eq!(rotated_size(egui::vec2(300.0, 200.0), 1), egui::vec2(200.0, 300.0));
        assert_eq!(rotated_size(egui::vec2(300.0, 200.0), 2), egui::vec2(300.0, 200.0));
    }

    #[test]
    fn survey_layout_keeps_photos_large() {
        let wide = egui::vec2(1500.0, 800.0);
        assert_eq!(survey_columns(1, wide, 1.5), 1);
        assert_eq!(survey_columns(2, wide, 1.5), 2, "side by side on a wide screen");
        assert_eq!(survey_columns(4, wide, 1.5), 2, "2 x 2");
        assert_eq!(survey_columns(2, egui::vec2(600.0, 1000.0), 1.5), 1, "stacked on a tall area");
        assert_eq!(survey_columns(12, wide, 1.5), 4);
    }

    #[test]
    fn zoomed_view_stays_inside_the_photo() {
        // Showing 25% of the width: the center can go from 0.125 to 0.875.
        let v = egui::vec2(0.25, 2.0);
        assert_eq!(clamp_center(egui::vec2(0.0, 0.3), v), egui::vec2(0.125, 0.5), "y fits: centered");
        assert_eq!(clamp_center(egui::vec2(0.99, 0.3), v).x, 0.875);
        assert_eq!(clamp_center(egui::vec2(0.4, 0.3), v).x, 0.4);
    }
}
