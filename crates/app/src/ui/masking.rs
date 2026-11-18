//! Masking mode (LrC's Masks): the floating Masks panel, the selected mask's sliders in the
//! right panel, and drawing and editing shapes on the photo.
//!
//! Shapes live in the untransformed photo's coordinates (0..1); the photo on screen may be
//! transformed, cropped and rotated, so everything goes through [`Canvas`].

use eframe::egui::{self, Align, Color32, Layout, Pos2, Rect, RichText, Stroke, Vec2};
use imaging::develop::{
    CombineMode, Component, DevelopSettings, LocalParam, Mask, Shape, Slider, Stroke as BrushStroke,
    Transform,
};

use super::develop::{Spec, rotate, slider_row, slider_row_as, unrotate, weak};
use super::theme;
use crate::app::{Command, PhotoLibraryApp};
use crate::develop::{
    AiKind, BrushSettings, DevelopState, Handle, MaskDrag, MaskTool, Masking, ShapeKind, ToolTarget,
};

/// Pins and handles: how close (screen points) counts as on them.
const GRAB: f32 = 8.0;
const PIN: Color32 = Color32::from_rgb(235, 235, 235);
const SELECTED: Color32 = Color32::from_rgb(70, 150, 255);

/// Screen ↔ photo for the shapes: the photo as shown is the untransformed photo through
/// the Transform (and Constrain Crop), then the catalog's rotation.
pub(super) struct Canvas {
    img_rect: Rect,
    rotation: u8,
    transform: Transform,
    /// The untransformed photo's size in pixels.
    size: (u32, u32),
}

impl Canvas {
    pub(super) fn new(img_rect: Rect, rotation: u8, transform: Transform, size: (u32, u32)) -> Canvas {
        Canvas { img_rect, rotation, transform, size }
    }

    /// A point of the untransformed photo (0..1) on screen.
    pub(super) fn screen(&self, p: [f32; 2]) -> Option<Pos2> {
        let q = self.transform.output_point(p[0], p[1], self.size.0, self.size.1)?;
        let (u, v) = rotate(q, self.rotation);
        Some(self.img_rect.min + egui::vec2(u, v) * self.img_rect.size())
    }

    /// The untransformed photo's point (0..1) under a screen position.
    pub(super) fn photo(&self, q: Pos2) -> [f32; 2] {
        let (x, y) = unrotate((q - self.img_rect.min) / self.img_rect.size(), self.rotation);
        let (u, v) = self.transform.source_point(x, y, self.size.0, self.size.1);
        [u, v]
    }

    /// 0..1 → pixels of the untransformed photo (for distances and angles), and back.
    pub(super) fn px(&self, p: [f32; 2]) -> Vec2 {
        egui::vec2(p[0] * self.size.0 as f32, p[1] * self.size.1 as f32)
    }

    pub(super) fn unit(&self, v: Vec2) -> [f32; 2] {
        [v.x / self.size.0.max(1) as f32, v.y / self.size.1.max(1) as f32]
    }

    /// Half the long side in pixels (the unit of radii and brush sizes).
    pub(super) fn half(&self) -> f32 {
        self.size.0.max(self.size.1) as f32 / 2.0
    }

    pub(super) fn screen_px(&self, v: Vec2) -> Option<Pos2> {
        self.screen(self.unit(v))
    }
}

/// `v` turned by `a` radians (clockwise on screen, y down).
fn turn_by(v: Vec2, a: f32) -> Vec2 {
    let (sin, cos) = a.sin_cos();
    egui::vec2(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
}

/// Where a component's pin sits (0..1).
fn pin(shape: &Shape) -> Option<[f32; 2]> {
    match shape {
        Shape::Linear { start, end } => Some([(start[0] + end[0]) / 2.0, (start[1] + end[1]) / 2.0]),
        Shape::Radial { center, .. } => Some(*center),
        Shape::Brush { strokes } => strokes.first().and_then(|s| s.points.first()).copied(),
        Shape::Raster { .. } => None,
    }
}

/// A radial gradient's axes in photo pixels: centre, and its x and y half-axes.
fn radial_axes(c: &Canvas, center: [f32; 2], radii: [f32; 2], angle: f32) -> (Vec2, Vec2, Vec2) {
    let a = angle.to_radians();
    let ax = egui::vec2(a.cos(), a.sin()) * radii[0] * c.half();
    let ay = egui::vec2(-a.sin(), a.cos()) * radii[1] * c.half();
    (c.px(center), ax, ay)
}

/// Where a drag on the selected component starts: which handle is under `p`.
fn hit(c: &Canvas, shape: &Shape, p: Pos2) -> Option<Handle> {
    match shape {
        Shape::Linear { start, end } => {
            let (s, e) = (c.screen(*start)?, c.screen(*end)?);
            let m = s + (e - s) / 2.0;
            if p.distance(m) < GRAB {
                return Some(Handle::Move);
            }
            // The three lines run across the gradient's direction; how far along it is `p`?
            let d = (e - s).normalized();
            let along = |o: Pos2| (p - o).dot(d).abs();
            if along(s) < GRAB * 0.75 {
                Some(Handle::Start)
            } else if along(e) < GRAB * 0.75 {
                Some(Handle::End)
            } else if along(m) < GRAB * 0.75 {
                Some(Handle::Rotate)
            } else {
                None
            }
        }
        Shape::Radial { center, radii, angle, .. } => {
            let (o, ax, ay) = radial_axes(c, *center, *radii, *angle);
            if c.screen(*center).is_some_and(|q| q.distance(p) < GRAB) {
                return Some(Handle::Move);
            }
            for (v, h) in
                [(ax, Handle::RadiusX), (-ax, Handle::RadiusX), (ay, Handle::RadiusY), (-ay, Handle::RadiusY)]
            {
                if c.screen_px(o + v).is_some_and(|q| q.distance(p) < GRAB) {
                    return Some(h);
                }
            }
            // Inside moves; a band just outside the edge turns it.
            let q = c.px(c.photo(p)) - o;
            let (lx, ly) = (q.dot(ax) / ax.length_sq().max(1e-6), q.dot(ay) / ay.length_sq().max(1e-6));
            let d = (lx * lx + ly * ly).sqrt();
            if d < 1.0 {
                Some(Handle::Move)
            } else if d < 1.25 {
                Some(Handle::Rotate)
            } else {
                None
            }
        }
        Shape::Brush { .. } | Shape::Raster { .. } => None,
    }
}

/// The shape after dragging `handle` from `grab` to `to` (0..1), starting from `original`.
fn drag_shape(c: &Canvas, original: &Shape, handle: Handle, grab: [f32; 2], to: [f32; 2]) -> Shape {
    let (g, q) = (c.px(grab), c.px(to));
    let turn = |origin: Vec2| (q - origin).angle() - (g - origin).angle();
    let rotated = |v: Vec2, origin: Vec2, a: f32| origin + turn_by(v - origin, a);
    match original {
        Shape::Linear { start, end } => {
            let (s, e) = (c.px(*start), c.px(*end));
            let (s, e) = match handle {
                Handle::Move => (s + (q - g), e + (q - g)),
                Handle::Start => (q, e),
                Handle::End | Handle::RadiusX | Handle::RadiusY => (s, q),
                Handle::Rotate => {
                    let m = (s + e) / 2.0;
                    let a = turn(m);
                    (rotated(s, m, a), rotated(e, m, a))
                }
            };
            Shape::Linear { start: c.unit(s), end: c.unit(e) }
        }
        Shape::Radial { center, radii, angle, feather } => {
            let o = c.px(*center);
            let a = angle.to_radians();
            // `to` in the ellipse's own axes.
            let local = turn_by(q - o, -a);
            let r = |v: f32| (v.abs() / c.half()).max(0.005);
            let (center, radii, angle) = match handle {
                Handle::Move => (c.unit(o + (q - g)), *radii, *angle),
                Handle::RadiusX => (*center, [r(local.x), radii[1]], *angle),
                Handle::RadiusY => (*center, [radii[0], r(local.y)], *angle),
                // Drawing a new one: the drag sets both radii.
                Handle::Start | Handle::End => (*center, [r(local.x), r(local.y)], *angle),
                Handle::Rotate => (*center, *radii, angle + turn(o).to_degrees()),
            };
            Shape::Radial { center, radii, angle, feather: *feather }
        }
        Shape::Brush { .. } | Shape::Raster { .. } => original.clone(),
    }
}

/// A new shape of `kind` started at `p` (0..1), and the handle the drag moves.
fn new_shape(kind: ShapeKind, p: [f32; 2]) -> (Shape, Handle) {
    match kind {
        ShapeKind::Linear => (Shape::Linear { start: p, end: [p[0], p[1] + 0.001] }, Handle::End),
        ShapeKind::Radial => {
            (Shape::Radial { center: p, radii: [0.005, 0.005], angle: 0.0, feather: 50.0 }, Handle::End)
        }
        ShapeKind::Brush | ShapeKind::Ai => (Shape::Brush { strokes: Vec::new() }, Handle::Move),
    }
}

/// The next free "Mask N" name.
fn next_name(masks: &[Mask]) -> String {
    crate::develop::next_mask_name(masks)
}

/// Put a new shape where `tool` says (a new mask, or a component of the selected one),
/// select it and put the tool away.
fn add_shape(st: &mut DevelopSettings, m: &mut Masking, tool: MaskTool, shape: Shape) {
    match (tool.target, m.selected.filter(|&i| i < st.masks.len())) {
        (ToolTarget::Component(mode), Some(i)) => {
            st.masks[i].components.push(Component { mode, invert: false, shape });
            m.component = st.masks[i].components.len() - 1;
        }
        _ => {
            st.masks.push(Mask::new(next_name(&st.masks), shape));
            m.selected = Some(st.masks.len() - 1);
            m.component = 0;
        }
    }
    m.tool = None;
}

/// The selected component's shape, to change.
fn selected_shape<'a>(st: &'a mut DevelopSettings, m: &Masking) -> Option<&'a mut Shape> {
    let comp = st.masks.get_mut(m.selected?)?.components.get_mut(m.component)?;
    Some(&mut comp.shape)
}

