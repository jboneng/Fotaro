//! Masking (LrC's Masks): local adjustments confined to brushed, linear-gradient or
//! radial-gradient areas.
//!
//! A [`Mask`] has components combined in order (the first adds; later ones add, subtract
//! or intersect, each possibly inverted) and its own local sliders. Shapes are stored in
//! the untransformed photo's coordinates (0..1 across and down; sizes in half long sides),
//! so they stay on the same content through Transform and Constrain Crop.
//!
//! For a render, [`MaskField`] holds every active mask's value per output pixel; the
//! pipeline turns them into local adjustments (see `pipeline.rs`).

use std::sync::{Arc, Mutex};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::pipeline::Region;
use super::source::WorkingImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CombineMode {
    #[default]
    Add,
    Subtract,
    Intersect,
}

impl CombineMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "Add",
            Self::Subtract => "Subtract",
            Self::Intersect => "Intersect",
        }
    }
}

/// One brush stroke: a path painted (or erased) with round dabs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Stroke {
    /// The path, in 0..1 of the untransformed photo.
    pub points: Vec<[f32; 2]>,
    /// Radius in half long sides of the photo.
    pub size: f32,
    /// Feather, flow and density, 0..100.
    pub feather: f32,
    pub flow: f32,
    pub density: f32,
    pub erase: bool,
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke { points: Vec::new(), size: 0.05, feather: 50.0, flow: 100.0, density: 100.0, erase: false }
    }
}

/// A component's shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Shape {
    /// Full effect at `start`, none at `end` (0..1 of the photo), a smooth ramp between.
    Linear {
        start: [f32; 2],
        end: [f32; 2],
    },
    /// An ellipse: centre (0..1), radii in half long sides, rotation in degrees, and how
    /// much of the radius fades out (0..100).
    Radial {
        center: [f32; 2],
        radii: [f32; 2],
        angle: f32,
        feather: f32,
    },
    Brush {
        strokes: Vec<Stroke>,
    },
    /// A mask made by a model (Select Subject, Select Sky): its picture is kept in the
    /// mask store (see [`set_mask_store`]) as `<id>.png`, covering the untransformed
    /// photo; `selects` is what it selects and `model` what made it, so it can be made again.
    Raster {
        id: String,
        selects: String,
        model: String,
    },
}

impl Shape {
    pub fn label(&self) -> &'static str {
        match self {
            Shape::Linear { .. } => "Linear Gradient",
            Shape::Radial { .. } => "Radial Gradient",
            Shape::Brush { .. } => "Brush",
            Shape::Raster { selects, .. } => match selects.as_str() {
                "Subject" => "Subject",
                "Sky" => "Sky",
                "Background" => "Background",
                "Object" => "Object",
                _ => "AI Mask",
            },
        }
    }
}

/// Where AI masks' pictures are kept (the catalog's folder), and those already read.
static MASK_STORE: std::sync::RwLock<Option<std::path::PathBuf>> = std::sync::RwLock::new(None);
static MASK_PICTURES: Mutex<Vec<(String, Arc<image::GrayImage>)>> = Mutex::new(Vec::new());

/// Set the folder AI masks' pictures are read from and saved to (the open catalog's).
pub fn set_mask_store(dir: Option<std::path::PathBuf>) {
    *MASK_STORE.write().unwrap_or_else(|e| e.into_inner()) = dir;
    MASK_PICTURES.lock().unwrap_or_else(|e| e.into_inner()).clear();
    FILL_PICTURES.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Save an AI mask's picture as `<id>.png` in the mask store.
pub fn save_mask_picture(id: &str, picture: &image::GrayImage) -> anyhow::Result<()> {
    let dir = MASK_STORE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no catalog to keep the mask in"))?;
    std::fs::create_dir_all(&dir)?;
    // Fast compression: full-size masks (Dust & Scratches) are tens of megapixels, and
    // mostly one value, which compresses well anyway.
    let file = std::io::BufWriter::new(std::fs::File::create(dir.join(format!("{id}.png")))?);
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        file,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    );
    picture.write_with_encoder(encoder)?;
    MASK_PICTURES.lock().unwrap_or_else(|e| e.into_inner()).push((id.to_owned(), Arc::new(picture.clone())));
    Ok(())
}

