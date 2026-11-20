//! Lightroom develop presets: their settings (Camera Raw names, `crs:` in `.xmp` presets,
//! the `settings` table of old `.lrtemplate` ones) applied to our sliders.
//!
//! Our sliders use Lightroom's units and ranges, so values carry over one for one; the
//! look is close, not Adobe's (our renderer is our own). A preset sets only the settings
//! it has, as in Lightroom. What we have no slider for (camera and creative profiles, lens
//! corrections, calibration, local adjustments, the 2010 process sliders) is reported.

use serde::{Deserialize, Serialize};

use super::effects::VignetteStyle;
use super::groups::SettingsGroup;
use super::settings::{DevelopSettings, Treatment, WbPreset};

/// A setting's value as a preset has it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LrValue {
    Bool(bool),
    Num(f64),
    Str(String),
    List(Vec<LrValue>),
    Table(Vec<(String, LrValue)>),
}

impl LrValue {
    fn num(&self) -> Option<f32> {
        match self {
            LrValue::Num(n) => Some(*n as f32),
            LrValue::Str(s) => s.trim().trim_start_matches('+').parse().ok(),
            LrValue::Bool(b) => Some(f32::from(u8::from(*b))),
            _ => None,
        }
    }

    fn truth(&self) -> Option<bool> {
        match self {
            LrValue::Bool(b) => Some(*b),
            LrValue::Str(s) => match s.to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            LrValue::Num(n) => Some(*n != 0.0),
            _ => None,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            LrValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Curve points: `"x, y"` strings (`.xmp`) or a flat number list (`.lrtemplate`),
    /// 0..255 → 0..1. A straight line is no curve.
    fn curve(&self) -> Option<Vec<[f32; 2]>> {
        let LrValue::List(items) = self else { return None };
        let nums: Vec<f32> = items
            .iter()
            .flat_map(|v| match v {
                LrValue::Str(s) => s.split(',').filter_map(|p| p.trim().parse().ok()).collect(),
                v => v.num().into_iter().collect::<Vec<f32>>(),
            })
            .collect();
        if nums.len() < 4 || !nums.len().is_multiple_of(2) {
            return None;
        }
        let mut pts: Vec<[f32; 2]> =
            nums.chunks(2).map(|p| [(p[0] / 255.0).clamp(0.0, 1.0), (p[1] / 255.0).clamp(0.0, 1.0)]).collect();
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        let straight = pts.iter().all(|p| (p[0] - p[1]).abs() < 1e-6);
        Some(if straight { Vec::new() } else { pts })
    }
}

/// A Lightroom preset read from its file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LrPreset {
    pub name: String,
    /// Its group (folder) in Lightroom, when the file says.
    pub group: Option<String>,
    pub values: Vec<(String, LrValue)>,
}

/// An `.xmp` preset's `crs:` properties (as [`xmp::XmpDoc::properties`] reads them):
/// single values become numbers, booleans or text; lists stay lists.
pub fn lr_values_from_xmp(props: Vec<(String, Vec<String>)>) -> LrPreset {
    let mut name = None;
    let mut group = None;
    let mut values = Vec::new();
    for (k, v) in props {
        match k.as_str() {
            "Name" => name = v.into_iter().next(),
            "Group" => group = v.into_iter().next(),
            // Lists: curves (and structured values, reported as unsupported).
            _ if v.len() != 1 || k.starts_with("ToneCurve") || k == "Look" => {
                values.push((k, LrValue::List(v.into_iter().map(LrValue::Str).collect())))
            }
            _ => {
                let s = v.into_iter().next().unwrap_or_default();
                let val = match s.to_ascii_lowercase().as_str() {
                    "true" => LrValue::Bool(true),
                    "false" => LrValue::Bool(false),
                    _ => match s.trim().trim_start_matches('+').parse::<f64>() {
                        Ok(n) => LrValue::Num(n),
                        Err(_) => LrValue::Str(s),
                    },
                };
                values.push((k, val));
            }
        }
    }
    LrPreset { name: name.unwrap_or_default(), group: group.filter(|g| !g.is_empty()), values }
}

// --- .lrtemplate (Lua) ---------------------------------------------------------------

/// Read an old `.lrtemplate` preset: `s = { title = …, value = { settings = { … } } }`.
pub fn read_lrtemplate(text: &str) -> Result<LrPreset, String> {
    let mut p = Lua { s: text.as_bytes(), i: 0 };
    p.space();
    // `s = { … }`, or a bare table.
    if p.peek() != Some(b'{') {
        p.word().ok_or("not a Lightroom template")?;
        p.space();
        p.expect(b'=')?;
    }
    let LrValue::Table(top) = p.value()? else { return Err("not a Lightroom template".into()) };
    let field = |t: &[(String, LrValue)], k: &str| t.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let name = field(&top, "title").or_else(|| field(&top, "internalName"));
    let name = name.as_ref().and_then(LrValue::text).map(zstr).unwrap_or_default();
    let Some(LrValue::Table(value)) = field(&top, "value") else { return Err("no settings in the template".into()) };
    let Some(LrValue::Table(settings)) = field(&value, "settings") else {
        return Err("not a develop preset (no settings)".into());
    };
    Ok(LrPreset { name, group: None, values: settings })
}

/// A localisable string (`$$$/Path=Text`) → its text.
fn zstr(s: &str) -> String {
    match s.strip_prefix("$$$/") {
        Some(rest) => rest.split_once('=').map_or(rest, |(_, t)| t).to_string(),
        None => s.to_string(),
    }
}

struct Lua<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lua<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn space(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--") {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.i += 1;
                }
            } else {
                return;
            }
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        self.space();
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", c as char, self.i))
        }
    }

    fn word(&mut self) -> Option<String> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
            self.i += 1;
        }
        (self.i > start).then(|| String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
    }

    fn string(&mut self) -> Result<String, String> {
        let q = self.peek().ok_or("unexpected end")?;
        self.i += 1;
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                c if c == q => return Ok(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let e = self.peek().ok_or("unexpected end")?;
                    self.i += 1;
                    out.push(match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        e => e,
                    });
                }
                c => out.push(c),
            }
        }
        Err("unterminated string".into())
    }

    fn value(&mut self) -> Result<LrValue, String> {
        self.space();
        match self.peek().ok_or("unexpected end")? {
            b'{' => self.table(),
            b'"' | b'\'' => Ok(LrValue::Str(self.string()?)),
            c if c.is_ascii_digit() || c == b'-' || c == b'.' || c == b'+' => {
                let start = self.i;
                while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || b"+-.".contains(&c)) {
                    self.i += 1;
                }
                let t = String::from_utf8_lossy(&self.s[start..self.i]);
                t.parse().map(LrValue::Num).map_err(|_| format!("bad number {t}"))
            }
            _ => {
                let w = self.word().ok_or_else(|| format!("unexpected text at byte {}", self.i))?;
                match w.as_str() {
                    "true" => Ok(LrValue::Bool(true)),
                    "false" => Ok(LrValue::Bool(false)),
                    "nil" => Ok(LrValue::Str(String::new())),
                    // ZSTR "…" (a localisable string).
                    _ => {
                        self.space();
                        match self.peek() {
                            Some(b'"' | b'\'') => Ok(LrValue::Str(self.string()?)),
                            _ => Ok(LrValue::Str(w)),
                        }
                    }
                }
            }
        }
    }

    /// `{ key = value, ["key"] = value, value, … }`: keyed entries make a table, bare
    /// ones a list.
    fn table(&mut self) -> Result<LrValue, String> {
        self.expect(b'{')?;
        let (mut keyed, mut list) = (Vec::new(), Vec::new());
        loop {
            self.space();
            match self.peek() {
                Some(b'}') => {
                    self.i += 1;
                    break;
                }
                None => return Err("unterminated table".into()),
                _ => {}
            }
            let save = self.i;
            let key = if self.peek() == Some(b'[') {
                self.i += 1;
                self.space();
                let k = match self.value()? {
                    LrValue::Str(s) => s,
                    LrValue::Num(n) => n.to_string(),
                    _ => return Err("bad key".into()),
                };
                self.expect(b']')?;
                self.expect(b'=')?;
                Some(k)
            } else if let Some(w) = self.word() {
                self.space();
                if self.peek() == Some(b'=') {
                    self.i += 1;
                    Some(w)
                } else {
                    self.i = save;
                    None
                }
            } else {
                None
            };
            let v = self.value()?;
            match key {
                Some(k) => keyed.push((k, v)),
                None => list.push(v),
            }
            self.space();
            if matches!(self.peek(), Some(b',' | b';')) {
                self.i += 1;
            }
        }
        Ok(if keyed.is_empty() && !list.is_empty() { LrValue::List(list) } else { LrValue::Table(keyed) })
    }
}

