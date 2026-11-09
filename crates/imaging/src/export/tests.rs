use super::*;

fn opts(mode: ResizeMode) -> ImageOptions {
    ImageOptions { resize: true, resize_mode: mode, ..Default::default() }
}

#[test]
fn target_sizes_follow_lrc_resize_modes() {
    let mut o = opts(ResizeMode::LongEdge);
    o.edge = 1000;
    assert_eq!(o.target_size(6000, 4000), (1000, 667));
    assert_eq!(o.target_size(4000, 6000), (667, 1000));
    assert_eq!(o.target_size(800, 600), (800, 600), "don't enlarge");
    o.dont_enlarge = false;
    assert_eq!(o.target_size(800, 600), (1000, 750));

    let mut o = opts(ResizeMode::ShortEdge);
    o.edge = 1000;
    assert_eq!(o.target_size(6000, 4000), (1500, 1000));

    let mut o = opts(ResizeMode::WidthHeight);
    (o.width, o.height) = (1000, 1000);
    assert_eq!(o.target_size(4000, 2000), (1000, 500));
    (o.width, o.height) = (0, 500); // no width limit
    assert_eq!(o.target_size(4000, 2000), (1000, 500));

    let mut o = opts(ResizeMode::Dimensions);
    (o.width, o.height) = (1200, 800);
    assert_eq!(o.target_size(6000, 4000), (1200, 800));
    assert_eq!(o.target_size(4000, 6000), (800, 1200), "either orientation");
    let mut o = opts(ResizeMode::WidthHeight);
    (o.width, o.height) = (1200, 800);
    assert_eq!(o.target_size(4000, 6000), (533, 800), "as given");

    let mut o = opts(ResizeMode::Megapixels);
    o.megapixels = 6.0;
    let (w, h) = o.target_size(6000, 4000);
    assert!((w * h).abs_diff(6_000_000) < 10_000, "{w}x{h}");

    let mut o = opts(ResizeMode::Percentage);
    o.percent = 25;
    assert_eq!(o.target_size(6000, 4000), (1500, 1000));

    assert_eq!(ImageOptions::default().target_size(6000, 4000), (6000, 4000), "resize off");
}

fn sample() -> RgbImage {
    RgbImage::from_fn(64, 48, |x, y| image::Rgb([(x * 4) as u8, (y * 5) as u8, 128]))
}

fn meta() -> EmbeddedMetadata {
    EmbeddedMetadata {
        camera: Some(Metadata {
            make: Some("NIKON CORPORATION".into()),
            model: Some("NIKON D750".into()),
            lens: Some("50mm f/1.8".into()),
            iso: Some(400.0),
            shutter: Some(1.0 / 250.0),
            aperture: Some(5.6),
            focal_len: Some(50.0),
            capture_time: Some(1_700_000_000),
            ..Default::default()
        }),
        artist: Some("Jo".into()),
        copyright: Some("(c) Jo".into()),
        description: None,
        xmp: Some("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><marker/></x:xmpmeta>".into()),
    }
}

fn exif_of(bytes: &[u8]) -> exif::Exif {
    exif::Reader::new().read_from_container(&mut Cursor::new(bytes)).expect("readable EXIF")
}

fn check_exif(e: &exif::Exif) {
    let s = |t| e.get_field(t, In::PRIMARY).map(|f| f.display_value().to_string());
    assert_eq!(s(Tag::Model).as_deref(), Some("\"NIKON D750\""));
    assert_eq!(s(Tag::Copyright).as_deref(), Some("\"(c) Jo\""));
    assert_eq!(s(Tag::ExposureTime).as_deref(), Some("1/250"));
    assert_eq!(s(Tag::FNumber).as_deref(), Some("5.6"));
    assert_eq!(s(Tag::PhotographicSensitivity).as_deref(), Some("400"));
    assert_eq!(s(Tag::DateTimeOriginal).as_deref(), Some("2023-11-14 22:13:20"));
    assert_eq!(e.get_field(Tag::Orientation, In::PRIMARY).unwrap().value.get_uint(0), Some(1));
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn jpeg_has_pixels_icc_exif_and_xmp() {
    let o = ImageOptions { jpeg_quality: 90, ..Default::default() };
    let bytes = encode(&sample(), &o, &meta()).unwrap();
    let img = image::load_from_memory(&bytes).unwrap();
    assert_eq!((img.width(), img.height()), (64, 48));
    check_exif(&exif_of(&bytes));
    assert!(contains(&bytes, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta"));
    assert!(contains(&bytes, b"ICC_PROFILE\0"));
}

#[test]
fn tiff_round_trips_compressed_and_not() {
    for zip in [true, false] {
        let o = ImageOptions { format: FileFormat::Tiff, tiff_zip: zip, ..Default::default() };
        let bytes = encode(&sample(), &o, &meta()).unwrap();
        let img = image::load_from_memory(&bytes).unwrap().into_rgb8();
        assert_eq!(img, sample(), "lossless (zip: {zip})");
        let e = exif_of(&bytes);
        check_exif(&e);
        assert!(e.get_field(Tag(exif::Context::Tiff, 34675), In::PRIMARY).is_some(), "ICC");
        assert!(e.get_field(Tag(exif::Context::Tiff, 700), In::PRIMARY).is_some(), "XMP");
    }
}

#[test]
fn png_round_trips_with_metadata() {
    let o = ImageOptions { format: FileFormat::Png, ..Default::default() };
    let bytes = encode(&sample(), &o, &meta()).unwrap();
    let img = image::load_from_memory(&bytes).unwrap().into_rgb8();
    assert_eq!(img, sample());
    let reader = png::Decoder::new(Cursor::new(&bytes)).read_info().unwrap();
    let info = reader.info();
    assert!(info.icc_profile.is_some());
    check_exif(&exif_of(&bytes));
    assert!(info.utf8_text.iter().any(|t| t.keyword == "XML:com.adobe.xmp"));
}

#[test]
fn rendering_applies_rotation_and_size() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("a.png");
    RgbImage::from_pixel(400, 200, image::Rgb([200, 10, 10])).save(&src).unwrap();
    let mut o = opts(ResizeMode::LongEdge);
    o.edge = 100;
    let (img, _) = render(&src, SourceKind::Image, 1, &o).unwrap();
    assert_eq!(img.dimensions(), (50, 100), "turned upright, then sized");

    let out = tmp.path().join("a.jpg");
    write_atomically(&out, &encode(&img, &o, &EmbeddedMetadata::default()).unwrap()).unwrap();
    write_atomically(&out, b"again").unwrap(); // overwrite
    assert_eq!(std::fs::read(&out).unwrap(), b"again");
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 2, "no .part left behind");
}
