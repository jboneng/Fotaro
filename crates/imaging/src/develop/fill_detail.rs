//! Remove: what a model's fill lacks, taken from the photo itself.
//!
//! The model fills at its own small size (512 px), whatever the size of the area in the
//! photo, so a large fill is soft and has no grain. Here the model's picture is kept for
//! what it is good at (structure and tone), and everything finer than it can draw is
//! copied from real pixels around the hole: for each part of the hole, from the place
//! outside it that looks most alike at the model's size (PatchMatch). Copied, not
//! averaged, so grain stays grain.

use image::{Rgb, RgbImage};

use super::heal::FillInput;
use super::local::box_blur_par;

/// A fill ready to keep: its picture, the part of the photo it covers (0..1), how much
/// detail it holds (model pixels per picture pixel, 1: all there is) and whether it is at
/// the photo's own size with the photo's fine detail.
pub struct FinishedFill {
    pub picture: RgbImage,
    pub rect: [f32; 4],
    pub detail: f32,
    pub full: bool,
}

/// Crops up to this side keep the model's picture as it is: there is little to add.
const PLAIN_SIDE: u32 = 640;
/// Half the side of the patches compared (7 × 7), in model pixels.
const PATCH: isize = 3;
/// Photo pixels kept around the hole in the picture.
const MARGIN: usize = 8;

/// Turn the model's result `out` (square, at the model's size, the whole of `input`'s
/// crop) into the fill to keep. A large crop gets the photo's fine detail (see the
/// module's notes) and a picture of the hole's box only, at the photo's size; a small one
/// the model's picture as it is. `seed` picks among equally good places to copy from.
pub fn finish_fill(input: &FillInput, out: &RgbImage, seed: u64) -> FinishedFill {
    let side = input.image.width();
    let m = out.width();
    if side > PLAIN_SIDE
        && m > 0
        && let Some(done) = with_detail(input, out, seed)
    {
        return done;
    }
    let picture = image::imageops::resize(out, side, side, image::imageops::FilterType::Lanczos3);
    FinishedFill { picture, rect: input.rect, detail: (m as f32 / side as f32).min(1.0), full: false }
}

fn with_detail(input: &FillInput, out: &RgbImage, seed: u64) -> Option<FinishedFill> {
    let side = input.image.width() as usize;
    let m = out.width() as usize;
    let k = side as f32 / m as f32;
    let mask = input.mask.as_raw();
    // The hole's box in the crop, and the hole at the model's size (a cell with any of it).
    let (mut x0, mut y0, mut x1, mut y1) = (side, side, 0usize, 0usize);
    let mut hole = vec![false; m * m];
    for y in 0..side {
        for x in 0..side {
            if mask[y * side + x] > 0 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                hole[((y as f32 / k) as usize).min(m - 1) * m + ((x as f32 / k) as usize).min(m - 1)] = true;
            }
        }
    }
    if x1 <= x0 {
        return None;
    }
    let guide: Vec<[f32; 3]> =
        out.pixels().map(|p| [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]).collect();
    let from = patch_match(&guide, &hole, m, seed)?;

    // The photo's fine layer: each channel less its own blur at the size the model can
    // hold, the blur taken from outside the hole only (nothing of what is removed in it).
    let r = ((0.8 * k).round() as usize).max(1);
    let keep: Vec<f32> = mask.iter().map(|v| if *v > 0 { 0.0 } else { 1.0 }).collect();
    let blur3 = |v: &[f32]| {
        box_blur_par(&box_blur_par(&box_blur_par(v, side, side, r), side, side, r), side, side, r)
    };
    let kept = blur3(&keep);
    let photo = input.image.as_raw();
    let fine: [Vec<f32>; 3] = std::array::from_fn(|c| {
        let p: Vec<f32> = (0..side * side).map(|i| f32::from(photo[3 * i + c]) * keep[i]).collect();
        let lo = blur3(&p);
        (0..side * side)
            .map(|i| if kept[i] > 0.05 { p[i] - keep[i] * lo[i] / kept[i] } else { 0.0 })
            .collect()
    });

    let (bx, by) = (x0.saturating_sub(MARGIN), y0.saturating_sub(MARGIN));
    let (bw, bh) = ((x1 + MARGIN).min(side) - bx, (y1 + MARGIN).min(side) - by);
    let picture = RgbImage::from_fn(bw as u32, bh as u32, |px, py| {
        let (x, y) = (bx + px as usize, by + py as usize);
        let i = y * side + x;
        if mask[i] == 0 {
            return *input.image.get_pixel(x as u32, y as u32);
        }
        let cell = ((y as f32 / k) as usize).min(m - 1) * m + ((x as f32 / k) as usize).min(m - 1);
        let (dx, dy) = from[cell];
        let sx = (x as isize + (f32::from(dx) * k).round() as isize).clamp(0, side as isize - 1) as usize;
        let sy = (y as isize + (f32::from(dy) * k).round() as isize).clamp(0, side as isize - 1) as usize;
        let coarse = sample(&guide, m, (x as f32 + 0.5) / k - 0.5, (y as f32 + 0.5) / k - 0.5);
        Rgb(std::array::from_fn(|c| (coarse[c] + fine[c][sy * side + sx]).round().clamp(0.0, 255.0) as u8))
    });
    let [l, t, rr, b] = input.rect;
    let (fw, fh) = ((rr - l) / side as f32, (b - t) / side as f32);
    let rect = [l + bx as f32 * fw, t + by as f32 * fh, l + (bx + bw) as f32 * fw, t + (by + bh) as f32 * fh];
    Some(FinishedFill { picture, rect, detail: 1.0, full: true })
}

