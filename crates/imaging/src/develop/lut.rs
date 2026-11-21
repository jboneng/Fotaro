//! 3D LUTs (`.cube` files) as a Profile (LrC's creative profiles): a look applied after
//! the base tone, with an Amount.
//!
//! A `.cube` LUT maps sRGB-encoded colour (what most creative LUTs are made for) to
//! sRGB-encoded colour. The pipeline hands it display-referred values, converted to sRGB
//! and encoded, and converts the answer back.
//!
//! Imported files are copied to the LUT store (`<hash>-<name>.cube`) and settings name
//! them by content hash, so a photo keeps its look while the file is there.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use super::matrix;

/// A photo's Profile: which LUT, and how much of it (0..200, 100 as made).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LutProfile {
    pub name: String,
    /// The file's content hash (16 hex digits), naming it in the LUT store.
    pub hash: String,
    pub amount: f32,
}

impl Default for LutProfile {
    fn default() -> Self {
        LutProfile { name: String::new(), hash: String::new(), amount: 100.0 }
    }
}

/// A parsed `.cube`: an optional 1D shaper, then a 3D table.
#[derive(Debug, Clone, PartialEq)]
pub struct Lut {
    pub title: Option<String>,
    shaper: Option<Table1>,
    cube: Option<Table3>,
}

#[derive(Debug, Clone, PartialEq)]
struct Table1 {
    min: [f32; 3],
    max: [f32; 3],
    values: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq)]
struct Table3 {
    min: [f32; 3],
    max: [f32; 3],
    n: usize,
    /// Red changes fastest, then green, then blue (the `.cube` order).
    values: Vec<[f32; 3]>,
}

/// Read a `.cube` file's text (Adobe's and Resolve's dialects: `LUT_3D_SIZE`,
/// `LUT_1D_SIZE`, `DOMAIN_MIN/MAX`, `LUT_*_INPUT_RANGE`, `TITLE`, `#` comments).
pub fn parse_cube(text: &str) -> Result<Lut, String> {
    let (mut title, mut n1, mut n3) = (None, 0usize, 0usize);
    let (mut min, mut max) = ([0.0f32; 3], [1.0f32; 3]);
    let (mut range1, mut range3): (Option<[f32; 2]>, Option<[f32; 2]>) = (None, None);
    let mut rows: Vec<[f32; 3]> = Vec::new();
    let three = |words: &[&str], line: &str| -> Result<[f32; 3], String> {
        let v: Vec<f32> = words.iter().filter_map(|w| w.parse().ok()).collect();
        v.get(..3).map(|v| [v[0], v[1], v[2]]).ok_or_else(|| format!("bad line: {line}"))
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        match words[0] {
            "TITLE" => title = Some(line[5..].trim().trim_matches('"').to_string()),
            "LUT_3D_SIZE" => n3 = words.get(1).and_then(|w| w.parse().ok()).ok_or("bad LUT_3D_SIZE")?,
            "LUT_1D_SIZE" => n1 = words.get(1).and_then(|w| w.parse().ok()).ok_or("bad LUT_1D_SIZE")?,
            "DOMAIN_MIN" => min = three(&words[1..], line)?,
            "DOMAIN_MAX" => max = three(&words[1..], line)?,
            "LUT_1D_INPUT_RANGE" | "LUT_3D_INPUT_RANGE" => {
                let v: Vec<f32> = words[1..].iter().filter_map(|w| w.parse().ok()).collect();
                let r = v.get(..2).map(|v| [v[0], v[1]]).ok_or("bad input range")?;
                if words[0] == "LUT_1D_INPUT_RANGE" { range1 = Some(r) } else { range3 = Some(r) }
            }
            w if w.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.') => {
                rows.push(three(&words, line)?)
            }
            // Other keywords (LUT_IN_VIDEO_RANGE…) are left alone.
            _ => {}
        }
    }
    if n1 == 0 && n3 == 0 {
        return Err("not a LUT: no LUT_3D_SIZE or LUT_1D_SIZE".into());
    }
    if !(2..=256).contains(&n3) && n3 != 0 || n1 == 1 || n1 > 65536 {
        return Err("unusual LUT size".into());
    }
    if rows.len() != n1 + n3 * n3 * n3 {
        return Err(format!("expected {} rows, found {}", n1 + n3 * n3 * n3, rows.len()));
    }
    let span = |r: Option<[f32; 2]>| r.map(|[a, b]| ([a; 3], [b; 3]));
    // A Resolve file with both: the shaper first.
    let shaper = (n1 > 0).then(|| {
        let (lo, hi) = span(range1).unwrap_or((min, max));
        Table1 { min: lo, max: hi, values: rows[..n1].to_vec() }
    });
    let cube = (n3 > 0).then(|| {
        // Behind a shaper the 3D table takes the shaper's output (0..1 unless told).
        let (lo, hi) = span(range3).unwrap_or(if n1 > 0 { ([0.0; 3], [1.0; 3]) } else { (min, max) });
        Table3 { min: lo, max: hi, n: n3, values: rows[n1..].to_vec() }
    });
    if [&shaper.as_ref().map(|t| (t.min, t.max)), &cube.as_ref().map(|t| (t.min, t.max))]
        .iter()
        .flat_map(|r| r.iter())
        .any(|(lo, hi)| (0..3).any(|c| hi[c] <= lo[c]))
    {
        return Err("bad domain".into());
    }
    Ok(Lut { title, shaper, cube })
}

