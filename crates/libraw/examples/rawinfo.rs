//! Spike tool: `cargo run -p libraw --example rawinfo --release -- <files...>`
//! Prints metadata, saves the embedded preview, and times a half-size render.

use std::time::Instant;

fn main() {
    println!("LibRaw {}", libraw::version());
    let out_dir = std::env::temp_dir().join("fotaro-rawinfo");
    std::fs::create_dir_all(&out_dir).unwrap();

    for path in std::env::args().skip(1) {
        println!("\n== {path}");
        let t = Instant::now();
        let mut raw = match libraw::RawFile::open(&path) {
            Ok(r) => r,
            Err(e) => {
                println!("   open FAILED: {e}");
                continue;
            }
        };
        let i = raw.info();
        println!("   open {:?}  gps {:?}", t.elapsed(), i.gps);
        println!(
            "   {} {} | {} | ISO {} 1/{:.0}s f/{:.1} {}mm | {}x{} {:?} | ts {:?}",
            i.make,
            i.model,
            i.lens,
            i.iso,
            1.0 / i.shutter.max(f32::EPSILON),
            i.aperture,
            i.focal_len,
            i.width,
            i.height,
            i.orientation,
            i.timestamp
        );

        let t = Instant::now();
        let stem = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().into_owned();
        match raw.thumbnail() {
            Ok(libraw::Thumbnail::Jpeg(bytes)) => {
                let p = out_dir.join(format!("{stem}.thumb.jpg"));
                std::fs::write(&p, &bytes).unwrap();
                println!(
                    "   thumb JPEG {}x{} {} KB in {:?} -> {}",
                    i.thumb_width,
                    i.thumb_height,
                    bytes.len() / 1024,
                    t.elapsed(),
                    p.display()
                );
            }
            Ok(libraw::Thumbnail::JpegXl(bytes)) => {
                println!("   thumb JPEG XL {} KB in {:?}", bytes.len() / 1024, t.elapsed())
            }
            Ok(libraw::Thumbnail::Bitmap(b)) => println!(
                "   thumb bitmap {}x{}x{} @{}bit in {:?}",
                b.width,
                b.height,
                b.colors,
                b.bits,
                t.elapsed()
            ),
            Err(e) => println!("   thumb FAILED: {e}"),
        }

        let t = Instant::now();
        match raw.render(true) {
            Ok(b) => {
                println!("   half-size render {}x{}x{} in {:?}", b.width, b.height, b.colors, t.elapsed())
            }
            Err(e) => println!("   render FAILED: {e}"),
        }

        let c = raw.color();
        println!("   colour: {} channels, as shot {:?}, daylight {:?}", c.colors, c.as_shot, c.daylight);
        println!("   rgb_cam {:?}\n   cam_xyz {:?}", c.rgb_cam, c.cam_xyz);
        for half in [true, false] {
            let t = Instant::now();
            match raw.render_camera_linear(half) {
                Ok(img) => {
                    let max = img.data.iter().flatten().fold(0f32, |a, v| a.max(*v));
                    let mean: [f32; 3] = std::array::from_fn(|c| {
                        img.data.iter().map(|p| p[c]).sum::<f32>() / img.data.len().max(1) as f32
                    });
                    println!(
                        "   camera render (half {half}) {}x{} in {:?}, max {max:.3}, mean {mean:.3?}",
                        img.width,
                        img.height,
                        t.elapsed()
                    );
                }
                Err(e) => println!("   camera render FAILED: {e}"),
            }
        }
    }
}
