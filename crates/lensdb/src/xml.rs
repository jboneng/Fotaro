//! Reading Lensfun's XML (database versions 1 and 2).

use anyhow::{Context, Result};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::{
    Calibration, Camera, Database, Distortion, DistortionAt, Lens, LensType, Mount, Tca, TcaAt, Vignetting,
    VignettingAt,
};

/// A small element tree: name, attributes, text, children.
#[derive(Debug, Default)]
struct El {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<El>,
}

impl El {
    fn attr(&self, k: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    fn num(&self, k: &str) -> Option<f32> {
        self.attr(k).and_then(|v| v.trim().parse().ok())
    }

    fn kids<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// The identifier text of child `name` (the one without `lang`), and the others.
    fn names(&self, name: &str) -> (Option<String>, Vec<String>) {
        let mut id = None;
        let mut other = Vec::new();
        for c in self.kids(name) {
            let t = c.text.trim().to_string();
            if t.is_empty() {
                continue;
            }
            if c.attr("lang").is_none() && id.is_none() {
                id = Some(t);
            } else {
                other.push(t);
            }
        }
        (id.or_else(|| other.first().cloned()), other)
    }

    fn child_text(&self, name: &str) -> Option<String> {
        self.kids(name).next().map(|c| c.text.trim().to_string()).filter(|t| !t.is_empty())
    }
}

fn element(s: &BytesStart) -> Result<El> {
    let mut e = El { name: String::from_utf8_lossy(s.name().as_ref()).into_owned(), ..Default::default() };
    for a in s.attributes() {
        let a = a.context("malformed attribute")?;
        let v = a.unescape_value().context("malformed attribute value")?;
        e.attrs.push((String::from_utf8_lossy(a.key.as_ref()).into_owned(), v.into_owned()));
    }
    Ok(e)
}

fn parse(text: &str) -> Result<El> {
    let mut r = Reader::from_str(text);
    let mut stack: Vec<El> = vec![El::default()];
    loop {
        match r.read_event().with_context(|| format!("XML error at byte {}", r.error_position()))? {
            Event::Start(s) => stack.push(element(&s)?),
            Event::Empty(s) => {
                let e = element(&s)?;
                stack.last_mut().context("bad XML")?.children.push(e);
            }
            Event::End(_) => {
                let e = stack.pop().context("unbalanced XML")?;
                stack.last_mut().context("unbalanced XML")?.children.push(e);
            }
            Event::Text(t) => {
                let t = t.unescape().context("malformed text")?;
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t);
                }
            }
            Event::CData(c) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&c));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    anyhow::ensure!(stack.len() == 1, "XML ended inside an element");
    stack.pop().context("empty XML")
}

/// Add one file's mounts, cameras and lenses to `db`.
pub(crate) fn read_into(text: &str, db: &mut Database) -> Result<()> {
    let doc = parse(text)?;
    let root = doc.kids("lensdatabase").next().context("not a Lensfun database (no <lensdatabase>)")?;
    for m in root.kids("mount") {
        if let Some(name) = m.child_text("name") {
            db.mounts
                .push(Mount { name, compat: m.kids("compat").map(|c| c.text.trim().to_string()).collect() });
        }
    }
    for c in root.kids("camera") {
        let (Some(maker), _) = c.names("maker") else { continue };
        let (Some(model), names) = c.names("model") else { continue };
        db.cameras.push(Camera {
            maker,
            model,
            names,
            variant: c.child_text("variant"),
            mount: c.child_text("mount").unwrap_or_default(),
            crop: c.child_text("cropfactor").and_then(|t| t.parse().ok()).unwrap_or(1.0),
        });
    }
    for l in root.kids("lens") {
        if let Some(lens) = read_lens(l) {
            db.lenses.push(lens);
        }
    }
    Ok(())
}

fn aspect(v: &str) -> Option<f32> {
    match v.split_once(':') {
        Some((a, b)) => Some(a.trim().parse::<f32>().ok()? / b.trim().parse::<f32>().ok()?),
        None => v.trim().parse().ok(),
    }
}

