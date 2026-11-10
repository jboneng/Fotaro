//! Summarize a catalog: `cargo run -p catalog --example catalog_info -- <catalog.plcat>`

use catalog::{Catalog, Criterion, SortOrder, Source};

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: catalog_info <catalog.plcat>");
    let t = std::time::Instant::now();
    let cat = Catalog::open(&dir)?;
    println!("opened (and migrated if needed) in {:.0?}", t.elapsed());
    println!("schema v{}, {} photos", cat.schema_version()?, cat.photo_count()?);
    println!("quick collection: {}", cat.quick_collection_count()?);
    for c in cat.collections()? {
        println!("collection {:?} {:?} (parent {:?}): {} photos", c.kind, c.name, c.parent_id, c.count);
    }
    let t = std::time::Instant::now();
    let all = cat.photos_filtered(Source::AllPhotographs, SortOrder::default(), &Criterion::AllOf(vec![]))?;
    println!("grid query in {:.0?}", t.elapsed());
    let t = std::time::Instant::now();
    let five = Criterion::Rating { op: catalog::NumOp::AtLeast, stars: 1 };
    let rated = cat.photos_filtered(Source::AllPhotographs, SortOrder::default(), &five)?;
    println!("filtered grid query ({} units) in {:.0?}", rated.len(), t.elapsed());
    println!("{} units in All Photographs (one per picture or group)", all.len());
    for p in all.iter().filter(|p| p.unit.is_stack()) {
        println!("  {} {:?}", p.file_name, p.unit);
    }
    Ok(())
}
