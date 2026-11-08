//! The catalog: a directory (`MyLibrary.plcat/`) holding the SQLite database
//! and the preview cache, mirroring Lightroom Classic's catalog + Previews.lrdata.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::scan::FileKind;
use crate::schema;

pub const CATALOG_EXTENSION: &str = "plcat";
const DB_FILE: &str = "catalog.db";
const PREVIEWS_DIR: &str = "previews";

pub struct Catalog {
    pub(crate) conn: Connection,
    dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: i64,
    pub path: PathBuf,
    pub parent_id: Option<i64>,
    /// Photos directly in this folder (not subfolders), including missing ones.
    pub photo_count: i64,
}

/// Pick/reject flag, as in LrC (P / X / U).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Flag {
    Rejected = -1,
    #[default]
    Unflagged = 0,
    Picked = 1,
}

impl Flag {
    fn from_db(v: i64) -> Self {
        match v {
            -1 => Flag::Rejected,
            1 => Flag::Picked,
            _ => Flag::Unflagged,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PhotoRow {
    pub id: i64,
    pub path: PathBuf,
    pub file_name: String,
    pub kind: FileKind,
    pub size_bytes: u64,
    pub mtime_ms: i64,
    pub missing: bool,
    pub rating: u8,
    pub flag: Flag,
    pub color_label: Option<String>,
    pub capture_time: Option<i64>,
    /// Extra rotation applied in the catalog (LrC "Rotate Left/Right"), in clockwise
    /// quarter turns 0..=3, on top of the file's own orientation. Files are never changed.
    pub rotation: u8,
    /// Changed in the catalog since it was last written to the XMP sidecar.
    pub metadata_dirty: bool,
}

/// Camera/exposure metadata stored per photo. `capture_time` is "naive local"
/// seconds (camera wall-clock time encoded as if UTC).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoMetadata {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<f64>,
    pub shutter: Option<f64>,
    pub aperture: Option<f64>,
    pub focal_len: Option<f64>,
    pub capture_time: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The file's own orientation in clockwise quarter turns (EXIF / RAW flip), if known.
    pub orientation: Option<u8>,
}

/// Where the grid's photos come from (the LrC "source").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    AllPhotographs,
    PreviousImport,
    Folder { id: i64, include_subfolders: bool },
}

/// Grid sort criteria, following LrC's toolbar "Sort" menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SortKey {
    #[default]
    CaptureTime,
    AddedOrder,
    FileName,
    Extension,
    Rating,
    Pick,
    LabelColor,
    AspectRatio,
}

