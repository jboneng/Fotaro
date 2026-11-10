//! NegPy: detection (a release newer than 0.68.0, or a source checkout with its
//! `.venv`) and its launch recipe. NegPy names its export `<original>_NegPy.<ext>`
//! next to the original and gives it the original's modified time.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{EditInPrefs, Launch, ResultName};

/// The last NegPy release that can't open files from the command line.
const LAST_RELEASE_WITHOUT_LAUNCH: (u32, u32, u32) = (0, 68, 0);

/// What NegPy writes after the original's name: `scan.NEF` → `scan_NegPy.tif`.
pub const RESULT_SUFFIX: &str = "NegPy";

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

    /// The program, the arguments before NegPy's own, and the folder to run in.
    fn command(&self) -> (PathBuf, Vec<OsString>, Option<PathBuf>) {
        match &self.launcher {
            Launcher::App { exe } => (exe.clone(), Vec::new(), exe.parent().map(Path::to_path_buf)),
            Launcher::Source { python, repo } => {
                (python.clone(), vec![repo.join("desktop.py").into_os_string()], Some(repo.clone()))
            }
        }
    }

    /// The launches that open `originals` (photo id, file): one per folder, each
    /// exporting back into its folder. A NegPy that is already open takes the files
    /// and the launched process exits at once; a single photo closes NegPy after its
    /// export.
    pub fn launches(&self, originals: &[(i64, PathBuf)], prefs: &EditInPrefs) -> Vec<Launch> {
        let mut by_dir: std::collections::BTreeMap<PathBuf, Vec<(i64, PathBuf)>> = Default::default();
        for (id, path) in originals {
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let group = by_dir.entry(dir).or_default();
            if !group.iter().any(|(_, p)| p == path) {
                group.push((*id, path.clone()));
            }
        }
        let quit_after = originals.len() == 1;
        by_dir
            .into_iter()
            .map(|(dir, group)| {
                let (program, mut args, cwd) = self.command();
                let files: Vec<PathBuf> = group.iter().map(|(_, p)| p.clone()).collect();
                args.extend(launch_args(&files, &dir, prefs, quit_after));
                let results = group
                    .iter()
                    .map(|(id, p)| {
                        let stem = p.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                        (*id, dir.clone(), ResultName::NegPy { stem })
                    })
                    .collect();
                Launch {
                    editor: super::Editor::NegPy,
                    program,
                    args,
                    cwd,
                    results,
                    wait_for_exit: false,
                    then: Vec::new(),
                    clear_first: Vec::new(),
                    finish: Vec::new(),
                }
            })
            .collect()
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

pub(crate) fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
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
