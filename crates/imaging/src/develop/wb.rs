//! White balance: Kelvin/tint ↔ chromaticity (Robertson's isotemperature lines) and
//! chromaticity ↔ camera channel multipliers.

use super::matrix::{Mat3, inv, mul_vec, xy_to_xyz};

/// Robertson's table: reciprocal megakelvin, CIE 1960 (u, v) on the Planckian locus, and
/// the slope of the isotemperature line there.
const ROBERTSON: [(f64, f64, f64, f64); 31] = [
    (0.0, 0.18006, 0.26352, -0.24341),
    (10.0, 0.18066, 0.26589, -0.25479),
    (20.0, 0.18133, 0.26846, -0.26876),
    (30.0, 0.18208, 0.27119, -0.28539),
    (40.0, 0.18293, 0.27407, -0.30470),
    (50.0, 0.18388, 0.27709, -0.32675),
    (60.0, 0.18494, 0.28021, -0.35156),
    (70.0, 0.18611, 0.28342, -0.37915),
    (80.0, 0.18740, 0.28668, -0.40955),
    (90.0, 0.18880, 0.28997, -0.44278),
    (100.0, 0.19032, 0.29326, -0.47888),
    (125.0, 0.19462, 0.30141, -0.58204),
    (150.0, 0.19962, 0.30921, -0.70471),
    (175.0, 0.20525, 0.31647, -0.84901),
    (200.0, 0.21142, 0.32312, -1.0182),
    (225.0, 0.21807, 0.32909, -1.2168),
    (250.0, 0.22511, 0.33439, -1.4512),
    (275.0, 0.23247, 0.33904, -1.7298),
    (300.0, 0.24010, 0.34308, -2.0637),
    (325.0, 0.24792, 0.34655, -2.4681),
    (350.0, 0.25591, 0.34951, -2.9641),
    (375.0, 0.26400, 0.35200, -3.5814),
    (400.0, 0.27218, 0.35407, -4.3633),
    (425.0, 0.28039, 0.35577, -5.3762),
    (450.0, 0.28863, 0.35714, -6.7262),
    (475.0, 0.29685, 0.35823, -8.5955),
    (500.0, 0.30505, 0.35907, -11.324),
    (525.0, 0.31320, 0.35968, -15.628),
    (550.0, 0.32129, 0.36011, -23.325),
    (575.0, 0.32931, 0.36038, -40.770),
    (600.0, 0.33724, 0.36051, -116.45),
];

/// Tint units per unit of (u, v) distance from the locus (the DNG convention's scale).
const TINT_SCALE: f64 = 3000.0;

pub const MIN_KELVIN: f64 = 1700.0;
pub const MAX_KELVIN: f64 = 50000.0;

/// Unit vector along the isotemperature line with slope `t`, pointing to the green side
/// (increasing v).
fn green_dir(t: f64) -> (f64, f64) {
    let n = (1.0 + t * t).sqrt();
    (-1.0 / n, -t / n)
}

fn xy_to_uv((x, y): (f64, f64)) -> (f64, f64) {
    let d = -2.0 * x + 12.0 * y + 3.0;
    (4.0 * x / d, 6.0 * y / d)
}

fn uv_to_xy((u, v): (f64, f64)) -> (f64, f64) {
    let d = 2.0 * u - 8.0 * v + 4.0;
    (3.0 * u / d, 2.0 * v / d)
}

