//! The catalog: a directory (`MyLibrary.plcat/`) holding the SQLite database
//! and the preview cache, mirroring Lightroom Classic's catalog + Previews.lrdata.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params};

use crate::pictures::{Role, UnitInfo};
use crate::query::{Criterion, Facet, MetaColumn, SmartRules};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
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
    /// Its picture, group and role, and the size of its unit (see `pictures.rs`).
    pub unit: UnitInfo,
    pub in_quick_collection: bool,
    /// A virtual copy's name ("Copy 1"); `None` for master photos.
    pub virtual_copy: Option<String>,
    /// The master photo (itself, unless this is a virtual copy): copies share its previews.
    pub master_id: i64,
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
    /// GPS position: decimal degrees (south and west negative), metres above sea level.
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
    pub gps_alt: Option<f64>,
}

/// Latitude, longitude (decimal degrees, south and west negative) and altitude (metres).
pub type GpsPosition = (f64, f64, Option<f64>);

/// Where the grid's photos come from (the LrC "source").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    AllPhotographs,
    PreviousImport,
    Folder {
        id: i64,
        include_subfolders: bool,
    },
    /// A collection, smart collection, or collection set (photos of all collections in it).
    Collection {
        id: i64,
    },
    QuickCollection,
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

    /// Compare two grid rows (ascending), each order ending with stable tie-breakers:
    /// capture time (of the picture's original), folder, name, id. Higher ratings and
    /// picks come first, as in LrC. Sorting happens here rather than in SQL: with every
    /// column carried through SQLite's sorter, a 10,000-photo grid sorted several times
    /// slower.
    fn compare(self, a: &SortRow, b: &SortRow) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let nocase = |x: &str, y: &str| {
            x.bytes().map(|c| c.to_ascii_lowercase()).cmp(y.bytes().map(|c| c.to_ascii_lowercase()))
        };
        // Unknown capture times last, then by time, folder, name and id.
        let tail = |a: &SortRow, b: &SortRow| -> Ordering {
            a.time
                .is_none()
                .cmp(&b.time.is_none())
                .then(a.time.cmp(&b.time))
                .then_with(|| a.folder.cmp(&b.folder))
                .then_with(|| a.row.file_name.cmp(&b.row.file_name))
                .then(a.row.id.cmp(&b.row.id))
        };
        let label_rank = |r: &SortRow| match r.row.color_label.as_deref() {
            Some("red") => 1,
            Some("yellow") => 2,
            Some("green") => 3,
            Some("blue") => 4,
            Some("purple") => 5,
            _ => 6,
        };
        match self {
            SortKey::CaptureTime => tail(a, b),
            SortKey::AddedOrder => a.row.id.cmp(&b.row.id),
            SortKey::FileName => nocase(&a.row.file_name, &b.row.file_name)
                .then_with(|| a.folder.cmp(&b.folder))
                .then(a.row.id.cmp(&b.row.id)),
            SortKey::Extension => a
                .ext
                .cmp(&b.ext)
                .then_with(|| nocase(&a.row.file_name, &b.row.file_name))
                .then_with(|| a.folder.cmp(&b.folder))
                .then(a.row.id.cmp(&b.row.id)),
            SortKey::Rating => b.row.rating.cmp(&a.row.rating).then_with(|| tail(a, b)),
            SortKey::Pick => (b.row.flag as i8).cmp(&(a.row.flag as i8)).then_with(|| tail(a, b)),
            SortKey::LabelColor => label_rank(a).cmp(&label_rank(b)).then_with(|| tail(a, b)),
            SortKey::AspectRatio => a
                .aspect
                .is_none()
                .cmp(&b.aspect.is_none())
                .then(a.aspect.partial_cmp(&b.aspect).unwrap_or(Ordering::Equal))
                .then_with(|| tail(a, b)),
        }
    }
}