/// Start a stroke at `p` in the selected brush component.
fn start_stroke(st: &mut DevelopSettings, m: &Masking, brush: &BrushSettings, erase: bool, p: [f32; 2]) {
    if let Some(Shape::Brush { strokes }) = selected_shape(st, m) {
        strokes.push(BrushStroke {
            points: vec![round_point(p)],
            size: brush.radius(),
            feather: brush.feather,
            flow: brush.flow,
            density: brush.density,
            erase,
        });
    }
}

/// Points are stored to 1/10000 of the photo (the settings stay compact).
fn round_point(p: [f32; 2]) -> [f32; 2] {
    p.map(|v| (v * 10_000.0).round() / 10_000.0)
}

/// The brush cursor: the full size, the inside the feather leaves solid, and a minus
/// sign while erasing.
fn draw_brush_cursor(painter: &egui::Painter, c: &Canvas, at: Pos2, brush: &BrushSettings, erase: bool) {
    let p = c.photo(at);
    let r_px = brush.radius() * c.half();
    // The radius on screen: measured along the photo's x axis.
    let Some(edge) = c.screen_px(c.px(p) + egui::vec2(r_px, 0.0)) else { return };
    let r = at.distance(edge).max(1.5);
    let inner = r * (1.0 - brush.feather / 100.0);
    for (radius, width) in [(r, 1.5), (inner, 1.0)] {
        if radius > 1.0 {
            painter.circle_stroke(at, radius, Stroke::new(width + 2.0, Color32::from_black_alpha(110)));
            painter.circle_stroke(at, radius, Stroke::new(width, Color32::WHITE));
        }
    }
    let sign = if erase { "−" } else { "+" };
    painter.text(at, egui::Align2::CENTER_CENTER, sign, egui::FontId::proportional(13.0), Color32::WHITE);
}

/// The selected mask and component, kept valid (Undo may have removed them).
fn selection(m: &mut Masking, masks: &[Mask]) -> Option<(usize, usize)> {
    let i = m.selected.filter(|&i| i < masks.len())?;
    m.selected = Some(i);
    m.component = m.component.min(masks[i].components.len().saturating_sub(1));
    Some((i, m.component))
}

fn draw_dashed(painter: &egui::Painter, points: &[Pos2], stroke: Stroke) {
    for (k, w) in points.windows(2).enumerate() {
        if k % 2 == 0 {
            painter.line_segment([w[0], w[1]], stroke);
        }
    }
}

/// The selected component's outline and handles.
fn draw_shape(painter: &egui::Painter, c: &Canvas, shape: &Shape) {
    let strong = Stroke::new(1.5, Color32::WHITE);
    let thin = Stroke::new(1.0, Color32::from_white_alpha(200));
    let shadow = Stroke::new(3.0, Color32::from_black_alpha(90));
    match shape {
        Shape::Linear { start, end } => {
            let (s, e) = (c.px(*start), c.px(*end));
            let dir = (e - s).normalized();
            let across = egui::vec2(-dir.y, dir.x);
            let reach = (c.size.0 as f32).hypot(c.size.1 as f32) * 1.5;
            for (o, stroke) in [(s, thin), ((s + e) / 2.0, strong), (e, thin)] {
                // A long line across the gradient, drawn in pieces (it may bend on screen
                // only through rounding, but pieces also stay visible when an end is far).
                let pts: Vec<Pos2> = (0..=16)
                    .filter_map(|k| c.screen_px(o + across * reach * (k as f32 / 8.0 - 1.0)))
                    .collect();
                for w in pts.windows(2) {
                    painter.line_segment([w[0], w[1]], shadow);
                    painter.line_segment([w[0], w[1]], stroke);
                }
            }
        }
        Shape::Radial { center, radii, angle, feather } => {
            let (o, ax, ay) = radial_axes(c, *center, *radii, *angle);
            let ring = |k: f32| -> Vec<Pos2> {
                (0..=72)
                    .filter_map(|i| {
                        let t = i as f32 / 72.0 * std::f32::consts::TAU;
                        c.screen_px(o + (ax * t.cos() + ay * t.sin()) * k)
                    })
                    .collect()
            };
            let outer = ring(1.0);
            painter.add(egui::Shape::line(outer.clone(), shadow));
            painter.add(egui::Shape::line(outer, strong));
            let inner = 1.0 - feather.clamp(0.0, 100.0) / 100.0;
            if inner > 0.02 && inner < 0.98 {
                draw_dashed(painter, &ring(inner), thin);
            }
            for v in [ax, -ax, ay, -ay] {
                if let Some(q) = c.screen_px(o + v) {
                    let r = Rect::from_center_size(q, egui::vec2(7.0, 7.0));
                    painter.rect_filled(r, 1.0, Color32::WHITE);
                    painter.rect_stroke(r, 1.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Outside);
                }
            }
        }
        Shape::Brush { .. } | Shape::Raster { .. } => {}
    }
}

