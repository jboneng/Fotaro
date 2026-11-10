//! Pictures and groups, which replace LrC's stacks.
//!
//! - A **picture** is one capture and everything made from it. Every photo belongs to
//!   exactly one picture, with a [`Role`]: the original, passes (an IR scan, a RAW's
//!   JPEG), edits (NegPy, Luminar), virtual copies, exports. A lone photo is a picture
//!   of one. Each picture has an **active** member.
//! - A **group** holds pictures put together by hand (bursts, brackets). Each group has
//!   an active picture.
//! - The grid shows one cell per **unit** (a group, or a picture in no group): the active
//!   member of the active picture. A unit is shown when any of its members matches.
//!
//! Triggers (schema v6) keep it consistent: a new photo gets a picture of its own; a
//! picture that loses its last photo goes; a group of fewer than two pictures dissolves.
//! Passes and edits join their original by file name when they are added
//! ([`adopt`]), following NegPy's own rules.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::catalog::{Catalog, PhotoRow};

/// What a photo is within its picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Role {
    #[default]
    Original,
    /// Another file of the same capture: an IR pass, its mask, a RAW's JPEG.
    Pass,
    /// Made from the original in an external editor.
    Edit,
    /// A virtual copy.
    Copy,
    /// An export added back to the catalog.
    Export,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Pass => "pass",
            Self::Edit => "edit",
            Self::Copy => "copy",
            Self::Export => "export",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "pass" => Self::Pass,
            "edit" => Self::Edit,
            "copy" => Self::Copy,
            "export" => Self::Export,
            _ => Self::Original,
        }
    }
}

/// A photo's place among the grid's units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnitInfo {
    pub picture_id: i64,
    pub group_id: Option<i64>,
    pub role: Role,
    /// Photos in the whole unit (all pictures of its group).
    pub members: u32,
    /// Pictures in the unit (1 unless grouped).
    pub pictures: u32,
    /// The grid shows this photo for a unit in which another member matched the filter.
    pub hidden_match: bool,
}

impl UnitInfo {
    /// More than one photo: the version strip has something to show.
    pub fn is_stack(&self) -> bool {
        self.members > 1
    }
}

/// One photo of a unit, for the version strip.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub photo: PhotoRow,
    /// The active member of its picture.
    pub active: bool,
    /// Its picture is the active one of the group (always true outside groups).
    pub picture_active: bool,
    /// For edits, copies and exports: the photo it was made from.
    pub derived_from: Option<i64>,
}

// ---------------------------------------------------------------------------------
// Joining by name

/// What a file name says about the photo: (role, original stems to look for, whether
/// the original must be a TIFF).
fn classify(stem: &str, ext: &str) -> Option<(Role, Vec<String>, bool)> {
    let lower = stem.to_lowercase();
    let tiff = matches!(ext.to_lowercase().as_str(), "tif" | "tiff");
    // NegPy's IR sidecars: `<name>_IR.tif` (and its mask `<name>_IR_valid.tif`) beside `<name>.tif`.
    if tiff {
        for suffix in ["_ir_valid", "_ir"] {
            if let Some(base) = lower.strip_suffix(suffix).filter(|b| !b.is_empty()) {
                return Some((Role::Pass, vec![base.to_owned()], true));
            }
        }
    }
    // NegPy exports: `<name>_NegPy`, and `<name>_<n>_NegPy` for half frames.
    if let Some(base) = lower.strip_suffix("_negpy").filter(|b| !b.is_empty()) {
        let mut bases = vec![base.to_owned()];
        if let Some((half_of, n)) = base.rsplit_once('_')
            && !half_of.is_empty()
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
        {
            bases.push(half_of.to_owned());
        }
        return Some((Role::Edit, bases, false));
    }
    // Luminar Neo: `<name>_LuminarNeo-edit`, `<name>_LuminarNeo-edit_<n>`.
    let luminar = lower.rsplit_once("_luminarneo-edit").filter(|(base, rest)| {
        !base.is_empty()
            && (rest.is_empty()
                || rest.strip_prefix('_').is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())))
    });
    if let Some((base, _)) = luminar {
        return Some((Role::Edit, vec![base.to_owned()], false));
    }
    None
}

fn split_name(name: &str) -> (String, String) {
    let p = Path::new(name);
    (
        p.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
        p.extension().unwrap_or_default().to_string_lossy().into_owned(),
    )
}

