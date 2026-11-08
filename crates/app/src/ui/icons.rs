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
