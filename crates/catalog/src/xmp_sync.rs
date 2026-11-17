//! IPTC text fields, and moving metadata between the catalog and XMP sidecars
//! (LrC's "Save Metadata to File" / "Read Metadata from File").
//!
//! The catalog stays the source of truth: sidecars are written on request (or
//! automatically if the user turns that on) and read when photos are added or
//! when the user asks. Originals are never modified.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use xmp::XmpData;

use crate::catalog::{Catalog, Flag};
use crate::keywords::{photo_keyword_paths, set_photo_keyword_paths};
use crate::scan::FileKind;

/// LrC's IPTC Content/Copyright fields this app edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IptcField {
    Title,
    Caption,
    Creator,
    Copyright,
}

impl IptcField {
    pub const ALL: [IptcField; 4] = [Self::Title, Self::Caption, Self::Creator, Self::Copyright];

    pub fn label(self) -> &'static str {
        match self {
            Self::Title => "Title",
            Self::Caption => "Caption",
            Self::Creator => "Creator",
            Self::Copyright => "Copyright",
        }
    }

    fn column(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Caption => "caption",
            Self::Creator => "creator",
            Self::Copyright => "copyright",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Iptc {
    pub title: Option<String>,
    pub caption: Option<String>,
    pub creator: Option<String>,
    pub copyright: Option<String>,
}

impl Iptc {
    pub fn get(&self, field: IptcField) -> Option<&str> {
        match field {
            IptcField::Title => self.title.as_deref(),
            IptcField::Caption => self.caption.as_deref(),
            IptcField::Creator => self.creator.as_deref(),
            IptcField::Copyright => self.copyright.as_deref(),
        }
    }
}

/// Reads a file's own orientation (clockwise quarter turns) when the catalog doesn't
/// know it yet, e.g. for photos added before it was recorded.
pub type OrientationReader<'a> = &'a dyn Fn(&Path, FileKind) -> Option<u8>;

const LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

fn label_to_xmp(label: &str) -> String {
    let mut s = label.to_owned();
    if let Some(first) = s.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    s
}

fn label_from_xmp(label: &str) -> Option<String> {
    let l = label.trim().to_lowercase();
    LABELS.contains(&l.as_str()).then_some(l)
}

/// EXIF orientation code <-> clockwise quarter turns (mirrored codes have no turns).
fn turns_from_code(code: u8) -> Option<u8> {
    match code {
        1 => Some(0),
        6 => Some(1),
        3 => Some(2),
        8 => Some(3),
        _ => None,
    }
}

fn code_from_turns(turns: u8) -> u8 {
    [1, 6, 3, 8][usize::from(turns % 4)]
}

/// Everything needed to write one photo's sidecar.
struct Gathered {
    path: PathBuf,
    kind: FileKind,
    data: XmpData,
    user_turns: u8,
    file_turns: Option<u8>,
}

fn gather(conn: &Connection, photo_id: i64) -> Result<Gathered> {
    let (
        folder,
        name,
        kind,
        rating,
        flag,
        label,
        orientation,
        title,
        caption,
        creator,
        copyright,
        file_turns,
    ) = conn
        .query_row(
            "SELECT fo.path, f.name, f.kind, p.rating, p.flag, p.color_label, p.orientation,
                    p.title, p.caption, p.creator, p.copyright, e.orientation
               FROM photos p JOIN files f ON f.id = p.master_file_id JOIN folders fo ON fo.id = f.folder_id
               LEFT JOIN exif e ON e.photo_id = p.id
              WHERE p.id = ?1",
            [photo_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, Option<String>>(10)?,
                    r.get::<_, Option<u8>>(11)?,
                ))
            },
        )
        .optional()?
        .context("photo not found")?;
    let user_turns = turns_from_code(u8::try_from(orientation).unwrap_or(1)).unwrap_or(0);
    let some_or_empty = |v: Option<String>| Some(v.unwrap_or_default());
    Ok(Gathered {
        path: Path::new(&folder).join(name),
        kind: FileKind::parse(&kind).unwrap_or(FileKind::Image),
        data: XmpData {
            rating: Some(rating as i8),
            label: Some(label.as_deref().map(label_to_xmp).unwrap_or_default()),
            pick: Some(flag as i8),
            orientation: None, // composed by the caller once the file orientation is known
            keywords: Some(photo_keyword_paths(conn, photo_id)?),
            title: some_or_empty(title),
            caption: some_or_empty(caption),
            creator: some_or_empty(creator),
            copyright: some_or_empty(copyright),
            develop: Some(crate::develop::load(conn, photo_id)?.unwrap_or_default()),
        },
        user_turns,
        file_turns,
    })
}

