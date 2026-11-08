//! "Add" import and "Synchronize Folder": reconcile a folder tree on disk with the catalog.
//!
//! Files that vanished are marked `missing` rather than deleted, so ratings,
//! keywords and collections survive unplugged drives and renamed volumes (as in LrC).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use walkdir::WalkDir;

use crate::catalog::{Catalog, PhotoMetadata, write_metadata};
use crate::scan::{FileKind, file_kind};
use crate::xmp_sync::apply_xmp;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub added: usize,
    pub updated: usize,
    pub restored: usize,
    pub missing: usize,
    pub errors: usize,
    /// JPEGs attached to a raw photo of the same name (RAW+JPEG).
    pub paired: usize,
}

struct Known {
    id: i64,
    size: i64,
    mtime_ms: i64,
    missing: bool,
}

/// Hidden files and folders, plus macOS AppleDouble `._*` files, are never photos.
fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

fn unix_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Reads camera metadata for a file during sync (supplied by the caller, so this
/// crate needs no image decoders). Must be thread-safe: files are read in parallel.
pub type MetadataReader<'a> = &'a (dyn Fn(&Path, FileKind) -> Option<PhotoMetadata> + Sync);

/// What the walk found for one photo file.
struct FileChange {
    path: PathBuf,
    kind: FileKind,
    size: i64,
    mtime_ms: i64,
    /// `None` for new files.
    existing_id: Option<i64>,
    metadata: Option<PhotoMetadata>,
    /// An XMP sidecar found next to a new file (from LrC, darktable, or an earlier catalog).
    sidecar: Option<xmp::XmpData>,
}

/// Read metadata (and, for new files, XMP sidecars) for `files` on all cores.
fn read_metadata_parallel(files: &mut [FileChange], reader: Option<MetadataReader>) {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = files.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for part in files.chunks_mut(chunk) {
            scope.spawn(move || {
                for f in part {
                    if let Some(reader) = reader {
                        f.metadata = reader(&f.path, f.kind);
                    }
                    if f.existing_id.is_none() {
                        f.sidecar = read_sidecar(&f.path, f.kind);
                    }
                }
            });
        }
    });
}

impl Catalog {
    /// Add `root` (recursively) to the catalog, or re-synchronize it if already present.
    pub fn sync_folder(&mut self, root: impl AsRef<Path>) -> Result<SyncReport> {
        self.sync_folder_with(root, None)
    }

