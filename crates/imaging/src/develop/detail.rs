//! Detail (LrC's Detail panel): sharpening and noise reduction, done last, on the rendered
//! image's lightness and colour (Oklab L and a, b).
//!
//! Radii are in pixels of the full-size photo and scale with the image being rendered, so a
//! 1:1 view and an export agree; a small screen-sized preview shows a scaled-down effect,
//! as in LrC.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::local::box_blur_par;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Detail {
    /// Sharpening: Amount 0..150, Radius 0.5..3 px, Detail 0..100, Masking 0..100.
    pub sharpen_amount: f32,
    pub sharpen_radius: f32,
    pub sharpen_detail: f32,
    pub sharpen_masking: f32,
    /// Noise reduction, luminance: amount, Detail, Contrast (0..100).
    pub luminance: f32,
    pub luminance_detail: f32,
    pub luminance_contrast: f32,
    /// Noise reduction, colour: amount, Detail, Smoothness (0..100).
    pub color: f32,
    pub color_detail: f32,
    pub color_smoothness: f32,
}

impl Default for Detail {
    /// Nothing applied (the defaults of rendered files, and of settings saved before
    /// Detail existed).
    fn default() -> Self {
        Detail {
            sharpen_amount: 0.0,
            sharpen_radius: 1.0,
            sharpen_detail: 25.0,
            sharpen_masking: 0.0,
            luminance: 0.0,
            luminance_detail: 50.0,
            luminance_contrast: 0.0,
            color: 0.0,
            color_detail: 50.0,
            color_smoothness: 50.0,
        }
    }
}

impl Detail {
    /// LrC's defaults for RAW files: some sharpening and colour noise reduction (raw data
    /// is soft and has colour noise; rendered files had theirs done already).
    pub fn raw() -> Detail {
        Detail { sharpen_amount: 40.0, color: 25.0, ..Detail::default() }
    }

    pub fn is_active(&self) -> bool {
        self.sharpen_amount > 0.0 || self.luminance > 0.0 || self.color > 0.0
    }

    /// Pixels around a region the filters reach into, at `scale` (rendered pixels per
    /// full-size pixel).
    pub(super) fn margin(&self, scale: f32) -> usize {
        let sharpen = 3.0 * self.sharpen_radius;
        let color = colour_radius(self) as f32;
        ((sharpen.max(color).max(3.0) * scale).ceil() as usize) * 2 + 3 * self.grid(scale) + 2
    }

    /// The grid a region's filtered area must start on (colour noise reduction works in
    /// blocks), so a region renders exactly like the same part of the whole image.
    pub(super) fn grid(&self, scale: f32) -> usize {
        if self.color > 0.0 { colour_block(self, scale) } else { 1 }
    }
}

/// A Detail panel slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetailParam {
    SharpenAmount,
    SharpenRadius,
    SharpenDetail,
    SharpenMasking,
    Luminance,
    LuminanceDetail,
    LuminanceContrast,
    Color,
    ColorDetail,
    ColorSmoothness,
}

impl DetailParam {
    pub const SHARPENING: [DetailParam; 4] =
        [Self::SharpenAmount, Self::SharpenRadius, Self::SharpenDetail, Self::SharpenMasking];
    pub const NOISE: [DetailParam; 6] = [
        Self::Luminance,
        Self::LuminanceDetail,
        Self::LuminanceContrast,
        Self::Color,
        Self::ColorDetail,
        Self::ColorSmoothness,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::SharpenAmount => "Amount",
            Self::SharpenRadius => "Radius",
            Self::SharpenDetail | Self::LuminanceDetail | Self::ColorDetail => "Detail",
            Self::SharpenMasking => "Masking",
            Self::Luminance => "Luminance",
            Self::LuminanceContrast => "Contrast",
            Self::Color => "Color",
            Self::ColorSmoothness => "Smoothness",
        }
    }

    /// The History name's group ("Sharpening", "Noise Reduction").
    pub fn group(self) -> &'static str {
        if Self::SHARPENING.contains(&self) { "Sharpening" } else { "Noise Reduction" }
    }

    pub fn range(self) -> (f32, f32) {
        match self {
            Self::SharpenAmount => (0.0, 150.0),
            Self::SharpenRadius => (0.5, 3.0),
            _ => (0.0, 100.0),
        }
    }

    pub fn get(self, d: &Detail) -> f32 {
        *match self {
            Self::SharpenAmount => &d.sharpen_amount,
            Self::SharpenRadius => &d.sharpen_radius,
            Self::SharpenDetail => &d.sharpen_detail,
            Self::SharpenMasking => &d.sharpen_masking,
            Self::Luminance => &d.luminance,
            Self::LuminanceDetail => &d.luminance_detail,
            Self::LuminanceContrast => &d.luminance_contrast,
            Self::Color => &d.color,
            Self::ColorDetail => &d.color_detail,
            Self::ColorSmoothness => &d.color_smoothness,
        }
    }

    pub fn get_mut(self, d: &mut Detail) -> &mut f32 {
        match self {
            Self::SharpenAmount => &mut d.sharpen_amount,
            Self::SharpenRadius => &mut d.sharpen_radius,
            Self::SharpenDetail => &mut d.sharpen_detail,
            Self::SharpenMasking => &mut d.sharpen_masking,
            Self::Luminance => &mut d.luminance,
            Self::LuminanceDetail => &mut d.luminance_detail,
            Self::LuminanceContrast => &mut d.luminance_contrast,
            Self::Color => &mut d.color,
            Self::ColorDetail => &mut d.color_detail,
            Self::ColorSmoothness => &mut d.color_smoothness,
        }
    }
}

