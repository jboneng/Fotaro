//! Crop & Straighten (LrC's crop overlay, R): the whole turned photo with the crop
//! rectangle over it, handles to resize it, a drag inside to move it and outside to turn
//! the photo; the Straighten tool; Aspect presets with a lock; Constrain to Image.
//!
//! The crop is kept as 0..1 of the frame (the photo as the Transform shows it, before
//! cropping); the photo on screen may also be rotated by the catalog, so points go
//! through [`Frame`].

use eframe::egui::{self, Align, Color32, Layout, Pos2, Rect, RichText, Stroke, Vec2};
use imaging::develop::{Crop, MAX_CROP_ANGLE, Slider};

use super::develop::{Spec, rotate, slider_row, unrotate, weak};
use super::masking::{header, pill};
use super::theme;
use crate::app::{Command, FotaroApp};
use crate::develop::{CropDrag, CropHandle, CropTool, DevelopState};

/// Screen ↔ frame (0..1 across and down) for the photo drawn in `img_rect`.
struct Frame {
    img_rect: Rect,
    rotation: u8,
}

impl Frame {
    fn screen(&self, p: [f32; 2]) -> Pos2 {
        let (u, v) = rotate((p[0], p[1]), self.rotation);
        self.img_rect.min + egui::vec2(u, v) * self.img_rect.size()
    }

    fn frame(&self, q: Pos2) -> [f32; 2] {
        let (x, y) = unrotate((q - self.img_rect.min) / self.img_rect.size(), self.rotation);
        [x, y]
    }

    /// Screen points per frame unit along the frame's x and y.
    fn scale(&self) -> Vec2 {
        let s = self.img_rect.size();
        if self.rotation % 2 == 1 { egui::vec2(s.y, s.x) } else { s }
    }

    /// The crop rectangle on screen.
    fn rect(&self, c: &Crop) -> Rect {
        Rect::from_two_pos(self.screen([c.left, c.top]), self.screen([c.right, c.bottom]))
    }
}

/// How close (screen points) counts as on a corner or an edge.
const CORNER: f32 = 12.0;
const EDGE: f32 = 7.0;
/// The smallest crop, as a share of the frame.
const MIN_SIZE: f32 = 0.02;

/// What a drag starting at `p` does.
fn hit(c: &Crop, f: &Frame, p: Pos2) -> CropHandle {
    let q = f.frame(p);
    let s = f.scale();
    let (dl, dr) = ((q[0] - c.left).abs() * s.x, (q[0] - c.right).abs() * s.x);
    let (dt, db) = ((q[1] - c.top).abs() * s.y, (q[1] - c.bottom).abs() * s.y);
    let along_x = q[0] > c.left - EDGE / s.x && q[0] < c.right + EDGE / s.x;
    let along_y = q[1] > c.top - EDGE / s.y && q[1] < c.bottom + EDGE / s.y;
    match () {
        _ if dl < CORNER && dt < CORNER => CropHandle::TopLeft,
        _ if dr < CORNER && dt < CORNER => CropHandle::TopRight,
        _ if dl < CORNER && db < CORNER => CropHandle::BottomLeft,
        _ if dr < CORNER && db < CORNER => CropHandle::BottomRight,
        _ if dl < EDGE && along_y => CropHandle::Left,
        _ if dr < EDGE && along_y => CropHandle::Right,
        _ if dt < EDGE && along_x => CropHandle::Top,
        _ if db < EDGE && along_x => CropHandle::Bottom,
        _ if q[0] > c.left && q[0] < c.right && q[1] > c.top && q[1] < c.bottom => CropHandle::Move,
        _ => CropHandle::Rotate,
    }
}

