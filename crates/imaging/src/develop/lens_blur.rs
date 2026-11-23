//! Lens Blur: what is away from the distance in focus is blurred, more the farther from it,
//! by the photo's depth map (see `Effects::fog_depth`).
//!
//! A point out of focus becomes a disc, as through a lens. Discs are drawn on a pyramid of
//! the picture (each level's cells twice the size of the one before): a disc is at most
//! [`DISC`] cells wide on its level, so the work a pixel does not grow with the blur, which
//! a full-size photo needs.
//!
//! What is behind the distance in focus and what is in front of it are blurred apart:
//!
//! - Behind ([`LensBlur::behind`]): a pixel gathers the disc around it, from what is at
//!   least as blurred as itself. What is in focus is left out, so a sharp subject does not
//!   smear into the blur behind it.
//! - In front ([`LensBlur::in_front`]): every pixel spreads over its disc, and the sum is
//!   laid over what is behind, so something blurred in front of something sharp loses its
//!   outline. What it hid is not in the photo: there it shows the surroundings, continued.
//!
//! Bright points can be made to weigh more in both, as they do through a real lens.

use rayon::prelude::*;

use super::effects::Effects;

/// The widest disc (radius, in a level's own cells) a level draws; wider ones go to the
/// next level.
const DISC: f32 = 6.0;
/// A level hands over to the next across the last part of its range (this share of
/// [`DISC`] and up), so there is no seam where the level changes.
const HANDOVER: f32 = 0.75;
/// A disc this small (pixels) or smaller is partly the pixel as it was.
const SOFT: f32 = 1.5;
/// Cells a side in a block of [`Reach`]: more than a disc reaches.
const BLOCK: usize = 8;
/// Weights below this count as none.
const TINY: f32 = 1e-6;

#[derive(Debug, Clone, Copy)]
pub(super) struct LensBlur {
    /// The widest disc (radius, in rendered pixels).
    most: f32,
    /// The distance in focus (0 near .. 1 far), half the depth of the sharp zone, and how
    /// much farther the blur takes to reach its most.
    focus: f32,
    half: f32,
    ramp: f32,
    /// Extra weight of bright points, 0..1.
    highlights: f32,
    /// The pyramid's levels above the picture itself (at least one).
    levels: u32,
}

/// How much of a disc of `c` pixels level `k` (of `top`) draws: all of it inside the
/// level's range, shared with the next level where the ranges meet.
#[inline]
fn share(c: f32, k: u32, top: u32) -> f32 {
    let r = c / (1u32 << k) as f32;
    let step = |r: f32| ((r - HANDOVER * DISC) / ((1.0 - HANDOVER) * DISC)).clamp(0.0, 1.0);
    let entering = if k == 0 { 1.0 } else { step(2.0 * r) };
    let leaving = if k == top { 0.0 } else { step(r) };
    entering * (1.0 - leaving)
}

/// How much of the cell `d` cells away a disc of radius `r` cells covers.
#[inline]
fn covered(r: f32, d: f32) -> f32 {
    (r - d + 0.5).clamp(0.0, 1.0)
}

/// The area of a disc as [`covered`] draws it, by radius (in sixteenths of a cell).
fn disc_areas() -> Vec<f32> {
    let reach = DISC as i32 + 2;
    (0..=((DISC + 1.0) * 16.0) as usize)
        .map(|i| {
            let r = i as f32 / 16.0;
            let mut sum = 0.0;
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    sum += covered(r, ((dx * dx + dy * dy) as f32).sqrt());
                }
            }
            sum
        })
        .collect()
}

#[inline]
fn area(areas: &[f32], r: f32) -> f32 {
    let f = (r * 16.0).clamp(0.0, (areas.len() - 1) as f32);
    let i = (f as usize).min(areas.len() - 2);
    areas[i] + (f - i as f32) * (areas[i + 1] - areas[i])
}

