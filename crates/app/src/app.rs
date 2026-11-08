//! Application state and behaviour. Drawing lives in `ui/`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use catalog::{Catalog, Flag, Folder, PhotoMetadata, PhotoRow, Source};
use crossbeam_channel::Receiver;
use eframe::egui;
use imaging::{PreviewCache, PreviewSize};
use serde::{Deserialize, Serialize};

use crate::jobs::{Event, Histogram, Job, JobSystem, PhotoRef};
use crate::textures::TextureCache;
use crate::ui::panels::SectionStack;
use crate::ui::shortcuts::{Action, SHORTCUTS, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Grid,
    Loupe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lights {
    #[default]
    On,
    Dim,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GridStyle {
    #[default]
    Compact,
    Expanded,
    Minimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MetadataPreset {
    #[default]
    Default,
    Exif,
}

/// The Loupe zoom level that Z / a click toggles to from Fit (Navigator header: FIT FILL 1:1 2:1 3:1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ZoomLevel {
    Fill,
    #[default]
    OneToOne,
    TwoToOne,
    ThreeToOne,
}

impl ZoomLevel {
    pub const ALL: [ZoomLevel; 4] = [Self::Fill, Self::OneToOne, Self::TwoToOne, Self::ThreeToOne];

    pub fn label(self) -> &'static str {
        match self {
            Self::Fill => "FILL",
            Self::OneToOne => "1:1",
            Self::TwoToOne => "2:1",
            Self::ThreeToOne => "3:1",
        }
    }

    /// Screen points per image pixel. `fill` is the scale that fills the view;
    /// 1:1 means one image pixel per physical screen pixel.
    pub fn scale(self, fill: f32, pixels_per_point: f32) -> f32 {
        match self {
            Self::Fill => fill,
            Self::OneToOne => 1.0 / pixels_per_point,
            Self::TwoToOne => 2.0 / pixels_per_point,
            Self::ThreeToOne => 3.0 / pixels_per_point,
        }
    }
}

/// Loupe zoom state for this session (LrC keeps the zoom position when moving between
/// photos, which makes checking focus across a burst quick).
#[derive(Debug, Clone, Copy)]
pub struct Zoom {
    pub zoomed: bool,
    /// The image point (0..1, in displayed orientation) shown at the center of the Loupe.
    pub center: egui::Vec2,
    /// Zoom so this image point (0..1) lands under this screen position (set by clicks
    /// and Z with the pointer over the photo; resolved when the Loupe is next drawn).
    pub anchor: Option<(egui::Vec2, egui::Pos2)>,
    /// Where the photo was drawn last frame, and which part of it (0..1) was visible:
    /// for the Navigator rectangle and for anchoring Z at the pointer.
    pub image_rect: Option<egui::Rect>,
    pub visible: Option<egui::Rect>,
}

impl Default for Zoom {
    fn default() -> Self {
        Zoom { zoomed: false, center: egui::vec2(0.5, 0.5), anchor: None, image_rect: None, visible: None }
    }
}

impl Zoom {
    /// Zoom in so the image point under `pointer` stays put (if the pointer is over the photo).
    pub fn zoom_in_at(&mut self, pointer: Option<egui::Pos2>) {
        self.zoomed = true;
        if let (Some(p), Some(r)) = (pointer, self.image_rect)
            && r.contains(p)
        {
            let q = (p - r.min) / r.size();
            self.anchor = Some((q, p));
        }
    }
}

/// Panel layout and view preferences, remembered between runs.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub show_top: bool,
    pub show_left: bool,
    pub show_right: bool,
    pub show_filmstrip: bool,
    pub show_toolbar: bool,
    pub show_filter_bar: bool,
    pub left: SectionStack,
    pub right: SectionStack,
    pub include_subfolders: bool,
    pub thumb_size: f32,
    pub grid_style: GridStyle,
    /// Loupe info overlay: 0 = off, 1 = Info 1, 2 = Info 2.
    pub info_overlay: u8,
    pub metadata_preset: MetadataPreset,
    pub sort: catalog::SortOrder,
    /// Loupe zoom level used by Z / click (the last one picked in the Navigator).
    pub zoom_level: ZoomLevel,
    /// Photo ▸ Auto Advance: rating, flagging and labelling move to the next photo.
    pub auto_advance: bool,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            show_top: true,
            show_left: true,
            show_right: true,
            show_filmstrip: true,
            show_toolbar: true,
            show_filter_bar: true,
            left: SectionStack::default(),
            right: SectionStack::default(),
            include_subfolders: true,
            thumb_size: 170.0,
            grid_style: GridStyle::Compact,
            info_overlay: 1,
            metadata_preset: MetadataPreset::Default,
            sort: catalog::SortOrder::default(),
            zoom_level: ZoomLevel::default(),
            auto_advance: false,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    last_catalog: Option<PathBuf>,
    ui: UiState,
}

