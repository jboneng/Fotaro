//! Develop: non-destructive adjustments (LrC's Basic panel), rendered from the original
//! file's pixels. The catalog stores [`DevelopSettings`]; nothing is written to the photo.

mod curve;
mod dehaze;
mod detail;
mod dust;
mod effects;
mod grading;
mod heal;
mod hsl;
mod local;
mod masks;
mod matrix;
mod pipeline;
mod settings;
mod source;
mod transform;
mod upright;
mod wb;

#[cfg(test)]
mod tests;

pub use curve::{CurveChannel, CurveFns, DEFAULT_SPLITS, POINT_PRESETS, Spline, ToneCurve};
pub use detail::{Detail, DetailParam, SharpenView};
pub use dust::{
    DustLayer, DustSource, IrPass, detect_both, detect_dust, detect_ir, detect_picture, dust_lightness, fill_masked,
    find_sensor_dust, mask_at, render_dust_overlay, sensor_dust_spots,
};
pub use effects::{EffectParam, Effects, VignetteStyle};
pub use grading::{ColorGrading, GradeParam, GradeWheel, Wheel, wheel_color};
pub use heal::{
    Fill, FillInput, Spot, SpotArea, SpotMode, SpotStroke, fill_decode, fill_encode, fill_input, find_source,
};
pub use hsl::{COLORS, Hsl, HslGroup, SWATCHES};
pub use masks::{
    CombineMode, Component, LocalAdjust, LocalParam, Mask, Shape, Stroke, save_fill_picture,
    save_mask_picture, set_mask_store,
};
pub use pipeline::{
    Region, auto_tone, render_linear16, render_mask, render_region_srgb8, render_region_srgb8_view,
    render_srgb8,
};
pub use settings::{DevelopSettings, PROCESS_VERSION, Slider, Treatment, WbPreset};
pub use source::{ResolvedWb, SourceColor, WorkingImage};
pub use transform::{Crop, MAX_CROP_ANGLE, MAX_GUIDES, Transform, TransformParam, Upright};
pub use upright::{apply_upright, guided_angles, straighten_angle};
