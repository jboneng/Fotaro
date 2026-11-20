//! Develop presets (LrC's Presets panel): app-wide, one JSON file each under
//! `<app data>/Develop Presets/<Folder>/<Name>.json`, so packs are easy to back up and
//! share. Our own presets hold settings and the groups they set; imported Lightroom
//! presets keep Lightroom's values and are applied through [`apply_lightroom`], setting
//! only what they have.

use std::path::{Path, PathBuf};

use imaging::develop::{
    DevelopSettings, LrValue, SettingsGroup, Treatment, apply_lightroom, copy_groups, lightroom_summary,
    lr_values_from_xmp, read_lrtemplate,
};
use serde::{Deserialize, Serialize};

/// What a preset file holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresetFile {
    pub name: String,
    /// Our own preset: the settings, and which groups of them it sets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<DevelopSettings>,
    #[serde(default)]
    pub groups: Vec<SettingsGroup>,
    /// An imported Lightroom preset: its values as Lightroom has them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightroom: Option<Vec<(String, LrValue)>>,
    /// What couldn't come from Lightroom ("Profile", "Lens Corrections"…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsupported: Vec<String>,
}

impl PresetFile {
    /// `settings` with this preset applied (`kelvin`: the photo's Temp is in Kelvin).
    pub fn apply(&self, settings: &DevelopSettings, kelvin: bool) -> DevelopSettings {
        let mut out = settings.clone();
        if let Some(s) = &self.settings {
            copy_groups(s, &mut out, &self.groups);
        }
        if let Some(values) = &self.lightroom {
            apply_lightroom(values, &mut out, kelvin);
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub folder: String,
    pub file: PresetFile,
    /// Where it is stored; `None` for the built-in ones.
    pub path: Option<PathBuf>,
}

impl Preset {
    pub fn name(&self) -> &str {
        &self.file.name
    }
}

pub const BUILT_IN: &str = "Built-in";
pub const USER: &str = "User Presets";

fn built_in() -> Vec<Preset> {
    use SettingsGroup as G;
    let make = |name: &str, groups: Vec<G>, f: &dyn Fn(&mut DevelopSettings)| {
        let mut s = DevelopSettings::default();
        f(&mut s);
        Preset {
            folder: BUILT_IN.into(),
            file: PresetFile {
                name: name.into(),
                settings: Some(s),
                groups,
                lightroom: None,
                unsupported: Vec::new(),
            },
            path: None,
        }
    };
    vec![
        make("Black & White", vec![G::Treatment, G::BasicTone], &|s| {
            s.treatment = Treatment::BlackAndWhite;
            s.contrast = 20.0;
        }),
        make("Flat", vec![G::BasicTone], &|s| {
            (s.contrast, s.highlights, s.shadows, s.whites, s.blacks) = (-30.0, -40.0, 40.0, -10.0, 20.0);
        }),
        make("Punch", vec![G::BasicTone, G::Presence], &|s| {
            (s.contrast, s.clarity, s.vibrance) = (25.0, 10.0, 20.0);
        }),
        make("Soft Matte", vec![G::BasicTone, G::ToneCurve], &|s| {
            s.contrast = -10.0;
            s.curve.rgb = vec![[0.0, 0.08], [0.25, 0.25], [0.75, 0.76], [1.0, 0.96]];
        }),
    ]
}

/// A name usable as a file name.
fn file_safe(name: &str) -> String {
    let s: String =
        name.chars().map(|c| if c.is_alphanumeric() || " -_()&+,.'".contains(c) { c } else { '_' }).collect();
    let s = s.trim().trim_matches('.').to_string();
    if s.is_empty() { "Preset".into() } else { s }
}

/// What an import found.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ImportReport {
    pub imported: usize,
    /// Presets with settings we have no slider for.
    pub partial: usize,
    /// Files that weren't develop presets (or couldn't be read), with why.
    pub skipped: Vec<(PathBuf, String)>,
}

#[derive(Debug, Default)]
pub struct PresetStore {
    root: Option<PathBuf>,
    pub presets: Vec<Preset>,
}

impl PresetStore {
    /// The presets under `root` (folders one level deep), after the built-in ones.
    pub fn load(root: Option<PathBuf>) -> PresetStore {
        let mut store = PresetStore { root, presets: Vec::new() };
        store.reload();
        store
    }