impl Lut {
    /// An sRGB-encoded colour through the LUT.
    pub fn apply(&self, mut c: [f32; 3]) -> [f32; 3] {
        if let Some(t) = &self.shaper {
            let n = t.values.len();
            for (k, v) in c.iter_mut().enumerate() {
                let f = ((*v - t.min[k]) / (t.max[k] - t.min[k])).clamp(0.0, 1.0) * (n - 1) as f32;
                let i = (f as usize).min(n - 2);
                let w = f - i as f32;
                *v = t.values[i][k] + w * (t.values[i + 1][k] - t.values[i][k]);
            }
        }
        match &self.cube {
            Some(t) => t.tetrahedral(c),
            None => c,
        }
    }
}

impl Table3 {
    fn tetrahedral(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.n;
        let top = (n - 1) as f32;
        let mut i = [0usize; 3];
        let mut f = [0f32; 3];
        for k in 0..3 {
            let v = ((c[k] - self.min[k]) / (self.max[k] - self.min[k])).clamp(0.0, 1.0) * top;
            i[k] = (v as usize).min(n - 2);
            f[k] = v - i[k] as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.values[(i[2] + b) * n * n + (i[1] + g) * n + i[0] + r];
        let (fr, fg, fb) = (f[0], f[1], f[2]);
        let c000 = at(0, 0, 0);
        let c111 = at(1, 1, 1);
        // The tetrahedron the point is in, by the order of its fractions.
        let (a, b, w) = if fr > fg {
            if fg > fb {
                (at(1, 0, 0), at(1, 1, 0), [fr, fg, fb])
            } else if fr > fb {
                (at(1, 0, 0), at(1, 0, 1), [fr, fb, fg])
            } else {
                (at(0, 0, 1), at(1, 0, 1), [fb, fr, fg])
            }
        } else if fb > fg {
            (at(0, 0, 1), at(0, 1, 1), [fb, fg, fr])
        } else if fb > fr {
            (at(0, 1, 0), at(0, 1, 1), [fg, fb, fr])
        } else {
            (at(0, 1, 0), at(1, 1, 0), [fg, fr, fb])
        };
        std::array::from_fn(|k| {
            (1.0 - w[0]) * c000[k] + (w[0] - w[1]) * a[k] + (w[1] - w[2]) * b[k] + w[2] * c111[k]
        })
    }
}

/// A LUT ready for the pipeline: display-linear working space in and out.
pub(super) struct PreparedLut {
    lut: Arc<Lut>,
    amount: f32,
    to_srgb: matrix::Mat3f,
    from_srgb: matrix::Mat3f,
}

/// Entries of the encode / decode tables.
const CODE: usize = 4096;

static ENCODE: LazyLock<Vec<f32>> = LazyLock::new(|| {
    // Indexed by √ of linear (fine steps in the shadows).
    (0..=CODE)
        .map(|i| {
            let v = (i as f32 / CODE as f32).powi(2);
            if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
        })
        .collect()
});
static DECODE: LazyLock<Vec<f32>> = LazyLock::new(|| {
    (0..=CODE)
        .map(|i| {
            let e = i as f32 / CODE as f32;
            if e <= 0.040_45 { e / 12.92 } else { ((e + 0.055) / 1.055).powf(2.4) }
        })
        .collect()
});

#[inline]
fn table(t: &[f32], x: f32) -> f32 {
    let f = x.clamp(0.0, 1.0) * CODE as f32;
    let i = (f as usize).min(CODE - 1);
    t[i] + (f - i as f32) * (t[i + 1] - t[i])
}

impl PreparedLut {
    pub(super) fn new(p: &LutProfile) -> Option<PreparedLut> {
        if p.amount == 0.0 {
            return None;
        }
        let lut = load_lut(&p.hash)?;
        Some(PreparedLut {
            lut,
            amount: p.amount / 100.0,
            to_srgb: matrix::to_f32(&matrix::work_to_srgb()),
            from_srgb: matrix::to_f32(&matrix::srgb_to_work()),
        })
    }

