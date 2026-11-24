//! Effects (LrC's Effects panel): Post-Crop Vignetting, Grain and Glow.
//!
//! The vignette's shape follows the image's frame (there is no crop yet, so the whole
//! image). Grain is sized in full-size pixels, like Detail: a 1:1 view shows the real
//! grain, a smaller view the averaged-out texture, as in LrC.

use serde::{Deserialize, Serialize};

use super::local::fast_exp2;

/// How the vignette changes the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VignetteStyle {
    /// Like an exposure change before the tone curve: highlights roll off, colours shift
    /// a little.
    #[default]
    HighlightPriority,
    /// After the tone curve, keeping hues; bright areas may lose detail.
    ColorPriority,
    /// Blends towards black or white (LrC's older, flatter style).
    PaintOverlay,
}

impl VignetteStyle {
    pub const ALL: [VignetteStyle; 3] = [Self::HighlightPriority, Self::ColorPriority, Self::PaintOverlay];

    pub fn label(self) -> &'static str {
        match self {
            Self::HighlightPriority => "Highlight Priority",
            Self::ColorPriority => "Color Priority",
            Self::PaintOverlay => "Paint Overlay",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Effects {
    pub vignette_style: VignetteStyle,
    /// −100 (darken) ..+100 (lighten).
    pub vignette_amount: f32,
    /// How far in the effect reaches, 0..100 (smaller: further in).
    pub vignette_midpoint: f32,
    /// −100 (follows the frame's rectangle) ..+100 (a circle).
    pub vignette_roundness: f32,
    /// Softness of the edge, 0..100.
    pub vignette_feather: f32,
    /// Keeps bright areas bright when darkening (Highlight and Color Priority), 0..100.
    pub vignette_highlights: f32,
    pub grain_amount: f32,
    /// Grain size and roughness, 0..100.
    pub grain_size: f32,
    pub grain_roughness: f32,
    /// Glow: Amount 0..100; Range 0..100 (only the brightest highlights .. broader
    /// tones); Spread 0..100 (how far it reaches); Warmth −100..+100.
    pub glow_amount: f32,
    pub glow_range: f32,
    pub glow_spread: f32,
    pub glow_warmth: f32,
    /// Fog, by how far away things are: Amount 0..100 (the farthest all fog); Distance 0..100 (where it begins, 0: right in front); Density
    /// 0..100 (how soon it thickens); Height 0..100 (0: at every height, more: lying
    /// low); Warmth −100..+100.
    pub fog_amount: f32,
    pub fog_distance: f32,
    pub fog_density: f32,
    pub fog_height: f32,
    pub fog_warmth: f32,
    /// The photo's depth map (a picture in the mask store, white near), made by a model
    /// when Fog is first used on the photo. Belongs to its photo: not copied to others.
    pub fog_depth: Option<String>,
    /// Lens Blur, by the same depth map: Amount 0..100; Focus 0..100 (the distance that
    /// stays sharp, 0: the nearest thing in the photo); Range 0..100 (how deep the sharp
    /// zone is); Highlights 0..100 (how much bright points stand out in the blur).
    pub blur_amount: f32,
    pub blur_focus: f32,
    pub blur_range: f32,
    pub blur_highlights: f32,
    /// Relight, by the same depth map: a light on a slice of the photo's depth, and what
    /// is in front of the slice and behind it lit separately. Amount −100..+100 (darker ..
    /// brighter) and Warmth −100..+100 for the slice; Depth 0..100 (where it is, 0: the
    /// nearest thing in the photo); Range 0..100 (how deep); Feather 0..100 (how
    /// gradually it turns to near and far); Size 0..100 (the round area around
    /// `relight_at` its light is kept to, 100: the whole photo); Near, Far and their
    /// Warmths −100..+100.
    pub relight_amount: f32,
    pub relight_warmth: f32,
    pub relight_range: f32,
    pub relight_size: f32,
    /// The middle of the light's round area, 0..1 of the untransformed photo.
    pub relight_at: [f32; 2],
    pub relight_near: f32,
    pub relight_far: f32,
    pub relight_depth: f32,
    pub relight_feather: f32,
    pub relight_warmth_near: f32,
    pub relight_warmth_far: f32,
    /// For looking only (never kept): render the depth map in place of the photo.
    #[serde(skip)]
    pub fog_show: bool,
    /// For looking only (never kept): dim what Relight's slice does not light.
    #[serde(skip)]
    pub relight_show: bool,
}

impl Default for Effects {
    fn default() -> Self {
        Effects {
            vignette_style: VignetteStyle::HighlightPriority,
            vignette_amount: 0.0,
            vignette_midpoint: 50.0,
            vignette_roundness: 0.0,
            vignette_feather: 50.0,
            vignette_highlights: 0.0,
            grain_amount: 0.0,
            grain_size: 25.0,
            grain_roughness: 50.0,
            glow_amount: 0.0,
            glow_range: 50.0,
            glow_spread: 50.0,
            glow_warmth: 0.0,
            fog_amount: 0.0,
            fog_distance: 30.0,
            fog_density: 50.0,
            fog_height: 0.0,
            fog_warmth: 0.0,
            fog_depth: None,
            blur_amount: 0.0,
            blur_focus: 15.0,
            blur_range: 20.0,
            blur_highlights: 0.0,
            relight_amount: 0.0,
            relight_warmth: 0.0,
            relight_range: 20.0,
            relight_size: 100.0,
            relight_at: [0.5, 0.5],
            relight_near: 0.0,
            relight_far: 0.0,
            relight_depth: 45.0,
            relight_feather: 50.0,
            relight_warmth_near: 0.0,
            relight_warmth_far: 0.0,
            fog_show: false,
            relight_show: false,
        }
    }
}

impl Effects {
    /// Relight changes something.
    pub fn relights(&self) -> bool {
        self.relight_lights()
            || self.relight_near != 0.0
            || self.relight_far != 0.0
            || self.relight_warmth_near != 0.0
            || self.relight_warmth_far != 0.0
    }

    /// Relight's slice has a light of its own.
    pub fn relight_lights(&self) -> bool {
        self.relight_amount != 0.0 || self.relight_warmth != 0.0
    }

    /// That light is kept to a round area.
    pub fn relight_spots(&self) -> bool {
        self.relight_lights() && self.relight_size < 100.0
    }

    /// Something here goes by the photo's depth map (which is made when first needed).
    pub fn uses_depth(&self) -> bool {
        self.fog_amount != 0.0 || self.blur_amount > 0.0 || self.relights()
    }
}

/// Stops of exposure for Relight's Amount, Near or Far at ±100.
const RELIGHT_STOPS: f32 = 2.0;

/// Relight as the settings ask for it: what each distance's light is multiplied by.
#[derive(Debug, Clone, Copy)]
pub(super) struct Relight {
    /// What light is multiplied by (R, G, B): in front of the slice, in it, behind it.
    gains: [[f32; 3]; 3],
    /// Where the slice begins and ends (0..1 far), each the middle of a turn `half` wide
    /// to either side: Range is how deep the slice is fully lit.
    edges: [f32; 2],
    half: f32,
    /// The slice's light is kept to a round area on the photo.
    pub spot: bool,
    /// Show where the slice's light falls.
    pub show: bool,
}

impl Relight {
    pub fn new(e: &Effects, luma: [f32; 3]) -> Option<Relight> {
        if !e.relights() || e.fog_show {
            return None;
        }
        let part = |v: f32| (v / 100.0).clamp(-1.0, 1.0);
        // Brighter or darker, and warmer or cooler without a grey getting brighter or
        // darker by that.
        let gains = |light: f32, warmth: f32| {
            let gain = 2f32.powf(part(light) * RELIGHT_STOPS);
            let warm = part(warmth);
            let tint = [1.0 + 0.25 * warm, 1.0, 1.0 - 0.25 * warm];
            let grey = luma[0] * tint[0] + luma[1] * tint[1] + luma[2] * tint[2];
            tint.map(|t| gain * t / grey)
        };
        let depth = e.relight_depth.clamp(0.0, 100.0) / 100.0;
        let half = 0.005 + 0.12 * e.relight_feather.clamp(0.0, 100.0) / 100.0;
        let reach = 0.5 * e.relight_range.clamp(0.0, 100.0) / 100.0 + half;
        Some(Relight {
            gains: [
                gains(e.relight_near, e.relight_warmth_near),
                gains(e.relight_amount, e.relight_warmth),
                gains(e.relight_far, e.relight_warmth_far),
            ],
            edges: [depth - reach, depth + reach],
            half,
            spot: e.relight_spots(),
            show: e.relight_show,
        })
    }

    /// How much of something `far` away (0: the nearest thing in the photo, 1: the
    /// farthest) is nearer than the slice's front (`edge` 0) or its back (1).
    #[inline]
    pub fn nearer(&self, edge: usize, far: f32) -> f32 {
        1.0 - smoothstep(self.edges[edge] - self.half, self.edges[edge] + self.half, far)
    }

    /// Whether what is on the two sides of an edge is lit differently at all (else the
    /// border there needs no care).
    pub fn divides(&self, edge: usize) -> bool {
        let [near, light, far] = self.gains;
        let side = if edge == 0 { near } else { far };
        self.show || side != light || (self.spot && side != [1.0; 3])
    }

    /// What scene-linear RGB is multiplied by where `front` of the light comes off what
    /// is nearer than the slice, `back` off what is nearer than its far side (so
    /// `back − front` off the slice itself), with `spot` of the slice's light reaching
    /// here.
    #[inline]
    pub fn gains(&self, front: f32, back: f32, spot: f32) -> [f32; 3] {
        let [near, light, far] = self.gains;
        [0, 1, 2].map(|c| {
            let lit = 1.0 + spot * (light[c] - 1.0);
            far[c] + back * (lit - far[c]) + front * (near[c] - lit)
        })
    }
}

/// Fog as the settings ask for it: how much of a pixel is fog, from how far away it is.
#[derive(Debug, Clone, Copy)]
pub(super) struct Fog {
    /// 0..1.
    pub amount: f32,
    start: f32,
    rate: f32,
    full: f32,
    height: f32,
    warmth: f32,
}

impl Fog {
    pub fn new(e: &Effects) -> Option<Fog> {
        if e.fog_amount <= 0.0 {
            return None;
        }
        let rate = 0.5 + 7.5 * (e.fog_density.clamp(0.0, 100.0) / 100.0).powi(2);
        Some(Fog {
            amount: (e.fog_amount / 100.0).min(1.0),
            start: e.fog_distance.clamp(0.0, 100.0) / 100.0,
            rate,
            full: 1.0 - (-rate).exp(),
            height: e.fog_height.clamp(0.0, 100.0) / 100.0,
            warmth: (e.fog_warmth / 100.0).clamp(-1.0, 1.0),
        })
    }

    /// How much of the full effect applies `far` away (0: the nearest thing in the photo,
    /// 1: the farthest) and `up` the frame (0: the bottom edge, 1: the top): none before
    /// the Distance, all at the farthest, thickening sooner with more Density, and
    /// thinning upward with Height.
    #[inline]
    pub fn share(&self, far: f32, up: f32) -> f32 {
        let reach = ((far - self.start) / (1.0 - self.start).max(0.02)).clamp(0.0, 1.0);
        let thick = (1.0 - (-self.rate * reach).exp()) / self.full;
        thick * (-4.0 * self.height * up).exp()
    }

    /// The fog's colour: the photo's own haze colour, half way to grey, warmer or cooler.
    pub fn color(&self, airlight: [f32; 3], luma: [f32; 3]) -> [f32; 3] {
        let grey = airlight[0] * luma[0] + airlight[1] * luma[1] + airlight[2] * luma[2];
        let tint = [1.0 + 0.3 * self.warmth, 1.0, 1.0 - 0.3 * self.warmth];
        [0, 1, 2].map(|c| (0.5 * (airlight[c] + grey) * tint[c]).max(0.0))
    }
}

/// An Effects panel slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectParam {
    VignetteAmount,
    VignetteMidpoint,
    VignetteRoundness,
    VignetteFeather,
    VignetteHighlights,
    GrainAmount,
    GrainSize,
    GrainRoughness,
    GlowAmount,
    GlowRange,
    GlowSpread,
    GlowWarmth,
    FogAmount,
    FogDistance,
    FogDensity,
    FogHeight,
    FogWarmth,
    BlurAmount,
    BlurFocus,
    BlurRange,
    BlurHighlights,
    RelightAmount,
    RelightWarmth,
    RelightDepth,
    RelightRange,
    RelightFeather,
    RelightSize,
    RelightNear,
    RelightWarmthNear,
    RelightFar,
    RelightWarmthFar,
}

impl EffectParam {
    pub const VIGNETTE: [EffectParam; 5] = [
        Self::VignetteAmount,
        Self::VignetteMidpoint,
        Self::VignetteRoundness,
        Self::VignetteFeather,
        Self::VignetteHighlights,
    ];
    pub const GRAIN: [EffectParam; 3] = [Self::GrainAmount, Self::GrainSize, Self::GrainRoughness];
    pub const GLOW: [EffectParam; 4] =
        [Self::GlowAmount, Self::GlowRange, Self::GlowSpread, Self::GlowWarmth];
    pub const FOG: [EffectParam; 5] =
        [Self::FogAmount, Self::FogDistance, Self::FogDensity, Self::FogHeight, Self::FogWarmth];
    pub const BLUR: [EffectParam; 4] =
        [Self::BlurAmount, Self::BlurFocus, Self::BlurRange, Self::BlurHighlights];
    pub const RELIGHT: [EffectParam; 10] = [
        Self::RelightAmount,
        Self::RelightWarmth,
        Self::RelightDepth,
        Self::RelightRange,
        Self::RelightFeather,
        Self::RelightSize,
        Self::RelightNear,
        Self::RelightWarmthNear,
        Self::RelightFar,
        Self::RelightWarmthFar,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::VignetteAmount | Self::GrainAmount | Self::GlowAmount => "Amount",
            Self::VignetteMidpoint => "Midpoint",
            Self::VignetteRoundness => "Roundness",
            Self::VignetteFeather => "Feather",
            Self::VignetteHighlights => "Highlights",
            Self::GrainSize => "Size",
            Self::GrainRoughness => "Roughness",
            Self::GlowRange => "Range",
            Self::GlowSpread => "Spread",
            Self::GlowWarmth => "Warmth",
            Self::FogAmount => "Amount",
            Self::FogDistance => "Distance",
            Self::FogDensity => "Density",
            Self::FogHeight => "Height",
            Self::FogWarmth => "Warmth",
            Self::BlurAmount => "Amount",
            Self::BlurFocus => "Focus",
            Self::BlurRange => "Range",
            Self::BlurHighlights => "Highlights",
            Self::RelightAmount => "Amount",
            Self::RelightWarmth => "Warmth",
            Self::RelightRange => "Range",
            Self::RelightSize => "Size",
            Self::RelightNear => "Near",
            Self::RelightFar => "Far",
            Self::RelightDepth => "Depth",
            Self::RelightFeather => "Feather",
            Self::RelightWarmthNear => "Warmth Near",
            Self::RelightWarmthFar => "Warmth Far",
        }
    }