/// A photo's catalog metadata as XMP fields (orientation left out).
pub(crate) fn gather_data(conn: &Connection, photo_id: i64) -> Result<XmpData> {
    Ok(gather(conn, photo_id)?.data)
}

/// Make the catalog match a sidecar (LrC "Read Metadata from File"). Properties the
/// sidecar doesn't have are cleared, except the pick flag, which only this app writes.
pub(crate) fn apply_xmp(conn: &Connection, photo_id: i64, d: &XmpData, file_turns: u8) -> Result<()> {
    let (rating, rejected) = match d.rating {
        Some(r) if r < 0 => (0, true), // Bridge's "Reject"
        Some(r) => (r.clamp(0, 5), false),
        None => (0, false),
    };
    conn.execute("UPDATE photos SET rating = ?1 WHERE id = ?2", params![rating, photo_id])?;
    let flag = match d.pick {
        Some(p) if p > 0 => Some(Flag::Picked),
        Some(p) if p < 0 => Some(Flag::Rejected),
        Some(_) => Some(Flag::Unflagged),
        None => rejected.then_some(Flag::Rejected),
    };
    if let Some(flag) = flag {
        conn.execute("UPDATE photos SET flag = ?1 WHERE id = ?2", params![flag as i64, photo_id])?;
    }
    let label = d.label.as_deref().and_then(label_from_xmp);
    conn.execute("UPDATE photos SET color_label = ?1 WHERE id = ?2", params![label, photo_id])?;
    // tiff:Orientation is the orientation as displayed; the catalog stores our turns on top.
    let user_turns = match d.orientation.and_then(turns_from_code) {
        Some(shown) => (shown + 4 - file_turns % 4) % 4,
        None => 0,
    };
    conn.execute(
        "UPDATE photos SET orientation = ?1 WHERE id = ?2",
        params![i64::from(code_from_turns(user_turns)), photo_id],
    )?;
    set_photo_keyword_paths(conn, photo_id, d.keywords.as_deref().unwrap_or_default())?;
    // Develop settings are taken when the sidecar has them, but never cleared because it
    // doesn't: another app rewriting the sidecar may drop our property.
    if let Some(json) = d.develop.as_deref().filter(|j| !j.trim().is_empty())
        && crate::develop::load(conn, photo_id)?.as_deref() != Some(json)
    {
        crate::develop::change(conn, photo_id, Some(json), "Read Metadata from File")?;
    }
    conn.execute(
        "UPDATE photos SET title = ?1, caption = ?2, creator = ?3, copyright = ?4, metadata_dirty = 0
          WHERE id = ?5",
        params![d.title, d.caption, d.creator, d.copyright, photo_id],
    )?;
    Ok(())
}

impl Catalog {
    pub fn iptc(&self, photo_id: i64) -> Result<Iptc> {
        Ok(self.conn.query_row(
            "SELECT title, caption, creator, copyright FROM photos WHERE id = ?1",
            [photo_id],
            |r| Ok(Iptc { title: r.get(0)?, caption: r.get(1)?, creator: r.get(2)?, copyright: r.get(3)? }),
        )?)
    }