impl LensBlur {
    /// `scale`: rendered pixels per full-size pixel; `long`: the full-size photo's long
    /// side. `None` when off (or while the depth map itself is shown).
    pub fn new(e: &Effects, scale: f32, long: u32) -> Option<LensBlur> {
        if e.blur_amount <= 0.0 || e.fog_show {
            return None;
        }
        // At full Amount a disc of 1.2% of the long side.
        let most = 0.012 * (e.blur_amount.min(100.0) / 100.0) * long as f32 * scale;
        if most < 0.6 {
            return None;
        }
        let levels = ((most / DISC).log2().ceil().max(1.0) as u32).min(8);
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
        (2 * DISC as usize + 2) * self.grid()
    }

    /// The disc (radius, pixels) of something `far` away (0 near .. 1 far): negative for
    /// what is in front of the distance in focus.
    #[inline]
    fn disc(&self, far: f32) -> f32 {
        let off = far - self.focus;
        let c = self.most * ((off.abs() - self.half).max(0.0) / self.ramp).min(1.0);
        if off < 0.0 { -c } else { c }
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
        if n == 0 {
            return;
        }
        let disc: Vec<f32> = (0..n).into_par_iter().map(|k| self.disc(far(k % w, k / w))).collect();
        if disc.iter().all(|c| c.abs() < 0.05) {
            return;
        }
        // Bright points weigh more.
        let boost: Vec<f32> = (0..n)
            .into_par_iter()
            .map(|k| {
                let lum = 0.2126 * rgb[0][k] + 0.7152 * rgb[1][k] + 0.0722 * rgb[2][k];
                let bright = ((lum - 0.5) / 0.5).clamp(0.0, 1.0);
                1.0 + 8.0 * self.highlights * bright * bright
            })
            .collect();
        let picture = Picture { rgb, disc: &disc, boost: &boost, w, h };
        super::perf::mark("lens blur: discs");
        let mut out = self.behind(&picture);
        super::perf::mark("lens blur: behind");
        if disc.iter().any(|c| *c < -0.05) {
            self.in_front(&picture, &mut out);
            super::perf::mark("lens blur: in front");
        }
        *rgb = out;
    }