    /// The History name's group.
    pub fn group(self) -> &'static str {
        if Self::VIGNETTE.contains(&self) {
            "Post-Crop Vignette"
        } else if Self::GLOW.contains(&self) {
            "Glow"
        } else if Self::FOG.contains(&self) {
            "Fog"
        } else if Self::BLUR.contains(&self) {
            "Lens Blur"
        } else if Self::RELIGHT.contains(&self) {
            "Relight"
        } else {
            "Grain"
        }
    }

    /// Amount and Roundness go both ways; the rest are 0..100.
    pub fn signed(self) -> bool {
        matches!(
            self,
            Self::VignetteAmount
                | Self::VignetteRoundness
                | Self::GlowWarmth
                | Self::FogWarmth
                | Self::RelightAmount
                | Self::RelightWarmth
                | Self::RelightNear
                | Self::RelightFar
                | Self::RelightWarmthNear
                | Self::RelightWarmthFar
        )
    }

    pub fn range(self) -> (f32, f32) {
        if self.signed() { (-100.0, 100.0) } else { (0.0, 100.0) }
    }

    pub fn get(self, e: &Effects) -> f32 {
        *match self {
            Self::VignetteAmount => &e.vignette_amount,
            Self::VignetteMidpoint => &e.vignette_midpoint,
            Self::VignetteRoundness => &e.vignette_roundness,
            Self::VignetteFeather => &e.vignette_feather,
            Self::VignetteHighlights => &e.vignette_highlights,
            Self::GrainAmount => &e.grain_amount,
            Self::GrainSize => &e.grain_size,
            Self::GrainRoughness => &e.grain_roughness,
            Self::GlowAmount => &e.glow_amount,
            Self::GlowRange => &e.glow_range,
            Self::GlowSpread => &e.glow_spread,
            Self::GlowWarmth => &e.glow_warmth,
            Self::FogAmount => &e.fog_amount,
            Self::FogDistance => &e.fog_distance,
            Self::FogDensity => &e.fog_density,
            Self::FogHeight => &e.fog_height,
            Self::FogWarmth => &e.fog_warmth,
            Self::BlurAmount => &e.blur_amount,
            Self::BlurFocus => &e.blur_focus,
            Self::BlurRange => &e.blur_range,
            Self::BlurHighlights => &e.blur_highlights,
            Self::RelightAmount => &e.relight_amount,
            Self::RelightWarmth => &e.relight_warmth,
            Self::RelightRange => &e.relight_range,
            Self::RelightSize => &e.relight_size,
            Self::RelightNear => &e.relight_near,
            Self::RelightFar => &e.relight_far,
            Self::RelightDepth => &e.relight_depth,
            Self::RelightFeather => &e.relight_feather,
            Self::RelightWarmthNear => &e.relight_warmth_near,
            Self::RelightWarmthFar => &e.relight_warmth_far,
        }
    }

    pub fn get_mut(self, e: &mut Effects) -> &mut f32 {
        match self {
            Self::VignetteAmount => &mut e.vignette_amount,
            Self::VignetteMidpoint => &mut e.vignette_midpoint,
            Self::VignetteRoundness => &mut e.vignette_roundness,
            Self::VignetteFeather => &mut e.vignette_feather,
            Self::VignetteHighlights => &mut e.vignette_highlights,
            Self::GrainAmount => &mut e.grain_amount,
            Self::GrainSize => &mut e.grain_size,
            Self::GrainRoughness => &mut e.grain_roughness,
            Self::GlowAmount => &mut e.glow_amount,
            Self::GlowRange => &mut e.glow_range,
            Self::GlowSpread => &mut e.glow_spread,
            Self::GlowWarmth => &mut e.glow_warmth,
            Self::FogAmount => &mut e.fog_amount,
            Self::FogDistance => &mut e.fog_distance,
            Self::FogDensity => &mut e.fog_density,
            Self::FogHeight => &mut e.fog_height,
            Self::FogWarmth => &mut e.fog_warmth,
            Self::BlurAmount => &mut e.blur_amount,
            Self::BlurFocus => &mut e.blur_focus,
            Self::BlurRange => &mut e.blur_range,
            Self::BlurHighlights => &mut e.blur_highlights,
            Self::RelightAmount => &mut e.relight_amount,
            Self::RelightWarmth => &mut e.relight_warmth,
            Self::RelightRange => &mut e.relight_range,
            Self::RelightSize => &mut e.relight_size,
            Self::RelightNear => &mut e.relight_near,
            Self::RelightFar => &mut e.relight_far,
            Self::RelightDepth => &mut e.relight_depth,
            Self::RelightFeather => &mut e.relight_feather,
            Self::RelightWarmthNear => &mut e.relight_warmth_near,
            Self::RelightWarmthFar => &mut e.relight_warmth_far,
        }
    }
}

