//! Rendering a [`WorkingImage`] with [`DevelopSettings`].
//!
//! Per pixel: white balance (and the camera matrix) → Dehaze (see [`super::dehaze`]) →
//! exposure, Highlights / Shadows, Clarity and Texture (gains from local brightness maps,
//! see [`super::local`]) → tone curve (whites, blacks, contrast, highlight shoulder; plus
//! the base rendering for RAW files) → vibrance / saturation → B&W → output. Values are
//! linear until the output encoding.

use image::{ImageBuffer, Rgb, RgbImage};
use rayon::prelude::*;

use super::curve::{CurveFns, ToneCurve};
use super::dehaze::{HazeMap, add_haze};
use super::hsl::HslAdjust;
use super::local::{BaseMap, CLARITY_MAP, TONE_MAP, box_blur_par, fast_exp2, fast_log2, tone_gain};
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
const MIDDLE_GREY: f32 = 0.18;
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

/// The tone table with the Tone Curve applied, per channel. Curves work on perceptual
/// values; the parametric part is kept from ever going down (its regions overlap).
fn channel_luts(tone: Vec<f32>, curve: &ToneCurve) -> [Vec<f32>; 3] {
    if curve.is_default() {
        return [tone.clone(), tone.clone(), tone];
    }
    let f = CurveFns::new(curve);
    let mut floor = 0.0f32;
    let master: Vec<f32> = tone
        .iter()
        .map(|d| {
            let p = curve.parametric(d.max(0.0).powf(1.0 / GAMMA)).max(floor);
            floor = p;
            f.master_after_parametric(p)
        })
        .collect();
    std::array::from_fn(|c| master.iter().map(|m| f.channel(c, *m).clamp(0.0, 1.0).powf(GAMMA)).collect())
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

/// A rectangle of an image, in pixels (file orientation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Region {
    pub fn whole(img: &WorkingImage) -> Region {
        Region { x: 0, y: 0, w: img.width(), h: img.height() }
    }

    /// This region clipped to `img`, at least one pixel.
    pub fn clipped(self, img: &WorkingImage) -> Region {
        let x = self.x.min(img.width() - 1);
        let y = self.y.min(img.height() - 1);
        Region { x, y, w: self.w.clamp(1, img.width() - x), h: self.h.clamp(1, img.height() - y) }
    }
}

/// Everything per-pixel rendering needs, prepared once per render.
struct Prepared {
    input: Input,
    gain: f32,
    /// Tone, then Tone Curve, per channel (R, G, B): scene-linear → display-linear.
    luts: [Vec<f32>; 3],
    vibrance: f32,
    saturation: f32,
    bw: bool,
    /// HSL (colour) or B&W Mix (black & white), when either changes something.
    hsl: Option<HslAdjust>,
    luma: [f32; 3],
    /// Highlights / Shadows, Clarity, Texture: −1..+1.
    highlights: f32,
    shadows: f32,
    clarity: f32,
    texture: f32,
    /// With any of them on: the exposure gain of every pixel of `region` (`gain` with
    /// the local adjustments applied), computed in one pass before rendering.
    gains: Option<Vec<f32>>,
    /// The part of the image being rendered.
    region: Region,
    /// Dehaze, −1..+1, and the haze map (also used for its colour when adding haze).
    dehaze: f32,
    haze: Option<HazeMap>,
}