/// An AI mask's picture, from memory or the mask store; `None` if it is missing.
pub fn mask_picture(id: &str) -> Option<Arc<image::GrayImage>> {
    let mut cache = MASK_PICTURES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, p)) = cache.iter().find(|(k, _)| k == id) {
        return Some(p.clone());
    }
    let dir = MASK_STORE.read().unwrap_or_else(|e| e.into_inner()).clone()?;
    match image::open(dir.join(format!("{id}.png"))) {
        Ok(img) => {
            let p = Arc::new(img.to_luma8());
            cache.push((id.to_owned(), p.clone()));
            // A few at a time: they are ~1 MB each.
            if cache.len() > 16 {
                cache.remove(0);
            }
            Some(p)
        }
        Err(e) => {
            log::warn!("AI mask {id}: {e}");
            None
        }
    }
}

/// Remove fills' pictures already read (colour; in the same store as the masks').
static FILL_PICTURES: Mutex<Vec<(String, Arc<image::RgbImage>)>> = Mutex::new(Vec::new());

/// Save a Remove fill's picture as `<id>.png` in the mask store.
pub fn save_fill_picture(id: &str, picture: &image::RgbImage) -> anyhow::Result<()> {
    let dir = MASK_STORE
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no catalog to keep the fill in"))?;
    std::fs::create_dir_all(&dir)?;
    picture.save(dir.join(format!("{id}.png")))?;
    FILL_PICTURES.lock().unwrap_or_else(|e| e.into_inner()).push((id.to_owned(), Arc::new(picture.clone())));
    Ok(())
}

/// A Remove fill's picture, from memory or the store; `None` if it is missing.
pub(super) fn fill_picture(id: &str) -> Option<Arc<image::RgbImage>> {
    let mut cache = FILL_PICTURES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, p)) = cache.iter().find(|(k, _)| k == id) {
        return Some(p.clone());
    }
    let dir = MASK_STORE.read().unwrap_or_else(|e| e.into_inner()).clone()?;
    match image::open(dir.join(format!("{id}.png"))) {
        Ok(img) => {
            let p = Arc::new(img.to_rgb8());
            cache.push((id.to_owned(), p.clone()));
            if cache.len() > 32 {
                cache.remove(0);
            }
            Some(p)
        }
        Err(e) => {
            log::warn!("Remove fill {id}: {e}");
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Component {
    #[serde(default)]
    pub mode: CombineMode,
    #[serde(default)]
    pub invert: bool,
    pub shape: Shape,
}

/// A mask's local sliders (LrC's), all 0 by default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalAdjust {
    pub temp: f32,
    pub tint: f32,
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    pub hue: f32,
    pub saturation: f32,
    pub sharpness: f32,
    pub noise: f32,
    /// A colour tint: hue in degrees, saturation 0..100.
    pub color: Option<[f32; 2]>,
}

/// A local slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalParam {
    Temp,
    Tint,
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Texture,
    Clarity,
    Dehaze,
    Hue,
    Saturation,
    Sharpness,
    Noise,
}

impl LocalParam {
    /// LrC's groups, in order.
    pub const GROUPS: [&'static [LocalParam]; 5] = [
        &[Self::Temp, Self::Tint],
        &[Self::Exposure, Self::Contrast, Self::Highlights, Self::Shadows, Self::Whites, Self::Blacks],
        &[Self::Texture, Self::Clarity, Self::Dehaze],
        &[Self::Hue, Self::Saturation],
        &[Self::Sharpness, Self::Noise],
    ];

