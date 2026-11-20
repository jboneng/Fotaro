//! A lens's corrections for one photo's focal length, aperture and distance, interpolated
//! between its calibration points the way Lensfun does:
//! - the calibration set: the one made on the largest sensor the photo's still covers
//!   (photo crop / calibration crop ≥ 0.96, the smallest such ratio);
//! - distortion and TCA: Hermite splines through the neighbouring focal lengths, with the
//!   terms that fall off as 1/f multiplied by the focal length first (and divided after);
//! - vignetting: inverse distance weighting (power 3.5) over focal length (normalised to the
//!   zoom range), 4 / aperture and 0.1 / distance; nothing when every point is far.

use crate::{Calibration, Distortion, Lens, LensProfile, Tca, Vignetting};

/// Lensfun's Hermite step between `y2` and `y3` (`t` 0..1), with `y1`, `y4` the outer
/// neighbours when there are any.
fn hermite(y1: Option<f64>, y2: f64, y3: f64, y4: Option<f64>, t: f64) -> f64 {
    let tg2 = y1.map_or(y3 - y2, |y1| (y3 - y1) * 0.5);
    let tg3 = y4.map_or(y3 - y2, |y4| (y4 - y2) * 0.5);
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * y2 + (t3 - 2.0 * t2 + t) * tg2 + (-2.0 * t3 + 3.0 * t2) * y3 + (t3 - t2) * tg3
}

/// Interpolate terms given at focal lengths to `focal`. `scaled[i]`: term i falls off as
/// 1/f (multiplied by the focal length before interpolating).
fn by_focal(points: &[(f32, Vec<f64>)], focal: f32, scaled: &[bool]) -> Option<Vec<f64>> {
    if let Some((_, t)) = points.iter().find(|(f, _)| *f == focal) {
        return Some(t.clone());
    }
    let mut below: Vec<&(f32, Vec<f64>)> = points.iter().filter(|(f, _)| *f < focal).collect();
    let mut above: Vec<&(f32, Vec<f64>)> = points.iter().filter(|(f, _)| *f > focal).collect();
    below.sort_by(|a, b| b.0.total_cmp(&a.0));
    above.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (p1, p2) = match (below.first(), above.first()) {
        (Some(a), Some(b)) => (*a, *b),
        // Outside the calibrated range: the nearest point.
        (Some(a), None) => return Some(a.1.clone()),
        (None, Some(b)) => return Some(b.1.clone()),
        (None, None) => return None,
    };
    let (p0, p3) = (below.get(1).copied(), above.get(1).copied());
    let t = f64::from((focal - p1.0) / (p2.0 - p1.0));
    let n = p1.1.len();
    Some(
        (0..n)
            .map(|i| {
                let s = |f: f32| if scaled.get(i).copied().unwrap_or(false) { f64::from(f) } else { 1.0 };
                let v = |p: &(f32, Vec<f64>)| p.1[i] * s(p.0);
                hermite(p0.map(v), v(p1), v(p2), p3.map(v), t) / s(focal)
            })
            .collect(),
    )
}

fn distortion_terms(d: &Distortion) -> (u8, Vec<f64>) {
    match *d {
        Distortion::Poly3 { k1 } => (0, vec![f64::from(k1)]),
        Distortion::Poly5 { k1, k2 } => (1, vec![f64::from(k1), f64::from(k2)]),
        Distortion::Ptlens { a, b, c } => (2, vec![f64::from(a), f64::from(b), f64::from(c)]),
    }
}

fn distortion_from(kind: u8, t: &[f64]) -> Distortion {
    let f = |i: usize| t[i] as f32;
    match kind {
        0 => Distortion::Poly3 { k1: f(0) },
        1 => Distortion::Poly5 { k1: f(0), k2: f(1) },
        _ => Distortion::Ptlens { a: f(0), b: f(1), c: f(2) },
    }
}

/// Lensfun's term order: [vr, vb, cr, cb, br, bb] (linear: [kr, kb]); the first two stay
/// as they are, the rest fall off as 1/f.
fn tca_terms(t: &Tca) -> (u8, Vec<f64>) {
    match *t {
        Tca::Linear { kr, kb } => (0, vec![f64::from(kr), f64::from(kb)]),
        Tca::Poly3 { vr, cr, br, vb, cb, bb } => (1, [vr, vb, cr, cb, br, bb].map(f64::from).to_vec()),
    }
}

fn tca_from(kind: u8, t: &[f64]) -> Tca {
    let f = |i: usize| t[i] as f32;
    match kind {
        0 => Tca::Linear { kr: f(0), kb: f(1) },
        _ => Tca::Poly3 { vr: f(0), vb: f(1), cr: f(2), cb: f(3), br: f(4), bb: f(5) },
    }
}

/// The calibration set to use for a photo of crop factor `crop`, of those having `has`.
fn calibration(lens: &Lens, crop: f32, has: impl Fn(&Calibration) -> bool) -> Option<&Calibration> {
    lens.calibrations
        .iter()
        .filter(|c| has(c))
        .map(|c| (crop / c.crop.max(0.01), c))
        .filter(|(r, _)| *r >= 0.96)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, c)| c)
}

