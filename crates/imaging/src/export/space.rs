//! Output colour spaces (LrC's File Settings ▸ Color Space) and the conversion of
//! decoded pixels into them.

use image::{DynamicImage, ImageBuffer, Luma, Rgb};
use lcms2::{
    CIExyY, CIExyYTRIPLE, ColorSpaceSignature, DisallowCache, Flags, GlobalContext, Intent, Locale, MLU,
    PixelFormat, Profile, Tag, TagSignature, ToneCurve, Transform,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::decode::SourceSpace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ColorSpace {
    #[default]
    Srgb,
    DisplayP3,
    AdobeRgb,
    ProPhotoRgb,
}

const D65: (f64, f64) = (0.3127, 0.3290);
const D50: (f64, f64) = (0.3457, 0.3585);
const PROPHOTO: [(f64, f64); 3] = [(0.7347, 0.2653), (0.1596, 0.8404), (0.0366, 0.0001)];

impl ColorSpace {
    pub const ALL: [ColorSpace; 4] = [Self::Srgb, Self::DisplayP3, Self::AdobeRgb, Self::ProPhotoRgb];

    pub fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::DisplayP3 => "Display P3",
            Self::AdobeRgb => "Adobe RGB (1998)",
            Self::ProPhotoRgb => "ProPhoto RGB",
        }
    }

    /// The profile embedded in exported files.
    pub(crate) fn profile(self) -> Profile {
        match self {
            Self::Srgb => Profile::new_srgb(),
            Self::DisplayP3 => {
                // The sRGB tone curve with P3 primaries.
                let curve =
                    ToneCurve::new_parametric(4, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045])
                        .unwrap_or_else(|_| ToneCurve::new(2.2));
                rgb_profile(D65, [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)], &curve, "Display P3")
            }
            Self::AdobeRgb => rgb_profile(
                D65,
                [(0.64, 0.33), (0.21, 0.71), (0.15, 0.06)],
                &ToneCurve::new(563.0 / 256.0),
                "Adobe RGB (1998) compatible",
            ),
            Self::ProPhotoRgb => rgb_profile(D50, PROPHOTO, &ToneCurve::new(1.8), "ProPhoto RGB"),
        }
    }

    pub(crate) fn icc(self) -> Vec<u8> {
        self.profile().icc().unwrap_or_default()
    }

    /// EXIF ColorSpace: 1 for sRGB, "uncalibrated" (the embedded profile says) otherwise.
    pub(crate) fn exif_code(self) -> u16 {
        if self == Self::Srgb { 1 } else { 0xFFFF }
    }
}

fn xy_y((x, y): (f64, f64)) -> CIExyY {
    CIExyY { x, y, Y: 1.0 }
}

fn rgb_profile(white: (f64, f64), primaries: [(f64, f64); 3], curve: &ToneCurve, name: &str) -> Profile {
    let triple =
        CIExyYTRIPLE { Red: xy_y(primaries[0]), Green: xy_y(primaries[1]), Blue: xy_y(primaries[2]) };
    let mut p = Profile::new_rgb(&xy_y(white), &triple, &[curve, curve, curve])
        .unwrap_or_else(|_| Profile::new_srgb());
    let mut desc = MLU::new(1);
    desc.set_text_ascii(name, Locale::none());
    p.write_tag(TagSignature::ProfileDescriptionTag, Tag::MLU(&desc));
    p
}

/// LibRaw's linear 16-bit ProPhoto render (its white is D65, which an ICC transform
/// maps to white either way).
pub(crate) fn linear_prophoto() -> Profile {
    rgb_profile(D50, PROPHOTO, &ToneCurve::new(1.0), "Linear ProPhoto RGB")
}

