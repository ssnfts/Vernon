//! Tuft detection in a grayscale image: find thin bright line segments and
//! measure their angle to the free-stream direction.
//!
//! Pipeline (all hand written, no image crate):
//! 1. separable Gaussian blur;
//! 2. local-contrast normalisation `(I - local mean) / max(local std, floor)`
//!    from box statistics (integral images); the floor stops flat noise being
//!    amplified;
//! 3. Sobel gradient and non-maximum suppression along the quantised gradient
//!    direction (Canny, 1986, without the hysteresis step);
//! 4. a gradient-directed Hough transform: every edge pixel votes only for
//!    the line normals within a few degrees of its own gradient direction,
//!    the accumulator is smoothed across the offset by the maximum tuft
//!    width so the two edges of a thin line add up (Duda & Hart, 1972, for
//!    the (theta, rho) parametrisation);
//! 5. peaks are turned into segments: edge pixels near the line are split at
//!    gaps, each run is refit by principal-axis analysis (sub-bin angle) and
//!    filtered by length and perpendicular width, and its votes are removed
//!    before the next peak (a greedy stand-in for the progressive
//!    probabilistic Hough of Matas et al., 2000 -- UNVERIFIED that this is
//!    equivalent in detail).
//!
//! A segment has no head or tail in a single image, so the reported angle is
//! the line orientation folded into `(-90, 90]`: positive is counter-clockwise
//! on screen (image y points down). Tufts that point back upstream therefore
//! cannot be told from attached ones here; the root of the tuft is needed for
//! that and is not available from the image alone.

use crate::cp::{classify_deviation, TuftClass, TuftThresholds};

/// An 8-bit grayscale image, row-major, `px.len() == w * h`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Gray {
    /// An image filled with one grey level.
    pub fn new(w: usize, h: usize, fill: u8) -> Self {
        Self { w, h, px: vec![fill; w * h] }
    }

    /// Pixel at `(x, y)`; panics when out of range.
    pub fn at(&self, x: usize, y: usize) -> u8 {
        self.px[y * self.w + x]
    }
}

/// One detected tuft, in image space (pixels, y down). This is the image-side
/// record; it is not the surface tuft of [`crate::tuft`].
#[derive(Debug, Clone, PartialEq)]
pub struct Tuft {
    /// Centre of the segment.
    pub x_px: f32,
    pub y_px: f32,
    /// Orientation relative to the free stream, degrees in `(-90, 90]`,
    /// counter-clockwise on screen positive.
    pub angle_deg_to_freestream: f32,
    /// Length of the segment along its axis.
    pub length_px: f32,
    /// Share of the expected edge pixels that were present, in `[0, 1]`.
    pub confidence: f32,
}

/// Detector settings.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectParams {
    /// Gaussian blur sigma in pixels.
    pub blur_sigma: f32,
    /// Half size of the box used for local mean and deviation.
    pub norm_radius: usize,
    /// Smallest local deviation (grey levels) used to normalise contrast.
    pub contrast_floor: f32,
    /// Minimum normalised gradient magnitude of an edge pixel.
    pub grad_threshold: f32,
    /// Shortest accepted segment.
    pub min_length_px: f32,
    /// Widest accepted segment, measured between its two edge lines.
    pub max_width_px: f32,
    /// Largest gap along a segment before it is split in two.
    pub max_gap_px: f32,
    /// Free-stream direction in image coordinates (y down); default +x.
    pub freestream: (f32, f32),
    /// Upper bound on the number of tufts returned.
    pub max_tufts: usize,
}

impl Default for DetectParams {
    fn default() -> Self {
        Self {
            blur_sigma: 1.0,
            norm_radius: 8,
            contrast_floor: 10.0,
            grad_threshold: 0.25,
            min_length_px: 12.0,
            max_width_px: 6.0,
            max_gap_px: 6.0,
            freestream: (1.0, 0.0),
            max_tufts: 256,
        }
    }
}