/// Stops of exposure at the frame's edge for Amount ±100.
const VIGNETTE_STOPS: f32 = 2.5;

#[inline]
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Entries of the table from a squarer vignette's summed distance terms to its weight.
const SHAPE_LUT: usize = 4096;

/// The vignette, ready to render an image `w × h`.
pub(super) struct Vignette {
    pub(super) style: VignetteStyle,
    /// −1..+1.
    amount: f32,
    highlights: f32,
    /// Where the effect is half on (0 centre, 1 corner) and half its transition width.
    centre: f32,
    half_width: f32,
    /// Roundness above 0: how far towards a circle.
    circle: f32,
    /// Distance terms per column and per row (they add up): the frame's shape
    /// (`|n|^p / 2`, n −1..+1 across the frame) and, for a rounder vignette, the squared
    /// distance from the centre with the corners at 1.
    frame_x: Vec<f32>,
    frame_y: Vec<f32>,
    round_x: Vec<f32>,
    round_y: Vec<f32>,
    /// For a squarer vignette (exponent above 2): summed frame terms → weight.
    shape: Option<Vec<f32>>,
}

impl Vignette {
    pub(super) fn new(e: &Effects, w: u32, h: u32) -> Option<Vignette> {
        if e.vignette_amount == 0.0 {
            return None;
        }
        let (w, h) = (w.max(1) as usize, h.max(1) as usize);
        let roundness = e.vignette_roundness / 100.0;
        // Below 0 the frame's ellipse turns into a squarer superellipse.
        let p = 2.0 + 8.0 * (-roundness).max(0.0);
        let half_diag = (w as f32).hypot(h as f32) / 2.0;
        let axis = |n: usize, f: &dyn Fn(f32) -> f32| -> Vec<f32> {
            (0..n).map(|i| f(((i as f32 + 0.5) / n as f32 * 2.0 - 1.0).abs())).collect()
        };
        let frame = |t: f32| if p == 2.0 { t * t * 0.5 } else { t.powf(p) * 0.5 };
        let circle = roundness.max(0.0);
        let (round_x, round_y) = if circle > 0.0 {
            let (hw, hh) = (w as f32 / 2.0 / half_diag, h as f32 / 2.0 / half_diag);
            (axis(w, &|t| (t * hw).powi(2)), axis(h, &|t| (t * hh).powi(2)))
        } else {
            (Vec::new(), Vec::new())
        };
        let mut v = Vignette {
            style: e.vignette_style,
            amount: e.vignette_amount / 100.0,
            highlights: e.vignette_highlights / 100.0,
            centre: 0.35 + 0.7 * e.vignette_midpoint / 100.0,
            half_width: 0.01 + 0.5 * e.vignette_feather / 100.0,
            circle,
            frame_x: axis(w, &frame),
            frame_y: axis(h, &frame),
            round_x,
            round_y,
            shape: None,
        };
        if p != 2.0 {
            let table = (0..SHAPE_LUT)
                .map(|i| v.falloff((i as f32 / (SHAPE_LUT - 1) as f32).powf(1.0 / p)))
                .collect();
            v.shape = Some(table);
        }
        Some(v)
    }

