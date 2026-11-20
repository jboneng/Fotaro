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

#[test]
fn crop_keeps_the_rectangle_turns_clockwise_and_regions_still_match() {
    // Left half dark, right half bright, a bright square near the top-left.
    let img: RgbImage = ImageBuffer::from_fn(160, 100, |x, y| {
        if (10..30).contains(&x) && (10..30).contains(&y) || x >= 80 {
            Rgb([220, 220, 220])
        } else {
            Rgb([40, 40, 40])
        }
    });
    let w = rendered(&img);
    let crop = |c: Crop| DevelopSettings {
        transform: Transform { crop: c, ..Default::default() },
        ..Default::default()
    };
    // The left half: half as wide, all dark except the square.
    let half = crop(Crop { right: 0.5, ..Crop::default() });
    let out = render_srgb8(&w, &half);
    assert_eq!(out.dimensions(), (80, 100));
    assert!(out.get_pixel(70, 50)[0] < 100 && out.get_pixel(20, 20)[0] > 180);
    assert_eq!(half.transform.output_size(160, 100), (80, 100));
    // A clockwise turn: the dark/bright edge (vertical, at the centre) leans with its top to
    // the right, so above the centre is dark and below it bright.
    let turned = render_srgb8(&w, &crop(Crop { angle: 30.0, ..Crop::default() }));
    let plain = render_srgb8(&w, &DevelopSettings::default());
    assert!(plain.get_pixel(120, 50)[0] > 180, "right is bright");
    assert!(turned.get_pixel(80, 15)[0] < 100, "above the centre: dark");
    assert!(turned.get_pixel(80, 85)[0] > 180, "below the centre: bright");
    // Regions of a cropped, turned photo match it.
    let s = crop(Crop { left: 0.1, top: 0.2, right: 0.8, bottom: 0.9, angle: -7.5 });
    let whole = render_srgb8(&w, &s);
    let part = render_region_srgb8(&w, &s, Region { x: 10, y: 10, w: 40, h: 30 });
    for y in 0..30 {
        for x in 0..40 {
            let (a, b) = (part.get_pixel(x, y), whole.get_pixel(x + 10, y + 10));
            assert!((0..3).all(|c| a[c].abs_diff(b[c]) <= 1), "({x}, {y})");
        }
    }
    // Old settings (no crop) load with the full frame.
    let old = DevelopSettings::from_json(r#"{"transform":{"vertical":10}}"#).unwrap();
    assert!(old.transform.crop.is_full());
}

#[test]
fn constrain_to_image_and_aspect_presets() {
    let aspect = 1.5;
    let mut c = Crop { angle: 10.0, ..Crop::default() };
    assert!(!c.inside(aspect));
    c.fit_inside(aspect);
    assert!(c.inside(aspect) && c.right - c.left > 0.6, "{c:?}");
    assert!((c.ratio(aspect) - 1.5).abs() < 1e-3, "keeps its shape");
    assert!(((c.left + c.right) / 2.0 - 0.5).abs() < 1e-4);
    // A square crop of a 3:2 frame is the full height, centred.
    let sq = Crop::default().with_ratio(1.0, aspect);
    assert!((sq.ratio(aspect) - 1.0).abs() < 1e-4 && sq.top == 0.0 && sq.bottom == 1.0);
    assert!(((sq.left + sq.right) / 2.0 - 0.5).abs() < 1e-5);
}

#[test]
fn auto_straighten_levels_a_tilted_photo() {
    // Level bars and posts, then tilted 4° clockwise by a render.
    let img: RgbImage = ImageBuffer::from_fn(360, 240, |x, y| {
        if y % 60 < 3 || x % 90 < 3 { Rgb([30, 30, 30]) } else { Rgb([200, 200, 200]) }
    });
    let level = rendered(&img);
    let tilt = DevelopSettings {
        transform: Transform { crop: Crop { angle: 4.0, ..Crop::default() }, ..Default::default() },
        ..Default::default()
    };
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("tilted.png");
    render_srgb8(&level, &tilt).save(&path).unwrap();
    let tilted = WorkingImage::load(&path, crate::SourceKind::Image, None).unwrap().0;
    let angle = straighten_angle(&tilted, &Transform::default());
    assert!((angle + 4.0).abs() < 0.5, "{angle}");
}

#[test]
fn spots_heal_in_renders_and_regions_still_match() {
    // Mid grey with a dark dust spot at (40, 30).
    let img: RgbImage = ImageBuffer::from_fn(120, 80, |x, y| {
        if (x as f32 - 40.0).hypot(y as f32 - 30.0) < 3.0 { Rgb([20, 20, 20]) } else { Rgb([128, 128, 128]) }
    });
    let w = rendered(&img);
    let spot = Spot {
        points: vec![[40.0 / 120.0, 30.0 / 80.0]],
        size: 0.1,
        offset: find_source(
            &w,
            &Spot { points: vec![[40.0 / 120.0, 30.0 / 80.0]], size: 0.1, ..Spot::default() },
            None,
        ),
        ..Spot::default()
    };
    let s = DevelopSettings { spots: vec![spot], ..Default::default() };
    let out = render_srgb8(&w, &s);
    assert!(out.get_pixel(40, 30)[0].abs_diff(128) <= 3, "{:?}", out.get_pixel(40, 30));
    // With a crop on top, a region matches the whole render.
    let cropped = DevelopSettings {
        transform: Transform {
            crop: Crop { left: 0.1, right: 0.9, angle: 3.0, ..Crop::default() },
            ..Default::default()
        },
        ..s.clone()
    };
    let whole = render_srgb8(&w, &cropped);
    let part = render_region_srgb8(&w, &cropped, Region { x: 20, y: 10, w: 40, h: 40 });
    assert!((0..40).all(|y| (0..40).all(|x| part.get_pixel(x, y) == whole.get_pixel(x + 20, y + 10))));
    assert_eq!(s.describe_change(&DevelopSettings::default(), false), "Remove: Heal");
}

#[test]
fn visualize_spots_shows_edges_white_on_black() {
    // Flat grey with one small dark dust spot.
    let img: RgbImage = ImageBuffer::from_fn(80, 60, |x, y| {
        if (x as f32 - 40.0).hypot(y as f32 - 30.0) < 2.0 { Rgb([60, 60, 60]) } else { Rgb([150, 150, 150]) }
    });
    let w = rendered(&img);
    let s = DevelopSettings::default();
    let low = render_region_srgb8_view(&w, &s, Region::whole(&w), Some(SharpenView::Spots(0)));
    let high = render_region_srgb8_view(&w, &s, Region::whole(&w), Some(SharpenView::Spots(100)));
    assert_eq!(low.get_pixel(5, 5).0, [0, 0, 0], "flat areas are black");
    assert!(high.get_pixel(42, 30)[0] > 200, "the spot's edge is white: {:?}", high.get_pixel(42, 30));
    assert!(high.get_pixel(42, 30)[0] >= low.get_pixel(42, 30)[0], "more shows as the threshold rises");
    assert!(high.pixels().all(|p| p[0] == p[1] && p[1] == p[2]), "grey");
}

#[test]
fn ai_masks_follow_the_photos_edges_and_regions_still_match() {
    // A bright disc on a dark, slightly noisy ground; the "model" saw it as a coarse blur.
    let (cx, cy, radius) = (120.0f32, 80.0f32, 40.0f32);
    let img: RgbImage = ImageBuffer::from_fn(240, 160, |x, y| {
        let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy);
        let n = ((x * 7 + y * 13) % 9) as u8;
        if d < radius { Rgb([220 + n, 215 + n, 200 + n]) } else { Rgb([40 + n, 45 + n, 50 + n]) }
    });
    let w = rendered(&img);
    let picture = image::GrayImage::from_fn(30, 20, |x, y| {
        let d = ((x as f32 + 0.5) * 8.0 - cx).hypot((y as f32 + 0.5) * 8.0 - cy);
        image::Luma([((0.5 - (d - radius) / 32.0).clamp(0.0, 1.0) * 255.0) as u8])
    });
    let _store_lock = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let store = tempfile::tempdir().unwrap();
    set_mask_store(Some(store.path().to_path_buf()));
    save_mask_picture("test-disc", &picture).unwrap();
    assert!(store.path().join("test-disc.png").exists());
    let shape = Shape::Raster { id: "test-disc".into(), selects: "Subject".into(), model: "test".into() };
    let mut mask = Mask::new("Mask 1", shape);
    mask.adjust.exposure = 1.0;
    let s = DevelopSettings { masks: vec![mask], ..Default::default() };

    // The refined mask is closer to the disc than the picture, inside and outside.
    let m = render_mask(&w, &s, 0).unwrap();
    let at = |d: f32| f32::from(m.get_pixel((cx + radius + d) as u32, cy as u32)[0]) / 255.0;
    let coarse = |d: f32| 0.5 - d / 32.0;
    assert!(at(-7.0) > coarse(-6.5) + 0.07, "inside {} vs {}", at(-7.0), coarse(-6.5));
    assert!(at(7.0) < coarse(7.5) * 0.5, "outside {} vs {}", at(7.0), coarse(7.5));

    // Regions render like the whole.
    let whole = render_srgb8(&w, &s);
    let part = render_region_srgb8(&w, &s, Region { x: 70, y: 30, w: 90, h: 70 });
    for y in 0..70 {
        for x in 0..90 {
            let (p, q) = (part.get_pixel(x, y), whole.get_pixel(x + 70, y + 30));
            assert!((0..3).all(|c| p[c].abs_diff(q[c]) <= 1), "({x}, {y}): {p:?} vs {q:?}");
        }
    }

    // A missing picture selects nothing.
    let mut gone = s.clone();
    gone.masks[0].components[0].shape =
        Shape::Raster { id: "not-there".into(), selects: "Sky".into(), model: "test".into() };
    assert_eq!(render_srgb8(&w, &gone), render_srgb8(&w, &DevelopSettings::default()));
    set_mask_store(None);
}

