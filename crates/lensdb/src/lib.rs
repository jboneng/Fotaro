//! The Lensfun lens database (<https://lensfun.github.io>, data under CC BY-SA 3.0): its
//! XML files read into [`Database`], a photo's camera and lens found in it ([`Database::find_camera`],
//! [`Database::find_lenses`]), and a lens's corrections resolved for a photo
//! ([`resolve`]) as the [`LensProfile`] the develop settings keep.
//!
//! The database is downloaded on first use ([`store`]); nothing of Lensfun's library is
//! used, only its data and the way it interpolates and scales it.

mod matching;
mod resolve;
pub mod store;
mod xml;

pub use imaging::develop::{Distortion, LensProfile, Tca, Vignetting};
pub use matching::{LensMatch, PhotoLens, fuzzy_score, parse_name};
pub use resolve::resolve;

/// A lens mount and the mounts whose lenses fit it.
#[derive(Debug, Clone, PartialEq)]
pub struct Mount {
    pub name: String,
    pub compat: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    pub maker: String,
    pub model: String,
    /// Other names (`lang` variants).
    pub names: Vec<String>,
    pub variant: Option<String>,
    pub mount: String,
    pub crop: f32,
}

/// What kind of projection a lens has (only rectilinear lenses are corrected for now).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LensType {
    #[default]
    Rectilinear,
    Fisheye,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DistortionAt {
    pub focal: f32,
    pub model: Distortion,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TcaAt {
    pub focal: f32,
    pub model: Tca,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VignettingAt {
    pub focal: f32,
    pub aperture: f32,
    pub distance: f32,
    pub model: Vignetting,
}

/// One set of calibration data, made with a camera of `crop` and `aspect`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Calibration {
    pub crop: f32,
    pub aspect: f32,
    pub distortion: Vec<DistortionAt>,
    pub tca: Vec<TcaAt>,
    pub vignetting: Vec<VignettingAt>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lens {
    pub maker: String,
    pub model: String,
    /// Other names (`lang` variants), for matching and showing.
    pub names: Vec<String>,
    pub mounts: Vec<String>,
    pub crop: f32,
    pub aspect: f32,
    pub kind: LensType,
    pub center: [f32; 2],
    pub min_focal: f32,
    pub max_focal: f32,
    /// The widest aperture (f-number) at the short and long end.
    pub min_aperture: f32,
    pub max_aperture: f32,
    pub calibrations: Vec<Calibration>,
}

impl Lens {
    pub fn has_corrections(&self) -> bool {
        self.calibrations
            .iter()
            .any(|c| !c.distortion.is_empty() || !c.tca.is_empty() || !c.vignetting.is_empty())
    }

    pub fn is_zoom(&self) -> bool {
        self.max_focal > self.min_focal + 0.01
    }
}

#[derive(Debug, Clone, Default)]
pub struct Database {
    pub mounts: Vec<Mount>,
    pub cameras: Vec<Camera>,
    pub lenses: Vec<Lens>,
    /// When the data was made (Unix seconds), if known.
    pub timestamp: Option<i64>,
}

impl Database {
    /// Read every `.xml` file in `dir`.
    pub fn load(dir: &std::path::Path) -> anyhow::Result<Database> {
        let mut db = Database::default();
        let mut files: Vec<_> = std::fs::read_dir(dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml")))
            .collect();
        files.sort();
        anyhow::ensure!(!files.is_empty(), "no lens database in {}", dir.display());
        for f in files {
            let text = std::fs::read_to_string(&f)?;
            xml::read_into(&text, &mut db).map_err(|e| anyhow::anyhow!("{}: {e}", f.display()))?;
        }
        db.timestamp =
            std::fs::read_to_string(dir.join("timestamp.txt")).ok().and_then(|t| t.trim().parse().ok());
        Ok(db)
    }

    /// Read one XML document (tests, and single files).
    pub fn from_xml(text: &str) -> anyhow::Result<Database> {
        let mut db = Database::default();
        xml::read_into(text, &mut db)?;
        Ok(db)
    }

    /// Mounts whose lenses fit a camera of mount `mount`: itself and what it lists as
    /// compatible (one level, as Lensfun).
    pub fn compatible_mounts(&self, mount: &str) -> Vec<String> {
        let mut out = vec![mount.to_string()];
        if let Some(m) = self.mounts.iter().find(|m| m.name == mount) {
            out.extend(m.compat.iter().cloned());
        }
        out
    }

    /// The lens makers, sorted (for the panel's Make menu).
    pub fn makers(&self) -> Vec<String> {
        let mut m: Vec<String> = self.lenses.iter().map(|l| l.maker.clone()).collect();
        m.sort_by_key(|s| s.to_lowercase());
        m.dedup();
        m
    }
}