/// The picture `g` (`m` square) at a place between its pixels, smoothly (Catmull-Rom).
fn sample(g: &[[f32; 3]], m: usize, x: f32, y: f32) -> [f32; 3] {
    let weights = |t: f32| {
        [
            ((-0.5 * t + 1.0) * t - 0.5) * t,
            (1.5 * t - 2.5) * t * t + 1.0,
            ((-1.5 * t + 2.0) * t + 0.5) * t,
            (0.5 * t - 0.5) * t * t,
        ]
    };
    let (ix, iy) = (x.floor(), y.floor());
    let (wx, wy) = (weights(x - ix), weights(y - iy));
    let mut v = [0.0f32; 3];
    for (j, wyj) in wy.iter().enumerate() {
        let yy = (iy as isize + j as isize - 1).clamp(0, m as isize - 1) as usize;
        for (i, wxi) in wx.iter().enumerate() {
            let xx = (ix as isize + i as isize - 1).clamp(0, m as isize - 1) as usize;
            let p = g[yy * m + xx];
            for c in 0..3 {
                v[c] += wxi * wyj * p[c];
            }
        }
    }
    v
}

/// For each cell of the hole in `guide` (`m` square), how far away (in cells) the place
/// outside the hole is whose surroundings look most like its own: PatchMatch, a few
/// passes of taking a neighbour's answer when it is better and of looking at random ever
/// nearer the best so far. `None` when there is nowhere to take from.
fn patch_match(guide: &[[f32; 3]], hole: &[bool], m: usize, seed: u64) -> Option<Vec<(i16, i16)>> {
    let mi = m as isize;
    // Places to take from: a patch and a little more with nothing of the hole in it.
    let reach = PATCH + 2;
    let mut near = vec![0u32; (m + 1) * (m + 1)];
    for y in 0..m {
        for x in 0..m {
            near[(y + 1) * (m + 1) + x + 1] =
                u32::from(hole[y * m + x]) + near[y * (m + 1) + x + 1] + near[(y + 1) * (m + 1) + x]
                    - near[y * (m + 1) + x];
        }
    }
    let free = |x: isize, y: isize| {
        if x < reach || y < reach || x >= mi - reach || y >= mi - reach {
            return false;
        }
        let (l, t, r, b) =
            ((x - reach) as usize, (y - reach) as usize, (x + reach + 1) as usize, (y + reach + 1) as usize);
        near[b * (m + 1) + r] + near[t * (m + 1) + l] == near[t * (m + 1) + r] + near[b * (m + 1) + l]
    };
    let sources: Vec<(i16, i16)> = (0..mi)
        .flat_map(|y| (0..mi).map(move |x| (x, y)))
        .filter(|(x, y)| free(*x, *y))
        .map(|(x, y)| (x as i16, y as i16))
        .collect();
    if sources.is_empty() {
        return None;
    }
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // How unlike the surroundings of a hole cell and of a source are (and, a little, how
    // far apart: nearer places have more alike light and grain). Stops once over `limit`.
    let cost = |tx: isize, ty: isize, sx: isize, sy: isize, limit: f32| -> f32 {
        let far = (((sx - tx).pow(2) + (sy - ty).pow(2)) as f32).sqrt();
        // In steps, so that places side by side cost the same: the nearest place of all
        // would be another for every cell, with a seam between each.
        let mut sum = 100.0 * (far / 48.0).floor();
        for dy in -PATCH..=PATCH {
            let t_row = (ty + dy).clamp(0, mi - 1) * mi;
            let s_row = (sy + dy) * mi;
            for dx in -PATCH..=PATCH {
                let a = guide[(t_row + (tx + dx).clamp(0, mi - 1)) as usize];
                let b = guide[(s_row + sx + dx) as usize];
                sum += (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2);
            }
            if sum > limit {
                return sum;
            }
        }
        sum
    };
    let mut from = vec![(0i16, 0i16); m * m];
    let mut best = vec![f32::MAX; m * m];
    let cells: Vec<(isize, isize)> = (0..mi)
        .flat_map(|y| (0..mi).map(move |x| (x, y)))
        .filter(|(x, y)| hole[(y * mi + x) as usize])
        .collect();
    for &(x, y) in &cells {
        let (sx, sy) = sources[(random() % sources.len() as u64) as usize];
        let i = (y * mi + x) as usize;
        from[i] = (sx - x as i16, sy - y as i16);
        best[i] = cost(x, y, isize::from(sx), isize::from(sy), f32::MAX);
    }
    for pass in 0..5 {
        let back = pass % 2 == 1;
        let step: isize = if back { 1 } else { -1 };
        for n in 0..cells.len() {
            let (x, y) = cells[if back { cells.len() - 1 - n } else { n }];
            let i = (y * mi + x) as usize;
            // `slack`: how much worse a place may be and still be taken (for a neighbour's
            // answer: cells that copy from side by side leave no seam in the grain).
            let consider = |sx: isize, sy: isize, slack: f32, from: &mut [(i16, i16)], best: &mut [f32]| {
                if free(sx, sy) && from[i] != ((sx - x) as i16, (sy - y) as i16) {
                    let limit = best[i] * (1.0 + slack) + 400.0 * slack.max(0.0);
                    let c = cost(x, y, sx, sy, limit);
                    if c < limit {
                        best[i] = c;
                        from[i] = ((sx - x) as i16, (sy - y) as i16);
                    }
                }
            };
            // A neighbour's answer, moved along with us.
            for (nx, ny) in [(x + step, y), (x, y + step)] {
                if nx >= 0 && ny >= 0 && nx < mi && ny < mi && hole[(ny * mi + nx) as usize] {
                    let (dx, dy) = from[(ny * mi + nx) as usize];
                    consider(x + isize::from(dx), y + isize::from(dy), 0.15, &mut from, &mut best);
                }
            }
            // At random around the best so far, ever nearer.
            let mut radius = mi;
            while radius >= 1 {
                let (cx, cy) = (x + isize::from(from[i].0), y + isize::from(from[i].1));
                let span = (2 * radius + 1) as u64;
                let sx = cx + (random() % span) as isize - radius;
                let sy = cy + (random() % span) as isize - radius;
                consider(sx, sy, -0.03, &mut from, &mut best);
                radius /= 2;
            }
        }
    }
    Some(from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GrayImage, Luma};

    /// A grainy photo (coarse grain over a slope of grey), a round hole, and what a
    /// perfect model would give: the slope without the grain, at the model's size.
    fn case(side: u32, m: u32) -> (FillInput, RgbImage) {
        let grain = super::super::heal::noise_plane(0, 0, side as usize, side as usize, 1.0, 5);
        let level = |x: u32| 60.0 + 120.0 * x as f32 / side as f32;
        let image = RgbImage::from_fn(side, side, |x, y| {
            let v = (level(x) + 14.0 * grain[(y * side + x) as usize]).round().clamp(0.0, 255.0) as u8;
            Rgb([v, v, v])
        });
        let mask = GrayImage::from_fn(side, side, |x, y| {
            let d = (x as f32 - side as f32 / 2.0).hypot(y as f32 - side as f32 / 2.0);
            Luma([if d < side as f32 * 0.2 { 255 } else { 0 }])
        });
        let out = RgbImage::from_fn(m, m, |x, _| {
            let v = level(x * side / m + side / m / 2).round() as u8;
            Rgb([v, v, v])
        });
        (FillInput { image, mask, rect: [0.25, 0.25, 0.75, 0.75], gains: [1.0; 3] }, out)
    }

    /// The spread of pixels from their four neighbours' mean, where `inside` says.
    fn roughness(img: &RgbImage, inside: impl Fn(u32, u32) -> bool) -> f32 {
        let (mut sum, mut n) = (0.0f64, 0u32);
        for y in 1..img.height() - 1 {
            for x in 1..img.width() - 1 {
                if inside(x, y) {
                    let at = |x: u32, y: u32| f32::from(img.get_pixel(x, y)[0]);
                    let d = at(x, y) - 0.25 * (at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1));
                    sum += f64::from(d * d);
                    n += 1;
                }
            }
        }
        (sum / f64::from(n.max(1))).sqrt() as f32
    }

    #[test]
    fn a_large_fill_gets_the_photos_grain_and_covers_the_hole_only() {
        let (side, m) = (1024u32, 256u32);
        let (input, out) = case(side, m);
        let done = finish_fill(&input, &out, 1);
        assert!(done.full && done.detail == 1.0);
        // The hole is 0.2 of the side in radius, about the middle: the picture is its box.
        let (w, h) = (done.picture.width(), done.picture.height());
        assert!((w as f32 - 0.4 * side as f32).abs() < 24.0 && w == h, "{w} x {h}");
        assert!((done.rect[0] - (0.25 + 0.5 * (0.3 - 8.0 / 1024.0))).abs() < 0.005, "{:?}", done.rect);
        assert!((done.rect[2] - done.rect[0] - 0.5 * w as f32 / side as f32).abs() < 1e-5);
        // As grainy inside as the photo is outside, and at the right level.
        let (c, r) = (w as f32 / 2.0, 0.17 * side as f32);
        let inside = |x: u32, y: u32| (x as f32 - c).hypot(y as f32 - c) < r;
        let (filled, photo) = (roughness(&done.picture, inside), roughness(&input.image, |_, _| true));
        assert!((filled / photo - 1.0).abs() < 0.2, "grain {filled} in the fill, {photo} in the photo");
        let middle = f32::from(done.picture.get_pixel(w / 2, h / 2)[0]);
        assert!((middle - 120.0).abs() < 45.0, "level {middle}");
        let mean: f32 =
            (0..w).map(|x| f32::from(done.picture.get_pixel(x, h / 2)[0])).sum::<f32>() / w as f32;
        assert!((mean - 120.0).abs() < 6.0, "mean level across the hole {mean}");
        // Outside the hole it is the photo, and the same every time.
        let at = |v: f32| ((v - 0.25) / 0.5 * side as f32).round() as u32;
        assert_eq!(done.picture.get_pixel(0, 0), input.image.get_pixel(at(done.rect[0]), at(done.rect[1])));
        assert_eq!(finish_fill(&input, &out, 1).picture, done.picture);
        assert_ne!(finish_fill(&input, &out, 2).picture, done.picture, "another seed, other places");
    }

    #[test]
    fn a_small_fill_is_the_models_picture() {
        let (input, out) = case(400, 256);
        let done = finish_fill(&input, &out, 1);
        assert!(!done.full);
        assert_eq!(done.rect, input.rect);
        assert_eq!(done.picture.width(), 400);
        assert!((done.detail - 0.64).abs() < 1e-6);
    }

    #[test]
    fn nowhere_to_take_from_leaves_the_models_picture() {
        let (mut input, out) = case(1024, 256);
        input.mask = GrayImage::from_pixel(1024, 1024, Luma([255]));
        assert!(!finish_fill(&input, &out, 1).full);
    }
}
