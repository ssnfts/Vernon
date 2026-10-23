//! Home grid (spec 2.5 and 3.9): adaptive spacing, line lists, edge fade.

/// Home grid settings. Spacing in mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSettings {
    pub spacing: f64,
    pub major_every: i64,
    /// Perspective grid half-length in grid squares.
    pub extent: i64,
    pub adaptive: bool,
    /// Stop subdividing below `spacing` when zooming in.
    pub inhibit_subdivision: bool,
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            spacing: 10.0,
            major_every: 10,
            extent: 7,
            adaptive: true,
            inhibit_subdivision: true,
        }
    }
}

/// Grow or shrink the spacing by powers of ten so a square stays 8-80 px.
pub fn adaptive_spacing(base: f64, world_per_px: f64, inhibit: bool) -> f64 {
    let mut s = base;
    if !(world_per_px.is_finite() && world_per_px > 0.0 && base > 0.0) {
        return base;
    }
    for _ in 0..40 {
        if s / world_per_px < 8.0 {
            s *= 10.0;
        } else {
            break;
        }
    }
    let floor = if inhibit { base } else { base / 1000.0 };
    for _ in 0..40 {
        if s / world_per_px > 80.0 && s > floor * (1.0 + 1e-9) {
            s /= 10.0;
        } else {
            break;
        }
    }
    s
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridLine {
    /// Line index: the line lies at `k * spacing`.
    pub k: i64,
    pub major: bool,
}

/// Lines `kmin..=kmax`; every `major_every`-th line (and the axis) is major.
pub fn lines(kmin: i64, kmax: i64, major_every: i64) -> Vec<GridLine> {
    let n = major_every.max(2);
    (kmin..=kmax)
        .map(|k| GridLine { k, major: k % n == 0 })
        .collect()
}

/// Lines of a grid of half-length `extent` squares.
pub fn extent_lines(extent: i64, major_every: i64) -> Vec<GridLine> {
    lines(-extent, extent, major_every)
}

/// Opacity of line `k`: 1 inside 80 % of the extent, fading to 0 at the edge.
pub fn fade(k: i64, extent: i64) -> f32 {
    let e = extent.max(1) as f32;
    let a = k.unsigned_abs() as f32 / e;
    if a <= 0.8 {
        1.0
    } else {
        ((1.0 - a) / 0.2).clamp(0.0, 1.0)
    }
}

/// Short text for the status bar, e.g. `10 mm` or `1 m`.
pub fn format_spacing(mm: f64) -> String {
    if mm >= 1000.0 {
        format!("{} m", trim(mm / 1000.0))
    } else {
        format!("{} mm", trim(mm))
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_line_counts() {
        let l = extent_lines(7, 10);
        assert_eq!(l.len(), 15);
        assert_eq!(l.iter().filter(|g| g.major).count(), 1);
        let l = extent_lines(10, 10);
        assert_eq!(l.len(), 21);
        let majors: Vec<i64> = l.iter().filter(|g| g.major).map(|g| g.k * 100).collect();
        assert_eq!(majors, vec![-1000, 0, 1000]);
    }

    #[test]
    fn golden_adaptive() {
        assert_eq!(adaptive_spacing(10.0, 5.0, true), 100.0);
        assert_eq!(adaptive_spacing(10.0, 0.5, true), 10.0);
        // Inhibit stops at the base; without it the grid keeps subdividing.
        assert_eq!(adaptive_spacing(10.0, 0.001, true), 10.0);
        assert!(adaptive_spacing(10.0, 0.001, false) < 10.0);
    }

    #[test]
    fn fade_and_format() {
        assert_eq!(fade(0, 10), 1.0);
        assert_eq!(fade(10, 10), 0.0);
        assert!(fade(9, 10) > 0.0 && fade(9, 10) < 1.0);
        assert_eq!(format_spacing(10.0), "10 mm");
        assert_eq!(format_spacing(2500.0), "2.5 m");
    }
}