    /// The picture with what is behind the distance in focus blurred; the rest as it was.
    fn behind(&self, p: &Picture) -> [Vec<f32>; 3] {
        let (w, h, top) = (p.w, p.h, self.levels);
        // What a pixel gives to the blur of others: nothing in focus (or in front).
        let weight = |i: usize| {
            let c = p.disc[i];
            if c > 0.0 { (c / SOFT).min(1.0) * p.boost[i] } else { 0.0 }
        };
        // Levels 1..: colour times weight, the weight, and the disc times the weight.
        let mut levels: Vec<Grid<5>> = vec![Grid::cells(w, h, 1, &|i| {
            let k = weight(i);
            [p.rgb[0][i] * k, p.rgb[1][i] * k, p.rgb[2][i] * k, k, p.disc[i].max(0.0) * k]
        })];
        for _ in 1..top {
            let next = levels.last().expect("one").halved();
            levels.push(next);
        }
        // Each level blurred, where a pixel will read it: colour (times whether there is
        // any), and whether there is any.
        let blurred: Vec<Grid<4>> = levels
            .iter()
            .enumerate()
            .map(|(n, g)| {
                let k = n as u32 + 1;
                let cell = (1u32 << k) as f32;
                let wanted = Grid::<1>::cells(w, h, k, &|i| {
                    [f32::from(p.disc[i] > 0.0 && share(p.disc[i], k, top) > 0.0)]
                });
                let disc_of = |i: usize| {
                    let v = g.cells[i];
                    (v[3] > TINY).then(|| v[4] / v[3] / cell)
                };
                let cells = (0..g.w * g.h)
                    .into_par_iter()
                    .map(|i| {
                        let (x, y) = ((i % g.w) as i32, (i / g.w) as i32);
                        // Pixels read the cells around them too.
                        if !around(g.w, g.h, x, y).any(|j| wanted.cells[j][0] > 0.0) {
                            return [0.0; 4];
                        }
                        // A cell with nothing blurred in it takes its neighbours' disc.
                        let r = disc_of(i)
                            .unwrap_or_else(|| around(g.w, g.h, x, y).filter_map(disc_of).fold(0.0, f32::max))
                            .min(DISC);
                        let m = (r + 0.5) as i32;
                        let mut acc = [0.0f32; 4];
                        for dy in -m..=m {
                            let yy = y + dy;
                            if yy < 0 || yy >= g.h as i32 {
                                continue;
                            }
                            for dx in -m..=m {
                                let xx = x + dx;
                                if xx < 0 || xx >= g.w as i32 {
                                    continue;
                                }
                                let q = g.cells[yy as usize * g.w + xx as usize];
                                if q[3] <= TINY {
                                    continue;
                                }
                                // From what is at least as blurred: a sharper thing
                                // keeps to its own, smaller disc.
                                let d = ((dx * dx + dy * dy) as f32).sqrt();
                                let k = covered((q[4] / q[3] / cell).min(r), d);
                                acc = [
                                    acc[0] + k * q[0],
                                    acc[1] + k * q[1],
                                    acc[2] + k * q[2],
                                    acc[3] + k * q[3],
                                ];
                            }
                        }
                        if acc[3] > TINY {
                            [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3], 1.0]
                        } else {
                            [0.0; 4]
                        }
                    })
                    .collect();
                Grid { w: g.w, h: g.h, cells }
            })
            .collect();
        let mut out: [Vec<f32>; 3] = std::array::from_fn(|_| vec![0.0f32; w * h]);
        let [r, g, b] = &mut out;
        (r.par_chunks_mut(w), g.par_chunks_mut(w), b.par_chunks_mut(w)).into_par_iter().enumerate().for_each(
            |(y, (r, g, b))| {
                for x in 0..w {
                    let i = y * w + x;
                    let own = [p.rgb[0][i], p.rgb[1][i], p.rgb[2][i]];
                    (r[x], g[x], b[x]) = (own[0], own[1], own[2]);
                    let c = p.disc[i];
                    if c < 0.05 {
                        continue;
                    }
                    let (mut acc, mut sum) = ([0.0f32; 3], 0.0f32);
                    let s = share(c, 0, top);
                    if s > 0.0 {
                        // The picture itself: the disc around the pixel.
                        let rad = c.min(DISC);
                        let m = (rad + 0.5) as i32;
                        let mut v = [0.0f32; 4];
                        for dy in -m..=m {
                            let yy = y as i32 + dy;
                            if yy < 0 || yy >= h as i32 {
                                continue;
                            }
                            for dx in -m..=m {
                                let xx = x as i32 + dx;
                                if xx < 0 || xx >= w as i32 {
                                    continue;
                                }
                                let j = yy as usize * w + xx as usize;
                                let cq = p.disc[j];
                                if cq <= 0.0 {
                                    continue;
                                }
                                let d = ((dx * dx + dy * dy) as f32).sqrt();
                                let k = covered(cq.min(rad), d) * weight(j);
                                v = [
                                    v[0] + k * p.rgb[0][j],
                                    v[1] + k * p.rgb[1][j],
                                    v[2] + k * p.rgb[2][j],
                                    v[3] + k,
                                ];
                            }
                        }
                        if v[3] > TINY {
                            acc = [s * v[0] / v[3], s * v[1] / v[3], s * v[2] / v[3]];
                            sum = s;
                        }
                    }
                    for (n, level) in blurred.iter().enumerate() {
                        let k = n as u32 + 1;
                        let s = share(c, k, top);
                        if s <= 0.0 {
                            continue;
                        }
                        let v = level.at_pixel(x, y, k);
                        if v[3] > 1e-3 {
                            acc = [
                                acc[0] + s * v[0] / v[3],
                                acc[1] + s * v[1] / v[3],
                                acc[2] + s * v[2] / v[3],
                            ];
                            sum += s;
                        }
                    }
                    if sum > 0.0 {
                        (r[x], g[x], b[x]) = (acc[0] / sum, acc[1] / sum, acc[2] / sum);
                    }
                }
            },
        );
        out
    }

    /// Lay what is in front of the distance in focus, blurred, over `out` (the picture
    /// with the rest done).
    fn in_front(&self, p: &Picture, out: &mut [Vec<f32>; 3]) {
        let (w, h, top) = (p.w, p.h, self.levels);
        let areas = disc_areas();
        // How much of a pixel is in front (and so spread), and how much of it level `k`
        // spreads.
        let front = |i: usize| (-p.disc[i] / SOFT).clamp(0.0, 1.0);
        let spread = |i: usize, k: u32| {
            let c = -p.disc[i];
            if c > 0.0 { (c / SOFT).min(1.0) * share(c, k, top) } else { 0.0 }
        };
        // What is behind it, also where it hides that: the rest of the picture continued
        // inward from around it, level by level from the coarsest.
        let shown = |i: usize| (1.0 - front(i)).max(1e-4);
        let behind: Vec<Grid<3>> = {
            let mut levels: Vec<Grid<4>> = vec![Grid::cells(w, h, 1, &|i| {
                let k = shown(i);
                [out[0][i] * k, out[1][i] * k, out[2][i] * k, k]
            })];
            for _ in 1..top {
                let next = levels.last().expect("one").halved();
                levels.push(next);
            }
            let coarsest = levels.last().expect("one");
            let mut filled: Vec<Grid<3>> = vec![Grid {
                w: coarsest.w,
                h: coarsest.h,
                cells: coarsest.cells.iter().map(|v| [v[0] / v[3], v[1] / v[3], v[2] / v[3]]).collect(),
            }];
            for g in levels.iter().rev().skip(1) {
                let upper = filled.last().expect("one");
                let cells = (0..g.w * g.h)
                    .into_par_iter()
                    .map(|i| {
                        let (x, y) = ((i % g.w) as f32, (i / g.w) as f32);
                        let up = upper.sample((x + 0.5) / 2.0 - 0.5, (y + 0.5) / 2.0 - 0.5);
                        let v = g.cells[i];
                        let t = (1.0 - v[3]).max(0.0);
                        [v[0] + t * up[0], v[1] + t * up[1], v[2] + t * up[2]]
                    })
                    .collect();
                filled.push(Grid { w: g.w, h: g.h, cells });
            }
            filled.reverse();
            filled
        };
        // Each level's cells spread over their discs: colour times weight, the weight (more
        // for bright points), and how much of the cell is covered.
        let spreads: Vec<(u32, Grid<5>)> = (1..=top)
            .filter_map(|k| {
                let cell = (1u32 << k) as f32;
                let g = Grid::<6>::cells(w, h, k, &|i| {
                    let a = spread(i, k);
                    if a <= 0.0 {
                        return [0.0; 6];
                    }
                    let ah = a * p.boost[i];
                    [p.rgb[0][i] * ah, p.rgb[1][i] * ah, p.rgb[2][i] * ah, ah, a, -p.disc[i] * a]
                });
                let disc_of = |i: usize| {
                    let v = g.cells[i];
                    if v[4] > TINY { (v[5] / v[4] / cell).min(DISC) } else { 0.0 }
                };
                let reach = Reach::new(g.w, g.h, &disc_of);
                if reach.none() {
                    return None;
                }
                let cells = (0..g.w * g.h)
                    .into_par_iter()
                    .map(|i| {
                        let (x, y) = ((i % g.w) as i32, (i / g.w) as i32);
                        let far = reach.at(x as usize, y as usize);
                        let mut acc = [0.0f32; 5];
                        if far <= 0.0 {
                            return acc;
                        }
                        let m = (far + 0.5) as i32;
                        for dy in -m..=m {
                            // Past the picture's edge: as at the edge.
                            let yy = (y + dy).clamp(0, g.h as i32 - 1) as usize;
                            for dx in -m..=m {
                                let j = yy * g.w + (x + dx).clamp(0, g.w as i32 - 1) as usize;
                                let q = g.cells[j];
                                if q[4] <= TINY {
                                    continue;
                                }
                                let r = disc_of(j);
                                let k = covered(r, ((dx * dx + dy * dy) as f32).sqrt());
                                if k <= 0.0 {
                                    continue;
                                }
                                let k = k / area(&areas, r);
                                for (a, v) in acc.iter_mut().zip(q) {
                                    *a += k * v;
                                }
                            }
                        }
                        acc
                    })
                    .collect();
                Some((k, Grid { w: g.w, h: g.h, cells }))
            })
            .collect();
        // The picture's own pixels spread directly.
        let reach = Reach::new(w, h, &|i| if spread(i, 0) > 0.0 { (-p.disc[i]).min(DISC) } else { 0.0 });
        let [r, g, b] = out;
        (r.par_chunks_mut(w), g.par_chunks_mut(w), b.par_chunks_mut(w)).into_par_iter().enumerate().for_each(
            |(y, (r, g, b))| {
                for x in 0..w {
                    let mut acc = [0.0f32; 5];
                    let far = reach.at(x, y);
                    if far > 0.0 {
                        let m = (far + 0.5) as i32;
                        for dy in -m..=m {
                            let yy = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                            for dx in -m..=m {
                                let j = yy * w + (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                                let a = spread(j, 0);
                                if a <= 0.0 {
                                    continue;
                                }
                                let c = (-p.disc[j]).min(DISC);
                                let k = covered(c, ((dx * dx + dy * dy) as f32).sqrt());
                                if k <= 0.0 {
                                    continue;
                                }
                                let k = a * k / area(&areas, c);
                                let kh = k * p.boost[j];
                                acc = [
                                    acc[0] + kh * p.rgb[0][j],
                                    acc[1] + kh * p.rgb[1][j],
                                    acc[2] + kh * p.rgb[2][j],
                                    acc[3] + kh,
                                    acc[4] + k,
                                ];
                            }
                        }
                    }
                    for (k, level) in &spreads {
                        for (a, v) in acc.iter_mut().zip(level.at_pixel(x, y, *k)) {
                            *a += v;
                        }
                    }
                    if acc[4] < 1e-4 || acc[3] < 1e-9 {
                        continue;
                    }
                    let i = y * w + x;
                    let k = shown(i);
                    let fill = behind[0].at_pixel(x, y, 1);
                    let under = [
                        k * r[x] + (1.0 - k) * fill[0],
                        k * g[x] + (1.0 - k) * fill[1],
                        k * b[x] + (1.0 - k) * fill[2],
                    ];
                    let alpha = acc[4].min(1.0);
                    r[x] = under[0] + alpha * (acc[0] / acc[3] - under[0]);
                    g[x] = under[1] + alpha * (acc[1] / acc[3] - under[1]);
                    b[x] = under[2] + alpha * (acc[2] / acc[3] - under[2]);
                }
            },
        );
    }
}

