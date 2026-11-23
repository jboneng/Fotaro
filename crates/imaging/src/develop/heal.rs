//! The Remove tool's Heal and Clone (LrC's spot removal): spots painted on the photo, each
//! filled from a source area nearby. Clone copies the source as is; Heal copies its
//! texture and matches it to the colour and brightness around the spot.
//!
//! Spots live in the untransformed photo's coordinates and are applied to the photo's own
//! pixels before anything else (as LrC does), so the Transform, crop and masks follow
//! them. The healed copy is cached with the image until the spots change; so is the photo
//! with only the spots before the one being worked on, so adding, moving or resizing a spot
//! redoes that spot and the ones after it, not all of them.
//!
//! Healing: for the spot's area, the source's pixels times the ratio of the destination's
//! and the source's local average colour, both measured on the ring around the spot only
//! (a normalised convolution, so the blemish itself never counts).

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::local::box_blur_par;
use super::source::WorkingImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SpotMode {
    #[default]
    Heal,
    Clone,
    /// Content-aware: filled by a model (its picture in `Spot::fill`).
    Remove,
}

impl SpotMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Heal => "Heal",
            Self::Clone => "Clone",
            Self::Remove => "Remove",
        }
    }
}

/// One spot: a circle (one point) or a brushed path, filled from the same shape moved by
/// `offset`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Spot {
    pub mode: SpotMode,
    /// Where it is, in 0..1 of the untransformed photo.
    pub points: Vec<[f32; 2]>,
    /// Radius in half long sides of the photo.
    pub size: f32,
    /// Feather and Opacity, 0..100.
    pub feather: f32,
    pub opacity: f32,
    /// Where the pixels come from, relative to the spot (0..1 of the photo).
    pub offset: [f32; 2],
    /// Remove: the model's fill (made for the spot's current shape, else ignored).
    pub fill: Option<Fill>,
    /// Strokes painted into the spot afterwards: more area, or less (`erase`).
    pub strokes: Vec<SpotStroke>,
    /// Made by Find (sensor dust), not painted: Find again replaces it, Clear found removes it.
    pub found: bool,
    /// An area given as a picture (People: a person's outline) instead of the spot's own
    /// path; strokes still add to it or take from it.
    pub area: Option<SpotMask>,
}

/// A spot's area as a picture in the mask store (`<id>.png`, 255 inside) over `rect`
/// (left, top, right, bottom; 0..1 of the photo, file orientation).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotMask {
    pub id: String,
    pub rect: [f32; 4],
}

/// A stroke added to a spot: a path (0..1 of the photo) and its radius (half long sides);
/// `erase` takes area away instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpotStroke {
    pub points: Vec<[f32; 2]>,
    pub size: f32,
    #[serde(default)]
    pub erase: bool,
}

impl Default for Spot {
    fn default() -> Self {
        Spot {
            mode: SpotMode::Heal,
            points: Vec::new(),
            size: 0.02,
            feather: 50.0,
            opacity: 100.0,
            offset: [0.05, 0.0],
            fill: None,
            strokes: Vec::new(),
            found: false,
            area: None,
        }
    }
}

/// A Remove spot's fill: a picture (`<id>.png` in the mask store, see
/// [`super::masks::save_fill_picture`]) covering `rect` (left, top, right, bottom; 0..1 of the
/// photo), in the colours the model saw: `T(p) = sRGB(p · gains)`. `made_for` is the spot
/// shape it was made for ([`Spot::shape_key`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    pub id: String,
    pub rect: [f32; 4],
    pub gains: [f32; 3],
    pub made_for: u64,
    /// What made it (for information).
    #[serde(default)]
    pub model: String,
    /// How much detail the picture holds: model pixels per picture pixel (1: all there
    /// is). 0 in fills made before this was kept (worked out from the picture's size).
    #[serde(default)]
    pub detail: f32,
    /// The picture is at the photo's own size, with the photo's fine detail and grain in
    /// it (see [`super::fill_detail`]): nothing is added when it is shown.
    #[serde(default)]
    pub full: bool,
}

impl Spot {
    /// What a fill depends on: the spot's path and size.
    pub fn shape_key(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |v: u32| h = (h ^ u64::from(v)).wrapping_mul(0x0000_0100_0000_01b3);
        for v in self.points.iter().flatten().chain([&self.size]) {
            eat(v.to_bits());
        }
        for s in &self.strokes {
            for v in s.points.iter().flatten().chain([&s.size]) {
                eat(v.to_bits());
            }
            eat(u32::from(s.erase) + 7);
        }
        if let Some(a) = &self.area {
            for b in a.id.bytes() {
                eat(u32::from(b));
            }
            for v in a.rect {
                eat(v.to_bits());
            }
        }
        h
    }

    /// A Remove spot's fill, if it was made for the spot as it is now.
    pub fn current_fill(&self) -> Option<&Fill> {
        self.fill.as_ref().filter(|f| self.mode == SpotMode::Remove && f.made_for == self.shape_key())
    }

    /// A Remove spot still waiting for its fill.
    pub fn needs_fill(&self) -> bool {
        self.mode == SpotMode::Remove && !self.points.is_empty() && self.current_fill().is_none()
    }
}

#[inline]
fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

#[inline]
fn srgb_decode(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// A photo pixel as the model sees it (8-bit sRGB after `gains`).
pub fn fill_encode(p: [f32; 3], gains: [f32; 3]) -> [u8; 3] {
    std::array::from_fn(|c| (srgb_encode(p[c] * gains[c]) * 255.0).round() as u8)
}

/// The inverse: a model pixel (0..255, fractional after resampling) back to the photo.
pub fn fill_decode(v: [f32; 3], gains: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|c| srgb_decode(v[c] / 255.0) / gains[c].max(1e-6))
}

/// What a Remove spot's model sees: the photo (with the spots before it applied) around
/// the spot, the area to fill, where that is (`rect`, 0..1) and the colour gains.
pub struct FillInput {
    pub image: image::RgbImage,
    /// 255 where to fill.
    pub mask: image::GrayImage,
    pub rect: [f32; 4],
    pub gains: [f32; 3],
}