/// Join photos that were just added to the picture they belong to, by file name: IR
/// passes and edits always, a JPEG beside a raw file of the same name when
/// `pair_jpegs`. Only photos alone in their picture are moved (so a photo detached by
/// hand stays detached). Returns how many joined.
pub(crate) fn adopt(conn: &Connection, photo_ids: &[i64], pair_jpegs: bool) -> Result<usize> {
    let mut joined = 0;
    for &id in photo_ids {
        let row: Option<(i64, String, String, i64, String)> = conn
            .query_row(
                "SELECT f.folder_id, f.name, f.kind, p.picture_id, p.role
                   FROM photos p JOIN files f ON f.id = p.master_file_id WHERE p.id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((folder, name, kind, picture, role)) = row else { continue };
        let alone: bool =
            conn.query_row("SELECT COUNT(*) = 1 FROM photos WHERE picture_id = ?1", [picture], |r| r.get(0))?;
        if !alone || role != "original" {
            continue;
        }
        let (stem, ext) = split_name(&name);
        let jpeg = matches!(ext.to_lowercase().as_str(), "jpg" | "jpeg");
        let (role, bases, want) = if pair_jpegs && jpeg && kind == "image" {
            (Role::Pass, vec![stem.to_lowercase()], Want::Raw)
        } else {
            match classify(&stem, &ext) {
                Some((role, bases, true)) => (role, bases, Want::Tiff),
                Some((role, bases, false)) => (role, bases, Want::Any),
                None => continue,
            }
        };
        if let Some(original) = find_original(conn, folder, id, &bases, want)? {
            join(
                conn,
                id,
                original,
                role,
                matches!(role, Role::Edit).then_some(original),
                role == Role::Edit,
            )?;
            joined += 1;
        }
    }
    Ok(joined)
}

#[derive(Clone, Copy, PartialEq)]
enum Want {
    Any,
    Raw,
    Tiff,
}

/// The original photo in `folder` whose file stem is one of `bases` (in order of
/// preference), raw files first.
fn find_original(
    conn: &Connection,
    folder: i64,
    not: i64,
    bases: &[String],
    want: Want,
) -> Result<Option<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT p.id, f.name, f.kind FROM photos p JOIN files f ON f.id = p.master_file_id
          WHERE f.folder_id = ?1 AND p.id <> ?2 AND p.role = 'original' AND p.virtual_of IS NULL
          ORDER BY f.kind = 'raw' DESC, p.id",
    )?;
    let rows: Vec<(i64, String, String)> = stmt
        .query_map(params![folder, not], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for base in bases {
        for (id, name, kind) in &rows {
            let (stem, ext) = split_name(name);
            let fits = match want {
                Want::Any => true,
                Want::Raw => kind == "raw",
                Want::Tiff => matches!(ext.to_lowercase().as_str(), "tif" | "tiff"),
            };
            if fits && stem.to_lowercase() == *base {
                return Ok(Some(*id));
            }
        }
    }
    Ok(None)
}

fn picture_of(conn: &Connection, photo: i64) -> Result<i64> {
    conn.query_row("SELECT picture_id FROM photos WHERE id = ?1", [photo], |r| r.get(0))
        .optional()?
        .context("photo not found")
}

/// Move `photo` into the picture of `to`, with `role`, after its other members.
pub(crate) fn join(
    conn: &Connection,
    photo: i64,
    to: i64,
    role: Role,
    derived_from: Option<i64>,
    make_active: bool,
) -> Result<()> {
    let picture = picture_of(conn, to)?;
    conn.execute(
        "UPDATE photos SET picture_id = ?1, role = ?2, derived_from = ?3,
                member_pos = (SELECT coalesce(MAX(member_pos), 0) + 1 FROM photos WHERE picture_id = ?1)
          WHERE id = ?4",
        params![picture, role.as_str(), derived_from, photo],
    )?;
    // Rating, flag and label belong to the stack: the newcomer takes them.
    take_stack_marks(conn, photo, to)?;
    if make_active {
        set_active_in(conn, photo)?;
    }
    Ok(())
}

/// Give `photo` the rating, flag and colour label of `from` (marked for the sidecar).
fn take_stack_marks(conn: &Connection, photo: i64, from: i64) -> Result<()> {
    conn.execute(
        "UPDATE photos SET (rating, flag, color_label, metadata_dirty) =
                (SELECT rating, flag, color_label, 1 FROM photos WHERE id = ?1)
          WHERE id = ?2 AND (rating, flag, coalesce(color_label, '')) IS NOT
                (SELECT rating, flag, coalesce(color_label, '') FROM photos WHERE id = ?1)",
        params![from, photo],
    )?;
    Ok(())
}