    /// Weight at distance `d` (0 centre, 1 corner).
    #[inline]
    fn falloff(&self, d: f32) -> f32 {
        smoothstep(self.centre - self.half_width, self.centre + self.half_width, d)
    }

    /// How much of the effect pixel (x, y) gets, 0 (centre) ..1.
    #[inline]
    fn weight(&self, x: usize, y: usize) -> f32 {
        let s = self.frame_x[x] + self.frame_y[y];
        if let Some(shape) = &self.shape {
            let f = s.min(1.0) * (SHAPE_LUT - 1) as f32;
            let i = (f as usize).min(SHAPE_LUT - 2);
            return shape[i] + (f - i as f32) * (shape[i + 1] - shape[i]);
        }
        let frame = s.sqrt();
        let d = if self.circle > 0.0 {
            let round = (self.round_x[x] + self.round_y[y]).sqrt();
            frame + self.circle * (round - frame)
        } else {
            frame
        };
        self.falloff(d)
    }

    /// Bright areas keep their brightness when darkening, by the Highlights amount.
    #[inline]
    fn protect(&self, v: f32, bright: f32) -> f32 {
        if self.amount < 0.0 && self.highlights > 0.0 {
            v * (1.0 - self.highlights * smoothstep(0.2, 0.9, bright))
        } else {
            v
        }
    }

