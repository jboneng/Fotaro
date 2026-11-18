//! Transform (LrC's Transform panel): Upright, perspective, rotation, aspect, scale and
//! offset, and Constrain Crop.
//!
//! The photo is treated as seen through a camera of a normal focal length; Upright and the
//! Vertical / Horizontal sliders turn that camera (keystone corrections), Rotate turns the
//! picture. The output keeps the photo's size; where the transformed photo doesn't reach,
//! the result is white, as in LrC, unless Constrain Crop keeps only the largest part of
//! the frame (of the photo's aspect) that the photo covers.
//!
//! The transformed image is made once per Transform and cached with the [`WorkingImage`],
//! so the rest of the pipeline (local adjustments, Detail, Effects) works on it unchanged.

use std::sync::{Arc, Mutex};

use image::{ImageBuffer, Rgb32FImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::matrix::Mat3;
use super::source::WorkingImage;

/// LrC's Upright modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Upright {
    #[default]
    Off,
    /// Balanced: level, and verticals and horizontals straightened in moderation.
    Auto,
    /// From guides drawn on the photo.
    Guided,
    /// Only level (rotation).
    Level,
    /// Level, and converging verticals made parallel.
    Vertical,
    /// Level, verticals and horizontals all straightened.
    Full,
}

impl Upright {
    pub const ALL: [Upright; 6] =
        [Self::Off, Self::Auto, Self::Guided, Self::Level, Self::Vertical, Self::Full];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Auto => "Auto",
            Self::Guided => "Guided",
            Self::Level => "Level",
            Self::Vertical => "Vertical",
            Self::Full => "Full",
        }
    }
}

/// Upright Guided allows this many guides, as LrC does.
pub const MAX_GUIDES: usize = 4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transform {
    pub upright: Upright,
    /// The camera turn Upright found, in degrees (pitch, yaw, roll); applied before the
    /// sliders. Stored, so a photo looks the same without analysing it again.
    pub upright_angles: [f32; 3],
    /// Upright Guided's guides, `[x1, y1, x2, y2]` in 0..1 of the untransformed photo.
    pub guides: Vec<[f32; 4]>,
    /// Keep only the largest part of the frame the transformed photo covers.
    pub constrain_crop: bool,
    /// Keystone corrections, −100..+100.
    pub vertical: f32,
    pub horizontal: f32,
    /// Degrees, −10..+10, clockwise.
    pub rotate: f32,
    /// −100 (taller) ..+100 (wider).
    pub aspect: f32,
    /// Percent, 50..150.
    pub scale: f32,
    /// Moves the photo, −100..+100.
    pub x_offset: f32,
    pub y_offset: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            upright: Upright::Off,
            upright_angles: [0.0; 3],
            guides: Vec::new(),
            constrain_crop: false,
            vertical: 0.0,
            horizontal: 0.0,
            rotate: 0.0,
            aspect: 0.0,
            scale: 100.0,
            x_offset: 0.0,
            y_offset: 0.0,
        }
    }
}

/// A Transform panel slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransformParam {
    Vertical,
    Horizontal,
    Rotate,
    Aspect,
    Scale,
    XOffset,
    YOffset,
}

impl TransformParam {
    pub const ALL: [TransformParam; 7] = [
        Self::Vertical,
        Self::Horizontal,
        Self::Rotate,
        Self::Aspect,
        Self::Scale,
        Self::XOffset,
        Self::YOffset,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Vertical => "Vertical",
            Self::Horizontal => "Horizontal",
            Self::Rotate => "Rotate",
            Self::Aspect => "Aspect",
            Self::Scale => "Scale",
            Self::XOffset => "X Offset",
            Self::YOffset => "Y Offset",
        }
    }

    pub fn range(self) -> (f32, f32) {
        match self {
            Self::Rotate => (-10.0, 10.0),
            Self::Scale => (50.0, 150.0),
            _ => (-100.0, 100.0),
        }
    }

    pub fn decimals(self) -> usize {
        if self == Self::Rotate { 1 } else { 0 }
    }

    pub fn get(self, t: &Transform) -> f32 {
        *match self {
            Self::Vertical => &t.vertical,
            Self::Horizontal => &t.horizontal,
            Self::Rotate => &t.rotate,
            Self::Aspect => &t.aspect,
            Self::Scale => &t.scale,
            Self::XOffset => &t.x_offset,
            Self::YOffset => &t.y_offset,
        }
    }

    pub fn get_mut(self, t: &mut Transform) -> &mut f32 {
        match self {
            Self::Vertical => &mut t.vertical,
            Self::Horizontal => &mut t.horizontal,
            Self::Rotate => &mut t.rotate,
            Self::Aspect => &mut t.aspect,
            Self::Scale => &mut t.scale,
            Self::XOffset => &mut t.x_offset,
            Self::YOffset => &mut t.y_offset,
        }
    }
}

