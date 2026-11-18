//! The Remove tool (Q; LrC's spot removal without the AI modes): click for a round spot,
//! drag to brush one; each takes its pixels from a source found nearby (drag the source
//! to choose, / for another). Heal matches the source to the spot's surroundings, Clone
//! copies it.

use eframe::egui::{self, Align, Color32, Layout, Pos2, Rect, RichText, Stroke};
use imaging::develop::{DevelopSettings, Slider, Spot, SpotMode};

use super::develop::{Spec, slider_row_as, weak};
use super::masking::{Canvas, header, pill};
use super::theme;
use crate::app::{Command, PhotoLibraryApp};
use crate::develop::{DevelopState, RemoveDrag, RemoveSettings, RemoveTool, spot_radius, spot_size};

/// How close (screen points) to a circle's edge resizes it.
const EDGE: f32 = 6.0;
const SELECTED: Color32 = Color32::from_rgb(70, 150, 255);

/// Distance from `p` to a path (a circle's centre when it has one point), in pixels.
fn dist_to_path(path: &[egui::Vec2], p: egui::Vec2) -> f32 {
    match path {
        [] => f32::MAX,
        [a] => (p - *a).length(),
        _ => path
            .windows(2)
            .map(|w| {
                let (a, b) = (w[0], w[1]);
                let ab = b - a;
                let t = ((p - a).dot(ab) / ab.length_sq().max(1e-9)).clamp(0.0, 1.0);
                (p - (a + ab * t)).length()
            })
            .fold(f32::MAX, f32::min),
    }
}

/// A spot's path in photo pixels, moved by `shift` (0..1 units; its source with the offset).
fn path_px(c: &Canvas, spot: &Spot, shift: [f32; 2]) -> Vec<egui::Vec2> {
    spot.points.iter().map(|p| c.px([p[0] + shift[0], p[1] + shift[1]])).collect()
}

/// Screen points per photo pixel around `at` (for edge distances on screen).
fn screen_scale(c: &Canvas, at: egui::Vec2) -> f32 {
    match (c.screen_px(at), c.screen_px(at + egui::vec2(10.0, 0.0))) {
        (Some(a), Some(b)) => a.distance(b) / 10.0,
        _ => 1.0,
    }
}

/// What is under `p`: a spot's source (selected spot only), its edge, or the spot itself.
fn hit(c: &Canvas, spots: &[Spot], selected: Option<usize>, p: Pos2) -> Option<RemoveDrag> {
    let q = c.px(c.photo(p));
    let grab = c.photo(p);
    let order: Vec<usize> =
        selected.into_iter().chain((0..spots.len()).rev().filter(|i| Some(*i) != selected)).collect();
    for i in order {
        let s = &spots[i];
        let r = s.size * c.half();
        let dest = path_px(c, s, [0.0, 0.0]);
        let d = dist_to_path(&dest, q);
        let scale = screen_scale(c, q);
        // The edge resizes a round spot; on a small one only its outer part does, so the
        // middle still moves it.
        let (ds, rs) = (d * scale, r * scale);
        if s.points.len() == 1 && (ds - rs).abs() < EDGE && ds > rs * 0.6 {
            return Some(RemoveDrag::Resize { index: i, original: s.clone() });
        }
        if d < r {
            return Some(RemoveDrag::Spot { index: i, grab, original: s.clone() });
        }
        if Some(i) == selected && dist_to_path(&path_px(c, s, s.offset), q) < r {
            return Some(RemoveDrag::Source { index: i, grab, original: s.clone() });
        }
    }
    None
}

