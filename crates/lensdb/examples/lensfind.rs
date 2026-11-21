//! Find a photo's lens in the database and show what it resolves to:
//! `cargo run -p lensdb --example lensfind -- <db dir> <camera maker> <camera model> <lens> [focal] [aperture] [distance]`

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [dir, maker, model, lens, rest @ ..] = a.as_slice() else {
        anyhow::bail!("usage: lensfind <db dir> <maker> <model> <lens> [focal] [aperture] [distance]");
    };
    let num = |i: usize| rest.get(i).and_then(|s| s.parse::<f32>().ok());
    let db = lensdb::Database::load(std::path::Path::new(dir))?;
    let camera = db.find_camera(maker, model);
    println!("camera: {:?}", camera.map(|c| (&c.maker, &c.model, &c.mount, c.crop)));
    let photo = lensdb::PhotoLens {
        camera_maker: maker.clone(),
        camera_model: model.clone(),
        lens: lens.clone(),
        focal: num(0),
    };
    let found = db.find_lenses(&photo);
    for m in found.iter().take(8) {
        let l = &db.lenses[m.index];
        println!(
            "  {:3} {} {:45} {}-{}mm f/{}",
            m.score,
            if m.sure { "sure" } else { "    " },
            l.model,
            l.min_focal,
            l.max_focal,
            l.min_aperture
        );
    }
    if let Some(m) = found.first() {
        let l = &db.lenses[m.index];
        let crop = camera.map_or(1.0, |c| c.crop);
        let focal = num(0).unwrap_or(l.min_focal);
        let p = lensdb::resolve(l, model, crop, focal, num(1).unwrap_or(8.0), num(2).unwrap_or(1000.0));
        println!("resolved: {p:#?}");
    }
    Ok(())
}