    /// Like [`Catalog::sync_folder`], also reading metadata (capture time, camera...) of
    /// new and changed files so they sort correctly straight away.
    ///
    /// Work happens in three phases so the write lock is held only briefly:
    /// walk + diff (read-only), metadata reading (no lock), then one transaction.
    pub fn sync_folder_with(
        &mut self,
        root: impl AsRef<Path>,
        reader: Option<MetadataReader>,
    ) -> Result<SyncReport> {
        let root = dunce::canonicalize(root.as_ref())
            .with_context(|| format!("resolving {}", root.as_ref().display()))?;
        anyhow::ensure!(root.is_dir(), "{} is not a directory", root.display());

        let t_walk = std::time::Instant::now();
        // Phase 1: walk the tree and diff against the catalog.
        let mut report = SyncReport::default();
        let mut known = load_known(&self.conn, &root)?;
        let mut dirs: Vec<(PathBuf, usize)> = Vec::new();
        let mut changes: Vec<FileChange> = Vec::new();
        let mut restored: Vec<i64> = Vec::new();

        let walker = WalkDir::new(&root)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !is_hidden(e.file_name()));
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    log::warn!("sync: skipping entry: {err}");
                    report.errors += 1;
                    continue;
                }
            };
            let path = entry.path();
            if entry.file_type().is_dir() {
                dirs.push((path.to_path_buf(), entry.depth()));
                continue;
            }
            let Some(kind) = file_kind(path).filter(|_| entry.file_type().is_file()) else {
                continue;
            };
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(err) => {
                    log::warn!("sync: no metadata for {}: {err}", path.display());
                    report.errors += 1;
                    continue;
                }
            };
            let size = meta.len() as i64;
            let mtime_ms = meta.modified().map(unix_ms).unwrap_or(0);
            let change = |existing_id| FileChange {
                path: path.to_path_buf(),
                kind,
                size,
                mtime_ms,
                existing_id,
                metadata: None,
                sidecar: None,
            };
            match known.remove(path) {
                Some(k) if k.size != size || k.mtime_ms != mtime_ms => changes.push(change(Some(k.id))),
                Some(k) if k.missing => restored.push(k.id),
                Some(_) => {}
                None => changes.push(change(None)),
            }
        }

        let walk_time = t_walk.elapsed();
        let t_meta = std::time::Instant::now();
        // Phase 2: metadata for new/changed files, in parallel, without holding a lock.
        read_metadata_parallel(&mut changes, reader);

        let meta_time = t_meta.elapsed();
        let t_write = std::time::Instant::now();
        // Phase 3: apply everything in one transaction.
        let separate_raw_jpeg = self.separate_raw_jpeg()?;
        let tx = self.conn.transaction()?;
        let now = unix_ms(SystemTime::now()) / 1000;
        let mut folder_ids: HashMap<PathBuf, i64> = HashMap::new();
        for (path, depth) in &dirs {
            let parent_id = match path.parent() {
                Some(parent) if *depth > 0 => folder_ids.get(parent).copied(),
                // The root: link it under an existing catalog folder if there is one.
                Some(parent) => folder_id(&tx, parent)?,
                None => None,
            };
            let id = ensure_folder(&tx, path, parent_id)?;
            folder_ids.insert(path.clone(), id);
        }

        // New JPEGs next to a raw file of the same name become part of that raw photo
        // (unless the catalog treats them separately); handle them after the raws.
        let pair_jpegs = !separate_raw_jpeg;
        let is_pair_candidate = |c: &FileChange| {
            pair_jpegs && c.existing_id.is_none() && c.kind == FileKind::Image && is_jpeg(&c.path)
        };
        let ordered = changes
            .iter()
            .filter(|c| !is_pair_candidate(c))
            .chain(changes.iter().filter(|c| is_pair_candidate(c)));
        for c in ordered {
            let photo_id = match c.existing_id {
                Some(file_id) => {
                    tx.execute(
                        "UPDATE files SET size = ?1, mtime_ms = ?2, missing = 0 WHERE id = ?3",
                        params![c.size, c.mtime_ms, file_id],
                    )?;
                    report.updated += 1;
                    let photo: Option<i64> = tx
                        .query_row("SELECT id FROM photos WHERE master_file_id = ?1", [file_id], |r| r.get(0))
                        .optional()?;
                    match photo {
                        Some(p) => p,
                        None => continue, // a paired JPEG: no photo of its own
                    }
                }
                None => {
                    let folder_id = *c
                        .path
                        .parent()
                        .and_then(|p| folder_ids.get(p))
                        .context("file outside walked folders")?;
                    let name = c.path.file_name().unwrap_or_default().to_string_lossy();
                    let ext = c.path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
                    if is_pair_candidate(c)
                        && let Some(raw_photo) = raw_partner(&tx, folder_id, &c.path)?
                    {
                        tx.execute(
                            "INSERT INTO files (folder_id, name, ext, kind, size, mtime_ms, sidecar_of)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            params![folder_id, name, ext, c.kind.as_str(), c.size, c.mtime_ms, raw_photo],
                        )?;
                        report.paired += 1;
                        continue;
                    }
                    tx.execute(
                        "INSERT INTO files (folder_id, name, ext, kind, size, mtime_ms)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![folder_id, name, ext, c.kind.as_str(), c.size, c.mtime_ms],
                    )?;
                    tx.execute(
                        "INSERT INTO photos (master_file_id, imported_at) VALUES (?1, ?2)",
                        params![tx.last_insert_rowid(), now],
                    )?;
                    report.added += 1;
                    tx.last_insert_rowid()
                }
            };
            if let Some(m) = &c.metadata {
                write_metadata(&tx, photo_id, m)?;
            }
            if let Some(sidecar) = &c.sidecar {
                let file_turns = c.metadata.as_ref().and_then(|m| m.orientation).unwrap_or(0);
                apply_xmp(&tx, photo_id, sidecar, file_turns)?;
            }
        }
        for id in &restored {
            tx.execute("UPDATE files SET missing = 0 WHERE id = ?1", [id])?;
        }
        report.restored = restored.len();
        for k in known.into_values().filter(|k| !k.missing) {
            tx.execute("UPDATE files SET missing = 1 WHERE id = ?1", [k.id])?;
            report.missing += 1;
        }

        tx.commit()?;
        log::info!(
            "synchronized {}: {report:?} (walk {walk_time:.2?}, metadata {meta_time:.2?}, write {:.2?})",
            root.display(),
            t_write.elapsed()
        );
        Ok(report)
    }
}