/// Focal length of the assumed lens, in half long sides of the frame (about a 35 mm lens
/// on a full-frame camera).
pub(super) const FOCAL: f64 = 1.9;
/// How far the camera turns at Vertical or Horizontal ±100.
const MAX_TILT_DEG: f64 = 30.0;

fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn transpose(m: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| m[j][i]))
}

fn inverse(m: &Mat3) -> Mat3 {
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let det = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    let adj: Mat3 = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    adj.map(|row| row.map(|v| v / det))
}

/// Apply a homography to a point; `None` behind the camera.
#[inline]
fn apply(m: &Mat3, x: f64, y: f64) -> Option<(f64, f64)> {
    let z = m[2][0] * x + m[2][1] * y + m[2][2];
    (z > 1e-9).then(|| ((m[0][0] * x + m[0][1] * y + m[0][2]) / z, (m[1][0] * x + m[1][1] * y + m[1][2]) / z))
}

/// The camera turned by `pitch` (about the horizontal axis), `yaw` (vertical axis) and
/// `roll` (radians): rotation of a source ray into the output.
pub(super) fn camera(pitch: f64, yaw: f64, roll: f64) -> Mat3 {
    let rx: Mat3 = [[1.0, 0.0, 0.0], [0.0, pitch.cos(), -pitch.sin()], [0.0, pitch.sin(), pitch.cos()]];
    let ry: Mat3 = [[yaw.cos(), 0.0, yaw.sin()], [0.0, 1.0, 0.0], [-yaw.sin(), 0.0, yaw.cos()]];
    let rz: Mat3 = [[roll.cos(), -roll.sin(), 0.0], [roll.sin(), roll.cos(), 0.0], [0.0, 0.0, 1.0]];
    mul(&rz, &mul(&rx, &ry))
}

/// Where a point of the source plane (normalised: centre 0, half long side 1) appears
/// after turning the camera by `rot`, the photo's centre kept in place.
pub(super) fn turned(rot: &Mat3, p: [f64; 2]) -> Option<[f64; 2]> {
    let proj = |v: [f64; 3]| (v[2] > 1e-9).then(|| [FOCAL * v[0] / v[2], FOCAL * v[1] / v[2]]);
    let ray = |q: [f64; 3]| std::array::from_fn(|i| (0..3).map(|k| rot[i][k] * q[k]).sum());
    let c = proj(ray([0.0, 0.0, FOCAL]))?;
    let q = proj(ray([p[0], p[1], FOCAL]))?;
    Some([q[0] - c[0], q[1] - c[1]])
}

/// Maps points of the output to points of the source, in pixels, for a `w × h` source.
#[derive(Debug, Clone, Copy)]
pub(super) struct Mapping {
    /// Output pixel → source pixel, a homography (homogeneous coordinates).
    h: Mat3,
    /// The output's size (smaller than the source with Constrain Crop).
    pub(super) out: (u32, u32),
}

impl Mapping {
    pub(super) fn new(t: &Transform, w: u32, h: u32) -> Mapping {
        let uncropped = Self::frame(t, w, h);
        let (fw, fh) = (f64::from(w.max(1)), f64::from(h.max(1)));
        if t.constrain_crop
            && let Some((x0, y0, s)) = inscribed(&uncropped, fw, fh)
        {
            let out = (((fw * s).round() as u32).max(1), ((fh * s).round() as u32).max(1));
            let crop: Mat3 =
                [[fw * s / f64::from(out.0), 0.0, x0], [0.0, fh * s / f64::from(out.1), y0], [0.0, 0.0, 1.0]];
            return Mapping { h: mul(&uncropped, &crop), out };
        }
        Mapping { h: uncropped, out: (w.max(1), h.max(1)) }
    }