/// The input for spot `index` of `spots` on `img`: a square around the spot grown by 1.5
/// radii (at least 64 px), white balanced by `wb` and scaled so the bright end sits near
/// 0.9 before sRGB encoding.
pub fn fill_input(
    img: &WorkingImage,
    spots: &[Spot],
    dust: Option<&super::dust::DustLayer>,
    index: usize,
    wb: [f32; 3],
) -> Option<FillInput> {
    let spot = spots.get(index)?;
    let before = healed(img, &spots[..index], dust);
    let img: &WorkingImage = &before;
    let f = Frame::of(img);
    let s = SpotPx::new(spot, f);
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (bx, by, bw, bh) = s.bbox(0.0, w, h);
    if bw == 0 || bh == 0 {
        return None;
    }
    // Room around the spot for the model to take from. An area picture (a person) is big:
    // less room keeps more of its detail at the model's size.
    let grow = (if s.raster.is_some() { 0.75 } else { 1.5 } * s.r).max(64.0);
    let side = ((bw.max(bh) as f32 + 2.0 * grow).ceil() as usize).min(w.min(h));
    let (cx, cy) = (bx as f32 + bw as f32 / 2.0, by as f32 + bh as f32 / 2.0);
    let x0 = ((cx - side as f32 / 2.0).round().max(0.0) as usize).min(w - side);
    let y0 = ((cy - side as f32 / 2.0).round().max(0.0) as usize).min(h - side);
    let px = img.pixels.as_raw();
    let at = |x: usize, y: usize| {
        let i = 3 * (y * w + x);
        [px[i], px[i + 1], px[i + 2]]
    };
    // Exposure: the 99th percentile of the crop's brightest channel (after white balance)
    // to 0.9.
    let mut bright: Vec<f32> = (0..side * side)
        .step_by(((side * side) / 20_000).max(1))
        .map(|k| {
            let p = at(x0 + k % side, y0 + k / side);
            (p[0] * wb[0]).max(p[1] * wb[1]).max(p[2] * wb[2])
        })
        .collect();
    bright.sort_by(f32::total_cmp);
    let top = bright[((bright.len() - 1) as f32 * 0.99) as usize].max(1e-4);
    let k = (0.9 / top).clamp(0.05, 64.0);
    let gains = wb.map(|g| g * k);
    let image = image::RgbImage::from_fn(side as u32, side as u32, |x, y| {
        image::Rgb(fill_encode(at(x0 + x as usize, y0 + y as usize), gains))
    });
    let mask = image::GrayImage::from_fn(side as u32, side as u32, |x, y| {
        image::Luma([if s.coverage(x0 + x as usize, y0 + y as usize) > 0.0 { 255 } else { 0 }])
    });
    let rect = [x0 as f32 / f.w, y0 as f32 / f.h, (x0 + side) as f32 / f.w, (y0 + side) as f32 / f.h];
    Some(FillInput { image, mask, rect, gains })
}

/// The part `rect` (0..1) of the photo with `spots` applied, as a model sees it (8-bit
/// sRGB after `gains`): for looking at what a fill came to.
pub fn healed_crop(
    img: &WorkingImage,
    spots: &[Spot],
    dust: Option<&super::dust::DustLayer>,
    rect: [f32; 4],
    gains: [f32; 3],
) -> image::RgbImage {
    let done = healed(img, spots, dust);
    let img: &WorkingImage = &done;
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (x0, y0) = ((rect[0] * w as f32).round() as usize, (rect[1] * h as f32).round() as usize);
    let (x1, y1) =
        (((rect[2] * w as f32).round() as usize).min(w), ((rect[3] * h as f32).round() as usize).min(h));
    let px = img.pixels.as_raw();
    image::RgbImage::from_fn(x1.saturating_sub(x0) as u32, y1.saturating_sub(y0) as u32, |x, y| {
        let i = 3 * ((y0 + y as usize) * w + x0 + x as usize);
        image::Rgb(fill_encode([px[i], px[i + 1], px[i + 2]], gains))
    })
}

/// The photo's pixel frame: 0..1 coordinates and sizes to pixels.
#[derive(Clone, Copy)]
struct Frame {
    w: f32,
    h: f32,
    half: f32,
}

impl Frame {
    fn of(img: &WorkingImage) -> Frame {
        let (w, h) = (img.width() as f32, img.height() as f32);
        Frame { w, h, half: w.max(h) / 2.0 }
    }

    fn px(&self, p: [f32; 2]) -> [f32; 2] {
        [p[0] * self.w, p[1] * self.h]
    }
}

/// Distance from (x, y) to a path of points (a circle's centre when it has one).
#[inline]
fn dist_to_path(path: &[[f32; 2]], x: f32, y: f32) -> f32 {
    match path {
        [] => f32::MAX,
        [p] => (x - p[0]).hypot(y - p[1]),
        _ => path
            .windows(2)
            .map(|w| {
                let (a, b) = (w[0], w[1]);
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let len2 = (dx * dx + dy * dy).max(1e-9);
                let t = (((x - a[0]) * dx + (y - a[1]) * dy) / len2).clamp(0.0, 1.0);
                (x - a[0] - t * dx).hypot(y - a[1] - t * dy)
            })
            .fold(f32::MAX, f32::min),
    }
}

#[inline]
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One stroke of a spot in pixels: its path, radius, solid inner radius, and whether it
/// takes area away.
struct StrokePx {
    path: Vec<[f32; 2]>,
    r: f32,
    r_in: f32,
    erase: bool,
}

/// A spot's area picture in pixels: the picture's signed distance (its pixels: positive
/// inside), where it sits on the photo, its scale (picture pixels a photo pixel) and the
/// feather (photo pixels).
struct RasterPx {
    sdf: Arc<Vec<f32>>,
    pw: usize,
    ph: usize,
    x0: f32,
    y0: f32,
    scale: f32,
    feather: f32,
    /// Photo pixels: left, top, right, bottom.
    bounds: [f32; 4],
}

impl RasterPx {
    fn new(area: &SpotMask, f: Frame, feather: f32) -> Option<RasterPx> {
        let (sdf, pw, ph) = area_distance(&area.id)?;
        let [l, t, r, b] = area.rect;
        let (x0, y0, x1, y1) = (l * f.w, t * f.h, r * f.w, b * f.h);
        let scale = pw as f32 / (x1 - x0).max(1e-3);
        let span = (x1 - x0).min(y1 - y0).max(1.0);
        Some(RasterPx {
            sdf,
            pw,
            ph,
            x0,
            y0,
            scale,
            feather: 1.0 + feather * 0.04 * span,
            bounds: [x0, y0, x1, y1],
        })
    }

    /// Signed distance (photo pixels) to the picture's outline at (x, y): positive inside.
    fn edge(&self, x: f32, y: f32) -> f32 {
        let fx = (x - self.x0) * self.scale - 0.5;
        let fy = (y - self.y0) * self.scale - 0.5;
        let (mx, my) = ((self.pw - 1) as f32, (self.ph - 1) as f32);
        // Outside the picture: how far beyond it, and beyond the picture's own border value.
        let (cx, cy) = (fx.clamp(0.0, mx), fy.clamp(0.0, my));
        let out = (fx - cx).hypot(fy - cy);
        let (ix, iy) = (cx as usize, cy as usize);
        let (jx, jy) = ((ix + 1).min(self.pw - 1), (iy + 1).min(self.ph - 1));
        let (tx, ty) = (cx - ix as f32, cy - iy as f32);
        let at = |x: usize, y: usize| self.sdf[y * self.pw + x];
        let top = at(ix, iy) + tx * (at(jx, iy) - at(ix, iy));
        let bottom = at(ix, jy) + tx * (at(jx, jy) - at(ix, jy));
        let v = top + ty * (bottom - top);
        (if out > 0.0 { v.min(0.0) - out } else { v }) / self.scale
    }

