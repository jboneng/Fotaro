use image::{ImageBuffer, Rgb, RgbImage};

use super::*;

/// A rendered (non-RAW) image from 8-bit sRGB pixels, as `WorkingImage::load` makes it.
fn rendered(img: &RgbImage) -> WorkingImage {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("in.png");
    img.save(&path).unwrap();
    WorkingImage::load(&path, crate::SourceKind::Image, None).unwrap().0
}

fn gradient() -> RgbImage {
    ImageBuffer::from_fn(64, 16, |x, y| Rgb([(x * 4) as u8, (y * 16) as u8, (255 - x * 4) as u8]))
}

#[test]
fn defaults_round_trip_rendered_images() {
    let src = gradient();
    let out = render_srgb8(&rendered(&src), &DevelopSettings::default());
    for (a, b) in src.pixels().zip(out.pixels()) {
        for c in 0..3 {
            assert!((i16::from(a[c]) - i16::from(b[c])).abs() <= 2, "{a:?} -> {b:?}");
        }
    }
    assert!(DevelopSettings::default().is_default());
}

fn mean(img: &RgbImage) -> f32 {
    img.as_raw().iter().map(|v| f32::from(*v)).sum::<f32>() / img.as_raw().len() as f32
}

#[test]
fn exposure_doubles_linear_light() {
    let src = RgbImage::from_pixel(4, 4, Rgb([60, 60, 60]));
    let img = rendered(&src);
    let s = DevelopSettings { exposure: 1.0, ..Default::default() };
    let out = render_srgb8(&img, &s).get_pixel(0, 0).0[0];
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let ratio = lin(out) / lin(60);
    assert!((1.85..2.15).contains(&ratio), "ratio {ratio}");
}

#[test]
fn sliders_move_the_right_way() {
    let img = rendered(&gradient());
    let base = render_srgb8(&img, &DevelopSettings::default());
    let with = |f: fn(&mut DevelopSettings)| {
        let mut s = DevelopSettings::default();
        f(&mut s);
        render_srgb8(&img, &s)
    };
    assert!(mean(&with(|s| s.exposure = -1.0)) < mean(&base));
    assert!(mean(&with(|s| s.whites = 60.0)) > mean(&base));
    assert!(mean(&with(|s| s.blacks = -60.0)) < mean(&base));
    // Contrast spreads values away from the middle.
    let spread = |i: &RgbImage| {
        let m = mean(i);
        i.as_raw().iter().map(|v| (f32::from(*v) - m).abs()).sum::<f32>()
    };
    assert!(spread(&with(|s| s.contrast = 60.0)) > spread(&base));
    assert!(spread(&with(|s| s.contrast = -60.0)) < spread(&base));
    // Saturation: more distance between channels.
    let chroma = |i: &RgbImage| i.pixels().map(|p| f32::from(p[0].max(p[2]) - p[0].min(p[2]))).sum::<f32>();
    assert!(chroma(&with(|s| s.saturation = 50.0)) > chroma(&base));
    assert!(chroma(&with(|s| s.vibrance = -50.0)) < chroma(&base));
}

#[test]
fn black_and_white_is_neutral() {
    let img = rendered(&gradient());
    let s = DevelopSettings { treatment: Treatment::BlackAndWhite, ..Default::default() };
    for p in render_srgb8(&img, &s).pixels() {
        assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
    }
}

#[test]
fn relative_temp_warms_and_auto_neutralizes() {
    let img = rendered(&RgbImage::from_pixel(8, 8, Rgb([150, 120, 90])));
    let warm =
        render_srgb8(&img, &DevelopSettings { wb: WbPreset::Custom, temp: 50.0, ..Default::default() });
    let p = warm.get_pixel(0, 0);
    assert!(p[0] > 150 && p[2] < 90, "{p:?}");
    let auto = render_srgb8(&img, &DevelopSettings { wb: WbPreset::Auto, ..Default::default() });
    let p = auto.get_pixel(0, 0);
    assert!(p[0].abs_diff(p[1]) <= 3 && p[1].abs_diff(p[2]) <= 3, "{p:?}");
}