/// A mask over the left part of the photo (full left of 45%, none right of 55%).
fn left_mask(adjust: impl Fn(&mut LocalAdjust)) -> Mask {
    let mut m = Mask::new("Mask 1", Shape::Linear { start: [0.45, 0.5], end: [0.55, 0.5] });
    adjust(&mut m.adjust);
    m
}

#[test]
fn masked_clarity_texture_and_dehaze_act_only_inside_and_regions_still_match() {
    let left = |x0: u32| x0..x0 + 100;
    let rows = 80..220;
    for (pattern, set) in [
        (patterned(16, 0), (|a: &mut LocalAdjust| a.clarity = 80.0) as fn(&mut LocalAdjust)),
        (patterned(0, 12), |a: &mut LocalAdjust| a.texture = 80.0),
    ] {
        let img = rendered(&pattern);
        let base = render_srgb8(&img, &DevelopSettings::default());
        let s = DevelopSettings { masks: vec![left_mask(set)], ..Default::default() };
        let out = render_srgb8(&img, &s);
        let (inside, before) =
            (spread_in(&out, left(40), rows.clone()), spread_in(&base, left(40), rows.clone()));
        assert!(inside > before * 1.15, "{inside} vs {before}");
        // Right of the gradient: untouched.
        for (x, y) in [(300, 100), (360, 200), (250, 150)] {
            assert_eq!(out.get_pixel(x, y), base.get_pixel(x, y));
        }
    }

    // Dehaze on a hazy photo: clearer (more contrast) on the left only; negative adds haze.
    let hazy: RgbImage = ImageBuffer::from_fn(400, 300, |x, y| {
        let v = if (x / 30 + y / 30) % 2 == 0 { 170 } else { 200 };
        Rgb([v - 10, v, v + 20])
    });
    let img = rendered(&hazy);
    let base = render_srgb8(&img, &DevelopSettings::default());
    for (amount, more) in [(70.0, true), (-70.0, false)] {
        let s = DevelopSettings { masks: vec![left_mask(|a| a.dehaze = amount)], ..Default::default() };
        let out = render_srgb8(&img, &s);
        let (inside, before) =
            (spread_in(&out, left(20), rows.clone()), spread_in(&base, left(20), rows.clone()));
        if more {
            assert!(inside > before * 1.2, "{inside} vs {before}");
        } else {
            assert!(inside < before * 0.8, "{inside} vs {before}");
        }
        assert_eq!(out.get_pixel(320, 150), base.get_pixel(320, 150));
    }

    // All three with the global ones on too: a region across the gradient matches the whole
    // render (also through Detail's path).
    let img = rendered(&patterned(16, 12));
    let mut s = DevelopSettings {
        clarity: 20.0,
        texture: -10.0,
        dehaze: 10.0,
        masks: vec![left_mask(|a| (a.clarity, a.texture, a.dehaze) = (60.0, 50.0, 40.0))],
        ..Default::default()
    };
    for sharpen in [0.0, 40.0] {
        s.detail.sharpen_amount = sharpen;
        let whole = render_srgb8(&img, &s);
        let region = Region { x: 120, y: 60, w: 160, h: 120 };
        let part = render_region_srgb8(&img, &s, region);
        for y in 0..region.h {
            for x in 0..region.w {
                let (p, q) = (part.get_pixel(x, y), whole.get_pixel(x + region.x, y + region.y));
                assert!((0..3).all(|c| p[c].abs_diff(q[c]) <= 1), "({x}, {y}): {p:?} vs {q:?}");
            }
        }
    }
}

