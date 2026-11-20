//! Develop settings and their History (LrC's Develop module). The catalog stores the
//! settings as JSON and doesn't interpret them; the imaging crate does.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::catalog::Catalog;

/// One History panel step: the settings after it (`None`: as imported).
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryStep {
    pub id: i64,
    pub label: String,
    pub settings: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
}

/// A Snapshots panel entry: settings kept under a name (`None`: as imported).
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub id: i64,
    pub name: String,
    pub settings: Option<String>,
    /// Unix seconds (taken or last updated).
    pub created_at: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Store (or with `None`, remove) a photo's settings. Doesn't touch History.
pub(crate) fn store(conn: &Connection, photo_id: i64, settings: Option<&str>) -> Result<()> {
    match settings {
        Some(json) => conn.execute(
            "INSERT INTO develop (photo_id, settings, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(photo_id) DO UPDATE SET settings = excluded.settings, updated_at = excluded.updated_at",
            params![photo_id, json, now()],
        )?,
        None => conn.execute("DELETE FROM develop WHERE photo_id = ?1", [photo_id])?,
    };
    Ok(())
}

pub(crate) fn load(conn: &Connection, photo_id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT settings FROM develop WHERE photo_id = ?1", [photo_id], |r| r.get(0))
        .optional()?)
}

/// Store new settings and add a History step named `label`. The first change also
/// records an "Import" step for the state before it, as LrC's History starts with one.
pub(crate) fn change(conn: &Connection, photo_id: i64, settings: Option<&str>, label: &str) -> Result<()> {
    let has_history: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM develop_history WHERE photo_id = ?1)",
        [photo_id],
        |r| r.get(0),
    )?;
    if !has_history {
        conn.execute(
            "INSERT INTO develop_history (photo_id, label, settings, created_at) VALUES (?1, 'Import', ?2, ?3)",
            params![photo_id, load(conn, photo_id)?, now()],
        )?;
    }
    store(conn, photo_id, settings)?;
    conn.execute(
        "INSERT INTO develop_history (photo_id, label, settings, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![photo_id, label, settings, now()],
    )?;
    Ok(())
}

impl Catalog {
    /// A photo's Develop settings, if it has any.
    pub fn develop_settings(&self, photo_id: i64) -> Result<Option<String>> {
        load(&self.conn, photo_id)
    }

