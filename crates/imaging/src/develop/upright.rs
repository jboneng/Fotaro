//! Upright: find the straight lines of a photo and the camera turn that makes them
//! vertical and level (LrC's Level, Vertical, Full and Auto), or that straightens guides
//! drawn on the photo (Guided).
//!
//! Lines are found the way line-segment detectors do: pixels with a clear edge are grown
//! into regions whose edge direction agrees, and long thin regions become segments. The
//! turn (pitch, yaw, roll) is then fitted by robust least squares: each near-vertical
//! segment should come out vertical, each near-horizontal one level.

use super::settings::DevelopSettings;
use super::source::WorkingImage;
use super::transform::{Upright, camera, turned};

/// A straight edge of the photo, in normalised coordinates (centre 0, half long side 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub a: [f64; 2],
    pub b: [f64; 2],
}

impl Segment {
    fn length(&self) -> f64 {
        (self.b[0] - self.a[0]).hypot(self.b[1] - self.a[1])
    }

    /// Degrees from vertical (0..90).
    fn tilt_from_vertical(&self) -> f64 {
        let (dx, dy) = ((self.b[0] - self.a[0]).abs(), (self.b[1] - self.a[1]).abs());
        dx.atan2(dy).to_degrees()
    }
}

/// Long side of the image lines are looked for in.
const DETECT_PX: usize = 900;

/// The photo's straight edges.
pub fn detect_lines(img: &WorkingImage) -> Vec<Segment> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let k = w.max(h).div_ceil(DETECT_PX).max(1);
    let (dw, dh) = (w / k, h / k);
    if dw < 16 || dh < 16 {
        return Vec::new();
    }
    // Perceptual lightness, averaged over k × k blocks.
    let px = img.pixels.as_raw();
    let mut lum = vec![0.0f32; dw * dh];
    for (j, row) in lum.chunks_mut(dw).enumerate() {
        for (i, v) in row.iter_mut().enumerate() {
            let mut sum = 0.0;
            for y in j * k..(j + 1) * k {
                for x in i * k..(i + 1) * k {
                    let p = 3 * (y * w + x);
                    sum += px[p] + px[p + 1] + px[p + 2];
                }
            }
            *v = (sum / (3 * k * k) as f32).max(0.0).sqrt();
        }
    }
    // A light blur, then Sobel gradients.
    let at = |v: &[f32], x: isize, y: isize| {
        v[(y.clamp(0, dh as isize - 1) as usize) * dw + x.clamp(0, dw as isize - 1) as usize]
    };
    let blurred: Vec<f32> = (0..dw * dh)
        .map(|i| {
            let (x, y) = ((i % dw) as isize, (i / dw) as isize);
            let mut s = 0.0;
            for (dy, wy) in [(-1, 1.0), (0, 2.0), (1, 1.0)] {
                for (dx, wx) in [(-1, 1.0), (0, 2.0), (1, 1.0)] {
                    s += wx * wy * at(&lum, x + dx, y + dy);
                }
            }
            s / 16.0
        })
        .collect();
    let mut mag = vec![0.0f32; dw * dh];
    // Level-line direction (along the edge) as a doubled angle, so opposite directions agree.
    let mut dir = vec![(0.0f32, 0.0f32); dw * dh];
    for y in 1..dh as isize - 1 {
        for x in 1..dw as isize - 1 {
            let b = |dx, dy| at(&blurred, x + dx, y + dy);
            let gx = (b(1, -1) + 2.0 * b(1, 0) + b(1, 1)) - (b(-1, -1) + 2.0 * b(-1, 0) + b(-1, 1));
            let gy = (b(-1, 1) + 2.0 * b(0, 1) + b(1, 1)) - (b(-1, -1) + 2.0 * b(0, -1) + b(1, -1));
            let i = y as usize * dw + x as usize;
            mag[i] = gx.hypot(gy);
            let theta = gx.atan2(-gy);
            dir[i] = ((2.0 * theta).cos(), (2.0 * theta).sin());
        }
    }
    // Edge pixels: the strongest sixth of all pixels, and not just noise.
    let mut sorted = mag.clone();
    if sorted.is_empty() {
        return Vec::new();
    }
    sorted.sort_by(f32::total_cmp);
    let threshold = sorted[sorted.len() * 5 / 6].max(0.02);
    let mut seeds: Vec<usize> = (0..dw * dh).filter(|&i| mag[i] > threshold).collect();
    seeds.sort_by(|a, b| mag[*b].total_cmp(&mag[*a]));

    // Grow regions of agreeing direction (within about 22.5°: cos 45° of doubled angles).
    let tolerance = std::f32::consts::FRAC_PI_4.cos();
    let mut used = vec![false; dw * dh];
    let mut segments = Vec::new();
    let min_len = (0.04 * dw.max(dh) as f64).max(12.0);
    let (half, cx, cy) = (w.max(h) as f64 / 2.0, w as f64 / 2.0, h as f64 / 2.0);
    let mut stack = Vec::new();
    for seed in seeds {
        if used[seed] {
            continue;
        }
        used[seed] = true;
        let mut region = vec![seed];
        let mut sum = dir[seed];
        stack.push(seed);
        while let Some(p) = stack.pop() {
            let (x, y) = ((p % dw) as isize, (p / dw) as isize);
            let n = (sum.0 * sum.0 + sum.1 * sum.1).sqrt().max(1e-9);
            let mean = (sum.0 / n, sum.1 / n);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= dw as isize || ny >= dh as isize {
                        continue;
                    }
                    let q = ny as usize * dw + nx as usize;
                    if used[q] || mag[q] <= threshold {
                        continue;
                    }
                    if dir[q].0 * mean.0 + dir[q].1 * mean.1 < tolerance {
                        continue;
                    }
                    used[q] = true;
                    region.push(q);
                    sum = (sum.0 + dir[q].0, sum.1 + dir[q].1);
                    stack.push(q);
                }
            }
        }
        if region.len() < 12 {
            continue;
        }
        if let Some(s) = fit_segment(&region, &mag, dw, min_len) {
            // Downscaled pixels → normalised coordinates of the image.
            let norm =
                |p: [f64; 2]| [((p[0] + 0.5) * k as f64 - cx) / half, ((p[1] + 0.5) * k as f64 - cy) / half];
            segments.push(Segment { a: norm(s.0), b: norm(s.1) });
        }
    }
    segments
}

