//! HSL / Color and B&W Mix (LrC's HSL / Color panel, and its B&W panel in Black & White):
//! hue, saturation and luminance per colour range, worked in OkLCh (a perceptual colour
//! space, so hue shifts keep brightness and saturation changes don't darken).

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::matrix::{self, Mat3f};

/// The eight colour ranges, in hue order.
pub const COLORS: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];

/// Each range's reference colour (linear sRGB); its OkLCh hue is the range's centre. Also
/// what the panel paints its sliders with.
pub const SWATCHES: [[f32; 3]; 8] = [
    [1.0, 0.0, 0.0],
    [1.0, 0.35, 0.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 1.0, 1.0],
    [0.0, 0.0, 1.0],
    [0.35, 0.0, 1.0],
    [1.0, 0.0, 1.0],
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HslGroup {
    Hue,
    Saturation,
    Luminance,
}

impl HslGroup {
    pub const ALL: [HslGroup; 3] = [Self::Hue, Self::Saturation, Self::Luminance];

    pub fn label(self) -> &'static str {
        match self {
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Luminance => "Luminance",
        }
    }
}

/// HSL settings, −100..+100 per colour range.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hsl {
    pub hue: [f32; 8],
    pub saturation: [f32; 8],
    pub luminance: [f32; 8],
}

impl Hsl {
    pub fn get(&self, group: HslGroup) -> &[f32; 8] {
        match group {
            HslGroup::Hue => &self.hue,
            HslGroup::Saturation => &self.saturation,
            HslGroup::Luminance => &self.luminance,
        }
    }

    pub fn get_mut(&mut self, group: HslGroup) -> &mut [f32; 8] {
        match group {
            HslGroup::Hue => &mut self.hue,
            HslGroup::Saturation => &mut self.saturation,
            HslGroup::Luminance => &mut self.luminance,
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Hsl::default()
    }
}

// Oklab (Björn Ottosson): linear sRGB → LMS → cube root → Lab.
const M1: Mat3f = [
    [0.412_221_46, 0.536_332_55, 0.051_445_995],
    [0.211_903_5, 0.680_699_5, 0.107_396_96],
    [0.088_302_46, 0.281_718_85, 0.629_978_7],
];
const M2: Mat3f = [
    [0.210_454_26, 0.793_617_8, -0.004_072_047],
    [1.977_998_5, -2.428_592_2, 0.450_593_7],
    [0.025_904_037, 0.782_771_77, -0.808_675_77],
];
const M2_INV: Mat3f = [
    [1.0, 0.396_337_78, 0.215_803_76],
    [1.0, -0.105_561_346, -0.063_854_17],
    [1.0, -0.089_484_18, -1.291_485_5],
];
const M1_INV: Mat3f = [
    [4.076_741_7, -3.307_711_6, 0.230_969_94],
    [-1.268_438, 2.609_757_4, -0.341_319_38],
    [-0.004_196_086_3, -0.703_418_6, 1.707_614_7],
];

pub(super) fn srgb_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    let lms = matrix::apply(&M1, rgb).map(fast_cbrt);
    matrix::apply(&M2, lms)
}

/// Cube root to within a few float steps (relative 5e-7): a bit-level first guess, then Newton steps;
/// several times faster than libm's.
#[inline]
pub(super) fn fast_cbrt(x: f32) -> f32 {
    let a = x.abs();
    if a < 1e-30 {
        return 0.0;
    }
    let mut y = f32::from_bits(a.to_bits() / 3 + 0x2a51_4067);
    for _ in 0..3 {
        y = (2.0 * y + a / (y * y)) * (1.0 / 3.0);
    }
    y.copysign(x)
}

pub(super) fn oklab_to_srgb(lab: [f32; 3]) -> [f32; 3] {
    let lms = matrix::apply(&M2_INV, lab).map(|v| v * v * v);
    matrix::apply(&M1_INV, lms)
}

/// Hue in degrees 0..360 and chroma of an Oklab colour.
pub(super) fn hue_chroma(lab: [f32; 3]) -> (f32, f32) {
    (lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0), lab[1].hypot(lab[2]))
}

/// The ranges' centre hues (OkLCh degrees), ascending.
fn centres() -> &'static [f32; 8] {
    static C: OnceLock<[f32; 8]> = OnceLock::new();
    C.get_or_init(|| SWATCHES.map(|s| hue_chroma(srgb_to_oklab(s)).0))
}

/// How much each range applies at hue `h`: the two ranges around it, eased between their
/// centres; the weights sum to 1.
pub fn weights(h: f32) -> [f32; 8] {
    let c = centres();
    let mut w = [0.0; 8];
    for i in 0..8 {
        let (a, b) = (c[i], c[(i + 1) % 8]);
        let span = (b - a).rem_euclid(360.0);
        let d = (h - a).rem_euclid(360.0);
        if d < span {
            let t = d / span;
            let t = t * t * (3.0 - 2.0 * t);
            w[i] = 1.0 - t;
            w[(i + 1) % 8] = t;
            break;
        }
    }
    w
}