    /// Change a photo's Develop settings (`None`: reset to none) and add a History step
    /// named `label` (see [`change`]). The photo's sidecar then needs saving.
    pub fn set_develop_settings(&mut self, photo_id: i64, settings: Option<&str>, label: &str) -> Result<()> {
        let tx = self.conn.transaction()?;
        change(&tx, photo_id, settings, label)?;
        tx.execute("UPDATE photos SET metadata_dirty = 1 WHERE id = ?1", [photo_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Go back to a History step: its settings become current and the steps after it
    /// are dropped (as when you click a step in LrC and then edit).
    pub fn revert_to_history_step(&mut self, photo_id: i64, step_id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        let settings: Option<String> = tx
            .query_row(
                "SELECT settings FROM develop_history WHERE id = ?1 AND photo_id = ?2",
                params![step_id, photo_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| anyhow::anyhow!("history step not found"))?;
        store(&tx, photo_id, settings.as_deref())?;
        tx.execute(
            "DELETE FROM develop_history WHERE photo_id = ?1 AND id > ?2",
            params![photo_id, step_id],
        )?;
        tx.execute("UPDATE photos SET metadata_dirty = 1 WHERE id = ?1", [photo_id])?;
        tx.commit()?;
        Ok(())
    }

    /// The photo's History, oldest first.
    pub fn develop_history(&self, photo_id: i64) -> Result<Vec<HistoryStep>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, label, settings, created_at FROM develop_history WHERE photo_id = ?1 ORDER BY id",
        )?;
        let steps = stmt.query_map([photo_id], |r| {
            Ok(HistoryStep { id: r.get(0)?, label: r.get(1)?, settings: r.get(2)?, created_at: r.get(3)? })
        })?;
        Ok(steps.collect::<rusqlite::Result<_>>()?)
    }

    /// LrC's History ▸ Clear: forget the steps, keep the current settings.
    pub fn clear_develop_history(&mut self, photo_id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM develop_history WHERE photo_id = ?1", [photo_id])?;
        Ok(())
    }

    /// The photo's Snapshots, by name.
    pub fn snapshots(&self, photo_id: i64) -> Result<Vec<Snapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, settings, created_at FROM develop_snapshots WHERE photo_id = ?1
             ORDER BY name COLLATE NOCASE, id",
        )?;
        let rows = stmt.query_map([photo_id], |r| {
            Ok(Snapshot { id: r.get(0)?, name: r.get(1)?, settings: r.get(2)?, created_at: r.get(3)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Keep `settings` as a Snapshot named `name`; returns its id.
    pub fn add_snapshot(&mut self, photo_id: i64, name: &str, settings: Option<&str>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO develop_snapshots (photo_id, name, settings, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![photo_id, name, settings, now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_snapshot(&mut self, id: i64, name: &str) -> Result<()> {
        self.conn.execute("UPDATE develop_snapshots SET name = ?2 WHERE id = ?1", params![id, name])?;
        Ok(())
    }

    /// LrC's Update with Current Settings.
    pub fn update_snapshot(&mut self, id: i64, settings: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE develop_snapshots SET settings = ?2, created_at = ?3 WHERE id = ?1",
            params![id, settings, now()],
        )?;
        Ok(())
    }

    pub fn delete_snapshot(&mut self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM develop_snapshots WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Photos with Develop settings (for the grid's "edited" badge).
    pub fn developed_photo_ids(&self) -> Result<HashSet<i64>> {
        let mut stmt = self.conn.prepare("SELECT photo_id FROM develop")?;
        let ids = stmt.query_map([], |r| r.get(0))?;
        Ok(ids.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use crate::{Catalog, Source};

    fn catalog_with_photo() -> (tempfile::TempDir, Catalog, i64) {
        let tmp = tempfile::tempdir().unwrap();
        let photos = tmp.path().join("photos");
        std::fs::create_dir(&photos).unwrap();
        std::fs::write(photos.join("a.jpg"), b"x").unwrap();
        let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
        cat.sync_folder(&photos).unwrap();
        let id = cat.photos(Source::AllPhotographs).unwrap()[0].id;
        (tmp, cat, id)
    }

    #[test]
    fn settings_history_and_revert() {
        let (_tmp, mut cat, id) = catalog_with_photo();
        assert_eq!(cat.develop_settings(id).unwrap(), None);
        cat.set_develop_settings(id, Some(r#"{"exposure":0.5}"#), "Exposure +0.50").unwrap();
        cat.set_develop_settings(id, Some(r#"{"exposure":0.5,"contrast":10}"#), "Contrast +10").unwrap();
        assert_eq!(cat.develop_settings(id).unwrap().as_deref(), Some(r#"{"exposure":0.5,"contrast":10}"#));
        assert!(cat.developed_photo_ids().unwrap().contains(&id));
        assert!(cat.dirty_photo_ids().unwrap().contains(&id));

        let h = cat.develop_history(id).unwrap();
        let labels: Vec<&str> = h.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Import", "Exposure +0.50", "Contrast +10"]);
        assert_eq!(h[0].settings, None);

        cat.revert_to_history_step(id, h[1].id).unwrap();
        assert_eq!(cat.develop_settings(id).unwrap().as_deref(), Some(r#"{"exposure":0.5}"#));
        assert_eq!(cat.develop_history(id).unwrap().len(), 2);

        cat.revert_to_history_step(id, h[0].id).unwrap();
        assert_eq!(cat.develop_settings(id).unwrap(), None);
        assert!(cat.developed_photo_ids().unwrap().is_empty());

        cat.set_develop_settings(id, Some("{}"), "Reset").unwrap();
        cat.clear_develop_history(id).unwrap();
        assert!(cat.develop_history(id).unwrap().is_empty());
        assert_eq!(cat.develop_settings(id).unwrap().as_deref(), Some("{}"));
    }

    #[test]
    fn settings_survive_a_lost_catalog_through_the_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let photos = tmp.path().join("photos");
        std::fs::create_dir(&photos).unwrap();
        std::fs::write(photos.join("a.NEF"), b"x").unwrap();
        let none: crate::OrientationReader = &|_, _| None;
        let json = r#"{"exposure":0.5,"wb":"Tungsten"}"#;

        let mut cat = Catalog::create(tmp.path().join("one.plcat")).unwrap();
        cat.sync_folder(&photos).unwrap();
        let id = cat.photos(Source::AllPhotographs).unwrap()[0].id;
        cat.set_develop_settings(id, Some(json), "Exposure +0.50").unwrap();
        cat.write_xmp(id, none).unwrap();
        assert!(cat.dirty_photo_ids().unwrap().is_empty());

        // A new catalog picks the settings up when it adds the folder.
        let mut other = Catalog::create(tmp.path().join("two.plcat")).unwrap();
        other.sync_folder(&photos).unwrap();
        let id2 = other.photos(Source::AllPhotographs).unwrap()[0].id;
        assert_eq!(other.develop_settings(id2).unwrap().as_deref(), Some(json));

        // A sidecar without them (rewritten by another app) leaves the catalog's alone.
        std::fs::write(photos.join("a.xmp"), "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\"/></rdf:RDF></x:xmpmeta>").unwrap();
        assert!(cat.read_xmp(id, none).unwrap());
        assert_eq!(cat.develop_settings(id).unwrap().as_deref(), Some(json));
    }

    #[test]
    fn snapshots_are_kept_renamed_updated_and_deleted() {
        let (_tmp, mut cat, id) = catalog_with_photo();
        let a = cat.add_snapshot(id, "Before", None).unwrap();
        let b = cat.add_snapshot(id, "after edits", Some(r#"{"exposure":1.0}"#)).unwrap();
        let names: Vec<String> = cat.snapshots(id).unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["after edits", "Before"]);
        cat.rename_snapshot(b, "Bright").unwrap();
        cat.update_snapshot(a, Some(r#"{"contrast":5}"#)).unwrap();
        let s = cat.snapshots(id).unwrap();
        assert_eq!((s[0].name.as_str(), s[0].settings.as_deref()), ("Before", Some(r#"{"contrast":5}"#)));
        assert_eq!(s[1].name, "Bright");
        cat.delete_snapshot(a).unwrap();
        assert_eq!(cat.snapshots(id).unwrap().len(), 1);
        // A virtual copy has none of its own.
        let copy = cat.create_virtual_copy(id).unwrap();
        assert!(cat.snapshots(copy).unwrap().is_empty());
    }

    #[test]
    fn virtual_copies_start_with_the_masters_settings() {
        let (_tmp, mut cat, id) = catalog_with_photo();
        cat.set_develop_settings(id, Some(r#"{"exposure":1.0}"#), "Exposure +1.00").unwrap();
        let copy = cat.create_virtual_copy(id).unwrap();
        assert_eq!(cat.develop_settings(copy).unwrap().as_deref(), Some(r#"{"exposure":1.0}"#));
        cat.set_develop_settings(copy, Some(r#"{"exposure":-1.0}"#), "Exposure -1.00").unwrap();
        assert_eq!(cat.develop_settings(id).unwrap().as_deref(), Some(r#"{"exposure":1.0}"#));
        cat.delete_virtual_copies(&[copy]).unwrap();
        assert!(!cat.developed_photo_ids().unwrap().contains(&copy));
    }
}