    pub fn reload(&mut self) {
        let mut found = Vec::new();
        if let Some(root) = &self.root
            && let Ok(dirs) = std::fs::read_dir(root)
        {
            for d in dirs.flatten() {
                let path = d.path();
                if path.is_dir() {
                    let folder = d.file_name().to_string_lossy().into_owned();
                    for f in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                        read_preset(&f.path(), &folder, &mut found);
                    }
                } else {
                    read_preset(&path, USER, &mut found);
                }
            }
        }
        found.sort_by(|a: &Preset, b: &Preset| {
            (a.folder.to_lowercase(), a.name().to_lowercase()).cmp(&(b.folder.to_lowercase(), b.name().to_lowercase()))
        });
        self.presets = built_in();
        self.presets.extend(found);
    }

    /// The folders, built-in first, then by name.
    pub fn folders(&self) -> Vec<String> {
        let mut f: Vec<String> = Vec::new();
        for p in &self.presets {
            if !f.contains(&p.folder) {
                f.push(p.folder.clone());
            }
        }
        if !f.iter().any(|x| x == USER) {
            f.push(USER.into());
        }
        f
    }

    fn root(&self) -> anyhow::Result<&Path> {
        self.root.as_deref().ok_or_else(|| anyhow::anyhow!("no folder for presets"))
    }

    /// A file path for `name` in `folder` not used yet.
    fn free_path(&self, folder: &str, name: &str) -> anyhow::Result<PathBuf> {
        let dir = self.root()?.join(file_safe(folder));
        std::fs::create_dir_all(&dir)?;
        let base = file_safe(name);
        let mut path = dir.join(format!("{base}.json"));
        let mut n = 2;
        while path.exists() {
            path = dir.join(format!("{base} {n}.json"));
            n += 1;
        }
        Ok(path)
    }

    /// Save a new preset in `folder`; replaces one of the same name there.
    pub fn add(&mut self, folder: &str, file: PresetFile) -> anyhow::Result<()> {
        if let Some(old) = self.presets.iter().find(|p| p.folder == folder && p.name() == file.name && p.path.is_some()) {
            write(old.path.as_ref().unwrap(), &file)?;
        } else {
            let path = self.free_path(folder, &file.name)?;
            write(&path, &file)?;
        }
        self.reload();
        Ok(())
    }

    /// LrC's Update with Current Settings: the same groups, from `settings`.
    pub fn update(&mut self, index: usize, settings: &DevelopSettings) -> anyhow::Result<()> {
        let p = self.presets.get(index).ok_or_else(|| anyhow::anyhow!("no such preset"))?;
        let path = p.path.clone().ok_or_else(|| anyhow::anyhow!("built-in presets can't be changed"))?;
        let mut file = p.file.clone();
        // A Lightroom preset becomes our own, with the groups it had.
        if let Some(values) = file.lightroom.take() {
            file.groups = lightroom_summary(&values).groups;
            file.unsupported.clear();
        }
        file.settings = Some(settings.clone());
        write(&path, &file)?;
        self.reload();
        Ok(())
    }

    pub fn rename(&mut self, index: usize, name: &str) -> anyhow::Result<()> {
        let p = self.presets.get(index).ok_or_else(|| anyhow::anyhow!("no such preset"))?;
        let old = p.path.clone().ok_or_else(|| anyhow::anyhow!("built-in presets can't be renamed"))?;
        let mut file = p.file.clone();
        file.name = name.trim().to_string();
        let path = self.free_path(&p.folder.clone(), name)?;
        write(&path, &file)?;
        std::fs::remove_file(old)?;
        self.reload();
        Ok(())
    }