fn draw_pin(painter: &egui::Painter, at: Pos2, selected: bool, current_mask: bool) {
    // LrC style: a white ring; the selected component filled, other masks' pins smaller.
    let r = if current_mask { 6.0 } else { 5.0 };
    painter.circle_filled(at, r + 1.5, Color32::from_black_alpha(150));
    painter.circle_stroke(at, r, Stroke::new(1.8, if current_mask { PIN } else { Color32::from_gray(170) }));
    if selected {
        painter.circle_filled(at, r - 1.6, SELECTED);
    }
}

/// Masking on the photo: draw new shapes with the active tool, move and reshape the
/// selected one by its handles, select by pins; the overlay and outlines. Returns true
/// when a change is finished (to be saved as a History step).
pub(super) fn canvas(
    ui: &egui::Ui,
    resp: &egui::Response,
    d: &mut DevelopState,
    rotation: u8,
    img_rect: Rect,
    painter: &egui::Painter,
) -> bool {
    let Some(size) = d.info.as_ref().map(|i| i.full_size) else { return false };
    let Some(mut m) = d.masking.clone() else { return false };
    let c = Canvas::new(img_rect, rotation, d.settings.transform.clone(), size);
    let mut st = d.settings.clone();
    let sel = selection(&mut m, &st.masks);
    let mut done = false;
    // The brush paints when its tool is waiting, or a brush component is selected.
    let brushing = match m.tool {
        Some(t) => t.kind == ShapeKind::Brush,
        None => sel.is_some_and(|(i, j)| matches!(st.masks[i].components[j].shape, Shape::Brush { .. })),
    };
    // The wheel sizes the brush over the photo (up: larger), as in LrC.
    if brushing && resp.hover_pos().is_some_and(|p| img_rect.contains(p)) {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            d.brush.scroll_size(dy);
        }
    }
    let brush = d.brush;
    let erase = brush.erase || ui.input(|i| i.modifiers.alt);

    // The overlay first, under the outlines.
    if let Some(t) = &d.overlay {
        super::views::paint_image(painter, t, img_rect, rotation, Color32::WHITE);
    }

    // A drag starts: a new shape with the tool, else a handle of the selected component.
    if resp.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        let p = c.photo(origin);
        if let Some(tool) = m.tool {
            let (shape, handle) = new_shape(tool.kind, p);
            add_shape(&mut st, &mut m, tool, shape.clone());
            if tool.kind == ShapeKind::Brush {
                start_stroke(&mut st, &m, &brush, erase, p);
                m.drag = Some(MaskDrag::Paint);
            } else {
                m.drag = Some(MaskDrag::Edit { handle, grab: p, original: shape });
            }
        } else if brushing {
            start_stroke(&mut st, &m, &brush, erase, p);
            m.drag = Some(MaskDrag::Paint);
        } else if let Some((i, j)) = sel {
            let shape = &st.masks[i].components[j].shape;
            if let Some(handle) = hit(&c, shape, origin) {
                m.drag = Some(MaskDrag::Edit { handle, grab: p, original: shape.clone() });
            }
        }
    }
    let pointer = ui.input(|i| i.pointer.latest_pos());
    if let (Some(MaskDrag::Edit { handle, grab, original }), Some(q), Some(i)) =
        (&m.drag, pointer, m.selected)
        && let Some(comp) = st.masks.get_mut(i).and_then(|mask| mask.components.get_mut(m.component))
    {
        comp.shape = drag_shape(&c, original, *handle, *grab, c.photo(q));
    }
    // Painting: extend the stroke as the pointer moves (a point every quarter radius).
    if let (Some(MaskDrag::Paint), Some(q)) = (&m.drag, pointer)
        && let Some(Shape::Brush { strokes }) = selected_shape(&mut st, &m)
        && let Some(stroke) = strokes.last_mut()
    {
        let p = c.photo(q);
        let last = stroke.points.last().copied().unwrap_or(p);
        let step = (stroke.size * c.half() / 4.0).max(1.0);
        if (c.px(p) - c.px(last)).length() >= step {
            stroke.points.push(round_point(p));
        }
    }
    if resp.drag_stopped() && m.drag.take().is_some() {
        // A click with the radial tool makes one of a default size; a linear gradient
        // needs a drag.
        if let Some(i) = m.selected
            && let Some(comp) = st.masks.get_mut(i).and_then(|mask| mask.components.get_mut(m.component))
        {
            match &mut comp.shape {
                Shape::Radial { radii, .. } if radii[0] < 0.01 && radii[1] < 0.01 => *radii = [0.2, 0.2],
                Shape::Linear { start, end }
                    if c.screen(*start).zip(c.screen(*end)).is_some_and(|(a, b)| a.distance(b) < 4.0) =>
                {
                    st.masks[i].components.remove(m.component);
                    if st.masks[i].components.is_empty() {
                        st.masks.remove(i);
                        m.selected = None;
                    }
                }
                _ => {}
            }
        }
        done = true;
    }

    // A click on a pin selects its mask (and component).
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let found = st.masks.iter().enumerate().find_map(|(i, mask)| {
            mask.components.iter().enumerate().find_map(|(j, comp)| {
                let at = c.screen(pin(&comp.shape)?)?;
                (at.distance(p) < GRAB).then_some((i, j))
            })
        });
        if let Some((i, j)) = found {
            m.selected = Some(i);
            m.component = j;
        } else if let Some(tool) = m.tool {
            // A click with a tool: a radial gradient of a default size, or one brush dab.
            let at = c.photo(p);
            match tool.kind {
                ShapeKind::Radial => {
                    let shape = Shape::Radial { center: at, radii: [0.2, 0.2], angle: 0.0, feather: 50.0 };
                    add_shape(&mut st, &mut m, tool, shape);
                    done = true;
                }
                ShapeKind::Brush => {
                    add_shape(&mut st, &mut m, tool, Shape::Brush { strokes: Vec::new() });
                    start_stroke(&mut st, &m, &brush, erase, at);
                    done = true;
                }
                ShapeKind::Linear | ShapeKind::Ai => {}
            }
        } else if brushing {
            start_stroke(&mut st, &m, &brush, erase, c.photo(p));
            done = true;
        }
    }

    // Outlines and pins, on the photo only.
    let painter = &painter.with_clip_rect(img_rect.intersect(painter.clip_rect()));
    if let Some((i, j)) = selection(&mut m, &st.masks) {
        draw_shape(painter, &c, &st.masks[i].components[j].shape);
    }
    for (i, mask) in st.masks.iter().enumerate() {
        let current = m.selected == Some(i);
        for (j, comp) in mask.components.iter().enumerate() {
            // Other masks show one pin; the selected mask one per component.
            if (j > 0 && !current) || !mask.visible && !current {
                continue;
            }
            if let Some(at) = pin(&comp.shape).and_then(|p| c.screen(p)) {
                draw_pin(painter, at, current && j == m.component, current);
            }
        }
    }

    // The cursor: drawing, or over a handle.
    let brushing_now = match m.tool {
        Some(t) => t.kind == ShapeKind::Brush,
        None => selection(&mut m, &st.masks)
            .is_some_and(|(i, j)| matches!(st.masks[i].components[j].shape, Shape::Brush { .. })),
    };
    if brushing_now && let Some(p) = resp.hover_pos().filter(|p| img_rect.contains(*p)) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        draw_brush_cursor(painter, &c, p, &brush, erase);
    } else if resp.hover_pos().is_some_and(|p| img_rect.contains(p)) {
        let icon = if m.tool.is_some() {
            egui::CursorIcon::Crosshair
        } else if m.drag.is_some() {
            egui::CursorIcon::Grabbing
        } else if let (Some((i, j)), Some(p)) = (m.selected.zip(Some(m.component)), resp.hover_pos())
            && st
                .masks
                .get(i)
                .and_then(|mask| mask.components.get(j))
                .and_then(|comp| hit(&c, &comp.shape, p))
                .is_some()
        {
            egui::CursorIcon::Grab
        } else {
            egui::CursorIcon::Default
        };
        ui.ctx().set_cursor_icon(icon);
    }

    d.masking = Some(m);
    if st != d.settings {
        d.set_live(st);
    }
    d.request_renders();
    done
}

