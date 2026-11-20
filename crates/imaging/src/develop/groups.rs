//! Settings groups: the parts of [`DevelopSettings`] a preset, Copy… or a Lightroom
//! preset can carry (LrC's checkboxes in New Preset and Copy Settings).
//!
//! Every field belongs to exactly one group, so copying all groups copies everything.

use serde::{Deserialize, Serialize};

use super::settings::{DevelopSettings, WbPreset};
use super::transform::Transform;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SettingsGroup {
    Treatment,
    WhiteBalance,
    BasicTone,
    Presence,
    Profile,
    ToneCurve,
    Hsl,
    BwMix,
    ColorGrading,
    Sharpening,
    NoiseReduction,
    Vignette,
    Grain,
    Glow,
    Transform,
    Crop,
    Masks,
    Spots,
    Dust,
}

impl SettingsGroup {
    /// In panel order.
    pub const ALL: [SettingsGroup; 19] = [
        Self::Treatment,
        Self::WhiteBalance,
        Self::BasicTone,
        Self::Presence,
        Self::Profile,
        Self::ToneCurve,
        Self::Hsl,
        Self::BwMix,
        Self::ColorGrading,
        Self::Sharpening,
        Self::NoiseReduction,
        Self::Vignette,
        Self::Grain,
        Self::Glow,
        Self::Transform,
        Self::Crop,
        Self::Masks,
        Self::Spots,
        Self::Dust,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Treatment => "Treatment (Color / B&W)",
            Self::WhiteBalance => "White Balance",
            Self::BasicTone => "Basic Tone",
            Self::Presence => "Presence",
            Self::Profile => "Profile (LUT)",
            Self::ToneCurve => "Tone Curve",
            Self::Hsl => "HSL / Color",
            Self::BwMix => "Black & White Mix",
            Self::ColorGrading => "Color Grading",
            Self::Sharpening => "Sharpening",
            Self::NoiseReduction => "Noise Reduction",
            Self::Vignette => "Post-Crop Vignetting",
            Self::Grain => "Grain",
            Self::Glow => "Glow",
            Self::Transform => "Transform",
            Self::Crop => "Crop",
            Self::Masks => "Masks",
            Self::Spots => "Remove (Spots)",
            Self::Dust => "Dust & Scratches",
        }
    }

    /// The panel a group sits under (for headings in the dialogs).
    pub fn panel(self) -> &'static str {
        match self {
            Self::Treatment | Self::WhiteBalance | Self::BasicTone | Self::Presence | Self::Profile => "Basic",
            Self::ToneCurve | Self::Hsl | Self::BwMix | Self::ColorGrading => "Color",
            Self::Sharpening | Self::NoiseReduction => "Detail",
            Self::Vignette | Self::Grain | Self::Glow => "Effects",
            Self::Transform | Self::Crop => "Geometry",
            Self::Masks | Self::Spots | Self::Dust => "Local",
        }
    }

    /// Belongs to one photo (its framing or its blemishes): left out unless asked for, as
    /// LrC leaves them unticked.
    pub fn per_photo(self) -> bool {
        matches!(self, Self::Transform | Self::Crop | Self::Masks | Self::Spots | Self::Dust)
    }
}

/// Set the fields of `groups` in `to` from `from`; the rest of `to` stays.
pub fn copy_groups(from: &DevelopSettings, to: &mut DevelopSettings, groups: &[SettingsGroup]) {
    for &g in groups {
        copy_group(from, to, g);
    }
}

