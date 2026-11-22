//! Application state and behaviour. Drawing lives in `ui/`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use catalog::{Catalog, Flag, Folder, IptcField, Keyword, PhotoMetadata, PhotoRow, Source};
use crossbeam_channel::Receiver;
use eframe::egui;
use imaging::{PreviewCache, PreviewSize};
use serde::{Deserialize, Serialize};

use crate::jobs::{Event, Histogram, Job, JobSystem, PhotoRef};
use crate::textures::TextureCache;
use crate::ui::panels::SectionStack;
use crate::ui::shortcuts::{Action, SHORTCUTS, Status};

/// The module shown (LrC's module picker): Library, or Develop for the active photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Module {
    #[default]
    Library,
    Develop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Grid,
    Loupe,
    /// Two photos side by side (C): the Select and a Candidate.
    Compare,
    /// All selected photos at once (N).
    Survey,
}

/// Compare view's two photos. The active one (that rating and flagging apply to) is
/// `Session::active`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compare {
    pub select: i64,
    pub candidate: i64,
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
    Iptc,
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
    /// Zoom so this image point (0..1) lands this far from the view's center (set by
    /// clicks and Z with the pointer over the photo; resolved when next drawn). Relative
    /// to the center so both Compare panes zoom to the same spot.
    pub anchor: Option<(egui::Vec2, egui::Vec2)>,
    /// Where the photo was drawn last frame (and in what view), and which part of it
    /// (0..1) was visible: for the Navigator rectangle and for anchoring Z at the pointer.
    pub image_rect: Option<egui::Rect>,
    pub view_rect: Option<egui::Rect>,
    pub visible: Option<egui::Rect>,
}

impl Default for Zoom {
    fn default() -> Self {
        Zoom {
            zoomed: false,
            center: egui::vec2(0.5, 0.5),
            anchor: None,
            image_rect: None,
            view_rect: None,
            visible: None,
        }
    }
}

impl Zoom {
    /// Zoom in so the image point under `pointer` stays put (if the pointer is over the photo).
    pub fn zoom_in_at(&mut self, pointer: Option<egui::Pos2>) {
        if let (Some(p), Some(r), Some(v)) = (pointer, self.image_rect, self.view_rect) {
            self.zoom_in_at_in(p, r, v);
        } else {
            self.zoomed = true;
        }
    }