/// A grid row with the values it sorts by.
struct SortRow {
    row: PhotoRow,
    /// Capture time of the picture's original (or the photo's own).
    time: Option<i64>,
    folder: String,
    ext: String,
    aspect: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct SortOrder {
    pub key: SortKey,
    /// Reverse the whole order (LrC's A–Z / Z–A toggle).
    pub descending: bool,
}

pub(crate) const PHOTOS_FROM: &str = "FROM photos p
    JOIN files f     ON f.id = p.master_file_id
    JOIN folders fo  ON fo.id = f.folder_id
    JOIN pictures pi ON pi.id = p.picture_id
    LEFT JOIN exif e ON e.photo_id = p.id";

pub(crate) const PHOTO_COLUMNS: &str = "p.id, fo.path, f.name, f.kind, f.size, f.mtime_ms, f.missing,
     p.rating, p.flag, p.color_label, p.capture_time, p.orientation, p.metadata_dirty,
     p.picture_id, p.role, pi.group_id,
     CASE WHEN pi.group_id IS NULL
          THEN (SELECT COUNT(*) FROM photos m WHERE m.picture_id = p.picture_id)
          ELSE (SELECT COUNT(*) FROM photos m JOIN pictures g ON g.id = m.picture_id
                 WHERE g.group_id = pi.group_id) END,
     CASE WHEN pi.group_id IS NULL THEN 1
          ELSE (SELECT COUNT(*) FROM pictures g WHERE g.group_id = pi.group_id) END,
     EXISTS (SELECT 1 FROM quick_collection q WHERE q.photo_id = p.id),
     CASE WHEN p.virtual_of IS NULL THEN NULL ELSE coalesce(p.copy_name, 'Copy') END,
     coalesce(p.virtual_of, p.id)";

/// How many columns [`PHOTO_COLUMNS`] has (queries add their own after them).
pub(crate) const PHOTO_COLUMN_COUNT: usize = 21;

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

pub(crate) fn photo_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PhotoRow> {
    let folder: String = r.get(1)?;
    let name: String = r.get(2)?;
    let kind: String = r.get(3)?;
    let count = |i: usize| -> rusqlite::Result<u32> { Ok(r.get::<_, i64>(i)?.max(1) as u32) };
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
        unit: UnitInfo {
            picture_id: r.get(13)?,
            role: Role::parse(&r.get::<_, String>(14)?),
            group_id: r.get(15)?,
            members: count(16)?,
            pictures: count(17)?,
            hidden_match: false,
        },
        in_quick_collection: r.get(18)?,
        virtual_copy: r.get(19)?,
        master_id: r.get(20)?,
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
        conn.create_scalar_function(
            crate::query::FOLD_FN,
            1,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8
                | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
            |ctx| Ok(ctx.get::<Option<String>>(0)?.map(|s| s.to_lowercase())),
        )?;
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
        self.photos_filtered(source, sort, &Criterion::AllOf(vec![]))
    }