/// A spot's outline on screen: a circle, or the brushed band (both edges of the path).
fn outline(c: &Canvas, spot: &Spot, shift: [f32; 2]) -> Vec<Vec<Pos2>> {
    let r = spot.size * c.half();
    let path = path_px(c, spot, shift);
    match path.as_slice() {
        [] => Vec::new(),
        [o] => {
            let ring = (0..=48)
                .filter_map(|k| {
                    let t = k as f32 / 48.0 * std::f32::consts::TAU;
                    c.screen_px(*o + egui::vec2(t.cos(), t.sin()) * r)
                })
                .collect();
            vec![ring]
        }
        _ => {
            // The band's two sides, and round ends.
            let side = |sign: f32| -> Vec<Pos2> {
                path.windows(2)
                    .flat_map(|w| {
                        let n = (w[1] - w[0]).normalized().rot90() * r * sign;
                        [w[0] + n, w[1] + n]
                    })
                    .filter_map(|p| c.screen_px(p))
                    .collect()
            };
            let cap = |o: egui::Vec2| -> Vec<Pos2> {
                (0..=24)
                    .filter_map(|k| {
                        let t = k as f32 / 24.0 * std::f32::consts::TAU;
                        c.screen_px(o + egui::vec2(t.cos(), t.sin()) * r)
                    })
                    .collect()
            };
            vec![side(1.0), side(-1.0), cap(path[0]), cap(*path.last().expect("two points"))]
        }
    }
}

fn draw_outline(painter: &egui::Painter, lines: &[Vec<Pos2>], stroke: Stroke) {
    let shadow = Stroke::new(stroke.width + 2.0, Color32::from_black_alpha(110));
    for l in lines {
        painter.add(egui::Shape::line(l.clone(), shadow));
        painter.add(egui::Shape::line(l.clone(), stroke));
    }
}

/// The centre of a spot (its pin), 0..1.
fn centre(spot: &Spot) -> [f32; 2] {
    let n = spot.points.len().max(1) as f32;
    let (x, y) = spot.points.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
    [x / n, y / n]
}

