//! Finding a photo's camera and lens in the database.
//!
//! Lens names in photos are often only numbers (LibRaw describes Nikon lenses as
//! "60.0 mm f/2.8"), so besides Lensfun's word matching ([`fuzzy_score`]) the focal range
//! and widest aperture read from the names ([`parse_name`]) must agree.

use crate::{Camera, Database, LensType};

/// Focal range and widest aperture(s) read from a lens name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NameParams {
    pub min_focal: f32,
    pub max_focal: f32,
    pub min_aperture: f32,
    pub max_aperture: f32,
}

/// Read "24-70mm f/2.8", "18-55 mm f/3.5-5.6", "50mm F1.4", "60.0 mm f/2.8", "1:2.8 50mm".
pub fn parse_name(name: &str) -> Option<NameParams> {
    let s = name.to_lowercase();
    let b = s.as_bytes();
    // Numbers with where they are.
    let mut nums: Vec<(usize, usize, f32)> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if let Ok(v) = s[start..i].trim_end_matches('.').parse::<f32>() {
                nums.push((start, i, v));
            }
        } else {
            i += 1;
        }
    }
    let after = |end: usize| s[end..].trim_start();
    let before = |start: usize| s[..start].trim_end();
    // Focal: a number (or range) followed by "mm".
    let mut focal = None;
    for (k, &(_, end, v)) in nums.iter().enumerate() {
        if after(end).starts_with("mm") {
            let lo = (k > 0 && before(nums[k].0).ends_with('-')).then(|| nums[k - 1].2);
            focal = Some(match lo {
                Some(lo) => (lo, v),
                None => (v, v),
            });
            break;
        }
    }
    // Aperture: after "f/", "f", "1:" (a range "2.8-4" too).
    let mut aperture = None;
    for (k, &(start, end, v)) in nums.iter().enumerate() {
        let pre = before(start);
        let is_ap = pre.ends_with("f/") || pre.ends_with('f') || pre.ends_with("1:") || pre.ends_with("f /");
        if is_ap && !after(end).starts_with("mm") {
            let hi = nums.get(k + 1).filter(|n| s[end..n.0].trim() == "-").map(|n| n.2);
            aperture = Some((v, hi.unwrap_or(v)));
            break;
        }
    }
    let (min_focal, max_focal) = focal?;
    let (min_aperture, max_aperture) = aperture.unwrap_or((0.0, 0.0));
    Some(NameParams { min_focal, max_focal, min_aperture, max_aperture })
}

/// Lensfun's word split: digits (with points; a trailing ".0" dropped), runs of
/// punctuation, runs of other characters; a lone punctuation mark or "f" is skipped (but
/// not "+" or "*"); case folded; sorted.
fn words(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }
        let start = i;
        i += 1;
        let c = chars[start];
        let mut strip = 0;
        if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            if i - start >= 2 && chars[i - 2] == '.' && chars[i - 1] == '0' {
                strip = 2;
            }
        } else if c.is_ascii_punctuation() {
            while i < chars.len() && chars[i].is_ascii_punctuation() {
                i += 1;
            }
        } else {
            while i < chars.len()
                && !chars[i].is_whitespace()
                && !chars[i].is_ascii_digit()
                && !chars[i].is_ascii_punctuation()
            {
                i += 1;
            }
        }
        if i - start == 1 && (c.is_ascii_punctuation() || c.eq_ignore_ascii_case(&'f')) && c != '*' && c != '+' {
            continue;
        }
        out.push(chars[start..i - strip].iter().collect::<String>().to_lowercase());
    }
    out.sort();
    out
}

/// Lensfun's fuzzy comparison of `pattern` (from the photo) and `name` (the database's):
/// 0..100, 200 × matched words / all words.
pub fn fuzzy_score(pattern: &str, name: &str) -> u32 {
    let (p, m) = (words(pattern), words(name));
    if p.is_empty() || m.is_empty() {
        return 0;
    }
    let mut mi = 0usize;
    let mut score = 0u32;
    // Both lists are sorted: walk the database's words once per photo word; a word not
    // found leaves the position where it was.
    for pw in &p {
        let old = mi;
        let mut found = false;
        while mi < m.len() {
            match pw.as_str().cmp(m[mi].as_str()) {
                std::cmp::Ordering::Equal => {
                    found = true;
                    break;
                }
                std::cmp::Ordering::Less => break,
                std::cmp::Ordering::Greater => mi += 1,
            }
        }
        if found {
            score += 1;
            mi += 1;
        } else {
            mi = old;
        }
    }
    score * 200 / (p.len() + m.len()) as u32
}

