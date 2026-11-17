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
    DevelopSettings, Region, ResolvedWb, WbPreset, WorkingImage, auto_tone, render_region_srgb8, render_srgb8,
};

use crate::jobs::Histogram;

/// What the center view shows: the photo as developed, or before any adjustments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Current,
    Before,
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
        seq: u64,
    },
    /// Render part of the full-size photo (1:1 zoom), loading it first if needed.
    Region { photo_id: i64, view: RegionView, seq: u64 },
    /// Settings computed from the pixels.
    Adjust { photo_id: i64, op: Adjust, settings: DevelopSettings },
}

/// What a full-resolution region render shows.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionView {
    pub settings: DevelopSettings,
    pub clipping: bool,
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
}

impl Adjust {
    fn label(self) -> &'static str {
        match self {
            Adjust::PickWb { .. } => "White Balance: Custom",
            Adjust::AutoTone => "Auto Settings",
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
        seq: u64,
        view: RegionView,
        image: egui::ColorImage,
    },
    Adjusted {
        photo_id: i64,
        op: Adjust,
        settings: Option<DevelopSettings>,
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
    let mut pending_region: Option<(i64, RegionView, u64)> = None;
    let (full_tx, full_rx) = crossbeam_channel::unbounded::<FullLoaded>();
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
                            pending_region = None;
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
        let mut renders: HashMap<Slot, (i64, DevelopSettings, bool, u64)> = HashMap::new();
        let mut adjusts = Vec::new();
        for req in first.into_iter().chain(rx.try_iter()) {
            match req {
                Request::Load { photo_id, path, kind, max_px } => load = Some((photo_id, path, kind, max_px)),
                Request::Render { photo_id, slot, settings, clipping, seq } => {
                    renders.insert(slot, (photo_id, settings, clipping, seq));
                }
                Request::Region { photo_id, view, seq } => pending_region = Some((photo_id, view, seq)),
                Request::Adjust { photo_id, op, settings } => adjusts.push((photo_id, op, settings)),
            }
        }
        if let Some((photo_id, path, kind, max_px)) = load {
            if source.as_ref().is_none_or(|s| s.0 != photo_id) {
                // Another photo: forget this one's full-size image and pending region.
                full = None;
                pending_region = pending_region.filter(|r| r.0 == photo_id);
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
        let Some((id, img)) = &current else {
            ctx.request_repaint();
            continue;
        };
        for slot in [Slot::Current, Slot::Before] {
            let Some((photo_id, settings, paint, seq)) = renders.remove(&slot).filter(|r| r.0 == *id) else {
                continue;
            };
            let mut rgb = render_srgb8(img, &settings);
            let histogram = histogram(rgb.as_raw());
            let clipped = clipping(&mut rgb, paint);
            let wb = img.resolve_wb(&settings);
            let image = color_image(&rgb);
            let _ = tx.send(Reply::Rendered { photo_id, slot, seq, image, histogram, wb, clipped });
        }
        for (photo_id, op, settings) in adjusts.into_iter().filter(|a| a.0 == *id) {
            let settings = match op {
                Adjust::PickWb { x, y } => {
                    img.picked_multipliers(x, y).map(|m| img.settings_with_multipliers(&settings, m))
                }
                Adjust::AutoTone => Some(auto_tone(img, &settings)),
            };
            let _ = tx.send(Reply::Adjusted { photo_id, op, settings });
        }
        // 1:1: render the region from the full-size image, loading that first.
        if let Some((photo_id, view, seq)) = pending_region.take_if(|r| r.0 == *id) {
            match &full {
                Some((fid, full_img)) if *fid == photo_id => {
                    let started = std::time::Instant::now();
                    let mut rgb = render_region_srgb8(full_img, &view.settings, view.region);
                    clipping(&mut rgb, view.clipping);
                    log::debug!("develop: 1:1 region {:?} in {:.0?}", view.region, started.elapsed());
                    let image = color_image(&rgb);
                    let _ = tx.send(Reply::Region { photo_id, seq, view, image });
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
                    pending_region = Some((photo_id, view, seq));
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
    /// Settings (and their sequence number) last sent per slot; the numbers drop outdated renders.
    sent: HashMap<Slot, (DevelopSettings, bool, u64)>,
    seq: u64,
    shown: HashMap<Slot, u64>,
    /// `\`: show the photo before any adjustments.
    pub before: bool,
    /// W: the White Balance Selector is active (click the photo to pick).
    pub picking_wb: bool,
    /// Long side, in pixels, the center view shows the photo at (rounded up), and the
    /// proxy size the current photo was loaded at: renders cost in proportion.
    pub view_px: u32,
    loaded_px: u32,
    /// J: show clipped pixels; and how many there are in the current render.
    pub clipping: bool,
    pub clipped: Clipped,
    /// 1:1 zoom: the full-resolution render of the visible region, and what it shows.
    pub region: Option<(RegionView, egui::TextureHandle)>,
    /// The region view last requested, its sequence number, and the newest one shown.
    region_sent: Option<RegionView>,
    region_seq: u64,
    region_shown: u64,
    /// When the shown settings last changed (regions wait until they settle).
    changed_at: std::time::Instant,
    /// Copy Settings / Paste Settings.
    pub clipboard: Option<DevelopSettings>,
    /// Tone Curve panel: point curve (else parametric), the channel edited, and the
    /// point being dragged.
    pub curve_point_mode: bool,
    pub curve_channel: imaging::develop::CurveChannel,
    pub curve_drag: Option<usize>,
    pub history: Vec<HistoryStep>,
    /// Steps undone (Ctrl+Z, or clicking an earlier History step), newest last.
    redo: Vec<(String, Option<String>)>,
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
            view_px: 1536,
            loaded_px: 0,
            clipping: false,
            clipped: Clipped::default(),
            region: None,
            region_sent: None,
            region_seq: 0,
            region_shown: 0,
            changed_at: std::time::Instant::now(),
            clipboard: None,
            curve_point_mode: false,
            curve_channel: imaging::develop::CurveChannel::Rgb,
            curve_drag: None,
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
        self.info = None;
        self.textures.clear();
        self.region = None;
        self.region_sent = None;
        self.histogram = None;
        self.wb = None;
        self.error = None;
        self.sent.clear();
        self.shown.clear();
        self.picking_wb = false;
        self.redo.clear();
        self.adopt(catalog.develop_settings(photo.id)?.as_deref());
        self.history = catalog.develop_history(photo.id)?;
        if photo.missing {
            self.error = Some("The file could not be found.".into());
        } else {
            let kind = if photo.kind == FileKind::Raw { SourceKind::Raw } else { SourceKind::Image };
            self.loaded_px = self.view_px;
            let max_px = self.view_px;
            let _ =
                self.tx.send(Request::Load { photo_id: photo.id, path: photo.path.clone(), kind, max_px });
            self.request_renders();
        }
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

    /// Take stored settings (JSON) as both shown and saved.
    fn adopt(&mut self, json: Option<&str>) {
        let s = json.and_then(DevelopSettings::from_json).unwrap_or_default();
        self.settings = s.clone();
        self.saved = s;
        self.request_renders();
    }

    /// Ask for renders of whatever changed since the last request.
    pub fn request_renders(&mut self) {
        let Some(photo_id) = self.photo else { return };
        let clipping = self.clipping;
        let mut wanted = vec![(Slot::Current, self.settings.clone())];
        if self.before {
            wanted.push((Slot::Before, DevelopSettings::default()));
        }
        for (slot, settings) in wanted {
            if self.sent.get(&slot).is_some_and(|(s, c, _)| *s == settings && *c == clipping) {
                continue;
            }
            self.seq += 1;
            self.sent.insert(slot, (settings.clone(), clipping, self.seq));
            let _ = self.tx.send(Request::Render { photo_id, slot, settings, clipping, seq: self.seq });
        }
    }

    /// Is a render of newer settings still on its way?
    pub fn rendering(&self) -> bool {
        self.sent
            .get(&Slot::Current)
            .is_some_and(|(_, _, sent)| self.shown.get(&Slot::Current).copied().unwrap_or(0) < *sent)
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
                    if slot == Slot::Current {
                        self.histogram = Some(histogram);
                        self.wb = Some(wb);
                        self.clipped = clipped;
                    }
                }
                Reply::Adjusted { photo_id, op, settings } if Some(photo_id) == self.photo => {
                    match settings {
                        Some(s) => {
                            self.set_live(s);
                            changed = changed.or(self.commit(catalog, Some(op.label().into()))?);
                        }
                        None => self.error = Some("Pick a neutral, unclipped area".into()),
                    }
                }
                Reply::Region { photo_id, seq, view, image } if Some(photo_id) == self.photo => {
                    if seq < self.region_shown {
                        continue;
                    }
                    self.region_shown = seq;
                    let opts = egui::TextureOptions::LINEAR;
                    match &mut self.region {
                        Some((v, t)) if t.size() == image.size => {
                            t.set(image, opts);
                            *v = view;
                        }
                        _ => self.region = Some((view, ctx.load_texture("develop_region", image, opts))),
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
        let json = (!self.settings.is_default()).then(|| self.settings.to_json());
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

    /// What the center view shows: the settings (none for Before) and clipping.
    fn shown_view(&self, region: Region) -> RegionView {
        let settings = if self.before { DevelopSettings::default() } else { self.settings.clone() };
        RegionView { settings, clipping: self.clipping, region }
    }

    /// 1:1 zoom shows `region` of the full-size image: ask for it once the settings have
    /// settled (not on every step of a slider drag). Returns how long to wait before
    /// asking again, if it's too early.
    pub fn want_region(&mut self, region: Region) -> Option<std::time::Duration> {
        const SETTLE: std::time::Duration = std::time::Duration::from_millis(200);
        let photo_id = self.photo?;
        let view = self.shown_view(region);
        if self.region_sent.as_ref() == Some(&view) {
            return None;
        }
        let since = self.changed_at.elapsed();
        if since < SETTLE {
            return Some(SETTLE - since);
        }
        self.region_seq += 1;
        self.region_sent = Some(view.clone());
        let _ = self.tx.send(Request::Region { photo_id, view, seq: self.region_seq });
        None
    }

    /// The full-resolution region to draw over the proxy, if it shows what the view
    /// shows now (same settings and clipping; it may cover another part while panning).
    pub fn region_for_view(&self) -> Option<(Region, &egui::TextureHandle)> {
        let (v, t) = self.region.as_ref()?;
        let now = self.shown_view(v.region);
        (*v == now).then_some((v.region, t))
    }

    /// Is a 1:1 render on its way (or the full-size image loading)?
    pub fn region_pending(&self) -> bool {
        self.region_seq > self.region_shown
    }

    /// Show the Before slot too (rendered on demand).
    pub fn set_before(&mut self, before: bool) {
        self.before = before;
        self.request_renders();
    }
}
