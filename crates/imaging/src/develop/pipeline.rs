//! Rendering a [`WorkingImage`] with [`DevelopSettings`].
//!
//! Per pixel: white balance (and the camera matrix) → Dehaze (see [`super::dehaze`]) →
//! exposure, Highlights / Shadows, Clarity and Texture (gains from local brightness maps,
//! see [`super::local`]) → tone curve (whites, blacks, contrast, highlight shoulder; plus
//! the base rendering for RAW files) → HSL → vibrance / saturation or B&W → Color Grading
//! → vignette (Highlight Priority acts as an exposure change before the tone curve) →
//! Detail (sharpening, noise reduction) → grain → output. Values are linear until the
//! output encoding.

use image::{ImageBuffer, Rgb, RgbImage};
use rayon::prelude::*;

use std::sync::Arc;

use super::cache::{GainsKey, WbKey};
use super::curve::{CurveFns, ToneCurve};
use super::dehaze::{HazeMap, add_haze};
use super::detail::{SharpenView, apply as apply_detail, apply_local};
use super::effects::{Effects, Fog, Glow, Grain, Relight, Vignette, VignetteStyle};
use super::grading::Grading;
use super::heal::healed;
use super::hsl::{HslAdjust, oklab_to_srgb, srgb_to_oklab};
use super::local::{BaseMap, CLARITY_MAP, TONE_MAP, box_blur_par, fast_exp2, fast_log2, tone_gain};
use super::masks::{LocalAdjust, Mask, MaskField};
use super::matrix::{self, Mat3f};
use super::settings::{DevelopSettings, Treatment};
use super::source::{SourceColor, WorkingImage};
use super::transform::transformed;
use super::wb::relative_multipliers;

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

/// How near everything in `region` of `shown` is (1: the nearest), for Fog, Lens Blur and
/// Relight: the photo's depth map read as a mask is, so it follows the crop and the
/// transform and has its edges fitted to the photo's. `None` when none of them is on, or
/// without a depth map.
fn nearness(
    source: &WorkingImage,
    shown: &WorkingImage,
    s: &DevelopSettings,
    region: Region,
) -> Option<Arc<MaskField>> {
    let e = &s.effects;
    let id = e.fog_depth.as_ref().filter(|_| e.uses_depth() || e.fog_show)?;
    super::masks::mask_picture(id)?;
    let shape = super::masks::Shape::Raster {
        id: id.clone(),
        selects: "Depth".into(),
        model: String::new(),
        range: None,
    };
    let map = [Mask::new("Depth", shape)];
    let field = MaskField::build_for(source, shown, &map, vec![0], region).map(Arc::new);
    super::perf::mark("depth for fog");
    field
}

/// How much of Relight's light reaches each pixel of `region` of `shown`, when it is kept
/// to a round area: that area read as a mask is, so it follows the crop and the transform.
fn relight_spot(
    source: &WorkingImage,
    shown: &WorkingImage,
    s: &DevelopSettings,
    region: Region,
) -> Option<Arc<MaskField>> {
    let e = &s.effects;
    if !e.relight_spots() {
        return None;
    }
    // (Of half the photo's longer side: small steps where the area is small.)
    let t = (e.relight_size / 100.0).clamp(0.0, 1.0);
    let radius = 0.04 + 1.6 * t * t;
    let shape = super::masks::Shape::Radial {
        center: e.relight_at,
        radii: [radius, radius],
        angle: 0.0,
        feather: 75.0,
    };
    let map = [Mask::new("Light", shape)];
    MaskField::build_for(source, shown, &map, vec![0], region).map(Arc::new)
}

/// What Relight needs of the photo around a region: how near everything is, and where its
/// light's round area is.
struct Lights<'a> {
    depth: &'a dyn Fn(Region) -> Option<Arc<MaskField>>,
    spot: &'a dyn Fn(Region) -> Option<Arc<MaskField>>,
}

/// Relight looks at what is around each pixel on a coarser grid: a cell's side in pixels,
/// and how many cells its blur reaches (about 2% of the image's longer side in all).
fn relight_grid(img: &WorkingImage) -> (usize, usize) {
    let reach = (0.02 * img.width().max(img.height()) as f32).max(2.0);
    let step = ((reach / 4.0).round() as usize).max(1);
    (step, ((reach / step as f32).round() as usize).max(1))
}

/// Cells around a region that Relight's look around reaches into (two blurs, and the
/// scaling back up).
fn relight_pad(img: &WorkingImage) -> usize {
    2 * relight_grid(img).1 + 2
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
    /// Color Grading, when it does something.
    grading: Option<Grading>,
    luma: [f32; 3],
    /// Highlights / Shadows, Clarity, Texture: −1..+1.
    highlights: f32,
    shadows: f32,
    clarity: f32,
    texture: f32,
    /// With any of them on: the exposure gain of every pixel of `region` (`gain` with
    /// the local adjustments applied), computed in one pass before rendering.
    gains: Option<Arc<Vec<f32>>>,
    /// The part of the image being rendered.
    region: Region,
    /// Dehaze, −1..+1, and the haze map (also used for its colour when adding haze).
    dehaze: f32,
    haze: Option<Arc<HazeMap>>,
    /// Post-Crop Vignetting, when on.
    vignette: Option<Vignette>,
    /// Masks' local adjustments, when any mask changes something.
    locals: Option<Locals>,
    /// Glow, when on.
    glow: Option<Glow>,
    /// Fog, when on and the photo has its depth map: how near each pixel of the region
    /// is, the fog's colour, and the image's height.
    fog: Option<(Fog, Arc<MaskField>, [f32; 3], f32)>,
    /// Relight, when on and the photo has its depth map.
    relight: Option<Relit>,
    /// Show Depth: the depth map (how near each pixel of the region is) in place of the
    /// photo.
    depth_shown: Option<Arc<MaskField>>,
    /// Profile: a 3D LUT after the base tone, when set (and its file is there).
    lut: Option<super::lut::PreparedLut>,
    /// Lens Corrections' Defringe, when on.
    fringe: Option<super::lens::Defringe>,
}