#[test]
fn hash_and_json() {
    let a = DevelopSettings::default();
    let mut b = a.clone();
    b.clarity = 1.0;
    assert_ne!(a.stable_hash(), b.stable_hash());
    assert_eq!(DevelopSettings::from_json(&b.to_json()), Some(b.clone()));
    // Missing fields (older settings) take defaults.
    assert_eq!(DevelopSettings::from_json(r#"{"exposure":0.5}"#).unwrap().exposure, 0.5);
    assert_eq!(b.describe_change(&a, true), "Clarity +1");
    let c = DevelopSettings { wb: WbPreset::Tungsten, ..a.clone() };
    assert_eq!(c.describe_change(&a, true), "White Balance: Tungsten");
}

#[test]
fn slider_labels() {
    assert_eq!(Slider::Exposure.format(0.351, false), "+0.35");
    assert_eq!(Slider::Exposure.format(0.0, false), "0.00");
    assert_eq!(Slider::Temp.format(5500.0, true), "5500");
    assert_eq!(Slider::Shadows.format(-12.0, false), "-12");
}

/// A left-dark / right-bright image (a window in a dim room).
fn dim_room() -> RgbImage {
    ImageBuffer::from_fn(200, 100, |x, y| {
        let v = if x < 100 { 25 + ((x + y) % 3) as u8 } else { 235 - ((x + y) % 3) as u8 };
        Rgb([v, v, v])
    })
}

fn half_mean(img: &RgbImage, left: bool) -> f32 {
    let xs = if left { 10..90 } else { 110..190 };
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in 10..90 {
        for x in xs.clone() {
            sum += f32::from(img.get_pixel(x, y)[1]);
            n += 1.0;
        }
    }
    sum / n
}

#[test]
fn shadows_lift_dark_areas_and_highlights_pull_bright_ones() {
    let img = rendered(&dim_room());
    let base = render_srgb8(&img, &DevelopSettings::default());
    let lifted = render_srgb8(&img, &DevelopSettings { shadows: 80.0, ..Default::default() });
    assert!(half_mean(&lifted, true) > half_mean(&base, true) + 15.0, "shadows lift the dark side");
    assert!((half_mean(&lifted, false) - half_mean(&base, false)).abs() < 6.0, "bright side stays");
    let pulled = render_srgb8(&img, &DevelopSettings { highlights: -80.0, ..Default::default() });
    assert!(half_mean(&pulled, false) < half_mean(&base, false) - 15.0, "highlights darken the bright side");
    assert!((half_mean(&pulled, true) - half_mean(&base, true)).abs() < 6.0, "dark side stays");
}

#[test]
fn auto_tone_brightens_a_dark_photo() {
    let dark: RgbImage = ImageBuffer::from_fn(64, 64, |x, y| {
        let v = 8 + ((x * 3 + y) % 30) as u8;
        Rgb([v, v, v])
    });
    let s = auto_tone(&rendered(&dark), &DevelopSettings { vibrance: 10.0, ..Default::default() });
    assert!(s.exposure > 1.0, "{s:?}");
    assert_eq!(s.vibrance, 10.0, "other settings are kept");
}

/// Standard deviation of the green channel inside a box.
fn spread_in(img: &RgbImage, xs: std::ops::Range<u32>, ys: std::ops::Range<u32>) -> f32 {
    let vals: Vec<f32> = ys
        .flat_map(|y| xs.clone().map(move |x| (x, y)))
        .map(|(x, y)| f32::from(img.get_pixel(x, y)[1]))
        .collect();
    let m = vals.iter().sum::<f32>() / vals.len() as f32;
    (vals.iter().map(|v| (v - m).powi(2)).sum::<f32>() / vals.len() as f32).sqrt()
}

/// Mid-grey with a coarse pattern (blobs of ~20 px) and a fine one (1-2 px).
fn patterned(coarse: u8, fine: u8) -> RgbImage {
    ImageBuffer::from_fn(400, 300, |x, y| {
        let c = if (x / 20 + y / 20) % 2 == 0 { coarse } else { 0 };
        let f = if (x + y) % 2 == 0 { fine } else { 0 };
        let v = 100 + c + f;
        Rgb([v, v, v])
    })
}

#[test]
fn clarity_raises_mid_scale_contrast() {
    let img = rendered(&patterned(16, 0));
    let base = render_srgb8(&img, &DevelopSettings::default());
    let more = render_srgb8(&img, &DevelopSettings { clarity: 80.0, ..Default::default() });
    let less = render_srgb8(&img, &DevelopSettings { clarity: -80.0, ..Default::default() });
    let s = |i: &RgbImage| spread_in(i, 100..300, 80..220);
    assert!(s(&more) > s(&base) * 1.15, "{} vs {}", s(&more), s(&base));
    assert!(s(&less) < s(&base) * 0.9, "{} vs {}", s(&less), s(&base));
}

#[test]
fn texture_raises_fine_contrast() {
    let img = rendered(&patterned(0, 12));
    let base = render_srgb8(&img, &DevelopSettings::default());
    let more = render_srgb8(&img, &DevelopSettings { texture: 80.0, ..Default::default() });
    let s = |i: &RgbImage| spread_in(i, 100..300, 80..220);
    assert!(s(&more) > s(&base) * 1.15, "{} vs {}", s(&more), s(&base));
}

#[test]
fn dehaze_adds_contrast_to_a_hazy_photo_and_negative_adds_haze() {
    // Low-contrast, bright, bluish: a hazy landscape.
    let hazy: RgbImage = ImageBuffer::from_fn(300, 200, |x, y| {
        let v = if (x / 30 + y / 30) % 2 == 0 { 170 } else { 200 };
        Rgb([v - 10, v, v + 20])
    });
    let img = rendered(&hazy);
    let base = render_srgb8(&img, &DevelopSettings::default());
    let clear = render_srgb8(&img, &DevelopSettings { dehaze: 70.0, ..Default::default() });
    let foggy = render_srgb8(&img, &DevelopSettings { dehaze: -70.0, ..Default::default() });
    let s = |i: &RgbImage| spread_in(i, 0..300, 0..200);
    assert!(s(&clear) > s(&base) * 1.2, "{} vs {}", s(&clear), s(&base));
    assert!(s(&foggy) < s(&base) * 0.8, "{} vs {}", s(&foggy), s(&base));
}

#[test]
fn a_region_renders_like_that_part_of_the_whole() {
    let img = rendered(&patterned(16, 12));
    let s =
        DevelopSettings { clarity: 60.0, texture: 50.0, shadows: 40.0, dehaze: 30.0, ..Default::default() };
    let whole = render_srgb8(&img, &s);
    let part = render_region_srgb8(&img, &s, Region { x: 37, y: 51, w: 120, h: 90 });
    assert_eq!((part.width(), part.height()), (120, 90));
    for y in 0..90 {
        for x in 0..120 {
            let (a, b) = (part.get_pixel(x, y), whole.get_pixel(x + 37, y + 51));
            for c in 0..3 {
                assert!(a[c].abs_diff(b[c]) <= 1, "({x}, {y}): {a:?} vs {b:?}");
            }
        }
    }
    // Regions past the edge are clipped to the image.
    let edge = render_region_srgb8(&img, &s, Region { x: 380, y: 290, w: 100, h: 100 });
    assert_eq!((edge.width(), edge.height()), (20, 10));
}