/// The colour noise reduction's reach in full-size pixels.
fn colour_radius(d: &Detail) -> usize {
    (3.0 + 9.0 * d.color_smoothness / 100.0).round() as usize
}

/// Gaussian-like blur of a plane: three box blurs for wide ones, a small kernel otherwise.
fn blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma >= 2.0 {
        // Three box passes of radius r approximate a Gaussian with σ² = 3·r(r+1)/3.
        let r = ((sigma * sigma + 0.25).sqrt() - 0.5).round().max(1.0) as usize;
        return box_blur_par(&box_blur_par(&box_blur_par(src, w, h, r), w, h, r), w, h, r);
    }
    let sigma = sigma.max(0.1);
    let radius = (sigma * 3.0).ceil() as isize;
    let kernel: Vec<f32> =
        (-radius..=radius).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.iter().map(|k| k / sum).collect();
    let at = |v: isize, n: usize| v.clamp(0, n as isize - 1) as usize;
    let mut rows = vec![0.0; src.len()];
    rows.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(out, line)| {
        for (x, o) in out.iter_mut().enumerate() {
            *o = kernel
                .iter()
                .enumerate()
                .map(|(k, c)| c * line[at(x as isize + k as isize - radius, w)])
                .sum();
        }
    });
    let mut out = vec![0.0; src.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, line)| {
        for (k, c) in kernel.iter().enumerate() {
            let row = &rows[at(y as isize + k as isize - radius, h) * w..][..w];
            line.iter_mut().zip(row).for_each(|(o, v)| *o += c * v);
        }
    });
    out
}

/// Guided filter (He et al.) with a plane as its own guide (edge-preserving smoothing):
/// `eps` sets how strong an edge must be to be kept. The guide's statistics are also the
/// input's, so it needs four box blurs instead of six.
fn self_guided(i: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    let (mi, mii) = guide_stats(i, w, h, r);
    let (a, b): (Vec<f32>, Vec<f32>) = (0..i.len())
        .into_par_iter()
        .map(|k| {
            let var = mii[k] - mi[k] * mi[k];
            let a = var / (var + eps);
            (a, mi[k] - a * mi[k])
        })
        .unzip();
    drop((mi, mii));
    let (ma, mb) = rayon::join(|| box_blur_par(&a, w, h, r), || box_blur_par(&b, w, h, r));
    (0..i.len()).into_par_iter().map(|k| ma[k] * i[k] + mb[k]).collect()
}

/// The colour noise reduction's block size (a "fast guided filter"): colour noise is
/// coarse, so the filter's coefficients are found on an image averaged in blocks, then
/// applied at full resolution against the full-resolution lightness.
fn colour_block(d: &Detail, scale: f32) -> usize {
    (colour_radius(d) as f32 * scale / 2.5).round().clamp(2.0, 4.0) as usize
}

/// Average `s × s` blocks of a plane (partial blocks at the right and bottom).
fn downsample(p: &[f32], w: usize, h: usize, s: usize) -> (Vec<f32>, usize, usize) {
    let (lw, lh) = (w.div_ceil(s), h.div_ceil(s));
    let mut out = vec![0.0; lw * lh];
    out.par_chunks_mut(lw).enumerate().for_each(|(v, line)| {
        let ys = v * s..((v + 1) * s).min(h);
        let rows = ys.len() as f32;
        for y in ys {
            let row = &p[y * w..(y + 1) * w];
            for (o, block) in line.iter_mut().zip(row.chunks(s)) {
                *o += block.iter().sum::<f32>();
            }
        }
        for (u, o) in line.iter_mut().enumerate() {
            *o /= rows * (((u + 1) * s).min(w) - u * s) as f32;
        }
    });
    (out, lw, lh)
}

