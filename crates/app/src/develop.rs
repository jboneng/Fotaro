//! The Develop module's state, and the thread that renders the photo being developed.
//!
//! The worker keeps the current photo's pixels (a proxy about the screen's size) and
//! renders the latest settings it was sent: while a slider is dragged, requests pile up
//! faster than renders finish, and only the newest one is rendered.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use catalog::{Catalog, FileKind, HistoryStep, PhotoRow};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use imaging::SourceKind;
use imaging::develop::{
    DevelopSettings, Region, ResolvedWb, SharpenView, Upright, WbPreset, WorkingImage, apply_upright,
    auto_tone, find_source, render_mask, render_region_srgb8_view, straighten_angle,
};

use crate::jobs::Histogram;

/// What the center view shows: the photo as developed, or before any adjustments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Current,
    Before,
    /// A preset or snapshot under the pointer, for the Navigator.
    Preview,
}

/// The Remove tool's settings for new spots (kept while the app runs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoveSettings {
    pub mode: imaging::develop::SpotMode,
    /// Size 1..100 (as the brush's), Feather and Opacity 0..100.
    pub size: f32,
    pub feather: f32,
    pub opacity: f32,
}

impl Default for RemoveSettings {
    fn default() -> Self {
        RemoveSettings { mode: imaging::develop::SpotMode::Remove, size: 12.0, feather: 50.0, opacity: 100.0 }
    }
}

/// The Size slider ↔ a spot's radius (half long sides), as the brush's: fine control for
/// small spots.
pub fn spot_radius(size: f32) -> f32 {
    (size / 100.0).powi(2) * 0.5
}

pub fn spot_size(radius: f32) -> f32 {
    ((radius / 0.5).max(0.0).sqrt() * 100.0).clamp(1.0, 100.0)
}

/// A drag with the Remove tool.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoveDrag {
    /// Painting a new brushed spot (its path so far, 0..1 of the photo).
    Paint(Vec<[f32; 2]>),
    /// Painting more area into spot `index`, or (with `erase`) taking some away.
    Extend {
        index: usize,
        path: Vec<[f32; 2]>,
        erase: bool,
    },
    /// Moving spot `index` (the source stays put), its source, or its size.
    Spot {
        index: usize,
        grab: [f32; 2],
        original: imaging::develop::Spot,
    },
    Source {
        index: usize,
        grab: [f32; 2],
        original: imaging::develop::Spot,
    },
    Resize {
        index: usize,
        original: imaging::develop::Spot,
    },
    /// Painting out of Dust & Scratches' found defects (the Subtract brush).
    Subtract(Vec<[f32; 2]>),
}

/// The Remove tool (Q): the selected spot and the drag in progress.
#[derive(Debug, Clone, Default)]
pub struct RemoveTool {
    pub selected: Option<usize>,
    pub drag: Option<RemoveDrag>,
    /// Editing the selected spot's area: painting adds to it (`Some(false)`) or takes away
    /// (`Some(true)`). Shift+drag and Ctrl+drag do the same for one stroke.
    pub area: Option<bool>,
    /// Dust & Scratches' Subtract brush: drags paint out of what was found.
    pub subtract: bool,
}

/// What a drag on the photo does with the crop tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CropHandle {
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// Inside: move the rectangle.
    Move,
    /// Outside: turn the photo.
    Rotate,
}

/// A drag with the crop tool, from `grab` (0..1 of the frame), starting from `original`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CropDrag {
    Handle {
        handle: CropHandle,
        grab: [f32; 2],
        original: imaging::develop::Crop,
    },
    /// The Straighten tool: a line drawn from this screen point.
    Straighten {
        start: egui::Pos2,
    },
}

/// Crop & Straighten (R): the tool's state.
#[derive(Debug, Clone, Default)]
pub struct CropTool {
    pub drag: Option<CropDrag>,
    /// The Straighten tool is out: a drag draws the line to level by.
    pub straighten: bool,
}

/// The shapes a mask component can have (the tools).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    Brush,
    Linear,
    Radial,
    /// Select Object: brush over the thing, or box it (`DevelopState::object_rect`).
    Object,
    /// A mask made by a model (Subject, Sky, Background); not a tool to draw with.
    Ai,
}

impl ShapeKind {
    pub fn label(self) -> &'static str {
        match self {
            ShapeKind::Brush => "Brush",
            ShapeKind::Linear => "Linear Gradient",
            ShapeKind::Radial => "Radial Gradient",
            ShapeKind::Object => "Object",
            ShapeKind::Ai => "AI Mask",
        }
    }
}

/// Where a shape drawn with a tool goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTarget {
    NewMask,
    /// A new component of the selected mask.
    Component(imaging::develop::CombineMode),
}

/// A tool waiting for a drag on the photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaskTool {
    pub kind: ShapeKind,
    pub target: ToolTarget,
}

/// What part of a shape a drag moves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Handle {
    /// The pin or the inside: move the whole shape.
    Move,
    /// A linear gradient's start or end line.
    Start,
    End,
    /// A radial gradient's radius along its own x or y axis.
    RadiusX,
    RadiusY,
    /// Turn the shape around its centre.
    Rotate,
}

/// A drag on the photo while masking.
#[derive(Debug, Clone, PartialEq)]
pub enum MaskDrag {
    /// Moving a handle of the selected component, from `grab`, starting from `original`.
    Edit { handle: Handle, grab: [f32; 2], original: imaging::develop::Shape },
    /// Select Object: brushing over the thing or boxing it, in screen positions.
    Object { path: Vec<egui::Pos2> },
    /// Painting (or erasing) a brush stroke into the selected component.
    Paint,
}

/// The brush's settings (kept while the app runs, as LrC keeps them).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushSettings {
    /// Size 1..100 (a radius from a few pixels to half the photo), Feather, Flow, Density 0..100.
    pub size: f32,
    pub feather: f32,
    pub flow: f32,
    pub density: f32,
    /// Paint erases (also while Alt is held).
    pub erase: bool,
}

impl Default for BrushSettings {
    fn default() -> Self {
        BrushSettings { size: 15.0, feather: 50.0, flow: 100.0, density: 100.0, erase: false }
    }
}

impl BrushSettings {
    /// The brush radius in half long sides of the photo (as strokes store it): size 10
    /// is 0.5%, 50 is 12.5%, 100 is half the long side.
    pub fn radius(&self) -> f32 {
        (self.size / 100.0).powi(2) * 0.5
    }

    /// `[` / `]`: about a sixth smaller or larger.
    pub fn step_size(&mut self, dir: i8) {
        // At least one step either way (small sizes would round back to themselves).
        let next = if dir < 0 {
            (self.size / 1.18).round().min(self.size - 1.0)
        } else {
            (self.size * 1.18).round().max(self.size + 1.0)
        };
        self.size = next.clamp(1.0, 100.0);
    }

    /// The mouse wheel: `dy` points scrolled (up is positive): larger going up, about a
    /// tenth per wheel notch, smoothly for trackpads.
    pub fn scroll_size(&mut self, dy: f32) {
        self.size = (self.size * (dy / 300.0).exp2()).clamp(1.0, 100.0);
    }

    /// Shift+[ / Shift+]: feather in steps of 10.
    pub fn step_feather(&mut self, dir: i8) {
        self.feather = (self.feather + 10.0 * f32::from(dir.signum())).clamp(0.0, 100.0);
    }
}

/// Masking mode (Shift+W): what is selected and what the photo does with drags.
#[derive(Debug, Clone, Default)]
pub struct Masking {
    /// The mask being edited, and its component.
    pub selected: Option<usize>,
    pub component: usize,
    pub tool: Option<MaskTool>,
    /// O: show the selected mask as a red overlay.
    pub overlay: bool,
    /// A mask hovered in the Masks panel (its overlay shows meanwhile).
    pub hover: Option<usize>,
    pub drag: Option<MaskDrag>,
    /// The mask whose name is being edited in the Masks panel, and the text so far.
    pub renaming: Option<usize>,
    pub rename_text: String,
}

impl Masking {
    /// The mask whose overlay to show, if any.
    pub fn overlay_mask(&self) -> Option<usize> {
        self.hover.or(self.selected.filter(|_| self.overlay))
    }
}

/// The next free "Mask N" name.
pub fn next_mask_name(masks: &[imaging::develop::Mask]) -> String {
    let n =
        masks.iter().filter_map(|m| m.name.strip_prefix("Mask ")?.parse::<usize>().ok()).max().unwrap_or(0);
    format!("Mask {}", n.max(masks.len()) + 1)
}

/// What an AI mask selects (the Masks panel's Subject, Sky, Background and Object).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiKind {
    Subject,
    Sky,
    /// Everything but the subject (the subject model's picture, inverted).
    Background,
    /// The thing brushed over, boxed or clicked (Segment Anything).
    Object,
    /// The Remove tool's content-aware fill (LaMa).
    Remove,
    /// Dust & Scratches found by the AI model.
    Dust,
    /// Distractions → People: the people in the photo found and outlined.
    People,
}

impl AiKind {
    pub fn label(self) -> &'static str {
        match self {
            AiKind::Subject => "Subject",
            AiKind::Sky => "Sky",
            AiKind::Background => "Background",
            AiKind::Object => "Object",
            AiKind::Remove => "Remove",
            AiKind::Dust => "Dust & Scratches",
            AiKind::People => "People",
        }
    }

    pub fn model(self) -> segment::AiModel {
        match self {
            AiKind::Subject | AiKind::Background => segment::AiModel::Subject,
            AiKind::Sky => segment::AiModel::Sky,
            AiKind::Object => segment::AiModel::Object,
            AiKind::Remove => segment::AiModel::Remove,
            AiKind::Dust => segment::AiModel::Scratches,
            AiKind::People => segment::AiModel::People,
        }
    }

    fn model_name(self) -> &'static str {
        match self.model() {
            segment::AiModel::Subject => "RMBG-1.4",
            segment::AiModel::Sky => "SegFormer-B2",
            segment::AiModel::Object => "SAM 2.1 tiny",
            segment::AiModel::Remove => "LaMa-Dilated",
            segment::AiModel::RemoveBest => "big-LaMa",
            segment::AiModel::Scratches => "scratch detector",
            segment::AiModel::People => "RT-DETR",
        }
    }
}

/// A Segment Anything point: pixels, and whether it is on the object.
type SamPoint = (f32, f32, bool);

/// Select Object's prompt, in the untransformed photo (0..1, file orientation): points on
/// the object, and a box around it (left, top, right, bottom).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObjectPrompt {
    pub points: Vec<[f32; 2]>,
    pub rect: Option<[f32; 4]>,
}

impl ObjectPrompt {
    /// In pixels of the photo turned `rotation` quarter turns clockwise (`w × h` then).
    fn upright(&self, rotation: u8, w: u32, h: u32) -> (Vec<SamPoint>, Option<[f32; 4]>) {
        let turn = |[u, v]: [f32; 2]| {
            let (x, y) = match rotation % 4 {
                1 => (1.0 - v, u),
                2 => (1.0 - u, 1.0 - v),
                3 => (v, 1.0 - u),
                _ => (u, v),
            };
            (x * w as f32, y * h as f32)
        };
        let points = self.points.iter().map(|&p| {
            let (x, y) = turn(p);
            (x, y, true)
        });
        let rect = self.rect.map(|[l, t, r, b]| {
            let corners = [[l, t], [r, t], [l, b], [r, b]].map(turn);
            let xs = corners.map(|c| c.0);
            let ys = corners.map(|c| c.1);
            let min = |a: [f32; 4]| a.iter().copied().fold(f32::MAX, f32::min);
            let max = |a: [f32; 4]| a.iter().copied().fold(f32::MIN, f32::max);
            [min(xs), min(ys), max(xs), max(ys)]
        });
        (points.collect(), rect)
    }
}

/// An AI mask being made: what it selects (and, for an object, where), where it goes, and
/// how far along it is (share done 0..1, or below 0 while the model runs, and what is
/// happening).
#[derive(Debug, Clone)]
pub struct AiJob {
    pub kind: AiKind,
    pub target: ToolTarget,
    pub prompt: Option<ObjectPrompt>,
    pub progress: std::sync::Arc<std::sync::Mutex<(f32, String)>>,
}

