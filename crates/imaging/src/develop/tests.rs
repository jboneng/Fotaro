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

fn grey(v: u8) -> WorkingImage {
    rendered(&RgbImage::from_pixel(4, 4, Rgb([v, v, v])))
}

fn with_curve(curve: ToneCurve) -> DevelopSettings {
    DevelopSettings { curve, ..Default::default() }
}

#[test]
fn parametric_curve_moves_its_tones() {
    let (dark, light) = (grey(40), grey(215));
    let shadows = with_curve(ToneCurve { shadows: 100.0, ..Default::default() });
    let px = |img: &WorkingImage, s: &DevelopSettings| render_srgb8(img, s).get_pixel(0, 0)[1];
    assert!(px(&dark, &shadows) > 50, "shadows +100 lifts dark tones");
    assert!(px(&light, &shadows).abs_diff(215) <= 2, "and leaves light ones");
    let highlights = with_curve(ToneCurve { highlights: -100.0, ..Default::default() });
    assert!(px(&light, &highlights) < 205);
    assert!(px(&dark, &highlights).abs_diff(40) <= 2);
}

#[test]
fn point_curves_raise_midtones_and_tint_channels() {
    let mid = grey(128);
    let lift = with_curve(ToneCurve { rgb: vec![[0.0, 0.0], [0.5, 0.65], [1.0, 1.0]], ..Default::default() });
    let p = render_srgb8(&mid, &lift).get_pixel(0, 0).0;
    assert!(p[1] > 150 && p[0] == p[1] && p[1] == p[2], "{p:?}");
    let red = with_curve(ToneCurve { red: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]], ..Default::default() });
    let p = render_srgb8(&mid, &red).get_pixel(0, 0).0;
    // Curves act on the working space's (ProPhoto) channels, so sRGB green moves a little too.
    assert!(p[0] > p[1] + 30 && p[0] > p[2] + 30 && p[1].abs_diff(128) <= 10, "a red tint: {p:?}");
}

#[test]
fn curve_settings_round_trip_and_name_their_history_steps() {
    let old = DevelopSettings::default();
    let s = with_curve(ToneCurve { darks: -20.0, ..Default::default() });
    assert_eq!(DevelopSettings::from_json(&s.to_json()), Some(s.clone()));
    assert_ne!(s.stable_hash(), old.stable_hash());
    assert_eq!(s.describe_change(&old, true), "Tone Curve: Darks -20");
    let p = with_curve(ToneCurve { rgb: POINT_PRESETS[1].1.to_vec(), ..Default::default() });
    assert_eq!(p.describe_change(&old, true), "Tone Curve: Point Curve");
    // Settings saved before the Tone Curve existed still load, with a neutral curve.
    let legacy = DevelopSettings::from_json(r#"{"exposure":0.5}"#).unwrap();
    assert!(legacy.curve.is_default());
}

/// Render one pixel of a flat patch of an sRGB colour.
fn patch(rgb: [u8; 3], s: &DevelopSettings) -> [u8; 3] {
    render_srgb8(&rendered(&RgbImage::from_pixel(4, 4, Rgb(rgb))), s).get_pixel(0, 0).0
}

fn with_hsl(f: impl FnOnce(&mut Hsl)) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    f(&mut s.hsl);
    s
}

const RED: [u8; 3] = [200, 40, 40];
const BLUE: [u8; 3] = [40, 70, 200];
const GREEN: [u8; 3] = [50, 170, 60];

#[test]
fn hsl_saturation_and_luminance_affect_only_their_colour() {
    let desat_red = with_hsl(|h| h.saturation[0] = -100.0);
    let r = patch(RED, &desat_red);
    assert!(r[0].abs_diff(r[1]) <= 4 && r[1].abs_diff(r[2]) <= 4, "red turns grey: {r:?}");
    let b = patch(BLUE, &desat_red);
    assert!(b.iter().zip(BLUE).all(|(x, y)| x.abs_diff(y) <= 2), "blue stays: {b:?}");

    let dark_blue = with_hsl(|h| h.luminance[5] = -100.0);
    let b = patch(BLUE, &dark_blue);
    let lum = |p: [u8; 3]| 0.3 * f32::from(p[0]) + 0.59 * f32::from(p[1]) + 0.11 * f32::from(p[2]);
    assert!(lum(b) < lum(BLUE) - 15.0, "blue darkens: {b:?}");
    let r = patch(RED, &dark_blue);
    assert!(r.iter().zip(RED).all(|(x, y)| x.abs_diff(y) <= 2), "red stays: {r:?}");
    // Greys have no hue: untouched.
    let g = patch([120, 120, 120], &dark_blue);
    assert!(g.iter().all(|v| v.abs_diff(120) <= 1), "{g:?}");
}

