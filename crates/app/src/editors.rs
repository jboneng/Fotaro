//! Edit In (LrC Photo ▸ Edit In): open originals in NegPy, then find the files NegPy
//! exports and bring them into the catalog, stacked with their originals.
//!
//! NegPy is told on its command line where to export and how to name the result
//! (`<original name>_NegPy.tif` next to the original), so the result can be found by
//! name. NegPy itself is detected: a release installed in the usual place, or a source
//! checkout (with its `.venv`) in one of the usual development folders, or wherever
//! the user points the External Editing settings.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The last NegPy release that can't open files from the command line.
const LAST_RELEASE_WITHOUT_LAUNCH: (u32, u32, u32) = (0, 68, 0);

/// What NegPy writes after the original's name: `scan.NEF` → `scan_NegPy.tif`.
pub const RESULT_SUFFIX: &str = "NegPy";

/// LrC's Preferences ▸ External Editing, for NegPy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditInPrefs {
    /// A NegPy source checkout, an install folder or the program itself. `None`: detect.
    pub negpy_location: Option<PathBuf>,
    pub format: EditFormat,
    /// TIFF and PNG: 16 bits per channel.
    pub sixteen_bit: bool,
    pub color_space: EditColorSpace,
    /// JPEG quality.
    pub quality: u8,
}

impl Default for EditInPrefs {
    fn default() -> Self {
        EditInPrefs {
            negpy_location: None,
            format: EditFormat::Tiff,
            sixteen_bit: true,
            color_space: EditColorSpace::AdobeRgb,
            quality: 92,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EditFormat {
    #[default]
    Tiff,
    Jpeg,
    Png,
}

impl EditFormat {
    pub const ALL: [EditFormat; 3] = [Self::Tiff, Self::Jpeg, Self::Png];

    pub fn label(self) -> &'static str {
        match self {
            Self::Tiff => "TIFF",
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
        }
    }

    fn arg(self) -> &'static str {
        match self {
            Self::Tiff => "tiff",
            Self::Jpeg => "jpeg",
            Self::Png => "png",
        }
    }

    pub fn has_bit_depth(self) -> bool {
        self != Self::Jpeg
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EditColorSpace {
    Srgb,
    #[default]
    AdobeRgb,
    ProPhotoRgb,
    DisplayP3,
}

impl EditColorSpace {
    pub const ALL: [EditColorSpace; 4] = [Self::Srgb, Self::AdobeRgb, Self::ProPhotoRgb, Self::DisplayP3];

    pub fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::AdobeRgb => "Adobe RGB (1998)",
            Self::ProPhotoRgb => "ProPhoto RGB",
            Self::DisplayP3 => "Display P3",
        }
    }

    /// NegPy's `--color-space` names.
    fn arg(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::AdobeRgb => "adobe-rgb",
            Self::ProPhotoRgb => "prophoto-rgb",
            Self::DisplayP3 => "p3-d65",
        }
    }
}

/// How to start NegPy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher {
    /// An installed release.
    App { exe: PathBuf },
    /// A source checkout run with its virtual environment's Python.
    Source { python: PathBuf, repo: PathBuf },
}

/// A NegPy that can open files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegPy {
    pub launcher: Launcher,
    pub version: Option<String>,
}

impl NegPy {
    /// "NegPy 0.68.0 (source, C:\…\NegPy)".
    pub fn describe(&self) -> String {
        let version = self.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
        match &self.launcher {
            Launcher::App { exe } => format!("NegPy{version} ({})", exe.display()),
            Launcher::Source { repo, .. } => format!("NegPy{version} from source ({})", repo.display()),
        }
    }

    fn command(&self) -> Command {
        match &self.launcher {
            Launcher::App { exe } => {
                let mut c = Command::new(exe);
                if let Some(dir) = exe.parent() {
                    c.current_dir(dir);
                }
                c
            }
            Launcher::Source { python, repo } => {
                let mut c = Command::new(python);
                c.arg(repo.join("desktop.py")).current_dir(repo);
                c
            }
        }
    }