/// The Remove tool on the photo: click or brush new spots, select, move a spot or its
/// source, resize a round one. Returns true when a change is finished (to be saved).
pub(super) fn canvas(
    ui: &egui::Ui,
    resp: &egui::Response,
    d: &mut DevelopState,
    rotation: u8,
    img_rect: Rect,
    painter: &egui::Painter,
) -> bool {
    let Some(size) = d.info.as_ref().map(|i| i.full_size) else { return false };
    let Some(mut tool) = d.removing.clone() else { return false };
    let c = Canvas::new(img_rect, rotation, d.settings.transform.clone(), size);
    let mut st = d.settings.clone();
    let mut done = false;
    let mut find: Option<usize> = None;
    tool.selected = tool.selected.filter(|&i| i < st.spots.len());
    let settings = d.remove;
    let new_spot = |points: Vec<[f32; 2]>| {
        let radius = spot_radius(settings.size);
        // A first guess at the source (to the right); the worker picks a better one.
        let (w, h) = (size.0 as f32, size.1 as f32);
        let half = w.max(h) / 2.0;
        Spot {
            mode: settings.mode,
            points,
            size: radius,
            feather: settings.feather,
            opacity: settings.opacity,
            offset: [2.5 * radius * half / w, 0.0],
        }
    };

    // The wheel sizes new spots (or the selected one), as the brush.
    if resp.hover_pos().is_some_and(|p| img_rect.contains(p)) {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            match tool.selected {
                Some(i) => {
                    let s = &mut st.spots[i];
                    s.size = spot_radius((spot_size(s.size) * (dy / 300.0).exp2()).clamp(1.0, 100.0));
                }
                None => d.remove.size = (d.remove.size * (dy / 300.0).exp2()).clamp(1.0, 100.0),
            }
        }
    }

    if resp.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        tool.drag = Some(match hit(&c, &st.spots, tool.selected, origin) {
            Some(drag) => {
                if let RemoveDrag::Spot { index, .. }
                | RemoveDrag::Source { index, .. }
                | RemoveDrag::Resize { index, .. } = drag
                {
                    tool.selected = Some(index);
                }
                drag
            }
            None => RemoveDrag::Paint(vec![c.photo(origin)]),
        });
    }
    let pointer = ui.input(|i| i.pointer.latest_pos());
    if let Some(q) = pointer {
        let at = c.photo(q);
        match &mut tool.drag {
            Some(RemoveDrag::Paint(path)) => {
                let step = (spot_radius(settings.size) * c.half() / 4.0).max(1.0);
                if path.last().is_none_or(|last| (c.px(at) - c.px(*last)).length() >= step) {
                    path.push(at);
                }
            }
            Some(RemoveDrag::Spot { index, grab, original }) => {
                // The spot moves, its source stays where it is.
                let (dx, dy) = (at[0] - grab[0], at[1] - grab[1]);
                let s = &mut st.spots[*index];
                s.points = original.points.iter().map(|p| [p[0] + dx, p[1] + dy]).collect();
                s.offset = [original.offset[0] - dx, original.offset[1] - dy];
            }
            Some(RemoveDrag::Source { index, grab, original }) => {
                let s = &mut st.spots[*index];
                s.offset = [original.offset[0] + at[0] - grab[0], original.offset[1] + at[1] - grab[1]];
            }
            Some(RemoveDrag::Resize { index, original }) => {
                let path = path_px(&c, original, [0.0, 0.0]);
                st.spots[*index].size = (dist_to_path(&path, c.px(at)) / c.half()).clamp(0.0005, 0.5);
            }
            None => {}
        }
    }
    if resp.drag_stopped()
        && let Some(drag) = tool.drag.take()
    {
        match drag {
            RemoveDrag::Paint(path) => {
                st.spots.push(new_spot(path));
                tool.selected = Some(st.spots.len() - 1);
                find = tool.selected;
            }
            _ => done = true,
        }
    }
    // A click: select the spot under it, or make a round one.
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        match hit(&c, &st.spots, tool.selected, p) {
            Some(
                RemoveDrag::Spot { index, .. }
                | RemoveDrag::Source { index, .. }
                | RemoveDrag::Resize { index, .. },
            ) => {
                tool.selected = Some(index);
            }
            _ => {
                st.spots.push(new_spot(vec![c.photo(p)]));
                tool.selected = Some(st.spots.len() - 1);
                find = tool.selected;
            }
        }
    }

    // Spots: outlines; the selected one with its source and an arrow from it.
    let painter = painter.with_clip_rect(img_rect.intersect(painter.clip_rect()));
    for (i, s) in st.spots.iter().enumerate() {
        let selected = tool.selected == Some(i);
        let stroke = if selected {
            Stroke::new(1.6, Color32::WHITE)
        } else {
            Stroke::new(1.0, Color32::from_white_alpha(150))
        };
        draw_outline(&painter, &outline(&c, s, [0.0, 0.0]), stroke);
        if selected {
            draw_outline(
                &painter,
                &outline(&c, s, s.offset),
                Stroke::new(1.0, Color32::from_white_alpha(200)),
            );
            let (a, b) = (centre(s), centre(s));
            if let (Some(from), Some(to)) = (c.screen([a[0] + s.offset[0], a[1] + s.offset[1]]), c.screen(b))
            {
                let dir = (to - from).normalized();
                painter.line_segment([from, to - dir * 6.0], Stroke::new(1.2, Color32::WHITE));
                let tip = to - dir * 6.0;
                let side = dir.rot90() * 4.0;
                painter.add(egui::Shape::convex_polygon(
                    vec![tip + dir * 6.0, tip + side, tip - side],
                    Color32::WHITE,
                    Stroke::NONE,
                ));
                painter.circle_filled(to, 3.0, SELECTED);
            }
        }
    }
    // The stroke being painted, and the brush cursor elsewhere.
    if let Some(RemoveDrag::Paint(path)) = &tool.drag {
        let preview = Spot { points: path.clone(), ..new_spot(Vec::new()) };
        draw_outline(
            &painter,
            &outline(&c, &preview, [0.0, 0.0]),
            Stroke::new(1.5, Color32::from_rgb(255, 205, 60)),
        );
    }
    if let Some(p) = resp.hover_pos().filter(|p| img_rect.contains(*p)) {
        match (&tool.drag, hit(&c, &st.spots, tool.selected, p)) {
            (None, None) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                let at = c.px(c.photo(p));
                let r = spot_radius(d.remove.size) * c.half() * screen_scale(&c, at);
                painter.circle_stroke(p, r.max(2.0), Stroke::new(3.0, Color32::from_black_alpha(110)));
                painter.circle_stroke(p, r.max(2.0), Stroke::new(1.3, Color32::WHITE));
            }
            (None, Some(RemoveDrag::Resize { .. })) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal)
            }
            (None, Some(_)) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grab),
            (Some(RemoveDrag::Paint(_)), _) => ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair),
            (Some(_), _) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing),
        }
    }

    d.removing = Some(tool);
    if st != d.settings {
        d.set_live(st);
    }
    if let Some(i) = find {
        // The new spot's source; saved when the worker has picked it.
        d.find_source(i, false);
    }
    done
}