/// Which tool a component's shape belongs to.
fn kind_of(shape: &Shape) -> ShapeKind {
    match shape {
        Shape::Brush { .. } => ShapeKind::Brush,
        Shape::Linear { .. } => ShapeKind::Linear,
        Shape::Radial { .. } => ShapeKind::Radial,
        Shape::Raster { .. } => ShapeKind::Ai,
    }
}

/// A painted glyph for a tool: a brush, three lines (linear), rings (radial).
fn tool_icon(painter: &egui::Painter, center: Pos2, r: f32, kind: ShapeKind, color: Color32) {
    match kind {
        ShapeKind::Brush => {
            let tip = center + egui::vec2(-r * 0.4, r * 0.4);
            painter.line_segment([center + egui::vec2(r * 0.8, -r * 0.8), tip], Stroke::new(1.6, color));
            painter.circle_filled(tip, r * 0.42, color);
        }
        ShapeKind::Linear => {
            // A square fading from solid (top) to clear (bottom), as LrC draws it.
            let rect = Rect::from_center_size(center, egui::vec2(2.0 * r, 2.0 * r));
            let clear = color.gamma_multiply(0.0);
            let mut mesh = egui::Mesh::default();
            for (pos, c) in [
                (rect.left_top(), color),
                (rect.right_top(), color),
                (rect.right_bottom(), clear),
                (rect.left_bottom(), clear),
            ] {
                mesh.colored_vertex(pos, c);
            }
            mesh.add_triangle(0, 1, 2);
            mesh.add_triangle(0, 2, 3);
            painter.add(egui::Shape::mesh(mesh));
            painter.rect_stroke(rect, 1.5, Stroke::new(1.0, color), egui::StrokeKind::Inside);
        }
        ShapeKind::Radial => {
            painter.circle_stroke(center, r, Stroke::new(1.3, color));
            painter.circle_filled(center, r * 0.42, color);
        }
        ShapeKind::Ai => sparkle(painter, center, r, color),
    }
}

/// A four-pointed star: AI masks' glyph.
fn sparkle(painter: &egui::Painter, center: Pos2, r: f32, color: Color32) {
    let w = r * 0.28;
    let pts: Vec<Pos2> = [(0.0, -r), (w, -w), (r, 0.0), (w, w), (0.0, r), (-w, w), (-r, 0.0), (-w, -w)]
        .iter()
        .map(|&(x, y)| center + egui::vec2(x, y))
        .collect();
    painter.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// Row colours of the Masks panel.
const ROW_HOVER: Color32 = Color32::from_rgb(0x3c, 0x3c, 0x3c);
const ROW_SELECTED: Color32 = Color32::from_rgb(0x4e, 0x4e, 0x4e);
const PANEL_BG: Color32 = Color32::from_rgb(0x2b, 0x2b, 0x2b);

/// A choice in a Create / Add / Subtract / Intersect menu.
#[derive(Debug, Clone, Copy, PartialEq)]
enum MenuPick {
    Tool(ShapeKind),
    Ai(AiKind),
}

/// The AI selections first (as LrC lists them), then the tools with their keys, as menu
/// rows; `pick` gets the chosen one.
fn tool_menu(ui: &mut egui::Ui, mut pick: impl FnMut(MenuPick)) {
    let ai = [AiKind::Subject, AiKind::Sky, AiKind::Background].map(|k| (MenuPick::Ai(k), k.label(), ""));
    let tools = [(ShapeKind::Brush, "K"), (ShapeKind::Linear, "M"), (ShapeKind::Radial, "Shift+M")]
        .map(|(k, key)| (MenuPick::Tool(k), k.label(), key));
    for (n, (choice, label, key)) in ai.into_iter().chain(tools).enumerate() {
        if n == 3 {
            ui.separator();
        }
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(210.0, 26.0), egui::Sense::click());
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, 3.0, ROW_SELECTED);
        }
        let kind = match choice {
            MenuPick::Tool(k) => k,
            MenuPick::Ai(_) => ShapeKind::Ai,
        };
        tool_icon(painter, rect.left_center() + egui::vec2(15.0, 0.0), 6.0, kind, theme::TEXT_STRONG);
        let font = egui::FontId::proportional(13.0);
        painter.text(
            rect.left_center() + egui::vec2(32.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            font,
            theme::TEXT,
        );
        let small = egui::FontId::proportional(11.5);
        painter.text(
            rect.right_center() - egui::vec2(8.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            key,
            small,
            theme::TEXT_WEAK,
        );
        if resp.clicked() {
            pick(choice);
            ui.close();
        }
    }
}

/// While an AI mask is being made: what is happening, with a bar or a spinner.
fn ai_progress(ui: &mut egui::Ui, job: &crate::develop::AiJob) {
    let (share, what) = job.progress();
    egui::Frame::new()
        .fill(Color32::from_rgb(0x24, 0x34, 0x4a))
        .corner_radius(3)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if share < 0.0 {
                    ui.add(egui::Spinner::new().size(12.0));
                }
                ui.label(RichText::new(what).size(11.5).color(theme::TEXT_STRONG));
            });
            if share >= 0.0 {
                ui.add(egui::ProgressBar::new(share).desired_height(6.0));
            }
        });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
}