/// The picture being blurred: its colour, and each pixel's disc and weight.
struct Picture<'a> {
    rgb: &'a [Vec<f32>; 3],
    /// See [`LensBlur::disc`].
    disc: &'a [f32],
    boost: &'a [f32],
    w: usize,
    h: usize,
}

/// The cells around (x, y) of a `w × h` grid that exist, itself among them.
fn around(w: usize, h: usize, x: i32, y: i32) -> impl Iterator<Item = usize> {
    (-1..=1)
        .flat_map(move |dy| (-1..=1).map(move |dx| (x + dx, y + dy)))
        .filter(move |&(x, y)| x >= 0 && y >= 0 && x < w as i32 && y < h as i32)
        .map(move |(x, y)| y as usize * w + x as usize)
}

/// A level of the pyramid: `N` values a cell.
struct Grid<const N: usize> {
    w: usize,
    h: usize,
    cells: Vec<[f32; N]>,
}

impl<const N: usize> Grid<N> {
    /// Level `k` of a `w × h` picture: each cell the mean of `f` over its pixels.
    fn cells(w: usize, h: usize, k: u32, f: &(dyn Fn(usize) -> [f32; N] + Sync)) -> Grid<N> {
        let s = 1usize << k;
        let (gw, gh) = (w.div_ceil(s), h.div_ceil(s));
        let cells = (0..gw * gh)
            .into_par_iter()
            .map(|i| {
                let (x0, y0) = ((i % gw) * s, (i / gw) * s);
                let (x1, y1) = ((x0 + s).min(w), (y0 + s).min(h));
                let mut sum = [0.0f32; N];
                for y in y0..y1 {
                    for x in x0..x1 {
                        for (a, v) in sum.iter_mut().zip(f(y * w + x)) {
                            *a += v;
                        }
                    }
                }
                let n = ((x1 - x0) * (y1 - y0)) as f32;
                sum.map(|v| v / n)
            })
            .collect();
        Grid { w: gw, h: gh, cells }
    }