/// A Heal / Clone button: filled when chosen.
fn mode_button(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let r = pill(ui, text, false);
    if on {
        ui.painter().rect_filled(r.rect.shrink(1.0), 10.0, Color32::from_white_alpha(40));
    }
    r
}

/// The right panel with the Remove tool open: Heal or Clone, Size, Feather, Opacity (of the
/// selected spot, or for new ones), New Source, Delete, Reset.
pub(super) fn right_panel(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    let Some(mut tool) = d.removing.clone() else { return };
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;
    let mut leave = false;
    let mut new_source = false;
    tool.selected = tool.selected.filter(|&i| i < st.spots.len());
    let title = match tool.selected {
        Some(i) => format!("Remove: Spot {}", i + 1),
        None => "Remove".into(),
    };
    header(ui, &title, None);
    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        // Editing the selected spot, or the settings new spots get.
        let mut values = match tool.selected {
            Some(i) => {
                let s = &st.spots[i];
                RemoveSettings { mode: s.mode, size: spot_size(s.size), feather: s.feather, opacity: s.opacity }
            }
            None => d.remove,
        };
        let before = values;
        ui.horizontal(|ui| {
            ui.add_space(super::develop::LABEL_W - 40.0);
            for mode in [SpotMode::Heal, SpotMode::Clone] {
                let tip = match mode {
                    SpotMode::Heal => "Copy texture from the source, matched to the colour and brightness around the spot",
                    SpotMode::Clone => "Copy the source as it is",
                };
                if mode_button(ui, mode.label(), values.mode == mode).on_hover_text(tip).clicked() {
                    values.mode = mode;
                    done = Some(None);
                }
            }
        });
        ui.add_space(4.0);
        let defaults = RemoveSettings::default();
        let rows: [(&str, &mut f32, f32, f32, u8); 3] = [
            ("Size", &mut values.size, defaults.size, 1.0, 240),
            ("Feather", &mut values.feather, defaults.feather, 0.0, 241),
            ("Opacity", &mut values.opacity, defaults.opacity, 0.0, 242),
        ];
        for (label, value, default, min, id) in rows {
            // Stand-in develop sliders for the row's id and whole-number format.
            let spec = Spec {
                slider: Slider::MaskAmount(id),
                range: (min, 100.0),
                default,
                log: false,
                gradient: None,
                kelvin: false,
            };
            if slider_row_as(ui, &spec, label, value).done {
                done = Some(None);
            }
        }
        if values != before {
            match tool.selected {
                Some(i) => {
                    let s = &mut st.spots[i];
                    (s.mode, s.size, s.feather, s.opacity) =
                        (values.mode, spot_radius(values.size), values.feather, values.opacity);
                }
                None => d.remove = values,
            }
        }
        // Settings for new spots aren't History steps.
        if tool.selected.is_none() {
            done = None;
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if let Some(i) = tool.selected {
                if pill(ui, "New Source", false).on_hover_text("Pick another source for this spot (/)").clicked() {
                    new_source = true;
                }
                if pill(ui, "Delete", false).on_hover_text("Delete this spot (Delete)").clicked() {
                    st.spots.remove(i);
                    tool.selected = None;
                    done = Some(None);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pill(ui, "Close", false).on_hover_text("Done (Q)").clicked() {
                    leave = true;
                }
                if !st.spots.is_empty() && pill(ui, "Reset", false).on_hover_text("Remove every spot").clicked() {
                    st.spots.clear();
                    tool.selected = None;
                    done = Some(Some("Reset Spots".into()));
                }
            });
        });
        ui.add_space(6.0);
        let n = st.spots.len();
        ui.label(RichText::new(format!("{n} spot{}", if n == 1 { "" } else { "s" })).size(12.0).color(theme::TEXT_WEAK));
        ui.label(weak(
            "Click a blemish to remove it, or paint over it. Drag a spot or its source to move it, a round spot's edge to resize it. The wheel or [ ] sets the size.",
        ));
    });
    let index = tool.selected;
    d.removing = (!leave).then_some(tool);
    if st != d.settings {
        d.set_live(st);
    }
    if leave {
        d.request_renders();
    }
    if new_source && let Some(i) = index {
        d.find_source(i, true);
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
}