/// Commands from menus, buttons and shortcuts, run after drawing each frame.
#[derive(Debug, Clone)]
pub enum Command {
    Action(Action),
    CreateDefaultCatalog,
    NewCatalog,
    OpenCatalog,
    SyncFolder(i64),
    SetSource(Source),
    SetSort(catalog::SortOrder),
    /// Ask for confirmation, then remove a folder (and subfolders) from the catalog.
    RemoveFolder(i64),
    ConfirmRemoveFolder(i64),
    ShowParentFolder(i64),
    /// "Find Missing Folder…" / "Update Folder Location…": pick where the folder is now.
    LocateFolder(i64),
    /// Open the "Create Folder" / "Rename" dialogs.
    CreateFolderIn(i64),
    RenameFolder(i64),
    ApplyCreateFolder {
        parent: i64,
        name: String,
    },
    ApplyRenameFolder {
        id: i64,
        name: String,
    },
    /// Build standard previews for the selected photos.
    BuildPreviews,
    Reveal(PathBuf),
    SetLights(Lights),
    /// A not-yet-implemented feature; shows when it arrives.
    Planned(&'static str, &'static str),
    Quit,
}

pub struct Toast {
    pub text: String,
    pub at: Instant,
}

/// An open catalog and everything derived from it.
pub struct Session {
    pub catalog: Catalog,
    pub jobs: JobSystem,
    events: Receiver<Event>,
    pub folders: Vec<Folder>,
    pub folder_tree: FolderTree,
    /// Catalog folders not found on disk (checked in the background).
    pub missing_folders: HashSet<i64>,
    pub collapsed_folders: HashSet<i64>,
    pub free_space: crate::platform::FreeSpaceCache,
    pub counts: (i64, i64),
    pub source: Source,
    pub sort: catalog::SortOrder,
    pub photos: Vec<PhotoRow>,
    pub index_of: HashMap<i64, usize>,
    pub selected: HashSet<i64>,
    pub active: Option<i64>,
    anchor: Option<i64>,
    pub thumbs: TextureCache,
    pub previews: TextureCache,
    /// Full-resolution textures for 1:1 zoom (~100 MB each for 24 MP, so only a few).
    pub full: TextureCache,
    pub histograms: HashMap<i64, Box<Histogram>>,
    metadata: Option<(i64, Option<PhotoMetadata>)>,
    pub scroll_to_active: bool,
    reload_due: Option<Instant>,
}

impl Session {
    fn open(ctx: &egui::Context, catalog: Catalog, sort: catalog::SortOrder) -> Result<Self> {
        let cache = PreviewCache::new(catalog.previews_dir());
        let (jobs, events) = JobSystem::start(ctx.clone(), catalog.dir().to_path_buf(), cache);
        let mut s = Session {
            catalog,
            jobs,
            events,
            folders: Vec::new(),
            folder_tree: FolderTree::default(),
            missing_folders: HashSet::new(),
            collapsed_folders: HashSet::new(),
            free_space: Default::default(),
            counts: (0, 0),
            source: Source::AllPhotographs,
            sort,
            photos: Vec::new(),
            index_of: HashMap::new(),
            selected: HashSet::new(),
            active: None,
            anchor: None,
            thumbs: TextureCache::new("thumb", 1500),
            previews: TextureCache::new("preview", 12),
            full: TextureCache::new("full", 3),
            histograms: HashMap::new(),
            metadata: None,
            scroll_to_active: false,
            reload_due: None,
        };
        s.refresh_tree()?;
        s.reload_photos()?;
        s.jobs.submit_normal(Job::PurgeOrphanPreviews);
        Ok(s)
    }

    pub fn refresh_tree(&mut self) -> Result<()> {
        self.folders = self.catalog.folders()?;
        self.folder_tree = FolderTree::build(&self.folders);
        let folders = self.folders.iter().map(|f| (f.id, f.path.clone())).collect();
        self.jobs.submit_normal(Job::CheckFolders { folders });
        self.counts = self.catalog.source_counts()?;
        Ok(())
    }

    /// Reload the grid's photos, keeping selection for photos that are still there.
    pub fn reload_photos(&mut self) -> Result<()> {
        let started = Instant::now();
        self.photos = self.catalog.photos_sorted(self.source, self.sort)?;
        if started.elapsed() > Duration::from_millis(50) {
            log::warn!("slow reload: {} photos in {:.0?}", self.photos.len(), started.elapsed());
        }
        self.index_of = self.photos.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
        self.selected.retain(|id| self.index_of.contains_key(id));
        if self.active.is_none_or(|id| !self.index_of.contains_key(&id)) {
            self.active = self.photos.first().map(|p| p.id);
            self.selected.extend(self.active);
        }
        self.metadata = None;
        Ok(())
    }

    pub fn active_photo(&self) -> Option<&PhotoRow> {
        self.active.and_then(|id| self.index_of.get(&id)).map(|&i| &self.photos[i])
    }

    pub fn active_index(&self) -> Option<usize> {
        self.active.and_then(|id| self.index_of.get(&id).copied())
    }

