//! Lens Corrections (LrC's panel): a lens profile's distortion, lateral chromatic
//! aberration (TCA) and vignetting, as calibrated in the Lensfun database; chromatic
//! aberration measured from the photo; and the Manual tab's distortion and vignetting.
//!
//! The geometry is applied in the Transform's warp, before the perspective and crop: an
//! output point goes through the Transform to a point of the corrected photo, and through
//! the lens map to the uncorrected source pixel, per colour channel when there is TCA.
//! Vignetting multiplies the sampled (linear) pixel.
//!
//! The profile's terms are Lensfun's, resolved for the photo's focal length, aperture and
//! distance (see the `lensdb` crate) and stored in the settings, so a photo keeps its look
//! without the database. Their units are Hugin's, as Lensfun 0.3 applies them:
//! - distortion and TCA: r = 1 at half the short side of the calibration camera's frame;
//! - vignetting: r = 1 at the calibration frame's corner;
//!
//! scaled to the photo by the two cameras' crop factors.

use serde::{Deserialize, Serialize};

/// A lens's distortion model (Lensfun's): the distorted radius from the corrected one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "model", rename_all = "lowercase")]
pub enum Distortion {
    /// Rd = Ru (1 − k1 + k1 Ru²).
    Poly3 { k1: f32 },
    /// Rd = Ru (1 + k1 Ru² + k2 Ru⁴).
    Poly5 { k1: f32, k2: f32 },
    /// Rd = Ru (a Ru³ + b Ru² + c Ru + 1 − a − b − c).
    Ptlens { a: f32, b: f32, c: f32 },
}

impl Distortion {
    /// Distorted / corrected radius at corrected radius `ru` (Hugin units).
    #[inline]
    pub fn ratio(&self, ru: f64) -> f64 {
        match *self {
            Distortion::Poly3 { k1 } => {
                let k1 = f64::from(k1);
                1.0 - k1 + k1 * ru * ru
            }
            Distortion::Poly5 { k1, k2 } => {
                let r2 = ru * ru;
                1.0 + f64::from(k1) * r2 + f64::from(k2) * r2 * r2
            }
            Distortion::Ptlens { a, b, c } => {
                let (a, b, c) = (f64::from(a), f64::from(b), f64::from(c));
                a * ru * ru * ru + b * ru * ru + c * ru + 1.0 - a - b - c
            }
        }
    }
}

/// Lateral chromatic aberration (Lensfun's TCA): where red and blue are, relative to green.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "model", rename_all = "lowercase")]
pub enum Tca {
    /// Rd = k Ru.
    Linear { kr: f32, kb: f32 },
    /// Rd = Ru (b Ru² + c Ru + v).
    Poly3 { vr: f32, cr: f32, br: f32, vb: f32, cb: f32, bb: f32 },
}

impl Tca {
    /// (red, blue) radius / green radius at green radius `r` (Hugin units).
    #[inline]
    fn ratios(&self, r: f64) -> (f64, f64) {
        match *self {
            Tca::Linear { kr, kb } => (f64::from(kr), f64::from(kb)),
            Tca::Poly3 { vr, cr, br, vb, cb, bb } => {
                let f = |v: f32, c: f32, b: f32| f64::from(b) * r * r + f64::from(c) * r + f64::from(v);
                (f(vr, cr, br), f(vb, cb, bb))
            }
        }
    }
}

/// Lensfun's "pa" vignetting: the source is darker by 1 + k1 r² + k2 r⁴ + k3 r⁶.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Vignetting {
    pub k1: f32,
    pub k2: f32,
    pub k3: f32,
}

/// A lens profile resolved for one photo: which lens, for what, and its terms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensProfile {
    pub maker: String,
    pub lens: String,
    /// The camera it was resolved for (shown; the crop factor is what counts).
    pub camera: String,
    /// What it was resolved for: focal length (mm), aperture (f-number), distance (m).
    pub focal: f32,
    pub aperture: f32,
    pub distance: f32,
    /// The photo's crop factor, and the calibration camera's crop factor and aspect.
    pub crop: f32,
    pub calib_crop: f32,
    pub calib_aspect: f32,
    /// The lens's optical centre, off the frame's centre in half short sides.
    pub center: [f32; 2],
    pub distortion: Option<Distortion>,
    pub tca: Option<Tca>,
    pub vignetting: Option<Vignetting>,
}