    /// Highlight Priority: an exposure gain on scene-linear values, before the tone curve;
    /// `bright` is the pixel's luminance after exposure.
    #[inline]
    pub(super) fn gain(&self, x: usize, y: usize, bright: f32) -> f32 {
        let v = self.protect(self.amount * self.weight(x, y), bright.max(0.0).sqrt());
        fast_exp2(VIGNETTE_STOPS * v)
    }

    /// Color Priority and Paint Overlay: on display-linear values, after the tone curve.
    #[inline]
    pub(super) fn after_tone(&self, d: [f32; 3], x: usize, y: usize) -> [f32; 3] {
        let v = self.amount * self.weight(x, y);
        if v == 0.0 {
            return d;
        }
        match self.style {
            VignetteStyle::HighlightPriority => d,
            VignetteStyle::ColorPriority => {
                let hi = d[0].max(d[1]).max(d[2]);
                let v = self.protect(v, hi.sqrt());
                let g = fast_exp2(VIGNETTE_STOPS * v);
                // One gain for all channels keeps the hue; past white, scale back down
                // rather than clip channels apart.
                let top = (hi * g).max(1.0);
                d.map(|c| c * g / top)
            }
            VignetteStyle::PaintOverlay => {
                // Mixed with black or white on perceptual values: flat, like paint.
                let target = if v > 0.0 { 1.0 } else { 0.0 };
                let k = v.abs();
                d.map(|c| {
                    let e = c.max(0.0).sqrt();
                    let e = e + k * (target - e);
                    e * e
                })
            }
        }
    }
}

