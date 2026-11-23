//! Develop: non-destructive adjustments (LrC's Basic panel), rendered from the original
//! file's pixels. The catalog stores [`DevelopSettings`]; nothing is written to the photo.

mod cache;
mod curve;
mod dehaze;
mod detail;
mod dust;
mod effects;
mod fill_detail;
mod grading;
mod groups;
mod heal;
mod hsl;
mod lens;
mod lightroom;
mod local;
mod lut;
mod masks;
mod matrix;
mod perf;
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
    DustLayer, DustPolarity, DustSource, IrPass, detect_both, detect_dust, detect_ir, detect_picture,
    detect_picture_as, dust_from_map, dust_lightness, fill_masked, find_sensor_dust, mask_at,
    render_dust_overlay, sensor_dust_spots,
};
pub use effects::{EffectParam, Effects, VignetteStyle};
pub use fill_detail::{FinishedFill, finish_fill};
pub use grading::{ColorGrading, GradeParam, GradeWheel, Wheel, wheel_color};
pub use groups::{SettingsGroup, changed_groups, copy_groups};
pub use heal::{
    Fill, FillInput, Spot, SpotArea, SpotMask, SpotMode, SpotStroke, fill_decode, fill_encode, fill_input,
    find_source, healed_crop, prepare_area,
};
pub use hsl::{COLORS, Hsl, HslGroup, SWATCHES};
pub use lens::{
    Distortion, LensCorrections, LensProfile, LensSetup, Tca, Vignetting, lens_correction_at, measure_ca,
};
pub use lightroom::{
    LrPreset, LrSummary, LrValue, apply_lightroom, lightroom_summary, lr_values_from_xmp, read_lrtemplate,
};
pub use lut::{
    Lut, LutProfile, StoredLut, content_hash, import_lut, list_luts, load_lut, lut_available, lut_store,
    parse_cube, set_lut_store,
};
pub use masks::{
    CombineMode, Component, DepthRange, LocalAdjust, LocalParam, Mask, Shape, Stroke, mask_picture,
    save_fill_picture, save_mask_picture, set_mask_store,
};
pub use pipeline::{
    Region, auto_tone, render_linear16, render_mask, render_region_srgb8, render_region_srgb8_until,
    render_region_srgb8_view, render_srgb8,
};
pub use settings::{DevelopSettings, PROCESS_VERSION, Slider, Treatment, WbPreset};
pub use source::{ResolvedWb, SourceColor, WorkingImage};
pub use transform::{Crop, MAX_CROP_ANGLE, MAX_GUIDES, PointMap, Transform, TransformParam, Upright};
pub use upright::{apply_upright, guided_angles, straighten_angle};
