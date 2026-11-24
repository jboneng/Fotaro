//! A photo's Develop settings (LrC's Basic panel), stored as JSON in the catalog and XMP.

use serde::{Deserialize, Serialize};

use super::curve::ToneCurve;
use super::detail::{Detail, DetailParam};
use super::dust::DustLayer;
use super::effects::{EffectParam, Effects};
use super::grading::{ColorGrading, GradeParam, GradeWheel};
use super::heal::Spot;
use super::hsl::{COLORS, Hsl, HslGroup};
use super::lut::LutProfile;
use super::masks::{Component, LocalParam, Mask, Shape};
use super::transform::{Transform, TransformParam};

/// Bumped when the renderer changes how existing settings look.
pub const PROCESS_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Treatment {
    #[default]
    Color,
    BlackAndWhite,
}

/// The White Balance menu. RAW files with a colour matrix offer all of them; other
/// files only As Shot, Auto and Custom (their Temp/Tint are relative, −100..+100).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum WbPreset {
    #[default]
    AsShot,
    Auto,
    Daylight,
    Cloudy,
    Shade,
    Tungsten,
    Fluorescent,
    Flash,
    Custom,
}

impl WbPreset {
    pub const RAW: [WbPreset; 9] = [
        Self::AsShot,
        Self::Auto,
        Self::Daylight,
        Self::Cloudy,
        Self::Shade,
        Self::Tungsten,
        Self::Fluorescent,
        Self::Flash,
        Self::Custom,
    ];
    pub const RELATIVE: [WbPreset; 3] = [Self::AsShot, Self::Auto, Self::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Self::AsShot => "As Shot",
            Self::Auto => "Auto",
            Self::Daylight => "Daylight",
            Self::Cloudy => "Cloudy",
            Self::Shade => "Shade",
            Self::Tungsten => "Tungsten",
            Self::Fluorescent => "Fluorescent",
            Self::Flash => "Flash",
            Self::Custom => "Custom",
        }
    }

    /// The fixed Kelvin / tint of a light-source preset.
    pub fn kelvin(self) -> Option<(f32, f32)> {
        Some(match self {
            Self::Daylight => (5500.0, 10.0),
            Self::Cloudy => (6500.0, 10.0),
            Self::Shade => (7500.0, 10.0),
            Self::Tungsten => (2850.0, 0.0),
            Self::Fluorescent => (3800.0, 21.0),
            Self::Flash => (5500.0, 0.0),
            _ => return None,
        })
    }
}

/// Every slider is in LrC's units: Temp in Kelvin (or −100..+100 for non-RAW files),
/// Exposure in stops, the rest −100..+100.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevelopSettings {
    pub process_version: u32,
    pub treatment: Treatment,
    pub wb: WbPreset,
    /// Used when `wb` is Custom.
    pub temp: f32,
    pub tint: f32,
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    pub vibrance: f32,
    pub saturation: f32,
    /// Profile: a 3D LUT (creative look) after the base tone, with its amount.
    pub profile: Option<LutProfile>,
    /// The Tone Curve panel.
    pub curve: ToneCurve,
    /// The HSL / Color panel.
    pub hsl: Hsl,
    /// Black & White Mix (the HSL panel in Black & White), −100..+100 per colour range.
    pub bw_mix: [f32; 8],
    /// The Color Grading panel.
    pub grading: ColorGrading,
    /// The Detail panel (sharpening, noise reduction).
    pub detail: Detail,
    /// The Effects panel (post-crop vignetting, grain).
    pub effects: Effects,
    /// The Transform panel (perspective, rotation, scale…).
    pub transform: Transform,
    /// Masking: local adjustments, applied in order.
    pub masks: Vec<Mask>,
    /// The Remove tool's spots (Heal, Clone), applied first.
    pub spots: Vec<Spot>,
    /// Dust & Scratches (film): found defects filled, before the spots.
    pub dust: Option<DustLayer>,
}

impl Default for DevelopSettings {
    fn default() -> Self {
        DevelopSettings {
            process_version: PROCESS_VERSION,
            treatment: Treatment::Color,
            wb: WbPreset::AsShot,
            temp: 0.0,
            tint: 0.0,
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            profile: None,
            curve: ToneCurve::default(),
            hsl: Hsl::default(),
            bw_mix: [0.0; 8],
            grading: ColorGrading::default(),
            detail: Detail::default(),
            effects: Effects::default(),
            transform: Transform::default(),
            masks: Vec::new(),
            spots: Vec::new(),
            dust: None,
        }
    }
}

