//! Center views (Grid, Loupe), filmstrip, toolbar, filter bar, full-screen preview.

use catalog::{Flag, PhotoRow};
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Rect, RichText, Sense, Stroke, Vec2};
use imaging::PreviewSize;

use super::shortcuts::Action;
use super::sidebars::{format_capture_time, format_shutter};
use super::{icons, theme};
use crate::app::{Command, GridStyle, PhotoLibraryApp, Session, View};

const UV: Rect = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

/// Largest rect with the image's aspect ratio that fits in `bounds`, centered.
pub fn fit_rect(image: Vec2, bounds: Rect) -> Rect {
    if image.x <= 0.0 || image.y <= 0.0 {
        return bounds;
    }
    let scale = (bounds.width() / image.x).min(bounds.height() / image.y);
    Rect::from_center_size(bounds.center(), image * scale)
}

struct CellState {
    index: usize,
    selected: bool,
    active: bool,
}

/// Stars centered on `center`; `size` is roughly the glyph height.
fn paint_stars(painter: &egui::Painter, center: egui::Pos2, rating: u8, size: f32, color: Color32) {
    icons::stars(painter, center, size * 0.42, rating, color);
}

fn paint_flag(painter: &egui::Painter, left_center: egui::Pos2, flag: Flag, size: f32) {
    match flag {
        Flag::Picked => icons::flag(painter, left_center, size, theme::PICK, Color32::from_gray(0x30), false),
        Flag::Rejected => icons::flag(painter, left_center, size, theme::REJECT, Color32::from_gray(0x10), true),
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
        painter.text(cell.left_top() + egui::vec2(6.0, 2.0), Align2::LEFT_TOP, (st.index + 1).to_string(), FontId::proportional(cell.height() * 0.22), theme::CELL_INDEX);
    }
    if style == GridStyle::Expanded {
        let y = cell.top() + header / 2.0;
        painter.text(egui::pos2(cell.left() + 6.0, y), Align2::LEFT_CENTER, (st.index + 1).to_string(), FontId::proportional(13.0), text_color);
        let ext = photo.path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
        painter.text(egui::pos2(cell.right() - 6.0, y), Align2::RIGHT_CENTER, ext, FontId::proportional(10.5), text_color);
        let name = photo.file_name.rsplit_once('.').map_or(photo.file_name.as_str(), |(n, _)| n);
        painter.text(egui::pos2(cell.left() + 30.0, y), Align2::LEFT_CENTER, name, FontId::proportional(11.0), text_color);
    }

    let margin = (cell.width() * 0.09).max(6.0);
    let image_area = Rect::from_min_max(
        cell.min + egui::vec2(margin, margin * 0.6 + header),
        cell.max - egui::vec2(margin, margin * 0.6 + footer),
    );
    match tex {
        Some(tex) => {
            let r = fit_rect(tex.size_vec2(), image_area);
            painter.rect_filled(r.translate(egui::vec2(1.5, 1.5)), 0.0, Color32::from_black_alpha(90));
            let tint = if photo.flag == Flag::Rejected { Color32::from_white_alpha(90) } else { Color32::WHITE };
            painter.image(tex.id(), r, UV, tint);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_black_alpha(120)), egui::StrokeKind::Outside);
        }
        None => {
            let r = Rect::from_center_size(image_area.center(), image_area.size() * egui::vec2(0.8, 0.6));
            painter.rect_filled(r, 0.0, Color32::from_black_alpha(25));
            if failed || photo.missing {
                painter.text(r.center(), Align2::CENTER_CENTER, "⚠", FontId::proportional(18.0), Color32::from_black_alpha(140));
            }
        }
    }

    if footer > 0.0 {
        let y = cell.bottom() - footer / 2.0 - 2.0;
        paint_flag(&painter, egui::pos2(cell.left() + 7.0, y), photo.flag, 13.0);
        paint_stars(&painter, egui::pos2(cell.center().x, y), photo.rating, 11.0, if st.selected { theme::TEXT_STRONG } else { theme::STAR });
    } else if style == GridStyle::Minimal && photo.flag != Flag::Unflagged {
        paint_flag(&painter, cell.left_top() + egui::vec2(5.0, 9.0), photo.flag, 12.0);
    }
    if photo.missing {
        painter.text(cell.right_top() + egui::vec2(-6.0, header + 4.0), Align2::RIGHT_TOP, "!", FontId::proportional(15.0), theme::MISSING);
    }
}

