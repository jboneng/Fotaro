//! Vector icons painted with shapes (stars, flags, triangles), so they look the
//! same on every OS and don't depend on font glyph coverage.

use eframe::egui::{self, Color32, Pos2, Sense, Shape, Stroke, Vec2, pos2, vec2};

pub enum Dir {
    Down,
    Left,
    Right,
}

pub fn triangle(painter: &egui::Painter, center: Pos2, r: f32, dir: Dir, color: Color32) {
    let pts = match dir {
        Dir::Down => {
            vec![center + vec2(-r, -r * 0.6), center + vec2(r, -r * 0.6), center + vec2(0.0, r * 0.7)]
        }
        Dir::Right => {
            vec![center + vec2(-r * 0.6, -r), center + vec2(-r * 0.6, r), center + vec2(r * 0.7, 0.0)]
        }
        Dir::Left => {
            vec![center + vec2(r * 0.6, -r), center + vec2(r * 0.6, r), center + vec2(-r * 0.7, 0.0)]
        }
    };
    painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
}

fn star_points(center: Pos2, r: f32) -> Vec<Pos2> {
    (0..10)
        .map(|i| {
            let radius = if i % 2 == 0 { r } else { r * 0.45 };
            let a = std::f32::consts::PI * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect()
}

/// A five-pointed star; outline only when `filled` is false.
pub fn star(painter: &egui::Painter, center: Pos2, r: f32, filled: bool, color: Color32) {
    let pts = star_points(center, r);
    if filled {
        // Fan from the center: a star is not convex.
        for i in 0..10 {
            painter.add(Shape::convex_polygon(
                vec![center, pts[i], pts[(i + 1) % 10]],
                color,
                Stroke::new(0.5, color),
            ));
        }
    } else {
        painter.add(Shape::closed_line(pts, Stroke::new(1.0, color)));
    }
}

/// `count` filled stars centered on `center`.
pub fn stars(painter: &egui::Painter, center: Pos2, r: f32, count: u8, color: Color32) {
    if count == 0 {
        return;
    }
    let step = r * 2.3;
    let start = center.x - step * (count as f32 - 1.0) / 2.0;
    for i in 0..count {
        star(painter, pos2(start + step * i as f32, center.y), r, true, color);
    }
}

/// A pennant flag; `rejected` adds a cross, like LrC's black reject flag.
pub fn flag(
    painter: &egui::Painter,
    left_center: Pos2,
    size: f32,
    fill: Color32,
    outline: Color32,
    rejected: bool,
) {
    let top = left_center + vec2(0.0, -size * 0.5);
    let pole = Stroke::new((size * 0.1).max(1.0), outline);
    painter.line_segment([top, top + vec2(0.0, size)], pole);
    let banner = vec![top, top + vec2(size * 0.8, size * 0.25), top + vec2(0.0, size * 0.55)];
    painter.add(Shape::convex_polygon(banner, fill, Stroke::new(1.0, outline)));
    if rejected {
        let c = top + vec2(size * 0.28, size * 0.27);
        let d = size * 0.13;
        let s = Stroke::new(1.2, Color32::WHITE);
        painter.line_segment([c + vec2(-d, -d), c + vec2(d, d)], s);
        painter.line_segment([c + vec2(-d, d), c + vec2(d, -d)], s);
    }
}

/// A small clickable area for a painted icon; returns its response and paint rect.
pub fn icon_button(ui: &mut egui::Ui, size: Vec2, selected: bool) -> (egui::Response, egui::Rect) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 2.0, Color32::from_rgb(0x58, 0x58, 0x58));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 2.0, Color32::from_rgb(0x48, 0x48, 0x48));
    }
    (resp, rect)
}

/// A plus sign (new mask, add).
pub fn plus(painter: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    let s = Stroke::new(1.5, color);
    painter.line_segment([center + vec2(-r, 0.0), center + vec2(r, 0.0)], s);
    painter.line_segment([center + vec2(0.0, -r), center + vec2(0.0, r)], s);
}

/// An on/off switch (a pill with a knob), `size` wide; returns the click response.
pub fn toggle_switch(ui: &mut egui::Ui, on: bool, size: Vec2) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let t = ui.ctx().animate_bool_responsive(resp.id, on);
    let track = Color32::from_rgb(0x2a, 0x2a, 0x2a).lerp_to_gamma(Color32::from_rgb(0x9a, 0x9a, 0x9a), t);
    let r = rect.height() / 2.0;
    ui.painter().rect_filled(rect, r, track);
    let border = if resp.hovered() { Color32::from_gray(0xb0) } else { Color32::from_gray(0x1a) };
    ui.painter().rect_stroke(rect, r, Stroke::new(1.0, border), egui::StrokeKind::Inside);
    let x = egui::lerp((rect.left() + r)..=(rect.right() - r), t);
    let knob = if on { Color32::from_gray(0xf0) } else { Color32::from_gray(0x8c) };
    ui.painter().circle_filled(pos2(x, rect.center().y), r - 2.0, knob);
    resp
}

/// LrC's Masking tool icon: a circle, half filled, with a dashed edge on the empty half.
pub fn masking(painter: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    let half: Vec<Pos2> = (0..=16)
        .map(|i| {
            let a = std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 16.0;
            center + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    painter.add(Shape::convex_polygon(half, color, Stroke::NONE));
    for i in 0..8 {
        let a0 = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * (i as f32 + 0.15) / 8.0;
        let a1 = a0 + std::f32::consts::PI * 0.6 / 8.0;
        painter.line_segment(
            [center + vec2(a0.cos(), a0.sin()) * r, center + vec2(a1.cos(), a1.sin()) * r],
            Stroke::new(1.3, color),
        );
    }
}

/// The crop tool: two overlapping right angles.
pub fn crop(painter: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    let s = Stroke::new(1.6, color);
    let (a, b) = (r * 0.55, r);
    painter.line_segment([center + vec2(-a, -b), center + vec2(-a, a)], s);
    painter.line_segment([center + vec2(-a, a), center + vec2(b, a)], s);
    painter.line_segment([center + vec2(a, b), center + vec2(a, -a)], s);
    painter.line_segment([center + vec2(a, -a), center + vec2(-b, -a)], s);
}

/// The Remove tool: a plaster (a rounded bar at an angle with a pad).
pub fn remove(painter: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    let k = std::f32::consts::FRAC_1_SQRT_2;
    let d = vec2(k, -k);
    let n = vec2(k, k);
    let corners = [
        center + d * r - n * r * 0.38,
        center + d * r + n * r * 0.38,
        center - d * r + n * r * 0.38,
        center - d * r - n * r * 0.38,
    ];
    painter.add(Shape::closed_line(corners.to_vec(), Stroke::new(1.4, color)));
    let pad = [
        center + d * r * 0.32 - n * r * 0.38,
        center + d * r * 0.32 + n * r * 0.38,
        center - d * r * 0.32 + n * r * 0.38,
        center - d * r * 0.32 - n * r * 0.38,
    ];
    painter.add(Shape::convex_polygon(pad.to_vec(), color, Stroke::NONE));
}

/// Edit (the panels' sliders): three lines with a knob each, at different places.
pub fn sliders(painter: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    let s = Stroke::new(1.5, color);
    for (k, knob) in [(-1.0f32, 0.35f32), (0.0, -0.45), (1.0, 0.15)] {
        let y = k * r * 0.7;
        painter.line_segment([center + vec2(-r, y), center + vec2(r, y)], s);
        let at = center + vec2(knob * r, y);
        painter.circle_filled(at, r * 0.3, color);
    }
}