    /// Output pixel → source pixel over the whole frame (no crop).
    fn frame(t: &Transform, w: u32, h: u32) -> Mat3 {
        let (w, h) = (f64::from(w.max(1)), f64::from(h.max(1)));
        let half = w.max(h) / 2.0;
        let (cx, cy) = (w / 2.0, h / 2.0);
        // Output pixels → normalised output (centre 0, half long side 1), undoing the
        // offset, scale and aspect.
        let scale = f64::from(t.scale.max(1.0)) / 100.0;
        let aspect = 2f64.powf(f64::from(t.aspect) / 100.0 * 0.5);
        let (sx, sy) = (scale * aspect.sqrt(), scale / aspect.sqrt());
        let (ox, oy) = (f64::from(t.x_offset) / 100.0 * 0.5 * w, f64::from(t.y_offset) / 100.0 * 0.5 * h);
        let to_norm: Mat3 = [
            [1.0 / (half * sx), 0.0, -(cx + ox) / (half * sx)],
            [0.0, 1.0 / (half * sy), -(cy + oy) / (half * sy)],
            [0.0, 0.0, 1.0],
        ];
        // Turning the camera: a point p of the source plane is seen in the output at
        // the projection of R·(p, f). Output → source uses Rᵀ. Upright's turn first, then
        // the sliders'.
        let manual = camera(
            (-f64::from(t.vertical) / 100.0 * MAX_TILT_DEG).to_radians(),
            (f64::from(t.horizontal) / 100.0 * MAX_TILT_DEG).to_radians(),
            f64::from(t.rotate).to_radians(),
        );
        let [p, y, r] = t.upright_angles.map(|a| f64::from(a).to_radians());
        let rot = mul(&manual, &camera(p, y, r));
        let k: Mat3 = [[FOCAL, 0.0, 0.0], [0.0, FOCAL, 0.0], [0.0, 0.0, 1.0]];
        let k_inv: Mat3 = [[1.0 / FOCAL, 0.0, 0.0], [0.0, 1.0 / FOCAL, 0.0], [0.0, 0.0, 1.0]];
        // Keep the photo's centre at the frame's centre (turning the camera alone would move
        // it): the output point where the source centre lands becomes the origin.
        let (cx_out, cy_out) = (FOCAL * rot[0][2] / rot[2][2], FOCAL * rot[1][2] / rot[2][2]);
        let recentre: Mat3 = [[1.0, 0.0, cx_out], [0.0, 1.0, cy_out], [0.0, 0.0, 1.0]];
        let back = mul(&k, &mul(&transpose(&rot), &mul(&k_inv, &recentre)));
        // Normalised source → source pixels.
        let from_norm: Mat3 = [[half, 0.0, cx], [0.0, half, cy], [0.0, 0.0, 1.0]];
        mul(&from_norm, &mul(&back, &to_norm))
    }

    /// The source point (pixels) shown at output point (x, y) (pixels), if it is in front
    /// of the camera.
    #[inline]
    pub(super) fn source(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        apply(&self.h, x, y)
    }

    /// Where source point (u, v) (pixels) is shown in the output (pixels).
    pub(super) fn output(&self, u: f64, v: f64) -> Option<(f64, f64)> {
        apply(&inverse(&self.h), u, v)
    }
}

