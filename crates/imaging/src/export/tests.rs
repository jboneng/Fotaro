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

fn sample_rgb() -> RgbImage {
    RgbImage::from_fn(64, 48, |x, y| image::Rgb([(x * 4) as u8, (y * 5) as u8, 128]))
}

fn sample() -> DynamicImage {
    DynamicImage::ImageRgb8(sample_rgb())
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
        gps: None,
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
        assert_eq!(img, sample_rgb(), "lossless (zip: {zip})");
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
    assert_eq!(img, sample_rgb());
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
    let (img, _) = render(&src, SourceKind::Image, 1, &o, None).unwrap();
    assert_eq!((img.width(), img.height()), (50, 100), "turned upright, then sized");

    let out = tmp.path().join("a.jpg");
    write_atomically(&out, &encode(&img, &o, &EmbeddedMetadata::default()).unwrap()).unwrap();
    write_atomically(&out, b"again").unwrap(); // overwrite
    assert_eq!(std::fs::read(&out).unwrap(), b"again");
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 2, "no .part left behind");
}

fn sample16() -> DynamicImage {
    let img: image::ImageBuffer<image::Rgb<u16>, Vec<u16>> = image::ImageBuffer::from_fn(64, 48, |x, y| {
        image::Rgb([(x * 1000) as u16, (y * 1300) as u16, 40_001])
    });
    DynamicImage::ImageRgb16(img)
}

#[test]
fn sixteen_bit_tiff_and_png_round_trip() {
    for (format, zip) in [(FileFormat::Tiff, true), (FileFormat::Tiff, false), (FileFormat::Png, false)] {
        let o = ImageOptions { format, tiff_zip: zip, sixteen_bit: true, ..Default::default() };
        assert!(o.sixteen());
        let bytes = encode(&sample16(), &o, &meta()).unwrap();
        let back = image::load_from_memory(&bytes).unwrap();
        assert_eq!(back.color(), image::ColorType::Rgb16, "{format:?}");
        assert_eq!(back.into_rgb16(), sample16().into_rgb16(), "lossless ({format:?}, zip {zip})");
    }
    let jpeg = ImageOptions { sixteen_bit: true, ..Default::default() };
    assert!(!jpeg.sixteen(), "JPEG is always 8-bit");
}

#[test]
fn rendering_keeps_16_bit_sources_and_converts_colour_spaces() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("scan.tif");
    let scan: image::ImageBuffer<image::Rgb<u16>, Vec<u16>> =
        image::ImageBuffer::from_pixel(40, 20, image::Rgb([60_000, 3_000, 3_000]));
    scan.save(&src).unwrap();

    let mut o = ImageOptions { format: FileFormat::Tiff, sixteen_bit: true, ..Default::default() };
    let (img, _) = render(&src, SourceKind::Image, 0, &o, None).unwrap();
    let DynamicImage::ImageRgb16(px) = &img else { panic!("expected 16 bits, got {:?}", img.color()) };
    assert_eq!(px.get_pixel(0, 0).0, [60_000, 3_000, 3_000], "untagged sRGB stays as is");

    o.color_space = ColorSpace::ProPhotoRgb;
    let (img, _) = render(&src, SourceKind::Image, 0, &o, None).unwrap();
    let p = img.as_rgb16().unwrap().get_pixel(0, 0).0;
    assert!(p[0] < 60_000 && p[1] > 3_000, "a saturated sRGB red is less saturated in ProPhoto: {p:?}");

    o.sixteen_bit = false;
    o.color_space = ColorSpace::AdobeRgb;
    let (img, _) = render(&src, SourceKind::Image, 0, &o, None).unwrap();
    assert!(matches!(img, DynamicImage::ImageRgb8(_)));
    let bytes = encode(&img, &o, &EmbeddedMetadata::default()).unwrap();
    let icc = lcms2::Profile::new_icc(&e_tiff_icc(&bytes).expect("ICC in the TIFF")).unwrap();
    let desc = icc.info(lcms2::InfoType::Description, lcms2::Locale::none()).unwrap();
    assert!(desc.contains("Adobe RGB"), "{desc}");
    let e = exif_of(&bytes);
    assert_eq!(e.get_field(Tag::ColorSpace, In::PRIMARY).unwrap().value.get_uint(0), Some(0xFFFF));
}

fn e_tiff_icc(bytes: &[u8]) -> Option<Vec<u8>> {
    match &exif_of(bytes).get_field(Tag(exif::Context::Tiff, 34675), In::PRIMARY)?.value {
        Value::Undefined(v, _) => Some(v.clone()),
        Value::Byte(v) => Some(v.clone()),
        _ => None,
    }
}

#[test]
fn every_colour_space_has_a_valid_profile() {
    for cs in ColorSpace::ALL {
        let icc = cs.icc();
        let p = lcms2::Profile::new_icc(&icc).unwrap();
        assert_eq!(p.color_space(), lcms2::ColorSpaceSignature::RgbData, "{cs:?}");
        let desc = p.info(lcms2::InfoType::Description, lcms2::Locale::none()).unwrap_or_default();
        assert!(!desc.is_empty(), "{cs:?} has a name");
    }
}

