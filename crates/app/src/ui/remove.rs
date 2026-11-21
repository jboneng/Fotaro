//! The Remove tool (Q; LrC's spot removal): click for a round spot, drag to brush one.
//! Remove fills it with what is around it (LaMa, on this computer); Heal and Clone take
//! their pixels from a source found nearby (drag the source to choose, / for another):
//! Heal matches the source to the spot's surroundings, Clone copies it.

use eframe::egui::{self, Align, Color32, Layout, Pos2, Rect, RichText, Stroke};
use imaging::develop::{DevelopSettings, DustLayer, DustPolarity, DustSource, Slider, Spot, SpotArea, SpotMode, SpotStroke};

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
        let area = SpotArea::new(s, c.size.0, c.size.1);
        let scale = screen_scale(c, q);
        // The edge resizes a round spot; on a small one only its outer part does, so the
        // middle still moves it.
        if s.points.len() == 1 && s.strokes.is_empty() {
            let r = s.size * c.half();
            let (ds, rs) = ((q - c.px(s.points[0])).length() * scale, r * scale);
            if (ds - rs).abs() < EDGE && ds > rs * 0.6 {
                return Some(RemoveDrag::Resize { index: i, original: s.clone() });
            }
        }
        // Inside, or within a few screen points of the edge (thin strokes are hard to hit).
        let near = -EDGE / scale.max(1e-3);
        if area.edge(q.x, q.y) > near {
            return Some(RemoveDrag::Spot { index: i, grab, original: s.clone() });
        }
        let off = c.px(s.offset);
        if Some(i) == selected && s.mode != SpotMode::Remove && area.edge(q.x - off.x, q.y - off.y) > near {
            return Some(RemoveDrag::Source { index: i, grab, original: s.clone() });
        }
    }
    None
}

/// Line segments joined end to end into paths (a segment's end is exactly the next one's
/// start: marching squares gives neighbouring cells the same crossing point). A path is
/// drawn as one line, far cheaper than its segments one by one.
fn join_segments(segs: &[[egui::Vec2; 2]]) -> Vec<Vec<egui::Vec2>> {
    use std::collections::HashMap;
    let key = |p: egui::Vec2| (p.x.to_bits(), p.y.to_bits());
    let mut ends: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        ends.entry(key(s[0])).or_default().push(i);
        ends.entry(key(s[1])).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    // The far end of an unused segment touching `p`.
    let next = |p: egui::Vec2, used: &mut [bool]| -> Option<egui::Vec2> {
        let i = *ends.get(&key(p))?.iter().find(|i| !used[**i])?;
        used[i] = true;
        Some(if key(segs[i][0]) == key(p) { segs[i][1] } else { segs[i][0] })
    };
    let mut paths = Vec::new();
    for i in 0..segs.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut path = std::collections::VecDeque::from(segs[i]);
        while let Some(q) = next(path[path.len() - 1], &mut used) {
            path.push_back(q);
        }
        while let Some(q) = next(path[0], &mut used) {
            path.push_front(q);
        }
        paths.push(Vec::from(path));
    }
    paths
}

/// A spot's outline in photo pixels: paths around where it covers anything (marching
/// squares over its area), cached per shape and photo size.
fn contour_px(spot: &Spot, size: (u32, u32)) -> std::sync::Arc<Vec<Vec<egui::Vec2>>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    type Cache = Mutex<HashMap<(u64, u32, (u32, u32)), Arc<Vec<Vec<egui::Vec2>>>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let key = (spot.shape_key(), spot.feather.to_bits(), size);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return v.clone();
    }
    let area = SpotArea::new(spot, size.0, size.1);
    let [l, t, r, b] = area.bounds();
    let mut segs = Vec::new();
    if l < r {
        let (l, t, r, b) = (l - 2.0, t - 2.0, r + 2.0, b + 2.0);
        // Cells about a third of the finest stroke's radius (so thin strokes trace smoothly),
        // at most ~200 across, never under half a pixel.
        let step = (area.finest() / 3.0).max((r - l).max(b - t) / 200.0).max(0.5);
        let (nx, ny) = (((r - l) / step).ceil() as usize + 1, ((b - t) / step).ceil() as usize + 1);
        // The signed distance to the edge: linear, so the crossings interpolate exactly.
        let v: Vec<f32> =
            (0..nx * ny).map(|k| area.edge(l + (k % nx) as f32 * step, t + (k / nx) as f32 * step)).collect();
        let at = |i: usize, j: usize| v[j * nx + i];
        let pt = |i: usize, j: usize| egui::vec2(l + i as f32 * step, t + j as f32 * step);
        // Where the edge between two corners crosses the iso line.
        let cross =
            |p: egui::Vec2, q: egui::Vec2, a: f32, b: f32| p + (q - p) * (a / (a - b)).clamp(0.0, 1.0);
        for j in 0..ny - 1 {
            for i in 0..nx - 1 {
                let (a, b2, c2, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                let case = usize::from(a > 0.0)
                    | usize::from(b2 > 0.0) << 1
                    | usize::from(c2 > 0.0) << 2
                    | usize::from(d > 0.0) << 3;
                if case == 0 || case == 15 {
                    continue;
                }
                let (pa, pb, pc, pd) = (pt(i, j), pt(i + 1, j), pt(i + 1, j + 1), pt(i, j + 1));
                let top = cross(pa, pb, a, b2);
                let right = cross(pb, pc, b2, c2);
                let bottom = cross(pd, pc, d, c2);
                let left = cross(pa, pd, a, d);
                let mut push = |p: egui::Vec2, q: egui::Vec2| segs.push([p, q]);
                match case {
                    1 | 14 => push(left, top),
                    2 | 13 => push(top, right),
                    3 | 12 => push(left, right),
                    4 | 11 => push(right, bottom),
                    6 | 9 => push(top, bottom),
                    7 | 8 => push(left, bottom),
                    5 => {
                        push(left, top);
                        push(right, bottom);
                    }
                    10 => {
                        push(top, right);
                        push(left, bottom);
                    }
                    _ => {}
                }
            }
        }
    }
    let segs = Arc::new(join_segments(&segs));
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() > 512 {
        map.clear();
    }
    map.insert(key, segs.clone());
    segs
}

