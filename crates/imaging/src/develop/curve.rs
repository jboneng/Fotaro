//! Tone Curve (LrC's Tone Curve panel): a parametric curve (Highlights, Lights, Darks,
//! Shadows, with movable split points) and point curves for RGB and each channel.
//!
//! Curves work on perceptual values 0..1 (gamma 2.2 encoded), after the Basic panel's
//! tone adjustments.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CurveChannel {
    #[default]
    Rgb,
    Red,
    Green,
    Blue,
}

impl CurveChannel {
    pub const ALL: [CurveChannel; 4] = [Self::Rgb, Self::Red, Self::Green, Self::Blue];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rgb => "RGB",
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Blue => "Blue",
        }
    }
}

/// Point curve presets (LrC's Point Curve menu), as (input, output) pairs 0..1.
pub const POINT_PRESETS: [(&str, &[[f32; 2]]); 3] = [
    ("Linear", &[[0.0, 0.0], [1.0, 1.0]]),
    ("Medium Contrast", &[[0.0, 0.0], [0.125, 0.086], [0.25, 0.22], [0.5, 0.5], [0.75, 0.77], [1.0, 1.0]]),
    ("Strong Contrast", &[[0.0, 0.0], [0.125, 0.063], [0.25, 0.196], [0.5, 0.5], [0.75, 0.79], [1.0, 1.0]]),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToneCurve {
    /// Parametric regions, −100..+100.
    pub highlights: f32,
    pub lights: f32,
    pub darks: f32,
    pub shadows: f32,
    /// Where the regions meet, 0..100 (LrC's split points; 25, 50, 75 by default).
    pub splits: [f32; 3],
    /// Point curves, (input, output) 0..1 sorted by input; empty means linear.
    pub rgb: Vec<[f32; 2]>,
    pub red: Vec<[f32; 2]>,
    pub green: Vec<[f32; 2]>,
    pub blue: Vec<[f32; 2]>,
}

pub const DEFAULT_SPLITS: [f32; 3] = [25.0, 50.0, 75.0];

impl Default for ToneCurve {
    fn default() -> Self {
        ToneCurve {
            highlights: 0.0,
            lights: 0.0,
            darks: 0.0,
            shadows: 0.0,
            splits: DEFAULT_SPLITS,
            rgb: Vec::new(),
            red: Vec::new(),
            green: Vec::new(),
            blue: Vec::new(),
        }
    }
}

const LINEAR: [[f32; 2]; 2] = [[0.0, 0.0], [1.0, 1.0]];

/// A raised-cosine bump: 0 outside (a, c), 1 at `peak`.
fn bump(x: f32, a: f32, peak: f32, c: f32) -> f32 {
    if x <= a || x >= c {
        return 0.0;
    }
    let t = if x < peak { (x - a) / (peak - a) } else { (c - x) / (c - peak) };
    0.5 - 0.5 * (std::f32::consts::PI * t.clamp(0.0, 1.0)).cos()
}

impl ToneCurve {
    pub fn points(&self, ch: CurveChannel) -> &Vec<[f32; 2]> {
        match ch {
            CurveChannel::Rgb => &self.rgb,
            CurveChannel::Red => &self.red,
            CurveChannel::Green => &self.green,
            CurveChannel::Blue => &self.blue,
        }
    }

    pub fn points_mut(&mut self, ch: CurveChannel) -> &mut Vec<[f32; 2]> {
        match ch {
            CurveChannel::Rgb => &mut self.rgb,
            CurveChannel::Red => &mut self.red,
            CurveChannel::Green => &mut self.green,
            CurveChannel::Blue => &mut self.blue,
        }
    }

    /// A channel's points, the two end points if it has none.
    pub fn points_or_linear(&self, ch: CurveChannel) -> Vec<[f32; 2]> {
        let p = self.points(ch);
        if p.len() >= 2 { p.clone() } else { LINEAR.to_vec() }
    }

    fn is_linear(points: &[[f32; 2]]) -> bool {
        points.len() < 2 || points.iter().all(|p| (p[0] - p[1]).abs() < 1e-6)
    }

    pub fn is_default(&self) -> bool {
        self.parametric_is_neutral() && CurveChannel::ALL.iter().all(|c| Self::is_linear(self.points(*c)))
    }

    pub fn parametric_is_neutral(&self) -> bool {
        self.highlights == 0.0 && self.lights == 0.0 && self.darks == 0.0 && self.shadows == 0.0
    }

    /// The parametric curve at `x` (not yet forced monotonic; see [`CurveLuts`]).
    pub fn parametric(&self, x: f32) -> f32 {
        if self.parametric_is_neutral() {
            return x;
        }
        let [s1, s2, s3] = self.splits.map(|s| (s / 100.0).clamp(0.02, 0.98));
        // The outer regions peak at the outer split points and fall away gently to black
        // and white (peaking nearer the ends made the curve kink steeply there).
        let (outer, inner) = (0.16 / 100.0, 0.2 / 100.0);
        let shift = outer * self.shadows * bump(x, 0.0, s1, s2)
            + inner * self.darks * bump(x, s1 * 0.4, (s1 + s2) / 2.0, s3)
            + inner * self.lights * bump(x, s1, (s2 + s3) / 2.0, s3 + (1.0 - s3) * 0.6)
            + outer * self.highlights * bump(x, s2, s3, 1.0);
        (x + shift).clamp(0.0, 1.0)
    }

    /// Put the split points in order, a little apart (LrC keeps them so).
    pub fn clamp_splits(splits: [f32; 3]) -> [f32; 3] {
        let s1 = splits[0].clamp(5.0, 85.0);
        let s2 = splits[1].clamp(s1 + 5.0, 90.0);
        let s3 = splits[2].clamp(s2 + 5.0, 95.0);
        [s1, s2, s3]
    }
}

/// A smooth curve through points that never overshoots them (monotone piecewise cubic
/// Hermite, Fritsch–Carlson): it rises where the points rise and is flat at their extremes.
#[derive(Debug, Clone)]
pub struct Spline {
    xs: Vec<f32>,
    ys: Vec<f32>,
    ms: Vec<f32>,
}

impl Spline {
    pub fn new(points: &[[f32; 2]]) -> Spline {
        let mut pts: Vec<[f32; 2]> = if points.len() >= 2 { points.to_vec() } else { LINEAR.to_vec() };
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6);
        if pts.len() < 2 {
            pts = LINEAR.to_vec();
        }
        let xs: Vec<f32> = pts.iter().map(|p| p[0]).collect();
        let ys: Vec<f32> = pts.iter().map(|p| p[1]).collect();
        let n = xs.len();
        let d: Vec<f32> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i])).collect();
        let mut ms = vec![0.0; n];
        ms[0] = d[0];
        ms[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            ms[i] = if d[i - 1] * d[i] <= 0.0 {
                0.0
            } else {
                // Weighted harmonic mean (Fritsch–Butland), which keeps the shape.
                let (h0, h1) = (xs[i] - xs[i - 1], xs[i + 1] - xs[i]);
                let (w1, w2) = (2.0 * h1 + h0, h1 + 2.0 * h0);
                (w1 + w2) / (w1 / d[i - 1] + w2 / d[i])
            };
        }
        Spline { xs, ys, ms }
    }

    pub fn eval(&self, x: f32) -> f32 {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0];
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        let i = self.xs.partition_point(|v| *v <= x).saturating_sub(1).min(n - 2);
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        (h00 * self.ys[i] + h10 * h * self.ms[i] + h01 * self.ys[i + 1] + h11 * h * self.ms[i + 1])
            .clamp(0.0, 1.0)
    }
}