    /// SQL condition selecting a source's photos (parameters appended to `args`).
    pub(crate) fn source_condition(&self, source: Source, args: &mut Vec<Value>) -> Result<String> {
        let mut bind = |v: Value| {
            args.push(v);
            format!("?{}", args.len())
        };
        Ok(match source {
            Source::AllPhotographs => "1".into(),
            Source::PreviousImport => "p.imported_at = (SELECT MAX(imported_at) FROM photos)".into(),
            Source::Folder { id, include_subfolders } => {
                let (root, sub) = (bind(id.into()), bind(i64::from(include_subfolders).into()));
                format!(
                    "f.folder_id IN (WITH RECURSIVE tree(id) AS (
                         SELECT {root} UNION ALL
                         SELECT x.id FROM folders x JOIN tree ON x.parent_id = tree.id WHERE {sub}
                     ) SELECT id FROM tree)"
                )
            }
            Source::QuickCollection => "p.id IN (SELECT photo_id FROM quick_collection)".into(),
            Source::Collection { id } => {
                let kind: Option<(String, Option<String>)> = self
                    .conn
                    .query_row("SELECT kind, smart_rules FROM collections WHERE id = ?1", [id], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?;
                match kind {
                    None => "0".into(),
                    Some((kind, rules)) if kind == "smart" => {
                        let rules: SmartRules = match rules.as_deref().map(serde_json::from_str).transpose() {
                            Ok(r) => r.unwrap_or_default(),
                            Err(e) => {
                                log::warn!("smart collection {id} has unreadable rules: {e}");
                                return Ok("0".into());
                            }
                        };
                        rules.criterion().to_sql(args)
                    }
                    // A collection, or a set: the photos of every collection inside it.
                    Some(_) => {
                        let root = bind(id.into());
                        format!(
                            "p.id IN (SELECT photo_id FROM collection_photos WHERE collection_id IN (
                                 WITH RECURSIVE ct(id) AS (
                                     SELECT {root} UNION ALL
                                     SELECT c.id FROM collections c JOIN ct ON c.parent_id = ct.id
                                 ) SELECT id FROM ct))"
                        )
                    }
                }
            }
        })
    }

    /// The grid: one row per unit (a group, or a picture in no group) of which any
    /// member is in `source` and matches `filter`, showing the unit's active photo, in
    /// sort order. Capture time sorts by the shown picture's original, so a film roll stays
    /// in frame order whatever its edits are dated.
    pub fn photos_filtered(
        &self,
        source: Source,
        sort: SortOrder,
        filter: &Criterion,
    ) -> Result<Vec<PhotoRow>> {
        use std::collections::{HashMap, HashSet};
        // 1. The matching photos, and what each one's unit shows. (Two simple queries
        //    and the unit logic here: SQLite planned the one-query version several
        //    times slower on a 10,000-photo catalog.)
        let mut args = Vec::new();
        let source = self.source_condition(source, &mut args)?;
        let filter = filter.to_sql(&mut args);
        let sql = format!(
            "SELECT p.id, pi.group_id, pi.active_photo_id {PHOTOS_FROM} WHERE ({source}) AND ({filter})"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let matched: Vec<(i64, Option<i64>, Option<i64>)> = stmt
            .query_map(rusqlite::params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut group_shows: HashMap<i64, i64> = HashMap::new();
        if matched.iter().any(|(_, g, _)| g.is_some()) {
            let mut stmt = self.conn.prepare_cached(
                "SELECT g.id, ap.active_photo_id FROM groups g JOIN pictures ap ON ap.id = g.active_picture_id",
            )?;
            for row in stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))? {
                let (g, p): (i64, Option<i64>) = row?;
                group_shows.extend(p.map(|p| (g, p)));
            }
        }
        let matched_ids: HashSet<i64> = matched.iter().map(|(id, _, _)| *id).collect();
        let shown: HashSet<i64> = matched
            .iter()
            .filter_map(|&(id, group, active)| match group {
                Some(g) => group_shows.get(&g).copied(),
                None => Some(active.unwrap_or(id)),
            })
            .collect();
        if shown.is_empty() {
            return Ok(Vec::new());
        }

        // 2. Those photos' rows, with what they sort by; sorted here.
        let cols = format!(
            "SELECT {PHOTO_COLUMNS}, coalesce(o.capture_time, p.capture_time), fo.path, f.ext,
                    CAST(e.width AS REAL) / MAX(e.height, 1)
               {PHOTOS_FROM}
               LEFT JOIN photos o ON o.picture_id = p.picture_id AND o.role = 'original'"
        );
        let n = PHOTO_COLUMN_COUNT;
        let read = |r: &rusqlite::Row<'_>| -> rusqlite::Result<SortRow> {
            let mut row = photo_row(r)?;
            row.unit.hidden_match = !matched_ids.contains(&row.id);
            Ok(SortRow {
                row,
                time: r.get(n)?,
                folder: r.get(n + 1)?,
                ext: r.get(n + 2)?,
                aspect: r.get(n + 3)?,
            })
        };
        let mut rows: Vec<SortRow> = Vec::with_capacity(shown.len());
        // Most of the catalog: read it all and keep the shown ones; a few: ask for them.
        let total = self.photo_count()?;
        if shown.len() as i64 * 4 > total {
            let mut stmt = self.conn.prepare_cached(&cols)?;
            for row in stmt.query_map([], read)? {
                let row = row?;
                if shown.contains(&row.row.id) {
                    rows.push(row);
                }
            }
        } else {
            let ids: Vec<i64> = shown.into_iter().collect();
            for chunk in ids.chunks(500) {
                let marks = vec!["?"; chunk.len()].join(",");
                let mut stmt = self.conn.prepare_cached(&format!("{cols} WHERE p.id IN ({marks})"))?;
                for row in stmt.query_map(rusqlite::params_from_iter(chunk), read)? {
                    rows.push(row?);
                }
            }
        }
        rows.sort_by(|a, b| sort.key.compare(a, b));
        let mut photos: Vec<PhotoRow> = rows.into_iter().map(|r| r.row).collect();
        if sort.descending {
            photos.reverse();
        }
        Ok(photos)
    }

    /// How many photos match a source and filter.
    pub fn count_filtered(&self, source: Source, filter: &Criterion) -> Result<i64> {
        let mut args = Vec::new();
        let source = self.source_condition(source, &mut args)?;
        let filter = filter.to_sql(&mut args);
        let sql = format!("SELECT COUNT(*) {PHOTOS_FROM} WHERE ({source}) AND ({filter})");
        Ok(self.conn.query_row(&sql, rusqlite::params_from_iter(args), |r| r.get(0))?)
    }

    /// The values of a Metadata filter column among a source's photos that match
    /// `filter`, with counts (LrC's column browser).
    pub fn facets(&self, source: Source, filter: &Criterion, column: MetaColumn) -> Result<Vec<Facet>> {
        let mut args = Vec::new();
        let source = self.source_condition(source, &mut args)?;
        let filter = filter.to_sql(&mut args);
        let filter = format!("WHERE ({source}) AND ({filter})");
        let params = rusqlite::params_from_iter(args);
        let text_facets = |expr: &str, order: &str| -> Result<Vec<(Option<String>, i64)>> {
            let sql =
                format!("SELECT {expr} AS v, COUNT(*) {PHOTOS_FROM} {filter} GROUP BY v ORDER BY {order}");
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params.clone(), |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        };
        let unknown_last = "v IS NULL OR v = '', v COLLATE NOCASE";
        Ok(match column {
            MetaColumn::Keyword => {
                let sql = format!(
                    "SELECT k.id, k.name, COUNT(DISTINCT p.id) {PHOTOS_FROM}
                       JOIN photo_keywords pk ON pk.photo_id = p.id JOIN keywords k ON k.id = pk.keyword_id
                     {filter} GROUP BY k.id ORDER BY k.name COLLATE NOCASE"
                );
                let mut stmt = self.conn.prepare(&sql)?;
                let rows = stmt.query_map(params, |r| {
                    Ok(Facet { criterion: Criterion::Keyword(r.get(0)?), label: r.get(1)?, count: r.get(2)? })
                })?;
                rows.collect::<rusqlite::Result<_>>()?
            }
            MetaColumn::Date => {
                text_facets("strftime('%Y', p.capture_time, 'unixepoch')", "v IS NULL, v DESC")?
                    .into_iter()
                    .map(|(year, count)| {
                        let year: Option<i32> = year.and_then(|y| y.parse().ok());
                        Facet {
                            label: year.map_or_else(|| "Unknown Date".into(), |y| y.to_string()),
                            count,
                            criterion: Criterion::Year(year),
                        }
                    })
                    .collect()
            }
            MetaColumn::Camera => text_facets(crate::query::CAMERA_SQL, unknown_last)?
                .into_iter()
                .map(|(c, count)| {
                    let c = c.unwrap_or_default();
                    let label = if c.is_empty() { "Unknown Camera".into() } else { c.clone() };
                    Facet { label, count, criterion: Criterion::Camera(c) }
                })
                .collect(),
            MetaColumn::Lens => text_facets("coalesce(e.lens, '')", unknown_last)?
                .into_iter()
                .map(|(l, count)| {
                    let l = l.unwrap_or_default();
                    let label = if l.is_empty() { "Unknown Lens".into() } else { l.clone() };
                    Facet { label, count, criterion: Criterion::Lens(l) }
                })
                .collect(),
            MetaColumn::FileType => text_facets("upper(f.ext)", "v")?
                .into_iter()
                .map(|(ext, count)| {
                    let ext = ext.unwrap_or_default();
                    Facet { label: ext.clone(), count, criterion: Criterion::FileType(ext) }
                })
                .collect(),
            MetaColumn::Label => text_facets(
                "p.color_label",
                "CASE v WHEN 'red' THEN 1 WHEN 'yellow' THEN 2 WHEN 'green' THEN 3
                        WHEN 'blue' THEN 4 WHEN 'purple' THEN 5 ELSE 6 END",
            )?
            .into_iter()
            .map(|(l, count)| {
                let label = match &l {
                    Some(l) => {
                        let mut s = l.clone();
                        if let Some(c) = s.get_mut(..1) {
                            c.make_ascii_uppercase();
                        }
                        s
                    }
                    None => "No Label".into(),
                };
                Facet { label, count, criterion: Criterion::Label(l) }
            })
            .collect(),
        })
    }

    pub fn photo(&self, photo_id: i64) -> Result<Option<PhotoRow>> {
        let sql = format!("SELECT {PHOTO_COLUMNS} {PHOTOS_FROM} WHERE p.id = ?1");
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
                        e.focal_len, p.capture_time, e.width, e.height, e.orientation,
                        e.gps_lat, e.gps_lon, e.gps_alt
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
                        gps_lat: r.get(11)?,
                        gps_lon: r.get(12)?,
                        gps_alt: r.get(13)?,
                    })
                },
            )
            .optional()?)
    }

    /// Store metadata read from the file; also sets the photo's capture time.
    pub fn set_metadata(&self, photo_id: i64, m: &PhotoMetadata) -> Result<()> {
        write_metadata(&self.conn, photo_id, m)
    }

    /// Photos whose GPS position was never read from their file (catalogs made before
    /// GPS support): (photo id, path, kind), at most `limit`. Missing files and
    /// virtual copies (they follow their master) are left out.
    pub fn gps_unread(&self, limit: usize) -> Result<Vec<(i64, PathBuf, FileKind)>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, fo.path, f.name, f.kind FROM exif e
               JOIN photos p   ON p.id = e.photo_id
               JOIN files f    ON f.id = p.master_file_id
               JOIN folders fo ON fo.id = f.folder_id
              WHERE e.gps_read = 0 AND p.virtual_of IS NULL AND f.missing = 0
              LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            let (dir, name, kind): (String, String, String) = (r.get(1)?, r.get(2)?, r.get(3)?);
            Ok((r.get(0)?, Path::new(&dir).join(name), FileKind::parse(&kind).unwrap_or(FileKind::Image)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Store a GPS position read from the file (`None`: it has none) for a photo and its
    /// virtual copies, in one transaction for many photos.
    pub fn set_gps(&mut self, positions: &[(i64, Option<GpsPosition>)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "UPDATE exif SET gps_lat = ?2, gps_lon = ?3, gps_alt = ?4, gps_read = 1
                  WHERE photo_id IN (SELECT id FROM photos WHERE id = ?1 OR virtual_of = ?1)",
            )?;
            for (id, pos) in positions {
                let (lat, lon, alt) = match pos {
                    Some((lat, lon, alt)) => (Some(*lat), Some(*lon), *alt),
                    None => (None, None, None),
                };
                stmt.execute(params![id, lat, lon, alt])?;
            }
        }
        tx.commit()?;
        Ok(())
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
              focal_len, width, height, orientation, gps_lat, gps_lon, gps_alt, gps_read)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 1)",
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
            m.orientation,
            m.gps_lat,
            m.gps_lon,
            m.gps_alt
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