#[test]
fn hsl_hue_moves_towards_the_neighbour() {
    let r = patch(RED, &with_hsl(|h| h.hue[0] = 100.0));
    assert!(r[1] > RED[1] + 20, "red +100 turns orange: {r:?}");
    let r = patch(RED, &with_hsl(|h| h.hue[0] = -100.0));
    assert!(r[2] > RED[2] + 20, "red -100 turns magenta: {r:?}");
}

#[test]
fn bw_mix_sets_how_light_each_colour_turns() {
    let mut s = DevelopSettings { treatment: Treatment::BlackAndWhite, ..Default::default() };
    let plain_red = patch(RED, &s)[0];
    let plain_green = patch(GREEN, &s)[0];
    s.bw_mix[0] = 80.0;
    let r = patch(RED, &s);
    assert!(r[0] == r[1] && r[1] == r[2], "still grey: {r:?}");
    assert!(r[0] > plain_red + 20, "red lighter: {} vs {plain_red}", r[0]);
    assert!(patch(GREEN, &s)[0].abs_diff(plain_green) <= 2, "green unchanged");
    // In Color, the B&W Mix does nothing (and HSL does nothing in Black & White).
    let colour = DevelopSettings { bw_mix: s.bw_mix, ..Default::default() };
    assert!(patch(RED, &colour).iter().zip(RED).all(|(x, y)| x.abs_diff(y) <= 2));
}