/// Relight over a region.
#[derive(Clone)]
struct Relit {
    relight: Relight,
    /// How near each pixel of the region is.
    near: Arc<MaskField>,
    /// Of the light at each pixel of the region, the share that comes off what is nearer
    /// than the slice's front, and than its back; where the two sides of one are lit
    /// alike, nothing.
    shares: [Option<Arc<Vec<f32>>>; 2],
    /// How much of the slice's light reaches each pixel, when kept to a round area.
    spot: Option<Arc<MaskField>>,
}

impl Prepared {
    /// Prepare to render `region` of `img`, with the masks' values over it (`field`), and
    /// `lights` to tell how near everything in a region is (see [`nearness`]) and where
    /// Relight's round area is. Local
    /// adjustments look at the whole image, so a region renders exactly like that part of
    /// a full render.
    fn new(
        img: &WorkingImage,
        s: &DevelopSettings,
        region: Region,
        field: Option<Arc<MaskField>>,
        lights: &Lights,
    ) -> Prepared {
        let nearness = (lights.depth)(region);
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
            grading: Grading::new(&s.grading),
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
            vignette: Vignette::new(&s.effects, img.width(), img.height()),
            locals: field.map(|field| Locals::new(field, &s.masks)),
            glow: None,
            fog: None,
            relight: None,
            depth_shown: nearness.clone().filter(|_| s.effects.fog_show),
            lut: s.profile.as_ref().and_then(super::lut::PreparedLut::new),
            fringe: None,
        };
        super::perf::mark("tone tables, setup");
        if s.transform.lens.defringes() {
            let (w, h) = (img.width() as usize, img.height() as usize);
            let scale = img.width() as f32 / img.full_size.0.max(1) as f32;
            let lum = |x: usize, y: usize| prep.luminance(prep.linear(input_at(img, x, y)));
            prep.fringe = super::lens::Defringe::new(&s.transform.lens, w, h, region, scale, lum);
            super::perf::mark("defringe map");
        }
        if let Some(relight) = Relight::new(&s.effects, prep.luma)
            && let Some(near) = &nearness
        {
            // What its look around each pixel reaches, on whole cells of its grid.
            let (step, _) = relight_grid(img);
            let (x0, y0) = (region.x as usize / step, region.y as usize / step);
            let (x1, y1) = (
                ((region.x + region.w) as usize).div_ceil(step),
                ((region.y + region.h) as usize).div_ceil(step),
            );
            let pad = relight_pad(img);
            let (x0, y0) = (x0.saturating_sub(pad) * step, y0.saturating_sub(pad) * step);
            let (x1, y1) = (
                ((x1 + pad) * step).min(img.width() as usize),
                ((y1 + pad) * step).min(img.height() as usize),
            );
            let around = Region { x: x0 as u32, y: y0 as u32, w: (x1 - x0) as u32, h: (y1 - y0) as u32 };
            let wide = if around == region { Some(near.clone()) } else { (lights.depth)(around) };
            if let Some(wide) = wide {
                let shares = [0, 1].map(|edge| {
                    relight
                        .divides(edge)
                        .then(|| Arc::new(prep.light_share(img, &relight, edge, region, &wide, around)))
                });
                let spot = (lights.spot)(region);
                prep.relight = Some(Relit { relight, near: near.clone(), shares, spot });
            }
            super::perf::mark("relight");
        }
        // Masks' Dehaze, Clarity and Texture need the same maps as the global ones.
        let local = |f: fn(&LocalParams) -> f32| {
            prep.locals.as_ref().is_some_and(|l| l.params.iter().any(|p| f(p) != 0.0))
        };
        let (local_dehaze, local_detail) =
            (local(|p| p.dehaze), local(|p| p.clarity) || local(|p| p.texture));
        let fog = Fog::new(&s.effects).zip(nearness);
        if prep.dehaze != 0.0 || local_dehaze || fog.is_some() {
            let (w, h) = (img.width() as usize, img.height() as usize);
            let map = img
                .render_cache
                .haze
                .get_or(prep.wb_key(), || HazeMap::build(w, h, |x, y| prep.linear(input_at(img, x, y))));
            if let Some((fog, near)) = fog {
                prep.fog = Some((fog, near, fog.color(map.airlight, prep.luma), img.height() as f32));
            }
            prep.haze = Some(map);
            super::perf::mark("haze map");
        }
        let tone = prep.highlights != 0.0 || prep.shadows != 0.0;
        let detail = prep.clarity != 0.0 || prep.texture != 0.0 || local_detail;
        if tone || detail {
            prep.gains = Some(prep.local_gains(img));
            super::perf::mark("gains pass");
        }
        // Glow: from the whole photo developed (without the region-only parts: local
        // gains and masks, which only cover the region), so every region agrees.
        if s.effects.glow_amount > 0.0 {
            let (gains, locals, fog) = (prep.gains.take(), prep.locals.take(), prep.fog.take());
            let (shown, relight) = (prep.depth_shown.take(), prep.relight.take());
            let (w, h) = (img.width() as usize, img.height() as usize);
            prep.glow = Glow::new(&s.effects, w, h, prep.luma, |x, y| prep.pixel(input_at(img, x, y), x, y));
            (prep.gains, prep.locals, prep.fog, prep.depth_shown) = (gains, locals, fog, shown);
            prep.relight = relight;
            super::perf::mark("glow");
        }
        prep
    }

    /// The exposure gain of every pixel of the region with Highlights / Shadows, Clarity
    /// and Texture applied: maps of local brightness first (from the whole image), then
    /// one simple pass over the region's pixels.
    fn local_gains(&self, img: &WorkingImage) -> Arc<Vec<f32>> {
        // The masks only matter here when one of them has Clarity or Texture.
        let locals =
            self.locals.as_ref().filter(|l| l.params.iter().any(|p| p.clarity != 0.0 || p.texture != 0.0));
        let key = GainsKey {
            wb: self.wb_key(),
            region: self.region,
            amounts: [self.gain, self.highlights, self.shadows, self.clarity, self.texture].map(f32::to_bits),
            field: locals.map(|l| l.field.clone()),
            local: locals.map_or_else(Vec::new, |l| {
                l.params.iter().map(|p| [p.clarity.to_bits(), p.texture.to_bits()]).collect()
            }),
        };
        img.render_cache.gains.get_or(key, || self.make_local_gains(img))
    }

    /// The white balance as a cache key: everything [`Self::linear`] depends on (the
    /// camera matrix is the image's own).
    fn wb_key(&self) -> WbKey {
        match &self.input {
            Input::Camera { m, .. } => m.map(f32::to_bits),
            Input::Gains(m) => m.map(f32::to_bits),
        }
    }

    fn make_local_gains(&self, img: &WorkingImage) -> Vec<f32> {
        let cache = &img.render_cache;
        let wb = self.wb_key();
        let (w, h) = (img.width() as usize, img.height() as usize);
        // Rendering everything (the proxy, exports): every pixel's luminance once, shared by
        // the maps and the detail pass. A region of a full-size image instead computes it
        // where needed: the maps only sample a fraction of the pixels.
        let whole = self.region == Region::whole(img);
        let lum: Option<Arc<Vec<f32>>> = whole.then(|| {
            cache.luminance.get_or(wb, || {
                img.pixels
                    .as_raw()
                    .par_chunks(3)
                    .map(|p| self.luminance(self.linear([p[0], p[1], p[2]])))
                    .collect()
            })
        });
        super::perf::mark("luminance");
        let lum_at = |x: usize, y: usize| match &lum {
            Some(l) => l[y * w + x],
            None => self.luminance(self.linear(input_at(img, x, y))),
        };
        let tone = (self.highlights != 0.0 || self.shadows != 0.0).then(|| {
            let mut map =
                BaseMap::clone(&cache.tone_map.get_or(wb, || BaseMap::build(w, h, TONE_MAP, lum_at)));
            // Brightness (stops from middle grey after exposure) → gain, per map cell.
            let offset = self.gain.log2() - MIDDLE_GREY.log2();
            let (hl, sh) = (self.highlights, self.shadows);
            map.transform(|v| tone_gain(v + offset, hl, sh).exp2());
            map
        });
        // Clarity and Texture: the global amounts, and masks' (per pixel, below).
        let locals = self.locals.as_ref();
        let local = |f: fn(&LocalParams) -> f32| locals.is_some_and(|l| l.params.iter().any(|p| f(p) != 0.0));
        let clarity_on = self.clarity != 0.0 || local(|p| p.clarity);
        let texture_on = self.texture != 0.0 || local(|p| p.texture);
        super::perf::mark("tone map");
        let clarity =
            clarity_on.then(|| cache.clarity_map.get_or(wb, || BaseMap::build(w, h, CLARITY_MAP, lum_at)));
        super::perf::mark("clarity map");

        // log2 luminance over the region, plus the margin Texture's blur reaches into.
        let r = (w.max(h) as f32 / 1000.0).round().max(1.0) as usize;
        let margin = if texture_on { 2 * r + 1 } else { 0 };
        let reg = self.region;
        let (ex0, ey0) = ((reg.x as usize).saturating_sub(margin), (reg.y as usize).saturating_sub(margin));
        let ex1 = (reg.x as usize + reg.w as usize + margin).min(w);
        let ey1 = (reg.y as usize + reg.h as usize + margin).min(h);
        let (ew, eh) = (ex1 - ex0, ey1 - ey0);
        let extent = (wb, [ex0, ey0, ew, eh]);
        let ylog: Option<Arc<Vec<f32>>> = (clarity_on || texture_on).then(|| {
            cache.log_luminance.get_or(extent, || {
                (ey0..ey1)
                    .into_par_iter()
                    .flat_map_iter(|y| (ex0..ex1).map(move |x| fast_log2(lum_at(x, y).max(1e-6))))
                    .collect()
            })
        });
        // Texture: log luminance blurred twice (close to a Gaussian) over a few pixels; the
        // radius follows the image size so a preview and a full-size export agree.
        let texture = texture_on.then(|| {
            let ylog: &[f32] = ylog.as_ref().map_or(&[], |v| v.as_slice());
            cache.texture.get_or(extent, || box_blur_par(&box_blur_par(ylog, ew, eh, r), ew, eh, r))
        });
        super::perf::mark("log lum, texture blur");
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
                // The amounts here: global plus the masks'.
                let (local_clarity, local_texture) = match locals {
                    Some(lo) => lo.detail(lo.field.at(x, y)),
                    None => (0.0, 0.0),
                };
                let (clarity_amount, texture_amount) =
                    (self.clarity + local_clarity, self.texture + local_texture);
                let mut stops = 0.0;
                if let Some(map) = &clarity
                    && clarity_amount != 0.0
                {
                    let d = (l - map.sample(x, y)).clamp(-1.5, 1.5);
                    // Mostly the midtones: deep shadows and highlights are left alone.
                    let rel = l + fast_log2(*g) - grey;
                    stops += clarity_amount * 0.9 * d / (1.0 + rel * rel / 6.0);
                }
                if let Some(base) = &texture
                    && texture_amount != 0.0
                {
                    let d = (l - base[e]).clamp(-1.0, 1.0);
                    // Soft threshold: the smallest differences are mostly noise.
                    stops += texture_amount * 1.2 * d * (d.abs() / (d.abs() + 0.04));
                }
                *g *= fast_exp2(stops);
            }
        });
        gains
    }

    /// Relight: of the light at each pixel of `region`, the share that comes off what is
    /// nearer than one `edge` of its slice (`near`: how near everything in `around`, the region and what this looks at
    /// around it, is).
    ///
    /// Where something near meets something far, a pixel is a mix of the two (the lens
    /// and the depth map both leave the border soft), and lighting it by how near it is
    /// would light the mix as a whole: a dark thing against a bright background gets a
    /// rim, the background's light brightened along with it. So the two sides' own
    /// brightness around the pixel is weighed in: of a pixel half fur and half sky, nearly
    /// all the light is the sky's, and is lit as the sky is.
    ///
    /// The depth map's borders are also only roughly where the photo's are (the model
    /// sees the photo in blocks). Where the two sides are plainly apart in brightness,
    /// each evenly lit, a pixel near the border is told apart by its own brightness
    /// instead: which of the two it is, or how much of each.
    fn light_share(
        &self,
        img: &WorkingImage,
        relight: &Relight,
        edge: usize,
        region: Region,
        near: &MaskField,
        around: Region,
    ) -> Vec<f32> {
        use super::local::box_blur_par;
        let (step, reach) = relight_grid(img);
        let (ax, ay) = (around.x as usize, around.y as usize);
        let (ex, ey) = (ax + around.w as usize, ay + around.h as usize);
        let (gw, gh) = ((ex - ax).div_ceil(step), (ey - ay).div_ceil(step));
        let nearly = |x: usize, y: usize| relight.nearer(edge, 1.0 - near.at(x, y)[0]);
        // Per cell: how much of it is near, and far, and the light of each.
        let cells: Vec<[f32; 6]> = (0..gw * gh)
            .into_par_iter()
            .map(|k| {
                let (cx, cy) = (ax + (k % gw) * step, ay + (k / gw) * step);
                let (cx1, cy1) = ((cx + step).min(ex), (cy + step).min(ey));
                let mut sum = [0.0f32; 6];
                for y in cy..cy1 {
                    for x in cx..cx1 {
                        let n = nearly(x, y);
                        let l = self.luminance(self.linear(input_at(img, x, y)));
                        // (Squared: what is clearly one or the other counts most.)
                        let (a, b) = (n * n, (1.0 - n) * (1.0 - n));
                        let add = [a, a * l, a * l * l, b, b * l, b * l * l];
                        sum = std::array::from_fn(|c| sum[c] + add[c]);
                    }
                }
                let count = ((cx1 - cx) * (cy1 - cy)) as f32;
                sum.map(|v| v / count)
            })
            .collect();
        let planes: [Vec<f32>; 6] = std::array::from_fn(|c| {
            let v: Vec<f32> = cells.iter().map(|s| s[c]).collect();
            box_blur_par(&box_blur_par(&v, gw, gh, reach), gw, gh, reach)
        });
        let (w, h) = (region.w as usize, region.h as usize);
        let mut share = vec![0.0f32; w * h];
        share.par_chunks_mut(w).enumerate().for_each(|(j, row)| {
            let y = region.y as usize + j;
            let fy = (((y - ay) as f32 + 0.5) / step as f32 - 0.5).clamp(0.0, (gh - 1) as f32);
            let (y0, ty) = (fy as usize, fy.fract());
            let y1 = (y0 + 1).min(gh - 1);
            for (i, out) in row.iter_mut().enumerate() {
                let x = region.x as usize + i;
                let fx = (((x - ax) as f32 + 0.5) / step as f32 - 0.5).clamp(0.0, (gw - 1) as f32);
                let (x0, tx) = (fx as usize, fx.fract());
                let x1 = (x0 + 1).min(gw - 1);
                let at = |c: usize| {
                    let p = &planes[c];
                    let top = p[y0 * gw + x0] + tx * (p[y0 * gw + x1] - p[y0 * gw + x0]);
                    let bottom = p[y1 * gw + x0] + tx * (p[y1 * gw + x1] - p[y1 * gw + x0]);
                    top + ty * (bottom - top)
                };
                let n = nearly(x, y);
                let (a, b) = (at(0), at(3));
                *out = n;
                if a <= 1e-4 || b <= 1e-4 {
                    continue;
                }
                // Each side's brightness around here, and how even it is.
                let (front, back) = (at(1) / a, at(4) / b);
                let spread = |sq: f32, mean: f32| (sq - mean * mean).max(0.0).sqrt();
                let uneven = spread(at(2) / a, front) + spread(at(5) / b, back);
                let apart = (front - back).abs() / (uneven + 0.02 * (front + back) + 1e-9);
                // Near a border between two sides plainly apart: by the pixel's own
                // brightness.
                let step = |lo: f32, hi: f32, v: f32| {
                    let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t)
                };
                let sure = step(2.0, 4.0, apart) * step(0.02, 0.15, a.min(b) / (a + b));
                let mut part = n;
                if sure > 0.0 {
                    let l = self.luminance(self.linear(input_at(img, x, y)));
                    part += sure * (((l - back) / (front - back)).clamp(0.0, 1.0) - n);
                }
                let (front, back) = (part * front, (1.0 - part) * back);
                if front + back > 1e-9 {
                    *out = front / (front + back);
                }
            }
        });
        share
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
        if let Some(near) = &self.depth_shown {
            // White near, black far, about as the map is kept.
            let v = near.at(x, y)[0];
            return [v * v; 3];
        }
        let mut lin = self.linear(p);
        // Masks: how much of each active mask applies here.
        let masks = self.locals.as_ref().map(|l| (l, l.field.at(x, y)));
        if let Some((l, ms)) = masks {
            lin = l.white_balance(lin, ms);
        }
        if let Some(haze) = &self.haze {
            // Global Dehaze plus the masks'.
            let amount = self.dehaze + masks.map_or(0.0, |(l, ms)| l.dehaze(ms));
            if amount > 0.0 {
                lin = haze.clear(lin, x, y, amount.min(1.0));
            } else if amount < 0.0 {
                lin = add_haze(lin, (-amount).min(1.0), haze.airlight);
            }
        }
        // Relight: the light on its slice of the depth, and on what is in front of that
        // and behind it (then any fog over it).
        if let Some(lit) = &self.relight {
            let r = &self.region;
            let k = (y - r.y as usize) * r.w as usize + (x - r.x as usize);
            let far = 1.0 - lit.near.at(x, y)[0];
            let [front, back] = [0, 1].map(|edge| match &lit.shares[edge] {
                Some(share) => share[k],
                None => lit.relight.nearer(edge, far),
            });
            let spot = lit.spot.as_ref().map_or(1.0, |s| s.at(x, y)[0]);
            let mut by = lit.relight.gains(front, back, spot);
            if lit.relight.show {
                // What the slice's light does not reach, dimmed.
                let dim = 0.25 + 0.75 * ((back - front) * spot).clamp(0.0, 1.0);
                by = by.map(|g| g * dim);
            }
            lin = [lin[0] * by[0], lin[1] * by[1], lin[2] * by[2]];
        }
        if let Some((fog, near, color, height)) = &self.fog {
            let far = 1.0 - near.at(x, y)[0];
            let share = fog.share(far, 1.0 - (y as f32 + 0.5) / height) * fog.amount;
            lin = [0, 1, 2].map(|c| lin[c] + share * (color[c] - lin[c]));
        }
        let g = match &self.gains {
            Some(gains) => {
                let r = &self.region;
                gains[(y - r.y as usize) * r.w as usize + (x - r.x as usize)]
            }
            None => self.gain,
        };
        let g = match &self.vignette {
            Some(v) if v.style == VignetteStyle::HighlightPriority => {
                g * v.gain(x, y, self.luminance(lin) * g)
            }
            _ => g,
        };
        let g = match masks {
            Some((l, ms)) => g * l.exposure(ms),
            None => g,
        };
        let mut d = [
            lookup(&self.luts[0], lin[0] * g),
            lookup(&self.luts[1], lin[1] * g),
            lookup(&self.luts[2], lin[2] * g),
        ];
        if let Some((l, ms)) = masks {
            d = l.tone(d, ms, &self.luma);
        }
        // Defringe: purple / green fringes at high-contrast edges lose their colour.
        if let Some(f) = &self.fringe {
            d = f.apply(d, x, y);
        }
        // Profile: the LUT's look on the base tone; the colour panels adjust on top.
        if let Some(lut) = &self.lut {
            d = lut.apply(d);
        }
        // HSL in colour; in Black & White the B&W Mix decides how light each colour turns.
        if let Some(hsl) = &self.hsl
            && !self.bw
        {
            d = hsl.apply(d);
        }
        let luma = self.luminance(d);
        if self.bw {
            d = [self.hsl.as_ref().map_or(luma, |m| m.grey(d, luma)).min(1.0); 3];
        } else if self.vibrance != 0.0 || self.saturation != 0.0 {
            let (hi, lo) = (d[0].max(d[1]).max(d[2]), d[0].min(d[1]).min(d[2]));
            let sat = if hi > 1e-6 { (hi - lo) / hi } else { 0.0 };
            let mut v = self.vibrance * (1.0 - sat).powi(2);
            if self.vibrance > 0.0 && d[0] > d[1] && d[1] > d[2] {
                // Protect skin tones (orange hues) from vibrance boosts.
                let h = (d[1] - d[2]) / (d[0] - d[2]);
                v *= 1.0 - 0.6 * (-((h - 0.4) / 0.2).powi(2)).exp();
            }
            let f = (1.0 + v) * (1.0 + self.saturation);
            d = d.map(|c| (luma + (c - luma) * f).max(0.0));
        }
        if let Some((l, ms)) = masks
            && !self.bw
        {
            d = l.color(d, ms);
        }
        // Color Grading, so it also tones black & white photos (split toning); then the
        // vignette's after-tone styles.
        if let Some(g) = &self.grading {
            d = g.apply(d);
        }
        if let Some(glow) = &self.glow {
            d = glow.apply(d, x, y);
        }
        match &self.vignette {
            Some(v) => v.after_tone(d, x, y),
            None => d,
        }
    }
}