    /// Half the size: each cell the mean of the (up to) four under it.
    fn halved(&self) -> Grid<N> {
        let (w, h) = (self.w.div_ceil(2), self.h.div_ceil(2));
        let cells = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let (x1, y1) = ((2 * x + 2).min(self.w), (2 * y + 2).min(self.h));
                let mut sum = [0.0f32; N];
                for yy in 2 * y..y1 {
                    for xx in 2 * x..x1 {
                        for (a, v) in sum.iter_mut().zip(self.cells[yy * self.w + xx]) {
                            *a += v;
                        }
                    }
                }
                let n = ((x1 - 2 * x) * (y1 - 2 * y)) as f32;
                sum.map(|v| v / n)
            })
            .collect();
        Grid { w, h, cells }
    }

    /// Between the cells, at (x, y) in cells (a cell's middle is at its whole number).
    #[inline]
    fn sample(&self, x: f32, y: f32) -> [f32; N] {
        let fx = x.clamp(0.0, (self.w - 1) as f32);
        let fy = y.clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let (a, b) = (&self.cells[y0 * self.w + x0], &self.cells[y0 * self.w + x1]);
        let (c, d) = (&self.cells[y1 * self.w + x0], &self.cells[y1 * self.w + x1]);
        std::array::from_fn(|n| {
            let top = a[n] + tx * (b[n] - a[n]);
            let bottom = c[n] + tx * (d[n] - c[n]);
            top + ty * (bottom - top)
        })
    }

    /// This level (number `k`) at pixel (x, y) of the picture.
    #[inline]
    fn at_pixel(&self, x: usize, y: usize, k: u32) -> [f32; N] {
        let cell = (1u32 << k) as f32;
        self.sample((x as f32 + 0.5) / cell - 0.5, (y as f32 + 0.5) / cell - 0.5)
    }
}

