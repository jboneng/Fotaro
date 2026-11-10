//! Finding and grouping: filters, metadata facets, collections, Quick Collection,
//! stacks and RAW+JPEG pairing, end to end against a real catalog.

use std::fs;
use std::path::PathBuf;

use crate::query::{Criterion, MetaColumn, NumOp, SmartRules, TextField, TextOp};
use crate::{Catalog, CollectionKind, Flag, IptcField, PhotoMetadata, Source};

struct Fixture {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    cat: Catalog,
}

fn fixture(files: &[&str]) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("photos");
    fs::create_dir(&dir).unwrap();
    for f in files {
        fs::write(dir.join(f), b"x").unwrap();
    }
    let mut cat = Catalog::create(tmp.path().join("c.plcat")).unwrap();
    cat.sync_folder(&dir).unwrap();
    Fixture { _tmp: tmp, dir, cat }
}

impl Fixture {
    fn id(&self, name: &str) -> i64 {
        self.cat.photos(Source::AllPhotographs).unwrap().iter().find(|p| p.file_name == name).unwrap().id
    }

    fn names(&self, source: Source, filter: Criterion) -> Vec<String> {
        let mut names: Vec<String> = self
            .cat
            .photos_filtered(source, Default::default(), &filter)
            .unwrap()
            .into_iter()
            .map(|p| p.file_name)
            .collect();
        names.sort();
        names
    }
}

fn text(field: TextField, op: TextOp, value: &str) -> Criterion {
    Criterion::Text { field, op, value: value.into() }
}

#[test]
fn text_filters_fold_case_for_all_scripts() {
    let mut fx = fixture(&["a.jpg", "b.jpg", "c.jpg"]);
    let (a, b) = (fx.id("a.jpg"), fx.id("b.jpg"));
    fx.cat.set_iptc(&[a], IptcField::Title, "Ålesund harbour").unwrap();
    fx.cat.set_iptc(&[b], IptcField::Caption, "Trip to ÅLESUND in the rain").unwrap();
    let kw = fx.cat.ensure_keyword(&["Places".into(), "Hegra".into()]).unwrap();
    fx.cat.add_keyword(&[b], kw).unwrap();

    let all = Source::AllPhotographs;
    assert_eq!(fx.names(all, text(TextField::Any, TextOp::ContainsAll, "ålesund")), ["a.jpg", "b.jpg"]);
    assert_eq!(fx.names(all, text(TextField::Any, TextOp::ContainsAll, "ålesund rain")), ["b.jpg"]);
    assert_eq!(fx.names(all, text(TextField::Any, TextOp::Contains, "harbour hegra")), ["a.jpg", "b.jpg"]);
    assert_eq!(fx.names(all, text(TextField::Title, TextOp::StartsWith, "ÅLE")), ["a.jpg"]);
    assert_eq!(fx.names(all, text(TextField::FileName, TextOp::EndsWith, "C.JPG")), ["c.jpg"]);
    assert_eq!(fx.names(all, text(TextField::Keywords, TextOp::DoesNotContain, "hegra")), ["a.jpg", "c.jpg"]);
    // Odd characters are data, not SQL or LIKE patterns.
    assert!(fx.names(all, text(TextField::Any, TextOp::Contains, "%' OR 1=1 --")).is_empty());
    assert_eq!(fx.names(all, text(TextField::Any, TextOp::ContainsAll, "  ")).len(), 3, "empty = no filter");
}