/// Open or close the Remove tool (Q); the crop tool and masking close.
pub fn set_removing(d: &mut DevelopState, on: bool) {
    d.removing = on.then(RemoveTool::default);
    if on {
        d.masking = None;
        d.cropping = None;
        d.picking_wb = false;
        d.picking_detail = false;
    }
    d.request_renders();
}

/// Delete with the Remove tool open: the selected spot.
pub fn delete_spot(d: &mut DevelopState) -> bool {
    let Some(mut tool) = d.removing.clone() else { return false };
    let Some(i) = tool.selected.filter(|&i| i < d.settings.spots.len()) else { return false };
    let mut st: DevelopSettings = d.settings.clone();
    st.spots.remove(i);
    tool.selected = None;
    d.removing = Some(tool);
    d.set_live(st);
    true
}

/// / with the Remove tool open: another source for the selected spot.
pub fn new_source(d: &mut DevelopState) {
    if let Some(i) = d.removing.as_ref().and_then(|t| t.selected).filter(|&i| i < d.settings.spots.len()) {
        d.find_source(i, true);
    }
}

/// [ ] with the Remove tool open: the selected spot's size, or new spots'.
pub fn step_size(d: &mut DevelopState, dir: i8) {
    let f = if dir < 0 { 1.0 / 1.18 } else { 1.18 };
    match d.removing.as_ref().and_then(|t| t.selected).filter(|&i| i < d.settings.spots.len()) {
        Some(i) => {
            let mut st = d.settings.clone();
            let s = &mut st.spots[i];
            s.size = spot_radius((spot_size(s.size) * f).clamp(1.0, 100.0));
            d.set_live(st);
        }
        None => d.remove.size = (d.remove.size * f).clamp(1.0, 100.0).round(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imaging::develop::Transform;

    fn canvas() -> Canvas {
        Canvas::new(
            Rect::from_min_size(Pos2::ZERO, egui::vec2(300.0, 200.0)),
            0,
            Transform::default(),
            (3000, 2000),
        )
    }

    #[test]
    fn spots_and_sources_are_hit_where_drawn() {
        let c = canvas();
        // A round spot at (0.5, 0.5), radius 0.02 half long sides = 30 px = 3 points.
        let spot = Spot { points: vec![[0.5, 0.5]], size: 0.02, offset: [0.1, 0.0], ..Spot::default() };
        let spots = [spot];
        assert!(matches!(
            hit(&c, &spots, None, egui::pos2(150.0, 100.0)),
            Some(RemoveDrag::Spot { index: 0, .. })
        ));
        assert!(matches!(hit(&c, &spots, None, egui::pos2(153.0, 100.0)), Some(RemoveDrag::Resize { .. })));
        // The source (30 points to the right) only for the selected spot.
        assert!(hit(&c, &spots, None, egui::pos2(180.0, 100.0)).is_none());
        assert!(matches!(
            hit(&c, &spots, Some(0), egui::pos2(180.0, 100.0)),
            Some(RemoveDrag::Source { .. })
        ));
        // A brushed spot is hit along its path.
        let brushed = [Spot { points: vec![[0.2, 0.2], [0.4, 0.2]], size: 0.02, ..Spot::default() }];
        assert!(hit(&c, &brushed, None, egui::pos2(90.0, 40.0)).is_some());
        assert!(hit(&c, &brushed, None, egui::pos2(90.0, 60.0)).is_none());
    }

    #[test]
    fn size_slider_maps_to_radius_and_back() {
        for size in [1.0, 12.0, 50.0, 100.0] {
            assert!((spot_size(spot_radius(size)) - size).abs() < 1e-3);
        }
    }
}