/// Entries of a mask's tone table (indexed by √ of display-linear luminance).
const LOCAL_LUT: usize = 1024;

/// One mask's local adjustments, ready to apply.
struct LocalParams {
    /// Temp / Tint as channel gains.
    wb: Option<[f32; 3]>,
    /// Exposure, stops.
    ev: f32,
    /// Contrast, Highlights, Shadows, Whites, Blacks: output / input luminance by input.
    tone: Option<Vec<f32>>,
    /// Saturation (−1..+1) and Hue (radians).
    saturation: f32,
    hue: f32,
    /// Texture, Clarity and Dehaze, −1..+1 (added to the global ones where the mask is).
    texture: f32,
    clarity: f32,
    dehaze: f32,
    /// Sharpness −1..+1 and Noise 0..1 (applied with Detail).
    sharpness: f32,
    noise: f32,
    /// The Color swatch as an Oklab (a, b) shift.
    tint: Option<[f32; 2]>,
}

impl LocalParams {
    fn new(a: &LocalAdjust) -> LocalParams {
        let tone_on = [a.contrast, a.highlights, a.shadows, a.whites, a.blacks].iter().any(|v| *v != 0.0);
        let tone = tone_on.then(|| {
            let tp = ToneParams {
                contrast: (a.contrast / 100.0).clamp(-1.0, 1.0),
                whites: a.whites / 100.0,
                blacks: a.blacks / 100.0,
                shoulder: 1.0,
            };
            let (hl, sh) = (a.highlights / 100.0, a.shadows / 100.0);
            (0..LOCAL_LUT)
                .map(|i| {
                    let s = i as f32 / (LOCAL_LUT - 1) as f32;
                    let y = (s * s).max(1e-5);
                    // Highlights / Shadows bend the tones by brightness, then the curve.
                    let y1 = y * tone_gain((y / MIDDLE_GREY).log2(), hl, sh).exp2();
                    tone(y1, &tp) / y
                })
                .collect()
        });
        LocalParams {
            wb: (a.temp != 0.0 || a.tint != 0.0).then(|| relative_multipliers(a.temp, a.tint)),
            ev: a.exposure,
            tone,
            saturation: a.saturation / 100.0,
            hue: a.hue.to_radians(),
            texture: a.texture / 100.0,
            clarity: a.clarity / 100.0,
            dehaze: a.dehaze / 100.0,
            sharpness: a.sharpness / 100.0,
            noise: a.noise.max(0.0) / 100.0,
            tint: a.color.filter(|c| c[1] > 0.0).map(|[h, s]| super::grading::tint(h, s)),
        }
    }
}

