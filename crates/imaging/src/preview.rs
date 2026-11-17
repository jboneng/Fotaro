//! On-disk preview cache (the equivalent of LrC's `Previews.lrdata`).
//!
//! Layout: `<catalog>/previews/<xx>/<photo_id>_<size>_<fingerprint>.jpg`, where `xx`
//! spreads files over 256 subfolders and the fingerprint changes whenever the
//! source file's size or modification time changes, so edited files get fresh previews.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{DynamicImage, RgbImage};

use crate::decode::{SourceKind, decode, decode_full};
use crate::develop::{DevelopSettings, WorkingImage, render_srgb8};

/// Full-size (1:1) previews are large; like LrC, discard ones unused for this long.
const FULL_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);
use crate::metadata::Metadata;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewSize {
    /// Grid and filmstrip thumbnails.
    Thumb,
    /// Loupe / full-screen preview.
    Standard,
    /// Full resolution, for 1:1 zoom (LrC's "1:1 previews"). Built on demand.
    Full,
}

impl PreviewSize {
    pub const fn max_px(self) -> u32 {
        match self {
            PreviewSize::Thumb => 256,
            PreviewSize::Standard => 2048,
            PreviewSize::Full => u32::MAX,
        }
    }

    const fn tag(self) -> &'static str {
        match self {
            PreviewSize::Thumb => "t",
            PreviewSize::Standard => "s",
            PreviewSize::Full => "f",
        }
    }

    const fn quality(self) -> u8 {
        match self {
            PreviewSize::Thumb => 80,
            PreviewSize::Standard => 88,
            PreviewSize::Full => 90,
        }
    }
}