    /// Coverage (0..1) at (x, y): 1 inside, fading out over the feather at the outline.
    fn at(&self, x: f32, y: f32) -> f32 {
        smoothstep(-0.5, self.feather, self.edge(x, y))
    }
}

/// An area picture's signed distance: its id, the distances, its width and height.
type AreaDistance = (String, Arc<Vec<f32>>, usize, usize);

/// Area pictures' signed distances (picture pixels, positive inside), made once each.
static AREA_DISTANCES: std::sync::LazyLock<Mutex<Vec<AreaDistance>>> =
    std::sync::LazyLock::new(|| Mutex::new(Vec::new()));

/// Get area picture `id` ready for use (its signed distance takes a moment to make for a
/// large picture): call it where the picture is made, off the thread that draws.
pub fn prepare_area(id: &str) {
    area_distance(id);
}

/// Distances kept, in values (4 bytes each): 256 MB. A found person's picture is their
/// size in the full-size photo, so a crowd is many small ones or a few large.
const AREA_DISTANCE_VALUES: usize = 64_000_000;

/// The signed distance of area picture `id` (and its size), from the mask store.
fn area_distance(id: &str) -> Option<(Arc<Vec<f32>>, usize, usize)> {
    {
        let cache = AREA_DISTANCES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, d, w, h)) = cache.iter().find(|(k, ..)| k == id) {
            return Some((d.clone(), *w, *h));
        }
    }
    let pic = super::masks::mask_picture(id)?;
    let (w, h) = (pic.width() as usize, pic.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let inside: Vec<bool> = pic.as_raw().iter().map(|v| *v >= 128).collect();
    let sdf = Arc::new(signed_distance(&inside, w, h));
    let mut cache = AREA_DISTANCES.lock().unwrap_or_else(|e| e.into_inner());
    cache.push((id.to_owned(), sdf.clone(), w, h));
    // The oldest go when they are too many (the newest always stays).
    while cache.len() > 1 && cache.iter().map(|c| c.1.len()).sum::<usize>() > AREA_DISTANCE_VALUES {
        cache.remove(0);
    }
    Some((sdf, w, h))
}

/// Signed distance (pixels; positive inside, negative outside, ±0.5 at the boundary) of a
/// `w × h` mask, by two chamfer passes each way (3-4 weights: within a few % of Euclidean).
fn signed_distance(inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    let dist_to = |target: bool| -> Vec<f32> {
        let big = 1e9f32;
        let mut d: Vec<f32> = inside.iter().map(|v| if *v == target { 0.0 } else { big }).collect();
        let (a, b) = (1.0f32, std::f32::consts::SQRT_2);
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let mut v = d[i];
                if x > 0 {
                    v = v.min(d[i - 1] + a);
                }
                if y > 0 {
                    v = v.min(d[i - w] + a);
                    if x > 0 {
                        v = v.min(d[i - w - 1] + b);
                    }
                    if x + 1 < w {
                        v = v.min(d[i - w + 1] + b);
                    }
                }
                d[i] = v;
            }
        }
        for y in (0..h).rev() {
            for x in (0..w).rev() {
                let i = y * w + x;
                let mut v = d[i];
                if x + 1 < w {
                    v = v.min(d[i + 1] + a);
                }
                if y + 1 < h {
                    v = v.min(d[i + w] + a);
                    if x + 1 < w {
                        v = v.min(d[i + w + 1] + b);
                    }
                    if x > 0 {
                        v = v.min(d[i + w - 1] + b);
                    }
                }
                d[i] = v;
            }
        }
        d
    };
    let (to_out, to_in) = (dist_to(false), dist_to(true));
    (0..w * h).map(|i| if inside[i] { to_out[i] - 0.5 } else { 0.5 - to_in[i] }).collect()
}

/// A spot in pixels: its strokes (the first is the spot's own path, unless it has an area
/// picture), the area picture, the largest radius of what adds area, and the source offset.
struct SpotPx {
    strokes: Vec<StrokePx>,
    raster: Option<RasterPx>,
    r: f32,
    off: (isize, isize),
}

impl SpotPx {
    fn new(s: &Spot, f: Frame) -> SpotPx {
        let feather = s.feather.clamp(0.0, 100.0) / 100.0;
        let stroke = |points: &[[f32; 2]], size: f32, erase: bool| {
            let r = (size * f.half).max(1.0);
            StrokePx {
                path: points.iter().map(|p| f.px(*p)).collect(),
                r,
                r_in: (r * (1.0 - feather)).min(r - 0.75).max(0.0),
                erase,
            }
        };
        let raster = s.area.as_ref().and_then(|a| RasterPx::new(a, f, feather));
        let own = if raster.is_some() { None } else { Some(stroke(&s.points, s.size, false)) };
        let strokes: Vec<StrokePx> = own
            .into_iter()
            .chain(
                s.strokes.iter().filter(|t| !t.points.is_empty()).map(|t| stroke(&t.points, t.size, t.erase)),
            )
            .collect();
        let raster_r =
            raster.as_ref().map_or(1.0, |p| (p.bounds[2] - p.bounds[0]).max(p.bounds[3] - p.bounds[1]) / 2.0);
        let r = strokes.iter().filter(|t| !t.erase).map(|t| t.r).fold(raster_r, f32::max);
        SpotPx {
            strokes,
            raster,
            r,
            off: ((s.offset[0] * f.w).round() as isize, (s.offset[1] * f.h).round() as isize),
        }
    }

    /// How much of the spot covers point (x, y) (pixels): 1 inside, fading over the
    /// feather; strokes that erase take their part away.
    #[inline]
    fn at(&self, x: f32, y: f32) -> f32 {
        let (mut add, mut sub) = (0.0f32, 0.0f32);
        for t in &self.strokes {
            let d = dist_to_path(&t.path, x, y);
            if d >= t.r {
                continue;
            }
            let v = 1.0 - smoothstep(t.r_in, t.r, d);
            if t.erase {
                sub = sub.max(v);
            } else {
                add = add.max(v);
            }
        }
        if let Some(p) = &self.raster {
            add = add.max(p.at(x, y));
        }
        add * (1.0 - sub)
    }

