//! Dehaze, after the dark channel prior (He, Sun, Tang): in haze-free areas some colour
//! channel is nearly black; haze lifts every channel towards the airlight (the haze's
//! colour). So the darkest channel, over a neighbourhood, says how much haze there is.
//!
//! The haze map is built at low resolution (about 256 px), smoothed so it doesn't show
//! blocks, and sampled bilinearly per pixel.

use rayon::prelude::*;

const MAP_PX: usize = 256;

pub(super) struct HazeMap {
    w: usize,
    h: usize,
    factor: f32,
    /// The dark channel over the airlight (smoothed): 0 = no haze, 1 = all haze.
    dark: Vec<f32>,
    /// The haze's colour (linear working space).
    pub airlight: [f32; 3],
}

impl HazeMap {
    /// Build from `rgb(x, y)`, linear working-space pixels of a `width × height` image.
    pub fn build(width: usize, height: usize, rgb: impl Fn(usize, usize) -> [f32; 3] + Sync) -> HazeMap {
        let f = (width.max(height) as f32 / MAP_PX as f32).max(1.0).ceil() as usize;
        let (w, h) = (width.div_ceil(f), height.div_ceil(f));
        let step = (f / 3).max(1);
        // Per cell: the darkest channel value, and the average colour.
        let cells: Vec<(f32, [f32; 3])> = (0..h)
            .into_par_iter()
            .flat_map_iter(|j| {
                let mut dark = vec![f32::MAX; w];
                let mut sums = vec![[0f32; 3]; w];
                let mut counts = vec![0u32; w];
                for y in (j * f..((j + 1) * f).min(height)).step_by(step) {
                    for x in (0..width).step_by(step) {
                        let p = rgb(x, y);
                        let c = x / f;
                        dark[c] = dark[c].min(p[0].min(p[1]).min(p[2]).max(0.0));
                        for k in 0..3 {
                            sums[c][k] += p[k];
                        }
                        counts[c] += 1;
                    }
                }
                (0..w).map(|c| (dark[c], sums[c].map(|s| s / counts[c].max(1) as f32))).collect::<Vec<_>>()
            })
            .collect();

        // Airlight: the average colour of the haziest 0.5% of cells (brightest dark channel).
        let mut order: Vec<usize> = (0..cells.len()).collect();
        order.sort_unstable_by(|&a, &b| cells[b].0.total_cmp(&cells[a].0));
        let top = &order[..(cells.len() / 200).max(1)];
        let airlight: [f32; 3] = std::array::from_fn(|k| {
            (top.iter().map(|&i| cells[i].1[k]).sum::<f32>() / top.len() as f32).max(1e-4)
        });

        // Dark channel of the image divided by the airlight, over a 5×5-cell neighbourhood.
        let norm = |i: usize| {
            let d = cells[i].0;
            d / airlight[0].min(airlight[1]).min(airlight[2])
        };
        let r = 2isize;
        let dark: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as isize, (i / w) as isize);
                let mut m = f32::MAX;
                for dy in -r..=r {
                    for dx in -r..=r {
                        let (xx, yy) = ((x + dx).clamp(0, w as isize - 1), (y + dy).clamp(0, h as isize - 1));
                        m = m.min(norm(yy as usize * w + xx as usize));
                    }
                }
                m
            })
            .collect();
        // Smooth the blocky minimum so it shows no edges of its own.
        let dark = super::local::box_blur_par(&dark, w, h, 3);
        HazeMap { w, h, factor: f as f32, dark, airlight }
    }

    /// Transmission at full-size pixel (x, y) when removing `amount` (0..1) of the haze:
    /// 1 = clear, near 0 = all haze.
    #[inline]
    pub fn transmission(&self, x: usize, y: usize, amount: f32) -> f32 {
        let fx = ((x as f32 + 0.5) / self.factor - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = ((y as f32 + 0.5) / self.factor - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize| self.dark[y * self.w + x];
        let top = at(x0, y0) + tx * (at(x1, y0) - at(x0, y0));
        let bottom = at(x0, y1) + tx * (at(x1, y1) - at(x0, y1));
        let dark = top + ty * (bottom - top);
        (1.0 - 0.95 * amount * dark).clamp(0.1, 1.0)
    }

    /// Remove `amount` (0..1) of the haze from pixel `p` at (x, y).
    #[inline]
    pub fn clear(&self, p: [f32; 3], x: usize, y: usize, amount: f32) -> [f32; 3] {
        let t = self.transmission(x, y, amount);
        let a = self.airlight;
        [0, 1, 2].map(|k| ((p[k] - a[k]) / t + a[k]).max(0.0))
    }
}

/// Negative Dehaze: add haze, `amount` 0..1, towards `airlight`.
#[inline]
pub(super) fn add_haze(p: [f32; 3], amount: f32, airlight: [f32; 3]) -> [f32; 3] {
    let t = 1.0 - 0.6 * amount;
    [0, 1, 2].map(|k| p[k] * t + airlight[k] * (1.0 - t))
}