/// The spot relative to its first point (and that point): outlines are traced like this, so
/// a moved spot reuses its outline.
fn local_spot(spot: &Spot) -> (Spot, [f32; 2]) {
    let o = spot.points.first().copied().unwrap_or_default();
    let mv = |p: &[f32; 2]| [p[0] - o[0], p[1] - o[1]];
    let local = Spot {
        points: spot.points.iter().map(mv).collect(),
        strokes: spot
            .strokes
            .iter()
            .map(|t| SpotStroke { points: t.points.iter().map(mv).collect(), ..t.clone() })
            .collect(),
        fill: None,
        ..spot.clone()
    };
    (local, o)
}

/// Trace a found person's outline on a photo of `size` now (it is kept), so drawing them
/// the first time doesn't have to: for the thread that found them.
pub(crate) fn prepare_person(person: &crate::develop::FoundPerson, size: (u32, u32)) {
    contour_px(&local_spot(&person_spot(person)).0, size);
}

/// A spot's outline on screen, moved by `shift` (0..1 units; its source with the offset):
/// one clean line around the whole area, however the strokes went.
fn outline(c: &Canvas, spot: &Spot, shift: [f32; 2]) -> Vec<Vec<Pos2>> {
    let (local, o) = local_spot(spot);
    let d = c.px([shift[0] + o[0], shift[1] + o[1]]);
    // A path is cut where a point can't be shown.
    let mut paths = Vec::new();
    for path in contour_px(&local, c.size).iter() {
        let mut run = Vec::with_capacity(path.len());
        for p in path {
            match c.screen_px(*p + d) {
                Some(q) => run.push(q),
                None if run.len() > 1 => paths.push(std::mem::take(&mut run)),
                None => run.clear(),
            }
        }
        if run.len() > 1 {
            paths.push(run);
        }
    }
    paths
}

fn draw_outline(painter: &egui::Painter, paths: &[Vec<Pos2>], stroke: Stroke) {
    let shadow = Stroke::new(stroke.width + 2.0, Color32::from_black_alpha(110));
    for stroke in [shadow, stroke] {
        painter.extend(paths.iter().map(|p| egui::Shape::line(p.clone(), stroke)));
    }
}

/// A small turning arc on a dark disc: a spot being filled.
fn spinner(painter: &egui::Painter, at: Pos2, time: f64) {
    painter.circle_filled(at, 9.0, Color32::from_black_alpha(140));
    let start = (time * 4.0) as f32;
    let pts: Vec<Pos2> = (0..=16)
        .map(|k| {
            let t = start + k as f32 / 16.0 * 4.5;
            at + egui::vec2(t.cos(), t.sin()) * 6.0
        })
        .collect();
    painter.add(egui::Shape::line(pts, Stroke::new(2.0, Color32::WHITE)));
}

/// A found person as a spot (for outlining and hit testing).
fn person_spot(f: &crate::develop::FoundPerson) -> Spot {
    let [l, t, r, b] = f.area.rect;
    Spot { points: vec![[(l + r) / 2.0, (t + b) / 2.0]], area: Some(f.area.clone()), feather: 0.0, ..Spot::default() }
}