// --- classification ---------------------------------------------------------

/// An orientation within this many degrees of the free stream counts as
/// attached. 30 deg matches the default `attached_deg` of the surface tuft
/// classifier; it is a convention, not a physical constant.
pub const ATTACHED_MAX_DEG: f32 = 30.0;
/// Beyond this a tuft would point back upstream. A single image gives an
/// orientation folded into 90 deg, so this value is never reached from
/// [`detect`]; it is kept so [`classify`] agrees with the surface classifier.
pub const REVERSED_MIN_DEG: f32 = 120.0;

/// Class of a tuft from its angle to the free stream. Flutter needs several
/// frames, so a single image gives only `Attached` or `Separated` (plus
/// `Reversed` for angles beyond [`REVERSED_MIN_DEG`] if a caller has a signed
/// direction). Reuses the surface classifier's thresholds and class type.
pub fn classify(angle_deg: f32) -> TuftClass {
    let th = TuftThresholds {
        attached_deg: ATTACHED_MAX_DEG as f64,
        reversed_deg: REVERSED_MIN_DEG as f64,
        sd_attached_deg: f64::INFINITY,
        sd_separated_deg: f64::INFINITY,
    };
    classify_deviation(angle_deg as f64, 0.0, &th)
}

// --- image filters ------------------------------------------------------------

fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let r = (3.0 * sigma).ceil().max(1.0) as isize;
    let mut k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let s: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= s);
    k
}

/// Separable blur with clamped borders.
fn blur(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.0 {
        return src.to_vec();
    }
    let k = gaussian_kernel(sigma);
    let r = (k.len() / 2) as isize;
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let xx = (x as isize + i as isize - r).clamp(0, w as isize - 1) as usize;
                s += kv * src[y * w + xx];
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let yy = (y as isize + i as isize - r).clamp(0, h as isize - 1) as usize;
                s += kv * tmp[yy * w + x];
            }
            out[y * w + x] = s;
        }
    }
    out
}

/// `(I - box mean) / max(box std, floor)` with a `(2r+1)^2` box clamped to the image.
fn normalise_contrast(src: &[f32], w: usize, h: usize, r: usize, floor: f32) -> Vec<f32> {
    // integral images with a zero first row and column
    let iw = w + 1;
    let mut s1 = vec![0.0f64; iw * (h + 1)];
    let mut s2 = vec![0.0f64; iw * (h + 1)];
    for y in 0..h {
        let (mut r1, mut r2) = (0.0f64, 0.0f64);
        for x in 0..w {
            let v = src[y * w + x] as f64;
            r1 += v;
            r2 += v * v;
            s1[(y + 1) * iw + x + 1] = s1[y * iw + x + 1] + r1;
            s2[(y + 1) * iw + x + 1] = s2[y * iw + x + 1] + r2;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let n = ((x1 - x0) * (y1 - y0)) as f64;
            let sum = |s: &[f64]| s[y1 * iw + x1] - s[y0 * iw + x1] - s[y1 * iw + x0] + s[y0 * iw + x0];
            let mean = sum(&s1) / n;
            let var = (sum(&s2) / n - mean * mean).max(0.0);
            let sd = (var.sqrt() as f32).max(floor);
            out[y * w + x] = (src[y * w + x] - mean as f32) / sd;
        }
    }
    out
}

/// An edge pixel after non-maximum suppression.
struct Edge {
    x: f32,
    y: f32,
    /// Gradient direction folded into `[0, pi)`.
    normal: f32,
}