/// The masks' local adjustments for a render.
struct Locals {
    field: Arc<MaskField>,
    params: Vec<LocalParams>,
    to_srgb: Mat3f,
    to_work: Mat3f,
}

impl Locals {
    fn new(field: Arc<MaskField>, masks: &[Mask]) -> Locals {
        let params = field.active.iter().map(|&i| LocalParams::new(&masks[i].adjust)).collect();
        Locals {
            field,
            params,
            to_srgb: matrix::to_f32(&matrix::work_to_srgb()),
            to_work: matrix::to_f32(&matrix::srgb_to_work()),
        }
    }

    /// The masks that apply at a pixel, with their values.
    #[inline]
    fn applying<'a>(&'a self, ms: &'a [f32]) -> impl Iterator<Item = (f32, &'a LocalParams)> {
        ms.iter().zip(&self.params).filter(|(m, _)| **m > 0.0).map(|(m, p)| (*m, p))
    }

    /// Temp / Tint, on scene-linear values.
    #[inline]
    fn white_balance(&self, mut lin: [f32; 3], ms: &[f32]) -> [f32; 3] {
        for (m, p) in self.applying(ms) {
            if let Some(g) = p.wb {
                lin = std::array::from_fn(|c| lin[c] * (1.0 + m * (g[c] - 1.0)));
            }
        }
        lin
    }

    /// Clarity and Texture to add here.
    #[inline]
    fn detail(&self, ms: &[f32]) -> (f32, f32) {
        self.applying(ms).fold((0.0, 0.0), |(c, t), (m, p)| (c + m * p.clarity, t + m * p.texture))
    }

    /// Sharpness and Noise here.
    #[inline]
    fn sharpness_noise(&self, ms: &[f32]) -> (f32, f32) {
        self.applying(ms).fold((0.0, 0.0), |(s, n), (m, p)| (s + m * p.sharpness, n + m * p.noise))
    }

    /// Dehaze to add here.
    #[inline]
    fn dehaze(&self, ms: &[f32]) -> f32 {
        self.applying(ms).map(|(m, p)| m * p.dehaze).sum()
    }

    /// Exposure, as a gain.
    #[inline]
    fn exposure(&self, ms: &[f32]) -> f32 {
        let stops: f32 = self.applying(ms).map(|(m, p)| m * p.ev).sum();
        if stops == 0.0 { 1.0 } else { fast_exp2(stops) }
    }

    /// Contrast, Highlights, Shadows, Whites, Blacks: on display-linear values, through
    /// luminance (keeping hues).
    #[inline]
    fn tone(&self, mut d: [f32; 3], ms: &[f32], luma: &[f32; 3]) -> [f32; 3] {
        for (m, p) in self.applying(ms) {
            let Some(lut) = &p.tone else { continue };
            let y = (luma[0] * d[0] + luma[1] * d[1] + luma[2] * d[2]).max(0.0);
            let f = (y.sqrt() * (LOCAL_LUT - 1) as f32).min((LOCAL_LUT - 1) as f32);
            let i = (f as usize).min(LOCAL_LUT - 2);
            let ratio = lut[i] + (f - i as f32) * (lut[i + 1] - lut[i]);
            let k = 1.0 + m * (ratio - 1.0);
            d = d.map(|c| c * k);
        }
        d
    }

    /// Saturation and Hue, in Oklab.
    #[inline]
    fn color(&self, d: [f32; 3], ms: &[f32]) -> [f32; 3] {
        let (mut sat, mut hue, mut shift) = (1.0f32, 0.0f32, [0.0f32; 2]);
        for (m, p) in self.applying(ms) {
            sat *= (1.0 + m * p.saturation).max(0.0);
            hue += m * p.hue;
            if let Some(t) = p.tint {
                shift = [shift[0] + m * t[0], shift[1] + m * t[1]];
            }
        }
        if sat == 1.0 && hue == 0.0 && shift == [0.0; 2] {
            return d;
        }
        let [l, a, b] = srgb_to_oklab(matrix::apply(&self.to_srgb, d));
        let (cos, sin) = (hue.cos(), hue.sin());
        // Hue and Saturation, then the Color swatch's tint (scaled down in the deepest
        // shadows, which it would otherwise colour).
        let (a, b) = ((a * cos - b * sin) * sat, (a * sin + b * cos) * sat);
        let k = (l / 0.3).min(1.0);
        let (a, b) = (a + k * shift[0], b + k * shift[1]);
        matrix::apply(&self.to_work, oklab_to_srgb([l, a, b])).map(|v| v.max(0.0))
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
    render_region_srgb8_view(img, s, region, None)
}

