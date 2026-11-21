//! Dust: the Remove tool's Distractions.
//!
//! - **Dust & Scratches** (film scans): many small, sharp defects (specks, hairs,
//!   scratches), dark or light. Found from the scanner's infrared pass (dust and
//!   scratches block infrared, the film's dyes don't), from the picture itself, or both;
//!   kept as a mask picture and filled at render time with a push-pull fill.
//! - **Sensor dust** (digital captures): a few soft, round, slightly darker blobs in
//!   smooth areas, found at several scales; the app turns them into Remove spots.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::WorkingImage;
use super::local::box_blur_par;

/// Where film defects are looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DustSource {
    /// The scanner's infrared pass.
    Ir,
    /// The picture itself.
    #[default]
    Picture,
    /// Either.
    Both,
}

impl DustSource {
    pub const ALL: [DustSource; 3] = [Self::Ir, Self::Picture, Self::Both];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ir => "IR",
            Self::Picture => "Picture",
            Self::Both => "Both",
        }
    }

    pub fn uses_ir(self) -> bool {
        self != Self::Picture
    }
}

/// Which way film defects depart from the picture, seen in the picture. Dust and hairs
/// block light: on a negative that comes out **light** in the positive, on a slide
/// **dark**. Looking one way only leaves the picture's own detail of the other alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DustPolarity {
    /// Scans of negatives (colour or black & white), inverted.
    #[default]
    Light,
    /// Slides.
    Dark,
    /// Either (finds more of the picture's own detail too).
    Both,
}

impl DustPolarity {
    pub const ALL: [DustPolarity; 3] = [Self::Light, Self::Dark, Self::Both];

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Both => "Both",
        }
    }
}

/// Dust & Scratches: what to find, and the mask found (`picture`, in the mask store,
/// made for `made_for` = [`DustLayer::key`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DustLayer {
    pub enabled: bool,
    pub source: DustSource,
    /// 0..100: higher finds fainter defects.
    pub sensitivity: f32,
    /// The thickest defect, in full-size pixels (hairs and scratches can be long).
    pub max_size: f32,
    /// Which way defects look in the picture (for Picture, and Both's picture part).
    pub polarity: DustPolarity,
    /// Picture: find defects with an AI model (slower, cleaner) instead of by filtering.
    pub ai: bool,
    /// The model's map of the photo (probabilities as a picture in the mask store), made
    /// once: Sensitivity and the rest only threshold it.
    pub ai_map: Option<String>,
    /// Painted out of what was found (the Subtract brush): kept whatever is found again.
    pub erase: Vec<super::SpotStroke>,
    pub picture: Option<String>,
    pub made_for: u64,
}

impl Default for DustLayer {
    fn default() -> Self {
        DustLayer {
            enabled: true,
            source: DustSource::Picture,
            sensitivity: 50.0,
            max_size: 12.0,
            polarity: DustPolarity::Light,
            ai: false,
            ai_map: None,
            erase: Vec::new(),
            picture: None,
            made_for: 0,
        }
    }
}