#[test]
fn attribute_and_metadata_filters_combine() {
    let fx = fixture(&["a.NEF", "b.jpg", "c.jpg", "d.jpg"]);
    let (a, b, c) = (fx.id("a.NEF"), fx.id("b.jpg"), fx.id("c.jpg"));
    fx.cat.set_flag(a, Flag::Picked).unwrap();
    fx.cat.set_flag(b, Flag::Rejected).unwrap();
    fx.cat.set_rating(a, 4).unwrap();
    fx.cat.set_rating(c, 2).unwrap();
    fx.cat.set_color_label(c, Some("red")).unwrap();
    let meta = |model: &str, t: i64| PhotoMetadata {
        make: Some("Nikon".into()),
        model: Some(model.into()),
        capture_time: Some(t),
        ..Default::default()
    };
    fx.cat.set_metadata(a, &meta("D750", 1_704_067_200)).unwrap(); // 2024
    fx.cat.set_metadata(c, &meta("FM2", 1_735_689_600)).unwrap(); // 2025

    let all = Source::AllPhotographs;
    // Flagged OR unflagged (two flag buttons on), and rating ≥ 2.
    let filter = Criterion::AllOf(vec![
        Criterion::AnyOf(vec![Criterion::Flag(Flag::Picked), Criterion::Flag(Flag::Unflagged)]),
        Criterion::Rating { op: NumOp::AtLeast, stars: 2 },
    ]);
    assert_eq!(fx.names(all, filter), ["a.NEF", "c.jpg"]);
    assert_eq!(fx.names(all, Criterion::Label(None)).len(), 3);
    assert_eq!(fx.names(all, Criterion::Kind(crate::FileKind::Raw)), ["a.NEF"]);

    let years = fx.cat.facets(all, &Criterion::AllOf(vec![]), MetaColumn::Date).unwrap();
    let labels: Vec<(&str, i64)> = years.iter().map(|f| (f.label.as_str(), f.count)).collect();
    assert_eq!(labels, [("2025", 1), ("2024", 1), ("Unknown Date", 2)]);
    let cameras = fx.cat.facets(all, &Criterion::AllOf(vec![]), MetaColumn::Camera).unwrap();
    assert_eq!(cameras[0].label, "Nikon D750");
    assert_eq!(cameras.last().unwrap().label, "Unknown Camera");
    // Picking a facet value filters to it; facets of later columns follow the filter.
    assert_eq!(fx.names(all, years[0].criterion.clone()), ["c.jpg"]);
    let types = fx.cat.facets(all, &cameras[0].criterion, MetaColumn::FileType).unwrap();
    assert_eq!(types.iter().map(|f| (f.label.as_str(), f.count)).collect::<Vec<_>>(), [("NEF", 1)]);
    let labels = fx.cat.facets(all, &Criterion::AllOf(vec![]), MetaColumn::Label).unwrap();
    assert_eq!(labels[0].label, "Red");
}

#[test]
fn collections_sets_and_smart_collections() {
    let mut fx = fixture(&["a.jpg", "b.jpg", "c.jpg"]);
    let (a, b, c) = (fx.id("a.jpg"), fx.id("b.jpg"), fx.id("c.jpg"));
    let set = fx.cat.create_collection(None, "Trips", CollectionKind::Set, None).unwrap();
    let hegra = fx.cat.create_collection(Some(set), "Hegra", CollectionKind::Collection, None).unwrap();
    let oslo = fx.cat.create_collection(Some(set), "Oslo", CollectionKind::Collection, None).unwrap();
    assert!(fx.cat.create_collection(Some(set), "hegra", CollectionKind::Collection, None).is_err());
    assert!(fx.cat.create_collection(Some(hegra), "Inside", CollectionKind::Collection, None).is_err());
    assert_eq!(fx.cat.add_to_collection(hegra, &[a, b]).unwrap(), 2);
    assert_eq!(fx.cat.add_to_collection(hegra, &[a]).unwrap(), 0, "already there");
    fx.cat.add_to_collection(oslo, &[c]).unwrap();
    assert_eq!(fx.names(Source::Collection { id: hegra }, Criterion::AllOf(vec![])), ["a.jpg", "b.jpg"]);
    assert_eq!(
        fx.names(Source::Collection { id: set }, Criterion::AllOf(vec![])).len(),
        3,
        "a set shows all"
    );
    fx.cat.remove_from_collection(hegra, &[b]).unwrap();

    fx.cat.set_rating(b, 5).unwrap();
    let rules =
        SmartRules { match_all: true, rules: vec![Criterion::Rating { op: NumOp::AtLeast, stars: 4 }] };
    let smart = fx.cat.create_collection(None, "Five stars", CollectionKind::Smart, Some(&rules)).unwrap();
    assert!(fx.cat.add_to_collection(smart, &[a]).is_err(), "smart collections pick their own photos");
    assert_eq!(fx.names(Source::Collection { id: smart }, Criterion::AllOf(vec![])), ["b.jpg"]);
    // Smart collections follow edits immediately.
    fx.cat.set_rating(c, 4).unwrap();
    let counts: Vec<(String, i64)> =
        fx.cat.collections().unwrap().into_iter().map(|c| (c.name, c.count)).collect();
    assert!(
        counts.contains(&("Five stars".into(), 2)) && counts.contains(&("Hegra".into(), 1)),
        "{counts:?}"
    );

    fx.cat.rename_collection(hegra, "Hegra fort").unwrap();
    fx.cat.delete_collection(set).unwrap();
    assert_eq!(fx.cat.collections().unwrap().len(), 1, "deleting a set deletes what is in it");
    assert_eq!(fx.cat.photos(Source::AllPhotographs).unwrap().len(), 3, "photos are untouched");
}

