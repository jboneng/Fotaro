//! Measure Remove's fills against the truth: a hole is put where nothing needs removing,
//! filled as the app fills it, and compared with what was there.
//! `cargo run -p segment --example fillbench --release -- <out dir> [photo ...]`
//!
//! Without photos, those in `testdata/real` that are there. Each photo gets a tall hole
//! (the size of a person near the camera) and a small one. Per hole it prints how far the
//! coarse picture is from the truth, and how grainy and how coarse-grained the fill is
//! against the truth (1.00: the same), for the model's picture as it was shown before
//! ("plain") and with the photo's own detail ("detail"); the step in tone across the
//! hole's border (of 255) in the truth, with detail only, and with the border matched;
//! and writes `<name>.png`: truth | plain | detail | detail with the border matched, at
//! full size, around the hole's middle.

use std::path::{Path, PathBuf};

use image::{GrayImage, Luma, RgbImage, imageops};
use imaging::SourceKind;
use imaging::develop::{
    DevelopSettings, Fill, FillInput, Finish, Spot, SpotMode, WorkingImage, fill_input, finish_fill_with,
    healed_crop, save_fill_picture, set_mask_store,
};

const PHOTOS: &[&str] = &[
    "testdata/real/film/20260926_001.tiff",
    "testdata/real/film/20260926_003.tiff",
    "testdata/real/tiff/20261002_001.tif",
    "testdata/real/other/_JMB5147.jpg",
    "testdata/real/nef/_JMB4583.NEF",
];

/// The holes: a name, the middle (0..1 of the photo), half the length (0..1 of the
/// height) and the brush radius (in half long sides).
const HOLES: &[(&str, [f32; 2], f32, f32)] = &[
    ("tall", [0.42, 0.5], 0.16, 0.09),
    ("tall2", [0.7, 0.55], 0.12, 0.06),
    ("small", [0.3, 0.3], 0.0, 0.035),
];

/// Pixels of `img` less the mean of their four neighbours `d` away, squared and summed
/// where `inside`, as a spread.
fn roughness(img: &RgbImage, inside: &GrayImage, d: u32) -> f32 {
    let (mut sum, mut n) = (0.0f64, 0u64);
    for y in d..img.height() - d {
        for x in d..img.width() - d {
            if inside.get_pixel(x, y)[0] > 0 {
                let at = |x: u32, y: u32| f32::from(img.get_pixel(x, y)[1]);
                let v = at(x, y) - 0.25 * (at(x - d, y) + at(x + d, y) + at(x, y - d) + at(x, y + d));
                sum += f64::from(v * v);
                n += 1;
            }
        }
    }
    (sum / n.max(1) as f64).sqrt() as f32
}

/// How far the coarse pictures (at the model's size) are apart inside the hole.
fn coarse_error(a: &RgbImage, b: &RgbImage, inside: &GrayImage) -> f32 {
    let s = 512;
    let small = |i: &RgbImage| imageops::resize(i, s, s, imageops::FilterType::Triangle);
    let (a, b) = (small(a), small(b));
    let m = imageops::resize(inside, s, s, imageops::FilterType::Triangle);
    let (mut sum, mut n) = (0.0f64, 0u64);
    for (x, y, p) in a.enumerate_pixels() {
        if m.get_pixel(x, y)[0] == 255 {
            let q = b.get_pixel(x, y);
            for c in 0..3 {
                sum += f64::from((f32::from(p[c]) - f32::from(q[c])).powi(2));
            }
            n += 3;
        }
    }
    (sum / n.max(1) as f64).sqrt() as f32
}