/// Identifies one version of a source file.
pub fn fingerprint(size: u64, mtime_ms: i64) -> u64 {
    // FNV-1a over both values: stable across runs and platforms.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in size.to_le_bytes().into_iter().chain(mtime_ms.to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The photo as previews show it: the camera's rendering (`decode`), or developed. `None`
/// for `max_px`: full size.
fn render(
    source: &Path,
    kind: SourceKind,
    max_px: Option<u32>,
    develop: Option<&DevelopSettings>,
) -> Result<(RgbImage, Metadata)> {
    match (develop, max_px) {
        (Some(s), _) => {
            let (img, meta) = WorkingImage::load(source, kind, max_px)?;
            Ok((render_srgb8(&img, s), meta))
        }
        (None, Some(px)) => decode(source, kind, px),
        (None, None) => decode_full(source, kind),
    }
}

/// The fingerprint of a developed photo's previews: the file's, mixed with the settings.
pub fn developed_fingerprint(file: u64, settings: &DevelopSettings) -> u64 {
    let mut h = file;
    for b in settings.stable_hash().to_le_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub struct BuiltPreviews {
    pub thumb: PathBuf,
    pub standard: PathBuf,
    pub metadata: Metadata,
}

#[derive(Debug, Clone)]
pub struct PreviewCache {
    root: PathBuf,
}

impl PreviewCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        PreviewCache { root: root.into() }
    }

    fn bucket(&self, photo_id: i64) -> PathBuf {
        self.root.join(format!("{:02x}", photo_id.rem_euclid(256)))
    }

    pub fn path(&self, photo_id: i64, size: PreviewSize, fingerprint: u64) -> PathBuf {
        self.bucket(photo_id).join(format!("{photo_id}_{}_{fingerprint:016x}.jpg", size.tag()))
    }

    /// The cached preview for this exact file version, if present.
    pub fn get(&self, photo_id: i64, size: PreviewSize, fingerprint: u64) -> Option<PathBuf> {
        Some(self.path(photo_id, size, fingerprint)).filter(|p| p.is_file())
    }

    pub fn has_all(&self, photo_id: i64, fingerprint: u64) -> bool {
        [PreviewSize::Thumb, PreviewSize::Standard]
            .into_iter()
            .all(|s| self.get(photo_id, s, fingerprint).is_some())
    }

    /// Decode the source once and write both preview sizes, removing stale versions.
    /// With `develop`, the previews show the photo as developed.
    pub fn build(
        &self,
        photo_id: i64,
        fingerprint: u64,
        source: &Path,
        kind: SourceKind,
        develop: Option<&DevelopSettings>,
    ) -> Result<BuiltPreviews> {
        let (standard, metadata) = render(source, kind, Some(PreviewSize::Standard.max_px()), develop)?;
        let thumb = DynamicImage::ImageRgb8(standard.clone())
            .thumbnail(PreviewSize::Thumb.max_px(), PreviewSize::Thumb.max_px())
            .into_rgb8();

        std::fs::create_dir_all(self.bucket(photo_id))?;
        self.remove_stale(photo_id, fingerprint);
        let thumb_path = self.write(photo_id, PreviewSize::Thumb, fingerprint, &thumb)?;
        let standard_path = self.write(photo_id, PreviewSize::Standard, fingerprint, &standard)?;
        Ok(BuiltPreviews { thumb: thumb_path, standard: standard_path, metadata })
    }

    fn write(&self, photo_id: i64, size: PreviewSize, fp: u64, img: &RgbImage) -> Result<PathBuf> {
        let path = self.path(photo_id, size, fp);
        // Write to a temp name then rename, so a crash never leaves a truncated preview.
        let tmp = path.with_extension("tmp");
        // jpeg-encoder (SIMD) is ~3x faster than image's encoder for previews.
        let (w, h) = (u16::try_from(img.width())?, u16::try_from(img.height())?);
        let mut buf = Vec::with_capacity(img.as_raw().len() / 10);
        jpeg_encoder::Encoder::new(&mut buf, size.quality())
            .encode(img.as_raw(), w, h, jpeg_encoder::ColorType::Rgb)
            .context("encoding preview JPEG")?;
        std::fs::write(&tmp, &buf)?;
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// The full-resolution preview for this file version, building it if needed.
    /// Cache hits are "touched" so [`Self::purge_orphans`] keeps previews in use.
    pub fn full(
        &self,
        photo_id: i64,
        fingerprint: u64,
        source: &Path,
        kind: SourceKind,
        develop: Option<&DevelopSettings>,
    ) -> Result<PathBuf> {
        if let Some(path) = self.get(photo_id, PreviewSize::Full, fingerprint) {
            let touched = std::fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|f| f.set_modified(std::time::SystemTime::now()));
            if let Err(e) = touched {
                log::debug!("could not touch {}: {e}", path.display());
            }
            return Ok(path);
        }
        let (img, _) = render(source, kind, None, develop)?;
        std::fs::create_dir_all(self.bucket(photo_id))?;
        self.write(photo_id, PreviewSize::Full, fingerprint, &img)
    }

    /// Delete previews of older versions of this photo.
    fn remove_stale(&self, photo_id: i64, fingerprint: u64) {
        let prefix = format!("{photo_id}_");
        let keep = format!("{fingerprint:016x}.jpg");
        let Ok(entries) = std::fs::read_dir(self.bucket(photo_id)) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && !name.ends_with(&keep) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Remove all previews of a photo (e.g. when it is removed from the catalog).
    pub fn remove(&self, photo_id: i64) {
        self.remove_stale(photo_id, u64::MAX);
    }

    /// Delete previews nothing will ever ask for again: photos no longer in the catalog,
    /// older versions of changed files, and temp files from interrupted writes. These pile
    /// up after crashes, older app versions, or catalog edits made while previews built.
    ///
    /// `wanted(photo_id, fingerprint)` says whether a preview is still current. Files
    /// modified after `snapshot` are kept: the caller's `wanted` view of the catalog was
    /// taken then, so newer previews may belong to photos it does not know about yet.
    /// Files that do not look like previews are left alone.
    pub fn purge_orphans(
        &self,
        snapshot: std::time::SystemTime,
        wanted: impl Fn(i64, u64) -> bool,
    ) -> PurgeReport {
        let mut report = PurgeReport::default();
        let Ok(buckets) = std::fs::read_dir(&self.root) else { return report };
        for bucket in buckets.flatten() {
            let Ok(entries) = std::fs::read_dir(bucket.path()) else { continue };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(parsed) = parse_preview_name(&name) else { continue };
                let Ok(meta) = entry.metadata() else { continue };
                if meta.modified().is_ok_and(|m| m >= snapshot) {
                    continue;
                }
                report.scanned += 1;
                let unused_full = |m: &std::fs::Metadata| {
                    m.modified()
                        .ok()
                        .and_then(|t| snapshot.duration_since(t).ok())
                        .is_some_and(|age| age > FULL_MAX_AGE)
                };
                let orphan = match parsed {
                    PreviewName::Temp => true,
                    PreviewName::Preview { photo_id, fingerprint, full } => {
                        !wanted(photo_id, fingerprint) || (full && unused_full(&meta))
                    }
                };
                if orphan && std::fs::remove_file(entry.path()).is_ok() {
                    report.removed += 1;
                    report.bytes += meta.len();
                }
            }
        }
        report
    }
}

