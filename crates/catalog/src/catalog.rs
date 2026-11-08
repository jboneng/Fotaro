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
    pub missing: bool,
    pub rating: u8,
    pub flag: Flag,
    pub color_label: Option<String>,
    pub capture_time: Option<i64>,
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
        let mut conn =
            Connection::open(db).with_context(|| format!("opening {}", db.display()))?;
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
        let mut stmt = self.conn.prepare(
            "WITH RECURSIVE tree(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT fo.id FROM folders fo JOIN tree ON fo.parent_id = tree.id WHERE ?2
             )
             SELECT p.id, fo.path, f.name, f.kind, f.size, f.missing,
                    p.rating, p.flag, p.color_label, p.capture_time
               FROM photos p
               JOIN files f    ON f.id = p.master_file_id
               JOIN folders fo ON fo.id = f.folder_id
              WHERE f.folder_id IN tree
              ORDER BY fo.path, f.name",
        )?;
        let rows = stmt.query_map(params![folder_id, include_subfolders], |r| {
            let folder: String = r.get(1)?;
            let name: String = r.get(2)?;
            let kind: String = r.get(3)?;
            Ok(PhotoRow {
                id: r.get(0)?,
                path: Path::new(&folder).join(&name),
                file_name: name,
                kind: FileKind::parse(&kind).unwrap_or(FileKind::Image),
                size_bytes: r.get::<_, i64>(4)?.max(0) as u64,
                missing: r.get(5)?,
                rating: r.get::<_, i64>(6)?.clamp(0, 5) as u8,
                flag: Flag::from_db(r.get(7)?),
                color_label: r.get(8)?,
                capture_time: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn photo(&self, photo_id: i64) -> Result<Option<(i64, u8, Flag)>> {
        Ok(self
            .conn
            .query_row("SELECT id, rating, flag FROM photos WHERE id = ?1", [photo_id], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1)? as u8, Flag::from_db(r.get(2)?)))
            })
            .optional()?)
    }

    pub fn set_rating(&self, photo_id: i64, rating: u8) -> Result<()> {
        anyhow::ensure!(rating <= 5, "rating must be 0-5, got {rating}");
        self.conn.execute("UPDATE photos SET rating = ?1 WHERE id = ?2", params![rating, photo_id])?;
        Ok(())
    }

    pub fn set_flag(&self, photo_id: i64, flag: Flag) -> Result<()> {
        self.conn.execute("UPDATE photos SET flag = ?1 WHERE id = ?2", params![flag as i64, photo_id])?;
        Ok(())
    }
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
    fn rejects_catalog_from_newer_app() {
        let tmp = tempfile::tempdir().unwrap();
        let cat = Catalog::create(tmp.path()).unwrap();
        cat.conn.pragma_update(None, "user_version", schema::LATEST_VERSION + 1).unwrap();
        drop(cat);
        let err = Catalog::open(tmp.path()).err().unwrap().to_string();
        assert!(err.contains("newer"), "{err}");
    }
}