impl Default for LensProfile {
    fn default() -> Self {
        LensProfile {
            maker: String::new(),
            lens: String::new(),
            camera: String::new(),
            focal: 50.0,
            aperture: 8.0,
            distance: 1000.0,
            crop: 1.0,
            calib_crop: 1.0,
            calib_aspect: 1.5,
            center: [0.0; 2],
            distortion: None,
            tca: None,
            vignetting: None,
        }
    }
}

/// How the profile was chosen (LrC's Setup).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LensSetup {
    /// From the photo's metadata, again for each photo it is pasted to.
    #[default]
    Auto,
    /// Chosen by hand: the lens is kept, the focal length follows the photo.
    Custom,
}

/// The Lens Corrections panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensCorrections {
    /// Profile: Enable Profile Corrections, the setup, the resolved profile, and how much of
    /// its distortion and vignetting correction (0..200).
    pub profile_enable: bool,
    pub setup: LensSetup,
    pub profile: Option<LensProfile>,
    pub distortion_amount: f32,
    pub vignetting_amount: f32,
    /// Remove Chromatic Aberration, and what was measured: red and blue radius / green
    /// radius as `1 + s + q r²` (r = 1 at the frame's corner): `[s_red, q_red, s_blue,
    /// q_blue]`.
    pub remove_ca: bool,
    pub ca: Option<[f32; 4]>,
    /// Manual: Distortion −100..+100 (positive corrects barrel distortion), Lens Vignetting
    /// Amount −100..+100 (positive lightens the corners) and Midpoint 0..100.
    pub distortion: f32,
    pub vignette_amount: f32,
    pub vignette_midpoint: f32,
    /// Defringe: purple and green fringes along high-contrast edges, Amount 0..20 and the
    /// hue range (degrees: purple 30..70 of LrC's scale, green 40..60).
    pub purple_amount: f32,
    pub purple_hue: [f32; 2],
    pub green_amount: f32,
    pub green_hue: [f32; 2],
}

impl Default for LensCorrections {
    fn default() -> Self {
        LensCorrections {
            profile_enable: false,
            setup: LensSetup::Auto,
            profile: None,
            distortion_amount: 100.0,
            vignetting_amount: 100.0,
            remove_ca: false,
            ca: None,
            distortion: 0.0,
            vignette_amount: 0.0,
            vignette_midpoint: 50.0,
            purple_amount: 0.0,
            purple_hue: [30.0, 70.0],
            green_amount: 0.0,
            green_hue: [40.0, 60.0],
        }
    }
}

impl LensCorrections {
    /// The profile in use (enabled and resolved).
    pub fn active_profile(&self) -> Option<&LensProfile> {
        self.profile.as_ref().filter(|_| self.profile_enable)
    }

    /// The measured chromatic aberration in use.
    fn active_ca(&self) -> Option<[f32; 4]> {
        self.ca.filter(|_| self.remove_ca)
    }

    /// Moves pixels (distortion: the photo's geometry changes).
    pub fn moves_pixels(&self) -> bool {
        self.distortion != 0.0
            || self.active_profile().is_some_and(|p| p.distortion.is_some() && self.distortion_amount != 0.0)
    }

    /// Changes anything in the warp (geometry, colour channels' positions, brightness).
    pub fn warps(&self) -> bool {
        self.moves_pixels()
            || self.vignette_amount != 0.0
            || self.active_ca().is_some()
            || self
                .active_profile()
                .is_some_and(|p| p.tca.is_some() || (p.vignetting.is_some() && self.vignetting_amount != 0.0))
    }

    /// Defringe is on.
    pub fn defringes(&self) -> bool {
        self.purple_amount > 0.0 || self.green_amount > 0.0
    }
}

/// The lens corrections for a `w × h` image, ready to map points (pixels, centres at +0.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LensMap {
    cx: f64,
    cy: f64,
    /// Pixels → Hugin units (distortion, TCA) and vignetting units; pixels → corner units
    /// (the Manual sliders and the measured CA: r = 1 at the photo's corner).
    to_hugin: f64,
    to_vig: f64,
    to_corner: f64,
    distortion: Option<Distortion>,
    distortion_amount: f64,
    /// Manual distortion: Rd = Ru (1 + k Ru²), corner units.
    manual_k: f64,
    tca: Option<Tca>,
    ca: Option<[f64; 4]>,
    vignetting: Option<Vignetting>,
    vignetting_amount: f64,
    /// Manual vignetting: stops at the corner, and the falloff's exponent.
    manual_stops: f64,
    manual_power: f64,
}