fn photo_context_menu(resp: &egui::Response, app: &mut PhotoLibraryApp, photo: &PhotoRow) {
    resp.context_menu(|ui| {
        if ui.button(crate::platform::REVEAL_LABEL).clicked() {
            app.run(Command::Reveal(photo.path.clone()));
            ui.close();
        }
        ui.separator();
        ui.menu_button("Set Flag", |ui| {
            for (name, flag) in [("Flagged", Flag::Picked), ("Unflagged", Flag::Unflagged), ("Rejected", Flag::Rejected)] {
                if ui.button(name).clicked() {
                    app.run(Command::Action(Action::SetFlag { flag, advance: false }));
                    ui.close();
                }
            }
        });
        ui.menu_button("Set Rating", |ui| {
            for stars in 0..=5u8 {
                let label = match stars { 0 => "None".to_string(), 1 => "1 Star".to_string(), n => format!("{n} Stars") };
                if ui.button(label).clicked() {
                    app.run(Command::Action(Action::SetRating { stars, advance: false }));
                    ui.close();
                }
            }
        });
        ui.menu_button("Set Color Label", |ui| {
            for (label, name) in [("red", "Red"), ("yellow", "Yellow"), ("green", "Green"), ("blue", "Blue")] {
                if ui.button(name).clicked() {
                    app.run(Command::Action(Action::ColorLabel { label, advance: false }));
                    ui.close();
                }
            }
        });
        ui.separator();
        let _ = ui.add_enabled(false, egui::Button::new("Edit In")).on_disabled_hover_text("Arrives in Phase 7");
    });
}

/// Handle a click on a photo (cell or filmstrip frame).
fn handle_click(app: &mut PhotoLibraryApp, resp: &egui::Response, photo: &PhotoRow, open_loupe_on_double: bool) {
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

pub fn grid(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let style = app.ui.grid_style;
    let cell_w = app.ui.thumb_size.clamp(80.0, 360.0);
    let cell_h = cell_w * if style == GridStyle::Expanded { 1.18 } else { 1.0 };
    let avail = ui.available_width() - 8.0;
    let cols = ((avail / cell_w).floor() as usize).max(1);
    let cell_w = avail / cols as f32;
    app.grid_cols = cols;

    let Some(s) = &mut app.session else { return };
    if s.photos.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("No photos here yet.\nUse File ▸ Import Photos (Add Folder) to add some.").color(theme::TEXT_WEAK).size(14.0));
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
                let rect = Rect::from_min_size(row_rect.min + egui::vec2(4.0 + col as f32 * cell_w, 0.0), egui::vec2(cell_w, cell_h));
                let tex = s.thumbs.get(photo.id);
                if tex.is_none() && !photo.missing {
                    s.request(&photo, PreviewSize::Thumb);
                }
                let st = CellState { index: i, selected: s.selected.contains(&photo.id), active: s.active == Some(photo.id) };
                paint_cell(ui, rect, &photo, tex.as_ref(), s.thumbs.is_failed(photo.id), style, &st);
                let resp = ui.interact(rect, egui::Id::unique(("cell", photo.id)), Sense::click());
                let resp = if photo.missing { resp.on_hover_text("The file could not be found.") } else { resp };
                clicks.push((resp, photo));
            }
        }
    });
    app.grid_scroll = (out.state.clamped_offset().y, out.inner_rect.height());

    for (resp, photo) in clicks {
        handle_click(app, &resp, &photo, true);
        photo_context_menu(&resp, app, &photo);
    }
}

fn info_overlay(app: &mut PhotoLibraryApp, ui: &egui::Ui, rect: Rect, photo: &PhotoRow) {
    if app.ui.info_overlay == 0 {
        return;
    }
    let Some(s) = &mut app.session else { return };
    let m = s.active_metadata().unwrap_or_default();
    let line2 = match app.ui.info_overlay {
        1 => {
            let dims = m.width.zip(m.height).map(|(w, h)| format!("{w} x {h}")).unwrap_or_default();
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
    for (i, (text, size)) in [(photo.file_name.as_str(), 15.0), (line2.as_str(), 12.0)].into_iter().enumerate() {
        let p = pos + egui::vec2(0.0, i as f32 * 20.0);
        painter.text(p + egui::vec2(1.0, 1.0), Align2::LEFT_TOP, text, FontId::proportional(size), Color32::from_black_alpha(160));
        painter.text(p, Align2::LEFT_TOP, text, FontId::proportional(size), theme::TEXT_STRONG);
    }
}

/// Draw the best available image for `photo` fitted into `rect`
/// (standard preview, falling back to the thumbnail while it loads).
fn draw_preview(s: &mut Session, ui: &egui::Ui, photo: &PhotoRow, rect: Rect) {
    let tex = s.previews.get(photo.id).or_else(|| {
        if !photo.missing {
            s.request(photo, PreviewSize::Standard);
        }
        s.thumbs.get(photo.id)
    });
    match tex {
        Some(tex) => {
            let r = fit_rect(tex.size_vec2(), rect);
            ui.painter().image(tex.id(), r, UV, Color32::WHITE);
        }
        None => {
            if !photo.missing {
                s.request(photo, PreviewSize::Thumb);
            }
            let msg = if photo.missing { "The file could not be found." } else { "Loading…" };
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), theme::TEXT_WEAK);
        }
    }
}

