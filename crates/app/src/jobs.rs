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

use anyhow::Result;
use catalog::{Catalog, FileKind, PhotoMetadata, PhotoRow, SyncReport};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use imaging::{PreviewCache, PreviewSize, SourceKind};

/// Everything a worker needs to know about a photo.
#[derive(Debug, Clone)]
pub struct PhotoRef {
    pub id: i64,
    pub path: PathBuf,
    pub kind: FileKind,
    pub fingerprint: u64,
}

impl From<&PhotoRow> for PhotoRef {
    fn from(p: &PhotoRow) -> Self {
        PhotoRef {
            id: p.id,
            path: p.path.clone(),
            kind: p.kind,
            fingerprint: imaging::fingerprint(p.size_bytes, p.mtime_ms),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Job {
    /// Add or re-synchronize a folder tree, then queue previews for it.
    Sync { root: PathBuf },
    /// Load (building first if needed) a preview and send it to the UI.
    Load { photo: PhotoRef, size: PreviewSize },
    /// Make sure both previews exist on disk (and metadata is read).
    BuildPreviews { photo: PhotoRef },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum JobKey {
    Load(i64, PreviewSize),
    Build(i64),
}

impl Job {
    fn key(&self) -> Option<JobKey> {
        match self {
            Job::Sync { .. } => None,
            Job::Load { photo, size } => Some(JobKey::Load(photo.id, *size)),
            Job::BuildPreviews { photo } => Some(JobKey::Build(photo.id)),
        }
    }
}

/// Per-channel (R, G, B) counts of 8-bit values.
pub type Histogram = [[u32; 256]; 3];

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
    /// Metadata (capture time, camera...) was read and stored for a photo.
    MetadataUpdated {
        photo_id: i64,
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
    fn pop(&mut self) -> Option<Job> {
        let job = self
            .visible
            .pop_back()
            .or_else(|| self.normal.pop_front())
            .or_else(|| self.background.pop_front())?;
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
    syncing: Mutex<Option<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    pub syncing: Option<String>,
    pub previews: Option<(usize, usize)>,
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
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).max(1));
        for i in 0..workers {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name(format!("worker-{i}"))
                .spawn(move || worker_loop(&shared))
                .expect("spawn worker thread");
        }
        log::info!("started {workers} worker threads");
        (JobSystem { shared }, rx)
    }

    pub fn submit_visible(&self, job: Job) {
        self.submit(job, Lane::Visible);
    }

    pub fn submit_normal(&self, job: Job) {
        self.submit(job, Lane::Normal);
    }

    fn submit(&self, job: Job, lane: Lane) {
        submit(&self.shared, job, lane);
    }

    pub fn progress(&self) -> ProgressSnapshot {
        let p = &self.shared.progress;
        let total = p.previews_total.load(Ordering::Relaxed);
        let done = p.previews_done.load(Ordering::Relaxed);
        ProgressSnapshot {
            syncing: p.syncing.lock().unwrap().clone(),
            previews: (total > 0 && done < total).then_some((done, total)),
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
        Lane::Background => lanes.background.push_back(job),
    }
    drop(lanes);
    shared.wake.notify_one();
}

fn worker_loop(shared: &Shared) {
    // Each worker has its own SQLite connection (WAL mode allows this safely).
    let mut catalog: Option<Catalog> = None;
    loop {
        let job = {
            let mut lanes = shared.lanes.lock().unwrap();
            loop {
                if shared.shutdown.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(job) = lanes.pop() {
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
    }
}

/// Make sure both previews for `photo` exist (building them and storing metadata if
/// not). If another worker is already building this photo, wait for it instead of
/// racing it on the same cache files.
fn ensure_previews(shared: &Shared, cat: &Catalog, photo: &PhotoRef) -> Result<()> {
    {
        let mut building = shared.building.lock().unwrap();
        while building.contains(&photo.id) {
            building = shared.building_done.wait(building).unwrap();
        }
        if shared.cache.has_all(photo.id, photo.fingerprint) {
            return Ok(());
        }
        building.insert(photo.id);
    }
    let result = (|| {
        anyhow::ensure!(photo.path.exists(), "file is missing");
        let built = shared.cache.build(photo.id, photo.fingerprint, &photo.path, source_kind(photo.kind))?;
        cat.set_metadata(photo.id, &to_catalog_metadata(built.metadata))?;
        let _ = shared.events.send(Event::MetadataUpdated { photo_id: photo.id });
        Ok(())
    })();
    shared.building.lock().unwrap().remove(&photo.id);
    shared.building_done.notify_all();
    result
}

fn run(shared: &Shared, cat: &mut Catalog, job: Job) {
    match job {
        Job::Sync { root } => {
            *shared.progress.syncing.lock().unwrap() =
                Some(format!("Synchronizing {}", root.file_name().unwrap_or_default().to_string_lossy()));
            let result = sync_and_queue(shared, cat, &root).map_err(|e| format!("{e:#}"));
            *shared.progress.syncing.lock().unwrap() = None;
            let _ = shared.events.send(Event::SyncDone { root, result });
        }
        Job::Load { photo, size } => {
            let result = (|| -> Result<(egui::ColorImage, Option<Box<Histogram>>)> {
                let path = match shared.cache.get(photo.id, size, photo.fingerprint) {
                    Some(p) => p,
                    None => {
                        ensure_previews(shared, cat, &photo)?;
                        shared.cache.path(photo.id, size, photo.fingerprint)
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
        Job::BuildPreviews { photo } => {
            if photo.path.exists()
                && let Err(e) = ensure_previews(shared, cat, &photo)
            {
                log::warn!("building previews for {}: {e:#}", photo.path.display());
            }
            shared.progress.previews_done.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn sync_and_queue(shared: &Shared, cat: &mut Catalog, root: &std::path::Path) -> Result<SyncReport> {
    let report = cat.sync_folder(root)?;
    let canonical = dunce::canonicalize(root)?;
    if let Some(folder) = cat.folder_by_path(&canonical)? {
        let todo: Vec<PhotoRef> = cat
            .photos_in_folder(folder, true)?
            .iter()
            .filter(|p| !p.missing)
            .map(PhotoRef::from)
            .filter(|p| !shared.cache.has_all(p.id, p.fingerprint))
            .collect();
        let p = &shared.progress;
        if p.previews_done.load(Ordering::Relaxed) >= p.previews_total.load(Ordering::Relaxed) {
            p.previews_total.store(0, Ordering::Relaxed);
            p.previews_done.store(0, Ordering::Relaxed);
        }
        p.previews_total.fetch_add(todo.len(), Ordering::Relaxed);
        for photo in todo {
            submit(shared, Job::BuildPreviews { photo }, Lane::Background);
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: i64) -> PhotoRef {
        PhotoRef { id, path: PathBuf::new(), kind: FileKind::Image, fingerprint: 0 }
    }

    #[test]
    fn visible_lane_is_lifo_and_takes_priority() {
        let mut lanes = Lanes::default();
        lanes.background.push_back(Job::BuildPreviews { photo: photo(1) });
        lanes.normal.push_back(Job::Sync { root: PathBuf::from("x") });
        lanes.visible.push_back(Job::Load { photo: photo(2), size: PreviewSize::Thumb });
        lanes.visible.push_back(Job::Load { photo: photo(3), size: PreviewSize::Thumb });

        let order: Vec<String> = std::iter::from_fn(|| lanes.pop())
            .map(|j| match j {
                Job::Load { photo, .. } => format!("load{}", photo.id),
                Job::Sync { .. } => "sync".into(),
                Job::BuildPreviews { photo } => format!("build{}", photo.id),
            })
            .collect();
        assert_eq!(order, ["load3", "load2", "sync", "build1"]);
    }
}
