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
    finish_fill_with(input, out, &Finish { seed, ..Finish::default() })
}

/// How a fill is finished (see [`finish_fill_with`]).
#[derive(Clone, Copy)]
pub struct Finish<'a> {
    /// Picks among equally good places to copy detail from.
    pub seed: u64,
    /// The photo's depth map (white near, the whole untransformed photo): detail is then
    /// copied from places about as far away as what is behind the hole.
    pub depth: Option<&'a image::GrayImage>,
    /// Match the fill's tone to the photo along the hole's border.
    pub border: bool,
}

impl Default for Finish<'_> {
    fn default() -> Self {
        Finish { seed: 0, depth: None, border: true }
    }
}

/// [`finish_fill`] with the photo's depth map (white near, covering the whole untransformed
/// photo), when it has one: the fine detail is then copied from places about as far away
/// as what is behind the hole, so a wall's fill does not get its grain and texture from
/// the grass in front of it.
pub fn finish_fill_by_depth(
    input: &FillInput,
    out: &RgbImage,
    seed: u64,
    depth: Option<&image::GrayImage>,
) -> FinishedFill {
    finish_fill_with(input, out, &Finish { seed, depth, ..Finish::default() })
}

/// [`finish_fill`] as `how` says: first the fill's tone is matched to the photo along
/// the hole's border (see [`match_border`]), then a large crop gets the photo's detail.
pub fn finish_fill_with(input: &FillInput, out: &RgbImage, how: &Finish) -> FinishedFill {
    let side = input.image.width();
    let m = out.width();
    let matched = (how.border && m > 0).then(|| match_border(input, out));
    let out = matched.as_ref().unwrap_or(out);
    if side > PLAIN_SIDE
        && m > 0
        && let Some(done) = with_detail(input, out, how.seed, how.depth)
    {
        return done;
    }
    let picture = image::imageops::resize(out, side, side, image::imageops::FilterType::Lanczos3);
    FinishedFill { picture, rect: input.rect, detail: (m as f32 / side as f32).min(1.0), full: false }
}

/// The hole of `input` at the model's size (`m` square): the cells with any of it.
fn hole_cells(input: &FillInput, m: usize) -> Vec<bool> {
    let side = input.image.width() as usize;
    let k = side as f32 / m as f32;
    let mut hole = vec![false; m * m];
    for (i, v) in input.mask.as_raw().iter().enumerate() {
        if *v > 0 {
            let (x, y) = (i % side, i / side);
            hole[((y as f32 / k) as usize).min(m - 1) * m + ((x as f32 / k) as usize).min(m - 1)] = true;
        }
    }
    hole
}

/// The most a fill's tone is moved to meet the photo (of 255): what the model drew at
/// the border on purpose (something dark going on into the hole) is not undone.
const BORDER_MOST: f32 = 48.0;
/// Over how many cells along the border the difference is averaged (to either side):
/// grain and texture are not a difference in tone.
const BORDER_ALONG: usize = 6;