// --- Applying -------------------------------------------------------------------------

/// What applying a preset changes and what it can't.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LrSummary {
    pub groups: Vec<SettingsGroup>,
    /// What we have no slider for, by kind ("Profile", "Lens Corrections"…).
    pub unsupported: Vec<String>,
}

/// Keys that describe the preset rather than set anything.
const ABOUT: &[&str] = &[
    "Version",
    "ProcessVersion",
    "CompatibleVersion",
    "PresetType",
    "Cluster",
    "UUID",
    "SupportsAmount",
    "SupportsAmount2",
    "SupportsColor",
    "SupportsMonochrome",
    "SupportsHighDynamicRange",
    "SupportsNormalDynamicRange",
    "SupportsSceneReferred",
    "SupportsOutputReferred",
    "RequiresRGBTables",
    "CameraModelRestriction",
    "Copyright",
    "ContactInfo",
    "Name",
    "ShortName",
    "SortName",
    "Group",
    "Description",
    "HasSettings",
    "HasCrop",
    "AlreadyApplied",
    "ToneCurveName",
    "ToneCurveName2012",
    "ToneCurveName2012Red",
    "ToneCurveName2012Green",
    "ToneCurveName2012Blue",
    "WhiteBalanceName",
    "OverrideLookVignette",
    "Amount",
    "uuid",
    "CropConstrainToWarp",
    "CropConstrainAspectRatio",
    "CropUnit",
    "CropWidth",
    "CropHeight",
    "UprightVersion",
    "UprightCenterMode",
    "UprightCenterNormX",
    "UprightCenterNormY",
    "UprightFocalMode",
    "UprightFocalLength35mm",
    "UprightPreview",
    "UprightTransformCount",
    "UprightDependentDigest",
    "UprightFourSegmentsCount",
    "UprightGuidedDependentDigest",
    "ShowInPresets",
    "Baseline",
    "ISODependent",
    // Glow's style options: our Glow has one style.
    "GlowStyle",
    "GlowDirection",
    "GlowDirectionalSoftness",
];