    /// Signed distance (pixels) to the spot's outer edge at (x, y): positive inside, negative
    /// outside (linear, unlike the coverage, so its outline traces cleanly).
    fn edge(&self, x: f32, y: f32) -> f32 {
        let (mut add, mut sub) = (f32::MIN, f32::MIN);
        for t in &self.strokes {
            let v = t.r - dist_to_path(&t.path, x, y);
            if t.erase {
                sub = sub.max(v);
            } else {
                add = add.max(v);
            }
        }
        if let Some(p) = &self.raster {
            add = add.max(p.edge(x, y));
        }
        if sub == f32::MIN { add } else { add.min(-sub) }
    }

    /// How much of the spot covers pixel (x, y).
    #[inline]
    fn coverage(&self, x: usize, y: usize) -> f32 {
        self.at(x as f32 + 0.5, y as f32 + 0.5)
    }

    /// The bounds of the strokes adding area: left, top, right, bottom (pixels).
    fn bounds(&self) -> [f32; 4] {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for t in self.strokes.iter().filter(|t| !t.erase) {
            for p in &t.path {
                b = [b[0].min(p[0] - t.r), b[1].min(p[1] - t.r), b[2].max(p[0] + t.r), b[3].max(p[1] + t.r)];
            }
        }
        if let Some(p) = &self.raster {
            let q = p.bounds;
            b = [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[2]), b[3].max(q[3])];
        }
        b
    }

    /// The spot's bounding box grown by `grow` pixels, within a `w × h` image.
    fn bbox(&self, grow: f32, w: usize, h: usize) -> (usize, usize, usize, usize) {
        let [lo_x, lo_y, hi_x, hi_y] = self.bounds();
        if lo_x > hi_x {
            return (0, 0, 0, 0);
        }
        let x0 = (lo_x - grow).floor().clamp(0.0, w as f32) as usize;
        let y0 = (lo_y - grow).floor().clamp(0.0, h as f32) as usize;
        let x1 = (hi_x + grow).ceil().clamp(0.0, w as f32) as usize;
        let y1 = (hi_y + grow).ceil().clamp(0.0, h as f32) as usize;
        (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    }
}

/// A spot's area on a photo `w × h` pixels, for drawing its outline and hit testing.
pub struct SpotArea(SpotPx);

impl SpotArea {
    pub fn new(spot: &Spot, w: u32, h: u32) -> SpotArea {
        let (w, h) = (w as f32, h as f32);
        SpotArea(SpotPx::new(spot, Frame { w, h, half: w.max(h) / 2.0 }))
    }

    /// Coverage (0..1) at photo pixel (x, y).
    pub fn at(&self, x: f32, y: f32) -> f32 {
        self.0.at(x, y)
    }

    /// Signed distance (photo pixels) to the area's edge: positive inside.
    pub fn edge(&self, x: f32, y: f32) -> f32 {
        self.0.edge(x, y)
    }

    /// The smallest radius of the strokes (pixels): how fine the outline must be traced.
    pub fn finest(&self) -> f32 {
        // An area picture: its outline is traced at about a hundredth of its size.
        let raster = self.0.raster.as_ref().map_or(f32::MAX, |p| {
            ((p.bounds[2] - p.bounds[0]).max(p.bounds[3] - p.bounds[1]) / 100.0).max(1.0)
        });
        self.0.strokes.iter().map(|t| t.r).fold(raster, f32::min)
    }

    /// Left, top, right, bottom of the area, in photo pixels.
    pub fn bounds(&self) -> [f32; 4] {
        self.0.bounds()
    }
}

/// Apply one spot to interleaved RGB pixels of a `w × h` image.
fn apply_spot(px: &mut [f32], w: usize, h: usize, spot: &Spot, f: Frame) {
    if spot.points.is_empty() || spot.opacity <= 0.0 {
        return;
    }
    if spot.mode == SpotMode::Remove {
        apply_fill(px, w, h, spot, f);
        return;
    }
    let s = SpotPx::new(spot, f);
    let opacity = spot.opacity.clamp(0.0, 100.0) / 100.0;
    // The ring around the spot that Heal measures colour on reaches about a radius out.
    let blur = (s.r * 0.7).max(1.0) as usize;
    let grow = if spot.mode == SpotMode::Heal { 2.2 * s.r + 2.0 } else { 1.0 };
    let (x0, y0, bw, bh) = s.bbox(grow, w, h);
    if bw == 0 || bh == 0 {
        return;
    }
    let at = |x: isize, y: isize, c: usize| {
        let (x, y) = (x.clamp(0, w as isize - 1) as usize, y.clamp(0, h as isize - 1) as usize);
        px[3 * (y * w + x) + c]
    };
    let n = bw * bh;
    let cover: Vec<f32> = (0..n).map(|k| s.coverage(x0 + k % bw, y0 + k / bw)).collect();
    // Source and destination over the box.
    let plane = |c: usize, shift: (isize, isize)| -> Vec<f32> {
        (0..n).map(|k| at((x0 + k % bw) as isize + shift.0, (y0 + k / bw) as isize + shift.1, c)).collect()
    };
    let mut out: [Vec<f32>; 3] = std::array::from_fn(|c| plane(c, (0, 0)));
    let smooth = |v: &[f32]| {
        let b = box_blur_par(v, bw, bh, blur);
        box_blur_par(&box_blur_par(&b, bw, bh, blur), bw, bh, blur)
    };
    // Heal measures local averages on the ring only (weights 0 where the spot is).
    let weight: Vec<f32> = cover.iter().map(|v| if *v > 0.0 { 0.0 } else { 1.0 }).collect();
    let nw = if spot.mode == SpotMode::Heal { smooth(&weight) } else { Vec::new() };
    for (c, dst) in out.iter_mut().enumerate() {
        let src = plane(c, s.off);
        let target: Vec<f32> = match spot.mode {
            SpotMode::Clone | SpotMode::Remove => src,
            SpotMode::Heal => {
                let dst_w: Vec<f32> = dst.iter().zip(&weight).map(|(v, k)| v * k).collect();
                let src_w: Vec<f32> = src.iter().zip(&weight).map(|(v, k)| v * k).collect();
                let (nd, ns) = (smooth(&dst_w), smooth(&src_w));
                (0..n)
                    .map(|k| {
                        if nw[k] < 1e-6 {
                            return src[k];
                        }
                        let (ld, ls) = (nd[k] / nw[k], ns[k] / nw[k]);
                        let ratio = (ld.max(1e-5) / ls.max(1e-5)).clamp(0.25, 4.0);
                        src[k] * ratio
                    })
                    .collect()
            }
        };
        for (k, o) in dst.iter_mut().enumerate() {
            let a = cover[k] * opacity;
            if a > 0.0 {
                *o += a * (target[k] - *o);
            }
        }
    }
    for k in 0..n {
        if cover[k] > 0.0 {
            let i = 3 * ((y0 + k / bw) * w + x0 + k % bw);
            px[i..i + 3].copy_from_slice(&[out[0][k], out[1][k], out[2][k]]);
        }
    }
}