    pub fn active_metadata(&mut self) -> Option<PhotoMetadata> {
        let id = self.active?;
        if self.metadata.as_ref().is_none_or(|(mid, _)| *mid != id) {
            let m = self.catalog.metadata(id).unwrap_or_else(|e| {
                log::warn!("reading metadata: {e:#}");
                None
            });
            self.metadata = Some((id, m));
        }
        self.metadata.as_ref().and_then(|(_, m)| m.clone())
    }

    pub fn cache_for(&mut self, size: PreviewSize) -> &mut TextureCache {
        match size {
            PreviewSize::Thumb => &mut self.thumbs,
            PreviewSize::Standard => &mut self.previews,
            PreviewSize::Full => &mut self.full,
        }
    }

    /// Ask the workers for an image if it isn't loaded or already requested.
    pub fn request(&mut self, photo: &PhotoRow, size: PreviewSize) {
        let cache = self.cache_for(size);
        if cache.should_request(photo.id) {
            self.jobs.submit_visible(Job::Load { photo: PhotoRef::from(photo), size });
        }
    }

    pub fn source_label(&self) -> String {
        match self.source {
            Source::AllPhotographs => "Catalog : All Photographs".into(),
            Source::PreviousImport => "Catalog : Previous Import".into(),
            Source::Folder { id, .. } => {
                let name = self
                    .folders
                    .iter()
                    .find(|f| f.id == id)
                    .and_then(|f| f.path.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_default();
                format!("Folder : {name}")
            }
        }
    }

    // ---- selection (LrC semantics) ----

    pub fn click(&mut self, id: i64, modifiers: egui::Modifiers) {
        if modifiers.shift {
            let anchor = self.anchor.or(self.active).unwrap_or(id);
            if let (Some(&a), Some(&b)) = (self.index_of.get(&anchor), self.index_of.get(&id)) {
                let (lo, hi) = (a.min(b), a.max(b));
                if !modifiers.command {
                    self.selected.clear();
                }
                self.selected.extend(self.photos[lo..=hi].iter().map(|p| p.id));
            }
        } else if modifiers.command {
            if !self.selected.remove(&id) {
                self.selected.insert(id);
            }
            self.anchor = Some(id);
        } else {
            self.selected.clear();
            self.selected.insert(id);
            self.anchor = Some(id);
        }
        self.active = Some(id);
    }

    pub fn move_active(&mut self, delta: isize) {
        let Some(i) = self.active_index() else {
            if let Some(p) = self.photos.first() {
                self.click(p.id, egui::Modifiers::NONE);
            }
            return;
        };
        let j = (i as isize + delta).clamp(0, self.photos.len() as isize - 1) as usize;
        let id = self.photos[j].id;
        self.click(id, egui::Modifiers::NONE);
        self.scroll_to_active = true;
    }

    /// Photos a rating/flag/label applies to: all selected in Grid, else the active one.
    fn targets(&self, view: View) -> Vec<i64> {
        if view == View::Grid && self.selected.len() > 1 {
            let mut ids: Vec<i64> = self.selected.iter().copied().collect();
            ids.sort_by_key(|id| self.index_of.get(id));
            ids
        } else {
            self.active.into_iter().collect()
        }
    }

    fn update_rows(&mut self, ids: &[i64], f: impl Fn(&mut PhotoRow)) {
        for id in ids {
            if let Some(&i) = self.index_of.get(id) {
                f(&mut self.photos[i]);
            }
        }
    }

    fn process_events(&mut self, ctx: &egui::Context) -> Vec<String> {
        let started = Instant::now();
        let mut messages = Vec::new();
        let mut handled = 0usize;
        for event in self.events.try_iter().collect::<Vec<_>>() {
            handled += 1;
            match event {
                Event::SyncDone { root, result } => {
                    let name = root.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    match result {
                        Ok(r) => {
                            let mut parts = vec![format!("{} added", r.added)];
                            if r.updated > 0 {
                                parts.push(format!("{} updated", r.updated));
                            }
                            if r.missing > 0 {
                                parts.push(format!("{} missing", r.missing));
                            }
                            if r.restored > 0 {
                                parts.push(format!("{} found again", r.restored));
                            }
                            messages.push(format!("{name}: {}", parts.join(", ")));
                            for id in self.photos.iter().map(|p| p.id).collect::<Vec<_>>() {
                                self.thumbs.invalidate(id);
                            }
                        }
                        Err(e) => messages.push(format!("Could not synchronize {name}: {e}")),
                    }
                    if let Err(e) = self.refresh_tree().and_then(|_| self.reload_photos()) {
                        messages.push(format!("Catalog error: {e:#}"));
                    }
                }
                Event::Image { photo_id, size, image, histogram } => {
                    let cache = self.cache_for(size);
                    cache.insert(ctx, photo_id, image);
                    if let Some(h) = histogram {
                        if self.histograms.len() > 3000 {
                            self.histograms.clear();
                        }
                        self.histograms.insert(photo_id, h);
                    }
                }
                Event::ImageFailed { photo_id, size } => self.cache_for(size).mark_failed(photo_id),
                Event::FoldersChecked { missing } => self.missing_folders = missing,
                Event::MetadataUpdated { photo_id } => {
                    if self.active == Some(photo_id) {
                        self.metadata = None;
                    }
                    // Capture times change the sort order; reload, but not on every event.
                    self.reload_due.get_or_insert_with(|| Instant::now() + Duration::from_millis(1500));
                }
            }
        }
        if self.reload_due.is_some_and(|t| Instant::now() >= t) {
            self.reload_due = None;
            if let Err(e) = self.reload_photos() {
                log::warn!("reloading photos: {e:#}");
            }
        }
        if let Some(t) = self.reload_due {
            ctx.request_repaint_after(t.saturating_duration_since(Instant::now()));
        }
        if started.elapsed() > Duration::from_millis(50) {
            log::warn!("slow event processing: {handled} events in {:.0?}", started.elapsed());
        }
        messages
    }
}

pub struct PhotoLibraryApp {
    pub ui: UiState,
    last_catalog: Option<PathBuf>,
    pub session: Option<Session>,
    pub view: View,
    pub lights: Lights,
    pub fullscreen: bool,
    pub zoom: Zoom,
    pub show_shortcuts: bool,
    pub toast: Option<Toast>,
    pub error: Option<String>,
    /// The open modal dialog, if any.
    pub dialog: Option<Dialog>,
    pub commands: Vec<Command>,
    /// Rects of panels drawn this frame (dimmed by Lights Out).
    pub panel_rects: Vec<egui::Rect>,
    /// Grid geometry from the last frame, for Up/Down and scroll-to-active.
    pub grid_cols: usize,
    /// Whole rows visible in the grid (for Page Up / Page Down).
    pub grid_rows_visible: usize,
    pub grid_scroll: (f32, f32),
    perf: crate::perf::FrameStats,
}

impl PhotoLibraryApp {
    pub fn new(cc: &eframe::CreationContext<'_>, args: crate::Args) -> Self {
        crate::ui::theme::apply(&cc.egui_ctx);
        let settings: Settings =
            cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();
        let mut app = PhotoLibraryApp {
            ui: settings.ui,
            last_catalog: None,
            session: None,
            view: View::Grid,
            lights: Lights::On,
            fullscreen: false,
            zoom: Zoom::default(),
            show_shortcuts: false,
            toast: None,
            error: None,
            dialog: None,
            commands: Vec::new(),
            panel_rects: Vec::new(),
            grid_cols: 1,
            grid_rows_visible: 1,
            grid_scroll: (0.0, 0.0),
            perf: crate::perf::FrameStats::from_env(),
        };
        if let Some(dir) = args.catalog {
            let create = !dir.join("catalog.db").is_file();
            app.open_catalog(&cc.egui_ctx, &dir, create);
        } else if let Some(dir) = settings.last_catalog.filter(|d| d.join("catalog.db").is_file()) {
            app.open_catalog(&cc.egui_ctx, &dir, false);
        }
        if let (Some(s), Some(root)) = (&app.session, args.add) {
            s.jobs.submit_normal(Job::Sync { root });
        }
        app
    }