/// A Basic panel slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slider {
    Temp,
    Tint,
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Texture,
    Clarity,
    Dehaze,
    Vibrance,
    Saturation,
    /// Tone Curve (parametric) regions.
    CurveHighlights,
    CurveLights,
    CurveDarks,
    CurveShadows,
    /// HSL: a group's slider for a colour range (index into [`COLORS`]).
    Hsl(HslGroup, u8),
    /// B&W Mix for a colour range.
    BwMix(u8),
    /// Color Grading: a wheel's hue, saturation or luminance.
    Grade(GradeWheel, GradeParam),
    GradeBlending,
    GradeBalance,
    /// The Detail panel.
    Detail(DetailParam),
    /// The Effects panel.
    Effect(EffectParam),
    /// The Transform panel.
    Transform(TransformParam),
    /// A mask's local slider (mask index, slider).
    Local(u8, LocalParam),
    /// A mask's Amount.
    MaskAmount(u8),
    /// A radial gradient's or a depth range's Feather (mask index, component index).
    MaskFeather(u8, u8),
    /// A depth range's Near and Far.
    MaskDepthNear(u8, u8),
    MaskDepthFar(u8, u8),
    /// Crop & Straighten: the Angle.
    CropAngle,
}

impl Slider {
    pub const WB: [Slider; 2] = [Self::Temp, Self::Tint];
    pub const TONE: [Slider; 6] =
        [Self::Exposure, Self::Contrast, Self::Highlights, Self::Shadows, Self::Whites, Self::Blacks];
    pub const PRESENCE: [Slider; 5] =
        [Self::Texture, Self::Clarity, Self::Dehaze, Self::Vibrance, Self::Saturation];
    /// The Tone Curve panel's region sliders, in LrC's order.
    pub const CURVE: [Slider; 4] =
        [Self::CurveHighlights, Self::CurveLights, Self::CurveDarks, Self::CurveShadows];