#[test]
fn masked_sharpness_noise_and_color_act_only_inside_and_regions_still_match() {
    let rows = 80..220;
    let img = rendered(&patterned(0, 12));
    let base = render_srgb8(&img, &DevelopSettings::default());
    let spread = |i: &RgbImage| spread_in(i, 40..140, rows.clone());
    let with = |set: fn(&mut LocalAdjust)| {
        render_srgb8(&img, &DevelopSettings { masks: vec![left_mask(set)], ..Default::default() })
    };
    // Sharpness: more fine contrast inside; negative softens. Noise: smooths the fine
    // pattern (it reads as noise). Outside: untouched.
    let sharper = with(|a| a.sharpness = 80.0);
    let softer = with(|a| a.sharpness = -80.0);
    let smooth = with(|a| a.noise = 100.0);
    assert!(spread(&sharper) > spread(&base) * 1.1, "{} vs {}", spread(&sharper), spread(&base));
    assert!(spread(&softer) < spread(&base) * 0.8, "{} vs {}", spread(&softer), spread(&base));
    assert!(spread(&smooth) < spread(&base) * 0.8, "{} vs {}", spread(&smooth), spread(&base));
    for out in [&sharper, &softer, &smooth] {
        for (x, y) in [(300, 100), (360, 200), (250, 150)] {
            assert_eq!(out.get_pixel(x, y), base.get_pixel(x, y));
        }
    }

    // Color: a blue swatch tints the inside blue, not the outside.
    let blue = with(|a| a.color = Some([240.0, 80.0]));
    let (p, q) = (blue.get_pixel(60, 150), base.get_pixel(60, 150));
    assert!(i32::from(p[2]) - i32::from(p[0]) > i32::from(q[2]) - i32::from(q[0]) + 10, "{p:?} vs {q:?}");
    assert_eq!(blue.get_pixel(330, 150), base.get_pixel(330, 150));

    // With global Detail too: a region across the gradient matches the whole render.
    let mut s = DevelopSettings {
        masks: vec![left_mask(|a| (a.sharpness, a.noise, a.color) = (60.0, 40.0, Some([30.0, 50.0])))],
        ..Default::default()
    };
    for amount in [0.0, 40.0] {
        s.detail.sharpen_amount = amount;
        let whole = render_srgb8(&img, &s);
        let region = Region { x: 120, y: 60, w: 160, h: 120 };
        let part = render_region_srgb8(&img, &s, region);
        for y in 0..region.h {
            for x in 0..region.w {
                let (p, q) = (part.get_pixel(x, y), whole.get_pixel(x + region.x, y + region.y));
                assert!((0..3).all(|c| p[c].abs_diff(q[c]) <= 1), "({x}, {y}): {p:?} vs {q:?}");
            }
        }
    }
}