    pub fn delete(&mut self, index: usize) -> anyhow::Result<()> {
        let p = self.presets.get(index).ok_or_else(|| anyhow::anyhow!("no such preset"))?;
        let path = p.path.clone().ok_or_else(|| anyhow::anyhow!("built-in presets can't be deleted"))?;
        std::fs::remove_file(&path)?;
        // An emptied folder goes too.
        if let Some(dir) = path.parent()
            && std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none())
        {
            let _ = std::fs::remove_dir(dir);
        }
        self.reload();
        Ok(())
    }

    /// Import Lightroom presets (`.xmp`, `.lrtemplate`) from files or folders (searched
    /// through). Each goes in its Lightroom group, else the folder it came from, else
    /// `folder`.
    pub fn import(&mut self, paths: &[PathBuf], folder: &str) -> ImportReport {
        let mut report = ImportReport::default();
        let mut files = Vec::new();
        for p in paths {
            collect_presets(p, p.parent().unwrap_or(p), &mut files);
        }
        for (path, from_folder) in files {
            match read_lightroom(&path) {
                Ok(lr) => {
                    let summary = lightroom_summary(&lr.values);
                    if summary.groups.is_empty() && summary.unsupported.is_empty() {
                        report.skipped.push((path, "sets nothing we have".into()));
                        continue;
                    }
                    let name = if lr.name.is_empty() {
                        path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                    } else {
                        lr.name
                    };
                    let group = lr.group.or(from_folder).unwrap_or_else(|| folder.to_string());
                    let file = PresetFile {
                        name,
                        settings: None,
                        groups: summary.groups,
                        lightroom: Some(lr.values),
                        unsupported: summary.unsupported,
                    };
                    let partial = !file.unsupported.is_empty();
                    match self.free_path(&group, &file.name).and_then(|p| write(&p, &file)) {
                        Ok(()) => {
                            report.imported += 1;
                            report.partial += usize::from(partial);
                        }
                        Err(e) => report.skipped.push((path, e.to_string())),
                    }
                }
                Err(e) => report.skipped.push((path, e)),
            }
        }
        self.reload();
        report
    }
}

fn write(path: &Path, file: &PresetFile) -> anyhow::Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(file)?)?;
    Ok(())
}

fn read_preset(path: &Path, folder: &str, out: &mut Vec<Preset>) {
    if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("json")) {
        return;
    }
    match std::fs::read_to_string(path).map_err(anyhow::Error::from).and_then(|t| Ok(serde_json::from_str(&t)?)) {
        Ok(file) => out.push(Preset { folder: folder.to_string(), file, path: Some(path.to_path_buf()) }),
        Err(e) => log::warn!("preset {}: {e}", path.display()),
    }
}

/// Lightroom preset files under `path`, each with the folder it is in (relative to
/// `base`: the group a folder of presets stands for).
fn collect_presets(path: &Path, base: &Path, out: &mut Vec<(PathBuf, Option<String>)>) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> =
            std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            collect_presets(&e, base, out);
        }
        return;
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
    if matches!(ext.as_deref(), Some("xmp" | "lrtemplate")) {
        let folder = path
            .parent()
            .filter(|p| *p != base)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        out.push((path.to_path_buf(), folder));
    }
}

/// Read a Lightroom preset file.
pub fn read_lightroom(path: &Path) -> Result<imaging::develop::LrPreset, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let lr = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lrtemplate")) {
        read_lrtemplate(&text)?
    } else {
        let doc = xmp::XmpDoc::parse(&text).map_err(|e| e.to_string())?;
        let props = doc.properties(xmp::ns::CRS);
        if props.is_empty() {
            return Err("not a Lightroom preset".into());
        }
        // Profiles are .xmp files too, but no develop presets.
        if props.iter().any(|(k, v)| k == "PresetType" && v.first().is_some_and(|t| t == "Look")) {
            return Err("a profile, not a preset".into());
        }
        lr_values_from_xmp(props)
    };
    Ok(lr)
}

