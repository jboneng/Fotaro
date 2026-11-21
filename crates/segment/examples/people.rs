//! Find the people in a picture (Distractions → People):
//! `cargo run -p segment --example people --release -- <image> <out overlay.png> [threshold]`
//!
//! Prints each person found (where, how sure) and the time, and saves the picture a
//! quarter size with their outlines in red. Downloads the models if they are missing.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [image, out, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: people <image> <out overlay.png> [threshold]");
    };
    let threshold: f32 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(0.35);
    let img = image::open(image)?.to_rgb8();
    let models = [segment::AiModel::People, segment::AiModel::Object];
    if !segment::ready(&models) {
        segment::ensure(&models, &mut |share, what| eprint!("\r{what} {:.0}%   ", share * 100.0))?;
        eprintln!();
    }
    let started = std::time::Instant::now();
    let people = segment::find_people(&img, threshold, true, &|share, what| {
        eprint!("\r{what} {:.0}%   ", share * 100.0)
    })?;
    eprintln!();
    println!("{} people in {:.1?}", people.len(), started.elapsed());
    let mut small = image::imageops::resize(
        &img,
        img.width() / 4,
        img.height() / 4,
        image::imageops::FilterType::Triangle,
    );
    for p in &people {
        println!(
            "  {:.2}  {},{} {}x{}",
            p.score,
            p.rect[0],
            p.rect[1],
            p.rect[2] - p.rect[0],
            p.rect[3] - p.rect[1]
        );
        for (x, y, v) in p.mask.enumerate_pixels() {
            if v[0] > 0 {
                let (sx, sy) = ((p.rect[0] + x) / 4, (p.rect[1] + y) / 4);
                if sx < small.width() && sy < small.height() {
                    let q = small.get_pixel_mut(sx, sy);
                    *q = image::Rgb(if p.score >= 0.5 { [255, 40, 40] } else { [255, 200, 40] });
                }
            }
        }
    }
    small.save(out)?;
    Ok(())
}