/// The model's picture `out` with its tone matched to the photo along the hole's border:
/// where the fill meets the photo lighter, darker or of another colour than the photo is
/// right there (a sky or a wall drawn a shade off), the difference is taken off at the
/// border and, fading smoothly, across the hole. What the model drew stays as drawn, on
/// another level: the border no longer shows as a step.
pub fn match_border(input: &FillInput, out: &RgbImage) -> RgbImage {
    let m = out.width() as usize;
    if m == 0 || out.height() as usize != m {
        return out.clone();
    }
    let hole = hole_cells(input, m);
    // The photo as the model saw it.
    let photo =
        image::imageops::resize(&input.image, m as u32, m as u32, image::imageops::FilterType::Triangle);
    // At the hole's outermost cells: the photo right outside less the fill right inside.
    let mut diff = vec![0.0f32; 3 * m * m];
    let mut edge = vec![0.0f32; m * m];
    for y in 0..m {
        for x in 0..m {
            if !hole[y * m + x] {
                continue;
            }
            let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
            for (dx, dy) in [(-1isize, 0isize), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if nx < 0 || ny < 0 || nx >= m as isize || ny >= m as isize {
                    continue;
                }
                if !hole[ny as usize * m + nx as usize] {
                    let p = photo.get_pixel(nx as u32, ny as u32);
                    for c in 0..3 {
                        sum[c] += f32::from(p[c]);
                    }
                    n += 1.0;
                }
            }
            if n > 0.0 {
                let o = out.get_pixel(x as u32, y as u32);
                for c in 0..3 {
                    diff[3 * (y * m + x) + c] = sum[c] / n - f32::from(o[c]);
                }
                edge[y * m + x] = 1.0;
            }
        }
    }
    if edge.iter().all(|e| *e == 0.0) {
        return out.clone();
    }
    // Averaged along the border.
    let blur = |v: &[f32]| box_blur_par(&box_blur_par(v, m, m, BORDER_ALONG), m, m, BORDER_ALONG);
    let weight = blur(&edge);
    let mut level = vec![0.0f32; 3 * m * m];
    for c in 0..3 {
        let plane: Vec<f32> = (0..m * m).map(|i| diff[3 * i + c]).collect();
        let sum = blur(&plane);
        for i in 0..m * m {
            if edge[i] > 0.0 {
                level[3 * i + c] = (sum[i] / weight[i].max(1e-6)).clamp(-BORDER_MOST, BORDER_MOST);
            }
        }
    }
    // Across the hole: smoothly from what it is at the border.
    let unknown: Vec<u8> = edge.iter().map(|e| if *e > 0.0 { 0 } else { 255 }).collect();
    super::dust::fill_masked(&mut level, m, m, &unknown);
    RgbImage::from_fn(m as u32, m as u32, |x, y| {
        let i = y as usize * m + x as usize;
        let p = out.get_pixel(x, y);
        if !hole[i] {
            return *p;
        }
        Rgb(std::array::from_fn(|c| (f32::from(p[c]) + level[3 * i + c]).round().clamp(0.0, 255.0) as u8))
    })
}

