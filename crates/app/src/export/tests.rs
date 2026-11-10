use super::*;
use catalog::Source;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    photos: PathBuf,
    cat: Catalog,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = dunce::canonicalize(tmp.path()).unwrap();
    let photos = root.join("photos");
    std::fs::create_dir(&photos).unwrap();
    image::RgbImage::from_pixel(300, 200, image::Rgb([200, 30, 30])).save(photos.join("a.png")).unwrap();
    image::RgbImage::from_pixel(120, 90, image::Rgb([30, 30, 200])).save(photos.join("b.tif")).unwrap();
    let mut cat = Catalog::create(root.join("c.plcat")).unwrap();
    cat.sync_folder(&photos).unwrap();
    Fixture { _tmp: tmp, root, photos, cat }
}

fn ids(cat: &Catalog) -> Vec<i64> {
    cat.photos(Source::AllPhotographs).unwrap().iter().map(|p| p.id).collect()
}

fn export(cat: &mut Catalog, ids: &[i64], prefs: &ExportPrefs) -> ExportReport {
    run(cat, ids, prefs, None, &|_, _| {}, &AtomicBool::new(false)).unwrap()
}

#[test]
fn jpeg_export_resizes_renames_rotates_and_embeds_metadata() {
    let mut f = fixture();
    let all = ids(&f.cat);
    let a = all[0];
    f.cat.set_rotation(a, 1).unwrap();
    f.cat.set_iptc(&[a], catalog::IptcField::Copyright, "(c) Jo").unwrap();
    let out = f.root.join("out");
    let mut prefs = ExportPrefs { folder: out.clone(), rename_on: true, ..Default::default() };
    prefs.template = "{Text}-{Seq:2}".into();
    prefs.custom_text = "trip".into();
    prefs.image.resize = true;
    prefs.image.edge = 150;

    let r = export(&mut f.cat, &all, &prefs);
    assert_eq!((r.exported.len(), r.failed.len(), r.skipped), (2, 0, 0), "{:?}", r.failed);
    assert_eq!(r.exported, vec![out.join("trip-01.jpg"), out.join("trip-02.jpg")]);
    let img = image::open(&r.exported[0]).unwrap();
    assert_eq!((img.width(), img.height()), (100, 150), "turned upright, 150 px long edge");
    let img = image::open(&r.exported[1]).unwrap();
    assert_eq!((img.width(), img.height()), (120, 90), "don't enlarge");
    let bytes = std::fs::read(&r.exported[0]).unwrap();
    assert!(bytes.windows(6).any(|w| w == b"(c) Jo"));
    assert!(r.added.is_empty(), "not added unless asked");

    // Again: new names, then skip.
    let r = export(&mut f.cat, &all[..1], &prefs);
    assert_eq!(r.exported, vec![out.join("trip-01-2.jpg")]);
    prefs.existing = ExistingFiles::Skip;
    let r = export(&mut f.cat, &all[..1], &prefs);
    assert_eq!((r.exported.len(), r.skipped), (0, 1));
}

#[test]
fn same_folder_export_is_added_and_stacked_with_its_original() {
    let mut f = fixture();
    let all = ids(&f.cat);
    let prefs = ExportPrefs {
        export_to: ExportTo::SameFolder,
        add_to_catalog: true,
        add_to_stack: true,
        image: ImageOptions { format: FileFormat::Tiff, ..Default::default() },
        ..Default::default()
    };
    let r = export(&mut f.cat, &all[..1], &prefs);
    // a.png -> a.tif is free; b.tif's own name is taken on disk.
    assert_eq!(r.exported, vec![f.photos.join("a.tif")]);
    assert_eq!(r.added.len(), 1);
    let photos = f.cat.photos(Source::AllPhotographs).unwrap();
    let order: Vec<(&str, u32)> = photos.iter().map(|p| (p.file_name.as_str(), p.unit.members)).collect();
    assert_eq!(order, [("a.png", 2), ("b.tif", 1)], "in the original's stack, which still shows it");
    let members = f.cat.unit_members(all[0]).unwrap();
    assert_eq!(members[1].photo.unit.role, catalog::Role::Export);

    let r = export(&mut f.cat, &all[1..2], &prefs);
    assert_eq!(r.exported, vec![f.photos.join("b-2.tif")]);
}

#[test]
fn original_export_copies_the_file_and_writes_a_sidecar() {
    let mut f = fixture();
    let all = ids(&f.cat);
    let b = all[1];
    f.cat.set_rating(b, 4).unwrap();
    f.cat.set_rotation(b, 3).unwrap();
    let out = f.root.join("orig");
    let mut prefs = ExportPrefs {
        folder: out.clone(),
        subfolder_on: true,
        subfolder: "sub".into(),
        ..Default::default()
    };
    prefs.image.format = FileFormat::Original;
    let r = export(&mut f.cat, &[b], &prefs);
    let dest = out.join("sub").join("b.tif");
    assert_eq!(r.exported, vec![dest.clone()]);
    assert_eq!(std::fs::read(&dest).unwrap(), std::fs::read(f.photos.join("b.tif")).unwrap());
    let xml = std::fs::read_to_string(out.join("sub").join("b.tif.xmp")).unwrap();
    assert!(xml.contains("Rating") && xml.contains('4'), "{xml}");
    assert!(xml.contains("Orientation") && xml.contains('8'), "rotation travels in the sidecar: {xml}");
}

