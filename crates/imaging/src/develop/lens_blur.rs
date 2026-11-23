//! Lens Blur: what is away from the distance in focus is blurred, more the farther from it,
//! by the photo's depth map (see `Effects::fog_depth`).
//!
//! The blur is read from a pyramid of the picture: each level half the size of the one
//! before and twice as blurred, a pixel taking from the two levels around the blur it
//! wants. The levels are built with weights (a normalised convolution): what is in focus
//! weighs next to nothing, so a sharp subject does not smear into the blur behind it, and
//! bright points can be made to weigh more, as they do through a real lens. That is O(1)
//! work a pixel whatever the blur, which a full-size photo needs.
//!
//! What it does not do: something blurred in front of something sharp keeps its outline
//! (a real lens spreads it over what is behind), and the blur is soft, not a hard disc.

use rayon::prelude::*;

use super::effects::Effects;
use super::local::box_blur_par;

/// The blur of a pyramid level in its own cells (the halving, two small blurs and the
/// scaling back up together), so level `k` blurs by about this × 2^k pixels.
const LEVEL_BLUR: f32 = 1.25;

#[derive(Debug, Clone, Copy)]
pub(super) struct LensBlur {
    /// The most blur (as a Gaussian's spread, in rendered pixels).
    most: f32,
    /// The distance in focus (0 near .. 1 far), half the depth of the sharp zone, and how
    /// much farther the blur takes to reach its most.
    focus: f32,
    half: f32,
    ramp: f32,
    /// Extra weight of bright points, 0..1.
    highlights: f32,
    /// The pyramid's levels above the picture itself.
    levels: u32,
}

impl LensBlur {
    /// `scale`: rendered pixels per full-size pixel; `long`: the full-size photo's long
    /// side. `None` when off (or while the depth map itself is shown).
    pub fn new(e: &Effects, scale: f32, long: u32) -> Option<LensBlur> {
        if e.blur_amount <= 0.0 || e.fog_show {
            return None;
        }
        // At full Amount a disc of 1.2% of the long side; a Gaussian of half its radius
        // looks about as blurred.
        let most = 0.5 * 0.012 * (e.blur_amount.min(100.0) / 100.0) * long as f32 * scale;
        if most < 0.3 {
            return None;
        }
        let levels = ((most / LEVEL_BLUR).log2().ceil().max(1.0) as u32).min(7);
        Some(LensBlur {
            most,
            focus: e.blur_focus.clamp(0.0, 100.0) / 100.0,
            half: 0.5 * e.blur_range.clamp(0.0, 100.0) / 100.0,
            ramp: 0.35,
            highlights: e.blur_highlights.clamp(0.0, 100.0) / 100.0,
            levels,
        })
    }

    /// The pyramid's coarsest cell in pixels: the area blurred must start on a multiple of
    /// it (so a part of the photo is blurred exactly as in the whole).
    pub fn grid(&self) -> usize {
        1 << self.levels
    }

    /// How far around a pixel the blur reads.
    pub fn margin(&self) -> usize {
        5 * self.grid()
    }

    /// The blur (spread, pixels) of something `far` away (0 near .. 1 far).
    #[inline]
    fn spread(&self, far: f32) -> f32 {
        let off = ((far - self.focus).abs() - self.half).max(0.0);
        self.most * (off / self.ramp).min(1.0)
    }

    /// Blur `rgb` (linear light, `w` × `h`, starting on a multiple of [`Self::grid`] of
    /// the image) where `far(x, y)` says the pixel is out of focus.
    pub fn apply(
        &self,
        rgb: &mut [Vec<f32>; 3],
        w: usize,
        h: usize,
        far: &(dyn Fn(usize, usize) -> f32 + Sync),
    ) {
        let n = w * h;
        let spread: Vec<f32> = (0..n).into_par_iter().map(|k| self.spread(far(k % w, k / w))).collect();
        if spread.iter().all(|s| *s < 0.05) {
            return;
        }
        // A pixel's weight in the blur of others: by how blurred it is itself, and more
        // when bright.
        let weight: Vec<f32> = (0..n)
            .into_par_iter()
            .map(|k| {
                let lum = 0.2126 * rgb[0][k] + 0.7152 * rgb[1][k] + 0.0722 * rgb[2][k];
                let bright = ((lum - 0.5) / 0.5).clamp(0.0, 1.0);
                (spread[k] / self.most).max(1e-4) * (1.0 + 8.0 * self.highlights * bright * bright)
            })
            .collect();
        // Level 0: the picture times its weights, and the weights; each next level half
        // the size and blurred.
        let mut pyramid: Vec<Level> = Vec::with_capacity(self.levels as usize + 1);
        pyramid.push(Level {
            w,
            h,
            planes: [
                rgb[0].iter().zip(&weight).map(|(v, k)| v * k).collect(),
                rgb[1].iter().zip(&weight).map(|(v, k)| v * k).collect(),
                rgb[2].iter().zip(&weight).map(|(v, k)| v * k).collect(),
                weight,
            ],
        });
        let mut sharp = pyramid[0].halved();
        for _ in 0..self.levels {
            let next = sharp.halved();
            pyramid.push(sharp.blurred());
            sharp = next;
        }
        let top = self.levels as f32;
        let (r, rest) = rgb.split_at_mut(1);
        let (g, b) = rest.split_at_mut(1);
        (r[0].par_chunks_mut(w), g[0].par_chunks_mut(w), b[0].par_chunks_mut(w))
            .into_par_iter()
            .enumerate()
            .for_each(|(y, (r, g, b))| {
                for x in 0..w {
                    let s = spread[y * w + x];
                    if s < 0.05 {
                        continue;
                    }
                    // Which two levels, and how much of the upper.
                    let (lo, t) = if s <= 2.0 * LEVEL_BLUR {
                        (0usize, s / (2.0 * LEVEL_BLUR))
                    } else {
                        let l = (s / LEVEL_BLUR).log2().min(top);
                        (l.floor() as usize, l.fract())
                    };
                    let hi = (lo + 1).min(self.levels as usize);
                    let (a, c) = (pyramid[lo].at(x, y, lo), pyramid[hi].at(x, y, hi));
                    let v: [f32; 4] = std::array::from_fn(|k| a[k] + t * (c[k] - a[k]));
                    if v[3] > 1e-7 {
                        (r[x], g[x], b[x]) = (v[0] / v[3], v[1] / v[3], v[2] / v[3]);
                    }
                }
            });
    }
}