fn vignetting(lens: &Lens, cal: &Calibration, focal: f32, aperture: f32, distance: f32) -> Option<Vignetting> {
    let span = lens.max_focal - lens.min_focal;
    let norm = |f: f32| if span != 0.0 { (f - lens.min_focal) / span } else { f - lens.min_focal };
    let dist = |v: &crate::VignettingAt| {
        let df = norm(v.focal) - norm(focal);
        let da = 4.0 / v.aperture.max(0.1) - 4.0 / aperture.max(0.1);
        let dd = 0.1 / v.distance.max(0.01) - 0.1 / distance.max(0.01);
        (df * df + da * da + dd * dd).sqrt()
    };
    let (mut total, mut sum, mut nearest) = (0.0f64, [0.0f64; 3], f32::MAX);
    for v in &cal.vignetting {
        let d = dist(v);
        if d < 1e-4 {
            return Some(v.model);
        }
        nearest = nearest.min(d);
        let w = 1.0 / f64::from(d).powf(3.5);
        for (s, k) in sum.iter_mut().zip([v.model.k1, v.model.k2, v.model.k3]) {
            *s += w * f64::from(k);
        }
        total += w;
    }
    (nearest <= 1.0 && total > 0.0).then(|| Vignetting {
        k1: (sum[0] / total) as f32,
        k2: (sum[1] / total) as f32,
        k3: (sum[2] / total) as f32,
    })
}

/// `lens`'s corrections for a photo of crop factor `crop` taken at `focal` (mm),
/// `aperture` (f-number) and `distance` (m); `camera` is shown only.
pub fn resolve(lens: &Lens, camera: &str, crop: f32, focal: f32, aperture: f32, distance: f32) -> LensProfile {
    let focal = focal.clamp(lens.min_focal.min(lens.max_focal), lens.max_focal.max(lens.min_focal));
    let dcal = calibration(lens, crop, |c| !c.distortion.is_empty());
    let distortion = dcal.and_then(|c| {
        let kind = distortion_terms(&c.distortion[0].model).0;
        let points: Vec<(f32, Vec<f64>)> = c
            .distortion
            .iter()
            .map(|d| (d.focal, distortion_terms(&d.model)))
            .filter(|(_, (k, _))| *k == kind)
            .map(|(f, (_, t))| (f, t))
            .collect();
        by_focal(&points, focal, &[true; 3]).map(|t| distortion_from(kind, &t))
    });
    let tca = calibration(lens, crop, |c| !c.tca.is_empty()).and_then(|c| {
        let kind = tca_terms(&c.tca[0].model).0;
        let points: Vec<(f32, Vec<f64>)> = c
            .tca
            .iter()
            .map(|d| (d.focal, tca_terms(&d.model)))
            .filter(|(_, (k, _))| *k == kind)
            .map(|(f, (_, t))| (f, t))
            .collect();
        by_focal(&points, focal, &[false, false, true, true, true, true]).map(|t| tca_from(kind, &t))
    });
    let vcal = calibration(lens, crop, |c| !c.vignetting.is_empty());
    let vig = vcal.and_then(|c| vignetting(lens, c, focal, aperture, distance));
    // The calibration's sensor (the distortion's, else any used).
    let cal = dcal.or(vcal).or_else(|| lens.calibrations.first());
    LensProfile {
        maker: lens.maker.clone(),
        lens: lens.model.clone(),
        camera: camera.to_string(),
        focal,
        aperture,
        distance,
        crop,
        calib_crop: cal.map_or(lens.crop, |c| c.crop),
        calib_aspect: cal.map_or(lens.aspect, |c| c.aspect),
        center: lens.center,
        distortion,
        tca,
        vignetting: vig,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::FIXTURE;

    #[test]
    fn exact_points_are_taken_as_they_are() {
        let db = crate::Database::from_xml(FIXTURE).unwrap();
        let z = &db.lenses[0];
        let p = resolve(z, "Nikon D750", 1.0, 24.0, 2.8, 1000.0);
        assert_eq!(p.distortion, Some(Distortion::Ptlens { a: 0.01436, b: -0.06611, c: 0.06436 }));
        assert_eq!(p.vignetting, Some(Vignetting { k1: -0.2153778, k2: -0.9491463, k3: 0.476_064 }));
        let Some(Tca::Poly3 { vr, vb, .. }) = p.tca else { panic!() };
        assert!((vr - 1.0006434).abs() < 1e-6 && (vb - 0.9998886).abs() < 1e-6);
        assert_eq!((p.calib_crop, p.calib_aspect), (1.0, 1.5));
    }

    #[test]
    fn between_points_the_terms_interpolate() {
        let db = crate::Database::from_xml(FIXTURE).unwrap();
        let z = &db.lenses[0];
        let Some(Distortion::Ptlens { a, .. }) = resolve(z, "", 1.0, 30.0, 2.8, 1000.0).distortion else { panic!() };
        // Between 24 (a·f = 0.345) and 35 (0.366), divided by 30.
        assert!(a > 0.0099 && a < 0.0122, "{a}");
        // Beyond the range: clamped to the lens's focal lengths.
        assert_eq!(resolve(z, "", 1.0, 10.0, 2.8, 1000.0).focal, 24.0);
        // Vignetting between apertures lands between the two.
        let v = resolve(z, "", 1.0, 24.0, 4.0, 1000.0).vignetting.unwrap();
        assert!(v.k1 < -0.2153 && v.k1 > -0.3053, "{v:?}");
    }

    #[test]
    fn a_smaller_sensors_calibration_is_not_used_on_full_frame() {
        let db = crate::Database::from_xml(FIXTURE).unwrap();
        let dx = &db.lenses[2];
        assert!(resolve(dx, "", 1.0, 35.0, 1.8, 1000.0).distortion.is_none());
        assert!(resolve(dx, "", 1.523, 35.0, 1.8, 1000.0).distortion.is_some());
    }
}
