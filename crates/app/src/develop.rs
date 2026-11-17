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
use imaging::develop::{DevelopSettings, ResolvedWb, WbPreset, WorkingImage, render_srgb8};

use crate::jobs::Histogram;

/// Long side of the image the sliders work on.
const PROXY_PX: u32 = 2560;

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
    },
    Render {
        photo_id: i64,
        slot: Slot,
        settings: DevelopSettings,
        seq: u64,
    },
    /// White Balance Selector: neutralize the point (x, y), 0..1 of the unrotated image.
    Pick {
        photo_id: i64,
        x: f32,
        y: f32,
        settings: DevelopSettings,
    },
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
    },
    Picked {
        photo_id: i64,
        settings: Option<DevelopSettings>,
    },
    Failed {
        photo_id: i64,
        error: String,
    },
}

/// What the Basic panel needs to know about the photo's source.
#[derive(Debug, Clone)]
pub struct SourceInfo {
    /// Temp/Tint in Kelvin (RAW with a colour matrix), else relative.
    pub kelvin: bool,
    pub presets: &'static [WbPreset],
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

fn worker(rx: Receiver<Request>, tx: Sender<Reply>, ctx: egui::Context) {
    let mut current: Option<(i64, WorkingImage)> = None;
    while let Ok(first) = rx.recv() {
        // Take everything queued: the last load, the last render per slot, every pick.
        let mut load = None;
        let mut renders: HashMap<Slot, (i64, DevelopSettings, u64)> = HashMap::new();
        let mut picks = Vec::new();
        for req in std::iter::once(first).chain(rx.try_iter()) {
            match req {
                Request::Load { photo_id, path, kind } => load = Some((photo_id, path, kind)),
                Request::Render { photo_id, slot, settings, seq } => {
                    renders.insert(slot, (photo_id, settings, seq));
                }
                Request::Pick { photo_id, x, y, settings } => picks.push((photo_id, x, y, settings)),
            }
        }
        if let Some((photo_id, path, kind)) = load {
            if current.as_ref().is_none_or(|(id, _)| *id != photo_id) {
                current = None;
                let started = std::time::Instant::now();
                match WorkingImage::load(&path, kind, Some(PROXY_PX)) {
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
                let info = SourceInfo { kelvin: img.kelvin(), presets: img.wb_presets() };
                let _ = tx.send(Reply::Loaded { photo_id, info });
            }
        }
        let Some((id, img)) = &current else {
            ctx.request_repaint();
            continue;
        };
        for slot in [Slot::Current, Slot::Before] {
            let Some((photo_id, settings, seq)) = renders.remove(&slot).filter(|r| r.0 == *id) else {
                continue;
            };
            let rgb = render_srgb8(img, &settings);
            let image =
                egui::ColorImage::from_rgb([rgb.width() as usize, rgb.height() as usize], rgb.as_raw());
            let reply = Reply::Rendered {
                photo_id,
                slot,
                seq,
                image,
                histogram: histogram(rgb.as_raw()),
                wb: img.resolve_wb(&settings),
            };
            let _ = tx.send(reply);
        }
        for (photo_id, x, y, settings) in picks.into_iter().filter(|p| p.0 == *id) {
            let picked = img.picked_multipliers(x, y).map(|m| img.settings_with_multipliers(&settings, m));
            let _ = tx.send(Reply::Picked { photo_id, settings: picked });
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
    sent: HashMap<Slot, (DevelopSettings, u64)>,
    seq: u64,
    shown: HashMap<Slot, u64>,
    /// `\`: show the photo before any adjustments.
    pub before: bool,
    /// W: the White Balance Selector is active (click the photo to pick).
    pub picking_wb: bool,
    /// Copy Settings / Paste Settings.
    pub clipboard: Option<DevelopSettings>,
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
            clipboard: None,
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
            let _ = self.tx.send(Request::Load { photo_id: photo.id, path: photo.path.clone(), kind });
            self.request_renders();
        }
        Ok(changed)
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
        let mut wanted = vec![(Slot::Current, self.settings.clone())];
        if self.before {
            wanted.push((Slot::Before, DevelopSettings::default()));
        }
        for (slot, settings) in wanted {
            if self.sent.get(&slot).is_some_and(|(s, _)| *s == settings) {
                continue;
            }
            self.seq += 1;
            self.sent.insert(slot, (settings.clone(), self.seq));
            let _ = self.tx.send(Request::Render { photo_id, slot, settings, seq: self.seq });
        }
    }

    /// Is a render of newer settings still on its way?
    pub fn rendering(&self) -> bool {
        self.sent
            .get(&Slot::Current)
            .is_some_and(|(_, sent)| self.shown.get(&Slot::Current).copied().unwrap_or(0) < *sent)
    }

    /// Handle the worker's replies. A picked white balance is saved right away.
    pub fn poll(&mut self, ctx: &egui::Context, catalog: &mut Catalog) -> Result<Changed> {
        let mut changed = None;
        for reply in self.rx.try_iter().collect::<Vec<_>>() {
            match reply {
                Reply::Loaded { photo_id, info } if Some(photo_id) == self.photo => self.info = Some(info),
                Reply::Rendered { photo_id, slot, seq, image, histogram, wb }
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
                    }
                }
                Reply::Picked { photo_id, settings } if Some(photo_id) == self.photo => match settings {
                    Some(s) => {
                        self.settings = s;
                        self.request_renders();
                        changed = changed.or(self.commit(catalog, Some("White Balance: Custom".into()))?);
                    }
                    None => self.error = Some("Pick a neutral, unclipped area".into()),
                },
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
        let _ = self.tx.send(Request::Pick { photo_id, x, y, settings: self.settings.clone() });
    }

    /// Show the Before slot too (rendered on demand).
    pub fn set_before(&mut self, before: bool) {
        self.before = before;
        self.request_renders();
    }
}