/// How far Manual Distortion ±100 goes (k at the corner).
const MANUAL_K: f64 = 0.12;
/// Manual Lens Vignetting ±100: stops at the corner.
const MANUAL_STOPS: f64 = 1.5;
/// The full-frame diagonal, mm.
fn full_frame_diagonal() -> f64 {
    36f64.hypot(24.0)
}

impl LensMap {
    /// `None` when nothing would change.
    pub(super) fn new(l: &LensCorrections, w: u32, h: u32) -> Option<LensMap> {
        if !l.warps() {
            return None;
        }
        let (fw, fh) = (f64::from(w.max(1)), f64::from(h.max(1)));
        let diag_px = fw.hypot(fh);
        let p = l.active_profile();
        let (crop, calib_crop, calib_aspect, center) = p.map_or((1.0, 1.0, 1.5, [0.0; 2]), |p| {
            (
                f64::from(p.crop.max(0.01)),
                f64::from(p.calib_crop.max(0.01)),
                f64::from(p.calib_aspect.max(1.0)),
                p.center,
            )
        });
        let mm_per_px = full_frame_diagonal() / crop / diag_px;
        let hugin_mm = full_frame_diagonal() / calib_crop / calib_aspect.hypot(1.0) / 2.0;
        let vig_mm = full_frame_diagonal() / calib_crop / 2.0;
        let size = fw.min(fh);
        Some(LensMap {
            cx: fw / 2.0 + size / 2.0 * f64::from(center[0]),
            cy: fh / 2.0 + size / 2.0 * f64::from(center[1]),
            to_hugin: mm_per_px / hugin_mm,
            to_vig: mm_per_px / vig_mm,
            to_corner: 2.0 / diag_px,
            distortion: p.and_then(|p| p.distortion),
            distortion_amount: f64::from(l.distortion_amount) / 100.0,
            manual_k: f64::from(l.distortion) / 100.0 * MANUAL_K,
            tca: p.and_then(|p| p.tca),
            ca: l.active_ca().map(|c| c.map(f64::from)),
            vignetting: p.and_then(|p| p.vignetting).filter(|_| l.vignetting_amount != 0.0),
            vignetting_amount: f64::from(l.vignetting_amount) / 100.0,
            manual_stops: f64::from(l.vignette_amount) / 100.0 * MANUAL_STOPS,
            // Midpoint 50: r⁴; lower reaches further in (r²), higher only the corners (r⁸).
            manual_power: 2f64.powf(2.0 + (f64::from(l.vignette_midpoint) - 50.0) / 50.0),
        })
    }

    /// Distorted / corrected radius at corrected radius `r` (pixels from the centre).
    #[inline]
    fn radial(&self, r: f64) -> f64 {
        let mut ratio = 1.0;
        if let Some(d) = &self.distortion {
            ratio += self.distortion_amount * (d.ratio(r * self.to_hugin) - 1.0);
        }
        if self.manual_k != 0.0 {
            let rc = r * self.to_corner;
            ratio *= 1.0 + self.manual_k * rc * rc;
        }
        ratio
    }

    /// The source point for green at corrected point (x, y).
    #[inline]
    pub(super) fn source(&self, x: f64, y: f64) -> (f64, f64) {
        let (dx, dy) = (x - self.cx, y - self.cy);
        let k = self.radial(dx.hypot(dy));
        (self.cx + dx * k, self.cy + dy * k)
    }

    /// The source points of red, green and blue at corrected point (x, y).
    #[inline]
    pub(super) fn source_rgb(&self, x: f64, y: f64) -> [(f64, f64); 3] {
        let (gx, gy) = self.source(x, y);
        if self.tca.is_none() && self.ca.is_none() {
            return [(gx, gy); 3];
        }
        let (dx, dy) = (gx - self.cx, gy - self.cy);
        let r = dx.hypot(dy);
        let (mut kr, mut kb) = self.tca.map_or((1.0, 1.0), |t| t.ratios(r * self.to_hugin));
        if let Some([sr, qr, sb, qb]) = self.ca {
            let rc2 = (r * self.to_corner).powi(2);
            kr *= 1.0 + sr + qr * rc2;
            kb *= 1.0 + sb + qb * rc2;
        }
        [(self.cx + dx * kr, self.cy + dy * kr), (gx, gy), (self.cx + dx * kb, self.cy + dy * kb)]
    }