/// A region's principal axis as a segment, if the region is long and thin enough.
fn fit_segment(region: &[usize], mag: &[f32], dw: usize, min_len: f64) -> Option<([f64; 2], [f64; 2])> {
    let (mut sw, mut mx, mut my) = (0.0, 0.0, 0.0);
    for &p in region {
        let wgt = f64::from(mag[p]);
        sw += wgt;
        mx += wgt * (p % dw) as f64;
        my += wgt * (p / dw) as f64;
    }
    let (mx, my) = (mx / sw, my / sw);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for &p in region {
        let wgt = f64::from(mag[p]);
        let (dx, dy) = ((p % dw) as f64 - mx, (p / dw) as f64 - my);
        sxx += wgt * dx * dx;
        syy += wgt * dy * dy;
        sxy += wgt * dx * dy;
    }
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (ex, ey) = (angle.cos(), angle.sin());
    let (mut lo, mut hi, mut across) = (f64::MAX, f64::MIN, 0.0f64);
    for &p in region {
        let (dx, dy) = ((p % dw) as f64 - mx, (p / dw) as f64 - my);
        let along = dx * ex + dy * ey;
        lo = lo.min(along);
        hi = hi.max(along);
        across = across.max((-dx * ey + dy * ex).abs());
    }
    let len = hi - lo;
    (len >= min_len && len >= 6.0 * (2.0 * across + 1.0))
        .then_some(([mx + lo * ex, my + lo * ey], [mx + hi * ex, my + hi * ey]))
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Vertical,
    Horizontal,
}

/// What a fit may change and how it weighs the evidence.
struct Fit {
    /// Pitch, yaw, roll: free or held at 0.
    free: [bool; 3],
    /// Tolerance (degrees) for a segment to count as meant vertical / horizontal, and the
    /// weight of each kind.
    vertical: (f64, f64),
    horizontal: (f64, f64),
    /// Pull towards no turn (keeps a fit sensible when the evidence is thin).
    regularise: f64,
    /// Robust: discount lines that disagree with the rest (residual scale), or not.
    robust: bool,
}

