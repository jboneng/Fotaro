//! LrC's Import dialog behind the scenes: find photos on a card or in a folder,
//! skip ones already in the catalog, then Add them where they are, or Copy/Move them
//! into dated destination folders with optional renaming, and add them to the catalog.
//!
//! Safety rules: originals are only deleted (Move) after their copy is verified;
//! files are written under a temporary name and renamed into place; existing files
//! are never overwritten (names get `-2`, `-3`...).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::catalog::Catalog;
use crate::scan::{FileKind, file_kind};
use crate::sync::{MetadataReader, SyncOptions, ensure_folder, folder_id, is_hidden, unix_ms};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ImportMode {
    /// Copy into the destination; originals stay (LrC's default for cards).
    #[default]
    Copy,
    /// Copy into the destination, then delete the originals.
    Move,
    /// Add photos where they are.
    Add,
}

/// Destination folder layout for "Organize: By date" (LrC's date formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DateFolders {
    /// `2026/2026-11-08`
    #[default]
    YearThenDate,
    /// `2026/11/08`
    YearMonthDay,
    /// `2026/2026-11`
    YearThenMonth,
    /// `2026-11-08`
    Date,
    /// `2026`
    Year,
    /// No date folders ("Into one folder").
    None,
}

impl DateFolders {
    pub const ALL: [DateFolders; 6] =
        [Self::YearThenDate, Self::YearMonthDay, Self::YearThenMonth, Self::Date, Self::Year, Self::None];

    pub fn label(self) -> &'static str {
        match self {
            Self::YearThenDate => "2026/2026-11-08",
            Self::YearMonthDay => "2026/11/08",
            Self::YearThenMonth => "2026/2026-11",
            Self::Date => "2026-11-08",
            Self::Year => "2026",
            Self::None => "Into one folder",
        }
    }

    fn folders(self, t: &DateParts) -> Vec<String> {
        let (y, m, d) = (format!("{:04}", t.year), format!("{:02}", t.month), format!("{:02}", t.day));
        match self {
            Self::YearThenDate => vec![y.clone(), format!("{y}-{m}-{d}")],
            Self::YearMonthDay => vec![y, m, d],
            Self::YearThenMonth => vec![y.clone(), format!("{y}-{m}")],
            Self::Date => vec![format!("{y}-{m}-{d}")],
            Self::Year => vec![y],
            Self::None => vec![],
        }
    }
}

/// LrC-style file renaming presets; `Custom` uses the template text as typed.
pub const RENAME_PRESETS: &[(&str, &str)] = &[
    ("Date - Filename", "{YYYY}{MM}{DD}-{Filename}"),
    ("Custom Name - Sequence", "{Text}-{Seq:3}"),
    ("Date - Sequence", "{YYYY}{MM}{DD}-{Seq:4}"),
    ("Custom Name - Original File Number", "{Text}-{OrigNum}"),
    ("Date and Time - Filename", "{YYYY}{MM}{DD}-{hh}{mm}{ss}-{Filename}"),
];

/// Tokens a renaming template can use (for the UI's help text).
pub const RENAME_TOKENS: &str =
    "{Filename} {Text} {Seq} {Seq:3} {OrigNum} {YYYY} {YY} {MM} {DD} {hh} {mm} {ss}";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportSettings {
    pub mode: ImportMode,
    /// Copy/Move: where files go.
    pub destination: PathBuf,
    /// Copy/Move: an extra folder inside the destination ("Into Subfolder").
    pub subfolder: Option<String>,
    pub date_folders: DateFolders,
    /// Copy/Move: renaming template (`None`: keep file names).
    pub rename: Option<String>,
    /// `{Text}` in the template.
    pub custom_text: String,
    /// First `{Seq}` number.
    pub start_number: u32,
    /// "Don't Import Suspected Duplicates".
    pub skip_duplicates: bool,
    /// Keyword paths to apply to every imported photo.
    pub keywords: Vec<Vec<String>>,
}

impl Default for ImportSettings {
    fn default() -> Self {
        ImportSettings {
            mode: ImportMode::Copy,
            destination: PathBuf::new(),
            subfolder: None,
            date_folders: DateFolders::YearThenDate,
            rename: None,
            custom_text: String::new(),
            start_number: 1,
            skip_duplicates: true,
            keywords: Vec::new(),
        }
    }
}

/// A file found in the import source.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    /// Naive-local capture time (seconds), from the reader or the file date.
    pub capture_time: Option<i64>,
    /// Same name and size as a file already in the catalog (LrC's "suspected duplicate").
    pub duplicate: bool,
}