fn copy_group(f: &DevelopSettings, t: &mut DevelopSettings, g: SettingsGroup) {
    use SettingsGroup as G;
    match g {
        G::Treatment => t.treatment = f.treatment,
        G::WhiteBalance => (t.wb, t.temp, t.tint) = (f.wb, f.temp, f.tint),
        G::BasicTone => {
            (t.exposure, t.contrast, t.highlights) = (f.exposure, f.contrast, f.highlights);
            (t.shadows, t.whites, t.blacks) = (f.shadows, f.whites, f.blacks);
        }
        G::Presence => {
            (t.texture, t.clarity, t.dehaze) = (f.texture, f.clarity, f.dehaze);
            (t.vibrance, t.saturation) = (f.vibrance, f.saturation);
        }
        G::Profile => t.profile = f.profile.clone(),
        G::ToneCurve => t.curve = f.curve.clone(),
        G::Hsl => t.hsl = f.hsl.clone(),
        G::BwMix => t.bw_mix = f.bw_mix,
        G::ColorGrading => t.grading = f.grading.clone(),
        G::Sharpening => {
            let (d, s) = (&mut t.detail, &f.detail);
            (d.sharpen_amount, d.sharpen_radius) = (s.sharpen_amount, s.sharpen_radius);
            (d.sharpen_detail, d.sharpen_masking) = (s.sharpen_detail, s.sharpen_masking);
        }
        G::NoiseReduction => {
            let (d, s) = (&mut t.detail, &f.detail);
            (d.luminance, d.luminance_detail, d.luminance_contrast) =
                (s.luminance, s.luminance_detail, s.luminance_contrast);
            (d.color, d.color_detail, d.color_smoothness) = (s.color, s.color_detail, s.color_smoothness);
        }
        G::Vignette => {
            let (e, s) = (&mut t.effects, &f.effects);
            (e.vignette_style, e.vignette_amount, e.vignette_midpoint) =
                (s.vignette_style, s.vignette_amount, s.vignette_midpoint);
            (e.vignette_roundness, e.vignette_feather, e.vignette_highlights) =
                (s.vignette_roundness, s.vignette_feather, s.vignette_highlights);
        }
        G::Grain => {
            let (e, s) = (&mut t.effects, &f.effects);
            (e.grain_amount, e.grain_size, e.grain_roughness) = (s.grain_amount, s.grain_size, s.grain_roughness);
        }
        G::Glow => {
            let (e, s) = (&mut t.effects, &f.effects);
            (e.glow_amount, e.glow_range, e.glow_spread, e.glow_warmth) =
                (s.glow_amount, s.glow_range, s.glow_spread, s.glow_warmth);
        }
        G::Transform => {
            // Everything of the Transform panel but the crop.
            let crop = t.transform.crop;
            t.transform = Transform { crop, ..f.transform.clone() };
        }
        G::Crop => t.transform.crop = f.transform.crop,
        G::Masks => t.masks = f.masks.clone(),
        G::Spots => t.spots = f.spots.clone(),
        G::Dust => t.dust = f.dust.clone(),
    }
}

/// The groups in which `s` differs from a new photo's settings (what New Preset and
/// Copy… tick to begin with). Temp/Tint only count with a Custom white balance.
pub fn changed_groups(s: &DevelopSettings, raw: bool) -> Vec<SettingsGroup> {
    let base = DevelopSettings::default_for(raw);
    SettingsGroup::ALL
        .into_iter()
        .filter(|&g| match g {
            SettingsGroup::WhiteBalance => {
                s.wb != base.wb || (s.wb == WbPreset::Custom && (s.temp, s.tint) != (base.temp, base.tint))
            }
            _ => {
                let mut t = base.clone();
                copy_group(s, &mut t, g);
                t != base
            }
        })
        .collect()
}