    pub fn run(&mut self, cmd: Command) {
        self.commands.push(cmd);
    }

    pub fn notify(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), at: Instant::now() });
    }

    pub fn default_catalog_dir() -> PathBuf {
        let base = directories::UserDirs::new()
            .and_then(|d| d.picture_dir().map(Path::to_path_buf))
            .or_else(|| directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("Photo Library").join(format!("Photo Library.{}", catalog::CATALOG_EXTENSION))
    }

    fn open_catalog(&mut self, ctx: &egui::Context, dir: &Path, create: bool) {
        let result = if create { Catalog::create(dir) } else { Catalog::open(dir) }
            .and_then(|c| Session::open(ctx, c, self.ui.sort));
        match result {
            Ok(session) => {
                log::info!("opened catalog {}", dir.display());
                self.session = Some(session);
                self.last_catalog = Some(dir.to_path_buf());
                self.view = View::Grid;
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "{} - Photo Library",
                    dir.file_stem().unwrap_or_default().to_string_lossy()
                )));
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn add_folder(&mut self) {
        let Some(session) = &self.session else { return };
        if let Some(dir) = rfd::FileDialog::new().set_title("Add Photos from Folder").pick_folder() {
            session.jobs.submit_normal(Job::Sync { root: dir });
        }
    }

    fn apply_to_targets(
        &mut self,
        describe: impl Fn(usize) -> String,
        f: impl Fn(&Catalog, i64) -> Result<()>,
        update: impl Fn(&mut PhotoRow),
        advance: bool,
    ) {
        let Some(s) = &mut self.session else { return };
        let ids = s.targets(self.view);
        if ids.is_empty() {
            return;
        }
        for id in &ids {
            if let Err(e) = f(&s.catalog, *id) {
                self.error = Some(format!("{e:#}"));
                return;
            }
        }
        s.update_rows(&ids, update);
        if advance {
            s.move_active(1);
        }
        let n = ids.len();
        self.notify(describe(n));
    }

    fn action(&mut self, ctx: &egui::Context, action: Action) {
        if let Some(sc) = SHORTCUTS.iter().find(|s| s.action == action)
            && let Status::Planned(phase) = sc.status
        {
            self.notify(format!("{} arrives in {phase}", sc.description));
            return;
        }
        let for_n = |n: usize| if n > 1 { format!(" for {n} Photos") } else { String::new() };
        match action {
            Action::GridView => self.view = View::Grid,
            Action::LoupeView => {
                if self.session.as_ref().is_some_and(|s| s.active.is_some()) {
                    self.view = View::Loupe;
                }
            }
            Action::ToggleLoupe => {
                self.view = if self.view == View::Grid { View::Loupe } else { View::Grid };
                if let Some(s) = &mut self.session {
                    s.scroll_to_active = true;
                }
            }
            Action::FullScreenPreview => {
                self.fullscreen = !self.fullscreen;
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
            }
            Action::Escape => {
                if self.fullscreen {
                    self.fullscreen = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                } else if self.lights != Lights::On {
                    self.lights = Lights::On;
                } else {
                    self.view = View::Grid;
                }
            }
            Action::CycleLightsOut => {
                self.lights = match self.lights {
                    Lights::On => Lights::Dim,
                    Lights::Dim => Lights::Off,
                    Lights::Off => Lights::On,
                };
            }
            Action::ToggleSidePanels => {
                let show = !(self.ui.show_left || self.ui.show_right);
                self.ui.show_left = show;
                self.ui.show_right = show;
            }
            Action::ToggleAllPanels => {
                let u = &mut self.ui;
                let show = !(u.show_left || u.show_right || u.show_top || u.show_filmstrip);
                (u.show_left, u.show_right, u.show_top, u.show_filmstrip) = (show, show, show, show);
            }
            Action::ToggleToolbar => self.ui.show_toolbar ^= true,
            Action::ToggleFilterBar => self.ui.show_filter_bar ^= true,
            Action::ToggleTopPanel => self.ui.show_top ^= true,
            Action::ToggleFilmstrip => self.ui.show_filmstrip ^= true,
            Action::ToggleLeftPanel => self.ui.show_left ^= true,
            Action::ToggleRightPanel => self.ui.show_right ^= true,
            Action::CycleGridStyle => {
                self.ui.grid_style = match self.ui.grid_style {
                    GridStyle::Compact => GridStyle::Expanded,
                    GridStyle::Expanded => GridStyle::Minimal,
                    GridStyle::Minimal => GridStyle::Compact,
                };
                let name = format!("{:?}", self.ui.grid_style);
                self.notify(format!("Grid View Style: {name}"));
            }
            Action::CycleInfoOverlay => {
                self.ui.info_overlay = (self.ui.info_overlay + 1) % 3;
                let text = ["Info Overlay: Off", "Info 1", "Info 2"][self.ui.info_overlay as usize];
                self.notify(text);
            }
            Action::SetFlag { flag, advance } => {
                let text = match flag {
                    Flag::Picked => "Flag as Pick",
                    Flag::Rejected => "Flag as Rejected",
                    Flag::Unflagged => "Remove Flag",
                };
                self.apply_to_targets(
                    |n| format!("{text}{}", for_n(n)),
                    |c, id| c.set_flag(id, flag),
                    |p| p.flag = flag,
                    advance || self.auto_advance(),
                );
            }
            Action::SetRating { stars, advance } => {
                let text = match stars {
                    0 => "Remove Rating".to_string(),
                    1 => "Set Rating to 1 Star".to_string(),
                    n => format!("Set Rating to {n} Stars"),
                };
                self.apply_to_targets(
                    |n| format!("{text}{}", for_n(n)),
                    |c, id| c.set_rating(id, stars),
                    |p| p.rating = stars,
                    advance || self.auto_advance(),
                );
            }
            Action::ColorLabel { label, advance } => {
                let Some(s) = &self.session else { return };
                let ids = s.targets(self.view);
                let all_have = ids.iter().all(|id| {
                    s.index_of.get(id).is_some_and(|&i| s.photos[i].color_label.as_deref() == Some(label))
                });
                let new = if all_have { None } else { Some(label) };
                let mut name = label.to_string();
                name[..1].make_ascii_uppercase();
                let text = if all_have { "Remove Label".to_string() } else { format!("Set Label to {name}") };
                self.apply_to_targets(
                    |n| format!("{text}{}", for_n(n)),
                    |c, id| c.set_color_label(id, new),
                    |p| p.color_label = new.map(str::to_owned),
                    advance || self.auto_advance(),
                );
            }
            Action::ZoomToggle => {
                if self.session.as_ref().and_then(|s| s.active).is_none() {
                    return;
                }
                if self.view == View::Grid {
                    // LrC: Z in Grid opens the photo in Loupe, zoomed in.
                    self.view = View::Loupe;
                    self.zoom.zoomed = true;
                } else if self.zoom.zoomed {
                    self.zoom.zoomed = false;
                } else {
                    let pointer = ctx.pointer_hover_pos();
                    self.zoom.zoom_in_at(pointer);
                }
            }
            Action::ZoomIn | Action::ZoomOut => {
                if self.session.as_ref().and_then(|s| s.active).is_none() {
                    return;
                }
                // Steps: Fit, then the Navigator levels.
                let current = if self.zoom.zoomed && self.view == View::Loupe {
                    ZoomLevel::ALL.iter().position(|&l| l == self.ui.zoom_level).map_or(0, |i| i + 1)
                } else {
                    0
                };
                let next = if action == Action::ZoomIn {
                    (current + 1).min(ZoomLevel::ALL.len())
                } else {
                    current.saturating_sub(1)
                };
                self.view = View::Loupe;
                self.zoom.zoomed = next > 0;
                if next > 0 {
                    self.ui.zoom_level = ZoomLevel::ALL[next - 1];
                }
                let label = if next == 0 { "Fit" } else { self.ui.zoom_level.label() };
                self.notify(format!("Zoom: {label}"));
            }
            Action::RotateLeft => self.rotate_targets(3),
            Action::RotateRight => self.rotate_targets(1),
            Action::ToggleAutoAdvance => {
                self.ui.auto_advance ^= true;
                self.notify(if self.ui.auto_advance { "Auto Advance on" } else { "Auto Advance off" });
            }
            Action::Next => self.with_session(|s| s.move_active(1)),
            Action::Previous => self.with_session(|s| s.move_active(-1)),
            Action::Up | Action::Down => {
                let cols = if self.view == View::Grid { self.grid_cols.max(1) as isize } else { 1 };
                let d = if action == Action::Up { -cols } else { cols };
                self.with_session(|s| s.move_active(d));
            }
            Action::First => self.with_session(|s| s.move_active(-(s.photos.len() as isize))),
            Action::Last => self.with_session(|s| s.move_active(s.photos.len() as isize)),
            Action::PageUp | Action::PageDown => {
                let step = if self.view == View::Grid {
                    (self.grid_cols * self.grid_rows_visible).max(1) as isize
                } else {
                    1
                };
                let d = if action == Action::PageUp { -step } else { step };
                self.with_session(|s| s.move_active(d));
            }
            Action::SelectAll => self.with_session(|s| s.selected = s.photos.iter().map(|p| p.id).collect()),
            Action::SelectNone => self.with_session(|s| s.selected.clear()),
            Action::Import => self.add_folder(),
            Action::ShowShortcuts => self.show_shortcuts ^= true,
            // Planned actions are handled above via their Status.
            Action::CompareView
            | Action::SurveyView
            | Action::QuickCollection
            | Action::Stack
            | Action::Unstack
            | Action::EditIn
            | Action::Keywording => {}
        }
    }

    /// Photo ▸ Auto Advance, or Caps Lock held on (as in LrC).
    fn auto_advance(&self) -> bool {
        self.ui.auto_advance || crate::platform::caps_lock_on()
    }

    /// Rotate the target photos by `quarter_turns` clockwise (catalog only; files untouched).
    fn rotate_targets(&mut self, quarter_turns: u8) {
        let Some(s) = &mut self.session else { return };
        let ids = s.targets(self.view);
        for &id in &ids {
            let Some(&i) = s.index_of.get(&id) else { continue };
            let rotation = (s.photos[i].rotation + quarter_turns) % 4;
            if let Err(e) = s.catalog.set_rotation(id, rotation) {
                self.error = Some(format!("{e:#}"));
                return;
            }
            s.photos[i].rotation = rotation;
        }
        let dir = if quarter_turns == 1 { "Right" } else { "Left" };
        let n = ids.len();
        self.notify(if n > 1 { format!("Rotate {dir} for {n} Photos") } else { format!("Rotate {dir}") });
    }

    fn with_session(&mut self, f: impl FnOnce(&mut Session)) {
        if let Some(s) = &mut self.session {
            f(s);
        }
    }

    fn execute(&mut self, ctx: &egui::Context, cmd: Command) {
        match cmd {
            Command::Action(a) => self.action(ctx, a),
            Command::CreateDefaultCatalog => {
                let dir = Self::default_catalog_dir();
                let exists = dir.join("catalog.db").is_file();
                self.open_catalog(ctx, &dir, !exists);
            }
            Command::NewCatalog => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Create Folder with New Catalog")
                    .set_file_name(format!("My Catalog.{}", catalog::CATALOG_EXTENSION))
                    .save_file()
                {
                    let dir = if path.extension().is_some_and(|e| e == catalog::CATALOG_EXTENSION) {
                        path
                    } else {
                        path.with_extension(catalog::CATALOG_EXTENSION)
                    };
                    self.open_catalog(ctx, &dir, true);
                }
            }
            Command::OpenCatalog => {
                if let Some(dir) = rfd::FileDialog::new()
                    .set_title(format!("Open Catalog (a .{} folder)", catalog::CATALOG_EXTENSION))
                    .pick_folder()
                {
                    self.open_catalog(ctx, &dir, false);
                }
            }
            Command::SyncFolder(id) => {
                if let Some(s) = &self.session
                    && let Some(f) = s.folders.iter().find(|f| f.id == id)
                {
                    s.jobs.submit_normal(Job::Sync { root: f.path.clone() });
                    let name = f.path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    self.notify(format!("Synchronizing {name}"));
                }
            }
            Command::SetSort(sort) => {
                self.ui.sort = sort;
                if let Some(s) = &mut self.session {
                    s.sort = sort;
                    if let Err(e) = s.reload_photos() {
                        self.error = Some(format!("{e:#}"));
                    }
                    s.scroll_to_active = true;
                }
            }
            Command::SetSource(source) => {
                if let Some(s) = &mut self.session {
                    s.source = source;
                    s.active = None;
                    s.selected.clear();
                    if let Err(e) = s.reload_photos() {
                        self.error = Some(format!("{e:#}"));
                    }
                    self.view = View::Grid;
                }
            }
            Command::Reveal(path) => crate::platform::reveal(&path),
            Command::SetLights(l) => self.lights = l,
            Command::Planned(what, phase) => self.notify(format!("{what} arrives in {phase}")),
            Command::RemoveFolder(id) => {
                if let Some(s) = &self.session
                    && let Some(f) = s.folders.iter().find(|f| f.id == id)
                {
                    let photos = s.folder_tree.totals.get(&id).copied().unwrap_or(0);
                    self.dialog =
                        Some(Dialog::ConfirmRemoveFolder { id, name: folder_name(&f.path), photos });
                }
            }
            Command::ConfirmRemoveFolder(id) => self.folder_op(|s| {
                let removed = s.catalog.remove_folder(id)?;
                let n = removed.len();
                s.jobs.submit_normal(Job::PurgePreviews { photo_ids: removed });
                Ok(format!("Removed {n} photos from the catalog"))
            }),
            Command::ShowParentFolder(id) => self.folder_op(|s| {
                s.catalog.show_parent_folder(id)?;
                Ok("Showing parent folder".into())
            }),
            Command::LocateFolder(id) => {
                let Some(s) = &self.session else { return };
                let Some(f) = s.folders.iter().find(|f| f.id == id) else { return };
                let name = folder_name(&f.path);
                let title = if s.missing_folders.contains(&id) {
                    format!("Find Missing Folder \"{name}\"")
                } else {
                    format!("Update Folder Location for \"{name}\"")
                };
                // Start the dialog at the closest part of the old path that still exists.
                let start = f.path.ancestors().skip(1).find(|p| p.is_dir()).map(Path::to_path_buf);
                let mut dialog = rfd::FileDialog::new().set_title(title);
                if let Some(start) = start {
                    dialog = dialog.set_directory(start);
                }
                if let Some(new) = dialog.pick_folder() {
                    self.folder_op(|s| {
                        let moved = s.catalog.relocate_folder(id, &new)?;
                        // Refresh missing/changed files at the new location.
                        s.jobs.submit_normal(Job::Sync { root: moved.path.clone() });
                        let mut msg = format!("\"{name}\" now points to {}", moved.path.display());
                        if moved.found < moved.total {
                            msg += &format!(" ({} of {} photos found)", moved.found, moved.total);
                        }
                        Ok(msg)
                    });
                }
            }
            Command::CreateFolderIn(parent) => {
                self.dialog = Some(Dialog::CreateFolder { parent, name: String::new() })
            }
            Command::RenameFolder(id) => {
                if let Some(f) = self.session.as_ref().and_then(|s| s.folders.iter().find(|f| f.id == id)) {
                    self.dialog = Some(Dialog::RenameFolder { id, name: folder_name(&f.path) });
                }
            }
            Command::ApplyCreateFolder { parent, name } => self.folder_op(|s| {
                s.catalog.create_subfolder(parent, &name)?;
                Ok(format!("Created folder {}", name.trim()))
            }),
            Command::ApplyRenameFolder { id, name } => self.folder_op(|s| {
                s.catalog.rename_folder(id, &name)?;
                Ok(format!("Renamed folder to {}", name.trim()))
            }),
            Command::BuildPreviews => {
                if let Some(s) = &self.session {
                    let photos: Vec<PhotoRef> = s
                        .photos
                        .iter()
                        .filter(|p| s.selected.contains(&p.id) && !p.missing)
                        .map(PhotoRef::from)
                        .collect();
                    let n = photos.len();
                    s.jobs.build_previews(photos);
                    self.notify(format!("Building previews for {n} photos"));
                }
            }
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }
}