/// How far `line` is from straight (sine of its lean) after turning by `a` (radians).
fn residual(a: &[f64; 3], (s, kind, _): &(Segment, Kind, f64)) -> f64 {
    let rot = camera(a[0], a[1], a[2]);
    let (Some(p), Some(q)) = (turned(&rot, s.a), turned(&rot, s.b)) else { return 1.0 };
    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
    let len = dx.hypot(dy).max(1e-12);
    match kind {
        Kind::Vertical => dx / len,
        Kind::Horizontal => dy / len,
    }
}

/// The share of the lines' weight that ends up straight (within 1.5°) after turning by
/// `a`: high for architecture, low for lines that only happen to be near vertical.
fn support(lines: &[(Segment, Kind, f64)], a: &[f64; 3]) -> f64 {
    let total: f64 = lines.iter().map(|l| l.2).sum();
    let straight: f64 = lines.iter().filter(|l| residual(a, l).abs() < 0.026).map(|l| l.2).sum();
    straight / total.max(1e-12)
}

/// Solve for the turn (pitch, yaw, roll, radians) that straightens `lines`.
fn solve(lines: &[(Segment, Kind, f64)], fit: &Fit) -> [f64; 3] {
    let free: Vec<usize> = (0..3).filter(|&i| fit.free[i]).collect();
    let mut angles = [0.0f64; 3];
    if free.is_empty() || lines.is_empty() {
        return angles;
    }
    for iter in 0..30 {
        // The robust scale shrinks as the fit settles (about 10° at first, then 3°).
        let c = if iter < 5 { 0.18 } else { 0.05 };
        let n = free.len();
        let mut jtj = [[0.0f64; 3]; 3];
        let mut jtr = [0.0f64; 3];
        for line in lines {
            let r = residual(&angles, line);
            let robust = if fit.robust { 1.0 / (1.0 + (r / c).powi(2)) } else { 1.0 };
            let wgt = line.2 * robust;
            let mut grad = [0.0f64; 3];
            for (gi, &k) in free.iter().enumerate() {
                let mut a = angles;
                a[k] += 1e-6;
                grad[gi] = (residual(&a, line) - r) / 1e-6;
            }
            for i in 0..n {
                jtr[i] += wgt * grad[i] * r;
                for j in 0..n {
                    jtj[i][j] += wgt * grad[i] * grad[j];
                }
            }
        }
        for (gi, &k) in free.iter().enumerate() {
            jtj[gi][gi] += fit.regularise;
            jtr[gi] += fit.regularise * angles[k];
        }
        let Some(step) = solve_linear(&jtj, &jtr, n) else { break };
        for (gi, &k) in free.iter().enumerate() {
            angles[k] -= step[gi].clamp(-0.2, 0.2);
        }
        if step.iter().take(n).all(|s| s.abs() < 1e-7) {
            break;
        }
    }
    angles
}

/// Solve the n × n system `a · x = b` (n ≤ 3) by elimination.
fn solve_linear(a: &[[f64; 3]; 3], b: &[f64; 3], n: usize) -> Option<[f64; 3]> {
    let mut m = [[0.0f64; 4]; 3];
    for i in 0..n {
        m[i][..n].copy_from_slice(&a[i][..n]);
        m[i][3] = b[i];
    }
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| m[i][col].abs().total_cmp(&m[j][col].abs()))?;
        if m[pivot][col].abs() < 1e-15 {
            return None;
        }
        m.swap(col, pivot);
        let pivot_row = m[col];
        for (row, r) in m.iter_mut().enumerate().take(n) {
            if row != col {
                let f = r[col] / pivot_row[col];
                for (v, p) in r.iter_mut().zip(pivot_row).skip(col) {
                    *v -= f * p;
                }
            }
        }
    }
    let mut x = [0.0f64; 3];
    for i in 0..n {
        x[i] = m[i][3] / m[i][i];
    }
    Some(x)
}