/// How far the widest disc near a cell reaches (at least: it only bounds how far a cell
/// looks), by blocks of [`BLOCK`] cells.
struct Reach {
    w: usize,
    blocks: Vec<f32>,
}

impl Reach {
    /// `disc(i)`: the disc cell `i` of a `w × h` grid spreads over (0: none).
    fn new(w: usize, h: usize, disc: &(dyn Fn(usize) -> f32 + Sync)) -> Reach {
        let (bw, bh) = (w.div_ceil(BLOCK), h.div_ceil(BLOCK));
        let own: Vec<f32> = (0..bw * bh)
            .into_par_iter()
            .map(|i| {
                let (x0, y0) = ((i % bw) * BLOCK, (i / bw) * BLOCK);
                let mut most = 0.0f32;
                for y in y0..(y0 + BLOCK).min(h) {
                    for x in x0..(x0 + BLOCK).min(w) {
                        most = most.max(disc(y * w + x));
                    }
                }
                most
            })
            .collect();
        let blocks = (0..bw * bh)
            .map(|i| around(bw, bh, (i % bw) as i32, (i / bw) as i32).map(|j| own[j]).fold(0.0, f32::max))
            .collect();
        Reach { w: bw, blocks }
    }

    fn none(&self) -> bool {
        self.blocks.iter().all(|v| *v <= 0.0)
    }