/// Film-like grain, ready to render an image at `scale` rendered pixels per full-size
/// pixel.
pub(super) struct Grain {
    /// Stops of brightness change at full strength.
    amount: f32,
    /// Rendered pixels per noise cell (at least one).
    cell: f32,
    roughness: f32,
}

impl Grain {
    pub(super) fn new(e: &Effects, scale: f32) -> Option<Grain> {
        if e.grain_amount <= 0.0 {
            return None;
        }
        // Grain 1–5 full-size pixels across. Grains smaller than a rendered pixel
        // average out: the texture gets finer and weaker, as when viewing a scan smaller.
        let size = 1.0 + 4.0 * e.grain_size / 100.0;
        let rendered = size * scale.max(1e-3);
        Some(Grain {
            amount: 0.6 * e.grain_amount / 100.0 * rendered.min(1.0),
            cell: rendered.max(1.0),
            roughness: e.grain_roughness / 100.0,
        })
    }

    /// Value noise in −1..1 (about) at a point measured in noise cells.
    #[inline]
    fn noise(u: f32, v: f32, seed: u32) -> f32 {
        let (i, j) = (u.floor(), v.floor());
        let (fu, fv) = (u - i, v - j);
        let (fu, fv) = (fu * fu * (3.0 - 2.0 * fu), fv * fv * (3.0 - 2.0 * fv));
        let (i, j) = (i as i32 as u32, j as i32 as u32);
        let at = |a: u32, b: u32| hash(a, b, seed);
        let top = at(i, j) + fu * (at(i + 1, j) - at(i, j));
        let bottom = at(i, j + 1) + fu * (at(i + 1, j + 1) - at(i, j + 1));
        top + fv * (bottom - top)
    }