/// A Remove spot: its fill picture, back in the photo's colours, blended in over the spot
/// (feathered, at its opacity). Without a current fill (still being made, or missing) the
/// photo shows through.
fn apply_fill(px: &mut [f32], w: usize, h: usize, spot: &Spot, f: Frame) {
    let Some(fill) = spot.current_fill() else { return };
    let Some(whole) = super::masks::fill_picture(&fill.id) else { return };
    let [l, t, r, b] = fill.rect;
    // Shown smaller than it is (a preview): from a copy halved until it is about the size
    // shown, so every pixel of the picture counts (its grain averages out as the photo's).
    let shown = (whole.width() as f32 / ((r - l) * f.w).max(1e-3))
        .min(whole.height() as f32 / ((b - t) * f.h).max(1e-3));
    let halvings = if shown >= 2.0 { (shown.log2().floor() as u32).min(6) } else { 0 };
    let Some(pic) = super::masks::fill_picture_halved(&fill.id, halvings) else { return };
    // What the model drew, per pixel shown.
    let drawn = if fill.full {
        1.0
    } else if fill.detail > 0.0 {
        fill.detail
    } else {
        (512.0 / whole.width().max(1) as f32).min(1.0)
    } * shown;
    let s = SpotPx::new(spot, f);
    let opacity = spot.opacity.clamp(0.0, 100.0) / 100.0;
    let (x0, y0, bw, bh) = s.bbox(1.0, w, h);
    let (pw, ph) = (pic.width() as usize, pic.height() as usize);
    if pw == 0 || ph == 0 {
        return;
    }
    let (sx, sy) = (pw as f32 / ((r - l) * f.w).max(1e-3), ph as f32 / ((b - t) * f.h).max(1e-3));
    // A fill made smaller than it is shown (a large spot) is smooth: the grain it lacks,
    // measured around the spot, goes back in, as fine or coarse as it is there and as
    // strong as it is at each lightness (grain is far weaker in the dark than in the light).
    let lost = (1.0 - drawn.powi(2)).max(0.0).sqrt();
    let grain = if drawn < 0.8 { surrounding_grain(px, w, h, &s) } else { None };
    // The grain's part that is the same in every channel (lightness), and each channel's own
    // (colour): as around the spot, so a black and white photo gets no coloured grain.
    let noise = grain.as_ref().map(|g| {
        let (same, own) = (g.shared.sqrt(), (1.0 - g.shared).sqrt());
        let shared = noise_plane(x0, y0, bw, bh, g.sigma, 1);
        let each: Option<[Vec<f32>; 3]> = (own > 0.05)
            .then(|| std::array::from_fn(|c| noise_plane(x0, y0, bw, bh, g.sigma, 10 + c as u32)));
        (same, own, shared, each)
    });
    let raw = pic.as_raw();
    for y in y0..y0 + bh {
        for x in x0..x0 + bw {
            let a = s.coverage(x, y) * opacity;
            if a <= 0.0 {
                continue;
            }
            // The picture's pixel here, bilinear.
            let fx = ((x as f32 + 0.5 - l * f.w) * sx - 0.5).clamp(0.0, (pw - 1) as f32);
            let fy = ((y as f32 + 0.5 - t * f.h) * sy - 0.5).clamp(0.0, (ph - 1) as f32);
            let (ix, iy) = (fx as usize, fy as usize);
            let (jx, jy) = ((ix + 1).min(pw - 1), (iy + 1).min(ph - 1));
            let (tx, ty) = (fx - ix as f32, fy - iy as f32);
            let v: [f32; 3] = std::array::from_fn(|c| {
                let at = |x: usize, y: usize| f32::from(raw[3 * (y * pw + x) + c]);
                let top = at(ix, iy) + tx * (at(jx, iy) - at(ix, iy));
                let bottom = at(ix, jy) + tx * (at(jx, jy) - at(ix, jy));
                top + ty * (bottom - top)
            });
            let mut target = fill_decode(v, fill.gains);
            if let (Some(g), Some((same, own, shared, each))) = (&grain, &noise) {
                let k = (y - y0) * bw + (x - x0);
                for (c, t) in target.iter_mut().enumerate() {
                    let n = same * shared[k] + each.as_ref().map_or(0.0, |e| own * e[c][k]);
                    *t = (*t + lost * g.spread(c, *t) * n).max(0.0);
                }
            }
            let i = 3 * (y * w + x);
            for c in 0..3 {
                px[i + c] += a * (target[c] - px[i + c]);
            }
        }
    }
}

/// Noise by position (the same every render), evenly spread over -0.5..0.5.
fn hash_noise(x: isize, y: isize, k: u32) -> f32 {
    let mut z = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ k;
    z ^= z >> 15;
    z = z.wrapping_mul(0x2C1B_3C6D);
    z ^= z >> 12;
    z = z.wrapping_mul(0x297A_2D39);
    z ^= z >> 15;
    (z as f32 / u32::MAX as f32) - 0.5
}

/// A box of noise with unit spread, about normal, the same every render: white, or (with
/// `sigma`, in pixels) smoothed into clumps that size, as film grain is in a scan.
pub(super) fn noise_plane(x0: usize, y0: usize, bw: usize, bh: usize, sigma: f32, k: u32) -> Vec<f32> {
    use rayon::prelude::*;
    let r = if sigma < 0.25 { 0 } else { (3.0 * sigma).ceil() as usize };
    let (pw, ph) = (bw + 2 * r, bh + 2 * r);
    let mut white = vec![0.0f32; pw * ph];
    white.par_chunks_mut(pw.max(1)).enumerate().for_each(|(y, row)| {
        let yy = (y0 + y) as isize - r as isize;
        for (x, v) in row.iter_mut().enumerate() {
            let xx = (x0 + x) as isize - r as isize;
            *v = (hash_noise(xx, yy, k)
                + hash_noise(xx, yy, k ^ 0x5bd1_e995)
                + hash_noise(xx, yy, k ^ 0x1b87_3593))
                * 2.0;
        }
    });
    if r == 0 {
        return white;
    }
    // Weights whose squares sum to 1 keep the spread at 1 through each pass.
    let mut kernel: Vec<f32> =
        (0..=2 * r).map(|i| (-((i as f32 - r as f32).powi(2)) / (2.0 * sigma * sigma)).exp()).collect();
    let norm = kernel.iter().map(|v| v * v).sum::<f32>().sqrt();
    kernel.iter_mut().for_each(|v| *v /= norm);
    let mut across = vec![0.0f32; bw * ph];
    across.par_chunks_mut(bw.max(1)).enumerate().for_each(|(y, row)| {
        let src = &white[y * pw..(y + 1) * pw];
        for (x, v) in row.iter_mut().enumerate() {
            *v = kernel.iter().zip(&src[x..]).map(|(k, s)| k * s).sum();
        }
    });
    let mut out = vec![0.0f32; bw * bh];
    out.par_chunks_mut(bw.max(1)).enumerate().for_each(|(y, row)| {
        for (i, k) in kernel.iter().enumerate() {
            let src = &across[(y + i) * bw..(y + i + 1) * bw];
            for (v, s) in row.iter_mut().zip(src) {
                *v += k * s;
            }
        }
    });
    out
}