/// Prepared HSL (or B&W Mix) for rendering.
pub(super) struct HslAdjust {
    /// Hue shift (degrees), saturation factor − 1, luminance change per range.
    shift: [f32; 8],
    sat: [f32; 8],
    lum: [f32; 8],
    to_srgb: Mat3f,
    to_work: Mat3f,
}

/// Grey and near-grey colours have no meaningful hue: their colour adjustments fade out.
fn colourfulness(chroma: f32) -> f32 {
    let t = (chroma / 0.05).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl HslAdjust {
    fn new(shift: [f32; 8], sat: [f32; 8], lum: [f32; 8]) -> HslAdjust {
        HslAdjust {
            shift,
            sat,
            lum,
            to_srgb: matrix::to_f32(&matrix::work_to_srgb()),
            to_work: matrix::to_f32(&matrix::srgb_to_work()),
        }
    }

    /// Colour HSL: `None` when it changes nothing.
    pub fn color(hsl: &Hsl) -> Option<HslAdjust> {
        if hsl.is_default() {
            return None;
        }
        let c = centres();
        // +100 moves a range's hue three quarters of the way to the next range, −100 to the
        // previous one.
        let shift = std::array::from_fn(|i| {
            let v = hsl.hue[i] / 100.0;
            let gap = if v >= 0.0 {
                (c[(i + 1) % 8] - c[i]).rem_euclid(360.0)
            } else {
                (c[i] - c[(i + 7) % 8]).rem_euclid(360.0)
            };
            0.75 * v * gap
        });
        Some(Self::new(shift, hsl.saturation.map(|v| v / 100.0), hsl.luminance.map(|v| 0.35 * v / 100.0)))
    }

    /// B&W Mix: how light each colour becomes in grey. `None` when it changes nothing.
    pub fn bw_mix(mix: &[f32; 8]) -> Option<HslAdjust> {
        if mix.iter().all(|v| *v == 0.0) {
            return None;
        }
        Some(Self::new([0.0; 8], [0.0; 8], mix.map(|v| 0.5 * v / 100.0)))
    }

    /// Adjust a display-referred linear working-space colour.
    #[inline]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let lab = srgb_to_oklab(matrix::apply(&self.to_srgb, rgb));
        let (h, chroma) = hue_chroma(lab);
        let w = weights(h);
        let f = colourfulness(chroma);
        let dot = |v: &[f32; 8]| w.iter().zip(v).map(|(a, b)| a * b).sum::<f32>();
        let h2 = (h + dot(&self.shift)).to_radians();
        let c2 = chroma * (1.0 + dot(&self.sat)).max(0.0);
        let l2 = lab[0] * (1.0 + f * dot(&self.lum)).max(0.0);
        let out = oklab_to_srgb([l2, c2 * h2.cos(), c2 * h2.sin()]);
        matrix::apply(&self.to_work, out).map(|v| v.max(0.0))
    }

    /// B&W Mix: the grey (linear luminance) for a colour whose plain grey is `y`.
    #[inline]
    pub fn grey(&self, rgb: [f32; 3], y: f32) -> f32 {
        let lab = srgb_to_oklab(matrix::apply(&self.to_srgb, rgb));
        let (h, chroma) = hue_chroma(lab);
        let w = weights(h);
        let k = 1.0 + colourfulness(chroma) * w.iter().zip(&self.lum).map(|(a, b)| a * b).sum::<f32>();
        // Oklab L is a cube root of luminance: scaling L by k scales luminance by k³.
        y * k.max(0.0).powi(3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_cbrt_matches_libm() {
        for k in -400..=400 {
            let x = (k as f32 * 0.05).exp2() * if k % 3 == 0 { -1.0 } else { 1.0 };
            let (fast, exact) = (fast_cbrt(x), x.cbrt());
            assert!((fast - exact).abs() <= exact.abs() * 5e-7, "{x}: {fast} vs {exact}");
        }
        assert_eq!(fast_cbrt(0.0), 0.0);
    }

    #[test]
    fn oklab_round_trips_and_centres_ascend() {
        for rgb in [[0.2, 0.5, 0.8], [1.0, 0.0, 0.0], [0.05, 0.05, 0.05]] {
            let back = oklab_to_srgb(srgb_to_oklab(rgb));
            assert!(rgb.iter().zip(back).all(|(a, b)| (a - b).abs() < 1e-3), "{rgb:?} -> {back:?}");
        }
        let c = centres();
        assert!(c.windows(2).all(|w| w[0] < w[1]), "{c:?}");
    }

    #[test]
    fn weights_sum_to_one_and_peak_at_centres() {
        for h in (0..360).step_by(7) {
            let w = weights(h as f32);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5, "hue {h}");
        }
        for (i, c) in centres().iter().enumerate() {
            assert!(weights(*c + 0.01)[i] > 0.99, "{}", COLORS[i]);
        }
    }
}