/// The Upright turn (degrees: pitch, yaw, roll) for `mode` from the photo's lines.
pub fn upright_angles(lines: &[Segment], mode: Upright) -> [f32; 3] {
    let fit = match mode {
        Upright::Off | Upright::Guided => return [0.0; 3],
        Upright::Level => Fit {
            free: [false, false, true],
            vertical: (20.0, 1.0),
            horizontal: (20.0, 1.0),
            regularise: 1e-4,
            robust: true,
        },
        Upright::Vertical => Fit {
            free: [true, false, true],
            vertical: (35.0, 1.0),
            horizontal: (20.0, 0.4),
            regularise: 1e-3,
            robust: true,
        },
        Upright::Full => Fit {
            free: [true, true, true],
            vertical: (35.0, 1.0),
            horizontal: (35.0, 1.0),
            regularise: 1e-3,
            robust: true,
        },
        // Balanced: horizontals count less (their convergence is often depth, wanted), and
        // turns cost more.
        Upright::Auto => Fit {
            free: [true, true, true],
            vertical: (30.0, 1.0),
            horizontal: (25.0, 0.5),
            regularise: 0.02,
            robust: true,
        },
    };
    let total: f64 = lines.iter().map(Segment::length).sum::<f64>().max(1e-9);
    let classified: Vec<(Segment, Kind, f64)> = lines
        .iter()
        .filter_map(|s| {
            let tilt = s.tilt_from_vertical();
            let wgt = s.length() / total;
            if tilt <= fit.vertical.0 {
                Some((*s, Kind::Vertical, wgt * fit.vertical.1))
            } else if 90.0 - tilt <= fit.horizontal.0 {
                Some((*s, Kind::Horizontal, wgt * fit.horizontal.1))
            } else {
                None
            }
        })
        .collect();
    // Only straighten what the photo shows: a perspective needs at least two lines of its
    // kind, long enough together (a third of the long side), or it is left alone.
    let evidence = |kind: Kind| {
        let of_kind = classified.iter().filter(|l| l.1 == kind);
        let (count, length) = of_kind.fold((0, 0.0), |(n, len), l| (n + 1, len + l.0.length()));
        count >= 2 && length >= 0.66
    };
    let fit = Fit {
        free: [
            fit.free[0] && evidence(Kind::Vertical),
            fit.free[1] && evidence(Kind::Horizontal),
            fit.free[2] && !classified.is_empty(),
        ],
        ..fit
    };
    // A big turn, or one that straightens few of the lines, means they weren't architecture
    // (whiskers, branches): fall back to levelling only, and then to nothing.
    let plausible = |a: &[f64; 3]| {
        a[0].abs() < 0.5 && a[1].abs() < 0.5 && a[2].abs() < 0.3 && support(&classified, a) >= 0.3
    };
    let angles = solve(&classified, &fit);
    if plausible(&angles) {
        return finish(angles);
    }
    let level = Fit { free: [false, false, fit.free[2]], ..fit };
    let angles = solve(&classified, &level);
    finish(if plausible(&angles) { angles } else { [0.0; 3] })
}

/// The turn (degrees) that makes the guides vertical or level, each as it is closest to,
/// for a `w × h` photo. Guides are `[x1, y1, x2, y2]` in 0..1 of the untransformed photo.
pub fn guided_angles(guides: &[[f32; 4]], w: u32, h: u32) -> [f32; 3] {
    let (w, h) = (f64::from(w.max(1)), f64::from(h.max(1)));
    let half = w.max(h) / 2.0;
    let norm = |x: f32, y: f32| [(f64::from(x) * w - w / 2.0) / half, (f64::from(y) * h - h / 2.0) / half];
    let lines: Vec<(Segment, Kind, f64)> = guides
        .iter()
        .map(|g| {
            let s = Segment { a: norm(g[0], g[1]), b: norm(g[2], g[3]) };
            let kind = if s.tilt_from_vertical() <= 45.0 { Kind::Vertical } else { Kind::Horizontal };
            (s, kind, 1.0)
        })
        .collect();
    let verticals = lines.iter().filter(|l| l.1 == Kind::Vertical).count();
    let horizontals = lines.len() - verticals;
    // As many unknowns as the guides pin down: two verticals fix the pitch, two
    // horizontals the yaw; any guide fixes the roll.
    let free = [verticals >= 2, horizontals >= 2, !lines.is_empty()];
    let fit = Fit { free, vertical: (90.0, 1.0), horizontal: (90.0, 1.0), regularise: 1e-9, robust: false };
    finish(solve(&lines, &fit))
}

/// Radians → degrees, within sensible limits.
fn finish(a: [f64; 3]) -> [f32; 3] {
    let limits = [40.0, 40.0, 45.0];
    std::array::from_fn(|i| (a[i].to_degrees().clamp(-limits[i], limits[i]) * 100.0).round() as f32 / 100.0)
}

