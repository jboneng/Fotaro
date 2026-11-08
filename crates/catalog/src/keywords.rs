//! Keyword tags: a tree like LrC's Keyword List, assigned to photos.
//!
//! Keyword names are matched case-insensitively within a parent, as in LrC.
//! Every change to a photo's keywords marks its metadata as needing to be saved.

use std::collections::HashMap;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use crate::catalog::Catalog;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyword {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    /// Photos tagged with exactly this keyword.
    pub photo_count: i64,
}

/// Keyword paths typed in the Keywording panel: comma- or semicolon-separated, with
/// hierarchy written LrC-style as `Trondheim < Norway < Places`, or as
/// `Places > Norway > Trondheim` or `Places|Norway|Trondheim`. Returns root-first paths.
pub fn parse_keyword_input(input: &str) -> Vec<Vec<String>> {
    input
        .split([',', ';'])
        .filter_map(|item| {
            let item = item.trim();
            let mut parts: Vec<String> = if item.contains('<') {
                item.rsplit('<').map(|s| s.trim().to_owned()).collect()
            } else {
                item.split(['>', '|']).map(|s| s.trim().to_owned()).collect()
            };
            parts.retain(|p| !p.is_empty());
            (!parts.is_empty()).then_some(parts)
        })
        .collect()
}

pub(crate) fn find_keyword(conn: &Connection, parent: Option<i64>, name: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM keywords WHERE parent_id IS ?1 AND name = ?2 COLLATE NOCASE",
            params![parent, name],
            |r| r.get(0),
        )
        .optional()?)
}

/// Find or create every level of a root-first keyword path; returns the leaf's id.
pub(crate) fn ensure_keyword_path(conn: &Connection, path: &[String]) -> Result<i64> {
    anyhow::ensure!(!path.is_empty(), "empty keyword");
    let mut parent = None;
    for name in path {
        let name = name.trim();
        anyhow::ensure!(!name.is_empty(), "empty keyword name");
        let id = match find_keyword(conn, parent, name)? {
            Some(id) => id,
            None => {
                conn.execute(
                    "INSERT INTO keywords (name, parent_id) VALUES (?1, ?2)",
                    params![name, parent],
                )?;
                conn.last_insert_rowid()
            }
        };
        parent = Some(id);
    }
    Ok(parent.expect("path is not empty"))
}

/// Root-first keyword paths of a photo, sorted.
pub(crate) fn photo_keyword_paths(conn: &Connection, photo_id: i64) -> Result<Vec<Vec<String>>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE up(leaf, id, name, parent_id, depth) AS (
             SELECT k.id, k.id, k.name, k.parent_id, 0
               FROM photo_keywords pk JOIN keywords k ON k.id = pk.keyword_id
              WHERE pk.photo_id = ?1
             UNION ALL
             SELECT up.leaf, k.id, k.name, k.parent_id, up.depth + 1
               FROM keywords k JOIN up ON k.id = up.parent_id
         )
         SELECT leaf, name FROM up ORDER BY leaf, depth DESC",
    )?;
    let mut paths: HashMap<i64, Vec<String>> = HashMap::new();
    for row in stmt.query_map([photo_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (leaf, name) = row?;
        paths.entry(leaf).or_default().push(name);
    }
    let mut paths: Vec<Vec<String>> = paths.into_values().collect();
    paths.sort_by_key(|p| p.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>());
    Ok(paths)
}

/// Replace a photo's keywords with `paths` (creating keywords as needed).
pub(crate) fn set_photo_keyword_paths(conn: &Connection, photo_id: i64, paths: &[Vec<String>]) -> Result<()> {
    conn.execute("DELETE FROM photo_keywords WHERE photo_id = ?1", [photo_id])?;
    for path in paths.iter().filter(|p| !p.is_empty()) {
        let id = ensure_keyword_path(conn, path)?;
        conn.execute(
            "INSERT OR IGNORE INTO photo_keywords (photo_id, keyword_id) VALUES (?1, ?2)",
            params![photo_id, id],
        )?;
    }
    Ok(())
}

