//! Background work: folder sync, preview building and thumbnail loading on worker
//! threads, so the UI never blocks.
//!
//! Three priority lanes:
//! - **Visible**: images the user is looking at right now. LIFO (the most recently
//!   requested, i.e. currently on screen, goes first) and capped, so fast scrolling
//!   through thousands of photos never builds a backlog.
//! - **Normal**: user-initiated operations such as Synchronize Folder.
//! - **Background**: building previews for everything after an import (FIFO).

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use anyhow::{Context, Result};
use catalog::{Catalog, FileKind, PhotoMetadata, PhotoRow, SyncReport};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use imaging::{PreviewCache, PreviewSize, SourceKind};

/// Everything a worker needs to know about a photo.
#[derive(Debug, Clone)]
pub struct PhotoRef {
    pub id: i64,
    /// Whose previews these are on disk: the master's for a virtual copy (same file).
    pub cache_id: i64,
    pub path: PathBuf,
    pub kind: FileKind,
    pub fingerprint: u64,
}

impl From<&PhotoRow> for PhotoRef {
    fn from(p: &PhotoRow) -> Self {
        PhotoRef {
            id: p.id,
            cache_id: p.master_id,
            path: p.path.clone(),
            kind: p.kind,
            fingerprint: imaging::fingerprint(p.size_bytes, p.mtime_ms),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Job {
    /// Add or re-synchronize a folder tree, then queue previews for it.
    Sync {
        root: PathBuf,
    },
    /// Load (building first if needed) a preview and send it to the UI.
    Load {
        photo: PhotoRef,
        size: PreviewSize,
    },
    /// Make sure both previews exist on disk (and metadata is read).
    BuildPreviews {
        photo: PhotoRef,
    },
    /// Check which catalog folders still exist on disk (can be slow for network drives).
    CheckFolders {
        folders: Vec<(i64, PathBuf)>,
    },
    /// Delete cached previews of photos removed from the catalog.
    PurgePreviews {
        photo_ids: Vec<i64>,
    },
    /// Delete previews that no catalog photo will ask for again (crash leftovers, old
    /// versions, photos removed by older app versions). Run once per catalog open.
    PurgeOrphanPreviews,
    /// Read GPS positions that older versions of the app never read, a batch at a time
    /// (each batch queues the next on the background lane).
    ReadGps,
    /// LrC "Save Metadata to File(s)": write XMP sidecars.
    WriteXmp {
        photo_ids: Vec<i64>,
        quiet: bool,
    },
    /// LrC "Read Metadata from File(s)": replace catalog metadata with the sidecars'.
    ReadXmp {
        photo_ids: Vec<i64>,
    },
    /// Import dialog: list a source's photos (`generation` tags the answer).
    ImportScan {
        root: PathBuf,
        recursive: bool,
        generation: u64,
    },
    /// Import dialog: a thumbnail for one candidate.
    ImportThumb {
        key: i64,
        path: PathBuf,
        kind: FileKind,
    },
    Import {
        chosen: Vec<catalog::Candidate>,
        settings: catalog::ImportSettings,
    },
    /// File ▸ Export: render and write these photos.
    Export {
        photo_ids: Vec<i64>,
        prefs: Box<crate::export::ExportPrefs>,
    },
    /// Edit In: look for NegPy results next to the originals sent to it.
    CheckEdits {
        watches: Vec<crate::editors::EditWatch>,
    },
    /// Edit In: bring a result into the catalog, stacked below its original.
    AddEdited {
        original: i64,
        path: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum JobKey {
    Load(i64, PreviewSize),
    Build(i64),
    ImportThumb(i64),
}

impl Job {
    fn key(&self) -> Option<JobKey> {
        match self {
            Job::Sync { .. }
            | Job::CheckFolders { .. }
            | Job::PurgePreviews { .. }
            | Job::PurgeOrphanPreviews
            | Job::ReadGps
            | Job::WriteXmp { .. }
            | Job::ReadXmp { .. }
            | Job::ImportScan { .. }
            | Job::Import { .. }
            | Job::Export { .. }
            | Job::CheckEdits { .. }
            | Job::AddEdited { .. } => None,
            Job::ImportThumb { key, .. } => Some(JobKey::ImportThumb(*key)),
            Job::Load { photo, size } => Some(JobKey::Load(photo.id, *size)),
            Job::BuildPreviews { photo } => Some(JobKey::Build(photo.id)),
        }
    }
}

/// Per-channel (R, G, B) counts of 8-bit values.
pub type Histogram = [[u32; 256]; 3];

/// Load (building if needed) the full-resolution preview as a texture-ready image,
/// scaled down only if it exceeds the GPU's maximum texture size.
fn load_full(shared: &Shared, photo: &PhotoRef) -> Result<egui::ColorImage> {
    let started = std::time::Instant::now();
    let path = shared.cache.full(photo.cache_id, photo.fingerprint, &photo.path, source_kind(photo.kind))?;
    let mut img = image::open(&path)?;
    // Read now, not at startup: before the first frame egui reports a 2048 px default.
    let max_side = shared.ctx.input(|i| i.max_texture_side) as u32;
    if img.width().max(img.height()) > max_side {
        img = imaging::fit_image(img, max_side);
    }
    let rgb = img.into_rgb8();
    let image = egui::ColorImage::from_rgb([rgb.width() as usize, rgb.height() as usize], rgb.as_raw());
    log::debug!("1:1 preview of {} ready in {:.0?}", photo.path.display(), started.elapsed());
    Ok(image)
}

fn histogram(rgba: &[u8]) -> Box<Histogram> {
    let mut h = Box::new([[0u32; 256]; 3]);
    for px in rgba.as_chunks::<4>().0 {
        h[0][px[0] as usize] += 1;
        h[1][px[1] as usize] += 1;
        h[2][px[2] as usize] += 1;
    }
    h
}

pub enum Event {
    SyncDone {
        root: PathBuf,
        result: Result<SyncReport, String>,
    },
    /// `histogram` is computed for thumbnails (cheap, and plenty for display).
    Image {
        photo_id: i64,
        size: PreviewSize,
        image: egui::ColorImage,
        histogram: Option<Box<Histogram>>,
    },
    ImageFailed {
        photo_id: i64,
        size: PreviewSize,
    },
    /// Folders (by id) that no longer exist on disk.
    FoldersChecked {
        missing: HashSet<i64>,
    },
    /// Metadata (capture time, camera...) was read and stored for a photo.
    MetadataUpdated {
        photo_id: i64,
    },
    ImportScanned {
        generation: u64,
        result: Result<Vec<catalog::Candidate>, String>,
    },
    ImportThumb {
        key: i64,
        image: egui::ColorImage,
    },
    ImportDone {
        result: Result<catalog::ImportReport, String>,
    },
    ExportDone {
        result: Result<crate::export::ExportReport, String>,
        /// Post-processing: the folder to show in Explorer / Finder.
        reveal: Option<PathBuf>,
    },
    EditsChecked {
        found: Vec<(i64, PathBuf, crate::editors::FileVersion)>,
    },
    /// A NegPy result is in the catalog: `Ok((photo id, new))`, `new` false when it was
    /// already there and was exported again.
    EditAdded {
        path: PathBuf,
        result: Result<(i64, bool), String>,
    },
    /// Sidecars were written (`read: false`) or read for these photos.
    XmpDone {
        read: bool,
        /// Automatic write: only report failures.
        quiet: bool,
        done: Vec<i64>,
        /// Photos without a sidecar (reading only).
        skipped: usize,
        failed: Vec<(i64, String)>,
    },
}

const VISIBLE_CAP: usize = 300;

#[derive(Default)]
struct Lanes {
    visible: VecDeque<Job>,
    normal: VecDeque<Job>,
    background: VecDeque<Job>,
    keys: HashSet<JobKey>,
}

impl Lanes {
    /// Next job for a worker; `background` says whether this worker may take bulk work.
    fn pop(&mut self, background: bool) -> Option<Job> {
        let job = self
            .visible
            .pop_back()
            .or_else(|| self.normal.pop_front())
            .or_else(|| if background { self.background.pop_front() } else { None })?;
        if let Some(k) = job.key() {
            self.keys.remove(&k);
        }
        Some(job)
    }
}

/// Counters for the activity display ("Building Previews 120 / 500").
#[derive(Default)]
pub struct Progress {
    previews_total: AtomicUsize,
    previews_done: AtomicUsize,
    previews_started: Mutex<Option<std::time::Instant>>,
    syncing: Mutex<Option<String>>,
    /// (done, total) while an import runs.
    import: Mutex<Option<(usize, usize)>>,
    import_cancel: AtomicBool,
    /// (done, total) while an export runs.
    export: Mutex<Option<(usize, usize)>>,
    export_cancel: AtomicBool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub syncing: Option<String>,
    pub previews: Option<(usize, usize)>,
    pub import: Option<(usize, usize)>,
    pub export: Option<(usize, usize)>,
}

struct Shared {
    lanes: Mutex<Lanes>,
    wake: Condvar,
    shutdown: AtomicBool,
    progress: Progress,
    events: Sender<Event>,
    ctx: egui::Context,
    catalog_dir: PathBuf,
    cache: PreviewCache,
    /// Photos whose previews are being built right now (one builder per photo).
    building: Mutex<HashSet<i64>>,
    building_done: Condvar,
}

pub struct JobSystem {
    shared: Arc<Shared>,
}

impl JobSystem {
    pub fn start(ctx: egui::Context, catalog_dir: PathBuf, cache: PreviewCache) -> (Self, Receiver<Event>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let shared = Arc::new(Shared {
            lanes: Mutex::new(Lanes::default()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
            progress: Progress::default(),
            events: tx,
            ctx,
            catalog_dir,
            cache,
            building: Mutex::new(HashSet::new()),
            building_done: Condvar::new(),
        });
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        let workers = cores.saturating_sub(1).max(1);
        // Workers run at the lowest OS priority, so the UI thread always wins the CPU.
        // Bulk preview building leaves one worker free for on-screen requests.
        let background_workers = workers.saturating_sub(1).max(1);
        for i in 0..workers {
            let may_background = i < background_workers;
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name(format!("worker-{i}"))
                .spawn(move || worker_loop(&shared, may_background))
                .expect("spawn worker thread");
        }
        log::info!("started {workers} worker threads ({background_workers} for background work)");
        (JobSystem { shared }, rx)
    }

    pub fn submit_visible(&self, job: Job) {
        self.submit(job, Lane::Visible);
    }

    pub fn submit_normal(&self, job: Job) {
        self.submit(job, Lane::Normal);
    }

    pub fn submit_background(&self, job: Job) {
        self.submit(job, Lane::Background);
    }

    /// Build previews in the background (Library ▸ Previews ▸ Build Standard-Sized Previews).
    pub fn build_previews(&self, photos: impl IntoIterator<Item = PhotoRef>) {
        for photo in photos {
            self.submit(Job::BuildPreviews { photo }, Lane::Background);
        }
    }

    fn submit(&self, job: Job, lane: Lane) {
        submit(&self.shared, job, lane);
    }

    /// Stop the running import after the file it is on.
    pub fn cancel_import(&self) {
        self.shared.progress.import_cancel.store(true, Ordering::Relaxed);
    }

    /// Stop the running export after the photos being written.
    pub fn cancel_export(&self) {
        self.shared.progress.export_cancel.store(true, Ordering::Relaxed);
    }

    pub fn submit_import_thumb(&self, job: Job) {
        self.submit(job, Lane::Visible);
    }

    pub fn progress(&self) -> ProgressSnapshot {
        let p = &self.shared.progress;
        let total = p.previews_total.load(Ordering::Relaxed);
        let done = p.previews_done.load(Ordering::Relaxed);
        ProgressSnapshot {
            syncing: p.syncing.lock().unwrap().clone(),
            previews: (total > 0 && done < total).then_some((done, total)),
            import: *p.import.lock().unwrap(),
            export: *p.export.lock().unwrap(),
        }
    }
}

impl Drop for JobSystem {
    fn drop(&mut self) {
        // Workers finish their current job and exit; we don't block the UI joining them.
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.shared.wake.notify_all();
    }
}

#[derive(Clone, Copy)]
enum Lane {
    Visible,
    Normal,
    Background,
}

fn submit(shared: &Shared, job: Job, lane: Lane) {
    let mut lanes = shared.lanes.lock().unwrap();
    if let Some(k) = job.key()
        && !lanes.keys.insert(k)
    {
        return; // already queued
    }
    match lane {
        Lane::Visible => {
            lanes.visible.push_back(job);
            while lanes.visible.len() > VISIBLE_CAP {
                if let Some(k) = lanes.visible.pop_front().and_then(|j| j.key()) {
                    lanes.keys.remove(&k);
                }
            }
        }
        Lane::Normal => lanes.normal.push_back(job),
        Lane::Background => {
            // Progress accounting happens under the queue lock, so a worker can never
            // count a job as done before it was counted as queued.
            let p = &shared.progress;
            if p.previews_done.load(Ordering::Relaxed) >= p.previews_total.load(Ordering::Relaxed) {
                p.previews_total.store(0, Ordering::Relaxed);
                p.previews_done.store(0, Ordering::Relaxed);
                *p.previews_started.lock().unwrap() = Some(std::time::Instant::now());
            }
            p.previews_total.fetch_add(1, Ordering::Relaxed);
            lanes.background.push_back(job);
        }
    }
    drop(lanes);
    // Only some workers take background jobs, so wake everyone for those.
    if matches!(lane, Lane::Background) { shared.wake.notify_all() } else { shared.wake.notify_one() }
}

fn worker_loop(shared: &Shared, may_background: bool) {
    if let Err(e) = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min) {
        log::debug!("could not lower worker thread priority: {e:?}");
    }
    // Each worker has its own SQLite connection (WAL mode allows this safely).
    let mut catalog: Option<Catalog> = None;
    loop {
        let job = {
            let mut lanes = shared.lanes.lock().unwrap();
            loop {
                if shared.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(job) = lanes.pop(may_background) {
                    break job;
                }
                lanes = shared.wake.wait(lanes).unwrap();
            }
        };
        if catalog.is_none() {
            match Catalog::open(&shared.catalog_dir) {
                Ok(c) => catalog = Some(c),
                Err(e) => log::error!("worker could not open catalog: {e:#}"),
            }
        }
        let Some(cat) = catalog.as_mut() else { continue };
        run(shared, cat, job);
        shared.ctx.request_repaint();
    }
}

fn source_kind(kind: FileKind) -> SourceKind {
    match kind {
        FileKind::Raw => SourceKind::Raw,
        FileKind::Image => SourceKind::Image,
    }
}

fn to_catalog_metadata(m: imaging::Metadata) -> PhotoMetadata {
    PhotoMetadata {
        make: m.make,
        model: m.model,
        lens: m.lens,
        iso: m.iso,
        shutter: m.shutter,
        aperture: m.aperture,
        focal_len: m.focal_len,
        capture_time: m.capture_time,
        width: m.width,
        height: m.height,
        orientation: m.orientation,
        gps_lat: m.gps.map(|g| g.latitude),
        gps_lon: m.gps.map(|g| g.longitude),
        gps_alt: m.gps.and_then(|g| g.altitude),
    }
}

/// Make sure both previews for `photo` exist (building them and storing metadata if
/// not). If another worker is already building this photo, wait for it instead of
/// racing it on the same cache files.
fn ensure_previews(shared: &Shared, cat: &Catalog, photo: &PhotoRef) -> Result<()> {
    {
        let mut building = shared.building.lock().unwrap();
        while building.contains(&photo.cache_id) {
            building = shared.building_done.wait(building).unwrap();
        }
        if shared.cache.has_all(photo.cache_id, photo.fingerprint) {
            return Ok(());
        }
        building.insert(photo.cache_id);
    }
    let result = (|| {
        anyhow::ensure!(photo.path.exists(), "file is missing");
        let built =
            shared.cache.build(photo.cache_id, photo.fingerprint, &photo.path, source_kind(photo.kind))?;
        // Sync normally stored metadata already; only fill it in if that failed, so
        // preview workers (many threads) rarely contend for the SQLite write lock.
        if cat.metadata(photo.id)?.is_none() {
            cat.set_metadata(photo.id, &to_catalog_metadata(built.metadata))?;
            let _ = shared.events.send(Event::MetadataUpdated { photo_id: photo.id });
        }
        Ok(())
    })();
    shared.building.lock().unwrap().remove(&photo.cache_id);
    shared.building_done.notify_all();
    result
}

fn run(shared: &Shared, cat: &mut Catalog, job: Job) {
    match job {
        Job::Sync { root } => {
            *shared.progress.syncing.lock().unwrap() =
                Some(format!("Synchronizing {}", root.file_name().unwrap_or_default().to_string_lossy()));
            let started = std::time::Instant::now();
            let result = sync_and_queue(shared, cat, &root).map_err(|e| format!("{e:#}"));
            log::info!("sync of {} took {:.2?}", root.display(), started.elapsed());
            *shared.progress.syncing.lock().unwrap() = None;
            let _ = shared.events.send(Event::SyncDone { root, result });
        }
        Job::Load { photo, size } => {
            let result = (|| -> Result<(egui::ColorImage, Option<Box<Histogram>>)> {
                if size == PreviewSize::Full {
                    return load_full(shared, &photo).map(|image| (image, None));
                }
                let path = match shared.cache.get(photo.cache_id, size, photo.fingerprint) {
                    Some(p) => p,
                    None => {
                        ensure_previews(shared, cat, &photo)?;
                        shared.cache.path(photo.cache_id, size, photo.fingerprint)
                    }
                };
                let rgba = image::open(&path)?.into_rgba8();
                let (w, h) = (rgba.width() as usize, rgba.height() as usize);
                let hist = (size == PreviewSize::Thumb).then(|| histogram(rgba.as_raw()));
                Ok((egui::ColorImage::from_rgba_unmultiplied([w, h], rgba.as_raw()), hist))
            })();
            let event = match result {
                Ok((image, histogram)) => Event::Image { photo_id: photo.id, size, image, histogram },
                Err(e) => {
                    log::warn!("preview for {}: {e:#}", photo.path.display());
                    Event::ImageFailed { photo_id: photo.id, size }
                }
            };
            let _ = shared.events.send(event);
        }
        Job::CheckFolders { folders } => {
            let missing = folders.into_iter().filter(|(_, p)| !p.is_dir()).map(|(id, _)| id).collect();
            let _ = shared.events.send(Event::FoldersChecked { missing });
        }
        Job::PurgePreviews { photo_ids } => {
            for id in photo_ids {
                shared.cache.remove(id);
            }
        }
        Job::WriteXmp { photo_ids, quiet } => run_xmp(shared, cat, photo_ids, false, quiet),
        Job::ReadXmp { photo_ids } => run_xmp(shared, cat, photo_ids, true, false),
        Job::ImportScan { root, recursive, generation } => {
            let started = std::time::Instant::now();
            let reader = |path: &std::path::Path, kind: FileKind| {
                imaging::read_metadata(path, source_kind(kind)).ok().map(to_catalog_metadata)
            };
            let result = cat
                .duplicate_keys()
                .and_then(|known| catalog::import::scan_source(&root, recursive, Some(&reader), &known))
                .map_err(|e| format!("{e:#}"));
            if let Ok(found) = &result {
                log::info!(
                    "import source {}: {} photos in {:.0?}",
                    root.display(),
                    found.len(),
                    started.elapsed()
                );
            }
            let _ = shared.events.send(Event::ImportScanned { generation, result });
        }
        Job::ImportThumb { key, path, kind } => {
            match imaging::decode(&path, source_kind(kind), PreviewSize::Thumb.max_px()) {
                Ok((rgb, _)) => {
                    let size = [rgb.width() as usize, rgb.height() as usize];
                    let image = egui::ColorImage::from_rgb(size, rgb.as_raw());
                    let _ = shared.events.send(Event::ImportThumb { key, image });
                }
                Err(e) => log::debug!("import thumbnail of {}: {e:#}", path.display()),
            }
        }
        Job::Import { chosen, settings } => {
            let p = &shared.progress;
            p.import_cancel.store(false, Ordering::Relaxed);
            *p.import.lock().unwrap() = Some((0, chosen.len()));
            let started = std::time::Instant::now();
            let reader = |path: &std::path::Path, kind: FileKind| {
                imaging::read_metadata(path, source_kind(kind)).ok().map(to_catalog_metadata)
            };
            let progress = |done: usize, total: usize| {
                *p.import.lock().unwrap() = Some((done, total));
                shared.ctx.request_repaint();
            };
            let result = cat.import(&chosen, &settings, Some(&reader), &progress, &p.import_cancel);
            *p.import.lock().unwrap() = None;
            if let Ok(report) = &result {
                log::info!(
                    "imported {} photos ({} failed) in {:.1?}",
                    report.imported,
                    report.failed.len(),
                    started.elapsed()
                );
                // Previews for the new photos, in the background.
                for id in &report.photo_ids {
                    if let Ok(Some(row)) = cat.photo(*id) {
                        submit(shared, Job::BuildPreviews { photo: PhotoRef::from(&row) }, Lane::Background);
                    }
                }
            }
            let _ = shared.events.send(Event::ImportDone { result: result.map_err(|e| format!("{e:#}")) });
        }
        Job::Export { photo_ids, prefs } => {
            let p = &shared.progress;
            p.export_cancel.store(false, Ordering::Relaxed);
            *p.export.lock().unwrap() = Some((0, photo_ids.len()));
            let started = std::time::Instant::now();
            let reader = |path: &std::path::Path, kind: FileKind| {
                imaging::read_metadata(path, source_kind(kind)).ok().map(to_catalog_metadata)
            };
            let progress = |done: usize, total: usize| {
                *p.export.lock().unwrap() = Some((done, total));
                shared.ctx.request_repaint();
            };
            let result =
                crate::export::run(cat, &photo_ids, &prefs, Some(&reader), &progress, &p.export_cancel);
            *p.export.lock().unwrap() = None;
            let mut reveal = None;
            if let Ok(report) = &result {
                log::info!(
                    "exported {} photos ({} skipped, {} failed) in {:.1?}",
                    report.exported.len(),
                    report.skipped,
                    report.failed.len(),
                    started.elapsed()
                );
                for id in &report.added {
                    if let Ok(Some(row)) = cat.photo(*id) {
                        submit(shared, Job::BuildPreviews { photo: PhotoRef::from(&row) }, Lane::Background);
                    }
                }
                if prefs.reveal {
                    reveal = report.exported.first().cloned();
                }
            }
            let _ = shared
                .events
                .send(Event::ExportDone { result: result.map_err(|e| format!("{e:#}")), reveal });
        }
        Job::CheckEdits { watches } => {
            let found = crate::editors::scan(&watches);
            let _ = shared.events.send(Event::EditsChecked { found });
            shared.ctx.request_repaint();
        }
        Job::AddEdited { original, path } => {
            let result = add_edited(shared, cat, original, &path).map_err(|e| format!("{e:#}"));
            let _ = shared.events.send(Event::EditAdded { path, result });
            shared.ctx.request_repaint();
        }
        Job::ReadGps => read_gps_batch(shared, cat),
        Job::PurgeOrphanPreviews => {
            let started = std::time::Instant::now();
            // Previews written after this moment are kept whatever the snapshot says.
            let snapshot = std::time::SystemTime::now();
            let current: std::collections::HashMap<i64, u64> = match cat.photo_versions() {
                Ok(v) => {
                    v.into_iter().map(|(id, size, mtime)| (id, imaging::fingerprint(size, mtime))).collect()
                }
                Err(e) => {
                    log::warn!("skipping preview cleanup: {e:#}");
                    return;
                }
            };
            // An empty catalog next to a full cache smells like a damaged or wrong catalog
            // database, not like intent (removing folders already purges their previews).
            if current.is_empty() {
                log::info!("preview cleanup skipped: the catalog has no photos");
                return;
            }
            let r = shared.cache.purge_orphans(snapshot, |id, fp| current.get(&id) == Some(&fp));
            log::info!(
                "preview cleanup: removed {} of {} files ({}) in {:.0?}",
                r.removed,
                r.scanned,
                crate::platform::human_bytes(r.bytes),
                started.elapsed()
            );
        }
        Job::BuildPreviews { photo } => {
            if photo.path.exists()
                && let Err(e) = ensure_previews(shared, cat, &photo)
            {
                log::warn!("building previews for {}: {e:#}", photo.path.display());
            }
            let p = &shared.progress;
            let done = p.previews_done.fetch_add(1, Ordering::Relaxed) + 1;
            if done == p.previews_total.load(Ordering::Relaxed)
                && let Some(t) = p.previews_started.lock().unwrap().take()
            {
                log::info!("built previews for {done} photos in {:.1?}", t.elapsed());
            }
        }
    }
}

fn sync_and_queue(shared: &Shared, cat: &mut Catalog, root: &std::path::Path) -> Result<SyncReport> {
    let reader = |path: &std::path::Path, kind: FileKind| {
        imaging::read_metadata(path, source_kind(kind)).ok().map(to_catalog_metadata)
    };
    let report = cat.sync_folder_with(root, Some(&reader))?;
    let canonical = dunce::canonicalize(root)?;
    if let Some(folder) = cat.folder_by_path(&canonical)? {
        let todo: Vec<PhotoRef> = cat
            .photos_in_folder(folder, true)?
            .iter()
            .filter(|p| !p.missing)
            .map(PhotoRef::from)
            .filter(|p| !shared.cache.has_all(p.cache_id, p.fingerprint))
            .collect();
        for photo in todo {
            submit(shared, Job::BuildPreviews { photo }, Lane::Background);
        }
    }
    Ok(report)
}

/// Write (or read) XMP sidecars for `photo_ids` and report back to the UI.
/// [`Job::AddEdited`]: add (or update) a NegPy result. A new one gets its original's
/// catalog metadata and goes right below it in a stack, as LrC's Edit In does.
fn add_edited(
    shared: &Shared,
    cat: &mut Catalog,
    original: i64,
    path: &std::path::Path,
) -> Result<(i64, bool)> {
    let path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let known = cat.photo_ids_for_paths(std::slice::from_ref(&path))?.first().copied();
    let reader = |p: &std::path::Path, kind: FileKind| {
        imaging::read_metadata(p, source_kind(kind)).ok().map(to_catalog_metadata)
    };
    let id = cat.add_files(std::slice::from_ref(&path), Some(&reader))?.first().copied().flatten();
    let id = id.with_context(|| format!("{} could not be added", path.display()))?;
    let new = known.is_none();
    if !new && let Some(row) = cat.photo(id)? {
        // Sync goes by size and modified time, which a new version may share with the
        // old one (see below); its dimensions may still have changed.
        if let Some(m) = reader(&row.path, row.kind) {
            cat.set_metadata(id, &m)?;
        }
    }
    if new && cat.photo(original)?.is_some() {
        cat.copy_metadata(original, id)?;
        cat.add_to_picture(original, id, catalog::Role::Edit)?;
    }
    if let Some(row) = cat.photo(id)? {
        let photo = PhotoRef::from(&row);
        if !new {
            // NegPy gives every export of an original the same modified time, so a new
            // version can have the same size and date as the old: drop the old previews.
            shared.cache.remove(photo.cache_id);
        }
        submit(shared, Job::BuildPreviews { photo }, Lane::Visible);
    }
    log::info!("Edit In: {} {}", if new { "added" } else { "updated" }, path.display());
    Ok((id, new))
}

/// One batch of [`Job::ReadGps`]: read the files' GPS, store it, queue the next batch.
fn read_gps_batch(shared: &Shared, cat: &mut Catalog) {
    const BATCH: usize = 200;
    let todo = match cat.gps_unread(BATCH) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("reading GPS: {e:#}");
            return;
        }
    };
    if todo.is_empty() {
        return;
    }
    let started = std::time::Instant::now();
    let mut positions = Vec::with_capacity(todo.len());
    for (id, path, kind) in &todo {
        if shared.shutdown.load(Ordering::SeqCst) {
            return;
        }
        // Unreadable files count as "no GPS", so they aren't tried again and again.
        let gps = imaging::read_metadata(path, source_kind(*kind)).ok().and_then(|m| m.gps);
        positions.push((*id, gps.map(|g| (g.latitude, g.longitude, g.altitude))));
    }
    if let Err(e) = cat.set_gps(&positions) {
        log::warn!("storing GPS: {e:#}");
        return;
    }
    let found: Vec<i64> = positions.iter().filter(|(_, p)| p.is_some()).map(|(id, _)| *id).collect();
    log::info!(
        "read GPS of {} photos ({} with a position) in {:.0?}",
        todo.len(),
        found.len(),
        started.elapsed()
    );
    for photo_id in found {
        let _ = shared.events.send(Event::MetadataUpdated { photo_id });
    }
    if todo.len() == BATCH {
        submit(shared, Job::ReadGps, Lane::Background);
    }
}

fn run_xmp(shared: &Shared, cat: &mut Catalog, photo_ids: Vec<i64>, read: bool, quiet: bool) {
    let started = std::time::Instant::now();
    // Older catalogs don't know files' own orientation; read it on demand.
    let orientation_of: catalog::OrientationReader =
        &|path, kind| imaging::read_metadata(path, source_kind(kind)).ok().and_then(|m| m.orientation);
    let (mut done, mut skipped, mut failed) = (Vec::new(), 0, Vec::new());
    for id in photo_ids {
        let result = if read {
            cat.read_xmp(id, orientation_of).map(|found| found.then_some(()))
        } else {
            cat.write_xmp(id, orientation_of).map(|p| p.map(|_| ()))
        };
        match result {
            Ok(Some(())) => done.push(id),
            Ok(None) => skipped += 1,
            Err(e) => {
                log::warn!("{} metadata of photo {id}: {e:#}", if read { "reading" } else { "saving" });
                failed.push((id, format!("{e:#}")));
            }
        }
    }
    log::info!(
        "{} {} sidecars in {:.0?} ({skipped} without sidecar, {} failed)",
        if read { "read" } else { "wrote" },
        done.len(),
        started.elapsed(),
        failed.len()
    );
    let _ = shared.events.send(Event::XmpDone { read, quiet, done, skipped, failed });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: i64) -> PhotoRef {
        PhotoRef { id, cache_id: id, path: PathBuf::new(), kind: FileKind::Image, fingerprint: 0 }
    }

    /// The Import button's path end to end: scan a "card", import through the job
    /// system, get the done event, and find the photo in the catalog.
    #[test]
    fn import_runs_through_the_job_system() {
        let tmp = tempfile::tempdir().unwrap();
        let card = tmp.path().join("DCIM");
        std::fs::create_dir(&card).unwrap();
        image::RgbImage::from_pixel(64, 48, image::Rgb([200, 100, 50])).save(card.join("IMG_1.png")).unwrap();
        let dest = tmp.path().join("Photos");
        std::fs::create_dir(&dest).unwrap();
        let dir = tmp.path().join("c.plcat");
        drop(Catalog::create(&dir).unwrap());

        let (jobs, events) =
            JobSystem::start(egui::Context::default(), dir.clone(), PreviewCache::new(dir.join("previews")));
        let wait = |pred: &dyn Fn(&Event) -> bool| loop {
            let e = events.recv_timeout(std::time::Duration::from_secs(30)).expect("job event");
            if pred(&e) {
                return e;
            }
        };
        jobs.submit_normal(Job::ImportScan { root: card.clone(), recursive: true, generation: 7 });
        let Event::ImportScanned { generation: 7, result: Ok(found) } =
            wait(&|e| matches!(e, Event::ImportScanned { .. }))
        else {
            panic!("scan failed")
        };
        assert_eq!(found.len(), 1);
        let settings = catalog::ImportSettings { destination: dest.clone(), ..Default::default() };
        jobs.submit_normal(Job::Import { chosen: found, settings });
        let Event::ImportDone { result: Ok(report) } = wait(&|e| matches!(e, Event::ImportDone { .. }))
        else {
            panic!("import failed")
        };
        assert_eq!((report.imported, report.failed.len()), (1, 0));
        assert!(jobs.progress().import.is_none(), "progress is cleared when done");
        let cat = Catalog::open(&dir).unwrap();
        let photos = cat.photos(catalog::Source::PreviousImport).unwrap();
        assert_eq!(photos.len(), 1);
        assert!(photos[0].path.starts_with(dunce::canonicalize(&dest).unwrap()), "{:?}", photos[0].path);
        assert!(card.join("IMG_1.png").exists(), "Copy keeps the original");
    }

    /// File ▸ Export end to end: export into the original's folder, add to the catalog
    /// and stack, through the job system.
    #[test]
    fn export_runs_through_the_job_system() {
        let tmp = tempfile::tempdir().unwrap();
        let photos = tmp.path().join("p");
        std::fs::create_dir(&photos).unwrap();
        image::RgbImage::from_pixel(64, 48, image::Rgb([20, 100, 50])).save(photos.join("a.png")).unwrap();
        let dir = tmp.path().join("c.plcat");
        let id = {
            let mut cat = Catalog::create(&dir).unwrap();
            cat.sync_folder(&photos).unwrap();
            cat.photos(catalog::Source::AllPhotographs).unwrap()[0].id
        };
        let (jobs, events) =
            JobSystem::start(egui::Context::default(), dir.clone(), PreviewCache::new(dir.join("previews")));
        let prefs = crate::export::ExportPrefs {
            export_to: crate::export::ExportTo::SameFolder,
            add_to_catalog: true,
            add_to_stack: true,
            reveal: false,
            ..Default::default()
        };
        jobs.submit_normal(Job::Export { photo_ids: vec![id], prefs: Box::new(prefs) });
        let done = loop {
            match events.recv_timeout(std::time::Duration::from_secs(30)).expect("job event") {
                Event::ExportDone { result, reveal } => break (result, reveal),
                _ => continue,
            }
        };
        let (Ok(report), None) = done else { panic!("export failed: {done:?}") };
        assert_eq!((report.exported.len(), report.added.len()), (1, 1));
        assert!(jobs.progress().export.is_none(), "progress is cleared when done");
        let cat = Catalog::open(&dir).unwrap();
        let all = cat.photos(catalog::Source::AllPhotographs).unwrap();
        assert_eq!(
            all.iter().map(|p| (p.file_name.as_str(), p.unit.members)).collect::<Vec<_>>(),
            [("a.png", 2)]
        );
    }

    #[test]
    fn visible_lane_is_lifo_and_takes_priority() {
        let mut lanes = Lanes::default();
        lanes.background.push_back(Job::BuildPreviews { photo: photo(1) });
        lanes.normal.push_back(Job::Sync { root: PathBuf::from("x") });
        lanes.visible.push_back(Job::Load { photo: photo(2), size: PreviewSize::Thumb });
        lanes.visible.push_back(Job::Load { photo: photo(3), size: PreviewSize::Thumb });

        let order: Vec<String> = std::iter::from_fn(|| lanes.pop(true))
            .map(|j| match j {
                Job::Load { photo, .. } => format!("load{}", photo.id),
                Job::Sync { .. } => "sync".into(),
                Job::BuildPreviews { photo } => format!("build{}", photo.id),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(order, ["load3", "load2", "sync", "build1"]);
    }

    #[test]
    fn foreground_only_workers_never_take_background_jobs() {
        let mut lanes = Lanes::default();
        lanes.background.push_back(Job::BuildPreviews { photo: photo(1) });
        assert!(lanes.pop(false).is_none());
        assert!(lanes.pop(true).is_some());
    }
}