/// What kind of setting an unmapped key is.
fn kind(key: &str) -> String {
    let starts = |p: &[&str]| p.iter().any(|p| key.starts_with(p));
    if starts(&["CameraProfile", "Look", "ProfileName", "ConvertToGrayscaleProfile"]) {
        "Profile".into()
    } else if starts(&[
        "Lens",
        "AutoLateralCA",
        "Defringe",
        "VignetteAmount",
        "VignetteMidpoint",
        "ChromaticAberration",
        "PerspectiveUpright",
    ]) {
        "Lens Corrections".into()
    } else if starts(&["ShadowTint", "RedHue", "RedSaturation", "GreenHue", "GreenSaturation", "BlueHue", "BlueSaturation"]) {
        "Calibration".into()
    } else if starts(&["MaskGroup", "Gradient", "CircularGradient", "PaintBased", "RetouchInfo", "RedEye", "RetouchAreas"]) {
        "Local adjustments".into()
    } else if [
        "Exposure",
        "Contrast",
        "Brightness",
        "Shadows",
        "FillLight",
        "HighlightRecovery",
        "Clarity",
        "ToneCurve",
        "ToneCurveRed",
        "ToneCurveGreen",
        "ToneCurveBlue",
        "AutoBrightness",
        "AutoContrast",
        "AutoExposure",
        "AutoShadows",
    ]
    .contains(&key)
    {
        "Older (2010) process sliders".into()
    } else if starts(&["PointColors", "CurveRefineSaturation"]) {
        "Point Color".into()
    } else if starts(&["Auto"]) {
        "Auto settings".into()
    } else if starts(&["EnableMaskGroupBasedCorrections", "Enable"]) {
        String::new()
    } else {
        key.to_string()
    }
}

