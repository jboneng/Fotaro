//! On-disk preview cache (the equivalent of LrC's `Previews.lrdata`).
//!
//! Layout: `<catalog>/previews/<xx>/<photo_id>_<size>_<fingerprint>.jpg`, where `xx`
//! spreads files over 256 subfolders and the fingerprint changes whenever the
//! source file's size or modification time changes, so edited files get fresh previews.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{DynamicImage, RgbImage};

use crate::decode::{SourceKind, decode};
use crate::metadata::Metadata;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewSize {
    /// Grid and filmstrip thumbnails.
    Thumb,
    /// Loupe / full-screen preview.
    Standard,
}

impl PreviewSize {
    pub const fn max_px(self) -> u32 {
        match self {
            PreviewSize::Thumb => 256,
            PreviewSize::Standard => 2048,
        }
    }

    const fn tag(self) -> &'static str {
        match self {
            PreviewSize::Thumb => "t",
            PreviewSize::Standard => "s",
        }
    }

    const fn quality(self) -> u8 {
        match self {
            PreviewSize::Thumb => 80,
            PreviewSize::Standard => 88,
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
    pub fn build(
        &self,
        photo_id: i64,
        fingerprint: u64,
        source: &Path,
        kind: SourceKind,
    ) -> Result<BuiltPreviews> {
        let (standard, metadata) = decode(source, kind, PreviewSize::Standard.max_px())?;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

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

        let v1 = cache.build(7, 1, &src, SourceKind::Image).unwrap();
        let t = image::open(&v1.thumb).unwrap();
        let s = image::open(&v1.standard).unwrap();
        assert_eq!((t.width(), t.height()), (256, 171));
        assert_eq!((s.width(), s.height()), (2048, 1365));
        assert!(cache.has_all(7, 1));
        assert_eq!(v1.metadata.width, Some(3000));

        cache.build(7, 2, &src, SourceKind::Image).unwrap();
        assert!(cache.has_all(7, 2));
        assert!(!v1.thumb.exists() && !v1.standard.exists(), "stale previews must be removed");
        assert!(cache.get(7, PreviewSize::Thumb, 1).is_none());

        cache.remove(7);
        assert!(!cache.has_all(7, 2));
    }
}