    /// Open `files` in NegPy, exporting to `output_dir`. A NegPy that is already open
    /// takes the files and the launched process exits at once.
    pub fn open(
        &self,
        files: &[PathBuf],
        output_dir: &Path,
        prefs: &EditInPrefs,
        quit_after: bool,
    ) -> Result<Child> {
        let mut c = self.command();
        c.args(launch_args(files, output_dir, prefs, quit_after));
        c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        c.spawn().with_context(|| format!("starting {}", self.describe()))
    }
}

/// NegPy's command line: the files, then the export recipe.
pub fn launch_args(
    files: &[PathBuf],
    output_dir: &Path,
    prefs: &EditInPrefs,
    quit_after: bool,
) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["--format".into(), prefs.format.arg().into()];
    if prefs.format.has_bit_depth() {
        a.extend(["--bit-depth".into(), if prefs.sixteen_bit { "16" } else { "8" }.into()]);
    } else {
        a.extend(["--quality".into(), prefs.quality.clamp(1, 100).to_string().into()]);
    }
    a.extend(["--color-space".into(), prefs.color_space.arg().into()]);
    a.extend(["--output-dir".into(), output_dir.as_os_str().to_owned()]);
    a.extend(["--filename".into(), format!("{{{{ original_name }}}}-{RESULT_SUFFIX}").into()]);
    if quit_after {
        a.push("--quit-after-export".into());
    }
    // Paths last; NegPy reads anything that isn't a flag as a file.
    a.extend(files.iter().map(|f| f.as_os_str().to_owned()));
    a
}

/// Is `file` (a file name) NegPy's result for an original with this stem? NegPy names
/// it `<stem>_NegPy`, or `<stem>_<n>_NegPy` for a half frame.
pub fn is_result_of(file: &Path, original_stem: &str) -> bool {
    let Some(stem) = file.file_stem().map(|s| s.to_string_lossy().to_lowercase()) else { return false };
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if !matches!(ext.as_str(), "tif" | "tiff" | "jpg" | "jpeg" | "png" | "webp" | "jxl") {
        return false;
    }
    let original = original_stem.to_lowercase();
    let suffix = format!("_{}", RESULT_SUFFIX.to_lowercase());
    let Some(middle) = stem.strip_prefix(&original).and_then(|r| r.strip_suffix(&suffix)) else {
        return false;
    };
    middle.is_empty()
        || middle.strip_prefix('_').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

// ---------------------------------------------------------------------------------
// Detection

fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut parts = v.trim().trim_start_matches('v').split(['.', '-', '+']).map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)))
}