    pub fn all() -> impl Iterator<Item = LocalParam> {
        Self::GROUPS.into_iter().flatten().copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Temp => "Temp",
            Self::Tint => "Tint",
            Self::Exposure => "Exposure",
            Self::Contrast => "Contrast",
            Self::Highlights => "Highlights",
            Self::Shadows => "Shadows",
            Self::Whites => "Whites",
            Self::Blacks => "Blacks",
            Self::Texture => "Texture",
            Self::Clarity => "Clarity",
            Self::Dehaze => "Dehaze",
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Sharpness => "Sharpness",
            Self::Noise => "Noise",
        }
    }

    pub fn range(self) -> (f32, f32) {
        match self {
            Self::Exposure => (-4.0, 4.0),
            Self::Hue => (-180.0, 180.0),
            _ => (-100.0, 100.0),
        }
    }

    pub fn decimals(self) -> usize {
        match self {
            Self::Exposure => 2,
            Self::Hue => 1,
            _ => 0,
        }
    }

    pub fn get(self, a: &LocalAdjust) -> f32 {
        *match self {
            Self::Temp => &a.temp,
            Self::Tint => &a.tint,
            Self::Exposure => &a.exposure,
            Self::Contrast => &a.contrast,
            Self::Highlights => &a.highlights,
            Self::Shadows => &a.shadows,
            Self::Whites => &a.whites,
            Self::Blacks => &a.blacks,
            Self::Texture => &a.texture,
            Self::Clarity => &a.clarity,
            Self::Dehaze => &a.dehaze,
            Self::Hue => &a.hue,
            Self::Saturation => &a.saturation,
            Self::Sharpness => &a.sharpness,
            Self::Noise => &a.noise,
        }
    }

    pub fn get_mut(self, a: &mut LocalAdjust) -> &mut f32 {
        match self {
            Self::Temp => &mut a.temp,
            Self::Tint => &mut a.tint,
            Self::Exposure => &mut a.exposure,
            Self::Contrast => &mut a.contrast,
            Self::Highlights => &mut a.highlights,
            Self::Shadows => &mut a.shadows,
            Self::Whites => &mut a.whites,
            Self::Blacks => &mut a.blacks,
            Self::Texture => &mut a.texture,
            Self::Clarity => &mut a.clarity,
            Self::Dehaze => &mut a.dehaze,
            Self::Hue => &mut a.hue,
            Self::Saturation => &mut a.saturation,
            Self::Sharpness => &mut a.sharpness,
            Self::Noise => &mut a.noise,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mask {
    pub name: String,
    pub visible: bool,
    /// How much of the mask's adjustments apply, 0..100.
    pub amount: f32,
    pub components: Vec<Component>,
    pub adjust: LocalAdjust,
}

impl Default for Mask {
    fn default() -> Self {
        Mask {
            name: "Mask".into(),
            visible: true,
            amount: 100.0,
            components: Vec::new(),
            adjust: LocalAdjust::default(),
        }
    }
}

impl Mask {
    /// A new mask with one component.
    pub fn new(name: impl Into<String>, shape: Shape) -> Mask {
        Mask {
            name: name.into(),
            components: vec![Component { mode: CombineMode::Add, invert: false, shape }],
            ..Mask::default()
        }
    }

    /// Does it change anything?
    pub fn is_active(&self) -> bool {
        self.visible
            && self.amount > 0.0
            && !self.components.is_empty()
            && self.adjust != LocalAdjust::default()
    }
}

/// The photo's frame, for turning stored coordinates into source pixels.
#[derive(Debug, Clone, Copy)]
struct Frame {
    w: f32,
    h: f32,
    half: f32,
}

impl Frame {
    fn new(w: u32, h: u32) -> Frame {
        let (w, h) = (w.max(1) as f32, h.max(1) as f32);
        Frame { w, h, half: w.max(h) / 2.0 }
    }

    /// 0..1 of the photo → pixels.
    fn px(&self, p: [f32; 2]) -> [f32; 2] {
        [p[0] * self.w, p[1] * self.h]
    }
}

#[inline]
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A component ready to evaluate at source pixels.
enum Evaluator {
    Linear { start: [f32; 2], dir: [f32; 2], inv_len2: f32 },
    Radial { center: [f32; 2], cos: f32, sin: f32, inv_rx: f32, inv_ry: f32, inner: f32 },
    Brush(Arc<Raster>),
}

impl Evaluator {
    #[inline]
    fn value(&self, u: f32, v: f32) -> f32 {
        match self {
            Evaluator::Linear { start, dir, inv_len2 } => {
                let t = ((u - start[0]) * dir[0] + (v - start[1]) * dir[1]) * inv_len2;
                1.0 - smoothstep(0.0, 1.0, t)
            }
            Evaluator::Radial { center, cos, sin, inv_rx, inv_ry, inner } => {
                let (dx, dy) = (u - center[0], v - center[1]);
                let (x, y) = ((dx * cos + dy * sin) * inv_rx, (-dx * sin + dy * cos) * inv_ry);
                let d = (x * x + y * y).sqrt();
                if *inner >= 1.0 {
                    // No feather: a hard edge with a pixel of anti-aliasing (a pixel is
                    // about 1/radius in these units).
                    let pixel = inv_rx.max(*inv_ry);
                    ((1.0 - d) / pixel + 0.5).clamp(0.0, 1.0)
                } else {
                    1.0 - smoothstep(*inner, 1.0, d)
                }
            }
            Evaluator::Brush(r) => r.sample(u, v),
        }
    }
}

/// A mask ready to evaluate: its components in order and its strength.
struct Prepared {
    parts: Vec<(CombineMode, bool, Evaluator)>,
    amount: f32,
}

impl Prepared {
    #[inline]
    fn value(&self, u: f32, v: f32) -> f32 {
        let mut out = 0.0f32;
        for (i, (mode, invert, e)) in self.parts.iter().enumerate() {
            let c = e.value(u, v);
            let c = if *invert { 1.0 - c } else { c };
            out = if i == 0 {
                c
            } else {
                match mode {
                    CombineMode::Add => out.max(c),
                    CombineMode::Subtract => out * (1.0 - c),
                    CombineMode::Intersect => out * c,
                }
            };
        }
        out * self.amount
    }
}

/// Brush paint in source pixels over a box of the photo.
#[derive(Debug)]
pub(super) struct Raster {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    data: Vec<f32>,
}

impl Raster {
    /// Bilinear sample at source pixel (u, v); 0 outside.
    #[inline]
    fn sample(&self, u: f32, v: f32) -> f32 {
        let (fx, fy) = (u - 0.5 - self.x0 as f32, v - 0.5 - self.y0 as f32);
        if fx < -1.0 || fy < -1.0 || fx > self.w as f32 || fy > self.h as f32 {
            return 0.0;
        }
        let at = |x: isize, y: isize| {
            if x < 0 || y < 0 || x >= self.w as isize || y >= self.h as isize {
                0.0
            } else {
                self.data[y as usize * self.w + x as usize]
            }
        };
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x0, y0) = (x0 as isize, y0 as isize);
        let top = at(x0, y0) + tx * (at(x0 + 1, y0) - at(x0, y0));
        let bottom = at(x0, y0 + 1) + tx * (at(x0 + 1, y0 + 1) - at(x0, y0 + 1));
        top + ty * (bottom - top)
    }
}

/// One dab of a stroke, in source pixels.
#[derive(Debug, Clone, Copy)]
struct Dab {
    x: f32,
    y: f32,
    r: f32,
    r_in: f32,
    flow: f32,
    density: f32,
    erase: bool,
}

fn dabs(strokes: &[Stroke], frame: Frame) -> Vec<Dab> {
    let mut out = Vec::new();
    for s in strokes {
        let r = (s.size * frame.half).max(0.5);
        let r_in = (r * (1.0 - s.feather.clamp(0.0, 100.0) / 100.0)).min(r - 0.75).max(0.0);
        let dab = |p: [f32; 2]| Dab {
            x: p[0],
            y: p[1],
            r,
            r_in,
            flow: s.flow.clamp(0.0, 100.0) / 100.0,
            density: s.density.clamp(0.0, 100.0) / 100.0,
            erase: s.erase,
        };
        let spacing = (r / 4.0).max(0.5);
        let mut points = s.points.iter().map(|p| frame.px(*p));
        let Some(first) = points.next() else { continue };
        out.push(dab(first));
        // Dabs every `spacing` along the path, carrying the remainder across corners.
        let (mut prev, mut carry) = (first, 0.0f32);
        for p in points {
            let (dx, dy) = (p[0] - prev[0], p[1] - prev[1]);
            let len = dx.hypot(dy);
            let mut t = spacing - carry;
            while t <= len {
                out.push(dab([prev[0] + dx * t / len, prev[1] + dy * t / len]));
                t += spacing;
            }
            carry = len - (t - spacing);
            prev = p;
        }
    }
    out
}

/// Paint `strokes` over the box (x0, y0, w, h) of a photo framed by `frame`.
fn rasterize(strokes: &[Stroke], frame: Frame, bbox: (usize, usize, usize, usize)) -> Raster {
    let (x0, y0, w, h) = bbox;
    let dabs = dabs(strokes, frame);
    let mut data = vec![0.0f32; w * h];
    // Bands of rows in parallel; within a band every dab in order (paint and erase don't
    // commute).
    const BAND: usize = 16;
    data.par_chunks_mut(BAND * w.max(1)).enumerate().for_each(|(b, band)| {
        let by0 = y0 + b * BAND;
        let by1 = by0 + band.len() / w.max(1);
        for d in &dabs {
            let (ty0, ty1) =
                ((d.y - d.r).floor().max(by0 as f32) as usize, ((d.y + d.r).ceil() as usize).min(by1));
            let (tx0, tx1) =
                ((d.x - d.r).floor().max(x0 as f32) as usize, ((d.x + d.r).ceil() as usize).min(x0 + w));
            if ty0 >= ty1 || tx0 >= tx1 {
                continue;
            }
            for y in ty0..ty1 {
                let row = &mut band[(y - by0) * w..(y - by0 + 1) * w];
                let dy = y as f32 + 0.5 - d.y;
                for x in tx0..tx1 {
                    let dx = x as f32 + 0.5 - d.x;
                    let dist = (dx * dx + dy * dy).sqrt();
                    if dist >= d.r {
                        continue;
                    }
                    let f = 1.0 - smoothstep(d.r_in, d.r, dist);
                    let a = d.flow * f;
                    let v = &mut row[x - x0];
                    if d.erase {
                        *v *= 1.0 - a;
                    } else if *v < d.density {
                        *v += a * (d.density - *v);
                    }
                }
            }
        }
    });
    Raster { x0, y0, w, h, data }
}

/// Brush rasters kept with the source image, so slider drags reuse them.
#[derive(Debug, Default)]
pub(super) struct MaskCache(Mutex<Vec<(u64, Arc<Raster>)>>);

impl Clone for MaskCache {
    /// A copy starts empty (it is only a cache).
    fn clone(&self) -> Self {
        MaskCache::default()
    }
}

fn hash_strokes(strokes: &[Stroke], extra: [usize; 6]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |v: u64| {
        h = (h ^ v).wrapping_mul(0x0000_0100_0000_01b3);
    };
    for s in strokes {
        for p in &s.points {
            eat(u64::from(p[0].to_bits()));
            eat(u64::from(p[1].to_bits()));
        }
        for v in [s.size, s.feather, s.flow, s.density] {
            eat(u64::from(v.to_bits()));
        }
        eat(u64::from(s.erase));
        eat(0xffff_ffff);
    }
    for v in extra {
        eat(v as u64);
    }
    h
}

fn brush_raster(
    source: &WorkingImage,
    strokes: &[Stroke],
    bbox: (usize, usize, usize, usize),
) -> Arc<Raster> {
    let key = hash_strokes(
        strokes,
        [bbox.0, bbox.1, bbox.2, bbox.3, source.width() as usize, source.height() as usize],
    );
    let mut cache = source.mask_cache.0.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, r)) = cache.iter().find(|(k, _)| *k == key) {
        return r.clone();
    }
    let r = Arc::new(rasterize(strokes, Frame::new(source.width(), source.height()), bbox));
    cache.push((key, r.clone()));
    // A few rasters: the proxy, the 1:1 view and the Detail box.
    if cache.len() > 6 {
        cache.remove(0);
    }
    r
}

