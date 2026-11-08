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
    pub collapsed_folders: HashSet<i64>,
    pub counts: (i64, i64),
    pub source: Source,
    pub photos: Vec<PhotoRow>,
    pub index_of: HashMap<i64, usize>,
    pub selected: HashSet<i64>,
    pub active: Option<i64>,
    anchor: Option<i64>,
    pub thumbs: TextureCache,
    pub previews: TextureCache,
    pub histograms: HashMap<i64, Box<Histogram>>,
    metadata: Option<(i64, Option<PhotoMetadata>)>,
    pub scroll_to_active: bool,
    reload_due: Option<Instant>,
}

impl Session {
    fn open(ctx: &egui::Context, catalog: Catalog) -> Result<Self> {
        let cache = PreviewCache::new(catalog.previews_dir());
        let (jobs, events) = JobSystem::start(ctx.clone(), catalog.dir().to_path_buf(), cache);
        let mut s = Session {
            catalog,
            jobs,
            events,
            folders: Vec::new(),
            collapsed_folders: HashSet::new(),
            counts: (0, 0),
            source: Source::AllPhotographs,
            photos: Vec::new(),
            index_of: HashMap::new(),
            selected: HashSet::new(),
            active: None,
            anchor: None,
            thumbs: TextureCache::new("thumb", 1500),
            previews: TextureCache::new("preview", 12),
            histograms: HashMap::new(),
            metadata: None,
            scroll_to_active: false,
            reload_due: None,
        };
        s.refresh_tree()?;
        s.reload_photos()?;
        Ok(s)
    }

    pub fn refresh_tree(&mut self) -> Result<()> {
        self.folders = self.catalog.folders()?;
        self.counts = self.catalog.source_counts()?;
        Ok(())
    }

    /// Reload the grid's photos, keeping selection for photos that are still there.
    pub fn reload_photos(&mut self) -> Result<()> {
        self.photos = self.catalog.photos(self.source)?;
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

    /// Ask the workers for an image if it isn't loaded or already requested.
    pub fn request(&mut self, photo: &PhotoRow, size: PreviewSize) {
        let cache = match size {
            PreviewSize::Thumb => &mut self.thumbs,
            PreviewSize::Standard => &mut self.previews,
        };
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
        let mut messages = Vec::new();
        for event in self.events.try_iter().collect::<Vec<_>>() {
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
                    let cache = match size {
                        PreviewSize::Thumb => &mut self.thumbs,
                        PreviewSize::Standard => &mut self.previews,
                    };
                    cache.insert(ctx, photo_id, image);
                    if let Some(h) = histogram {
                        if self.histograms.len() > 3000 {
                            self.histograms.clear();
                        }
                        self.histograms.insert(photo_id, h);
                    }
                }
                Event::ImageFailed { photo_id, size } => match size {
                    PreviewSize::Thumb => self.thumbs.mark_failed(photo_id),
                    PreviewSize::Standard => self.previews.mark_failed(photo_id),
                },
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
    pub show_shortcuts: bool,
    pub toast: Option<Toast>,
    pub error: Option<String>,
    pub commands: Vec<Command>,
    /// Rects of panels drawn this frame (dimmed by Lights Out).
    pub panel_rects: Vec<egui::Rect>,
    /// Grid geometry from the last frame, for Up/Down and scroll-to-active.
    pub grid_cols: usize,
    pub grid_scroll: (f32, f32),
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
            show_shortcuts: false,
            toast: None,
            error: None,
            commands: Vec::new(),
            panel_rects: Vec::new(),
            grid_cols: 1,
            grid_scroll: (0.0, 0.0),
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
            .and_then(|c| Session::open(ctx, c));
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
                    advance,
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
                    advance,
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
                    advance,
                );
            }
            Action::Next => self.with_session(|s| s.move_active(1)),
            Action::Previous => self.with_session(|s| s.move_active(-1)),
            Action::Up | Action::Down => {
                let cols = if self.view == View::Grid { self.grid_cols.max(1) as isize } else { 1 };
                let d = if action == Action::Up { -cols } else { cols };
                self.with_session(|s| s.move_active(d));
            }
            Action::SelectAll => self.with_session(|s| s.selected = s.photos.iter().map(|p| p.id).collect()),
            Action::SelectNone => self.with_session(|s| s.selected.clear()),
            Action::Import => self.add_folder(),
            Action::ShowShortcuts => self.show_shortcuts ^= true,
            // Planned actions are handled above via their Status.
            Action::CompareView
            | Action::SurveyView
            | Action::ZoomToggle
            | Action::RotateLeft
            | Action::RotateRight
            | Action::QuickCollection
            | Action::Stack
            | Action::Unstack
            | Action::EditIn
            | Action::Keywording => {}
        }
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
            Command::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }
}

impl eframe::App for PhotoLibraryApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(s) = &mut self.session {
            s.thumbs.begin_frame();
            s.previews.begin_frame();
            for msg in s.process_events(&ctx) {
                self.toast = Some(Toast { text: msg, at: Instant::now() });
            }
        }

        for sc in crate::ui::shortcuts::take_actions(&ctx) {
            self.commands.push(Command::Action(sc.action));
        }

        crate::ui::layout::draw(self, ui);

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
