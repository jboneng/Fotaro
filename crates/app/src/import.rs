//! State of the Import dialog (LrC: File ▸ Import Photos…). The work itself is in
//! `catalog::import`; scanning, thumbnails and the import run as background jobs.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use catalog::{Candidate, ImportSettings};
use serde::{Deserialize, Serialize};

use crate::platform::Volume;
use crate::textures::TextureCache;

/// Import choices remembered between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportPrefs {
    pub settings: ImportSettings,
    pub include_subfolders: bool,
    pub rename_on: bool,
    pub template: String,
    /// Keywords to apply, as typed (LrC syntax).
    pub keywords: String,
    pub subfolder_on: bool,
    pub subfolder: String,
    pub last_source: Option<PathBuf>,
}

impl Default for ImportPrefs {
    fn default() -> Self {
        ImportPrefs {
            settings: ImportSettings::default(),
            include_subfolders: true,
            rename_on: false,
            template: catalog::RENAME_PRESETS[0].1.to_owned(),
            keywords: String::new(),
            subfolder_on: false,
            subfolder: String::new(),
            last_source: None,
        }
    }
}

impl ImportPrefs {
    /// The settings for an import, from what the dialog shows.
    pub fn effective(&self) -> ImportSettings {
        let mut s = self.settings.clone();
        s.rename = self.rename_on.then(|| self.template.clone());
        s.subfolder = self.subfolder_on.then(|| self.subfolder.clone()).filter(|v| !v.trim().is_empty());
        s.keywords = catalog::parse_keyword_input(&self.keywords);
        s
    }
}

pub struct ImportDialog {
    pub source: Option<PathBuf>,
    /// Bumped per scan, so late answers for an old source are ignored.
    pub generation: u64,
    /// `None` while scanning.
    pub candidates: Option<Vec<Candidate>>,
    pub checked: Vec<bool>,
    pub error: Option<String>,
    pub thumbs: TextureCache,
    /// Filled by a background thread (probing drives can be slow).
    pub volumes: Arc<Mutex<Option<Vec<Volume>>>>,
}

impl ImportDialog {
    pub fn new() -> Self {
        let volumes = Arc::new(Mutex::new(None));
        let fill = Arc::clone(&volumes);
        std::thread::spawn(move || {
            let found = crate::platform::volumes();
            *fill.lock().unwrap() = Some(found);
        });
        ImportDialog {
            source: None,
            generation: 0,
            candidates: None,
            checked: Vec::new(),
            error: None,
            thumbs: TextureCache::new("import", 600),
            volumes,
        }
    }

    /// Thumbnail cache key: the scan generation plus the candidate's index.
    pub fn thumb_key(&self, index: usize) -> i64 {
        ((self.generation as i64) << 24) | index as i64
    }

    /// Photos that will be imported: checked, and not skipped as duplicates.
    pub fn chosen(&self, skip_duplicates: bool) -> Vec<Candidate> {
        let Some(c) = &self.candidates else { return Vec::new() };
        c.iter()
            .zip(&self.checked)
            .filter(|(c, on)| **on && !(skip_duplicates && c.duplicate))
            .map(|(c, _)| c.clone())
            .collect()
    }
}
