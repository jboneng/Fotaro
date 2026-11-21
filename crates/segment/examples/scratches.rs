//! Run Dust & Scratches' AI model on a picture:
//! `cargo run -p segment --example scratches --release -- <image> <out map.png> [scale]`
//!
//! Saves the model's map (the chance of dust, hairs or scratches, 0..255) at the image's
//! size, and prints how long it took and on what. Downloads the model if it is missing.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [image, out, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: scratches <image> <out map.png> [scale]");
    };
    let scale: f32 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(0.5);
    let img = image::open(image)?.to_rgb32f();
    let (w, h) = (img.width() as usize, img.height() as usize);
    // The lightness Dust & Scratches looks at (as the develop module computes it).
    let gray: Vec<f32> = img
        .as_raw()
        .chunks(3)
        .map(|p| {
            let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            (0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2])).clamp(0.0, 1.0).sqrt()
        })
        .collect();
    let model = segment::AiModel::Scratches;
    if !segment::ready(&[model]) {
        segment::ensure(&[model], &mut |share, what| eprint!("\r{what} {:.0}%   ", share * 100.0))?;
        eprintln!();
    }
    let started = std::time::Instant::now();
    let (map, device) =
        segment::scratches(&gray, w, h, scale, true, &|share| eprint!("\r{:.0}%  ", share * 100.0))?;
    eprintln!();
    println!("{w}×{h} at scale {scale} on {} in {:.1?}", device.label(), started.elapsed());
    image::GrayImage::from_raw(w as u32, h as u32, map)
        .ok_or_else(|| anyhow::anyhow!("map size"))?
        .save(out)?;
    Ok(())
}