/// The crop after dragging `handle` from `grab` to `to` (frame units), from `original`,
/// for a frame of `aspect`; `lock` keeps width / height; `constrain` keeps it on the photo.
fn drag_crop(
    original: Crop,
    handle: CropHandle,
    grab: [f32; 2],
    to: [f32; 2],
    aspect: f32,
    lock: Option<f32>,
    constrain: bool,
) -> Crop {
    use CropHandle::*;
    let (dx, dy) = (to[0] - grab[0], to[1] - grab[1]);
    let o = original;
    let mut c = o;
    match handle {
        Move => {
            let (w, h) = (o.right - o.left, o.bottom - o.top);
            c.left = (o.left + dx).clamp(0.0, 1.0 - w);
            c.top = (o.top + dy).clamp(0.0, 1.0 - h);
            (c.right, c.bottom) = (c.left + w, c.top + h);
        }
        Rotate => {}
        _ => {
            let x = |v: f32| v.clamp(0.0, 1.0);
            if matches!(handle, Left | TopLeft | BottomLeft) {
                c.left = x(o.left + dx).min(o.right - MIN_SIZE);
            }
            if matches!(handle, Right | TopRight | BottomRight) {
                c.right = x(o.right + dx).max(o.left + MIN_SIZE);
            }
            if matches!(handle, Top | TopLeft | TopRight) {
                c.top = x(o.top + dy).min(o.bottom - MIN_SIZE);
            }
            if matches!(handle, Bottom | BottomLeft | BottomRight) {
                c.bottom = x(o.bottom + dy).max(o.top + MIN_SIZE);
            }
            if let Some(ratio) = lock {
                c = keep_ratio(c, handle, ratio, aspect);
            }
        }
    }
    if constrain && !c.inside(aspect) && o.inside(aspect) {
        // As far towards the new rectangle as stays on the photo.
        let mix = |t: f32| Crop {
            left: o.left + t * (c.left - o.left),
            top: o.top + t * (c.top - o.top),
            right: o.right + t * (c.right - o.right),
            bottom: o.bottom + t * (c.bottom - o.bottom),
            angle: o.angle,
        };
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..20 {
            let mid = (lo + hi) / 2.0;
            if mix(mid).inside(aspect) { lo = mid } else { hi = mid }
        }
        c = mix(lo);
    }
    c
}

/// Make `c` `ratio` wide per high (in frame units, `aspect` wide per high) after `handle`
/// moved: corners keep the opposite corner, edges the centre across them.
fn keep_ratio(c: Crop, handle: CropHandle, ratio: f32, aspect: f32) -> Crop {
    use CropHandle::*;
    let (w, h) = ((c.right - c.left) * aspect, c.bottom - c.top);
    let mut out = c;
    match handle {
        Left | Right => {
            let (cy, hh) = ((c.top + c.bottom) / 2.0, (w / ratio / 2.0).min(0.5));
            let cy = cy.clamp(hh, 1.0 - hh);
            (out.top, out.bottom) = (cy - hh, cy + hh);
            // Too tall for the frame: narrow back to fit.
            let w = hh * 2.0 * ratio / aspect;
            if matches!(handle, Left) { out.left = c.right - w } else { out.right = c.left + w }
        }
        Top | Bottom => {
            let (cx, hw) = ((c.left + c.right) / 2.0, (h * ratio / aspect / 2.0).min(0.5));
            let cx = cx.clamp(hw, 1.0 - hw);
            (out.left, out.right) = (cx - hw, cx + hw);
            let h = hw * 2.0 * aspect / ratio;
            if matches!(handle, Top) { out.top = c.bottom - h } else { out.bottom = c.top + h }
        }
        _ => {
            // The larger of the two sizes wins; the opposite corner stays.
            let (mut w, mut h) = if w / ratio > h { (w, w / ratio) } else { (h * ratio, h) };
            let (ax, ay) = match handle {
                TopLeft => (c.right, c.bottom),
                TopRight => (c.left, c.bottom),
                BottomLeft => (c.right, c.top),
                _ => (c.left, c.top),
            };
            let left = matches!(handle, TopLeft | BottomLeft);
            let up = matches!(handle, TopLeft | TopRight);
            // Room in the frame from the anchor.
            let room_w = if left { ax } else { 1.0 - ax } * aspect;
            let room_h = if up { ay } else { 1.0 - ay };
            let fit = (room_w / w).min(room_h / h).min(1.0);
            (w, h) = (w * fit, h * fit);
            let w = w / aspect;
            (out.left, out.right) = if left { (ax - w, ax) } else { (ax, ax + w) };
            (out.top, out.bottom) = if up { (ay - h, ay) } else { (ay, ay + h) };
        }
    }
    out
}

