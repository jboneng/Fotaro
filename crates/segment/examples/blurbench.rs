//! Try Lens Blur on a real photo:
//! `cargo run -p segment --example blurbench --release -- <models dir> <out dir> <photo> [key=value ...]`
//!
//! The models dir holds Depth Anything V2 Small's `model.onnx`. Keys: `amount`, `focus`,
//! `range`, `highlights` (Lens Blur's sliders), `light`, `warm`, `depth`, `lrange`,
//! `feather`, `size`, `near`, `far`, `warmnear`, `warmfar` (Relight's), `lat=x,y` (put
//! Relight's light on that point of the photo, 0..1), `lshow` (show where it falls),
//! `show` (the depth map in place of the photo), `proxy` (the long side rendered, default
//! 2560; 0 for full size), `at=x,y` (take Focus from that point of the photo, 0..1),
//! `crop=x,y,w,h` (render only that part, in pixels) and `name` (the output's name:
//! `<photo>_<name>.png`, `blur` unless given). The photo's depth map is made once and
//! kept in the out dir. Prints the time the render took.

use std::path::PathBuf;
use std::time::Instant;

use image::{GrayImage, Luma};
use imaging::SourceKind;
use imaging::develop::{DevelopSettings, Region, WorkingImage, render_region_srgb8, render_srgb8};
use segment::{Device, Model};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [models, out, photo, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: blurbench <models dir> <out dir> <photo> [key=value ...]")
    };
    let (models, out, photo) = (PathBuf::from(models), PathBuf::from(out), PathBuf::from(photo));
    std::fs::create_dir_all(&out)?;
    let value = |key: &str| rest.iter().find_map(|a| a.strip_prefix(key)?.strip_prefix('='));
    let number = |key: &str, default: f32| value(key).and_then(|v| v.parse().ok()).unwrap_or(default);
    let raw = photo.extension().is_some_and(|e| ["nef", "dng"].iter().any(|r| e.eq_ignore_ascii_case(r)));
    let kind = if raw { SourceKind::Raw } else { SourceKind::Image };
    let stem = photo.file_stem().and_then(|s| s.to_str()).unwrap_or("photo").to_owned();

    imaging::develop::set_mask_store(Some(out.clone()));
    let id = format!("{stem}_depth");
    if imaging::develop::mask_picture(&id).is_none() {
        let (img, _) = imaging::decode(&photo, kind, 2048)?;
        // The GPU first; the CPU if it fails (or gives nothing).
        let (near, w, h) = [Device::DirectMl, Device::Cpu]
            .into_iter()
            .find_map(|device| {
                let made = Model::load(&models.join("model.onnx"), device)
                    .and_then(|mut model| segment::depth(&mut model, &img, 1036));
                made.inspect_err(|e| println!("depth on {}: {e:#}", device.label())).ok()
            })
            .ok_or_else(|| anyhow::anyhow!("no depth map"))?;
        let picture = GrayImage::from_fn(w, h, |x, y| {
            Luma([(near[(y * w + x) as usize].max(0.0).sqrt() * 255.0).round() as u8])
        });
        imaging::develop::save_mask_picture(&id, &picture)?;
    }

    let proxy = number("proxy", 2560.0) as u32;
    let (img, _) = WorkingImage::load(&photo, kind, (proxy > 0).then_some(proxy))?;
    let mut s = DevelopSettings::default();
    s.effects.fog_depth = Some(id.clone());
    s.effects.blur_amount = number("amount", 50.0);
    s.effects.blur_focus = number("focus", s.effects.blur_focus);
    s.effects.blur_range = number("range", s.effects.blur_range);
    s.effects.blur_highlights = number("highlights", 0.0);
    s.effects.fog_show = value("show").is_some();
    s.effects.relight_amount = number("light", 0.0);
    s.effects.relight_warmth = number("warm", 0.0);
    s.effects.relight_range = number("lrange", s.effects.relight_range);
    s.effects.relight_size = number("size", s.effects.relight_size);
    s.effects.relight_show = value("lshow").is_some();
    s.effects.relight_near = number("near", 0.0);
    s.effects.relight_far = number("far", 0.0);
    s.effects.relight_depth = number("depth", s.effects.relight_depth);
    s.effects.relight_feather = number("feather", s.effects.relight_feather);
    s.effects.relight_warmth_near = number("warmnear", 0.0);
    s.effects.relight_warmth_far = number("warmfar", 0.0);
    if let Some((x, y)) = value("at").and_then(|v| v.split_once(',')) {
        let far = imaging::develop::depth_at(&id, x.parse()?, y.parse()?);
        println!("at {x}, {y}: {far:?}");
        s.effects.blur_focus = far.unwrap_or(s.effects.blur_focus);
    }
    if let Some((x, y)) = value("lat").and_then(|v| v.split_once(',')) {
        let (x, y): (f32, f32) = (x.parse()?, y.parse()?);
        let far = imaging::develop::depth_at(&id, x, y);
        println!("light at {x}, {y}: {far:?}");
        s.effects.relight_depth = far.unwrap_or(s.effects.relight_depth);
        s.effects.relight_at = [x, y];
    }
    let crop: Option<Vec<u32>> = value("crop").map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect());
    let render = |s: &DevelopSettings| match crop.as_deref() {
        Some(&[x, y, w, h]) => render_region_srgb8(&img, s, Region { x, y, w, h }),
        _ => render_srgb8(&img, s),
    };
    // Twice: the first also fits the depth map's edges to the photo, once a photo.
    let t = Instant::now();
    render(&s);
    let first = t.elapsed();
    s.effects.blur_amount += 0.01;
    let t = Instant::now();
    let rendered = render(&s);
    println!(
        "{}x{}: {:.2?}, then {:.2?} (amount {}, focus {})",
        img.width(),
        img.height(),
        first,
        t.elapsed(),
        s.effects.blur_amount,
        s.effects.blur_focus
    );
    let path = out.join(format!("{stem}_{}.png", value("name").unwrap_or("blur")));
    rendered.save(&path)?;
    println!("{}", path.display());
    Ok(())
}