impl eframe::App for PhotoLibraryApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.perf.record(frame.info().cpu_usage);
        let ctx = ui.ctx().clone();
        if let Some(s) = &mut self.session {
            s.thumbs.begin_frame();
            s.previews.begin_frame();
            s.full.begin_frame();
            for msg in s.process_events(&ctx) {
                self.toast = Some(Toast { text: msg, at: Instant::now() });
            }
        }

        for sc in crate::ui::shortcuts::take_actions(&ctx) {
            self.commands.push(Command::Action(sc.action));
        }

        let draw_started = Instant::now();
        crate::ui::layout::draw(self, ui);
        if draw_started.elapsed() > Duration::from_millis(100) {
            log::warn!("slow frame: drawing took {:.0?}", draw_started.elapsed());
        }

        for cmd in std::mem::take(&mut self.commands) {
            self.execute(&ctx, cmd);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let settings = Settings { last_catalog: self.last_catalog.clone(), ui: std::mem::take(&mut self.ui) };
        eframe::set_value(storage, eframe::APP_KEY, &settings);
        self.ui = settings.ui;
    }
}

/// Folders panel structure, computed when the catalog's folders change rather than
/// every frame. Indices refer to `Session::folders`.
#[derive(Default)]
pub struct FolderTree {
    pub children: HashMap<Option<i64>, Vec<usize>>,
    /// Photo counts including all subfolders.
    pub totals: HashMap<i64, i64>,
    /// (volume key, display name, root folder indices), sorted by key.
    pub volumes: Vec<(String, String, Vec<usize>)>,
}