fn read_lens(l: &El) -> Option<Lens> {
    let (maker, _) = l.names("maker");
    let (model, names) = l.names("model");
    let (maker, model) = (maker?, model?);
    let crop = l.child_text("cropfactor").and_then(|t| t.parse().ok()).unwrap_or(1.0);
    let aspect_ratio = l.child_text("aspect-ratio").and_then(|t| aspect(&t)).unwrap_or(1.5);
    let kind = match l.child_text("type").as_deref() {
        None | Some("rectilinear") => LensType::Rectilinear,
        Some(t) if t.starts_with("fisheye") => LensType::Fisheye,
        Some(_) => LensType::Other,
    };
    let center =
        l.kids("center").next().map_or([0.0; 2], |c| [c.num("x").unwrap_or(0.0), c.num("y").unwrap_or(0.0)]);
    // Focal and aperture ranges: given, else read from the name ("24-70mm f/2.8").
    let parsed = crate::matching::parse_name(&model);
    let (mut min_focal, mut max_focal) = parsed.map_or((0.0, 0.0), |p| (p.min_focal, p.max_focal));
    if let Some(f) = l.kids("focal").next() {
        if let Some(v) = f.num("value") {
            (min_focal, max_focal) = (v, v);
        }
        if let (Some(a), Some(b)) = (f.num("min"), f.num("max")) {
            (min_focal, max_focal) = (a, b);
        }
    }
    let (mut min_aperture, mut max_aperture) =
        parsed.map_or((0.0, 0.0), |p| (p.min_aperture, p.max_aperture));
    if let Some(a) = l.kids("aperture").next()
        && let Some(v) = a.num("min")
    {
        min_aperture = v;
        max_aperture = parsed.map_or(v, |p| p.max_aperture.max(v));
    }
    let mut calibrations = Vec::new();
    for c in l.kids("calibration") {
        let mut cal = Calibration {
            crop: c.num("cropfactor").unwrap_or(crop),
            aspect: c.attr("aspect-ratio").and_then(aspect).unwrap_or(aspect_ratio),
            ..Default::default()
        };
        for e in &c.children {
            let Some(focal) = e.num("focal") else { continue };
            let n = |k: &str| e.num(k).unwrap_or(0.0);
            match (e.name.as_str(), e.attr("model")) {
                ("distortion", Some(m)) => {
                    let model = match m {
                        "poly3" => Distortion::Poly3 { k1: n("k1") },
                        "poly5" => Distortion::Poly5 { k1: n("k1"), k2: n("k2") },
                        "ptlens" => Distortion::Ptlens { a: n("a"), b: n("b"), c: n("c") },
                        _ => continue,
                    };
                    cal.distortion.push(DistortionAt { focal, model });
                }
                ("tca", Some(m)) => {
                    let one = |k: &str| e.num(k).unwrap_or(1.0);
                    let model = match m {
                        "linear" => Tca::Linear { kr: one("kr"), kb: one("kb") },
                        "poly3" => Tca::Poly3 {
                            vr: one("vr"),
                            cr: n("cr"),
                            br: n("br"),
                            vb: one("vb"),
                            cb: n("cb"),
                            bb: n("bb"),
                        },
                        _ => continue,
                    };
                    cal.tca.push(TcaAt { focal, model });
                }
                ("vignetting", Some("pa")) => cal.vignetting.push(VignettingAt {
                    focal,
                    aperture: n("aperture"),
                    distance: e.num("distance").unwrap_or(1000.0),
                    model: Vignetting { k1: n("k1"), k2: n("k2"), k3: n("k3") },
                }),
                _ => {}
            }
        }
        calibrations.push(cal);
    }
    if min_focal <= 0.0 {
        // Without a focal length the lens can't be used.
        return None;
    }
    Some(Lens {
        maker,
        model,
        names,
        mounts: l.kids("mount").map(|m| m.text.trim().to_string()).collect(),
        crop,
        aspect: aspect_ratio,
        kind,
        center,
        min_focal,
        max_focal,
        min_aperture,
        max_aperture,
        calibrations,
    })
}

