//! Render a photo with Develop settings:
//! `cargo run -p imaging --example develop --release -- <file> [out.png] [field=value ...]`
//! (fields as in the settings JSON, e.g. `exposure=0.5 contrast=20 wb="Tungsten"`).
//! Also saves the embedded/standard preview next to it for comparison, and times the steps.

use std::time::Instant;

use imaging::SourceKind;
use imaging::develop::{DevelopSettings, WorkingImage, render_srgb8};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("usage: develop <file> [out.png] [k=v...]"));
    let rest: Vec<String> = args.collect();
    let out = rest
        .iter()
        .find(|a| a.ends_with(".png"))
        .map(Into::into)
        .unwrap_or_else(|| std::env::temp_dir().join("develop.png"));
    let mut json = serde_json::to_value(DevelopSettings::default()).unwrap();
    for kv in rest.iter().filter(|a| a.contains('=')) {
        let (k, v) = kv.split_once('=').unwrap();
        json[k] = serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.into()));
    }
    let settings: DevelopSettings = serde_json::from_value(json).expect("valid settings");
    let raw = ["nef", "dng", "cr2", "cr3", "arw", "raf", "orf", "rw2"]
        .iter()
        .any(|e| path.extension().is_some_and(|x| x.eq_ignore_ascii_case(e)));
    let kind = if raw { SourceKind::Raw } else { SourceKind::Image };

    let t = Instant::now();
    let (img, _) = WorkingImage::load(
        &path,
        kind,
        Some(std::env::var("PROXY").ok().and_then(|v| v.parse().ok()).unwrap_or(2560)),
    )
    .expect("load");
    println!("load proxy {}x{} in {:?} ({:?})", img.width(), img.height(), t.elapsed(), img.color);
    let wb = img.resolve_wb(&settings);
    println!("white balance {:?}", wb);
    let mut rendered = None;
    let mut best = std::time::Duration::MAX;
    for _ in 0..15 {
        let t = Instant::now();
        rendered = Some(render_srgb8(&img, &settings));
        best = best.min(t.elapsed());
    }
    println!("render: best of 15 {best:?}");
    rendered.unwrap().save(&out).unwrap();
    let (preview, _) = imaging::decode(&path, kind, 2560).expect("preview");
    let prev_out = out.with_extension("preview.png");
    preview.save(&prev_out).unwrap();
    println!("-> {}\n-> {}", out.display(), prev_out.display());
}