/// Where one file will go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub source: PathBuf,
    pub dest: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub imported: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub photo_ids: Vec<i64>,
    /// Destination folders (Copy/Move), or the folders photos were added from.
    pub folders: Vec<PathBuf>,
    pub cancelled: bool,
}

/// Calendar fields of a naive-local timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateParts {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl DateParts {
    /// From seconds since 1970 (days-from-civil inverse, Howard Hinnant's algorithm).
    pub fn from_timestamp(secs: i64) -> Self {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = yoe + era * 400 + i64::from(month <= 2);
        DateParts {
            year,
            month,
            day,
            hour: (rem / 3600) as u32,
            minute: (rem % 3600 / 60) as u32,
            second: (rem % 60) as u32,
        }
    }
}

struct RenameContext<'a> {
    stem: &'a str,
    text: &'a str,
    seq: u32,
    date: DateParts,
}

/// Expand a renaming template. Unknown tokens are an error, so typos show up in the
/// dialog's preview instead of in file names.
pub fn render_template(
    template: &str,
    stem: &str,
    text: &str,
    seq: u32,
    capture_time: i64,
) -> Result<String> {
    render(template, &RenameContext { stem, text, seq, date: DateParts::from_timestamp(capture_time) })
}

fn render(template: &str, cx: &RenameContext) -> Result<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').map(|c| open + c).context("a { has no matching }")?;
        let token = &rest[open + 1..close];
        let (name, arg) = token.split_once(':').unwrap_or((token, ""));
        let d = &cx.date;
        let value = match name {
            "Filename" => cx.stem.to_owned(),
            "Text" => cx.text.to_owned(),
            "Seq" => {
                let width: usize =
                    if arg.is_empty() { 1 } else { arg.parse().context("{Seq:N} needs a number")? };
                format!("{:0width$}", cx.seq)
            }
            "OrigNum" => {
                let digits: String = cx.stem.chars().rev().take_while(char::is_ascii_digit).collect();
                digits.chars().rev().collect()
            }
            "YYYY" => format!("{:04}", d.year),
            "YY" => format!("{:02}", d.year.rem_euclid(100)),
            "MM" => format!("{:02}", d.month),
            "DD" => format!("{:02}", d.day),
            "hh" => format!("{:02}", d.hour),
            "mm" => format!("{:02}", d.minute),
            "ss" => format!("{:02}", d.second),
            other => anyhow::bail!("unknown token {{{other}}}"),
        };
        out.push_str(&value);
        rest = &rest[close + 1..];
    }
    anyhow::ensure!(!rest.contains('}'), "a }} has no matching {{");
    out.push_str(rest);
    // Characters no common file system accepts.
    let cleaned: String =
        out.chars().map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_owned();
    anyhow::ensure!(!cleaned.is_empty(), "the template gives an empty file name");
    Ok(cleaned)
}

fn stem(p: &Path) -> String {
    p.file_stem().unwrap_or_default().to_string_lossy().into_owned()
}

/// Work out every file's destination. Photos sharing a base name in the same source
/// folder (RAW+JPEG) get the same sequence number and the same collision suffix, so
/// they stay a pair. Existing files are never targeted.
pub fn plan(candidates: &[Candidate], s: &ImportSettings) -> Result<Vec<Planned>> {
    if s.mode == ImportMode::Add {
        return Ok(candidates
            .iter()
            .map(|c| Planned { source: c.path.clone(), dest: c.path.clone() })
            .collect());
    }
    anyhow::ensure!(!s.destination.as_os_str().is_empty(), "choose a destination folder");
    let mut order: Vec<&Candidate> = candidates.iter().collect();
    order.sort_by(|a, b| a.capture_time.cmp(&b.capture_time).then_with(|| a.path.cmp(&b.path)));

    // Groups of files that belong together (same folder and base name), in order.
    let mut groups: Vec<Vec<&Candidate>> = Vec::new();
    let mut index: HashMap<(PathBuf, String), usize> = HashMap::new();
    for c in order {
        let key = (c.path.parent().unwrap_or(Path::new("")).to_path_buf(), stem(&c.path).to_lowercase());
        let i = *index.entry(key).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[i].push(c);
    }

    let mut taken: HashSet<String> = HashSet::new(); // lowercase: Windows/macOS ignore case
    let mut planned = Vec::new();
    for (g, members) in groups.iter().enumerate() {
        let first = members[0];
        let time = first.capture_time.unwrap_or(0);
        let mut dir = s.destination.clone();
        if let Some(sub) = s.subfolder.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            dir.push(crate::folders::validate_folder_name(sub)?);
        }
        for part in s.date_folders.folders(&DateParts::from_timestamp(time)) {
            dir.push(part);
        }
        let base = match &s.rename {
            Some(t) => render(
                t,
                &RenameContext {
                    stem: &stem(&first.path),
                    text: s.custom_text.trim(),
                    seq: s.start_number + g as u32,
                    date: DateParts::from_timestamp(time),
                },
            )?,
            None => stem(&first.path),
        };
        let name_for = |m: &Candidate, suffix: u32| {
            let ext = m.path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            if suffix < 2 { format!("{base}{ext}") } else { format!("{base}-{suffix}{ext}") }
        };
        let mut suffix = 1;
        loop {
            let free = members.iter().all(|m| {
                let p = dir.join(name_for(m, suffix));
                !taken.contains(&p.to_string_lossy().to_lowercase()) && !p.exists()
            });
            if free {
                break;
            }
            suffix += 1;
        }
        for m in members {
            let dest = dir.join(name_for(m, suffix));
            taken.insert(dest.to_string_lossy().to_lowercase());
            planned.push(Planned { source: m.path.clone(), dest });
        }
    }
    Ok(planned)
}