/// The step in tone across the hole's border (of 255, green): at the model's size, the
/// mean just inside against the mean just outside, along the border.
fn border_step(img: &RgbImage, inside: &GrayImage) -> f32 {
    let s = 512u32;
    let a = imageops::resize(img, s, s, imageops::FilterType::Triangle);
    let m = imageops::resize(inside, s, s, imageops::FilterType::Triangle);
    let hole = |x: i64, y: i64| m.get_pixel(x.clamp(0, 511) as u32, y.clamp(0, 511) as u32)[0] > 0;
    let (mut sum, mut n) = (0.0f64, 0u64);
    for y in 0..i64::from(s) {
        for x in 0..i64::from(s) {
            if !hole(x, y) || (hole(x - 1, y) && hole(x + 1, y) && hole(x, y - 1) && hole(x, y + 1)) {
                continue;
            }
            let (mut i, mut ni, mut o, mut no) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            for dy in -4..=4 {
                for dx in -4..=4 {
                    let (px, py) = (x + dx, y + dy);
                    if px < 0 || py < 0 || px >= i64::from(s) || py >= i64::from(s) {
                        continue;
                    }
                    let v = f32::from(a.get_pixel(px as u32, py as u32)[1]);
                    if hole(px, py) {
                        (i, ni) = (i + v, ni + 1.0);
                    } else {
                        (o, no) = (o + v, no + 1.0);
                    }
                }
            }
            if ni > 0.0 && no > 0.0 {
                sum += f64::from((i / ni - o / no).abs());
                n += 1;
            }
        }
    }
    (sum / n.max(1) as f64) as f32
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((out, photos)) = args.split_first() else {
        anyhow::bail!("usage: fillbench <out dir> [photo ...]")
    };
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out)?;
    let store = out.join("fills");
    set_mask_store(Some(store.clone()));
    let photos: Vec<PathBuf> = if photos.is_empty() {
        PHOTOS.iter().map(PathBuf::from).filter(|p| p.exists()).collect()
    } else {
        photos.iter().map(PathBuf::from).collect()
    };
    println!(
        "{:<30} {:>5} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
        "hole",
        "crop",
        "coarse",
        "coarse",
        "coarse",
        "grain",
        "grain",
        "size",
        "step",
        "step",
        "step",
        "took"
    );
    println!(
        "{:<30} {:>5} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
        "", "px", "plain", "detail", "border", "plain", "border", "border", "truth", "detail", "border", ""
    );
    for path in &photos {
        let raw = path
            .extension()
            .is_some_and(|e| ["nef", "dng", "cr2", "arw"].iter().any(|r| e.eq_ignore_ascii_case(r)));
        let kind = if raw { SourceKind::Raw } else { SourceKind::Image };
        let (img, _) = WorkingImage::load(path, kind, None)?;
        let wb = img.resolve_wb(&DevelopSettings::default()).multipliers;
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("photo").to_owned();
        for (k, (name, middle, half, size)) in HOLES.iter().enumerate() {
            let points = if *half > 0.0 {
                vec![[middle[0], middle[1] - half], [middle[0], middle[1] + half]]
            } else {
                vec![*middle]
            };
            let spot = Spot {
                mode: SpotMode::Remove,
                points,
                size: *size,
                feather: 10.0,
                opacity: 100.0,
                ..Spot::default()
            };
            let Some(input) = fill_input(&img, std::slice::from_ref(&spot), None, 0, wb) else { continue };
            let filled = model(&input)?;
            let side = input.image.width();
            // As it was: the model's picture at no more than 1024 px, over the whole crop.
            let plain_side = side.min(1024);
            let plain = imageops::resize(&filled, plain_side, plain_side, imageops::FilterType::Lanczos3);
            let shown = |id: &str,
                         picture: &RgbImage,
                         rect: [f32; 4],
                         detail: f32,
                         full: bool|
             -> anyhow::Result<RgbImage> {
                save_fill_picture(id, picture)?;
                let fill = Fill {
                    id: id.to_owned(),
                    rect,
                    gains: input.gains,
                    made_for: spot.shape_key(),
                    model: "bench".into(),
                    detail,
                    full,
                    variant: 0,
                };
                let with = Spot { fill: Some(fill), ..spot.clone() };
                Ok(healed_crop(&img, &[with], None, input.rect, input.gains))
            };
            let before = shown(&format!("{stem}-{k}-plain"), &plain, input.rect, 0.0, false)?;
            let started = std::time::Instant::now();
            let how = Finish { seed: spot.shape_key(), ..Finish::default() };
            let done = finish_fill_with(&input, &filled, &how);
            let took = started.elapsed();
            let after =
                shown(&format!("{stem}-{k}-border"), &done.picture, done.rect, done.detail, done.full)?;
            let old = finish_fill_with(&input, &filled, &Finish { border: false, ..how });
            let unmatched =
                shown(&format!("{stem}-{k}-detail"), &old.picture, old.rect, old.detail, old.full)?;
            let truth = &input.image;
            let r = |i: &RgbImage, d: u32| roughness(i, &input.mask, d);
            let (t1, t2) = (r(truth, 1), r(truth, 2));
            let ratio =
                |i: &RgbImage| (r(i, 1) / t1.max(1e-6), (r(i, 2) / r(i, 1).max(1e-6)) / (t2 / t1.max(1e-6)));
            let ((gb, _), (ga, sa)) = (ratio(&before), ratio(&after));
            println!(
                "{:<30} {:>5} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>6.1}s",
                format!("{stem} {name}"),
                side,
                coarse_error(truth, &before, &input.mask),
                coarse_error(truth, &unmatched, &input.mask),
                coarse_error(truth, &after, &input.mask),
                gb,
                ga,
                sa,
                border_step(truth, &input.mask),
                border_step(&unmatched, &input.mask),
                border_step(&after, &input.mask),
                took.as_secs_f32()
            );
            sheet(
                &out.join(format!("{stem}_{name}.png")),
                &[truth, &before, &unmatched, &after],
                &input.mask,
            )?;
        }
    }
    let _ = std::fs::remove_dir_all(store);
    Ok(())
}