/// An AI mask over the box `bbox` of the source, at the source's resolution: the model's
/// picture scaled up, its edges then fitted to the photo's with a guided filter (models
/// work at ~1000 px, so on their own they leave soft, blocky edges). Missing pictures
/// select nothing.
fn ai_raster(source: &WorkingImage, id: &str, bbox: (usize, usize, usize, usize)) -> Arc<Raster> {
    let (sw, sh) = (source.width() as usize, source.height() as usize);
    let mut key: u64 = 0x9e37_79b9_7f4a_7c15;
    for b in id.bytes().map(u64::from).chain([bbox.0, bbox.1, bbox.2, bbox.3, sw, sh].map(|v| v as u64)) {
        key = (key ^ b).wrapping_mul(0x0000_0100_0000_01b3);
    }
    {
        let cache = source.mask_cache.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, r)) = cache.iter().find(|(k, _)| *k == key) {
            return r.clone();
        }
    }
    let r = Arc::new(match mask_picture(id) {
        Some(p) => refine(source, &p, bbox),
        None => Raster { x0: 0, y0: 0, w: 0, h: 0, data: Vec::new() },
    });
    let mut cache = source.mask_cache.0.lock().unwrap_or_else(|e| e.into_inner());
    cache.push((key, r.clone()));
    if cache.len() > 6 {
        cache.remove(0);
    }
    r
}

