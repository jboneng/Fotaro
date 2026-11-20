//! Time a proxy render after one setting changes, as dragging a slider does:
//! `cargo run -p imaging --example renderbench [--release] -- <file> [proxy px, default 1280]`
//!
//! The photo starts from a typical edit (RAW defaults, some tone and presence, one radial
//! and one brush mask); each scenario then alternates one setting between two values and
//! reports the median render time.

use std::time::{Duration, Instant};

use imaging::SourceKind;
use imaging::develop::{DevelopSettings, Mask, Shape, Spot, Stroke, WorkingImage, render_srgb8};

// The app's allocator, so timings match the app.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const RUNS: usize = 7;
const PAUSE_MS: u64 = 2500;

/// The fastest and the median run (a laptop under other load is noisy: the fastest is the
/// steadier number).
fn best_median(mut t: Vec<Duration>) -> (f64, f64) {
    t.sort();
    (t[0].as_secs_f64() * 1000.0, t[t.len() / 2].as_secs_f64() * 1000.0)
}

/// Render `a`, then time renders alternating `b` and `a` (each differs from the last).
fn scenario(name: &str, img: &WorkingImage, a: &DevelopSettings, b: &DevelopSettings) {
    // `PL_ONLY=<text>`: only the scenarios whose name has it (with `PL_STAGES=1`, the stages
    // of each render are printed too).
    if std::env::var("PL_ONLY").is_ok_and(|only| !name.contains(&only)) {
        return;
    }
    render_srgb8(img, a);
    // Let the CPU cool: this laptop slows to a half or a third after seconds of full load,
    // which would make later scenarios look slower than earlier ones.
    std::thread::sleep(Duration::from_millis(PAUSE_MS));
    let times = (0..RUNS)
        .map(|i| {
            let s = if i % 2 == 0 { b } else { a };
            let t = Instant::now();
            std::hint::black_box(render_srgb8(img, s));
            t.elapsed()
        })
        .collect();
    let (best, median) = best_median(times);
    println!("{name:28} {best:7.1} ms   (median {median:.1})");
}

fn stroke(n: usize) -> Stroke {
    let points = (0..n).map(|i| [0.2 + 0.5 * i as f32 / 200.0, 0.3 + 0.1 * (i as f32 / 9.0).sin()]).collect();
    Stroke { points, size: 0.04, ..Stroke::default() }
}

