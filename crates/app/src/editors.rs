//! Edit In (LrC Photo ▸ Edit In): open originals in an external editor, then find the
//! files it saves and bring them into the catalog, stacked with their originals.
//!
//! - [`negpy`]: told on its command line where to export and how to name the result
//!   (`<original>_NegPy.tif` next to the original); found by name.
//! - [`luminar`]: opened like its Lightroom plug-in does, with the exact result path
//!   (`<original>_LuminarNeo-edit.tif`); one photo at a time.
//!
//! Both editors are detected (an installed release, or for NegPy a source checkout),
//! or taken from wherever the user points the External Editing settings.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

pub mod luminar;
pub mod negpy;

pub use luminar::Luminar;
pub use negpy::*;

/// The external editors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Editor {
    #[default]
    NegPy,
    Luminar,
}

impl Editor {
    pub const ALL: [Editor; 2] = [Self::NegPy, Self::Luminar];

    pub fn label(self) -> &'static str {
        match self {
            Self::NegPy => "NegPy",
            Self::Luminar => "Luminar Neo",
        }
    }

    /// The other one (LrC's "additional external editor").
    pub fn other(self) -> Self {
        match self {
            Self::NegPy => Self::Luminar,
            Self::Luminar => Self::NegPy,
        }
    }
}

/// LrC's Preferences ▸ External Editing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditInPrefs {
    /// Ctrl+E opens this one; Ctrl+Alt+E the other.
    pub primary: Editor,
    /// A NegPy source checkout, an install folder or the program itself. `None`: detect.
    pub negpy_location: Option<PathBuf>,
    /// NegPy's export: format, bit depth, colour space, JPEG quality.
    pub format: EditFormat,
    pub sixteen_bit: bool,
    pub color_space: EditColorSpace,
    pub quality: u8,
    /// Luminar Neo's program or install folder. `None`: detect.
    pub luminar_location: Option<PathBuf>,
}

impl Default for EditInPrefs {
    fn default() -> Self {
        EditInPrefs {
            primary: Editor::NegPy,
            negpy_location: None,
            format: EditFormat::Tiff,
            sixteen_bit: true,
            color_space: EditColorSpace::AdobeRgb,
            quality: 92,
            luminar_location: None,
        }
    }
}

/// A detected editor, ready to plan launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    NegPy(NegPy),
    Luminar(Luminar),
}

impl Found {
    pub fn describe(&self) -> String {
        match self {
            Self::NegPy(n) => n.describe(),
            Self::Luminar(l) => l.describe(),
        }
    }

    /// The launches that open `originals` (photo id, file).
    pub fn launches(&self, originals: &[(i64, PathBuf)], prefs: &EditInPrefs) -> Vec<Launch> {
        match self {
            Self::NegPy(n) => n.launches(originals, prefs),
            Self::Luminar(l) => l.launches(originals),
        }
    }
}

/// Look for `editor` (where the settings say, else in the usual places).
pub fn find(editor: Editor, prefs: &EditInPrefs) -> Result<Found, String> {
    match editor {
        Editor::NegPy => negpy::detect(prefs.negpy_location.as_deref()).map(Found::NegPy),
        Editor::Luminar => luminar::detect(prefs.luminar_location.as_deref()).map(Found::Luminar),
    }
}

// ---------------------------------------------------------------------------------
// Watching for results

/// How an editor names its result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultName {
    /// NegPy: `<stem>_NegPy.<ext>` (or `<stem>_<n>_NegPy` for a half frame).
    NegPy { stem: String },
    /// Exactly this file name (Luminar: the path it was given).
    Exact(PathBuf),
}

impl ResultName {
    pub fn matches(&self, file: &Path) -> bool {
        match self {
            Self::NegPy { stem } => is_result_of(file, stem),
            Self::Exact(name) => {
                file.file_name().map(|n| n.to_string_lossy().to_lowercase())
                    == Some(name.to_string_lossy().to_lowercase())
            }
        }
    }
}

/// An original sent to an editor, whose result is looked for in `dir`.
#[derive(Debug, Clone)]
pub struct EditWatch {
    pub original: i64,
    pub editor: Editor,
    pub dir: PathBuf,
    pub name: ResultName,
    /// Results older than this were there before (with a second of slack for coarse
    /// file system clocks).
    pub since: SystemTime,
    /// When the editor process that was to write it exited (Luminar), if it did.
    pub ended: Option<Instant>,
}