    /// Zoom in at `pointer` over a photo drawn at `image` inside `view`.
    pub fn zoom_in_at_in(&mut self, pointer: egui::Pos2, image: egui::Rect, view: egui::Rect) {
        self.zoomed = true;
        if image.contains(pointer) {
            let q = (pointer - image.min) / image.size();
            self.anchor = Some((q, pointer - view.center()));
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
    /// The Develop module's panels (sections open independently of Library's).
    pub dev_left: SectionStack,
    pub dev_right: SectionStack,
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
    /// LrC Catalog Settings ▸ "Automatically write changes into XMP".
    pub auto_write_xmp: bool,
    pub filter: crate::filter::FilterBar,
    pub import: crate::import::ImportPrefs,
    pub export: crate::export::ExportPrefs,
    /// The user's export presets (app-wide, like LrC's).
    pub export_presets: Vec<crate::export::ExportPreset>,
    /// External Editing settings (Edit In).
    pub edit_in: crate::editors::EditInPrefs,
    /// The version strip above the filmstrip (S).
    pub show_version_strip: bool,
    /// Run AI masks on the GPU (DirectML) when it can.
    pub ai_gpu: bool,
    /// Help ▸ Check for Updates Automatically: at start and once a day.
    pub check_updates: bool,
    /// The release the user chose to skip (not offered again unless they check).
    pub skipped_update: Option<String>,
    /// Lenses chosen by hand in Lens Corrections (maker, model), newest first.
    pub recent_lenses: Vec<(String, String)>,
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
            dev_left: SectionStack::default(),
            dev_right: SectionStack::default(),
            include_subfolders: true,
            thumb_size: 170.0,
            grid_style: GridStyle::Compact,
            info_overlay: 1,
            metadata_preset: MetadataPreset::Default,
            sort: catalog::SortOrder::default(),
            zoom_level: ZoomLevel::default(),
            auto_advance: false,
            auto_write_xmp: false,
            filter: crate::filter::FilterBar::default(),
            import: crate::import::ImportPrefs::default(),
            export: crate::export::ExportPrefs::default(),
            export_presets: Vec::new(),
            edit_in: crate::editors::EditInPrefs::default(),
            show_version_strip: true,
            ai_gpu: true,
            check_updates: true,
            skipped_update: None,
            recent_lenses: Vec::new(),
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
    /// Keywording panel: add keywords typed as text (LrC syntax) to the target photos.
    AddKeywords(String),
    /// Keyword List checkbox / Recent Keywords: tag (true) or untag the target photos.
    SetKeyword {
        id: i64,
        on: bool,
    },
    /// The Keyword Tags text was edited: apply additions and removals.
    SetKeywordTags {
        before: String,
        after: String,
    },
    SetIptc {
        field: IptcField,
        value: String,
    },
    CreateKeyword(Option<i64>),
    ApplyCreateKeyword {
        parent: Option<i64>,
        name: String,
    },
    RenameKeyword(i64),
    ApplyRenameKeyword {
        id: i64,
        name: String,
    },
    DeleteKeyword(i64),
    ConfirmDeleteKeyword(i64),
    /// Metadata ▸ Read Metadata from Files (asks first) for the target photos.
    ReadMetadata,
    ConfirmReadMetadata(Vec<i64>),
    ToggleAutoWriteXmp,
    /// Open the dialog to create a collection, smart collection or set (inside `parent`).
    CreateCollection {
        parent: Option<i64>,
        kind: catalog::CollectionKind,
    },
    ApplyCreateCollection {
        parent: Option<i64>,
        kind: catalog::CollectionKind,
        name: String,
        include_selected: bool,
        rules: Option<catalog::SmartRules>,
    },
    RenameCollection(i64),
    ApplyRenameCollection {
        id: i64,
        name: String,
    },
    DeleteCollection(i64),
    ConfirmDeleteCollection(i64),
    EditSmartCollection(i64),
    ApplySmartCollection {
        id: i64,
        name: String,
        rules: catalog::SmartRules,
    },
    /// Add photos (`None`: the target photos) to a regular collection.
    AddToCollection {
        id: i64,
        photos: Option<Vec<i64>>,
    },
    ToggleSeparateRawJpeg,
    /// Open the Import dialog (optionally starting in a mode, e.g. Add for "Add Folder").
    OpenImport(Option<catalog::ImportMode>),
    /// Import dialog: look at this source.
    ImportSource(PathBuf),
    /// Import dialog: the Import button.
    StartImport,
    CloseImport,
    CancelImport,
    /// Photo ▸ Edit In ▸ Edit in NegPy / Luminar Neo (Ctrl+E: the primary editor,
    /// Ctrl+Alt+E: the other) for the selected photos.
    EditIn(crate::editors::Editor),
    /// External Editing settings (where the editors are, NegPy's file format).
    OpenEditSettings,
    /// File ▸ Export… for the selected photos.
    OpenExport,
    /// File ▸ Export with Previous: no dialog.
    ExportWithPrevious,
    /// File ▸ Export with Preset: no dialog (the preset becomes "previous").
    ExportWithPreset(crate::export::PresetId),
    StartExport,
    CloseExport,
    CancelExport,
    DeleteVirtualCopies(Vec<i64>),
    /// Remove photos from the catalog (files stay on disk).
    RemovePhotos(Vec<i64>),
    /// Photos dropped on the Quick Collection.
    QuickCollectionAdd(Vec<i64>),
    Reveal(PathBuf),
    SetLights(Lights),
    /// Develop: save the shown settings as a History step (named, or after what changed).
    DevelopCommit(Option<String>),
    /// An Upright button in the Transform panel.
    DevelopUpright(imaging::develop::Upright),
    /// Develop: go back to a History step.
    DevelopRevert(i64),
    /// A not-yet-implemented feature; shows when it arrives.
    Planned(&'static str, &'static str),
    Quit,
}

pub struct Toast {
    pub text: String,
    pub at: Instant,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FacetKey {
    source: Source,
    bar: crate::filter::FilterBar,
    meta_gen: u64,
}

/// A metadata field across several photos: LrC shows `< mixed >` when they differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    Same(String),
    Mixed,
}

/// What the Keywording and Metadata panels show for the target photos.
#[derive(Debug, Clone, Default)]
pub struct SelectionMeta {
    key: (Vec<i64>, u64),
    pub ids: Vec<i64>,
    pub iptc: HashMap<IptcField, FieldValue>,
    /// Keyword id -> how many of the target photos carry it.
    pub keyword_counts: HashMap<i64, usize>,
    /// Keywords on any target photo, sorted by name: (id, name, on all targets).
    pub tags: Vec<(i64, String, bool)>,
}

impl SelectionMeta {
    fn load(catalog: &Catalog, keywords: &[Keyword], ids: Vec<i64>) -> Result<Self> {
        let mut iptc = HashMap::new();
        let all: Vec<catalog::Iptc> = ids.iter().map(|&id| catalog.iptc(id)).collect::<Result<_>>()?;
        for field in IptcField::ALL {
            let mut values = all.iter().map(|i| i.get(field).unwrap_or_default());
            let value = match values.next() {
                Some(first) if values.all(|v| v == first) => FieldValue::Same(first.to_owned()),
                Some(_) => FieldValue::Mixed,
                None => FieldValue::Same(String::new()),
            };
            iptc.insert(field, value);
        }
        let keyword_counts = catalog.keyword_counts_for(&ids)?;
        let mut tags: Vec<(i64, String, bool)> = keyword_counts
            .iter()
            .filter_map(|(&kw, &n)| {
                let name = keywords.iter().find(|k| k.id == kw)?.name.clone();
                Some((kw, name, n == ids.len()))
            })
            .collect();
        tags.sort_by_key(|(_, name, _)| name.to_lowercase());
        Ok(SelectionMeta { key: Default::default(), ids, iptc, keyword_counts, tags })
    }

    /// LrC's Keyword Tags text: `a, b, c*` (`*` = only on some of the photos).
    pub fn tags_text(&self) -> String {
        self.tags
            .iter()
            .map(|(_, n, all)| if *all { n.clone() } else { format!("{n}*") })
            .collect::<Vec<_>>()
            .join(", ")
    }
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
    pub quick_count: i64,
    /// The Library Filter in effect for `photos`.
    pub filter: catalog::Criterion,
    pub collections: Vec<catalog::Collection>,
    pub collapsed_collections: HashSet<i64>,
    /// `meta_gen` when `collections` (and their counts) were last read.
    collections_gen: Option<u64>,
    /// Metadata filter columns, cached for (source, filter bar, meta_gen).
    pub facets: Option<(FacetKey, Vec<Vec<catalog::Facet>>)>,
    /// The Import dialog, while open.
    pub import: Option<crate::import::ImportDialog>,
    /// The Export dialog, while open.
    pub export: Option<crate::ui::export::ExportDialog>,
    /// Photos sent to NegPy, and its results being looked for.
    pub edits: crate::editors::EditTracker,
    /// Photo pixel sizes (as stored), read on demand for views showing several photos.
    dims: HashMap<i64, Option<egui::Vec2>>,
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
    /// The keyword tree, refreshed whenever keywords change.
    pub keywords: Vec<Keyword>,
    pub collapsed_keywords: HashSet<i64>,
    /// Most recently used keywords first (LrC's "Recent Keywords" set).
    pub recent_keywords: Vec<i64>,
    /// Bumped on every metadata change, to refresh cached panel contents.
    pub meta_gen: u64,
    selection_meta: Option<SelectionMeta>,
    metadata: Option<(i64, Option<PhotoMetadata>)>,
    pub scroll_to_active: bool,
    reload_due: Option<Instant>,
    /// A member of the active unit shown instead of the unit's own photo (picked in the
    /// version strip): (the grid cell's photo id, the member).
    pub viewing: Option<(i64, PhotoRow)>,
    /// The active unit's members, for the version strip: (cell photo id, meta_gen, members).
    strip: Option<(i64, u64, Vec<catalog::Member>)>,
    /// The Develop module (its render thread runs while the catalog is open).
    pub develop: crate::develop::DevelopState,
}

impl Session {
    fn open(ctx: &egui::Context, catalog: Catalog, sort: catalog::SortOrder) -> Result<Self> {
        let cache = PreviewCache::new(catalog.previews_dir());
        imaging::develop::set_mask_store(Some(catalog.dir().join("masks")));
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
            quick_count: 0,
            filter: catalog::Criterion::AllOf(vec![]),
            collections: Vec::new(),
            collapsed_collections: HashSet::new(),
            collections_gen: None,
            facets: None,
            import: None,
            export: None,
            edits: Default::default(),
            dims: HashMap::new(),
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
            keywords: Vec::new(),
            collapsed_keywords: HashSet::new(),
            recent_keywords: Vec::new(),
            meta_gen: 0,
            selection_meta: None,
            metadata: None,
            scroll_to_active: false,
            reload_due: None,
            viewing: None,
            strip: None,
            develop: crate::develop::DevelopState::new(ctx),
        };
        s.refresh_tree()?;
        s.reload_photos()?;
        s.refresh_keywords()?;
        s.collections_fresh();
        s.jobs.submit_normal(Job::PurgeOrphanPreviews);
        s.jobs.submit_background(Job::ReadGps);
        Ok(s)
    }

    pub fn refresh_tree(&mut self) -> Result<()> {
        self.folders = self.catalog.folders()?;
        self.folder_tree = FolderTree::build(&self.folders);
        let folders = self.folders.iter().map(|f| (f.id, f.path.clone())).collect();
        self.jobs.submit_normal(Job::CheckFolders { folders });
        self.counts = self.catalog.source_counts()?;
        self.quick_count = self.catalog.quick_collection_count()?;
        Ok(())
    }

    /// Reload the grid's photos, keeping selection for photos that are still there.
    pub fn reload_photos(&mut self) -> Result<()> {
        let started = Instant::now();
        let old_index = self.active_index();
        // Units by (group or picture), to follow a unit whose shown photo changes (a new
        // edit, another member made active).
        let unit_key = |p: &PhotoRow| p.unit.group_id.map_or((false, p.unit.picture_id), |g| (true, g));
        let old_units: HashMap<i64, (bool, i64)> = self.photos.iter().map(|p| (p.id, unit_key(p))).collect();
        let old_develop: HashMap<i64, Option<String>> =
            self.photos.iter().map(|p| (p.id, p.develop.clone())).collect();
        self.photos = self.catalog.photos_filtered(self.source, self.sort, &self.filter)?;
        // Develop settings changed outside the Develop module (Read Metadata from File, a
        // sync reading a sidecar): show them.
        let developed: Vec<PhotoRow> = self
            .photos
            .iter()
            .filter(|p| old_develop.get(&p.id).is_some_and(|d| *d != p.develop))
            .cloned()
            .collect();
        for row in &developed {
            self.reload_images(row);
            if self.develop.photo == Some(row.id) {
                let history = self.catalog.develop_history(row.id)?;
                self.develop.external_change(row.develop.as_deref(), history);
            }
        }
        if started.elapsed() > Duration::from_millis(50) {
            log::warn!("slow reload: {} photos in {:.0?}", self.photos.len(), started.elapsed());
        }
        self.index_of = self.photos.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
        let by_unit: HashMap<(bool, i64), i64> = self.photos.iter().map(|p| (unit_key(p), p.id)).collect();
        let follow = |id: i64| -> Option<i64> { old_units.get(&id).and_then(|u| by_unit.get(u)).copied() };
        if let Some(a) = self.active.filter(|a| !self.index_of.contains_key(a)) {
            self.active = follow(a).or(self.active);
        }
        self.selected = self
            .selected
            .iter()
            .filter_map(|&id| if self.index_of.contains_key(&id) { Some(id) } else { follow(id) })
            .collect();
        self.strip = None;
        self.viewing = match self.viewing.take() {
            // Still in the same unit (and still a member): refresh it.
            Some((cell, m)) if Some(cell) == self.active => {
                self.catalog.photo(m.id).ok().flatten().filter(|r| r.id != cell).map(|r| (cell, r))
            }
            _ => None,
        };
        if self.active.is_none_or(|id| !self.index_of.contains_key(&id)) {
            // The active photo left the view (filtered out, stacked away): stay at the
            // same place in the grid rather than jumping to the start.
            let i = old_index.unwrap_or(0).min(self.photos.len().saturating_sub(1));
            self.active = self.photos.get(i).map(|p| p.id);
            self.selected.extend(self.active);
        }
        self.metadata = None;
        Ok(())
    }

    /// The photo shown: the active cell's, or the member of its unit picked in the
    /// version strip. Ratings, flags, metadata… apply to it.
    pub fn active_photo(&self) -> Option<&PhotoRow> {
        if let Some((cell, member)) = &self.viewing
            && self.active == Some(*cell)
        {
            return Some(member);
        }
        self.active.and_then(|id| self.index_of.get(&id)).map(|&i| &self.photos[i])
    }

    /// The active unit's members (for the version strip), read when the unit or the
    /// catalog's metadata changed.
    pub fn strip_members(&mut self) -> &[catalog::Member] {
        let Some(cell) = self.active else { return &[] };
        let fresh = self.strip.as_ref().is_some_and(|(c, g, _)| *c == cell && *g == self.meta_gen);
        if !fresh {
            let members = if self.active_photo().is_some_and(|p| p.unit.is_stack()) {
                self.catalog.unit_members(cell).unwrap_or_else(|e| {
                    log::warn!("reading the version strip: {e:#}");
                    Vec::new()
                })
            } else {
                Vec::new()
            };
            self.strip = Some((cell, self.meta_gen, members));
        }
        self.strip.as_ref().map_or(&[], |(_, _, m)| m.as_slice())
    }

    /// Show a member of the active unit in place of the cell's photo (the cell's own
    /// photo: back to it).
    pub fn view_member(&mut self, photo: PhotoRow) {
        let Some(cell) = self.active else { return };
        self.viewing = (photo.id != cell).then_some((cell, photo));
        self.metadata = None;
    }

    /// Alt+← / Alt+→: the previous / next member of the active unit.
    pub fn step_member(&mut self, forward: bool) {
        let shown = self.active_photo().map(|p| p.id);
        let members: Vec<PhotoRow> = self.strip_members().iter().map(|m| m.photo.clone()).collect();
        let Some(i) = members.iter().position(|m| Some(m.id) == shown) else { return };
        let j = if forward { i + 1 } else { i.wrapping_sub(1) };
        if let Some(m) = members.get(j) {
            self.view_member(m.clone());
        }
    }

    pub fn active_index(&self) -> Option<usize> {
        self.active.and_then(|id| self.index_of.get(&id).copied())
    }

    pub fn active_metadata(&mut self) -> Option<PhotoMetadata> {
        let id = self.active_photo()?.id;
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

    /// A photo's Develop settings changed in the catalog: take them into its row and
    /// load its thumbnail and preview again (they are rebuilt with the new settings).
    /// The old images stay on screen until the new ones arrive.
    pub fn develop_changed(&mut self, id: i64) -> Result<()> {
        let json = self.catalog.develop_settings(id)?;
        self.update_rows(&[id], |p| p.develop = json.clone());
        // Its developed size may have changed (Constrain Crop).
        self.dims.remove(&id);
        let row = self
            .index_of
            .get(&id)
            .map(|&i| self.photos[i].clone())
            .or_else(|| self.viewing.as_ref().map(|(_, m)| m.clone()).filter(|m| m.id == id));
        if let Some(row) = row {
            self.reload_images(&row);
        }
        Ok(())
    }

    /// Load a photo's thumbnail and preview again (the old ones stay up meanwhile) and
    /// forget its 1:1 preview: its Develop settings changed.
    fn reload_images(&mut self, row: &PhotoRow) {
        self.full.invalidate(row.id);
        if !row.missing {
            for size in [PreviewSize::Thumb, PreviewSize::Standard] {
                self.jobs.submit_visible(Job::Load { photo: PhotoRef::from(row), size });
            }
        }
    }

    pub fn source_label(&self) -> String {
        match self.source {
            Source::AllPhotographs => "Catalog : All Photographs".into(),
            Source::PreviousImport => "Catalog : Previous Import".into(),
            Source::QuickCollection => "Catalog : Quick Collection".into(),
            Source::Collection { id } => {
                let name =
                    self.collections.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_default();
                format!("Collection : {name}")
            }
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

    /// Shift+arrow: move to a neighbour and select everything from the anchor to it.
    pub fn extend_selection(&mut self, delta: isize) {
        let Some(i) = self.active_index() else { return self.move_active(delta) };
        let j = (i as isize + delta).clamp(0, self.photos.len() as isize - 1) as usize;
        let id = self.photos[j].id;
        self.click(id, egui::Modifiers::SHIFT);
        self.scroll_to_active = true;
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
    /// Collections with fresh counts (smart collection counts follow every edit).
    pub fn collections_fresh(&mut self) -> &[catalog::Collection] {
        if self.collections_gen != Some(self.meta_gen) {
            match self.catalog.collections() {
                Ok(c) => self.collections = c,
                Err(e) => log::warn!("reading collections: {e:#}"),
            }
            self.quick_count = self.catalog.quick_collection_count().unwrap_or(self.quick_count);
            self.collections_gen = Some(self.meta_gen);
        }
        &self.collections
    }

    /// Something about collections changed: re-read them, and the grid if it shows one.
    pub fn collections_changed(&mut self) -> Result<()> {
        self.meta_gen += 1;
        self.collections_gen = None;
        self.collections_fresh();
        if matches!(self.source, Source::Collection { .. } | Source::QuickCollection) {
            self.reload_photos()?;
        }
        Ok(())
    }

    /// Values of each Metadata filter column, computed only when the filter, source or
    /// metadata change.
    pub fn facets(&mut self, bar: &crate::filter::FilterBar) -> &[Vec<catalog::Facet>] {
        let key = FacetKey { source: self.source, bar: bar.clone(), meta_gen: self.meta_gen };
        if self.facets.as_ref().is_none_or(|(k, _)| *k != key) {
            let started = Instant::now();
            let columns = (0..crate::filter::COLUMNS)
                .map(|i| {
                    self.catalog.facets(self.source, &bar.criterion_before(i), bar.columns[i]).unwrap_or_else(
                        |e| {
                            log::warn!("metadata filter column: {e:#}");
                            Vec::new()
                        },
                    )
                })
                .collect();
            if started.elapsed() > Duration::from_millis(50) {
                log::warn!("slow metadata filter columns: {:.0?}", started.elapsed());
            }
            self.facets = Some((key, columns));
        }
        &self.facets.as_ref().expect("just set").1
    }

    /// A photo's pixel size as developed (file orientation applied; smaller than the file
    /// with Constrain Crop), cached.
    pub fn photo_dims(&mut self, id: i64) -> Option<egui::Vec2> {
        if let Some(d) = self.dims.get(&id) {
            return *d;
        }
        let d = self
            .catalog
            .metadata(id)
            .ok()
            .flatten()
            .and_then(|m| Some((m.width?, m.height?)))
            // As developed: Constrain Crop makes it smaller than the file.
            .map(|(w, h)| {
                let develop = self.index_of.get(&id).and_then(|&i| self.photos[i].develop.as_deref());
                let t = develop.and_then(imaging::develop::DevelopSettings::from_json).map(|s| s.transform);
                let (w, h) = t.map_or((w, h), |t| t.output_size(w, h));
                egui::vec2(w as f32, h as f32)
            });
        self.dims.insert(id, d);
        d
    }

    pub fn refresh_keywords(&mut self) -> Result<()> {
        self.keywords = self.catalog.keywords()?;
        let known: HashSet<i64> = self.keywords.iter().map(|k| k.id).collect();
        self.recent_keywords.retain(|id| known.contains(id));
        self.meta_gen += 1;
        Ok(())
    }

    pub fn note_recent_keyword(&mut self, id: i64) {
        self.recent_keywords.retain(|&k| k != id);
        self.recent_keywords.insert(0, id);
        self.recent_keywords.truncate(9);
    }

    /// Root-first names of a keyword, e.g. `["Places", "Norway"]`.
    pub fn keyword_path(&self, id: i64) -> Vec<String> {
        let mut path = Vec::new();
        let mut next = Some(id);
        while let Some(id) = next {
            let Some(k) = self.keywords.iter().find(|k| k.id == id) else { break };
            path.push(k.name.clone());
            next = k.parent_id;
        }
        path.reverse();
        path
    }

    /// IPTC values and keywords of the photos the panels edit (cached until the
    /// selection or any metadata changes).
    pub fn selection_meta(&mut self, view: View) -> &SelectionMeta {
        let ids = self.targets(view);
        let key = (ids.clone(), self.meta_gen);
        if self.selection_meta.as_ref().is_none_or(|m| m.key != key) {
            let meta = SelectionMeta::load(&self.catalog, &self.keywords, ids).unwrap_or_else(|e| {
                log::warn!("reading selection metadata: {e:#}");
                SelectionMeta::default()
            });
            self.selection_meta = Some(SelectionMeta { key, ..meta });
        }
        self.selection_meta.as_ref().expect("just set")
    }

    pub fn targets(&self, view: View) -> Vec<i64> {
        if view == View::Grid && self.selected.len() > 1 {
            let mut ids: Vec<i64> = self.selected.iter().copied().collect();
            ids.sort_by_key(|id| self.index_of.get(id));
            ids
        } else {
            self.active_photo().map(|p| p.id).into_iter().collect()
        }
    }

    fn update_rows(&mut self, ids: &[i64], f: impl Fn(&mut PhotoRow)) {
        if let Some((_, m)) = &mut self.viewing
            && ids.contains(&m.id)
        {
            f(m);
        }
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
                Event::ImportScanned { generation, result } => {
                    if let Some(d) = &mut self.import
                        && d.generation == generation
                    {
                        match result {
                            Ok(found) => {
                                d.checked = vec![true; found.len()];
                                d.candidates = Some(std::sync::Arc::new(found));
                                d.error = None;
                            }
                            Err(e) => {
                                d.candidates = Some(Default::default());
                                d.error = Some(e);
                            }
                        }
                    }
                }
                Event::ImportThumb { key, image } => {
                    if let Some(d) = &mut self.import
                        && key >> 24 == d.generation as i64
                    {
                        d.thumbs.insert(ctx, key, image);
                    }
                }
                Event::ImportDone { result } => match result {
                    Ok(r) => {
                        let mut msg = format!(
                            "Imported {} photo{}",
                            r.imported,
                            if r.imported == 1 { "" } else { "s" }
                        );
                        if !r.failed.is_empty() {
                            msg += &format!(", {} failed: {}", r.failed.len(), r.failed[0].1);
                        }
                        if r.cancelled {
                            msg += " (cancelled)";
                        }
                        messages.push(msg);
                        // Like LrC, show what was just imported.
                        self.source = Source::PreviousImport;
                        self.meta_gen += 1;
                        if let Err(e) = self
                            .refresh_tree()
                            .and_then(|_| self.refresh_keywords())
                            .and_then(|()| self.reload_photos())
                        {
                            messages.push(format!("Catalog error: {e:#}"));
                        }
                    }
                    Err(e) => messages.push(format!("Import failed: {e}")),
                },
                Event::ExportDone { result, reveal } => match result {
                    Ok(r) => {
                        let n = r.exported.len();
                        let mut msg = format!("Exported {n} file{}", if n == 1 { "" } else { "s" });
                        if r.skipped > 0 {
                            msg += &format!(", skipped {} (already there)", r.skipped);
                        }
                        if let Some((path, e)) = r.failed.first() {
                            let name = path.file_name().unwrap_or_default().to_string_lossy();
                            msg += &format!(", {} failed ({name}: {e})", r.failed.len());
                        }
                        if r.cancelled {
                            msg += " (cancelled)";
                        }
                        messages.push(msg);
                        if let Some(path) = reveal {
                            crate::platform::reveal(&path);
                        }
                        if !r.added.is_empty()
                            && let Err(e) = self.refresh_tree().and_then(|()| self.reload_photos())
                        {
                            messages.push(format!("Catalog error: {e:#}"));
                        }
                    }
                    Err(e) => messages.push(format!("Export failed: {e}")),
                },
                Event::EditsChecked { found } => {
                    for (original, path) in self.edits.checked(found) {
                        self.jobs.submit_normal(Job::AddEdited { original, path });
                    }
                }
                Event::EditAdded { path, result } => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    match result {
                        Ok((id, new)) => {
                            for cache in [&mut self.thumbs, &mut self.previews, &mut self.full] {
                                cache.invalidate(id);
                            }
                            self.dims.remove(&id);
                            self.histograms.remove(&id);
                            self.metadata = None;
                            self.meta_gen += 1;
                            if let Err(e) = self.refresh_tree().and_then(|()| self.reload_photos()) {
                                messages.push(format!("Catalog error: {e:#}"));
                            }
                            let verb = if new { "Added" } else { "Updated" };
                            messages.push(format!("{verb} {name} from NegPy"));
                        }
                        Err(e) => messages.push(format!("Could not add {name} from NegPy: {e}")),
                    }
                }
                Event::XmpDone { read, quiet, done, skipped, failed } => {
                    if read {
                        // Ratings, labels, rotation, keywords... may all have changed.
                        if let Err(e) = self.refresh_keywords().and_then(|()| self.reload_photos()) {
                            messages.push(format!("Catalog error: {e:#}"));
                        }
                    } else {
                        self.update_rows(&done, |p| p.metadata_dirty = false);
                    }
                    self.meta_gen += 1;
                    let verb = if read { "Read metadata from" } else { "Saved metadata to" };
                    let n = done.len();
                    let mut msg = format!("{verb} {n} file{}", if n == 1 { "" } else { "s" });
                    if skipped > 0 {
                        msg += &format!(", {skipped} without a sidecar");
                    }
                    if let Some((_, first)) = failed.first() {
                        msg += &format!(", {} failed: {first}", failed.len());
                    }
                    // Quiet when auto-writing succeeds; LrC doesn't announce those either.
                    if !quiet || !failed.is_empty() {
                        messages.push(msg);
                    }
                }
                Event::MetadataUpdated { photo_id } => {
                    self.dims.remove(&photo_id);
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

pub struct FotaroApp {
    pub ui: UiState,
    last_catalog: Option<PathBuf>,
    pub session: Option<Session>,
    pub module: Module,
    pub view: View,
    pub lights: Lights,
    pub fullscreen: bool,
    pub zoom: Zoom,
    pub compare: Option<Compare>,
    /// Ctrl+K: put the cursor in the Keywording panel's entry field next frame.
    pub focus_keywording: bool,
    /// Ctrl+F: put the cursor in the Text filter next frame.
    pub focus_find: bool,
    pub show_shortcuts: bool,
    /// Help ▸ About Fotaro.
    pub show_about: bool,
    /// New releases on GitHub: checked for, downloaded and installed.
    pub updater: crate::update::Updater,
    pub toast: Option<Toast>,
    pub error: Option<String>,
    /// Develop presets (app-wide, files in the app's data folder) and their panel's state.
    pub presets: crate::presets::PresetStore,
    pub presets_ui: crate::ui::presets::PresetsUi,
    /// The open modal dialog, if any.
    pub dialog: Option<Dialog>,
    pub commands: Vec<Command>,
    /// External editors as last looked for (missing: not looked for yet, or the settings
    /// changed).
    pub editors: std::collections::HashMap<crate::editors::Editor, Result<crate::editors::Found, String>>,
    /// The External Editing settings window is open.
    pub edit_settings: bool,
    /// Compare with Original: two members of one picture side by side (the original, then
    /// the member that was shown).
    pub member_compare: Option<(PhotoRow, PhotoRow)>,
    /// Rects of panels drawn this frame (dimmed by Lights Out).
    pub panel_rects: Vec<egui::Rect>,
    /// Grid geometry from the last frame, for Up/Down and scroll-to-active.
    pub grid_cols: usize,
    /// Whole rows visible in the grid (for Page Up / Page Down).
    pub grid_rows_visible: usize,
    pub grid_scroll: (f32, f32),
    perf: crate::perf::FrameStats,
}

impl FotaroApp {
    pub fn new(cc: &eframe::CreationContext<'_>, args: crate::Args) -> Self {
        crate::ui::theme::apply(&cc.egui_ctx);
        let settings: Settings =
            cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();
        let mut app = FotaroApp {
            ui: settings.ui,
            last_catalog: None,
            session: None,
            module: Module::Library,
            view: View::Grid,
            lights: Lights::On,
            fullscreen: false,
            zoom: Zoom::default(),
            compare: None,
            focus_keywording: false,
            focus_find: false,
            show_shortcuts: false,
            show_about: false,
            updater: Default::default(),
            toast: None,
            error: None,
            dialog: None,
            commands: Vec::new(),
            editors: Default::default(),
            edit_settings: false,
            member_compare: None,
            panel_rects: Vec::new(),
            grid_cols: 1,
            grid_rows_visible: 1,
            grid_scroll: (0.0, 0.0),
            perf: crate::perf::FrameStats::from_env(),
            presets: Default::default(),
            presets_ui: Default::default(),
        };
        // Presets and imported LUTs live in the app's data folder (PL_DATA_DIR overrides).
        let data = std::env::var_os("PL_DATA_DIR").map(PathBuf::from).or_else(|| {
            directories::ProjectDirs::from("", "", crate::APP_NAME).map(|d| d.data_dir().to_path_buf())
        });
        if let Some(data) = &data {
            imaging::develop::set_lut_store(data.join("LUTs"));
            crate::lens::set_dir(data.join("lensfun"));
        }
        app.presets = crate::presets::PresetStore::load(data.map(|d| d.join("Develop Presets")));
        if let Some(dir) = args.catalog {
            let create = !dir.join("catalog.db").is_file();
            app.open_catalog(&cc.egui_ctx, &dir, create);
        } else if let Some(dir) = settings.last_catalog.filter(|d| d.join("catalog.db").is_file()) {
            app.open_catalog(&cc.egui_ctx, &dir, false);
        }
        if let (Some(s), Some(root)) = (&app.session, args.add) {
            s.jobs.submit_normal(Job::Sync { root });
        }
        if let Some(src) = args.import {
            app.execute(&cc.egui_ctx, Command::OpenImport(None));
            app.execute(&cc.egui_ctx, Command::ImportSource(src));
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
        base.join("Fotaro").join(format!("Fotaro.{}", catalog::CATALOG_EXTENSION))
    }

    fn open_catalog(&mut self, ctx: &egui::Context, dir: &Path, create: bool) {
        let result = if create { Catalog::create(dir) } else { Catalog::open(dir) }
            .and_then(|c| Session::open(ctx, c, self.ui.sort));
        match result {
            Ok(mut session) => {
                session.develop.ai_gpu = self.ui.ai_gpu;
                log::info!("opened catalog {}", dir.display());
                self.session = Some(session);
                self.last_catalog = Some(dir.to_path_buf());
                self.view = View::Grid;
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "{} - Fotaro",
                    dir.file_stem().unwrap_or_default().to_string_lossy()
                )));
            }
            Err(e) => self.error = Some(format!("{e:#}")),
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
        let targets = s.targets(self.view);
        if targets.is_empty() {
            return;
        }
        // Rating, flag and label belong to the stack: every member of the targets' stacks.
        let ids = match s.catalog.unit_photo_ids(&targets) {
            Ok(ids) => ids,
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        };
        for id in &ids {
            if let Err(e) = f(&s.catalog, *id) {
                self.error = Some(format!("{e:#}"));
                return;
            }
        }
        s.update_rows(&ids, update);
        let n = targets.len();
        self.metadata_changed(&ids);
        if advance {
            self.step(1);
        }
        self.notify(describe(n));
    }

    fn action(&mut self, ctx: &egui::Context, action: Action) {
        if let Some(sc) = SHORTCUTS.iter().find(|s| s.action == action)
            && let Status::Planned(phase) = sc.status
        {
            self.notify(format!("{} arrives in {phase}", sc.description));
            return;
        }
        // Delete with the Remove tool open: the selected spot.
        if self.module == Module::Develop
            && action == Action::RemoveFromCollection
            && let Some(s) = &mut self.session
            && s.develop.removing.is_some()
        {
            if crate::ui::delete_spot(&mut s.develop) {
                self.develop_op(|d, c| d.commit(c, None));
            }
            return;
        }
        // With the crop tool open, X swaps portrait / landscape and Enter closes it, as in LrC.
        if self.module == Module::Develop
            && let Some(s) = &mut self.session
            && s.develop.cropping.is_some()
        {
            match action {
                Action::SetFlag { flag: Flag::Rejected, advance: false } => {
                    if crate::ui::swap_orientation(&mut s.develop) {
                        self.develop_op(|d, c| d.commit(c, None));
                    }
                    return;
                }
                Action::LoupeView => {
                    crate::ui::set_cropping(&mut s.develop, false);
                    return;
                }
                _ => {}
            }
        }
        // In Compare and Survey the arrow keys move within the view.
        if matches!(self.view, View::Compare | View::Survey)
            && matches!(action, Action::Next | Action::Previous | Action::Up | Action::Down)
        {
            self.view_step(action);
            return;
        }
        let for_n = |n: usize| if n > 1 { format!(" for {n} Photos") } else { String::new() };
        match action {
            Action::GridView => {
                self.leave_develop();
                self.view = View::Grid;
            }
            Action::LoupeView => {
                self.leave_develop();
                if self.session.as_ref().is_some_and(|s| s.active.is_some()) {
                    self.view = View::Loupe;
                }
            }
            // Space in Develop is the hand tool in LrC; there is nothing to toggle.
            Action::ToggleLoupe if self.module == Module::Develop => {}
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
                if egui::Popup::is_any_open(ctx) {
                    // Esc closes the open menu and nothing else.
                    egui::Popup::close_all(ctx);
                } else if self.fullscreen {
                    self.fullscreen = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                } else if self.lights != Lights::On {
                    self.lights = Lights::On;
                } else if self.module == Module::Develop {
                    if let Some(s) = &mut self.session {
                        s.develop.picking_wb = false;
                        s.develop.picking_detail = false;
                        // Remove: Esc closes it.
                        if s.develop.removing.is_some() {
                            crate::ui::set_removing(&mut s.develop, false);
                        }
                        // Crop: Esc puts the Straighten tool away, then closes the crop tool.
                        let d = &mut s.develop;
                        match &mut d.cropping {
                            Some(t) if t.straighten => t.straighten = false,
                            Some(_) => crate::ui::set_cropping(d, false),
                            None => {}
                        }
                        // Masking: Esc drops the tool, then leaves masking.
                        let d = &mut s.develop;
                        match &mut d.masking {
                            Some(m) if m.tool.is_some() => m.tool = None,
                            Some(_) => crate::ui::set_masking(d, false),
                            None => {}
                        }
                    }
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
            Action::ExtendSelection(step) => {
                // Rows: isize::MAX / MIN stand for "a row down / up" in the Grid.
                let cols = if self.view == View::Grid { self.grid_cols.max(1) as isize } else { 1 };
                let delta = match step {
                    isize::MAX => cols,
                    isize::MIN => -cols,
                    d => d,
                };
                self.with_session(|s| s.extend_selection(delta));
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
            Action::Import => self.execute(ctx, Command::OpenImport(None)),
            Action::Export => self.execute(ctx, Command::OpenExport),
            Action::ExportWithPrevious => self.execute(ctx, Command::ExportWithPrevious),
            Action::ShowShortcuts => self.show_shortcuts ^= true,
            Action::SaveMetadata => self.save_metadata(),
            Action::QuickCollection => {
                let view = self.view;
                let Some(s) = &mut self.session else { return };
                let ids = s.targets(view);
                if ids.is_empty() {
                    return;
                }
                match s.catalog.toggle_quick_collection(&ids) {
                    Ok(added) => {
                        s.update_rows(&ids, |p| p.in_quick_collection = added);
                        if let Err(e) = s.collections_changed() {
                            self.error = Some(format!("{e:#}"));
                        }
                        let what =
                            if added { "Added to Quick Collection" } else { "Removed from Quick Collection" };
                        self.notify(what);
                    }
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Action::NewCollection => self.execute(
                ctx,
                Command::CreateCollection { parent: None, kind: catalog::CollectionKind::Collection },
            ),
            Action::ShowQuickCollection => self.execute(ctx, Command::SetSource(Source::QuickCollection)),
            Action::ClearQuickCollection => {
                let Some(s) = &mut self.session else { return };
                let result = s.catalog.clear_quick_collection().and_then(|()| {
                    for p in &mut s.photos {
                        p.in_quick_collection = false;
                    }
                    s.collections_changed()
                });
                match result {
                    Ok(()) => self.notify("Quick Collection cleared"),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Action::Stack
            | Action::Unstack
            | Action::ToggleStack
            | Action::MoveToTopOfStack
            | Action::PreviousMember
            | Action::NextMember
            | Action::DetachFromPicture
            | Action::CompareWithOriginal => self.stacking(action),
            Action::ToggleFilters => {
                self.ui.filter.enabled ^= true;
                self.notify(if self.ui.filter.enabled { "Filters on" } else { "Filters off" });
            }
            Action::Find => {
                self.ui.show_filter_bar = true;
                self.ui.filter.enabled = true;
                self.ui.filter.text_on = true;
                self.focus_find = true;
                self.view = View::Grid;
            }
            Action::DeleteRejected => {
                let Some(s) = &self.session else { return };
                match s.catalog.rejected_in(s.source) {
                    Ok(ids) if ids.is_empty() => {
                        let msg = format!("No rejected photos in {}", s.source_label());
                        self.notify(msg);
                    }
                    Ok(ids) => {
                        let source = s.source_label();
                        self.dialog = Some(Dialog::ConfirmRemoveRejected { ids, source });
                    }
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            // Delete while masking: the selected mask component.
            Action::RemoveFromCollection
                if self.module == Module::Develop
                    && self.session.as_ref().is_some_and(|s| s.develop.masking.is_some()) =>
            {
                if let Some(s) = &mut self.session
                    && crate::ui::delete_selected(&mut s.develop)
                {
                    self.develop_op(|d, c| d.commit(c, None));
                }
            }
            Action::RemoveFromCollection => {
                let view = self.view;
                let Some(s) = &mut self.session else { return };
                let Source::Collection { id } = s.source else {
                    // Outside collections, Delete removes virtual copies (after asking).
                    let copies: Vec<i64> = s
                        .targets(view)
                        .into_iter()
                        .filter(|id| s.index_of.get(id).is_some_and(|&i| s.photos[i].virtual_copy.is_some()))
                        .collect();
                    if copies.is_empty() {
                        self.notify("Delete removes virtual copies, or photos from a collection; Photo ▸ Delete Rejected Photos removes rejected ones from the catalog");
                    } else {
                        self.dialog = Some(Dialog::ConfirmDeleteVirtualCopies { ids: copies });
                    }
                    return;
                };
                if s.collections.iter().any(|c| c.id == id && c.kind != catalog::CollectionKind::Collection) {
                    self.notify("Smart collections choose their photos from their rules");
                    return;
                }
                let ids = s.targets(view);
                match s
                    .catalog
                    .remove_from_collection(id, &ids)
                    .and_then(|n| s.collections_changed().map(|()| n))
                {
                    Ok(n) => self.notify(format!(
                        "Removed {n} photo{} from the collection",
                        if n == 1 { "" } else { "s" }
                    )),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Action::Keywording => {
                if self.session.as_ref().and_then(|s| s.active).is_some() {
                    self.ui.show_right = true;
                    self.ui.right.set_open("Keywording", true, crate::ui::sidebars::RIGHT);
                    self.focus_keywording = true;
                }
            }
            // Planned actions are handled above via their Status.
            Action::CompareView => {
                self.leave_develop();
                self.enter_compare();
            }
            Action::LibraryModule => self.leave_develop(),
            Action::DevelopModule => self.enter_develop(),
            Action::BeforeAfter
            | Action::AutoTone
            | Action::AutoWhiteBalance
            | Action::WhiteBalanceSelector
            | Action::ClippingIndicators
            | Action::Masking
            | Action::NewBrushMask
            | Action::CropTool
            | Action::RemoveTool
            | Action::NewSource
            | Action::VisualizeSpots
            | Action::ResetCrop
            | Action::BrushSize(_)
            | Action::BrushFeather(_)
            | Action::NewLinearMask
            | Action::NewRadialMask
            | Action::MaskOverlay
            | Action::ResetSettings
            | Action::CopySettings
            | Action::PasteSettings
            | Action::NewSnapshot
            | Action::Undo
            | Action::Redo => {
                if self.module == Module::Develop {
                    self.develop_action(action);
                }
            }
            Action::SurveyView => {
                self.leave_develop();
                if self.session.as_ref().is_some_and(|s| !s.selected.is_empty()) {
                    self.compare = None;
                    self.view = View::Survey;
                }
            }
            Action::CreateVirtualCopy => self.create_virtual_copies(),
            Action::EditIn => self.execute(ctx, Command::EditIn(self.ui.edit_in.primary)),
            Action::EditInOther => self.execute(ctx, Command::EditIn(self.ui.edit_in.secondary)),
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
        self.metadata_changed(&ids);
        let dir = if quarter_turns == 1 { "Right" } else { "Left" };
        let n = ids.len();
        self.notify(if n > 1 { format!("Rotate {dir} for {n} Photos") } else { format!("Rotate {dir}") });
    }

    /// Next photo after a rating/flag with Auto Advance: within Compare (the Candidate)
    /// and Survey (the selected photos), elsewhere the next photo in the grid.
    fn step(&mut self, delta: isize) {
        match self.view {
            View::Compare | View::Survey => {
                self.view_step(if delta > 0 { Action::Next } else { Action::Previous })
            }
            _ => self.with_session(|s| s.move_active(delta)),
        }
    }

    /// LrC Compare (C): the active photo is the Select; the Candidate is another
    /// selected photo, or else the next one.
    fn enter_compare(&mut self) {
        let Some(s) = &mut self.session else { return };
        let Some(select) = s.active else { return };
        let ordered: Vec<i64> = s.photos.iter().map(|p| p.id).collect();
        let i = s.index_of[&select];
        let candidate = ordered
            .iter()
            .copied()
            .find(|id| *id != select && s.selected.contains(id))
            .or_else(|| ordered.get(i + 1).copied())
            .or_else(|| i.checked_sub(1).and_then(|j| ordered.get(j).copied()));
        let Some(candidate) = candidate else {
            self.notify("Compare needs at least two photos");
            return;
        };
        self.compare = Some(Compare { select, candidate });
        self.view = View::Compare;
    }

    /// Arrow keys in Compare (←/→ Candidate, ↑ Make Select, ↓ Swap) and Survey (move
    /// between the selected photos).
    fn view_step(&mut self, action: Action) {
        let Some(s) = &mut self.session else { return };
        let forward = matches!(action, Action::Next | Action::Down);
        match self.view {
            View::Compare => {
                let Some(mut c) = self.compare else { return };
                // The neighbour of `from` in the grid, skipping `skip`.
                let neighbour = |from: i64, skip: i64, forward: bool| -> Option<i64> {
                    let i = *s.index_of.get(&from)? as isize;
                    let step = if forward { 1 } else { -1 };
                    let mut j = i + step;
                    while let Some(p) = usize::try_from(j).ok().and_then(|j| s.photos.get(j)) {
                        if p.id != skip {
                            return Some(p.id);
                        }
                        j += step;
                    }
                    None
                };
                match action {
                    Action::Next | Action::Previous => {
                        if let Some(n) = neighbour(c.candidate, c.select, forward) {
                            c.candidate = n;
                        }
                        s.active = Some(c.candidate);
                    }
                    Action::Up => {
                        // Make Select: the Candidate wins; the next photo is the new Candidate.
                        c.select = c.candidate;
                        if let Some(n) = neighbour(c.select, c.select, true)
                            .or_else(|| neighbour(c.select, c.select, false))
                        {
                            c.candidate = n;
                        }
                        s.active = Some(c.select);
                    }
                    _ => std::mem::swap(&mut c.select, &mut c.candidate),
                }
                self.compare = Some(c);
                s.scroll_to_active = true;
            }
            View::Survey => {
                let ids: Vec<i64> =
                    s.photos.iter().map(|p| p.id).filter(|id| s.selected.contains(id)).collect();
                let Some(i) = s.active.and_then(|a| ids.iter().position(|id| *id == a)) else { return };
                let j = if forward { (i + 1).min(ids.len() - 1) } else { i.saturating_sub(1) };
                s.active = Some(ids[j]);
            }
            _ => {}
        }
    }

    /// LrC Photo ▸ Create Virtual Copy (Ctrl+'), for each target photo.
    fn create_virtual_copies(&mut self) {
        let view = self.view;
        let Some(s) = &mut self.session else { return };
        let ids = s.targets(view);
        let mut made = Vec::new();
        for id in ids {
            match s.catalog.create_virtual_copy(id) {
                Ok(copy) => made.push(copy),
                Err(e) => {
                    self.error = Some(format!("{e:#}"));
                    return;
                }
            }
        }
        if let Err(e) = s.refresh_tree().and_then(|()| s.reload_photos()) {
            self.error = Some(format!("{e:#}"));
        }
        // Like LrC, the new copy becomes the active photo.
        if let Some(&last) = made.last() {
            s.click(last, egui::Modifiers::NONE);
            s.scroll_to_active = true;
        }
        let n = made.len();
        self.notify(if n == 1 {
            "Created virtual copy".to_string()
        } else {
            format!("Created {n} virtual copies")
        });
    }

    /// Photo ▸ Stacking: group pictures (Ctrl+G), ungroup (Ctrl+Shift+G), show / hide
    /// the version strip (S), make the member shown the active one (Shift+S), step
    /// through members (Alt+← / Alt+→), detach a member, compare with the original.
    fn stacking(&mut self, action: Action) {
        let view = self.view;
        let Some(s) = &mut self.session else { return };
        let ids = s.targets(view);
        let shown = s.active_photo().cloned();
        let result: Result<Option<String>> = match action {
            Action::Stack if ids.len() < 2 => Ok(Some("Select two or more photos to group".into())),
            Action::Stack => {
                s.catalog.group_photos(&ids).map(|_| Some(format!("Grouped {} pictures", ids.len())))
            }
            Action::Unstack => s.catalog.ungroup(&ids).map(|n| (n > 0).then(|| "Ungrouped".to_string())),
            Action::ToggleStack => {
                self.ui.show_version_strip = !self.ui.show_version_strip;
                return;
            }
            Action::MoveToTopOfStack => match &shown {
                Some(p) if p.unit.is_stack() => {
                    let id = p.id;
                    s.catalog.set_active(id).map(|()| {
                        // The unit now shows this photo: it is the active cell.
                        s.selected.retain(|x| Some(*x) != s.active);
                        s.active = Some(id);
                        s.selected.insert(id);
                        s.viewing = None;
                        Some(format!("{} is now shown for its stack", p.file_name))
                    })
                }
                _ => Ok(None),
            },
            Action::PreviousMember | Action::NextMember => {
                s.step_member(action == Action::NextMember);
                return;
            }
            Action::DetachFromPicture => match &shown {
                Some(p) if p.unit.members > p.unit.pictures => {
                    let id = p.id;
                    s.catalog.detach(id).map(|()| {
                        s.viewing = None;
                        Some(format!("{} is now a picture of its own", p.file_name))
                    })
                }
                _ => Ok(Some("Pick a member of a picture in the version strip first".into())),
            },
            Action::CompareWithOriginal => {
                let members = s.strip_members().to_vec();
                let Some(shown) = shown else { return };
                let original = members
                    .iter()
                    .filter(|m| m.photo.unit.picture_id == shown.unit.picture_id)
                    .find(|m| m.photo.unit.role == catalog::Role::Original)
                    .map(|m| m.photo.clone());
                match original {
                    Some(o) if o.id != shown.id => {
                        self.member_compare = Some((o, shown));
                        self.view = View::Compare;
                        return;
                    }
                    _ => Ok(Some("This photo is the original of its picture".into())),
                }
            }
            _ => Ok(None),
        };
        let reloaded = result.and_then(|msg| s.reload_photos().map(|()| msg));
        // Select the new group's cell (its other pictures are now inside it).
        if action == Action::Stack
            && reloaded.is_ok()
            && let Some(cell) = s.active
        {
            s.click(cell, egui::Modifiers::NONE);
        }
        s.meta_gen += 1;
        match reloaded {
            Ok(Some(msg)) => self.notify(msg),
            Ok(None) => {}
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    /// After any catalog metadata edit: flag the photos as needing saving (the grid
    /// badge), refresh cached panel contents, and write sidecars if auto-write is on.
    /// Run a Develop operation that may save settings; a saved photo's previews are then
    /// rebuilt, and its sidecar needs writing, as for any metadata change.
    /// [`Self::develop_op`] for the panels.
    pub fn develop_with(
        &mut self,
        f: impl FnOnce(&mut crate::develop::DevelopState, &mut Catalog) -> Result<crate::develop::Changed>,
    ) {
        self.develop_op(f);
    }

    fn develop_op(
        &mut self,
        f: impl FnOnce(&mut crate::develop::DevelopState, &mut Catalog) -> Result<crate::develop::Changed>,
    ) {
        let Some(s) = &mut self.session else { return };
        match f(&mut s.develop, &mut s.catalog) {
            Ok(Some(id)) => {
                if let Err(e) = s.develop_changed(id) {
                    self.error = Some(format!("{e:#}"));
                }
                self.metadata_changed(&[id]);
            }
            Ok(None) => {}
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    /// Each frame in Develop: follow the active photo and take the renders that arrived.
    fn develop_frame(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        self.develop_op(|d, catalog| d.poll(&ctx, catalog));
        let photo = self.session.as_ref().and_then(|s| s.active_photo().cloned());
        match photo {
            Some(p) => self.develop_op(|d, catalog| d.show(catalog, &p)),
            None => self.leave_develop(),
        }
    }

    /// D: develop the active photo (LrC shows it alone, so the view becomes the Loupe).
    fn enter_develop(&mut self) {
        if self.session.as_ref().and_then(|s| s.active_photo()).is_none() {
            self.notify("Select a photo to develop");
            return;
        }
        self.module = Module::Develop;
        self.view = View::Loupe;
        self.compare = None;
        self.zoom.zoomed = false;
    }

    /// Back to the Library (G, E, C, N, Ctrl+Alt+1, the module picker), saving any
    /// change still pending.
    fn leave_develop(&mut self) {
        if self.module != Module::Develop {
            return;
        }
        self.module = Module::Library;
        self.develop_op(|d, catalog| {
            d.picking_wb = false;
            d.picking_detail = false;
            d.set_sharpen_view(None);
            d.masking = None;
            d.commit(catalog, None)
        });
    }

    fn develop_action(&mut self, action: Action) {
        use imaging::develop::{DevelopSettings, WbPreset};
        let Some(s) = &mut self.session else { return };
        let d = &mut s.develop;
        match action {
            Action::BeforeAfter => d.set_before(!d.before),
            Action::WhiteBalanceSelector => {
                d.picking_wb = !d.picking_wb;
                d.picking_detail = false;
            }
            Action::AutoTone => d.auto_tone(),
            Action::ClippingIndicators => d.set_clipping(!d.clipping),
            Action::Masking => crate::ui::set_masking(d, d.masking.is_none()),
            Action::NewBrushMask => crate::ui::new_mask_tool(d, crate::develop::ShapeKind::Brush),
            Action::CropTool => crate::ui::set_cropping(d, d.cropping.is_none()),
            Action::RemoveTool => crate::ui::set_removing(d, d.removing.is_none()),
            Action::NewSource => crate::ui::new_source(d),
            Action::VisualizeSpots if d.removing.is_some() => {
                d.set_visualize_spots(!d.visualize_spots, d.spot_threshold)
            }
            Action::ResetCrop => self.develop_op(|d, c| {
                let mut s = d.settings.clone();
                s.transform.crop = Default::default();
                d.apply(c, s, "Reset Crop")
            }),
            // [ ] size the Remove tool's spots while it is open, else the brush.
            Action::BrushSize(dir) if d.removing.is_some() => crate::ui::step_spot_size(d, dir),
            Action::BrushSize(dir) => d.brush.step_size(dir),
            Action::BrushFeather(dir) => d.brush.step_feather(dir),
            Action::NewLinearMask => crate::ui::new_mask_tool(d, crate::develop::ShapeKind::Linear),
            Action::NewRadialMask => crate::ui::new_mask_tool(d, crate::develop::ShapeKind::Radial),
            Action::MaskOverlay => {
                if let Some(m) = &mut d.masking {
                    m.overlay = !m.overlay;
                }
                d.request_renders();
            }
            Action::CopySettings => {
                // LrC's Copy Settings dialog, ticked for what differs from a new photo
                // (the per-photo groups only if they were copied last time).
                let last = d.clipboard.as_ref().map(|(_, g)| g.clone());
                let mut groups = imaging::develop::changed_groups(&d.settings, d.is_raw());
                groups.retain(|g| !g.per_photo() || last.as_ref().is_some_and(|l| l.contains(g)));
                self.presets_ui.copy = Some(groups);
            }
            Action::NewSnapshot => crate::ui::presets::take_snapshot(self),
            Action::AutoWhiteBalance => self.develop_op(|d, c| {
                let s = DevelopSettings { wb: WbPreset::Auto, ..d.settings.clone() };
                d.apply(c, s, "White Balance: Auto")
            }),
            Action::ResetSettings => self.develop_op(|d, c| {
                let defaults = d.defaults();
                d.apply(c, defaults, "Reset")
            }),
            Action::PasteSettings => match d.clipboard.clone() {
                Some((clip, groups)) => self.develop_op(|d, c| {
                    let mut s = d.settings.clone();
                    imaging::develop::copy_groups(&clip, &mut s, &groups);
                    d.apply(c, s, "Paste Settings")
                }),
                None => self.notify("Copy settings from a photo first"),
            },
            Action::Undo => self.develop_op(|d, c| d.undo(c)),
            Action::Redo => self.develop_op(|d, c| d.redo(c)),
            _ => {}
        }
    }

    fn metadata_changed(&mut self, ids: &[i64]) {
        let Some(s) = &mut self.session else { return };
        s.meta_gen += 1;
        s.update_rows(ids, |p| p.metadata_dirty = true);
        if self.ui.auto_write_xmp && !ids.is_empty() {
            s.jobs.submit_normal(Job::WriteXmp { photo_ids: ids.to_vec(), quiet: true });
        }
        // Filtered views and smart collections follow edits immediately, as in LrC.
        let smart = matches!(s.source, Source::Collection { id }
            if s.collections.iter().any(|c| c.id == id && c.kind == catalog::CollectionKind::Smart));
        if (s.filter != catalog::Criterion::AllOf(vec![]) || smart)
            && let Err(e) = s.reload_photos()
        {
            self.error = Some(format!("{e:#}"));
        }
    }

    /// LrC Metadata ▸ Save Metadata to Files (Ctrl+S) for the target photos.
    fn save_metadata(&mut self) {
        let Some(s) = &self.session else { return };
        let ids: Vec<i64> = s
            .targets(self.view)
            .into_iter()
            .filter(|id| s.index_of.get(id).is_some_and(|&i| !s.photos[i].missing))
            .collect();
        if ids.is_empty() {
            return;
        }
        let n = ids.len();
        s.jobs.submit_normal(Job::WriteXmp { photo_ids: ids, quiet: false });
        self.notify(if n == 1 {
            "Saving metadata".to_string()
        } else {
            format!("Saving metadata for {n} photos")
        });
    }

    /// Tag (or untag) the target photos with keyword `id`.
    fn set_keyword(&mut self, id: i64, on: bool) {
        let view = self.view;
        let Some(s) = &mut self.session else { return };
        let ids = s.targets(view);
        let result = if on { s.catalog.add_keyword(&ids, id) } else { s.catalog.remove_keyword(&ids, id) };
        if let Err(e) = result.and_then(|()| s.refresh_keywords()) {
            self.error = Some(format!("{e:#}"));
            return;
        }
        if on {
            s.note_recent_keyword(id);
        }
        self.metadata_changed(&ids);
    }

    /// Add keywords typed in LrC syntax (`a, b < parent`) to the target photos.
    fn add_keywords(&mut self, text: &str) {
        let paths = catalog::parse_keyword_input(text);
        let Some(s) = &mut self.session else { return };
        let mut ids = Vec::new();
        for path in &paths {
            match s.catalog.ensure_keyword(path) {
                Ok(id) => ids.push(id),
                Err(e) => {
                    self.error = Some(format!("{e:#}"));
                    return;
                }
            }
        }
        if let Err(e) = s.refresh_keywords() {
            self.error = Some(format!("{e:#}"));
        }
        for id in ids {
            self.set_keyword(id, true);
        }
    }

    /// The Keyword Tags text was edited: removed names are untagged, new ones added.
    /// Names marked `*` (on only some photos) are left as they are.
    fn set_keyword_tags(&mut self, before: &str, after: &str) {
        let names = |s: &str| -> Vec<String> {
            s.split([',', ';']).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect()
        };
        let (before, after) = (names(before), names(after));
        let Some(s) = &mut self.session else { return };
        let tags = s.selection_meta(self.view).tags.clone();
        let mut removed = Vec::new();
        for name in before.iter().filter(|n| !after.contains(n)) {
            let plain = name.trim_end_matches('*');
            if let Some((id, ..)) = tags.iter().find(|(_, n, _)| n == plain) {
                removed.push(*id);
            }
        }
        let added: Vec<String> = after.into_iter().filter(|n| !before.contains(n)).collect();
        for id in removed {
            self.set_keyword(id, false);
        }
        if !added.is_empty() {
            self.add_keywords(&added.join(", "));
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
            Command::DevelopCommit(label) => self.develop_op(|d, c| d.commit(c, label)),
            Command::DevelopUpright(mode) => self.develop_op(|d, c| d.upright(c, mode)),
            Command::DevelopRevert(step) => self.develop_op(|d, c| d.revert_to(c, step)),
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
            Command::AddKeywords(text) => self.add_keywords(&text),
            Command::SetKeyword { id, on } => self.set_keyword(id, on),
            Command::SetKeywordTags { before, after } => self.set_keyword_tags(&before, &after),
            Command::SetIptc { field, value } => {
                let view = self.view;
                let Some(s) = &mut self.session else { return };
                let ids = s.targets(view);
                match s.catalog.set_iptc(&ids, field, &value) {
                    Ok(()) => self.metadata_changed(&ids),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Command::CreateKeyword(parent) => {
                self.dialog = Some(Dialog::CreateKeyword { parent, name: String::new() })
            }
            Command::ApplyCreateKeyword { parent, name } => {
                let Some(s) = &mut self.session else { return };
                match s.catalog.create_keyword(parent, &name).and_then(|_| s.refresh_keywords()) {
                    Ok(()) => {
                        if let Some(p) = parent {
                            s.collapsed_keywords.remove(&p);
                        }
                    }
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Command::RenameKeyword(id) => {
                let Some(s) = &self.session else { return };
                if let Some(k) = s.keywords.iter().find(|k| k.id == id) {
                    self.dialog = Some(Dialog::RenameKeyword { id, name: k.name.clone() });
                }
            }
            Command::ApplyRenameKeyword { id, name } => {
                let Some(s) = &mut self.session else { return };
                let result = s
                    .catalog
                    .rename_keyword(id, &name)
                    .and_then(|()| s.refresh_keywords())
                    .and_then(|()| s.catalog.dirty_photo_ids());
                match result {
                    Ok(dirty) => self.metadata_changed(&dirty),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Command::DeleteKeyword(id) => {
                let Some(s) = &self.session else { return };
                if let Some(k) = s.keywords.iter().find(|k| k.id == id) {
                    // Count photos with this keyword or any keyword below it.
                    let mut below = vec![id];
                    let mut i = 0;
                    while i < below.len() {
                        let parent = below[i];
                        below.extend(s.keywords.iter().filter(|k| k.parent_id == Some(parent)).map(|k| k.id));
                        i += 1;
                    }
                    let photos = s
                        .keywords
                        .iter()
                        .filter(|k| below.contains(&k.id))
                        .map(|k| k.photo_count as usize)
                        .sum();
                    self.dialog = Some(Dialog::ConfirmDeleteKeyword { id, name: k.name.clone(), photos });
                }
            }
            Command::ConfirmDeleteKeyword(id) => {
                let Some(s) = &mut self.session else { return };
                match s.catalog.delete_keyword(id).and_then(|ids| s.refresh_keywords().map(|()| ids)) {
                    Ok(ids) => self.metadata_changed(&ids),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Command::ReadMetadata => {
                let Some(s) = &self.session else { return };
                let ids = s.targets(self.view);
                if !ids.is_empty() {
                    self.dialog = Some(Dialog::ConfirmReadMetadata { ids });
                }
            }
            Command::ConfirmReadMetadata(ids) => {
                if let Some(s) = &self.session {
                    s.jobs.submit_normal(Job::ReadXmp { photo_ids: ids });
                }
            }
            Command::ToggleAutoWriteXmp => {
                self.ui.auto_write_xmp ^= true;
                if self.ui.auto_write_xmp {
                    // Like LrC, turning it on writes everything that is waiting.
                    if let Some(s) = &self.session {
                        match s.catalog.dirty_photo_ids() {
                            Ok(ids) if !ids.is_empty() => {
                                let n = ids.len();
                                s.jobs.submit_normal(Job::WriteXmp { photo_ids: ids, quiet: false });
                                self.notify(format!("Writing metadata for {n} photos to XMP"));
                            }
                            Ok(_) => self.notify("Changes will be written to XMP automatically"),
                            Err(e) => self.error = Some(format!("{e:#}")),
                        }
                    }
                } else {
                    self.notify("Metadata changes stay in the catalog until saved (Ctrl+S)");
                }
            }
            Command::CreateCollection { parent, kind } => {
                let has_selection = self.session.as_ref().is_some_and(|s| !s.selected.is_empty());
                self.dialog = Some(Dialog::NewCollection {
                    parent,
                    kind,
                    name: String::new(),
                    include_selected: has_selection && kind == catalog::CollectionKind::Collection,
                    rules: catalog::SmartRules::default(),
                });
            }
            Command::ApplyCreateCollection { parent, kind, name, include_selected, rules } => {
                let view = self.view;
                let Some(s) = &mut self.session else { return };
                let result =
                    s.catalog.create_collection(parent, &name, kind, rules.as_ref()).and_then(|id| {
                        if include_selected {
                            let ids = s.targets(view);
                            s.catalog.add_to_collection(id, &ids)?;
                        }
                        if let Some(p) = parent {
                            s.collapsed_collections.remove(&p);
                        }
                        s.collections_changed()
                    });
                if let Err(e) = result {
                    self.error = Some(format!("{e:#}"));
                }
            }
            Command::RenameCollection(id) => {
                let Some(s) = &self.session else { return };
                if let Some(c) = s.collections.iter().find(|c| c.id == id) {
                    self.dialog = Some(Dialog::RenameCollection { id, name: c.name.clone() });
                }
            }
            Command::ApplyRenameCollection { id, name } => {
                let Some(s) = &mut self.session else { return };
                if let Err(e) = s.catalog.rename_collection(id, &name).and_then(|()| s.collections_changed())
                {
                    self.error = Some(format!("{e:#}"));
                }
            }
            Command::DeleteCollection(id) => {
                let Some(s) = &self.session else { return };
                if let Some(c) = s.collections.iter().find(|c| c.id == id) {
                    self.dialog = Some(Dialog::ConfirmDeleteCollection { id, name: c.name.clone() });
                }
            }
            Command::ConfirmDeleteCollection(id) => {
                let Some(s) = &mut self.session else { return };
                let showing = matches!(s.source, Source::Collection { id: shown } if shown == id
                    || s.collections.iter().any(|c| c.id == shown && c.parent_id == Some(id)));
                let result = s.catalog.delete_collection(id).and_then(|()| {
                    if showing {
                        s.source = Source::AllPhotographs;
                        s.reload_photos()?;
                    }
                    s.collections_changed()
                });
                if let Err(e) = result {
                    self.error = Some(format!("{e:#}"));
                }
            }
            Command::EditSmartCollection(id) => {
                let Some(s) = &self.session else { return };
                if let Some(c) = s.collections.iter().find(|c| c.id == id) {
                    self.dialog = Some(Dialog::EditSmartCollection {
                        id,
                        name: c.name.clone(),
                        rules: c.rules.clone().unwrap_or_default(),
                    });
                }
            }
            Command::ApplySmartCollection { id, name, rules } => {
                let Some(s) = &mut self.session else { return };
                let result = s
                    .catalog
                    .rename_collection(id, &name)
                    .and_then(|()| s.catalog.set_smart_rules(id, &rules))
                    .and_then(|()| s.collections_changed());
                if let Err(e) = result {
                    self.error = Some(format!("{e:#}"));
                }
            }
            Command::AddToCollection { id, photos } => {
                let view = self.view;
                let Some(s) = &mut self.session else { return };
                let ids = photos.unwrap_or_else(|| s.targets(view));
                let name =
                    s.collections.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_default();
                match s.catalog.add_to_collection(id, &ids).and_then(|n| s.collections_changed().map(|()| n))
                {
                    Ok(n) => {
                        self.notify(format!("Added {n} photo{} to \"{name}\"", if n == 1 { "" } else { "s" }))
                    }
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Command::OpenImport(mode) => {
                let last_source = self.ui.import.last_source.clone();
                let Some(s) = &mut self.session else {
                    self.notify("Open or create a catalog first");
                    return;
                };
                if let Some(mode) = mode {
                    self.ui.import.settings.mode = mode;
                }
                // LrC's default destination is the Pictures folder.
                if self.ui.import.settings.destination.as_os_str().is_empty()
                    && let Some(pictures) =
                        directories::UserDirs::new().and_then(|d| d.picture_dir().map(Path::to_path_buf))
                {
                    self.ui.import.settings.destination = pictures;
                }
                // The dialog starts from the last source once a background check finds
                // it still there (see `ui::import::source_panel`).
                s.import = Some(crate::import::ImportDialog::new(ctx, last_source));
            }
            Command::ImportSource(path) => {
                let recursive = self.ui.import.include_subfolders;
                let Some(s) = &mut self.session else { return };
                let Some(d) = &mut s.import else { return };
                d.generation += 1;
                d.source = Some(path.clone());
                d.candidates = None;
                d.checked.clear();
                d.error = None;
                s.jobs.submit_normal(Job::ImportScan { root: path, recursive, generation: d.generation });
            }
            Command::StartImport => {
                let prefs = self.ui.import.clone();
                let Some(s) = &mut self.session else { return };
                let Some(d) = s.import.take() else { return };
                let chosen = d.chosen(prefs.settings.skip_duplicates);
                if chosen.is_empty() {
                    return;
                }
                self.ui.import.last_source = d.source.clone();
                let n = chosen.len();
                s.jobs.drop_import_thumbs();
                s.jobs.start_import(chosen, prefs.effective());
                self.notify(format!("Importing {n} photo{}", if n == 1 { "" } else { "s" }));
            }
            Command::CloseImport => {
                if let Some(s) = &mut self.session {
                    s.import = None;
                    s.jobs.drop_import_thumbs();
                }
            }
            Command::CancelImport => {
                if let Some(s) = &self.session {
                    s.jobs.cancel_import();
                    self.notify("Stopping the import after the current photo");
                }
            }
            Command::EditIn(editor) => {
                if self.session.is_none() {
                    self.notify("Open or create a catalog first");
                    return;
                }
                // Look again each time: the editor may have been installed or updated meanwhile.
                let found = crate::editors::find(editor, &self.ui.edit_in);
                self.editors.insert(editor, found.clone());
                let found = match found {
                    Ok(f) => f,
                    Err(why) => {
                        self.notify(format!(
                            "Can't edit in {}: {why}. Set its location in External Editing settings.",
                            editor.label()
                        ));
                        self.edit_settings = true;
                        return;
                    }
                };
                let view = self.view;
                let Some(s) = &mut self.session else { return };
                let ids = s.targets(view);
                let rows: Vec<PhotoRow> =
                    ids.iter().filter_map(|id| s.catalog.photo(*id).ok().flatten()).collect();
                let originals: Vec<(i64, PathBuf)> = rows
                    .iter()
                    .filter(|r| !r.missing && r.path.is_file())
                    .map(|r| (r.id, r.path.clone()))
                    .collect();
                if originals.is_empty() {
                    self.notify(if rows.is_empty() {
                        "Select the photos to edit"
                    } else {
                        "The selected photos' files are missing"
                    });
                    return;
                }
                let n = originals.len();
                s.edits.enqueue(found.launches(&originals, &self.ui.edit_in));
                let skipped = rows.len() - n;
                let name = editor.label();
                let mut msg = format!("Opening {n} photo{} in {name}", if n == 1 { "" } else { "s" });
                if editor == crate::editors::Editor::Luminar && n > 1 {
                    msg += ", one at a time";
                }
                if editor == crate::editors::Editor::Darktable {
                    msg += "; close darktable when you're done and the edited photos are rendered";
                }
                if skipped > 0 {
                    msg += &format!(" ({skipped} missing, skipped)");
                }
                self.notify(msg + "; edits come back stacked with the originals");
            }
            Command::OpenEditSettings => {
                self.editors.clear();
                self.edit_settings = true;
            }
            Command::OpenExport => {
                let view = self.view;
                let Some(s) = &mut self.session else {
                    self.notify("Open or create a catalog first");
                    return;
                };
                let photo_ids = s.targets(view);
                if photo_ids.is_empty() {
                    self.notify("Select the photos to export");
                    return;
                }
                let sample = s.catalog.photo(photo_ids[0]).ok().flatten().map(|p| {
                    let stem = p.path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                    let ext = p.path.extension().unwrap_or_default().to_string_lossy().into_owned();
                    (stem, ext, p.capture_time.unwrap_or(0))
                });
                // LrC's default destination is the Pictures folder.
                if self.ui.export.folder.as_os_str().is_empty()
                    && let Some(pictures) =
                        directories::UserDirs::new().and_then(|d| d.picture_dir().map(Path::to_path_buf))
                {
                    self.ui.export.folder = pictures;
                }
                s.export = Some(crate::ui::export::ExportDialog::new(photo_ids, sample));
            }
            Command::ExportWithPrevious => {
                let view = self.view;
                let Some(s) = &mut self.session else { return };
                let photo_ids = s.targets(view);
                if photo_ids.is_empty() {
                    self.notify("Select the photos to export");
                    return;
                }
                if let Some(problem) = self.ui.export.problem() {
                    // Nothing usable from before: show the dialog instead.
                    self.notify(problem);
                    self.execute(ctx, Command::OpenExport);
                    return;
                }
                let n = photo_ids.len();
                s.jobs.submit_normal(Job::Export { photo_ids, prefs: Box::new(self.ui.export.clone()) });
                self.notify(format!(
                    "Exporting {n} photo{} ({})",
                    if n == 1 { "" } else { "s" },
                    self.ui.export.summary()
                ));
            }
            Command::ExportWithPreset(id) => {
                match crate::export::preset_prefs(&self.ui.export, &self.ui.export_presets, &id) {
                    Some(prefs) => {
                        self.ui.export = prefs;
                        self.execute(ctx, Command::ExportWithPrevious);
                    }
                    None => self.notify("That export preset no longer exists"),
                }
            }
            Command::StartExport => {
                let Some(s) = &mut self.session else { return };
                let Some(d) = s.export.take() else { return };
                let n = d.photo_ids.len();
                s.jobs.submit_normal(Job::Export {
                    photo_ids: d.photo_ids,
                    prefs: Box::new(self.ui.export.clone()),
                });
                self.notify(format!("Exporting {n} photo{}", if n == 1 { "" } else { "s" }));
            }
            Command::CloseExport => {
                if let Some(s) = &mut self.session {
                    s.export = None;
                }
            }
            Command::CancelExport => {
                if let Some(s) = &self.session {
                    s.jobs.cancel_export();
                    self.notify("Stopping the export after the photos being written");
                }
            }
            Command::RemovePhotos(ids) => {
                let Some(s) = &mut self.session else { return };
                let result = s.catalog.remove_photos(&ids).and_then(|previews| {
                    s.refresh_tree()?;
                    s.reload_photos()?;
                    s.collections_changed()?;
                    Ok(previews)
                });
                match result {
                    Ok(previews) => {
                        s.jobs.submit_normal(Job::PurgePreviews { photo_ids: previews });
                        s.meta_gen += 1;
                        let n = ids.len();
                        self.notify(format!(
                            "Removed {n} photo{} from the catalog (the files are still on disk)",
                            if n == 1 { "" } else { "s" }
                        ));
                    }
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Command::DeleteVirtualCopies(ids) => {
                let Some(s) = &mut self.session else { return };
                match s
                    .catalog
                    .delete_virtual_copies(&ids)
                    .and_then(|n| s.refresh_tree().and_then(|()| s.reload_photos()).map(|()| n))
                {
                    Ok(n) => {
                        s.jobs.submit_normal(Job::PurgePreviews { photo_ids: ids });
                        self.notify(format!("Deleted {n} virtual cop{}", if n == 1 { "y" } else { "ies" }));
                    }
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Command::QuickCollectionAdd(ids) => {
                let Some(s) = &mut self.session else { return };
                let result = s.catalog.add_to_quick_collection(&ids).and_then(|()| {
                    s.update_rows(&ids, |p| p.in_quick_collection = true);
                    s.collections_changed()
                });
                match result {
                    Ok(()) => self.notify("Added to Quick Collection"),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
            Command::ToggleSeparateRawJpeg => {
                let Some(s) = &mut self.session else { return };
                let separate = !s.catalog.separate_raw_jpeg().unwrap_or(false);
                match s.catalog.set_separate_raw_jpeg(separate) {
                    Ok(()) => self.notify(if separate {
                        "New JPEGs next to raw files will be imported as separate photos"
                    } else {
                        "New JPEGs next to raw files will be kept with the raw photo (RAW+JPEG)"
                    }),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
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

impl eframe::App for FotaroApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.perf.record(frame.info().cpu_usage);
        let ctx = ui.ctx().clone();
        static TEXTURE_LIMIT: std::sync::Once = std::sync::Once::new();
        TEXTURE_LIMIT.call_once(|| log::info!("GPU texture limit: {} px", ctx.input(|i| i.max_texture_side)));
        if let Some(s) = &mut self.session {
            s.thumbs.begin_frame();
            s.previews.begin_frame();
            s.full.begin_frame();
            for msg in s.process_events(&ctx) {
                self.toast = Some(Toast { text: msg, at: Instant::now() });
            }
            if !s.edits.is_idle() {
                // Editor launches waiting their turn, and results to look for.
                for msg in s.edits.tick() {
                    self.toast = Some(Toast { text: msg, at: Instant::now() });
                }
                if let Some(watches) = s.edits.due_check() {
                    s.jobs.submit_normal(Job::CheckEdits { watches });
                }
                ctx.request_repaint_after(Duration::from_secs(1));
            }
            // Apply Library Filter changes.
            let wanted = self.ui.filter.criterion();
            if s.filter != wanted {
                s.filter = wanted;
                if let Err(e) = s.reload_photos() {
                    self.error = Some(format!("{e:#}"));
                }
                s.scroll_to_active = true;
            }
        }
        if self.module == Module::Develop {
            self.develop_frame(&ctx);
        }
        if self.ui.check_updates {
            self.updater.check_daily(&ctx);
        }
        if let Some(text) = self.updater.poll(self.ui.skipped_update.as_deref()) {
            self.toast = Some(Toast { text, at: Instant::now() });
        }

        for sc in crate::ui::shortcuts::take_actions(&ctx, self.module) {
            self.commands.push(Command::Action(sc.action));
        }

        let draw_started = Instant::now();
        crate::ui::layout::draw(self, ui);
        if self.view != View::Compare {
            self.member_compare = None;
        }
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
    ConfirmRemoveFolder {
        id: i64,
        name: String,
        photos: i64,
    },
    CreateFolder {
        parent: i64,
        name: String,
    },
    RenameFolder {
        id: i64,
        name: String,
    },
    CreateKeyword {
        parent: Option<i64>,
        name: String,
    },
    RenameKeyword {
        id: i64,
        name: String,
    },
    ConfirmDeleteKeyword {
        id: i64,
        name: String,
        photos: usize,
    },
    ConfirmReadMetadata {
        ids: Vec<i64>,
    },
    ConfirmDeleteVirtualCopies {
        ids: Vec<i64>,
    },
    ConfirmRemoveRejected {
        ids: Vec<i64>,
        source: String,
    },
    NewCollection {
        parent: Option<i64>,
        kind: catalog::CollectionKind,
        name: String,
        include_selected: bool,
        rules: catalog::SmartRules,
    },
    RenameCollection {
        id: i64,
        name: String,
    },
    ConfirmDeleteCollection {
        id: i64,
        name: String,
    },
    EditSmartCollection {
        id: i64,
        name: String,
        rules: catalog::SmartRules,
    },
}

pub fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

impl FotaroApp {
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