/// Chromaticity of a light source at `kelvin` with `tint` (positive = greener light,
/// which the correction turns magenta, as in LrC).
pub fn temp_tint_to_xy(kelvin: f64, tint: f64) -> (f64, f64) {
    let mired = 1e6 / kelvin.clamp(MIN_KELVIN, MAX_KELVIN);
    let i = ROBERTSON.iter().position(|r| r.0 > mired).unwrap_or(ROBERTSON.len() - 1).max(1);
    let (a, b) = (ROBERTSON[i - 1], ROBERTSON[i]);
    let f = ((mired - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    let (u, v) = (a.1 + f * (b.1 - a.1), a.2 + f * (b.2 - a.2));
    let (da, db) = (green_dir(a.3), green_dir(b.3));
    let (du, dv) = (da.0 + f * (db.0 - da.0), da.1 + f * (db.1 - da.1));
    let n = (du * du + dv * dv).sqrt();
    let off = tint / TINT_SCALE;
    uv_to_xy((u + off * du / n, v + off * dv / n))
}

/// Inverse of [`temp_tint_to_xy`].
pub fn xy_to_temp_tint(xy: (f64, f64)) -> (f64, f64) {
    let (u, v) = xy_to_uv(xy);
    // Signed distance from each isotemperature line (positive on the warmer side).
    let dist = |r: &(f64, f64, f64, f64)| ((v - r.2) - r.3 * (u - r.1)) / (1.0 + r.3 * r.3).sqrt();
    let mut prev = dist(&ROBERTSON[0]);
    let mut i = 1;
    while i < ROBERTSON.len() - 1 {
        let d = dist(&ROBERTSON[i]);
        if d < 0.0 && prev >= 0.0 || d >= 0.0 && prev < 0.0 {
            break;
        }
        prev = d;
        i += 1;
    }
    let d = dist(&ROBERTSON[i]);
    let (a, b) = (ROBERTSON[i - 1], ROBERTSON[i]);
    let f = if (prev - d).abs() > 1e-12 { (prev / (prev - d)).clamp(0.0, 1.0) } else { 0.0 };
    let mired = a.0 + f * (b.0 - a.0);
    let (lu, lv) = (a.1 + f * (b.1 - a.1), a.2 + f * (b.2 - a.2));
    let (da, db) = (green_dir(a.3), green_dir(b.3));
    let (du, dv) = (da.0 + f * (db.0 - da.0), da.1 + f * (db.1 - da.1));
    let n = (du * du + dv * dv).sqrt();
    let tint = ((u - lu) * du + (v - lv) * dv) / n * TINT_SCALE;
    let kelvin = if mired > 0.0 { 1e6 / mired } else { MAX_KELVIN };
    (kelvin.clamp(MIN_KELVIN, MAX_KELVIN), tint)
}

/// Camera multipliers (G = 1) that make a light of chromaticity `xy` neutral.
pub fn xy_to_multipliers(cam_xyz: &Mat3, xy: (f64, f64)) -> [f32; 3] {
    let n = mul_vec(cam_xyz, xy_to_xyz(xy));
    let m = [1.0 / n[0], 1.0 / n[1], 1.0 / n[2]];
    let m = [m[0] / m[1], 1.0, m[2] / m[1]];
    if m.iter().all(|v| v.is_finite() && *v > 0.0) { m.map(|v| v as f32) } else { [1.0; 3] }
}

/// The light chromaticity that multipliers `m` neutralize.
pub fn multipliers_to_xy(cam_xyz: &Mat3, m: [f32; 3]) -> Option<(f64, f64)> {
    let neutral = [1.0 / f64::from(m[0]), 1.0 / f64::from(m[1]), 1.0 / f64::from(m[2])];
    let xyz = mul_vec(&inv(cam_xyz)?, neutral);
    let s = xyz[0] + xyz[1] + xyz[2];
    (s > 0.0 && xyz[1] > 0.0).then(|| (xyz[0] / s, xyz[1] / s))
}

/// Relative white balance (files without a camera matrix): Temp and Tint −100..+100 as
/// channel gains, up to ±1 stop. Normalized to keep luminance about the same.
pub fn relative_multipliers(temp: f32, tint: f32) -> [f32; 3] {
    let t = temp / 100.0;
    let g = tint / 100.0;
    let m = [2f32.powf(0.6 * t), 2f32.powf(-0.5 * g), 2f32.powf(-0.6 * t)];
    let luma = 0.3 * m[0] + 0.6 * m[1] + 0.1 * m[2];
    m.map(|v| v / luma)
}

/// Inverse of [`relative_multipliers`] (for Auto and the eyedropper on such files).
pub fn relative_from_multipliers(m: [f32; 3]) -> (f32, f32) {
    let temp = (m[0] / m[2]).log2() / 1.2 * 100.0;
    let tint = -((m[1] / (m[0] * m[2]).sqrt()).log2() / 0.5) * 100.0;
    (temp.clamp(-100.0, 100.0), tint.clamp(-100.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_tint_round_trips() {
        for kelvin in [2000.0, 2850.0, 4000.0, 5500.0, 6504.0, 9000.0, 20000.0] {
            for tint in [-50.0, 0.0, 10.0, 80.0] {
                let (k, t) = xy_to_temp_tint(temp_tint_to_xy(kelvin, tint));
                assert!((k - kelvin).abs() / kelvin < 0.005, "{kelvin} K {tint}: got {k} K");
                assert!((t - tint).abs() < 0.5, "{kelvin} K {tint}: got tint {t}");
            }
        }
    }

    #[test]
    fn d65_is_near_6500k_with_small_tint() {
        let (k, t) = xy_to_temp_tint((0.3127, 0.3290));
        assert!((6400.0..6600.0).contains(&k), "{k}");
        assert!(t.abs() < 15.0, "{t}");
        // Light greener than the locus has positive tint.
        let (x, y) = temp_tint_to_xy(5000.0, 30.0);
        let (x0, y0) = temp_tint_to_xy(5000.0, 0.0);
        assert!(y > y0 && x < x0 + 0.01, "({x}, {y}) vs ({x0}, {y0})");
    }

    #[test]
    fn camera_multipliers_round_trip() {
        // Nikon D750's XYZ → camera matrix.
        let cam: Mat3 = [[0.902, -0.289, -0.0715], [-0.4535, 1.2436, 0.2348], [-0.0934, 0.1919, 0.7086]];
        let m_day = xy_to_multipliers(&cam, temp_tint_to_xy(5500.0, 0.0));
        let m_tung = xy_to_multipliers(&cam, temp_tint_to_xy(2850.0, 0.0));
        assert!(m_tung[2] > m_day[2] && m_tung[0] < m_day[0], "tungsten needs more blue, less red");
        let (k, t) = xy_to_temp_tint(multipliers_to_xy(&cam, m_day).unwrap());
        assert!((k - 5500.0).abs() < 30.0 && t.abs() < 1.0, "{k} {t}");
    }

    #[test]
    fn relative_round_trips() {
        for (temp, tint) in [(0.0, 0.0), (40.0, -20.0), (-80.0, 60.0)] {
            let (t, g) = relative_from_multipliers(relative_multipliers(temp, tint));
            assert!((t - temp).abs() < 0.5 && (g - tint).abs() < 0.5, "{temp},{tint} -> {t},{g}");
        }
    }
}