impl AiJob {
    pub fn progress(&self) -> (f32, String) {
        self.progress.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// An AI mask waiting for the user to agree to download what it needs (`bytes`).
#[derive(Debug, Clone)]
pub struct AiConsent {
    pub kind: AiKind,
    pub target: ToolTarget,
    pub prompt: Option<ObjectPrompt>,
    pub bytes: u64,
}

/// Make an AI mask of `rgb` (the photo as displayed, `rotation` quarter turns from the
/// file, `size` pixels; empty when Select Object has it encoded as `key` already):
/// download what is missing, run the model, save its picture. Returns the picture's id
/// and the device that ran it.
#[allow(clippy::too_many_arguments)]
fn run_ai(
    photo_id: i64,
    job: &AiJob,
    rgb: &image::RgbImage,
    size: (u32, u32),
    key: u64,
    rotation: u8,
    gpu: bool,
    ctx: &egui::Context,
) -> Result<(String, &'static str)> {
    use image::imageops::{rotate90, rotate180, rotate270};
    let kind = job.kind;
    let set = |share: f32, what: String| {
        *job.progress.lock().unwrap_or_else(|e| e.into_inner()) = (share, what);
        ctx.request_repaint();
    };
    let model = kind.model();
    if !segment::ready(&[model]) {
        let mut last = std::time::Instant::now();
        segment::ensure(&[model], &mut |share, what| {
            if last.elapsed().as_millis() > 100 {
                last = std::time::Instant::now();
                set(share, format!("Downloading {what}"));
            }
        })?;
    }
    set(-1.0, format!("Finding the {}…", kind.label().to_lowercase()));
    let started = std::time::Instant::now();
    let (mask, device) = match &job.prompt {
        Some(prompt) if kind == AiKind::Object => {
            let (points, rect) = prompt.upright(rotation, size.0, size.1);
            let (mask, device) = segment::select_object(key, rgb, &points, rect, gpu)?;
            // Whatever the decoder thinks: nothing outside the box (it can leak).
            let mut mask = mask;
            if let Some([l, t, r, b]) = rect {
                let pad = 0.02 * size.0.max(size.1) as f32;
                for (x, y, p) in mask.enumerate_pixels_mut() {
                    let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
                    if x < l - pad || x > r + pad || y < t - pad || y > b + pad {
                        p[0] = 0;
                    }
                }
            }
            (mask, device)
        }
        _ => segment::select(model, rgb, gpu)?,
    };
    log::info!("AI mask: {} on {} in {:.0?}", kind.label(), device.label(), started.elapsed());
    // Background is everything but the subject (its own picture, so Invert starts off).
    let mut mask = mask;
    if kind == AiKind::Background {
        image::imageops::invert(&mut mask);
    }
    // Back to the file's orientation, which masks are kept in.
    let mask = match rotation % 4 {
        1 => rotate270(&mask),
        2 => rotate180(&mask),
        3 => rotate90(&mask),
        _ => mask,
    };
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let id = format!("{photo_id}-{:x}", stamp.as_nanos());
    imaging::develop::save_mask_picture(&id, &mask)?;
    Ok((id, device.label()))
}

/// The overlay index that stands for Dust & Scratches' found defects (not a mask).
const DUST_OVERLAY: usize = usize::MAX;

/// A film scan's infrared pass beside it in its unit: `<name>_IR` (and its valid mask,
/// `<name>_IR_valid`), as NegPy writes them.
fn find_ir_pass(catalog: &Catalog, photo: &PhotoRow) -> Option<IrFiles> {
    let stem = photo.path.file_stem()?.to_string_lossy().to_lowercase();
    let members = catalog.unit_members(photo.id).ok()?;
    let named = |suffix: &str| {
        let want = format!("{stem}{suffix}");
        members
            .iter()
            .find(|m| {
                !m.photo.missing
                    && m.photo.path.file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase() == want)
            })
            .map(|m| m.photo.path.clone())
    };
    Some(IrFiles { pass: named("_ir")?, valid: named("_ir_valid") })
}

/// Dust & Scratches: find the defects (`lum` is the full-size photo's lightness, `w × h`)
/// and keep them as a mask picture; returns its id.
#[allow(clippy::too_many_arguments)]
fn run_dust(
    photo_id: i64,
    lum: &[f32],
    w: usize,
    h: usize,
    layer: &imaging::develop::DustLayer,
    ir: Option<&IrFiles>,
    job: &AiJob,
    gpu: bool,
    ctx: &egui::Context,
) -> Result<(String, Option<String>)> {
    let started = std::time::Instant::now();
    let set = |share: f32, what: String| {
        *job.progress.lock().unwrap_or_else(|e| e.into_inner()) = (share, what);
        ctx.request_repaint();
    };
    let stamp = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    if layer.uses_ai() {
        // The model's map of this photo: kept from before (made for this photo, at this
        // size), else made now (and kept: slider moves only threshold it).
        let kept = layer
            .ai_map
            .as_deref()
            .filter(|id| id.starts_with(&format!("{photo_id}-dustmap-")))
            .and_then(imaging::develop::mask_picture)
            .filter(|m| m.width() as usize == w && m.height() as usize == h);
        let (map, new_map) = match kept {
            Some(m) => (m.as_raw().clone(), None),
            None => {
                let model = segment::AiModel::Scratches;
                if !segment::ready(&[model]) {
                    let mut last = std::time::Instant::now();
                    segment::ensure(&[model], &mut |share, what| {
                        if last.elapsed().as_millis() > 100 {
                            last = std::time::Instant::now();
                            set(share, format!("Downloading {what}"));
                        }
                    })?;
                }
                set(0.0, "Finding dust & scratches (AI)…".into());
                let (map, device) = segment::scratches(lum, w, h, AI_DUST_SCALE, gpu, &|share| {
                    set(share, "Finding dust & scratches (AI)…".into())
                })?;
                log::info!("Dust & Scratches: AI map on {} in {:.1?}", device.label(), started.elapsed());
                let id = format!("{photo_id}-dustmap-{:x}", stamp());
                let picture = image::GrayImage::from_raw(w as u32, h as u32, map).context("the map's size")?;
                imaging::develop::save_mask_picture(&id, &picture)?;
                (picture.into_raw(), Some(id))
            }
        };
        set(-1.0, "Finding dust & scratches…".into());
        let mask = imaging::develop::dust_from_map(&map, lum, w, h, layer);
        let picture = image::GrayImage::from_raw(w as u32, h as u32, mask).context("the mask's size")?;
        let id = format!("{photo_id}-dust-{:x}", stamp());
        imaging::develop::save_mask_picture(&id, &picture)?;
        return Ok((id, new_map));
    }
    set(-1.0, "Finding dust & scratches…".into());
    let ir = match ir.filter(|_| layer.source.uses_ir()) {
        Some(files) => {
            let pass = image::open(&files.pass)
                .with_context(|| format!("read the infrared pass {}", files.pass.display()))?
                .to_luma32f();
            anyhow::ensure!(
                pass.width() as usize == w && pass.height() as usize == h,
                "the infrared pass is {}×{}, the photo {w}×{h}",
                pass.width(),
                pass.height()
            );
            // Its valid mask only if it fits.
            let valid = files
                .valid
                .as_ref()
                .and_then(|p| image::open(p).ok())
                .map(|m| m.to_luma8())
                .filter(|m| m.width() as usize == w && m.height() as usize == h)
                .map(image::GrayImage::into_raw);
            Some(imaging::develop::IrPass { pixels: pass.into_raw(), valid })
        }
        None => None,
    };
    let read = started.elapsed();
    let mask = imaging::develop::detect_dust(lum, w, h, layer, ir.as_ref());
    let detected = started.elapsed();
    let found = mask.iter().filter(|v| **v > 0).count();
    let picture = image::GrayImage::from_raw(w as u32, h as u32, mask).context("the mask's size")?;
    let id = format!("{photo_id}-dust-{:x}", stamp());
    imaging::develop::save_mask_picture(&id, &picture)?;
    log::info!(
        "Dust & Scratches: {found} px from {} in {:.1?} (infrared read {:.1?}, found {:.1?}, saved {:.1?})",
        if ir.is_some() { layer.source.label() } else { "Picture" },
        started.elapsed(),
        read,
        detected - read,
        started.elapsed() - detected
    );
    Ok((id, None))
}

/// Distractions → People: find the people in `rgb` (the photo developed, turned upright
/// by `rotation`; `size` is its size in file orientation), keep each outline as a picture,
/// and return them in file orientation.
fn run_people(
    photo_id: i64,
    job: &AiJob,
    rgb: &image::RgbImage,
    size: (u32, u32),
    rotation: u8,
    gpu: bool,
    ctx: &egui::Context,
) -> Result<Vec<FoundPerson>> {
    use image::imageops::{rotate90, rotate180, rotate270};
    let set = |share: f32, what: String| {
        *job.progress.lock().unwrap_or_else(|e| e.into_inner()) = (share, what);
        ctx.request_repaint();
    };
    let models = [segment::AiModel::People, segment::AiModel::Object];
    if !segment::ready(&models) {
        let mut last = std::time::Instant::now();
        segment::ensure(&models, &mut |share, what| {
            if last.elapsed().as_millis() > 100 {
                last = std::time::Instant::now();
                set(share, format!("Downloading {what}"));
            }
        })?;
    }
    let started = std::time::Instant::now();
    let people = segment::find_people(rgb, 0.3, gpu, &|share, what| set(share, what.to_owned()))?;
    log::info!("People: {} found in {:.1?}", people.len(), started.elapsed());
    let (fw, fh) = (size.0 as f32, size.1 as f32);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let mut out = Vec::new();
    for (k, p) in people.into_iter().enumerate() {
        // Back to the file's orientation (as AI masks are kept).
        let [l, t, r, b] = p.rect.map(|v| v as f32);
        let (rect, mask) = match rotation % 4 {
            1 => ([t, fh - r, b, fh - l], rotate270(&p.mask)),
            2 => ([fw - r, fh - b, fw - l, fh - t], rotate180(&p.mask)),
            3 => ([fw - b, l, fw - t, r], rotate90(&p.mask)),
            _ => ([l, t, r, b], p.mask),
        };
        let id = format!("{photo_id}-person-{stamp:x}-{k}");
        imaging::develop::save_mask_picture(&id, &mask)?;
        out.push(FoundPerson {
            area: imaging::develop::SpotMask { id, rect: [rect[0] / fw, rect[1] / fh, rect[2] / fw, rect[3] / fh] },
            score: p.score,
            picked: p.score >= PEOPLE_SURE,
        });
    }
    Ok(out)
}

/// How much smaller the AI model sees the photo (film specks must stay a few pixels; a
/// third is twice as fast but finds less, measured on annotated film scans).
const AI_DUST_SCALE: f32 = 0.5;

/// Upright Guided: what a drag on the photo is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GuideDrag {
    /// Drawing a new guide from this screen point.
    New { start: egui::Pos2 },
    /// Moving one end (0 or 1) of a guide.
    End { index: usize, end: usize },
}

/// Where a full-resolution region is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RegionTarget {
    /// The center view zoomed to 1:1 (or more).
    Zoom,
    /// The Detail panel's 1:1 preview box.
    DetailBox,
}

enum Request {
    Load {
        photo_id: i64,
        path: PathBuf,
        kind: SourceKind,
        /// Long side of the proxy to work on.
        max_px: u32,
    },
    Render {
        photo_id: i64,
        slot: Slot,
        settings: DevelopSettings,
        /// Paint clipped pixels (J): highlights red, shadows blue.
        clipping: bool,
        /// A greyscale sharpening preview instead of the photo.
        view: Option<SharpenView>,
        seq: u64,
    },
    /// Render part of the full-size photo (1:1), loading it first if needed.
    Region { photo_id: i64, target: RegionTarget, view: RegionView, seq: u64 },
    /// Settings computed from the pixels.
    Adjust { photo_id: i64, op: Adjust, settings: DevelopSettings },
    /// A mask's values over the proxy, for the red overlay.
    Overlay { photo_id: i64, settings: DevelopSettings, index: usize, seq: u64 },
    /// Remove: fill spot `index` (its shape `key`) from the full-size photo.
    Fill {
        photo_id: i64,
        job: AiJob,
        index: usize,
        key: u64,
        settings: DevelopSettings,
        gpu: bool,
        best: bool,
    },
    /// Small pictures of every mask (the Masks panel's thumbnails).
    Thumbs { photo_id: i64, settings: DevelopSettings, seq: u64 },
    /// Make an AI mask of the photo developed with `settings`.
    Select { photo_id: i64, job: AiJob, settings: DevelopSettings, rotation: u8, gpu: bool },
    /// Dust & Scratches: find the defects for `layer` (its key `key`) in the full-size photo,
    /// with its infrared pass (and that pass's valid mask) if it has one.
    Dust { photo_id: i64, key: u64, layer: imaging::develop::DustLayer, ir: Option<IrFiles>, job: AiJob, gpu: bool },
    /// Dust & Scratches' found defects over the proxy, for the red overlay.
    DustOverlay { photo_id: i64, settings: DevelopSettings, seq: u64 },
    /// Distractions → People: find the people in the full-size photo developed with
    /// `settings` (turned `rotation` quarter turns to stand upright).
    FindPeople { photo_id: i64, job: AiJob, settings: DevelopSettings, rotation: u8, gpu: bool },
}