/// What a photo says about its camera and lens.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoLens {
    pub camera_maker: String,
    pub camera_model: String,
    pub lens: String,
    pub focal: Option<f32>,
}

/// A lens candidate: its index in [`Database::lenses`] and how well it fits (higher
/// better; [`LensMatch::sure`] when it can be taken without asking).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LensMatch {
    pub index: usize,
    pub score: i32,
    pub sure: bool,
}

fn alnum_words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

impl Database {
    /// The photo's camera: same maker (one name contains the other's first word), and the
    /// same model words once the maker's are left out ("NIKON D750" = "Nikon D750" = "D750").
    pub fn find_camera(&self, maker: &str, model: &str) -> Option<&Camera> {
        let pm = alnum_words(maker);
        let strip = |s: &str, maker_words: &[String]| -> Vec<String> {
            let mut w: Vec<String> =
                alnum_words(s).into_iter().filter(|x| !maker_words.contains(x) && !pm.contains(x)).collect();
            w.sort();
            w
        };
        self.cameras.iter().find(|c| {
            let cm = alnum_words(&c.maker);
            let same_maker = cm.first().is_some_and(|f| pm.contains(f)) || pm.first().is_some_and(|f| cm.contains(f));
            if !same_maker {
                return false;
            }
            let want = strip(model, &cm);
            !want.is_empty()
                && std::iter::once(&c.model).chain(&c.names).any(|n| strip(n, &cm) == want)
        })
    }

