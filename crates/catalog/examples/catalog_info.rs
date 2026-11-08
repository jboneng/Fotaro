//! Summarize a catalog: `cargo run -p catalog --example catalog_info -- <catalog.plcat>`

use catalog::{Catalog, Criterion, SortOrder, Source};

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: catalog_info <catalog.plcat>");
    let cat = Catalog::open(&dir)?;
    println!("schema v{}, {} photos", cat.schema_version()?, cat.photo_count()?);
    println!("quick collection: {}", cat.quick_collection_count()?);
    for c in cat.collections()? {
        println!("collection {:?} {:?} (parent {:?}): {} photos", c.kind, c.name, c.parent_id, c.count);
    }
    let all = cat.photos_filtered(Source::AllPhotographs, SortOrder::default(), &Criterion::AllOf(vec![]))?;
    println!("{} shown in All Photographs (collapsed stacks hide members)", all.len());
    for p in all.iter().filter(|p| p.stack.is_some() || p.paired_ext.is_some()) {
        println!("  {} stack {:?} paired {:?}", p.file_name, p.stack, p.paired_ext);
    }
    Ok(())
}
