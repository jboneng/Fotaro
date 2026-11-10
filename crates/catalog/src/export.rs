//! Export helpers that need the catalog: which metadata goes into exported files,
//! and how exported files are named (LrC's Export dialog). Rendering and encoding
//! live in the imaging crate.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use xmp::XmpData;

use crate::catalog::Catalog;
use crate::sync::{MetadataReader, SyncOptions};

/// LrC's Metadata ▸ Include.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MetadataLevel {
    CopyrightOnly,
    CopyrightAndContact,
    AllExceptCameraInfo,
    #[default]
    All,
}

impl MetadataLevel {
    pub const ALL: [MetadataLevel; 4] =
        [Self::CopyrightOnly, Self::CopyrightAndContact, Self::AllExceptCameraInfo, Self::All];

    pub fn label(self) -> &'static str {
        match self {
            Self::CopyrightOnly => "Copyright Only",
            Self::CopyrightAndContact => "Copyright & Contact Info Only",
            Self::AllExceptCameraInfo => "All Except Camera Info",
            Self::All => "All Metadata",
        }
    }

    /// Camera, lens, exposure and capture time.
    pub fn camera_info(self) -> bool {
        self == Self::All
    }

    /// The GPS position (LrC keeps it unless Remove Location Info is ticked).
    pub fn location(self) -> bool {
        matches!(self, Self::AllExceptCameraInfo | Self::All)
    }

    /// Keep only what this level exports. Fields left out become `None` (not written).
    /// The pick flag is this app's own and never leaves the catalog.
    pub fn filter(self, d: &XmpData) -> XmpData {
        let mut out = XmpData { copyright: d.copyright.clone(), ..Default::default() };
        if self != Self::CopyrightOnly {
            out.creator = d.creator.clone();
        }
        if matches!(self, Self::AllExceptCameraInfo | Self::All) {
            out.title = d.title.clone();
            out.caption = d.caption.clone();
            out.keywords = d.keywords.clone();
            out.rating = d.rating.filter(|r| *r > 0);
            out.label = d.label.clone();
        }
        // Empty strings mean "remove" when updating a sidecar; in a new file, leave them out.
        for v in [&mut out.copyright, &mut out.creator, &mut out.title, &mut out.caption, &mut out.label] {
            if v.as_deref().is_some_and(|s| s.trim().is_empty()) {
                *v = None;
            }
        }
        if out.keywords.as_ref().is_some_and(Vec::is_empty) {
            out.keywords = None;
        }
        out
    }
}

/// An XMP packet holding `data`, for embedding in an exported file.
pub fn xmp_packet(data: &XmpData) -> String {
    let mut doc = xmp::XmpDoc::default();
    doc.apply(data);
    doc.to_xml()
}

/// Update (or create) the XMP sidecar of an exported original.
pub fn write_sidecar(photo: &Path, raw: bool, data: &XmpData) -> Result<()> {
    xmp::write_sidecar(photo, raw, data, |_| true).map(|_| ())
}

/// LrC's Export Location ▸ Existing Files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExistingFiles {
    /// "Choose a new name for the exported file": `name-2.jpg`.
    #[default]
    NewName,
    /// "Overwrite WITHOUT WARNING".
    Overwrite,
    /// "Skip".
    Skip,
}

impl ExistingFiles {
    pub const ALL: [ExistingFiles; 3] = [Self::NewName, Self::Overwrite, Self::Skip];

    pub fn label(self) -> &'static str {
        match self {
            Self::NewName => "Choose a new name for the exported file",
            Self::Overwrite => "Overwrite WITHOUT WARNING",
            Self::Skip => "Skip",
        }
    }
}

/// Where one export goes: `dir/name.ext`, made unique against files on disk (per
/// `existing`) and against the other files of the same export (`taken`), so a photo
/// and its virtual copy never overwrite each other. `None` means skip it.
pub fn export_path(
    dir: &Path,
    name: &str,
    ext: &str,
    existing: ExistingFiles,
    taken: &mut HashSet<PathBuf>,
) -> Option<PathBuf> {
    let file = |n: &str| if ext.is_empty() { dir.join(n) } else { dir.join(format!("{n}.{ext}")) };
    let key = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    let first = file(name);
    let clash_on_disk = first.exists();
    if !taken.contains(&key(&first)) && (!clash_on_disk || existing == ExistingFiles::Overwrite) {
        taken.insert(key(&first));
        return Some(first);
    }
    if clash_on_disk && existing == ExistingFiles::Skip {
        return None;
    }
    (2..).map(|i| file(&format!("{name}-{i}"))).find(|p| !taken.contains(&key(p)) && !p.exists()).inspect(
        |p| {
            taken.insert(key(p));
        },
    )
}

impl Catalog {
    /// The photo's catalog metadata as XMP fields (no orientation).
    pub fn xmp_data(&self, photo_id: i64) -> Result<XmpData> {
        crate::xmp_sync::gather_data(&self.conn, photo_id)
    }