impl SortKey {
    pub const ALL: [SortKey; 8] = [
        SortKey::CaptureTime,
        SortKey::AddedOrder,
        SortKey::Pick,
        SortKey::LabelColor,
        SortKey::Rating,
        SortKey::FileName,
        SortKey::Extension,
        SortKey::AspectRatio,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SortKey::CaptureTime => "Capture Time",
            SortKey::AddedOrder => "Added Order",
            SortKey::FileName => "File Name",
            SortKey::Extension => "Extension",
            SortKey::Rating => "Rating",
            SortKey::Pick => "Pick",
            SortKey::LabelColor => "Label Color",
            SortKey::AspectRatio => "Aspect Ratio",
        }
    }

    /// SQL ORDER BY terms (ascending), each ending with stable tie-breakers.
    /// Higher ratings and picks come first, as in LrC.
    fn order_by(self) -> &'static str {
        match self {
            SortKey::CaptureTime => "p.capture_time IS NULL, p.capture_time, fo.path, f.name",
            SortKey::AddedOrder => "p.id",
            SortKey::FileName => "f.name COLLATE NOCASE, fo.path",
            SortKey::Extension => "f.ext, f.name COLLATE NOCASE, fo.path",
            SortKey::Rating => "p.rating DESC, p.capture_time IS NULL, p.capture_time, fo.path, f.name",
            SortKey::Pick => "p.flag DESC, p.capture_time IS NULL, p.capture_time, fo.path, f.name",
            SortKey::LabelColor => {
                "CASE p.color_label WHEN 'red' THEN 1 WHEN 'yellow' THEN 2 WHEN 'green' THEN 3
                     WHEN 'blue' THEN 4 WHEN 'purple' THEN 5 ELSE 6 END,
                 p.capture_time IS NULL, p.capture_time, fo.path, f.name"
            }
            SortKey::AspectRatio => {
                "e.width IS NULL, CAST(e.width AS REAL) / MAX(e.height, 1),
                 p.capture_time IS NULL, p.capture_time, fo.path, f.name"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct SortOrder {
    pub key: SortKey,
    /// Reverse the whole order (LrC's A–Z / Z–A toggle).
    pub descending: bool,
}

const PHOTO_COLUMNS: &str = "p.id, fo.path, f.name, f.kind, f.size, f.mtime_ms, f.missing,
     p.rating, p.flag, p.color_label, p.capture_time, p.orientation, p.metadata_dirty";

/// `photos.orientation` holds the catalog rotation as an EXIF/XMP orientation code
/// (1 = none, 6 = 90° CW, 3 = 180°, 8 = 90° CCW), the form XMP sidecars will use.
fn rotation_from_orientation(code: i64) -> u8 {
    match code {
        6 => 1,
        3 => 2,
        8 => 3,
        _ => 0,
    }
}

fn orientation_from_rotation(quarter_turns: u8) -> i64 {
    [1, 6, 3, 8][usize::from(quarter_turns % 4)]
}

fn photo_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PhotoRow> {
    let folder: String = r.get(1)?;
    let name: String = r.get(2)?;
    let kind: String = r.get(3)?;
    Ok(PhotoRow {
        id: r.get(0)?,
        path: Path::new(&folder).join(&name),
        file_name: name,
        kind: FileKind::parse(&kind).unwrap_or(FileKind::Image),
        size_bytes: r.get::<_, i64>(4)?.max(0) as u64,
        mtime_ms: r.get(5)?,
        missing: r.get(6)?,
        rating: r.get::<_, i64>(7)?.clamp(0, 5) as u8,
        flag: Flag::from_db(r.get(8)?),
        color_label: r.get(9)?,
        capture_time: r.get(10)?,
        rotation: rotation_from_orientation(r.get(11)?),
        metadata_dirty: r.get(12)?,
    })
}

impl Catalog {
    /// Create a new, empty catalog in `dir` (created if needed).
    pub fn create(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let db = dir.join(DB_FILE);
        anyhow::ensure!(!db.exists(), "a catalog already exists at {}", dir.display());
        std::fs::create_dir_all(dir.join(PREVIEWS_DIR))
            .with_context(|| format!("creating catalog directory {}", dir.display()))?;
        Self::open_db(dir, &db)
    }

    /// Open an existing catalog, upgrading its schema if needed.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let db = dir.join(DB_FILE);
        anyhow::ensure!(db.is_file(), "no catalog found at {}", dir.display());
        std::fs::create_dir_all(dir.join(PREVIEWS_DIR))?;
        Self::open_db(dir, &db)
    }

    fn open_db(dir: &Path, db: &Path) -> Result<Self> {
        let mut conn = Connection::open(db).with_context(|| format!("opening {}", db.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::migrate(&mut conn)?;
        Ok(Catalog { conn, dir: dir.to_path_buf() })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn previews_dir(&self) -> PathBuf {
        self.dir.join(PREVIEWS_DIR)
    }

    pub fn schema_version(&self) -> Result<u32> {
        schema::version(&self.conn)
    }

    /// All folders, sorted by path.
    pub fn folders(&self) -> Result<Vec<Folder>> {
        let mut stmt = self.conn.prepare(
            "SELECT fo.id, fo.path, fo.parent_id,
                    (SELECT COUNT(*) FROM files f JOIN photos p ON p.master_file_id = f.id
                      WHERE f.folder_id = fo.id)
               FROM folders fo ORDER BY fo.path",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Folder {
                id: r.get(0)?,
                path: PathBuf::from(r.get::<_, String>(1)?),
                parent_id: r.get(2)?,
                photo_count: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn photo_count(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM photos", [], |r| r.get(0))?)
    }

    /// Photos in a folder, optionally including all subfolders, sorted by path.
    pub fn photos_in_folder(&self, folder_id: i64, include_subfolders: bool) -> Result<Vec<PhotoRow>> {
        self.photos(Source::Folder { id: folder_id, include_subfolders })
    }

    /// Photos for a source, sorted by capture time (then path), like LrC's default.
    pub fn photos(&self, source: Source) -> Result<Vec<PhotoRow>> {
        self.photos_sorted(source, SortOrder::default())
    }

    /// Photos for a source in the given sort order. Ties always fall back to
    /// capture time, then path, so the order is stable.
    pub fn photos_sorted(&self, source: Source, sort: SortOrder) -> Result<Vec<PhotoRow>> {
        let order = format!("ORDER BY {}", sort.key.order_by());
        let order = order.as_str();
        let from = "FROM photos p
                    JOIN files f    ON f.id = p.master_file_id
                    JOIN folders fo ON fo.id = f.folder_id
                    LEFT JOIN exif e ON e.photo_id = p.id";
        let (sql, args): (String, Vec<i64>) = match source {
            Source::AllPhotographs => (format!("SELECT {PHOTO_COLUMNS} {from} {order}"), vec![]),
            Source::PreviousImport => (
                format!(
                    "SELECT {PHOTO_COLUMNS} {from}
                      WHERE p.imported_at = (SELECT MAX(imported_at) FROM photos) {order}"
                ),
                vec![],
            ),
            Source::Folder { id, include_subfolders } => (
                format!(
                    "WITH RECURSIVE tree(id) AS (
                         SELECT ?1
                         UNION ALL
                         SELECT fo.id FROM folders fo JOIN tree ON fo.parent_id = tree.id WHERE ?2
                     )
                     SELECT {PHOTO_COLUMNS} {from} WHERE f.folder_id IN tree {order}"
                ),
                vec![id, include_subfolders.into()],
            ),
        };
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args), photo_row)?;
        let mut photos: Vec<PhotoRow> = rows.collect::<rusqlite::Result<_>>()?;
        if sort.descending {
            photos.reverse();
        }
        Ok(photos)
    }

    pub fn photo(&self, photo_id: i64) -> Result<Option<PhotoRow>> {
        let sql = format!(
            "SELECT {PHOTO_COLUMNS} FROM photos p
               JOIN files f    ON f.id = p.master_file_id
               JOIN folders fo ON fo.id = f.folder_id
              WHERE p.id = ?1"
        );
        Ok(self.conn.query_row(&sql, [photo_id], photo_row).optional()?)
    }

    /// (photo id, file size, file mtime in ms) for every photo: what preview fingerprints
    /// are computed from, without touching the disk.
    pub fn photo_versions(&self) -> Result<Vec<(i64, u64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, f.size, f.mtime_ms FROM photos p JOIN files f ON f.id = p.master_file_id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn folder_by_path(&self, path: &Path) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT id FROM folders WHERE path = ?1", [path.to_string_lossy()], |r| r.get(0))
            .optional()?)
    }

    pub fn metadata(&self, photo_id: i64) -> Result<Option<PhotoMetadata>> {
        Ok(self
            .conn
            .query_row(
                "SELECT e.camera_make, e.camera_model, e.lens, e.iso, e.shutter, e.aperture,
                        e.focal_len, p.capture_time, e.width, e.height, e.orientation
                   FROM exif e JOIN photos p ON p.id = e.photo_id WHERE e.photo_id = ?1",
                [photo_id],
                |r| {
                    Ok(PhotoMetadata {
                        make: r.get(0)?,
                        model: r.get(1)?,
                        lens: r.get(2)?,
                        iso: r.get(3)?,
                        shutter: r.get(4)?,
                        aperture: r.get(5)?,
                        focal_len: r.get(6)?,
                        capture_time: r.get(7)?,
                        width: r.get(8)?,
                        height: r.get(9)?,
                        orientation: r.get(10)?,
                    })
                },
            )
            .optional()?)
    }

    /// Store metadata read from the file; also sets the photo's capture time.
    pub fn set_metadata(&self, photo_id: i64, m: &PhotoMetadata) -> Result<()> {
        write_metadata(&self.conn, photo_id, m)
    }
    pub fn set_rating(&self, photo_id: i64, rating: u8) -> Result<()> {
        anyhow::ensure!(rating <= 5, "rating must be 0-5, got {rating}");
        self.conn.execute(
            "UPDATE photos SET rating = ?1, metadata_dirty = 1 WHERE id = ?2",
            params![rating, photo_id],
        )?;
        Ok(())
    }

    pub fn set_flag(&self, photo_id: i64, flag: Flag) -> Result<()> {
        self.conn.execute(
            "UPDATE photos SET flag = ?1, metadata_dirty = 1 WHERE id = ?2",
            params![flag as i64, photo_id],
        )?;
        Ok(())
    }

    /// Set the catalog rotation in clockwise quarter turns (taken modulo 4).
    pub fn set_rotation(&self, photo_id: i64, quarter_turns: u8) -> Result<()> {
        self.conn.execute(
            "UPDATE photos SET orientation = ?1, metadata_dirty = 1 WHERE id = ?2",
            params![orientation_from_rotation(quarter_turns), photo_id],
        )?;
        Ok(())
    }

    /// Colour label name (`"red"`, `"yellow"`, `"green"`, `"blue"`, `"purple"`) or `None`.
    pub fn set_color_label(&self, photo_id: i64, label: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE photos SET color_label = ?1, metadata_dirty = 1 WHERE id = ?2",
            params![label, photo_id],
        )?;
        Ok(())
    }

    /// Photo counts for the Catalog panel: (all photographs, previous import).
    pub fn source_counts(&self) -> Result<(i64, i64)> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*),
                    COUNT(*) FILTER (WHERE imported_at = (SELECT MAX(imported_at) FROM photos))
               FROM photos",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }
}