#[test]
fn quick_collection_toggles_like_b() {
    let mut fx = fixture(&["a.jpg", "b.jpg"]);
    let (a, b) = (fx.id("a.jpg"), fx.id("b.jpg"));
    assert!(fx.cat.toggle_quick_collection(&[a]).unwrap());
    // Mixed selection: B adds the rest rather than removing.
    assert!(fx.cat.toggle_quick_collection(&[a, b]).unwrap());
    assert_eq!(fx.cat.quick_collection_count().unwrap(), 2);
    assert!(fx.cat.photo(a).unwrap().unwrap().in_quick_collection);
    assert!(!fx.cat.toggle_quick_collection(&[a, b]).unwrap());
    assert_eq!(fx.names(Source::QuickCollection, Criterion::AllOf(vec![])).len(), 0);
}

#[test]
fn raw_and_jpeg_pairs_become_one_picture() {
    let mut fx = fixture(&["IMG_1.NEF", "IMG_1.JPG", "IMG_2.JPG"]);
    let photos = fx.cat.photos(Source::AllPhotographs).unwrap();
    let names: Vec<&str> = photos.iter().map(|p| p.file_name.as_str()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    let raw = photos.iter().find(|p| p.file_name == "IMG_1.NEF").unwrap();
    assert_eq!(raw.unit.members, 2, "the JPEG is a member of the raw's picture");

    // A JPEG added later still pairs; a changed paired JPEG re-syncs without errors.
    fs::write(fx.dir.join("IMG_3.NEF"), b"x").unwrap();
    fs::write(fx.dir.join("IMG_3.jpeg"), b"x").unwrap();
    fs::write(fx.dir.join("IMG_1.JPG"), b"changed").unwrap();
    let r = fx.cat.sync_folder(&fx.dir).unwrap();
    assert_eq!((r.added, r.paired, r.errors), (2, 1, 0));
    assert_eq!(fx.cat.photos(Source::AllPhotographs).unwrap().len(), 3);
    assert_eq!(fx.cat.folders().unwrap()[0].photo_count, 5, "every file is a photo");

    // With pairing turned off, new JPEGs are photos of their own.
    fx.cat.set_separate_raw_jpeg(true).unwrap();
    fs::write(fx.dir.join("IMG_4.NEF"), b"x").unwrap();
    fs::write(fx.dir.join("IMG_4.JPG"), b"x").unwrap();
    let r = fx.cat.sync_folder(&fx.dir).unwrap();
    assert_eq!((r.added, r.paired), (2, 0));
}