    pub fn label(self) -> &'static str {
        match self {
            Self::Temp => "Temp",
            Self::Tint => "Tint",
            Self::Exposure => "Exposure",
            Self::Contrast => "Contrast",
            Self::Highlights => "Highlights",
            Self::Shadows => "Shadows",
            Self::Whites => "Whites",
            Self::Blacks => "Blacks",
            Self::Texture => "Texture",
            Self::Clarity => "Clarity",
            Self::Dehaze => "Dehaze",
            Self::Vibrance => "Vibrance",
            Self::Saturation => "Saturation",
            Self::CurveHighlights => "Highlights",
            Self::CurveLights => "Lights",
            Self::CurveDarks => "Darks",
            Self::CurveShadows => "Shadows",
            Self::Hsl(_, i) | Self::BwMix(i) => COLORS[usize::from(i) % 8],
            Self::Grade(_, p) => p.label(),
            Self::GradeBlending => "Blending",
            Self::GradeBalance => "Balance",
            Self::Detail(p) => p.label(),
            Self::Effect(p) => p.label(),
            Self::Transform(p) => p.label(),
            Self::Local(_, p) => p.label(),
            Self::MaskAmount(_) => "Amount",
            Self::MaskFeather(..) => "Feather",
            Self::MaskDepthNear(..) => "Near",
            Self::MaskDepthFar(..) => "Far",
            Self::CropAngle => "Angle",
        }
    }

    /// Slider range; `kelvin` is whether Temp/Tint are absolute (RAW with a colour matrix).
    pub fn range(self, kelvin: bool) -> (f32, f32) {
        match self {
            Self::Temp if kelvin => (2000.0, 50000.0),
            Self::Tint if kelvin => (-150.0, 150.0),
            Self::Exposure => (-5.0, 5.0),
            Self::Detail(p) => p.range(),
            Self::Effect(p) => p.range(),
            Self::Transform(p) => p.range(),
            Self::CropAngle => (-super::transform::MAX_CROP_ANGLE, super::transform::MAX_CROP_ANGLE),
            Self::Local(_, p) => p.range(),
            Self::MaskAmount(_)
            | Self::MaskFeather(..)
            | Self::MaskDepthNear(..)
            | Self::MaskDepthFar(..) => (0.0, 100.0),
            Self::Grade(_, GradeParam::Hue) => (0.0, 360.0),
            Self::Grade(_, GradeParam::Saturation) | Self::GradeBlending => (0.0, 100.0),
            _ => (-100.0, 100.0),
        }
    }

    pub fn decimals(self) -> usize {
        match self {
            Self::Exposure => 2,
            Self::Detail(DetailParam::SharpenRadius) => 1,
            Self::Transform(p) => p.decimals(),
            Self::CropAngle => 2,
            Self::Local(_, p) => p.decimals(),
            _ => 0,
        }
    }

    pub fn get(self, s: &DevelopSettings) -> f32 {
        match self {
            Self::Temp => s.temp,
            Self::Tint => s.tint,
            Self::Exposure => s.exposure,
            Self::Contrast => s.contrast,
            Self::Highlights => s.highlights,
            Self::Shadows => s.shadows,
            Self::Whites => s.whites,
            Self::Blacks => s.blacks,
            Self::Texture => s.texture,
            Self::Clarity => s.clarity,
            Self::Dehaze => s.dehaze,
            Self::Vibrance => s.vibrance,
            Self::Saturation => s.saturation,
            Self::CurveHighlights => s.curve.highlights,
            Self::CurveLights => s.curve.lights,
            Self::CurveDarks => s.curve.darks,
            Self::CurveShadows => s.curve.shadows,
            Self::Hsl(g, i) => s.hsl.get(g)[usize::from(i) % 8],
            Self::BwMix(i) => s.bw_mix[usize::from(i) % 8],
            Self::Grade(w, p) => s.grading.wheel(w).get(p),
            Self::GradeBlending => s.grading.blending,
            Self::GradeBalance => s.grading.balance,
            Self::Detail(p) => p.get(&s.detail),
            Self::Effect(p) => p.get(&s.effects),
            Self::Transform(p) => p.get(&s.transform),
            Self::CropAngle => s.transform.crop.angle,
            Self::Local(i, p) => s.masks.get(usize::from(i)).map_or(0.0, |m| p.get(&m.adjust)),
            Self::MaskAmount(i) => s.masks.get(usize::from(i)).map_or(0.0, |m| m.amount),
            Self::MaskFeather(i, j) => {
                match s.masks.get(usize::from(i)).and_then(|m| m.components.get(usize::from(j))) {
                    Some(Component { shape: Shape::Radial { feather, .. }, .. }) => *feather,
                    Some(Component { shape: Shape::Raster { range: Some(r), .. }, .. }) => r.feather,
                    _ => 0.0,
                }
            }
            Self::MaskDepthNear(i, j) | Self::MaskDepthFar(i, j) => {
                match s.masks.get(usize::from(i)).and_then(|m| m.components.get(usize::from(j))) {
                    Some(Component { shape: Shape::Raster { range: Some(r), .. }, .. }) => {
                        if matches!(self, Self::MaskDepthNear(..)) { r.near } else { r.far }
                    }
                    _ => 0.0,
                }
            }
        }
    }

    /// Set the value; moving Temp or Tint makes the white balance Custom.
    pub fn set(self, s: &mut DevelopSettings, v: f32) {
        // Mask sliders: nothing to set if the mask is gone.
        match self {
            Self::Local(i, p) => {
                if let Some(m) = s.masks.get_mut(usize::from(i)) {
                    *p.get_mut(&mut m.adjust) = v;
                }
                return;
            }
            Self::MaskAmount(i) => {
                if let Some(m) = s.masks.get_mut(usize::from(i)) {
                    m.amount = v;
                }
                return;
            }
            Self::MaskFeather(i, j) => {
                let c = s.masks.get_mut(usize::from(i)).and_then(|m| m.components.get_mut(usize::from(j)));
                match c {
                    Some(Component { shape: Shape::Radial { feather, .. }, .. }) => *feather = v,
                    Some(Component { shape: Shape::Raster { range: Some(r), .. }, .. }) => r.feather = v,
                    _ => {}
                }
                return;
            }
            Self::MaskDepthNear(i, j) | Self::MaskDepthFar(i, j) => {
                let c = s.masks.get_mut(usize::from(i)).and_then(|m| m.components.get_mut(usize::from(j)));
                if let Some(Component { shape: Shape::Raster { range: Some(r), .. }, .. }) = c {
                    // Near stays in front of Far: the one moved pushes the other along.
                    if matches!(self, Self::MaskDepthNear(..)) {
                        r.near = v;
                        r.far = r.far.max(v);
                    } else {
                        r.far = v;
                        r.near = r.near.min(v);
                    }
                }
                return;
            }
            _ => {}
        }
        let field = match self {
            Self::Temp | Self::Tint => {
                s.wb = WbPreset::Custom;
                if self == Self::Temp { &mut s.temp } else { &mut s.tint }
            }
            Self::Exposure => &mut s.exposure,
            Self::Contrast => &mut s.contrast,
            Self::Highlights => &mut s.highlights,
            Self::Shadows => &mut s.shadows,
            Self::Whites => &mut s.whites,
            Self::Blacks => &mut s.blacks,
            Self::Texture => &mut s.texture,
            Self::Clarity => &mut s.clarity,
            Self::Dehaze => &mut s.dehaze,
            Self::Vibrance => &mut s.vibrance,
            Self::Saturation => &mut s.saturation,
            Self::CurveHighlights => &mut s.curve.highlights,
            Self::CurveLights => &mut s.curve.lights,
            Self::CurveDarks => &mut s.curve.darks,
            Self::CurveShadows => &mut s.curve.shadows,
            Self::Hsl(g, i) => &mut s.hsl.get_mut(g)[usize::from(i) % 8],
            Self::BwMix(i) => &mut s.bw_mix[usize::from(i) % 8],
            Self::Grade(w, p) => s.grading.wheel_mut(w).get_mut(p),
            Self::GradeBlending => &mut s.grading.blending,
            Self::GradeBalance => &mut s.grading.balance,
            Self::Detail(p) => p.get_mut(&mut s.detail),
            Self::Effect(p) => p.get_mut(&mut s.effects),
            Self::Transform(p) => p.get_mut(&mut s.transform),
            Self::CropAngle => &mut s.transform.crop.angle,
            Self::Local(..)
            | Self::MaskAmount(_)
            | Self::MaskFeather(..)
            | Self::MaskDepthNear(..)
            | Self::MaskDepthFar(..) => unreachable!("handled above"),
        };
        *field = v;
    }

    /// The value as the panel shows it ("+0.35", "-12", "5500").
    pub fn format(self, v: f32, kelvin: bool) -> String {
        match self {
            Self::Temp if kelvin => format!("{v:.0}"),
            // Angles and amounts that are never negative show without a sign, as in LrC.
            Self::Grade(_, GradeParam::Hue | GradeParam::Saturation) | Self::GradeBlending => {
                format!("{v:.0}")
            }
            Self::Detail(_) => format!("{v:.*}", self.decimals()),
            Self::Effect(p) if !p.signed() => format!("{v:.0}"),
            Self::Transform(TransformParam::Scale)
            | Self::MaskAmount(_)
            | Self::MaskFeather(..)
            | Self::MaskDepthNear(..)
            | Self::MaskDepthFar(..) => {
                format!("{v:.0}")
            }
            _ if v.abs() < 0.5 * 10f32.powi(-(self.decimals() as i32)) => {
                format!("{:.*}", self.decimals(), 0.0)
            }
            _ => format!("{v:+.*}", self.decimals()),
        }
    }
}

