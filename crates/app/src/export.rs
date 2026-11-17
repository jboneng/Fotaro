//! LrC's Export dialog settings and the export itself: plan file names, render and
//! encode photos in parallel, then optionally add the results to the catalog.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::{Context, Result};
use catalog::{Catalog, ExistingFiles, FileKind, MetadataLevel};
use imaging::export::{ColorSpace, EmbeddedMetadata, FileFormat, ImageOptions, SharpenFor, WatermarkStyle};
use serde::{Deserialize, Serialize};

/// LrC's Export Location ▸ Export To.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExportTo {
    #[default]
    SpecificFolder,
    SameFolder,
}

impl ExportTo {
    pub fn label(self) -> &'static str {
        match self {
            Self::SpecificFolder => "Specific folder",
            Self::SameFolder => "Same folder as original photo",
        }
    }
}

/// Export settings, remembered between runs (Export with Previous uses them as is).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportPrefs {
    pub export_to: ExportTo,
    pub folder: PathBuf,
    pub subfolder_on: bool,
    pub subfolder: String,
    pub add_to_catalog: bool,
    /// Each export joins its original's picture (role Export).
    pub add_to_stack: bool,
    pub existing: ExistingFiles,
    pub rename_on: bool,
    pub template: String,
    pub custom_text: String,
    pub start_number: u32,
    pub image: ImageOptions,
    pub metadata: MetadataLevel,
    /// Metadata ▸ Remove Location Info: leave out the GPS position.
    pub remove_location: bool,
    /// Post-Processing ▸ Show in Explorer / Finder.
    pub reveal: bool,
}

impl Default for ExportPrefs {
    fn default() -> Self {
        ExportPrefs {
            export_to: ExportTo::SpecificFolder,
            folder: PathBuf::new(),
            subfolder_on: false,
            subfolder: String::new(),
            add_to_catalog: false,
            add_to_stack: false,
            existing: ExistingFiles::NewName,
            rename_on: false,
            template: "{Filename}".to_owned(),
            custom_text: String::new(),
            start_number: 1,
            image: ImageOptions::default(),
            metadata: MetadataLevel::All,
            remove_location: false,
            reveal: true,
        }
    }
}

impl ExportPrefs {
    /// A short line for the dialog and notifications: "JPEG 80, 2048 px long edge".
    pub fn summary(&self) -> String {
        let mut s = self.file_settings();
        if let Some(sizing) = self.sizing() {
            s += ", ";
            s += &sizing;
        }
        s
    }

    /// "JPEG 80", "JPEG ≤ 500 K", "TIFF 16-bit ProPhoto RGB"…
    pub fn file_settings(&self) -> String {
        let i = &self.image;
        let mut s = i.format.label().to_owned();
        match i.format {
            FileFormat::Jpeg if i.limit_size => s += &format!(" ≤ {} K", i.limit_kb),
            FileFormat::Jpeg => s += &format!(" {}", i.jpeg_quality),
            FileFormat::Original => return s,
            _ if i.sixteen() => s += " 16-bit",
            _ => {}
        }
        if i.color_space != ColorSpace::Srgb {
            s += " ";
            s += i.color_space.label();
        }
        s
    }

    /// "2048 px long edge"…, or `None` at full size.
    pub fn sizing(&self) -> Option<String> {
        let i = &self.image;
        if !i.resize || i.format == FileFormat::Original {
            return None;
        }
        use imaging::export::ResizeMode as R;
        Some(match i.resize_mode {
            R::WidthHeight | R::Dimensions => format!("fit {}×{}", i.width, i.height),
            R::LongEdge => format!("{} px long edge", i.edge),
            R::ShortEdge => format!("{} px short edge", i.edge),
            R::Megapixels => format!("{} MP", i.megapixels),
            R::Percentage => format!("{}%", i.percent),
        })
    }