/// Lightroom's own default profiles: nothing lost without them (our base look is the
/// counterpart).
fn default_profile(key: &str, v: &LrValue) -> bool {
    let default = |s: &str| {
        let n = s.strip_prefix("Name=").unwrap_or(s);
        matches!(n, "Adobe Color" | "Adobe Standard" | "Embedded" | "Default Color" | "Default Profile" | "Default Monochrome" | "Adobe Monochrome")
    };
    match (key, v) {
        ("CameraProfile" | "ProfileName", LrValue::Str(s)) => default(s),
        ("Look", LrValue::List(items)) => {
            items.iter().any(|i| matches!(i, LrValue::Str(s) if s.starts_with("Name=") && default(s)))
        }
        _ => false,
    }
}

/// Is a profile (its name, or a Look's fields) a black & white one?
fn monochrome(v: &LrValue) -> bool {
    let named = |s: &str| {
        let n = s.strip_prefix("Name=").unwrap_or(s);
        n.contains("Monochrome") || n.starts_with("B&W") || n.starts_with("B&amp;W")
    };
    match v {
        LrValue::Str(s) => named(s),
        LrValue::List(items) => items.iter().any(|i| matches!(i, LrValue::Str(s) if s.starts_with("Name=") && named(s))),
        _ => false,
    }
}

const COLORS: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];