/// A result file as last seen: (size, change time). The change time, not the modified
/// time: NegPy gives its exports the original's modified time, but any write (or date
/// change) moves the change time to now.
pub type FileVersion = (u64, SystemTime);

/// When the file's contents or attributes last changed (`st_ctime`; NTFS `ChangeTime`).
pub fn change_time(path: &Path) -> Option<SystemTime> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(path).ok()?;
        let nanos = u64::try_from(m.ctime()).ok()? * 1_000_000_000 + u64::try_from(m.ctime_nsec()).ok()?;
        Some(SystemTime::UNIX_EPOCH + Duration::from_nanos(nanos))
    }
    #[cfg(windows)]
    {
        windows_change_time(path)
    }
}

#[cfg(windows)]
fn windows_change_time(path: &Path) -> Option<SystemTime> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    #[derive(Default)]
    struct FileBasicInfo {
        creation_time: i64,
        last_access_time: i64,
        last_write_time: i64,
        change_time: i64,
        file_attributes: u32,
    }
    unsafe extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut std::ffi::c_void,
            class: i32,
            info: *mut FileBasicInfo,
            size: u32,
        ) -> i32;
    }
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_BASIC_INFO_CLASS: i32 = 0;
    // Attributes only: works while NegPy is still writing the file.
    let file = std::fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).open(path).ok()?;
    let mut info = FileBasicInfo::default();
    // SAFETY: a valid handle, and a buffer of the size we pass.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FILE_BASIC_INFO_CLASS,
            &mut info,
            size_of::<FileBasicInfo>() as u32,
        )
    };
    // 100 ns ticks since 1601.
    const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;
    let ticks = u64::try_from(info.change_time.checked_sub(UNIX_EPOCH_TICKS)?).ok()?;
    (ok != 0).then(|| SystemTime::UNIX_EPOCH + Duration::from_nanos(ticks * 100))
}

/// Files in the watched folders that look like results, with their versions.
pub fn scan(watches: &[EditWatch]) -> Vec<(i64, PathBuf, FileVersion)> {
    let mut by_dir: BTreeMap<&Path, Vec<&EditWatch>> = BTreeMap::new();
    for w in watches {
        by_dir.entry(&w.dir).or_default().push(w);
    }
    let mut found = Vec::new();
    for (dir, ws) in by_dir {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(w) = ws.iter().find(|w| w.name.matches(&path)) else { continue };
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let Some(changed) = change_time(&path) else { continue };
            if changed + Duration::from_secs(1) >= w.since {
                found.push((w.original, path, (meta.len(), changed)));
            }
        }
    }
    found
}

// ---------------------------------------------------------------------------------
// The session's Edit In state

/// How often the watched folders are looked at.
const CHECK_EVERY: Duration = Duration::from_secs(2);
/// A NegPy launch for another folder waits for the previous one to exit (it handed its
/// files to a running NegPy) or this long (it is the NegPy the next one hands off to).
const LAUNCH_GAP: Duration = Duration::from_secs(8);
/// Originals are watched this long after they were sent.
const WATCH_FOR: Duration = Duration::from_secs(24 * 3600);
/// After Luminar closes, its result is waited for this long before giving up.
const AFTER_EXIT: Duration = Duration::from_secs(15);

/// One editor process to start.
#[derive(Debug, Clone)]
pub struct Launch {
    pub editor: Editor,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// (photo id, folder of its result, the result's name).
    pub results: Vec<(i64, PathBuf, ResultName)>,
    /// The next launch waits until this one exits (Luminar edits one photo at a time).
    pub wait_for_exit: bool,
}

impl Launch {
    fn spawn(&self) -> std::io::Result<Child> {
        let mut c = Command::new(&self.program);
        c.args(&self.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Some(dir) = &self.cwd {
            c.current_dir(dir);
        }
        c.spawn()
    }
}

struct Running {
    child: Child,
    started: Instant,
    editor: Editor,
    wait_for_exit: bool,
    originals: Vec<i64>,
}

/// Edit In state for one catalog session.
#[derive(Default)]
pub struct EditTracker {
    pub watches: Vec<EditWatch>,
    /// Result versions already brought into the catalog.
    handled: std::collections::HashMap<PathBuf, FileVersion>,
    /// Versions seen once; taken when the next look finds them unchanged (written).
    pending: std::collections::HashMap<PathBuf, FileVersion>,
    check_running: bool,
    last_check: Option<Instant>,
    queue: std::collections::VecDeque<Launch>,
    running: Vec<Running>,
}

impl EditTracker {
    pub fn is_idle(&self) -> bool {
        self.watches.is_empty() && self.queue.is_empty() && self.running.is_empty()
    }

