//! Deterministic beam search for a split plan of a closed mesh.
//!
//! Input: a closed [`TriMesh`], a build volume, candidate cut polylines (closed or open
//! paths on the surface) and an optional per-triangle visibility mask. Output: the subset of
//! candidates, applied in increasing candidate order, whose pieces all fit the build volume
//! in some axis-aligned orientation, at minimum cost.
//!
//! Cost (lower is better), documented so callers can reason about trade-offs:
//! ```text
//! cost = w.parts   * (number of parts)
//!      + w.seam    * (sum of seam lengths of the used candidates, mm)
//!      + w.visible * (sum of seam length lying on visible triangles, mm)
//! ```
//! Defaults: `parts = 1000`, `seam = 1`, `visible = 4`, so one extra part costs as much as
//! 1 m of seam, and a millimetre of visible seam costs five times a hidden one.
//! A seam's length and visible length are measured once, on the original mesh, by walking
//! each candidate across it (the seam of a candidate is the same whichever piece it cuts).
//!
//! Search: level k holds partial plans with k cuts. Every plan is extended by each candidate
//! of larger index (cuts of disjoint seams commute, so each subset is visited once); children
//! that split nothing are dropped. A level keeps the `beam_width` plans with the fewest
//! non-fitting parts, then lowest cost, then a tie-break key drawn from a seeded xorshift64*
//! generator in creation order, so a given seed always returns the same plan. The best fully
//! fitting plan seen at any level is returned. Candidates are assumed to be independent seams
//! (they must not cross each other); a candidate that does not lie on a piece is skipped for
//! that piece. The beam may miss the optimum when `beam_width` is smaller than the number of
//! promising partial plans; it never returns an infeasible plan.
use crate::polyline_project::{
    cut_along_path, path_segments, project_polyline, PathError, TriMesh,
};
use glam::DVec3;
use std::fmt;

/// A candidate seam.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub points: Vec<DVec3>,
    pub closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    pub parts: f64,
    pub seam: f64,
    pub visible: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Weights { parts: 1000.0, seam: 1.0, visible: 4.0 }
    }
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Build volume (x, y, z) in mm.
    pub build: DVec3,
    pub weights: Weights,
    pub beam_width: usize,
    /// Maximum number of cuts in a plan.
    pub max_cuts: usize,
    pub seed: u64,
    /// Distance within which candidate points must lie on a piece's surface.
    pub surface_tol: f64,
}

impl SearchOptions {
    pub fn new(build: DVec3) -> Self {
        SearchOptions { build, weights: Weights::default(), beam_width: 8, max_cuts: 8, seed: 1, surface_tol: 1e-6 }
    }
}

#[derive(Debug, Clone)]
pub struct SplitPlan {
    /// Indices into the candidate list, ascending.
    pub cuts: Vec<usize>,
    pub parts: Vec<TriMesh>,
    pub seam_mm: f64,
    pub visible_seam_mm: f64,
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SearchError {
    BadBuildVolume,
    OpenMesh,
    BadVisibleMask,
    BadCandidate { index: usize, error: PathError },
    NoFeasiblePlan,
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchError::BadBuildVolume => write!(f, "build volume must be positive and finite"),
            SearchError::OpenMesh => write!(f, "mesh must be closed and consistently wound"),
            SearchError::BadVisibleMask => write!(f, "visible mask length differs from triangle count"),
            SearchError::BadCandidate { index, error } => write!(f, "candidate {index}: {error}"),
            SearchError::NoFeasiblePlan => write!(f, "no plan makes every part fit the build volume"),
        }
    }
}
impl std::error::Error for SearchError {}

/// Does an axis-aligned box of size `dims` fit the build volume in some axis permutation?
/// (For boxes this is equivalent to comparing the sorted extents.)
pub fn fits_build(dims: DVec3, build: DVec3) -> bool {
    let mut d = [dims.x, dims.y, dims.z];
    let mut b = [build.x, build.y, build.z];
    d.sort_by(|a, b| a.total_cmp(b));
    b.sort_by(|a, b| a.total_cmp(b));
    d.iter().zip(&b).all(|(d, b)| *d <= *b + 1e-9)
}

