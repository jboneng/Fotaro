//! Color Grading (LrC's Color Grading panel): a tint (hue and saturation) and luminance for
//! the shadows, midtones and highlights, plus a global one; Blending sets how much the
//! three ranges overlap and Balance moves them towards the shadows or the highlights.
//! Works in Black & White too (split toning).

use serde::{Deserialize, Serialize};

use super::hsl::{hue_chroma, oklab_to_srgb, srgb_to_oklab};
use super::matrix::{self, Mat3f};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GradeWheel {
    Shadows,
    Midtones,
    Highlights,
    Global,
}

impl GradeWheel {
    pub const ALL: [GradeWheel; 4] = [Self::Shadows, Self::Midtones, Self::Highlights, Self::Global];

    pub fn label(self) -> &'static str {
        match self {
            Self::Shadows => "Shadows",
            Self::Midtones => "Midtones",
            Self::Highlights => "Highlights",
            Self::Global => "Global",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GradeParam {
    Hue,
    Saturation,
    Luminance,
}

impl GradeParam {
    pub fn label(self) -> &'static str {
        match self {
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Luminance => "Luminance",
        }
    }
}

/// One wheel: hue 0..360 (colour-wheel degrees: 0 red, 120 green, 240 blue), saturation
/// 0..100, luminance −100..+100.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wheel {
    pub hue: f32,
    pub saturation: f32,
    pub luminance: f32,
}

impl Wheel {
    pub fn get(&self, p: GradeParam) -> f32 {
        match p {
            GradeParam::Hue => self.hue,
            GradeParam::Saturation => self.saturation,
            GradeParam::Luminance => self.luminance,
        }
    }

    pub fn get_mut(&mut self, p: GradeParam) -> &mut f32 {
        match p {
            GradeParam::Hue => &mut self.hue,
            GradeParam::Saturation => &mut self.saturation,
            GradeParam::Luminance => &mut self.luminance,
        }
    }

    fn is_neutral(&self) -> bool {
        self.saturation == 0.0 && self.luminance == 0.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorGrading {
    pub shadows: Wheel,
    pub midtones: Wheel,
    pub highlights: Wheel,
    pub global: Wheel,
    /// 0..100 (50 by default): how far the ranges reach into each other.
    pub blending: f32,
    /// −100..+100: positive gives the highlights more of the tonal range.
    pub balance: f32,
}

impl Default for ColorGrading {
    fn default() -> Self {
        ColorGrading {
            shadows: Wheel::default(),
            midtones: Wheel::default(),
            highlights: Wheel::default(),
            global: Wheel::default(),
            blending: 50.0,
            balance: 0.0,
        }
    }
}

impl ColorGrading {
    pub fn wheel(&self, w: GradeWheel) -> &Wheel {
        match w {
            GradeWheel::Shadows => &self.shadows,
            GradeWheel::Midtones => &self.midtones,
            GradeWheel::Highlights => &self.highlights,
            GradeWheel::Global => &self.global,
        }
    }

    pub fn wheel_mut(&mut self, w: GradeWheel) -> &mut Wheel {
        match w {
            GradeWheel::Shadows => &mut self.shadows,
            GradeWheel::Midtones => &mut self.midtones,
            GradeWheel::Highlights => &mut self.highlights,
            GradeWheel::Global => &mut self.global,
        }
    }

    /// Nothing to apply (hues without saturation, Blending and Balance alone, do nothing).
    pub fn is_neutral(&self) -> bool {
        GradeWheel::ALL.iter().all(|w| self.wheel(*w).is_neutral())
    }
}

/// The colour-wheel colour at `hue` degrees (fully saturated, linear sRGB).
pub fn wheel_color(hue: f32) -> [f32; 3] {
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    match h as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    }
}

/// Oklab chroma a wheel at saturation 100 adds.
const MAX_TINT: f32 = 0.07;
/// Oklab lightness change (relative) at luminance ±100.
const MAX_LUM: f32 = 0.3;

/// Prepared Color Grading for rendering.
pub(super) struct Grading {
    /// Per wheel (shadows, midtones, highlights, global): Oklab (a, b) tint and lightness.
    tints: [[f32; 2]; 4],
    lums: [f32; 4],
    pivot: f32,
    width: f32,
    to_srgb: Mat3f,
    to_work: Mat3f,
}

impl Grading {
    pub fn new(g: &ColorGrading) -> Option<Grading> {
        if g.is_neutral() {
            return None;
        }
        let wheels = [g.shadows, g.midtones, g.highlights, g.global];
        let tints = wheels.map(|w| {
            // The direction of the wheel's colour in Oklab, scaled by its saturation.
            let lab = srgb_to_oklab(wheel_color(w.hue));
            let (h, _) = hue_chroma(lab);
            let k = MAX_TINT * w.saturation.clamp(0.0, 100.0) / 100.0;
            let h = h.to_radians();
            [k * h.cos(), k * h.sin()]
        });
        Some(Grading {
            tints,
            lums: wheels.map(|w| MAX_LUM * w.luminance / 100.0),
            // Balance moves the meeting point of shadows and highlights; Blending widens it.
            pivot: 0.55 - 0.25 * g.balance / 100.0,
            width: 0.12 + 0.38 * g.blending.clamp(0.0, 100.0) / 100.0,
            to_srgb: matrix::to_f32(&matrix::work_to_srgb()),
            to_work: matrix::to_f32(&matrix::srgb_to_work()),
        })
    }