/// The angle the Straighten line from `a` to `b` (screen) asks for, from `angle`: the
/// line becomes level or plumb, whichever it is nearer.
fn straighten(angle: f32, a: Pos2, b: Pos2) -> f32 {
    let phi = (b - a).angle();
    let quarter = std::f32::consts::FRAC_PI_2;
    let dev = phi - (phi / quarter).round() * quarter;
    (angle - dev.to_degrees()).clamp(-MAX_CROP_ANGLE, MAX_CROP_ANGLE)
}

/// Crop & Straighten on the photo. Returns true when a change is finished (to be saved).
pub(super) fn canvas(
    ui: &egui::Ui,
    resp: &egui::Response,
    d: &mut DevelopState,
    rotation: u8,
    img_rect: Rect,
    painter: &egui::Painter,
) -> bool {
    let Some(aspect) = d.crop_frame_aspect() else { return false };
    let Some(mut tool) = d.cropping.clone() else { return false };
    let f = Frame { img_rect, rotation };
    let mut st = d.settings.clone();
    let mut done = false;
    let crop = st.transform.crop;
    let lock = d.crop_lock.then(|| crop.ratio(aspect));

    if resp.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        tool.drag = Some(if tool.straighten {
            CropDrag::Straighten { start: origin }
        } else {
            CropDrag::Handle { handle: hit(&crop, &f, origin), grab: f.frame(origin), original: crop }
        });
    }
    let pointer = ui.input(|i| i.pointer.latest_pos());
    match (tool.drag, pointer) {
        (Some(CropDrag::Handle { handle: CropHandle::Rotate, grab, original }), Some(q)) => {
            // Turn about the crop's centre, following the pointer.
            let centre =
                f.screen([(original.left + original.right) / 2.0, (original.top + original.bottom) / 2.0]);
            let pi = std::f32::consts::PI;
            let turn =
                ((q - centre).angle() - (f.screen(grab) - centre).angle() + pi).rem_euclid(2.0 * pi) - pi;
            let mut c = original;
            c.angle = (original.angle + turn.to_degrees()).clamp(-MAX_CROP_ANGLE, MAX_CROP_ANGLE);
            if d.crop_constrain {
                c.fit_inside(aspect);
            }
            st.transform.crop = c;
        }
        (Some(CropDrag::Handle { handle, grab, original }), Some(q)) => {
            st.transform.crop = drag_crop(original, handle, grab, f.frame(q), aspect, lock, d.crop_constrain);
        }
        _ => {}
    }
    if resp.drag_stopped()
        && let Some(drag) = tool.drag.take()
    {
        match (drag, pointer) {
            (CropDrag::Straighten { start }, Some(q)) if start.distance(q) > 10.0 => {
                let mut c = st.transform.crop;
                c.angle = straighten(c.angle, start, q);
                if d.crop_constrain {
                    c.fit_inside(aspect);
                }
                st.transform.crop = c;
                tool.straighten = false;
                done = true;
            }
            (CropDrag::Handle { .. }, _) => done = true,
            _ => {}
        }
    }

    // The rectangle: dimmed outside, a border, thirds, handles.
    let c = st.transform.crop;
    let r = f.rect(&c);
    let shown = img_rect.intersect(painter.clip_rect());
    let painter = painter.with_clip_rect(shown);
    let dim = Color32::from_black_alpha(150);
    for outside in [
        Rect::from_min_max(shown.min, egui::pos2(shown.max.x, r.top())),
        Rect::from_min_max(egui::pos2(shown.min.x, r.bottom()), shown.max),
        Rect::from_min_max(egui::pos2(shown.min.x, r.top()), egui::pos2(r.left(), r.bottom())),
        Rect::from_min_max(egui::pos2(r.right(), r.top()), egui::pos2(shown.max.x, r.bottom())),
    ] {
        if outside.is_positive() {
            painter.rect_filled(outside, 0.0, dim);
        }
    }
    let grid = Stroke::new(1.0, Color32::from_white_alpha(if tool.drag.is_some() { 110 } else { 60 }));
    for k in [1.0 / 3.0, 2.0 / 3.0] {
        painter.line_segment(
            [egui::pos2(r.left() + r.width() * k, r.top()), egui::pos2(r.left() + r.width() * k, r.bottom())],
            grid,
        );
        painter.line_segment(
            [egui::pos2(r.left(), r.top() + r.height() * k), egui::pos2(r.right(), r.top() + r.height() * k)],
            grid,
        );
    }
    painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_white_alpha(220)), egui::StrokeKind::Middle);
    let handle = Stroke::new(3.0, Color32::WHITE);
    let len = 16.0f32.min(r.width() / 3.0).min(r.height() / 3.0);
    for (corner, sx, sy) in [
        (r.left_top(), 1.0, 1.0),
        (r.right_top(), -1.0, 1.0),
        (r.left_bottom(), 1.0, -1.0),
        (r.right_bottom(), -1.0, -1.0),
    ] {
        painter.line_segment([corner, corner + egui::vec2(sx * len, 0.0)], handle);
        painter.line_segment([corner, corner + egui::vec2(0.0, sy * len)], handle);
    }
    for (mid, horizontal) in [
        (r.center_top(), true),
        (r.center_bottom(), true),
        (r.left_center(), false),
        (r.right_center(), false),
    ] {
        let v = if horizontal { egui::vec2(len / 2.0, 0.0) } else { egui::vec2(0.0, len / 2.0) };
        painter.line_segment([mid - v, mid + v], handle);
    }
    if let (Some(CropDrag::Straighten { start }), Some(q)) = (tool.drag, pointer) {
        painter.line_segment([start, q], Stroke::new(1.5, Color32::from_rgb(255, 205, 60)));
    }

    // The cursor says what a drag would do.
    if let Some(p) = resp.hover_pos().filter(|p| img_rect.expand(40.0).contains(*p)) {
        let flip = rotation % 2 == 1;
        let icon = if tool.straighten {
            egui::CursorIcon::Crosshair
        } else {
            match tool.drag.map_or_else(
                || hit(&c, &f, p),
                |d| match d {
                    CropDrag::Handle { handle, .. } => handle,
                    CropDrag::Straighten { .. } => CropHandle::Move,
                },
            ) {
                CropHandle::Left | CropHandle::Right if !flip => egui::CursorIcon::ResizeHorizontal,
                CropHandle::Top | CropHandle::Bottom if flip => egui::CursorIcon::ResizeHorizontal,
                CropHandle::Left | CropHandle::Right | CropHandle::Top | CropHandle::Bottom => {
                    egui::CursorIcon::ResizeVertical
                }
                CropHandle::TopLeft | CropHandle::BottomRight => egui::CursorIcon::ResizeNwSe,
                CropHandle::TopRight | CropHandle::BottomLeft => egui::CursorIcon::ResizeNeSw,
                CropHandle::Move if tool.drag.is_some() => egui::CursorIcon::Grabbing,
                CropHandle::Move => egui::CursorIcon::Grab,
                CropHandle::Rotate => egui::CursorIcon::AllScroll,
            }
        };
        ui.ctx().set_cursor_icon(icon);
    }

    d.cropping = Some(tool);
    if st != d.settings {
        d.set_live(st);
    }
    done
}