impl DustLayer {
    /// What the mask depends on.
    pub fn key(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for v in [
            self.source as u32,
            self.sensitivity.to_bits(),
            self.max_size.to_bits(),
            self.polarity as u32,
            u32::from(self.uses_ai()),
        ] {
            h = (h ^ u64::from(v)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    /// Found with the AI model: asked for, and looking at the picture alone.
    pub fn uses_ai(&self) -> bool {
        self.ai && self.source == DustSource::Picture
    }

    /// The mask to fill with at `w × h` (any scale of the photo): what was found, less what
    /// was painted out. `None` when off, not found yet, or its picture is missing.
    pub fn mask_at_size(&self, w: usize, h: usize) -> Option<Vec<u8>> {
        let picture = super::masks::mask_picture(self.current()?)?;
        let mut mask = mask_at(&picture, w, h);
        erase_strokes(&mut mask, w, h, &self.erase);
        Some(mask)
    }

    /// The mask to fill with, if on and made for these settings.
    pub fn current(&self) -> Option<&str> {
        self.picture.as_deref().filter(|_| self.enabled && self.made_for == self.key())
    }

    /// On, but its mask is missing or was made for other settings.
    pub fn needs_detection(&self) -> bool {
        self.enabled && self.current().is_none()
    }
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Min (or max) over a `(2r + 1)²` square, separably, by van Herk / Gil-Werman: in blocks
/// of the window's length, running extremes forward and backward, so each output is the
/// extreme of two (three operations a pixel whatever `r`). Columns are done a strip of
/// whole rows at a time, so memory is read in order.
fn min_max_filter(src: &[f32], w: usize, h: usize, r: usize, max: bool) -> Vec<f32> {
    let id = if max { f32::NEG_INFINITY } else { f32::INFINITY };
    let op = move |a: f32, b: f32| if max { a.max(b) } else { a.min(b) };
    let k = 2 * r + 1;
    // Rows.
    let mut rows = vec![0.0f32; w * h];
    rows.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(out, line)| {
        let n = w + 2 * r;
        let at = |p: usize| if p < r || p >= r + w { id } else { line[p - r] };
        let mut fwd = vec![id; n];
        let mut back = vec![id; n];
        for p in 0..n {
            fwd[p] = if p % k == 0 { at(p) } else { op(fwd[p - 1], at(p)) };
        }
        for p in (0..n).rev() {
            back[p] = if p % k == k - 1 || p == n - 1 { at(p) } else { op(back[p + 1], at(p)) };
        }
        for (x, o) in out.iter_mut().enumerate() {
            *o = op(back[x], fwd[x + 2 * r]);
        }
    });
    // Columns, in strips.
    const STRIP: usize = 256;
    let strips: Vec<(usize, usize, Vec<f32>)> = (0..w.div_ceil(STRIP))
        .into_par_iter()
        .map(|s| {
            let x0 = s * STRIP;
            let sw = STRIP.min(w - x0);
            let n = h + 2 * r;
            let row = |p: usize| -> Option<&[f32]> {
                (p >= r && p < r + h).then(|| &rows[(p - r) * w + x0..(p - r) * w + x0 + sw])
            };
            let mut fwd = vec![id; n * sw];
            let mut back = vec![id; n * sw];
            for p in 0..n {
                let (done, cur) = fwd.split_at_mut(p * sw);
                let cur = &mut cur[..sw];
                match (row(p), p % k == 0) {
                    (Some(v), true) => cur.copy_from_slice(v),
                    (Some(v), false) => {
                        for ((c, prev), v) in cur.iter_mut().zip(&done[(p - 1) * sw..]).zip(v) {
                            *c = op(*prev, *v);
                        }
                    }
                    (None, true) => {}
                    (None, false) => cur.copy_from_slice(&done[(p - 1) * sw..p * sw]),
                }
            }
            for p in (0..n).rev() {
                let (cur, later) = back.split_at_mut((p + 1) * sw);
                let cur = &mut cur[p * sw..];
                match (row(p), p % k == k - 1 || p == n - 1) {
                    (Some(v), true) => cur.copy_from_slice(v),
                    (Some(v), false) => {
                        for ((c, next), v) in cur.iter_mut().zip(&later[..sw]).zip(v) {
                            *c = op(*next, *v);
                        }
                    }
                    (None, true) => {}
                    (None, false) => cur.copy_from_slice(&later[..sw]),
                }
            }
            let mut res = vec![0.0f32; h * sw];
            for y in 0..h {
                let (b, f) = (&back[y * sw..(y + 1) * sw], &fwd[(y + 2 * r) * sw..(y + 2 * r + 1) * sw]);
                for ((o, b), f) in res[y * sw..(y + 1) * sw].iter_mut().zip(b).zip(f) {
                    *o = op(*b, *f);
                }
            }
            (x0, sw, res)
        })
        .collect();
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, line)| {
        for (x0, sw, res) in &strips {
            line[*x0..x0 + sw].copy_from_slice(&res[y * sw..(y + 1) * sw]);
        }
    });
    out
}

/// Keep the connected parts of `mask` (non-zero, 8-connected) that are thin enough:
/// area ÷ length (the longer side of their box) at most `thick`, and at least `min_area`
/// pixels. Long hairs stay; big dark shapes (film edges, the picture's own) go.
fn keep_thin(mask: &mut [u8], w: usize, h: usize, thick: f32, min_area: usize) {
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let mut part = Vec::new();
    for start in 0..w * h {
        if mask[start] == 0 || seen[start] {
            continue;
        }
        part.clear();
        stack.push(start);
        seen[start] = true;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        while let Some(i) = stack.pop() {
            part.push(i);
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if mask[j] != 0 && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        let length = ((x1 - x0 + 1).max(y1 - y0 + 1)) as f32;
        let thin = part.len() as f32 / length <= thick;
        if !thin || part.len() < min_area {
            for &i in &part {
                mask[i] = 0;
            }
        }
    }
}

/// Drop the long parts of `mask` (longer than `2 × size`) whose mean response `d` is not
/// at least twice their threshold `thr`.
fn keep_strong_lines(mask: &mut [u8], w: usize, h: usize, size: f32, d: &[f32], thr: &[f32]) {
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let mut part = Vec::new();
    for start in 0..w * h {
        if mask[start] == 0 || seen[start] {
            continue;
        }
        part.clear();
        stack.push(start);
        seen[start] = true;
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        let (mut sd, mut st) = (0.0f32, 0.0f32);
        while let Some(i) = stack.pop() {
            part.push(i);
            sd += d[i];
            st += thr[i];
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if mask[j] != 0 && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        let length = ((x1 - x0 + 1).max(y1 - y0 + 1)) as f32;
        if length > 2.0 * size && sd < 1.2 * st {
            for &i in &part {
                mask[i] = 0;
            }
        }
    }
}

/// The `weak` pixels connected (8-way) to `seeds` (non-zero) through weak pixels, and the
/// seeds; 255 there.
fn flood(seeds: &[u8], weak: &[bool], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    let mut stack: Vec<usize> = (0..w * h).filter(|&i| seeds[i] > 0).collect();
    for &i in &stack {
        out[i] = 255;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        for ny in y.saturating_sub(1)..(y + 2).min(h) {
            for nx in x.saturating_sub(1)..(x + 2).min(w) {
                let j = ny * w + nx;
                if weak[j] && out[j] == 0 {
                    out[j] = 255;
                    stack.push(j);
                }
            }
        }
    }
    out
}

/// Hysteresis: the seeds (non-zero), and the `weak` pixels (non-zero) connected to them
/// within half `size` of a seed; 255 there. A defect takes in its fainter rest, but no
/// grain beyond it.
fn extend(seeds: &[u8], weak: &[f32], w: usize, h: usize, size: f32) -> Vec<u8> {
    let near = grow(seeds, w, h, (size / 2.0).ceil() as usize);
    let weak: Vec<bool> = (0..w * h).map(|i| weak[i] > 0.0 && near[i] > 0).collect();
    let mut out = vec![0u8; w * h];
    let mut stack: Vec<usize> = (0..w * h).filter(|&i| seeds[i] > 0).collect();
    for &i in &stack {
        out[i] = 255;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if weak[j] && out[j] == 0 {
                    out[j] = 255;
                    stack.push(j);
                }
            }
        }
    }
    out
}

/// Grow a mask by `r` pixels (so a defect's soft edge is covered too).
fn grow(mask: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let f: Vec<f32> = mask.iter().map(|v| f32::from(*v)).collect();
    min_max_filter(&f, w, h, r, true).iter().map(|v| *v as u8).collect()
}

/// The IR threshold (a share of the background) for a sensitivity of 0..100.
fn ir_threshold(sensitivity: f32) -> f32 {
    lerp(0.12, 0.015, sensitivity / 100.0)
}

/// How much darker than its surroundings the infrared pass (`ir` 0..1, `w × h`) is at
/// each pixel, as a share (0 where `valid`, NegPy's `_IR_valid`, says it is untrusted),
/// less what the picture (`lum`, perceptual lightness) leaks into it.
fn ir_dip(
    ir: &[f32],
    lum: &[f32],
    valid: Option<&[u8]>,
    w: usize,
    h: usize,
    t: f32,
    max_size: f32,
) -> Vec<f32> {
    // Lightly smoothed: the scanner's noise shrinks, a speck keeps its depth.
    let (ir, pic) = rayon::join(
        || box_blur_par(ir, w, h, 1),
        || box_blur_par(&lum.iter().map(|v| v * v).collect::<Vec<_>>(), w, h, 1),
    );
    let r = ((4.0 * max_size) as usize + 8).min(w.max(h) / 4).max(2);
    // The background, twice: the second time without what the first found darker.
    let (bg0, pbg) = rayon::join(|| box_blur_par(&ir, w, h, r), || box_blur_par(&pic, w, h, r));
    let keep: Vec<f32> =
        ir.iter().zip(&bg0).map(|(v, b)| if *v > b * (1.0 - t) { 1.0 } else { 0.0 }).collect();
    let num: Vec<f32> = ir.iter().zip(&keep).map(|(v, k)| v * k).collect();
    let (sn, sk) = rayon::join(|| box_blur_par(&num, w, h, r), || box_blur_par(&keep, w, h, r));
    let ok = |i: usize| valid.is_none_or(|v| v[i] != 0);
    let dip: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let bg = if sk[i] > 1e-3 { sn[i] / sk[i] } else { bg0[i] };
            if ok(i) && bg > 1e-4 { (bg - ir[i]) / bg } else { 0.0 }
        })
        .collect();
    // The picture's own departures from its surroundings (darker > 0), and how much of them
    // shows in the infrared: scanners see a little of the film's dyes in infrared (about a
    // tenth here), so dense picture detail would pass for dust. The share is measured where
    // the picture departs a lot (dust is too rare to sway the median) and taken away.
    let dev: Vec<f32> =
        (0..w * h).into_par_iter().map(|i| if pbg[i] > 1e-5 { 1.0 - pic[i] / pbg[i] } else { 0.0 }).collect();
    let k = crosstalk(&dip, &dev, ok);
    dip.iter().zip(&dev).map(|(d, p)| d - k * p).collect()
}