/// Where Lightroom keeps its develop presets on this computer, if it is installed.
pub fn lightroom_folder() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let dir = PathBuf::from(appdata).join("Adobe").join("CameraRaw").join("Settings");
    dir.is_dir().then_some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_saved_applied_renamed_and_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = PresetStore::load(Some(tmp.path().to_path_buf()));
        let built = store.presets.len();
        assert!(built > 0 && store.presets.iter().all(|p| p.folder == BUILT_IN));
        let mine = DevelopSettings { exposure: 1.0, contrast: 30.0, ..DevelopSettings::default() };
        let file = PresetFile {
            name: "Bright/Wide".into(),
            settings: Some(mine.clone()),
            groups: vec![SettingsGroup::BasicTone],
            lightroom: None,
            unsupported: Vec::new(),
        };
        store.add("My Looks", file).unwrap();
        assert_eq!(store.presets.len(), built + 1);
        let i = store.presets.iter().position(|p| p.name() == "Bright/Wide").unwrap();
        assert_eq!(store.presets[i].folder, "My Looks");
        // Applying sets the group only.
        let photo = DevelopSettings { vibrance: 15.0, ..DevelopSettings::default() };
        let out = store.presets[i].file.apply(&photo, true);
        assert_eq!((out.exposure, out.contrast, out.vibrance), (1.0, 30.0, 15.0));
        store.rename(i, "Bright").unwrap();
        let i = store.presets.iter().position(|p| p.name() == "Bright").unwrap();
        store.update(i, &DevelopSettings { exposure: 2.0, ..DevelopSettings::default() }).unwrap();
        let i = store.presets.iter().position(|p| p.name() == "Bright").unwrap();
        assert_eq!(store.presets[i].file.apply(&photo, true).exposure, 2.0);
        store.delete(i).unwrap();
        assert_eq!(store.presets.len(), built);
        assert!(!tmp.path().join("My Looks").exists());
    }

    #[test]
    fn lightroom_presets_import_from_a_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("Film Pack")).unwrap();
        std::fs::write(
            src.join("Film Pack").join("Warm.lrtemplate"),
            "s = { title = \"Warm\", value = { settings = { Exposure2012 = 0.3, IncrementalTemperature = 10, \
             WhiteBalance = \"Custom\", LensProfileEnable = 1, ShadowTint = 4 } } }",
        )
        .unwrap();
        std::fs::write(
            src.join("Cool.xmp"),
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
             crs:Vibrance="-10" crs:Group="Cold" /></rdf:RDF></x:xmpmeta>"#,
        )
        .unwrap();
        std::fs::write(src.join("notes.txt"), "x").unwrap();
        std::fs::write(src.join("broken.xmp"), "<nope").unwrap();
        let mut store = PresetStore::load(Some(tmp.path().join("presets")));
        let report = store.import(std::slice::from_ref(&src), USER);
        assert_eq!((report.imported, report.partial, report.skipped.len()), (2, 1, 1));
        let warm = store.presets.iter().find(|p| p.name() == "Warm").unwrap();
        assert_eq!(warm.folder, "Film Pack");
        assert_eq!(warm.file.unsupported, vec!["Calibration".to_string()]);
        let out = warm.file.apply(&DevelopSettings::default(), false);
        assert_eq!((out.exposure, out.temp), (0.3, 10.0));
        let cool = store.presets.iter().find(|p| p.name() == "Cool").unwrap();
        assert_eq!(cool.folder, "Cold");
        assert_eq!(cool.file.apply(&DevelopSettings::default(), true).vibrance, -10.0);
    }
}