    pub fn enqueue(&mut self, launches: Vec<Launch>) {
        self.queue.extend(launches);
    }

    /// Start queued launches whose turn it is, notice editors that closed, and report
    /// what went wrong.
    pub fn tick(&mut self) -> Vec<String> {
        let mut messages = Vec::new();
        let now = Instant::now();
        let mut exited = Vec::new();
        self.running.retain_mut(|r| match r.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                if r.editor == Editor::NegPy && !status.success() {
                    let code = status.code().map_or_else(|| "?".to_owned(), |c| c.to_string());
                    let why = if code == "2" { "it didn't accept the launch options" } else { "it stopped" };
                    messages.push(format!(
                        "NegPy did not open the photos ({why}, exit code {code}; see negpy.log)"
                    ));
                }
                if r.wait_for_exit {
                    exited.extend(r.originals.iter().copied());
                }
                false
            }
            Err(_) => false,
        });
        for w in &mut self.watches {
            if exited.contains(&w.original) && w.ended.is_none() {
                w.ended = Some(now);
            }
        }

        let busy = self.running.iter().any(|r| r.wait_for_exit || now.duration_since(r.started) < LAUNCH_GAP);
        if !busy && let Some(launch) = self.queue.pop_front() {
            match launch.spawn() {
                Ok(child) => {
                    log::info!(
                        "Edit In: started {} for {} photo(s): {:?} {:?}",
                        launch.editor.label(),
                        launch.results.len(),
                        launch.program,
                        launch.args
                    );
                    let since = SystemTime::now();
                    for (id, dir, name) in &launch.results {
                        self.watches.retain(|w| !(w.original == *id && w.editor == launch.editor));
                        self.watches.push(EditWatch {
                            original: *id,
                            editor: launch.editor,
                            dir: dir.clone(),
                            name: name.clone(),
                            since,
                            ended: None,
                        });
                    }
                    self.running.push(Running {
                        child,
                        started: now,
                        editor: launch.editor,
                        wait_for_exit: launch.wait_for_exit,
                        originals: launch.results.iter().map(|(id, _, _)| *id).collect(),
                    });
                }
                Err(e) => messages.push(format!("Could not start {}: {e}", launch.editor.label())),
            }
        }

        // An editor that closed some time ago: its result came (or never will).
        let handled = &self.handled;
        self.watches.retain(|w| {
            let Some(ended) = w.ended else { return true };
            if now.duration_since(ended) < AFTER_EXIT {
                return true;
            }
            let ResultName::Exact(name) = &w.name else { return false };
            if !handled.contains_key(&w.dir.join(name)) {
                messages.push(format!(
                    "{} closed without saving an edit of photo {}",
                    w.editor.label(),
                    name.to_string_lossy().split(luminar::RESULT_SUFFIX).next().unwrap_or_default()
                ));
            }
            false
        });
        self.watches.retain(|w| w.since.elapsed().is_ok_and(|age| age < WATCH_FOR));
        messages
    }

    /// The watches to look at now, if a look is due (the caller runs [`scan`] off the
    /// UI thread and hands the result to [`EditTracker::checked`]).
    pub fn due_check(&mut self) -> Option<Vec<EditWatch>> {
        if self.watches.is_empty()
            || self.check_running
            || self.last_check.is_some_and(|t| t.elapsed() < CHECK_EVERY)
        {
            return None;
        }
        self.check_running = true;
        self.last_check = Some(Instant::now());
        Some(self.watches.clone())
    }

    /// Results of a look: the (original, result file) pairs to bring into the catalog
    /// now. A file is taken once two looks in a row see the same size and time, so a
    /// half-written export is never read.
    pub fn checked(&mut self, found: Vec<(i64, PathBuf, FileVersion)>) -> Vec<(i64, PathBuf)> {
        self.check_running = false;
        let mut ready = Vec::new();
        let mut pending = std::collections::HashMap::new();
        for (original, path, version) in found {
            if self.handled.get(&path) == Some(&version) {
                continue;
            }
            if self.pending.get(&path) == Some(&version) {
                self.handled.insert(path.clone(), version);
                ready.push((original, path));
            } else {
                pending.insert(path, version);
            }
        }
        self.pending = pending;
        ready
    }
}

#[cfg(test)]
mod tests;