/// Remove: the fills made one after another (Remove People makes many at once).
pub struct FillBatch {
    /// Shared by each fill in turn; its progress reads "Removing 3 of 12…".
    pub job: AiJob,
    pub done: usize,
    pub total: usize,
}

/// A person found by Distractions → People: their outline (in the mask store), how sure
/// the detector is, and whether they are to be removed.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundPerson {
    pub area: imaging::develop::SpotMask,
    pub score: f32,
    pub picked: bool,
}

/// Found people the detector is less sure of start not picked (the user ticks them).
const PEOPLE_SURE: f32 = 0.5;

/// A scan's infrared pass and, if there is one, its valid mask (NegPy's `_IR`, `_IR_valid`).
#[derive(Debug, Clone, PartialEq)]
pub struct IrFiles {
    pub pass: PathBuf,
    pub valid: Option<PathBuf>,
}

/// What a full-resolution region render shows.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionView {
    pub settings: DevelopSettings,
    pub clipping: bool,
    pub view: Option<SharpenView>,
    /// Pixels of the full-size image, in file orientation.
    pub region: Region,
}

/// Changes worked out from the photo's pixels.
#[derive(Debug, Clone, Copy)]
enum Adjust {
    /// White Balance Selector: neutralize the point (x, y), 0..1 of the unrotated image.
    PickWb {
        x: f32,
        y: f32,
    },
    AutoTone,
    /// Upright: analyse the photo's lines (or use its guides) for this mode.
    Upright(Upright),
    /// Crop & Straighten's Auto (and whether to keep the crop on the photo).
    AutoStraighten {
        constrain: bool,
    },
    /// The Remove tool: find spot `index` a source (another one than now with `other`).
    FindSource {
        index: usize,
        other: bool,
    },
    /// Distractions: find sensor dust (as spots, in place of those found before).
    SensorDust {
        sensitivity: f32,
    },
}

impl Adjust {
    fn label(self) -> String {
        match self {
            Adjust::PickWb { .. } => "White Balance: Custom".into(),
            Adjust::AutoTone => "Auto Settings".into(),
            Adjust::Upright(mode) => format!("Upright: {}", mode.label()),
            Adjust::AutoStraighten { .. } => "Auto Straighten".into(),
            Adjust::FindSource { other: true, .. } => "Remove: New Source".into(),
            Adjust::FindSource { .. } => "Remove: Spot".into(),
            Adjust::SensorDust { .. } => "Find Sensor Dust".into(),
        }
    }
}

enum Reply {
    Loaded {
        photo_id: i64,
        info: SourceInfo,
    },
    Rendered {
        photo_id: i64,
        slot: Slot,
        seq: u64,
        image: egui::ColorImage,
        histogram: Box<Histogram>,
        wb: ResolvedWb,
        clipped: Clipped,
    },
    Region {
        photo_id: i64,
        target: RegionTarget,
        seq: u64,
        view: RegionView,
        image: egui::ColorImage,
    },
    Adjusted {
        photo_id: i64,
        op: Adjust,
        settings: Option<DevelopSettings>,
    },
    Overlay {
        photo_id: i64,
        seq: u64,
        image: egui::ColorImage,
    },
    Failed {
        photo_id: i64,
        error: String,
    },
    /// A Remove spot's fill (for the spot shape `key`), or why it failed.
    Filled {
        photo_id: i64,
        key: u64,
        result: std::result::Result<imaging::develop::Fill, String>,
    },
    /// The masks' thumbnails, in order.
    Thumbs {
        photo_id: i64,
        seq: u64,
        images: Vec<egui::ColorImage>,
    },
    /// An AI mask's picture (its id, and the device that made it), or why it failed.
    Selected {
        photo_id: i64,
        result: std::result::Result<(String, &'static str), String>,
    },
    /// The people found, or why finding failed.
    People {
        photo_id: i64,
        result: std::result::Result<Vec<FoundPerson>, String>,
    },
    /// Dust & Scratches' mask picture for the layer key `key` (and the AI model's map, if
    /// it made one), or why it failed.
    Dusted {
        photo_id: i64,
        key: u64,
        result: std::result::Result<(String, Option<String>), String>,
    },
}

/// Share of pixels clipped to black (all channels 0) and to white (any channel 255).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Clipped {
    pub shadows: f32,
    pub highlights: f32,
}

/// What the Basic panel needs to know about the photo's source.
#[derive(Debug, Clone)]
pub struct SourceInfo {
    /// Temp/Tint in Kelvin (RAW with a colour matrix), else relative.
    pub kelvin: bool,
    pub presets: &'static [WbPreset],
    /// The full-size image's size (file orientation).
    pub full_size: (u32, u32),
}

fn histogram(rgb: &[u8]) -> Box<Histogram> {
    let mut h = Box::new([[0u32; 256]; 3]);
    for px in rgb.as_chunks::<3>().0 {
        for c in 0..3 {
            h[c][px[c] as usize] += 1;
        }
    }
    h
}

/// Count clipped pixels; with `paint`, mark them as LrC does (blue / red).
fn clipping(rgb: &mut [u8], paint: bool) -> Clipped {
    let (mut dark, mut bright) = (0usize, 0usize);
    let px = rgb.as_chunks_mut::<3>().0;
    let n = px.len().max(1) as f32;
    for p in px {
        if p.iter().all(|v| *v == 0) {
            dark += 1;
            if paint {
                *p = [30, 90, 255];
            }
        } else if p.contains(&255) {
            bright += 1;
            if paint {
                *p = [255, 30, 30];
            }
        }
    }
    Clipped { shadows: dark as f32 / n, highlights: bright as f32 / n }
}

fn color_image(rgb: &image::RgbImage) -> egui::ColorImage {
    egui::ColorImage::from_rgb([rgb.width() as usize, rgb.height() as usize], rgb.as_raw())
}

/// A full-size image being loaded on its own thread (a RAW takes seconds), so renders of
/// the proxy carry on meanwhile.
type FullLoaded = (i64, Result<WorkingImage, String>);

/// Long side of the masks' thumbnails, in pixels.
const THUMB_PX: u32 = 96;