    #[inline]
    pub(super) fn apply(&self, d: [f32; 3]) -> [f32; 3] {
        let s = matrix::apply(&self.to_srgb, d);
        let e = s.map(|v| table(&ENCODE, v.max(0.0).sqrt()));
        let o = self.lut.apply(e);
        // The amount mixes the encoded colours (above 100 pushes further).
        let m: [f32; 3] = std::array::from_fn(|k| (e[k] + self.amount * (o[k] - e[k])).clamp(0.0, 1.0));
        let lin = m.map(|v| table(&DECODE, v));
        // Colours outside sRGB come back inside it: the LUT knows no others.
        matrix::apply(&self.from_srgb, lin).map(|v| v.max(0.0))
    }
}

// --- The LUT store -----------------------------------------------------------------

static STORE: OnceLock<PathBuf> = OnceLock::new();
/// Parsed LUTs by hash.
type Loaded = Vec<(String, Arc<Lut>)>;
static LOADED: LazyLock<Mutex<Loaded>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Where imported LUTs are kept (set once at start-up).
pub fn set_lut_store(dir: PathBuf) {
    let _ = STORE.set(dir);
}

pub fn lut_store() -> Option<&'static Path> {
    STORE.get().map(PathBuf::as_path)
}

/// FNV-1a of a file's bytes, as 16 hex digits.
pub fn content_hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// An imported LUT: its name (title or file name) and hash.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredLut {
    pub name: String,
    pub hash: String,
}

/// Check a `.cube` file and copy it into the store; the same file twice is kept once.
pub fn import_lut(path: &Path) -> Result<StoredLut, String> {
    let dir = lut_store().ok_or("no LUT folder")?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let lut = parse_cube(&text)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = lut.title.clone().filter(|t| !t.is_empty()).unwrap_or(stem);
    let hash = content_hash(&bytes);
    if !list_luts().iter().any(|l| l.hash == hash) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let safe: String =
            name.chars().map(|c| if c.is_alphanumeric() || " -_().".contains(c) { c } else { '_' }).collect();
        std::fs::write(dir.join(format!("{hash}-{}.cube", safe.trim())), &bytes)
            .map_err(|e| e.to_string())?;
    }
    Ok(StoredLut { name, hash })
}

/// The imported LUTs, by name.
pub fn list_luts() -> Vec<StoredLut> {
    let Some(dir) = lut_store() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<StoredLut> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".cube")?;
            let (hash, rest) = stem.split_once('-')?;
            (hash.len() == 16).then(|| StoredLut { name: rest.to_string(), hash: hash.to_string() })
        })
        .collect();
    out.sort_by_key(|l| l.name.to_lowercase());
    out
}

fn stored_path(hash: &str) -> Option<PathBuf> {
    let dir = lut_store()?;
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with(&format!("{hash}-")))
        .map(|e| e.path())
}

/// Is the LUT a profile names in the store?
pub fn lut_available(hash: &str) -> bool {
    load_lut(hash).is_some()
}

/// A stored LUT, parsed once.
pub fn load_lut(hash: &str) -> Option<Arc<Lut>> {
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, l)) = loaded.iter().find(|(h, _)| h == hash) {
        return Some(l.clone());
    }
    let text = std::fs::read_to_string(stored_path(hash)?).ok()?;
    let lut = Arc::new(parse_cube(&text).ok()?);
    if loaded.len() >= 16 {
        loaded.remove(0);
    }
    loaded.push((hash.to_string(), lut.clone()));
    Some(lut)
}