    /// Lenses for the photo, best first: on the camera's mount (or one it takes), not
    /// calibrated on a smaller sensor, rectilinear and with corrections; scored by name and
    /// by focal range and aperture.
    pub fn find_lenses(&self, photo: &PhotoLens) -> Vec<LensMatch> {
        let camera = self.find_camera(&photo.camera_maker, &photo.camera_model);
        let mounts = camera.map(|c| self.compatible_mounts(&c.mount));
        let params = parse_name(&photo.lens);
        let maker_words = alnum_words(&photo.camera_maker);
        let mut out: Vec<LensMatch> = self
            .lenses
            .iter()
            .enumerate()
            .filter(|(_, l)| l.kind == LensType::Rectilinear && l.has_corrections())
            .filter(|(_, l)| mounts.as_ref().is_none_or(|m| l.mounts.iter().any(|x| m.contains(x))))
            .filter(|(_, l)| camera.is_none_or(|c| l.crop <= c.crop * 1.01))
            .filter_map(|(i, l)| {
                let name = std::iter::once(&l.model).chain(&l.names).map(|n| fuzzy_score(&photo.lens, n)).max();
                let mut score = name.unwrap_or(0) as i32;
                let mut numbers = false;
                if let Some(p) = params {
                    let same_range =
                        (p.min_focal - l.min_focal).abs() < 0.6 && (p.max_focal - l.max_focal).abs() < 0.6;
                    if !same_range {
                        return None;
                    }
                    score += 40;
                    if p.min_aperture > 0.0 && l.min_aperture > 0.0 {
                        if (p.min_aperture - l.min_aperture).abs() < 0.06 {
                            score += 25;
                            numbers = true;
                        } else {
                            return None;
                        }
                    }
                } else if let Some(f) = photo.focal
                    && (f < l.min_focal - 0.5 || f > l.max_focal + 0.5)
                {
                    return None;
                }
                let lens_maker = alnum_words(&l.maker);
                if lens_maker.iter().any(|w| maker_words.contains(w) || photo.lens.to_lowercase().contains(w.as_str())) {
                    score += 10;
                }
                let sure = name.unwrap_or(0) >= 60 || numbers;
                Some(LensMatch { index: i, score, sure })
            })
            .collect();
        out.sort_by(|a, b| b.score.cmp(&a.score).then(a.index.cmp(&b.index)));
        // Sure only when it stands out from the next one.
        if let [first, second, ..] = out.as_mut_slice()
            && first.score - second.score < 5
            && first.sure
            && second.sure
        {
            first.sure = false;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::FIXTURE;

    #[test]
    fn names_give_focal_and_aperture() {
        let p = |s| parse_name(s).map(|p| (p.min_focal, p.max_focal, p.min_aperture, p.max_aperture));
        assert_eq!(p("Nikon AF-S Zoom-Nikkor 24-70mm f/2.8G ED"), Some((24.0, 70.0, 2.8, 2.8)));
        assert_eq!(p("60.0 mm f/2.8"), Some((60.0, 60.0, 2.8, 2.8)));
        assert_eq!(p("24.0-120.0 mm f/4.0"), Some((24.0, 120.0, 4.0, 4.0)));
        assert_eq!(p("18-55mm f/3.5-5.6 VR"), Some((18.0, 55.0, 3.5, 5.6)));
        assert_eq!(p("Sigma 50mm F1.4 DG HSM | A"), Some((50.0, 50.0, 1.4, 1.4)));
        assert_eq!(p("Super-Takumar"), None);
    }

    #[test]
    fn fuzzy_scores_like_lensfun() {
        assert_eq!(fuzzy_score("Nikon AF-S Nikkor 50mm f/1.8G", "Nikon AF-S Nikkor 50mm f/1.8G"), 100);
        let a = fuzzy_score("AF-S Micro Nikkor 60mm f/2.8G ED", "Nikon AF Micro-Nikkor 60mm f/2.8D");
        let b = fuzzy_score("AF-S Micro Nikkor 60mm f/2.8G ED", "Laowa 60mm f/2.8 2X Ultra-Macro");
        assert!(a > b && a > 0, "{a} {b}");
        // "60.0" counts as "60".
        assert!(fuzzy_score("60.0 mm", "60mm") > 0);
    }

    #[test]
    fn cameras_and_lenses_are_found() {
        let db = crate::Database::from_xml(FIXTURE).unwrap();
        for (maker, model) in [("NIKON CORPORATION", "NIKON D750"), ("Nikon", "D750"), ("Nikon Corporation", "Nikon D750")] {
            assert_eq!(db.find_camera(maker, model).map(|c| c.model.as_str()), Some("Nikon D750"), "{maker} {model}");
        }
        assert!(db.find_camera("Canon", "D750").is_none());
        assert!(db.find_camera("Nikon", "D75").is_none());
        let photo = |lens: &str| PhotoLens {
            camera_maker: "Nikon".into(),
            camera_model: "D750".into(),
            lens: lens.into(),
            focal: None,
        };
        // LibRaw's generic name: the numbers find it.
        let m = db.find_lenses(&photo("24.0-70.0 mm f/2.8"));
        assert_eq!(db.lenses[m[0].index].model, "Nikon AF-S Zoom-Nikkor 24-70mm f/2.8G ED");
        assert!(m[0].sure);
        // A full name.
        let m = db.find_lenses(&photo("AF-S Zoom-Nikkor 24-70mm f/2.8G ED"));
        assert!(m[0].sure && m.len() == 1);
        // Another focal range: nothing.
        assert!(db.find_lenses(&photo("35.0 mm f/1.8")).is_empty(), "the DX 35 is for a smaller sensor");
        let dx = PhotoLens { camera_model: "D7000".into(), ..photo("35.0 mm f/1.8") };
        assert_eq!(db.find_lenses(&dx).len(), 1);
        // Fisheyes and lenses without data are left out; without a camera, every mount.
        let any = PhotoLens { lens: String::new(), focal: Some(60.0), ..Default::default() };
        let m = db.find_lenses(&any);
        // The 60 mm and the 24-70 zoom both reach 60 mm; nothing is sure without a name.
        let names: Vec<&str> = m.iter().map(|x| db.lenses[x.index].model.as_str()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.contains(&"Nikon AF Micro-Nikkor 60mm f/2.8D"));
        assert!(m.iter().all(|x| !x.sure));
    }
}