/// The median share of the picture's departures (`dev`) that shows in the infrared's
/// (`dip`), where the picture departs by more than a quarter; 0 when there are too few such
/// places to tell. Kept within ±0.5.
fn crosstalk(dip: &[f32], dev: &[f32], ok: impl Fn(usize) -> bool) -> f32 {
    let mut ratios: Vec<f32> = (0..dip.len())
        .step_by(7)
        .filter(|&i| ok(i) && dev[i].abs() > 0.25)
        .map(|i| dip[i] / dev[i])
        .collect();
    if ratios.len() < 50 {
        return 0.0;
    }
    let mid = ratios.len() / 2;
    let (_, m, _) = ratios.select_nth_unstable_by(mid, f32::total_cmp);
    m.clamp(-0.5, 0.5)
}

/// Film defects from the infrared pass (`ir` 0..1, `w × h`): where it is darker than its
/// surroundings by more than the sensitivity's share. `lum` is the picture (perceptual
/// lightness), used to take away what of it shows in the infrared. `valid` (NegPy's
/// `_IR_valid`): non-zero where the infrared can be trusted. Returns a mask, 255 at defects.
pub fn detect_ir(
    ir: &[f32],
    lum: &[f32],
    valid: Option<&[u8]>,
    w: usize,
    h: usize,
    sensitivity: f32,
    max_size: f32,
) -> Vec<u8> {
    let t = ir_threshold(sensitivity);
    let dip = ir_dip(ir, lum, valid, w, h, t, max_size);
    ir_mask(&dip, w, h, t, max_size)
}

fn ir_mask(dip: &[f32], w: usize, h: usize, t: f32, max_size: f32) -> Vec<u8> {
    // Defects judged on where they clearly stand out, then taking in their fainter rest.
    let mut seeds: Vec<u8> = dip.iter().map(|v| if *v > t { 255 } else { 0 }).collect();
    keep_thin(&mut seeds, w, h, max_size, 2);
    let weak: Vec<f32> = dip.iter().map(|v| if *v > 0.45 * t { 1.0 } else { 0.0 }).collect();
    grow(&extend(&seeds, &weak, w, h, max_size), w, h, 1)
}

/// Film defects from both the picture and the infrared pass: what the infrared finds, and
/// what the picture finds where the infrared shows at least a faint dip too, so the
/// picture's own fine detail (wires, edges, highlights) is left alone while the picture's
/// sharper view adds small specks the infrared only hints at. `layer` gives the
/// sensitivity, Max Size and polarity.
pub fn detect_both(
    lum: &[f32],
    ir: &[f32],
    valid: Option<&[u8]>,
    w: usize,
    h: usize,
    layer: &DustLayer,
) -> Vec<u8> {
    let (sensitivity, max_size, polarity) = (layer.sensitivity, layer.max_size, layer.polarity);
    let t = ir_threshold(sensitivity);
    let (dip, mut pic) = rayon::join(
        || ir_dip(ir, lum, valid, w, h, t, max_size),
        || detect_picture_as(lum, w, h, sensitivity, max_size, polarity),
    );
    // Confirmed: the infrared, over the whole of what the picture found, dips by a good
    // part of its threshold (the picture's own detail shows no dip, once its share is
    // taken away; a speck too small to clear the threshold in infrared still does).
    keep_parts(&mut pic, w, h, |part| {
        part.iter().map(|&i| dip[i]).sum::<f32>() > 0.4 * t * part.len() as f32
    });
    let found = ir_mask(&dip, w, h, t, max_size);
    pic.iter().zip(&found).map(|(a, b)| *a.max(b)).collect()
}

/// Whether a part (pixel indices in a `w`-wide image) is a long straight line: longer
/// than `2 × size`, and hardly wider across its main direction than a line is.
fn straight(part: &[usize], w: usize, size: f32) -> bool {
    let n = part.len() as f32;
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    for &i in part {
        sx += (i % w) as f32;
        sy += (i / w) as f32;
    }
    let (mx, my) = (sx / n, sy / n);
    let (mut xx, mut yy, mut xy) = (0.0f32, 0.0f32, 0.0f32);
    for &i in part {
        let (dx, dy) = ((i % w) as f32 - mx, (i / w) as f32 - my);
        xx += dx * dx;
        yy += dy * dy;
        xy += dx * dy;
    }
    let (xx, yy, xy) = (xx / n, yy / n, xy / n);
    // The spreads along and across the main direction.
    let mid = (xx + yy) / 2.0;
    let dev = (((xx - yy) / 2.0).powi(2) + xy * xy).sqrt();
    let (along, across) = ((mid + dev).sqrt(), (mid - dev).max(0.0).sqrt());
    let length = along * 12f32.sqrt();
    length > 2.0 * size && across < 1.5f32.max(0.025 * length)
}