    /// Why the export can't start, if it can't.
    pub fn problem(&self) -> Option<&'static str> {
        if self.export_to == ExportTo::SpecificFolder && self.folder.as_os_str().is_empty() {
            return Some("Choose a folder to export to");
        }
        if self.subfolder_on && catalog::validate_folder_name(self.subfolder.trim()).is_err() {
            return Some("The subfolder name isn't a valid folder name");
        }
        let w = &self.image.watermark;
        if w.enabled && self.image.format != FileFormat::Original {
            match w.style {
                WatermarkStyle::Text if w.text.trim().is_empty() => return Some("Type the watermark text"),
                WatermarkStyle::Graphic if w.graphic.as_os_str().is_empty() => {
                    return Some("Choose a watermark graphic");
                }
                _ => {}
            }
        }
        None
    }

    fn dir_for(&self, original: &Path) -> PathBuf {
        let base = match self.export_to {
            ExportTo::SpecificFolder => self.folder.clone(),
            ExportTo::SameFolder => original.parent().map(Path::to_path_buf).unwrap_or_default(),
        };
        match self.subfolder.trim() {
            sub if self.subfolder_on && !sub.is_empty() => base.join(sub),
            _ => base,
        }
    }
}

/// A named set of export settings, saved by the user (LrC's User Presets). It holds
/// every setting, including where the files go.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub prefs: ExportPrefs,
}

/// Which preset: a built-in one (by index into [`BUILTIN_PRESETS`]) or a user preset
/// (by name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetId {
    Builtin(usize),
    User(String),
}

/// Built-in presets. They set what goes into the files (format, size, sharpening,
/// metadata) and keep the export location, naming and watermark you already have.
/// Changes a preset makes to the current settings.
pub type ApplyPreset = fn(&mut ExportPrefs);

pub const BUILTIN_PRESETS: &[(&str, ApplyPreset)] = &[
    ("Full-Sized JPEGs", |p| {
        set_image(p, FileFormat::Jpeg, ColorSpace::Srgb, 100, None, false, 300);
        p.metadata = MetadataLevel::All;
    }),
    ("For Web (2048 px)", |p| {
        set_image(p, FileFormat::Jpeg, ColorSpace::Srgb, 80, Some(2048), false, 72);
        sharpen(p, SharpenFor::Screen);
        p.metadata = MetadataLevel::AllExceptCameraInfo;
    }),
    ("For Email", |p| {
        set_image(p, FileFormat::Jpeg, ColorSpace::Srgb, 70, Some(1200), false, 72);
        sharpen(p, SharpenFor::Screen);
        p.metadata = MetadataLevel::CopyrightAndContact;
        p.remove_location = true;
    }),
    ("16-bit TIFF for Editing", |p| {
        set_image(p, FileFormat::Tiff, ColorSpace::ProPhotoRgb, 100, None, true, 300);
        p.image.tiff_zip = true;
        p.metadata = MetadataLevel::All;
    }),
    ("Print (Adobe RGB, 300 ppi)", |p| {
        set_image(p, FileFormat::Tiff, ColorSpace::AdobeRgb, 100, None, true, 300);
        p.image.tiff_zip = true;
        sharpen(p, SharpenFor::GlossyPaper);
        p.metadata = MetadataLevel::All;
    }),
];

fn set_image(
    p: &mut ExportPrefs,
    format: FileFormat,
    color_space: ColorSpace,
    quality: u8,
    long_edge: Option<u32>,
    sixteen_bit: bool,
    ppi: u32,
) {
    let i = &mut p.image;
    i.format = format;
    i.color_space = color_space;
    i.jpeg_quality = quality;
    i.limit_size = false;
    i.sixteen_bit = sixteen_bit;
    i.resize = long_edge.is_some();
    if let Some(edge) = long_edge {
        i.resize_mode = imaging::export::ResizeMode::LongEdge;
        i.edge = edge;
        i.dont_enlarge = true;
    }
    i.ppi = ppi;
    i.sharpen = false;
    p.remove_location = false;
}

fn sharpen(p: &mut ExportPrefs, medium: SharpenFor) {
    p.image.sharpen = true;
    p.image.sharpen_for = medium;
    p.image.sharpen_amount = imaging::export::SharpenAmount::Standard;
}