    /// Add files this app wrote (exports, later Edit In results) as photos of their own:
    /// never paired with a raw file of the same name. Returns each path's photo id
    /// (`None` if it couldn't be added).
    pub fn add_files(
        &mut self,
        paths: &[PathBuf],
        reader: Option<MetadataReader>,
    ) -> Result<Vec<Option<i64>>> {
        let paths: Vec<PathBuf> =
            paths.iter().map(|p| dunce::canonicalize(p).unwrap_or_else(|_| p.clone())).collect();
        let only: HashSet<PathBuf> = paths.iter().cloned().collect();
        let mut dirs: Vec<&Path> = paths.iter().filter_map(|p| p.parent()).collect();
        dirs.sort();
        dirs.dedup();
        for dir in dirs {
            self.ensure_folder_path(dir, dir)?;
            let opts = SyncOptions {
                reader,
                only: Some(&only),
                recursive: false,
                imported_at: None,
                pair_raw_jpeg: false,
            };
            self.sync_folder_opts(dir, opts)?;
        }
        paths
            .iter()
            .map(|p| Ok(self.photo_ids_for_paths(std::slice::from_ref(p))?.first().copied()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_on_disk_and_within_an_export() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        std::fs::write(d.join("a.jpg"), b"x").unwrap();
        let mut taken = HashSet::new();
        let p = |name, existing, taken: &mut HashSet<PathBuf>| export_path(d, name, "jpg", existing, taken);
        assert_eq!(p("a", ExistingFiles::NewName, &mut taken), Some(d.join("a-2.jpg")));
        assert_eq!(p("a", ExistingFiles::NewName, &mut taken), Some(d.join("a-3.jpg")), "same export");
        assert_eq!(p("A", ExistingFiles::Skip, &mut taken), None, "case-insensitive clash on disk");
        assert_eq!(p("b", ExistingFiles::Skip, &mut taken), Some(d.join("b.jpg")));
        assert_eq!(p("b", ExistingFiles::Skip, &mut taken), Some(d.join("b-2.jpg")), "own files are renamed");

        let mut taken = HashSet::new();
        assert_eq!(p("a", ExistingFiles::Overwrite, &mut taken), Some(d.join("a.jpg")));
        assert_eq!(p("a", ExistingFiles::Overwrite, &mut taken), Some(d.join("a-2.jpg")), "never its own");
    }

    #[test]
    fn metadata_levels() {
        let d = XmpData {
            rating: Some(3),
            label: Some("Red".into()),
            pick: Some(1),
            orientation: Some(6),
            keywords: Some(vec![vec!["Places".into(), "Oslo".into()]]),
            title: Some("T".into()),
            caption: Some(String::new()),
            creator: Some("Jo".into()),
            copyright: Some("(c) Jo".into()),
        };
        let c = MetadataLevel::CopyrightOnly.filter(&d);
        assert_eq!(c, XmpData { copyright: Some("(c) Jo".into()), ..Default::default() });
        let c = MetadataLevel::CopyrightAndContact.filter(&d);
        assert_eq!(c.creator.as_deref(), Some("Jo"));
        assert!(c.keywords.is_none());
        let all = MetadataLevel::All.filter(&d);
        assert_eq!((all.rating, all.pick, all.orientation, all.caption.clone()), (Some(3), None, None, None));
        let xml = xmp_packet(&all);
        assert!(xml.contains("Oslo") && xml.contains("Places|Oslo") && xml.contains("(c) Jo"), "{xml}");
        assert_eq!(xmp::XmpDoc::parse(&xml).unwrap().data().title.as_deref(), Some("T"));
    }

    #[test]
    fn exported_file_joins_its_original_picture() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        std::fs::create_dir(&dir).unwrap();
        for n in ["a.NEF", "b.NEF"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        cat.sync_folder(&dir).unwrap();
        // An export named like its raw file is a photo of its own, not a RAW+JPEG pair.
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        let added = cat.add_files(&[dir.join("a.jpg")], None).unwrap();
        assert!(added[0].is_some());
        let id = |cat: &Catalog, n: &str| {
            cat.photos(crate::Source::AllPhotographs)
                .unwrap()
                .into_iter()
                .find(|p| p.file_name == n)
                .unwrap()
                .id
        };
        let (a, b, e) = (id(&cat, "a.NEF"), id(&cat, "b.NEF"), id(&cat, "a.jpg"));
        assert_eq!(added[0], Some(e));
        assert_ne!(
            cat.photo(e).unwrap().unwrap().unit.picture_id,
            cat.photo(a).unwrap().unwrap().unit.picture_id
        );
        cat.add_to_picture(a, e, crate::Role::Export).unwrap();
        let photos = cat.photos(crate::Source::AllPhotographs).unwrap();
        assert_eq!(photos.iter().map(|p| (p.id, p.unit.members)).collect::<Vec<_>>(), vec![(a, 2), (b, 1)]);
        let members = cat.unit_members(a).unwrap();
        assert_eq!((members[1].photo.id, members[1].photo.unit.role), (e, crate::Role::Export));
    }
}