/// Files already in the catalog at or below `root`, keyed by full path.
fn load_known(conn: &Connection, root: &Path) -> Result<HashMap<PathBuf, Known>> {
    let root_s = path_str(root);
    let sep = std::path::MAIN_SEPARATOR;
    let prefix = if root_s.ends_with(sep) { root_s.clone() } else { format!("{root_s}{sep}") };

    let mut stmt = conn.prepare(
        "SELECT f.id, fo.path, f.name, f.size, f.mtime_ms, f.missing
           FROM files f JOIN folders fo ON fo.id = f.folder_id
          WHERE fo.path = ?1 OR substr(fo.path, 1, length(?2)) = ?2",
    )?;
    let rows = stmt.query_map(params![root_s, prefix], |r| {
        let folder: String = r.get(1)?;
        let name: String = r.get(2)?;
        Ok((
            Path::new(&folder).join(name),
            Known { id: r.get(0)?, size: r.get(3)?, mtime_ms: r.get(4)?, missing: r.get(5)? },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn folder_id(tx: &Transaction, path: &Path) -> Result<Option<i64>> {
    Ok(tx.query_row("SELECT id FROM folders WHERE path = ?1", [path_str(path)], |r| r.get(0)).optional()?)
}

fn ensure_folder(tx: &Transaction, path: &Path, parent_id: Option<i64>) -> Result<i64> {
    if let Some(id) = folder_id(tx, path)? {
        if parent_id.is_some() {
            tx.execute("UPDATE folders SET parent_id = ?1 WHERE id = ?2", params![parent_id, id])?;
        }
        return Ok(id);
    }
    tx.execute("INSERT INTO folders (path, parent_id) VALUES (?1, ?2)", params![path_str(path), parent_id])?;
    Ok(tx.last_insert_rowid())
}

fn is_jpeg(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"))
}

/// The raw photo in `folder_id` with the same base name as `jpeg`, if any.
fn raw_partner(tx: &Transaction, folder_id: i64, jpeg: &Path) -> Result<Option<i64>> {
    let stem = jpeg.file_stem().unwrap_or_default().to_string_lossy().to_lowercase();
    let mut stmt = tx.prepare_cached(
        "SELECT p.id, f.name FROM files f JOIN photos p ON p.master_file_id = f.id
          WHERE f.folder_id = ?1 AND f.kind = 'raw'",
    )?;
    let rows = stmt.query_map([folder_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (id, name) = row?;
        if Path::new(&name).file_stem().is_some_and(|s| s.to_string_lossy().to_lowercase() == stem) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn read_sidecar(path: &Path, kind: FileKind) -> Option<xmp::XmpData> {
    match xmp::read_sidecar(path, kind == FileKind::Raw) {
        Ok(found) => found.map(|s| s.data),
        Err(e) => {
            log::warn!("ignoring unreadable sidecar of {}: {e:#}", path.display());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Flag, PhotoMetadata};
    use std::fs;

    fn setup() -> (tempfile::TempDir, PathBuf, Catalog) {
        let tmp = tempfile::tempdir().unwrap();
        let photos = tmp.path().join("Photos");
        fs::create_dir_all(photos.join("2026/Trip")).unwrap();
        fs::write(photos.join("a.NEF"), b"raw").unwrap();
        fs::write(photos.join("2026/b.jpg"), b"jpeg").unwrap();
        fs::write(photos.join("2026/Trip/c.tif"), b"tiff").unwrap();
        fs::write(photos.join("2026/Trip/notes.txt"), b"x").unwrap();
        fs::write(photos.join("2026/Trip/._c.tif"), b"appledouble").unwrap();
        fs::create_dir_all(photos.join(".cache")).unwrap();
        fs::write(photos.join(".cache/hidden.jpg"), b"x").unwrap();
        let cat = Catalog::create(tmp.path().join("Cat.plcat")).unwrap();
        (tmp, photos, cat)
    }

    #[test]
    fn adds_photos_and_folder_tree() {
        let (_tmp, photos, mut cat) = setup();
        let r = cat.sync_folder(&photos).unwrap();
        assert_eq!(r, SyncReport { added: 3, ..Default::default() });

        let folders = cat.folders().unwrap();
        assert_eq!(folders.len(), 3, "{folders:?}");
        let root = &folders[0];
        assert_eq!(root.parent_id, None);
        assert_eq!(root.photo_count, 1);
        assert!(folders[1..].iter().all(|f| f.parent_id.is_some()));

        let all = cat.photos_in_folder(root.id, true).unwrap();
        let names: Vec<_> = all.iter().map(|p| p.file_name.as_str()).collect();
        assert_eq!(names, ["a.NEF", "b.jpg", "c.tif"]);
        assert_eq!(all[0].kind, crate::FileKind::Raw);
        assert_eq!(cat.photos_in_folder(root.id, false).unwrap().len(), 1);
    }

    #[test]
    fn resync_is_a_no_op_then_detects_changes() {
        let (_tmp, photos, mut cat) = setup();
        cat.sync_folder(&photos).unwrap();
        assert_eq!(cat.sync_folder(&photos).unwrap(), SyncReport::default());

        fs::write(photos.join("2026/b.jpg"), b"a longer jpeg").unwrap();
        fs::write(photos.join("new.ARW"), b"raw").unwrap();
        let r = cat.sync_folder(&photos).unwrap();
        assert_eq!(r, SyncReport { added: 1, updated: 1, ..Default::default() });
    }

    #[test]
    fn missing_files_keep_their_metadata_and_can_be_restored() {
        let (_tmp, photos, mut cat) = setup();
        cat.sync_folder(&photos).unwrap();
        let root = cat.folders().unwrap()[0].id;
        let a = cat.photos_in_folder(root, false).unwrap()[0].clone();
        cat.set_rating(a.id, 4).unwrap();
        cat.set_flag(a.id, Flag::Picked).unwrap();

        fs::remove_file(&a.path).unwrap();
        assert_eq!(cat.sync_folder(&photos).unwrap(), SyncReport { missing: 1, ..Default::default() });
        let gone = &cat.photos_in_folder(root, false).unwrap()[0];
        assert!(gone.missing);
        assert_eq!((gone.rating, gone.flag), (4, Flag::Picked));

        fs::write(&a.path, b"raw").unwrap();
        let r = cat.sync_folder(&photos).unwrap();
        assert_eq!(r.restored + r.updated, 1, "{r:?}");
        let back = &cat.photos_in_folder(root, false).unwrap()[0];
        assert!(!back.missing);
        assert_eq!((back.id, back.rating), (a.id, 4));
    }

    #[test]
    fn reads_metadata_during_sync_so_capture_order_is_immediate() {
        let (_tmp, photos, mut cat) = setup();
        // Pretend capture times are the reverse of file-name order.
        let reader = |p: &Path, _kind: FileKind| {
            let t = match p.file_name()?.to_str()? {
                "a.NEF" => 300,
                "b.jpg" => 200,
                "c.tif" => 100,
                _ => return None,
            };
            Some(PhotoMetadata { capture_time: Some(t), model: Some("Test".into()), ..Default::default() })
        };
        cat.sync_folder_with(&photos, Some(&reader)).unwrap();
        let names: Vec<_> =
            cat.photos(crate::Source::AllPhotographs).unwrap().into_iter().map(|p| p.file_name).collect();
        assert_eq!(names, ["c.tif", "b.jpg", "a.NEF"]);

        // Changed files get their metadata re-read.
        fs::write(photos.join("a.NEF"), b"edited raw").unwrap();
        let reader2 =
            |_: &Path, _: FileKind| Some(PhotoMetadata { capture_time: Some(50), ..Default::default() });
        let r = cat.sync_folder_with(&photos, Some(&reader2)).unwrap();
        assert_eq!(r.updated, 1);
        let first = &cat.photos(crate::Source::AllPhotographs).unwrap()[0];
        assert_eq!((first.file_name.as_str(), first.capture_time), ("a.NEF", Some(50)));
    }

    #[test]
    fn syncing_a_subfolder_links_it_under_existing_parent() {
        let (_tmp, photos, mut cat) = setup();
        cat.sync_folder(&photos).unwrap();
        let r = cat.sync_folder(photos.join("2026")).unwrap();
        assert_eq!(r, SyncReport::default());
        assert_eq!(cat.folders().unwrap().iter().filter(|f| f.parent_id.is_none()).count(), 1);
    }
}