/// Sobel gradient of `z` (scaled so a unit step gives about 0.5) with NMS.
fn edge_pixels(z: &[f32], w: usize, h: usize, thr: f32) -> Vec<Edge> {
    if w < 3 || h < 3 {
        return Vec::new();
    }
    let mut gx = vec![0.0f32; w * h];
    let mut gy = vec![0.0f32; w * h];
    let mut mag = vec![0.0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let p = |dx: isize, dy: isize| z[(y as isize + dy) as usize * w + (x as isize + dx) as usize];
            let sx = (p(1, -1) + 2.0 * p(1, 0) + p(1, 1)) - (p(-1, -1) + 2.0 * p(-1, 0) + p(-1, 1));
            let sy = (p(-1, 1) + 2.0 * p(0, 1) + p(1, 1)) - (p(-1, -1) + 2.0 * p(0, -1) + p(1, -1));
            let i = y * w + x;
            gx[i] = sx / 8.0;
            gy[i] = sy / 8.0;
            mag[i] = (gx[i] * gx[i] + gy[i] * gy[i]).sqrt();
        }
    }
    let mut out = Vec::new();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            if mag[i] < thr {
                continue;
            }
            let a = gy[i].atan2(gx[i]).rem_euclid(std::f32::consts::PI);
            let q = ((a / std::f32::consts::FRAC_PI_4).round() as usize) % 4;
            let (dx, dy): (isize, isize) = match q {
                0 => (1, 0),
                1 => (1, 1),
                2 => (0, 1),
                _ => (-1, 1),
            };
            let at = |sx: isize, sy: isize| mag[(y as isize + sy) as usize * w + (x as isize + sx) as usize];
            if mag[i] >= at(dx, dy) && mag[i] >= at(-dx, -dy) {
                out.push(Edge { x: x as f32, y: y as f32, normal: a });
            }
        }
    }
    out
}

// --- Hough and segment extraction ----------------------------------------------

const THETA_BINS: usize = 180;
/// Half width, in 1-degree bins, of the range of line normals an edge pixel
/// votes for around its own gradient direction.
const VOTE_SPREAD_BINS: isize = 3;

struct Hough {
    rho_off: isize,
    n_rho: usize,
    acc: Vec<i32>,
    cos: [f32; THETA_BINS],
    sin: [f32; THETA_BINS],
}

impl Hough {
    fn new(w: usize, h: usize) -> Self {
        let diag = ((w * w + h * h) as f32).sqrt().ceil() as isize + 1;
        let mut cos = [0.0; THETA_BINS];
        let mut sin = [0.0; THETA_BINS];
        for t in 0..THETA_BINS {
            let a = (t as f32).to_radians();
            cos[t] = a.cos();
            sin[t] = a.sin();
        }
        let n_rho = (2 * diag + 1) as usize;
        Self { rho_off: diag, n_rho, acc: vec![0; THETA_BINS * n_rho], cos, sin }
    }

    /// Visit every `(theta bin, rho bin)` an edge votes for.
    fn bins(&self, e: &Edge) -> impl Iterator<Item = (usize, usize)> + '_ {
        let centre = (e.normal.to_degrees()).round() as isize;
        let (x, y) = (e.x, e.y);
        (-VOTE_SPREAD_BINS..=VOTE_SPREAD_BINS).map(move |k| {
            let t = (centre + k).rem_euclid(THETA_BINS as isize) as usize;
            let rho = (x * self.cos[t] + y * self.sin[t]).round() as isize + self.rho_off;
            (t, rho as usize)
        })
    }

    fn vote(&mut self, e: &Edge, delta: i32) {
        let bins: Vec<(usize, usize)> = self.bins(e).collect();
        for (t, r) in bins {
            self.acc[t * self.n_rho + r] += delta;
        }
    }

    /// Strongest `(theta, rho)` after summing `half` bins either side in rho.
    fn peak(&self, half: usize) -> (i32, usize, usize) {
        let mut best = (0, 0, 0);
        for t in 0..THETA_BINS {
            let row = &self.acc[t * self.n_rho..(t + 1) * self.n_rho];
            let mut window: i32 = row.iter().take(half).sum();
            for r in 0..self.n_rho {
                if r + half < self.n_rho {
                    window += row[r + half];
                }
                if r > half {
                    window -= row[r - half - 1];
                }
                if window > best.0 {
                    best = (window, t, r);
                }
            }
        }
        best
    }
}

