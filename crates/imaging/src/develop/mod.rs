//! Develop: non-destructive adjustments (LrC's Basic panel), rendered from the original
//! file's pixels. The catalog stores [`DevelopSettings`]; nothing is written to the photo.

mod matrix;
mod pipeline;
mod settings;
mod source;
mod wb;

#[cfg(test)]
mod tests;

pub use pipeline::{render_linear16, render_srgb8};
pub use settings::{DevelopSettings, PROCESS_VERSION, Slider, Treatment, WbPreset};
pub use source::{ResolvedWb, SourceColor, WorkingImage};