/// [`render_region_srgb8`], or one of LrC's greyscale sharpening previews (`view`).
pub fn render_region_srgb8_view(
    img: &WorkingImage,
    s: &DevelopSettings,
    region: Region,
    view: Option<SharpenView>,
) -> RgbImage {
    render_region_srgb8_until(img, s, region, view, &|| false).expect("a render nothing stops")
}

/// [`render_region_srgb8_view`] that gives up (`None`) once `stop` says so: it is asked
/// between the render's stages, so a render nobody waits for any more (the slider moved
/// on) ends within a few milliseconds.
pub fn render_region_srgb8_until(
    img: &WorkingImage,
    s: &DevelopSettings,
    region: Region,
    view: Option<SharpenView>,
    stop: &(dyn Fn() -> bool + Sync),
) -> Option<RgbImage> {
    super::perf::begin();
    // The Remove tool's spots first, on the photo's own pixels.
    let spotless = healed(img, &s.spots, s.dust.as_ref());
    super::perf::mark("heal");
    if stop() {
        return None;
    }
    let img: &WorkingImage = &spotless;
    let source = img;
    let geometry = transformed(img, &s.transform);
    super::perf::mark("transform");
    if stop() {
        return None;
    }
    let img: &WorkingImage = &geometry;
    let region = region.clipped(img);
    let enc = SrgbEncoder::new();
    let to_srgb = matrix::to_f32(&matrix::work_to_srgb());
    let mut out = RgbImage::new(region.w, region.h);
    let finish = Finish::new(img, s, view.is_none());
    let (rx, ry) = (region.x as usize, region.y as usize);
    if uses_detail(s) || view.is_some() {
        let detailed = Detailed::render(source, img, s, region, view, stop)?;
        out.par_chunks_mut(3 * region.w as usize).enumerate().for_each(|(j, row)| {
            for (i, o) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                *o = finish.apply(detailed.srgb(i, j), rx + i, ry + j).map(|v| enc.encode(v));
            }
        });
        super::perf::mark("encode");
        return Some(out);
    }
    let field = MaskField::build(source, img, &s.masks, region);
    super::perf::mark("mask field");
    if stop() {
        return None;
    }
    let lights =
        Lights { depth: &|r| nearness(source, img, s, r), spot: &|r| relight_spot(source, img, s, r) };
    let prep = Prepared::new(img, s, region, field, &lights);
    if stop() {
        return None;
    }
    render_rows(img, region, &mut out, 3, |o, p, x, y| {
        let l = finish.apply(matrix::apply(&to_srgb, prep.pixel(p, x, y)), x, y);
        o.copy_from_slice(&l.map(|v| enc.encode(v)));
    });
    super::perf::mark("pixels + encode");
    Some(out)
}