/// The model's fill of the input, as the app asks for it (at 512 px).
fn model(input: &FillInput) -> anyhow::Result<RgbImage> {
    let s = segment::inpaint::SIZE;
    let img = imageops::resize(&input.image, s, s, imageops::FilterType::Lanczos3);
    let mask = imageops::resize(&input.model_mask(), s, s, imageops::FilterType::Triangle);
    let mask = GrayImage::from_fn(s, s, |x, y| Luma([if mask.get_pixel(x, y)[0] > 0 { 255 } else { 0 }]));
    Ok(segment::remove(&img, &mask, true, true)?.0)
}

/// The pictures side by side at full size: a window around the hole's middle (the first
/// with the hole's edge marked).
fn sheet(path: &Path, pictures: &[&RgbImage], mask: &GrayImage) -> anyhow::Result<()> {
    let side = pictures[0].width();
    let win = side.min(760);
    let (mut sx, mut sy, mut n) = (0u64, 0u64, 0u64);
    for (x, y, p) in mask.enumerate_pixels() {
        if p[0] > 0 {
            (sx, sy, n) = (sx + u64::from(x), sy + u64::from(y), n + 1);
        }
    }
    let (cx, cy) = ((sx / n.max(1)) as u32, (sy / n.max(1)) as u32);
    let (x0, y0) = (cx.saturating_sub(win / 2).min(side - win), cy.saturating_sub(win / 2).min(side - win));
    let mut all = RgbImage::new(win * pictures.len() as u32 + 8 * (pictures.len() as u32 - 1), win);
    for (k, p) in pictures.iter().enumerate() {
        let mut part = imageops::crop_imm(*p, x0, y0, win, win).to_image();
        if k == 0 {
            for (x, y, px) in part.enumerate_pixels_mut() {
                let at = |dx: u32, dy: u32| {
                    mask.get_pixel((x0 + x + dx).min(side - 1), (y0 + y + dy).min(side - 1))[0] > 0
                };
                if at(0, 0) != at(1, 0) || at(0, 0) != at(0, 1) {
                    *px = image::Rgb([255, 140, 0]);
                }
            }
        }
        imageops::replace(&mut all, &part, i64::from(k as u32 * (win + 8)), 0);
    }
    all.save(path)?;
    Ok(())
}
