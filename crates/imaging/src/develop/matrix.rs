//! 3×3 colour matrices: the working space (linear ProPhoto primaries) and sRGB.

use std::sync::OnceLock;

pub type Mat3 = [[f64; 3]; 3];
pub type Mat3f = [[f32; 3]; 3];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

pub fn mul_vec(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

#[inline]
pub fn apply(m: &Mat3f, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

pub fn inv(m: &Mat3) -> Option<Mat3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    Some([
        [c(1, 1, 2, 2) / det, -c(0, 1, 2, 2) / det, c(0, 1, 1, 2) / det],
        [-c(1, 0, 2, 2) / det, c(0, 0, 2, 2) / det, -c(0, 0, 1, 2) / det],
        [c(1, 0, 2, 1) / det, -c(0, 0, 2, 1) / det, c(0, 0, 1, 1) / det],
    ])
}

pub fn to_f32(m: &Mat3) -> Mat3f {
    std::array::from_fn(|i| std::array::from_fn(|j| m[i][j] as f32))
}

/// XYZ of a white point given as xy (Y = 1).
pub fn xy_to_xyz((x, y): (f64, f64)) -> [f64; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// RGB → XYZ for an RGB space with these primaries and white.
fn rgb_to_xyz(primaries: [(f64, f64); 3], white: (f64, f64)) -> Mat3 {
    let p: Mat3 = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let (x, y) = primaries[j];
            [x / y, 1.0, (1.0 - x - y) / y][i]
        })
    });
    let s = mul_vec(&inv(&p).expect("primaries are independent"), xy_to_xyz(white));
    std::array::from_fn(|i| std::array::from_fn(|j| p[i][j] * s[j]))
}

const D65: (f64, f64) = (0.3127, 0.3290);
const D50: (f64, f64) = (0.3457, 0.3585);
const SRGB: [(f64, f64); 3] = [(0.64, 0.33), (0.30, 0.60), (0.15, 0.06)];
const PROPHOTO: [(f64, f64); 3] = [(0.7347, 0.2653), (0.1596, 0.8404), (0.0366, 0.0001)];

/// Bradford chromatic adaptation from white `from` to white `to`.
fn bradford(from: (f64, f64), to: (f64, f64)) -> Mat3 {
    const B: Mat3 = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
    let (s, d) = (mul_vec(&B, xy_to_xyz(from)), mul_vec(&B, xy_to_xyz(to)));
    let scale: Mat3 = [[d[0] / s[0], 0.0, 0.0], [0.0, d[1] / s[1], 0.0], [0.0, 0.0, d[2] / s[2]]];
    mul(&inv(&B).expect("Bradford is invertible"), &mul(&scale, &B))
}

struct Matrices {
    srgb_to_work: Mat3,
    work_to_srgb: Mat3,
}

fn matrices() -> &'static Matrices {
    static M: OnceLock<Matrices> = OnceLock::new();
    M.get_or_init(|| {
        // Working space: ProPhoto primaries with its D50 white (white = 1,1,1); sRGB white
        // maps to it by Bradford adaptation, as an ICC transform would.
        let pp_to_xyz50 = rgb_to_xyz(PROPHOTO, D50);
        let srgb_to_xyz65 = rgb_to_xyz(SRGB, D65);
        let xyz65_to_50 = bradford(D65, D50);
        let srgb_to_work = mul(&inv(&pp_to_xyz50).unwrap(), &mul(&xyz65_to_50, &srgb_to_xyz65));
        Matrices { work_to_srgb: inv(&srgb_to_work).unwrap(), srgb_to_work }
    })
}

/// Linear sRGB → linear working space.
pub fn srgb_to_work() -> Mat3 {
    matrices().srgb_to_work
}

/// Linear working space → linear sRGB.
pub fn work_to_srgb() -> Mat3 {
    matrices().work_to_srgb
}

/// Luminance (Y) weights of the working space.
pub fn work_luma() -> [f32; 3] {
    static L: OnceLock<[f32; 3]> = OnceLock::new();
    *L.get_or_init(|| {
        let m = rgb_to_xyz(PROPHOTO, D50);
        [m[1][0] as f32, m[1][1] as f32, m[1][2] as f32]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_maps_to_white_and_inverse_round_trips() {
        let w = mul_vec(&srgb_to_work(), [1.0; 3]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-6), "{w:?}");
        let id = mul(&srgb_to_work(), &work_to_srgb());
        for (i, row) in id.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                assert!((v - if i == j { 1.0 } else { 0.0 }).abs() < 1e-9);
            }
        }
        let l = work_luma();
        assert!((l.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }
}