fn worker(rx: Receiver<Request>, tx: Sender<Reply>, ctx: egui::Context) {
    let mut current: Option<(i64, WorkingImage)> = None;
    // The current photo's file, the full-size image once loaded, and whether it's loading.
    let mut source: Option<(i64, PathBuf, SourceKind)> = None;
    let mut full: Option<(i64, WorkingImage)> = None;
    let mut full_loading: Option<i64> = None;
    let mut pending_regions: HashMap<RegionTarget, (i64, RegionView, u64)> = HashMap::new();
    let (full_tx, full_rx) = crossbeam_channel::unbounded::<FullLoaded>();
    // Requests wait here until their photo is loaded: a render may arrive before (or in
    // another batch than) the load it needs.
    let mut renders: HashMap<Slot, (i64, DevelopSettings, bool, Option<SharpenView>, u64)> = HashMap::new();
    let mut adjusts: Vec<(i64, Adjust, DevelopSettings)> = Vec::new();
    let mut overlay: Option<(i64, DevelopSettings, usize, u64)> = None;
    let mut selects: Vec<(i64, AiJob, DevelopSettings, u8, bool)> = Vec::new();
    let mut thumbs: Option<(i64, DevelopSettings, u64)> = None;
    let mut fills: Vec<(i64, AiJob, usize, u64, DevelopSettings, bool, bool)> = Vec::new();
    let mut dusts: Vec<(i64, u64, imaging::develop::DustLayer, Option<IrFiles>, AiJob, bool)> = Vec::new();
    let mut dust_overlay: Option<(i64, DevelopSettings, u64)> = None;
    let mut finds: Vec<(i64, AiJob, DevelopSettings, u8, bool)> = Vec::new();
    // The photo small, for thumbnails.
    let mut small: Option<(i64, WorkingImage)> = None;
    loop {
        let first = crossbeam_channel::select! {
            recv(rx) -> req => match req {
                Ok(req) => Some(req),
                Err(_) => return,
            },
            recv(full_rx) -> loaded => {
                if let Ok((photo_id, result)) = loaded {
                    full_loading = full_loading.filter(|id| *id != photo_id);
                    match result {
                        Ok(img) if source.as_ref().is_some_and(|s| s.0 == photo_id) => full = Some((photo_id, img)),
                        Ok(_) => {}
                        Err(error) => {
                            pending_regions.clear();
                            let _ = tx.send(Reply::Failed { photo_id, error });
                        }
                    }
                }
                None
            },
        };
        // Take everything queued: the last load, the last render per slot and region,
        // every adjustment.
        let mut load = None;
        for req in first.into_iter().chain(rx.try_iter()) {
            match req {
                Request::Load { photo_id, path, kind, max_px } => load = Some((photo_id, path, kind, max_px)),
                Request::Render { photo_id, slot, settings, clipping, view, seq } => {
                    renders.insert(slot, (photo_id, settings, clipping, view, seq));
                }
                Request::Region { photo_id, target, view, seq } => {
                    pending_regions.insert(target, (photo_id, view, seq));
                }
                Request::Adjust { photo_id, op, settings } => adjusts.push((photo_id, op, settings)),
                Request::Overlay { photo_id, settings, index, seq } => {
                    overlay = Some((photo_id, settings, index, seq))
                }
                Request::Thumbs { photo_id, settings, seq } => thumbs = Some((photo_id, settings, seq)),
                Request::Fill { photo_id, job, index, key, settings, gpu, best } => {
                    fills.push((photo_id, job, index, key, settings, gpu, best))
                }
                Request::Select { photo_id, job, settings, rotation, gpu } => {
                    selects.push((photo_id, job, settings, rotation, gpu))
                }
                Request::Dust { photo_id, key, layer, ir, job, gpu } => dusts.push((photo_id, key, layer, ir, job, gpu)),
                Request::DustOverlay { photo_id, settings, seq } => dust_overlay = Some((photo_id, settings, seq)),
                Request::FindPeople { photo_id, job, settings, rotation, gpu } => {
                    finds.push((photo_id, job, settings, rotation, gpu))
                }
            }
        }
        if let Some((photo_id, path, kind, max_px)) = load {
            if source.as_ref().is_none_or(|s| s.0 != photo_id) {
                // Another photo: forget this one's full-size image and pending regions.
                full = None;
                pending_regions.retain(|_, r| r.0 == photo_id);
            }
            source = Some((photo_id, path.clone(), kind));
            // A new photo, or this one at another size.
            let stale = current.as_ref().is_none_or(|(id, img)| {
                *id != photo_id
                    || img.width().max(img.height()) < max_px.min(img.full_size.0.max(img.full_size.1))
            });
            if stale {
                current = None;
                let started = std::time::Instant::now();
                match WorkingImage::load(&path, kind, Some(max_px)) {
                    Ok((img, _)) => {
                        log::debug!("develop: loaded {} in {:.0?}", path.display(), started.elapsed());
                        current = Some((photo_id, img));
                    }
                    Err(e) => {
                        let _ = tx.send(Reply::Failed { photo_id, error: format!("{e:#}") });
                    }
                }
            }
            if let Some((_, img)) = current.as_ref().filter(|(id, _)| *id == photo_id) {
                let info =
                    SourceInfo { kelvin: img.kelvin(), presets: img.wb_presets(), full_size: img.full_size };
                let _ = tx.send(Reply::Loaded { photo_id, info });
            }
        }
        // Only the photo being developed matters now.
        let wanted = source.as_ref().map(|s| s.0);
        renders.retain(|_, r| Some(r.0) == wanted);
        adjusts.retain(|a| Some(a.0) == wanted);
        overlay = overlay.filter(|o| Some(o.0) == wanted);
        selects.retain(|s| Some(s.0) == wanted);
        thumbs = thumbs.filter(|t| Some(t.0) == wanted);
        fills.retain(|f| Some(f.0) == wanted);
        dusts.retain(|d| Some(d.0) == wanted);
        dust_overlay = dust_overlay.filter(|o| Some(o.0) == wanted);
        finds.retain(|f| Some(f.0) == wanted);
        let Some((id, img)) = &current else {
            ctx.request_repaint();
            continue;
        };
        for slot in [Slot::Current, Slot::Before, Slot::Preview] {
            let Some((photo_id, settings, paint, view, seq)) =
                renders.get(&slot).is_some_and(|r| r.0 == *id).then(|| renders.remove(&slot)).flatten()
            else {
                continue;
            };
            let mut rgb = render_region_srgb8_view(img, &settings, Region::whole(img), view);
            let histogram = histogram(rgb.as_raw());
            let clipped = clipping(&mut rgb, paint && view.is_none());
            let wb = img.resolve_wb(&settings);
            let image = color_image(&rgb);
            let _ = tx.send(Reply::Rendered { photo_id, slot, seq, image, histogram, wb, clipped });
        }
        let (ready, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut adjusts).into_iter().partition(|a| a.0 == *id);
        adjusts = waiting;
        for (photo_id, op, settings) in ready {
            let settings = match op {
                Adjust::PickWb { x, y } => {
                    // The click is on the transformed photo: sample what is shown there.
                    let (x, y) = settings.transform.source_point(x, y, img.width(), img.height());
                    img.picked_multipliers(x, y).map(|m| img.settings_with_multipliers(&settings, m))
                }
                Adjust::AutoTone => Some(auto_tone(img, &settings)),
                Adjust::Upright(mode) => Some(apply_upright(img, &settings, mode)),
                Adjust::SensorDust { sensitivity } => {
                    // Found spots are found again (edited ones aren't marked found any more).
                    let mut s = settings.clone();
                    s.spots.retain(|spot| !spot.found);
                    let feather = RemoveSettings::default().feather;
                    s.spots.extend(imaging::develop::sensor_dust_spots(img, sensitivity, feather));
                    Some(s)
                }
                Adjust::FindSource { index, other } => settings.spots.get(index).cloned().map(|spot| {
                    let mut s = settings.clone();
                    s.spots[index].offset = find_source(img, &spot, other.then_some(spot.offset));
                    s
                }),
                Adjust::AutoStraighten { constrain } => {
                    let mut s = settings.clone();
                    s.transform.crop.angle = straighten_angle(img, &settings.transform);
                    if constrain {
                        let t =
                            imaging::develop::Transform { crop: Default::default(), ..s.transform.clone() };
                        let (fw, fh) = t.output_size(img.width(), img.height());
                        s.transform.crop.fit_inside(fw as f32 / fh.max(1) as f32);
                    }
                    Some(s)
                }
            };
            let _ = tx.send(Reply::Adjusted { photo_id, op, settings });
        }
        // AI masks: the photo as the model should see it (developed, but without geometry,
        // effects or masks, upright), then the model on a thread of its own (it takes a
        // second or more, longer when it first downloads).
        let (ready, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut selects).into_iter().partition(|s| s.0 == *id);
        selects = waiting;
        for (photo_id, job, mut settings, rotation, gpu) in ready {
            settings.transform = Default::default();
            settings.effects = Default::default();
            settings.masks.clear();
            // Select Object keeps the photo's encoding: this photo, rendered like this.
            let size =
                if rotation % 2 == 1 { (img.height(), img.width()) } else { (img.width(), img.height()) };
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (photo_id, rotation, size, settings.stable_hash()).hash(&mut h);
                h.finish()
            };
            let rgb = if job.kind == AiKind::Object && segment::object_encoded(key) {
                image::RgbImage::new(0, 0)
            } else {
                let rgb = render_region_srgb8_view(img, &settings, Region::whole(img), None);
                match rotation % 4 {
                    1 => image::imageops::rotate90(&rgb),
                    2 => image::imageops::rotate180(&rgb),
                    3 => image::imageops::rotate270(&rgb),
                    _ => rgb,
                }
            };
            let (tx, ctx) = (tx.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("ai-mask".into())
                .spawn(move || {
                    let result = run_ai(photo_id, &job, &rgb, size, key, rotation, gpu, &ctx)
                        .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Reply::Selected { photo_id, result });
                    ctx.request_repaint();
                })
                .expect("start an AI mask thread");
        }
        // The mask overlay, after the render it goes over.
        if let Some((photo_id, settings, index, seq)) = overlay.take_if(|o| o.0 == *id)
            && let Some(mask) = render_mask(img, &settings, index)
        {
            let (w, h) = (mask.width() as usize, mask.height() as usize);
            let rgba: Vec<u8> = mask.as_raw().iter().flat_map(|v| [255, 40, 40, v / 2]).collect();
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            let _ = tx.send(Reply::Overlay { photo_id, seq, image });
        }
        // Dust & Scratches' overlay: the found mask, as the photo is shown.
        if let Some((photo_id, settings, seq)) = dust_overlay.take_if(|o| o.0 == *id)
            && let Some(mask) = imaging::develop::render_dust_overlay(img, &settings)
        {
            let (w, h) = (mask.width() as usize, mask.height() as usize);
            let rgba: Vec<u8> = mask.as_raw().iter().flat_map(|v| [255, 40, 40, *v]).collect();
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            let _ = tx.send(Reply::Overlay { photo_id, seq, image });
        }
        // Dust & Scratches: from the full-size photo (loading it first) and its infrared
        // pass, on a thread of its own (seconds).
        let (ready, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut dusts).into_iter().partition(|d| full.as_ref().is_some_and(|(fid, _)| *fid == d.0));
        dusts = waiting;
        if let Some(d) = dusts.first() {
            load_full(&mut full_loading, &source, d.0, &full_tx, &ctx);
        }
        for (photo_id, key, layer, ir, job, gpu) in ready {
            let Some((_, full_img)) = &full else { continue };
            let (w, h) = (full_img.width() as usize, full_img.height() as usize);
            let started = std::time::Instant::now();
            let lum = imaging::develop::dust_lightness(full_img);
            log::debug!("Dust & Scratches: lightness in {:.1?}", started.elapsed());
            let (tx, ctx) = (tx.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("dust".into())
                .spawn(move || {
                    let result = run_dust(photo_id, &lum, w, h, &layer, ir.as_ref(), &job, gpu, &ctx)
                        .map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Reply::Dusted { photo_id, key, result });
                    ctx.request_repaint();
                })
                .expect("start a dust thread");
        }
        // People: the full-size photo developed as AI masks see it (no geometry, effects or
        // masks; upright), then the models on a thread of their own.
        let (ready, waiting): (Vec<_>, Vec<_>) =
            std::mem::take(&mut finds).into_iter().partition(|f| full.as_ref().is_some_and(|(fid, _)| *fid == f.0));
        finds = waiting;
        if let Some(f) = finds.first() {
            load_full(&mut full_loading, &source, f.0, &full_tx, &ctx);
        }
        for (photo_id, job, mut settings, rotation, gpu) in ready {
            let Some((_, full_img)) = &full else { continue };
            settings.transform = Default::default();
            settings.effects = Default::default();
            settings.masks.clear();
            let size = (full_img.width(), full_img.height());
            let rgb = render_region_srgb8_view(full_img, &settings, Region::whole(full_img), None);
            let rgb = match rotation % 4 {
                1 => image::imageops::rotate90(&rgb),
                2 => image::imageops::rotate180(&rgb),
                3 => image::imageops::rotate270(&rgb),
                _ => rgb,
            };
            let (tx, ctx) = (tx.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("ai-people".into())
                .spawn(move || {
                    let result =
                        run_people(photo_id, &job, &rgb, size, rotation, gpu, &ctx).map_err(|e| format!("{e:#}"));
                    let _ = tx.send(Reply::People { photo_id, result });
                    ctx.request_repaint();
                })
                .expect("start a People thread");
        }
        // Remove fills: from the full-size photo (loading it first), then the model on a
        // thread of its own.
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut fills)
            .into_iter()
            .partition(|f| full.as_ref().is_some_and(|(fid, _)| *fid == f.0));
        fills = waiting;
        if let Some(f) = fills.first() {
            load_full(&mut full_loading, &source, f.0, &full_tx, &ctx);
        }
        for (photo_id, job, index, key, settings, gpu, best) in ready {
            let Some((_, full_img)) = &full else { continue };
            let wb = full_img.resolve_wb(&settings).multipliers;
            let input =
                imaging::develop::fill_input(full_img, &settings.spots, settings.dust.as_ref(), index, wb);
            let (tx, ctx) = (tx.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("ai-remove".into())
                .spawn(move || {
                    let result = match input {
                        Some(input) => run_fill(photo_id, &job, input, key, best, gpu, &ctx),
                        None => Err(anyhow::anyhow!("the spot is outside the photo")),
                    };
                    let _ = tx.send(Reply::Filled {
                        photo_id,
                        key,
                        result: result.map_err(|e| format!("{e:#}")),
                    });
                    ctx.request_repaint();
                })
                .expect("start a Remove thread");
        }
        // The Masks panel's thumbnails: each mask white on black, from the photo made small
        // (its brush and AI rasters are cached with it).
        if let Some((photo_id, settings, seq)) = thumbs.take_if(|t| t.0 == *id) {
            if small.as_ref().is_none_or(|(sid, _)| *sid != photo_id) {
                small = Some((photo_id, img.downscaled(THUMB_PX)));
            }
            if let Some((_, little)) = &small {
                let images = (0..settings.masks.len())
                    .map(|i| {
                        let m = render_mask(little, &settings, i)
                            .unwrap_or_else(|| image::GrayImage::new(little.width(), little.height()));
                        let (w, h) = (m.width() as usize, m.height() as usize);
                        let rgb: Vec<u8> = m.as_raw().iter().flat_map(|v| [*v, *v, *v]).collect();
                        egui::ColorImage::from_rgb([w, h], &rgb)
                    })
                    .collect();
                let _ = tx.send(Reply::Thumbs { photo_id, seq, images });
            }
        }
        // 1:1: render the regions from the full-size image, loading that first.
        for target in [RegionTarget::DetailBox, RegionTarget::Zoom] {
            let Some((photo_id, view, seq)) = pending_regions
                .get(&target)
                .is_some_and(|r| r.0 == *id)
                .then(|| pending_regions.remove(&target))
                .flatten()
            else {
                continue;
            };
            match &full {
                Some((fid, full_img)) if *fid == photo_id => {
                    let started = std::time::Instant::now();
                    let mut rgb = render_region_srgb8_view(full_img, &view.settings, view.region, view.view);
                    clipping(&mut rgb, view.clipping && view.view.is_none());
                    log::debug!("develop: 1:1 {target:?} {:?} in {:.0?}", view.region, started.elapsed());
                    let image = color_image(&rgb);
                    let _ = tx.send(Reply::Region { photo_id, target, seq, view, image });
                }
                _ => {
                    load_full(&mut full_loading, &source, photo_id, &full_tx, &ctx);
                    pending_regions.insert(target, (photo_id, view, seq));
                }
            }
        }
        ctx.request_repaint();
    }
}