/// LrC's aspect presets, as width / height of a landscape crop (`None`: the photo's own).
const ASPECTS: [(&str, Option<f32>); 9] = [
    ("Original", None),
    ("1 x 1", Some(1.0)),
    ("4 x 5 / 8 x 10", Some(1.25)),
    ("8.5 x 11", Some(11.0 / 8.5)),
    ("5 x 7", Some(1.4)),
    ("2 x 3 / 4 x 6", Some(1.5)),
    ("4 x 3", Some(4.0 / 3.0)),
    ("16 x 9", Some(16.0 / 9.0)),
    ("16 x 10", Some(1.6)),
];

/// The preset the crop's shape matches, if any.
fn aspect_label(ratio: f32, aspect: f32) -> &'static str {
    let landscape = if ratio < 1.0 { 1.0 / ratio } else { ratio };
    ASPECTS
        .iter()
        .find(|(_, r)| {
            (r.unwrap_or(if aspect < 1.0 { 1.0 / aspect } else { aspect }) - landscape).abs() < 0.01
        })
        .map_or("Custom", |(label, _)| label)
}

/// The crop with a preset's shape, the way round the crop is now.
fn with_preset(c: &Crop, preset: Option<f32>, aspect: f32, constrain: bool) -> Crop {
    let landscape = preset.unwrap_or(if aspect < 1.0 { 1.0 / aspect } else { aspect });
    let portrait = c.ratio(aspect) < 1.0;
    let ratio = if portrait { 1.0 / landscape } else { landscape };
    let mut out = c.with_ratio(ratio, aspect);
    if constrain {
        out.fit_inside(aspect);
    }
    out
}