fn part_fits(m: &TriMesh, build: DVec3) -> bool {
    m.bounds().map_or(true, |(lo, hi)| fits_build(hi - lo, build))
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

struct Prepared {
    seam: f64,
    visible: f64,
}

#[derive(Clone)]
struct State {
    parts: Vec<TriMesh>,
    cuts: Vec<usize>,
    seam: f64,
    visible: f64,
    unfit: usize,
    cost: f64,
    key: u64,
}

fn cost_of(parts: usize, seam: f64, visible: f64, w: &Weights) -> f64 {
    w.parts * parts as f64 + w.seam * seam + w.visible * visible
}

/// Apply one candidate to every part it lies on. `None` when it splits nothing.
fn apply(parts: &[TriMesh], c: &Candidate, tol: f64) -> Option<Vec<TriMesh>> {
    let mut out = Vec::new();
    let mut split_any = false;
    for p in parts {
        let cut = project_polyline(p, &c.points, c.closed, tol)
            .and_then(|path| cut_along_path(p, &path, c.closed, true));
        match cut {
            Ok(pieces) => {
                split_any = true;
                out.extend(pieces);
            }
            // Not on this piece (or not separating it): leave the piece alone.
            Err(_) => out.push(p.clone()),
        }
    }
    split_any.then_some(out)
}

fn is_closed(m: &TriMesh) -> bool {
    m.is_watertight()
}

/// Find the cheapest plan whose parts all fit `opts.build`.
pub fn search(
    mesh: &TriMesh,
    visible: &[bool],
    candidates: &[Candidate],
    opts: &SearchOptions,
) -> Result<SplitPlan, SearchError> {
    let b = opts.build;
    if !(b.is_finite() && b.min_element() > 0.0) {
        return Err(SearchError::BadBuildVolume);
    }
    if !is_closed(mesh) {
        return Err(SearchError::OpenMesh);
    }
    if !visible.is_empty() && visible.len() != mesh.tris.len() {
        return Err(SearchError::BadVisibleMask);
    }
    let w = opts.weights;
    let mut rng = Rng(opts.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);

    let root_unfit = usize::from(!part_fits(mesh, b));
    let root = State {
        parts: vec![mesh.clone()],
        cuts: Vec::new(),
        seam: 0.0,
        visible: 0.0,
        unfit: root_unfit,
        cost: cost_of(1, 0.0, 0.0, &w),
        key: rng.next(),
    };
    let finish = |s: State| SplitPlan {
        cuts: s.cuts,
        parts: s.parts,
        seam_mm: s.seam,
        visible_seam_mm: s.visible,
        cost: s.cost,
    };
    if root.unfit == 0 {
        return Ok(finish(root));
    }

    let mut prepared = Vec::with_capacity(candidates.len());
    for (index, c) in candidates.iter().enumerate() {
        let path = project_polyline(mesh, &c.points, c.closed, opts.surface_tol)
            .map_err(|error| SearchError::BadCandidate { index, error })?;
        let (mut seam, mut vis) = (0.0, 0.0);
        for (len, tris) in path_segments(mesh, &path, c.closed) {
            seam += len;
            if tris.iter().any(|&t| visible.get(t).copied().unwrap_or(false)) {
                vis += len;
            }
        }
        prepared.push(Prepared { seam, visible: vis });
    }

    let mut best: Option<State> = None;
    let mut frontier = vec![(root, 0usize)]; // (state, first candidate index allowed)
    for _depth in 0..opts.max_cuts {
        let mut children: Vec<(State, usize)> = Vec::new();
        for (s, first) in &frontier {
            for ci in *first..candidates.len() {
                let Some(parts) = apply(&s.parts, &candidates[ci], opts.surface_tol) else { continue };
                let (seam, vis) = (s.seam + prepared[ci].seam, s.visible + prepared[ci].visible);
                let unfit = parts.iter().filter(|p| !part_fits(p, b)).count();
                let mut cuts = s.cuts.clone();
                cuts.push(ci);
                let child = State {
                    cost: cost_of(parts.len(), seam, vis, &w),
                    parts,
                    cuts,
                    seam,
                    visible: vis,
                    unfit,
                    key: rng.next(),
                };
                if child.unfit == 0 && best.as_ref().map_or(true, |bs| child.cost < bs.cost) {
                    best = Some(child.clone());
                }
                children.push((child, ci + 1));
            }
        }
        if children.is_empty() {
            break;
        }
        children.sort_by(|a, b| {
            a.0.unfit
                .cmp(&b.0.unfit)
                .then(a.0.cost.total_cmp(&b.0.cost))
                .then(a.0.key.cmp(&b.0.key))
        });
        children.truncate(opts.beam_width.max(1));
        frontier = children;
    }
    best.map(finish).ok_or(SearchError::NoFeasiblePlan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polyline_project::testmesh::box_mesh;

    /// Closed rectangular loop around a box (cross-section at x = `x`).
    fn ring_x(x: f64, w: f64, h: f64) -> Candidate {
        Candidate {
            points: vec![DVec3::new(x, 0.0, 0.0), DVec3::new(x, w, 0.0), DVec3::new(x, w, h), DVec3::new(x, 0.0, h)],
            closed: true,
        }
    }

    fn extent_x(p: &TriMesh) -> f64 {
        let (lo, hi) = p.bounds().unwrap();
        hi.x - lo.x
    }

    #[test]
    fn partition_search_fitting_part_needs_no_cuts() {
        let m = box_mesh(30.0, 10.0, 10.0);
        let plan = search(&m, &[], &[ring_x(15.0, 10.0, 10.0)], &SearchOptions::new(DVec3::splat(40.0))).unwrap();
        assert!(plan.cuts.is_empty());
        assert_eq!(plan.parts.len(), 1);
    }

    #[test]
    fn partition_search_long_box_gets_minimum_parts() {
        // 100 mm box, 40 mm build: two cuts (three parts) are the minimum.
        let m = box_mesh(100.0, 10.0, 10.0);
        let cands: Vec<_> = [20.0, 40.0, 50.0, 60.0, 80.0].iter().map(|&x| ring_x(x, 10.0, 10.0)).collect();
        let plan = search(&m, &[], &cands, &SearchOptions::new(DVec3::splat(40.0))).unwrap();
        assert_eq!(plan.parts.len(), 3);
        assert_eq!(plan.cuts.len(), 2);
        for p in &plan.parts {
            assert!(p.is_watertight());
            assert!(extent_x(p) <= 40.0 + 1e-9, "{}", extent_x(p));
        }
        let vol: f64 = plan.parts.iter().map(|p| p.signed_volume()).sum();
        assert!((vol - 10_000.0).abs() < 1e-6, "{vol}");
        // Each seam is a 10x10 perimeter = 40 mm.
        assert!((plan.seam_mm - 80.0).abs() < 1e-6, "{}", plan.seam_mm);
        assert!((plan.cost - (3000.0 + 80.0)).abs() < 1e-6);
    }

    #[test]
    fn partition_search_is_deterministic_for_a_seed() {
        let m = box_mesh(100.0, 10.0, 10.0);
        let cands: Vec<_> = [20.0, 40.0, 50.0, 60.0, 80.0].iter().map(|&x| ring_x(x, 10.0, 10.0)).collect();
        let a = search(&m, &[], &cands, &SearchOptions::new(DVec3::splat(40.0))).unwrap();
        let b = search(&m, &[], &cands, &SearchOptions::new(DVec3::splat(40.0))).unwrap();
        assert_eq!(a.cuts, b.cuts);
    }

    #[test]
    fn partition_search_visible_seams_are_avoided() {
        // 60 mm box, 40 mm build: a single cut at x=20 or x=40 works. The loop crosses the
        // front face's lower triangle (index 0) for z < 10*x/60: 3.33 mm at x=20, 6.67 mm at
        // x=40. Marking that triangle visible makes x=20 cheaper although x=40 is listed first.
        let m = box_mesh(60.0, 10.0, 10.0);
        let cands = vec![ring_x(40.0, 10.0, 10.0), ring_x(20.0, 10.0, 10.0)];
        let mut vis = vec![false; m.tris.len()];
        vis[0] = true;
        let plan = search(&m, &vis, &cands, &SearchOptions::new(DVec3::splat(40.0))).unwrap();
        assert_eq!(plan.cuts, vec![1]);
        assert!((plan.visible_seam_mm - 10.0 / 3.0).abs() < 1e-6, "{}", plan.visible_seam_mm);
        // Without the mask the first (x=40) candidate wins the tie.
        let plain = search(&m, &[], &cands, &SearchOptions::new(DVec3::splat(40.0))).unwrap();
        assert_eq!(plain.parts.len(), 2);
    }

    #[test]
    fn partition_search_rotation_lets_a_long_part_fit() {
        assert!(fits_build(DVec3::new(10.0, 90.0, 10.0), DVec3::new(100.0, 20.0, 20.0)));
        assert!(!fits_build(DVec3::new(10.0, 90.0, 30.0), DVec3::new(100.0, 20.0, 20.0)));
        let m = box_mesh(90.0, 10.0, 10.0);
        let plan = search(&m, &[], &[], &SearchOptions::new(DVec3::new(20.0, 100.0, 20.0))).unwrap();
        assert!(plan.cuts.is_empty());
    }

    #[test]
    fn partition_search_reports_errors() {
        let m = box_mesh(100.0, 10.0, 10.0);
        let o = SearchOptions::new(DVec3::splat(40.0));
        assert_eq!(search(&m, &[], &[], &o).unwrap_err(), SearchError::NoFeasiblePlan);
        assert_eq!(
            search(&m, &[], &[], &SearchOptions::new(DVec3::new(0.0, 1.0, 1.0))).unwrap_err(),
            SearchError::BadBuildVolume
        );
        let mut open = m.clone();
        open.tris.pop();
        assert_eq!(search(&open, &[], &[], &o).unwrap_err(), SearchError::OpenMesh);
        assert_eq!(search(&m, &[true], &[], &o).unwrap_err(), SearchError::BadVisibleMask);
        let far = Candidate { points: vec![DVec3::new(500.0, 0.0, 0.0), DVec3::new(500.0, 5.0, 0.0)], closed: false };
        assert!(matches!(search(&m, &[], &[far], &o), Err(SearchError::BadCandidate { index: 0, .. })));
    }

    #[test]
    fn partition_search_unreachable_fit_is_infeasible() {
        // Cuts only along x cannot shrink a 100 mm wide part below a 30 mm build in y/z.
        let m = box_mesh(40.0, 100.0, 100.0);
        let cands = vec![ring_x(20.0, 100.0, 100.0)];
        let e = search(&m, &[], &cands, &SearchOptions::new(DVec3::splat(30.0))).unwrap_err();
        assert_eq!(e, SearchError::NoFeasiblePlan);
    }
}