#[test]
fn jpeg_size_limit_picks_the_best_quality_that_fits() {
    let noisy = DynamicImage::ImageRgb8(RgbImage::from_fn(400, 300, |x, y| {
        let v = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) as u8;
        image::Rgb([v, v.wrapping_mul(3), v.wrapping_add(90)])
    }));
    let free = encode(&noisy, &ImageOptions { jpeg_quality: 100, ..Default::default() }, &meta()).unwrap();
    let o = ImageOptions { limit_size: true, limit_kb: 40, ..Default::default() };
    let limited = encode(&noisy, &o, &meta()).unwrap();
    assert!(free.len() > 40 * 1024, "the test image must need limiting ({} bytes)", free.len());
    assert!(limited.len() <= 40 * 1024 && limited.len() > 20 * 1024, "{} bytes", limited.len());
    let o = ImageOptions { limit_size: true, limit_kb: 1, ..Default::default() };
    assert!(encode(&noisy, &o, &meta()).is_err(), "impossible limits are errors");
}

#[test]
fn gps_is_written_and_read_back() {
    let mut m = meta();
    m.gps = Some(Gps { latitude: 59.9139, longitude: -10.7522, altitude: Some(-12.5) });
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("g.jpg");
    std::fs::write(&path, encode(&sample(), &ImageOptions::default(), &m).unwrap()).unwrap();
    let back = crate::read_metadata(&path, SourceKind::Image).unwrap().gps.expect("GPS");
    assert!((back.latitude - 59.9139).abs() < 1e-5 && (back.longitude + 10.7522).abs() < 1e-5, "{back:?}");
    assert_eq!(back.altitude, Some(-12.5));

    std::fs::write(&path, encode(&sample(), &ImageOptions::default(), &meta()).unwrap()).unwrap();
    assert_eq!(crate::read_metadata(&path, SourceKind::Image).unwrap().gps, None);
}

#[test]
fn sharpening_raises_edge_contrast_only() {
    let edge = |x: u32| if x < 50 { 80u8 } else { 160 };
    let src = DynamicImage::ImageRgb8(RgbImage::from_fn(100, 20, |x, _| image::Rgb([edge(x); 3])));
    let mut img = src.clone();
    let (sigma, amount) = effects::sharpen_params(SharpenFor::Screen, SharpenAmount::High, 240);
    effects::sharpen(&mut img, sigma, amount);
    let (a, b) =
        (img.as_rgb8().unwrap().get_pixel(49, 10).0[0], img.as_rgb8().unwrap().get_pixel(50, 10).0[0]);
    assert!(a < 80 && b > 160, "edge got crisper: {a} | {b}");
    assert_eq!(img.as_rgb8().unwrap().get_pixel(10, 10).0, [80; 3], "flat areas are untouched");

    let mut img16 = DynamicImage::ImageRgb16(src.to_rgb16());
    effects::sharpen(&mut img16, sigma, amount);
    assert!(img16.as_rgb16().unwrap().get_pixel(49, 10).0[0] < 80 * 257);
}

#[test]
fn watermarks_draw_in_their_corner() {
    let black = || DynamicImage::ImageRgb8(RgbImage::new(400, 200));
    let mut wm = Watermark { enabled: true, opacity: 100, shadow: false, ..Default::default() };
    let lit = |img: &DynamicImage, x0: u32, x1: u32, y0: u32, y1: u32| {
        let rgb = img.as_rgb8().unwrap();
        (x0..x1)
            .flat_map(|x| (y0..y1).map(move |y| (x, y)))
            .filter(|&(x, y)| rgb.get_pixel(x, y).0[0] > 128)
            .count()
    };

    let mut img = black();
    wm.apply(&mut img, Some("© Jo Example"), ColorSpace::Srgb).unwrap();
    assert!(lit(&img, 0, 200, 100, 200) > 50, "bottom left has text");
    assert_eq!(lit(&img, 200, 400, 0, 200) + lit(&img, 0, 200, 0, 100), 0, "and nothing else");

    let mut img = black();
    wm.apply(&mut img, None, ColorSpace::Srgb).unwrap();
    assert_eq!(lit(&img, 0, 400, 0, 200), 0, "no copyright, no watermark");

    wm.style = WatermarkStyle::Text;
    wm.text = "PROOF".into();
    wm.anchor = Anchor::TopRight;
    let mut img = DynamicImage::ImageRgb16(black().to_rgb16());
    wm.apply(&mut img, None, ColorSpace::Srgb).unwrap();
    let img = DynamicImage::ImageRgb8(img.to_rgb8());
    assert!(lit(&img, 200, 400, 0, 100) > 50, "top right, 16-bit");

    let tmp = tempfile::tempdir().unwrap();
    let logo = tmp.path().join("logo.png");
    image::RgbaImage::from_pixel(10, 5, image::Rgba([255, 255, 255, 255])).save(&logo).unwrap();
    wm.style = WatermarkStyle::Graphic;
    wm.graphic = logo;
    wm.anchor = Anchor::Center;
    wm.size = 50;
    let mut img = black();
    wm.apply(&mut img, None, ColorSpace::Srgb).unwrap();
    assert_eq!(lit(&img, 0, 400, 0, 200), 200 * 100, "a 200×100 white logo in the middle");
    assert_eq!(img.as_rgb8().unwrap().get_pixel(200, 100).0, [255; 3]);
}
