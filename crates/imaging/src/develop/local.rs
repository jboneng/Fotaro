//! Local tone: a smoothed, edge-preserving map of how bright each part of the photo is
//! (log2 luminance), which Highlights, Shadows and Clarity work from; and the small blur
//! Texture works from.
//!
//! Built at low resolution (about 384 px) with a self-guided filter, so it follows large
//! edges (a bright sky against a dark building) without halos, and costs a few
//! milliseconds; each full-size pixel samples it bilinearly.

use rayon::prelude::*;

/// How a map is built.
#[derive(Debug, Clone, Copy)]
pub(super) struct MapSpec {
    /// Long side of the map.
    pub px: usize,
    /// Smoothing radius as a fraction of the image's long side (1/24: large areas).
    pub radius_div: usize,
    /// Guided filter regularization (stops²): edges with more contrast than about √eps
    /// stops are kept.
    pub eps: f32,
}

/// Highlights / Shadows: large areas.
pub(super) const TONE_MAP: MapSpec = MapSpec { px: 384, radius_div: 24, eps: 0.25 };
/// Clarity: mid-sized structure (a face, a tree), with edges kept more strictly.
pub(super) const CLARITY_MAP: MapSpec = MapSpec { px: 900, radius_div: 70, eps: 0.08 };

pub(super) struct BaseMap {
    w: usize,
    h: usize,
    /// Full-size pixels per map pixel.
    factor: f32,
    /// Smoothed log2 luminance.
    data: Vec<f32>,
}

/// Box filter of radius `r` along rows, then columns (running sums, clamped edges); the
/// straightforward version, which [`box_blur_par`] is tested against.
#[cfg(test)]
fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], w: usize, h: usize, stride_x: usize, stride_y: usize| -> Vec<f32> {
        // Blur along x for each of h lines; element (x, y) at y*stride_y + x*stride_x.
        let mut out = vec![0.0; src.len()];
        for y in 0..h {
            let at = |x: usize| src[y * stride_y + x * stride_x];
            let mut sum = 0.0;
            let mut n = 0.0;
            for x in 0..=r.min(w - 1) {
                sum += at(x);
                n += 1.0;
            }
            for x in 0..w {
                out[y * stride_y + x * stride_x] = sum / n;
                if x + r + 1 < w {
                    sum += at(x + r + 1);
                    n += 1.0;
                }
                if x >= r {
                    sum -= at(x - r);
                    n -= 1.0;
                }
            }
        }
        out
    };
    let rows = pass(src, w, h, 1, w);
    pass(&rows, h, w, w, 1)
}

/// The same box filter on a full-size image: rows in parallel, then columns with a
/// running sum per column (cache-friendly).
pub(super) fn box_blur_par(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut rows = vec![0.0; src.len()];
    rows.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(out, line)| {
        let mut sum = 0.0;
        let mut n = 0.0;
        for v in &line[..=r.min(w - 1)] {
            sum += v;
            n += 1.0;
        }
        for x in 0..w {
            out[x] = sum / n;
            if x + r + 1 < w {
                sum += line[x + r + 1];
                n += 1.0;
            }
            if x >= r {
                sum -= line[x - r];
                n -= 1.0;
            }
        }
    });
    let mut out = vec![0.0; src.len()];
    let mut sum = vec![0.0f32; w];
    let mut n = 0.0f32;
    for y in 0..=r.min(h - 1) {
        sum.iter_mut().zip(&rows[y * w..(y + 1) * w]).for_each(|(s, v)| *s += v);
        n += 1.0;
    }
    for y in 0..h {
        let inv = 1.0 / n;
        out[y * w..(y + 1) * w].iter_mut().zip(&sum).for_each(|(o, s)| *o = s * inv);
        if y + r + 1 < h {
            let add = &rows[(y + r + 1) * w..(y + r + 2) * w];
            sum.iter_mut().zip(add).for_each(|(s, v)| *s += v);
            n += 1.0;
        }
        if y >= r {
            let sub = &rows[(y - r) * w..(y - r + 1) * w];
            sum.iter_mut().zip(sub).for_each(|(s, v)| *s -= v);
            n -= 1.0;
        }
    }
    out
}

impl BaseMap {
    /// Build from `luminance(x, y)`: linear luminance of full-size pixel (x, y) of a
    /// `width × height` image.
    pub fn build(
        width: usize,
        height: usize,
        spec: MapSpec,
        luminance: impl Fn(usize, usize) -> f32 + Sync,
    ) -> BaseMap {
        let factor = (width.max(height) as f32 / spec.px as f32).max(1.0);
        let f = factor.ceil() as usize;
        let (w, h) = (width.div_ceil(f), height.div_ceil(f));
        let step = (f / 3).max(1);
        // Average linear luminance over each cell, then log.
        let data: Vec<f32> = (0..h)
            .into_par_iter()
            .flat_map_iter(|j| {
                let mut sums = vec![0f64; w];
                let mut counts = vec![0u32; w];
                // Every `step`-th pixel is plenty for an average over an f × f cell.
                for y in (j * f..((j + 1) * f).min(height)).step_by(step) {
                    for x in (0..width).step_by(step) {
                        sums[x / f] += f64::from(luminance(x, y));
                        counts[x / f] += 1;
                    }
                }
                sums.into_iter()
                    .zip(counts)
                    .map(|(s, n)| ((s / f64::from(n.max(1))) as f32).max(1e-6).log2())
                    .collect::<Vec<_>>()
            })
            .collect();
        // Self-guided filter (He et al.): smooth inside regions, keep strong edges.
        let r = (w.max(h) / spec.radius_div).max(2);
        let mean = box_blur_par(&data, w, h, r);
        let sq: Vec<f32> = data.iter().map(|v| v * v).collect();
        let mean_sq = box_blur_par(&sq, w, h, r);
        let (a, b): (Vec<f32>, Vec<f32>) = mean
            .iter()
            .zip(&mean_sq)
            .map(|(m, m2)| {
                let var = (m2 - m * m).max(0.0);
                let a = var / (var + spec.eps);
                (a, m - a * m)
            })
            .unzip();
        let (ma, mb) = (box_blur_par(&a, w, h, r), box_blur_par(&b, w, h, r));
        let data = data.iter().zip(ma.iter().zip(&mb)).map(|(i, (a, b))| a * i + b).collect();
        BaseMap { w, h, factor: f as f32, data }
    }