/// What [`PreviewCache::purge_orphans`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PurgeReport {
    pub scanned: usize,
    pub removed: usize,
    pub bytes: u64,
}

enum PreviewName {
    Preview { photo_id: i64, fingerprint: u64, full: bool },
    Temp,
}

/// Parse `<photo_id>_<t|s|f>_<fingerprint:016x>.jpg` (or the `.tmp` written before the rename).
fn parse_preview_name(name: &str) -> Option<PreviewName> {
    let (stem, ext) = name.rsplit_once('.')?;
    let mut parts = stem.split('_');
    let photo_id = parts.next()?.parse().ok()?;
    let tag = parts.next()?;
    let fp = parts.next()?;
    if parts.next().is_some() || !matches!(tag, "t" | "s" | "f") || fp.len() != 16 {
        return None;
    }
    let fingerprint = u64::from_str_radix(fp, 16).ok()?;
    match ext {
        "jpg" => Some(PreviewName::Preview { photo_id, fingerprint, full: tag == "f" }),
        "tmp" => Some(PreviewName::Temp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn purges_orphans_but_keeps_current_new_and_foreign_files() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = PreviewCache::new(tmp.path().join("previews"));
        let touch = |p: PathBuf| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"jpeg").unwrap();
            p
        };
        let current = touch(cache.path(1, PreviewSize::Thumb, 0xaa));
        let old_version = touch(cache.path(1, PreviewSize::Standard, 0xbb));
        let removed_photo = touch(cache.path(2, PreviewSize::Thumb, 0xcc));
        let temp = touch(cache.path(3, PreviewSize::Standard, 0xdd).with_extension("tmp"));
        let foreign = touch(cache.bucket(4).join("notes.txt"));

        let snapshot = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        let wanted = |id: i64, fp: u64| (id, fp) == (1, 0xaa);
        // Previews written after the catalog snapshot are never touched.
        let r = cache.purge_orphans(std::time::SystemTime::UNIX_EPOCH, wanted);
        assert_eq!(r.removed, 0);
        let r = cache.purge_orphans(snapshot, wanted);
        assert_eq!((r.scanned, r.removed, r.bytes), (4, 3, 12));
        assert!(current.exists() && foreign.exists());
        assert!(!old_version.exists() && !removed_photo.exists() && !temp.exists());
    }

    #[test]
    fn full_previews_are_native_size_reused_and_expire() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("photo.png");
        RgbImage::from_pixel(3000, 2000, Rgb([10, 20, 30])).save(&src).unwrap();
        let cache = PreviewCache::new(tmp.path().join("previews"));

        let p = cache.full(5, 9, &src, SourceKind::Image, None).unwrap();
        let img = image::open(&p).unwrap();
        assert_eq!((img.width(), img.height()), (3000, 2000));
        assert_eq!(cache.full(5, 9, &src, SourceKind::Image, None).unwrap(), p, "cached");

        let now = std::time::SystemTime::now();
        let old = now - FULL_MAX_AGE - std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
        let r = cache.purge_orphans(now, |_, _| true);
        assert_eq!(r.removed, 1, "unused 1:1 previews are discarded even for current photos");
        assert!(!p.exists());
    }

    #[test]
    fn fingerprint_changes_with_size_or_mtime() {
        let a = fingerprint(100, 1_000);
        assert_eq!(a, fingerprint(100, 1_000));
        assert_ne!(a, fingerprint(101, 1_000));
        assert_ne!(a, fingerprint(100, 1_001));
    }

    #[test]
    fn builds_both_sizes_and_replaces_stale_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("photo.png");
        RgbImage::from_pixel(3000, 2000, Rgb([200, 100, 50])).save(&src).unwrap();
        let cache = PreviewCache::new(tmp.path().join("previews"));

        let v1 = cache.build(7, 1, &src, SourceKind::Image, None).unwrap();
        let t = image::open(&v1.thumb).unwrap();
        let s = image::open(&v1.standard).unwrap();
        assert_eq!((t.width(), t.height()), (256, 171));
        assert_eq!((s.width(), s.height()), (2048, 1365));
        assert!(cache.has_all(7, 1));
        assert_eq!(v1.metadata.width, Some(3000));

        cache.build(7, 2, &src, SourceKind::Image, None).unwrap();
        assert!(cache.has_all(7, 2));
        assert!(!v1.thumb.exists() && !v1.standard.exists(), "stale previews must be removed");
        assert!(cache.get(7, PreviewSize::Thumb, 1).is_none());

        cache.remove(7);
        assert!(!cache.has_all(7, 2));
    }
}
