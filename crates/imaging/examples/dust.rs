//! Try Dust & Scratches and sensor-dust detection on a photo:
//! `cargo run -p imaging --example dust --release -- <out dir> <photo> [IR pass]`
//!
//! Detects film defects from the picture, the IR pass and both (`PL_DUST_SENS`,
//! `PL_DUST_SIZE` set sensitivity and max size), times them, and saves crops of the
//! photo (levels stretched, as scans are dim negatives): found defects in red, and filled.
//! Also runs sensor-dust finding and saves the photo small with the blobs ringed.

use std::path::{Path, PathBuf};
use std::time::Instant;

use image::{Rgb, RgbImage};
use imaging::SourceKind;
use imaging::develop::{
    DustLayer, DustPolarity, WorkingImage, detect_both, detect_ir, detect_picture_as, dust_lightness, fill_masked,
    find_sensor_dust,
};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (out, photo, ir) = match args.as_slice() {
        [o, p] => (PathBuf::from(o), PathBuf::from(p), None),
        [o, p, i] => (PathBuf::from(o), PathBuf::from(p), Some(PathBuf::from(i))),
        _ => anyhow::bail!("usage: dust <out dir> <photo> [IR pass]"),
    };
    std::fs::create_dir_all(&out)?;
    let sens: f32 = std::env::var("PL_DUST_SENS").ok().and_then(|v| v.parse().ok()).unwrap_or(50.0);
    let size: f32 = std::env::var("PL_DUST_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(12.0);
    let raw = ["nef", "dng", "cr2", "cr3", "arw"]
        .iter()
        .any(|e| photo.extension().is_some_and(|x| x.eq_ignore_ascii_case(e)));
    let kind = if raw { SourceKind::Raw } else { SourceKind::Image };
    let t = Instant::now();
    let (img, _) = WorkingImage::load(&photo, kind, None)?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    println!("{} {w}×{h}, loaded in {:.1?}", photo.display(), t.elapsed());
    // PL_SENSOR_TEST: sensor dust's recall on synthetic blobs over this photo at the app's
    // proxy size (soft, round, a few % darker), and what else it finds.
    if std::env::var_os("PL_SENSOR_TEST").is_some() {
        let small = img.downscaled(2048);
        let (sw, sh) = (small.width() as usize, small.height() as usize);
        let clean = dust_lightness(&small);
        let mut seed = 12345u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as f32 / (1u64 << 31) as f32
        };
        let blobs: Vec<(f32, f32, f32, f32)> =
            (0..40).map(|_| (rnd() * sw as f32, rnd() * sh as f32, 3.0 + rnd() * 9.0, 0.02 + rnd() * 0.04)).collect();
        let mut lum = clean.clone();
        for &(cx, cy, r, depth) in &blobs {
            for y in 0..sh {
                for x in 0..sw {
                    let d2 = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)) / (r * r);
                    if d2 < 9.0 {
                        // Darker by `depth` in linear light: lightness is its square root.
                        let i = y * sw + x;
                        lum[i] *= (1.0 - depth * (-d2).exp()).sqrt();
                    }
                }
            }
        }
        for sens in [30.0, 50.0, 70.0, 90.0] {
            let found = find_sensor_dust(&lum, sw, sh, sens);
            let before = find_sensor_dust(&clean, sw, sh, sens).len();
            let hit = blobs
                .iter()
                .filter(|(cx, cy, r, _)| {
                    found.iter().any(|(c, _)| (c[0] * sw as f32 - cx).hypot(c[1] * sh as f32 - cy) < r.max(4.0))
                })
                .count();
            println!("sensitivity {sens}: {hit}/40 blobs found, {} finds in all, {before} on the clean photo", found.len());
        }
        return Ok(());
    }
    let px = img.pixels.as_raw();
    let lum = dust_lightness(&img);
    let stem = photo.file_stem().map_or("photo".into(), |s| s.to_string_lossy().into_owned());

    let mut masks: Vec<(&str, Vec<u8>)> = Vec::new();
    let t = Instant::now();
    let polarity = match std::env::var("PL_DUST_POL").as_deref() {
        Ok("dark") => DustPolarity::Dark,
        Ok("both") => DustPolarity::Both,
        _ => DustPolarity::Light,
    };
    let pic = detect_picture_as(&lum, w, h, sens, size, polarity);
    println!("picture: {:.1?}, {} px", t.elapsed(), pic.iter().filter(|v| **v > 0).count());
    // PL_DUST_MASK: only the masks, as `<stem>.picture.png` (and `<stem>.ir.png` with an
    // IR pass), for measuring.
    if std::env::var_os("PL_DUST_MASK").is_some() {
        let save = |mask: Vec<u8>, name: &str| -> anyhow::Result<()> {
            image::GrayImage::from_raw(w as u32, h as u32, mask)
                .ok_or_else(|| anyhow::anyhow!("mask size"))?
                .save(out.join(format!("{stem}.{name}.png")))?;
            Ok(())
        };
        save(pic, "picture")?;
        if let Some(ir) = &ir {
            let irimg = image::open(ir)?.to_luma32f();
            let valid = valid_mask(ir).filter(|v| v.len() == w * h);
            save(detect_ir(irimg.as_raw(), &lum, valid.as_deref(), w, h, sens, size), "ir")?;
        }
        return Ok(());
    }
    masks.push(("picture", pic.clone()));
    if let Some(ir) = &ir {
        let t = Instant::now();
        let irimg = image::open(ir)?.to_luma32f();
        anyhow::ensure!(irimg.width() as usize == w && irimg.height() as usize == h, "the IR pass is another size");
        let valid = valid_mask(ir).filter(|v| v.len() == w * h);
        let ird = detect_ir(irimg.as_raw(), &lum, valid.as_deref(), w, h, sens, size);
        println!(
            "ir:      {:.1?}, {} px{}",
            t.elapsed(),
            ird.iter().filter(|v| **v > 0).count(),
            if valid.is_some() { " (with the valid mask)" } else { "" }
        );
        let t = Instant::now();
        let both = detect_both(
            &lum,
            irimg.as_raw(),
            valid.as_deref(),
            w,
            h,
            &DustLayer { sensitivity: sens, max_size: size, polarity, ..Default::default() },
        );
        println!("both:    {:.1?}, {} px", t.elapsed(), both.iter().filter(|v| **v > 0).count());
        masks.push(("ir", ird));
        masks.push(("both", both));
    }

    // The whole frame, about 1600 px.
    {
        let f = (w.max(h) / 1600).max(1);
        let (sw, sh) = (w / f, h / f);
        let mut over = RgbImage::from_fn(sw as u32, sh as u32, |x, y| {
            let v = lum[(y as usize * f) * w + x as usize * f];
            Rgb([(v * 200.0) as u8; 3])
        });
        for y in 0..sh * f {
            for x in 0..sw * f {
                let i = y * w + x;
                // With an IR pass: green IR, red what Both adds; else red the picture finds.
                let r = masks.get(1).is_some_and(|m| m.1[i] > 0);
                let p = masks.last().is_some_and(|m| m.1[i] > 0) && !r;
                if p || r {
                    let o = over.get_pixel_mut((x / f) as u32, (y / f) as u32);
                    o[0] = if p { 255 } else { o[0] / 3 };
                    o[1] = if r { 255 } else { o[1] / 3 };
                    o[2] /= 3;
                }
            }
        }
        over.save(out.join(format!("{stem}.overview.png")))?;
    }

    // Crops: the centre and two others, 700 px.
    let c = 700.min(w).min(h);
    let spots = [(w / 2 - c / 2, h / 2 - c / 2), (w / 5, h / 4), (w * 3 / 5, h * 3 / 5)];
    for (k, &(x0, y0)) in spots.iter().enumerate() {
        let crop = |p: &[f32], x: usize, y: usize| {
            let i = 3 * ((y0 + y) * w + x0 + x);
            [p[i], p[i + 1], p[i + 2]]
        };
        // Levels stretched per crop, so dim negatives show.
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for y in 0..c {
            for x in 0..c {
                let v = crop(px, x, y);
                for ch in 0..3 {
                    (lo[ch], hi[ch]) = (lo[ch].min(v[ch]), hi[ch].max(v[ch]));
                }
            }
        }
        let show = |v: [f32; 3]| Rgb(std::array::from_fn(|ch| (((v[ch] - lo[ch]) / (hi[ch] - lo[ch]).max(1e-6)).sqrt() * 255.0) as u8));
        let mut sheet = RgbImage::new(c as u32 * (1 + 2 * masks.len() as u32), c as u32);
        for y in 0..c {
            for x in 0..c {
                sheet.put_pixel(x as u32, y as u32, show(crop(px, x, y)));
            }
        }
        for (m, (_, mask)) in masks.iter().enumerate() {
            let mut region: Vec<f32> = (0..c * c).flat_map(|j| crop(px, j % c, j / c)).collect();
            let rmask: Vec<u8> = (0..c * c).map(|j| mask[(y0 + j / c) * w + x0 + j % c]).collect();
            fill_masked(&mut region, c, c, &rmask);
            for y in 0..c {
                for x in 0..c {
                    let found = rmask[y * c + x] > 0;
                    let mut p = show(crop(px, x, y));
                    if found {
                        p = Rgb([255, p[1] / 3, p[2] / 3]);
                    }
                    sheet.put_pixel(((1 + 2 * m) * c + x) as u32, y as u32, p);
                    let j = 3 * (y * c + x);
                    sheet.put_pixel(((2 + 2 * m) * c + x) as u32, y as u32, show([region[j], region[j + 1], region[j + 2]]));
                }
            }
        }
        let name = format!("{stem}.crop{k}.png");
        sheet.save(out.join(&name))?;
        println!("{name}: original | {}", masks.iter().map(|(n, _)| format!("{n} found | {n} filled")).collect::<Vec<_>>().join(" | "));
    }

    // Sensor dust.
    let t = Instant::now();
    let found = find_sensor_dust(&lum, w, h, sens);
    println!("sensor dust: {} found in {:.1?}", found.len(), t.elapsed());
    let f = (w.max(h) / 1600).max(1);
    let (sw, sh) = (w / f, h / f);
    let mut small = RgbImage::from_fn(sw as u32, sh as u32, |x, y| {
        let v = lum[(y as usize * f) * w + x as usize * f];
        Rgb([(v * 255.0) as u8; 3])
    });
    let half = w.max(h) as f32 / 2.0;
    for (c, r) in &found {
        let (cx, cy, rr) = (c[0] * sw as f32, c[1] * sh as f32, r * half / f as f32);
        for k in 0..64 {
            let a = k as f32 / 64.0 * std::f32::consts::TAU;
            let (x, y) = (cx + a.cos() * rr, cy + a.sin() * rr);
            if x >= 0.0 && y >= 0.0 && (x as usize) < sw && (y as usize) < sh {
                small.put_pixel(x as u32, y as u32, Rgb([255, 40, 40]));
            }
        }
    }
    small.save(out.join(format!("{stem}.sensor.png")))?;
    Ok(())
}

/// NegPy's `<name>_IR_valid.tif` beside an IR pass `<name>_IR.tif`, as non-zero where valid.
fn valid_mask(ir: &Path) -> Option<Vec<u8>> {
    let stem = ir.file_stem()?.to_string_lossy();
    let ext = ir.extension()?.to_string_lossy();
    let path = ir.with_file_name(format!("{stem}_valid.{ext}"));
    let img = image::open(path).ok()?.to_luma8();
    Some(img.into_raw())
}