/// Mask `index`'s values over the developed photo (the size `render_srgb8` makes it), as
/// 0..255: the red overlay of the mask being edited. `None` if there is no such mask or it
/// has no components.
pub fn render_mask(img: &WorkingImage, s: &DevelopSettings, index: usize) -> Option<image::GrayImage> {
    let mask = s.masks.get(index)?;
    if mask.components.is_empty() {
        return None;
    }
    // The overlay shows the mask's shape whatever its Amount or visibility.
    let mut masks = s.masks.clone();
    masks[index].amount = 100.0;
    let geometry = transformed(img, &s.transform);
    let shown: &WorkingImage = &geometry;
    let region = Region::whole(shown);
    let field = MaskField::build_for(img, shown, &masks, vec![index], region)?;
    let values = field.plane(0);
    image::GrayImage::from_raw(region.w, region.h, values.iter().map(|v| (v * 255.0).round() as u8).collect())
}

/// Rendered pixels per full-size pixel (Detail and Grain are sized in full-size pixels).
fn render_scale(img: &WorkingImage) -> f32 {
    img.width() as f32 / img.full_size.0.max(1) as f32
}

/// The last steps, on final linear RGB: grain, then white where a Transform left the frame
/// uncovered (as LrC shows it before cropping).
struct Finish<'a> {
    grain: Option<Grain>,
    coverage: Option<&'a [f32]>,
    width: usize,
}

