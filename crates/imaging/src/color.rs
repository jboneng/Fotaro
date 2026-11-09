//! ICC color management: convert embedded profiles (scanner, Adobe RGB, ProPhoto...) to sRGB.

use image::{GrayImage, RgbImage};
use lcms2::{ColorSpaceSignature, InfoType, Intent, Locale, PixelFormat, Profile, Transform};

pub(crate) fn is_srgb(profile: &Profile) -> bool {
    profile
        .info(InfoType::Description, Locale::none())
        .is_some_and(|d| d.to_ascii_lowercase().contains("srgb"))
}

/// Convert an RGB image in place from `icc` to sRGB. Unusable profiles are ignored.
pub fn rgb_to_srgb(img: &mut RgbImage, icc: &[u8]) {
    let Ok(profile) = Profile::new_icc(icc) else {
        log::warn!("ignoring unreadable ICC profile");
        return;
    };
    if profile.color_space() != ColorSpaceSignature::RgbData || is_srgb(&profile) {
        return;
    }
    match Transform::<u8, u8>::new(
        &profile,
        PixelFormat::RGB_8,
        &Profile::new_srgb(),
        PixelFormat::RGB_8,
        Intent::Perceptual,
    ) {
        Ok(t) => t.transform_in_place(img.as_mut()),
        Err(e) => log::warn!("ICC transform failed: {e}"),
    }
}

/// Convert a grayscale image with a gray ICC profile to sRGB. `None` if the profile
/// is not a usable gray profile (caller then expands gray to RGB as-is).
pub fn gray_to_srgb(img: &GrayImage, icc: &[u8]) -> Option<RgbImage> {
    let profile = Profile::new_icc(icc).ok()?;
    if profile.color_space() != ColorSpaceSignature::GrayData {
        return None;
    }
    let t = Transform::<u8, u8>::new(
        &profile,
        PixelFormat::GRAY_8,
        &Profile::new_srgb(),
        PixelFormat::RGB_8,
        Intent::Perceptual,
    )
    .ok()?;
    let mut out = RgbImage::new(img.width(), img.height());
    t.transform_pixels(img.as_raw(), out.as_mut());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_profile_is_a_no_op() {
        let icc = Profile::new_srgb().icc().unwrap();
        let mut img = RgbImage::from_pixel(2, 2, image::Rgb([10, 128, 250]));
        rgb_to_srgb(&mut img, &icc);
        assert_eq!(img.get_pixel(0, 0).0, [10, 128, 250]);
    }

    #[test]
    fn garbage_profile_is_ignored() {
        let mut img = RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3]));
        rgb_to_srgb(&mut img, b"not an icc profile");
        assert_eq!(img.get_pixel(0, 0).0, [1, 2, 3]);
    }
}
