//! Render a photo without and with Lens Corrections, as the app does:
//! `cargo run -p lensdb --example lensrender --release -- <db dir> <photo> <out dir> [lens name]`
//!
//! Finds the camera and lens from the photo's metadata (or the lens named), resolves the
//! profile, measures Remove CA, and saves `<stem>.off.png`, `.profile.png` (profile) and
//! `.all.png` (profile + Remove CA); prints how far the colours sit apart at edges before and
//! after (red and blue against green, pixels, near the corners).

use std::path::Path;

use imaging::SourceKind;
use imaging::develop::{DevelopSettings, LensCorrections, WorkingImage, measure_ca, render_srgb8};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [dir, photo, out, rest @ ..] = a.as_slice() else {
        anyhow::bail!("usage: lensrender <db dir> <photo> <out dir> [lens name]");
    };
    let path = Path::new(photo);
    let raw = path.extension().is_some_and(|e| ["nef", "dng", "cr2", "arw"].iter().any(|x| e.eq_ignore_ascii_case(x)));
    let kind = if raw { SourceKind::Raw } else { SourceKind::Image };
    let meta = imaging::read_metadata(path, kind)?;
    println!("camera {:?} {:?}, lens {:?}, {:?} mm f/{:?}", meta.make, meta.model, meta.lens, meta.focal_len, meta.aperture);
    let db = lensdb::Database::load(Path::new(dir))?;
    let photo_lens = lensdb::PhotoLens {
        camera_maker: meta.make.clone().unwrap_or_default(),
        camera_model: meta.model.clone().unwrap_or_default(),
        lens: rest.first().cloned().or(meta.lens.clone()).unwrap_or_default(),
        focal: meta.focal_len.map(|f| f as f32),
    };
    let camera = db.find_camera(&photo_lens.camera_maker, &photo_lens.camera_model);
    let found = db.find_lenses(&photo_lens);
    for m in found.iter().take(5) {
        println!("  {:3} {} {}", m.score, if m.sure { "sure" } else { "    " }, db.lenses[m.index].model);
    }
    let lens = &db.lenses[found.first().ok_or_else(|| anyhow::anyhow!("no lens found"))?.index];
    let profile = lensdb::resolve(
        lens,
        camera.map_or("", |c| c.model.as_str()),
        camera.map_or(1.0, |c| c.crop),
        meta.focal_len.map_or(lens.min_focal, |f| f as f32),
        meta.aperture.map_or(8.0, |f| f as f32),
        1000.0,
    );
    println!("profile: {:?} {:?} {:?}", profile.distortion, profile.tca, profile.vignetting);
    let (img, _) = WorkingImage::load(path, kind, Some(2560))?;
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    std::fs::create_dir_all(out)?;
    let base = DevelopSettings::default_for(raw);
    let with = |l: LensCorrections| {
        let mut s = base.clone();
        s.transform.lens = l;
        s
    };
    let prof = LensCorrections { profile_enable: true, profile: Some(profile.clone()), ..Default::default() };
    let t = std::time::Instant::now();
    let ca_before = measure_ca(&img, &LensCorrections::default());
    let ca_after_profile = measure_ca(&img, &prof);
    println!("CA measured on the photo: {ca_before:?}; left after the profile: {ca_after_profile:?} ({:?})", t.elapsed());
    let all = LensCorrections { remove_ca: true, ca: Some(ca_after_profile), ..prof.clone() };
    for (name, l) in [("off", LensCorrections::default()), ("profile", prof), ("all", all)] {
        let t = std::time::Instant::now();
        let rgb = render_srgb8(&img, &with(l));
        let p = Path::new(out).join(format!("{stem}.{name}.png"));
        rgb.save(&p)?;
        // How much CA the result still shows (measured on the render itself).
        let (shown, _) = WorkingImage::load(&p, SourceKind::Image, None)?;
        let c = measure_ca(&shown, &LensCorrections::default());
        let corner = |s: f32, q: f32| (s + q) * 0.5 * (2560f32.hypot(1707.0));
        println!("{name}: {:?} -> {}; CA left at the corners: red {:+.2} px, blue {:+.2} px", t.elapsed(), p.display(), corner(c[0], c[1]), corner(c[2], c[3]));
    }
    Ok(())
}