/// Settings that differ from the default in every field (tests: no field is left out of
/// a group).
#[cfg(test)]
pub(super) fn every_field_changed() -> DevelopSettings {
    use super::curve::ToneCurve;
    use super::grading::ColorGrading;
    use super::hsl::Hsl;
    use super::transform::Crop;
    use super::detail::Detail;
    use super::effects::{Effects, VignetteStyle};
    use super::grading::Wheel;
    use super::settings::Treatment;
    use super::transform::Upright;
    let wheel = Wheel { hue: 30.0, saturation: 20.0, luminance: 5.0 };
    DevelopSettings {
        process_version: super::settings::PROCESS_VERSION,
        treatment: Treatment::BlackAndWhite,
        wb: WbPreset::Custom,
        temp: 5000.0,
        tint: 7.0,
        exposure: 0.5,
        contrast: 10.0,
        highlights: -20.0,
        shadows: 20.0,
        whites: 5.0,
        blacks: -5.0,
        texture: 3.0,
        clarity: 4.0,
        dehaze: 6.0,
        vibrance: 7.0,
        saturation: -8.0,
        profile: Some(super::lut::LutProfile { name: "Test".into(), hash: "ab".into(), amount: 80.0 }),
        curve: ToneCurve {
            highlights: 10.0,
            lights: 5.0,
            darks: -5.0,
            shadows: -10.0,
            splits: [20.0, 50.0, 80.0],
            rgb: vec![[0.0, 0.1], [1.0, 0.9]],
            red: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            green: vec![[0.0, 0.0], [0.5, 0.4], [1.0, 1.0]],
            blue: vec![[0.0, 0.05], [1.0, 1.0]],
        },
        hsl: Hsl { hue: [1.0; 8], saturation: [2.0; 8], luminance: [3.0; 8] },
        bw_mix: [4.0; 8],
        grading: ColorGrading {
            shadows: wheel,
            midtones: wheel,
            highlights: wheel,
            global: wheel,
            blending: 40.0,
            balance: 10.0,
        },
        detail: Detail {
            sharpen_amount: 50.0,
            sharpen_radius: 1.5,
            sharpen_detail: 30.0,
            sharpen_masking: 10.0,
            luminance: 20.0,
            luminance_detail: 40.0,
            luminance_contrast: 10.0,
            color: 30.0,
            color_detail: 40.0,
            color_smoothness: 60.0,
        },
        effects: Effects {
            vignette_style: VignetteStyle::PaintOverlay,
            vignette_amount: -20.0,
            vignette_midpoint: 40.0,
            vignette_roundness: 10.0,
            vignette_feather: 60.0,
            vignette_highlights: 10.0,
            grain_amount: 20.0,
            grain_size: 30.0,
            grain_roughness: 40.0,
            glow_amount: 10.0,
            glow_range: 40.0,
            glow_spread: 60.0,
            glow_warmth: 5.0,
        },
        transform: Transform {
            upright: Upright::Auto,
            upright_angles: [1.0, 2.0, 3.0],
            guides: vec![[0.1, 0.1, 0.2, 0.9]],
            constrain_crop: true,
            vertical: 5.0,
            horizontal: 6.0,
            rotate: 1.0,
            aspect: 2.0,
            scale: 105.0,
            x_offset: 3.0,
            y_offset: 4.0,
            crop: Crop { left: 0.1, top: 0.1, right: 0.9, bottom: 0.9, angle: 2.0 },
        },
        masks: vec![super::masks::Mask::default()],
        spots: vec![super::heal::Spot::default()],
        dust: Some(super::dust::DustLayer::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::transform::Crop;
    use super::*;

    #[test]
    fn copying_every_group_copies_every_field() {
        let all = every_field_changed();
        for raw in [false, true] {
            let mut t = DevelopSettings::default_for(raw);
            copy_groups(&all, &mut t, &SettingsGroup::ALL);
            assert_eq!(t, all);
        }
    }

    #[test]
    fn each_group_changes_only_itself() {
        let all = every_field_changed();
        for g in SettingsGroup::ALL {
            let mut t = DevelopSettings::default();
            copy_groups(&all, &mut t, &[g]);
            assert_eq!(changed_groups(&t, false), vec![g], "{g:?}");
        }
        assert!(changed_groups(&DevelopSettings::default_for(true), true).is_empty());
        // A Temp left over from Custom doesn't count once As Shot is back.
        let s = DevelopSettings { temp: 4000.0, ..DevelopSettings::default() };
        assert!(changed_groups(&s, false).is_empty());
    }

    #[test]
    fn transform_and_crop_go_separately() {
        let all = every_field_changed();
        let mut t = DevelopSettings::default();
        copy_groups(&all, &mut t, &[SettingsGroup::Transform]);
        assert_eq!(t.transform.crop, Crop::default());
        assert_eq!(t.transform.vertical, 5.0);
        copy_groups(&all, &mut t, &[SettingsGroup::Crop]);
        assert_eq!(t.transform, all.transform);
    }
}