    /// The corrected point showing source point (u, v): the radial map inverted (Newton).
    pub(super) fn corrected(&self, u: f64, v: f64) -> Option<(f64, f64)> {
        let (dx, dy) = (u - self.cx, v - self.cy);
        let rd = dx.hypot(dy);
        if rd < 1e-9 {
            return Some((u, v));
        }
        let mut r = rd;
        for _ in 0..30 {
            let f = r * self.radial(r) - rd;
            if f.abs() < 1e-7 {
                break;
            }
            let e = (r * 1e-4).max(1e-6);
            let slope = ((r + e) * self.radial(r + e) - (r - e) * self.radial(r - e)) / (2.0 * e);
            if slope <= 1e-9 {
                return None;
            }
            r -= f / slope;
            if r <= 0.0 {
                return None;
            }
        }
        let k = r / rd;
        Some((self.cx + dx * k, self.cy + dy * k))
    }

    /// The vignetting correction's gain at source point (u, v).
    #[inline]
    pub(super) fn gain(&self, u: f64, v: f64) -> f32 {
        let r = (u - self.cx).hypot(v - self.cy);
        let mut g = 1.0;
        if let Some(vg) = &self.vignetting {
            let r2 = (r * self.to_vig).powi(2);
            let c =
                1.0 + f64::from(vg.k1) * r2 + f64::from(vg.k2) * r2 * r2 + f64::from(vg.k3) * r2 * r2 * r2;
            if c > 1e-3 {
                g *= (1.0 / c).powf(self.vignetting_amount);
            }
        }
        if self.manual_stops != 0.0 {
            let rc = (r * self.to_corner).min(1.5);
            g *= (self.manual_stops * rc.powf(self.manual_power)).exp2();
        }
        g as f32
    }

    pub(super) fn has_gain(&self) -> bool {
        self.vignetting.is_some() || self.manual_stops != 0.0
    }
}