/// Convert decoded pixels into `target`, as 8-bit or 16-bit RGB.
pub(crate) fn to_target(
    img: DynamicImage,
    source: SourceSpace,
    target: ColorSpace,
    sixteen: bool,
) -> DynamicImage {
    let src = match source {
        SourceSpace::Srgb => None,
        SourceSpace::LinearProPhoto => Some(linear_prophoto()),
        SourceSpace::Icc(bytes) => match Profile::new_icc(&bytes) {
            Ok(p) if p.color_space() == ColorSpaceSignature::GrayData && !img.color().has_color() => {
                if let Some(out) = gray_to(&img, &p, target, sixteen) {
                    return out;
                }
                None
            }
            Ok(p) if p.color_space() == ColorSpaceSignature::RgbData && !crate::color::is_srgb(&p) => Some(p),
            Ok(_) => None,
            Err(_) => {
                log::warn!("ignoring unreadable ICC profile");
                None
            }
        },
    };
    if src.is_none() && target == ColorSpace::Srgb {
        return if sixteen {
            DynamicImage::ImageRgb16(img.into_rgb16())
        } else {
            DynamicImage::ImageRgb8(img.into_rgb8())
        };
    }
    let src = src.unwrap_or_else(Profile::new_srgb);
    let dst = target.profile();
    if sixteen {
        let mut rgb = img.into_rgb16();
        transform_parallel(&src, &dst, PixelFormat::RGB_16, bytemuck::cast_slice_mut(rgb.as_mut()));
        DynamicImage::ImageRgb16(rgb)
    } else {
        let mut rgb = img.into_rgb8();
        transform_parallel(&src, &dst, PixelFormat::RGB_8, rgb.as_mut());
        DynamicImage::ImageRgb8(rgb)
    }
}

/// Convert interleaved RGB bytes in place, in parallel row bands (a single-threaded
/// 16-bit transform of a 24 MP photo takes ~9 s).
pub(crate) fn transform_parallel(src: &Profile, dst: &Profile, format: PixelFormat, bytes: &mut [u8]) {
    let t: Transform<u8, u8, GlobalContext, DisallowCache> = match Transform::new_flags_context(
        GlobalContext::new(),
        src,
        format,
        dst,
        format,
        Intent::Perceptual,
        Flags::NO_CACHE,
    ) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("colour conversion failed: {e}");
            return;
        }
    };
    let pixel = format.bytes_per_pixel();
    // 64K pixels per task.
    bytes.par_chunks_mut(pixel * 65_536).for_each(|chunk| t.transform_in_place(chunk));
}

/// A gray image with a gray profile, straight into the RGB target.
fn gray_to(img: &DynamicImage, src: &Profile, target: ColorSpace, sixteen: bool) -> Option<DynamicImage> {
    let dst = target.profile();
    let (w, h) = (img.width(), img.height());
    if sixteen {
        let gray: ImageBuffer<Luma<u16>, Vec<u16>> = img.to_luma16();
        let t = Transform::<u8, u8>::new(
            src,
            PixelFormat::GRAY_16,
            &dst,
            PixelFormat::RGB_16,
            Intent::Perceptual,
        )
        .ok()?;
        let mut out: ImageBuffer<Rgb<u16>, Vec<u16>> = ImageBuffer::new(w, h);
        t.transform_pixels(bytemuck::cast_slice(gray.as_raw()), bytemuck::cast_slice_mut(out.as_mut()));
        Some(DynamicImage::ImageRgb16(out))
    } else {
        let gray = img.to_luma8();
        let t =
            Transform::<u8, u8>::new(src, PixelFormat::GRAY_8, &dst, PixelFormat::RGB_8, Intent::Perceptual)
                .ok()?;
        let mut out = image::RgbImage::new(w, h);
        t.transform_pixels(gray.as_raw(), out.as_mut());
        Some(DynamicImage::ImageRgb8(out))
    }
}

/// Convert sRGB pixels (a graphic watermark) into `target`, in place.
pub(crate) fn srgb_to(rgb: &mut [u8], target: ColorSpace) {
    if target == ColorSpace::Srgb {
        return;
    }
    if let Ok(t) = Transform::<u8, u8>::new(
        &Profile::new_srgb(),
        PixelFormat::RGB_8,
        &target.profile(),
        PixelFormat::RGB_8,
        Intent::Perceptual,
    ) {
        t.transform_in_place(rgb);
    }
}