#[cfg(test)]
mod gps_tests {
    use super::*;

    #[test]
    fn gps_is_stored_and_old_catalogs_are_backfilled() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        std::fs::create_dir(&dir).unwrap();
        for n in ["a.jpg", "b.jpg"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        cat.sync_folder(&dir).unwrap();
        let ids: Vec<i64> = cat.photos(crate::Source::AllPhotographs).unwrap().iter().map(|p| p.id).collect();
        let (a, b) = (ids[0], ids[1]);
        let m = PhotoMetadata {
            gps_lat: Some(59.9),
            gps_lon: Some(-10.7),
            gps_alt: Some(12.0),
            ..Default::default()
        };
        cat.set_metadata(a, &m).unwrap();
        cat.set_metadata(b, &PhotoMetadata::default()).unwrap();
        let back = cat.metadata(a).unwrap().unwrap();
        assert_eq!((back.gps_lat, back.gps_lon, back.gps_alt), (Some(59.9), Some(-10.7), Some(12.0)));
        assert!(cat.gps_unread(10).unwrap().is_empty(), "freshly read metadata includes GPS");

        // A catalog from before GPS support: nothing read yet.
        let copy = cat.create_virtual_copy(b).unwrap();
        cat.conn.execute("UPDATE exif SET gps_read = 0, gps_lat = NULL, gps_lon = NULL", []).unwrap();
        let unread = cat.gps_unread(10).unwrap();
        assert_eq!(unread.iter().map(|u| u.0).collect::<Vec<_>>(), vec![a, b], "virtual copies follow");
        assert_eq!(unread[1].1, dir.join("b.jpg"));
        cat.set_gps(&[(a, None), (b, Some((1.5, 2.5, None)))]).unwrap();
        assert!(cat.gps_unread(10).unwrap().is_empty());
        let c = cat.metadata(copy).unwrap().unwrap();
        assert_eq!((c.gps_lat, c.gps_lon, c.gps_alt), (Some(1.5), Some(2.5), None));
        assert_eq!(cat.metadata(a).unwrap().unwrap().gps_lat, None);
    }
}