/// The settings `id` gives, starting from `current` (built-in presets only change part
/// of them). `None` if there is no such preset.
pub fn preset_prefs(current: &ExportPrefs, user: &[ExportPreset], id: &PresetId) -> Option<ExportPrefs> {
    match id {
        PresetId::Builtin(i) => {
            let (_, apply) = BUILTIN_PRESETS.get(*i)?;
            let mut p = current.clone();
            apply(&mut p);
            Some(p)
        }
        PresetId::User(name) => user.iter().find(|u| &u.name == name).map(|u| u.prefs.clone()),
    }
}

/// `name`, or `name 2`, `name 3`… so it doesn't clash with an existing user preset.
pub fn unique_preset_name(user: &[ExportPreset], name: &str) -> String {
    let name = name.trim();
    let name = if name.is_empty() { "Untitled Preset" } else { name };
    let taken = |n: &str| user.iter().any(|u| u.name.eq_ignore_ascii_case(n));
    if !taken(name) {
        return name.to_owned();
    }
    (2..).map(|i| format!("{name} {i}")).find(|n| !taken(n)).unwrap_or_default()
}

#[derive(Debug, Default)]
pub struct ExportReport {
    pub exported: Vec<PathBuf>,
    pub skipped: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub cancelled: bool,
    /// Photos added to the catalog.
    pub added: Vec<i64>,
}

struct Task {
    photo_id: i64,
    source: PathBuf,
    kind: FileKind,
    turns: u8,
    dest: PathBuf,
    /// Catalog metadata already reduced to the chosen Metadata level.
    data: catalog::XmpData,
    /// The photo's copyright, for the simple copyright watermark (whatever the level).
    copyright: Option<String>,
    /// The photo's Develop settings, applied to the exported pixels.
    develop: Option<imaging::develop::DevelopSettings>,
}

fn source_kind(kind: FileKind) -> imaging::SourceKind {
    if kind == FileKind::Raw { imaging::SourceKind::Raw } else { imaging::SourceKind::Image }
}

/// EXIF orientation code for clockwise quarter turns.
fn orientation_code(turns: u8) -> u8 {
    [1, 6, 3, 8][usize::from(turns % 4)]
}

/// Work out every file's destination. Photos that are skipped (existing files) or
/// can't be planned are counted in `report`.
fn plan(
    cat: &Catalog,
    photo_ids: &[i64],
    prefs: &ExportPrefs,
    report: &mut ExportReport,
) -> Result<Vec<Task>> {
    let mut taken = HashSet::new();
    let mut tasks = Vec::new();
    for (i, &id) in photo_ids.iter().enumerate() {
        let row = cat.photo(id)?.context("photo not found")?;
        let stem = row.path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let name = if prefs.rename_on {
            let seq = prefs.start_number.saturating_add(i as u32);
            match catalog::import::render_template(
                &prefs.template,
                &stem,
                &prefs.custom_text,
                seq,
                row.capture_time.unwrap_or(0),
            ) {
                Ok(n) if !n.trim().is_empty() => n,
                Ok(_) => stem.clone(),
                Err(e) => {
                    report.failed.push((row.path.clone(), format!("{e:#}")));
                    continue;
                }
            }
        } else {
            stem
        };
        let ext = match prefs.image.format.extension() {
            Some(e) => e.to_owned(),
            None => row.path.extension().unwrap_or_default().to_string_lossy().into_owned(),
        };
        let dir = prefs.dir_for(&row.path);
        let Some(dest) = catalog::export_path(&dir, &name, &ext, prefs.existing, &mut taken) else {
            report.skipped += 1;
            continue;
        };
        let all = cat.xmp_data(id)?;
        let data = prefs.metadata.filter(&all);
        tasks.push(Task {
            photo_id: id,
            source: row.path,
            kind: row.kind,
            turns: row.rotation,
            dest,
            data,
            copyright: all.copyright,
            develop: crate::jobs::render_settings(row.develop.as_deref(), row.kind),
        });
    }
    Ok(tasks)
}