#[test]
fn glow_spreads_light_around_bright_areas_and_regions_still_match() {
    // A dark room with a bright lamp (a white square) and a mid-grey patch.
    let scene: RgbImage = ImageBuffer::from_fn(400, 300, |x, y| {
        if (180..220).contains(&x) && (130..170).contains(&y) {
            Rgb([255, 255, 255])
        } else if (40..80).contains(&x) && (40..80).contains(&y) {
            Rgb([130, 130, 130])
        } else {
            Rgb([25, 25, 25])
        }
    });
    let img = rendered(&scene);
    let base = render_srgb8(&img, &DevelopSettings::default());
    let glow = |amount: f32, range: f32, warmth: f32| {
        let mut s = DevelopSettings::default();
        (s.effects.glow_amount, s.effects.glow_range, s.effects.glow_warmth) = (amount, range, warmth);
        s
    };
    // Off: nothing changes (and Glow's other sliders alone do nothing).
    assert_eq!(render_srgb8(&img, &glow(0.0, 100.0, 80.0)), base);
    let s = glow(80.0, 50.0, 0.0);
    let out = render_srgb8(&img, &s);
    let g = |i: &RgbImage, x, y| i32::from(i.get_pixel(x, y)[1]);
    // Next to the lamp: lifted; far from it: about the same.
    assert!(g(&out, 230, 150) > g(&base, 230, 150) + 15, "{} vs {}", g(&out, 230, 150), g(&base, 230, 150));
    assert!(g(&out, 380, 20) <= g(&base, 380, 20) + 3, "{} vs {}", g(&out, 380, 20), g(&base, 380, 20));
    // Range: at 0 only the brightest glow (not the grey patch); at 100 the patch does too.
    let near_patch = |s: &DevelopSettings| g(&render_srgb8(&img, s), 90, 60) - g(&base, 90, 60);
    assert!(near_patch(&glow(80.0, 100.0, 0.0)) > near_patch(&glow(80.0, 0.0, 0.0)) + 3);
    // Warmth: a warmer glow is redder than a cooler one.
    let rb = |s: &DevelopSettings| {
        let p = *render_srgb8(&img, s).get_pixel(232, 150);
        i32::from(p[0]) - i32::from(p[2])
    };
    assert!(rb(&glow(80.0, 50.0, 100.0)) > rb(&glow(80.0, 50.0, -100.0)) + 10);

    // A region renders like that part of the whole (also with Detail).
    let mut s = s;
    for amount in [0.0, 40.0] {
        s.detail.sharpen_amount = amount;
        let whole = render_srgb8(&img, &s);
        let region = Region { x: 150, y: 100, w: 140, h: 100 };
        let part = render_region_srgb8(&img, &s, region);
        for y in 0..region.h {
            for x in 0..region.w {
                let (p, q) = (part.get_pixel(x, y), whole.get_pixel(x + region.x, y + region.y));
                assert!((0..3).all(|c| p[c].abs_diff(q[c]) <= 1), "({x}, {y}): {p:?} vs {q:?}");
            }
        }
    }
}