/// Before the first AI mask: what has to be downloaded, and agreeing to it.
pub(super) fn ai_consent(app: &mut PhotoLibraryApp, ctx: &egui::Context) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    let Some((kind, target, bytes)) = d.ai_consent else { return };
    let mut answer = None;
    let modal = egui::Modal::new(egui::Id::unique("ai_consent")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.heading(format!("Select {}", kind.label()));
        ui.add_space(6.0);
        ui.label(format!(
            "AI masks run on this computer. The first time, they need {:.0} MB downloaded: the \
             model that finds the {}{}. Nothing about your photos is sent anywhere.",
            bytes as f64 / 1e6,
            kind.label().to_lowercase(),
            if cfg!(windows) && !segment::ready(&[]) {
                ", and ONNX Runtime with DirectML to run it"
            } else {
                ""
            },
        ));
        ui.add_space(4.0);
        ui.label(weak(format!("Saved in {}", segment::data_dir().display())));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Download").clicked() {
                answer = Some(true);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(false);
            }
        });
    });
    if modal.should_close() {
        answer = answer.or(Some(false));
    }
    match answer {
        Some(true) => d.start_ai(kind, target),
        Some(false) => d.ai_consent = None,
        None => {}
    }
}

/// A small text button with a thin outline (Add, Subtract…), and an optional ▾.
pub(super) fn pill(ui: &mut egui::Ui, text: &str, menu: bool) -> egui::Response {
    let font = egui::FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, theme::TEXT);
    let w = galley.size().x + if menu { 22.0 } else { 14.0 };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::click());
    let painter = ui.painter();
    let fill = if resp.hovered() { ROW_HOVER } else { Color32::TRANSPARENT };
    painter.rect_filled(rect, 10.0, fill);
    painter.rect_stroke(rect, 10.0, Stroke::new(1.0, Color32::from_gray(0x5a)), egui::StrokeKind::Inside);
    let text_color = if resp.hovered() { theme::TEXT_STRONG } else { theme::TEXT };
    painter.galley(rect.left_center() + egui::vec2(7.0, -galley.size().y / 2.0), galley, text_color);
    if menu {
        super::icons::triangle(
            painter,
            rect.right_center() - egui::vec2(9.0, 0.0),
            3.0,
            super::icons::Dir::Down,
            theme::TEXT_WEAK,
        );
    }
    resp
}

/// LrC's floating Masks panel over the photo: the masks (a row each, with a switch to show
/// or hide it), the selected mask's components, adding to it, and the overlay.
pub(super) fn masks_panel(app: &mut PhotoLibraryApp, ctx: &egui::Context, anchor: Rect) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    let Some(mut m) = d.masking.clone() else { return };
    let mut st = d.settings.clone();
    let mut done = false;
    let mut ai: Option<(AiKind, ToolTarget)> = None;
    let job = d.ai_job.clone();
    let mut gpu = d.ai_gpu;
    selection(&mut m, &st.masks);
    m.hover = None;
    let frame = egui::Frame::new()
        .fill(PANEL_BG)
        .stroke(Stroke::new(1.0, Color32::from_gray(0x16)))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(8))
        .shadow(egui::epaint::Shadow {
            offset: [0, 3],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(110),
        });
    egui::Area::new(egui::Id::unique("masks_panel"))
        .default_pos(anchor.left_top() + egui::vec2(12.0, 12.0))
        .constrain_to(anchor)
        .movable(true)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(220.0);
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
                // Header: title, and a + for a new mask.
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Masks").size(13.5).color(theme::TEXT_STRONG));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (resp, rect) = super::icons::icon_button(ui, egui::vec2(22.0, 20.0), false);
                        super::icons::plus(ui.painter(), rect.center(), 5.0, theme::TEXT_STRONG);
                        let resp = resp.on_hover_text("Create New Mask");
                        egui::Popup::menu(&resp).show(|ui| {
                            tool_menu(ui, |pick| match pick {
                                MenuPick::Tool(kind) => {
                                    m.tool = Some(MaskTool { kind, target: ToolTarget::NewMask })
                                }
                                MenuPick::Ai(k) => ai = Some((k, ToolTarget::NewMask)),
                            });
                        });
                    });
                });
                ui.add_space(4.0);
                if st.masks.is_empty() {
                    ui.label(weak("No masks yet. Click + or press K, M or Shift+M, then draw on the photo."));
                }
                let mut remove: Option<usize> = None;
                for i in 0..st.masks.len() {
                    let current = m.selected == Some(i);
                    // The mask row: tool glyph, name, and the show / hide switch.
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::click());
                    let switch = Rect::from_center_size(
                        rect.right_center() - egui::vec2(18.0, 0.0),
                        egui::vec2(24.0, 13.0),
                    );
                    let painter = ui.painter();
                    let bg = if current {
                        ROW_SELECTED
                    } else if resp.hovered() {
                        ROW_HOVER
                    } else {
                        Color32::TRANSPARENT
                    };
                    painter.rect_filled(rect, 3.0, bg);
                    let visible = st.masks[i].visible;
                    let text = if visible { theme::TEXT_STRONG } else { theme::TEXT_WEAK };
                    let kind = st.masks[i].components.first().map_or(ShapeKind::Brush, |c| kind_of(&c.shape));
                    tool_icon(painter, rect.left_center() + egui::vec2(13.0, 0.0), 5.5, kind, text);
                    painter.text(
                        rect.left_center() + egui::vec2(28.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &st.masks[i].name,
                        egui::FontId::proportional(13.0),
                        text,
                    );
                    let toggle = ui
                        .put(switch, |ui: &mut egui::Ui| {
                            super::icons::toggle_switch(ui, visible, switch.size())
                        })
                        .on_hover_text(if visible { "Hide this mask" } else { "Show this mask" });
                    if toggle.clicked() {
                        st.masks[i].visible = !visible;
                        done = true;
                    } else if resp.clicked() {
                        m.selected = Some(i);
                        m.component = 0;
                    }
                    if resp.hovered() {
                        m.hover = Some(i);
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Delete Mask").clicked() {
                            remove = Some(i);
                            ui.close();
                        }
                    });
                    if !current {
                        continue;
                    }
                    // The selected mask's components: mode, tool, name; the selected one marked.
                    let mut remove_component: Option<usize> = None;
                    for (j, comp) in st.masks[i].components.iter_mut().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 22.0),
                            egui::Sense::click(),
                        );
                        let painter = ui.painter();
                        if resp.hovered() {
                            painter.rect_filled(rect, 3.0, ROW_HOVER);
                        }
                        if m.component == j {
                            let bar = Rect::from_min_size(
                                rect.min + egui::vec2(18.0, 4.0),
                                egui::vec2(2.0, rect.height() - 8.0),
                            );
                            painter.rect_filled(bar, 1.0, SELECTED);
                        }
                        let mode = match (j, comp.mode) {
                            (0, _) | (_, CombineMode::Add) => "+",
                            (_, CombineMode::Subtract) => "−",
                            (_, CombineMode::Intersect) => "∩",
                        };
                        let color = if m.component == j { theme::TEXT_STRONG } else { theme::TEXT };
                        let small = egui::FontId::proportional(12.0);
                        painter.text(
                            rect.left_center() + egui::vec2(30.0, 0.0),
                            egui::Align2::CENTER_CENTER,
                            mode,
                            small.clone(),
                            theme::TEXT_WEAK,
                        );
                        tool_icon(
                            painter,
                            rect.left_center() + egui::vec2(46.0, 0.0),
                            5.0,
                            kind_of(&comp.shape),
                            color,
                        );
                        let name = if comp.invert {
                            format!("{} (inverted)", comp.shape.label())
                        } else {
                            comp.shape.label().to_owned()
                        };
                        painter.text(
                            rect.left_center() + egui::vec2(58.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            name,
                            small,
                            color,
                        );
                        if resp.clicked() {
                            m.component = j;
                        }
                        resp.context_menu(|ui| {
                            if ui.checkbox(&mut comp.invert, "Invert").changed() {
                                done = true;
                            }
                            if ui.button("Delete").clicked() {
                                remove_component = Some(j);
                                ui.close();
                            }
                        });
                    }
                    if let Some(j) = remove_component {
                        st.masks[i].components.remove(j);
                        if st.masks[i].components.is_empty() {
                            remove = Some(i);
                        }
                        done = true;
                    }
                    ui.add_space(3.0);
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        for mode in [CombineMode::Add, CombineMode::Subtract, CombineMode::Intersect] {
                            let resp = pill(ui, mode.label(), true);
                            egui::Popup::menu(&resp).show(|ui| {
                                tool_menu(ui, |pick| match pick {
                                    MenuPick::Tool(kind) => {
                                        m.tool = Some(MaskTool { kind, target: ToolTarget::Component(mode) })
                                    }
                                    MenuPick::Ai(k) => ai = Some((k, ToolTarget::Component(mode))),
                                });
                            });
                        }
                    });
                    ui.add_space(4.0);
                }
                if let Some(i) = remove {
                    st.masks.remove(i);
                    m.selected = (!st.masks.is_empty()).then(|| i.saturating_sub(1));
                    m.component = 0;
                    done = true;
                }
                // The tool waiting for a drag, and the overlay.
                if let Some(tool) = m.tool {
                    ui.add_space(2.0);
                    egui::Frame::new()
                        .fill(Color32::from_rgb(0x24, 0x34, 0x4a))
                        .corner_radius(3)
                        .inner_margin(egui::Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(format!(
                                    "Draw a {} on the photo  ·  Esc cancels",
                                    tool.kind.label()
                                ))
                                .size(11.5)
                                .color(theme::TEXT_STRONG),
                            );
                        });
                }
                if let Some(job) = &job {
                    ui.add_space(2.0);
                    ai_progress(ui, job);
                }
                ui.add_space(4.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Use GPU for AI").size(12.0).color(theme::TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(6.0);
                        let switch = super::icons::toggle_switch(ui, gpu, egui::vec2(24.0, 13.0));
                        let tip = "Run Select Subject, Sky and Background on the graphics card (DirectML) \
                                   when it can: several times faster than the processor";
                        if switch.on_hover_text(tip).clicked() {
                            gpu = !gpu;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Show Overlay").size(12.0).color(theme::TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(6.0);
                        let switch = super::icons::toggle_switch(ui, m.overlay, egui::vec2(24.0, 13.0));
                        if switch.on_hover_text("Show the selected mask in red (O)").clicked() {
                            m.overlay = !m.overlay;
                        }
                    });
                });
            });
        });
    d.masking = Some(m);
    d.ai_gpu = gpu;
    if st != d.settings {
        d.set_live(st);
    }
    if let Some((kind, target)) = ai {
        d.select_ai(kind, target);
    }
    d.request_renders();
    app.ui.ai_gpu = gpu;
    if done {
        app.run(Command::DevelopCommit(None));
    }
}