    /// How much of each wheel applies at Oklab lightness `l` (shadows, midtones,
    /// highlights, global).
    fn weights(&self, l: f32) -> [f32; 4] {
        let t = ((l - (self.pivot - self.width)) / (2.0 * self.width)).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        [(1.0 - s) * (1.0 - s), 4.0 * s * (1.0 - s), s * s, 1.0]
    }

    /// Grade a display-referred linear working-space colour.
    #[inline]
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [l, a, b] = srgb_to_oklab(matrix::apply(&self.to_srgb, rgb));
        let w = self.weights(l);
        let mut out = [l, a, b];
        for ((w, tint), lum) in w.iter().zip(&self.tints).zip(&self.lums) {
            out[1] += w * tint[0];
            out[2] += w * tint[1];
            out[0] *= 1.0 + w * lum;
        }
        matrix::apply(&self.to_work, oklab_to_srgb(out)).map(|v| v.max(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_grading_does_nothing() {
        assert!(Grading::new(&ColorGrading::default()).is_none());
        // A hue without saturation is still neutral.
        let mut g = ColorGrading::default();
        g.shadows.hue = 220.0;
        g.balance = 40.0;
        assert!(Grading::new(&g).is_none());
    }

    #[test]
    fn ranges_follow_brightness_and_balance() {
        let mut g = ColorGrading { balance: 0.0, ..Default::default() };
        g.highlights.saturation = 50.0;
        let p = Grading::new(&g).unwrap();
        let (dark, light) = (p.weights(0.15), p.weights(0.95));
        assert!(dark[0] > 0.9 && dark[2] < 0.05);
        assert!(light[2] > 0.9 && light[0] < 0.05);
        // More balance: the highlights reach further down.
        let high = Grading::new(&ColorGrading { balance: 100.0, ..g.clone() }).unwrap();
        let low = Grading::new(&ColorGrading { balance: -100.0, ..g }).unwrap();
        assert!(high.weights(0.5)[2] > low.weights(0.5)[2]);
    }

    #[test]
    fn wheel_colours_go_round() {
        assert_eq!(wheel_color(0.0), [1.0, 0.0, 0.0]);
        assert_eq!(wheel_color(120.0), [0.0, 1.0, 0.0]);
        assert_eq!(wheel_color(240.0), [0.0, 0.0, 1.0]);
        assert_eq!(wheel_color(360.0), [1.0, 0.0, 0.0]);
    }
}