/// The Develop module's state for the open catalog.
pub struct DevelopState {
    tx: Sender<Request>,
    rx: Receiver<Reply>,
    /// The photo being developed.
    pub photo: Option<i64>,
    /// Settings as shown (changing live while a slider moves).
    pub settings: DevelopSettings,
    /// Settings as stored in the catalog.
    saved: DevelopSettings,
    pub info: Option<SourceInfo>,
    pub textures: HashMap<Slot, egui::TextureHandle>,
    pub histogram: Option<Box<Histogram>>,
    /// The white balance of the last render (what Temp/Tint show for As Shot, Auto…).
    pub wb: Option<ResolvedWb>,
    pub error: Option<String>,
    /// What was last sent per slot (settings, clipping, sharpening preview) and its
    /// sequence number; the numbers drop outdated renders.
    sent: HashMap<Slot, (DevelopSettings, bool, Option<SharpenView>, u64)>,
    seq: u64,
    shown: HashMap<Slot, u64>,
    /// `\`: show the photo before any adjustments.
    pub before: bool,
    /// W: the White Balance Selector is active (click the photo to pick).
    pub picking_wb: bool,
    /// The photo is a RAW: its defaults include some sharpening and noise reduction.
    raw: bool,
    /// Long side, in pixels, the center view shows the photo at (rounded up), and the
    /// proxy size the current photo was loaded at: renders cost in proportion.
    pub view_px: u32,
    loaded_px: u32,
    /// J: show clipped pixels; and how many there are in the current render.
    pub clipping: bool,
    pub clipped: Clipped,
    /// Full-resolution renders: of the visible part when zoomed to 1:1, and for the Detail
    /// panel's preview box.
    regions: HashMap<RegionTarget, RegionRender>,
    region_seq: u64,
    /// Alt or Shift while dragging a Sharpening slider: LrC's greyscale preview.
    pub sharpen_view: Option<SharpenView>,
    /// The Detail panel's preview box: open, the point it shows (0..1 of the photo in file
    /// orientation), and whether the next click on the photo picks that point.
    pub detail_box_open: bool,
    pub detail_focus: (f32, f32),
    pub picking_detail: bool,
    /// A Transform slider is being dragged: show a grid over the photo.
    pub transform_grid: bool,
    /// Masking mode (Shift+W), when on.
    pub masking: Option<Masking>,
    pub brush: BrushSettings,
    /// Crop & Straighten (R), when open; and its settings (kept while the app runs).
    pub cropping: Option<CropTool>,
    /// The crop keeps its width / height (the Aspect lock).
    pub crop_lock: bool,
    /// Constrain to Image: the crop never shows past the photo's edge.
    pub crop_constrain: bool,
    /// The Remove tool (Q), when open, and its settings for new spots.
    pub removing: Option<RemoveTool>,
    pub remove: RemoveSettings,
    /// Visualize Spots (A, with the Remove tool open) and its threshold (0..100).
    pub visualize_spots: bool,
    pub spot_threshold: u8,
    /// The mask overlay: what was last asked for, the newest shown, and its texture.
    overlay_sent: Option<(DevelopSettings, usize)>,
    overlay_seq: u64,
    overlay_shown: u64,
    pub overlay: Option<egui::TextureHandle>,
    /// The masks' thumbnails (in order), and what they were last asked for.
    pub mask_thumbs: Vec<egui::TextureHandle>,
    thumbs_sent: Option<Vec<imaging::develop::Mask>>,
    thumbs_seq: u64,
    thumbs_shown: u64,
    /// Upright Guided: the guide being drawn or moved on the photo.
    pub guide_drag: Option<GuideDrag>,
    /// AI masks: run on the GPU (DirectML) when possible; the mask being made; and one
    /// waiting for the user to agree to download what it needs (with its size).
    pub ai_gpu: bool,
    pub ai_job: Option<AiJob>,
    pub ai_consent: Option<AiConsent>,
    /// Remove: the fill being made (and the spot shape it is for), shapes that failed
    /// (not tried again), and whether the user agreed to (or declined) the download.
    pub fill_job: Option<(AiJob, u64)>,
    /// Remove: the fills made one after another until none is left (one progress for
    /// them all, so the panel doesn't flash a message per spot).
    pub fill_batch: Option<FillBatch>,
    fill_failed: Vec<u64>,
    pub fill_allowed: bool,
    pub fill_declined: bool,
    /// Remove at best quality (big-LaMa) instead of fast (LaMa-Dilated).
    pub remove_best: bool,
    /// Distractions: the photo's infrared pass, if it is a scan that has one.
    pub ir_pass: Option<IrFiles>,
    /// Dust & Scratches: the detection running (for this layer key), and keys that failed
    /// (not tried again).
    pub dust_job: Option<(u64, AiJob)>,
    dust_failed: Vec<u64>,
    /// The user agreed to (or declined) the AI model's download for Dust & Scratches.
    dust_ai_allowed: bool,
    /// The user agreed to the People models' download.
    people_allowed: bool,
    /// Show Dust & Scratches' found defects in red.
    pub show_dust: bool,
    /// Sensor dust: Find's sensitivity (0..100).
    pub sensor_sensitivity: f32,
    /// Distractions → People: the search running, and the people it found (until removed
    /// or searched again); `people_searched` once a search has answered.
    pub people_job: Option<AiJob>,
    pub people: Vec<FoundPerson>,
    pub people_searched: bool,
    /// The found people's tints on the photo (by mask id), and the one the pointer is
    /// over in the panel's list (outlined boldly on the photo).
    pub people_tex: Vec<(String, egui::TextureHandle)>,
    pub people_hover: Option<usize>,
    /// Sensor dust: Find is running; what it last found (shown in the panel).
    pub sensor_finding: bool,
    pub sensor_result: Option<String>,
    /// Select Object draws a rectangle (else it brushes over the object).
    pub object_rect: bool,
    /// The photo's catalog rotation (clockwise quarter turns): AI models see it upright.
    rotation: u8,
    /// When the shown settings last changed (regions wait until they settle).
    changed_at: std::time::Instant,
    /// Copy Settings / Paste Settings.
    /// Copied settings and the groups Paste sets (LrC's Copy Settings dialog).
    pub clipboard: Option<(DevelopSettings, Vec<imaging::develop::SettingsGroup>)>,
    /// The photo's Snapshots (named settings), by name.
    pub snapshots: Vec<catalog::Snapshot>,
    /// The settings of the preset or snapshot under the pointer: the Navigator shows them.
    pub preview: Option<DevelopSettings>,
    /// Tone Curve panel: point curve (else parametric), the channel edited, and the
    /// point being dragged.
    pub curve_point_mode: bool,
    pub curve_channel: imaging::develop::CurveChannel,
    pub curve_drag: Option<usize>,
    /// HSL / Color panel: the group shown (`None`: All).
    pub hsl_tab: Option<imaging::develop::HslGroup>,
    /// Color Grading panel: one wheel, or `None` for the 3-way view.
    pub grading_view: Option<imaging::develop::GradeWheel>,
    pub history: Vec<HistoryStep>,
    /// Steps undone (Ctrl+Z, or clicking an earlier History step), newest last.
    redo: Vec<(String, Option<String>)>,
}

/// A full-resolution region render: the newest one shown (and what it shows), the one last
/// asked for, and their sequence numbers.
#[derive(Default)]
struct RegionRender {
    shown: Option<(RegionView, egui::TextureHandle)>,
    sent: Option<RegionView>,
    sent_seq: u64,
    shown_seq: u64,
}

/// Which photos' catalog settings a call changed (for sidecars and filters).
pub type Changed = Option<i64>;

/// A change to a Snapshot from its menu.
#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotEdit {
    Rename(String),
    Update,
    Delete,
}