impl DevelopSettings {
    /// True when nothing would change the photo (Temp/Tint only count when Custom).
    pub fn is_default(&self) -> bool {
        self.is_default_for(false)
    }

    /// A new photo's settings: LrC gives RAW files some sharpening and colour noise
    /// reduction; rendered files (JPEG, TIFF…) none.
    pub fn default_for(raw: bool) -> Self {
        let detail = if raw { Detail::raw() } else { Detail::default() };
        DevelopSettings { detail, ..Self::default() }
    }

    /// True when nothing differs from [`Self::default_for`] (Temp/Tint only count when
    /// Custom).
    pub fn is_default_for(&self, raw: bool) -> bool {
        let mut s = self.clone();
        if s.wb != WbPreset::Custom {
            (s.temp, s.tint) = (0.0, 0.0);
        }
        s.process_version = PROCESS_VERSION;
        s == Self::default_for(raw)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("settings serialize")
    }

    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }

    /// The photo's depth map, if one was made: Fog's and Lens Blur's, or a Depth Range
    /// mask's (its picture's id in the mask store).
    pub fn depth_map(&self) -> Option<&str> {
        self.effects.fog_depth.as_deref().or_else(|| {
            self.masks.iter().flat_map(|m| &m.components).find_map(|c| match &c.shape {
                super::masks::Shape::Raster { id, range: Some(_), .. } => Some(id.as_str()),
                _ => None,
            })
        })
    }

    /// A hash that stays the same across builds (for preview cache keys).
    pub fn stable_hash(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |v: u32| {
            for b in v.to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        eat(self.process_version);
        eat(self.treatment as u32);
        eat(self.wb as u32);
        for v in [
            self.temp,
            self.tint,
            self.exposure,
            self.contrast,
            self.highlights,
            self.shadows,
            self.whites,
            self.blacks,
            self.texture,
            self.clarity,
            self.dehaze,
            self.vibrance,
            self.saturation,
        ] {
            eat(v.to_bits());
        }
        // The Tone Curve only when it does something, so photos edited before it existed
        // keep their preview keys.
        let c = &self.curve;
        if !c.is_default() {
            for v in [c.highlights, c.lights, c.darks, c.shadows].iter().chain(&c.splits) {
                eat(v.to_bits());
            }
            for points in [&c.rgb, &c.red, &c.green, &c.blue] {
                eat(points.len() as u32);
                for p in points.iter().flatten() {
                    eat(p.to_bits());
                }
            }
        }
        // Likewise HSL and B&W Mix.
        if !self.hsl.is_default() {
            for v in self.hsl.hue.iter().chain(&self.hsl.saturation).chain(&self.hsl.luminance) {
                eat(v.to_bits());
            }
        }
        if self.detail != Detail::default() {
            eat(0xde_7a11);
            for p in DetailParam::SHARPENING.into_iter().chain(DetailParam::NOISE) {
                eat(p.get(&self.detail).to_bits());
            }
        }
        if let Some(d) = &self.dust {
            eat(0xd5_57);
            for b in serde_json::to_vec(d).unwrap_or_default() {
                eat(u32::from(b));
            }
        }
        if !self.spots.is_empty() {
            eat(0x5b_07);
            for b in serde_json::to_vec(&self.spots).unwrap_or_default() {
                eat(u32::from(b));
            }
        }
        if !self.masks.is_empty() {
            eat(0x3a_5c);
            for b in serde_json::to_vec(&self.masks).unwrap_or_default() {
                eat(u32::from(b));
            }
        }
        if self.transform != Transform::default() {
            let t = &self.transform;
            eat(0x7a_f0);
            for p in TransformParam::ALL {
                eat(p.get(t).to_bits());
            }
            eat(t.upright as u32);
            for a in t.upright_angles {
                eat(a.to_bits());
            }
            eat(u32::from(t.constrain_crop));
            let c = &t.crop;
            for v in [c.left, c.top, c.right, c.bottom, c.angle] {
                eat(v.to_bits());
            }
            for v in t.guides.iter().flatten() {
                eat(v.to_bits());
            }
            if t.lens != super::lens::LensCorrections::default() {
                eat(0x1e_25);
                for b in serde_json::to_vec(&t.lens).unwrap_or_default() {
                    eat(u32::from(b));
                }
            }
        }
        if self.effects != Effects::default() {
            eat(0xeff_ec75);
            eat(self.effects.vignette_style as u32);
            for p in EffectParam::VIGNETTE
                .into_iter()
                .chain(EffectParam::GRAIN)
                .chain(EffectParam::GLOW)
                .chain(EffectParam::FOG)
                .chain(EffectParam::BLUR)
            {
                eat(p.get(&self.effects).to_bits());
            }
            for b in self.effects.fog_depth.iter().flat_map(|id| id.bytes()) {
                eat(u32::from(b));
            }
            eat(u32::from(self.effects.fog_show));
        }
        if self.grading != ColorGrading::default() {
            eat(0x9_ade);
            let g = &self.grading;
            for w in [g.shadows, g.midtones, g.highlights, g.global] {
                for v in [w.hue, w.saturation, w.luminance] {
                    eat(v.to_bits());
                }
            }
            eat(g.blending.to_bits());
            eat(g.balance.to_bits());
        }
        if let Some(p) = &self.profile {
            eat(0x9f_0f11e);
            for b in p.hash.bytes() {
                eat(u32::from(b));
            }
            eat(p.amount.to_bits());
        }
        if self.bw_mix.iter().any(|v| *v != 0.0) {
            eat(0xb_a1);
            for v in &self.bw_mix {
                eat(v.to_bits());
            }
        }
        h
    }

    /// A History label for a Lens Corrections change.
    fn describe_lens(l: &super::lens::LensCorrections, o: &super::lens::LensCorrections) -> String {
        let on = |b: bool| if b { "On" } else { "Off" };
        if l.profile_enable != o.profile_enable {
            return format!("Lens Profile Corrections: {}", on(l.profile_enable));
        }
        if l.profile != o.profile {
            return match &l.profile {
                Some(p) if o.profile.as_ref().is_none_or(|q| q.lens != p.lens) => {
                    format!("Lens Profile: {}", p.lens)
                }
                _ => "Lens Profile".into(),
            };
        }
        if l.remove_ca != o.remove_ca {
            return format!("Remove Chromatic Aberration: {}", on(l.remove_ca));
        }
        let pairs: [(&str, f32, f32); 8] = [
            ("Lens Profile Distortion", l.distortion_amount, o.distortion_amount),
            ("Lens Profile Vignetting", l.vignetting_amount, o.vignetting_amount),
            ("Lens Distortion", l.distortion, o.distortion),
            ("Lens Vignetting", l.vignette_amount, o.vignette_amount),
            ("Lens Vignetting Midpoint", l.vignette_midpoint, o.vignette_midpoint),
            ("Defringe Purple Amount", l.purple_amount, o.purple_amount),
            ("Defringe Green Amount", l.green_amount, o.green_amount),
            ("", 0.0, 0.0),
        ];
        for (name, a, b) in pairs {
            if a != b && !name.is_empty() {
                return format!("{name} {a:+.0}");
            }
        }
        if l.purple_hue != o.purple_hue {
            return "Defringe Purple Hue".into();
        }
        if l.green_hue != o.green_hue {
            return "Defringe Green Hue".into();
        }
        "Lens Corrections".into()
    }

    /// A History panel label for the change from `old` to `self`, LrC style
    /// ("Exposure +0.35", "White Balance: Tungsten", "Treatment: Black & White").
    pub fn describe_change(&self, old: &DevelopSettings, kelvin: bool) -> String {
        if self.treatment != old.treatment {
            return match self.treatment {
                Treatment::Color => "Treatment: Color".into(),
                Treatment::BlackAndWhite => "Treatment: Black & White".into(),
            };
        }
        if self.profile != old.profile {
            return match (&self.profile, &old.profile) {
                (None, _) => "Profile: None".into(),
                (Some(p), Some(o)) if p.hash == o.hash => format!("Profile Amount: {:.0}", p.amount),
                (Some(p), _) => format!("Profile: {}", p.name),
            };
        }
        if self.curve != old.curve {
            let (c, o) = (&self.curve, &old.curve);
            let regions: Vec<Slider> =
                Slider::CURVE.into_iter().filter(|s| s.get(self) != s.get(old)).collect();
            return match regions.as_slice() {
                [s] => format!("Tone Curve: {} {}", s.label(), s.format(s.get(self) - s.get(old), kelvin)),
                [] if c.splits != o.splits => "Tone Curve: Split Points".into(),
                [] => "Tone Curve: Point Curve".into(),
                _ => "Tone Curve".into(),
            };
        }
        if self.detail != old.detail {
            let changed: Vec<DetailParam> = DetailParam::SHARPENING
                .into_iter()
                .chain(DetailParam::NOISE)
                .filter(|p| p.get(&self.detail) != p.get(&old.detail))
                .collect();
            return match changed.as_slice() {
                [p] => {
                    let s = Slider::Detail(*p);
                    format!("{}: {} {}", p.group(), p.label(), s.format(s.get(self), kelvin))
                }
                _ => "Detail".into(),
            };
        }
        if self.spots != old.spots {
            return describe_spots(&self.spots, &old.spots);
        }
        if self.dust != old.dust {
            return match (&self.dust, &old.dust) {
                (Some(d), _) if !d.enabled => "Dust & Scratches: Off".into(),
                (None, _) => "Dust & Scratches: Off".into(),
                _ => "Dust & Scratches".into(),
            };
        }
        if self.masks != old.masks {
            return describe_masks(&self.masks, &old.masks, kelvin);
        }
        if self.transform.lens != old.transform.lens {
            return Self::describe_lens(&self.transform.lens, &old.transform.lens);
        }
        if self.transform != old.transform {
            let (t, o) = (&self.transform, &old.transform);
            if t.upright != o.upright {
                return format!("Upright: {}", t.upright.label());
            }
            if t.guides != o.guides {
                return "Upright: Guided".into();
            }
            if t.crop != o.crop {
                return if (t.crop.left, t.crop.top, t.crop.right, t.crop.bottom)
                    == (o.crop.left, o.crop.top, o.crop.right, o.crop.bottom)
                {
                    format!("Crop Angle {:.2}", t.crop.angle)
                } else if t.crop.is_full() {
                    "Reset Crop".into()
                } else {
                    "Crop".into()
                };
            }
            if t.constrain_crop != o.constrain_crop {
                return format!("Constrain Crop: {}", if t.constrain_crop { "On" } else { "Off" });
            }
            if t.upright_angles != o.upright_angles {
                return format!("Upright: {}", t.upright.label());
            }
            let changed: Vec<TransformParam> = TransformParam::ALL
                .into_iter()
                .filter(|p| p.get(&self.transform) != p.get(&old.transform))
                .collect();
            return match changed.as_slice() {
                [p] => {
                    let s = Slider::Transform(*p);
                    format!("Transform: {} {}", p.label(), s.format(s.get(self), kelvin))
                }
                _ => "Transform".into(),
            };
        }
        if self.effects != old.effects {
            let (e, o) = (&self.effects, &old.effects);
            if e.vignette_style != o.vignette_style {
                return format!("Post-Crop Vignette: {}", e.vignette_style.label());
            }
            let changed: Vec<EffectParam> = EffectParam::VIGNETTE
                .into_iter()
                .chain(EffectParam::GRAIN)
                .chain(EffectParam::GLOW)
                .chain(EffectParam::FOG)
                .chain(EffectParam::BLUR)
                .filter(|p| p.get(e) != p.get(o))
                .collect();
            return match changed.as_slice() {
                [p] => {
                    let s = Slider::Effect(*p);
                    format!("{}: {} {}", p.group(), p.label(), s.format(s.get(self), kelvin))
                }
                _ => "Effects".into(),
            };
        }
        if self.grading != old.grading {
            let all = GradeWheel::ALL
                .into_iter()
                .flat_map(|w| {
                    [GradeParam::Hue, GradeParam::Saturation, GradeParam::Luminance]
                        .map(|p| Slider::Grade(w, p))
                })
                .chain([Slider::GradeBlending, Slider::GradeBalance]);
            let changed: Vec<Slider> = all.filter(|s| s.get(self) != s.get(old)).collect();
            return match changed.as_slice() {
                [s @ Slider::Grade(w, GradeParam::Luminance)] => {
                    format!(
                        "Color Grading: {} Luminance {}",
                        w.label(),
                        s.format(s.get(self) - s.get(old), kelvin)
                    )
                }
                [s @ (Slider::GradeBlending | Slider::GradeBalance)] => {
                    format!("Color Grading: {} {}", s.label(), s.format(s.get(self), kelvin))
                }
                // A wheel's hue and saturation (dragging its handle): "Color Grading: Shadows".
                [Slider::Grade(w, _), rest @ ..]
                    if rest.iter().all(|s| matches!(s, Slider::Grade(v, _) if v == w)) =>
                {
                    format!("Color Grading: {}", w.label())
                }
                _ => "Color Grading".into(),
            };
        }
        if self.hsl != old.hsl || self.bw_mix != old.bw_mix {
            let all = HslGroup::ALL
                .into_iter()
                .flat_map(|g| (0..8).map(move |i| Slider::Hsl(g, i)))
                .chain((0..8).map(Slider::BwMix));
            let changed: Vec<Slider> = all.filter(|s| s.get(self) != s.get(old)).collect();
            return match changed.as_slice() {
                [s @ Slider::Hsl(g, _)] => {
                    format!("{}: {} {}", g.label(), s.label(), s.format(s.get(self) - s.get(old), kelvin))
                }
                [s] => format!("B&W Mix: {} {}", s.label(), s.format(s.get(self) - s.get(old), kelvin)),
                _ if self.hsl != old.hsl => "HSL".into(),
                _ => "B&W Mix".into(),
            };
        }
        let changed: Vec<Slider> = Slider::WB
            .iter()
            .chain(&Slider::TONE)
            .chain(&Slider::PRESENCE)
            .copied()
            .filter(|s| s.get(self) != s.get(old))
            .collect();
        if self.wb != old.wb && (self.wb != WbPreset::Custom || changed.is_empty()) {
            return format!("White Balance: {}", self.wb.label());
        }
        match changed.as_slice() {
            [] => "Settings".into(),
            [s] if matches!(s, Slider::Temp | Slider::Tint) && kelvin => {
                format!("{} {}", s.label(), s.format(s.get(self), kelvin))
            }
            [s] => format!("{} {}", s.label(), s.format(s.get(self) - s.get(old), kelvin)),
            _ if changed.iter().all(|s| Slider::TONE.contains(s)) => "Auto Settings".into(),
            _ => "Settings".into(),
        }
    }
}