/// Settings with Upright `mode` applied, analysing `img` (the untransformed photo) for
/// the automatic modes.
pub fn apply_upright(img: &WorkingImage, s: &DevelopSettings, mode: Upright) -> DevelopSettings {
    let mut out = s.clone();
    out.transform.upright = mode;
    out.transform.upright_angles = match mode {
        Upright::Off => [0.0; 3],
        Upright::Guided => guided_angles(&s.transform.guides, img.width(), img.height()),
        _ => upright_angles(&detect_lines(img), mode),
    };
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Segments of a building photographed looking up: verticals converging towards the top
    /// (as `turned` by a pitch), plus level-ish horizontals, all slightly rolled.
    fn scene(pitch: f64, roll: f64) -> Vec<Segment> {
        let rot = camera(pitch, 0.0, roll);
        let mut lines = Vec::new();
        for x in [-0.8, -0.4, 0.3, 0.7] {
            let (a, b) = (turned(&rot, [x, -0.5]).unwrap(), turned(&rot, [x, 0.5]).unwrap());
            lines.push(Segment { a, b });
        }
        for y in [-0.3, 0.2] {
            let (a, b) = (turned(&rot, [-0.6, y]).unwrap(), turned(&rot, [0.6, y]).unwrap());
            lines.push(Segment { a, b });
        }
        lines
    }

    #[test]
    fn solving_undoes_a_known_turn() {
        let (pitch, roll) = (0.25f64, 0.05f64);
        let lines = scene(pitch, roll);
        // Vertical recovers the inverse turn: the corrected lines are straight.
        let a = upright_angles(&lines, Upright::Vertical);
        let rot = camera(f64::from(a[0]).to_radians(), 0.0, f64::from(a[2]).to_radians());
        for s in &lines[..4] {
            let (p, q) = (turned(&rot, s.a).unwrap(), turned(&rot, s.b).unwrap());
            let lean = (q[0] - p[0]).atan2(q[1] - p[1]).to_degrees().abs();
            assert!(lean < 0.5, "vertical leans {lean}° after {a:?}");
        }
        // Level only turns: the horizontals end up level.
        let a = upright_angles(&lines, Upright::Level);
        assert_eq!((a[0], a[1]), (0.0, 0.0));
        assert!(a[2].abs() > 1.0, "{a:?}");
        assert_eq!(upright_angles(&lines, Upright::Off), [0.0; 3]);
    }

    #[test]
    fn guides_are_made_exactly_vertical() {
        // Two converging guides (as 0..1 of a 3:2 photo).
        let guides = [[0.2, 0.9, 0.25, 0.1], [0.8, 0.9, 0.75, 0.1]];
        let a = guided_angles(&guides, 300, 200);
        assert_eq!(a[1], 0.0, "no yaw from verticals alone");
        let rot = camera(f64::from(a[0]).to_radians(), 0.0, f64::from(a[2]).to_radians());
        let (w, h, half) = (300.0, 200.0, 150.0);
        for g in guides {
            let n =
                |x: f32, y: f32| [(f64::from(x) * w - w / 2.0) / half, (f64::from(y) * h - h / 2.0) / half];
            let (p, q) = (turned(&rot, n(g[0], g[1])).unwrap(), turned(&rot, n(g[2], g[3])).unwrap());
            assert!((q[0] - p[0]).abs() < 1e-3 * (q[1] - p[1]).abs(), "{p:?} {q:?}");
        }
        // One guide only rotates.
        let one = guided_angles(&[[0.1, 0.5, 0.9, 0.55]], 300, 200);
        assert_eq!((one[0], one[1]), (0.0, 0.0));
        assert!(one[2].abs() > 1.0);
    }

    #[test]
    fn detects_the_edges_of_a_drawn_rectangle() {
        use image::{ImageBuffer, Rgb};
        let px = ImageBuffer::from_fn(400, 300, |x, y| {
            let inside = (100..300).contains(&x) && (60..240).contains(&y);
            let v = if inside { 0.8f32 } else { 0.1 };
            Rgb([v, v, v])
        });
        let img = WorkingImage::for_tests(px);
        let lines = detect_lines(&img);
        let verticals = lines.iter().filter(|s| s.tilt_from_vertical() < 5.0).count();
        let horizontals = lines.iter().filter(|s| s.tilt_from_vertical() > 85.0).count();
        assert!(verticals >= 2 && horizontals >= 2, "{lines:?}");
    }
}