/// The right panel with the crop tool open: Aspect and its lock, Angle, Straighten and
/// Auto, Constrain to Image, Reset.
pub(super) fn right_panel(app: &mut FotaroApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    let Some(mut tool) = d.cropping.clone() else { return };
    let Some(aspect) = d.crop_frame_aspect() else { return };
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;
    let mut leave = false;
    let mut auto = false;
    header(ui, "Crop & Straighten", None);
    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let crop = st.transform.crop;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Aspect").size(12.5).color(theme::TEXT));
            let current = aspect_label(crop.ratio(aspect), aspect);
            egui::ComboBox::from_id_salt("crop_aspect").selected_text(current).width(130.0).show_ui(ui, |ui| {
                for (label, preset) in ASPECTS {
                    if ui.selectable_label(current == label, label).clicked() {
                        st.transform.crop = with_preset(&crop, preset, aspect, d.crop_constrain);
                        d.crop_lock = true;
                        done = Some(Some(format!("Crop: {label}")));
                    }
                }
                if ui.selectable_label(current == "Custom", "Custom").clicked() {
                    d.crop_lock = false;
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.checkbox(&mut d.crop_lock, "Lock").on_hover_text("Keep the crop's shape while resizing");
            });
        });
        ui.add_space(4.0);
        let spec = Spec {
            slider: Slider::CropAngle,
            range: Slider::CropAngle.range(false),
            default: 0.0,
            log: false,
            gradient: None,
            kelvin: false,
        };
        let mut angle = crop.angle;
        let row = slider_row(ui, &spec, &mut angle);
        if row.changed {
            st.transform.crop.angle = angle;
            if d.crop_constrain {
                st.transform.crop.fit_inside(aspect);
            }
        }
        if row.done {
            done = Some(None);
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(super::develop::LABEL_W - 40.0);
            let label = if tool.straighten { "Straighten: draw a line" } else { "Straighten" };
            if pill(ui, label, false).on_hover_text("Draw along something that should be level or plumb").clicked() {
                tool.straighten = !tool.straighten;
            }
            if pill(ui, "Auto", false).on_hover_text("Level the photo from its lines").clicked() {
                auto = true;
            }
        });
        ui.add_space(4.0);
        if ui
            .checkbox(&mut d.crop_constrain, "Constrain to Image")
            .on_hover_text("Keep the crop on the photo (no white corners when turned)")
            .changed()
            && d.crop_constrain
            && !st.transform.crop.inside(aspect)
        {
            st.transform.crop.fit_inside(aspect);
            done = Some(None);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if pill(ui, "Reset", false).on_hover_text("The whole photo, not turned (Ctrl+Alt+R)").clicked() {
                st.transform.crop = Crop::default();
                done = Some(Some("Reset Crop".into()));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pill(ui, "Close", false).on_hover_text("Done cropping (R or Enter)").clicked() {
                    leave = true;
                }
            });
        });
        ui.add_space(6.0);
        ui.label(weak("Drag the edges or corners to crop, inside to move it, outside to turn the photo. X swaps portrait and landscape."));
    });
    d.cropping = (!leave).then_some(tool);
    if st != d.settings {
        d.set_live(st);
    }
    if leave {
        d.request_renders();
    }
    if auto {
        d.auto_straighten();
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
}

/// Open or close the crop tool (R); masking closes.
pub fn set_cropping(d: &mut DevelopState, on: bool) {
    d.cropping = on.then(CropTool::default);
    if on {
        d.masking = None;
        d.removing = None;
        d.picking_wb = false;
        d.picking_detail = false;
        d.picking_focus = false;
    }
    d.request_renders();
}