/// A level of the pyramid: the picture's three channels times the weights, and the
/// weights.
struct Level {
    w: usize,
    h: usize,
    planes: [Vec<f32>; 4],
}

impl Level {
    /// Half the size: each cell the mean of the (up to) four under it.
    fn halved(&self) -> Level {
        let (w, h) = (self.w.div_ceil(2), self.h.div_ceil(2));
        let planes = std::array::from_fn(|c| {
            let src = &self.planes[c];
            let mut out = vec![0.0f32; w * h];
            out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                let (y0, y1) = (2 * y, (2 * y + 1).min(self.h - 1));
                for (x, v) in row.iter_mut().enumerate() {
                    let (x0, x1) = (2 * x, (2 * x + 1).min(self.w - 1));
                    *v = 0.25
                        * (src[y0 * self.w + x0]
                            + src[y0 * self.w + x1]
                            + src[y1 * self.w + x0]
                            + src[y1 * self.w + x1]);
                }
            });
            out
        });
        Level { w, h, planes }
    }

    /// Blurred a little (two passes of a three-cell mean).
    fn blurred(&self) -> Level {
        let planes = std::array::from_fn(|c| {
            box_blur_par(&box_blur_par(&self.planes[c], self.w, self.h, 1), self.w, self.h, 1)
        });
        Level { w: self.w, h: self.h, planes }
    }

    /// The level (number `k`) at pixel (x, y) of the picture, between its cells.
    #[inline]
    fn at(&self, x: usize, y: usize, k: usize) -> [f32; 4] {
        if k == 0 {
            let i = y * self.w + x;
            return [self.planes[0][i], self.planes[1][i], self.planes[2][i], self.planes[3][i]];
        }
        let cell = (1usize << k) as f32;
        let fx = ((x as f32 + 0.5) / cell - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = ((y as f32 + 0.5) / cell - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        std::array::from_fn(|c| {
            let p = &self.planes[c];
            let top = p[y0 * self.w + x0] + tx * (p[y0 * self.w + x1] - p[y0 * self.w + x0]);
            let bottom = p[y1 * self.w + x0] + tx * (p[y1 * self.w + x1] - p[y1 * self.w + x0]);
            top + ty * (bottom - top)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blur(amount: f32, focus: f32, range: f32) -> LensBlur {
        let e = Effects { blur_amount: amount, blur_focus: focus, blur_range: range, ..Effects::default() };
        LensBlur::new(&e, 1.0, 4000).expect("on")
    }

    #[test]
    fn what_is_in_focus_stays_and_the_rest_blurs_more_the_farther() {
        let b = blur(100.0, 20.0, 20.0);
        assert_eq!(b.spread(0.2), 0.0);
        assert_eq!(b.spread(0.29), 0.0, "inside the sharp zone");
        assert!(b.spread(0.5) > 0.0 && b.spread(0.9) > b.spread(0.5));
        assert!((b.spread(1.0) - 24.0).abs() < 0.01, "1.2% of 4000 px as a disc, half as a spread");
        assert!(b.spread(0.0) > 0.0, "and in front of it");
        assert!(LensBlur::new(&Effects::default(), 1.0, 4000).is_none());
        assert_eq!(b.grid(), 32);

        // Stripes: sharp on the left (near, in focus), far on the right.
        let (w, h) = (256usize, 128usize);
        let stripes: Vec<f32> = (0..w * h).map(|k| if (k % w) / 4 % 2 == 0 { 0.8 } else { 0.2 }).collect();
        let mut rgb = [stripes.clone(), stripes.clone(), stripes.clone()];
        b.apply(&mut rgb, w, h, &|x, _| if x < 128 { 0.2 } else { 0.9 });
        let contrast = |from: usize, to: usize| {
            let row = &rgb[1][64 * w + from..64 * w + to];
            row.iter().copied().fold(f32::MIN, f32::max) - row.iter().copied().fold(f32::MAX, f32::min)
        };
        assert_eq!(&rgb[0][64 * w..64 * w + 120], &stripes[64 * w..64 * w + 120], "in focus: untouched");
        assert!(contrast(170, 250) < 0.05, "out of focus: the stripes are gone ({})", contrast(170, 250));
        // The sharp side does not smear into the blurred one: just past the edge it is
        // the far side's own mean (0.5), not pulled towards the last sharp stripe.
        assert!((rgb[1][64 * w + 134] - 0.5).abs() < 0.12, "{}", rgb[1][64 * w + 134]);
    }
}