pub fn loupe(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click());
    let Some(s) = &mut app.session else { return };
    let Some(photo) = s.active_photo().cloned() else {
        app.view = View::Grid;
        return;
    };
    // Prefetch neighbours first: the visible lane is LIFO, so the current photo wins.
    if let Some(i) = s.active_index() {
        for j in [i + 1, i.wrapping_sub(1)] {
            if let Some(p) = s.photos.get(j).cloned()
                && !p.missing
            {
                s.request(&p, PreviewSize::Standard);
            }
        }
    }
    draw_preview(s, ui, &photo, rect.shrink(18.0));
    info_overlay(app, ui, rect, &photo);
    if resp.double_clicked() {
        app.view = View::Grid;
    }
    photo_context_menu(&resp, app, &photo);
}

pub fn fullscreen(app: &mut PhotoLibraryApp, ctx: &egui::Context, full: Rect) {
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

pub fn filmstrip(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
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
    });

    let height = (ui.available_height() - 4.0).max(30.0);
    let item = height * 1.25;
    let n = s.photos.len();
    let mut scroll = egui::ScrollArea::horizontal().id_salt("filmstrip").auto_shrink([false, false]);
    if s.scroll_to_active
        && app.view == View::Loupe
        && let Some(i) = s.active_index()
    {
        scroll = scroll.horizontal_scroll_offset((i as f32 * item - ui.available_width() / 2.0 + item / 2.0).max(0.0));
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
            let rect = Rect::from_min_size(origin + egui::vec2(i as f32 * item, 0.0), egui::vec2(item, height)).shrink(2.0);
            let selected = s.selected.contains(&photo.id);
            let active = s.active == Some(photo.id);
            let bg = if active { theme::CELL_ACTIVE } else if selected { theme::CELL_SELECTED } else { Color32::from_rgb(0x3a, 0x3a, 0x3a) };
            ui.painter().rect_filled(rect, 1.0, bg);
            match s.thumbs.get(photo.id) {
                Some(tex) => {
                    let tint = if photo.flag == Flag::Rejected { Color32::from_white_alpha(90) } else { Color32::WHITE };
                    ui.painter().image(tex.id(), fit_rect(tex.size_vec2(), rect.shrink(5.0)), UV, tint);
                }
                None if !photo.missing => s.request(&photo, PreviewSize::Thumb),
                None => {}
            }
            paint_flag(ui.painter(), rect.left_top() + egui::vec2(4.0, 8.0), photo.flag, 10.0);
            paint_stars(ui.painter(), rect.center_bottom() - egui::vec2(0.0, 6.0), photo.rating, 8.0, theme::TEXT_STRONG);
            let resp = ui.interact(rect, egui::Id::unique(("film", photo.id)), Sense::click());
            clicks.push((resp, photo));
        }
    });
    for (resp, photo) in clicks {
        handle_click(app, &resp, &photo, true);
        photo_context_menu(&resp, app, &photo);
    }
}

pub fn toolbar(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        let views = [(View::Grid, "▦", "Grid (G)"), (View::Loupe, "▢", "Loupe (E)")];
        for (v, icon, tip) in views {
            if ui.add(egui::Button::new(RichText::new(icon).size(15.0)).selected(app.view == v)).on_hover_text(tip).clicked() {
                app.run(Command::Action(if v == View::Grid { Action::GridView } else { Action::LoupeView }));
            }
        }
        let _ = ui.add_enabled(false, egui::Button::new(RichText::new("XY").size(12.0))).on_disabled_hover_text("Compare (C) arrives in Phase 6");
        let _ = ui.add_enabled(false, egui::Button::new(RichText::new("▦▦").size(12.0))).on_disabled_hover_text("Survey (N) arrives in Phase 6");
        ui.separator();
        ui.label(RichText::new("Sort:").color(theme::TEXT_WEAK));
        ui.label("Capture Time");
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
            icons::flag(ui.painter(), rect.left_center() + egui::vec2(7.0, 0.0), 12.0, fill, Color32::from_gray(0x15), target == Flag::Rejected);
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
                ui.painter().rect_stroke(rect.expand(1.5), 1.0, Stroke::new(1.0, theme::TEXT_STRONG), egui::StrokeKind::Outside);
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

pub fn filter_bar(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.add_space((ui.available_width() / 2.0 - 160.0).max(0.0));
        ui.label(RichText::new("Library Filter :").color(theme::TEXT_WEAK));
        for name in ["Text", "Attribute", "Metadata"] {
            if ui.add(egui::Button::new(name).frame(false)).clicked() {
                app.run(Command::Planned("The Library Filter", "Phase 4"));
            }
        }
        ui.add(egui::Button::new(RichText::new("None").color(theme::TEXT_STRONG)).frame(false));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new("Filters Off").color(theme::TEXT_WEAK));
        });
    });
}