/// X with the crop tool open: the crop turned between portrait and landscape.
pub fn swap_orientation(d: &mut DevelopState) -> bool {
    let Some(aspect) = d.crop_frame_aspect() else { return false };
    let mut st = d.settings.clone();
    let c = st.transform.crop;
    let mut out = c.with_ratio(1.0 / c.ratio(aspect), aspect);
    if d.crop_constrain {
        out.fit_inside(aspect);
    }
    st.transform.crop = out;
    d.set_live(st);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: f32 = 1.5;

    fn frame() -> Frame {
        Frame { img_rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(300.0, 200.0)), rotation: 0 }
    }

    #[test]
    fn handles_are_found_where_drawn() {
        let c = Crop { left: 0.2, top: 0.2, right: 0.8, bottom: 0.8, angle: 0.0 };
        let f = frame();
        assert_eq!(hit(&c, &f, egui::pos2(60.0, 40.0)), CropHandle::TopLeft);
        assert_eq!(hit(&c, &f, egui::pos2(240.0, 100.0)), CropHandle::Right);
        assert_eq!(hit(&c, &f, egui::pos2(150.0, 160.0)), CropHandle::Bottom);
        assert_eq!(hit(&c, &f, egui::pos2(150.0, 100.0)), CropHandle::Move);
        assert_eq!(hit(&c, &f, egui::pos2(20.0, 100.0)), CropHandle::Rotate);
        // A quarter turn in the catalog: the frame's left edge is drawn at the top.
        let turned = Frame { rotation: 1, ..f };
        let top = turned.screen([0.2, 0.5]);
        assert_eq!(hit(&c, &turned, top), CropHandle::Left);
    }

    #[test]
    fn dragging_resizes_moves_and_keeps_the_shape() {
        let full = Crop::default();
        // A free corner drag.
        let c = drag_crop(full, CropHandle::TopLeft, [0.0, 0.0], [0.25, 0.1], A, None, true);
        assert_eq!((c.left, c.top, c.right, c.bottom), (0.25, 0.1, 1.0, 1.0));
        // Locked: the original 3:2 shape stays, the opposite corner too.
        let l = drag_crop(full, CropHandle::TopLeft, [0.0, 0.0], [0.25, 0.1], A, Some(A), true);
        assert!((l.ratio(A) - A).abs() < 1e-3 && l.right == 1.0 && l.bottom == 1.0, "{l:?}");
        // An edge with the lock: the height follows about the centre.
        let e = drag_crop(full, CropHandle::Right, [1.0, 0.5], [0.7, 0.5], A, Some(A), true);
        assert!((e.ratio(A) - A).abs() < 1e-3 && ((e.top + e.bottom) / 2.0 - 0.5).abs() < 1e-4, "{e:?}");
        // Moving stays in the frame.
        let half = Crop { right: 0.5, ..Crop::default() };
        let m = drag_crop(half, CropHandle::Move, [0.25, 0.5], [0.95, 0.5], A, None, true);
        assert_eq!((m.left, m.right), (0.5, 1.0));
        // Turned and constrained: a corner can't be pulled off the photo.
        let mut t = Crop { angle: 8.0, ..Crop::default() };
        t.fit_inside(A);
        let out = drag_crop(t, CropHandle::TopLeft, [t.left, t.top], [0.0, 0.0], A, None, true);
        assert!(out.inside(A) && out.left < t.left + 1e-6);
    }

    #[test]
    fn straighten_levels_the_line_drawn() {
        // A line rising 5° to the right (screen y up is negative): turn 5° clockwise… back.
        let a = egui::pos2(0.0, 0.0);
        let b = a + egui::vec2(100.0, -100.0 * 5f32.to_radians().tan());
        assert!((straighten(0.0, a, b) - 5.0).abs() < 1e-3);
        // A nearly plumb line: made plumb.
        let c = a + egui::vec2(100.0 * 3f32.to_radians().tan(), 100.0);
        assert!((straighten(2.0, a, c) - 5.0).abs() < 1e-3, "{}", straighten(2.0, a, c));
    }

    #[test]
    fn presets_follow_the_crops_orientation() {
        let c = Crop::default();
        let sq = with_preset(&c, Some(1.0), A, true);
        assert!((sq.ratio(A) - 1.0).abs() < 1e-4);
        let portrait = Crop { left: 0.4, right: 0.6, ..Crop::default() };
        let p = with_preset(&portrait, Some(1.5), A, true);
        assert!((p.ratio(A) - 1.0 / 1.5).abs() < 1e-3);
        assert_eq!(aspect_label(1.5, A), "Original");
        assert_eq!(aspect_label(9.0 / 16.0, A), "16 x 9");
        assert_eq!(aspect_label(1.1, A), "Custom");
    }
}