#[cfg(test)]
pub(crate) const FIXTURE: &str = r#"<lensdatabase version="2">
    <mount>
        <name>Nikon F AF</name>
        <compat>Nikon F</compat>
        <compat>Nikon F AI</compat>
    </mount>
    <camera>
        <maker>Nikon Corporation</maker>
        <maker lang="en">Nikon</maker>
        <model>Nikon D750</model>
        <model lang="en">D750</model>
        <mount>Nikon F AF</mount>
        <cropfactor>1</cropfactor>
    </camera>
    <camera>
        <maker>Nikon Corporation</maker>
        <model>Nikon D7000</model>
        <mount>Nikon F AF</mount>
        <cropfactor>1.523</cropfactor>
    </camera>
    <lens>
        <maker>Nikon</maker>
        <model>Nikon AF-S Zoom-Nikkor 24-70mm f/2.8G ED</model>
        <model lang="en">Nikkor AF-S 24-70mm f/2.8G ED</model>
        <mount>Nikon F AF</mount>
        <cropfactor>1</cropfactor>
        <calibration>
            <!-- Taken with Nikon D800 -->
            <distortion model="ptlens" focal="24" a="0.01436" b="-0.06611" c="0.06436"/>
            <distortion model="ptlens" focal="35" a="0.01046" b="-0.03445" c="0.04427"/>
            <distortion model="ptlens" focal="50" a="0.00843" b="-0.02693" c="0.04576"/>
            <distortion model="ptlens" focal="70" a="0.00933" b="-0.03902" c="0.05818"/>
            <tca model="poly3" focal="24.0" vr="1.0006434" vb="0.9998886"/>
            <tca model="poly3" focal="70.0" vr="1.0000295" vb="0.9998099"/>
            <vignetting model="pa" focal="24.0" aperture="2.8" distance="1000" k1="-0.2153778" k2="-0.9491463" k3="0.4760640"/>
            <vignetting model="pa" focal="24.0" aperture="8.0" distance="1000" k1="-0.3052602" k2="0.1858384" k3="-0.1360005"/>
            <vignetting model="pa" focal="70.0" aperture="2.8" distance="1000" k1="-0.5" k2="0.1" k3="0"/>
        </calibration>
    </lens>
    <lens>
        <maker>Nikon</maker>
        <model>Nikon AF Micro-Nikkor 60mm f/2.8D</model>
        <mount>Nikon F AF</mount>
        <cropfactor>1</cropfactor>
        <calibration>
            <distortion model="poly3" focal="60" k1="0.01"/>
            <real-focal-length focal="60" real-focal="58"/>
        </calibration>
    </lens>
    <lens>
        <maker>Nikon</maker>
        <model>Nikon AF-S DX Nikkor 35mm f/1.8G</model>
        <mount>Nikon F AF</mount>
        <cropfactor>1.523</cropfactor>
        <calibration>
            <distortion model="poly3" focal="35" k1="-0.02"/>
        </calibration>
    </lens>
    <lens>
        <maker>Samyang</maker>
        <model>Samyang 8mm f/3.5 Fish-Eye CS</model>
        <mount>Nikon F AF</mount>
        <type>fisheye</type>
        <cropfactor>1.5</cropfactor>
    </lens>
</lensdatabase>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixture_reads() {
        let db = Database::from_xml(FIXTURE).unwrap();
        assert_eq!(db.mounts.len(), 1);
        assert_eq!(db.compatible_mounts("Nikon F AF"), ["Nikon F AF", "Nikon F", "Nikon F AI"]);
        assert_eq!(db.cameras.len(), 2);
        assert_eq!(
            (db.cameras[0].maker.as_str(), db.cameras[0].model.as_str()),
            ("Nikon Corporation", "Nikon D750")
        );
        assert_eq!(db.cameras[0].names, ["D750"]);
        assert_eq!(db.lenses.len(), 4);
        let z = &db.lenses[0];
        assert_eq!((z.min_focal, z.max_focal, z.min_aperture), (24.0, 70.0, 2.8));
        assert_eq!(z.names, ["Nikkor AF-S 24-70mm f/2.8G ED"]);
        let c = &z.calibrations[0];
        assert_eq!((c.distortion.len(), c.tca.len(), c.vignetting.len()), (4, 2, 3));
        assert_eq!(c.aspect, 1.5);
        assert_eq!(db.lenses[3].kind, LensType::Fisheye);
        assert!(!db.lenses[3].has_corrections());
    }
}