/// The guided filter's radius in source pixels: four of the picture's pixels (models leave
/// edges that soft), and at least 1/400 of the photo.
fn refine_radius(source: (usize, usize), picture: (usize, usize)) -> usize {
    let scale = (source.0 as f32 / picture.0.max(1) as f32).max(source.1 as f32 / picture.1.max(1) as f32);
    ((4.0 * scale).round() as usize).max(source.0.max(source.1) / 400).max(2)
}

fn refine(source: &WorkingImage, picture: &image::GrayImage, bbox: (usize, usize, usize, usize)) -> Raster {
    let (sw, sh) = (source.width() as usize, source.height() as usize);
    let r = refine_radius((sw, sh), (picture.width() as usize, picture.height() as usize));
    // Work on the box plus what the filter reaches into (two box blurs), so a part of the
    // photo comes out exactly as it would in a whole one.
    let pad = 2 * r + 2;
    let (bx0, by0) = (bbox.0.saturating_sub(pad), bbox.1.saturating_sub(pad));
    let (bx1, by1) = ((bbox.0 + bbox.2 + pad).min(sw), (bbox.1 + bbox.3 + pad).min(sh));
    let (w, h) = (bx1.saturating_sub(bx0), by1.saturating_sub(by0));
    if w == 0 || h == 0 || bbox.2 == 0 || bbox.3 == 0 {
        return Raster { x0: bbox.0, y0: bbox.1, w: 0, h: 0, data: Vec::new() };
    }
    let (pw, ph) = (picture.width() as usize, picture.height() as usize);
    let (kx, ky) = (pw as f32 / sw as f32, ph as f32 / sh as f32);
    let px = picture.as_raw();
    let src = source.pixels.as_raw();
    // Guide: perceptual-ish lightness of the source; input: the picture, bilinear.
    let (guide, input): (Vec<f32>, Vec<f32>) = (0..w * h)
        .into_par_iter()
        .map(|k| {
            let (x, y) = (bx0 + k % w, by0 + k / w);
            let s = &src[(y * sw + x) * 3..(y * sw + x) * 3 + 3];
            let l = (0.2126 * s[0] + 0.7152 * s[1] + 0.0722 * s[2]).clamp(0.0, 1.0).sqrt();
            let fx = ((x as f32 + 0.5) * kx - 0.5).clamp(0.0, (pw - 1) as f32);
            let fy = ((y as f32 + 0.5) * ky - 0.5).clamp(0.0, (ph - 1) as f32);
            let (x0, y0) = (fx as usize, fy as usize);
            let (x1, y1) = ((x0 + 1).min(pw - 1), (y0 + 1).min(ph - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let at = |x: usize, y: usize| f32::from(px[y * pw + x]) / 255.0;
            let top = at(x0, y0) + tx * (at(x1, y0) - at(x0, y0));
            let bottom = at(x0, y1) + tx * (at(x1, y1) - at(x0, y1));
            (l, top + ty * (bottom - top))
        })
        .unzip();
    let (mi, mii) = super::detail::guide_stats(&guide, w, h, r);
    let (ma, mb) = super::detail::guided_coeffs(&guide, &mi, &mii, &input, w, h, r, 1e-4);
    let (ox, oy) = (bbox.0 - bx0, bbox.1 - by0);
    let mut data = vec![0.0f32; bbox.2 * bbox.3];
    data.par_chunks_mut(bbox.2).enumerate().for_each(|(j, row)| {
        let base = (j + oy) * w + ox;
        for (i, v) in row.iter_mut().enumerate() {
            let k = base + i;
            *v = (ma[k] * guide[k] + mb[k]).clamp(0.0, 1.0);
        }
    });
    Raster { x0: bbox.0, y0: bbox.1, w: bbox.2, h: bbox.3, data }
}

/// Every active mask's value at each pixel of a region of the rendered image.
pub(super) struct MaskField {
    /// The masks (indices into the settings' list) the values are for.
    pub(super) active: Vec<usize>,
    region: Region,
    values: Vec<f32>,
}

impl MaskField {
    /// Mask values over `region` of `shown` (the image being rendered: `source`, or its
    /// transformed copy). `None` when no mask changes anything.
    pub(super) fn build(
        source: &WorkingImage,
        shown: &WorkingImage,
        masks: &[Mask],
        region: Region,
    ) -> Option<MaskField> {
        let active: Vec<usize> =
            masks.iter().enumerate().filter(|(_, m)| m.is_active()).map(|(i, _)| i).collect();
        Self::build_for(source, shown, masks, active, region)
    }

    /// Values of the masks `active` (indices into `masks`), whatever they adjust: also for
    /// the overlay of a mask that has no adjustments yet.
    pub(super) fn build_for(
        source: &WorkingImage,
        shown: &WorkingImage,
        masks: &[Mask],
        active: Vec<usize>,
        region: Region,
    ) -> Option<MaskField> {
        if active.is_empty() {
            return None;
        }
        let frame = Frame::new(source.width(), source.height());
        let mapping = shown.warp_source;
        let to_source = |x: f64, y: f64| match &mapping {
            Some(m) => m.source(x, y).map(|(u, v)| (u as f32, v as f32)),
            None => Some((x as f32, y as f32)),
        };
        // The part of the source the region shows (its corners map to a convex quad), for
        // brush rasters.
        let (rx, ry, rw, rh) = (region.x as f64, region.y as f64, region.w as f64, region.h as f64);
        let corners = [(rx, ry), (rx + rw, ry), (rx, ry + rh), (rx + rw, ry + rh)];
        let (sw, sh) = (source.width() as usize, source.height() as usize);
        let bbox = corners.iter().map(|&(x, y)| to_source(x, y)).collect::<Option<Vec<_>>>().map_or(
            (0, 0, sw, sh),
            |pts| {
                let lo_x = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min);
                let hi_x = pts.iter().map(|p| p.0).fold(f32::MIN, f32::max);
                let lo_y = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
                let hi_y = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max);
                let x0 = (lo_x - 2.0).floor().clamp(0.0, sw as f32) as usize;
                let y0 = (lo_y - 2.0).floor().clamp(0.0, sh as f32) as usize;
                let x1 = (hi_x + 2.0).ceil().clamp(0.0, sw as f32) as usize;
                let y1 = (hi_y + 2.0).ceil().clamp(0.0, sh as f32) as usize;
                (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
            },
        );
        let norm = |p: [f32; 2]| frame.px(p);
        let prepared: Vec<Prepared> = active
            .iter()
            .map(|&i| {
                let m = &masks[i];
                let parts = m
                    .components
                    .iter()
                    .map(|c| {
                        let e = match &c.shape {
                            Shape::Linear { start, end } => {
                                let (s, e) = (norm(*start), norm(*end));
                                let dir = [e[0] - s[0], e[1] - s[1]];
                                let len2 = (dir[0] * dir[0] + dir[1] * dir[1]).max(1e-6);
                                Evaluator::Linear { start: s, dir, inv_len2: 1.0 / len2 }
                            }
                            Shape::Radial { center, radii, angle, feather } => {
                                let a = angle.to_radians();
                                let (rx, ry) =
                                    ((radii[0] * frame.half).max(0.5), (radii[1] * frame.half).max(0.5));
                                Evaluator::Radial {
                                    center: norm(*center),
                                    cos: a.cos(),
                                    sin: a.sin(),
                                    inv_rx: 1.0 / rx,
                                    inv_ry: 1.0 / ry,
                                    inner: 1.0 - feather.clamp(0.0, 100.0) / 100.0,
                                }
                            }
                            Shape::Brush { strokes } => Evaluator::Brush(brush_raster(source, strokes, bbox)),
                            Shape::Raster { id, .. } => Evaluator::Brush(ai_raster(source, id, bbox)),
                        };
                        (c.mode, c.invert, e)
                    })
                    .collect();
                Prepared { parts, amount: m.amount.clamp(0.0, 100.0) / 100.0 }
            })
            .collect();
        let k = prepared.len();
        let (w, h) = (region.w as usize, region.h as usize);
        let mut values = vec![0.0f32; w * h * k];
        values.par_chunks_mut(w * k).enumerate().for_each(|(j, row)| {
            let y = region.y as f64 + j as f64 + 0.5;
            for (i, px) in row.chunks_mut(k).enumerate() {
                let Some((u, v)) = to_source(region.x as f64 + i as f64 + 0.5, y) else { continue };
                for (out, p) in px.iter_mut().zip(&prepared) {
                    *out = p.value(u, v);
                }
            }
        });
        Some(MaskField { active, region, values })
    }

    /// The active masks' values at pixel (x, y) of the image (inside the region).
    #[inline]
    pub(super) fn at(&self, x: usize, y: usize) -> &[f32] {
        let k = self.active.len();
        let i = (y - self.region.y as usize) * self.region.w as usize + (x - self.region.x as usize);
        &self.values[i * k..(i + 1) * k]
    }

    /// The values of active mask `n` (its position in `active`) over the region, row by
    /// row: the overlay.
    pub(super) fn plane(&self, n: usize) -> Vec<f32> {
        let k = self.active.len();
        self.values.iter().skip(n).step_by(k).copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame::new(200, 100)
    }

    fn eval(c: Shape) -> impl Fn(f32, f32) -> f32 {
        let f = frame();
        let m = Mask::new("t", c);
        let p = match &m.components[0].shape {
            Shape::Linear { start, end } => {
                let (s, e) = (f.px(*start), f.px(*end));
                let dir = [e[0] - s[0], e[1] - s[1]];
                Evaluator::Linear { start: s, dir, inv_len2: 1.0 / (dir[0] * dir[0] + dir[1] * dir[1]) }
            }
            Shape::Radial { center, radii, angle, feather } => Evaluator::Radial {
                center: f.px(*center),
                cos: angle.to_radians().cos(),
                sin: angle.to_radians().sin(),
                inv_rx: 1.0 / (radii[0] * f.half),
                inv_ry: 1.0 / (radii[1] * f.half),
                inner: 1.0 - feather / 100.0,
            },
            Shape::Brush { strokes } => Evaluator::Brush(Arc::new(rasterize(strokes, f, (0, 0, 200, 100)))),
            Shape::Raster { .. } => unreachable!(),
        };
        move |u, v| p.value(u, v)
    }

    #[test]
    fn shapes_fall_off_as_drawn() {
        // Linear: full at the start, none past the end.
        let lin = eval(Shape::Linear { start: [0.5, 0.2], end: [0.5, 0.6] });
        assert_eq!(lin(100.0, 10.0), 1.0);
        assert_eq!(lin(100.0, 80.0), 0.0);
        assert!((lin(100.0, 40.0) - 0.5).abs() < 0.01);
        // Radial: inside on, outside off, round in pixels on a 2:1 photo.
        let rad = eval(Shape::Radial { center: [0.5, 0.5], radii: [0.3, 0.3], angle: 0.0, feather: 50.0 });
        assert_eq!(rad(100.0, 50.0), 1.0);
        assert_eq!(rad(100.0, 85.0), 0.0); // 35 px below: outside the 30 px radius
        assert_eq!(rad(135.0, 50.0), 0.0);
        assert!(rad(100.0, 70.0) > 0.4 && rad(100.0, 70.0) < 1.0, "in the feather");
        // Brush: paint where the stroke went, nothing elsewhere; erase removes it.
        let paint = Stroke { points: vec![[0.2, 0.5], [0.8, 0.5]], size: 0.1, ..Stroke::default() };
        let brush = eval(Shape::Brush { strokes: vec![paint.clone()] });
        assert!(brush(100.0, 50.0) > 0.99);
        assert_eq!(brush(100.0, 5.0), 0.0);
        let erase =
            Stroke { points: vec![[0.5, 0.2], [0.5, 0.8]], size: 0.05, erase: true, ..Stroke::default() };
        let erased = eval(Shape::Brush { strokes: vec![paint, erase] });
        assert!(erased(100.0, 50.0) < 0.01 && erased(60.0, 50.0) > 0.99);
    }

    #[test]
    fn components_combine_in_order() {
        // Every part is the same linear ramp from x = 0 to x = 1: 1 at u = -1, 0 at u = 2.
        let p = |parts: &[(CombineMode, bool)], amount: f32| Prepared {
            parts: parts
                .iter()
                .map(|&(m, inv)| {
                    (m, inv, Evaluator::Linear { start: [0.0, 0.0], dir: [1.0, 0.0], inv_len2: 1.0 })
                })
                .collect(),
            amount,
        };
        let add = p(&[(CombineMode::Add, false), (CombineMode::Add, true)], 1.0);
        assert_eq!(add.value(2.0, 0.0), 1.0, "0, then max with inverted 0");
        let sub = p(&[(CombineMode::Add, false), (CombineMode::Subtract, false)], 1.0);
        assert_eq!(sub.value(-1.0, 0.0), 0.0, "1 minus 1");
        let inter = p(&[(CombineMode::Add, false), (CombineMode::Intersect, true)], 1.0);
        assert_eq!(inter.value(-1.0, 0.0), 0.0, "1 and not 1");
        assert_eq!(p(&[(CombineMode::Add, false)], 0.5).value(-1.0, 0.0), 0.5);
    }

    #[test]
    fn dabs_follow_the_path_evenly() {
        let s = Stroke { points: vec![[0.0, 0.5], [0.5, 0.5], [0.5, 1.0]], size: 0.08, ..Stroke::default() };
        let d = dabs(&[s], frame());
        // 100 px + 50 px of path, spacing 2 px (radius 8 / 4): about 76 dabs.
        assert!((70..=80).contains(&d.len()), "{}", d.len());
        let gaps: Vec<f32> = d.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).collect();
        assert!(gaps.iter().all(|g| *g <= 2.01), "{gaps:?}");
    }
}