/// Apply a preset's values to `s` (a RAW photo's or not: White Balance differs), setting
/// only what it has. Returns what it set and what it couldn't.
pub fn apply_lightroom(values: &[(String, LrValue)], s: &mut DevelopSettings, raw: bool) -> LrSummary {
    let mut groups: Vec<SettingsGroup> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    let mut set = |g: SettingsGroup| {
        if !groups.contains(&g) {
            groups.push(g);
        }
    };
    let has_crop = values.iter().find(|(k, _)| k == "HasCrop").and_then(|(_, v)| v.truth()).unwrap_or(true);
    let mut wb_name: Option<WbPreset> = None;
    let mut mono = false;
    let mut temp: (Option<f32>, Option<f32>) = (None, None);
    for (k, v) in values {
        let n = v.num();
        let key = k.as_str();
        let mut done = true;
        // A slider: the value, into `field`, in `group`.
        macro_rules! slider {
            ($group:expr, $field:expr) => {{
                if let Some(n) = n {
                    $field = n;
                    set($group);
                }
            }};
        }
        use SettingsGroup as G;
        match key {
            "WhiteBalance" => {
                wb_name = v.text().and_then(|t| {
                    WbPreset::RAW.into_iter().find(|w| w.label().eq_ignore_ascii_case(t.trim()))
                });
            }
            "Temperature" if raw => temp.0 = n,
            "Tint" if raw => temp.1 = n,
            "IncrementalTemperature" if !raw => temp.0 = n,
            "IncrementalTint" if !raw => temp.1 = n,
            "Temperature" | "Tint" | "IncrementalTemperature" | "IncrementalTint" => {}
            "Exposure2012" => slider!(G::BasicTone, s.exposure),
            "Contrast2012" => slider!(G::BasicTone, s.contrast),
            "Highlights2012" => slider!(G::BasicTone, s.highlights),
            "Shadows2012" => slider!(G::BasicTone, s.shadows),
            "Whites2012" => slider!(G::BasicTone, s.whites),
            "Blacks2012" => slider!(G::BasicTone, s.blacks),
            "Texture" => slider!(G::Presence, s.texture),
            "Clarity2012" => slider!(G::Presence, s.clarity),
            "Dehaze" => slider!(G::Presence, s.dehaze),
            "Vibrance" => slider!(G::Presence, s.vibrance),
            "Saturation" => slider!(G::Presence, s.saturation),
            "ConvertToGrayscale" => {
                if let Some(b) = v.truth() {
                    s.treatment = if b { Treatment::BlackAndWhite } else { Treatment::Color };
                    set(G::Treatment);
                }
            }
            "ParametricShadows" => slider!(G::ToneCurve, s.curve.shadows),
            "ParametricDarks" => slider!(G::ToneCurve, s.curve.darks),
            "ParametricLights" => slider!(G::ToneCurve, s.curve.lights),
            "ParametricHighlights" => slider!(G::ToneCurve, s.curve.highlights),
            "ParametricShadowSplit" => slider!(G::ToneCurve, s.curve.splits[0]),
            "ParametricMidtoneSplit" => slider!(G::ToneCurve, s.curve.splits[1]),
            "ParametricHighlightSplit" => slider!(G::ToneCurve, s.curve.splits[2]),
            "ToneCurvePV2012" | "ToneCurvePV2012Red" | "ToneCurvePV2012Green" | "ToneCurvePV2012Blue" => {
                if let Some(points) = v.curve() {
                    let c = &mut s.curve;
                    *match key {
                        "ToneCurvePV2012" => &mut c.rgb,
                        "ToneCurvePV2012Red" => &mut c.red,
                        "ToneCurvePV2012Green" => &mut c.green,
                        _ => &mut c.blue,
                    } = points;
                    set(G::ToneCurve);
                }
            }
            "SplitToningShadowHue" => slider!(G::ColorGrading, s.grading.shadows.hue),
            "SplitToningShadowSaturation" => slider!(G::ColorGrading, s.grading.shadows.saturation),
            "SplitToningHighlightHue" => slider!(G::ColorGrading, s.grading.highlights.hue),
            "SplitToningHighlightSaturation" => slider!(G::ColorGrading, s.grading.highlights.saturation),
            "SplitToningBalance" => slider!(G::ColorGrading, s.grading.balance),
            "ColorGradeShadowLum" => slider!(G::ColorGrading, s.grading.shadows.luminance),
            "ColorGradeHighlightLum" => slider!(G::ColorGrading, s.grading.highlights.luminance),
            "ColorGradeMidtoneHue" => slider!(G::ColorGrading, s.grading.midtones.hue),
            "ColorGradeMidtoneSat" => slider!(G::ColorGrading, s.grading.midtones.saturation),
            "ColorGradeMidtoneLum" => slider!(G::ColorGrading, s.grading.midtones.luminance),
            "ColorGradeGlobalHue" => slider!(G::ColorGrading, s.grading.global.hue),
            "ColorGradeGlobalSat" => slider!(G::ColorGrading, s.grading.global.saturation),
            "ColorGradeGlobalLum" => slider!(G::ColorGrading, s.grading.global.luminance),
            "ColorGradeBlending" => slider!(G::ColorGrading, s.grading.blending),
            "Sharpness" => slider!(G::Sharpening, s.detail.sharpen_amount),
            "SharpenRadius" => slider!(G::Sharpening, s.detail.sharpen_radius),
            "SharpenDetail" => slider!(G::Sharpening, s.detail.sharpen_detail),
            "SharpenEdgeMasking" => slider!(G::Sharpening, s.detail.sharpen_masking),
            "LuminanceSmoothing" => slider!(G::NoiseReduction, s.detail.luminance),
            "LuminanceNoiseReductionDetail" => slider!(G::NoiseReduction, s.detail.luminance_detail),
            "LuminanceNoiseReductionContrast" => slider!(G::NoiseReduction, s.detail.luminance_contrast),
            "ColorNoiseReduction" => slider!(G::NoiseReduction, s.detail.color),
            "ColorNoiseReductionDetail" => slider!(G::NoiseReduction, s.detail.color_detail),
            "ColorNoiseReductionSmoothness" => slider!(G::NoiseReduction, s.detail.color_smoothness),
            "PostCropVignetteAmount" => slider!(G::Vignette, s.effects.vignette_amount),
            "PostCropVignetteMidpoint" => slider!(G::Vignette, s.effects.vignette_midpoint),
            "PostCropVignetteRoundness" => slider!(G::Vignette, s.effects.vignette_roundness),
            "PostCropVignetteFeather" => slider!(G::Vignette, s.effects.vignette_feather),
            "PostCropVignetteHighlightContrast" => slider!(G::Vignette, s.effects.vignette_highlights),
            "PostCropVignetteStyle" => {
                if let Some(n) = n {
                    s.effects.vignette_style = match n as i32 {
                        2 => VignetteStyle::ColorPriority,
                        3 => VignetteStyle::PaintOverlay,
                        _ => VignetteStyle::HighlightPriority,
                    };
                    set(G::Vignette);
                }
            }
            "Glow" => slider!(G::Glow, s.effects.glow_amount),
            "GlowRange" => slider!(G::Glow, s.effects.glow_range),
            "GlowSpread" => slider!(G::Glow, s.effects.glow_spread),
            "GlowWarmth" => slider!(G::Glow, s.effects.glow_warmth),
            // A black & white profile (Adobe Monochrome, the B&W looks) is what turns
            // Lightroom's B&W presets grey: our Black & White treatment does that here.
            "Look" | "CameraProfile" if monochrome(v) => {
                mono = true;
                done = false;
            }
            "GrainAmount" => slider!(G::Grain, s.effects.grain_amount),
            "GrainSize" => slider!(G::Grain, s.effects.grain_size),
            "GrainFrequency" => slider!(G::Grain, s.effects.grain_roughness),
            "PerspectiveVertical" => slider!(G::Transform, s.transform.vertical),
            "PerspectiveHorizontal" => slider!(G::Transform, s.transform.horizontal),
            "PerspectiveRotate" => slider!(G::Transform, s.transform.rotate),
            "PerspectiveAspect" => slider!(G::Transform, s.transform.aspect),
            "PerspectiveScale" => slider!(G::Transform, s.transform.scale),
            "PerspectiveX" => slider!(G::Transform, s.transform.x_offset),
            "PerspectiveY" => slider!(G::Transform, s.transform.y_offset),
            "CropTop" if has_crop => slider!(G::Crop, s.transform.crop.top),
            "CropLeft" if has_crop => slider!(G::Crop, s.transform.crop.left),
            "CropBottom" if has_crop => slider!(G::Crop, s.transform.crop.bottom),
            "CropRight" if has_crop => slider!(G::Crop, s.transform.crop.right),
            "CropAngle" if has_crop => slider!(G::Crop, s.transform.crop.angle),
            "CropTop" | "CropLeft" | "CropBottom" | "CropRight" | "CropAngle" => {}
            _ => done = false,
        }
        if done {
            continue;
        }
        // The 8-colour sliders.
        let colour = |prefix: &str| {
            key.strip_prefix(prefix).and_then(|c| COLORS.iter().position(|n| *n == c))
        };
        if let Some(n) = n {
            if let Some(i) = colour("HueAdjustment") {
                s.hsl.hue[i] = n;
                set(G::Hsl);
                continue;
            }
            if let Some(i) = colour("SaturationAdjustment") {
                s.hsl.saturation[i] = n;
                set(G::Hsl);
                continue;
            }
            if let Some(i) = colour("LuminanceAdjustment") {
                s.hsl.luminance[i] = n;
                set(G::Hsl);
                continue;
            }
            if let Some(i) = colour("GrayMixer") {
                s.bw_mix[i] = n;
                set(G::BwMix);
                continue;
            }
        }
        if ABOUT.contains(&key) || default_profile(key, v) {
            continue;
        }
        // Lens corrections, calibration… left at their defaults are no loss.
        if n == Some(0.0) || v.truth() == Some(false) {
            continue;
        }
        let k = kind(key);
        if !k.is_empty() && !unsupported.contains(&k) {
            unsupported.push(k);
        }
    }
    // White balance: a named one, or Custom with the preset's Temp / Tint.
    // (Whatever ConvertToGrayscale says: the profile decides in Lightroom.)
    if mono {
        s.treatment = Treatment::BlackAndWhite;
        set(SettingsGroup::Treatment);
    }
    // (A light-source preset on a JPEG, or Custom without values for this kind of
    // photo, leaves the white balance alone.)
    let custom = temp.0.is_some() || temp.1.is_some();
    let named = wb_name.filter(|w| raw || WbPreset::RELATIVE.contains(w));
    let wb_set = match named {
        Some(w) if w != WbPreset::Custom => {
            s.wb = w;
            true
        }
        _ if custom => {
            if s.wb != WbPreset::Custom {
                // From As Shot: the other one of Temp / Tint starts neutral.
                (s.temp, s.tint) = (if raw { 5500.0 } else { 0.0 }, 0.0);
            }
            s.wb = WbPreset::Custom;
            s.temp = temp.0.unwrap_or(s.temp);
            s.tint = temp.1.unwrap_or(s.tint);
            true
        }
        _ => false,
    };
    if wb_set {
        set(SettingsGroup::WhiteBalance);
    }
    groups.sort();
    LrSummary { groups, unsupported }
}