impl FolderTree {
    pub fn build(folders: &[Folder]) -> Self {
        let mut children: HashMap<Option<i64>, Vec<usize>> = HashMap::new();
        for (i, f) in folders.iter().enumerate() {
            children.entry(f.parent_id).or_default().push(i);
        }
        let index: HashMap<i64, usize> = folders.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
        // Folders are sorted by path, so children come after parents: sum in reverse.
        let mut totals: HashMap<i64, i64> = folders.iter().map(|f| (f.id, f.photo_count)).collect();
        for f in folders.iter().rev() {
            if let Some(parent) = f.parent_id.filter(|p| index.contains_key(p)) {
                let own = totals[&f.id];
                *totals.entry(parent).or_default() += own;
            }
        }
        let mut volumes: std::collections::BTreeMap<String, (String, Vec<usize>)> = Default::default();
        for &i in children.get(&None).into_iter().flatten() {
            let (key, name) = crate::platform::volume_of(&folders[i].path);
            volumes.entry(key).or_insert_with(|| (name, Vec::new())).1.push(i);
        }
        let volumes = volumes.into_iter().map(|(k, (n, roots))| (k, n, roots)).collect();
        FolderTree { children, totals, volumes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: i64, path: &str, parent: Option<i64>, count: i64) -> Folder {
        Folder { id, path: PathBuf::from(path), parent_id: parent, photo_count: count }
    }

    #[test]
    fn folder_tree_totals_include_subfolders() {
        let sep = std::path::MAIN_SEPARATOR;
        let p = |s: &str| s.replace('/', &sep.to_string());
        let folders = vec![
            folder(1, &p("/photos"), None, 1),
            folder(2, &p("/photos/2025"), Some(1), 2),
            folder(3, &p("/photos/2025/trip"), Some(2), 4),
            folder(4, &p("/photos/2026"), Some(1), 8),
        ];
        let t = FolderTree::build(&folders);
        assert_eq!(t.totals[&1], 15);
        assert_eq!(t.totals[&2], 6);
        assert_eq!(t.totals[&3], 4);
        assert_eq!(t.children[&Some(1)], vec![1, 3]);
        assert_eq!(t.volumes.len(), 1);
    }
}

/// Modal dialogs (LrC-style confirmations and name prompts).
// Variant names mirror the dialog titles; more (non-folder) dialogs will follow.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone)]
pub enum Dialog {
    ConfirmRemoveFolder { id: i64, name: String, photos: i64 },
    CreateFolder { parent: i64, name: String },
    RenameFolder { id: i64, name: String },
}

pub fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

impl PhotoLibraryApp {
    /// Run a folder operation, then refresh the folder tree and grid. If the grid's
    /// folder no longer exists, fall back to All Photographs.
    fn folder_op(&mut self, op: impl FnOnce(&mut Session) -> Result<String>) {
        let Some(s) = &mut self.session else { return };
        let result = op(s).and_then(|msg| {
            s.refresh_tree()?;
            if let Source::Folder { id, .. } = s.source
                && !s.folders.iter().any(|f| f.id == id)
            {
                s.source = Source::AllPhotographs;
            }
            s.reload_photos()?;
            Ok(msg)
        });
        match result {
            Ok(msg) => self.notify(msg),
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }
}