/// Bilinear taps into a block-averaged axis of `low` samples for each of `n` pixels.
fn taps(n: usize, s: usize, low: usize) -> Vec<(usize, usize, f32)> {
    (0..n)
        .map(|x| {
            let f = ((x as f32 + 0.5) / s as f32 - 0.5).clamp(0.0, (low - 1) as f32);
            let u0 = f as usize;
            (u0, (u0 + 1).min(low - 1), f - u0 as f32)
        })
        .collect()
}

/// The guide's local mean and mean square, shared by filters with the same guide.
fn guide_stats(i: &[f32], w: usize, h: usize, r: usize) -> (Vec<f32>, Vec<f32>) {
    let ii: Vec<f32> = i.par_iter().map(|x| x * x).collect();
    rayon::join(|| box_blur_par(i, w, h, r), || box_blur_par(&ii, w, h, r))
}

/// The guided filter's smoothed linear coefficients: the output is `ma · i + mb`.
#[allow(clippy::too_many_arguments)]
fn guided_coeffs(
    i: &[f32],
    mi: &[f32],
    mii: &[f32],
    p: &[f32],
    w: usize,
    h: usize,
    r: usize,
    eps: f32,
) -> (Vec<f32>, Vec<f32>) {
    let ip: Vec<f32> = i.par_iter().zip(p).map(|(x, y)| x * y).collect();
    let (mp, mip) = rayon::join(|| box_blur_par(p, w, h, r), || box_blur_par(&ip, w, h, r));
    drop(ip);
    let (a, b): (Vec<f32>, Vec<f32>) = (0..i.len())
        .into_par_iter()
        .map(|k| {
            let var = mii[k] - mi[k] * mi[k];
            let cov = mip[k] - mi[k] * mp[k];
            let a = cov / (var + eps);
            (a, mp[k] - a * mi[k])
        })
        .unzip();
    rayon::join(|| box_blur_par(&a, w, h, r), || box_blur_par(&b, w, h, r))
}