/// The brush's Size, Feather, Flow and Density, in slider rows like the others (Erase is
/// on the component row).
fn brush_settings(ui: &mut egui::Ui, b: &mut BrushSettings) {
    let defaults = BrushSettings::default();
    let rows: [(&str, &mut f32, f32, f32, u8); 4] = [
        ("Size", &mut b.size, defaults.size, 1.0, 250),
        ("Feather", &mut b.feather, defaults.feather, 0.0, 251),
        ("Flow", &mut b.flow, defaults.flow, 0.0, 252),
        ("Density", &mut b.density, defaults.density, 0.0, 253),
    ];
    for (label, value, default, min, id) in rows {
        // The slider row needs a develop slider for its id and number format; these are
        // stand-ins (whole numbers), not settings.
        let spec = Spec {
            slider: Slider::MaskAmount(id),
            range: (min, 100.0),
            default,
            log: false,
            gradient: None,
            kelvin: false,
        };
        slider_row_as(ui, &spec, label, value);
    }
}

/// One slider of the masking panel; true when the change is finished (to be saved).
fn mask_row(
    ui: &mut egui::Ui,
    st: &mut imaging::develop::DevelopSettings,
    slider: Slider,
    enabled: bool,
) -> bool {
    let default = match slider {
        Slider::MaskAmount(_) => 100.0,
        Slider::MaskFeather(..) => 50.0,
        _ => 0.0,
    };
    let gradient = match slider {
        Slider::Local(_, LocalParam::Temp) => {
            Some([Color32::from_rgb(70, 110, 220), Color32::from_rgb(230, 200, 60)])
        }
        Slider::Local(_, LocalParam::Tint) => {
            Some([Color32::from_rgb(70, 190, 80), Color32::from_rgb(210, 70, 200)])
        }
        _ => None,
    };
    let spec = Spec { slider, range: slider.range(false), default, log: false, gradient, kelvin: false };
    let mut v = slider.get(st);
    let r = ui.add_enabled_ui(enabled, |ui| slider_row(ui, &spec, &mut v));
    if r.inner.changed {
        slider.set(st, v);
    }
    r.inner.done
}

/// The local sliders that act already (the rest come with Texture, Clarity, Dehaze and
/// Detail support for masks).
fn implemented(p: LocalParam) -> bool {
    !matches!(
        p,
        LocalParam::Texture
            | LocalParam::Clarity
            | LocalParam::Dehaze
            | LocalParam::Sharpness
            | LocalParam::Noise
    )
}

/// A section header like the other panels' (title on the right), without collapsing.
pub(super) fn header(ui: &mut egui::Ui, title: &str, icon: Option<ShapeKind>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::SECTION_HEADER);
    let galley =
        painter.layout_no_wrap(title.to_owned(), egui::FontId::proportional(13.5), theme::TEXT_STRONG);
    let pos = rect.right_center() - egui::vec2(14.0 + galley.size().x, galley.size().y / 2.0);
    if let Some(kind) = icon {
        tool_icon(&painter, egui::pos2(pos.x - 14.0, rect.center().y), 5.5, kind, theme::TEXT);
    }
    painter.galley(pos, galley, theme::TEXT_STRONG);
}