/// What a preset would set and couldn't (for showing it, without a photo).
pub fn lightroom_summary(values: &[(String, LrValue)]) -> LrSummary {
    apply_lightroom(values, &mut DevelopSettings::default(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal"
   crs:UUID="0123"
   crs:SupportsAmount="True"
   crs:Version="15.0"
   crs:ProcessVersion="11.0"
   crs:WhiteBalance="Custom"
   crs:IncrementalTemperature="+12"
   crs:IncrementalTint="-4"
   crs:Exposure2012="+0.35"
   crs:Contrast2012="-12"
   crs:ConvertToGrayscale="False"
   crs:SplitToningShadowHue="220"
   crs:SplitToningShadowSaturation="15"
   crs:SplitToningBalance="-20"
   crs:ColorGradeMidtoneLum="5"
   crs:HueAdjustmentOrange="-6"
   crs:LuminanceAdjustmentAqua="10"
   crs:PostCropVignetteStyle="3"
   crs:PostCropVignetteAmount="-15"
   crs:GrainFrequency="60"
   crs:LensProfileEnable="1"
   crs:RedHue="0"
   crs:CameraProfile="Camera Vivid"
   crs:HasSettings="True">
   <crs:Name>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Moody Film</rdf:li>
    </rdf:Alt>
   </crs:Name>
   <crs:Group>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">My Looks</rdf:li>
    </rdf:Alt>
   </crs:Group>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 20</rdf:li>
     <rdf:li>128, 128</rdf:li>
     <rdf:li>255, 240</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
   <crs:ToneCurvePV2012Red>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012Red>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    fn xmp_preset() -> LrPreset {
        let doc = xmp::XmpDoc::parse(XMP).unwrap();
        lr_values_from_xmp(doc.properties("http://ns.adobe.com/camera-raw-settings/1.0/"))
    }

    #[test]
    fn an_xmp_preset_sets_what_it_has() {
        let p = xmp_preset();
        assert_eq!(p.name, "Moody Film");
        assert_eq!(p.group.as_deref(), Some("My Looks"));
        // On a JPEG: the incremental white balance.
        let mut s = DevelopSettings { highlights: 30.0, ..DevelopSettings::default() };
        let sum = apply_lightroom(&p.values, &mut s, false);
        assert_eq!((s.wb, s.temp, s.tint), (WbPreset::Custom, 12.0, -4.0));
        assert_eq!((s.exposure, s.contrast), (0.35, -12.0));
        // Only what the preset has: Highlights stays.
        assert_eq!(s.highlights, 30.0);
        assert_eq!(s.treatment, Treatment::Color);
        assert_eq!(s.grading.shadows.hue, 220.0);
        assert_eq!(s.grading.shadows.saturation, 15.0);
        assert_eq!(s.grading.balance, -20.0);
        assert_eq!(s.grading.midtones.luminance, 5.0);
        assert_eq!(s.hsl.hue[1], -6.0);
        assert_eq!(s.hsl.luminance[4], 10.0);
        assert_eq!(s.effects.vignette_style, VignetteStyle::PaintOverlay);
        assert_eq!(s.effects.vignette_amount, -15.0);
        assert_eq!(s.effects.grain_roughness, 60.0);
        assert_eq!(s.curve.rgb, vec![[0.0, 20.0 / 255.0], [128.0 / 255.0, 128.0 / 255.0], [1.0, 240.0 / 255.0]]);
        // A straight red curve is no curve.
        assert!(s.curve.red.is_empty());
        use SettingsGroup as G;
        assert_eq!(
            sum.groups,
            vec![G::Treatment, G::WhiteBalance, G::BasicTone, G::ToneCurve, G::Hsl, G::ColorGrading, G::Vignette, G::Grain]
        );
        // The lens profile and the camera profile can't come; Red Hue 0 is no loss.
        assert_eq!(sum.unsupported, vec!["Lens Corrections".to_string(), "Profile".to_string()]);
        // On a RAW photo it has no Temperature: the white balance stays.
        let mut r = DevelopSettings::default();
        let sum = apply_lightroom(&p.values, &mut r, true);
        assert_eq!(r.wb, WbPreset::AsShot);
        assert!(!sum.groups.contains(&G::WhiteBalance));
    }

    #[test]
    fn a_black_and_white_profile_makes_the_preset_black_and_white() {
        let look = |name: &str| {
            vec![
                ("ConvertToGrayscale".to_string(), LrValue::Bool(false)),
                (
                    "Look".to_string(),
                    LrValue::List(vec![LrValue::Str(format!("Name={name}")), LrValue::Str("Amount=1.2".into())]),
                ),
                ("Contrast2012".to_string(), LrValue::Num(10.0)),
            ]
        };
        let mut s = DevelopSettings::default();
        let sum = apply_lightroom(&look("B&W 11"), &mut s, true);
        assert_eq!(s.treatment, Treatment::BlackAndWhite);
        assert_eq!(sum.unsupported, vec!["Profile".to_string()]);
        // A colour creative look stays colour; Lightroom's default profile is no loss.
        let mut s = DevelopSettings::default();
        let sum = apply_lightroom(&look("Modern 04"), &mut s, true);
        assert_eq!((s.treatment, sum.unsupported.len()), (Treatment::Color, 1));
        let mut s = DevelopSettings::default();
        let sum = apply_lightroom(&[("CameraProfile".into(), LrValue::Str("Default Color".into()))], &mut s, true);
        assert!(sum.unsupported.is_empty() && sum.groups.is_empty());
    }

    #[test]
    fn an_lrtemplate_reads_and_applies() {
        let text = r#"s = {
	id = "8A6C",
	internalName = "Old Look",
	title = ZSTR "$$$/Presets/OldLook=Old Look",
	type = "Develop",
	value = {
		settings = {
			Exposure2012 = 0.5,
			Temperature = 6200,
			Tint = 8,
			WhiteBalance = "Custom",
			ConvertToGrayscale = true,
			GrayMixerBlue = -30,
			["ToneCurvePV2012"] = {
				0,
				10,
				255,
				250,
			},
			ShadowTint = 5, -- calibration
			FillLight = 20,
		},
		uuid = "1234",
	},
	version = 0,
}
"#;
        let p = read_lrtemplate(text).unwrap();
        assert_eq!(p.name, "Old Look");
        let mut s = DevelopSettings::default();
        let sum = apply_lightroom(&p.values, &mut s, true);
        assert_eq!((s.wb, s.temp, s.tint), (WbPreset::Custom, 6200.0, 8.0));
        assert_eq!(s.exposure, 0.5);
        assert_eq!(s.treatment, Treatment::BlackAndWhite);
        assert_eq!(s.bw_mix[5], -30.0);
        assert_eq!(s.curve.rgb, vec![[0.0, 10.0 / 255.0], [1.0, 250.0 / 255.0]]);
        assert_eq!(sum.unsupported, vec!["Calibration".to_string(), "Older (2010) process sliders".to_string()]);
        assert!(read_lrtemplate("s = { title = \"x\" }").is_err());
        assert!(read_lrtemplate("hello").is_err());
    }
}