    /// Grain on linear RGB at (x, y): one brightness change for all channels (so any
    /// linear RGB space), strongest in the midtones.
    #[inline]
    pub(super) fn apply(&self, rgb: [f32; 3], x: usize, y: usize) -> [f32; 3] {
        let (u, v) = ((x as f32 + 0.5) / self.cell, (y as f32 + 0.5) / self.cell);
        let smooth = Self::noise(u, v, 0x9e37_79b9);
        // Roughness: more of a finer, irregular layer.
        let fine = Self::noise(u * 2.3 + 17.0, v * 2.3 + 5.0, 0x85eb_ca6b);
        let r = self.roughness;
        let n = (smooth * (1.0 - 0.5 * r) + fine * r) * 1.8 / (1.0 + 0.5 * r);
        let e = rgb[0].max(rgb[1]).max(rgb[2]).clamp(0.0, 1.0).sqrt();
        let mid = 1.0 - (2.0 * e - 1.0).powi(4);
        let g = fast_exp2(self.amount * n * mid);
        rgb.map(|c| c * g)
    }
}

/// A well-mixed hash of a lattice point → −1..1.
#[inline]
fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ seed;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h as f32 * (2.0 / u32::MAX as f32) - 1.0
}

/// Long side of the glow map, in cells (the glow is soft: a small map is enough, and the
/// same for a preview and a full-size export).
const GLOW_MAP: usize = 384;

/// Glow, ready to render: the light the photo's bright areas spread around them, from a
/// small, blurred map of the whole photo (so a region renders like the whole).
pub(super) struct Glow {
    w: usize,
    h: usize,
    factor: f32,
    /// Display-linear light per cell.
    light: Vec<[f32; 3]>,
    /// 0..1.5: how strongly it is added.
    amount: f32,
}