/// Remove Chromatic Aberration: how much larger (or smaller) red and blue are than green
/// across the photo, measured along its edges (after the profile's own TCA, if any), as
/// [`LensCorrections::ca`]'s `[s_red, q_red, s_blue, q_blue]`.
///
/// At an edge, a channel drawn a little larger than green shows the edge a little further
/// out: the difference between the channels over green's slope across the edge is that
/// shift. Shifts of many edges fitted against the radius give the scale; the fit is
/// refined three times, each measuring what the previous one left.
pub fn measure_ca(img: &super::source::WorkingImage, lens: &LensCorrections) -> [f32; 4] {
    use rayon::prelude::*;
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w < 64 || h < 64 {
        return [0.0; 4];
    }
    let src = img.pixels.as_raw();
    let peak = src.iter().copied().fold(0.0f32, f32::max).max(1e-6);
    let sample = |x: f64, y: f64, c: usize| -> f32 {
        let x = (x - 0.5).clamp(0.0, (w - 1) as f64);
        let y = (y - 0.5).clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
        let at = |xx: usize, yy: usize| src[3 * (yy * w + xx) + c];
        let top = at(x0, y0) + fx * (at(x1, y0) - at(x0, y0));
        let bottom = at(x0, y1) + fx * (at(x1, y1) - at(x0, y1));
        top + fy * (bottom - top)
    };
    // The profile's TCA first (measured here is what it leaves).
    let profile_only = LensCorrections {
        remove_ca: false,
        ca: None,
        distortion: 0.0,
        vignette_amount: 0.0,
        distortion_amount: 0.0,
        vignetting_amount: 0.0,
        ..lens.clone()
    };
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let to_corner = 2.0 / (w as f64).hypot(h as f64);
    let step = ((w.max(h) / 1200).max(1)) * 2;
    let mut est = [0.0f64; 4];
    for _ in 0..3 {
        let mut current = profile_only.clone();
        current.remove_ca = true;
        current.ca = Some(est.map(|v| v as f32));
        let map = LensMap::new(&current, w as u32, h as u32);
        let points = |x: f64, y: f64| map.map_or([(x, y); 3], |m| m.source_rgb(x, y));
        // Normal equations per channel (red, blue): Δ = a + b rc², weighted by slope².
        let sums = (8..h - 8)
            .into_par_iter()
            .step_by(step)
            .map(|y| {
                let mut s = [[0.0f64; 5]; 2];
                for x in (8..w - 8).step_by(step) {
                    let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                    let (dx, dy) = (px - cx, py - cy);
                    let r = dx.hypot(dy);
                    let rc = r * to_corner;
                    if rc < 0.2 {
                        continue;
                    }
                    let (ux, uy) = (dx / r, dy / r);
                    let pts = points(px, py);
                    // Green's slope across the edge (along the radius), and its level.
                    let g = |t: f64| sample(pts[1].0 + ux * t, pts[1].1 + uy * t, 1);
                    let slope = f64::from(g(1.0) - g(-1.0)) / 2.0;
                    let mean_g: f32 = (-4..=4).map(|t| g(f64::from(t))).sum::<f32>() / 9.0;
                    if mean_g < 1e-4 || slope.abs() < 0.03 * f64::from(mean_g) {
                        continue;
                    }
                    if [-2.0, 0.0, 2.0]
                        .iter()
                        .any(|&t| (0..3).any(|c| sample(px + ux * t, py + uy * t, c) > 0.92 * peak))
                    {
                        continue;
                    }
                    for (k, c) in [(0usize, 0usize), (1, 2)] {
                        let ch = |t: f64| sample(pts[c].0 + ux * t, pts[c].1 + uy * t, c);
                        // Across the edge (±6 px): green = κ · channel + δ · green's slope,
                        // solved for the level κ and the shift δ together (pixels, the
                        // channel's edge further out when positive).
                        let (mut aa, mut ab, mut bb, mut ay, mut by) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
                        for t in -6..=6 {
                            let t = f64::from(t);
                            let a = f64::from(ch(t));
                            let b = f64::from(g(t + 1.0) - g(t - 1.0)) / 2.0;
                            let yv = f64::from(g(t));
                            (aa, ab, bb, ay, by) =
                                (aa + a * a, ab + a * b, bb + b * b, ay + a * yv, by + b * yv);
                        }
                        let det = aa * bb - ab * ab;
                        if det.abs() < 1e-12 * aa * bb.max(1e-12) || aa < 1e-10 {
                            continue;
                        }
                        let shift = (aa * by - ab * ay) / det;
                        let d = shift / r;
                        if d.abs() > 0.01 {
                            continue;
                        }
                        let wgt = bb / f64::from(mean_g * mean_g);
                        let q = rc * rc;
                        let e = &mut s[k];
                        e[0] += wgt;
                        e[1] += wgt * q;
                        e[2] += wgt * q * q;
                        e[3] += wgt * d;
                        e[4] += wgt * d * q;
                    }
                }
                s
            })
            .reduce(
                || [[0.0; 5]; 2],
                |a, b| std::array::from_fn(|k| std::array::from_fn(|i| a[k][i] + b[k][i])),
            );
        for (k, e) in sums.iter().enumerate() {
            let det = e[0] * e[2] - e[1] * e[1];
            if e[0] < 1e-6 || det.abs() < 1e-12 {
                continue;
            }
            let a = (e[3] * e[2] - e[4] * e[1]) / det;
            let b = (e[0] * e[4] - e[1] * e[3]) / det;
            est[2 * k] += a;
            est[2 * k + 1] += b;
        }
    }
    est.map(|v| v.clamp(-0.01, 0.01) as f32)
}

/// Defringe (LrC's): purple and green fringes along high-contrast edges (longitudinal
/// chromatic aberration) lose their colour. Each pixel's hue (Oklab) is tested against the
/// two ranges; the local contrast around it (brightest / darkest within a few pixels, in
/// stops) says whether it is at such an edge.
pub(super) struct Defringe {
    /// Contrast (stops) over the region, row by row.
    contrast: Vec<f32>,
    region: super::pipeline::Region,
    /// (amount 0..1, hue from, hue to) for purple and green, Oklab degrees.
    ranges: [(f32, f32, f32); 2],
    to_srgb: super::matrix::Mat3f,
    to_work: super::matrix::Mat3f,
}

/// The sliders' hue scales → Oklab hue: purple 0..100 is 240°..360° (blue to magenta-red),
/// green 0..100 is 90°..240° (yellow-green to blue).
fn purple_hue(v: f32) -> f32 {
    240.0 + v * 1.2
}
fn green_hue(v: f32) -> f32 {
    90.0 + v * 1.5
}

