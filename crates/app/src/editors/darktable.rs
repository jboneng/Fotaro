//! darktable: the originals open in darktable (all in one window); darktable keeps each
//! edit in its own sidecar (`photo.NEF.xmp`). When darktable closes, every photo whose
//! sidecar changed is rendered with `darktable-cli` to `<name>_darktable.tif` next to the
//! original (through a temporary file: darktable-cli never overwrites), which comes back
//! into its stack like any edit.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{EditColorSpace, EditInPrefs, Editor, FollowUp, Launch, ResultName};

/// darktable's result name for `photo.NEF`: `photo_darktable.tif`.
pub const RESULT_SUFFIX: &str = "_darktable";

/// An installed darktable (the program and its command-line renderer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Darktable {
    pub exe: PathBuf,
    pub cli: PathBuf,
    pub version: Option<String>,
}

/// darktable's sidecar for `photo`: `photo.NEF.xmp` (the whole file name, then `.xmp`).
pub fn sidecar(photo: &Path) -> PathBuf {
    let mut name = photo.as_os_str().to_owned();
    name.push(".xmp");
    PathBuf::from(name)
}

/// darktable-cli's `--icc-type` names.
fn icc_type(cs: EditColorSpace) -> &'static str {
    match cs {
        EditColorSpace::Srgb => "SRGB",
        EditColorSpace::AdobeRgb => "ADOBERGB",
        EditColorSpace::ProPhotoRgb => "PROPHOTO_RGB",
        EditColorSpace::DisplayP3 => "DISPLAY_P3",
    }
}

/// darktable-cli reads the output name as a file name pattern, where `\` escapes: give
/// it forward slashes.
fn forward_slashes(p: &Path) -> OsString {
    p.to_string_lossy().replace('\\', "/").into()
}

impl Darktable {
    /// "darktable 5.6.2 (C:\…\darktable.exe)".
    pub fn describe(&self) -> String {
        let version = self.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
        format!("darktable{version} ({})", self.exe.display())
    }

    /// The result of editing `original`.
    pub fn result_path(original: &Path) -> PathBuf {
        let stem = original.file_stem().unwrap_or_default().to_string_lossy();
        original.with_file_name(format!("{stem}{RESULT_SUFFIX}.tif"))
    }

    /// The render of one original into its result (via a temporary file).
    fn render(&self, id: i64, original: &Path, prefs: &EditInPrefs, n: usize) -> Launch {
        let result = Self::result_path(original);
        let dir = result.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = PathBuf::from(result.file_name().unwrap_or_default());
        let temp = std::env::temp_dir()
            .join("fotaro-darktable")
            .join(format!("{}-{id}-{n}.tif", std::process::id()));
        let bits = if prefs.darktable_sixteen_bit { "16" } else { "8" };
        let args: Vec<OsString> = vec![
            original.as_os_str().to_owned(),
            sidecar(original).into_os_string(),
            forward_slashes(&temp),
            "--icc-type".into(),
            icc_type(prefs.darktable_color_space).into(),
            "--apply-custom-presets".into(),
            "false".into(),
            "--core".into(),
            "--conf".into(),
            format!("plugins/imageio/format/tiff/bpp={bits}").into(),
        ];
        Launch {
            editor: Editor::Darktable,
            program: self.cli.clone(),
            args,
            cwd: self.cli.parent().map(Path::to_path_buf),
            results: vec![(id, dir, ResultName::Exact(name))],
            wait_for_exit: true,
            then: Vec::new(),
            clear_first: vec![temp.clone()],
            finish: vec![(temp, result)],
        }
    }

    /// One darktable window with every original; when it closes, a render for each
    /// photo whose sidecar changed.
    pub fn launches(&self, originals: &[(i64, PathBuf)], prefs: &EditInPrefs) -> Vec<Launch> {
        let mut unique: Vec<(i64, PathBuf)> = Vec::new();
        for (id, path) in originals {
            if !unique.iter().any(|(_, p)| p == path) {
                unique.push((*id, path.clone()));
            }
        }
        if unique.is_empty() {
            return Vec::new();
        }
        let results = unique
            .iter()
            .map(|(id, p)| {
                let result = Self::result_path(p);
                let dir = result.parent().map(Path::to_path_buf).unwrap_or_default();
                (*id, dir, ResultName::Exact(PathBuf::from(result.file_name().unwrap_or_default())))
            })
            .collect();
        let then = unique
            .iter()
            .enumerate()
            .map(|(n, (id, p))| FollowUp {
                original: *id,
                if_changed: sidecar(p),
                launch: self.render(*id, p, prefs, n),
            })
            .collect();
        vec![Launch {
            editor: Editor::Darktable,
            program: self.exe.clone(),
            args: unique.iter().map(|(_, p)| p.as_os_str().to_owned()).collect(),
            cwd: self.exe.parent().map(Path::to_path_buf),
            results,
            wait_for_exit: true,
            then,
            clear_first: Vec::new(),
            finish: Vec::new(),
        }]
    }
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) { format!("{base}.exe") } else { base.to_owned() }
}

/// darktable at `path`: the program, its folder, or the install folder (with `bin`).
pub fn at_location(path: &Path) -> Result<Darktable, String> {
    let dir = if path.is_file() {
        path.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        path.to_path_buf()
    };
    for d in [dir.clone(), dir.join("bin"), dir.join("Contents/MacOS")] {
        let (exe, cli) = (d.join(exe_name("darktable")), d.join(exe_name("darktable-cli")));
        if exe.is_file() {
            if !cli.is_file() {
                return Err(format!("{} has no darktable-cli, which renders the edits", d.display()));
            }
            return Ok(Darktable { exe, cli, version: installed_version() });
        }
    }
    Err(format!("no darktable in {}", path.display()))
}

#[cfg(windows)]
fn installed_version() -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let key = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\darktable_is1";
    RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(key).ok()?.get_value("DisplayVersion").ok()
}

#[cfg(not(windows))]
fn installed_version() -> Option<String> {
    None
}

/// Where darktable installs.
// What goes in depends on the system.
#[allow(clippy::vec_init_then_push)]
fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    #[cfg(windows)]
    {
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            if let Ok(dir) = RegKey::predef(root)
                .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\darktable_is1")
                .and_then(|k| k.get_value::<String, _>("InstallLocation"))
            {
                v.push(PathBuf::from(dir));
            }
        }
        if let Some(base) = std::env::var_os("ProgramFiles") {
            v.push(PathBuf::from(base).join("darktable"));
        }
    }
    #[cfg(target_os = "macos")]
    v.push(PathBuf::from("/Applications/darktable.app"));
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(path) = std::env::var_os("PATH") {
        v.extend(std::env::split_paths(&path).filter(|d| d.join("darktable").is_file()));
    }
    v
}

/// The darktable to use: the chosen location, else an installed one.
pub fn detect(configured: Option<&Path>) -> Result<Darktable, String> {
    if let Some(path) = configured {
        return at_location(path);
    }
    candidates()
        .iter()
        .filter(|p| p.exists())
        .find_map(|p| at_location(p).ok())
        .ok_or_else(|| "darktable was not found".to_owned())
}