fn export_one(t: &Task, prefs: &ExportPrefs) -> Result<()> {
    anyhow::ensure!(t.source.exists(), "the original file is missing");
    if let Some(dir) = t.dest.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    if prefs.image.format == FileFormat::Original {
        return export_original(t);
    }
    let (img, meta) = imaging::export::render(
        &t.source,
        source_kind(t.kind),
        t.turns,
        &prefs.image,
        t.copyright.as_deref(),
        t.develop.as_ref(),
    )?;
    let embedded = EmbeddedMetadata {
        gps: meta.gps.filter(|_| prefs.metadata.location() && !prefs.remove_location),
        camera: prefs.metadata.camera_info().then_some(meta),
        artist: t.data.creator.clone(),
        copyright: t.data.copyright.clone(),
        description: t.data.caption.clone(),
        xmp: Some(catalog::xmp_packet(&t.data)),
    };
    let bytes = imaging::export::encode(&img, &prefs.image, &embedded)?;
    imaging::export::write_atomically(&t.dest, &bytes)
}

/// The original file byte for byte, with an XMP sidecar holding the catalog metadata
/// (the original's sidecar is copied first, so other apps' settings travel along).
fn export_original(t: &Task) -> Result<()> {
    let raw = t.kind == FileKind::Raw;
    let mut part = t.dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::copy(&t.source, &part).with_context(|| format!("copying to {}", part.display()))?;
    std::fs::rename(&part, &t.dest).with_context(|| format!("writing {}", t.dest.display()))?;

    let src_sidecar = catalog::sidecar_path(&t.source, raw);
    let dest_sidecar = catalog::sidecar_path(&t.dest, raw);
    if src_sidecar.exists() {
        std::fs::copy(&src_sidecar, &dest_sidecar)
            .with_context(|| format!("copying {}", src_sidecar.display()))?;
    }
    let mut data = t.data.clone();
    if !t.turns.is_multiple_of(4) {
        let file_turns = imaging::read_metadata(&t.source, source_kind(t.kind))
            .ok()
            .and_then(|m| m.orientation)
            .unwrap_or(0);
        data.orientation = Some(orientation_code(file_turns + t.turns));
    }
    catalog::write_sidecar(&t.dest, raw, &data)
}

/// Run an export. `progress(done, total)` is called as files finish; `cancel` stops
/// before the next photo (finished files stay).
pub fn run(
    cat: &mut Catalog,
    photo_ids: &[i64],
    prefs: &ExportPrefs,
    reader: Option<catalog::MetadataReader>,
    progress: &(dyn Fn(usize, usize) + Sync),
    cancel: &AtomicBool,
) -> Result<ExportReport> {
    let mut report = ExportReport::default();
    let tasks = plan(cat, photo_ids, prefs, &mut report)?;
    let total = tasks.len();
    progress(0, total);

    // Full-size renders take ~150 MB each for a 45 MP photo (twice that at 16 bits): a
    // few at a time.
    let most = if prefs.image.sixteen() { 2 } else { 4 };
    let threads =
        std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, most)).min(total.max(1));
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Result<()>)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                // Like the job workers: the UI thread always wins the CPU.
                let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(t) = tasks.get(i) else { break };
                    let r = export_one(t, prefs);
                    if let Err(e) = &r {
                        log::warn!("export of {}: {e:#}", t.source.display());
                    }
                    results.lock().unwrap().push((i, r));
                    progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(i, _)| *i);
    report.cancelled = results.len() < total;
    let mut ok: Vec<&Task> = Vec::new();
    for (i, r) in results {
        let t = &tasks[i];
        match r {
            Ok(()) => {
                report.exported.push(t.dest.clone());
                ok.push(t);
            }
            Err(e) => report.failed.push((t.source.clone(), format!("{e:#}"))),
        }
    }

    if prefs.add_to_catalog && !ok.is_empty() {
        let paths: Vec<PathBuf> = ok.iter().map(|t| t.dest.clone()).collect();
        let ids = cat.add_files(&paths, reader)?;
        let stack = prefs.add_to_stack;
        for (t, id) in ok.iter().zip(ids) {
            let Some(id) = id else { continue };
            report.added.push(id);
            if stack && let Err(e) = cat.add_to_picture(t.photo_id, id, catalog::Role::Export) {
                log::warn!("stacking {}: {e:#}", t.dest.display());
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