#[test]
fn remove_spots_show_their_fill_inside_only_and_follow_their_shape() {
    // A gradient photo with a dark blob to remove.
    let photo: RgbImage = ImageBuffer::from_fn(300, 200, |x, y| {
        let d = (x as f32 - 150.0).hypot(y as f32 - 100.0);
        if d < 15.0 { Rgb([20, 20, 20]) } else { Rgb([(80 + x / 3) as u8, 120, (200 - y / 2) as u8]) }
    });
    let img = rendered(&photo);
    let spot = Spot {
        mode: SpotMode::Remove,
        points: vec![[0.5, 0.5]],
        size: 0.15, // 22.5 px
        feather: 0.0,
        opacity: 100.0,
        ..Spot::default()
    };
    assert!(spot.needs_fill());
    // Without a fill, nothing changes.
    let plain = render_srgb8(&img, &DevelopSettings::default());
    let mut s = DevelopSettings { spots: vec![spot.clone()], ..Default::default() };
    assert_eq!(render_srgb8(&img, &s), plain);

    // The input the model sees: a square around the spot, the hole marked.
    let input = fill_input(&img, &s.spots, None, 0, [1.0; 3]).unwrap();
    let (iw, ih) = input.image.dimensions();
    assert_eq!(iw, ih);
    assert_eq!(input.mask.get_pixel(iw / 2, ih / 2)[0], 255);
    assert_eq!(input.mask.get_pixel(2, 2)[0], 0);
    // A "model" that fills the hole with the colour just left of the blob.
    let left = *input.image.get_pixel(iw / 2 - 30, ih / 2);
    let mut filled = input.image.clone();
    for (x, y, p) in filled.enumerate_pixels_mut() {
        if input.mask.get_pixel(x, y)[0] > 127 {
            *p = left;
        }
    }
    let _store_lock = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let store = tempfile::tempdir().unwrap();
    set_mask_store(Some(store.path().to_path_buf()));
    save_fill_picture("test-fill", &filled).unwrap();
    s.spots[0].fill = Some(Fill {
        id: "test-fill".into(),
        rect: input.rect,
        gains: input.gains,
        made_for: spot.shape_key(),
        model: "test".into(),
    });
    assert!(!s.spots[0].needs_fill());
    let out = render_srgb8(&img, &s);
    // The blob is gone (about the colour beside it); away from the spot, nothing changed.
    let (c, beside) = (out.get_pixel(150, 100), plain.get_pixel(120, 100));
    assert!((0..3).all(|k| c[k].abs_diff(beside[k]) <= 6), "{c:?} vs {beside:?}");
    assert_eq!(out.get_pixel(40, 30), plain.get_pixel(40, 30));
    assert_eq!(out.get_pixel(150, 160), plain.get_pixel(150, 160));
    // Half opacity: halfway (in linear light, so not the midpoint of the encoded values).
    let mut half = s.clone();
    half.spots[0].opacity = 50.0;
    let h = render_srgb8(&img, &half);
    assert!(h.get_pixel(150, 100)[1] > plain.get_pixel(150, 100)[1] + 20);
    assert!(h.get_pixel(150, 100)[1] < out.get_pixel(150, 100)[1]);
    // A region renders like that part of the whole.
    let region = Region { x: 110, y: 60, w: 80, h: 80 };
    let part = render_region_srgb8(&img, &s, region);
    for y in 0..region.h {
        for x in 0..region.w {
            assert_eq!(part.get_pixel(x, y), out.get_pixel(x + region.x, y + region.y), "({x}, {y})");
        }
    }
    // Moved: the fill no longer fits, so it isn't shown (until a new one is made).
    let mut moved = s.clone();
    moved.spots[0].points = vec![[0.3, 0.5]];
    assert!(moved.spots[0].needs_fill());
    assert_eq!(render_srgb8(&img, &moved), plain);
    // Saved and read back, with and without the fill (older settings have none).
    let back = DevelopSettings::from_json(&s.to_json()).unwrap();
    assert_eq!(back.spots[0].fill, s.spots[0].fill);
    let old = r#"{"spots":[{"mode":"Heal","points":[[0.5,0.5]],"size":0.02}]}"#;
    assert!(DevelopSettings::from_json(old).unwrap().spots[0].fill.is_none());
    set_mask_store(None);
}