/// A found person's tint as a texture (their picture white, its coverage as alpha), made
/// once from the picture the search prepared.
fn person_texture(
    ctx: &egui::Context,
    cache: &mut Vec<(String, egui::TextureHandle)>,
    person: &crate::develop::FoundPerson,
) -> Option<egui::TextureHandle> {
    let id = &person.area.id;
    if let Some((_, t)) = cache.iter().find(|(k, _)| k == id) {
        return Some(t.clone());
    }
    let img = person.tint.clone()?;
    let tex = ctx.load_texture(format!("person-{id}"), img, egui::TextureOptions::LINEAR);
    cache.push((id.clone(), tex.clone()));
    Some(tex)
}

/// Paint a person's mask texture over `rect` (0..1 of the photo) in `tint`: a grid of
/// points through the canvas, so rotation and the photo's geometry follow.
fn paint_person(painter: &egui::Painter, c: &Canvas, tex: &egui::TextureHandle, rect: [f32; 4], tint: Color32) {
    const N: usize = 8;
    let [l, t, r, b] = rect;
    let mut at = Vec::with_capacity((N + 1) * (N + 1));
    let mut mesh = egui::Mesh::with_texture(tex.id());
    for j in 0..=N {
        for i in 0..=N {
            let (u, v) = (i as f32 / N as f32, j as f32 / N as f32);
            let pos = c.screen([l + (r - l) * u, t + (b - t) * v]);
            at.push(pos.is_some());
            let pos = pos.unwrap_or(Pos2::ZERO);
            mesh.vertices.push(egui::epaint::Vertex { pos, uv: egui::pos2(u, v), color: tint });
        }
    }
    for j in 0..N {
        for i in 0..N {
            let a = j * (N + 1) + i;
            let (b, c, d) = (a + 1, a + N + 1, a + N + 2);
            if [a, b, c, d].iter().all(|&k| at[k]) {
                mesh.add_triangle(a as u32, b as u32, d as u32);
                mesh.add_triangle(a as u32, d as u32, c as u32);
            }
        }
    }
    painter.add(egui::Shape::mesh(mesh));
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
            fill: None,
            strokes: Vec::new(),
            found: false,
            area: None,
        }
    };

    // Painting into the selected spot: area mode on, or Shift (add) / Ctrl (subtract) held.
    let mods = ui.input(|i| i.modifiers);
    // Dust & Scratches' Subtract brush: drags and clicks paint out of what was found.
    let subtracting = tool.subtract && st.dust.as_ref().is_some_and(|l| l.enabled);
    let extending = tool.selected.filter(|_| !subtracting && (tool.area.is_some() || mods.shift || mods.ctrl));
    let erasing = mods.ctrl || (tool.area == Some(true) && !mods.shift);
    // The wheel sizes new spots (or the selected one; the brush while editing an area).
    if resp.hover_pos().is_some_and(|p| img_rect.contains(p)) {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            match tool.selected.filter(|_| extending.is_none() && !subtracting) {
                Some(i) => {
                    let s = &mut st.spots[i];
                    s.size = spot_radius((spot_size(s.size) * (dy / 300.0).exp2()).clamp(1.0, 100.0));
                }
                None => d.remove.size = (d.remove.size * (dy / 300.0).exp2()).clamp(1.0, 100.0),
            }
        }
    }

    if resp.drag_started()
        && subtracting
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        tool.drag = Some(RemoveDrag::Subtract(vec![c.photo(origin)]));
    } else if resp.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        tool.drag = Some(
            match extending.map_or_else(
                || hit(&c, &st.spots, tool.selected, origin),
                |index| Some(RemoveDrag::Extend { index, path: vec![c.photo(origin)], erase: erasing }),
            ) {
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
            },
        );
    }
    let pointer = ui.input(|i| i.pointer.latest_pos());
    if let Some(q) = pointer {
        let at = c.photo(q);
        match &mut tool.drag {
            Some(RemoveDrag::Paint(path) | RemoveDrag::Extend { path, .. } | RemoveDrag::Subtract(path)) => {
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
                for (t, o) in s.strokes.iter_mut().zip(&original.strokes) {
                    t.points = o.points.iter().map(|p| [p[0] + dx, p[1] + dy]).collect();
                }
                if let (Some(a), Some(o)) = (s.area.as_mut(), original.area.as_ref()) {
                    a.rect = [o.rect[0] + dx, o.rect[1] + dy, o.rect[2] + dx, o.rect[3] + dy];
                }
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
            RemoveDrag::Extend { index, path, erase } => {
                let s = &mut st.spots[index];
                s.strokes.push(SpotStroke { points: path, size: spot_radius(settings.size), erase });
                // Edited: the user's own now (Find again leaves it).
                s.found = false;
                done = true;
            }
            RemoveDrag::Subtract(path) => {
                if let Some(l) = st.dust.as_mut() {
                    l.erase.push(SpotStroke { points: path, size: spot_radius(settings.size), erase: true });
                }
                done = true;
            }
            RemoveDrag::Spot { index, original, .. }
            | RemoveDrag::Source { index, original, .. }
            | RemoveDrag::Resize { index, original } => {
                // Moved, resized or given another source: the user's own now.
                if let Some(s) = st.spots.get_mut(index)
                    && *s != original
                {
                    s.found = false;
                }
                done = true;
            }
        }
    }
    // Found people (Distractions → People): a click on one picks it or leaves it.
    let person_at = |p: Pos2| -> Option<usize> {
        let at = c.px(c.photo(p));
        d.people.iter().position(|f| SpotArea::new(&person_spot(f), size.0, size.1).at(at.x, at.y) > 0.0)
    };
    let mut toggled = None;
    if resp.clicked()
        && !subtracting
        && let Some(p) = resp.interact_pointer_pos()
        && let Some(i) = person_at(p)
    {
        toggled = Some(i);
    }
    // A click: select the spot under it, or make a round one.
    if resp.clicked()
        && toggled.is_none()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let other = match hit(&c, &st.spots, None, p) {
            Some(RemoveDrag::Spot { index, .. } | RemoveDrag::Resize { index, .. })
                if Some(index) != tool.selected =>
            {
                Some(index)
            }
            _ => None,
        };
        if subtracting {
            // A dab of the Subtract brush.
            if let Some(l) = st.dust.as_mut() {
                l.erase.push(SpotStroke { points: vec![c.photo(p)], size: spot_radius(settings.size), erase: true });
            }
            done = true;
        } else if let (Some(_), Some(index)) = (extending, other) {
            // Another spot: select it (and edit its area next).
            tool.selected = Some(index);
        } else if let Some(index) = extending {
            // A dab of area.
            let size = spot_radius(settings.size);
            st.spots[index].strokes.push(SpotStroke { points: vec![c.photo(p)], size, erase: erasing });
            st.spots[index].found = false;
            done = true;
        } else {
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
    }

    // Found people: the ones to remove tinted orange and outlined, the rest outlined
    // faintly; the one under the pointer (here or in the panel's list) brighter.
    let hovered = resp.hover_pos().filter(|p| img_rect.contains(*p)).and_then(person_at);
    let lit = hovered.or(d.people_hover);
    d.people_tex.retain(|(id, _)| d.people.iter().any(|f| f.area.id == *id));
    let people_painter = painter.with_clip_rect(img_rect.intersect(painter.clip_rect()));
    for (i, f) in d.people.iter().enumerate() {
        let on = lit == Some(i);
        let tint = match (f.picked, on) {
            (true, false) => Some(Color32::from_rgba_unmultiplied(255, 140, 30, 90)),
            (true, true) => Some(Color32::from_rgba_unmultiplied(255, 140, 30, 140)),
            (false, true) => Some(Color32::from_white_alpha(40)),
            (false, false) => None,
        };
        if let Some(tint) = tint
            && let Some(tex) = person_texture(ui.ctx(), &mut d.people_tex, f)
        {
            paint_person(&people_painter, &c, &tex, f.area.rect, tint);
        }
        let segs = outline(&c, &person_spot(f), [0.0, 0.0]);
        let width = if on { 2.6 } else { 1.6 };
        let color = if f.picked { Color32::from_rgb(255, 150, 40) } else { Color32::from_white_alpha(110) };
        draw_outline(&people_painter, &segs, Stroke::new(width, color));
    }
    if hovered.is_some() && tool.drag.is_none() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if let Some(i) = toggled {
        d.people[i].picked = !d.people[i].picked;
    }
    // Dust & Scratches' found defects (Show found), under the spots.
    let painter = painter.with_clip_rect(img_rect.intersect(painter.clip_rect()));
    if (d.show_dust || subtracting)
        && let Some(t) = &d.overlay
    {
        super::views::paint_image(&painter, t, img_rect, rotation, Color32::WHITE);
    }
    // Spots: outlines; the selected one with its source and an arrow from it.
    let filling = d.fill_job.as_ref().map(|(_, key)| *key);
    for (i, s) in st.spots.iter().enumerate() {
        let selected = tool.selected == Some(i);
        let stroke = if selected {
            Stroke::new(1.6, Color32::WHITE)
        } else {
            Stroke::new(1.0, Color32::from_white_alpha(150))
        };
        draw_outline(&painter, &outline(&c, s, [0.0, 0.0]), stroke);
        // Being filled: a spinner in the middle.
        if filling == Some(s.shape_key())
            && let Some(at) = c.screen(centre(s))
        {
            spinner(&painter, at, ui.input(|i| i.time));
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
        }
        // Heal and Clone: the source, and an arrow from it.
        if selected && s.mode != SpotMode::Remove {
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
    if let Some(RemoveDrag::Extend { path, erase, .. }) = &tool.drag {
        let preview = Spot { points: path.clone(), ..new_spot(Vec::new()) };
        let color = if *erase { Color32::from_rgb(255, 90, 90) } else { Color32::from_rgb(120, 220, 120) };
        draw_outline(&painter, &outline(&c, &preview, [0.0, 0.0]), Stroke::new(1.5, color));
    }
    if let Some(RemoveDrag::Subtract(path)) = &tool.drag {
        let preview = Spot { points: path.clone(), ..new_spot(Vec::new()) };
        draw_outline(&painter, &outline(&c, &preview, [0.0, 0.0]), Stroke::new(1.5, Color32::from_rgb(90, 200, 255)));
    }
    if let Some(p) = resp.hover_pos().filter(|p| img_rect.contains(*p)) {
        let under = if extending.is_some() || subtracting { None } else { hit(&c, &st.spots, tool.selected, p) };
        match (&tool.drag, under) {
            (None, None) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                let at = c.px(c.photo(p));
                let r = spot_radius(d.remove.size) * c.half() * screen_scale(&c, at);
                painter.circle_stroke(p, r.max(2.0), Stroke::new(3.0, Color32::from_black_alpha(110)));
                painter.circle_stroke(p, r.max(2.0), Stroke::new(1.3, Color32::WHITE));
                // Editing an area: + adds, − takes away (the Subtract brush too).
                if extending.is_some() || subtracting {
                    let sign = if erasing || subtracting { "−" } else { "+" };
                    painter.text(
                        p,
                        egui::Align2::CENTER_CENTER,
                        sign,
                        egui::FontId::proportional(14.0),
                        Color32::WHITE,
                    );
                }
            }
            (None, Some(RemoveDrag::Resize { .. })) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal)
            }
            (None, Some(_)) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grab),
            (Some(RemoveDrag::Paint(_) | RemoveDrag::Extend { .. } | RemoveDrag::Subtract(_)), _) => {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair)
            }
            (Some(_), _) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing),
        }
    }

    d.removing = Some(tool);
    if st != d.settings {
        d.set_live(st);
    }
    if let Some(i) = find.filter(|&i| d.settings.spots[i].mode != SpotMode::Remove) {
        // The new spot's source; saved when the worker has picked it. (Remove spots are
        // filled on their own, and saved then.)
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
    let mut asks = DistractionAsks::default();
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
            for mode in [SpotMode::Remove, SpotMode::Heal, SpotMode::Clone] {
                let tip = match mode {
                    SpotMode::Heal => "Copy texture from the source, matched to the colour and brightness around the spot",
                    SpotMode::Clone => "Copy the source as it is",
                    SpotMode::Remove => "Fill with what is around it (an AI model)",
                };
                if mode_button(ui, mode.label(), values.mode == mode).on_hover_text(tip).clicked() {
                    values.mode = mode;
                    done = Some(None);
                }
            }
        });
        ui.add_space(4.0);
        if values.mode == SpotMode::Remove {
            ui.horizontal(|ui| {
                ui.add_space(super::develop::LABEL_W - 40.0);
                let mut best = d.remove_best;
                let tip = "Fill new Remove spots with the larger model (big-LaMa): better on \
                           large areas and textures. The first time it takes about half a \
                           minute to prepare, then under a second a spot on the GPU";
                if ui.checkbox(&mut best, "Best quality").on_hover_text(tip).changed() {
                    d.remove_best = best;
                    // Its model is another download: ask again.
                    d.fill_allowed = false;
                    let gpu = d.ai_gpu;
                    std::thread::spawn(move || segment::warm_remove(best, gpu));
                }
            });
        }
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
                    s.found = false;
                }
                None => d.remove = values,
            }
        }
        // Settings for new spots aren't History steps.
        if tool.selected.is_none() {
            done = None;
        }
        // Editing the selected spot's area: paint to add to it, or to take some away.
        if tool.selected.is_some() {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_space(super::develop::LABEL_W - 40.0);
                ui.label(RichText::new("Area").size(12.5).color(theme::TEXT));
                for (erase, label, tip) in [
                    (false, "Add", "Paint to add to this spot (or Shift+drag)"),
                    (true, "Subtract", "Paint to take area away from this spot (or Ctrl+drag)"),
                ] {
                    let on = tool.area == Some(erase);
                    if mode_button(ui, label, on).on_hover_text(tip).clicked() {
                        tool.area = (!on).then_some(erase);
                    }
                }
            });
        } else {
            tool.area = None;
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if let Some(i) = tool.selected {
                if st.spots[i].mode != SpotMode::Remove && pill(ui, "New Source", false).on_hover_text("Pick another source for this spot (/)").clicked() {
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
        // Filling: one progress for all the spots waiting (kept between them).
        if let Some(job) = d.fill_batch.as_ref().map(|b| &b.job).or(d.fill_job.as_ref().map(|(j, _)| j)) {
            ui.add_space(6.0);
            super::masking::ai_progress(ui, job);
        }
        distractions(ui, d, &mut st, &mut tool, &mut done, &mut asks);
        ui.add_space(6.0);
        let n = st.spots.len();
        ui.label(RichText::new(format!("{n} spot{}", if n == 1 { "" } else { "s" })).size(12.0).color(theme::TEXT_WEAK));
        ui.label(weak(
            "Click a blemish to remove it, or paint over it. Remove fills it with what is around it (an AI model, on this computer); Heal and Clone copy from a source: with the spot selected, drag the source outline (the thin one, with the arrow) to choose where from, or press / for another. Drag a spot to move it, a round spot's edge to resize it. Area: Add or Subtract (or Shift+drag, Ctrl+drag) paints the selected spot bigger or smaller. The wheel or [ ] sets the size.",
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
    if asks.find_dust {
        d.find_sensor_dust();
    }
    if asks.show_changed {
        d.request_renders();
    }
    // Dust & Scratches: saved now if nothing needs finding, else when the defects are found.
    if asks.dust_changed {
        if d.settings.dust.as_ref().is_some_and(DustLayer::needs_detection) {
            d.request_dust();
        } else {
            done = Some(None);
        }
    }
    if asks.recalculate {
        d.recalculate_dust();
    }
    if asks.find_people {
        d.find_people();
    }
    if asks.remove_people {
        let n = d.remove_people();
        if n > 0 {
            done = Some(Some(format!("Remove People ({n})")));
        }
    }
    if let Some(label) = done {
        app.run(Command::DevelopCommit(label));
    }
}

/// What the Distractions section asks for once the panel is drawn.
#[derive(Default)]
struct DistractionAsks {
    /// Sensor Dust's Find.
    find_dust: bool,
    /// Dust & Scratches' settings changed (find again, or save).
    dust_changed: bool,
    /// Show found was switched.
    show_changed: bool,
    /// Dust & Scratches' Recalculate.
    recalculate: bool,
    /// People: Find, and Remove the picked.
    find_people: bool,
    remove_people: bool,
}

/// The Remove panel's Distractions: Sensor Dust (Find makes spots) and Dust & Scratches
/// (film defects found from the infrared pass or the picture, filled as one layer), then
/// People in a section of its own (found, picked, made into Remove spots).
fn distractions(
    ui: &mut egui::Ui,
    d: &mut DevelopState,
    st: &mut DevelopSettings,
    tool: &mut RemoveTool,
    done: &mut Option<Option<String>>,
    asks: &mut DistractionAsks,
) {
    let spec = |id: u8, range: (f32, f32), default: f32| Spec {
        slider: Slider::MaskAmount(id),
        range,
        default,
        log: false,
        gradient: None,
        kelvin: false,
    };
    let row_label = |ui: &mut egui::Ui, text: &str| {
        ui.label(RichText::new(text).size(12.5).color(theme::TEXT));
    };
    ui.add_space(10.0);
    ui.separator();
    ui.label(RichText::new("Distractions").size(13.5).strong().color(theme::TEXT));
    ui.add_space(6.0);

    // Sensor dust: soft round blobs in a digital photo's smooth areas, found as Heal spots.
    ui.horizontal(|ui| {
        row_label(ui, "Sensor Dust");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let tip = "Find the soft, round spots dust on the sensor leaves in smooth areas \
                       (skies, walls), as Heal spots you can check, move or delete. Finding \
                       again replaces the found spots you haven't changed";
            if pill(ui, "Find", false).on_hover_text(tip).clicked() {
                asks.find_dust = true;
            }
        });
    });
    slider_row_as(ui, &spec(243, (0.0, 100.0), 50.0), "Sensitivity", &mut d.sensor_sensitivity);
    // What Find is doing, or found.
    if d.sensor_finding {
        ui.horizontal(|ui| {
            ui.add_space(super::develop::LABEL_W - 40.0);
            ui.add(egui::Spinner::new().size(12.0));
            ui.label(weak("Finding…").size(12.0));
        });
    } else if let Some(result) = &d.sensor_result {
        ui.horizontal(|ui| {
            ui.add_space(super::develop::LABEL_W - 40.0);
            ui.label(weak(result.as_str()).size(12.0));
        });
    }
    let found = st.spots.iter().filter(|s| s.found).count();
    if found > 0 {
        ui.horizontal(|ui| {
            ui.add_space(super::develop::LABEL_W - 40.0);
            ui.label(weak(format!("{found} found")).size(12.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pill(ui, "Clear Found", false).on_hover_text("Delete the found spots you haven't changed").clicked() {
                    st.spots.retain(|s| !s.found);
                    tool.selected = None;
                    *done = Some(Some("Clear Sensor Dust".into()));
                }
            });
        });
    }

    // Dust & Scratches: a film scan's many specks, hairs and scratches.
    ui.add_space(8.0);
    let has_ir = d.ir_pass.is_some();
    let mut layer = st.dust.clone();
    ui.horizontal(|ui| {
        let on = layer.as_ref().is_some_and(|l| l.enabled);
        let tip = "Find a film scan's dust, hairs and scratches and fill them from around \
                   them. With an infrared pass (NegPy's _IR file beside the scan) they are \
                   found from that";
        if super::icons::toggle_switch(ui, on, egui::vec2(24.0, 13.0)).on_hover_text(tip).clicked() {
            match &mut layer {
                Some(l) => l.enabled = !on,
                None => {
                    let source = if has_ir { DustSource::Both } else { DustSource::Picture };
                    layer = Some(DustLayer { source, ..Default::default() });
                }
            }
            asks.dust_changed = true;
        }
        row_label(ui, "Dust & Scratches");
        if on {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tip = "Find the dust and scratches again with these settings";
                let busy = d.dust_job.is_some();
                if ui.add_enabled_ui(!busy, |ui| pill(ui, "Recalculate", false)).inner.on_hover_text(tip).clicked() {
                    asks.recalculate = true;
                }
            });
        }
    });
    if let Some((_, job)) = &d.dust_job {
        ui.add_space(4.0);
        super::masking::ai_progress(ui, job);
    }
    if let Some(l) = layer.as_mut().filter(|l| l.enabled) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(super::develop::LABEL_W - 40.0);
            row_label(ui, "Find in");
            for source in DustSource::ALL {
                let tip = match source {
                    DustSource::Ir => "The infrared pass: dust and scratches block infrared, the film's dyes don't. Most exact; not for black & white (silver) film",
                    DustSource::Picture => "The picture itself: specks and hairs that stand out from the grain. For scans without infrared, and black & white film",
                    DustSource::Both => "The infrared pass, and small specks the picture shows where the infrared agrees",
                };
                let usable = has_ir || !source.uses_ir();
                let r = ui
                    .add_enabled_ui(usable, |ui| mode_button(ui, source.label(), l.source == source))
                    .inner
                    .on_hover_text(tip)
                    .on_disabled_hover_text("This photo has no infrared pass (a file named like it, ending in _IR)");
                if r.clicked() && l.source != source {
                    l.source = source;
                    asks.dust_changed = true;
                }
            }
        });
        // Which way defects look: picture detail of the other kind is left alone.
        if l.source != DustSource::Ir {
            ui.horizontal(|ui| {
                ui.add_space(super::develop::LABEL_W - 40.0);
                row_label(ui, "Defects");
                for polarity in DustPolarity::ALL {
                    let tip = match polarity {
                        DustPolarity::Light => "Light specks and hairs: scans of negatives (colour or black & white), where dust blocked light",
                        DustPolarity::Dark => "Dark specks and hairs: scans of slides",
                        DustPolarity::Both => "Either way: finds more of the picture's own fine detail too",
                    };
                    if mode_button(ui, polarity.label(), l.polarity == polarity).on_hover_text(tip).clicked()
                        && l.polarity != polarity
                    {
                        l.polarity = polarity;
                        asks.dust_changed = true;
                    }
                }
            });
        }
        if l.source == DustSource::Picture {
            ui.horizontal(|ui| {
                ui.add_space(super::develop::LABEL_W - 40.0);
                let tip = "Find defects with an AI model (Microsoft's scratch detector, run on this \
                           computer): far fewer false finds on detailed pictures, but it takes about \
                           a minute a photo the first time (then the sliders are quick)";
                if ui.checkbox(&mut l.ai, "Use AI (slower, cleaner)").on_hover_text(tip).changed() {
                    asks.dust_changed = true;
                }
            });
        }
        if slider_row_as(ui, &spec(244, (0.0, 100.0), 50.0), "Sensitivity", &mut l.sensitivity).done {
            asks.dust_changed = true;
        }
        if slider_row_as(ui, &spec(245, (2.0, 60.0), 12.0), "Max Size", &mut l.max_size).done {
            asks.dust_changed = true;
        }
        ui.horizontal(|ui| {
            ui.add_space(super::develop::LABEL_W - 40.0);
            let tip = "Show what was found in red";
            if ui.checkbox(&mut d.show_dust, "Show found").on_hover_text(tip).changed() {
                asks.show_changed = true;
            }
        });
        // The Subtract brush: paint out what was found but isn't dust (the picture's own
        // detail). Kept when the defects are found again.
        ui.horizontal(|ui| {
            ui.add_space(super::develop::LABEL_W - 40.0);
            row_label(ui, "Brush");
            let tip = "Paint over found areas that aren't dust to leave them as they are (they \
                       show in red while the brush is on). The wheel or [ ] sets its size";
            if mode_button(ui, "Subtract", tool.subtract).on_hover_text(tip).clicked() {
                tool.subtract = !tool.subtract;
                if tool.subtract {
                    (tool.selected, tool.area) = (None, None);
                }
                asks.show_changed = true;
            }
            if !l.erase.is_empty() {
                let n = l.erase.len();
                let tip = format!("Remove the {n} subtracted stroke{}", if n == 1 { "" } else { "s" });
                if pill(ui, &format!("Clear ({n})"), false).on_hover_text(tip).clicked() {
                    l.erase.clear();
                    asks.dust_changed = true;
                }
            }
        });
        if !has_ir && l.source == DustSource::Picture {
            ui.label(weak("Without an infrared pass, specks and hairs are found from the picture: check with Show found, and paint out what it misses with spots.").size(11.5));
        }
    }
    if layer != st.dust {
        st.dust = layer;
    }

    // People, a section of its own: find them, pick on the photo or in the list, remove.
    ui.add_space(12.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("People").size(13.5).strong().color(theme::TEXT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let tip = "Find the people in the photo (AI models, on this computer). The ones to \
                       remove are shown in orange: click one on the photo, or untick it in the \
                       list, to leave it in. Then Remove";
            let busy = d.people_job.is_some();
            let label = if d.people.is_empty() { "Find" } else { "Find Again" };
            if ui.add_enabled_ui(!busy, |ui| pill(ui, label, false)).inner.on_hover_text(tip).clicked() {
                asks.find_people = true;
            }
        });
    });
    ui.add_space(4.0);
    let mut hover = None;
    if let Some(job) = &d.people_job {
        super::masking::ai_progress(ui, job);
    } else if !d.people.is_empty() {
        let n = d.people.len();
        let picked = d.people.iter().filter(|p| p.picked).count();
        ui.horizontal(|ui| {
            ui.label(weak(format!("{n} found, {picked} to remove")).size(12.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pill(ui, "None", false).on_hover_text("Leave everyone in").clicked() {
                    d.people.iter_mut().for_each(|p| p.picked = false);
                }
                if pill(ui, "All", false).on_hover_text("Remove everyone found").clicked() {
                    d.people.iter_mut().for_each(|p| p.picked = true);
                }
            });
        });
        // Each person: ticked to remove; hovering one shows it on the photo.
        egui::ScrollArea::vertical().id_salt("found-people").max_height(170.0).show(ui, |ui| {
            for (i, p) in d.people.iter_mut().enumerate() {
                let text = format!("Person {}  ·  {:.0}% sure", i + 1, p.score * 100.0);
                let r = ui.checkbox(&mut p.picked, RichText::new(text).size(12.0));
                if r.hovered() {
                    hover = Some(i);
                }
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let tip = "Remove the people shown in orange: each becomes a Remove spot you can \
                           still edit or delete";
                if ui
                    .add_enabled_ui(picked > 0, |ui| pill(ui, &format!("Remove ({picked})"), false))
                    .inner
                    .on_hover_text(tip)
                    .clicked()
                {
                    asks.remove_people = true;
                }
            });
        });
        ui.label(
            weak("Orange: to be removed. Click a person on the photo, or untick them here, to leave them in. Those the finder was less sure of start unticked.")
                .size(11.5),
        );
    } else if d.people_searched {
        ui.label(weak("No people found").size(12.0));
    }
    d.people_hover = hover;
}