/// Fit the principal axis of a point set: `(centroid, unit axis)`.
fn principal_axis(pts: &[(f64, f64)]) -> ((f64, f64), (f64, f64)) {
    let n = pts.len() as f64;
    let (cx, cy) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for p in pts {
        let (dx, dy) = (p.0 - cx, p.1 - cy);
        sxx += dx * dx;
        syy += dy * dy;
        sxy += dx * dy;
    }
    let a = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    ((cx, cy), (a.cos(), a.sin()))
}

/// Orientation of image-space direction `u` against the free stream `f`
/// (both y down), folded into `(-90, 90]`, counter-clockwise on screen positive.
fn angle_to_freestream(u: (f64, f64), f: (f32, f32)) -> f32 {
    let (vx, vy) = (u.0, -u.1);
    let (fx, fy) = (f.0 as f64, -(f.1 as f64));
    let mut a = (fx * vy - fy * vx).atan2(fx * vx + fy * vy).to_degrees();
    if a > 90.0 {
        a -= 180.0;
    } else if a <= -90.0 {
        a += 180.0;
    }
    a as f32
}

/// Detect tufts in `img`.
pub fn detect(img: &Gray, p: &DetectParams) -> Vec<Tuft> {
    let (w, h) = (img.w, img.h);
    if w < 8 || h < 8 || img.px.len() != w * h {
        return Vec::new();
    }
    let f: Vec<f32> = img.px.iter().map(|&v| v as f32).collect();
    let z = normalise_contrast(&blur(&f, w, h, p.blur_sigma), w, h, p.norm_radius, p.contrast_floor);
    let edges = edge_pixels(&z, w, h, p.grad_threshold);

    let mut hough = Hough::new(w, h);
    for e in &edges {
        hough.vote(e, 1);
    }
    let half = (p.max_width_px / 2.0).ceil() as usize;
    let mut alive = vec![true; edges.len()];
    let mut out = Vec::new();
    // Each pass removes at least one edge pixel, so this terminates.
    while out.len() < p.max_tufts {
        let (votes, t, r) = hough.peak(half);
        if (votes as f32) < p.min_length_px {
            break;
        }
        let (c, s) = (hough.cos[t] as f64, hough.sin[t] as f64);
        // centre of the summed window in rho
        let rho = (r as isize - hough.rho_off) as f64 + 0.0;
        let near: Vec<usize> = (0..edges.len())
            .filter(|&i| alive[i] && (edges[i].x as f64 * c + edges[i].y as f64 * s - rho).abs() <= half as f64 + 1.0)
            .collect();
        if near.is_empty() {
            // the window peak sat on an empty bin; drop it so the loop moves on
            hough.acc[t * hough.n_rho + r] = 0;
            continue;
        }
        // positions along the line direction (-s, c)
        let mut order: Vec<(f64, usize)> = near.iter().map(|&i| (-(edges[i].x as f64) * s + edges[i].y as f64 * c, i)).collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut runs: Vec<Vec<usize>> = vec![vec![order[0].1]];
        for k in 1..order.len() {
            if order[k].0 - order[k - 1].0 > p.max_gap_px as f64 {
                runs.push(Vec::new());
            }
            runs.last_mut().unwrap().push(order[k].1);
        }
        // Only pixels of accepted segments are consumed; if nothing was
        // accepted the whole band goes, so every pass still makes progress.
        let mut used: Vec<usize> = Vec::new();
        for run in runs {
            if run.len() >= 3 {
                if let Some(t) = fit_run(&edges, &run, p) {
                    out.push(t);
                    used.extend(run);
                }
            }
        }
        let consumed = if used.is_empty() { near } else { used };
        for &i in &consumed {
            alive[i] = false;
            hough.vote(&edges[i], -1);
        }
    }
    out.truncate(p.max_tufts);
    out
}