fn read_version(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// A source checkout: `desktop.py`, the launch support and a virtual environment.
fn source_at(repo: &Path) -> std::result::Result<NegPy, String> {
    if !repo.join("desktop.py").is_file() || !repo.join("negpy").is_dir() {
        return Err(format!("{} is not a NegPy folder", repo.display()));
    }
    if !repo.join("negpy/desktop/launch.py").is_file() {
        return Err(format!("the NegPy source in {} can't open files from other apps yet", repo.display()));
    }
    let venv = repo.join(".venv");
    let candidates: &[&str] = if cfg!(windows) {
        &["Scripts/pythonw.exe", "Scripts/python.exe"]
    } else {
        &["bin/python3", "bin/python"]
    };
    let python = candidates.iter().map(|c| venv.join(c)).find(|p| p.is_file()).ok_or_else(|| {
        format!("the NegPy source in {} has no .venv; run `uv sync` there first", repo.display())
    })?;
    Ok(NegPy {
        launcher: Launcher::Source { python, repo: repo.to_path_buf() },
        version: read_version(&repo.join("VERSION")),
    })
}

/// An installed release (the program, or the folder it is in).
fn app_at(path: &Path) -> std::result::Result<NegPy, String> {
    let exe = if path.is_dir() {
        app_exe_in(path).ok_or_else(|| format!("no NegPy program in {}", path.display()))?
    } else {
        path.to_path_buf()
    };
    if !exe.is_file() {
        return Err(format!("{} not found", exe.display()));
    }
    let dir = exe.parent().unwrap_or(Path::new("."));
    let version = ["_internal/VERSION", "VERSION", "../Resources/VERSION"]
        .iter()
        .find_map(|v| read_version(&dir.join(v)));
    match version.as_deref().and_then(parse_version) {
        Some(v) if v > LAST_RELEASE_WITHOUT_LAUNCH => Ok(NegPy { launcher: Launcher::App { exe }, version }),
        Some(_) => Err(format!(
            "NegPy {} is installed, but this version can't open files from other apps; a newer release or a source checkout can",
            version.unwrap_or_default()
        )),
        None => Err(format!("can't tell which NegPy version {} is", exe.display())),
    }
}

fn app_exe_in(dir: &Path) -> Option<PathBuf> {
    ["NegPy.exe", "NegPy", "Contents/MacOS/NegPy"].iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// Use what the user chose: a source checkout, an install folder or the program.
pub fn at_location(path: &Path) -> std::result::Result<NegPy, String> {
    if path.is_dir() && path.join("desktop.py").is_file() { source_at(path) } else { app_at(path) }
}

/// Where releases install.
fn installed_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        for (root, key) in [
            (HKEY_LOCAL_MACHINE, r"Software\NegPy"),
            (HKEY_LOCAL_MACHINE, r"Software\WOW6432Node\NegPy"),
            (HKEY_CURRENT_USER, r"Software\NegPy"),
        ] {
            if let Ok(dir) =
                RegKey::predef(root).open_subkey(key).and_then(|k| k.get_value::<String, _>("InstallDir"))
            {
                v.push(PathBuf::from(dir));
            }
        }
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                v.push(base.join("NegPy"));
                v.push(base.join("Programs").join("NegPy"));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        v.push(PathBuf::from("/Applications/NegPy.app"));
        if let Some(home) = directories::BaseDirs::new() {
            v.push(home.home_dir().join("Applications/NegPy.app"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(path) = std::env::var_os("PATH") {
            v.extend(std::env::split_paths(&path).map(|d| d.join("NegPy")).filter(|p| p.is_file()));
        }
        v.push(PathBuf::from("/opt/NegPy"));
    }
    v
}

/// Where developers keep source checkouts (and `NEGPY_SOURCE`).
fn source_candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("NEGPY_SOURCE").map(PathBuf::from).into_iter().collect();
    if let Some(dirs) = directories::UserDirs::new() {
        let home = dirs.home_dir().to_path_buf();
        let mut bases = vec![home.clone()];
        bases.extend(dirs.desktop_dir().map(Path::to_path_buf));
        bases.extend(dirs.document_dir().map(Path::to_path_buf));
        for base in bases {
            for sub in [
                "NegPy",
                "Negpy/NegPy",
                "GitHub/NegPy",
                "source/repos/NegPy",
                "src/NegPy",
                "code/NegPy",
                "dev/NegPy",
                "Projects/NegPy",
                "git/NegPy",
                "repos/NegPy",
            ] {
                v.push(base.join(sub));
            }
        }
    }
    v
}

/// The NegPy to use: the chosen location, else an installed release that can open
/// files, else a source checkout. The error says why none could be used.
pub fn detect(configured: Option<&Path>) -> std::result::Result<NegPy, String> {
    if let Some(path) = configured {
        return at_location(path);
    }
    let mut reasons = Vec::new();
    for dir in installed_candidates() {
        if dir.exists() {
            match app_at(&dir) {
                Ok(n) => return Ok(n),
                Err(e) => reasons.push(e),
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for repo in source_candidates() {
        if repo.join("desktop.py").is_file()
            && seen.insert(dunce::canonicalize(&repo).unwrap_or(repo.clone()))
        {
            match source_at(&repo) {
                Ok(n) => return Ok(n),
                Err(e) => reasons.push(e),
            }
        }
    }
    Err(reasons.into_iter().next().unwrap_or_else(|| "NegPy was not found".to_owned()))
}

// ---------------------------------------------------------------------------------
// Watching for results

/// An original sent to NegPy, whose exports are looked for next to it.
#[derive(Debug, Clone)]
pub struct EditWatch {
    pub original: i64,
    pub dir: PathBuf,
    pub stem: String,
    /// Results older than this were there before (with a second of slack for coarse
    /// file system clocks).
    pub since: SystemTime,
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
            let Some(w) = ws.iter().find(|w| is_result_of(&path, &w.stem)) else { continue };
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
/// A launch for another folder waits for the previous one to exit (it handed its files
/// to a running NegPy) or this long (it is the NegPy the next one will hand off to).
const LAUNCH_GAP: Duration = Duration::from_secs(8);
/// Originals are watched this long after they were sent.
const WATCH_FOR: Duration = Duration::from_secs(24 * 3600);

/// One NegPy launch: originals from one folder, exported back into it.
#[derive(Debug, Clone)]
pub struct Launch {
    pub dir: PathBuf,
    /// (photo id, original file).
    pub originals: Vec<(i64, PathBuf)>,
    pub quit_after: bool,
}

/// Plan the launches for `originals` (photo id, file): one per folder, and NegPy
/// closes after exporting when it was opened for a single photo.
pub fn plan_launches(originals: &[(i64, PathBuf)]) -> Vec<Launch> {
    let mut by_dir: BTreeMap<PathBuf, Vec<(i64, PathBuf)>> = BTreeMap::new();
    for (id, path) in originals {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let group = by_dir.entry(dir).or_default();
        if !group.iter().any(|(_, p)| p == path) {
            group.push((*id, path.clone()));
        }
    }
    let single = originals.len() == 1;
    by_dir.into_iter().map(|(dir, originals)| Launch { dir, originals, quit_after: single }).collect()
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
    /// The NegPy processes started, with when.
    children: Vec<(Child, Instant)>,
}

impl EditTracker {
    pub fn is_idle(&self) -> bool {
        self.watches.is_empty() && self.queue.is_empty() && self.children.is_empty()
    }

    pub fn enqueue(&mut self, launches: Vec<Launch>) {
        self.queue.extend(launches);
    }

    /// Start queued launches whose turn it is, and report NegPy processes that failed.
    pub fn tick(&mut self, negpy: &NegPy, prefs: &EditInPrefs) -> Vec<String> {
        let mut messages = Vec::new();
        let now = Instant::now();
        self.children.retain_mut(|(child, _)| match child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                if !status.success() {
                    let code = status.code().map_or_else(|| "?".to_owned(), |c| c.to_string());
                    let why = if code == "2" { "it didn't accept the launch options" } else { "it stopped" };
                    messages.push(format!(
                        "NegPy did not open the photos ({why}, exit code {code}; see negpy.log)"
                    ));
                }
                false
            }
            Err(_) => false,
        });
        let previous_running = self.children.iter().any(|(_, at)| now.duration_since(*at) < LAUNCH_GAP);
        if !previous_running && let Some(launch) = self.queue.pop_front() {
            let files: Vec<PathBuf> = launch.originals.iter().map(|(_, p)| p.clone()).collect();
            match negpy.open(&files, &launch.dir, prefs, launch.quit_after) {
                Ok(child) => {
                    log::info!(
                        "opened {} photo(s) in {} (results in {})",
                        files.len(),
                        negpy.describe(),
                        launch.dir.display()
                    );
                    self.children.push((child, now));
                    let since = SystemTime::now();
                    for (id, path) in &launch.originals {
                        let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                        self.watches.retain(|w| w.original != *id);
                        self.watches.push(EditWatch { original: *id, dir: launch.dir.clone(), stem, since });
                    }
                }
                Err(e) => messages.push(format!("Could not start NegPy: {e:#}")),
            }
        }
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
