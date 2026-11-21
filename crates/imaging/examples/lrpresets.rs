//! Read every Lightroom preset under a folder (read-only) and report what maps:
//! `cargo run -p imaging --example lrpresets -- <folder>`
//!
//! Profiles (`.xmp` with `PresetType="Look"`) are counted and skipped, as the app does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use imaging::develop::{LrPreset, lightroom_summary, lr_values_from_xmp, read_lrtemplate};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else if p
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("xmp") || x.eq_ignore_ascii_case("lrtemplate"))
        {
            out.push(p);
        }
    }
}

fn read(p: &Path) -> Result<Option<LrPreset>, String> {
    let text = String::from_utf8_lossy(&std::fs::read(p).map_err(|e| e.to_string())?).into_owned();
    if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lrtemplate")) {
        return read_lrtemplate(&text).map(Some);
    }
    let doc = xmp::XmpDoc::parse(&text).map_err(|e| e.to_string())?;
    let props = doc.properties(xmp::ns::CRS);
    if props.iter().any(|(k, v)| k == "PresetType" && v.first().is_some_and(|t| t == "Look")) {
        return Ok(None);
    }
    if props.is_empty() {
        return Err("no crs".into());
    }
    Ok(Some(lr_values_from_xmp(props)))
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: lrpresets <folder>");
    let mut all = Vec::new();
    files(Path::new(&dir), &mut all);
    let (mut profiles, mut presets, mut partial) = (0, 0, 0);
    let mut errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut shown = 0;
    for p in &all {
        match read(p) {
            Ok(None) => profiles += 1,
            Ok(Some(lr)) => {
                presets += 1;
                let s = lightroom_summary(&lr.values);
                if !s.unsupported.is_empty() {
                    partial += 1;
                }
                for k in &s.unsupported {
                    *kinds.entry(k.clone()).or_default() += 1;
                }
                for g in &s.groups {
                    *groups.entry(format!("{g:?}")).or_default() += 1;
                }
                if shown < 12 {
                    shown += 1;
                    println!(
                        "{:40} {:28} sets {:?} — missing {:?}",
                        lr.name.chars().take(40).collect::<String>(),
                        lr.group.unwrap_or_default().chars().take(28).collect::<String>(),
                        s.groups,
                        s.unsupported
                    );
                }
            }
            Err(e) => *errors.entry(e).or_default() += 1,
        }
    }
    println!(
        "\n{} files: {presets} presets ({partial} with unsupported parts), {profiles} profiles",
        all.len()
    );
    println!("groups set: {groups:?}");
    println!("unsupported: {kinds:?}");
    println!("errors: {errors:?}");
}