impl Defringe {
    /// For `region` of a `w × h` image whose luminance (scene-linear, any scale) at a pixel
    /// is `lum`; `scale` is rendered pixels per full-size pixel.
    pub(super) fn new(
        l: &LensCorrections,
        w: usize,
        h: usize,
        region: super::pipeline::Region,
        scale: f32,
        lum: impl Fn(usize, usize) -> f32 + Sync,
    ) -> Option<Defringe> {
        use rayon::prelude::*;
        if !l.defringes() {
            return None;
        }
        // The fringe's reach: about 1..7 full-size pixels for Amount 1..20.
        let amount = l.purple_amount.max(l.green_amount);
        let r = ((1.0 + amount * 0.3) * scale).round().max(1.0) as usize;
        let (rx, ry, rw, rh) = (region.x as usize, region.y as usize, region.w as usize, region.h as usize);
        let (x0, y0) = (rx.saturating_sub(r), ry.saturating_sub(r));
        let (x1, y1) = ((rx + rw + r).min(w), (ry + rh + r).min(h));
        let (ew, eh) = (x1 - x0, y1 - y0);
        let logs: Vec<f32> = (y0..y1)
            .into_par_iter()
            .flat_map_iter(|y| (x0..x1).map(move |x| (y, x)))
            .map(|(y, x)| lum(x, y).max(1e-5).log2())
            .collect();
        // Brightest and darkest within r: rows, then columns.
        let window = |v: &[f32], n: usize, stride: usize, i: usize, max: bool| {
            let (lo, hi) = (i.saturating_sub(r), (i + r).min(n - 1));
            let mut m = v[lo * stride];
            for k in lo..=hi {
                let x = v[k * stride];
                m = if max { m.max(x) } else { m.min(x) };
            }
            m
        };
        let mut hmax = vec![0.0f32; ew * eh];
        let mut hmin = vec![0.0f32; ew * eh];
        hmax.par_chunks_mut(ew).zip(hmin.par_chunks_mut(ew)).enumerate().for_each(|(j, (a, b))| {
            let row = &logs[j * ew..(j + 1) * ew];
            for i in 0..ew {
                a[i] = window(row, ew, 1, i, true);
                b[i] = window(row, ew, 1, i, false);
            }
        });
        let mut contrast = vec![0.0f32; rw * rh];
        contrast.par_chunks_mut(rw).enumerate().for_each(|(j, out)| {
            let yy = ry + j - y0;
            for (i, o) in out.iter_mut().enumerate() {
                let xx = rx + i - x0;
                let col_max = window(&hmax[xx..], eh, ew, yy, true);
                let col_min = window(&hmin[xx..], eh, ew, yy, false);
                *o = col_max - col_min;
            }
        });
        let range = |amount: f32, hue: [f32; 2], f: fn(f32) -> f32| {
            ((amount / 20.0).clamp(0.0, 1.0), f(hue[0]), f(hue[1]))
        };
        Some(Defringe {
            contrast,
            region,
            ranges: [
                range(l.purple_amount, l.purple_hue, purple_hue),
                range(l.green_amount, l.green_hue, green_hue),
            ],
            to_srgb: super::matrix::to_f32(&super::matrix::work_to_srgb()),
            to_work: super::matrix::to_f32(&super::matrix::srgb_to_work()),
        })
    }

    /// A display-referred colour at (x, y) with its fringe colour taken out.
    #[inline]
    pub(super) fn apply(&self, d: [f32; 3], x: usize, y: usize) -> [f32; 3] {
        let r = &self.region;
        let (rx, ry) = (r.x as usize, r.y as usize);
        if x < rx || y < ry || x >= rx + r.w as usize || y >= ry + r.h as usize {
            return d;
        }
        // At an edge: from 1.5 stops of contrast, fully from 3.
        let c = self.contrast[(y - ry) * r.w as usize + (x - rx)];
        let edge = ((c - 1.5) / 1.5).clamp(0.0, 1.0);
        if edge <= 0.0 {
            return d;
        }
        let lab = super::hsl::srgb_to_oklab(super::matrix::apply(&self.to_srgb, d));
        let (hue, chroma) = super::hsl::hue_chroma(lab);
        if chroma < 1e-4 {
            return d;
        }
        // In a range (eased over 10° at its ends), by its amount.
        let mut k = 0.0f32;
        for &(amount, from, to) in &self.ranges {
            if amount <= 0.0 {
                continue;
            }
            let h = if hue < from - 20.0 { hue + 360.0 } else { hue };
            let inside = ((h - from + 10.0) / 10.0).min((to + 10.0 - h) / 10.0).clamp(0.0, 1.0);
            k = k.max(inside * amount.sqrt());
        }
        let k = (k * edge).min(1.0);
        if k <= 0.0 {
            return d;
        }
        let out = super::hsl::oklab_to_srgb([lab[0], lab[1] * (1.0 - k), lab[2] * (1.0 - k)]);
        super::matrix::apply(&self.to_work, out).map(|v| v.max(0.0))
    }
}

