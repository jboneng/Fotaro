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
    ///
    /// With `range` the picture is a depth map (white near, see [`DepthRange`]) and the
    /// mask is the part of the photo between two distances.
    Raster {
        id: String,
        selects: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<DepthRange>,
    },
}

/// The part of a depth map a mask selects: from `near` to `far`, each 0 (the nearest
/// thing in the photo) to 100 (the farthest), with `feather` (0..100) for how softly it
/// ends at both. The scale is the model's, relative to this photo: there are no metres.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DepthRange {
    pub near: f32,
    pub far: f32,
    pub feather: f32,
}

impl Default for DepthRange {
    fn default() -> Self {
        DepthRange { near: 0.0, far: 50.0, feather: 25.0 }
    }
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
                "Depth" => "Depth Range",
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

/// How far away the depth map `id` says the point (x, y) of the untransformed photo
/// (0..1) is: 0 (the nearest) to 100 (the farthest), as Lens Blur's Focus and a Depth
/// Range's Near and Far count it. The middle value of a small area around the point, so
/// a hair or a speck under it does not decide. `None` without the map.
pub fn depth_at(id: &str, x: f32, y: f32) -> Option<f32> {
    let p = mask_picture(id)?;
    let (w, h) = (p.width() as i32, p.height() as i32);
    if w == 0 || h == 0 {
        return None;
    }
    let (cx, cy) = ((x.clamp(0.0, 1.0) * w as f32) as i32, (y.clamp(0.0, 1.0) * h as f32) as i32);
    let r = (w.max(h) / 200).max(1);
    let mut around: Vec<u8> = Vec::new();
    for yy in (cy - r).max(0)..=(cy + r).min(h - 1) {
        for xx in (cx - r).max(0)..=(cx + r).min(w - 1) {
            around.push(p.get_pixel(xx as u32, yy as u32)[0]);
        }
    }
    around.sort_unstable();
    Some(100.0 * (1.0 - f32::from(*around.get(around.len() / 2)?) / 255.0))
}

/// Where Relight's light falls, as a small picture (`long_side` pixels) of the whole
/// untransformed photo in the file's orientation: white where the slice's light is full,
/// black where there is none, by the depth map alone (its edges not fitted to the photo's;
/// Shaping left out). For showing where the light is while it is placed. `None` without
/// the depth map.
pub fn relight_preview(e: &super::effects::Effects, long_side: u32) -> Option<image::GrayImage> {
    let depth = mask_picture(e.fog_depth.as_ref()?)?;
    let (w, h) = depth.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let k = long_side.max(16) as f32 / w.max(h) as f32;
    let (pw, ph) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    let (radius, feather) = e.relight_area();
    let (rounded, inner) = (e.relight_size < 100.0, 1.0 - feather / 100.0);
    let half = w.max(h) as f32 / 2.0;
    Some(image::GrayImage::from_fn(pw, ph, |x, y| {
        let (u, v) = ((x as f32 + 0.5) / pw as f32, (y as f32 + 0.5) / ph as f32);
        let mut lit = e.relight_weight(1.0 - f32::from(depth_value(&depth, u, v)) / 255.0);
        if rounded {
            let (dx, dy) = ((u - e.relight_at[0]) * w as f32, (v - e.relight_at[1]) * h as f32);
            let d = (dx * dx + dy * dy).sqrt() / (radius * half);
            lit *= 1.0 - smoothstep(inner, 1.0, d);
        }
        image::Luma([(lit * 255.0).round().clamp(0.0, 255.0) as u8])
    }))
}

/// The depth map's value (0..255, more is nearer) at (u, v), 0..1 of the photo.
fn depth_value(depth: &image::GrayImage, u: f32, v: f32) -> u8 {
    let (w, h) = depth.dimensions();
    depth.get_pixel(((u * w as f32) as u32).min(w - 1), ((v * h as f32) as u32).min(h - 1))[0]
}

/// How near what `mask` selects is by the depth map (both over the whole untransformed
/// photo, at any sizes): its mean value there, 0 (far) to 255. `None` for an empty mask.
pub fn mask_nearness(mask: &image::GrayImage, depth: &image::GrayImage) -> Option<f32> {
    let (w, h) = mask.dimensions();
    if w == 0 || h == 0 || depth.width() == 0 || depth.height() == 0 {
        return None;
    }
    let step = (w.max(h) / 256).max(1);
    let (mut sum, mut n) = (0.0f64, 0.0f64);
    for y in (step / 2..h).step_by(step as usize) {
        for x in (step / 2..w).step_by(step as usize) {
            let k = f64::from(mask.get_pixel(x, y)[0]);
            if k > 0.0 {
                let d = depth_value(depth, (x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                sum += k * f64::from(d);
                n += k;
            }
        }
    }
    (n > 0.0).then(|| (sum / n) as f32)
}

/// Where what `mask` selects is by the depth map (both over the whole untransformed
/// photo, at any sizes), for a light to be put on it: how far away the middle of it is
/// and how deep most of it is (both on the 0..100 scale of [`depth_at`]), and its centre
/// (0..1 of the photo). `None` when it selects next to nothing.
pub fn mask_depth_span(mask: &image::GrayImage, depth: &image::GrayImage) -> Option<(f32, f32, [f32; 2])> {
    let (w, h) = mask.dimensions();
    if w == 0 || h == 0 || depth.width() == 0 || depth.height() == 0 {
        return None;
    }
    let step = (w.max(h) / 256).max(1);
    let mut far: Vec<u8> = Vec::new();
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    for y in (step / 2..h).step_by(step as usize) {
        for x in (step / 2..w).step_by(step as usize) {
            if mask.get_pixel(x, y)[0] > 128 {
                let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                far.push(255 - depth_value(depth, u, v));
                sx += f64::from(u);
                sy += f64::from(v);
            }
        }
    }
    if far.len() < 20 {
        return None;
    }
    far.sort_unstable();
    let n = far.len();
    let at = |share: usize| 100.0 * f32::from(far[(n * share / 100).min(n - 1)]) / 255.0;
    let centre = [(sx / n as f64) as f32, (sy / n as f64) as f32];
    Some((at(50), at(90) - at(10), centre))
}

/// Take out of a Subject mask what the depth map says is clearly behind the subject: a
/// piece of the background the model took along with it. Careful on purpose: only what is
/// far behind the subject for how deep the subject itself is, and only where
/// everything close by is as far (the map's edges are soft and miss hair). Returns the
/// share of the subject taken out.
pub fn subject_by_depth(mask: &mut image::GrayImage, depth: &image::GrayImage) -> f32 {
    const GRID: usize = 256;
    let (w, h) = mask.dimensions();
    if w == 0 || h == 0 || depth.width() == 0 || depth.height() == 0 {
        return 0.0;
    }
    let k = GRID as f32 / w.max(h) as f32;
    let (gw, gh) = (((w as f32 * k).round() as usize).max(1), ((h as f32 * k).round() as usize).max(1));
    let at = |x: usize, y: usize| ((x as f32 + 0.5) / gw as f32, (y as f32 + 0.5) / gh as f32);
    let near: Vec<u8> = (0..gw * gh)
        .map(|i| {
            let (u, v) = at(i % gw, i / gw);
            depth_value(depth, u, v)
        })
        .collect();
    // How far away the subject is (the middle of it, where the model is sure) and how deep
    // (the middle of how far its parts are from that): neither moves until a piece of the
    // background is half of what was selected.
    let mut subject: Vec<u8> = (0..gw * gh)
        .filter(|i| {
            let (u, v) = at(i % gw, i / gw);
            mask.get_pixel(((u * w as f32) as u32).min(w - 1), ((v * h as f32) as u32).min(h - 1))[0] > 200
        })
        .map(|i| near[i])
        .collect();
    if subject.len() < 50 {
        return 0.0;
    }
    subject.sort_unstable();
    let middle = f32::from(subject[subject.len() / 2]);
    let mut off: Vec<f32> = subject.iter().map(|d| (f32::from(*d) - middle).abs()).collect();
    off.sort_unstable_by(f32::total_cmp);
    let hi = middle - 4.5 * off[off.len() / 2] - 16.0;
    let lo = hi - 16.0;
    if hi <= 0.0 {
        return 0.0;
    }
    // The nearest thing close by (1.5% of the photo around).
    let r = (GRID * 3 / 200).max(1) as isize;
    let widest = |v: &[u8], along_x: bool| -> Vec<u8> {
        (0..gw * gh)
            .map(|i| {
                let (x, y) = ((i % gw) as isize, (i / gw) as isize);
                (-r..=r)
                    .map(|d| {
                        let (xx, yy) = if along_x { (x + d, y) } else { (x, y + d) };
                        v[yy.clamp(0, gh as isize - 1) as usize * gw + xx.clamp(0, gw as isize - 1) as usize]
                    })
                    .max()
                    .unwrap_or(0)
            })
            .collect()
    };
    let around = widest(&widest(&near, true), false);
    let (mut before, mut after) = (0.0f64, 0.0f64);
    for (x, y, p) in mask.enumerate_pixels_mut() {
        if p[0] == 0 {
            continue;
        }
        // Between the grid's cells, so the cut has no steps.
        let fx = ((x as f32 + 0.5) / w as f32 * gw as f32 - 0.5).clamp(0.0, (gw - 1) as f32);
        let fy = ((y as f32 + 0.5) / h as f32 * gh as f32 - 0.5).clamp(0.0, (gh - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(gw - 1), (y0 + 1).min(gh - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let v = |x: usize, y: usize| f32::from(around[y * gw + x]);
        let top = v(x0, y0) + tx * (v(x1, y0) - v(x0, y0));
        let bottom = v(x0, y1) + tx * (v(x1, y1) - v(x0, y1));
        let keep = smoothstep(lo, hi, top + ty * (bottom - top));
        before += f64::from(p[0]);
        p[0] = (f32::from(p[0]) * keep).round() as u8;
        after += f64::from(p[0]);
    }
    if before > 0.0 { (1.0 - after / before) as f32 } else { 0.0 }
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

/// A Remove fill's picture halved `halvings` times (each pixel the mean of four), kept
/// like the pictures themselves: for showing a large fill in a small preview.
pub(super) fn fill_picture_halved(id: &str, halvings: u32) -> Option<Arc<image::RgbImage>> {
    if halvings == 0 {
        return fill_picture(id);
    }
    let key = format!("{id}@{halvings}");
    if let Some((_, p)) =
        FILL_PICTURES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key)
    {
        return Some(p.clone());
    }
    let from = fill_picture_halved(id, halvings - 1)?;
    let (w, h) = ((from.width() / 2).max(1), (from.height() / 2).max(1));
    let half = Arc::new(image::RgbImage::from_fn(w, h, |x, y| {
        let at = |dx: u32, dy: u32| {
            from.get_pixel((2 * x + dx).min(from.width() - 1), (2 * y + dy).min(from.height() - 1)).0
        };
        let (a, b, c, d) = (at(0, 0), at(1, 0), at(0, 1), at(1, 1));
        image::Rgb(std::array::from_fn(|i| {
            ((u16::from(a[i]) + u16::from(b[i]) + u16::from(c[i]) + u16::from(d[i]) + 2) / 4) as u8
        }))
    }));
    let mut cache = FILL_PICTURES.lock().unwrap_or_else(|e| e.into_inner());
    cache.push((key, half.clone()));
    if cache.len() > 32 {
        cache.remove(0);
    }
    Some(half)
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
    Linear {
        start: [f32; 2],
        dir: [f32; 2],
        inv_len2: f32,
    },
    Radial {
        center: [f32; 2],
        cos: f32,
        sin: f32,
        inv_rx: f32,
        inv_ry: f32,
        inner: f32,
    },
    Brush(Arc<Raster>),
    /// A depth map (1: near) and the part of it selected: how far (0..1) it starts and
    /// ends, and half the width of the ramp at each.
    Range {
        depth: Arc<Raster>,
        lo: f32,
        hi: f32,
        soft: f32,
    },
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
            Evaluator::Range { depth, lo, hi, soft } => {
                let far = 1.0 - depth.sample(u, v);
                // An end at the very front or back takes everything there.
                let from = if *lo <= 0.0 { 1.0 } else { smoothstep(lo - soft, lo + soft, far) };
                let to = if *hi >= 1.0 { 1.0 } else { 1.0 - smoothstep(hi - soft, hi + soft, far) };
                from * to
            }
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
pub(super) struct MaskCache(Arc<Mutex<Rasters>>);

/// Kept rasters by key, oldest first.
type Rasters = Vec<(u64, Arc<Raster>)>;

impl Clone for MaskCache {
    /// A copy starts empty (it is only a cache).
    fn clone(&self) -> Self {
        MaskCache::default()
    }
}

impl MaskCache {
    /// This very cache, for a healed copy of its image.
    pub(super) fn shared(&self) -> MaskCache {
        MaskCache(self.0.clone())
    }
}

/// Rasters kept per image: brush masks and AI masks for the proxy, the 1:1 view and the
/// Detail box, and AI masks again for each healed copy.
const MASK_RASTERS: usize = 12;

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
    if cache.len() > MASK_RASTERS {
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
    // Its edges are fitted to these pixels: each healed copy has its own.
    let pixels = source.pixels_id as usize;
    for b in
        id.bytes().map(u64::from).chain([bbox.0, bbox.1, bbox.2, bbox.3, sw, sh, pixels].map(|v| v as u64))
    {
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
    if cache.len() > MASK_RASTERS {
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

/// The picture scaled up to `bbox` of the source, its edges fitted to the photo's: a guided
/// filter with the photo's colour as the guide (He, Sun and Tang), so an edge between two
/// things of the same lightness but another colour is found too. The filter's sums are
/// made on a coarser grid (lined up with the photo, not the box) and its result scaled
/// back up, as in their fast variant: a full-size photo would need gigabytes otherwise.
fn refine(source: &WorkingImage, picture: &image::GrayImage, bbox: (usize, usize, usize, usize)) -> Raster {
    use super::local::box_blur_par;
    let (sw, sh) = (source.width() as usize, source.height() as usize);
    let r = refine_radius((sw, sh), (picture.width() as usize, picture.height() as usize));
    if bbox.2 == 0 || bbox.3 == 0 || sw == 0 || sh == 0 {
        return Raster { x0: bbox.0, y0: bbox.1, w: 0, h: 0, data: Vec::new() };
    }
    // The grid's step, and the radius in its cells.
    let step = (r / 3).clamp(1, 8);
    let rs = ((r as f32 / step as f32).round() as usize).max(1);
    // Work on the box plus what the filter reaches into (two box blurs and the scaling
    // back up), on whole cells, so a part of the photo comes out exactly as it would in a
    // whole one.
    let pad = (2 * rs + 3) * step;
    let (bx0, by0) = (bbox.0.saturating_sub(pad) / step * step, bbox.1.saturating_sub(pad) / step * step);
    let (bx1, by1) = ((bbox.0 + bbox.2 + pad).min(sw), (bbox.1 + bbox.3 + pad).min(sh));
    let (gw, gh) = ((bx1 - bx0).div_ceil(step), (by1 - by0).div_ceil(step));
    let (pw, ph) = (picture.width() as usize, picture.height() as usize);
    let (kx, ky) = (pw as f32 / sw as f32, ph as f32 / sh as f32);
    let px = picture.as_raw();
    let src = source.pixels.as_raw();
    // The guide: the photo's colour, each channel about as the eye spaces its tones.
    let guide = |x: usize, y: usize| -> [f32; 3] {
        let p = &src[(y * sw + x) * 3..(y * sw + x) * 3 + 3];
        [p[0].clamp(0.0, 1.0).sqrt(), p[1].clamp(0.0, 1.0).sqrt(), p[2].clamp(0.0, 1.0).sqrt()]
    };
    let shown = |x: f32, y: f32| -> f32 {
        let fx = (x * kx - 0.5).clamp(0.0, (pw - 1) as f32);
        let fy = (y * ky - 0.5).clamp(0.0, (ph - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(pw - 1), (y0 + 1).min(ph - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| f32::from(px[y * pw + x]) / 255.0;
        let top = at(x0, y0) + tx * (at(x1, y0) - at(x0, y0));
        let bottom = at(x0, y1) + tx * (at(x1, y1) - at(x0, y1));
        top + ty * (bottom - top)
    };
    // Per cell: the guide's mean colour, and the picture at its middle.
    let cells: Vec<([f32; 3], f32)> = (0..gw * gh)
        .into_par_iter()
        .map(|k| {
            let (cx, cy) = (bx0 + (k % gw) * step, by0 + (k / gw) * step);
            let (ex, ey) = ((cx + step).min(sw), (cy + step).min(sh));
            let mut sum = [0.0f32; 3];
            for y in cy..ey {
                for x in cx..ex {
                    let g = guide(x, y);
                    sum = [sum[0] + g[0], sum[1] + g[1], sum[2] + g[2]];
                }
            }
            let n = ((ex - cx) * (ey - cy)) as f32;
            (sum.map(|v| v / n), shown((cx + ex) as f32 / 2.0, (cy + ey) as f32 / 2.0))
        })
        .collect();
    type Cell = ([f32; 3], f32);
    let plane = |f: &(dyn Fn(&Cell) -> f32 + Sync)| -> Vec<f32> {
        let v: Vec<f32> = cells.iter().map(f).collect();
        box_blur_par(&v, gw, gh, rs)
    };
    let mean_g: [Vec<f32>; 3] = std::array::from_fn(|c| plane(&move |v| v.0[c]));
    let mean_p = plane(&|v| v.1);
    let mean_gp: [Vec<f32>; 3] = std::array::from_fn(|c| plane(&move |v| v.0[c] * v.1));
    // The guide's channels by each other: rr, rg, rb, gg, gb, bb.
    const PAIRS: [(usize, usize); 6] = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
    let mean_gg: [Vec<f32>; 6] = std::array::from_fn(|n| plane(&move |v| v.0[PAIRS[n].0] * v.0[PAIRS[n].1]));
    // Per cell: the picture as a line through the guide's colours, `a · colour + b`.
    const EPS: f32 = 1e-4;
    let lines: Vec<[f32; 4]> = (0..gw * gh)
        .into_par_iter()
        .map(|k| {
            let m = [mean_g[0][k], mean_g[1][k], mean_g[2][k]];
            let var = |n: usize| mean_gg[n][k] - m[PAIRS[n].0] * m[PAIRS[n].1];
            let (rr, rg, rb, gg, gb, bb) = (var(0) + EPS, var(1), var(2), var(3) + EPS, var(4), var(5) + EPS);
            let cov: [f32; 3] = std::array::from_fn(|c| mean_gp[c][k] - m[c] * mean_p[k]);
            // The inverse of the (symmetric) 3 × 3 by its cofactors.
            let (c00, c01, c02) = (gg * bb - gb * gb, rb * gb - rg * bb, rg * gb - rb * gg);
            let (c11, c12, c22) = (rr * bb - rb * rb, rg * rb - rr * gb, rr * gg - rg * rg);
            let det = rr * c00 + rg * c01 + rb * c02;
            if det.abs() < 1e-20 {
                return [0.0, 0.0, 0.0, mean_p[k]];
            }
            let a = [
                (c00 * cov[0] + c01 * cov[1] + c02 * cov[2]) / det,
                (c01 * cov[0] + c11 * cov[1] + c12 * cov[2]) / det,
                (c02 * cov[0] + c12 * cov[1] + c22 * cov[2]) / det,
            ];
            [a[0], a[1], a[2], mean_p[k] - a[0] * m[0] - a[1] * m[1] - a[2] * m[2]]
        })
        .collect();
    let line: [Vec<f32>; 4] = std::array::from_fn(|c| {
        let v: Vec<f32> = lines.iter().map(|l| l[c]).collect();
        box_blur_par(&v, gw, gh, rs)
    });
    let mut data = vec![0.0f32; bbox.2 * bbox.3];
    data.par_chunks_mut(bbox.2).enumerate().for_each(|(j, row)| {
        let y = bbox.1 + j;
        let fy = ((y - by0) as f32 + 0.5) / step as f32 - 0.5;
        let fy = fy.clamp(0.0, (gh - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(gh - 1);
        for (i, v) in row.iter_mut().enumerate() {
            let x = bbox.0 + i;
            let fx = (((x - bx0) as f32 + 0.5) / step as f32 - 0.5).clamp(0.0, (gw - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(gw - 1);
            let at = |c: usize| {
                let p = &line[c];
                let top = p[y0 * gw + x0] + tx * (p[y0 * gw + x1] - p[y0 * gw + x0]);
                let bottom = p[y1 * gw + x0] + tx * (p[y1 * gw + x1] - p[y1 * gw + x0]);
                top + ty * (bottom - top)
            };
            let g = guide(x, y);
            *v = (at(0) * g[0] + at(1) * g[1] + at(2) * g[2] + at(3)).clamp(0.0, 1.0);
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
    /// transformed copy). `None` when no mask changes anything. Kept with `shown` until the
    /// masks' shapes change (their adjustments don't matter).
    pub(super) fn build(
        source: &WorkingImage,
        shown: &WorkingImage,
        masks: &[Mask],
        region: Region,
    ) -> Option<Arc<MaskField>> {
        let active: Vec<usize> =
            masks.iter().enumerate().filter(|(_, m)| m.is_active()).map(|(i, _)| i).collect();
        if active.is_empty() {
            return None;
        }
        let key = (
            region,
            active.iter().map(|&i| (i, masks[i].amount.to_bits(), masks[i].components.clone())).collect(),
        );
        Some(shown.render_cache.field.get_or(key, || {
            Self::build_for(source, shown, masks, active, region).expect("there are active masks")
        }))
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
                            Shape::Raster { id, range: Some(r), .. } => Evaluator::Range {
                                depth: ai_raster(source, id, bbox),
                                lo: r.near.clamp(0.0, 100.0) / 100.0,
                                hi: r.far.clamp(0.0, 100.0) / 100.0,
                                soft: (r.feather.clamp(0.0, 100.0) / 100.0 * 0.2).max(0.004),
                            },
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
