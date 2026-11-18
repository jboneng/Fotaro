//! The Remove tool's Heal and Clone (LrC's spot removal): spots painted on the photo, each
//! filled from a source area nearby. Clone copies the source as is; Heal copies its
//! texture and matches it to the colour and brightness around the spot.
//!
//! Spots live in the untransformed photo's coordinates and are applied to the photo's own
//! pixels before anything else (as LrC does), so the Transform, crop and masks follow
//! them. The healed copy is cached with the image until the spots change.
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
}

impl SpotMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Heal => "Heal",
            Self::Clone => "Clone",
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
        }
    }
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

/// A spot in pixels: its path, radius, the solid inner radius, and the source offset.
struct SpotPx {
    path: Vec<[f32; 2]>,
    r: f32,
    r_in: f32,
    off: (isize, isize),
}

impl SpotPx {
    fn new(s: &Spot, f: Frame) -> SpotPx {
        let r = (s.size * f.half).max(1.0);
        SpotPx {
            path: s.points.iter().map(|p| f.px(*p)).collect(),
            r,
            r_in: (r * (1.0 - s.feather.clamp(0.0, 100.0) / 100.0)).min(r - 0.75).max(0.0),
            off: ((s.offset[0] * f.w).round() as isize, (s.offset[1] * f.h).round() as isize),
        }
    }

    /// How much of the spot covers pixel (x, y): 1 inside, fading over the feather.
    #[inline]
    fn coverage(&self, x: usize, y: usize) -> f32 {
        let d = dist_to_path(&self.path, x as f32 + 0.5, y as f32 + 0.5);
        if d >= self.r { 0.0 } else { 1.0 - smoothstep(self.r_in, self.r, d) }
    }

    /// The spot's bounding box grown by `grow` pixels, within a `w × h` image.
    fn bbox(&self, grow: f32, w: usize, h: usize) -> (usize, usize, usize, usize) {
        let lo_x = self.path.iter().map(|p| p[0]).fold(f32::MAX, f32::min) - self.r - grow;
        let hi_x = self.path.iter().map(|p| p[0]).fold(f32::MIN, f32::max) + self.r + grow;
        let lo_y = self.path.iter().map(|p| p[1]).fold(f32::MAX, f32::min) - self.r - grow;
        let hi_y = self.path.iter().map(|p| p[1]).fold(f32::MIN, f32::max) + self.r + grow;
        let x0 = lo_x.floor().clamp(0.0, w as f32) as usize;
        let y0 = lo_y.floor().clamp(0.0, h as f32) as usize;
        let x1 = hi_x.ceil().clamp(0.0, w as f32) as usize;
        let y1 = hi_y.ceil().clamp(0.0, h as f32) as usize;
        (x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
    }
}

/// Apply one spot to interleaved RGB pixels of a `w × h` image.
fn apply_spot(px: &mut [f32], w: usize, h: usize, spot: &Spot, f: Frame) {
    if spot.points.is_empty() || spot.opacity <= 0.0 {
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
            SpotMode::Clone => src,
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

/// The healed copy cached with the image.
#[derive(Debug, Default)]
pub(super) struct HealCache(Mutex<Option<(Vec<Spot>, Arc<WorkingImage>)>>);

impl Clone for HealCache {
    /// A copy starts without the cache (it is only a cache).
    fn clone(&self) -> Self {
        HealCache::default()
    }
}

/// `img` with `spots` removed (the same image when there are none).
pub(super) fn healed<'a>(img: &'a WorkingImage, spots: &[Spot]) -> super::transform::Geometry<'a> {
    use super::transform::Geometry;
    if spots.is_empty() {
        return Geometry::Same(img);
    }
    let mut cache = img.heal_cache.0.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((key, out)) = cache.as_ref()
        && key == spots
    {
        return Geometry::Transformed(out.clone());
    }
    let mut out = img.clone();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let f = Frame::of(img);
    let mut px = std::mem::take(&mut out.pixels).into_raw();
    for spot in spots {
        apply_spot(&mut px, w, h, spot, f);
    }
    out.pixels = image::ImageBuffer::from_raw(w as u32, h as u32, px).expect("buffer size");
    let out = Arc::new(out);
    *cache = Some((spots.to_vec(), out.clone()));
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
    // The ring just outside the spot, sampled sparsely.
    let stride = ((s.r / 6.0) as usize).max(1);
    let (x0, y0, bw, bh) = s.bbox(0.7 * s.r, w, h);
    let ring: Vec<(usize, usize)> = (y0..y0 + bh)
        .step_by(stride)
        .flat_map(|y| (x0..x0 + bw).step_by(stride).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let d = dist_to_path(&s.path, x as f32 + 0.5, y as f32 + 0.5);
            d >= s.r && d < 1.7 * s.r
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
            let (ox, oy) = (a.cos() * dist * s.r, a.sin() * dist * s.r);
            let (ix, iy) = (ox.round() as isize, oy.round() as isize);
            let inside = |x: usize, y: usize| {
                let (x, y) = (x as isize + ix, y as isize + iy);
                x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h
            };
            if !inside(sx0, sy0) || !inside(sx0 + sbw.saturating_sub(1), sy0 + sbh.saturating_sub(1)) {
                continue;
            }
            if let Some((ax, ay)) = avoid_px
                && (ox - ax).hypot(oy - ay) < s.r
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
    let (ox, oy) = best.map_or((2.5 * s.r, 0.0), |(_, o)| o);
    [ox / f.w, oy / f.h]
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};

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
        };
        // Clone: the source's value (from x = 110).
        let cloned = healed(&img, &[spot(SpotMode::Clone)]);
        assert!((at(&cloned, 60, 50) - at(&img, 110, 50)).abs() < 1e-4);
        // Heal: the blemish gone, at the destination's own brightness (not the source's).
        let healed_img = healed(&img, &[spot(SpotMode::Heal)]);
        let v = at(&healed_img, 60, 50);
        assert!((v - at(&img, 60, 40)).abs() < 0.02, "{v} vs {}", at(&img, 60, 40));
        // Away from the spot nothing changes.
        assert_eq!(at(&healed_img, 150, 20), at(&img, 150, 20));
        // Cached: the same image again.
        let again = healed(&img, &[spot(SpotMode::Heal)]);
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
        };
        let out = healed(&img, &[spot]);
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