/// Put a parsed LUT in the cache under `hash` (tests, without files).
#[cfg(test)]
pub(super) fn cache_lut(hash: &str, lut: Lut) {
    LOADED.lock().unwrap().push((hash.to_string(), Arc::new(lut)));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.cube` of size `n` mapping each grid colour through `f`.
    pub(crate) fn cube(n: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> String {
        let mut s = format!("TITLE \"Test\"\n# comment\nLUT_3D_SIZE {n}\n");
        let t = (n - 1) as f32;
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let o = f([r as f32 / t, g as f32 / t, b as f32 / t]);
                    s += &format!("{} {} {}\n", o[0], o[1], o[2]);
                }
            }
        }
        s
    }

    #[test]
    fn an_identity_lut_changes_nothing() {
        let lut = parse_cube(&cube(17, |c| c)).unwrap();
        assert_eq!(lut.title.as_deref(), Some("Test"));
        for c in [[0.1, 0.5, 0.9], [0.33, 0.33, 0.8], [1.0, 0.0, 0.42], [0.77, 0.2, 0.5]] {
            let o = lut.apply(c);
            assert!((0..3).all(|k| (o[k] - c[k]).abs() < 1e-5), "{c:?} → {o:?}");
        }
    }

    #[test]
    fn a_lut_maps_colours_between_grid_points() {
        // Swapping red and blue, and inverting, are linear: exact between grid points.
        let swap = parse_cube(&cube(5, |c| [c[2], c[1], c[0]])).unwrap();
        let o = swap.apply([0.9, 0.3, 0.12]);
        assert!(
            (o[0] - 0.12).abs() < 1e-5 && (o[1] - 0.3).abs() < 1e-5 && (o[2] - 0.9).abs() < 1e-5,
            "{o:?}"
        );
        let inv = parse_cube(&cube(3, |c| c.map(|v| 1.0 - v))).unwrap();
        let o = inv.apply([0.25, 0.6, 0.9]);
        assert!(
            (o[0] - 0.75).abs() < 1e-5 && (o[1] - 0.4).abs() < 1e-5 && (o[2] - 0.1).abs() < 1e-5,
            "{o:?}"
        );
    }

    #[test]
    fn domains_and_shapers_are_followed() {
        // A domain of 0..2: the input is halved before the lookup.
        let text =
            cube(2, |c| c).replace("LUT_3D_SIZE 2", "LUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2");
        let o = parse_cube(&text).unwrap().apply([1.0, 0.5, 2.0]);
        assert!(
            (o[0] - 0.5).abs() < 1e-5 && (o[1] - 0.25).abs() < 1e-5 && (o[2] - 1.0).abs() < 1e-5,
            "{o:?}"
        );
        // A 1D shaper squaring, before an identity cube.
        let mut s = String::from("LUT_1D_SIZE 3\nLUT_3D_SIZE 2\n0 0 0\n0.25 0.25 0.25\n1 1 1\n");
        s += cube(2, |c| c).split_once("LUT_3D_SIZE 2\n").unwrap().1;
        let o = parse_cube(&s).unwrap().apply([0.5, 1.0, 0.0]);
        assert!((o[0] - 0.25).abs() < 1e-5 && (o[1] - 1.0).abs() < 1e-5 && o[2].abs() < 1e-5, "{o:?}");
        assert!(parse_cube("LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(parse_cube("hello").is_err());
    }

    #[test]
    fn the_amount_mixes_and_pushes() {
        let darker = parse_cube(&cube(9, |c| c.map(|v| v * 0.5))).unwrap();
        cache_lut("00000000000000d1", darker);
        let at = |amount: f32| {
            let p =
                PreparedLut::new(&LutProfile { name: "d".into(), hash: "00000000000000d1".into(), amount });
            p.map_or(0.18, |p| p.apply([0.18, 0.18, 0.18])[1])
        };
        let (none, half, full, more) = (at(0.0), at(50.0), at(100.0), at(200.0));
        assert!((none - 0.18).abs() < 1e-6);
        assert!(full < half && half < none && more < full, "{none} {half} {full} {more}");
        // Grey stays grey, and an identity round trip keeps the colour.
        cache_lut("00000000000000e0", parse_cube(&cube(17, |c| c)).unwrap());
        let id = PreparedLut::new(&LutProfile {
            name: "i".into(),
            hash: "00000000000000e0".into(),
            amount: 100.0,
        })
        .unwrap();
        let c = [0.3, 0.2, 0.1];
        let o = id.apply(c);
        assert!((0..3).all(|k| (o[k] - c[k]).abs() < 2e-3), "{o:?}");
    }
}