/// Keep the connected parts (non-zero, 8-connected) of `mask` that `keep` accepts.
fn keep_parts(mask: &mut [u8], w: usize, h: usize, keep: impl Fn(&[usize]) -> bool) {
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let mut part = Vec::new();
    for start in 0..w * h {
        if mask[start] == 0 || seen[start] {
            continue;
        }
        part.clear();
        stack.push(start);
        seen[start] = true;
        while let Some(i) = stack.pop() {
            part.push(i);
            let (x, y) = (i % w, i / w);
            for ny in y.saturating_sub(1)..(y + 2).min(h) {
                for nx in x.saturating_sub(1)..(x + 2).min(w) {
                    let j = ny * w + nx;
                    if mask[j] != 0 && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if !keep(&part) {
            for &i in &part {
                mask[i] = 0;
            }
        }
    }
}

/// Film defects from the picture (`lum`: perceptual lightness 0..1, `w × h`): specks,
/// hairs and scratches thinner than `max_size` that stand out from their surroundings —
/// dark or light — by more than the film's grain (measured locally) times a factor the
/// sensitivity sets. Returns a mask, 255 at defects.
pub fn detect_picture(lum: &[f32], w: usize, h: usize, sensitivity: f32, max_size: f32) -> Vec<u8> {
    detect_picture_as(lum, w, h, sensitivity, max_size, DustPolarity::Both)
}

/// How far each pixel of `lum` stands out, the way `polarity` asks, from what is around it
/// within `r` pixels: closing (max then min) fills dark features smaller than the window,
/// opening (min then max) removes light ones; their differences from the picture.
fn departure(lum: &[f32], w: usize, h: usize, r: usize, polarity: DustPolarity) -> Vec<f32> {
    let closing = || min_max_filter(&min_max_filter(lum, w, h, r, true), w, h, r, false);
    let opening = || min_max_filter(&min_max_filter(lum, w, h, r, false), w, h, r, true);
    match polarity {
        DustPolarity::Light => {
            let o = opening();
            lum.par_iter().zip(&o).map(|(v, o)| v - o).collect()
        }
        DustPolarity::Dark => {
            let c = closing();
            lum.par_iter().zip(&c).map(|(v, c)| c - v).collect()
        }
        DustPolarity::Both => {
            let (c, o) = rayon::join(closing, opening);
            (0..w * h).into_par_iter().map(|i| (c[i] - lum[i]).max(lum[i] - o[i])).collect()
        }
    }
}

/// [`detect_picture`] looking one way (or both): see [`DustPolarity`].
pub fn detect_picture_as(
    lum: &[f32],
    w: usize,
    h: usize,
    sensitivity: f32,
    max_size: f32,
    polarity: DustPolarity,
) -> Vec<u8> {
    let s = sensitivity / 100.0;
    let r = (max_size / 2.0).ceil() as usize + 1;
    // Lightly smoothed: grain extremes shrink a lot, a speck of a few pixels keeps its depth.
    let smooth = box_blur_par(lum, w, h, 1);
    let lum = smooth.as_slice();
    // How far each pixel stands out (darker or lighter than what is around it at this
    // size), and how much that is usual here: film grain clumps stand out a little
    // everywhere, a speck far more than its surroundings do.
    let d = departure(lum, w, h, r, polarity);
    // Measured twice, the second time with the defects clipped, so a long hair does
    // not raise the usual level along itself.
    let reach = (4 * r).max(12);
    let rough = box_blur_par(&d, w, h, reach);
    let clipped: Vec<f32> = d.iter().zip(&rough).map(|(v, u)| v.min(2.0 * u)).collect();
    let usual = box_blur_par(&clipped, w, h, reach);
    // Looking one way, the usual level is about half: stricter factors keep the same
    // sensitivity meaning about as much (measured on annotated film scans).
    let (k, floor) = match polarity {
        DustPolarity::Both => (lerp(9.0, 3.5, s), lerp(0.08, 0.02, s)),
        _ => (lerp(11.0, 5.5, s), lerp(0.1, 0.04, s)),
    };
    let thr: Vec<f32> = usual.iter().map(|u| (k * u).max(floor)).collect();
    // Defects judged on where they clearly stand out: thin enough, and long thin shapes
    // (often the picture's own: wires, edges, letters) only when very strong; then
    // taking in their fainter rest.
    let mut seeds: Vec<u8> = (0..w * h).map(|i| if d[i] > thr[i] { 255 } else { 0 }).collect();
    keep_thin(&mut seeds, w, h, max_size, 3);
    keep_strong_lines(&mut seeds, w, h, max_size, &d, &thr);
    // Long and straight: a frame, an edge or the film border (hairs curl). Straight
    // scratches are lost too, and can be painted out with spots.
    keep_parts(&mut seeds, w, h, |part| !straight(part, w, max_size));
    // Hairs too faint to stand out pixel by pixel in heavy grain, found along their length.
    let hairs = hair_seeds(lum, w, h, s, max_size, polarity);
    for (a, b) in seeds.iter_mut().zip(&hairs) {
        *a = (*a).max(*b);
    }
    let weak: Vec<f32> = (0..w * h).map(|i| if d[i] > 0.45 * thr[i] { 1.0 } else { 0.0 }).collect();
    grow(&extend(&seeds, &weak, w, h, max_size), w, h, 1)
}

/// Hairs and curled scratches in heavy grain (`lum`, lightly smoothed): how far each pixel
/// stands out from what is around it within a few pixels (a hair is thin; the picture's own
/// lines are mostly wider and softer), averaged along short lines in 8 directions, the
/// strongest kept. Grain averages away; a hair keeps its strength along itself. Where that
/// stands well above its surroundings' level (by their spread, times a factor the
/// sensitivity `s` 0..1 sets) a hair starts, and is followed along its fainter rest; it
/// must be long, thin and curling. Worked at half size. Returns a mask, 255 at the hairs'
/// pixels that stand out.
fn hair_seeds(lum: &[f32], w: usize, h: usize, s: f32, max_size: f32, polarity: DustPolarity) -> Vec<u8> {
    const L: isize = 6;
    let (hw, hh) = (w / 2, h / 2);
    if (hw as isize) < 4 * L || (hh as isize) < 4 * L {
        return vec![0; w * h];
    }
    let d = departure(lum, w, h, 2, polarity);
    let usual = box_blur_par(&d, w, h, 12);
    let half: Vec<f32> = (0..hw * hh)
        .into_par_iter()
        .map(|k| {
            let (x, y) = (k % hw * 2, k / hw * 2);
            (d[y * w + x] + d[y * w + x + 1] + d[(y + 1) * w + x] + d[(y + 1) * w + x + 1]) / 4.0
        })
        .collect();
    let dirs: Vec<Vec<isize>> = (0..8)
        .map(|k| {
            let a = k as f32 * std::f32::consts::PI / 8.0;
            (-L..=L)
                .map(|t| {
                    let (dx, dy) =
                        ((t as f32 * a.cos()).round() as isize, (t as f32 * a.sin()).round() as isize);
                    dy * hw as isize + dx
                })
                .collect()
        })
        .collect();
    let n = (2 * L + 1) as f32;
    let line: Vec<f32> = (0..hw * hh)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % hw) as isize, (i / hw) as isize);
            if x < L || y < L || x >= hw as isize - L || y >= hh as isize - L {
                return 0.0;
            }
            dirs.iter()
                .map(|o| o.iter().map(|off| half[(i as isize + off) as usize]).sum::<f32>() / n)
                .fold(0.0, f32::max)
        })
        .collect();
    // The surroundings' level and spread, with the hairs themselves clipped.
    let reach = 24;
    let rough = box_blur_par(&line, hw, hh, reach);
    let clipped: Vec<f32> = line.iter().zip(&rough).map(|(v, u)| v.min(1.5 * u)).collect();
    let level = box_blur_par(&clipped, hw, hh, reach);
    let sq: Vec<f32> = clipped.iter().zip(&level).map(|(v, u)| (v - u) * (v - u)).collect();
    let spread = box_blur_par(&sq, hw, hh, reach);
    let k = lerp(6.0, 3.0, s);
    // Judged at half size, where a hair's hits join up.
    let z: Vec<f32> =
        (0..hw * hh).map(|i| (line[i] - level[i] - 1e-3) / spread[i].sqrt().max(1e-6)).collect();
    let strong: Vec<u8> = z.iter().map(|v| if *v > k { 255 } else { 0 }).collect();
    let weak: Vec<bool> = z.iter().map(|v| *v > lerp(3.5, 2.5, s)).collect();
    let mut hit = flood(&strong, &weak, hw, hh);
    keep_thin(&mut hit, hw, hh, 5.0, 1);
    let long = 0.75 * max_size + 3.0;
    keep_parts(&mut hit, hw, hh, |part| {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for &i in part {
            let (x, y) = (i % hw, i / hw);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        ((x1 - x0 + 1).max(y1 - y0 + 1)) as f32 >= long && !straight(part, hw, max_size / 2.0)
    });
    // Back at full size: the hair's own pixels there.
    (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % w / 2).min(hw - 1), (i / w / 2).min(hh - 1));
            if hit[y * hw + x] > 0 && d[i] > usual[i] { 255 } else { 0 }
        })
        .collect()
}

/// Below this (log of perceptual lightness, about 0.12) no sensor dust is looked for.
const DARK: f32 = -2.1;

