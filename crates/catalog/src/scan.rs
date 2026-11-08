//! Discovering and indexing photos on disk. No GUI code here.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use walkdir::WalkDir;

/// Camera RAW extensions (compared case-insensitively). Decoded via LibRaw.
pub const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "cr2", "cr3", "crw", "dng", "erf", "iiq", "kdc", "mef", "mos", "mrw", "nef", "nrw", "orf",
    "pef", "raf", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

/// Non-RAW image extensions (compared case-insensitively).
pub const IMAGE_EXTENSIONS: &[&str] =
    &["jpg", "jpeg", "png", "gif", "bmp", "webp", "tif", "tiff", "heic", "heif"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FileKind {
    Raw,
    Image,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Raw => "raw",
            FileKind::Image => "image",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "raw" => Some(FileKind::Raw),
            "image" => Some(FileKind::Image),
            _ => None,
        }
    }
}

pub fn file_kind(path: &Path) -> Option<FileKind> {
    let ext = path.extension()?.to_str()?;
    let has = |list: &[&str]| list.iter().any(|e| e.eq_ignore_ascii_case(ext));
    if has(RAW_EXTENSIONS) {
        Some(FileKind::Raw)
    } else if has(IMAGE_EXTENSIONS) {
        Some(FileKind::Image)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Photo {
    pub path: PathBuf,
    pub file_name: String,
    pub size_bytes: u64,
}

pub fn is_photo(path: &Path) -> bool {
    file_kind(path).is_some()
}

/// Recursively scan `root` for photos, sorted by path.
/// Unreadable entries are logged and skipped rather than failing the scan.
pub fn scan(root: &Path) -> Result<Vec<Photo>> {
    if !root.is_dir() {
        anyhow::bail!("{} is not a directory", root.display());
    }

    let mut photos = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                log::warn!("skipping entry: {err}");
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_photo(entry.path()) {
            continue;
        }
        let size_bytes = entry
            .metadata()
            .with_context(|| format!("reading metadata for {}", entry.path().display()))
            .map(|m| m.len())
            .unwrap_or_else(|err| {
                log::warn!("{err:#}");
                0
            });
        photos.push(Photo {
            file_name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.into_path(),
            size_bytes,
        });
    }
    photos.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(photos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn recognizes_photo_extensions() {
        assert!(is_photo(Path::new("a.jpg")));
        assert!(is_photo(Path::new("B.JPEG")));
        assert!(is_photo(Path::new("dir/c.Png")));
        assert_eq!(file_kind(Path::new("x.NEF")), Some(FileKind::Raw));
        assert_eq!(file_kind(Path::new("x.cr3")), Some(FileKind::Raw));
        assert_eq!(file_kind(Path::new("scan.tif")), Some(FileKind::Image));
        assert!(!is_photo(Path::new("notes.txt")));
        assert!(!is_photo(Path::new("no_extension")));
    }

    #[test]
    fn scan_finds_nested_photos_only() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("one.jpg"), b"123").unwrap();
        fs::write(dir.path().join("sub/two.PNG"), b"").unwrap();
        fs::write(dir.path().join("readme.txt"), b"").unwrap();

        let photos = scan(dir.path()).unwrap();
        let names: Vec<_> = photos.iter().map(|p| p.file_name.as_str()).collect();
        assert_eq!(names, ["one.jpg", "two.PNG"]);
        assert_eq!(photos[0].size_bytes, 3);
    }

    #[test]
    fn scan_rejects_non_directory() {
        assert!(scan(Path::new("definitely/does/not/exist")).is_err());
    }
}