/// The right panel while masking: the selected mask's Amount, its component (Invert,
/// Feather or the brush) and the local sliders, in a section like the other panels.
pub(super) fn right_panel(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    let Some(mut m) = d.masking.clone() else { return };
    let mut st = d.settings.clone();
    let mut done: Option<Option<String>> = None;
    let mut leave = false;
    let sel = selection(&mut m, &st.masks);
    let title = sel.map_or("Masking".to_owned(), |(i, _)| st.masks[i].name.clone());
    let icon = sel.map(|(i, j)| kind_of(&st.masks[i].components[j].shape));
    header(ui, &title, icon);
    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let Some((i, j)) = sel else {
            ui.label(weak("Create a mask with + in the Masks panel, or press K (Brush), M (Linear Gradient) or Shift+M (Radial Gradient), then draw on the photo."));
            if m.tool.is_some_and(|t| t.kind == ShapeKind::Brush) {
                ui.checkbox(&mut d.brush.erase, "Erase").on_hover_text("Paint to remove (or hold Alt)");
                brush_settings(ui, &mut d.brush);
            }
            return;
        };
        let idx = i as u8;
        if mask_row(ui, &mut st, Slider::MaskAmount(idx), true) {
            done = Some(None);
        }
        // The selected component.
        ui.add_space(2.0);
        let comp = &mut st.masks[i].components[j];
        let kind = kind_of(&comp.shape);
        ui.horizontal(|ui| {
            ui.add_space(super::develop::LABEL_W - 60.0);
            ui.label(RichText::new(comp.shape.label()).size(12.5).color(theme::TEXT_WEAK));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.checkbox(&mut comp.invert, "Invert").changed() {
                    done = Some(None);
                }
                if kind == ShapeKind::Brush {
                    ui.add_space(8.0);
                    ui.checkbox(&mut d.brush.erase, "Erase").on_hover_text("Paint to remove (or hold Alt)");
                }
            });
        });
        if kind == ShapeKind::Radial && mask_row(ui, &mut st, Slider::MaskFeather(idx, j as u8), true) {
            done = Some(None);
        }
        // The brush's settings with a brush selected, or while the brush tool waits to paint a
        // new mask or component.
        let brush_tool = m.tool.is_some_and(|t| t.kind == ShapeKind::Brush);
        if brush_tool && kind != ShapeKind::Brush {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("New brush").size(12.5).color(theme::TEXT_STRONG));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.checkbox(&mut d.brush.erase, "Erase").on_hover_text("Paint to remove (or hold Alt)");
                });
            });
        }
        if kind == ShapeKind::Brush || brush_tool {
            brush_settings(ui, &mut d.brush);
        }
        for group in LocalParam::GROUPS {
            ui.add_space(5.0);
            let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter().rect_filled(line, 0.0, Color32::from_gray(0x2a));
            ui.add_space(4.0);
            for p in group.iter().copied() {
                if mask_row(ui, &mut st, Slider::Local(idx, p), implemented(p)) {
                    done = Some(None);
                }
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if pill(ui, "Reset Sliders", false).on_hover_text("Set this mask's sliders back to 0").clicked() {
                st.masks[i].adjust = Default::default();
                done = Some(Some(format!("{}: Reset", st.masks[i].name)));
            }
            if pill(ui, "Delete Mask", false).clicked() {
                st.masks.remove(i);
                m.selected = None;
                done = Some(None);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pill(ui, "Close", false).on_hover_text("Leave masking (Shift+W)").clicked() {
                    leave = true;
                }
            });
        });
    });
    d.masking = (!leave).then_some(m);
    if st != d.settings {
        d.set_live(st);
    }
    d.request_renders();
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
}

/// LrC's tool strip under the histogram: the Masking tool (Shift+W), with the number of
/// masks.
pub(super) fn masking_button(app: &mut PhotoLibraryApp, ui: &mut egui::Ui) {
    let Some(s) = &mut app.session else { return };
    let d = &mut s.develop;
    let n = d.settings.masks.len();
    let (strip, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::hover());
    ui.painter().rect_filled(strip, 0.0, theme::SECTION_HEADER);
    // Right to left: Masking, then Crop & Straighten.
    let slot = |k: f32| {
        Rect::from_center_size(
            strip.right_center() - egui::vec2(26.0 + 42.0 * k, 0.0),
            egui::vec2(34.0, 26.0),
        )
    };
    let tool = |ui: &mut egui::Ui, rect: Rect, on: bool, id: &str| {
        let resp = ui.interact(rect, ui.scope_id().with(id), egui::Sense::click());
        if on {
            ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(0x58, 0x58, 0x58));
        } else if resp.hovered() {
            ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(0x44, 0x44, 0x44));
        }
        let color = if on || resp.hovered() { theme::TEXT_STRONG } else { theme::TEXT };
        (resp, color)
    };
    let cropping = d.cropping.is_some();
    let (crop, color) = tool(ui, slot(2.0), cropping, "crop_tool");
    super::icons::crop(ui.painter(), slot(2.0).center(), 7.5, color);
    if crop.on_hover_text("Crop & Straighten (R)").clicked() && d.photo.is_some() {
        super::crop::set_cropping(d, !cropping);
    }
    let removing = d.removing.is_some();
    let (rm, color) = tool(ui, slot(1.0), removing, "remove_tool");
    super::icons::remove(ui.painter(), slot(1.0).center(), 7.5, color);
    if rm.on_hover_text("Remove: Heal and Clone spots (Q)").clicked() && d.photo.is_some() {
        super::remove::set_removing(d, !removing);
    }
    let on = d.masking.is_some();
    let (resp, color) = tool(ui, slot(0.0), on, "masking_tool");
    let painter = ui.painter();
    super::icons::masking(painter, slot(0.0).center(), 7.5, color);
    if n > 0 {
        let badge = slot(0.0).right_top() + egui::vec2(-4.0, 5.0);
        painter.circle_filled(badge, 6.5, SELECTED);
        painter.text(
            badge,
            egui::Align2::CENTER_CENTER,
            n.to_string(),
            egui::FontId::proportional(9.5),
            Color32::WHITE,
        );
    }
    if resp.on_hover_text("Masking: Brush, Linear and Radial Gradient (Shift+W)").clicked()
        && d.photo.is_some()
    {
        if !on {
            d.cropping = None;
            d.removing = None;
        }
        set_masking(d, !on);
    }
}

/// Is the brush out (its tool waiting, or a brush component selected)? Then the wheel
/// sizes it instead of panning.
pub(super) fn brush_active(d: &DevelopState) -> bool {
    let Some(m) = &d.masking else { return false };
    match m.tool {
        Some(t) => t.kind == ShapeKind::Brush,
        None => m
            .selected
            .and_then(|i| d.settings.masks.get(i))
            .and_then(|mask| mask.components.get(m.component))
            .is_some_and(|c| matches!(c.shape, Shape::Brush { .. })),
    }
}