/// Constrain Crop: the largest rectangle of the frame's shape (`w × h`) inside both the
/// frame and the transformed photo, as (left, top, share of the frame kept). `frame` maps
/// output pixels to source pixels.
fn inscribed(frame: &Mat3, w: f64, h: f64) -> Option<(f64, f64, f64)> {
    let fwd = inverse(frame);
    let quad: Vec<(f64, f64)> = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
        .iter()
        .map(|&(x, y)| apply(&fwd, x, y))
        .collect::<Option<_>>()?;
    // A convex quad: inside means on the same side of every edge.
    let area: f64 = (0..4).map(|i| quad[i].0 * quad[(i + 1) % 4].1 - quad[(i + 1) % 4].0 * quad[i].1).sum();
    let inside = |x: f64, y: f64| {
        (0..4).all(|i| {
            let (a, b) = (quad[i], quad[(i + 1) % 4]);
            ((b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)) * area.signum() >= -1e-9
        })
    };
    let fits = |cx: f64, cy: f64, s: f64| {
        let (hw, hh) = (s * w / 2.0, s * h / 2.0);
        let corners = [(cx - hw, cy - hh), (cx + hw, cy - hh), (cx + hw, cy + hh), (cx - hw, cy + hh)];
        corners
            .iter()
            .all(|&(x, y)| x >= -1e-6 && y >= -1e-6 && x <= w + 1e-6 && y <= h + 1e-6 && inside(x, y))
    };
    let largest = |cx: f64, cy: f64| {
        let (mut lo, mut hi) = (0.0, 1.0);
        if fits(cx, cy, hi) {
            return hi;
        }
        for _ in 0..30 {
            let mid = (lo + hi) / 2.0;
            if fits(cx, cy, mid) { lo = mid } else { hi = mid }
        }
        lo
    };
    // Search centres: a coarse grid over the middle of the frame, then finer around the
    // best (the frame's centre wins ties).
    let mut best = (w / 2.0, h / 2.0, largest(w / 2.0, h / 2.0));
    for (span, steps) in [(0.4, 8), (0.06, 6)] {
        let (bx, by) = (best.0, best.1);
        for i in 0..=steps {
            for j in 0..=steps {
                let cx = bx + w * span * (i as f64 / steps as f64 - 0.5);
                let cy = by + h * span * (j as f64 / steps as f64 - 0.5);
                let s = largest(cx, cy);
                if s > best.2 + 1e-4 {
                    best = (cx, cy, s);
                }
            }
        }
    }
    let (cx, cy, s) = best;
    (s > 0.05).then(|| (cx - s * w / 2.0, cy - s * h / 2.0, s))
}

impl Transform {
    /// True when the photo's geometry is unchanged (Constrain Crop then keeps it all).
    pub fn is_identity(&self) -> bool {
        let d = Transform::default();
        self.upright_angles == [0.0; 3]
            && (self.vertical, self.horizontal, self.rotate, self.aspect, self.scale)
                == (d.vertical, d.horizontal, d.rotate, d.aspect, d.scale)
            && (self.x_offset, self.y_offset) == (d.x_offset, d.y_offset)
    }

    /// The developed photo's size for a `w × h` source (smaller with Constrain Crop).
    pub fn output_size(&self, w: u32, h: u32) -> (u32, u32) {
        if self.is_identity() { (w, h) } else { Mapping::new(self, w, h).out }
    }

    /// The point of the untransformed photo shown at (x, y) of the developed one (both
    /// 0..1, for a `w × h` source): where the White Balance Selector samples, where a guide
    /// drawn on the photo lies.
    pub fn source_point(&self, x: f32, y: f32, w: u32, h: u32) -> (f32, f32) {
        if self.is_identity() {
            return (x, y);
        }
        let m = Mapping::new(self, w, h);
        let (fw, fh) = (f64::from(w.max(1)), f64::from(h.max(1)));
        m.source(f64::from(x) * f64::from(m.out.0), f64::from(y) * f64::from(m.out.1))
            .map_or((x, y), |(u, v)| ((u / fw) as f32, (v / fh) as f32))
    }

    /// Where point (x, y) (0..1) of the untransformed photo is shown in the developed one
    /// (0..1, possibly outside), for a `w × h` source.
    pub fn output_point(&self, x: f32, y: f32, w: u32, h: u32) -> Option<(f32, f32)> {
        if self.is_identity() {
            return Some((x, y));
        }
        let m = Mapping::new(self, w, h);
        let (fw, fh) = (f64::from(w.max(1)), f64::from(h.max(1)));
        m.output(f64::from(x) * fw, f64::from(y) * fh)
            .map(|(u, v)| ((u / f64::from(m.out.0)) as f32, (v / f64::from(m.out.1)) as f32))
    }
}