/// Sensor dust in a photo's lightness (`lum` 0..1, `w × h`, any size): soft round blobs
/// a little darker than their smooth surroundings. Returns their centres (0..1 of the
/// photo) and radii (in half long sides, as spots' sizes), strongest first.
pub fn find_sensor_dust(lum: &[f32], w: usize, h: usize, sensitivity: f32) -> Vec<([f32; 2], f32)> {
    let s = sensitivity / 100.0;
    // Work at about 1500 px: dust is soft, so nothing is lost.
    let f = (w.max(h) as f32 / 1500.0).ceil().max(1.0) as usize;
    let (sw, sh) = (w / f, h / f);
    if sw < 16 || sh < 16 {
        return Vec::new();
    }
    let small: Vec<f32> = (0..sw * sh)
        .into_par_iter()
        .map(|k| {
            let (x, y) = (k % sw * f, k / sw * f);
            let mut sum = 0.0;
            for dy in 0..f {
                for dx in 0..f {
                    sum += lum[(y + dy) * w + x + dx];
                }
            }
            (sum / (f * f) as f32).max(1e-4).ln()
        })
        .collect();
    let blur = |src: &[f32], r: usize| {
        box_blur_par(&box_blur_par(&box_blur_par(src, sw, sh, r), sw, sh, r), sw, sh, r)
    };
    // Fine detail: how far each pixel is from its 3×3 mean. Soft dust adds next to none;
    // texture, edges and lettering are full of it. The photo's quiet level is its 30th
    // percentile, measured over patches (grain and noise set it).
    let fine: Vec<f32> = {
        let b = box_blur_par(&small, sw, sh, 1);
        small.iter().zip(&b).map(|(a, b)| (a - b).abs()).collect()
    };
    let quiet = {
        let mut patches: Vec<f32> = box_blur_par(&fine, sw, sh, 8).into_iter().step_by(37).collect();
        let k = patches.len() * 3 / 10;
        if patches.is_empty() { 0.0 } else { *patches.select_nth_unstable_by(k, f32::total_cmp).1 }
    };
    // How dark a blob must be (in log lightness: 0.01 ≈ 1% darker): at least a share the
    // sensitivity sets, and well above the noise where it is. Sensor dust is faint: a few %.
    let t_min = lerp(0.012, 0.002, s);
    let k_noise = lerp(4.0, 1.2, s);
    let mut found: Vec<(f32, f32, f32, f32)> = Vec::new(); // (strength, x, y, radius) in small px
    for r in [1usize, 2, 3, 5, 8] {
        let inner = blur(&small, r);
        let outer = blur(&small, 2 * r + 1);
        // Difference of blurs: negative at a dark blob of about this size.
        let dog: Vec<f32> = inner.iter().zip(&outer).map(|(a, b)| a - b).collect();
        // How busy it is around each pixel: its fine detail (noise, grain, texture).
        let busy = box_blur_par(&fine, sw, sh, 4 * r + 4);
        // A blurred blob of this size keeps about this much of the noise's fine detail.
        let keep = 1.0 / (r as f32 + 1.0);
        let ring = (2 * r + 2) as isize;
        let edge = 3 * r + 3;
        for y in edge..sh.saturating_sub(edge) {
            for x in edge..sw.saturating_sub(edge) {
                let i = y * sw + x;
                let v = dog[i];
                let noise = busy[i] * keep;
                if v > -t_min.max(k_noise * noise) {
                    continue;
                }
                // A local minimum over its own size.
                let rr = r.max(1) as isize;
                let mut lowest = true;
                'n: for dy in -rr..=rr {
                    for dx in -rr..=rr {
                        if (dx, dy) != (0, 0)
                            && dog[(y as isize + dy) as usize * sw + (x as isize + dx) as usize] < v
                        {
                            lowest = false;
                            break 'n;
                        }
                    }
                }
                if !lowest {
                    continue;
                }
                // Not in deep shadow: noise is large there in log terms, and dust hardly shows.
                if inner[i] < DARK {
                    continue;
                }
                // No busier than the photo's quiet parts.
                if busy[i] > 2.0 * quiet + 0.002 {
                    continue;
                }
                // Darker than all around it, and the surroundings alike (an edge or a
                // shadow's side has a dark side; a gradient changes across).
                let around: Vec<f32> = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)]
                    .iter()
                    .map(|&(dx, dy): &(isize, isize)| {
                        let (rx, ry) = (x as isize + dx * ring, y as isize + dy * ring);
                        inner[ry as usize * sw + rx as usize]
                    })
                    .collect();
                let (lo, hi) = around.iter().fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
                if inner[i] > lo - 0.5 * -v || hi - lo > 1.5 * -v + 3.0 * noise {
                    continue;
                }
                found.push((-v, x as f32 + 0.5, y as f32 + 0.5, 1.6 * r as f32 + 1.0));
            }
        }
    }
    // Strongest first; one per place.
    found.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut kept: Vec<(f32, f32, f32, f32)> = Vec::new();
    for c in found {
        if kept.iter().all(|k| (k.1 - c.1).hypot(k.2 - c.2) > k.3.max(c.3)) {
            kept.push(c);
        }
        if kept.len() >= 200 {
            break;
        }
    }
    let half = (w.max(h) as f32) / 2.0;
    kept.iter()
        .map(|&(_, x, y, r)| ([x * f as f32 / w as f32, y * f as f32 / h as f32], r * f as f32 * 1.3 / half))
        .collect()
}

/// A push-pull level: its size, colours, and how known each pixel is (0..1).
type Level = (usize, usize, Vec<[f32; 3]>, Vec<f32>);

/// Fill the pixels a mask covers (`mask` 0..255, `w × h`) in interleaved RGB `px`, from
/// their surroundings: a push-pull pyramid (known pixels averaged down by halves until
/// every hole has a value, then the coarser levels pulled back up into the holes).
pub fn fill_masked(px: &mut [f32], w: usize, h: usize, mask: &[u8]) {
    if !mask.iter().any(|m| *m > 0) {
        return;
    }
    // Level 0: colour and how known each pixel is.
    let mut levels: Vec<Level> = Vec::new();
    let c0: Vec<[f32; 3]> = px.chunks(3).map(|p| [p[0], p[1], p[2]]).collect();
    let a0: Vec<f32> = mask.iter().map(|m| 1.0 - f32::from(*m) / 255.0).collect();
    levels.push((w, h, c0, a0));
    // Down: weighted 2 × 2 averages.
    loop {
        let (lw, lh, c, a) = levels.last().expect("a level");
        if (*lw <= 1 && *lh <= 1) || a.iter().all(|v| *v >= 1.0) {
            break;
        }
        let (nw, nh) = (lw.div_ceil(2), lh.div_ceil(2));
        let (lw, lh) = (*lw, *lh);
        let (mut nc, mut na) = (vec![[0.0f32; 3]; nw * nh], vec![0.0f32; nw * nh]);
        nc.par_chunks_mut(nw).zip(na.par_chunks_mut(nw)).enumerate().for_each(|(y, (crow, arow))| {
            for x in 0..nw {
                let (mut sum, mut wsum) = ([0.0f32; 3], 0.0f32);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((2 * x + dx).min(lw - 1), (2 * y + dy).min(lh - 1));
                    let k = sy * lw + sx;
                    let wt = a[k];
                    for ch in 0..3 {
                        sum[ch] += wt * c[k][ch];
                    }
                    wsum += wt;
                }
                if wsum > 0.0 {
                    crow[x] = sum.map(|v| v / wsum);
                }
                arow[x] = wsum.min(1.0);
            }
        });
        levels.push((nw, nh, nc, na));
    }
    // Up: each level's unknown part from the (already filled) coarser level, bilinear.
    for l in (0..levels.len() - 1).rev() {
        let (cw, ch_, coarse) = {
            let (cw, ch_, cc, _) = &levels[l + 1];
            (*cw, *ch_, cc.clone())
        };
        let (lw, _, c, a) = &mut levels[l];
        let lw = *lw;
        c.par_chunks_mut(lw).zip(a.par_chunks_mut(lw)).enumerate().for_each(|(y, (crow, arow))| {
            for x in 0..lw {
                if arow[x] >= 1.0 {
                    continue;
                }
                let fx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (cw - 1) as f32);
                let fy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (ch_ - 1) as f32);
                let (x0, y0) = (fx as usize, fy as usize);
                let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch_ - 1));
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let at = |x: usize, y: usize| coarse[y * cw + x];
                let v: [f32; 3] = std::array::from_fn(|k| {
                    let top = at(x0, y0)[k] + tx * (at(x1, y0)[k] - at(x0, y0)[k]);
                    let bottom = at(x0, y1)[k] + tx * (at(x1, y1)[k] - at(x0, y1)[k]);
                    top + ty * (bottom - top)
                });
                let wt = arow[x];
                crow[x] = std::array::from_fn(|k| wt * crow[x][k] + (1.0 - wt) * v[k]);
                arow[x] = 1.0;
            }
        });
    }
    let c = &levels[0].2;
    px.par_chunks_mut(3).zip(c.par_iter()).for_each(|(p, v)| p.copy_from_slice(v));
}

