//! Collections (and sets, and smart collections), the Quick Collection and
//! catalog settings: LrC's ways of grouping photos apart from folders.

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};

use crate::catalog::{Catalog, Source};
use crate::query::{Criterion, SmartRules};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollectionKind {
    /// A collection set: holds other collections, no photos of its own.
    Set,
    Collection,
    Smart,
}

impl CollectionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Collection => "collection",
            Self::Smart => "smart",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "set" => Self::Set,
            "smart" => Self::Smart,
            _ => Self::Collection,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub kind: CollectionKind,
    /// Smart collections only.
    pub rules: Option<SmartRules>,
    /// Photos in it (evaluated for smart collections; 0 for sets).
    pub count: i64,
}

fn validate_name(name: &str) -> Result<&str> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "the name is empty");
    Ok(name)
}

impl Catalog {
    /// All collections and sets, sorted by name, with photo counts.
    pub fn collections(&self) -> Result<Vec<Collection>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.name, c.parent_id, c.kind, c.smart_rules,
                    (SELECT COUNT(*) FROM collection_photos cp WHERE cp.collection_id = c.id)
               FROM collections c ORDER BY c.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, parent_id, kind, rules, count) = row?;
            let kind = CollectionKind::parse(&kind);
            let rules: Option<SmartRules> = rules.and_then(|r| serde_json::from_str(&r).ok());
            let count = match kind {
                CollectionKind::Smart => {
                    self.count_filtered(Source::Collection { id }, &Criterion::AllOf(vec![]))?
                }
                CollectionKind::Set => 0,
                CollectionKind::Collection => count,
            };
            out.push(Collection { id, name, parent_id, kind, rules, count });
        }
        Ok(out)
    }

    /// Create a collection, smart collection or set, optionally inside a set.
    pub fn create_collection(
        &mut self,
        parent: Option<i64>,
        name: &str,
        kind: CollectionKind,
        rules: Option<&SmartRules>,
    ) -> Result<i64> {
        let name = validate_name(name)?;
        if let Some(p) = parent {
            let parent_kind: String = self
                .conn
                .query_row("SELECT kind FROM collections WHERE id = ?1", [p], |r| r.get(0))
                .optional()?
                .context("collection set not found")?;
            anyhow::ensure!(parent_kind == "set", "collections can only be created inside a collection set");
        }
        self.ensure_unique_name(parent, name, None)?;
        let rules = match kind {
            CollectionKind::Smart => Some(serde_json::to_string(&rules.cloned().unwrap_or_default())?),
            _ => None,
        };
        self.conn.execute(
            "INSERT INTO collections (name, parent_id, kind, smart_rules) VALUES (?1, ?2, ?3, ?4)",
            params![name, parent, kind.as_str(), rules],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn ensure_unique_name(&self, parent: Option<i64>, name: &str, except: Option<i64>) -> Result<()> {
        let clash: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM collections WHERE parent_id IS ?1 AND name = ?2 COLLATE NOCASE AND id IS NOT ?3",
                params![parent, name, except],
                |r| r.get(0),
            )
            .optional()?;
        anyhow::ensure!(clash.is_none(), "\"{name}\" already exists here");
        Ok(())
    }

    pub fn rename_collection(&mut self, id: i64, name: &str) -> Result<()> {
        let name = validate_name(name)?;
        let parent: Option<i64> = self
            .conn
            .query_row("SELECT parent_id FROM collections WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .context("collection not found")?;
        self.ensure_unique_name(parent, name, Some(id))?;
        self.conn.execute("UPDATE collections SET name = ?1 WHERE id = ?2", params![name, id])?;
        Ok(())
    }

    /// Delete a collection (or a set with everything in it). Photos are untouched.
    pub fn delete_collection(&mut self, id: i64) -> Result<()> {
        let n = self.conn.execute("DELETE FROM collections WHERE id = ?1", [id])?;
        anyhow::ensure!(n == 1, "collection not found");
        Ok(())
    }

    pub fn set_smart_rules(&mut self, id: i64, rules: &SmartRules) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE collections SET smart_rules = ?1 WHERE id = ?2 AND kind = 'smart'",
            params![serde_json::to_string(rules)?, id],
        )?;
        anyhow::ensure!(n == 1, "smart collection not found");
        Ok(())
    }

    fn ensure_plain_collection(&self, id: i64) -> Result<()> {
        let kind: String = self
            .conn
            .query_row("SELECT kind FROM collections WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .context("collection not found")?;
        anyhow::ensure!(kind == "collection", "photos can only be added to regular collections");
        Ok(())
    }

    /// Add photos to a regular collection (already present ones are skipped).
    pub fn add_to_collection(&mut self, id: i64, photo_ids: &[i64]) -> Result<usize> {
        self.ensure_plain_collection(id)?;
        let tx = self.conn.transaction()?;
        let mut added = 0;
        for p in photo_ids {
            added += tx.execute(
                "INSERT OR IGNORE INTO collection_photos (collection_id, photo_id) VALUES (?1, ?2)",
                params![id, p],
            )?;
        }
        tx.commit()?;
        Ok(added)
    }

    pub fn remove_from_collection(&mut self, id: i64, photo_ids: &[i64]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut removed = 0;
        for p in photo_ids {
            removed += tx.execute(
                "DELETE FROM collection_photos WHERE collection_id = ?1 AND photo_id = ?2",
                params![id, p],
            )?;
        }
        tx.commit()?;
        Ok(removed)
    }

    // ---- Quick Collection (B) ----

    /// LrC's B: if every photo is in the Quick Collection, remove them; otherwise add
    /// them all. Returns true if they were added.
    pub fn toggle_quick_collection(&mut self, photo_ids: &[i64]) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let mut all_in = true;
        for p in photo_ids {
            let inside: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM quick_collection WHERE photo_id = ?1)",
                [p],
                |r| r.get(0),
            )?;
            all_in &= inside;
        }
        for p in photo_ids {
            if all_in {
                tx.execute("DELETE FROM quick_collection WHERE photo_id = ?1", [p])?;
            } else {
                tx.execute("INSERT OR IGNORE INTO quick_collection (photo_id) VALUES (?1)", [p])?;
            }
        }
        tx.commit()?;
        Ok(!all_in)
    }

    /// Add photos to the Quick Collection (dragging onto it never removes).
    pub fn add_to_quick_collection(&mut self, photo_ids: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for p in photo_ids {
            tx.execute("INSERT OR IGNORE INTO quick_collection (photo_id) VALUES (?1)", [p])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn clear_quick_collection(&mut self) -> Result<()> {
        self.conn.execute("DELETE FROM quick_collection", [])?;
        Ok(())
    }

    pub fn quick_collection_count(&self) -> Result<i64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM quick_collection", [], |r| r.get(0))?)
    }

    // ---- Settings ----

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// LrC Preferences: "Treat JPEG files next to raw files as separate photos" (off by
    /// default: the JPEG becomes part of the raw photo, shown as "NEF+JPG").
    pub fn separate_raw_jpeg(&self) -> Result<bool> {
        Ok(self.setting(SEPARATE_RAW_JPEG)?.as_deref() == Some("1"))
    }

    pub fn set_separate_raw_jpeg(&mut self, separate: bool) -> Result<()> {
        self.set_setting(SEPARATE_RAW_JPEG, if separate { "1" } else { "0" })
    }
}

const SEPARATE_RAW_JPEG: &str = "separate_raw_jpeg";