/// Lightness steps the grain is measured in (by log2 of the level, from `GRAIN_LOW` up).
const GRAIN_BINS: usize = 10;
/// The darkest level told apart (2^-10): below it grain is taken as at this level.
const GRAIN_LOW: f32 = 1.0 / 1024.0;

/// The grain around a spot.
#[derive(Debug, Clone, PartialEq)]
struct Grain {
    /// Per lightness step and channel: the grain's spread as a share of the level.
    ratio: [[f32; 3]; GRAIN_BINS],
    /// How much of the grain the channels share (0..1 of its variance): 1 in a black and
    /// white photo, where the grain has no colour.
    shared: f32,
    /// How coarse it is: the blur (pixels) that turns white noise into grain like it.
    sigma: f32,
}

fn grain_step(level: f32) -> f32 {
    ((level.max(GRAIN_LOW).log2() + 10.0) / 10.5 * GRAIN_BINS as f32 - 0.5)
        .clamp(0.0, (GRAIN_BINS - 1) as f32)
}

impl Grain {
    /// The grain's spread in channel `c` where the picture is at `level`.
    fn spread(&self, c: usize, level: f32) -> f32 {
        let p = grain_step(level);
        let (i, t) = (p as usize, p.fract());
        let j = (i + 1).min(GRAIN_BINS - 1);
        (self.ratio[i][c] + t * (self.ratio[j][c] - self.ratio[i][c])) * level.max(GRAIN_LOW)
    }
}

/// What a pixel less the mean of its four neighbours `d` away has of the variance of noise
/// that is white noise blurred by `sigma`.
fn residual_share(sigma: f32, d: f32) -> f32 {
    if sigma < 1e-3 {
        return 1.25;
    }
    let rho = |r: f32| (-(r * r) / (4.0 * sigma * sigma)).exp();
    1.25 + 0.5 * rho(std::f32::consts::SQRT_2 * d) + 0.25 * rho(2.0 * d) - 2.0 * rho(d)
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, f32::total_cmp).1
}

/// How grainy the photo is just around a spot: how far each pixel is from its four
/// neighbours' mean, in a ring outside the spot. Measured as a share of the level and per
/// lightness step (grain in the light must not go into a fill in the dark), by medians
/// (so the picture's own edges don't count as grain), with how coarse it is and how much
/// of it the channels share. `None` with too little to measure.
fn surrounding_grain(px: &[f32], w: usize, h: usize, s: &SpotPx) -> Option<Grain> {
    let (x0, y0, bw, bh) = s.bbox((0.5 * s.r).max(8.0), w, h);
    let step = ((bw * bh) / 40_000).max(1);
    let mut found: [[Vec<f32>; 3]; GRAIN_BINS] = Default::default();
    let (mut near, mut far) = (Vec::new(), Vec::new());
    let (mut sum, mut cross) = ([0.0f64; 3], [0.0f64; 3]);
    for k in (0..bw * bh).step_by(step) {
        let (x, y) = (x0 + k % bw, y0 + k / bw);
        if x < 2 || y < 2 || x + 2 >= w || y + 2 >= h || s.coverage(x, y) > 0.0 {
            continue;
        }
        let i = 3 * (y * w + x);
        // Relative to the level around, at one pixel away and at two.
        let at = |c: usize, d: usize| {
            let around = 0.25
                * (px[i - 3 * d + c] + px[i + 3 * d + c] + px[i - 3 * d * w + c] + px[i + 3 * d * w + c]);
            ((px[i + c] - around) / around.max(GRAIN_LOW), around)
        };
        let d: [(f32, f32); 3] = std::array::from_fn(|c| at(c, 1));
        for c in 0..3 {
            found[grain_step(d[c].1).round() as usize][c].push(d[c].0.abs());
            sum[c] += f64::from(d[c].0 * d[c].0);
            cross[c] += f64::from(d[c].0 * d[(c + 1) % 3].0);
        }
        near.push(d[1].0.abs());
        far.push(at(1, 2).0.abs());
    }
    if near.len() < 50 {
        return None;
    }
    // How coarse: smooth grain differs more from neighbours two away than from those next
    // to it; white noise differs the same.
    let q = (median(&mut far) / median(&mut near).max(1e-9)).powi(2);
    let sigma = (0..=40)
        .map(|i| i as f32 * 0.05)
        .find(|s| residual_share(*s, 2.0) / residual_share(*s, 1.0) >= q)
        .unwrap_or(2.0);
    let sigma = if sigma < 0.4 { 0.0 } else { sigma };
    // A median of distances to a spread (1.4826), and the residual's share of the noise.
    let scale = 1.4826 / residual_share(sigma, 1.0).sqrt();
    let mut ratio = [[0.0f32; 3]; GRAIN_BINS];
    let mut known = [false; GRAIN_BINS];
    // A step with few pixels holds the odd ones of its neighbours, not their grain.
    let few = (near.len() / 20).max(30);
    for (b, bin) in found.iter_mut().enumerate() {
        if bin[0].len() >= few {
            known[b] = true;
            for c in 0..3 {
                ratio[b][c] = median(&mut bin[c]) * scale;
            }
        }
    }
    if !known.contains(&true) {
        return None;
    }
    // Steps with too little around the spot: as the nearest measured one.
    for b in 0..GRAIN_BINS {
        if !known[b] {
            let nearest = (0..GRAIN_BINS).filter(|o| known[*o]).min_by_key(|o| o.abs_diff(b)).unwrap_or(b);
            ratio[b] = ratio[nearest];
        }
    }
    // How alike the channels' grain is: their mean correlation (1: the same grain in each).
    let pairs: Vec<f64> = (0..3)
        .filter_map(|c| {
            let both = (sum[c] * sum[(c + 1) % 3]).sqrt();
            (both > 0.0).then(|| cross[c] / both)
        })
        .collect();
    let shared =
        if pairs.is_empty() { 1.0 } else { (pairs.iter().sum::<f64>() / pairs.len() as f64).clamp(0.0, 1.0) };
    Some(Grain { ratio, shared: shared as f32, sigma })
}

/// The healed copies cached with the image: with all the spots last asked for (`top`), and
/// with the first of them only, up to the spot being worked on (`under`).
#[derive(Debug, Default)]
pub(super) struct HealCache(Mutex<HealKept>);