impl<'a> Finish<'a> {
    fn new(img: &'a WorkingImage, s: &DevelopSettings, grain: bool) -> Finish<'a> {
        Finish {
            grain: Grain::new(&s.effects, render_scale(img)).filter(|_| grain),
            coverage: img.coverage.as_deref().map(Vec::as_slice),
            width: img.width() as usize,
        }
    }

    #[inline]
    fn apply(&self, rgb: [f32; 3], x: usize, y: usize) -> [f32; 3] {
        let rgb = match &self.grain {
            Some(g) => g.apply(rgb, x, y),
            None => rgb,
        };
        match self.coverage {
            Some(c) if c[y * self.width + x] < 1.0 => {
                let a = c[y * self.width + x];
                rgb.map(|v| v * a + (1.0 - a))
            }
            _ => rgb,
        }
    }
}

/// Render for export: 16-bit linear working space (linear ProPhoto RGB).
pub fn render_linear16(img: &WorkingImage, s: &DevelopSettings) -> ImageBuffer<Rgb<u16>, Vec<u16>> {
    // The Remove tool's spots first, on the photo's own pixels.
    let spotless = healed(img, &s.spots, s.dust.as_ref());
    let img: &WorkingImage = &spotless;
    let source = img;
    let geometry = transformed(img, &s.transform);
    let img: &WorkingImage = &geometry;
    let region = Region::whole(img);
    let mut out = ImageBuffer::<Rgb<u16>, Vec<u16>>::new(img.width(), img.height());
    let to_u16 = |v: f32| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
    let finish = Finish::new(img, s, true);
    if uses_detail(s) {
        let detailed = Detailed::render(source, img, s, region, None, &|| false).expect("nothing stops it");
        let to_work = matrix::to_f32(&matrix::srgb_to_work());
        out.par_chunks_mut(3 * region.w as usize).enumerate().for_each(|(j, row)| {
            for (i, o) in row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                *o = finish.apply(matrix::apply(&to_work, detailed.srgb(i, j)), i, j).map(to_u16);
            }
        });
        return out;
    }
    let lights =
        Lights { depth: &|r| nearness(source, img, s, r), spot: &|r| relight_spot(source, img, s, r) };
    let prep = Prepared::new(img, s, region, MaskField::build(source, img, &s.masks, region), &lights);
    render_rows(img, region, &mut out, 3, |o, p, x, y| {
        o.copy_from_slice(&finish.apply(prep.pixel(p, x, y), x, y).map(to_u16));
    });
    out
}

/// A region rendered with Detail (sharpening, noise reduction): those filters reach across
/// pixels, so the region is rendered with a margin, filtered as Oklab planes, then read
/// back without the margin.
struct Detailed {
    planes: [Vec<f32>; 3],
    /// The rendered area's width, and where the requested region starts in it.
    width: usize,
    offset: (usize, usize),
}