fn with_detail(
    input: &FillInput,
    out: &RgbImage,
    seed: u64,
    depth: Option<&image::GrayImage>,
) -> Option<FinishedFill> {
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
    let far = depth.and_then(|d| distances(input.rect, d, &hole, m));
    let from = patch_match(&guide, &hole, m, seed, far.as_ref())?;

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

/// The depth map's values (0..255, more is nearer) a cell of the crop `rect` at the model's
/// size (`m` square): as the map has them, and as they would be without what is removed
/// (in the hole, what is around it continued inward). `None` when there is nothing around.
fn distances(
    rect: [f32; 4],
    depth: &image::GrayImage,
    hole: &[bool],
    m: usize,
) -> Option<(Vec<f32>, Vec<f32>)> {
    let (dw, dh) = depth.dimensions();
    if dw == 0 || dh == 0 {
        return None;
    }
    let [l, t, r, b] = rect;
    let own: Vec<f32> = (0..m * m)
        .map(|i| {
            let u = l + ((i % m) as f32 + 0.5) / m as f32 * (r - l);
            let v = t + ((i / m) as f32 + 0.5) / m as f32 * (b - t);
            let x = ((u * dw as f32) as u32).min(dw - 1);
            let y = ((v * dh as f32) as u32).min(dh - 1);
            f32::from(depth.get_pixel(x, y)[0])
        })
        .collect();
    // The map's edges are soft: what is right around the hole still has some of what is
    // removed in it, and is left out.
    let holes: Vec<f32> = hole.iter().map(|h| f32::from(u8::from(*h))).collect();
    let grown = box_blur_par(&holes, m, m, 6);
    let keep: Vec<f32> = grown.iter().map(|v| if *v > 1e-4 { 0.0 } else { 1.0 }).collect();
    if keep.iter().all(|k| *k == 0.0) {
        return None;
    }
    let kept: Vec<f32> = own.iter().zip(&keep).map(|(d, k)| d * k).collect();
    let mut behind = own.clone();
    let mut open: Vec<bool> = keep.iter().map(|k| *k == 0.0).collect();
    let mut radius = 8;
    while radius < m && open.iter().any(|o| *o) {
        let (sum, n) = (box_blur_par(&kept, m, m, radius), box_blur_par(&keep, m, m, radius));
        for i in 0..m * m {
            if open[i] && n[i] > 0.02 {
                (behind[i], open[i]) = (sum[i] / n[i], false);
            }
        }
        radius *= 2;
    }
    Some((own, behind))
}

/// How much farther or nearer (in the depth map's 255 steps) a place may be than what is
/// behind the hole before it costs, and what each step over costs (per step squared; a
/// patch that looks quite alike costs a few thousand).
const DEPTH_SLACK: f32 = 10.0;
const DEPTH_COST: f32 = 30.0;

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
/// nearer the best so far. With `depths` (see [`distances`]), places as far away as what
/// is behind the hole are preferred. `None` when there is nowhere to take from.
fn patch_match(
    guide: &[[f32; 3]],
    hole: &[bool],
    m: usize,
    seed: u64,
    depths: Option<&(Vec<f32>, Vec<f32>)>,
) -> Option<Vec<(i16, i16)>> {
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
        if let Some((own, behind)) = depths {
            let off = (own[(sy * mi + sx) as usize] - behind[(ty * mi + tx) as usize]).abs() - DEPTH_SLACK;
            if off > 0.0 {
                sum += DEPTH_COST * off * off;
            }
        }
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
    fn with_a_depth_map_detail_comes_from_as_far_away() {
        // The same grain everywhere, so looks cannot tell the halves apart; the left half
        // is near, the right far, and the hole is in the left, by the border.
        let m = 128usize;
        let grain = super::super::heal::noise_plane(0, 0, m, m, 1.0, 3);
        let guide: Vec<[f32; 3]> = grain.iter().map(|g| [120.0 + 30.0 * g; 3]).collect();
        let hole: Vec<bool> =
            (0..m * m).map(|i| (30..48).contains(&(i % m)) && (54..74).contains(&(i / m))).collect();
        let depth = GrayImage::from_fn(256, 256, |x, _| Luma([if x < 128 { 200 } else { 50 }]));
        let far = distances([0.0, 0.0, 1.0, 1.0], &depth, &hole, m).expect("there is a map");
        assert!(
            (far.1[64 * m + 40] - 200.0).abs() < 8.0,
            "behind the hole: as around it ({})",
            far.1[64 * m + 40]
        );
        let right = |from: &[(i16, i16)]| {
            (0..m * m).filter(|i| hole[*i] && (i % m) as i16 + from[*i].0 >= 64).count()
        };
        let plain = patch_match(&guide, &hole, m, 1, None).unwrap();
        let by_depth = patch_match(&guide, &hole, m, 1, Some(&far)).unwrap();
        assert!(right(&plain) > 20, "by looks alone, from both halves ({})", right(&plain));
        assert_eq!(right(&by_depth), 0, "by depth, from its own half only");
    }

    #[test]
    fn a_fill_a_shade_off_meets_the_photo_at_its_border() {
        let (side, m) = (1024u32, 256u32);
        let (input, out) = case(side, m);
        let hole = hole_cells(&input, m as usize);
        let inside = |x: u32, y: u32| hole[(y * m + x) as usize];
        // The model drew the hole 20 lighter than the photo is around it.
        let off = RgbImage::from_fn(m, m, |x, y| {
            let v = out.get_pixel(x, y)[0] + if inside(x, y) { 20 } else { 0 };
            Rgb([v, v, v])
        });
        let mean_off = |a: &RgbImage| {
            let (mut sum, mut n) = (0.0f32, 0.0f32);
            for (x, y, p) in a.enumerate_pixels() {
                if inside(x, y) {
                    sum += f32::from(p[0]) - f32::from(out.get_pixel(x, y)[0]);
                    n += 1.0;
                }
            }
            sum / n
        };
        let matched = match_border(&input, &off);
        assert!(mean_off(&matched).abs() < 2.5, "still {} off", mean_off(&matched));
        // Outside the hole nothing changes, and a fill that meets the photo stays as it is.
        assert_eq!(matched.get_pixel(2, 2), off.get_pixel(2, 2));
        let same = match_border(&input, &out);
        assert!(mean_off(&same).abs() < 2.5, "moved by {}", mean_off(&same));
        // Without the matching, the fill is kept a shade off.
        let kept = finish_fill_with(&input, &off, &Finish { seed: 1, depth: None, border: false });
        let done = finish_fill_with(&input, &off, &Finish { seed: 1, ..Finish::default() });
        let (w, h) = (done.picture.width(), done.picture.height());
        let row = |p: &RgbImage| (w / 4..3 * w / 4).map(|x| f32::from(p.get_pixel(x, h / 2)[0])).sum::<f32>();
        assert!(row(&kept.picture) - row(&done.picture) > 10.0 * (w / 2) as f32);
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