/// Turn masking mode on or off.
pub fn set_masking(d: &mut DevelopState, on: bool) {
    d.masking = on.then(|| Masking {
        selected: (!d.settings.masks.is_empty()).then_some(0),
        overlay: false,
        ..Masking::default()
    });
    if on {
        d.cropping = None;
        d.removing = None;
    }
    d.picking_wb = false;
    d.picking_detail = false;
    d.request_renders();
}

/// Start drawing a new mask of `kind` (M, Shift+M).
pub fn new_mask_tool(d: &mut DevelopState, kind: ShapeKind) {
    if d.masking.is_none() {
        set_masking(d, true);
    }
    if let Some(m) = &mut d.masking {
        m.tool = Some(MaskTool { kind, target: ToolTarget::NewMask });
    }
}

/// Delete (the key): remove the selected component, and the mask with its last one.
pub fn delete_selected(d: &mut DevelopState) -> bool {
    let Some(mut m) = d.masking.clone() else { return false };
    let mut st = d.settings.clone();
    let Some((i, j)) = selection(&mut m, &st.masks) else { return false };
    st.masks[i].components.remove(j);
    if st.masks[i].components.is_empty() {
        st.masks.remove(i);
        m.selected = None;
    }
    m.component = 0;
    d.masking = Some(m);
    d.set_live(st);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Canvas {
        Canvas::new(
            Rect::from_min_size(Pos2::ZERO, egui::vec2(300.0, 200.0)),
            0,
            Transform::default(),
            (3000, 2000),
        )
    }

    #[test]
    fn screen_and_photo_points_round_trip() {
        let c = canvas();
        let q = c.screen([0.25, 0.75]).unwrap();
        assert!((q.x - 75.0).abs() < 1e-3 && (q.y - 150.0).abs() < 1e-3);
        let p = c.photo(q);
        assert!((p[0] - 0.25).abs() < 1e-5 && (p[1] - 0.75).abs() < 1e-5);
    }

    #[test]
    fn handles_move_resize_and_turn_shapes() {
        let c = canvas();
        let lin = Shape::Linear { start: [0.5, 0.2], end: [0.5, 0.6] };
        // The pin is at the middle; the start line at the start.
        assert_eq!(hit(&c, &lin, Pos2::new(150.0, 80.0)), Some(Handle::Move));
        assert_eq!(hit(&c, &lin, Pos2::new(40.0, 40.0)), Some(Handle::Start));
        assert_eq!(hit(&c, &lin, Pos2::new(40.0, 120.0)), Some(Handle::End));
        assert_eq!(hit(&c, &lin, Pos2::new(40.0, 80.0)), Some(Handle::Rotate));
        let moved = drag_shape(&c, &lin, Handle::Move, [0.5, 0.4], [0.6, 0.4]);
        assert_eq!(moved, Shape::Linear { start: [0.6, 0.2], end: [0.6, 0.6] });
        // A quarter turn about the middle makes it horizontal.
        let turned = drag_shape(&c, &lin, Handle::Rotate, [0.6, 0.4], [0.5, 0.55]);
        let Shape::Linear { start, end } = turned else { unreachable!() };
        assert!((start[1] - end[1]).abs() < 1e-3, "{start:?} {end:?}");

        let rad = Shape::Radial { center: [0.5, 0.5], radii: [0.2, 0.1], angle: 0.0, feather: 50.0 };
        // 0.2 half long sides = 300 px = 30 points to the right of the centre.
        assert_eq!(hit(&c, &rad, Pos2::new(180.0, 100.0)), Some(Handle::RadiusX));
        assert_eq!(hit(&c, &rad, Pos2::new(160.0, 100.0)), Some(Handle::Move));
        // Just outside the edge on the diagonal, away from the handles: turn.
        assert_eq!(hit(&c, &rad, Pos2::new(173.3, 111.7)), Some(Handle::Rotate));
        assert_eq!(hit(&c, &rad, Pos2::new(150.0, 117.0)), Some(Handle::RadiusY));
        let wider = drag_shape(&c, &rad, Handle::RadiusX, [0.6, 0.5], [0.7, 0.5]);
        let Shape::Radial { radii, .. } = wider else { unreachable!() };
        assert!((radii[0] - 0.4).abs() < 1e-3 && radii[1] == 0.1);
    }

    #[test]
    fn new_masks_get_free_names() {
        let m = Mask::new("Mask 2", Shape::Brush { strokes: vec![] });
        assert_eq!(next_name(&[]), "Mask 1");
        assert_eq!(next_name(std::slice::from_ref(&m)), "Mask 3");
    }

    #[test]
    fn brush_tool_makes_a_mask_and_strokes_use_the_brush() {
        let mut st = DevelopSettings::default();
        let mut m = Masking::default();
        let brush = BrushSettings { size: 20.0, feather: 30.0, ..BrushSettings::default() };
        let tool = MaskTool { kind: ShapeKind::Brush, target: ToolTarget::NewMask };
        add_shape(&mut st, &mut m, tool, Shape::Brush { strokes: Vec::new() });
        start_stroke(&mut st, &m, &brush, true, [0.123_456, 0.5]);
        assert_eq!((m.selected, m.component, m.tool), (Some(0), 0, None));
        let Shape::Brush { strokes } = &st.masks[0].components[0].shape else { unreachable!() };
        let s = &strokes[0];
        assert_eq!(s.points, vec![[0.1235, 0.5]]);
        assert!((s.size - 0.02).abs() < 1e-6 && s.feather == 30.0 && s.erase);
        // A subtract component goes on the selected mask.
        let sub = MaskTool { kind: ShapeKind::Radial, target: ToolTarget::Component(CombineMode::Subtract) };
        add_shape(&mut st, &mut m, sub, new_shape(ShapeKind::Radial, [0.5, 0.5]).0);
        assert_eq!((st.masks.len(), st.masks[0].components.len(), m.component), (1, 2, 1));
        // Strokes only go into brush components.
        start_stroke(&mut st, &m, &brush, false, [0.5, 0.5]);
        assert!(matches!(st.masks[0].components[1].shape, Shape::Radial { .. }));
    }

    #[test]
    fn brush_keys_step_size_and_feather() {
        let mut b = BrushSettings::default();
        let start = b.size;
        b.step_size(1);
        assert!(b.size > start);
        b.step_size(-1);
        b.step_size(-1);
        assert!(b.size < start);
        for _ in 0..50 {
            b.step_size(-1);
        }
        assert_eq!(b.size, 1.0);
        b.step_feather(1);
        assert_eq!(b.feather, 60.0);
        assert!((BrushSettings { size: 100.0, ..b }.radius() - 0.5).abs() < 1e-6);
        // The wheel: up is larger, down smaller, within 1..100.
        let mut w = BrushSettings::default();
        w.scroll_size(50.0);
        assert!(w.size > 15.5 && w.size < 18.0, "{}", w.size);
        w.scroll_size(-100.0);
        assert!(w.size < 15.0);
        w.scroll_size(10_000.0);
        assert_eq!(w.size, 100.0);
    }
}