/// Tests that set the (global) mask and fill picture store take turns.
static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn spot_strokes_add_and_take_away_area() {
    let spot = Spot { points: vec![[0.3, 0.5]], size: 0.05, feather: 0.0, ..Spot::default() };
    let (w, h) = (400, 200);
    let at = |s: &Spot, x: f32, y: f32| SpotArea::new(s, w, h).at(x, y);
    // Radius 10 px around (120, 100).
    assert!(at(&spot, 120.0, 100.0) > 0.9);
    assert_eq!(at(&spot, 160.0, 100.0), 0.0);
    // A stroke to the right adds area; an erasing one takes some back.
    let mut bigger = spot.clone();
    bigger.strokes.push(SpotStroke { points: vec![[0.3, 0.5], [0.45, 0.5]], size: 0.05, erase: false });
    assert!(at(&bigger, 160.0, 100.0) > 0.9);
    let mut cut = bigger.clone();
    cut.strokes.push(SpotStroke { points: vec![[0.4, 0.5]], size: 0.03, erase: true });
    assert_eq!(at(&cut, 160.0, 100.0), 0.0);
    assert!(at(&cut, 175.0, 100.0) > 0.9);
    // The bounds cover what adds, and each change is a new shape (so Remove refills).
    let b = SpotArea::new(&bigger, w, h).bounds();
    assert!(b[0] <= 110.5 && b[2] >= 189.5, "{b:?}");
    assert_ne!(spot.shape_key(), bigger.shape_key());
    assert_ne!(bigger.shape_key(), cut.shape_key());
    // Healing covers the added area (it changes pixels there).
    let photo: RgbImage = ImageBuffer::from_fn(w, h, |x, _| Rgb([(x / 2) as u8, 100, 100]));
    let img = rendered(&photo);
    let plain = render_srgb8(&img, &DevelopSettings::default());
    let s = DevelopSettings {
        spots: vec![Spot { mode: SpotMode::Clone, offset: [0.0, 0.3], ..bigger }],
        ..Default::default()
    };
    let out = render_srgb8(&img, &s);
    assert_eq!(out.get_pixel(160, 100), plain.get_pixel(160, 160));
    // Old settings without strokes still load.
    let old = r#"{"spots":[{"mode":"Heal","points":[[0.5,0.5]],"size":0.02}]}"#;
    assert!(DevelopSettings::from_json(old).unwrap().spots[0].strokes.is_empty());
}

