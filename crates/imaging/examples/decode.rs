//! Decode files the way the app builds standard previews, and save the result for inspection:
//! `cargo run -p imaging --example decode --release -- <out dir> <files...>`

use std::path::Path;
use std::time::Instant;

use imaging::{PreviewSize, SourceKind};

fn main() {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().expect("usage: decode <out dir> <files...>"));
    std::fs::create_dir_all(&out).unwrap();
    for file in args {
        let path = Path::new(&file);
        let kind = match catalog::file_kind(path) {
            Some(catalog::FileKind::Raw) => SourceKind::Raw,
            Some(catalog::FileKind::Image) => SourceKind::Image,
            None => {
                println!("{file}: not a photo");
                continue;
            }
        };
        let t = Instant::now();
        match imaging::decode(path, kind, PreviewSize::Standard.max_px()) {
            Ok((img, meta)) => {
                let name = path.file_name().unwrap().to_string_lossy();
                let dest = out.join(format!("{name}.jpg"));
                img.save(&dest).unwrap();
                println!("{file}: {}x{} in {:.2?}  {meta:?}", img.width(), img.height(), t.elapsed());
            }
            Err(e) => println!("{file}: FAILED in {:.2?}: {e:#}", t.elapsed()),
        }
    }
}