#[derive(Debug, Default)]
struct HealKept {
    top: Option<HealKey>,
    under: Option<HealKey>,
}

/// Images above this many pixels keep no `under` copy (a full-size photo is hundreds of MB).
const UNDER_MAX_PIXELS: usize = 4_000_000;

/// The spots and the dust mask (its id, and what was painted out of it) a cached healed
/// copy was made with.
type HealKey = ((Vec<Spot>, Option<(String, Vec<SpotStroke>)>), Arc<WorkingImage>);

impl Clone for HealCache {
    /// A copy starts without the cache (it is only a cache).
    fn clone(&self) -> Self {
        HealCache::default()
    }
}

/// `img` with `spots` removed (the same image when there are none).
pub(super) fn healed<'a>(
    img: &'a WorkingImage,
    spots: &[Spot],
    dust: Option<&super::dust::DustLayer>,
) -> super::transform::Geometry<'a> {
    use super::transform::Geometry;
    // Dust & Scratches: its mask, if on and made for these settings.
    let dust_id = dust.and_then(|d| Some((d.current()?.to_owned(), d.erase.clone())));
    let (w, h) = (img.width() as usize, img.height() as usize);
    if spots.is_empty() && dust_id.is_none() {
        return Geometry::Same(img);
    }
    let mut kept = img.heal_cache.0.lock().unwrap_or_else(|e| e.into_inner());
    let is = |k: &HealKey, s: &[Spot]| k.0.0 == s && k.0.1 == dust_id;
    if let Some(k) = kept.top.as_ref().filter(|k| is(k, spots)) {
        return Geometry::Transformed(k.1.clone());
    }
    // One spot fewer (an undo, a deleted last spot): the copy under it.
    if let Some(k) = kept.under.clone().filter(|k| is(k, spots)) {
        kept.top = Some(k.clone());
        return Geometry::Transformed(k.1);
    }
    // Spots apply in order, so a copy made with the first few of these is where to go on
    // from. `split`: how many spots come before the one being worked on (the first that
    // differs from last time, or the last one).
    let common = kept
        .top
        .as_ref()
        .filter(|k| k.0.1 == dust_id)
        .map_or(0, |k| k.0.0.iter().zip(spots).take_while(|(a, b)| a == b).count());
    let split = common.min(spots.len().saturating_sub(1));
    let base = [kept.top.as_ref(), kept.under.as_ref()]
        .into_iter()
        .flatten()
        .filter(|k| k.0.0.len() <= split && is(k, &spots[..k.0.0.len()]))
        .max_by_key(|k| k.0.0.len())
        .cloned();
    let f = Frame::of(img);
    let copy = |px: Vec<f32>| Arc::new(img.with_pixels(px));
    let (done, mut px) = match &base {
        Some(k) => (k.0.0.len(), k.1.pixels.as_raw().clone()),
        None => {
            let mut px = img.pixels.as_raw().clone();
            // Film dust first (the spots go over it).
            if let Some(mask) = dust.and_then(|d| d.mask_at_size(w, h)) {
                super::dust::fill_masked(&mut px, w, h, &mask);
            }
            (0, px)
        }
    };
    if w * h <= UNDER_MAX_PIXELS {
        // The copy under the spot being worked on: the base itself, or made from it.
        let under = match base {
            Some(k) if done == split => k,
            _ => {
                for spot in &spots[done..split] {
                    apply_spot(&mut px, w, h, spot, f);
                }
                ((spots[..split].to_vec(), dust_id.clone()), copy(px.clone()))
            }
        };
        kept.under = Some(under);
    } else {
        for spot in &spots[done..split] {
            apply_spot(&mut px, w, h, spot, f);
        }
        kept.under = None;
    }
    for spot in &spots[split..] {
        apply_spot(&mut px, w, h, spot, f);
    }
    let out = copy(px);
    kept.top = Some(((spots.to_vec(), dust_id), out.clone()));
    Geometry::Transformed(out)
}

