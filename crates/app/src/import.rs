//! State of the Import dialog (LrC: File ▸ Import Photos…). The work itself is in
//! `catalog::import`; scanning, thumbnails and the import run as background jobs.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use catalog::{Candidate, ImportSettings};
use eframe::egui;
use serde::{Deserialize, Serialize};

use crate::platform::{FolderPick, Volume};
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
    /// `None` while scanning. Shared: the dialog reads the list every frame.
    pub candidates: Option<Arc<Vec<Candidate>>>,
    pub checked: Vec<bool>,
    pub error: Option<String>,
    pub thumbs: TextureCache,
    /// Filled by a background thread (probing drives can be slow).
    pub volumes: Arc<Mutex<Option<Vec<Volume>>>>,
    /// The last import source if it still exists; unset while that is being checked.
    /// Checked on a background thread: on a busy network share (or an automount that
    /// has to mount first) even `is_dir` can take seconds, and the UI must not wait.
    pub recent: Arc<OnceLock<Option<PathBuf>>>,
    /// The recent source has been offered as the starting source (once per dialog).
    pub recent_started: bool,
    /// An open folder picker: for the source, or for the destination.
    pub picking_source: Option<FolderPick>,
    pub picking_destination: Option<FolderPick>,
}

impl ImportDialog {
    pub fn new(ctx: &egui::Context, last_source: Option<PathBuf>) -> Self {
        let volumes = Arc::new(Mutex::new(None));
        let fill = Arc::clone(&volumes);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let found = crate::platform::volumes();
            *fill.lock().unwrap() = Some(found);
            repaint.request_repaint();
        });
        let recent = Arc::new(OnceLock::new());
        let fill = Arc::clone(&recent);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = fill.set(last_source.filter(|p| p.is_dir()));
            repaint.request_repaint();
        });
        ImportDialog {
            source: None,
            generation: 0,
            candidates: None,
            checked: Vec::new(),
            error: None,
            thumbs: TextureCache::new("import", 600),
            volumes,
            recent,
            recent_started: false,
            picking_source: None,
            picking_destination: None,
        }
    }

    /// Thumbnail cache key: the scan generation plus the candidate's index.
    pub fn thumb_key(&self, index: usize) -> i64 {
        ((self.generation as i64) << 24) | index as i64
    }

    /// Photos that will be imported: checked, and not skipped as duplicates.
    fn chosen_iter(&self, skip_duplicates: bool) -> impl Iterator<Item = &Candidate> {
        let all = self.candidates.as_deref().map(Vec::as_slice).unwrap_or_default();
        all.iter()
            .zip(&self.checked)
            .filter(move |(c, on)| **on && !(skip_duplicates && c.duplicate))
            .map(|(c, _)| c)
    }

    /// The photos that will be imported (copied out: for starting the import).
    pub fn chosen(&self, skip_duplicates: bool) -> Vec<Candidate> {
        self.chosen_iter(skip_duplicates).cloned().collect()
    }

    /// What will be imported, for the dialog's bottom line (drawn every frame, so
    /// nothing is copied): how many photos, their bytes, and the first of them.
    pub fn chosen_totals(&self, skip_duplicates: bool) -> (usize, u64, Option<&Candidate>) {
        let mut first = None;
        let (mut count, mut bytes) = (0, 0);
        for c in self.chosen_iter(skip_duplicates) {
            first.get_or_insert(c);
            count += 1;
            bytes += c.size;
        }
        (count, bytes, first)
    }
}