/// A mask (`picture`, any size) at `w × h`: each pixel the most any of the mask's pixels
/// under it covers (so specks smaller than a pixel still count), bilinear when enlarging.
pub fn mask_at(picture: &image::GrayImage, w: usize, h: usize) -> Vec<u8> {
    let (pw, ph) = (picture.width() as usize, picture.height() as usize);
    let raw = picture.as_raw();
    if pw == w && ph == h {
        return raw.clone();
    }
    let (sx, sy) = (pw as f32 / w as f32, ph as f32 / h as f32);
    (0..w * h)
        .into_par_iter()
        .map(|k| {
            let (x, y) = (k % w, k / w);
            let (x0, y0) = ((x as f32 * sx) as usize, (y as f32 * sy) as usize);
            let (x1, y1) = (
                (((x + 1) as f32 * sx).ceil() as usize).min(pw).max(x0 + 1),
                (((y + 1) as f32 * sy).ceil() as usize).min(ph).max(y0 + 1),
            );
            let mut m = 0u8;
            for yy in y0.min(ph - 1)..y1 {
                for xx in x0.min(pw - 1)..x1 {
                    m = m.max(raw[yy * pw + xx]);
                }
            }
            m
        })
        .collect()
}

/// Clear `mask` (`w × h`) under `strokes`: paths in 0..1 of the photo, their radius in
/// half long sides (as spots'), round ends.
pub fn erase_strokes(mask: &mut [u8], w: usize, h: usize, strokes: &[super::SpotStroke]) {
    let half = w.max(h) as f32 / 2.0;
    for stroke in strokes {
        let r = (stroke.size * half).max(0.5);
        let pts: Vec<(f32, f32)> = stroke.points.iter().map(|p| (p[0] * w as f32, p[1] * h as f32)).collect();
        let segs: Vec<((f32, f32), (f32, f32))> = match pts.as_slice() {
            [] => continue,
            [p] => vec![(*p, *p)],
            _ => pts.windows(2).map(|s| (s[0], s[1])).collect(),
        };
        for (a, b) in segs {
            let (x0, x1) =
                ((a.0.min(b.0) - r).floor().max(0.0) as usize, ((a.0.max(b.0) + r).ceil() as usize).min(w));
            let (y0, y1) =
                ((a.1.min(b.1) - r).floor().max(0.0) as usize, ((a.1.max(b.1) + r).ceil() as usize).min(h));
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len2 = (dx * dx + dy * dy).max(1e-6);
            for y in y0..y1 {
                for x in x0..x1 {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let t = (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0);
                    let (qx, qy) = (a.0 + t * dx, a.1 + t * dy);
                    if (px - qx).hypot(py - qy) <= r {
                        mask[y * w + x] = 0;
                    }
                }
            }
        }
    }
}

/// What the detectors look at: the photo's perceptual lightness (0..1), file orientation.
pub fn dust_lightness(img: &WorkingImage) -> Vec<f32> {
    img.pixels
        .as_raw()
        .par_chunks(3)
        .map(|p| (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).clamp(0.0, 1.0).sqrt())
        .collect()
}

/// The infrared pass for Dust & Scratches: its pixels (0..1, the photo's size) and, if
/// there is one, the mask of where they can be trusted (non-zero).
pub struct IrPass {
    pub pixels: Vec<f32>,
    pub valid: Option<Vec<u8>>,
}

/// Dust & Scratches' mask from the AI model's map (`map`: probabilities 0..255, `w × h`) for
/// `layer`: above a level the sensitivity sets, no thicker than Max Size, and departing the
/// way the polarity asks from the picture around (`lum`, lightness).
pub fn dust_from_map(map: &[u8], lum: &[f32], w: usize, h: usize, layer: &DustLayer) -> Vec<u8> {
    let s = layer.sensitivity / 100.0;
    let level = (lerp(0.65, 0.2, s) * 255.0) as u8;
    let mut mask: Vec<u8> = map.iter().map(|v| if *v >= level { 255 } else { 0 }).collect();
    keep_thin(&mut mask, w, h, 1.5 * layer.max_size, 2);
    if layer.polarity != DustPolarity::Both {
        let around = box_blur_par(lum, w, h, (2.0 * layer.max_size) as usize + 6);
        let light = layer.polarity == DustPolarity::Light;
        keep_parts(&mut mask, w, h, |part| {
            let dev: f32 = part.iter().map(|&i| lum[i] - around[i]).sum();
            (dev > 0.0) == light
        });
    }
    grow(&mask, w, h, 1)
}

/// Dust & Scratches' mask for `layer`, from the photo's lightness (`lum`, `w × h`, see
/// [`dust_lightness`]) and its infrared pass if it has one (without, IR and Both look at
/// the picture alone). 255 at defects.
pub fn detect_dust(lum: &[f32], w: usize, h: usize, layer: &DustLayer, ir: Option<&IrPass>) -> Vec<u8> {
    let (sens, size) = (layer.sensitivity, layer.max_size);
    match (layer.source, ir) {
        (DustSource::Ir, Some(ir)) => detect_ir(&ir.pixels, lum, ir.valid.as_deref(), w, h, sens, size),
        (DustSource::Both, Some(ir)) => detect_both(lum, &ir.pixels, ir.valid.as_deref(), w, h, layer),
        _ => detect_picture_as(lum, w, h, sens, size, layer.polarity),
    }
}

/// Sensor dust as Heal spots (marked `found`), each with a source picked from the photo.
pub fn sensor_dust_spots(img: &WorkingImage, sensitivity: f32, feather: f32) -> Vec<super::Spot> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    find_sensor_dust(&dust_lightness(img), w, h, sensitivity)
        .into_iter()
        .map(|(c, r)| {
            let mut spot = super::Spot {
                mode: super::SpotMode::Heal,
                points: vec![c],
                size: r,
                feather,
                opacity: 100.0,
                found: true,
                ..Default::default()
            };
            spot.offset = super::find_source(img, &spot, None);
            spot
        })
        .collect()
}