impl Detailed {
    fn render(
        source: &WorkingImage,
        img: &WorkingImage,
        s: &DevelopSettings,
        region: Region,
        view: Option<SharpenView>,
        stop: &(dyn Fn() -> bool + Sync),
    ) -> Option<Detailed> {
        // Rendered pixels per full-size pixel: the filters' radii are in full-size pixels.
        let scale = render_scale(img);
        // Lens Blur reads further around a pixel than Detail, on a grid of its own.
        let blur = super::lens_blur::LensBlur::new(&s.effects, scale, img.full_size.0.max(img.full_size.1));
        let (bm, bg) = blur.map_or((0, 1), |b| (b.margin(), b.grid()));
        let m = s.detail.margin(scale).max(bm) as u32;
        let g = {
            let dg = s.detail.grid(scale).max(1);
            let gcd = |mut a: usize, mut b: usize| {
                while b != 0 {
                    (a, b) = (b, a % b);
                }
                a
            };
            (dg / gcd(dg, bg) * bg) as u32
        };
        let (x0, y0) = (region.x.saturating_sub(m) / g * g, region.y.saturating_sub(m) / g * g);
        let area = Region {
            x: x0,
            y: y0,
            w: (region.x + region.w + m).min(img.width()) - x0,
            h: (region.y + region.h + m).min(img.height()) - y0,
        };
        let field = MaskField::build(source, img, &s.masks, area);
        super::perf::mark("mask field");
        if stop() {
            return None;
        }
        // The area developed up to Detail: kept until something before Detail changes, so
        // the Detail sliders (and grain) only redo what comes after.
        let effects = Effects {
            grain_amount: 0.0,
            grain_size: Effects::default().grain_size,
            grain_roughness: Effects::default().grain_roughness,
            ..s.effects.clone()
        };
        let key = (DevelopSettings { detail: Default::default(), effects, ..s.clone() }, area);
        let (aw, ah, w) = (area.w as usize, area.h as usize, img.width() as usize);
        let developed = match img.render_cache.planes.get(&key) {
            Some(planes) => planes,
            None => {
                let near = nearness(source, img, s, area);
                let depth = |r: Region| if r == area { near.clone() } else { nearness(source, img, s, r) };
                let lights = Lights { depth: &depth, spot: &|r| relight_spot(source, img, s, r) };
                let prep = Prepared::new(img, s, area, field.clone(), &lights);
                if stop() {
                    return None;
                }
                let to_srgb = matrix::to_f32(&matrix::work_to_srgb());
                let mut planes: [Vec<f32>; 3] = std::array::from_fn(|_| vec![0.0; aw * ah]);
                let [l, a, b] = &mut planes;
                let src = img.pixels.as_raw();
                (l.par_chunks_mut(aw), a.par_chunks_mut(aw), b.par_chunks_mut(aw))
                    .into_par_iter()
                    .enumerate()
                    .for_each(|(j, (l, a, b))| {
                        let (x0, y) = (area.x as usize, area.y as usize + j);
                        let row = &src[3 * (y * w + x0)..3 * (y * w + x0 + aw)];
                        for (i, p) in row.as_chunks::<3>().0.iter().enumerate() {
                            let lab = srgb_to_oklab(matrix::apply(&to_srgb, prep.pixel(*p, x0 + i, y)));
                            (l[i], a[i], b[i]) = (lab[0], lab[1], lab[2]);
                        }
                    });
                if let (Some(blur), Some(near)) = (blur, near) {
                    // In linear light, as a lens blurs.
                    let mut rgb: [Vec<f32>; 3] = std::array::from_fn(|_| vec![0.0; aw * ah]);
                    {
                        let [r, g, b] = &mut rgb;
                        (r.par_iter_mut(), g.par_iter_mut(), b.par_iter_mut())
                            .into_par_iter()
                            .enumerate()
                            .for_each(|(k, (r, g, b))| {
                                let v = oklab_to_srgb([planes[0][k], planes[1][k], planes[2][k]]);
                                (*r, *g, *b) = (v[0].max(0.0), v[1].max(0.0), v[2].max(0.0));
                            });
                    }
                    super::perf::mark("lens blur: to linear");
                    let (ax, ay) = (area.x as usize, area.y as usize);
                    blur.apply(&mut rgb, aw, ah, &|x, y| 1.0 - near.at(ax + x, ay + y)[0]);
                    {
                        let [l, a, b] = &mut planes;
                        (l.par_iter_mut(), a.par_iter_mut(), b.par_iter_mut())
                            .into_par_iter()
                            .enumerate()
                            .for_each(|(k, (l, a, b))| {
                                let lab = srgb_to_oklab([rgb[0][k], rgb[1][k], rgb[2][k]]);
                                (*l, *a, *b) = (lab[0], lab[1], lab[2]);
                            });
                    }
                    super::perf::mark("lens blur");
                }
                let planes = Arc::new(planes);
                img.render_cache.planes.put(key, planes.clone());
                planes
            }
        };
        super::perf::mark("pixels to Oklab");
        if stop() {
            return None;
        }
        let mut planes = <[Vec<f32>; 3]>::clone(&developed);
        apply_detail(&mut planes, area.w as usize, area.h as usize, &s.detail, scale, view);
        super::perf::mark("detail");
        // Masks' Sharpness and Noise, after the global Detail (not in its previews).
        let locals = field.filter(|_| view.is_none()).map(|f| Locals::new(f, &s.masks));
        if let Some(lo) = &locals
            && lo.params.iter().any(|p| p.sharpness != 0.0 || p.noise > 0.0)
        {
            let n = aw * ah;
            let (mut sharp, mut noise) = (vec![0.0f32; n], vec![0.0f32; n]);
            (sharp.par_chunks_mut(aw), noise.par_chunks_mut(aw)).into_par_iter().enumerate().for_each(
                |(j, (srow, nrow))| {
                    for i in 0..aw {
                        (srow[i], nrow[i]) =
                            lo.sharpness_noise(lo.field.at(area.x as usize + i, area.y as usize + j));
                    }
                },
            );
            apply_local(&mut planes[0], aw, ah, &sharp, &noise, scale);
        }
        Some(Detailed {
            planes,
            width: area.w as usize,
            offset: ((region.x - x0) as usize, (region.y - y0) as usize),
        })
    }

    /// Linear sRGB at (i, j) of the requested region.
    #[inline]
    fn srgb(&self, i: usize, j: usize) -> [f32; 3] {
        let k = (j + self.offset.1) * self.width + i + self.offset.0;
        oklab_to_srgb([self.planes[0][k], self.planes[1][k], self.planes[2][k]]).map(|v| v.max(0.0))
    }
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
        // The vignette would count as dark or bright corners.
        effects: Effects::default(),
        ..s.clone()
    };
    let none = Lights { depth: &|_| None, spot: &|_| None };
    let prep = Prepared::new(img, &neutral, Region::whole(img), None, &none);
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

/// Does a render need the Detail path (lightness planes with a margin): global Detail, or
/// a mask's Sharpness or Noise?
fn uses_detail(s: &DevelopSettings) -> bool {
    s.detail.is_active()
        || (s.effects.blur_amount > 0.0 && s.effects.fog_depth.is_some())
        || s.masks.iter().any(|m| m.is_active() && (m.adjust.sharpness != 0.0 || m.adjust.noise > 0.0))
}
