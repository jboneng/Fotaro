//! Rendering a [`WorkingImage`] with [`DevelopSettings`].
//!
//! Per pixel: white balance (and the camera matrix) → exposure → tone curve (whites,
//! blacks, contrast, highlight shoulder; plus the base rendering for RAW files) →
//! vibrance / saturation → B&W → output. Values are linear until the output encoding.

use image::{ImageBuffer, Rgb, RgbImage};
use rayon::prelude::*;

use super::matrix::{self, Mat3f};
use super::settings::{DevelopSettings, Treatment};
use super::source::{SourceColor, WorkingImage};

/// Extra exposure for scene-referred (RAW) data: cameras keep highlight headroom, so a
/// straight linear render looks dark next to their JPEGs.
const RAW_BASELINE_EV: f32 = 1.0;
/// Base contrast of the RAW rendering (on the same scale as Contrast / 100).
const RAW_BASE_CONTRAST: f32 = 0.7;
/// Where the RAW rendering starts rolling highlights off (perceptual 0..1).
const RAW_SHOULDER: f32 = 0.9;
/// Perceptual value of 18% grey, the contrast pivot.
const PIVOT: f32 = 0.459;
const GAMMA: f32 = 2.2;
/// Raw values (0..1 of the sensor range) above which a channel counts as clipping.
const CLIP_START: f32 = 0.92;

/// The tone curve, sampled on √(linear value) so shadows get fine steps.
const LUT_SIZE: usize = 8192;
const LUT_MAX_SQRT: f32 = 8.0;

struct ToneParams {
    contrast: f32,
    whites: f32,
    blacks: f32,
    shoulder: f32,
}

fn shoulder(x: f32, k: f32) -> f32 {
    if k >= 1.0 || x <= k { x } else { k + (1.0 - k) * (1.0 - (-(x - k) / (1.0 - k)).exp()) }
}

/// Scene-linear (after exposure) → display-linear 0..1.
fn tone(e: f32, p: &ToneParams) -> f32 {
    let mut x = e.max(0.0).powf(1.0 / GAMMA);
    // Whites: scale the top end.
    x *= 1.0 + 0.25 * p.whites * x.min(1.5).powi(2);
    // Blacks: move the black point and reshape the deep shadows.
    if p.blacks != 0.0 {
        let bp = -0.04 * p.blacks;
        x = ((x - bp) / (1.0 - bp)).max(0.0);
        let w = (1.0 - x / 0.6).max(0.0).powi(2);
        x = x * (1.0 - w) + w * x.powf(2f32.powf(-0.6 * p.blacks));
    }
    // Contrast: a monotonic cubic S around the pivot, continued linearly above 1.
    let a = p.contrast * 1.6;
    x = if x <= 1.0 {
        x + a * x * (1.0 - x) * (x - PIVOT)
    } else {
        1.0 + (1.0 - a * (1.0 - PIVOT)) * (x - 1.0)
    };
    shoulder(x, p.shoulder).clamp(0.0, 1.0).powf(GAMMA)
}

fn tone_lut(p: &ToneParams) -> Vec<f32> {
    (0..LUT_SIZE)
        .map(|i| {
            let s = i as f32 / (LUT_SIZE - 1) as f32 * LUT_MAX_SQRT;
            tone(s * s, p)
        })
        .collect()
}

#[inline]
fn lookup(lut: &[f32], e: f32) -> f32 {
    let f = e.max(0.0).sqrt() * ((LUT_SIZE - 1) as f32 / LUT_MAX_SQRT);
    let i = f as usize;
    if i >= LUT_SIZE - 1 {
        return lut[LUT_SIZE - 1];
    }
    let t = f - i as f32;
    lut[i] + t * (lut[i + 1] - lut[i])
}

/// How input pixels become working-space linear values.
enum Input {
    /// Camera RGB: multipliers, neutral where the sensor clipped, matrix.
    Camera { m: [f32; 3], mat: Mat3f },
    /// Working-space values: channel gains.
    Gains([f32; 3]),
}

/// Everything per-pixel rendering needs, prepared once per render.
struct Prepared {
    input: Input,
    gain: f32,
    lut: Vec<f32>,
    vibrance: f32,
    saturation: f32,
    bw: bool,
    luma: [f32; 3],
}

impl Prepared {
    fn new(img: &WorkingImage, s: &DevelopSettings) -> Prepared {
        let wb = img.resolve_wb(s);
        let input = match &img.color {
            SourceColor::Camera(c) => {
                let m = wb.multipliers;
                Input::Camera { m, mat: c.cam_to_work }
            }
            _ => Input::Gains(wb.multipliers),
        };
        let scene = img.scene_referred();
        let ev = s.exposure + if scene { RAW_BASELINE_EV } else { 0.0 };
        let (whites, blacks) = (s.whites / 100.0, s.blacks / 100.0);
        let shoulder = if scene {
            RAW_SHOULDER
        } else {
            // Rendered images keep their tones unless pushed past white.
            1.0 - 0.25 * (s.exposure.max(0.0) + 0.5 * whites.max(0.0)).min(1.0)
        };
        let base = if scene { RAW_BASE_CONTRAST } else { 0.0 };
        let tp =
            ToneParams { contrast: (base + s.contrast / 100.0).clamp(-1.0, 1.0), whites, blacks, shoulder };
        Prepared {
            input,
            gain: 2f32.powf(ev),
            lut: tone_lut(&tp),
            vibrance: s.vibrance / 100.0,
            saturation: s.saturation / 100.0,
            bw: s.treatment == Treatment::BlackAndWhite,
            luma: matrix::work_luma(),
        }
    }

