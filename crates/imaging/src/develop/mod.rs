//! Develop: non-destructive adjustments (LrC's Basic panel), rendered from the original
//! file's pixels. The catalog stores [`DevelopSettings`]; nothing is written to the photo.

mod curve;
mod dehaze;
mod detail;
mod effects;
mod grading;
mod hsl;
mod local;
mod matrix;
mod pipeline;
mod settings;
mod source;
mod wb;

#[cfg(test)]
mod tests;

pub use curve::{CurveChannel, CurveFns, DEFAULT_SPLITS, POINT_PRESETS, Spline, ToneCurve};
pub use detail::{Detail, DetailParam};
pub use effects::{EffectParam, Effects, VignetteStyle};
pub use grading::{ColorGrading, GradeParam, GradeWheel, Wheel, wheel_color};
pub use hsl::{COLORS, Hsl, HslGroup, SWATCHES};
pub use pipeline::{Region, auto_tone, render_linear16, render_region_srgb8, render_srgb8};
pub use settings::{DevelopSettings, PROCESS_VERSION, Slider, Treatment, WbPreset};
pub use source::{ResolvedWb, SourceColor, WorkingImage};
