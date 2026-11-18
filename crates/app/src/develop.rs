//! The Develop module's state, and the thread that renders the photo being developed.
//!
//! The worker keeps the current photo's pixels (a proxy about the screen's size) and
//! renders the latest settings it was sent: while a slider is dragged, requests pile up
//! faster than renders finish, and only the newest one is rendered.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
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
        RemoveSettings { mode: imaging::develop::SpotMode::Heal, size: 12.0, feather: 50.0, opacity: 100.0 }
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
}

/// The Remove tool (Q): the selected spot and the drag in progress.
#[derive(Debug, Clone, Default)]
pub struct RemoveTool {
    pub selected: Option<usize>,
    pub drag: Option<RemoveDrag>,
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
}

impl ShapeKind {
    pub fn label(self) -> &'static str {
        match self {
            ShapeKind::Brush => "Brush",
            ShapeKind::Linear => "Linear Gradient",
            ShapeKind::Radial => "Radial Gradient",
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
}

impl Masking {
    /// The mask whose overlay to show, if any.
    pub fn overlay_mask(&self) -> Option<usize> {
        self.hover.or(self.selected.filter(|_| self.overlay))
    }
}

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
        let Some((id, img)) = &current else {
            ctx.request_repaint();
            continue;
        };
        for slot in [Slot::Current, Slot::Before] {
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
        // The mask overlay, after the render it goes over.
        if let Some((photo_id, settings, index, seq)) = overlay.take_if(|o| o.0 == *id)
            && let Some(mask) = render_mask(img, &settings, index)
        {
            let (w, h) = (mask.width() as usize, mask.height() as usize);
            let rgba: Vec<u8> = mask.as_raw().iter().flat_map(|v| [255, 40, 40, v / 2]).collect();
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
            let _ = tx.send(Reply::Overlay { photo_id, seq, image });
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
                    if full_loading != Some(photo_id)
                        && let Some((_, path, kind)) = source.clone().filter(|s| s.0 == photo_id)
                    {
                        full_loading = Some(photo_id);
                        let full_tx = full_tx.clone();
                        let ctx = ctx.clone();
                        std::thread::spawn(move || {
                            let started = std::time::Instant::now();
                            let result = WorkingImage::load(&path, kind, None).map(|(img, _)| img);
                            log::debug!(
                                "develop: full size of {} in {:.1?}",
                                path.display(),
                                started.elapsed()
                            );
                            let _ = full_tx.send((photo_id, result.map_err(|e| format!("{e:#}"))));
                            ctx.request_repaint();
                        });
                    }
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
    /// The mask overlay: what was last asked for, the newest shown, and its texture.
    overlay_sent: Option<(DevelopSettings, usize)>,
    overlay_seq: u64,
    overlay_shown: u64,
    pub overlay: Option<egui::TextureHandle>,
    /// Upright Guided: the guide being drawn or moved on the photo.
    pub guide_drag: Option<GuideDrag>,
    /// When the shown settings last changed (regions wait until they settle).
    changed_at: std::time::Instant,
    /// Copy Settings / Paste Settings.
    pub clipboard: Option<DevelopSettings>,
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
            overlay_sent: None,
            overlay_seq: 0,
            overlay_shown: 0,
            overlay: None,
            guide_drag: None,
            changed_at: std::time::Instant::now(),
            clipboard: None,
            curve_point_mode: false,
            curve_channel: imaging::develop::CurveChannel::Rgb,
            curve_drag: None,
            hsl_tab: None,
            grading_view: None,
            history: Vec::new(),
            redo: Vec::new(),
        }
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
        self.histogram = None;
        self.wb = None;
        self.error = None;
        self.sent.clear();
        self.shown.clear();
        self.picking_wb = false;
        self.redo.clear();
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
    pub fn defaults(&self) -> DevelopSettings {
        DevelopSettings::default_for(self.raw)
    }

    /// Take stored settings (JSON) as both shown and saved.
    fn adopt(&mut self, json: Option<&str>) {
        let s = json.and_then(DevelopSettings::from_json).unwrap_or_else(|| self.defaults());
        self.settings = s.clone();
        self.saved = s;
        self.request_renders();
    }

    /// Ask for renders of whatever changed since the last request.
    pub fn request_renders(&mut self) {
        let Some(photo_id) = self.photo else { return };
        let clipping = self.clipping;
        let mut wanted = vec![(Slot::Current, self.view_settings(), self.sharpen_view)];
        if self.before {
            wanted.push((Slot::Before, self.defaults(), None));
        }
        for (slot, settings, view) in wanted {
            if self.sent.get(&slot).is_some_and(|(s, c, v, _)| *s == settings && *c == clipping && *v == view)
            {
                continue;
            }
            self.seq += 1;
            self.sent.insert(slot, (settings.clone(), clipping, view, self.seq));
            let _ = self.tx.send(Request::Render { photo_id, slot, settings, clipping, view, seq: self.seq });
        }
        // The mask overlay (masking with O on, or a mask hovered in the Masks panel).
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
            None => {
                self.overlay_sent = None;
                self.overlay = None;
            }
        }
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
                    if slot == Slot::Current && self.sharpen_view.is_none() {
                        self.histogram = Some(histogram);
                        self.wb = Some(wb);
                        self.clipped = clipped;
                    }
                }
                Reply::Adjusted { photo_id, op, settings } if Some(photo_id) == self.photo => {
                    match settings {
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
                Reply::Failed { photo_id, error } if Some(photo_id) == self.photo => {
                    log::warn!("develop: {error}");
                    self.error = Some(error);
                }
                _ => {}
            }
        }
        Ok(changed)
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
        let view = if before { None } else { self.sharpen_view };
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