#[test]
fn dust_and_scratches_fill_the_found_defects_only() {
    let _store_lock = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // A smooth ramp with a dark speck.
    let photo: RgbImage = ImageBuffer::from_fn(300, 200, |x, y| {
        if (x as f32 - 150.0).hypot(y as f32 - 100.0) < 4.0 {
            Rgb([10, 10, 10])
        } else {
            Rgb([(60 + x / 2) as u8, 120, 160])
        }
    });
    let img = rendered(&photo);
    let plain = render_srgb8(&img, &DevelopSettings::default());
    // Detect from the picture, store the mask, turn the layer on.
    let lum: Vec<f32> =
        img.pixels.pixels().map(|p| (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).sqrt()).collect();
    let mask = detect_picture(&lum, 300, 200, 50.0, 12.0);
    assert_eq!(mask[100 * 300 + 150], 255);
    let store = tempfile::tempdir().unwrap();
    set_mask_store(Some(store.path().to_path_buf()));
    save_mask_picture("test-dust", &image::GrayImage::from_raw(300, 200, mask).unwrap()).unwrap();
    let mut layer = DustLayer { picture: Some("test-dust".into()), ..DustLayer::default() };
    layer.made_for = layer.key();
    let s = DevelopSettings { dust: Some(layer.clone()), ..Default::default() };
    let out = render_srgb8(&img, &s);
    // The speck is gone (about its surroundings); elsewhere nothing changed.
    let (c, near) = (out.get_pixel(150, 100), plain.get_pixel(140, 100));
    assert!((0..3).all(|k| c[k].abs_diff(near[k]) <= 8), "{c:?} vs {near:?}");
    assert_eq!(out.get_pixel(40, 40), plain.get_pixel(40, 40));
    // A region renders like that part of the whole.
    let region = Region { x: 120, y: 70, w: 60, h: 60 };
    let part = render_region_srgb8(&img, &s, region);
    for y in 0..region.h {
        for x in 0..region.w {
            assert_eq!(part.get_pixel(x, y), out.get_pixel(x + region.x, y + region.y), "({x}, {y})");
        }
    }
    // Other settings than the mask was made for, or off: nothing filled.
    let mut stale = s.clone();
    stale.dust.as_mut().unwrap().sensitivity = 80.0;
    assert!(stale.dust.as_ref().unwrap().needs_detection());
    assert_eq!(render_srgb8(&img, &stale), plain);
    let mut off = s.clone();
    off.dust.as_mut().unwrap().enabled = false;
    assert_eq!(render_srgb8(&img, &off), plain);
    // Saved and read back; older settings have none.
    assert_eq!(DevelopSettings::from_json(&s.to_json()).unwrap().dust, s.dust);
    assert!(DevelopSettings::from_json(r#"{"exposure":0.5}"#).unwrap().dust.is_none());
    set_mask_store(None);
}

#[test]
fn a_spot_with_an_area_picture_covers_that_shape() {
    let _store_lock = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let store = tempfile::tempdir().unwrap();
    set_mask_store(Some(store.path().to_path_buf()));
    // A 40 × 80 px "person" (an ellipse) in a picture over (100..160, 50..150) of a
    // 300 × 200 photo.
    let pic = image::GrayImage::from_fn(60, 100, |x, y| {
        let (dx, dy) = ((x as f32 - 30.0) / 20.0, (y as f32 - 50.0) / 40.0);
        image::Luma([if dx * dx + dy * dy <= 1.0 { 255 } else { 0 }])
    });
    save_mask_picture("test-person", &pic).unwrap();
    let rect = [100.0 / 300.0, 50.0 / 200.0, 160.0 / 300.0, 150.0 / 200.0];
    let spot = Spot {
        mode: SpotMode::Clone,
        points: vec![[130.0 / 300.0, 100.0 / 200.0]],
        size: 0.01,
        feather: 0.0,
        area: Some(SpotMask { id: "test-person".into(), rect }),
        ..Spot::default()
    };
    let area = SpotArea::new(&spot, 300, 200);
    // Inside the ellipse, outside it (but inside its picture), and far away.
    assert!(area.at(130.0, 100.0) > 0.99);
    assert!(area.edge(130.0, 100.0) > 15.0, "{}", area.edge(130.0, 100.0));
    assert_eq!(area.at(103.0, 55.0), 0.0);
    assert!(area.edge(20.0, 20.0) < -50.0);
    // The edge sits on the ellipse: about 20 px either side of the centre across.
    assert!(area.edge(130.0 + 19.0, 100.0) > 0.0 && area.edge(130.0 + 21.5, 100.0) < 0.0);
    let b = area.bounds();
    assert!((b[0] - 100.0).abs() < 1.0 && (b[3] - 150.0).abs() < 1.0, "{b:?}");
    // A stroke taking area away cuts into it.
    let mut cut = spot.clone();
    cut.strokes.push(SpotStroke { points: vec![[130.0 / 300.0, 100.0 / 200.0]], size: 0.02, erase: true });
    assert_eq!(SpotArea::new(&cut, 300, 200).at(130.0, 100.0), 0.0);
    // The area is part of the shape a fill is made for.
    let mut moved = spot.clone();
    moved.area.as_mut().unwrap().rect[0] += 0.01;
    assert_ne!(moved.shape_key(), spot.shape_key());
    // Cloned from 60 px to the right: inside the ellipse only.
    let photo: RgbImage =
        ImageBuffer::from_fn(300, 200, |x, _| if x < 150 { Rgb([200, 40, 40]) } else { Rgb([40, 40, 200]) });
    let img = rendered(&photo);
    let s = DevelopSettings {
        spots: vec![Spot { offset: [60.0 / 300.0, 0.0], ..spot.clone() }],
        ..Default::default()
    };
    let out = render_srgb8(&img, &s);
    let plain = render_srgb8(&img, &DevelopSettings::default());
    assert!(out.get_pixel(130, 100)[2] > out.get_pixel(130, 100)[0], "the inside is cloned (blue)");
    assert_eq!(out.get_pixel(103, 55), plain.get_pixel(103, 55), "the picture's corner is not");
    // Saved and read back; older spots have no area.
    assert_eq!(DevelopSettings::from_json(&s.to_json()).unwrap().spots, s.spots);
    set_mask_store(None);
}

/// A render that reuses what an earlier render kept (see `cache`) equals one that starts
/// afresh, whatever changed in between: with and without Detail (the two render paths).
#[test]
fn kept_stages_never_change_a_render() {
    let src =
        RgbImage::from_fn(96, 64, |x, y| Rgb([(x * 2 + y) as u8, (y * 3) as u8, ((x * y) % 255) as u8]));
    let img = rendered(&src);
    let stroke = |n: usize| Stroke {
        points: (0..n).map(|i| [0.2 + 0.05 * i as f32, 0.3 + 0.04 * i as f32]).collect(),
        size: 0.15,
        ..Stroke::default()
    };
    let mut plain = DevelopSettings {
        exposure: 0.2,
        highlights: -30.0,
        shadows: 20.0,
        texture: 20.0,
        clarity: 15.0,
        dehaze: 10.0,
        vibrance: 10.0,
        ..DevelopSettings::default()
    };
    let mut radial = Mask::new(
        "Radial",
        Shape::Radial { center: [0.5, 0.5], radii: [0.4, 0.3], angle: 10.0, feather: 50.0 },
    );
    radial.adjust.exposure = 0.5;
    radial.adjust.clarity = 20.0;
    let mut brush = Mask::new("Brush", Shape::Brush { strokes: vec![stroke(4)] });
    brush.adjust.saturation = 30.0;
    plain.masks = vec![radial, brush];
    let mut detailed = plain.clone();
    detailed.detail.sharpen_amount = 40.0;
    detailed.detail.color = 25.0;
    detailed.detail.luminance = 10.0;
    detailed.masks[1].adjust.sharpness = 20.0;

    type Change = (&'static str, fn(&mut DevelopSettings));
    let changes: [Change; 24] = [
        ("exposure", |s| s.exposure = 0.6),
        ("contrast", |s| s.contrast = 25.0),
        ("highlights", |s| s.highlights = -60.0),
        ("shadows off", |s| (s.highlights, s.shadows) = (0.0, 0.0)),
        ("clarity", |s| s.clarity = 40.0),
        ("texture", |s| s.texture = -30.0),
        ("texture and clarity off", |s| (s.texture, s.clarity) = (0.0, 0.0)),
        ("dehaze", |s| s.dehaze = 40.0),
        ("temp", |s| s.temp = 20.0),
        ("saturation", |s| s.saturation = 20.0),
        ("black and white", |s| s.treatment = Treatment::BlackAndWhite),
        ("sharpening", |s| s.detail.sharpen_amount = 80.0),
        ("luminance noise", |s| s.detail.luminance = 40.0),
        ("grain", |s| s.effects.grain_amount = 40.0),
        ("vignette", |s| s.effects.vignette_amount = -40.0),
        ("glow", |s| s.effects.glow_amount = 40.0),
        ("mask exposure", |s| s.masks[0].adjust.exposure = -0.5),
        ("mask clarity", |s| s.masks[0].adjust.clarity = -20.0),
        ("mask sharpness", |s| s.masks[1].adjust.sharpness = 60.0),
        ("mask amount", |s| s.masks[0].amount = 50.0),
        ("mask moved", |s| {
            if let Shape::Radial { center, .. } = &mut s.masks[0].components[0].shape {
                center[0] = 0.4;
            }
        }),
        ("mask hidden", |s| s.masks[1].visible = false),
        ("a spot", |s| {
            s.spots.push(Spot { points: vec![[0.3, 0.3]], size: 0.1, offset: [0.3, 0.2], ..Spot::default() })
        }),
        ("straightened", |s| s.transform.crop.angle = 2.0),
    ];
    for (path, base) in [("plain", &plain), ("with Detail", &detailed)] {
        for (name, change) in changes {
            let mut s = base.clone();
            change(&mut s);
            // From the base's kept stages to the change, and back.
            render_srgb8(&img, base);
            assert!(render_srgb8(&img, &s) == render_srgb8(&img.clone(), &s), "{path}: {name}");
            assert!(render_srgb8(&img, base) == render_srgb8(&img.clone(), base), "{path}: {name}, back");
            // A region, after the whole.
            let r = Region { x: 20, y: 10, w: 40, h: 30 };
            assert!(
                render_region_srgb8(&img, &s, r) == render_region_srgb8(&img.clone(), &s, r),
                "{path}: {name}, a region"
            );
        }
    }
    // One more brush point (the brush raster and the field are made again).
    let mut more = detailed.clone();
    more.masks[1].components[0].shape = Shape::Brush { strokes: vec![stroke(5)] };
    render_srgb8(&img, &detailed);
    assert!(render_srgb8(&img, &more) == render_srgb8(&img.clone(), &more));
    assert!(render_srgb8(&img, &more) != render_srgb8(&img, &detailed));
}

/// A stopped render gives nothing, and leaves nothing wrong behind.
#[test]
fn a_stopped_render_gives_nothing() {
    let img = rendered(&gradient());
    let s = DevelopSettings { clarity: 30.0, ..Default::default() };
    assert!(render_region_srgb8_until(&img, &s, Region::whole(&img), None, &|| true).is_none());
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let later = || calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 2;
    assert!(render_region_srgb8_until(&img, &s, Region::whole(&img), None, &later).is_none());
    assert!(render_srgb8(&img, &s) == render_srgb8(&img.clone(), &s));
}

/// Spots are healed going on from what was kept (the copy under the spot being worked on):
/// every step of adding, moving, resizing and removing spots gives what healing from
/// scratch gives.
#[test]
fn spots_healed_step_by_step_match_healing_afresh() {
    let src = RgbImage::from_fn(120, 80, |x, y| {
        Rgb([(x * 2) as u8, (y * 3) as u8, ((x * 7 + y * 13) % 255) as u8])
    });
    let img = rendered(&src);
    let spot = |x: f32, y: f32, mode: SpotMode| Spot {
        mode,
        points: vec![[x, y]],
        size: 0.12,
        offset: [0.2, 0.1],
        ..Spot::default()
    };
    let (a, b, c) =
        (spot(0.3, 0.3, SpotMode::Heal), spot(0.35, 0.4, SpotMode::Clone), spot(0.6, 0.5, SpotMode::Heal));
    let moved = |s: &Spot, dx: f32| Spot { points: vec![[s.points[0][0] + dx, s.points[0][1]]], ..s.clone() };
    let steps: Vec<Vec<Spot>> = vec![
        vec![a.clone()],
        vec![a.clone(), b.clone()],
        vec![a.clone(), moved(&b, 0.02)],
        vec![a.clone(), moved(&b, 0.04)],
        vec![a.clone(), moved(&b, 0.04), c.clone()],
        // An earlier spot dragged, twice (the second step goes on from the copy under it).
        vec![moved(&a, 0.02), moved(&b, 0.04), c.clone()],
        vec![moved(&a, 0.04), moved(&b, 0.04), c.clone()],
        // The last one removed, then all but one, then none and one again.
        vec![moved(&a, 0.04), moved(&b, 0.04)],
        vec![c.clone()],
        vec![],
        vec![b.clone()],
    ];
    for (i, spots) in steps.into_iter().enumerate() {
        let s = DevelopSettings { spots, ..Default::default() };
        assert!(render_srgb8(&img, &s) == render_srgb8(&img.clone(), &s), "step {i}");
    }
}