/// Find importable photos under `root`, read their capture times on all cores, and
/// flag suspected duplicates against `known` (from [`Catalog::duplicate_keys`]).
pub fn scan_source(
    root: &Path,
    recursive: bool,
    reader: Option<MetadataReader>,
    known: &HashSet<(String, u64)>,
) -> Result<Vec<Candidate>> {
    let root = dunce::canonicalize(root).with_context(|| format!("{} is not available", root.display()))?;
    let mut found = Vec::new();
    let walker = WalkDir::new(&root)
        .max_depth(if recursive { usize::MAX } else { 1 })
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(e.file_name()));
    for entry in walker.flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(kind) = file_kind(entry.path()) else { continue };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let mtime = entry.metadata().ok().and_then(|m| m.modified().ok()).map(|t| unix_ms(t) / 1000);
        let name = entry.file_name().to_string_lossy().to_lowercase();
        found.push(Candidate {
            path: entry.into_path(),
            kind,
            size,
            capture_time: mtime,
            duplicate: known.contains(&(name, size)),
        });
    }
    if let Some(reader) = reader {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let chunk = found.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for part in found.chunks_mut(chunk) {
                scope.spawn(move || {
                    for c in part {
                        if let Some(t) = reader(&c.path, c.kind).and_then(|m| m.capture_time) {
                            c.capture_time = Some(t);
                        }
                    }
                });
            }
        });
    }
    found.sort_by(|a, b| a.capture_time.cmp(&b.capture_time).then_with(|| a.path.cmp(&b.path)));
    Ok(found)
}