/// The transformed image cached with the source, and how much of each output pixel the
/// transformed photo covers.
#[derive(Debug, Default)]
pub(super) struct TransformCache(Mutex<Option<(Transform, Arc<WorkingImage>)>>);

impl Clone for TransformCache {
    /// A copy starts without the cache (it is only a cache).
    fn clone(&self) -> Self {
        TransformCache::default()
    }
}

/// The image the pipeline renders: the source, or its transformed copy.
pub(super) enum Geometry<'a> {
    Same(&'a WorkingImage),
    Transformed(Arc<WorkingImage>),
}

impl std::ops::Deref for Geometry<'_> {
    type Target = WorkingImage;

    fn deref(&self) -> &WorkingImage {
        match self {
            Geometry::Same(img) => img,
            Geometry::Transformed(img) => img,
        }
    }
}

/// `img` with `t` applied (cached until `t` changes).
pub(super) fn transformed<'a>(img: &'a WorkingImage, t: &Transform) -> Geometry<'a> {
    if t.is_identity() {
        return Geometry::Same(img);
    }
    let mut cache = img.transform_cache.0.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((key, out)) = cache.as_ref()
        && key == t
    {
        return Geometry::Transformed(out.clone());
    }
    let out = Arc::new(warp(img, t));
    *cache = Some((t.clone(), out.clone()));
    Geometry::Transformed(out)
}