impl Glow {
    /// From `pixel(x, y)`, the developed photo (display-linear, before effects) at full
    /// resolution `width × height`, and the photo's luma weights.
    pub(super) fn new(
        e: &Effects,
        width: usize,
        height: usize,
        luma: [f32; 3],
        pixel: impl Fn(usize, usize) -> [f32; 3] + Sync,
    ) -> Option<Glow> {
        use rayon::prelude::*;
        if e.glow_amount <= 0.0 || width == 0 || height == 0 {
            return None;
        }
        let f = (width.max(height) as f32 / GLOW_MAP as f32).max(1.0).ceil() as usize;
        let (w, h) = (width.div_ceil(f), height.div_ceil(f));
        // Range: from only the brightest highlights (0) to most of the light tones (100).
        let t = 0.85 - 0.6 * e.glow_range.clamp(0.0, 100.0) / 100.0;
        let k = e.glow_warmth.clamp(-100.0, 100.0) / 100.0;
        let warmth = [1.0 + 0.35 * k, 1.0 + 0.05 * k, 1.0 - 0.35 * k];
        // A few samples per cell: what's bright there, by how far past the threshold.
        let step = (f / 2).max(1);
        let light: Vec<[f32; 3]> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let (cx, cy) = (i % w, i / w);
                let (mut sum, mut n) = ([0f32; 3], 0f32);
                for y in (cy * f..((cy + 1) * f).min(height)).step_by(step) {
                    for x in (cx * f..((cx + 1) * f).min(width)).step_by(step) {
                        let p = pixel(x, y);
                        let l = luma[0] * p[0] + luma[1] * p[1] + luma[2] * p[2];
                        let wgt = smoothstep(t, t + 0.25, l.sqrt());
                        for c in 0..3 {
                            sum[c] += p[c].min(4.0) * wgt * warmth[c];
                        }
                        n += 1.0;
                    }
                }
                sum.map(|s| s / n.max(1.0))
            })
            .collect();
        // Spread: blur radius from 0.5% to 8% of the long side (three box blurs ≈ Gaussian).
        let reach = 0.005 + 0.075 * e.glow_spread.clamp(0.0, 100.0) / 100.0;
        let r = ((reach * w.max(h) as f32 / 1.7).round() as usize).max(1);
        let mut planes: [Vec<f32>; 3] = std::array::from_fn(|c| light.iter().map(|p| p[c]).collect());
        for p in &mut planes {
            for _ in 0..3 {
                *p = super::local::box_blur_par(p, w, h, r);
            }
        }
        let light = (0..w * h).map(|i| [planes[0][i], planes[1][i], planes[2][i]]).collect();
        Some(Glow { w, h, factor: f as f32, light, amount: 1.5 * e.glow_amount.min(100.0) / 100.0 })
    }

    /// Pixel `d` (display-linear) at (x, y) with the glow screened over it.
    #[inline]
    pub(super) fn apply(&self, d: [f32; 3], x: usize, y: usize) -> [f32; 3] {
        let fx = ((x as f32 + 0.5) / self.factor - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = ((y as f32 + 0.5) / self.factor - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: usize, y: usize, c: usize| self.light[y * self.w + x][c];
        std::array::from_fn(|c| {
            let top = at(x0, y0, c) + tx * (at(x1, y0, c) - at(x0, y0, c));
            let bottom = at(x0, y1, c) + tx * (at(x1, y1, c) - at(x0, y1, c));
            let g = ((top + ty * (bottom - top)) * self.amount).clamp(0.0, 1.0);
            // Screen: lifts darker pixels most, never past white.
            let v = d[c].clamp(0.0, 1.0);
            v + g * (1.0 - v)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vignette_darkens_corners_not_the_centre() {
        let e = Effects { vignette_amount: -100.0, ..Effects::default() };
        let v = Vignette::new(&e, 300, 200).unwrap();
        assert!((v.gain(150, 100, 0.2) - 1.0).abs() < 1e-3, "centre untouched");
        assert!(v.gain(0, 0, 0.2) < 0.3, "corner darker");
        assert!(v.gain(0, 100, 0.2) < v.gain(75, 100, 0.2), "darker towards the edge");
        // Highlights protect bright pixels.
        let hl = Vignette::new(&Effects { vignette_highlights: 100.0, ..e.clone() }, 300, 200).unwrap();
        assert!(hl.gain(0, 0, 1.0) > 0.9);
        // Lightening, rounder, other styles.
        let light = Vignette::new(&Effects { vignette_amount: 100.0, ..e.clone() }, 300, 200).unwrap();
        assert!(light.gain(0, 0, 0.2) > 3.0);
        for style in VignetteStyle::ALL {
            let r = Effects { vignette_style: style, vignette_roundness: 100.0, ..e.clone() };
            let v = Vignette::new(&r, 300, 200).unwrap();
            let corner = if style == VignetteStyle::HighlightPriority {
                v.gain(0, 0, 0.2) * 0.5
            } else {
                v.after_tone([0.5; 3], 0, 0)[0]
            };
            assert!(corner < 0.3, "{style:?}: {corner}");
        }
        assert!(Vignette::new(&Effects::default(), 300, 200).is_none());
    }

    #[test]
    fn grain_varies_midtones_and_fades_when_small() {
        let e = Effects { grain_amount: 100.0, ..Effects::default() };
        let g = Grain::new(&e, 1.0).unwrap();
        let vals: Vec<f32> = (0..400).map(|i| g.apply([0.2; 3], i % 20, i / 20)[0]).collect();
        let spread = |v: &[f32]| {
            let m = v.iter().sum::<f32>() / v.len() as f32;
            (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
        };
        let s = spread(&vals);
        assert!(s > 0.01, "grain shows: {s}");
        // The same pixel gets the same grain (regions match whole renders).
        assert_eq!(g.apply([0.2; 3], 7, 9), g.apply([0.2; 3], 7, 9));
        // Black stays black; a small preview gets weaker grain.
        assert_eq!(g.apply([0.0; 3], 3, 3), [0.0; 3]);
        let small = Grain::new(&e, 0.1).unwrap();
        let sv: Vec<f32> = (0..400).map(|i| small.apply([0.2; 3], i % 20, i / 20)[0]).collect();
        assert!(spread(&sv) < s * 0.6);
        assert!(Grain::new(&Effects::default(), 1.0).is_none());
    }
}