/// Dust & Scratches' found defects over the photo as shown (after the Transform and
/// crop), at `img`'s scale: 255 at defects. `None` while there is no current mask.
pub fn render_dust_overlay(img: &WorkingImage, s: &super::DevelopSettings) -> Option<image::GrayImage> {
    let (w, h) = (img.width(), img.height());
    let at = s.dust.as_ref()?.mask_at_size(w as usize, h as usize)?;
    if s.transform.is_identity() {
        return image::GrayImage::from_raw(w, h, at);
    }
    let map = super::transform::Mapping::new(&s.transform, w, h);
    let (ow, oh) = map.out;
    let px: Vec<u8> = (0..ow as usize * oh as usize)
        .into_par_iter()
        .map(|i| {
            let (x, y) = ((i % ow as usize) as f64 + 0.5, (i / ow as usize) as f64 + 0.5);
            match map.source(x, y) {
                Some((u, v)) if u >= 0.0 && v >= 0.0 && u < f64::from(w) && v < f64::from(h) => {
                    at[v as usize * w as usize + u as usize]
                }
                _ => 0,
            }
        })
        .collect();
    image::GrayImage::from_raw(ow, oh, px)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mid-grey with grain (a fixed pseudo-random pattern, ±2%).
    fn grainy(w: usize, h: usize) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let v = ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40) as f32 / (1u64 << 24) as f32;
                0.5 + 0.04 * (v - 0.5)
            })
            .collect()
    }

    #[test]
    fn min_max_filter_finds_extremes_in_the_window() {
        let src: Vec<f32> = (0..25).map(|i| i as f32).collect();
        let max = min_max_filter(&src, 5, 5, 1, true);
        assert_eq!(max[0], 6.0); // (0..1, 0..1) → 6
        assert_eq!(max[12], 18.0);
        let min = min_max_filter(&src, 5, 5, 1, false);
        assert_eq!(min[24], 18.0);
        // Against the obvious way, odd sizes and windows past the edges.
        let (w, h) = (37, 23);
        let src: Vec<f32> = (0..w * h).map(|i| ((i * 7919) % 101) as f32).collect();
        for r in 0..6 {
            for max in [false, true] {
                let got = min_max_filter(&src, w, h, r, max);
                for y in 0..h {
                    for x in 0..w {
                        let mut want = if max { f32::MIN } else { f32::MAX };
                        for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                            for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                                let v = src[yy * w + xx];
                                want = if max { want.max(v) } else { want.min(v) };
                            }
                        }
                        assert_eq!(got[y * w + x], want, "r {r} max {max} at {x},{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn picture_detection_finds_specks_and_hairs_but_not_grain() {
        let (w, h) = (200, 150);
        let mut lum = grainy(w, h);
        assert!(detect_picture(&lum, w, h, 50.0, 10.0).iter().all(|m| *m == 0), "grain alone");
        // A dark speck, a light speck, and a long thin dark hair.
        for (cx, cy, v) in [(40.0f32, 40.0f32, 0.2f32), (120.0, 50.0, 0.85)] {
            for y in 0..h {
                for x in 0..w {
                    if (x as f32 - cx).hypot(y as f32 - cy) < 3.0 {
                        lum[y * w + x] = v;
                    }
                }
            }
        }
        // The hair curls (long straight lines are taken for the picture's own).
        for x in 30..170 {
            let y = (110.0 + 8.0 * ((x as f32 - 100.0) / 18.0).sin()).round() as usize;
            lum[y * w + x] = 0.25;
            lum[(y + 1) * w + x] = 0.25;
        }
        let m = detect_picture(&lum, w, h, 50.0, 10.0);
        assert_eq!(m[40 * w + 40], 255, "dark speck");
        assert_eq!(m[50 * w + 120], 255, "light speck");
        assert_eq!(m[110 * w + 100], 255, "hair");
        assert_eq!(m[80 * w + 80], 0, "clean grain");
        // A big dark shape (the picture's own) is left alone.
        let mut big = grainy(w, h);
        for y in 20..120 {
            for x in 20..120 {
                big[y * w + x] = 0.15;
            }
        }
        assert!(detect_picture(&big, w, h, 50.0, 10.0).iter().all(|m| *m == 0), "a large shape");
    }

    #[test]
    fn a_faint_hair_in_heavy_grain_is_followed_but_a_soft_line_is_not() {
        // Grain ±12%, a 2 px hair only 0.07 lighter, and a soft 7 px wide drawn curve.
        let (w, h) = (400, 300);
        let mut lum: Vec<f32> = (0..w * h)
            .map(|i| {
                let v = ((i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40) as f32 / (1u64 << 24) as f32;
                0.5 + 0.24 * (v - 0.5)
            })
            .collect();
        let hair = |x: usize| (100.0 + 14.0 * ((x as f32 - 60.0) / 30.0).sin()).round() as usize;
        for x in 60..340 {
            let y = hair(x);
            for yy in y..y + 2 {
                lum[yy * w + x] += 0.07;
            }
        }
        let soft = |x: usize| (220.0 + 14.0 * ((x as f32 - 60.0) / 30.0).sin()).round() as isize;
        for x in 60..340 {
            for dy in -6isize..=6 {
                let y = (soft(x) + dy) as usize;
                lum[y * w + x] -= 0.07 * (1.0 - (dy.abs() as f32 / 6.0));
            }
        }
        let m = detect_picture(&lum, w, h, 50.0, 12.0);
        let along = |line: &dyn Fn(usize) -> usize| {
            (70..330).filter(|&x| (0..4).any(|d| m[(line(x) + d).saturating_sub(1) * w + x] > 0)).count()
        };
        assert!(along(&hair) > 200, "hair followed along {} of 260", along(&hair));
        let soft_found = along(&|x| soft(x) as usize);
        assert!(soft_found < 20, "soft line taken along {soft_found}");
    }

    #[test]
    fn light_defects_only_leave_dark_ones_alone() {
        let (w, h) = (200, 150);
        let mut lum = grainy(w, h);
        for (cx, cy, v) in [(40.0f32, 40.0f32, 0.2f32), (120.0, 50.0, 0.85)] {
            for y in 0..h {
                for x in 0..w {
                    if (x as f32 - cx).hypot(y as f32 - cy) < 3.0 {
                        lum[y * w + x] = v;
                    }
                }
            }
        }
        let light = detect_picture_as(&lum, w, h, 50.0, 10.0, DustPolarity::Light);
        assert_eq!(light[50 * w + 120], 255, "light speck");
        assert_eq!(light[40 * w + 40], 0, "dark speck left");
        let dark = detect_picture_as(&lum, w, h, 50.0, 10.0, DustPolarity::Dark);
        assert_eq!((dark[40 * w + 40], dark[50 * w + 120]), (255, 0));
        assert!(
            detect_picture_as(&grainy(w, h), w, h, 50.0, 10.0, DustPolarity::Light).iter().all(|m| *m == 0)
        );
    }

    #[test]
    fn the_ai_map_is_thresholded_by_sensitivity_and_polarity() {
        let (w, h) = (100, 80);
        let mut lum = vec![0.5f32; w * h];
        let mut map = vec![0u8; w * h];
        // A light speck the model is fairly sure of, and a dark one it is sure of.
        for (cx, cy, v, p) in [(30usize, 40usize, 0.8f32, 120u8), (70, 40, 0.2, 230)] {
            for y in cy - 2..=cy + 2 {
                for x in cx - 2..=cx + 2 {
                    lum[y * w + x] = v;
                    map[y * w + x] = p;
                }
            }
        }
        let layer = |sensitivity: f32, polarity| DustLayer { sensitivity, polarity, ..Default::default() };
        let m = dust_from_map(&map, &lum, w, h, &layer(50.0, DustPolarity::Light));
        assert_eq!((m[40 * w + 30], m[40 * w + 70]), (255, 0), "light only");
        let m = dust_from_map(&map, &lum, w, h, &layer(10.0, DustPolarity::Both));
        assert_eq!((m[40 * w + 30], m[40 * w + 70]), (0, 255), "only the surest at low sensitivity");
    }

    #[test]
    fn subtracted_strokes_clear_the_mask_under_them_only() {
        let (w, h) = (100, 50);
        let mut mask = vec![255u8; w * h];
        // A horizontal stroke along y = 0.5 from x = 0.2 to 0.6, radius 0.04 half long sides
        // (2 px), and a dab at (0.9, 0.2).
        let strokes = [
            super::super::SpotStroke { points: vec![[0.2, 0.5], [0.6, 0.5]], size: 0.04, erase: true },
            super::super::SpotStroke { points: vec![[0.9, 0.2]], size: 0.04, erase: true },
        ];
        erase_strokes(&mut mask, w, h, &strokes);
        assert_eq!(mask[25 * w + 40], 0, "under the stroke");
        assert_eq!(mask[10 * w + 90], 0, "under the dab");
        assert_eq!(mask[25 * w + 70], 255, "past the stroke's end");
        assert_eq!(mask[20 * w + 40], 255, "beside the stroke");
        // Not part of what finding depends on: no detection again.
        let layer = DustLayer { erase: strokes.to_vec(), ..Default::default() };
        assert_eq!(layer.key(), DustLayer::default().key());
    }

    #[test]
    fn the_layer_key_follows_what_the_mask_depends_on() {
        let base = DustLayer::default();
        let key = base.key();
        assert_ne!(DustLayer { polarity: DustPolarity::Dark, ..base.clone() }.key(), key);
        assert_ne!(DustLayer { ai: true, ..base.clone() }.key(), key);
        // The AI map is of the photo, not of the settings.
        assert_eq!(DustLayer { ai_map: Some("x".into()), ..base.clone() }.key(), key);
        // Settings saved before polarity and AI existed still load (as Light, no AI).
        let old: DustLayer =
            serde_json::from_str(r#"{"enabled":true,"source":"Picture","sensitivity":40.0,"max_size":12.0}"#)
                .expect("old layer");
        assert_eq!((old.polarity, old.ai, old.sensitivity), (DustPolarity::Light, false, 40.0));
    }

    #[test]
    fn ir_detection_finds_dips_and_respects_the_valid_mask() {
        let (w, h) = (160, 120);
        let mut ir: Vec<f32> = (0..w * h).map(|i| 0.84 + 0.01 * ((i % 7) as f32 / 7.0 - 0.5)).collect();
        for y in 58..63 {
            for x in 78..83 {
                ir[y * w + x] = 0.4;
            }
        }
        let lum = vec![0.6; w * h];
        let m = detect_ir(&ir, &lum, None, w, h, 50.0, 12.0);
        assert_eq!(m[60 * w + 80], 255);
        assert_eq!(m[20 * w + 20], 0);
        // Not trusted there: nothing found.
        let valid: Vec<u8> = (0..w * h).map(|i| if i % w < 100 { 0 } else { 255 }).collect();
        assert_eq!(detect_ir(&ir, &lum, Some(&valid), w, h, 50.0, 12.0)[60 * w + 80], 0);
    }

    #[test]
    fn ir_detection_ignores_the_picture_showing_in_the_infrared() {
        // Dense picture detail (many small dark marks, about 90% darker) shows in the
        // infrared at a tenth of that; a speck of dust shows strongly in both.
        let (w, h) = (300, 200);
        let mut lum = vec![0.8f32; w * h];
        let mut ir = vec![0.84f32; w * h];
        for k in 0..12 {
            let (cx, cy) = (20 + (k % 6) * 45, 40 + (k / 6) * 100);
            for y in cy..cy + 8 {
                for x in cx..cx + 8 {
                    lum[y * w + x] = 0.8 * 0.1f32.sqrt();
                    ir[y * w + x] = 0.84 * (1.0 - 0.1 * 0.9);
                }
            }
        }
        for y in 98..102 {
            for x in 148..152 {
                lum[y * w + x] = 0.5;
                ir[y * w + x] = 0.5;
            }
        }
        let m = detect_ir(&ir, &lum, None, w, h, 60.0, 12.0);
        assert_eq!(m[100 * w + 150], 255, "dust");
        assert_eq!(m[44 * w + 24], 0, "the picture");
    }

    #[test]
    fn sensor_dust_finds_soft_blobs_on_smooth_areas_only() {
        let (w, h) = (600, 400);
        // A smooth sky gradient with two soft dust blobs; a textured lower half.
        let mut lum: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                if y > 250.0 {
                    0.3 + 0.15 * ((x * 0.7).sin() * (y * 0.9).cos())
                } else {
                    0.6 + 0.1 * x / w as f32
                }
            })
            .collect();
        for (cx, cy, r) in [(150.0f32, 100.0f32, 12.0f32), (420.0, 150.0, 20.0)] {
            for y in 0..h {
                for x in 0..w {
                    let d = (x as f32 - cx).hypot(y as f32 - cy) / r;
                    lum[y * w + x] *= 1.0 - 0.08 * (-d * d).exp();
                }
            }
        }
        let found = find_sensor_dust(&lum, w, h, 50.0);
        let near = |cx: f32, cy: f32| {
            found.iter().any(|(c, _)| (c[0] * w as f32 - cx).hypot(c[1] * h as f32 - cy) < 10.0)
        };
        assert!(near(150.0, 100.0) && near(420.0, 150.0), "{found:?}");
        assert!(found.iter().all(|(c, _)| c[1] * (h as f32) < 245.0), "texture is not dust: {found:?}");
    }

    #[test]
    fn fill_replaces_holes_with_their_surroundings() {
        let (w, h) = (64, 48);
        // A left-to-right ramp with a dark hole.
        let mut px: Vec<f32> = (0..w * h).flat_map(|i| [(i % w) as f32 / w as f32; 3]).collect();
        let mut mask = vec![0u8; w * h];
        for y in 20..28 {
            for x in 28..36 {
                px[3 * (y * w + x)..3 * (y * w + x) + 3].copy_from_slice(&[0.0; 3]);
                mask[y * w + x] = 255;
            }
        }
        let before = px.clone();
        fill_masked(&mut px, w, h, &mask);
        let v = px[3 * (24 * w + 32)];
        assert!((v - 0.5).abs() < 0.08, "{v}");
        // Outside the hole: untouched.
        assert_eq!(px[3 * (5 * w + 5)], before[3 * (5 * w + 5)]);
    }

    #[test]
    fn masks_shrink_by_the_most_under_each_pixel() {
        let mut pic = image::GrayImage::new(100, 100);
        pic.put_pixel(57, 33, image::Luma([255]));
        let m = mask_at(&pic, 10, 10);
        assert_eq!(m[3 * 10 + 5], 255);
        assert_eq!(m.iter().filter(|v| **v > 0).count(), 1);
    }
}