/// Resample the image through the Transform (bilinear). Outside the photo the nearest edge
/// pixel is repeated (so local adjustments see no hard border) and the coverage records
/// it, for the final white fill.
fn warp(img: &WorkingImage, t: &Transform) -> WorkingImage {
    let (w, h) = (img.width(), img.height());
    let map = Mapping::new(t, w, h);
    let src = img.pixels.as_raw();
    let (wu, hu) = (w as usize, h as usize);
    let (ow, oh) = (map.out.0 as usize, map.out.1 as usize);
    let mut pixels = vec![0.0f32; 3 * ow * oh];
    let mut coverage = vec![0.0f32; ow * oh];
    pixels.par_chunks_mut(3 * ow).zip(coverage.par_chunks_mut(ow)).enumerate().for_each(|(y, (row, cov))| {
        for (x, (o, c)) in row.as_chunks_mut::<3>().0.iter_mut().zip(cov.iter_mut()).enumerate() {
            let Some((u, v)) = map.source(x as f64 + 0.5, y as f64 + 0.5) else {
                *o = [0.0; 3];
                *c = 0.0;
                continue;
            };
            // Pixel centres at +0.5; how far outside the photo, in pixels, for a soft edge.
            let (u, v) = (u - 0.5, v - 0.5);
            let out_x = (-0.5 - u).max(u - (wu as f64 - 0.5)).max(0.0);
            let out_y = (-0.5 - v).max(v - (hu as f64 - 0.5)).max(0.0);
            *c = (1.0 - out_x.max(out_y)).clamp(0.0, 1.0) as f32;
            let u = u.clamp(0.0, (wu - 1) as f64);
            let v = v.clamp(0.0, (hu - 1) as f64);
            let (x0, y0) = (u as usize, v as usize);
            let (x1, y1) = ((x0 + 1).min(wu - 1), (y0 + 1).min(hu - 1));
            let (fx, fy) = ((u - x0 as f64) as f32, (v - y0 as f64) as f32);
            let at = |xx: usize, yy: usize| {
                let i = 3 * (yy * wu + xx);
                [src[i], src[i + 1], src[i + 2]]
            };
            let (a, b, cc, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
            *o = std::array::from_fn(|k| {
                let top = a[k] + fx * (b[k] - a[k]);
                let bottom = cc[k] + fx * (d[k] - cc[k]);
                top + fy * (bottom - top)
            });
        }
    });
    let pixels: Rgb32FImage = ImageBuffer::from_raw(map.out.0, map.out.1, pixels).expect("buffer size");
    // The full-size photo's developed size, as a full-size render of this Transform makes it.
    let full_size = t.output_size(img.full_size.0, img.full_size.1);
    WorkingImage {
        pixels,
        color: img.color.clone(),
        full_size,
        coverage: Some(Arc::new(coverage)),
        transform_cache: TransformCache::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(t: Transform) -> impl Fn(f64, f64) -> (f64, f64) {
        let m = Mapping::new(&t, 300, 200);
        move |x, y| m.source(x, y).unwrap()
    }

    #[test]
    fn identity_and_simple_moves() {
        let id = map(Transform::default());
        let (u, v) = id(37.0, 120.0);
        assert!((u - 37.0).abs() < 1e-9 && (v - 120.0).abs() < 1e-9);
        // Scale 150%: the output's corner shows a point a third of the way to the centre.
        let zoom = map(Transform { scale: 150.0, ..Default::default() });
        let (u, _) = zoom(0.0, 100.0);
        assert!((u - 50.0).abs() < 1e-6, "{u}");
        // X Offset +100 moves the photo right: the output's centre shows a point left of it.
        let shifted = map(Transform { x_offset: 100.0, ..Default::default() });
        assert!(shifted(150.0, 100.0).0 < 150.0);
        // Rotate clockwise: a point right of the centre in the output comes from above-right.
        let turned = map(Transform { rotate: 10.0, ..Default::default() });
        assert!(turned(250.0, 100.0).1 < 100.0);
    }

    #[test]
    fn keystones_widen_one_side() {
        // Vertical below 0 widens the top: the output's top corners show points nearer
        // the middle than the bottom corners do.
        let m = map(Transform { vertical: -50.0, ..Default::default() });
        let top = m(300.0, 0.0).0 - m(0.0, 0.0).0;
        let bottom = m(300.0, 200.0).0 - m(0.0, 200.0).0;
        assert!(top < bottom * 0.95, "{top} {bottom}");
        // Horizontal above 0 widens the right side.
        let m = map(Transform { horizontal: 50.0, ..Default::default() });
        let right = m(300.0, 200.0).1 - m(300.0, 0.0).1;
        let left = m(0.0, 200.0).1 - m(0.0, 0.0).1;
        assert!(right < left * 0.95, "{left} {right}");
        // The centre stays put.
        let t = Transform { vertical: -60.0, horizontal: 40.0, rotate: 5.0, ..Default::default() };
        let (u, v) = map(t)(150.0, 100.0);
        assert!((u - 150.0).abs() < 1e-6 && (v - 100.0).abs() < 1e-6, "{u} {v}");
    }

    #[test]
    fn mapping_round_trips_and_constrain_crop_stays_inside() {
        let t = Transform {
            vertical: -40.0,
            rotate: 4.0,
            upright_angles: [3.0, -2.0, 1.0],
            ..Default::default()
        };
        let m = Mapping::new(&t, 300, 200);
        let (u, v) = m.source(40.0, 170.0).unwrap();
        let (x, y) = m.output(u, v).unwrap();
        assert!((x - 40.0).abs() < 1e-6 && (y - 170.0).abs() < 1e-6);
        assert_eq!(m.out, (300, 200));

        // Cropped: smaller, same shape, and every output pixel shows the photo.
        let c = Transform { constrain_crop: true, ..t.clone() };
        let m = Mapping::new(&c, 300, 200);
        let (ow, oh) = m.out;
        assert!(ow < 300 && oh < 200 && ow > 150, "{ow}x{oh}");
        assert!((f64::from(ow) / f64::from(oh) - 1.5).abs() < 0.02);
        for (x, y) in [(0.0, 0.0), (f64::from(ow), 0.0), (0.0, f64::from(oh)), (f64::from(ow), f64::from(oh))]
        {
            let (u, v) = m.source(x, y).unwrap();
            assert!((-0.5..=300.5).contains(&u) && (-0.5..=200.5).contains(&v), "({x}, {y}) -> ({u}, {v})");
        }
        // Points map through the crop both ways, and the sizes agree with output_size.
        assert_eq!(c.output_size(300, 200), (ow, oh));
        let (sx, sy) = c.source_point(0.5, 0.5, 300, 200);
        let (ox, oy) = c.output_point(sx, sy, 300, 200).unwrap();
        assert!((ox - 0.5).abs() < 1e-4 && (oy - 0.5).abs() < 1e-4);
        // Nothing to crop without a transform.
        let none = Transform { constrain_crop: true, ..Default::default() };
        assert_eq!(none.output_size(300, 200), (300, 200));
    }
}