impl Catalog {
    /// All keywords with how many photos carry each (directly).
    pub fn keywords(&self) -> Result<Vec<Keyword>> {
        let mut stmt = self.conn.prepare(
            "SELECT k.id, k.name, k.parent_id, COUNT(pk.photo_id)
               FROM keywords k LEFT JOIN photo_keywords pk ON pk.keyword_id = k.id
              GROUP BY k.id
              ORDER BY k.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Keyword { id: r.get(0)?, name: r.get(1)?, parent_id: r.get(2)?, photo_count: r.get(3)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Find or create a root-first keyword path; returns the leaf's id.
    pub fn ensure_keyword(&mut self, path: &[String]) -> Result<i64> {
        let tx = self.conn.transaction()?;
        let id = ensure_keyword_path(&tx, path)?;
        tx.commit()?;
        Ok(id)
    }

    /// LrC "Create Keyword Tag": a new keyword under `parent` (or at the top).
    pub fn create_keyword(&mut self, parent: Option<i64>, name: &str) -> Result<i64> {
        let name = validate_keyword_name(name)?;
        anyhow::ensure!(
            find_keyword(&self.conn, parent, name)?.is_none(),
            "keyword \"{name}\" already exists here"
        );
        self.conn.execute("INSERT INTO keywords (name, parent_id) VALUES (?1, ?2)", params![name, parent])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_keyword(&mut self, id: i64, name: &str) -> Result<()> {
        let name = validate_keyword_name(name)?;
        let parent: Option<i64> = self
            .conn
            .query_row("SELECT parent_id FROM keywords WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .context("keyword not found")?;
        if let Some(other) = find_keyword(&self.conn, parent, name)? {
            anyhow::ensure!(other == id, "keyword \"{name}\" already exists here");
        }
        let tx = self.conn.transaction()?;
        tx.execute("UPDATE keywords SET name = ?1 WHERE id = ?2", params![name, id])?;
        mark_keyword_photos_dirty(&tx, id)?;
        tx.commit()?;
        Ok(())
    }

    /// Delete a keyword and everything below it, removing it from all photos.
    /// Returns the ids of the photos that lost a keyword.
    pub fn delete_keyword(&mut self, id: i64) -> Result<Vec<i64>> {
        let tx = self.conn.transaction()?;
        let affected = keyword_subtree_photo_ids(&tx, id)?;
        mark_keyword_photos_dirty(&tx, id)?;
        // photo_keywords and child keywords cascade.
        let n = tx.execute("DELETE FROM keywords WHERE id = ?1", [id])?;
        anyhow::ensure!(n == 1, "keyword not found");
        tx.commit()?;
        Ok(affected)
    }

    /// Tag photos with a keyword (no-op where already tagged).
    pub fn add_keyword(&mut self, photo_ids: &[i64], keyword_id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        for id in photo_ids {
            let n = tx.execute(
                "INSERT OR IGNORE INTO photo_keywords (photo_id, keyword_id) VALUES (?1, ?2)",
                params![id, keyword_id],
            )?;
            if n > 0 {
                tx.execute("UPDATE photos SET metadata_dirty = 1 WHERE id = ?1", [id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_keyword(&mut self, photo_ids: &[i64], keyword_id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        for id in photo_ids {
            let n = tx.execute(
                "DELETE FROM photo_keywords WHERE photo_id = ?1 AND keyword_id = ?2",
                params![id, keyword_id],
            )?;
            if n > 0 {
                tx.execute("UPDATE photos SET metadata_dirty = 1 WHERE id = ?1", [id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// For each keyword on any of `photo_ids`: how many of them carry it.
    pub fn keyword_counts_for(&self, photo_ids: &[i64]) -> Result<HashMap<i64, usize>> {
        let mut out = HashMap::new();
        let mut stmt =
            self.conn.prepare_cached("SELECT keyword_id FROM photo_keywords WHERE photo_id = ?1")?;
        for id in photo_ids {
            for kw in stmt.query_map([id], |r| r.get::<_, i64>(0))? {
                *out.entry(kw?).or_insert(0) += 1;
            }
        }
        Ok(out)
    }

    /// Root-first keyword paths of a photo.
    pub fn photo_keywords(&self, photo_id: i64) -> Result<Vec<Vec<String>>> {
        photo_keyword_paths(&self.conn, photo_id)
    }
}

fn validate_keyword_name(name: &str) -> Result<&str> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "the keyword name is empty");
    anyhow::ensure!(
        !name.contains([',', ';', '|', '<', '>']),
        "keyword names cannot contain , ; | < or > (they separate keywords and levels)"
    );
    Ok(name)
}

fn keyword_subtree_photo_ids(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "WITH RECURSIVE tree(id) AS (
             SELECT ?1 UNION ALL SELECT k.id FROM keywords k JOIN tree ON k.parent_id = tree.id
         )
         SELECT DISTINCT photo_id FROM photo_keywords WHERE keyword_id IN tree",
    )?;
    let ids = stmt.query_map([id], |r| r.get(0))?;
    Ok(ids.collect::<rusqlite::Result<_>>()?)
}

/// Keyword renames and deletes change the sidecars of every photo using them.
fn mark_keyword_photos_dirty(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "WITH RECURSIVE tree(id) AS (
             SELECT ?1 UNION ALL SELECT k.id FROM keywords k JOIN tree ON k.parent_id = tree.id
         )
         UPDATE photos SET metadata_dirty = 1
          WHERE id IN (SELECT photo_id FROM photo_keywords WHERE keyword_id IN tree)",
        [id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Source;

    fn path(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_lightroom_style_input() {
        assert_eq!(
            parse_keyword_input("cat, Trondheim < Norway < Places ; Film>Fuji C200, ,a|b"),
            vec![
                path(&["cat"]),
                path(&["Places", "Norway", "Trondheim"]),
                path(&["Film", "Fuji C200"]),
                path(&["a", "b"]),
            ]
        );
        assert!(parse_keyword_input(" , ;").is_empty());
    }

    fn catalog_with_two_photos() -> (tempfile::TempDir, Catalog, Vec<i64>) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("p");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("a.jpg"), b"x").unwrap();
        std::fs::write(dir.join("b.jpg"), b"x").unwrap();
        let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        cat.sync_folder(&dir).unwrap();
        let ids = cat.photos(Source::AllPhotographs).unwrap().iter().map(|p| p.id).collect();
        (tmp, cat, ids)
    }

    #[test]
    fn keyword_tree_assignment_and_paths() {
        let (_tmp, mut cat, ids) = catalog_with_two_photos();
        let oslo = cat.ensure_keyword(&path(&["Places", "Norway", "Oslo"])).unwrap();
        // Same path, different case: found, not duplicated.
        assert_eq!(cat.ensure_keyword(&path(&["places", "NORWAY", "oslo"])).unwrap(), oslo);
        let cat_kw = cat.ensure_keyword(&path(&["Cat"])).unwrap();
        assert_eq!(cat.keywords().unwrap().len(), 4);

        cat.add_keyword(&ids, oslo).unwrap();
        cat.add_keyword(&ids[..1], cat_kw).unwrap();
        assert_eq!(
            cat.photo_keywords(ids[0]).unwrap(),
            vec![path(&["Cat"]), path(&["Places", "Norway", "Oslo"])]
        );
        let counts = cat.keyword_counts_for(&ids).unwrap();
        assert_eq!((counts[&oslo], counts[&cat_kw]), (2, 1));
        assert!(cat.photo(ids[0]).unwrap().unwrap().metadata_dirty);

        cat.remove_keyword(&ids, cat_kw).unwrap();
        assert_eq!(cat.photo_keywords(ids[0]).unwrap(), vec![path(&["Places", "Norway", "Oslo"])]);
    }

    #[test]
    fn rename_and_delete_keywords() {
        let (_tmp, mut cat, ids) = catalog_with_two_photos();
        let oslo = cat.ensure_keyword(&path(&["Places", "Oslo"])).unwrap();
        let places = cat.keywords().unwrap().into_iter().find(|k| k.name == "Places").unwrap().id;
        cat.add_keyword(&ids[..1], oslo).unwrap();

        assert!(cat.create_keyword(None, "places").is_err(), "case-insensitive duplicate");
        assert!(cat.create_keyword(None, "a, b").is_err());
        let europe = cat.create_keyword(None, "Europe").unwrap();
        assert!(cat.rename_keyword(europe, "PLACES").is_err());
        cat.rename_keyword(places, "Steder").unwrap();
        assert_eq!(cat.photo_keywords(ids[0]).unwrap(), vec![path(&["Steder", "Oslo"])]);

        let affected = cat.delete_keyword(places).unwrap();
        assert_eq!(affected, vec![ids[0]]);
        assert!(cat.photo_keywords(ids[0]).unwrap().is_empty());
        assert_eq!(cat.keywords().unwrap().len(), 1, "subtree deleted, Europe stays");
    }
}