/// Make `photo` the active member of its picture, and its picture the active one of
/// its group.
fn set_active_in(conn: &Connection, photo: i64) -> Result<()> {
    let picture = picture_of(conn, photo)?;
    conn.execute("UPDATE pictures SET active_photo_id = ?1 WHERE id = ?2", params![photo, picture])?;
    conn.execute(
        "UPDATE groups SET active_picture_id = ?1
          WHERE id = (SELECT group_id FROM pictures WHERE id = ?1)",
        [picture],
    )?;
    Ok(())
}

/// Schema v6: join passes and edits already in the catalog to their originals (pairs
/// were moved by the SQL), and give groups an active picture.
pub(crate) fn finish_v6(conn: &Connection) -> Result<()> {
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM photos ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    adopt(conn, &ids, false)?;
    // Old stacks of a photo and its copies (now one picture) are groups of one.
    conn.execute_batch(
        "UPDATE pictures SET group_id = NULL, group_pos = NULL
          WHERE group_id IN (SELECT group_id FROM pictures WHERE group_id IS NOT NULL
                              GROUP BY group_id HAVING COUNT(*) < 2);
         DELETE FROM groups WHERE id NOT IN (SELECT group_id FROM pictures WHERE group_id IS NOT NULL);
         UPDATE groups SET active_picture_id =
                (SELECT id FROM pictures WHERE group_id = groups.id ORDER BY group_pos, id LIMIT 1)
          WHERE active_picture_id IS NULL
             OR active_picture_id NOT IN (SELECT id FROM pictures WHERE group_id = groups.id);",
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------------
// Catalog API

impl Catalog {
    /// Make `photo_id` what its unit shows (Shift+S / double-click in the version strip).
    pub fn set_active(&mut self, photo_id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        set_active_in(&tx, photo_id)?;
        tx.commit()?;
        Ok(())
    }

    /// Put `new` (an edit or export of `original`) into `original`'s picture. A new edit
    /// becomes what the unit shows.
    pub fn add_to_picture(&mut self, original: i64, new: i64, role: Role) -> Result<()> {
        let tx = self.conn.transaction()?;
        join(&tx, new, original, role, Some(original), role == Role::Edit)?;
        tx.commit()?;
        Ok(())
    }

    /// Ctrl+G: group the pictures of `photo_ids` (in this order; groups they are in merge
    /// into the new one). The first photo's picture becomes the active one. Returns the
    /// group's id.
    pub fn group_photos(&mut self, photo_ids: &[i64]) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let mut pictures: Vec<i64> = Vec::new();
        for &id in photo_ids {
            let (picture, group): (i64, Option<i64>) = tx
                .query_row(
                    "SELECT pi.id, pi.group_id FROM photos p JOIN pictures pi ON pi.id = p.picture_id
                      WHERE p.id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .context("photo not found")?;
            let members: Vec<i64> = match group {
                Some(g) => tx
                    .prepare("SELECT id FROM pictures WHERE group_id = ?1 ORDER BY group_pos, id")?
                    .query_map([g], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?,
                None => vec![picture],
            };
            for m in members {
                if !pictures.contains(&m) {
                    pictures.push(m);
                }
            }
        }
        anyhow::ensure!(pictures.len() >= 2, "a group needs at least two pictures");
        tx.execute("INSERT INTO groups (active_picture_id) VALUES (?1)", [pictures[0]])?;
        let group = tx.last_insert_rowid();
        for (pos, p) in pictures.iter().enumerate() {
            tx.execute(
                "UPDATE pictures SET group_id = ?1, group_pos = ?2 WHERE id = ?3",
                params![group, pos as i64, p],
            )?;
        }
        // One stack, one rating, flag and label: the first photo's.
        let first = photo_ids[0];
        let members: Vec<i64> = tx
            .prepare(
                "SELECT p.id FROM photos p JOIN pictures pi ON pi.id = p.picture_id WHERE pi.group_id = ?1",
            )?
            .query_map([group], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for m in members {
            take_stack_marks(&tx, m, first)?;
        }
        tx.commit()?;
        Ok(group)
    }

    /// Ctrl+Shift+G: dissolve the groups these photos are in (each picture keeps its
    /// members). Returns how many groups went.
    pub fn ungroup(&mut self, photo_ids: &[i64]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        for &id in photo_ids {
            let group: Option<i64> = tx
                .query_row(
                    "SELECT pi.group_id FROM photos p JOIN pictures pi ON pi.id = p.picture_id WHERE p.id = ?1",
                    [id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            if let Some(g) = group {
                tx.execute("UPDATE pictures SET group_id = NULL, group_pos = NULL WHERE group_id = ?1", [g])?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// Photo ▸ Detach from Picture: the photo becomes an original in a picture of its own
    /// (outside any group).
    pub fn detach(&mut self, photo_id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let alone: bool = tx.query_row(
            "SELECT COUNT(*) = 1 FROM photos WHERE picture_id = (SELECT picture_id FROM photos WHERE id = ?1)",
            [photo_id],
            |r| r.get(0),
        )?;
        anyhow::ensure!(!alone, "the photo is already a picture of its own");
        tx.execute("INSERT INTO pictures (active_photo_id) VALUES (?1)", [photo_id])?;
        let picture = tx.last_insert_rowid();
        tx.execute(
            "UPDATE photos SET picture_id = ?1, role = 'original', member_pos = 0 WHERE id = ?2",
            params![picture, photo_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Every photo in the stacks (units) of `photo_ids`, each once: what a rating, flag
    /// or label set on a stack applies to.
    pub fn unit_photo_ids(&self, photo_ids: &[i64]) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT m.id FROM photos p
               JOIN pictures pi ON pi.id = p.picture_id
               JOIN pictures mp ON mp.id = pi.id OR (pi.group_id IS NOT NULL AND mp.group_id = pi.group_id)
               JOIN photos m    ON m.picture_id = mp.id
              WHERE p.id = ?1",
        )?;
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for &id in photo_ids {
            for m in stmt.query_map([id], |r| r.get::<_, i64>(0))? {
                let m = m?;
                if seen.insert(m) {
                    out.push(m);
                }
            }
        }
        Ok(out)
    }

    /// The rejected photos of `source` (whatever the filter), for Delete Rejected Photos.
    pub fn rejected_in(&self, source: crate::Source) -> Result<Vec<i64>> {
        let mut args = Vec::new();
        let cond = self.source_condition(source, &mut args)?;
        let sql = format!(
            "SELECT p.id {} WHERE p.flag = -1 AND ({cond}) ORDER BY p.id",
            crate::catalog::PHOTOS_FROM
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let ids = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }

    /// Remove photos from the catalog; their files stay on disk. A photo goes with its
    /// file's catalog entry (and so with its virtual copies), so a later sync or import
    /// finds the file again; a virtual copy goes alone. Returns the ids of the photos
    /// whose previews can go (copies share their master's).
    pub fn remove_photos(&mut self, photo_ids: &[i64]) -> Result<Vec<i64>> {
        let tx = self.conn.transaction()?;
        let mut previews = Vec::new();
        for &id in photo_ids {
            let row: Option<(i64, bool)> = tx
                .query_row(
                    "SELECT master_file_id, virtual_of IS NOT NULL FROM photos WHERE id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match row {
                None => {}
                Some((_, true)) => {
                    tx.execute("DELETE FROM photos WHERE id = ?1", [id])?;
                }
                Some((file, false)) => {
                    tx.execute("DELETE FROM files WHERE id = ?1", [file])?;
                    previews.push(id);
                }
            }
        }
        tx.commit()?;
        Ok(previews)
    }

    /// Every photo of the unit `photo_id` is in, picture by picture (the group's order),
    /// each picture's original first, then passes, then the rest in the order they came.
    pub fn unit_members(&self, photo_id: i64) -> Result<Vec<Member>> {
        let (picture, group): (i64, Option<i64>) = self
            .conn
            .query_row(
                "SELECT pi.id, pi.group_id FROM photos p JOIN pictures pi ON pi.id = p.picture_id WHERE p.id = ?1",
                [photo_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .context("photo not found")?;
        let (filter, key) = match group {
            Some(g) => ("pi.group_id = ?1", g),
            None => ("pi.id = ?1", picture),
        };
        let sql = format!(
            "SELECT {cols}, pi.active_photo_id = p.id,
                    pi.group_id IS NULL OR (SELECT active_picture_id FROM groups WHERE id = pi.group_id) = pi.id,
                    p.derived_from
               {from}
              WHERE {filter}
              ORDER BY pi.group_pos, pi.id,
                       CASE p.role WHEN 'original' THEN 0 WHEN 'pass' THEN 1 ELSE 2 END, p.member_pos, p.id",
            cols = crate::catalog::PHOTO_COLUMNS,
            from = crate::catalog::PHOTOS_FROM,
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let n = crate::catalog::PHOTO_COLUMN_COUNT;
        let rows = stmt.query_map([key], |r| {
            Ok(Member {
                photo: crate::catalog::photo_row(r)?,
                active: r.get(n)?,
                picture_active: r.get(n + 1)?,
                derived_from: r.get(n + 2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests;