fn spot(x: f32, y: f32) -> Spot {
    Spot { points: vec![[x, y]], size: 0.015, offset: [0.04, 0.0], ..Spot::default() }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("usage: renderbench <file> [proxy px]"));
    let px: u32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(1280);
    let raw = ["nef", "dng", "cr2", "cr3", "arw", "raf", "orf", "rw2"]
        .iter()
        .any(|e| path.extension().is_some_and(|x| x.eq_ignore_ascii_case(e)));
    let kind = if raw { SourceKind::Raw } else { SourceKind::Image };
    let (img, _) = WorkingImage::load(&path, kind, Some(px)).expect("load");
    println!("{} at {}x{}", path.display(), img.width(), img.height());

    let plain = DevelopSettings::default_for(false);
    scenario("plain: exposure", &img, &plain, &DevelopSettings { exposure: 0.3, ..plain.clone() });

    // A typical edit.
    let mut base = DevelopSettings::default_for(raw);
    base.exposure = 0.3;
    base.highlights = -40.0;
    base.shadows = 30.0;
    base.texture = 20.0;
    base.clarity = 15.0;
    base.vibrance = 20.0;
    base.detail.sharpen_amount = 40.0;
    base.detail.color = 25.0;
    let mut radial = Mask::new(
        "Radial",
        Shape::Radial { center: [0.5, 0.5], radii: [0.3, 0.2], angle: 0.0, feather: 50.0 },
    );
    radial.adjust.exposure = 0.5;
    let mut brush = Mask::new("Brush", Shape::Brush { strokes: vec![stroke(120)] });
    brush.adjust.saturation = 30.0;
    base.masks = vec![radial, brush];
    let with = |f: &dyn Fn(&mut DevelopSettings)| {
        let mut s = base.clone();
        f(&mut s);
        s
    };

    // What each part of the edit costs: the edit without it.
    let cost = |name: &str, f: &dyn Fn(&mut DevelopSettings)| {
        let a = with(f);
        scenario(name, &img, &a, &DevelopSettings { exposure: 0.6, ..a.clone() });
    };
    println!("-- the edit without one part (exposure changes)");
    cost("all of it", &|_| {});
    cost("without detail", &|s| s.detail = Default::default());
    cost("without colour noise", &|s| s.detail.color = 0.0);
    cost("without sharpening", &|s| s.detail.sharpen_amount = 0.0);
    cost("without masks", &|s| s.masks.clear());
    cost("without texture, clarity", &|s| (s.texture, s.clarity) = (0.0, 0.0));
    cost("without highlights, shadows", &|s| (s.highlights, s.shadows) = (0.0, 0.0));
    cost("without vibrance", &|s| s.vibrance = 0.0);
    println!("-- one setting changes");
    scenario("exposure", &img, &base, &with(&|s| s.exposure = 0.6));
    scenario("contrast", &img, &base, &with(&|s| s.contrast = 25.0));
    scenario("highlights", &img, &base, &with(&|s| s.highlights = -60.0));
    scenario("texture", &img, &base, &with(&|s| s.texture = 40.0));
    scenario("dehaze", &img, &with(&|s| s.dehaze = 20.0), &with(&|s| s.dehaze = 40.0));
    scenario("saturation", &img, &base, &with(&|s| s.saturation = 20.0));
    scenario("temp", &img, &base, &with(&|s| s.temp += if raw { 300.0 } else { 10.0 }));
    scenario("sharpen amount", &img, &base, &with(&|s| s.detail.sharpen_amount = 70.0));
    scenario(
        "luminance noise",
        &img,
        &with(&|s| s.detail.luminance = 20.0),
        &with(&|s| s.detail.luminance = 40.0),
    );
    scenario(
        "grain",
        &img,
        &with(&|s| s.effects.grain_amount = 20.0),
        &with(&|s| s.effects.grain_amount = 40.0),
    );
    scenario(
        "vignette",
        &img,
        &with(&|s| s.effects.vignette_amount = -20.0),
        &with(&|s| s.effects.vignette_amount = -40.0),
    );
    scenario(
        "glow",
        &img,
        &with(&|s| s.effects.glow_amount = 20.0),
        &with(&|s| s.effects.glow_amount = 40.0),
    );
    scenario("mask exposure", &img, &base, &with(&|s| s.masks[0].adjust.exposure = 0.8));
    scenario(
        "mask move (radial)",
        &img,
        &base,
        &with(&|s| {
            if let Shape::Radial { center, .. } = &mut s.masks[0].components[0].shape {
                center[0] = 0.52;
            }
        }),
    );
    scenario(
        "brush: one more point",
        &img,
        &base,
        &with(&|s| {
            if let Shape::Brush { strokes } = &mut s.masks[1].components[0].shape {
                strokes[0] = stroke(121);
            }
        }),
    );
    let spots = with(&|s| s.spots = (0..20).map(|i| spot(0.1 + 0.04 * i as f32, 0.6)).collect());
    scenario(
        "spots: slider with 20 spots",
        &img,
        &spots,
        &DevelopSettings { exposure: 0.6, ..spots.clone() },
    );
    scenario("spots: one more (21st)", &img, &spots, &{
        let mut s = spots.clone();
        s.spots.push(spot(0.5, 0.2));
        s
    });
    scenario("spots: move one", &img, &spots, &{
        let mut s = spots.clone();
        s.spots[3].points[0][1] = 0.61;
        s
    });
    scenario(
        "rotate (crop angle)",
        &img,
        &with(&|s| s.transform.crop.angle = 1.0),
        &with(&|s| s.transform.crop.angle = 1.5),
    );
    scenario(
        "lens: manual distortion",
        &img,
        &with(&|s| s.transform.lens.distortion = 10.0),
        &with(&|s| s.transform.lens.distortion = 20.0),
    );
    scenario(
        "slider while rotated",
        &img,
        &with(&|s| s.transform.crop.angle = 1.0),
        &with(&|s| {
            s.transform.crop.angle = 1.0;
            s.exposure = 0.6;
        }),
    );
    // The draft shown while a control moves: the same edit on a quarter of the pixels.
    let draft = img.downscaled(img.width().max(img.height()).div_ceil(2));
    scenario("draft: exposure", &draft, &base, &with(&|s| s.exposure = 0.6));
    scenario("draft: saturation", &draft, &base, &with(&|s| s.saturation = 20.0));
    scenario("draft: sharpen amount", &draft, &base, &with(&|s| s.detail.sharpen_amount = 70.0));
    // The Navigator's preset preview: the same edit on a small copy.
    let small = img.downscaled(320);
    scenario("preview at 320 px", &small, &base, &with(&|s| s.exposure = 0.6));
}