    #[inline]
    fn at(&self, x: usize, y: usize) -> f32 {
        self.blocks[(y / BLOCK) * self.w + x / BLOCK]
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
        assert_eq!(b.disc(0.2), 0.0);
        assert_eq!(b.disc(0.29), 0.0, "inside the sharp zone");
        assert!(b.disc(0.5) > 0.0 && b.disc(0.9) > b.disc(0.5));
        assert!((b.disc(1.0) - 48.0).abs() < 0.01, "1.2% of 4000 px");
        assert!(b.disc(0.0) < 0.0, "and in front of it");
        assert!(LensBlur::new(&Effects::default(), 1.0, 4000).is_none());
        assert_eq!(b.grid(), 8);

        // Stripes: sharp on the left (in focus), far on the right.
        let (w, h) = (256usize, 128usize);
        let stripes: Vec<f32> = (0..w * h).map(|k| if (k % w) / 4 % 2 == 0 { 0.8 } else { 0.2 }).collect();
        let mut rgb = [stripes.clone(), stripes.clone(), stripes.clone()];
        b.apply(&mut rgb, w, h, &|x, _| if x < 128 { 0.2 } else { 0.9 });
        let contrast = |from: usize, to: usize| {
            let row = &rgb[1][64 * w + from..64 * w + to];
            row.iter().copied().fold(f32::MIN, f32::max) - row.iter().copied().fold(f32::MAX, f32::min)
        };
        assert_eq!(&rgb[0][64 * w..64 * w + 128], &stripes[64 * w..64 * w + 128], "in focus: untouched");
        assert!(contrast(170, 250) < 0.05, "out of focus: the stripes are gone ({})", contrast(170, 250));
        // The sharp side does not smear into the blurred one: just past the edge it is
        // the far side's own mean (0.5), not pulled towards the last sharp stripe.
        assert!((rgb[1][64 * w + 134] - 0.5).abs() < 0.12, "{}", rgb[1][64 * w + 134]);
    }

    #[test]
    fn the_levels_share_every_disc_whole() {
        for top in 1..6u32 {
            for n in 0..2000 {
                let c = n as f32 * 0.1;
                if c > DISC * (1u32 << top) as f32 {
                    break;
                }
                let sum: f32 = (0..=top).map(|k| share(c, k, top)).sum();
                assert!((sum - 1.0).abs() < 1e-5, "disc {c} of {top} levels: {sum}");
            }
        }
    }

    #[test]
    fn a_bright_point_becomes_a_disc() {
        // One bright point far away on black, everything as far.
        let b = blur(50.0, 0.0, 0.0);
        let (w, h) = (192usize, 192usize);
        let mut rgb: [Vec<f32>; 3] = std::array::from_fn(|_| vec![0.0f32; w * h]);
        for y in 94..98 {
            for x in 94..98 {
                rgb.iter_mut().for_each(|p| p[y * w + x] = 1.0);
            }
        }
        b.apply(&mut rgb, w, h, &|_, _| 1.0);
        // 24 px of disc: flat inside it, nothing outside, and all the light still there.
        let at = |x: usize| rgb[1][96 * w + x];
        assert!(at(96) > 0.0 && (at(96 + 12) / at(96) - 1.0).abs() < 0.2, "{} {}", at(96), at(108));
        assert!(at(96 + 34) < 0.02 * at(96), "{}", at(130));
        let sum: f32 = rgb[1].iter().sum();
        assert!((sum / 16.0 - 1.0).abs() < 0.1, "{sum}");
    }

    #[test]
    fn what_is_blurred_in_front_spreads_over_what_is_sharp_behind() {
        // White in front on the left (blurred), black in focus on the right.
        let b = blur(50.0, 100.0, 0.0);
        let (w, h) = (192usize, 96usize);
        let mut rgb: [Vec<f32>; 3] =
            std::array::from_fn(|_| (0..w * h).map(|k| if k % w < 96 { 1.0 } else { 0.0 }).collect());
        b.apply(&mut rgb, w, h, &|x, _| if x < 96 { 0.0 } else { 1.0 });
        let at = |x: usize| rgb[1][48 * w + x];
        assert!((at(40) - 1.0).abs() < 0.02, "well inside: itself ({})", at(40));
        assert!(at(100) > 0.25 && at(100) < 0.5, "over the edge: some of it ({})", at(100));
        assert!(at(110) > 0.02 && at(110) < at(100), "less farther on ({})", at(110));
        assert_eq!(at(130), 0.0, "past its disc: none");
        // And at the picture's edge it is whole, not thinned out.
        assert!((rgb[1][2] - 1.0).abs() < 0.02, "{}", rgb[1][2]);
    }
}