/// A good place to take `spot`'s pixels from: nearby, inside the photo, where the
/// surroundings look most like the spot's own surroundings. `avoid`: an offset not to
/// pick again (the `/` key: another source). Returns the offset (0..1 of the photo).
pub fn find_source(img: &WorkingImage, spot: &Spot, avoid: Option<[f32; 2]>) -> [f32; 2] {
    let f = Frame::of(img);
    let s = SpotPx::new(spot, f);
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = img.pixels.as_raw();
    // The ring just outside the spot (its whole area), sampled sparsely.
    let stride = ((s.r / 6.0) as usize).max(1);
    let (x0, y0, bw, bh) = s.bbox(0.7 * s.r, w, h);
    // How far a source must be: by the whole spot's size (a long stroke's source must clear
    // all of it), as a radius.
    let [bl, bt, br, bb] = s.bounds();
    let extent = ((br - bl).max(bb - bt) / 2.0).max(s.r);
    let ring: Vec<(usize, usize)> = (y0..y0 + bh)
        .step_by(stride)
        .flat_map(|y| (x0..x0 + bw).step_by(stride).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let e = s.edge(x as f32 + 0.5, y as f32 + 0.5);
            e <= 0.0 && e > -0.7 * s.r
        })
        .collect();
    let lum = |x: usize, y: usize| {
        let i = 3 * (y * w + x);
        (px[i] + px[i + 1] + px[i + 2]).max(0.0).sqrt()
    };
    // The spot's own extent, so a source keeps all of it inside the photo.
    let (sx0, sy0, sbw, sbh) = s.bbox(0.0, w, h);
    let avoid_px = avoid.map(|a| (a[0] * f.w, a[1] * f.h));
    let mut best: Option<(f32, (f32, f32))> = None;
    for dist in [2.3f32, 3.2, 4.5] {
        for k in 0..24 {
            let a = k as f32 / 24.0 * std::f32::consts::TAU;
            let (ox, oy) = (a.cos() * dist * extent, a.sin() * dist * extent);
            let (ix, iy) = (ox.round() as isize, oy.round() as isize);
            let inside = |x: usize, y: usize| {
                let (x, y) = (x as isize + ix, y as isize + iy);
                x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h
            };
            if !inside(sx0, sy0) || !inside(sx0 + sbw.saturating_sub(1), sy0 + sbh.saturating_sub(1)) {
                continue;
            }
            if let Some((ax, ay)) = avoid_px
                && (ox - ax).hypot(oy - ay) < extent
            {
                continue;
            }
            let (mut err, mut n) = (0.0f32, 0usize);
            for &(x, y) in &ring {
                if inside(x, y) {
                    let d = lum(x, y) - lum((x as isize + ix) as usize, (y as isize + iy) as usize);
                    err += d * d;
                    n += 1;
                }
            }
            if n < ring.len() / 2 {
                continue;
            }
            // Nearer is better when the surroundings match about as well.
            let score = err / n.max(1) as f32 * (1.0 + 0.05 * dist);
            if best.is_none_or(|(b, _)| score < b) {
                best = Some((score, (ox, oy)));
            }
        }
    }
    let (ox, oy) = best.map_or((2.5 * extent, 0.0), |(_, o)| o);
    [ox / f.w, oy / f.h]
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

    #[test]
    fn grain_around_a_spot_knows_whether_it_has_colour() {
        // Noise by position: the same in every channel (black and white film), or each
        // channel its own.
        let noise = |x: usize, y: usize, k: u32| {
            let mut z = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ k;
            z ^= z >> 15;
            z = z.wrapping_mul(0x2C1B_3C6D);
            z ^= z >> 12;
            (z % 1000) as f32 / 1000.0 - 0.5
        };
        let (w, h) = (200usize, 100usize);
        let spot = Spot { points: vec![[0.5, 0.5]], size: 0.1, ..Spot::default() };
        let s = SpotPx::new(&spot, Frame { w: w as f32, h: h as f32, half: 100.0 });
        let photo = |own: bool| -> Vec<f32> {
            (0..w * h)
                .flat_map(|i| {
                    let (x, y) = (i % w, i / w);
                    [0u32, 1, 2].map(|c| 0.4 + 0.1 * noise(x, y, if own { c } else { 0 }))
                })
                .collect()
        };
        let g = surrounding_grain(&photo(false), w, h, &s).unwrap();
        assert!(g.shared > 0.98, "black and white: the channels share their grain ({})", g.shared);
        assert_eq!(g.sigma, 0.0, "white noise is not coarse");
        let spread = g.spread(0, 0.4);
        assert!((spread - 0.0289).abs() < 0.006, "0.1 of even noise has a spread of 0.029: {spread}");
        assert!((g.spread(1, 0.4) - spread).abs() < 1e-6);
        let g = surrounding_grain(&photo(true), w, h, &s).unwrap();
        assert!(g.shared < 0.1, "each channel its own grain ({})", g.shared);
    }

    #[test]
    fn grain_is_measured_per_lightness_and_as_coarse_as_it_is() {
        // Coarse grain (noise blurred by 1.2 px), a tenth of the level: light on the left
        // half, dark on the right.
        let (w, h) = (400usize, 200usize);
        let plane = noise_plane(0, 0, w, h, 1.2, 7);
        let mean = plane.iter().sum::<f32>() / plane.len() as f32;
        let var = plane.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / plane.len() as f32;
        assert!((var - 1.0).abs() < 0.15, "the noise has unit spread ({var})");
        let px: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let level = if i % w < w / 2 { 0.5 } else { 0.02 };
                [level * (1.0 + 0.1 * plane[i]); 3]
            })
            .collect();
        let spot = Spot { points: vec![[0.5, 0.5]], size: 0.1, ..Spot::default() };
        let s = SpotPx::new(&spot, Frame { w: w as f32, h: h as f32, half: 200.0 });
        let g = surrounding_grain(&px, w, h, &s).unwrap();
        assert!((g.sigma - 1.2).abs() < 0.4, "how coarse: {}", g.sigma);
        let (light, dark) = (g.spread(0, 0.5), g.spread(0, 0.02));
        assert!((light - 0.05).abs() < 0.02, "light: {light}");
        assert!((dark - 0.002).abs() < 0.001, "dark: {dark}");
    }

    fn image(f: impl Fn(u32, u32) -> f32) -> WorkingImage {
        WorkingImage::for_tests(ImageBuffer::from_fn(200, 100, |x, y| {
            let v = f(x, y);
            Rgb([v, v, v])
        }))
    }

    #[test]
    fn clone_copies_and_heal_matches_the_surroundings() {
        // A grey gradient with a dark blemish at (60, 50).
        let img = image(|x, y| {
            let base = 0.2 + x as f32 / 400.0;
            if (x as f32 - 60.0).hypot(y as f32 - 50.0) < 5.0 { 0.02 } else { base }
        });
        let at = |i: &WorkingImage, x: u32, y: u32| i.pixels.get_pixel(x, y)[0];
        let spot = |mode| Spot {
            mode,
            points: vec![[0.3, 0.5]],
            size: 0.1, // 10 px on a 200 px wide photo
            feather: 20.0,
            opacity: 100.0,
            offset: [0.25, 0.0], // 50 px to the right: brighter there
            fill: None,
            strokes: vec![],
            found: false,
            area: None,
        };
        // Clone: the source's value (from x = 110).
        let cloned = healed(&img, &[spot(SpotMode::Clone)], None);
        assert!((at(&cloned, 60, 50) - at(&img, 110, 50)).abs() < 1e-4);
        // Heal: the blemish gone, at the destination's own brightness (not the source's).
        let healed_img = healed(&img, &[spot(SpotMode::Heal)], None);
        let v = at(&healed_img, 60, 50);
        assert!((v - at(&img, 60, 40)).abs() < 0.02, "{v} vs {}", at(&img, 60, 40));
        // Away from the spot nothing changes.
        assert_eq!(at(&healed_img, 150, 20), at(&img, 150, 20));
        // Cached: the same image again.
        let again = healed(&img, &[spot(SpotMode::Heal)], None);
        assert_eq!(at(&again, 60, 50), v);
    }

    #[test]
    fn brushed_spots_cover_their_path() {
        let img = image(|_, y| if (48..=52).contains(&y) { 0.0 } else { 0.5 });
        // A dark line across; brush along part of it.
        let spot = Spot {
            mode: SpotMode::Heal,
            points: vec![[0.2, 0.5], [0.6, 0.5]],
            size: 0.06,
            feather: 30.0,
            opacity: 100.0,
            offset: [0.0, 0.25],
            fill: None,
            strokes: vec![],
            found: false,
            area: None,
        };
        let out = healed(&img, &[spot], None);
        let at = |x, y| out.pixels.get_pixel(x, y)[0];
        assert!(at(80, 50) > 0.4, "healed along the path: {}", at(80, 50));
        assert!(at(180, 50) < 0.05, "untouched past it");
    }

    #[test]
    fn sources_are_found_where_the_surroundings_match() {
        // Bright above, dark below; a spot just above the edge should take from the side,
        // along the edge, not from across it.
        let img = image(|_, y| if y < 50 { 0.8 } else { 0.1 });
        let spot = Spot { points: vec![[0.5, 0.4]], size: 0.06, ..Spot::default() };
        let off = find_source(&img, &spot, None);
        assert!(off[1].abs() < 0.08, "stays at the same height: {off:?}");
        assert!(off[0].abs() > 0.05, "moves sideways: {off:?}");
        // Asking again gives another one.
        let other = find_source(&img, &spot, Some(off));
        assert_ne!(other, off);
    }
}