#[test]
fn cancel_stops_before_the_next_photo() {
    let mut f = fixture();
    let all = ids(&f.cat);
    let prefs = ExportPrefs { folder: f.root.join("out"), ..Default::default() };
    let r = run(&mut f.cat, &all, &prefs, None, &|_, _| {}, &AtomicBool::new(true)).unwrap();
    assert!(r.cancelled && r.exported.is_empty());
    assert!(prefs.problem().is_none());
    assert!(ExportPrefs::default().problem().is_some(), "needs a folder");
}

#[test]
fn presets_apply_and_names_stay_unique() {
    let current = ExportPrefs { folder: PathBuf::from("out"), rename_on: true, ..Default::default() };
    let email = BUILTIN_PRESETS.iter().position(|(n, _)| *n == "For Email").unwrap();
    let p = preset_prefs(&current, &[], &PresetId::Builtin(email)).unwrap();
    assert_eq!((p.folder.clone(), p.rename_on), (PathBuf::from("out"), true), "location and naming kept");
    assert!(p.image.resize && p.image.sharpen && p.remove_location);
    assert_eq!(p.metadata, MetadataLevel::CopyrightAndContact);
    for i in 0..BUILTIN_PRESETS.len() {
        assert!(preset_prefs(&current, &[], &PresetId::Builtin(i)).unwrap().problem().is_none());
    }

    let user = vec![ExportPreset { name: "Web".into(), prefs: p.clone() }];
    assert_eq!(preset_prefs(&current, &user, &PresetId::User("Web".into())), Some(p));
    assert_eq!(preset_prefs(&current, &user, &PresetId::User("Gone".into())), None);
    assert_eq!(unique_preset_name(&user, " web "), "web 2");
    assert_eq!(unique_preset_name(&user, "Print"), "Print");
    assert_eq!(unique_preset_name(&user, "  "), "Untitled Preset");
}

#[test]
fn gps_travels_unless_location_is_removed() {
    let mut f = fixture();
    // A JPEG with a GPS position, as a phone would write it.
    let src = f.photos.join("phone.jpg");
    let meta = imaging::export::EmbeddedMetadata {
        gps: Some(imaging::Gps { latitude: 59.9, longitude: 10.75, altitude: None }),
        ..Default::default()
    };
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(40, 30, image::Rgb([9, 99, 199])));
    std::fs::write(&src, imaging::export::encode(&img, &ImageOptions::default(), &meta).unwrap()).unwrap();
    f.cat.sync_folder(&f.photos).unwrap();
    let id = f.cat.photo_ids_for_paths(std::slice::from_ref(&src)).unwrap()[0];

    let gps_of = |p: &Path| imaging::read_metadata(p, imaging::SourceKind::Image).unwrap().gps;
    let mut prefs = ExportPrefs { folder: f.root.join("out"), ..Default::default() };
    let r = export(&mut f.cat, &[id], &prefs);
    assert!(gps_of(&r.exported[0]).is_some_and(|g| (g.latitude - 59.9).abs() < 1e-6));

    prefs.remove_location = true;
    let r = export(&mut f.cat, &[id], &prefs);
    assert_eq!(gps_of(&r.exported[0]), None, "Remove Location Info");

    prefs.remove_location = false;
    prefs.metadata = MetadataLevel::CopyrightOnly;
    let r = export(&mut f.cat, &[id], &prefs);
    assert_eq!(gps_of(&r.exported[0]), None, "not part of Copyright Only");
}

#[test]
fn copyright_watermark_and_sixteen_bit_tiff_through_the_whole_export() {
    let mut f = fixture();
    let all = ids(&f.cat);
    let a = all[0]; // a.png, 300 × 200 red
    f.cat.set_iptc(&[a], catalog::IptcField::Copyright, "© Jo").unwrap();
    let mut prefs = ExportPrefs {
        folder: f.root.join("out"),
        metadata: MetadataLevel::CopyrightOnly,
        ..Default::default()
    };
    prefs.image.format = FileFormat::Png;
    prefs.image.watermark.enabled = true;
    prefs.image.watermark.opacity = 100;
    let r = export(&mut f.cat, &all, &prefs);
    assert_eq!(r.failed, vec![]);
    let marked = image::open(&r.exported[0]).unwrap().into_rgb8();
    let whiteish = |img: &image::RgbImage| img.pixels().filter(|p| p.0[1] > 200).count();
    assert!(whiteish(&marked) > 20, "copyright drawn even though only copyright metadata is exported");
    let plain = image::open(&r.exported[1]).unwrap().into_rgb8();
    assert_eq!(whiteish(&plain), 0, "b.tif has no copyright, so no watermark");

    prefs.image.watermark.enabled = false;
    prefs.image.format = FileFormat::Tiff;
    prefs.image.sixteen_bit = true;
    let r = export(&mut f.cat, &all[..1], &prefs);
    let img = image::open(&r.exported[0]).unwrap();
    assert_eq!(img.color(), image::ColorType::Rgb16, "8-bit sources still make valid 16-bit files");
}