#[test]
fn hsl_settings_round_trip_and_name_history_steps() {
    let old = DevelopSettings::default();
    let s = with_hsl(|h| h.saturation[5] = -30.0);
    assert_eq!(DevelopSettings::from_json(&s.to_json()), Some(s.clone()));
    assert_ne!(s.stable_hash(), old.stable_hash());
    assert_eq!(s.describe_change(&old, true), "Saturation: Blue -30");
    let mut m = old.clone();
    m.bw_mix[1] = 15.0;
    assert_eq!(m.describe_change(&old, true), "B&W Mix: Orange +15");
    assert!(DevelopSettings::from_json(r#"{"exposure":0.5}"#).unwrap().hsl.is_default());
}

fn graded(f: impl FnOnce(&mut ColorGrading)) -> DevelopSettings {
    let mut s = DevelopSettings::default();
    f(&mut s.grading);
    s
}

#[test]
fn grading_tints_shadows_and_highlights_separately() {
    let (dark, light) = ([45u8, 45, 45], [215u8, 215, 215]);
    let blue_shadows = graded(|g| {
        g.shadows.hue = 220.0;
        g.shadows.saturation = 80.0;
    });
    let d = patch(dark, &blue_shadows);
    assert!(d[2] > d[0] + 8, "dark tones turn blue: {d:?}");
    let l = patch(light, &blue_shadows);
    assert!(l[2].abs_diff(l[0]) <= 3, "light tones stay neutral: {l:?}");

    let warm_highlights = graded(|g| {
        g.highlights.hue = 35.0;
        g.highlights.saturation = 80.0;
    });
    let l = patch(light, &warm_highlights);
    assert!(l[0] > l[2] + 8, "light tones warm up: {l:?}");
    let d = patch(dark, &warm_highlights);
    assert!(d[0].abs_diff(d[2]) <= 3, "dark tones stay neutral: {d:?}");
}

#[test]
fn grading_luminance_and_split_toning_black_and_white() {
    let darker = graded(|g| g.midtones.luminance = -60.0);
    assert!(patch([128, 128, 128], &darker)[1] < 115);
    // Black & White with toned shadows and highlights: split toning.
    let mut s = graded(|g| {
        g.shadows.hue = 220.0;
        g.shadows.saturation = 70.0;
        g.highlights.hue = 40.0;
        g.highlights.saturation = 70.0;
    });
    s.treatment = Treatment::BlackAndWhite;
    let d = patch([60, 60, 60], &s);
    let l = patch([220, 220, 220], &s);
    assert!(d[2] > d[0] && l[0] > l[2], "cool shadows {d:?}, warm highlights {l:?}");
}

#[test]
fn grading_settings_round_trip_and_name_history_steps() {
    let old = DevelopSettings::default();
    let s = graded(|g| {
        g.shadows.hue = 200.0;
        g.shadows.saturation = 25.0;
    });
    assert_eq!(DevelopSettings::from_json(&s.to_json()), Some(s.clone()));
    assert_ne!(s.stable_hash(), old.stable_hash());
    assert_eq!(s.describe_change(&old, true), "Color Grading: Shadows");
    let b = graded(|g| g.balance = 30.0);
    assert_eq!(b.describe_change(&old, true), "Color Grading: Balance +30");
    let h = graded(|g| g.highlights.luminance = -10.0);
    assert_eq!(h.describe_change(&old, true), "Color Grading: Highlights Luminance -10");
    assert_eq!(DevelopSettings::from_json(r#"{"exposure":0.5}"#).unwrap().grading, ColorGrading::default());
}

#[test]
fn detail_defaults_depend_on_the_file_type() {
    let raw = DevelopSettings::default_for(true);
    assert_eq!((raw.detail.sharpen_amount, raw.detail.color), (40.0, 25.0));
    assert!(raw.is_default_for(true) && !raw.is_default_for(false));
    assert!(DevelopSettings::default_for(false).is_default());
    // Settings saved before Detail existed keep their look: no Detail.
    assert!(!DevelopSettings::from_json(r#"{"exposure":0.5}"#).unwrap().detail.is_active());
    let s = DevelopSettings {
        detail: Detail { sharpen_amount: 60.0, ..Detail::default() },
        ..Default::default()
    };
    assert_eq!(s.describe_change(&DevelopSettings::default(), true), "Sharpening: Amount 60");
    assert_ne!(s.stable_hash(), DevelopSettings::default().stable_hash());
}

#[test]
fn detail_sharpens_and_smooths_and_regions_still_match() {
    // A soft edge with a little noise.
    let img: RgbImage = ImageBuffer::from_fn(160, 120, |x, y| {
        let base = if x < 80 { 70.0 } else { 170.0 };
        let soft = base + (x as f32 - 80.0).clamp(-3.0, 3.0) * 4.0;
        let n = ((x * 31 + y * 17) % 7) as f32 - 3.0;
        let v = (soft + n * 3.0).clamp(0.0, 255.0) as u8;
        Rgb([v, v, v])
    });
    let w = rendered(&img);
    let plain = render_srgb8(&w, &DevelopSettings::default());
    let sharp = render_srgb8(
        &w,
        &DevelopSettings {
            detail: Detail { sharpen_amount: 120.0, ..Detail::default() },
            ..Default::default()
        },
    );
    // The steepest step across the soft edge.
    let edge = |i: &RgbImage| {
        (76..84)
            .map(|x| f32::from(i.get_pixel(x + 1, 60)[1]) - f32::from(i.get_pixel(x, 60)[1]))
            .fold(0.0, f32::max)
    };
    assert!(edge(&sharp) > edge(&plain) + 5.0, "{} vs {}", edge(&sharp), edge(&plain));
    let smooth = render_srgb8(
        &w,
        &DevelopSettings { detail: Detail { luminance: 90.0, ..Detail::default() }, ..Default::default() },
    );
    let s = |i: &RgbImage| spread_in(i, 10..60, 20..100);
    assert!(s(&smooth) < s(&plain) * 0.7, "noise drops: {} vs {}", s(&smooth), s(&plain));

    // A region of a detailed render matches that part of the full render (the margin).
    let st = DevelopSettings {
        detail: Detail { sharpen_amount: 80.0, luminance: 40.0, color: 50.0, ..Detail::default() },
        ..Default::default()
    };
    let whole = render_srgb8(&w, &st);
    let part = render_region_srgb8(&w, &st, Region { x: 50, y: 30, w: 60, h: 50 });
    for y in 0..50 {
        for x in 0..60 {
            let (a, b) = (part.get_pixel(x, y), whole.get_pixel(x + 50, y + 30));
            for c in 0..3 {
                assert!(a[c].abs_diff(b[c]) <= 1, "({x}, {y}): {a:?} vs {b:?}");
            }
        }
    }
}

#[test]
fn effects_vignette_and_grain_render_and_regions_still_match() {
    let img: RgbImage = ImageBuffer::from_pixel(160, 120, Rgb([128, 128, 128]));
    let w = rendered(&img);
    let centre_and_corner = |i: &RgbImage| (i.get_pixel(80, 60)[1], i.get_pixel(2, 2)[1]);
    for style in VignetteStyle::ALL {
        let st = DevelopSettings {
            effects: Effects { vignette_style: style, vignette_amount: -80.0, ..Effects::default() },
            ..Default::default()
        };
        let (c, k) = centre_and_corner(&render_srgb8(&w, &st));
        assert!(c.abs_diff(128) <= 2 && k + 30 < c, "{style:?}: centre {c}, corner {k}");
    }
    let light = DevelopSettings {
        effects: Effects { vignette_amount: 80.0, ..Effects::default() },
        ..Default::default()
    };
    let (c, k) = centre_and_corner(&render_srgb8(&w, &light));
    assert!(k > c + 30, "lighter corners: {c} {k}");

    let grain = DevelopSettings {
        effects: Effects { grain_amount: 80.0, ..Effects::default() },
        ..Default::default()
    };
    let g = render_srgb8(&w, &grain);
    assert!(spread_in(&g, 10..60, 20..100) > 2.0, "grain shows");

    // Regions match the whole render, with and without Detail.
    for detail in [Detail::default(), Detail { sharpen_amount: 60.0, color: 40.0, ..Detail::default() }] {
        let st = DevelopSettings {
            effects: Effects { vignette_amount: -60.0, grain_amount: 60.0, ..Effects::default() },
            detail,
            ..Default::default()
        };
        let whole = render_srgb8(&w, &st);
        let part = render_region_srgb8(&w, &st, Region { x: 50, y: 30, w: 60, h: 50 });
        for y in 0..50 {
            for x in 0..60 {
                let (a, b) = (part.get_pixel(x, y), whole.get_pixel(x + 50, y + 30));
                assert!((0..3).all(|c| a[c].abs_diff(b[c]) <= 1), "({x}, {y}): {a:?} vs {b:?}");
            }
        }
    }
}

#[test]
fn sharpening_previews_are_grey_and_show_the_mask() {
    // A hard edge in a colourful image.
    let img: RgbImage =
        ImageBuffer::from_fn(80, 60, |x, _| if x < 40 { Rgb([90, 20, 10]) } else { Rgb([150, 200, 255]) });
    let w = rendered(&img);
    let st = DevelopSettings {
        detail: Detail { sharpen_amount: 60.0, sharpen_masking: 60.0, ..Detail::default() },
        ..Default::default()
    };
    let whole = Region::whole(&w);
    for view in [SharpenView::Amount, SharpenView::Edges, SharpenView::Mask] {
        let out = render_region_srgb8_view(&w, &st, whole, Some(view));
        assert!(
            out.pixels().all(|p| p[0].abs_diff(p[1]) <= 1 && p[1].abs_diff(p[2]) <= 1),
            "{view:?} is grey"
        );
    }
    // The mask: white along the edge, black in the flat areas.
    let mask = render_region_srgb8_view(&w, &st, whole, Some(SharpenView::Mask));
    let row: Vec<u8> = (30..50).map(|x| mask.get_pixel(x, 30)[0]).collect();
    assert!(mask.get_pixel(40, 30)[0] > 200 && mask.get_pixel(10, 30)[0] < 30, "{row:?}");
    // Edges: mid grey in flat areas, lighter and darker on either side of the edge.
    let edges = render_region_srgb8_view(&w, &st, whole, Some(SharpenView::Edges));
    let flat = edges.get_pixel(10, 30)[0];
    let (a, b) = (edges.get_pixel(39, 30)[0], edges.get_pixel(40, 30)[0]);
    assert!(a.abs_diff(flat) > 10 && b.abs_diff(flat) > 10, "{flat} {a} {b}");
    // Without a preview the photo renders normally.
    let normal = render_region_srgb8_view(&w, &st, whole, None);
    assert_eq!(normal, render_srgb8(&w, &st));
}

#[test]
fn transform_moves_pixels_fills_white_and_regions_still_match() {
    // A dark frame with a bright square in the middle.
    let img: RgbImage = ImageBuffer::from_fn(120, 80, |x, y| {
        if (50..70).contains(&x) && (30..50).contains(&y) { Rgb([220, 220, 220]) } else { Rgb([40, 40, 40]) }
    });
    let w = rendered(&img);
    let plain = render_srgb8(&w, &DevelopSettings::default());
    // Identity changes nothing.
    let same = DevelopSettings { transform: Transform::default(), ..Default::default() };
    assert_eq!(render_srgb8(&w, &same), plain);
    // Shrinking leaves white around the photo; offset moves the square.
    let small =
        DevelopSettings { transform: Transform { scale: 60.0, ..Default::default() }, ..Default::default() };
    let out = render_srgb8(&w, &small);
    assert_eq!(out.get_pixel(1, 1).0, [255, 255, 255]);
    assert!(out.get_pixel(60, 40)[0] > 200, "the centre stays");
    let moved = DevelopSettings {
        transform: Transform { x_offset: 50.0, ..Default::default() },
        ..Default::default()
    };
    let out = render_srgb8(&w, &moved);
    assert!(out.get_pixel(60, 40)[0] < 100 && out.get_pixel(90, 40)[0] > 200, "the square moved right");

    // With everything that reaches across pixels on, a region matches the whole render.
    let st = DevelopSettings {
        transform: Transform { vertical: -40.0, rotate: 3.0, scale: 90.0, ..Default::default() },
        detail: Detail { sharpen_amount: 50.0, color: 30.0, ..Detail::default() },
        clarity: 30.0,
        ..Default::default()
    };
    let whole = render_srgb8(&w, &st);
    let part = render_region_srgb8(&w, &st, Region { x: 30, y: 20, w: 50, h: 40 });
    for y in 0..40 {
        for x in 0..50 {
            let (a, b) = (part.get_pixel(x, y), whole.get_pixel(x + 30, y + 20));
            assert!((0..3).all(|c| a[c].abs_diff(b[c]) <= 1), "({x}, {y}): {a:?} vs {b:?}");
        }
    }
    // The picker samples what is shown: the square's centre after the offset.
    let (sx, _) = moved.transform.source_point(0.75, 0.5, 120, 80);
    assert!((sx - 0.5).abs() < 0.01, "{sx}");
}

#[test]
fn constrain_crop_removes_the_white_and_upright_straightens_a_leaning_frame() {
    // Mid grey with dark vertical bars leaning outwards towards the top (converging
    // verticals, as when looking up at a building), and level bars.
    let (w, h) = (240u32, 160u32);
    let img: RgbImage = ImageBuffer::from_fn(w, h, |x, y| {
        let t = 1.0 - y as f32 / h as f32;
        let lean = |base: f32| base + (base - 120.0) * 0.12 * t;
        let near = |c: f32| (x as f32 - c).abs() < 2.0;
        let bar = [40.0, 80.0, 160.0, 200.0].iter().any(|&b| near(lean(b))) || y % 40 < 2;
        if bar { Rgb([30, 30, 30]) } else { Rgb([150, 150, 150]) }
    });
    let src = rendered(&img);
    let s = apply_upright(&src, &DevelopSettings::default(), Upright::Vertical);
    let [pitch, yaw, roll] = s.transform.upright_angles;
    assert!(pitch.abs() > 1.0 && yaw == 0.0 && roll.abs() < 1.0, "{:?}", s.transform.upright_angles);

    // Without the crop the corners turn white; with it, they don't, and the photo is smaller
    // in the same shape.
    let open = render_srgb8(&src, &s);
    assert_eq!(open.dimensions(), (w, h));
    let white = |i: &RgbImage| {
        [(0, 0), (i.width() - 1, 0), (0, i.height() - 1), (i.width() - 1, i.height() - 1)]
            .iter()
            .filter(|&&(x, y)| i.get_pixel(x, y).0 == [255, 255, 255])
            .count()
    };
    assert!(white(&open) >= 2);
    let cropped =
        DevelopSettings { transform: Transform { constrain_crop: true, ..s.transform.clone() }, ..s.clone() };
    let out = render_srgb8(&src, &cropped);
    let (cw, ch) = out.dimensions();
    assert!(cw < w && ch < h, "{cw}x{ch}");
    assert!((cw as f32 / ch as f32 - 1.5).abs() < 0.03);
    assert_eq!(white(&out), 0);
    assert_eq!((cw, ch), cropped.transform.output_size(w, h));
    // Regions of the cropped photo match it.
    let part = render_region_srgb8(&src, &cropped, Region { x: 10, y: 10, w: 40, h: 30 });
    assert_eq!(part.get_pixel(5, 5), out.get_pixel(15, 15));
}

fn radial(center: [f32; 2], r: f32, exposure: f32) -> Mask {
    let mut m = Mask::new("Mask 1", Shape::Radial { center, radii: [r, r], angle: 0.0, feather: 0.0 });
    m.adjust.exposure = exposure;
    m
}

#[test]
fn masked_exposure_stays_inside_and_regions_still_match() {
    let img: RgbImage = ImageBuffer::from_fn(120, 80, |x, y| {
        let v = 90 + ((x * 7 + y * 3) % 11) as u8;
        Rgb([v, v, v - 10])
    });
    let w = rendered(&img);
    let plain = render_srgb8(&w, &DevelopSettings::default());
    let s = DevelopSettings { masks: vec![radial([0.25, 0.5], 0.15, 1.0)], ..Default::default() };
    let out = render_srgb8(&w, &s);
    // Inside (30, 40): brighter; far outside (100, 40): unchanged.
    assert!(out.get_pixel(30, 40)[1] > plain.get_pixel(30, 40)[1] + 30);
    assert_eq!(out.get_pixel(100, 40), plain.get_pixel(100, 40));
    // A hidden mask, or one at Amount 0, does nothing.
    let mut hidden = s.clone();
    hidden.masks[0].visible = false;
    assert_eq!(render_srgb8(&w, &hidden), plain);

    // Every local slider of stage 1 changes the inside; a region matches the whole render
    // (with Detail on, too).
    let mut all = s.clone();
    let a = &mut all.masks[0].adjust;
    (a.temp, a.tint, a.contrast, a.highlights, a.shadows, a.whites, a.blacks) =
        (30.0, -20.0, 40.0, -30.0, 30.0, 20.0, -20.0);
    (a.saturation, a.hue) = (-50.0, 40.0);
    all.detail.sharpen_amount = 40.0;
    let whole = render_srgb8(&w, &all);
    let part = render_region_srgb8(&w, &all, Region { x: 10, y: 20, w: 50, h: 40 });
    for y in 0..40 {
        for x in 0..50 {
            let (p, q) = (part.get_pixel(x, y), whole.get_pixel(x + 10, y + 20));
            assert!((0..3).all(|c| p[c].abs_diff(q[c]) <= 1), "({x}, {y}): {p:?} vs {q:?}");
        }
    }
}

#[test]
fn masks_follow_the_photo_through_transform_and_crop() {
    let img: RgbImage = ImageBuffer::from_pixel(120, 80, Rgb([100, 100, 100]));
    let w = rendered(&img);
    let transform = Transform { x_offset: 40.0, rotate: 3.0, constrain_crop: true, ..Default::default() };
    let s = DevelopSettings { masks: vec![radial([0.3, 0.5], 0.08, 1.5)], transform, ..Default::default() };
    let out = render_srgb8(&w, &s);
    let (ow, oh) = out.dimensions();
    // Where the mask's centre is shown now: bright; where it was before the transform: not.
    let (x, y) = s.transform.output_point(0.3, 0.5, 120, 80).unwrap();
    let at = |u: f32, v: f32| out.get_pixel((u * ow as f32) as u32, (v * oh as f32) as u32)[1];
    assert!(at(x, y) > 150, "{}", at(x, y));
    assert!(at(0.3, 0.5) < 120, "{}", at(0.3, 0.5));
    // The overlay is the mask, at the developed size.
    let overlay = render_mask(&w, &s, 0).unwrap();
    assert_eq!(overlay.dimensions(), (ow, oh));
    assert_eq!(overlay.get_pixel((x * ow as f32) as u32, (y * oh as f32) as u32)[0], 255);
    assert!(render_mask(&w, &s, 1).is_none());
}

#[test]
fn settings_without_masks_still_load_and_masks_are_named_in_history() {
    let old = r#"{"process_version":1,"exposure":0.5}"#;
    let s = DevelopSettings::from_json(old).unwrap();
    assert!(s.masks.is_empty());
    assert_eq!(s.stable_hash(), DevelopSettings { exposure: 0.5, ..Default::default() }.stable_hash());
    let with = DevelopSettings { masks: vec![radial([0.5, 0.5], 0.2, 0.0)], ..s.clone() };
    assert_eq!(with.describe_change(&s, false), "Add Radial Gradient Mask");
    let mut brighter = with.clone();
    brighter.masks[0].adjust.exposure = 0.5;
    assert_eq!(brighter.describe_change(&with, false), "Mask 1: Exposure +0.50");
    assert_ne!(brighter.stable_hash(), with.stable_hash());
    let mut sub = brighter.clone();
    sub.masks[0].components.push(Component {
        mode: CombineMode::Subtract,
        invert: false,
        shape: Shape::Linear { start: [0.0, 0.0], end: [0.0, 0.5] },
    });
    assert_eq!(sub.describe_change(&brighter, false), "Subtract Linear Gradient");
    assert_eq!(s.describe_change(&sub, false), "Delete Mask 1");
}