impl DevelopState {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, worker_rx) = crossbeam_channel::unbounded();
        let (worker_tx, rx) = crossbeam_channel::unbounded();
        let ctx2 = ctx.clone();
        std::thread::Builder::new()
            .name("develop".into())
            .spawn(move || worker(worker_rx, worker_tx, ctx2))
            .expect("start the develop thread");
        DevelopState {
            tx,
            rx,
            photo: None,
            settings: DevelopSettings::default(),
            saved: DevelopSettings::default(),
            info: None,
            textures: HashMap::new(),
            histogram: None,
            wb: None,
            error: None,
            sent: HashMap::new(),
            seq: 0,
            shown: HashMap::new(),
            before: false,
            picking_wb: false,
            raw: false,
            view_px: 1536,
            loaded_px: 0,
            clipping: false,
            clipped: Clipped::default(),
            regions: HashMap::new(),
            region_seq: 0,
            sharpen_view: None,
            detail_box_open: true,
            detail_focus: (0.5, 0.5),
            picking_detail: false,
            transform_grid: false,
            masking: None,
            brush: BrushSettings::default(),
            cropping: None,
            crop_lock: true,
            crop_constrain: true,
            removing: None,
            remove: RemoveSettings::default(),
            visualize_spots: false,
            spot_threshold: 50,
            overlay_sent: None,
            overlay_seq: 0,
            overlay_shown: 0,
            overlay: None,
            mask_thumbs: Vec::new(),
            thumbs_sent: None,
            thumbs_seq: 0,
            thumbs_shown: 0,
            guide_drag: None,
            ai_gpu: true,
            ai_job: None,
            ai_consent: None,
            fill_job: None,
            fill_batch: None,
            fill_failed: Vec::new(),
            fill_allowed: false,
            fill_declined: false,
            remove_best: true,
            ir_pass: None,
            dust_job: None,
            dust_failed: Vec::new(),
            dust_ai_allowed: false,
            people_allowed: false,
            show_dust: false,
            sensor_sensitivity: 50.0,
            people_job: None,
            people: Vec::new(),
            people_searched: false,
            people_tex: Vec::new(),
            people_hover: None,
            sensor_finding: false,
            sensor_result: None,
            object_rect: false,
            rotation: 0,
            changed_at: std::time::Instant::now(),
            clipboard: None,
            snapshots: Vec::new(),
            preview: None,
            curve_point_mode: false,
            curve_channel: imaging::develop::CurveChannel::Rgb,
            curve_drag: None,
            hsl_tab: None,
            grading_view: None,
            history: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// The photo's catalog rotation (clockwise quarter turns).
    pub fn rotation(&self) -> u8 {
        self.rotation
    }

    pub fn kelvin(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.kelvin)
    }

    /// Develop `photo` (if it isn't already): save pending changes of the previous one,
    /// read this one's settings and start loading its pixels.
    pub fn show(&mut self, catalog: &mut Catalog, photo: &PhotoRow) -> Result<Changed> {
        if self.photo == Some(photo.id) {
            return Ok(None);
        }
        let changed = self.commit(catalog, None)?;
        self.photo = Some(photo.id);
        self.raw = photo.kind == FileKind::Raw;
        self.rotation = photo.rotation;
        self.ai_job = None;
        self.ai_consent = None;
        self.fill_job = None;
        self.fill_batch = None;
        self.fill_failed.clear();
        self.dust_job = None;
        self.dust_failed.clear();
        self.sensor_finding = false;
        self.sensor_result = None;
        self.people_job = None;
        self.people.clear();
        self.people_searched = false;
        self.people_tex.clear();
        self.ir_pass = find_ir_pass(catalog, photo);
        self.info = None;
        self.textures.clear();
        self.regions.clear();
        self.detail_focus = (0.5, 0.5);
        self.picking_detail = false;
        self.guide_drag = None;
        self.masking = None;
        self.cropping = None;
        self.removing = None;
        self.overlay_sent = None;
        self.overlay = None;
        self.mask_thumbs.clear();
        self.thumbs_sent = None;
        self.histogram = None;
        self.wb = None;
        self.error = None;
        self.sent.clear();
        self.shown.clear();
        self.picking_wb = false;
        self.redo.clear();
        self.preview = None;
        // Load first: the renders asked for next need the photo.
        if photo.missing {
            self.error = Some("The file could not be found.".into());
        } else {
            let kind = if photo.kind == FileKind::Raw { SourceKind::Raw } else { SourceKind::Image };
            self.loaded_px = self.view_px;
            let max_px = self.view_px;
            let _ =
                self.tx.send(Request::Load { photo_id: photo.id, path: photo.path.clone(), kind, max_px });
        }
        self.adopt(catalog.develop_settings(photo.id)?.as_deref());
        self.history = catalog.develop_history(photo.id)?;
        self.snapshots = catalog.snapshots(photo.id)?;
        Ok(changed)
    }

    /// The center view shows the photo `px` pixels long (on its long side). Renders are
    /// made at about that size; if the view grew well past the loaded proxy (a bigger
    /// window), the photo is loaded again, larger.
    pub fn set_view_px(&mut self, px: u32, photo: &PhotoRow) {
        // Round up so small window changes don't reload, and keep a sensible minimum.
        let px = px.div_ceil(256).max(4) * 256;
        self.view_px = px;
        if self.photo == Some(photo.id) && self.loaded_px != 0 && px > self.loaded_px && !photo.missing {
            self.loaded_px = px;
            let kind = if photo.kind == FileKind::Raw { SourceKind::Raw } else { SourceKind::Image };
            let _ = self.tx.send(Request::Load {
                photo_id: photo.id,
                path: photo.path.clone(),
                kind,
                max_px: px,
            });
            self.sent.clear();
            self.request_renders();
        }
    }

    /// The photo's settings were changed in the catalog by something else (a sidecar was
    /// read): show them.
    pub fn external_change(&mut self, json: Option<&str>, history: Vec<HistoryStep>) {
        self.adopt(json);
        self.history = history;
        self.redo.clear();
    }

    /// A new photo's settings: LrC's defaults for its file type (RAWs get some sharpening
    /// and colour noise reduction).
    /// Is the photo a RAW file (its new settings have some sharpening)?
    pub fn is_raw(&self) -> bool {
        self.raw
    }

    /// Keep the current settings as a Snapshot named `name`.
    pub fn take_snapshot(&mut self, catalog: &mut Catalog, name: &str) -> Result<Changed> {
        let Some(id) = self.photo else { return Ok(None) };
        let changed = self.commit(catalog, None)?;
        let json = (!self.settings.is_default_for(self.raw)).then(|| self.settings.to_json());
        catalog.add_snapshot(id, name, json.as_deref())?;
        self.snapshots = catalog.snapshots(id)?;
        Ok(changed)
    }

    /// A Snapshot's settings become the photo's (a History step).
    pub fn apply_snapshot(&mut self, catalog: &mut Catalog, snapshot: i64) -> Result<Changed> {
        let Some(s) = self.snapshots.iter().find(|s| s.id == snapshot).cloned() else { return Ok(None) };
        let settings = s.settings.as_deref().and_then(DevelopSettings::from_json).unwrap_or_else(|| self.defaults());
        self.apply(catalog, settings, &format!("Snapshot: {}", s.name))
    }

    /// Rename, update with the current settings, or delete a Snapshot.
    pub fn edit_snapshot(&mut self, catalog: &mut Catalog, snapshot: i64, edit: SnapshotEdit) -> Result<Changed> {
        let Some(id) = self.photo else { return Ok(None) };
        let mut changed = None;
        match edit {
            SnapshotEdit::Rename(name) => catalog.rename_snapshot(snapshot, &name)?,
            SnapshotEdit::Update => {
                changed = self.commit(catalog, None)?;
                let json = (!self.settings.is_default_for(self.raw)).then(|| self.settings.to_json());
                catalog.update_snapshot(snapshot, json.as_deref())?;
            }
            SnapshotEdit::Delete => catalog.delete_snapshot(snapshot)?,
        }
        self.snapshots = catalog.snapshots(id)?;
        Ok(changed)
    }

    pub fn defaults(&self) -> DevelopSettings {
        DevelopSettings::default_for(self.raw)
    }

    /// Take stored settings (JSON) as both shown and saved.
    fn adopt(&mut self, json: Option<&str>) {
        let s = json.and_then(DevelopSettings::from_json).unwrap_or_else(|| self.defaults());
        self.settings = s.clone();
        self.saved = s;
        self.request_renders();
        self.request_dust();
    }

    /// Ask for renders of whatever changed since the last request.
    pub fn request_renders(&mut self) {
        let Some(photo_id) = self.photo else { return };
        let clipping = self.clipping;
        let mut wanted = vec![(Slot::Current, self.view_settings(), self.current_view())];
        if self.before {
            wanted.push((Slot::Before, self.defaults(), None));
        }
        if let Some(p) = &self.preview {
            wanted.push((Slot::Preview, p.clone(), None));
        }
        for (slot, settings, view) in wanted {
            // The Navigator's preview without clipping colours.
            let clipping = clipping && slot != Slot::Preview;
            if self.sent.get(&slot).is_some_and(|(s, c, v, _)| *s == settings && *c == clipping && *v == view)
            {
                continue;
            }
            self.seq += 1;
            self.sent.insert(slot, (settings.clone(), clipping, view, self.seq));
            let _ = self.tx.send(Request::Render { photo_id, slot, settings, clipping, view, seq: self.seq });
        }
        self.request_fills();
        // The Masks panel's thumbnails, while it is open and the masks changed.
        if self.masking.is_some() && self.thumbs_sent.as_ref() != Some(&self.settings.masks) {
            self.thumbs_seq += 1;
            self.thumbs_sent = Some(self.settings.masks.clone());
            let settings = self.settings.clone();
            let _ = self.tx.send(Request::Thumbs { photo_id, settings, seq: self.thumbs_seq });
        }
        // The mask overlay (masking with O on, or a mask hovered in the Masks panel), or
        // Dust & Scratches' found defects (the Remove tool with Show found on).
        let dust_overlay = self.removing.is_some()
            && (self.show_dust || self.removing.as_ref().is_some_and(|t| t.subtract))
            && !self.before
            && self.settings.dust.as_ref().is_some_and(|l| l.current().is_some());
        match self.masking.as_ref().and_then(Masking::overlay_mask).filter(|_| !self.before) {
            Some(index) => {
                let want = (self.settings.clone(), index);
                if self.overlay_sent.as_ref() != Some(&want) {
                    self.overlay_seq += 1;
                    let settings = want.0.clone();
                    let _ =
                        self.tx.send(Request::Overlay { photo_id, settings, index, seq: self.overlay_seq });
                    self.overlay_sent = Some(want);
                }
            }
            None if dust_overlay => {
                let want = (self.view_settings(), DUST_OVERLAY);
                if self.overlay_sent.as_ref() != Some(&want) {
                    self.overlay_seq += 1;
                    let settings = want.0.clone();
                    let _ = self.tx.send(Request::DustOverlay { photo_id, settings, seq: self.overlay_seq });
                    self.overlay_sent = Some(want);
                }
            }
            None => {
                self.overlay_sent = None;
                self.overlay = None;
            }
        }
    }

    /// Preview settings in the Navigator (a hovered preset or snapshot), or stop.
    pub fn set_preview(&mut self, settings: Option<DevelopSettings>) {
        if settings != self.preview {
            self.preview = settings;
            self.request_renders();
        }
    }

    /// The Navigator's preview, once it has been rendered for the settings hovered now.
    pub fn preview_texture(&self) -> Option<&egui::TextureHandle> {
        self.preview.as_ref()?;
        let sent = self.sent.get(&Slot::Preview)?.3;
        (self.shown.get(&Slot::Preview).copied().unwrap_or(0) >= sent).then(|| self.textures.get(&Slot::Preview))?
    }

    /// Is a render of newer settings still on its way?
    pub fn rendering(&self) -> bool {
        self.sent
            .get(&Slot::Current)
            .is_some_and(|(.., sent)| self.shown.get(&Slot::Current).copied().unwrap_or(0) < *sent)
    }

    /// Handle the worker's replies. Adjustments (picked white balance, Auto) are saved
    /// right away.
    pub fn poll(&mut self, ctx: &egui::Context, catalog: &mut Catalog) -> Result<Changed> {
        let mut changed = None;
        for reply in self.rx.try_iter().collect::<Vec<_>>() {
            match reply {
                Reply::Loaded { photo_id, info } if Some(photo_id) == self.photo => self.info = Some(info),
                Reply::Rendered { photo_id, slot, seq, image, histogram, wb, clipped }
                    if Some(photo_id) == self.photo =>
                {
                    if self.shown.get(&slot).is_some_and(|s| *s > seq) {
                        continue;
                    }
                    self.shown.insert(slot, seq);
                    let opts = egui::TextureOptions::LINEAR;
                    match self.textures.get_mut(&slot) {
                        Some(t) if t.size() == image.size => t.set(image, opts),
                        _ => {
                            self.textures
                                .insert(slot, ctx.load_texture(format!("develop_{slot:?}"), image, opts));
                        }
                    }
                    // A sharpening preview's grey says nothing about the photo's histogram.
                    if slot == Slot::Current && self.current_view().is_none() {
                        self.histogram = Some(histogram);
                        self.wb = Some(wb);
                        self.clipped = clipped;
                    }
                }
                Reply::Adjusted { photo_id, op, settings } if Some(photo_id) == self.photo => {
                    match settings {
                        Some(s) if matches!(op, Adjust::SensorDust { .. }) => {
                            // Say what Find found, even nothing (else it seems not to work).
                            self.sensor_finding = false;
                            let n = s.spots.iter().filter(|s| s.found).count();
                            log::info!("Sensor dust: {n} found");
                            self.sensor_result = Some(match n {
                                0 => "No sensor dust found: try a higher Sensitivity".into(),
                                1 => "Found 1 spot".into(),
                                n => format!("Found {n} spots"),
                            });
                            self.set_live(s);
                            let label = format!("Sensor Dust: {n} spot{}", if n == 1 { "" } else { "s" });
                            changed = changed.or(self.commit(catalog, Some(label))?);
                        }
                        Some(s) => {
                            self.set_live(s);
                            changed = changed.or(self.commit(catalog, Some(op.label()))?);
                        }
                        None => self.error = Some("Pick a neutral, unclipped area".into()),
                    }
                }
                Reply::Region { photo_id, target, seq, view, image } if Some(photo_id) == self.photo => {
                    let r = self.regions.entry(target).or_default();
                    if seq < r.shown_seq {
                        continue;
                    }
                    r.shown_seq = seq;
                    // Nearest: these are shown pixel for pixel (or enlarged, as pixels).
                    let opts = if target == RegionTarget::DetailBox {
                        egui::TextureOptions::NEAREST
                    } else {
                        egui::TextureOptions::LINEAR
                    };
                    match &mut r.shown {
                        Some((v, t)) if t.size() == image.size => {
                            t.set(image, opts);
                            *v = view;
                        }
                        _ => {
                            let name = format!("develop_region_{target:?}");
                            r.shown = Some((view, ctx.load_texture(name, image, opts)));
                        }
                    }
                }
                Reply::Overlay { photo_id, seq, image } if Some(photo_id) == self.photo => {
                    if seq < self.overlay_shown || self.overlay_sent.is_none() {
                        continue;
                    }
                    self.overlay_shown = seq;
                    let opts = egui::TextureOptions::LINEAR;
                    match &mut self.overlay {
                        Some(t) if t.size() == image.size => t.set(image, opts),
                        _ => self.overlay = Some(ctx.load_texture("develop_mask_overlay", image, opts)),
                    }
                }
                Reply::Filled { photo_id, key, result } if Some(photo_id) == self.photo => {
                    if self.fill_job.as_ref().is_some_and(|(_, k)| *k == key) {
                        self.fill_job = None;
                        if let Some(b) = &mut self.fill_batch {
                            b.done += 1;
                        }
                    }
                    match result {
                        Ok(fill) => changed = changed.or(self.add_fill(catalog, key, fill)?),
                        Err(e) => {
                            log::warn!("Remove: {e}");
                            self.fill_failed.push(key);
                            self.error = Some(format!("Couldn't remove: {e}"));
                        }
                    }
                    self.request_renders();
                }
                Reply::Thumbs { photo_id, seq, images } if Some(photo_id) == self.photo => {
                    if seq < self.thumbs_shown {
                        continue;
                    }
                    self.thumbs_shown = seq;
                    let opts = egui::TextureOptions::LINEAR;
                    self.mask_thumbs.truncate(images.len());
                    for (k, image) in images.into_iter().enumerate() {
                        match self.mask_thumbs.get_mut(k) {
                            Some(t) if t.size() == image.size => t.set(image, opts),
                            Some(t) => *t = ctx.load_texture(format!("mask_thumb_{k}"), image, opts),
                            None => self.mask_thumbs.push(ctx.load_texture(
                                format!("mask_thumb_{k}"),
                                image,
                                opts,
                            )),
                        }
                    }
                }
                Reply::Failed { photo_id, error } if Some(photo_id) == self.photo => {
                    log::warn!("develop: {error}");
                    self.error = Some(error);
                }
                Reply::People { photo_id, result } if Some(photo_id) == self.photo => {
                    self.people_job = None;
                    self.people_searched = true;
                    match result {
                        Ok(people) => self.people = people,
                        Err(e) => {
                            log::warn!("People: {e}");
                            self.error = Some(format!("Couldn't find people: {e}"));
                        }
                    }
                }
                Reply::Dusted { photo_id, key, result } if Some(photo_id) == self.photo => {
                    self.dust_job = None;
                    match result {
                        Ok((id, map)) => {
                            // Still the settings it was made for: keep it, and save (with
                            // the slider moves that asked for it). A new AI map is kept
                            // whatever the settings: it is of the photo.
                            let mut st = self.settings.clone();
                            let mut current = false;
                            if let Some(layer) = st.dust.as_mut() {
                                if map.is_some() {
                                    layer.ai_map = map;
                                }
                                if layer.key() == key {
                                    layer.picture = Some(id);
                                    layer.made_for = key;
                                    current = true;
                                }
                            }
                            self.set_live(st);
                            if current {
                                changed = changed.or(self.commit(catalog, Some("Dust & Scratches".into()))?);
                            }
                        }
                        Err(e) => {
                            log::warn!("Dust & Scratches: {e}");
                            self.dust_failed.push(key);
                            self.error = Some(format!("Couldn't find dust: {e}"));
                        }
                    }
                    // The settings may have moved on meanwhile.
                    self.request_dust();
                    self.request_renders();
                }
                Reply::Selected { photo_id, result } if Some(photo_id) == self.photo => {
                    let Some(job) = self.ai_job.take() else { continue };
                    match result {
                        Ok((id, _device)) => {
                            changed = changed.or(self.add_ai_mask(catalog, &job, id)?);
                        }
                        Err(e) => {
                            log::warn!("AI mask: {e}");
                            self.error =
                                Some(format!("Couldn't select the {}: {e}", job.kind.label().to_lowercase()));
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(changed)
    }

    /// Remove: start filling the first Remove spot that needs a fill (one at a time),
    /// asking before the model's first download.
    fn request_fills(&mut self) {
        let Some(photo_id) = self.photo else { return };
        if self.fill_job.is_some() {
            return;
        }
        if self.ai_consent.is_some() || self.fill_declined {
            self.fill_batch = None;
            return;
        }
        let pending: Vec<(usize, u64)> = self
            .settings
            .spots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.needs_fill() && !self.fill_failed.contains(&s.shape_key()))
            .map(|(i, s)| (i, s.shape_key()))
            .collect();
        let Some(&(index, key)) = pending.first() else {
            self.fill_batch = None;
            return;
        };
        let model = if self.remove_best { segment::AiModel::RemoveBest } else { segment::AiModel::Remove };
        if !self.fill_allowed && !segment::ready(&[model]) {
            let bytes = segment::download_size(&[model]);
            self.ai_consent =
                Some(AiConsent { kind: AiKind::Remove, target: ToolTarget::NewMask, prompt: None, bytes });
            return;
        }
        // One job for the whole batch: its progress is how many are done.
        let batch = self.fill_batch.get_or_insert_with(|| FillBatch {
            job: AiJob { kind: AiKind::Remove, target: ToolTarget::NewMask, prompt: None, progress: Default::default() },
            done: 0,
            total: 0,
        });
        batch.total = batch.done + pending.len();
        let quality = if self.remove_best { " (best quality)" } else { "" };
        *batch.job.progress.lock().unwrap_or_else(|e| e.into_inner()) = if batch.total > 1 {
            let share = batch.done as f32 / batch.total as f32;
            (share, format!("Removing {} of {}{quality}…", batch.done + 1, batch.total))
        } else {
            (-1.0, format!("Removing{quality}…"))
        };
        let job = batch.job.clone();
        let settings = self.settings.clone();
        let (gpu, best) = (self.ai_gpu, self.remove_best);
        let _ = self.tx.send(Request::Fill { photo_id, job: job.clone(), index, key, settings, gpu, best });
        self.fill_job = Some((job, key));
    }

    /// Dust & Scratches: find the defects if the layer is on and its mask is missing or
    /// was made for other settings (one detection at a time; the answer asks again).
    pub fn request_dust(&mut self) {
        let Some(photo_id) = self.photo else { return };
        let Some(layer) = self.settings.dust.clone().filter(|l| l.needs_detection()) else { return };
        let key = layer.key();
        if self.dust_job.is_some() || self.dust_failed.contains(&key) || self.ai_consent.is_some() {
            return;
        }
        // The AI model: ask before its first download.
        let model = segment::AiModel::Scratches;
        if layer.uses_ai() && !self.dust_ai_allowed && !segment::ready(&[model]) {
            let bytes = segment::download_size(&[model]);
            self.ai_consent = Some(AiConsent { kind: AiKind::Dust, target: ToolTarget::NewMask, prompt: None, bytes });
            return;
        }
        let job = AiJob { kind: AiKind::Dust, target: ToolTarget::NewMask, prompt: None, progress: Default::default() };
        *job.progress.lock().unwrap_or_else(|e| e.into_inner()) = (-1.0, "Finding dust & scratches…".into());
        let ir = self.ir_pass.clone();
        let gpu = self.ai_gpu;
        let _ = self.tx.send(Request::Dust { photo_id, key, layer, ir, job: job.clone(), gpu });
        self.dust_job = Some((key, job));
    }

    /// Distractions → People: find the people (asking before the models' first download).
    pub fn find_people(&mut self) {
        let Some(photo_id) = self.photo else { return };
        if self.people_job.is_some() {
            return;
        }
        let models = [segment::AiModel::People, segment::AiModel::Object];
        if !self.people_allowed && !segment::ready(&models) {
            let bytes = segment::download_size(&models);
            self.ai_consent =
                Some(AiConsent { kind: AiKind::People, target: ToolTarget::NewMask, prompt: None, bytes });
            return;
        }
        let job = AiJob { kind: AiKind::People, target: ToolTarget::NewMask, prompt: None, progress: Default::default() };
        *job.progress.lock().unwrap_or_else(|e| e.into_inner()) = (-1.0, "Finding people…".into());
        let (settings, rotation, gpu) = (self.settings.clone(), self.rotation, self.ai_gpu);
        let _ = self.tx.send(Request::FindPeople { photo_id, job: job.clone(), settings, rotation, gpu });
        self.people_job = Some(job);
        self.people.clear();
        self.people_searched = false;
    }

    /// Remove the picked people: a Remove spot each (filled one by one, as other Remove
    /// spots). Returns how many.
    pub fn remove_people(&mut self) -> usize {
        let picked: Vec<FoundPerson> = self.people.iter().filter(|p| p.picked).cloned().collect();
        if picked.is_empty() {
            return 0;
        }
        let (w, h) = self.info.as_ref().map_or((1, 1), |i| i.full_size);
        let half = w.max(h) as f32 / 2.0;
        let mut st = self.settings.clone();
        for p in &picked {
            let [l, t, r, b] = p.area.rect;
            let radius = ((r - l) * w as f32).max((b - t) * h as f32) / 2.0 / half;
            st.spots.push(imaging::develop::Spot {
                mode: imaging::develop::SpotMode::Remove,
                points: vec![[(l + r) / 2.0, (t + b) / 2.0]],
                size: radius,
                feather: 10.0,
                opacity: 100.0,
                offset: [0.0, 0.0],
                fill: None,
                strokes: Vec::new(),
                found: false,
                area: Some(p.area.clone()),
            });
        }
        self.people.clear();
        self.set_live(st);
        picked.len()
    }

    /// Dust & Scratches' Recalculate: find the defects again with the current settings
    /// (also after a failure). The AI model's map of the photo is kept: it would come out
    /// the same.
    pub fn recalculate_dust(&mut self) {
        let mut st = self.settings.clone();
        let Some(layer) = st.dust.as_mut().filter(|l| l.enabled) else { return };
        let key = layer.key();
        (layer.picture, layer.made_for) = (None, 0);
        self.dust_failed.retain(|k| *k != key);
        self.error = None;
        self.set_live(st);
        self.request_dust();
    }

    /// The AI model's download for Dust & Scratches was declined: find without it.
    pub fn decline_dust_ai(&mut self) {
        self.ai_consent = None;
        let mut st = self.settings.clone();
        if let Some(l) = st.dust.as_mut() {
            l.ai = false;
        }
        self.set_live(st);
        self.request_dust();
    }

    /// Sensor dust: find it (on the worker) as Heal spots, in place of those found before;
    /// saved when it answers.
    pub fn find_sensor_dust(&mut self) {
        let Some(photo_id) = self.photo else { return };
        let op = Adjust::SensorDust { sensitivity: self.sensor_sensitivity };
        let _ = self.tx.send(Request::Adjust { photo_id, op, settings: self.settings.clone() });
        self.sensor_finding = true;
        self.sensor_result = None;
    }

    /// A fill arrived: put it on the spot it was made for (if it is still that shape) and
    /// save.
    fn add_fill(&mut self, catalog: &mut Catalog, key: u64, fill: imaging::develop::Fill) -> Result<Changed> {
        let mut st = self.settings.clone();
        let Some(spot) = st
            .spots
            .iter_mut()
            .find(|s| s.mode == imaging::develop::SpotMode::Remove && s.shape_key() == key)
        else {
            return Ok(None);
        };
        spot.fill = Some(fill);
        self.set_live(st);
        self.commit(catalog, Some("Remove".into()))
    }

    /// Select Subject / Sky / Background (as a new mask, or a component of the selected
    /// one): ask before the first download, else start making it.
    pub fn select_ai(&mut self, kind: AiKind, target: ToolTarget) {
        self.request_ai(kind, target, None);
    }

    /// Select Object: the thing `prompt` points at (as a new mask, or a component).
    pub fn select_object(&mut self, prompt: ObjectPrompt, target: ToolTarget) {
        self.request_ai(AiKind::Object, target, Some(prompt));
    }

    fn request_ai(&mut self, kind: AiKind, target: ToolTarget, prompt: Option<ObjectPrompt>) {
        if self.ai_job.is_some() {
            return;
        }
        let bytes = segment::download_size(&[kind.model()]);
        if bytes > 0 {
            self.ai_consent = Some(AiConsent { kind, target, prompt, bytes });
        } else {
            self.start_ai(kind, target, prompt);
        }
    }

    /// Make an AI mask (downloading first whatever it still needs).
    pub fn start_ai(&mut self, kind: AiKind, target: ToolTarget, prompt: Option<ObjectPrompt>) {
        if kind == AiKind::People {
            (self.people_allowed, self.ai_consent) = (true, None);
            self.find_people();
            return;
        }
        if kind == AiKind::Dust {
            (self.dust_ai_allowed, self.ai_consent) = (true, None);
            self.request_dust();
            return;
        }
        if kind == AiKind::Remove {
            // Remove's fills start on their own once the download is allowed.
            (self.fill_allowed, self.ai_consent) = (true, None);
            self.request_renders();
            return;
        }
        let Some(photo_id) = self.photo else { return };
        self.ai_consent = None;
        let job = AiJob { kind, target, prompt, progress: Default::default() };
        *job.progress.lock().unwrap_or_else(|e| e.into_inner()) = (-1.0, "Starting…".into());
        let settings = self.settings.clone();
        let (rotation, gpu) = (self.rotation, self.ai_gpu);
        let _ = self.tx.send(Request::Select { photo_id, job: job.clone(), settings, rotation, gpu });
        self.ai_job = Some(job);
    }

    /// The AI mask `job` made (picture `id`): add it as it asked, and save.
    fn add_ai_mask(&mut self, catalog: &mut Catalog, job: &AiJob, id: String) -> Result<Changed> {
        use imaging::develop::{Component, Mask, Shape};
        let kind = job.kind;
        let shape = Shape::Raster { id, selects: kind.label().into(), model: kind.model_name().into() };
        let mut st = self.settings.clone();
        let selected = self.masking.as_ref().and_then(|m| m.selected).filter(|&i| i < st.masks.len());
        let (label, at) = match (job.target, selected) {
            (ToolTarget::Component(mode), Some(i)) => {
                st.masks[i].components.push(Component { mode, invert: false, shape });
                (format!("{} {}", mode.label(), kind.label()), (i, st.masks[i].components.len() - 1))
            }
            _ => {
                st.masks.push(Mask::new(next_mask_name(&st.masks), shape));
                (format!("Add {} Mask", kind.label()), (st.masks.len() - 1, 0))
            }
        };
        if let Some(m) = &mut self.masking {
            m.selected = Some(at.0);
            m.component = at.1;
        }
        self.set_live(st);
        self.commit(catalog, Some(label))
    }

    /// Change the shown settings (rendered right away, saved by [`Self::commit`]).
    pub fn set_live(&mut self, settings: DevelopSettings) {
        if settings != self.settings {
            self.settings = settings;
            self.changed_at = std::time::Instant::now();
            self.request_renders();
        }
    }

    /// Save the shown settings to the catalog as a History step (named `label`, or after
    /// what changed). Settings equal to the defaults are stored as none.
    pub fn commit(&mut self, catalog: &mut Catalog, label: Option<String>) -> Result<Changed> {
        let Some(id) = self.photo else { return Ok(None) };
        if self.settings == self.saved {
            return Ok(None);
        }
        let label = label.unwrap_or_else(|| self.settings.describe_change(&self.saved, self.kelvin()));
        let json = (!self.settings.is_default_for(self.raw)).then(|| self.settings.to_json());
        catalog.set_develop_settings(id, json.as_deref(), &label)?;
        self.saved = self.settings.clone();
        self.redo.clear();
        self.history = catalog.develop_history(id)?;
        Ok(Some(id))
    }

    /// Set and save in one go (buttons and menu commands).
    pub fn apply(
        &mut self,
        catalog: &mut Catalog,
        settings: DevelopSettings,
        label: &str,
    ) -> Result<Changed> {
        self.set_live(settings);
        self.commit(catalog, Some(label.to_owned()))
    }

    /// Ctrl+Z: back one History step.
    pub fn undo(&mut self, catalog: &mut Catalog) -> Result<Changed> {
        let Some(id) = self.photo else { return Ok(None) };
        self.commit(catalog, None)?;
        let history = catalog.develop_history(id)?;
        let [.., prev, last] = history.as_slice() else { return Ok(None) };
        catalog.revert_to_history_step(id, prev.id)?;
        self.redo.push((last.label.clone(), last.settings.clone()));
        self.after_revert(catalog, id, prev.settings.as_deref())
    }

    /// Ctrl+Y: redo the last undone step.
    pub fn redo(&mut self, catalog: &mut Catalog) -> Result<Changed> {
        let Some(id) = self.photo else { return Ok(None) };
        let Some((label, settings)) = self.redo.pop() else { return Ok(None) };
        catalog.set_develop_settings(id, settings.as_deref(), &label)?;
        let redo = std::mem::take(&mut self.redo);
        let changed = self.after_revert(catalog, id, settings.as_deref());
        self.redo = redo;
        changed
    }

    /// Click in the History panel: go back to that step (later steps can be redone).
    pub fn revert_to(&mut self, catalog: &mut Catalog, step_id: i64) -> Result<Changed> {
        let Some(id) = self.photo else { return Ok(None) };
        self.commit(catalog, None)?;
        let history = catalog.develop_history(id)?;
        let Some(pos) = history.iter().position(|h| h.id == step_id) else { return Ok(None) };
        if pos + 1 == history.len() {
            return Ok(None);
        }
        catalog.revert_to_history_step(id, step_id)?;
        let mut undone: Vec<_> =
            history[pos + 1..].iter().map(|h| (h.label.clone(), h.settings.clone())).collect();
        undone.reverse();
        let mut redo = std::mem::take(&mut self.redo);
        redo.extend(undone);
        let changed = self.after_revert(catalog, id, history[pos].settings.as_deref());
        self.redo = redo;
        changed
    }

    fn after_revert(&mut self, catalog: &mut Catalog, id: i64, json: Option<&str>) -> Result<Changed> {
        self.adopt(json);
        self.history = catalog.develop_history(id)?;
        Ok(Some(id))
    }

    pub fn can_undo(&self) -> bool {
        self.history.len() >= 2 || self.settings != self.saved
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// White Balance Selector click at (x, y), 0..1 of the image in file orientation.
    pub fn pick(&mut self, x: f32, y: f32) {
        let Some(photo_id) = self.photo else { return };
        self.picking_wb = false;
        let op = Adjust::PickWb { x, y };
        let _ = self.tx.send(Request::Adjust { photo_id, op, settings: self.settings.clone() });
    }

    /// An Upright button: the automatic modes analyse the photo (on the worker), Off and
    /// Guided are worked out here. Saved as a History step either way.
    pub fn upright(&mut self, catalog: &mut Catalog, mode: Upright) -> Result<Changed> {
        let Some(photo_id) = self.photo else { return Ok(None) };
        match mode {
            Upright::Off | Upright::Guided => {
                let mut s = self.settings.clone();
                s.transform.upright = mode;
                s.transform.upright_angles = match (mode, self.info.as_ref()) {
                    (Upright::Guided, Some(info)) => imaging::develop::guided_angles(
                        &s.transform.guides,
                        info.full_size.0,
                        info.full_size.1,
                    ),
                    _ => [0.0; 3],
                };
                self.apply(catalog, s, &format!("Upright: {}", mode.label()))
            }
            _ => {
                let op = Adjust::Upright(mode);
                let _ = self.tx.send(Request::Adjust { photo_id, op, settings: self.settings.clone() });
                Ok(None)
            }
        }
    }

    /// Set Upright Guided's guides (0..1 of the untransformed photo) and straighten them.
    pub fn set_guides(&mut self, guides: Vec<[f32; 4]>) {
        let Some(info) = self.info.as_ref() else { return };
        let mut s = self.settings.clone();
        s.transform.upright_angles =
            imaging::develop::guided_angles(&guides, info.full_size.0, info.full_size.1);
        s.transform.guides = guides;
        self.set_live(s);
    }

    /// The developed photo's size in full-size pixels (smaller than the file with
    /// Constrain Crop), as the center view shows it.
    pub fn output_size(&self) -> Option<(u32, u32)> {
        let (w, h) = self.info.as_ref()?.full_size;
        Some(if self.before { (w, h) } else { self.view_settings().transform.output_size(w, h) })
    }

    /// The settings the center view shows: with the crop tool open, the whole frame (turned
    /// by the crop's angle), so the crop rectangle can be drawn over it, as in LrC.
    pub fn view_settings(&self) -> DevelopSettings {
        let mut s = self.settings.clone();
        if self.cropping.is_some() {
            s.transform.crop = s.transform.crop.uncropped();
        }
        s
    }

    /// Width / height of the frame the crop is drawn on (the photo as the Transform shows it,
    /// before cropping).
    pub fn crop_frame_aspect(&self) -> Option<f32> {
        let (w, h) = self.info.as_ref()?.full_size;
        let t = imaging::develop::Transform { crop: Default::default(), ..self.settings.transform.clone() };
        let (fw, fh) = t.output_size(w, h);
        Some(fw as f32 / fh.max(1) as f32)
    }

    /// The Remove tool: pick spot `index` a source from the photo (on the worker; saved as
    /// a History step when it answers). `other`: a different one than now (the / key).
    pub fn find_source(&mut self, index: usize, other: bool) {
        let Some(photo_id) = self.photo else { return };
        let op = Adjust::FindSource { index, other };
        let _ = self.tx.send(Request::Adjust { photo_id, op, settings: self.settings.clone() });
    }

    /// Crop & Straighten's Auto: level the photo from its lines (on the worker).
    pub fn auto_straighten(&mut self) {
        let Some(photo_id) = self.photo else { return };
        let op = Adjust::AutoStraighten { constrain: self.crop_constrain };
        let _ = self.tx.send(Request::Adjust { photo_id, op, settings: self.settings.clone() });
    }

    /// Auto (Tone), from the photo's pixels; saved when the worker answers.
    pub fn auto_tone(&mut self) {
        let Some(photo_id) = self.photo else { return };
        let op = Adjust::AutoTone;
        let _ = self.tx.send(Request::Adjust { photo_id, op, settings: self.settings.clone() });
    }

    /// J: show clipped pixels.
    pub fn set_clipping(&mut self, on: bool) {
        self.clipping = on;
        self.request_renders();
    }

    /// What a full-resolution region shows: the settings (none for Before in the center
    /// view; the Detail box always shows the edit), clipping and the sharpening preview.
    fn shown_view(&self, target: RegionTarget, region: Region) -> RegionView {
        let before = self.before && target == RegionTarget::Zoom;
        let settings = if before { self.defaults() } else { self.view_settings() };
        let view = if before { None } else { self.current_view() };
        RegionView { settings, clipping: self.clipping && target == RegionTarget::Zoom, view, region }
    }

    /// `target` shows `region` of the full-size image: ask for it once the settings have
    /// settled (the zoomed view waits until a slider drag pauses; the small Detail box
    /// follows sooner). Returns how long to wait before asking again, if it's too early.
    pub fn want_region(&mut self, target: RegionTarget, region: Region) -> Option<std::time::Duration> {
        let settle = std::time::Duration::from_millis(match target {
            RegionTarget::Zoom => 200,
            RegionTarget::DetailBox => 60,
        });
        let photo_id = self.photo?;
        let view = self.shown_view(target, region);
        let r = self.regions.entry(target).or_default();
        if r.sent.as_ref() == Some(&view) {
            return None;
        }
        let since = self.changed_at.elapsed();
        if since < settle {
            return Some(settle - since);
        }
        self.region_seq += 1;
        r.sent = Some(view.clone());
        r.sent_seq = self.region_seq;
        let _ = self.tx.send(Request::Region { photo_id, target, view, seq: self.region_seq });
        None
    }

    /// The full-resolution region `target` shows, if it shows what it should now (same
    /// settings, clipping and preview; it may cover another part while panning).
    pub fn region_for(&self, target: RegionTarget) -> Option<(Region, &egui::TextureHandle)> {
        let (v, t) = self.regions.get(&target)?.shown.as_ref()?;
        let now = self.shown_view(target, v.region);
        (*v == now).then_some((v.region, t))
    }

    /// Is a 1:1 render for `target` on its way (or the full-size image loading)?
    pub fn region_pending(&self, target: RegionTarget) -> bool {
        self.regions.get(&target).is_some_and(|r| r.sent_seq > r.shown_seq)
    }

    /// The greyscale view the photo is shown in, if any: a sharpening preview, or Visualize
    /// Spots while the Remove tool is open.
    pub fn current_view(&self) -> Option<SharpenView> {
        match self.sharpen_view {
            Some(v) => Some(v),
            None => (self.visualize_spots && self.removing.is_some())
                .then_some(SharpenView::Spots(self.spot_threshold)),
        }
    }

    /// Visualize Spots on or off (A), or a new threshold.
    pub fn set_visualize_spots(&mut self, on: bool, threshold: u8) {
        self.visualize_spots = on;
        self.spot_threshold = threshold.min(100);
        self.request_renders();
    }

    /// Alt / Shift held while dragging a Sharpening slider (or not).
    pub fn set_sharpen_view(&mut self, view: Option<SharpenView>) {
        if view != self.sharpen_view {
            self.sharpen_view = view;
            // Regions follow at once: the box should switch with the main view.
            self.changed_at = std::time::Instant::now() - std::time::Duration::from_secs(1);
            self.request_renders();
        }
    }

    /// Show the Before slot too (rendered on demand).
    pub fn set_before(&mut self, before: bool) {
        self.before = before;
        self.request_renders();
    }
}

/// Remove: fill a spot with the model (downloading it first if needed), save the picture,
/// and describe the fill for spot shape `key`.
fn run_fill(
    photo_id: i64,
    job: &AiJob,
    input: imaging::develop::FillInput,
    key: u64,
    best: bool,
    gpu: bool,
    ctx: &egui::Context,
) -> Result<imaging::develop::Fill> {
    use image::imageops::{FilterType, resize};
    let set = |share: f32, what: String| {
        *job.progress.lock().unwrap_or_else(|e| e.into_inner()) = (share, what);
        ctx.request_repaint();
    };
    // What the batch shows (which fill of how many), back after a download.
    let (batch_share, batch_what) = job.progress();
    let model = if best { segment::AiModel::RemoveBest } else { segment::AiModel::Remove };
    if !segment::ready(&[model]) {
        let mut last = std::time::Instant::now();
        segment::ensure(&[model], &mut |share, what| {
            if last.elapsed().as_millis() > 100 {
                last = std::time::Instant::now();
                set(share, format!("Downloading {what}"));
            }
        })?;
    }
    set(batch_share, batch_what);
    let started = std::time::Instant::now();
    let s = segment::inpaint::SIZE;
    let img = resize(&input.image, s, s, FilterType::Lanczos3);
    // The hole a pixel larger after scaling: what the spot touches at all.
    let mask = resize(&input.mask, s, s, FilterType::Triangle);
    let mask = image::GrayImage::from_fn(s, s, |x, y| {
        image::Luma([if mask.get_pixel(x, y)[0] > 0 { 255 } else { 0 }])
    });
    let (out, device) = segment::remove(&img, &mask, best, gpu)?;
    log::info!("Remove: filled on {} in {:.0?}", device.label(), started.elapsed());
    // Back to the crop's size (at most 1024 px: larger areas come out softer).
    let side = input.image.width().min(1024);
    let out = resize(&out, side, side, FilterType::Lanczos3);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let id = format!("{photo_id}-fill-{:x}", stamp.as_nanos());
    imaging::develop::save_fill_picture(&id, &out)?;
    let model = if best { "big-LaMa" } else { "LaMa-Dilated" }.to_owned();
    Ok(imaging::develop::Fill { id, rect: input.rect, gains: input.gains, made_for: key, model })
}

/// Start loading the full-size image of `photo_id` on a thread of its own (unless it is
/// loading already); it arrives through `full_tx`.
fn load_full(
    full_loading: &mut Option<i64>,
    source: &Option<(i64, PathBuf, SourceKind)>,
    photo_id: i64,
    full_tx: &Sender<FullLoaded>,
    ctx: &egui::Context,
) {
    if *full_loading == Some(photo_id) {
        return;
    }
    let Some((_, path, kind)) = source.clone().filter(|s| s.0 == photo_id) else { return };
    *full_loading = Some(photo_id);
    let (full_tx, ctx) = (full_tx.clone(), ctx.clone());
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let result = WorkingImage::load(&path, kind, None).map(|(img, _)| img);
        log::info!("develop: full size of {} in {:.1?}", path.display(), started.elapsed());
        let _ = full_tx.send((photo_id, result.map_err(|e| format!("{e:#}"))));
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_prompts_turn_with_the_photo() {
        // A point near the top left and a box in the top left quarter of a 400 × 200 file.
        let p = ObjectPrompt { points: vec![[0.1, 0.2]], rect: Some([0.0, 0.0, 0.5, 0.5]) };
        let (pts, rect) = p.upright(0, 400, 200);
        assert_eq!(pts, vec![(40.0, 40.0, true)]);
        assert_eq!(rect, Some([0.0, 0.0, 200.0, 100.0]));
        // Turned a quarter clockwise (200 × 400 now): the top left goes to the top right.
        let (pts, rect) = p.upright(1, 200, 400);
        assert_eq!(pts, vec![(160.0, 40.0, true)]);
        assert_eq!(rect, Some([100.0, 0.0, 200.0, 200.0]));
        // Half a turn: to the bottom right.
        let (_, rect) = p.upright(2, 400, 200);
        assert_eq!(rect, Some([200.0, 100.0, 400.0, 200.0]));
    }
}