    /// Set one IPTC field on several photos; an empty value clears it.
    pub fn set_iptc(&mut self, photo_ids: &[i64], field: IptcField, value: &str) -> Result<()> {
        let value = Some(value.trim()).filter(|v| !v.is_empty());
        let sql = format!("UPDATE photos SET {} = ?1, metadata_dirty = 1 WHERE id = ?2", field.column());
        let tx = self.conn.transaction()?;
        for id in photo_ids {
            tx.execute(&sql, params![value, id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Photos whose catalog metadata hasn't been written to their sidecar yet.
    pub fn dirty_photo_ids(&self) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare("SELECT id FROM photos WHERE metadata_dirty = 1")?;
        let ids = stmt.query_map([], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }

    fn file_turns(&self, photo_id: i64, g: &Gathered, reader: OrientationReader) -> u8 {
        if let Some(t) = g.file_turns {
            return t;
        }
        let Some(t) = reader(&g.path, g.kind) else { return 0 };
        let stored = self.conn.execute(
            "INSERT INTO exif (photo_id, orientation) VALUES (?2, ?1)
             ON CONFLICT(photo_id) DO UPDATE SET orientation = excluded.orientation",
            params![t, photo_id],
        );
        if let Err(e) = stored {
            log::warn!("storing orientation of {}: {e}", g.path.display());
        }
        t
    }

    /// LrC "Save Metadata to File": update (or create) the photo's XMP sidecar.
    /// Returns the sidecar's path, or `None` for virtual copies (only masters have one).
    pub fn write_xmp(&mut self, photo_id: i64, orientation_of: OrientationReader) -> Result<Option<PathBuf>> {
        if self.is_virtual_copy(photo_id)? {
            return Ok(None);
        }
        let mut g = gather(&self.conn, photo_id)?;
        let file_turns = self.file_turns(photo_id, &g, orientation_of);
        g.data.orientation = Some(code_from_turns(file_turns + g.user_turns));
        let raw = g.kind == FileKind::Raw;
        let rotated = g.user_turns != 0;
        // Only mention orientation if the photo is rotated, or to keep an existing one right.
        let mtime = xmp::write_sidecar(&g.path, raw, &g.data, |had| rotated || had)?;
        self.conn.execute(
            "UPDATE photos SET metadata_dirty = 0, xmp_mtime_ms = ?1 WHERE id = ?2",
            params![mtime, photo_id],
        )?;
        Ok(Some(xmp::sidecar_path(&g.path, raw)))
    }

    /// LrC "Read Metadata from File": replace the photo's catalog metadata with its
    /// sidecar's. Returns false (and changes nothing) if it has no sidecar.
    pub fn read_xmp(&mut self, photo_id: i64, orientation_of: OrientationReader) -> Result<bool> {
        if self.is_virtual_copy(photo_id)? {
            return Ok(false);
        }
        let g = gather(&self.conn, photo_id)?;
        let Some(sidecar) = xmp::read_sidecar(&g.path, g.kind == FileKind::Raw)? else {
            return Ok(false);
        };
        let file_turns = self.file_turns(photo_id, &g, orientation_of);
        let tx = self.conn.transaction()?;
        apply_xmp(&tx, photo_id, &sidecar.data, file_turns)?;
        tx.execute("UPDATE photos SET xmp_mtime_ms = ?1 WHERE id = ?2", params![sidecar.mtime_ms, photo_id])?;
        tx.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Source;

    fn path(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    const NO_ORIENTATION: OrientationReader = &|_, _| None;

    fn setup(files: &[&str]) -> (tempfile::TempDir, PathBuf, Catalog) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        std::fs::create_dir(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        (tmp, dir, cat)
    }

    #[test]
    fn metadata_round_trips_through_a_sidecar() {
        let (_tmp, dir, mut cat) = setup(&["IMG_1.NEF"]);
        cat.sync_folder(&dir).unwrap();
        let id = cat.photos(Source::AllPhotographs).unwrap()[0].id;
        cat.set_rating(id, 4).unwrap();
        cat.set_flag(id, Flag::Picked).unwrap();
        cat.set_color_label(id, Some("green")).unwrap();
        cat.set_rotation(id, 1).unwrap();
        cat.set_iptc(&[id], IptcField::Title, "Hegra").unwrap();
        cat.set_iptc(&[id], IptcField::Creator, " J. B. ").unwrap();
        let kw = cat.ensure_keyword(&path(&["Film", "Fuji C200"])).unwrap();
        cat.add_keyword(&[id], kw).unwrap();
        assert_eq!(cat.dirty_photo_ids().unwrap(), vec![id]);

        // The camera rotated this file a quarter turn already: the sidecar gets the total.
        let file_rotated: OrientationReader = &|_, _| Some(3);
        let sidecar = cat.write_xmp(id, file_rotated).unwrap().unwrap();
        assert_eq!(sidecar, dir.join("IMG_1.xmp"));
        assert!(cat.dirty_photo_ids().unwrap().is_empty());
        let on_disk = xmp::read_sidecar(&dir.join("IMG_1.NEF"), true).unwrap().unwrap().data;
        assert_eq!(on_disk.orientation, Some(1), "file 270° + our 90° = upright");
        assert_eq!(on_disk.label.as_deref(), Some("Green"));

        // Wipe the catalog's values, then read them back from the sidecar.
        cat.set_rating(id, 0).unwrap();
        cat.set_color_label(id, None).unwrap();
        cat.set_rotation(id, 0).unwrap();
        cat.set_flag(id, Flag::Unflagged).unwrap();
        cat.remove_keyword(&[id], kw).unwrap();
        cat.set_iptc(&[id], IptcField::Title, "").unwrap();
        assert!(cat.read_xmp(id, NO_ORIENTATION).unwrap());
        let p = cat.photo(id).unwrap().unwrap();
        assert_eq!(
            (p.rating, p.flag, p.color_label.as_deref(), p.rotation),
            (4, Flag::Picked, Some("green"), 1)
        );
        assert!(!p.metadata_dirty);
        assert_eq!(cat.photo_keywords(id).unwrap(), vec![path(&["Film", "Fuji C200"])]);
        let iptc = cat.iptc(id).unwrap();
        assert_eq!((iptc.title.as_deref(), iptc.creator.as_deref()), (Some("Hegra"), Some("J. B.")));
    }

    #[test]
    fn adding_photos_reads_existing_lightroom_sidecars() {
        let (_tmp, dir, mut cat) = setup(&["IMG_2.CR2", "scan.tif"]);
        std::fs::write(
            dir.join("IMG_2.xmp"),
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
               <rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:lr="http://ns.adobe.com/lightroom/1.0/"
                 xmp:Rating="3" xmp:Label="Yellow"><lr:hierarchicalSubject><rdf:Bag><rdf:li>Places|Hegra</rdf:li>
               </rdf:Bag></lr:hierarchicalSubject></rdf:Description></rdf:RDF></x:xmpmeta>"#,
        )
        .unwrap();
        // A broken sidecar must not stop the import.
        std::fs::write(dir.join("scan.tif.xmp"), "garbage").unwrap();
        let report = cat.sync_folder(&dir).unwrap();
        assert_eq!(report.added, 2);
        let photos = cat.photos(Source::AllPhotographs).unwrap();
        let raw = photos.iter().find(|p| p.file_name == "IMG_2.CR2").unwrap();
        assert_eq!((raw.rating, raw.color_label.as_deref(), raw.metadata_dirty), (3, Some("yellow"), false));
        assert_eq!(cat.photo_keywords(raw.id).unwrap(), vec![path(&["Places", "Hegra"])]);
        let tif = photos.iter().find(|p| p.file_name == "scan.tif").unwrap();
        assert_eq!(tif.rating, 0);
    }

    #[test]
    fn read_without_sidecar_changes_nothing() {
        let (_tmp, dir, mut cat) = setup(&["a.jpg"]);
        cat.sync_folder(&dir).unwrap();
        let id = cat.photos(Source::AllPhotographs).unwrap()[0].id;
        cat.set_rating(id, 2).unwrap();
        assert!(!cat.read_xmp(id, NO_ORIENTATION).unwrap());
        assert_eq!(cat.photo(id).unwrap().unwrap().rating, 2);
    }
}