/// Copy `from` to `to` via a temporary file, keep its modification time, and check
/// the size before putting it in place.
fn copy_verified(from: &Path, to: &Path) -> Result<()> {
    let tmp = to.with_extension(format!(
        "{}.importing",
        to.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    let written = std::fs::copy(from, &tmp).with_context(|| format!("copying {}", from.display()))?;
    let source_meta = std::fs::metadata(from)?;
    if written != source_meta.len() || std::fs::metadata(&tmp)?.len() != source_meta.len() {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("the copy of {} is incomplete", from.display());
    }
    if let Ok(mtime) = source_meta.modified() {
        let _ = std::fs::File::options().write(true).open(&tmp).and_then(|f| f.set_modified(mtime));
    }
    anyhow::ensure!(!to.exists(), "{} appeared while importing; not overwriting it", to.display());
    std::fs::rename(&tmp, to).with_context(|| format!("placing {}", to.display()))?;
    Ok(())
}

/// Put one file (and its XMP sidecar) at its destination.
fn transfer(p: &Planned, kind: FileKind, mode: ImportMode) -> Result<()> {
    if mode == ImportMode::Add {
        return Ok(());
    }
    if let Some(dir) = p.dest.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let raw = kind == FileKind::Raw;
    let sidecar = xmp::sidecar_path(&p.source, raw);
    let dest_sidecar = xmp::sidecar_path(&p.dest, raw);
    let has_sidecar = sidecar.is_file() && !dest_sidecar.exists();
    match mode {
        ImportMode::Move => {
            // Same drive: a rename is instant and atomic. Otherwise copy, verify, delete.
            if std::fs::rename(&p.source, &p.dest).is_err() {
                copy_verified(&p.source, &p.dest)?;
                std::fs::remove_file(&p.source)
                    .with_context(|| format!("removing {}", p.source.display()))?;
            }
            if has_sidecar && std::fs::rename(&sidecar, &dest_sidecar).is_err() {
                copy_verified(&sidecar, &dest_sidecar)?;
                let _ = std::fs::remove_file(&sidecar);
            }
        }
        ImportMode::Copy => {
            copy_verified(&p.source, &p.dest)?;
            if has_sidecar {
                copy_verified(&sidecar, &dest_sidecar)?;
            }
        }
        ImportMode::Add => {}
    }
    Ok(())
}

impl Catalog {
    /// (lowercase file name, size) of every file in the catalog, for duplicate checks.
    pub fn duplicate_keys(&self) -> Result<HashSet<(String, u64)>> {
        let mut stmt = self.conn.prepare("SELECT name, size FROM files")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?.to_lowercase(), r.get::<_, i64>(1)?.max(0) as u64))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Make sure every folder from `top` down to `leaf` is in the catalog, linked as a
    /// tree (and `top` under its parent if that is in the catalog).
    pub(crate) fn ensure_folder_path(&mut self, top: &Path, leaf: &Path) -> Result<()> {
        let mut chain: Vec<&Path> = leaf.ancestors().take_while(|p| p.starts_with(top)).collect();
        chain.reverse();
        let tx = self.conn.transaction()?;
        for dir in chain {
            let parent = match dir.parent() {
                Some(p) => folder_id(&tx, p)?,
                None => None,
            };
            ensure_folder(&tx, dir, parent)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Run an import: transfer the chosen files, add them to the catalog (all with one
    /// import time) and apply keywords. `progress(done, total)` is called per file;
    /// setting `cancel` stops before the next file (finished files stay imported).
    pub fn import(
        &mut self,
        chosen: &[Candidate],
        settings: &ImportSettings,
        reader: Option<MetadataReader>,
        progress: &dyn Fn(usize, usize),
        cancel: &AtomicBool,
    ) -> Result<ImportReport> {
        let planned = plan(chosen, settings)?;
        let kinds: HashMap<&Path, FileKind> = chosen.iter().map(|c| (c.path.as_path(), c.kind)).collect();
        let mut report = ImportReport::default();
        let mut placed: Vec<PathBuf> = Vec::new();
        for (i, p) in planned.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                report.cancelled = true;
                break;
            }
            progress(i, planned.len());
            let kind = kinds.get(p.source.as_path()).copied().unwrap_or(FileKind::Image);
            match transfer(p, kind, settings.mode) {
                Ok(()) => placed.push(p.dest.clone()),
                Err(e) => {
                    log::warn!("import of {}: {e:#}", p.source.display());
                    report.failed.push((p.source.clone(), format!("{e:#}")));
                }
            }
        }
        progress(planned.len(), planned.len());

        // Add the placed files folder by folder, all as one import.
        let placed: Vec<PathBuf> =
            placed.iter().map(|p| dunce::canonicalize(p).unwrap_or_else(|_| p.clone())).collect();
        let only: HashSet<PathBuf> = placed.iter().cloned().collect();
        let mut dirs: Vec<PathBuf> =
            placed.iter().filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
        dirs.sort();
        dirs.dedup();
        let top = match settings.mode {
            ImportMode::Add => None,
            _ => dunce::canonicalize(&settings.destination).ok(),
        };
        let imported_at = unix_ms(std::time::SystemTime::now()) / 1000;
        for dir in &dirs {
            // Copy/Move: build the tree from the destination down (2026 > 2026-11-08).
            self.ensure_folder_path(top.as_deref().filter(|t| dir.starts_with(t)).unwrap_or(dir), dir)?;
            let opts = SyncOptions {
                reader,
                only: Some(&only),
                recursive: false,
                imported_at: Some(imported_at),
                ..Default::default()
            };
            self.sync_folder_opts(dir, opts)?;
        }
        report.folders = dirs;
        report.photo_ids = self.photo_ids_for_paths(&placed)?;
        report.imported = report.photo_ids.len();

        if !settings.keywords.is_empty() && !report.photo_ids.is_empty() {
            for path in &settings.keywords {
                let kw = self.ensure_keyword(path)?;
                self.add_keyword(&report.photo_ids, kw)?;
            }
        }
        Ok(report)
    }

    /// Photos whose master file is one of `paths` (paired JPEGs map to nothing).
    pub fn photo_ids_for_paths(&self, paths: &[PathBuf]) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.id FROM photos p JOIN files f ON f.id = p.master_file_id JOIN folders fo ON fo.id = f.folder_id
              WHERE fo.path = ?1 AND f.name = ?2 AND p.virtual_of IS NULL",
        )?;
        let mut ids = Vec::new();
        for p in paths {
            let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else { continue };
            let found: Option<i64> = rusqlite::OptionalExtension::optional(
                stmt.query_row(rusqlite::params![dir.to_string_lossy(), name.to_string_lossy()], |r| {
                    r.get(0)
                }),
            )?;
            ids.extend(found);
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests;