/// Refit one run of edge pixels and apply the length and width filters.
fn fit_run(edges: &[Edge], run: &[usize], p: &DetectParams) -> Option<Tuft> {
    let pts: Vec<(f64, f64)> = run.iter().map(|&i| (edges[i].x as f64, edges[i].y as f64)).collect();
    let (c, u) = principal_axis(&pts);
    let (mut tmin, mut tmax, mut pmin, mut pmax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for q in &pts {
        let (dx, dy) = (q.0 - c.0, q.1 - c.1);
        let along = dx * u.0 + dy * u.1;
        let perp = -dx * u.1 + dy * u.0;
        tmin = tmin.min(along);
        tmax = tmax.max(along);
        pmin = pmin.min(perp);
        pmax = pmax.max(perp);
    }
    let length = tmax - tmin;
    let width = pmax - pmin;
    // +1: edge pixels sit on whole pixels, which adds up to a pixel of spread
    if (length as f32) < p.min_length_px || width as f32 > p.max_width_px + 1.0 {
        return None;
    }
    let mid = (c.0 + u.0 * 0.5 * (tmin + tmax), c.1 + u.1 * 0.5 * (tmin + tmax));
    // two edge lines, about one pixel per step of the dominant axis each
    let expected = 2.0 * length * u.0.abs().max(u.1.abs());
    Some(Tuft {
        x_px: mid.0 as f32,
        y_px: mid.1 as f32,
        angle_deg_to_freestream: angle_to_freestream(u, p.freestream),
        length_px: length as f32,
        confidence: (pts.len() as f64 / expected.max(1.0)).min(1.0) as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: &mut u64) -> f32 {
        *seed ^= *seed >> 12;
        *seed ^= *seed << 25;
        *seed ^= *seed >> 27;
        ((seed.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u64 << 24) as f32
    }

    /// Grey 100 plus uniform noise of +-8, plus Gaussian-profile bright lines.
    /// Each line is `(centre x, centre y, length, angle deg ccw on screen, width sigma)`.
    fn scene(w: usize, h: usize, lines: &[(f32, f32, f32, f32, f32)], seed: u64) -> Gray {
        let mut s = seed;
        let mut v: Vec<f32> = (0..w * h).map(|_| 100.0 + 16.0 * (rng(&mut s) - 0.5)).collect();
        for &(cx, cy, len, ang, sig) in lines {
            let (ux, uy) = (ang.to_radians().cos(), -ang.to_radians().sin());
            let (ax, ay, bx, by) = (cx - ux * len / 2.0, cy - uy * len / 2.0, cx + ux * len / 2.0, cy + uy * len / 2.0);
            for y in 0..h {
                for x in 0..w {
                    let (px, py) = (x as f32 - ax, y as f32 - ay);
                    let (dx, dy) = (bx - ax, by - ay);
                    let t = ((px * dx + py * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                    let d2 = (px - t * dx).powi(2) + (py - t * dy).powi(2);
                    v[y * w + x] += 140.0 * (-d2 / (2.0 * sig * sig)).exp();
                }
            }
        }
        Gray { w, h, px: v.iter().map(|q| q.clamp(0.0, 255.0) as u8).collect() }
    }

    fn diff180(a: f32, b: f32) -> f32 {
        let d = (a - b).rem_euclid(180.0);
        d.min(180.0 - d)
    }

    const ANGLES: [f32; 8] = [0.0, 20.0, 45.0, 70.0, 90.0, -30.0, -60.0, 10.0];

    fn grid_lines() -> Vec<(f32, f32, f32, f32, f32)> {
        ANGLES
            .iter()
            .enumerate()
            .map(|(i, &a)| (30.0 + 60.0 * (i % 4) as f32, 30.0 + 60.0 * (i / 4) as f32, 36.0, a, 0.8))
            .collect()
    }

    #[test]
    fn tuft_detect_recovers_known_angles() {
        let mut found = 0;
        let mut total = 0;
        for seed in [1u64, 7, 99] {
            let lines = grid_lines();
            let img = scene(240, 120, &lines, seed);
            let tufts = detect(&img, &DetectParams::default());
            for l in &lines {
                total += 1;
                let hit = tufts.iter().find(|t| (t.x_px - l.0).hypot(t.y_px - l.1) < 6.0);
                if let Some(t) = hit {
                    assert!(
                        diff180(t.angle_deg_to_freestream, l.3) <= 3.0,
                        "seed {seed}: expected {} got {}",
                        l.3,
                        t.angle_deg_to_freestream
                    );
                    assert!((t.length_px - 36.0).abs() < 12.0, "length {}", t.length_px);
                    assert!(t.confidence > 0.5 && t.confidence <= 1.0);
                    found += 1;
                }
            }
            // no hallucinated extras
            assert!(tufts.len() <= lines.len() + 1, "{} detections for {} tufts: {:?}", tufts.len(), lines.len(), tufts);
        }
        assert!(found as f32 >= 0.9 * total as f32, "recall {found}/{total}");
    }

    #[test]
    fn tuft_detect_ignores_pure_noise() {
        let img = scene(160, 120, &[], 5);
        assert!(detect(&img, &DetectParams::default()).is_empty());
    }

    #[test]
    fn tuft_detect_rejects_wide_bar_and_short_dash() {
        // a 12 px wide bar (sigma 5) is not a tuft; an 8 px dash is too short
        let img = scene(160, 120, &[(50.0, 60.0, 50.0, 0.0, 5.0), (120.0, 60.0, 8.0, 30.0, 0.8)], 3);
        assert!(detect(&img, &DetectParams::default()).is_empty());
    }

    #[test]
    fn tuft_detect_free_stream_direction_shifts_angles() {
        // a horizontal tuft against a free stream pointing down the image is at 90 deg
        let img = scene(100, 100, &[(50.0, 50.0, 36.0, 0.0, 0.8)], 11);
        let p = DetectParams { freestream: (0.0, 1.0), ..DetectParams::default() };
        let t = detect(&img, &p);
        assert_eq!(t.len(), 1);
        assert!(diff180(t[0].angle_deg_to_freestream, 90.0) <= 3.0);
        // a 20 deg tuft against a free stream tilted by 20 deg (ccw) lies along it
        let img = scene(100, 100, &[(50.0, 50.0, 36.0, 20.0, 0.8)], 12);
        let (c, s) = (20f32.to_radians().cos(), 20f32.to_radians().sin());
        let p = DetectParams { freestream: (c, -s), ..DetectParams::default() };
        let t = detect(&img, &p);
        assert_eq!(t.len(), 1);
        assert!(t[0].angle_deg_to_freestream.abs() <= 3.0);
    }

    #[test]
    fn tuft_detect_classify_uses_named_thresholds() {
        assert_eq!(classify(0.0), TuftClass::Attached);
        assert_eq!(classify(ATTACHED_MAX_DEG - 0.1), TuftClass::Attached);
        assert_eq!(classify(-ATTACHED_MAX_DEG + 0.1), TuftClass::Attached);
        assert_eq!(classify(ATTACHED_MAX_DEG + 0.1), TuftClass::Separated);
        assert_eq!(classify(-75.0), TuftClass::Separated);
        assert_eq!(classify(150.0), TuftClass::Reversed);
    }

    #[test]
    fn tuft_detect_tiny_or_inconsistent_image_is_empty() {
        assert!(detect(&Gray::new(4, 4, 0), &DetectParams::default()).is_empty());
        let bad = Gray { w: 20, h: 20, px: vec![0; 5] };
        assert!(detect(&bad, &DetectParams::default()).is_empty());
    }
}