/// For tools and checks: the source points of red, green and blue shown at corrected point
/// (x, y) of a `w × h` photo (pixels, centres at +0.5), and the vignetting gain there.
pub fn lens_correction_at(l: &LensCorrections, w: u32, h: u32, x: f64, y: f64) -> ([(f64, f64); 3], f32) {
    match LensMap::new(l, w, h) {
        Some(m) => {
            let p = m.source_rgb(x, y);
            (p, m.gain(p[1].0, p[1].1))
        }
        None => ([(x, y); 3], 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_profile(p: LensProfile) -> LensCorrections {
        LensCorrections { profile_enable: true, profile: Some(p), ..Default::default() }
    }

    #[test]
    fn nothing_on_is_no_map() {
        assert!(LensMap::new(&LensCorrections::default(), 300, 200).is_none());
        // A profile without Enable, or measured CA without Remove CA, does nothing.
        let mut l = with_profile(LensProfile {
            distortion: Some(Distortion::Poly3 { k1: 0.1 }),
            ..Default::default()
        });
        l.profile_enable = false;
        l.ca = Some([0.001, 0.0, -0.001, 0.0]);
        assert!(LensMap::new(&l, 300, 200).is_none());
    }

    #[test]
    fn distortion_maps_and_inverts() {
        let l = with_profile(LensProfile {
            distortion: Some(Distortion::Ptlens { a: 0.01436, b: -0.06611, c: 0.06436 }),
            ..Default::default()
        });
        let m = LensMap::new(&l, 3000, 2000).unwrap();
        // The centre stays; the middle of the long edge (r = 1 in Hugin units) too.
        let (u, v) = m.source(1500.0, 1000.0);
        assert!((u - 1500.0).abs() < 1e-9 && (v - 1000.0).abs() < 1e-9);
        let (u, v) = m.source(1500.0, 0.0);
        assert!((u - 1500.0).abs() < 1e-9 && v.abs() < 1e-6, "{u} {v}");
        // Elsewhere points move, and the inverse finds them again.
        for (x, y) in [(10.0, 10.0), (2900.0, 400.0), (700.0, 1900.0)] {
            let (u, v) = m.source(x, y);
            assert!((u - x).abs() + (v - y).abs() > 0.1);
            let (bx, by) = m.corrected(u, v).unwrap();
            assert!((bx - x).abs() < 1e-4 && (by - y).abs() < 1e-4, "{x},{y} → {bx},{by}");
        }
        // Amount 0: nothing moves.
        let none = LensCorrections { distortion_amount: 0.0, ..l.clone() };
        assert!(LensMap::new(&none, 3000, 2000).is_none());
    }

    #[test]
    fn vignetting_lightens_the_corners_only() {
        let l = with_profile(LensProfile {
            vignetting: Some(Vignetting { k1: -0.3, k2: 0.0, k3: 0.0 }),
            ..Default::default()
        });
        let m = LensMap::new(&l, 3000, 2000).unwrap();
        assert!((m.gain(1500.0, 1000.0) - 1.0).abs() < 1e-6);
        // The corner: r = 1 (same crop as the calibration), so 1 / (1 − 0.3).
        assert!((m.gain(0.0, 0.0) - 1.0 / 0.7).abs() < 1e-4, "{}", m.gain(0.0, 0.0));
        let half = LensCorrections { vignetting_amount: 50.0, ..l };
        let mh = LensMap::new(&half, 3000, 2000).unwrap();
        assert!((mh.gain(0.0, 0.0) - (1.0f32 / 0.7).sqrt()).abs() < 1e-4);
    }

    #[test]
    fn tca_moves_red_and_blue_only() {
        let l = with_profile(LensProfile {
            tca: Some(Tca::Linear { kr: 1.001, kb: 0.999 }),
            ..Default::default()
        });
        let m = LensMap::new(&l, 3000, 2000).unwrap();
        let [r, g, b] = m.source_rgb(100.0, 1000.0);
        assert_eq!(g, (100.0, 1000.0));
        assert!((r.0 - (1500.0 - 1400.0 * 1.001)).abs() < 1e-3, "{r:?}");
        assert!((b.0 - (1500.0 - 1400.0 * 0.999)).abs() < 1e-3, "{b:?}");
    }

    #[test]
    fn chromatic_aberration_is_measured() {
        // Rings and spokes (edges both ways), red drawn 0.2 % larger and blue 0.15 % smaller
        // than green.
        let (w, h) = (900u32, 600u32);
        let (cx, cy) = (450.0, 300.0);
        let pattern = |x: f64, y: f64| {
            let (dx, dy) = (x - cx, y - cy);
            let r = dx.hypot(dy);
            let ring = (r / 23.0).sin();
            let spoke = (dy.atan2(dx) * 12.0).sin();
            let v = 0.5 + 0.3 * (ring * 6.0).tanh() + 0.1 * (spoke * 6.0).tanh();
            v.clamp(0.05, 0.9) as f32
        };
        let (kr, kb) = (1.002, 0.9985);
        let at = |x: f64, y: f64, k: f64| pattern(cx + (x - cx) / k, cy + (y - cy) / k);
        let pixels = image::ImageBuffer::from_fn(w, h, |x, y| {
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            image::Rgb([at(px, py, kr) * 0.8, at(px, py, 1.0), at(px, py, kb) * 1.2])
        });
        let img = super::super::source::WorkingImage::for_tests(pixels);
        let ca = measure_ca(&img, &LensCorrections::default());
        // A constant scale: s close to k − 1, little r² term.
        let (red, blue) =
            (f64::from(ca[0]) + f64::from(ca[1]) * 0.5, f64::from(ca[2]) + f64::from(ca[3]) * 0.5);
        assert!((red - 0.002).abs() < 0.0003, "{ca:?}");
        assert!((blue + 0.0015).abs() < 0.0003, "{ca:?}");
        // And applying it lines the channels up: red's source at a corner point is green's
        // scaled by about kr.
        let l = LensCorrections { remove_ca: true, ca: Some(ca), ..Default::default() };
        let m = LensMap::new(&l, w, h).unwrap();
        let [r, g, _] = m.source_rgb(850.0, 500.0);
        let ratio = (r.0 - cx) / (g.0 - cx);
        assert!((ratio - kr).abs() < 0.0004, "{ratio}");
    }

    #[test]
    fn defringe_takes_purple_out_at_edges_only() {
        use super::super::pipeline::Region;
        // Left half dark, right half bright: a hard edge at x = 50.
        let (w, h) = (100usize, 40usize);
        let lum = |x: usize, _y: usize| if x < 50 { 0.02 } else { 0.9 };
        let l = LensCorrections { purple_amount: 15.0, ..Default::default() };
        let region = Region { x: 0, y: 0, w: w as u32, h: h as u32 };
        let f = Defringe::new(&l, w, h, region, 1.0, lum).unwrap();
        // A purple in working space (via sRGB).
        let to_work = super::super::matrix::to_f32(&super::super::matrix::srgb_to_work());
        let purple = super::super::matrix::apply(&to_work, [0.35, 0.1, 0.5]);
        let chroma = |c: [f32; 3]| {
            let to_srgb = super::super::matrix::to_f32(&super::super::matrix::work_to_srgb());
            super::super::hsl::hue_chroma(super::super::hsl::srgb_to_oklab(super::super::matrix::apply(
                &to_srgb, c,
            )))
            .1
        };
        let at_edge = f.apply(purple, 50, 20);
        let flat = f.apply(purple, 10, 20);
        assert!(chroma(at_edge) < 0.3 * chroma(purple), "{} {}", chroma(at_edge), chroma(purple));
        assert!((chroma(flat) - chroma(purple)).abs() < 1e-4);
        // A green at the edge is left alone (green Amount is 0).
        let green = super::super::matrix::apply(&to_work, [0.1, 0.5, 0.2]);
        assert!((chroma(f.apply(green, 50, 20)) - chroma(green)).abs() < 1e-4);
    }

    #[test]
    fn manual_sliders_work_without_a_profile() {
        let l = LensCorrections { distortion: 50.0, vignette_amount: -50.0, ..Default::default() };
        let m = LensMap::new(&l, 3000, 2000).unwrap();
        // Positive distortion looks further out at the edges (corrects barrel).
        let (u, _) = m.source(0.0, 1000.0);
        assert!(u < 0.0);
        assert!(m.gain(0.0, 0.0) < 1.0 && (m.gain(1500.0, 1000.0) - 1.0).abs() < 1e-6);
    }
}