/// Store metadata for a photo (also sets its capture time). Shared by
/// [`Catalog::set_metadata`] and folder sync (inside its transaction).
pub(crate) fn write_metadata(conn: &Connection, photo_id: i64, m: &PhotoMetadata) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO exif
             (photo_id, camera_make, camera_model, lens, iso, shutter, aperture,
              focal_len, width, height, orientation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            photo_id,
            m.make,
            m.model,
            m.lens,
            m.iso,
            m.shutter,
            m.aperture,
            m.focal_len,
            m.width,
            m.height,
            m.orientation
        ],
    )?;
    conn.execute("UPDATE photos SET capture_time = ?1 WHERE id = ?2", params![m.capture_time, photo_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_migrates_to_latest_and_reopens() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Test.plcat");
        let cat = Catalog::create(&dir).unwrap();
        assert_eq!(cat.schema_version().unwrap(), schema::LATEST_VERSION);
        assert!(cat.previews_dir().is_dir());
        drop(cat);

        let cat = Catalog::open(&dir).unwrap();
        assert_eq!(cat.schema_version().unwrap(), schema::LATEST_VERSION);
        assert_eq!(cat.photo_count().unwrap(), 0);
    }

    #[test]
    fn create_refuses_existing_and_open_refuses_missing() {
        let tmp = tempfile::tempdir().unwrap();
        Catalog::create(tmp.path()).unwrap();
        assert!(Catalog::create(tmp.path()).is_err());
        assert!(Catalog::open(tmp.path().join("nope")).is_err());
    }

    #[test]
    fn rotation_round_trips_as_orientation_codes() {
        let tmp = tempfile::tempdir().unwrap();
        let photos = tmp.path().join("p");
        std::fs::create_dir(&photos).unwrap();
        std::fs::write(photos.join("a.jpg"), b"x").unwrap();
        let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        cat.sync_folder(&photos).unwrap();
        let id = cat.photos(Source::AllPhotographs).unwrap()[0].id;
        assert_eq!(cat.photo(id).unwrap().unwrap().rotation, 0);
        for (turns, code) in [(1, 6), (2, 3), (3, 8), (4, 1), (7, 8)] {
            cat.set_rotation(id, turns).unwrap();
            let stored: i64 = cat
                .conn
                .query_row("SELECT orientation FROM photos WHERE id = ?1", [id], |r| r.get(0))
                .unwrap();
            assert_eq!(stored, code, "{turns} quarter turns");
            assert_eq!(cat.photo(id).unwrap().unwrap().rotation, turns % 4);
        }
    }

    #[test]
    fn metadata_round_trips_and_orders_by_capture_time() {
        let tmp = tempfile::tempdir().unwrap();
        let photos = tmp.path().join("p");
        std::fs::create_dir(&photos).unwrap();
        for n in ["a.jpg", "b.jpg", "c.jpg"] {
            std::fs::write(photos.join(n), b"x").unwrap();
        }
        let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        cat.sync_folder(&photos).unwrap();
        let all = cat.photos(Source::AllPhotographs).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(cat.photos(Source::PreviousImport).unwrap().len(), 3);

        // Give c the earliest capture time and b a later one; a has none (sorts last).
        let meta =
            |t| PhotoMetadata { make: Some("Nikon".into()), capture_time: Some(t), ..Default::default() };
        cat.set_metadata(all[2].id, &meta(100)).unwrap();
        cat.set_metadata(all[1].id, &meta(200)).unwrap();
        let names: Vec<_> =
            cat.photos(Source::AllPhotographs).unwrap().into_iter().map(|p| p.file_name).collect();
        assert_eq!(names, ["c.jpg", "b.jpg", "a.jpg"]);
        assert_eq!(cat.metadata(all[2].id).unwrap().unwrap(), meta(100));
        assert_eq!(cat.metadata(all[0].id).unwrap(), None);
        assert_eq!(cat.photo(all[0].id).unwrap().unwrap().file_name, "a.jpg");
        assert!(cat.folder_by_path(&dunce::canonicalize(&photos).unwrap()).unwrap().is_some());
    }

    #[test]
    fn sort_orders() {
        let tmp = tempfile::tempdir().unwrap();
        let photos = tmp.path().join("p");
        std::fs::create_dir(&photos).unwrap();
        for n in ["b.jpg", "A.png", "c.NEF"] {
            std::fs::write(photos.join(n), b"x").unwrap();
        }
        let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        cat.sync_folder(&photos).unwrap();
        let id = |name: &str| {
            cat.photos(Source::AllPhotographs).unwrap().into_iter().find(|p| p.file_name == name).unwrap().id
        };
        let (a, b, c) = (id("A.png"), id("b.jpg"), id("c.NEF"));
        cat.set_rating(b, 5).unwrap();
        cat.set_rating(c, 3).unwrap();
        cat.set_flag(c, Flag::Picked).unwrap();
        cat.set_color_label(a, Some("green")).unwrap();
        cat.set_color_label(c, Some("red")).unwrap();

        let names = |key, descending| -> Vec<String> {
            cat.photos_sorted(Source::AllPhotographs, SortOrder { key, descending })
                .unwrap()
                .into_iter()
                .map(|p| p.file_name)
                .collect()
        };
        assert_eq!(names(SortKey::FileName, false), ["A.png", "b.jpg", "c.NEF"]);
        assert_eq!(names(SortKey::FileName, true), ["c.NEF", "b.jpg", "A.png"]);
        assert_eq!(names(SortKey::Extension, false), ["b.jpg", "c.NEF", "A.png"]);
        assert_eq!(names(SortKey::Rating, false), ["b.jpg", "c.NEF", "A.png"]);
        assert_eq!(names(SortKey::Pick, false)[0], "c.NEF");
        assert_eq!(names(SortKey::LabelColor, false), ["c.NEF", "A.png", "b.jpg"]);
        assert_eq!(names(SortKey::AspectRatio, false).len(), 3);
    }

    #[test]
    fn rejects_catalog_from_newer_app() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = Catalog::create(tmp.path()).unwrap();
        cat.conn.pragma_update(None, "user_version", schema::LATEST_VERSION + 1).unwrap();
        drop(cat);
        let err = Catalog::open(tmp.path()).err().unwrap().to_string();
        assert!(err.contains("newer"), "{err}");
    }
}