/// Evaluate the whole tone curve per channel: parametric, then the RGB point curve, then
/// the channel's own.
pub struct CurveFns {
    curve: ToneCurve,
    rgb: Spline,
    channels: [Spline; 3],
}

impl CurveFns {
    pub fn new(curve: &ToneCurve) -> CurveFns {
        CurveFns {
            curve: curve.clone(),
            rgb: Spline::new(curve.points(CurveChannel::Rgb)),
            channels: [CurveChannel::Red, CurveChannel::Green, CurveChannel::Blue]
                .map(|c| Spline::new(curve.points(c))),
        }
    }

    /// The parametric and RGB curves (shared by all channels).
    pub fn master(&self, x: f32) -> f32 {
        self.rgb.eval(self.curve.parametric(x))
    }

    /// The RGB point curve, for a value the parametric curve already went through.
    pub fn master_after_parametric(&self, p: f32) -> f32 {
        self.rgb.eval(p)
    }

    /// Channel `c` (0 red, 1 green, 2 blue) after the master curve.
    pub fn channel(&self, c: usize, master: f32) -> f32 {
        self.channels[c].eval(master)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_curve_is_the_identity() {
        let c = ToneCurve::default();
        assert!(c.is_default());
        let f = CurveFns::new(&c);
        for i in 0..=20 {
            let x = i as f32 / 20.0;
            assert!((f.master(x) - x).abs() < 1e-6);
            assert!((f.channel(1, x) - x).abs() < 1e-6);
        }
        // A curve whose points lie on the diagonal is still linear.
        let c = ToneCurve { rgb: vec![[0.0, 0.0], [0.4, 0.4], [1.0, 1.0]], ..Default::default() };
        assert!(c.is_default());
    }

    #[test]
    fn spline_goes_through_points_without_overshoot() {
        let s = Spline::new(&[[0.0, 0.0], [0.25, 0.1], [0.5, 0.5], [0.75, 0.9], [1.0, 1.0]]);
        for p in [[0.25, 0.1], [0.5, 0.5], [0.75, 0.9]] {
            assert!((s.eval(p[0]) - p[1]).abs() < 1e-5);
        }
        let mut last = -1.0;
        for i in 0..=100 {
            let y = s.eval(i as f32 / 100.0);
            assert!(y >= last - 1e-6, "monotone data gives a monotone curve");
            last = y;
        }
        // A peak stays a peak: nothing above it.
        let peak = Spline::new(&[[0.0, 0.0], [0.5, 0.8], [1.0, 0.2]]);
        assert!((0..=100).all(|i| peak.eval(i as f32 / 100.0) <= 0.8 + 1e-6));
    }

    #[test]
    fn parametric_regions_move_their_own_tones() {
        let shadows = ToneCurve { shadows: 100.0, ..Default::default() };
        assert!(shadows.parametric(0.15) > 0.2);
        assert!((shadows.parametric(0.85) - 0.85).abs() < 1e-6);
        let highlights = ToneCurve { highlights: -100.0, ..Default::default() };
        assert!(highlights.parametric(0.85) < 0.8);
        assert!((highlights.parametric(0.15) - 0.15).abs() < 1e-6);
        // Black and white stay put.
        assert_eq!(shadows.parametric(0.0), 0.0);
        assert_eq!(highlights.parametric(1.0), 1.0);
        assert_eq!(ToneCurve::clamp_splits([60.0, 40.0, 99.0]), [60.0, 65.0, 95.0]);
    }

    #[test]
    fn regions_at_full_strength_bend_the_curve_gently() {
        for c in [
            ToneCurve { highlights: -100.0, ..Default::default() },
            ToneCurve { highlights: 100.0, ..Default::default() },
            ToneCurve { shadows: -100.0, ..Default::default() },
            ToneCurve { shadows: 100.0, ..Default::default() },
        ] {
            let steepest = (0..100)
                .map(|i| (c.parametric((i + 1) as f32 / 100.0) - c.parametric(i as f32 / 100.0)) * 100.0)
                .fold(0.0, f32::max);
            assert!(steepest < 2.2, "{c:?}: slope {steepest}");
        }
    }
}