    /// One input pixel → display-referred linear working-space RGB, 0..1.
    #[inline]
    fn pixel(&self, p: [f32; 3]) -> [f32; 3] {
        let lin = match &self.input {
            Input::Camera { m, mat } => {
                let mut q = [p[0] * m[0], p[1] * m[1], p[2] * m[2]];
                // Where the sensor clipped, the colour is unknown: fade to neutral at the
                // brightest channel so blown highlights don't turn magenta or green.
                let hi = p[0].max(p[1]).max(p[2]);
                if hi > CLIP_START {
                    let w = ((hi - CLIP_START) / (1.0 - CLIP_START)).min(1.0);
                    let w = w * w * (3.0 - 2.0 * w);
                    let top = q[0].max(q[1]).max(q[2]);
                    q = q.map(|v| v + w * (top - v));
                }
                matrix::apply(mat, q)
            }
            Input::Gains(m) => [p[0] * m[0], p[1] * m[1], p[2] * m[2]],
        };
        let g = self.gain;
        let mut d =
            [lookup(&self.lut, lin[0] * g), lookup(&self.lut, lin[1] * g), lookup(&self.lut, lin[2] * g)];
        let y = self.luma[0] * d[0] + self.luma[1] * d[1] + self.luma[2] * d[2];
        if self.bw {
            return [y; 3];
        }
        if self.vibrance != 0.0 || self.saturation != 0.0 {
            let (hi, lo) = (d[0].max(d[1]).max(d[2]), d[0].min(d[1]).min(d[2]));
            let sat = if hi > 1e-6 { (hi - lo) / hi } else { 0.0 };
            let mut v = self.vibrance * (1.0 - sat).powi(2);
            if self.vibrance > 0.0 && d[0] > d[1] && d[1] > d[2] {
                // Protect skin tones (orange hues) from vibrance boosts.
                let h = (d[1] - d[2]) / (d[0] - d[2]);
                v *= 1.0 - 0.6 * (-((h - 0.4) / 0.2).powi(2)).exp();
            }
            let f = (1.0 + v) * (1.0 + self.saturation);
            d = d.map(|c| (y + (c - y) * f).max(0.0));
        }
        d
    }
}

/// Linear 0..1 → sRGB-encoded 8-bit, by table.
struct SrgbEncoder(Vec<u8>);

impl SrgbEncoder {
    const N: usize = 16384;

    fn new() -> Self {
        SrgbEncoder(
            (0..Self::N)
                .map(|i| {
                    let v = i as f32 / (Self::N - 1) as f32;
                    let e = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
                    (e * 255.0).round().clamp(0.0, 255.0) as u8
                })
                .collect(),
        )
    }

    #[inline]
    fn encode(&self, v: f32) -> u8 {
        self.0[(v.clamp(0.0, 1.0) * (Self::N - 1) as f32 + 0.5) as usize]
    }
}

/// Render for display: 8-bit sRGB.
pub fn render_srgb8(img: &WorkingImage, s: &DevelopSettings) -> RgbImage {
    let prep = Prepared::new(img, s);
    let enc = SrgbEncoder::new();
    let to_srgb = matrix::to_f32(&matrix::work_to_srgb());
    let (w, h) = (img.width(), img.height());
    let mut out = RgbImage::new(w, h);
    out.as_mut().par_chunks_mut(3 * w as usize).zip(img.pixels.as_raw().par_chunks(3 * w as usize)).for_each(
        |(dst, src)| {
            for (o, p) in dst.as_chunks_mut::<3>().0.iter_mut().zip(src.as_chunks::<3>().0) {
                let d = prep.pixel(*p);
                let l = matrix::apply(&to_srgb, d);
                *o = l.map(|v| enc.encode(v));
            }
        },
    );
    out
}

/// Render for export: 16-bit linear working space (linear ProPhoto RGB).
pub fn render_linear16(img: &WorkingImage, s: &DevelopSettings) -> ImageBuffer<Rgb<u16>, Vec<u16>> {
    let prep = Prepared::new(img, s);
    let (w, h) = (img.width(), img.height());
    let mut out = ImageBuffer::<Rgb<u16>, Vec<u16>>::new(w, h);
    out.as_mut().par_chunks_mut(3 * w as usize).zip(img.pixels.as_raw().par_chunks(3 * w as usize)).for_each(
        |(dst, src)| {
            for (o, p) in dst.as_chunks_mut::<3>().0.iter_mut().zip(src.as_chunks::<3>().0) {
                let d = prep.pixel(*p);
                *o = d.map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16);
            }
        },
    );
    out
}