impl Prepared {
    /// Prepare to render `region` of `img`. Local adjustments look at the whole image,
    /// so a region renders exactly like that part of a full render.
    fn new(img: &WorkingImage, s: &DevelopSettings, region: Region) -> Prepared {
        let wb = img.resolve_wb(s);
        let input = match &img.color {
            SourceColor::Camera(c) => Input::Camera { m: wb.multipliers, mat: c.cam_to_work },
            _ => Input::Gains(wb.multipliers),
        };
        let scene = img.scene_referred();
        let ev = s.exposure + if scene { RAW_BASELINE_EV } else { 0.0 };
        let (whites, blacks) = (s.whites / 100.0, s.blacks / 100.0);
        let shoulder = if scene {
            RAW_SHOULDER
        } else {
            // Rendered images keep their tones unless pushed past white.
            1.0 - 0.25
                * (s.exposure.max(0.0) + 0.5 * whites.max(0.0) - 0.5 * s.highlights.min(0.0) / 100.0).min(1.0)
        };
        let base = if scene { RAW_BASE_CONTRAST } else { 0.0 };
        let tp =
            ToneParams { contrast: (base + s.contrast / 100.0).clamp(-1.0, 1.0), whites, blacks, shoulder };
        let mut prep = Prepared {
            input,
            gain: 2f32.powf(ev),
            luts: channel_luts(tone_lut(&tp), &s.curve),
            vibrance: s.vibrance / 100.0,
            saturation: s.saturation / 100.0,
            bw: s.treatment == Treatment::BlackAndWhite,
            hsl: if s.treatment == Treatment::BlackAndWhite {
                HslAdjust::bw_mix(&s.bw_mix)
            } else {
                HslAdjust::color(&s.hsl)
            },
            luma: matrix::work_luma(),
            highlights: s.highlights / 100.0,
            shadows: s.shadows / 100.0,
            clarity: s.clarity / 100.0,
            gains: None,
            texture: s.texture / 100.0,
            region,
            dehaze: s.dehaze / 100.0,
            haze: None,
        };
        if prep.dehaze != 0.0 {
            let (w, h) = (img.width() as usize, img.height() as usize);
            let map = HazeMap::build(w, h, prep.dehaze.max(0.0), |x, y| prep.linear(input_at(img, x, y)));
            prep.haze = Some(map);
        }
        let tone = prep.highlights != 0.0 || prep.shadows != 0.0;
        let detail = prep.clarity != 0.0 || prep.texture != 0.0;
        if tone || detail {
            prep.gains = Some(prep.local_gains(img));
        }
        prep
    }

    /// The exposure gain of every pixel of the region with Highlights / Shadows, Clarity
    /// and Texture applied: maps of local brightness first (from the whole image), then
    /// one simple pass over the region's pixels.
    fn local_gains(&self, img: &WorkingImage) -> Vec<f32> {
        let (w, h) = (img.width() as usize, img.height() as usize);
        // Rendering everything (the proxy, exports): every pixel's luminance once, shared by
        // the maps and the detail pass. A region of a full-size image instead computes it
        // where needed: the maps only sample a fraction of the pixels.
        let whole = self.region == Region::whole(img);
        let lum: Option<Vec<f32>> = whole.then(|| {
            img.pixels
                .as_raw()
                .par_chunks(3)
                .map(|p| self.luminance(self.linear([p[0], p[1], p[2]])))
                .collect()
        });
        let lum_at = |x: usize, y: usize| match &lum {
            Some(l) => l[y * w + x],
            None => self.luminance(self.linear(input_at(img, x, y))),
        };
        let tone = (self.highlights != 0.0 || self.shadows != 0.0).then(|| {
            let mut map = BaseMap::build(w, h, TONE_MAP, lum_at);
            // Brightness (stops from middle grey after exposure) → gain, per map cell.
            let offset = self.gain.log2() - MIDDLE_GREY.log2();
            let (hl, sh) = (self.highlights, self.shadows);
            map.transform(|v| tone_gain(v + offset, hl, sh).exp2());
            map
        });
        let clarity = (self.clarity != 0.0).then(|| BaseMap::build(w, h, CLARITY_MAP, lum_at));

        // log2 luminance over the region, plus the margin Texture's blur reaches into.
        let r = (w.max(h) as f32 / 1000.0).round().max(1.0) as usize;
        let margin = if self.texture != 0.0 { 2 * r + 1 } else { 0 };
        let reg = self.region;
        let (ex0, ey0) = ((reg.x as usize).saturating_sub(margin), (reg.y as usize).saturating_sub(margin));
        let ex1 = (reg.x as usize + reg.w as usize + margin).min(w);
        let ey1 = (reg.y as usize + reg.h as usize + margin).min(h);
        let (ew, eh) = (ex1 - ex0, ey1 - ey0);
        let ylog: Option<Vec<f32>> = (self.clarity != 0.0 || self.texture != 0.0).then(|| {
            (ey0..ey1)
                .into_par_iter()
                .flat_map_iter(|y| (ex0..ex1).map(move |x| fast_log2(lum_at(x, y).max(1e-6))))
                .collect()
        });
        // Texture: log luminance blurred twice (close to a Gaussian) over a few pixels; the
        // radius follows the image size so a preview and a full-size export agree.
        let texture = (self.texture != 0.0).then(|| {
            let ylog = ylog.as_deref().unwrap_or_default();
            box_blur_par(&box_blur_par(ylog, ew, eh, r), ew, eh, r)
        });
        let grey = MIDDLE_GREY.log2();
        let (rx, ry, rw) = (reg.x as usize, reg.y as usize, reg.w as usize);
        let mut gains = vec![self.gain; rw * reg.h as usize];
        gains.par_chunks_mut(rw).enumerate().for_each(|(j, row)| {
            let y = ry + j;
            for (i, g) in row.iter_mut().enumerate() {
                let x = rx + i;
                if let Some(map) = &tone {
                    *g *= map.sample(x, y);
                }
                let Some(ylog) = &ylog else { continue };
                let e = (y - ey0) * ew + (x - ex0);
                let l = ylog[e];
                let mut stops = 0.0;
                if let Some(map) = &clarity {
                    let d = (l - map.sample(x, y)).clamp(-1.5, 1.5);
                    // Mostly the midtones: deep shadows and highlights are left alone.
                    let rel = l + fast_log2(*g) - grey;
                    stops += self.clarity * 0.9 * d / (1.0 + rel * rel / 6.0);
                }
                if let Some(base) = &texture {
                    let d = (l - base[e]).clamp(-1.0, 1.0);
                    // Soft threshold: the smallest differences are mostly noise.
                    stops += self.texture * 1.2 * d * (d.abs() / (d.abs() + 0.04));
                }
                *g *= fast_exp2(stops);
            }
        });
        gains
    }