/// A History label for a change to the masks, LrC style ("Add Brush Mask", "Brush Stroke",
/// "Mask 1: Exposure +0.50", "Subtract Radial Gradient").
fn describe_masks(new: &[Mask], old: &[Mask], kelvin: bool) -> String {
    if new.len() > old.len() {
        let kind = new.last().and_then(|m| m.components.first()).map_or("", |c| c.shape.label());
        return format!("Add {kind} Mask");
    }
    if new.len() < old.len() {
        let gone = old.iter().find(|m| !new.contains(m)).map_or("Mask", |m| m.name.as_str());
        return format!("Delete {gone}");
    }
    let Some(i) = (0..new.len()).find(|&i| new[i] != old[i]) else { return "Masks".into() };
    let (m, o) = (&new[i], &old[i]);
    let name = &m.name;
    if m.components.len() > o.components.len() {
        let c = m.components.last().expect("a component was added");
        return format!("{} {}", c.mode.label(), c.shape.label());
    }
    if m.components.len() < o.components.len() {
        let gone = o.components.iter().find(|c| !m.components.contains(c));
        return format!("Delete {}", gone.map_or("Component", |c| c.shape.label()));
    }
    if let Some((c, p)) = m.components.iter().zip(&o.components).find(|(c, p)| c != p) {
        return match (&c.shape, &p.shape) {
            _ if c.invert != p.invert => format!("Invert {}", c.shape.label()),
            _ if c.mode != p.mode => format!("{} {}", c.mode.label(), c.shape.label()),
            (Shape::Brush { .. }, _) => "Brush Stroke".into(),
            _ => format!("Edit {}", c.shape.label()),
        };
    }
    if m.visible != o.visible {
        return format!("{} {name}", if m.visible { "Show" } else { "Hide" });
    }
    if m.name != o.name {
        return format!("Rename {}", o.name);
    }
    if m.amount != o.amount {
        return format!("{name}: Amount {:.0}", m.amount);
    }
    if m.adjust.color != o.adjust.color {
        return format!("{name}: Color");
    }
    let changed: Vec<LocalParam> =
        LocalParam::all().filter(|p| p.get(&m.adjust) != p.get(&o.adjust)).collect();
    match changed.as_slice() {
        [p] => {
            let s = Slider::Local(i as u8, *p);
            format!("{name}: {} {}", p.label(), s.format(p.get(&m.adjust), kelvin))
        }
        _ => format!("{name}: Settings"),
    }
}

/// A History label for a change to the Remove tool's spots, LrC style.
fn describe_spots(new: &[Spot], old: &[Spot]) -> String {
    match new.len().cmp(&old.len()) {
        std::cmp::Ordering::Greater => {
            let mode = new.last().map_or("Spot", |s| s.mode.label());
            format!("Remove: {mode}")
        }
        std::cmp::Ordering::Less if new.is_empty() => "Reset Spots".into(),
        std::cmp::Ordering::Less => "Delete Spot".into(),
        std::cmp::Ordering::Equal => "Edit Spot".into(),
    }
}
