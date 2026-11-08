//! Show what this app reads from an XMP sidecar, and how an update would look:
//! `cargo run -p xmp --example xmpdump -- <file.xmp> [rating]`

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: xmpdump <file.xmp> [rating]");
    let xml = std::fs::read_to_string(&path).expect("read file");
    let mut doc = xmp::XmpDoc::parse(&xml).expect("parse XMP");
    println!("{:#?}", doc.data());
    if let Some(rating) = args.next() {
        let rating = rating.parse().expect("rating is a number");
        doc.apply(&xmp::XmpData {
            rating: Some(rating),
            keywords: Some(vec![vec!["Example".into(), "Keyword".into()]]),
            ..Default::default()
        });
        println!("--- updated (not written) ---\n{}", doc.to_xml());
    }
}
