//! Lens Corrections in the app: the Lensfun database (downloaded on first use, loaded on a
//! thread), what the photo says about its camera and lens, and which profile a photo gets.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crossbeam_channel::{Receiver, unbounded};
use imaging::develop::{LensCorrections, LensProfile, LensSetup};

static DIR: OnceLock<PathBuf> = OnceLock::new();

/// Where the lens database lives (set once at start-up).
pub fn set_dir(dir: PathBuf) {
    let _ = DIR.set(dir);
}

pub fn dir() -> Option<&'static std::path::Path> {
    DIR.get().map(PathBuf::as_path)
}

/// What a photo says about how it was taken.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoOptics {
    pub lens: lensdb::PhotoLens,
    pub aperture: Option<f32>,
}

impl PhotoOptics {
    pub fn from_metadata(p: Option<catalog::PhotoMetadata>) -> PhotoOptics {
        let p = p.unwrap_or_default();
        PhotoOptics {
            lens: lensdb::PhotoLens {
                camera_maker: p.make.clone().unwrap_or_default(),
                camera_model: p.model.clone().unwrap_or_default(),
                lens: p.lens.clone().unwrap_or_default(),
                focal: p.focal_len.filter(|f| *f > 0.0).map(|f| f as f32),
            },
            aperture: p.aperture.filter(|a| *a > 0.0).map(|a| a as f32),
        }
    }
}

/// Which tab of the panel is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LensTab {
    #[default]
    Profile,
    Manual,
}

type Loaded = Result<lensdb::Database, String>;

#[derive(Default)]
pub struct LensState {
    pub db: Option<Arc<lensdb::Database>>,
    loading: Option<Receiver<Loaded>>,
    /// Downloading: how far (0..1).
    pub downloading: Option<Arc<Mutex<f32>>>,
    pub error: Option<String>,
    /// The photo being developed, and what was last worked out for it.
    pub photo: Option<PhotoOptics>,
    pub(crate) checked: Option<(i64, LensCorrections)>,
    pub candidates: Vec<lensdb::LensMatch>,
    pub status: String,
    pub tab: LensTab,
    /// The Make menu's choice while choosing a lens by hand.
    pub maker: Option<String>,
    /// Remove CA is being measured.
    pub measuring: bool,
}

impl LensState {
    /// A new photo: forget what was worked out for the last one.
    pub fn show(&mut self, photo: PhotoOptics) {
        self.photo = Some(photo);
        self.checked = None;
        self.candidates.clear();
        self.status.clear();
        self.maker = None;
        self.measuring = false;
    }

    /// Is the database there (downloaded), whether loaded or not?
    pub fn available(&self) -> bool {
        self.db.is_some() || dir().is_some_and(lensdb::store::ready)
    }

    /// Start loading the database if it is there and not loaded.
    pub fn load(&mut self, ctx: &eframe::egui::Context) {
        if self.db.is_some() || self.loading.is_some() || self.downloading.is_some() {
            return;
        }
        let Some(dir) = dir().filter(|d| lensdb::store::ready(d)) else { return };
        let (tx, rx) = unbounded();
        let ctx = ctx.clone();
        let folder = lensdb::store::folder(dir);
        std::thread::spawn(move || {
            let r = lensdb::Database::load(&folder).map_err(|e| format!("{e:#}"));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.loading = Some(rx);
    }

    /// Download (or update) the database.
    pub fn download(&mut self, ctx: &eframe::egui::Context) {
        if self.downloading.is_some() {
            return;
        }
        let Some(dir) = dir().map(std::path::Path::to_path_buf) else {
            self.error = Some("No folder for the lens database".into());
            return;
        };
        let progress = Arc::new(Mutex::new(0.0f32));
        let (tx, rx) = unbounded();
        let (ctx, p) = (ctx.clone(), progress.clone());
        std::thread::spawn(move || {
            let r = std::fs::create_dir_all(&dir)
                .map_err(anyhow::Error::from)
                .and_then(|_| {
                    lensdb::store::download(&dir, &mut |f| {
                        *p.lock().unwrap_or_else(|e| e.into_inner()) = f;
                        ctx.request_repaint();
                    })
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.loading = Some(rx);
        self.downloading = Some(progress);
        self.error = None;
    }

    /// Take a finished load or download.
    pub fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        if let Ok(r) = rx.try_recv() {
            match r {
                Ok(db) => {
                    self.db = Some(Arc::new(db));
                    self.checked = None;
                }
                Err(e) => {
                    log::warn!("lens database: {e}");
                    self.error = Some(e);
                }
            }
            self.loading = None;
            self.downloading = None;
        }
    }

    pub fn busy(&self) -> bool {
        self.loading.is_some()
    }

    /// The camera's crop factor and name, as the database knows it.
    pub fn camera(&self, db: &lensdb::Database) -> (f32, String) {
        let Some(p) = &self.photo else { return (1.0, String::new()) };
        match db.find_camera(&p.lens.camera_maker, &p.lens.camera_model) {
            Some(c) => (c.crop, c.model.clone()),
            None => (1.0, p.lens.camera_model.clone()),
        }
    }

    /// `lens` resolved for the photo (focal, aperture from it; else `focal` / `aperture`).
    pub fn resolve_for(&self, db: &lensdb::Database, lens: &lensdb::Lens, focal: Option<f32>, aperture: Option<f32>) -> LensProfile {
        let (crop, camera) = self.camera(db);
        let p = self.photo.clone().unwrap_or_default();
        let focal = p.lens.focal.or(focal).unwrap_or(lens.min_focal);
        let aperture = p.aperture.or(aperture).unwrap_or(8.0);
        lensdb::resolve(lens, &camera, crop, focal, aperture, 1000.0)
    }

    /// The profile the settings should have for this photo (`None`: none found), and a
    /// line saying how it was chosen.
    pub fn wanted(&mut self, db: &lensdb::Database, l: &LensCorrections) -> Option<LensProfile> {
        let photo = self.photo.clone().unwrap_or_default();
        self.candidates = db.find_lenses(&photo.lens);
        match l.setup {
            LensSetup::Custom => {
                let current = l.profile.as_ref()?;
                let Some(lens) = db.lenses.iter().find(|x| x.model == current.lens && x.maker == current.maker) else {
                    self.status = format!("{} (not in this lens database)", current.lens);
                    return l.profile.clone();
                };
                self.status = format!("Chosen: {}", lens.model);
                Some(self.resolve_for(db, lens, Some(current.focal), Some(current.aperture)))
            }
            LensSetup::Auto => {
                if photo.lens.lens.is_empty() && photo.lens.focal.is_none() {
                    self.status = "The photo doesn't say which lens: choose it below".into();
                    return None;
                }
                let Some(m) = self.candidates.first().copied() else {
                    self.status = if photo.lens.lens.is_empty() {
                        "No lens profile found: choose the lens below".into()
                    } else {
                        format!("No profile for “{}”: choose the lens below", photo.lens.lens)
                    };
                    return None;
                };
                let lens = &db.lenses[m.index];
                self.status = if m.sure {
                    format!("Found: {}", lens.model)
                } else {
                    format!("Best guess ({} fit): {}", self.candidates.len(), lens.model)
                };
                Some(self.resolve_for(db, lens, None, None))
            }
        }
    }
}
