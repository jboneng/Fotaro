//! A photo's Develop settings (LrC's Basic panel), stored as JSON in the catalog and XMP.

use serde::{Deserialize, Serialize};

use super::curve::ToneCurve;

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
    /// The Tone Curve panel.
    pub curve: ToneCurve,
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
            curve: ToneCurve::default(),
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
        }
    }

    /// Slider range; `kelvin` is whether Temp/Tint are absolute (RAW with a colour matrix).
    pub fn range(self, kelvin: bool) -> (f32, f32) {
        match self {
            Self::Temp if kelvin => (2000.0, 50000.0),
            Self::Tint if kelvin => (-150.0, 150.0),
            Self::Exposure => (-5.0, 5.0),
            _ => (-100.0, 100.0),
        }
    }

    pub fn decimals(self) -> usize {
        if self == Self::Exposure { 2 } else { 0 }
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
        }
    }

    /// Set the value; moving Temp or Tint makes the white balance Custom.
    pub fn set(self, s: &mut DevelopSettings, v: f32) {
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
        };
        *field = v;
    }

    /// The value as the panel shows it ("+0.35", "-12", "5500").
    pub fn format(self, v: f32, kelvin: bool) -> String {
        match self {
            Self::Temp if kelvin => format!("{v:.0}"),
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
        let mut s = self.clone();
        if s.wb != WbPreset::Custom {
            (s.temp, s.tint) = (0.0, 0.0);
        }
        s.process_version = PROCESS_VERSION;
        s == Self::default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("settings serialize")
    }

    pub fn from_json(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
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
        h
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