/// Open or close the Remove tool (Q); the crop tool and masking close.
pub fn set_removing(d: &mut DevelopState, on: bool) {
    d.removing = on.then(RemoveTool::default);
    if on {
        // The Remove model takes a while to load: start now, before the first stroke.
        let (best, gpu) = (d.remove_best, d.ai_gpu);
        std::thread::spawn(move || segment::warm_remove(best, gpu));
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
    if let Some(i) = d
        .removing
        .as_ref()
        .and_then(|t| t.selected)
        .filter(|&i| i < d.settings.spots.len())
        .filter(|&i| d.settings.spots[i].mode != SpotMode::Remove)
    {
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

    #[test]
    fn segments_join_into_paths() {
        let v = egui::vec2;
        // A square given out of order and in mixed directions, and a lone segment.
        let segs = [
            [v(0.0, 0.0), v(1.0, 0.0)],
            [v(1.0, 1.0), v(0.0, 1.0)],
            [v(5.0, 5.0), v(6.0, 5.0)],
            [v(1.0, 1.0), v(1.0, 0.0)],
            [v(0.0, 1.0), v(0.0, 0.0)],
        ];
        let mut paths = join_segments(&segs);
        paths.sort_by_key(|p| p.len());
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].len(), 2);
        // Closed: five points, the first again at the end.
        assert_eq!(paths[1].len(), 5);
        assert_eq!(paths[1][0], paths[1][4]);
    }
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