    #[inline]
    fn luminance(&self, lin: [f32; 3]) -> f32 {
        (self.luma[0] * lin[0] + self.luma[1] * lin[1] + self.luma[2] * lin[2]).max(0.0)
    }

    /// An input pixel → scene-linear working-space RGB (white balanced, before exposure).
    #[inline]
    fn linear(&self, p: [f32; 3]) -> [f32; 3] {
        match &self.input {
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
        }
    }

    /// Input pixel `p` at (x, y) → display-referred linear working-space RGB, 0..1.
    #[inline]
    fn pixel(&self, p: [f32; 3], x: usize, y: usize) -> [f32; 3] {
        let mut lin = self.linear(p);
        if let Some(haze) = &self.haze {
            lin = if self.dehaze > 0.0 {
                haze.clear(lin, x, y)
            } else {
                add_haze(lin, -self.dehaze, haze.airlight)
            };
        }
        let g = match &self.gains {
            Some(gains) => {
                let r = &self.region;
                gains[(y - r.y as usize) * r.w as usize + (x - r.x as usize)]
            }
            None => self.gain,
        };
        let mut d = [
            lookup(&self.luts[0], lin[0] * g),
            lookup(&self.luts[1], lin[1] * g),
            lookup(&self.luts[2], lin[2] * g),
        ];
        // HSL in colour; in Black & White the B&W Mix decides how light each colour turns.
        if let Some(hsl) = &self.hsl
            && !self.bw
        {
            d = hsl.apply(d);
        }
        let y = self.luminance(d);
        if self.bw {
            let grey = self.hsl.as_ref().map_or(y, |m| m.grey(d, y)).min(1.0);
            return [grey; 3];
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

/// The input pixel at (x, y).
#[inline]
fn input_at(img: &WorkingImage, x: usize, y: usize) -> [f32; 3] {
    let i = 3 * (y * img.width() as usize + x);
    let px = img.pixels.as_raw();
    [px[i], px[i + 1], px[i + 2]]
}

/// Run `pixel` over every pixel of `region`, a row per task: `out` has `channels` values
/// per pixel; `pixel` gets image coordinates.
fn render_rows<T: Send>(
    img: &WorkingImage,
    region: Region,
    out: &mut [T],
    channels: usize,
    pixel: impl Fn(&mut [T], [f32; 3], usize, usize) + Sync,
) {
    let (w, rx, rw) = (img.width() as usize, region.x as usize, region.w as usize);
    let src = img.pixels.as_raw();
    out.par_chunks_mut(channels * rw).enumerate().for_each(|(j, dst)| {
        let y = region.y as usize + j;
        let row = &src[3 * (y * w + rx)..3 * (y * w + rx + rw)];
        for (i, (o, p)) in dst.chunks_mut(channels).zip(row.as_chunks::<3>().0).enumerate() {
            pixel(o, *p, rx + i, y);
        }
    });
}

/// Render for display: 8-bit sRGB.
pub fn render_srgb8(img: &WorkingImage, s: &DevelopSettings) -> RgbImage {
    render_region_srgb8(img, s, Region::whole(img))
}

/// Render part of the image for display (1:1 views of a full-size image): 8-bit sRGB,
/// the region's size.
pub fn render_region_srgb8(img: &WorkingImage, s: &DevelopSettings, region: Region) -> RgbImage {
    let region = region.clipped(img);
    let prep = Prepared::new(img, s, region);
    let enc = SrgbEncoder::new();
    let to_srgb = matrix::to_f32(&matrix::work_to_srgb());
    let mut out = RgbImage::new(region.w, region.h);
    render_rows(img, region, &mut out, 3, |o, p, x, y| {
        let l = matrix::apply(&to_srgb, prep.pixel(p, x, y));
        o.copy_from_slice(&l.map(|v| enc.encode(v)));
    });
    out
}

/// Render for export: 16-bit linear working space (linear ProPhoto RGB).
pub fn render_linear16(img: &WorkingImage, s: &DevelopSettings) -> ImageBuffer<Rgb<u16>, Vec<u16>> {
    let region = Region::whole(img);
    let prep = Prepared::new(img, s, region);
    let mut out = ImageBuffer::<Rgb<u16>, Vec<u16>>::new(img.width(), img.height());
    render_rows(img, region, &mut out, 3, |o, p, x, y| {
        o.copy_from_slice(&prep.pixel(p, x, y).map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16));
    });
    out
}

/// Percentile (0..1) of sorted values.
fn percentile(sorted: &[f32], q: f32) -> f32 {
    sorted[((sorted.len() - 1) as f32 * q).round() as usize]
}

/// LrC's Auto (Tone): Exposure, Contrast, Highlights, Shadows, Whites and Blacks chosen
/// from the photo's brightness, keeping everything else.
pub fn auto_tone(img: &WorkingImage, s: &DevelopSettings) -> DevelopSettings {
    let neutral = DevelopSettings {
        exposure: 0.0,
        contrast: 0.0,
        highlights: 0.0,
        shadows: 0.0,
        whites: 0.0,
        blacks: 0.0,
        ..s.clone()
    };
    let prep = Prepared::new(img, &neutral, Region::whole(img));
    // Luminance in stops from middle grey, on a sample of pixels.
    let px = img.pixels.as_raw().as_chunks::<3>().0;
    let step = (px.len() / 200_000).max(1);
    let mut stops: Vec<f32> = px
        .par_iter()
        .step_by(step)
        .map(|p| prep.luminance(prep.linear(*p)) * prep.gain)
        .filter(|y| *y > 1e-6)
        .map(|y| (y / MIDDLE_GREY).log2())
        .collect();
    if stops.len() < 100 {
        return s.clone();
    }
    stops.par_sort_unstable_by(f32::total_cmp);
    let (lo, p10, median, p90, hi) = (
        percentile(&stops, 0.005),
        percentile(&stops, 0.1),
        percentile(&stops, 0.5),
        percentile(&stops, 0.9),
        percentile(&stops, 0.995),
    );
    // Bring the middle of the photo near middle grey (a little under: most scenes have
    // more shadow than highlight).
    let exposure = ((-0.3 - median) * 0.85).clamp(-4.0, 4.0);
    let (lo, hi) = (lo + exposure, hi + exposure);
    // White is about 2.5 stops above middle grey.
    let highlights = if hi > 2.0 { -((hi - 2.0) * 40.0).min(80.0) } else { 0.0 };
    let whites = if hi < 1.5 { ((1.5 - hi) * 25.0).min(40.0) } else { 0.0 };
    let shadows = if lo < -6.0 { ((-6.0 - lo) * 15.0).min(60.0) } else { 0.0 };
    let blacks = if lo > -5.0 { -((lo + 5.0) * 12.0).min(30.0) } else { 0.0 };
    let contrast = ((5.0 - (p90 - p10)) * 8.0).clamp(-20.0, 30.0);
    DevelopSettings {
        exposure: (exposure * 20.0).round() / 20.0,
        contrast: contrast.round(),
        highlights: highlights.round(),
        shadows: shadows.round(),
        whites: whites.round(),
        blacks: blacks.round(),
        ..s.clone()
    }
}