/// Apply Detail to Oklab planes (L, a, b) of a `w × h` image rendered at `scale` rendered
/// pixels per full-size pixel.
pub(super) fn apply(planes: &mut [Vec<f32>; 3], w: usize, h: usize, d: &Detail, scale: f32) {
    let px = |full: f32| (full * scale).max(0.0);

    // Colour noise: smooth a and b, guided by lightness so colour edges along real edges
    // stay. Done in blocks (see `colour_block`); the planes must start on the block grid.
    if d.color > 0.0 {
        let s = colour_block(d, scale);
        let r = ((px(colour_radius(d) as f32) / s as f32).round() as usize).max(1);
        let eps = 0.0004 + 0.02 * (1.0 - d.color_detail / 100.0).powi(2);
        let k = d.color / 100.0;
        let [l, a, b] = planes;
        let l = &*l;
        let (ll, lw, lh) = downsample(l, w, h, s);
        let (mi, mii) = guide_stats(&ll, lw, lh, r);
        let (xt, yt) = (taps(w, s, lw), taps(h, s, lh));
        let smooth = |p: &mut Vec<f32>| {
            let (pl, ..) = downsample(p, w, h, s);
            let (ma, mb) = guided_coeffs(&ll, &mi, &mii, &pl, lw, lh, r, eps);
            p.par_chunks_mut(w).zip(l.par_chunks(w)).enumerate().for_each(|(y, (row, lrow))| {
                let (v0, v1, fy) = yt[y];
                let (r0, r1) = (v0 * lw, v1 * lw);
                for ((v, lv), &(u0, u1, fx)) in row.iter_mut().zip(lrow).zip(&xt) {
                    let at = |m: &[f32]| {
                        let top = m[r0 + u0] + fx * (m[r0 + u1] - m[r0 + u0]);
                        let bottom = m[r1 + u0] + fx * (m[r1 + u1] - m[r1 + u0]);
                        top + fy * (bottom - top)
                    };
                    let smooth = at(&ma) * lv + at(&mb);
                    *v += k * (smooth - *v);
                }
            });
        };
        rayon::join(|| smooth(a), || smooth(b));
    }

    // Luminance noise: an edge-preserving smoothing of L; Detail keeps finer structure,
    // Contrast brings some of the removed local contrast back.
    if d.luminance > 0.0 {
        let r = (px(2.0).round() as usize).max(1);
        let amount = d.luminance / 100.0;
        let eps = amount * amount * 0.003 * (0.25 + 1.5 * (1.0 - d.luminance_detail / 100.0));
        let smooth = self_guided(&planes[0], w, h, r, eps.max(1e-6));
        let back = 0.5 * d.luminance_contrast / 100.0;
        planes[0].par_iter_mut().zip(&smooth).for_each(|(v, s)| {
            let s = *v + amount.min(1.0) * (s - *v);
            *v = s + back * (*v - s);
        });
    }

    // Sharpening: unsharp mask on L. Detail limits halos (low Detail: small differences
    // only); Masking keeps it to edges.
    if d.sharpen_amount > 0.0 {
        let sigma = px(d.sharpen_radius);
        let blurred = blur(&planes[0], w, h, sigma);
        let k = 1.5 * d.sharpen_amount / 100.0;
        let limit = 0.02 + 0.3 * (d.sharpen_detail / 100.0).powi(2);
        let t0 = 0.12 * d.sharpen_masking / 100.0;
        let row = |y: usize| &blurred[y * w..(y + 1) * w];
        planes[0].par_chunks_mut(w).enumerate().for_each(|(y, line)| {
            let (up, here, down) = (row(y.saturating_sub(1)), row(y), row((y + 1).min(h - 1)));
            for (x, v) in line.iter_mut().enumerate() {
                let diff = *v - here[x];
                let soft = diff / (1.0 + diff.abs() / limit);
                // Masking: only where the (slightly blurred) lightness has an edge strong
                // enough; masking raises the bar.
                let m = if t0 > 0.0 {
                    let gx = here[(x + 1).min(w - 1)] - here[x.saturating_sub(1)];
                    let gy = down[x] - up[x];
                    let e = (gx * gx + gy * gy).sqrt() / scale.max(0.05);
                    let t = ((e - t0) / 0.04).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                } else {
                    1.0
                };
                *v = (*v + k * m * soft).max(0.0);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spread(v: &[f32]) -> f32 {
        let m = v.iter().sum::<f32>() / v.len() as f32;
        (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn small_blur_keeps_flat_areas_and_box_blur_matches_level() {
        let flat = vec![0.4; 64];
        assert!(blur(&flat, 8, 8, 0.8).iter().all(|v| (v - 0.4).abs() < 1e-5));
        assert!(blur(&flat, 8, 8, 4.0).iter().all(|v| (v - 0.4).abs() < 1e-5));
    }

    #[test]
    fn noise_reduction_smooths_and_sharpening_adds_contrast() {
        let (w, h) = (64, 64);
        // Mid-grey with pixel noise in L and in a.
        let noise = |i: usize| ((i * 2_654_435_761) % 97) as f32 / 97.0 - 0.5;
        let base = [
            (0..w * h).map(|i| 0.5 + 0.03 * noise(i)).collect::<Vec<f32>>(),
            (0..w * h).map(|i| 0.02 * noise(i + 7)).collect(),
            vec![0.0; w * h],
        ];
        let mut p = base.clone();
        apply(&mut p, w, h, &Detail { luminance: 80.0, color: 80.0, ..Detail::default() }, 1.0);
        assert!(spread(&p[0]) < spread(&base[0]) * 0.6, "luminance noise drops");
        assert!(spread(&p[1]) < spread(&base[1]) * 0.6, "colour noise drops");

        // A soft edge gets steeper with sharpening.
        let edge: Vec<f32> = (0..w * h).map(|i| if i % w < w / 2 { 0.3 } else { 0.7 }).collect();
        let soft = blur(&edge, w, h, 1.5);
        let mut s = [soft.clone(), vec![0.0; w * h], vec![0.0; w * h]];
        apply(&mut s, w, h, &Detail { sharpen_amount: 100.0, ..Detail::default() }, 1.0);
        let step = |v: &[f32]| v[w * 10 + w / 2] - v[w * 10 + w / 2 - 1];
        assert!(step(&s[0]) > step(&soft) * 1.2, "edge contrast rises");
        // Default settings do nothing.
        assert!(!Detail::default().is_active());
        assert!(Detail::raw().is_active());
    }
}
