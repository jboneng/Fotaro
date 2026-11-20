//! Compare our lens corrections with reference numbers from Lensfun itself:
//! `cargo run -p lensdb --example lenscompare -- <db dir> <reference.json>`
//!
//! The reference holds, per lens / focal / aperture, the source coordinates Lensfun maps
//! sample pixels to (geometry, per channel) and its vignetting gain there (pixel indices,
//! centres at integers). Prints the largest differences.

use imaging::develop::{LensCorrections, lens_correction_at};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [dir, reference] = args.as_slice() else { anyhow::bail!("usage: lenscompare <db dir> <reference.json>") };
    let db = lensdb::Database::load(std::path::Path::new(dir))?;
    println!("{} cameras, {} lenses", db.cameras.len(), db.lenses.len());
    let refs: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(reference)?)?;
    let (mut worst_geo, mut worst_sub, mut worst_vig) = (0f64, 0f64, 0f64);
    for r in refs.as_array().into_iter().flatten() {
        let name = r["lens"].as_str().unwrap_or_default();
        let Some(lens) = db.lenses.iter().find(|l| l.model == name) else {
            println!("missing {name}");
            continue;
        };
        let f = |k: &str| r[k].as_f64().unwrap_or(0.0) as f32;
        let (w, h) = (r["w"].as_u64().unwrap_or(1) as u32, r["h"].as_u64().unwrap_or(1) as u32);
        let profile = lensdb::resolve(lens, "Nikon D750", 1.0, f("focal"), f("aperture"), f("distance"));
        // Lensfun's "subpixel" map is the TCA alone: compare it with ours without distortion.
        // (Its vignetting is measured at the pixel itself: compare that without distortion too.)
        let tca_only = imaging::develop::LensProfile { distortion: None, ..profile.clone() };
        let lc_tca = LensCorrections { profile_enable: true, profile: Some(tca_only), ..Default::default() };
        let lc = LensCorrections { profile_enable: true, profile: Some(profile), ..Default::default() };
        let verbose = std::env::var_os("PL_VERBOSE").is_some();
        let (mut g, mut s, mut v) = (0f64, 0f64, 0f64);
        for p in r["points"].as_array().into_iter().flatten() {
            let (x, y) = (p["x"].as_f64().unwrap_or(0.0), p["y"].as_f64().unwrap_or(0.0));
            let (pts, _) = lens_correction_at(&lc, w, h, x + 0.5, y + 0.5);
            let geo = [p["geo"][0].as_f64().unwrap_or(0.0), p["geo"][1].as_f64().unwrap_or(0.0)];
            // Lensfun 0.3.4 sizes the frame as (w - 1) × (h - 1): a 0.2 % scale, taken out here.
            let k = (f64::from(w - 1)).hypot(f64::from(h - 1)) / f64::from(w).hypot(f64::from(h));
            let (cx, cy) = (f64::from(w - 1) / 2.0, f64::from(h - 1) / 2.0);
            let (gx, gy) = (cx + (geo[0] - cx) / k, cy + (geo[1] - cy) / k);
            g = g.max((pts[1].0 - 0.5 - gx).abs().max((pts[1].1 - 0.5 - gy).abs()));
            let (tpts, gain) = lens_correction_at(&lc_tca, w, h, x + 0.5, y + 0.5);
            if verbose {
                println!("  {x},{y}: ours geo ({:.3}, {:.3}) ref ({:.3}, {:.3}); gain {gain:.4} ref {:.4}", pts[1].0 - 0.5, pts[1].1 - 0.5, geo[0], geo[1], p["vig"][1].as_f64().unwrap_or(0.0));
            }
            for (c, pt) in tpts.iter().enumerate() {
                let sx = p["sub"][c][0].as_f64().unwrap_or(0.0);
                let sy = p["sub"][c][1].as_f64().unwrap_or(0.0);
                s = s.max((pt.0 - 0.5 - sx).abs().max((pt.1 - 0.5 - sy).abs()));
            }
            let vig = p["vig"][1].as_f64().unwrap_or(1.0);
            v = v.max((f64::from(gain) - vig).abs() / vig);
        }
        println!(
            "{name:45} f{:<5} f/{:<4}  geometry {g:.3} px  per channel {s:.3} px  vignetting {:.2} %",
            f("focal"),
            f("aperture"),
            v * 100.0
        );
        (worst_geo, worst_sub, worst_vig) = (worst_geo.max(g), worst_sub.max(s), worst_vig.max(v));
    }
    println!("worst: geometry {worst_geo:.3} px, per channel {worst_sub:.3} px, vignetting {:.2} %", worst_vig * 100.0);
    Ok(())
}