    /// Replace every value v with `f(v)` (e.g. turn brightness into a gain once per cell
    /// rather than once per pixel).
    pub fn transform(&mut self, f: impl Fn(f32) -> f32 + Sync) {
        self.data.par_iter_mut().for_each(|v| *v = f(*v));
    }

    /// The map at full-size pixel (x, y), bilinear.
    #[inline]
    pub fn sample(&self, x: usize, y: usize) -> f32 {
        let fx = ((x as f32 + 0.5) / self.factor - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = ((y as f32 + 0.5) / self.factor - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| self.data[y * self.w + x];
        let top = at(x0, y0) + tx * (at(x1, y0) - at(x0, y0));
        let bottom = at(x0, y1) + tx * (at(x1, y1) - at(x0, y1));
        top + ty * (bottom - top)
    }
}

/// Highlights / Shadows as a gain in stops for a neighbourhood at `base` stops relative
/// to middle grey. `highlights`, `shadows`: −1..+1.
#[inline]
pub(super) fn tone_gain(base: f32, highlights: f32, shadows: f32) -> f32 {
    let w_shadow = 1.0 / (1.0 + (1.6 * (base + 1.5)).exp());
    let w_high = 1.0 / (1.0 + (-1.6 * (base - 1.2)).exp());
    1.5 * (shadows * w_shadow + highlights * w_high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_strong_edges_and_smooths_texture() {
        // Left half dark (with fine texture), right half bright.
        let (w, h) = (800, 400);
        let lum =
            |x: usize, y: usize| if x < w / 2 { 0.01 * (1.0 + 0.3 * ((x + y) % 2) as f32) } else { 0.8 };
        let m = BaseMap::build(w, h, TONE_MAP, lum);
        let dark = m.sample(100, 200);
        let bright = m.sample(700, 200);
        assert!((dark - 0.0115f32.log2()).abs() < 0.3, "dark side {dark}");
        assert!((bright - 0.8f32.log2()).abs() < 0.3, "bright side {bright}");
        // Near the edge each side stays close to its own level (no wide halo).
        assert!(m.sample(w / 2 - 30, 200) < 0.0115f32.log2() + 1.5);
        assert!(m.sample(w / 2 + 30, 200) > 0.8f32.log2() - 1.5);
    }

    #[test]
    fn gains_hit_their_own_tones() {
        let grey = 0.0;
        let deep = -4.0;
        let white = 2.4;
        assert!(tone_gain(deep, 0.0, 1.0) > 1.2);
        assert!(tone_gain(white, 0.0, 1.0) < 0.05);
        assert!(tone_gain(white, -1.0, 0.0) < -1.1);
        assert!(tone_gain(deep, -1.0, 0.0).abs() < 0.05);
        assert!(tone_gain(grey, -1.0, 1.0).abs() < 0.3);
    }
}

#[cfg(test)]
mod blur_tests {
    use super::*;

    #[test]
    fn parallel_blur_matches_the_simple_one() {
        let (w, h) = (37, 23);
        let src: Vec<f32> = (0..w * h).map(|i| ((i * 7919) % 101) as f32).collect();
        for r in [1, 3, 30] {
            let a = box_blur(&src, w, h, r);
            let b = box_blur_par(&src, w, h, r);
            assert!(a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-3), "radius {r}");
        }
    }
}

/// log2, fast (about 1e-4 error): for gains, where libm's precision isn't needed.
#[inline]
pub(super) fn fast_log2(x: f32) -> f32 {
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xff) as i32 - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    // log2(m) for m in [1, 2): 2/ln 2 · atanh(t) with t = (m − 1)/(m + 1) ≤ 1/3.
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let p = 2.885_39 * t * (1.0 + t2 * (1.0 / 3.0 + t2 * (0.2 + t2 * (1.0 / 7.0 + t2 / 9.0))));
    e as f32 + p
}

/// 2^x, fast (about 1e-5 relative error).
#[inline]
pub(super) fn fast_exp2(x: f32) -> f32 {
    let x = x.clamp(-126.0, 126.0);
    let xi = x.floor();
    let f = x - xi;
    // Taylor series of 2^f = e^(f ln 2) on [0, 1).
    let p = 1.0
        + f * (std::f32::consts::LN_2
            + f * (0.240_226_5 + f * (0.055_504_1 + f * (0.009_618_1 + f * (0.001_333_4 + f * 0.000_154)))));
    f32::from_bits(((xi as i32 + 127) << 23) as u32) * p
}

#[cfg(test)]
mod fast_math_tests {
    use super::*;

    #[test]
    fn close_to_libm() {
        for x in [1e-6f32, 0.003, 0.18, 0.5, 1.0, 1.7, 9.0, 300.0] {
            assert!((fast_log2(x) - x.log2()).abs() < 2e-3, "log2 {x}");
        }
        for x in [-10.0f32, -2.5, -0.3, 0.0, 0.7, 1.0, 3.2] {
            assert!((fast_exp2(x) / x.exp2() - 1.0).abs() < 1e-4, "exp2 {x}");
        }
    }
}
