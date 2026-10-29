//! Guarded unsigned-distance wrapping with a uniform-grid dual contour.
//! Independently implemented from Ju et al. (2002), section 2; local contour
//! cycles and the distance guard are our own conservative engineering choices.

use std::collections::HashMap;

use glam::{DMat3, DVec3};
use rayon::prelude::*;
use vernon_core::Geometry;

use crate::kdtree::KdTree;
use crate::trimesh::{SignMode, TriMesh};

pub(crate) struct ConservativeParams {
    pub alpha: f64,
    pub offset: f64,
    pub max_grid_nodes: usize,
}

pub(crate) struct ConservativeOutput {
    pub pos: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
    pub metrics: ConservativeMetrics,
}

#[derive(Default)]
pub(crate) struct ConservativeMetrics {
    pub alpha: f64,
    pub offset: f64,
    pub voxel_size: f64,
    pub barrier_radius: f64,
    pub grid_nodes: usize,
    pub band_nodes: usize,
    pub source_triangles: usize,
    pub source_points: usize,
    pub components: usize,
    pub open_edges: usize,
    pub nonmanifold_edges: usize,
    pub inconsistent_edges: usize,
    pub nonmanifold_vertices: usize,
    pub degenerate_triangles: usize,
    pub distance_min: f64,
    pub distance_max: f64,
    pub distance_mean: f64,
    pub below_offset: usize,
    pub above_upper_bound: usize,
    pub checked_vertices: usize,
    pub projection_moves: usize,
    pub refinement_retries: usize,
}

pub(crate) fn conservative_wrap(
    input: &Geometry,
    params: &ConservativeParams,
) -> Result<ConservativeOutput, String> {
    if input.points.is_empty() {
        return Err("conservative wrap requires source points".into());
    }
    if !params.alpha.is_finite()
        || params.alpha < 0.0
        || !params.offset.is_finite()
        || params.offset < 0.0
    {
        return Err("alpha and offset must be finite and nonnegative".into());
    }
    if input.points.iter().any(|p| !p.is_finite())
        || input
            .prims
            .iter()
            .any(|p| p.vertices.iter().any(|&i| i as usize >= input.points.len()))
    {
        return Err("source has nonfinite points or invalid indices".into());
    }
    let bounds = input.bounds();
    let span = bounds.max - bounds.min;
    if !span.is_finite() || !span.length_squared().is_finite() {
        return Err("source extent exceeds finite distance arithmetic".into());
    }
    let alpha = if params.alpha == 0.0 {
        span.length() / 20.0
    } else {
        params.alpha
    };
    let offset = if params.offset == 0.0 {
        alpha / 30.0
    } else {
        params.offset
    };
    if !alpha.is_finite() || alpha <= 0.0 || !offset.is_finite() || alpha + offset == f64::INFINITY
    {
        return Err("automatic alpha needs a finite nonzero source extent".into());
    }
    // Translation before the QEF avoids subtracting nearly equal large dot
    // products. Return positions are round-tripped before final certification.
    let origin = bounds.min + span * 0.5;
    if !origin.is_finite() {
        return Err("source center exceeds finite arithmetic".into());
    }
    // Reject impossible requests before cloning source arrays or building a BVH.
    let first_h = alpha / 8.0;
    Grid::new(
        bounds.min - origin,
        bounds.max - origin,
        first_h,
        offset + alpha * 0.5 + 3.0f64.sqrt() * first_h,
        params.max_grid_nodes,
    )?;
    let source = Source::new(input, origin)?;
    let mut last_error = String::new();
    for retry in 0..=2 {
        let h = alpha / (8.0 * (1usize << retry) as f64);
        let radius = offset + alpha * 0.5 + 3.0f64.sqrt() * h;
        let grid = Grid::new(
            bounds.min - origin,
            bounds.max - origin,
            h,
            radius,
            params.max_grid_nodes,
        )
        .map_err(|e| {
            if last_error.is_empty() {
                e
            } else {
                format!("{last_error}; refinement: {e}")
            }
        })?;
        let (state, band_nodes) = field(&source, &grid, radius)?;
        match contour(&source, &grid, radius, &state) {
            Ok((mut pos, tris, cells)) => {
                let mut metrics = certify(&source, &pos, &tris, DVec3::ZERO, alpha, offset, h);
                let mut projection_moves = 0;
                if accepted(&metrics) && !tris.is_empty() {
                    let baseline = pos.clone();
                    projection_moves = project(&source, &mut pos, &tris, &cells, alpha, offset, h);
                    for p in &mut pos {
                        *p += origin;
                    }
                    metrics = certify(&source, &pos, &tris, origin, alpha, offset, h);
                    if !accepted(&metrics) && projection_moves > 0 {
                        // Translation rounding is certified too. Keep the raw
                        // baseline if any projected position fails final checks.
                        pos = baseline.into_iter().map(|p| p + origin).collect();
                        metrics = certify(&source, &pos, &tris, origin, alpha, offset, h);
                        projection_moves = 0;
                    }
                }
                metrics.projection_moves = projection_moves;
                metrics.alpha = alpha;
                metrics.offset = offset;
                metrics.voxel_size = h;
                metrics.barrier_radius = radius;
                metrics.grid_nodes = grid.len;
                metrics.band_nodes = band_nodes;
                metrics.source_triangles = source.mesh.as_ref().map_or(0, |m| m.tris.len());
                metrics.source_points = source.points.len();
                metrics.refinement_retries = retry;
                if accepted(&metrics) && !tris.is_empty() {
                    return Ok(ConservativeOutput { pos, tris, metrics });
                }
                last_error = format!("uncertified contour: open={} nonmanifold_edges={} orientation={} vertex_links={} degenerate={} below={} above={}",
                    metrics.open_edges,metrics.nonmanifold_edges,metrics.inconsistent_edges,
                    metrics.nonmanifold_vertices,metrics.degenerate_triangles,metrics.below_offset,metrics.above_upper_bound);
            }
            Err(e) => last_error = e,
        }
    }
    Err(format!(
        "conservative contour could not be certified after bounded refinement: {last_error}"
    ))
}

fn accepted(m: &ConservativeMetrics) -> bool {
    m.open_edges
        + m.nonmanifold_edges
        + m.inconsistent_edges
        + m.nonmanifold_vertices
        + m.degenerate_triangles
        + m.below_offset
        + m.above_upper_bound
        == 0
}

struct Source {
    mesh: Option<TriMesh>,
    points: Vec<DVec3>,
    tree: KdTree,
}

impl Source {
    fn new(input: &Geometry, origin: DVec3) -> Result<Self, String> {
        if input.points.len() > u32::MAX as usize {
            return Err("source exceeds supported point count".into());
        }
        let pos: Vec<_> = input.points.iter().map(|p| *p - origin).collect();
        let mut used = vec![false; pos.len()];
        let mut tris = Vec::new();
        for (_, t) in input.triangles() {
            let [a, b, c] = t.map(|i| pos[i as usize]);
            let e1 = b - a;
            let e2 = c - a;
            let cross = e1.cross(e2);
            let area2 = cross.length_squared();
            if !area2.is_finite() {
                return Err("source triangle area exceeds finite arithmetic".into());
            }
            let scale = e1.abs().max(e2.abs()).max_element();
            let nonzero = cross != DVec3::ZERO
                || (scale > 0.0 && (e1 / scale).cross(e2 / scale) != DVec3::ZERO);
            if nonzero && area2 <= 1e-30 {
                return Err(
                    "nonzero source triangle is below supported distance-oracle area".into(),
                );
            }
            if !nonzero && (a != b || a != c) {
                // A collapsed closed face can still span a source segment.
                // Endpoint balls are not a conservative distance oracle for it.
                return Err("line-like source triangle needs unsupported segment distances".into());
            }
            // Same usable-face threshold as the existing nearest-triangle BVH.
            if area2 > 1e-30 {
                for &i in &t {
                    used[i as usize] = true;
                }
                tris.push(t);
            }
        }
        if tris.len() > u32::MAX as usize {
            return Err("source exceeds supported face count".into());
        }
        let points: Vec<_> = pos
            .iter()
            .zip(&used)
            .filter_map(|(&p, &u)| (!u).then_some(p))
            .collect();
        let tree = KdTree::new(&points);
        let mesh = if tris.is_empty() {
            None
        } else {
            let src = vec![0; tris.len()];
            Some(TriMesh::new(pos, tris, src))
        };
        Ok(Self { mesh, points, tree })
    }

    fn closest(&self, p: DVec3, cap2: f64) -> Option<(f64, DVec3)> {
        let mut best = self
            .mesh
            .as_ref()
            .and_then(|m| m.closest(p, cap2, SignMode::None))
            .map(|h| (h.dist, h.point));
        let limit = best.map_or(cap2, |h| h.0 * h.0);
        if let Some((i, d2)) = self.tree.nearest_within(p, limit) {
            best = Some((d2.sqrt(), self.points[i as usize]));
        }
        best
    }

    fn distance(&self, p: DVec3) -> f64 {
        self.closest(p, f64::INFINITY)
            .map_or(f64::INFINITY, |h| h.0)
    }
}

struct Grid {
    min: DVec3,
    h: f64,
    n: [usize; 3],
    stride: [usize; 3],
    len: usize,
}

impl Grid {
    fn new(min: DVec3, max: DVec3, h: f64, radius: f64, cap: usize) -> Result<Self, String> {
        if !h.is_finite() || h <= 0.0 || !radius.is_finite() || cap == 0 {
            return Err("grid parameters exceed finite resource limits".into());
        }
        let pad = radius + 3.0 * h;
        let lower = min - DVec3::splat(pad);
        let size = max - min + DVec3::splat(2.0 * pad);
        let mut n = [0; 3];
        let mut len = 1usize;
        for axis in 0..3 {
            let cells = (size[axis] / h).ceil();
            if !cells.is_finite() || cells < 1.0 || cells >= cap as f64 {
                return Err("requested grid exceeds node resource cap".into());
            }
            n[axis] = (cells as usize)
                .checked_add(1)
                .ok_or("grid dimension overflow")?;
            len = len.checked_mul(n[axis]).ok_or("grid product overflow")?;
            if len > cap || len > u32::MAX as usize {
                return Err("requested grid exceeds node resource cap".into());
            }
        }
        if !lower.is_finite() || !size.is_finite() {
            return Err("grid coordinates overflow".into());
        }
        Ok(Self {
            min: lower,
            h,
            n,
            stride: [1, n[0], n[0] * n[1]],
            len,
        })
    }
    fn index(&self, c: [usize; 3]) -> usize {
        c[0] + self.n[0] * (c[1] + self.n[1] * c[2])
    }
    fn coords(&self, i: usize) -> [usize; 3] {
        [i % self.n[0], i / self.n[0] % self.n[1], i / self.stride[2]]
    }
    fn point(&self, c: [usize; 3]) -> DVec3 {
        self.min + DVec3::new(c[0] as f64, c[1] as f64, c[2] as f64) * self.h
    }
    fn corners(&self, i: usize) -> [usize; 8] {
        std::array::from_fn(|v| {
            i + (v & 1) * self.stride[0]
                + ((v >> 1) & 1) * self.stride[1]
                + ((v >> 2) & 1) * self.stride[2]
        })
    }
    fn edge_key(&self, base: usize, edge: usize) -> usize {
        let (a, _, axis) = EDGES[edge];
        let node = base
            + (a & 1) * self.stride[0]
            + ((a >> 1) & 1) * self.stride[1]
            + ((a >> 2) & 1) * self.stride[2];
        node * 3 + axis
    }
}

// Vertex ids use x + 2*y + 4*z. Edge direction is always positive.
const EDGES: [(usize, usize, usize); 12] = [
    (0, 1, 0),
    (2, 3, 0),
    (4, 5, 0),
    (6, 7, 0),
    (0, 2, 1),
    (1, 3, 1),
    (4, 6, 1),
    (5, 7, 1),
    (0, 4, 2),
    (1, 5, 2),
    (2, 6, 2),
    (3, 7, 2),
];
// Face corners and edges are cyclic. Face center has one integer coordinate
// (axis, side); the other two coordinates have a half-cell displacement.
const FACES: [([usize; 4], [usize; 4], usize, usize); 6] = [
    ([0, 2, 6, 4], [4, 10, 6, 8], 0, 0),
    ([1, 3, 7, 5], [5, 11, 7, 9], 0, 1),
    ([0, 1, 5, 4], [0, 9, 2, 8], 1, 0),
    ([2, 3, 7, 6], [1, 11, 3, 10], 1, 1),
    ([0, 1, 3, 2], [0, 5, 1, 4], 2, 0),
    ([4, 5, 7, 6], [2, 7, 3, 6], 2, 1),
];

enum Tile {
    Free,
    Blocked,
    Band(Box<[u8; 512]>),
}

fn field(source: &Source, grid: &Grid, radius: f64) -> Result<(Vec<u8>, usize), String> {
    let tn = grid.n.map(|n| n.div_ceil(8));
    let nt = tn[0] * tn[1] * tn[2];
    let r2 = (radius + 2.0 * grid.h).powi(2);
    // Only tiles intersecting the guarded distance level query/store their 8^3
    // individual samples. Dense byte classifications support a bounded flood.
    let tiles: Vec<_> = (0..nt)
        .into_par_iter()
        .map(|i| {
            let tc = [i % tn[0], i / tn[0] % tn[1], i / (tn[0] * tn[1])];
            let lo = tc.map(|c| c * 8);
            let hi = std::array::from_fn::<_, 3, _>(|a| (lo[a] + 7).min(grid.n[a] - 1));
            let center = (grid.point(lo) + grid.point(hi)) * 0.5;
            let half = (grid.point(hi) - grid.point(lo)).length() * 0.5;
            let d = source.distance(center);
            if d - half > radius {
                return Tile::Free;
            }
            if d + half <= radius {
                return Tile::Blocked;
            }
            let mut values = Box::new([0u8; 512]);
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        let p = grid.point([x, y, z]);
                        values[(x - lo[0]) + 8 * ((y - lo[1]) + 8 * (z - lo[2]))] =
                            u8::from(source.closest(p, r2).is_some_and(|h| h.0 <= radius));
                    }
                }
            }
            Tile::Band(values)
        })
        .collect();
    let mut state = Vec::new();
    state
        .try_reserve_exact(grid.len)
        .map_err(|_| "grid classification allocation failed")?;
    state.resize(grid.len, 0u8);
    let mut band_nodes = 0;
    for (i, tile) in tiles.into_iter().enumerate() {
        if matches!(tile, Tile::Free) {
            continue;
        }
        let lo = [
            i % tn[0] * 8,
            i / tn[0] % tn[1] * 8,
            i / (tn[0] * tn[1]) * 8,
        ];
        for z in lo[2]..(lo[2] + 8).min(grid.n[2]) {
            for y in lo[1]..(lo[1] + 8).min(grid.n[1]) {
                for x in lo[0]..(lo[0] + 8).min(grid.n[0]) {
                    state[grid.index([x, y, z])] = match &tile {
                        Tile::Blocked => 1,
                        Tile::Band(v) => {
                            band_nodes += 1;
                            v[(x - lo[0]) + 8 * ((y - lo[1]) + 8 * (z - lo[2]))]
                        }
                        Tile::Free => 0,
                    };
                }
            }
        }
    }
    let mut queue = Vec::<u32>::new();
    queue
        .try_reserve_exact(grid.len)
        .map_err(|_| "outside flood allocation failed")?;
    for z in 0..grid.n[2] {
        for y in 0..grid.n[1] {
            for x in 0..grid.n[0] {
                if x == 0
                    || y == 0
                    || z == 0
                    || x + 1 == grid.n[0]
                    || y + 1 == grid.n[1]
                    || z + 1 == grid.n[2]
                {
                    let i = grid.index([x, y, z]);
                    if state[i] != 0 {
                        return Err("padded grid boundary is not exterior".into());
                    }
                    state[i] = 2;
                    queue.push(i as u32);
                }
            }
        }
    }
    let mut read = 0;
    while read < queue.len() {
        let i = queue[read] as usize;
        read += 1;
        let c = grid.coords(i);
        for a in 0..3 {
            for plus in [false, true] {
                if (!plus && c[a] == 0) || (plus && c[a] + 1 == grid.n[a]) {
                    continue;
                }
                let j = if plus {
                    i + grid.stride[a]
                } else {
                    i - grid.stride[a]
                };
                if state[j] == 0 {
                    state[j] = 2;
                    queue.push(j as u32);
                }
            }
        }
    }
    Ok((state, band_nodes))
}

#[derive(Clone, Copy, Default)]
struct Hermite {
    p: DVec3,
    n: DVec3,
}

fn crossing(source: &Source, grid: &Grid, radius: f64, key: usize) -> Result<Hermite, String> {
    let node = key / 3;
    let axis = key % 3;
    let a = grid.point(grid.coords(node));
    let b = a + DVec3::AXES[axis] * grid.h;
    let mut lo = a;
    let mut hi = b;
    let mut fl = source.distance(lo) - radius;
    let mut fh = source.distance(hi) - radius;
    if (fl > 0.0) == (fh > 0.0) {
        return Err("flood interface lacks an original-distance crossing".into());
    }
    // A safeguarded secant keeps an actual bracket at every step. The final
    // point stays on its primal edge even at an unsigned-distance medial cusp.
    for _ in 0..12 {
        let t = (fl / (fl - fh)).clamp(0.1, 0.9);
        let p = lo.lerp(hi, t);
        let f = source.distance(p) - radius;
        if (f > 0.0) == (fl > 0.0) {
            lo = p;
            fl = f;
        } else {
            hi = p;
            fh = f;
        }
    }
    let p = lo.lerp(hi, (fl / (fl - fh)).clamp(0.0, 1.0));
    let (_, nearest) = source
        .closest(p, f64::INFINITY)
        .ok_or("empty source distance oracle")?;
    let n = (p - nearest).normalize_or_zero();
    if !p.is_finite() || n.length_squared() < 0.5 {
        return Err("invalid Hermite crossing".into());
    }
    Ok(Hermite { p, n })
}

fn patch_cycles(sign: [bool; 8], centers: [bool; 6]) -> Result<Vec<Vec<usize>>, String> {
    let mut adj = [[usize::MAX; 2]; 12];
    let mut degree = [0usize; 12];
    for (fi, (corners, edges, _, _)) in FACES.iter().enumerate() {
        let crossing: Vec<_> = (0..4)
            .filter(|&i| sign[corners[i]] != sign[corners[(i + 1) % 4]])
            .collect();
        let pairs = match crossing.len() {
            0 => Vec::new(),
            2 => vec![(edges[crossing[0]], edges[crossing[1]])],
            4 => {
                if centers[fi] == sign[corners[0]] {
                    vec![(edges[0], edges[1]), (edges[2], edges[3])]
                } else {
                    vec![(edges[3], edges[0]), (edges[1], edges[2])]
                }
            }
            _ => return Err("invalid face crossing parity".into()),
        };
        for (a, b) in pairs {
            if degree[a] >= 2 || degree[b] >= 2 {
                return Err("invalid local contour degree".into());
            }
            adj[a][degree[a]] = b;
            degree[a] += 1;
            adj[b][degree[b]] = a;
            degree[b] += 1;
        }
    }
    let mut seen = [false; 12];
    let mut cycles = Vec::new();
    for start in 0..12 {
        if degree[start] == 0 || seen[start] {
            continue;
        }
        let mut cycle = Vec::new();
        let mut current = start;
        let mut previous = usize::MAX;
        loop {
            if degree[current] != 2 || (seen[current] && current != start) {
                return Err("local contour is not a simple cycle".into());
            }
            if current == start && !cycle.is_empty() {
                break;
            }
            seen[current] = true;
            cycle.push(current);
            let next = if adj[current][0] != previous {
                adj[current][0]
            } else {
                adj[current][1]
            };
            previous = current;
            current = next;
            if cycle.len() > 12 {
                return Err("local contour cycle overflow".into());
            }
        }
        if cycle.len() < 3 {
            return Err("local contour has fewer than three crossings".into());
        }
        cycles.push(cycle);
    }
    Ok(cycles)
}

fn qef(crossings: &[Hermite], lo: DVec3, h: f64) -> DVec3 {
    let mean = crossings.iter().map(|v| v.p).sum::<DVec3>() / crossings.len() as f64;
    let mut matrix = DMat3::ZERO;
    let mut rhs = DVec3::ZERO;
    for v in crossings {
        matrix += DMat3::from_cols(v.n * v.n.x, v.n * v.n.y, v.n * v.n.z);
        rhs += v.n * v.n.dot(v.p - mean);
    }
    // Regularization about the mass point makes the planar rank-deficient
    // case stable. The interior clamp also prevents coincident cell vertices.
    matrix += DMat3::IDENTITY * (0.01 * crossings.len() as f64);
    let candidate = mean + matrix.inverse() * rhs;
    let candidate = if candidate.is_finite() {
        candidate
    } else {
        mean
    };
    candidate.clamp(
        lo + DVec3::splat(h * 1e-6),
        lo + DVec3::splat(h * (1.0 - 1e-6)),
    )
}

fn contour(
    source: &Source,
    grid: &Grid,
    radius: f64,
    state: &[u8],
) -> Result<(Vec<DVec3>, Vec<[u32; 3]>, Vec<(DVec3, DVec3)>), String> {
    let mut active = Vec::new();
    let mut keys = Vec::new();
    for z in 0..grid.n[2] - 1 {
        for y in 0..grid.n[1] - 1 {
            for x in 0..grid.n[0] - 1 {
                let base = grid.index([x, y, z]);
                let corners = grid.corners(base);
                let sign = corners.map(|i| state[i] == 2);
                if sign.iter().all(|&s| s == sign[0]) {
                    continue;
                }
                active.push(base);
                for (e, &(a, b, _)) in EDGES.iter().enumerate() {
                    if sign[a] != sign[b] {
                        keys.push(grid.edge_key(base, e));
                    }
                }
            }
        }
    }
    keys.sort_unstable();
    keys.dedup();
    let hermite: Vec<_> = keys
        .par_iter()
        .map(|&key| crossing(source, grid, radius, key))
        .collect::<Result<_, _>>()?;
    let crossings: HashMap<_, _> = keys.iter().copied().zip(hermite).collect();
    let mut vertices = Vec::new();
    let mut cells = Vec::new();
    let mut cell_vertices = HashMap::<usize, [u32; 12]>::with_capacity(active.len());
    for &base in &active {
        let c = grid.coords(base);
        let corners = grid.corners(base);
        let sign = corners.map(|i| state[i] == 2);
        let mut centers = [false; 6];
        for (fi, (fc, _, axis, side)) in FACES.iter().enumerate() {
            if (0..4).all(|i| sign[fc[i]] != sign[fc[(i + 1) % 4]]) {
                // Canonical global face coordinates give identical choices
                // to both incident cells, including exact grid-aligned ties.
                let mut p = DVec3::new(c[0] as f64 + 0.5, c[1] as f64 + 0.5, c[2] as f64 + 0.5);
                p[*axis] = c[*axis] as f64 + *side as f64;
                centers[fi] = source.distance(grid.min + p * grid.h) > radius;
            }
        }
        let cycles = patch_cycles(sign, centers)?;
        let mut ids = [u32::MAX; 12];
        for cycle in cycles {
            if vertices.len() >= u32::MAX as usize {
                return Err("dual vertex count overflow".into());
            }
            let hs: Vec<_> = cycle
                .iter()
                .map(|&e| crossings[&grid.edge_key(base, e)])
                .collect();
            let id = vertices.len() as u32;
            vertices.push(qef(&hs, grid.point(c), grid.h));
            cells.push((grid.point(c), grid.point(c) + DVec3::splat(grid.h)));
            for e in cycle {
                ids[e] = id;
            }
        }
        cell_vertices.insert(base, ids);
    }
    let mut tris = Vec::with_capacity(keys.len() * 2);
    for key in keys {
        let node = key / 3;
        let axis = key % 3;
        let c = grid.coords(node);
        let (u, v) = match axis {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        if c[u] == 0 || c[v] == 0 {
            return Err("contour reaches padded grid boundary".into());
        }
        let mut cells = [c; 4];
        cells[0][u] -= 1;
        cells[0][v] -= 1;
        cells[1][v] -= 1;
        cells[3][u] -= 1;
        let mut quad = [0u32; 4];
        for i in 0..4 {
            let cell = grid.index(cells[i]);
            let local = (0..12)
                .find(|&e| grid.edge_key(cell, e) == key)
                .ok_or("dual edge/cell mismatch")?;
            quad[i] = cell_vertices
                .get(&cell)
                .ok_or("missing incident dual patch")?[local];
            if quad[i] == u32::MAX {
                return Err("crossing missing from local patch".into());
            }
        }
        if state[node] == 2 {
            quad.reverse();
        }
        let a = [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]];
        let b = [[quad[0], quad[1], quad[3]], [quad[1], quad[2], quad[3]]];
        let quality = |ts: [[u32; 3]; 2]| {
            ts.iter()
                .map(|t| {
                    let [a, b, c] = t.map(|i| vertices[i as usize]);
                    (b - a).cross(c - a).length_squared()
                })
                .fold(f64::INFINITY, f64::min)
        };
        tris.extend(if quality(a) >= quality(b) { a } else { b });
    }
    Ok((vertices, tris, cells))
}

fn project(
    source: &Source,
    pos: &mut [DVec3],
    tris: &[[u32; 3]],
    cells: &[(DVec3, DVec3)],
    alpha: f64,
    offset: f64,
    h: f64,
) -> usize {
    let target = offset + alpha * 0.25;
    let guard = alpha * 1e-9;
    let mut incident = vec![Vec::new(); pos.len()];
    let normals: Vec<_> = tris
        .iter()
        .enumerate()
        .map(|(i, t)| {
            for &v in t {
                incident[v as usize].push(i);
            }
            let [a, b, c] = t.map(|v| pos[v as usize]);
            (b - a).cross(c - a)
        })
        .collect();
    let mut moved = 0;
    for v in 0..pos.len() {
        let current = pos[v];
        let Some((distance, nearest)) = source.closest(current, f64::INFINITY) else {
            continue;
        };
        if distance <= target || distance <= 0.0 {
            continue;
        }
        let step = (nearest - current) / distance * (distance - target).min(h * 0.25);
        let (lo, hi) = cells[v];
        for backtrack in 0..8 {
            let candidate = (current + step * 0.5f64.powi(backtrack))
                .clamp(lo + DVec3::splat(h * 1e-6), hi - DVec3::splat(h * 1e-6));
            if !candidate.is_finite() || candidate.distance(current) <= h * 1e-8 {
                continue;
            }
            let d = source.distance(candidate);
            if d < offset + guard || d > offset + alpha - guard || d >= distance - h * 1e-8 {
                continue;
            }
            let valid = incident[v].iter().all(|&ti| {
                let [a, b, c] = tris[ti].map(|i| {
                    if i as usize == v {
                        candidate
                    } else {
                        pos[i as usize]
                    }
                });
                let normal = (b - a).cross(c - a);
                let area2 = normal.length_squared();
                area2.is_finite()
                    && area2 > h.powi(4) * 1e-20
                    && normal.dot(normals[ti]) > 0.1 * (area2 * normals[ti].length_squared()).sqrt()
            });
            if valid {
                pos[v] = candidate;
                moved += 1;
                break;
            }
        }
    }
    moved
}

fn root(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        parent[i as usize] = parent[parent[i as usize] as usize];
        i = parent[i as usize];
    }
    i
}

fn certify(
    source: &Source,
    pos: &[DVec3],
    tris: &[[u32; 3]],
    origin: DVec3,
    alpha: f64,
    offset: f64,
    h: f64,
) -> ConservativeMetrics {
    let mut m = ConservativeMetrics {
        distance_min: f64::INFINITY,
        distance_max: 0.0,
        ..Default::default()
    };
    let distances: Vec<_> = pos
        .par_iter()
        .map(|&p| source.distance(p - origin))
        .collect();
    for d in distances {
        m.checked_vertices += 1;
        m.distance_min = m.distance_min.min(d);
        m.distance_max = m.distance_max.max(d);
        m.distance_mean += d;
        if !d.is_finite() || d < offset {
            m.below_offset += 1;
        }
        if !d.is_finite() || d > offset + alpha {
            m.above_upper_bound += 1;
        }
    }
    m.distance_mean /= m.checked_vertices.max(1) as f64;
    let mut edges = Vec::with_capacity(tris.len() * 3);
    let mut links = Vec::with_capacity(tris.len() * 3);
    let mut faces = Vec::with_capacity(tris.len());
    let mut parent: Vec<_> = (0..pos.len() as u32).collect();
    let mut used = vec![false; pos.len()];
    for t in tris {
        let [a, b, c] = t.map(|i| pos[i as usize]);
        let area = (b - a).cross(c - a).length_squared();
        if !area.is_finite()
            || area <= h.powi(4) * 1e-20
            || t[0] == t[1]
            || t[1] == t[2]
            || t[2] == t[0]
        {
            m.degenerate_triangles += 1;
        }
        let mut face = *t;
        face.sort_unstable();
        faces.push(face);
        for k in 0..3 {
            let (v, a, b) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
            used[v as usize] = true;
            edges.push((v.min(a), v.max(a), v < a));
            links.push((v, a, b));
            let rv = root(&mut parent, v);
            let ra = root(&mut parent, a);
            parent[rv as usize] = ra;
        }
    }
    faces.sort_unstable();
    m.degenerate_triangles += faces.windows(2).filter(|w| w[0] == w[1]).count();
    edges.sort_unstable();
    let mut i = 0;
    while i < edges.len() {
        let mut end = i + 1;
        while end < edges.len() && (edges[end].0, edges[end].1) == (edges[i].0, edges[i].1) {
            end += 1;
        }
        match end - i {
            1 => m.open_edges += 1,
            2 => {
                if edges[i].2 == edges[i + 1].2 {
                    m.inconsistent_edges += 1;
                }
            }
            _ => m.nonmanifold_edges += 1,
        }
        i = end;
    }
    links.sort_unstable();
    let mut i = 0;
    while i < links.len() {
        let v = links[i].0;
        let mut end = i + 1;
        while end < links.len() && links[end].0 == v {
            end += 1;
        }
        let mut adjacency = Vec::<(u32, Vec<u32>)>::new();
        for &(_, a, b) in &links[i..end] {
            for (x, y) in [(a, b), (b, a)] {
                if let Some((_, neighbors)) = adjacency.iter_mut().find(|entry| entry.0 == x) {
                    neighbors.push(y);
                } else {
                    adjacency.push((x, vec![y]));
                }
            }
        }
        let mut valid = adjacency.iter().all(|(_, a)| a.len() == 2);
        if valid {
            let mut seen = vec![false; adjacency.len()];
            let mut pending = vec![0];
            while let Some(j) = pending.pop() {
                if seen[j] {
                    continue;
                }
                seen[j] = true;
                for neighbor in &adjacency[j].1 {
                    if let Some(k) = adjacency.iter().position(|entry| entry.0 == *neighbor) {
                        pending.push(k);
                    }
                }
            }
            valid = seen.into_iter().all(|s| s);
        }
        if !valid {
            m.nonmanifold_vertices += 1;
        }
        i = end;
    }
    let mut components = Vec::new();
    for v in 0..pos.len() {
        if used[v] {
            components.push(root(&mut parent, v as u32));
        } else {
            m.nonmanifold_vertices += 1;
        }
    }
    components.sort_unstable();
    components.dedup();
    m.components = components.len();
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{sphere_geo, uv_sphere};
    use std::collections::{BTreeMap, BTreeSet};

    fn params(alpha: f64, offset: f64) -> ConservativeParams {
        ConservativeParams {
            alpha,
            offset,
            max_grid_nodes: 40_000_000,
        }
    }

    // Deliberately independent of the kernel certifier: brute source distance,
    // ordered incidence maps and explicit neighbor walks for every vertex link.
    fn independently_certify(input: &Geometry, out: &ConservativeOutput) {
        let faces = input.triangles();
        let mut used = vec![false; input.points.len()];
        let usable: Vec<_> = faces
            .iter()
            .filter_map(|(_, t)| {
                let p = t.map(|i| input.points[i as usize]);
                if (p[1] - p[0]).cross(p[2] - p[0]).length_squared() > 0.0 {
                    for &i in t {
                        used[i as usize] = true;
                    }
                    Some(p)
                } else {
                    None
                }
            })
            .collect();
        let mut edges: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
        let mut links: Vec<BTreeMap<u32, Vec<u32>>> = vec![BTreeMap::new(); out.pos.len()];
        let mut unique = BTreeSet::new();
        for t in &out.tris {
            let p = t.map(|v| out.pos[v as usize]);
            assert!((p[1] - p[0]).cross(p[2] - p[0]).length_squared() > 1e-24);
            let mut key = *t;
            key.sort_unstable();
            assert!(unique.insert(key));
            for k in 0..3 {
                let (v, a, b) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                edges.entry((v.min(a), v.max(a))).or_default().push((v, a));
                links[v as usize].entry(a).or_default().push(b);
                links[v as usize].entry(b).or_default().push(a);
            }
        }
        for incident in edges.values() {
            assert_eq!(incident.len(), 2);
            assert_eq!(incident[0], (incident[1].1, incident[1].0));
        }
        for link in links {
            assert!(!link.is_empty());
            assert!(link.values().all(|adj| adj.len() == 2));
            let start = *link.keys().next().unwrap();
            let mut visited = BTreeSet::new();
            let mut pending = vec![start];
            while let Some(v) = pending.pop() {
                if visited.insert(v) {
                    pending.extend(&link[&v]);
                }
            }
            assert_eq!(visited.len(), link.len());
        }
        for &p in &out.pos {
            let mut d2 = f64::INFINITY;
            for t in &usable {
                let q = crate::trimesh::closest_point_triangle(p, t[0], t[1], t[2]).0;
                d2 = d2.min(p.distance_squared(q));
            }
            for (i, &q) in input.points.iter().enumerate() {
                if !used[i] {
                    d2 = d2.min(p.distance_squared(q));
                }
            }
            let d = d2.sqrt();
            assert!(
                d >= out.metrics.offset && d <= out.metrics.offset + out.metrics.alpha,
                "independent distance {d} outside [{},{}]",
                out.metrics.offset,
                out.metrics.offset + out.metrics.alpha
            );
        }
    }

    fn plate(z: f64) -> Geometry {
        Geometry::from_polygons(
            vec![
                DVec3::new(-5., -5., z),
                DVec3::new(5., -5., z),
                DVec3::new(5., 5., z),
                DVec3::new(-5., 5., z),
            ],
            vec![vec![0, 1, 2], vec![0, 2, 3]],
        )
    }

    fn merged(a: &Geometry, b: &Geometry) -> Geometry {
        let mut g = a.clone();
        let n = g.points.len() as u32;
        g.points.extend(&b.points);
        g.prims.extend(b.prims.iter().map(|p| vernon_core::Prim {
            vertices: p.vertices.iter().map(|i| i + n).collect(),
            closed: p.closed,
        }));
        g
    }

    fn opened_sphere(diameter: f64) -> Geometry {
        let (nu, nv) = (32usize, 20usize);
        let r = 30.0;
        let theta = (diameter / (2.0 * r)).asin();
        let mut points = Vec::new();
        for j in 0..nv {
            let t = theta + (std::f64::consts::PI - theta) * j as f64 / nv as f64;
            for i in 0..nu {
                let ph = std::f64::consts::TAU * i as f64 / nu as f64;
                points.push(DVec3::new(
                    r * t.sin() * ph.cos(),
                    r * t.sin() * ph.sin(),
                    r * t.cos(),
                ));
            }
        }
        let south = points.len() as u32;
        points.push(DVec3::new(0., 0., -r));
        let id = |j: usize, i: usize| (j * nu + i % nu) as u32;
        let mut faces = Vec::new();
        for j in 0..nv - 1 {
            for i in 0..nu {
                faces.push(vec![id(j, i), id(j + 1, i), id(j + 1, i + 1)]);
                faces.push(vec![id(j, i), id(j + 1, i + 1), id(j, i + 1)]);
            }
        }
        for i in 0..nu {
            faces.push(vec![south, id(nv - 1, i + 1), id(nv - 1, i)]);
        }
        Geometry::from_polygons(points, faces)
    }

    fn ray_inside(out: &ConservativeOutput, p: DVec3) -> bool {
        let d = DVec3::new(0.137, 0.083, 1.0).normalize();
        let mut hits = Vec::new();
        for t in &out.tris {
            let [a, b, c] = t.map(|i| out.pos[i as usize]);
            let e1 = b - a;
            let e2 = c - a;
            let h = d.cross(e2);
            let det = e1.dot(h);
            if det.abs() < 1e-12 {
                continue;
            }
            let f = 1. / det;
            let s = p - a;
            let u = f * s.dot(h);
            if !(0. ..=1.).contains(&u) {
                continue;
            }
            let q = s.cross(e1);
            let v = f * d.dot(q);
            if v < 0. || u + v > 1. {
                continue;
            }
            let distance = f * e2.dot(q);
            if distance > 1e-8 {
                hits.push(distance);
            }
        }
        hits.sort_by(f64::total_cmp);
        hits.dedup_by(|a, b| (*a - *b).abs() < 1e-8);
        hits.len() % 2 == 1
    }

    #[test]
    fn wp6_small_aperture_closes_large_aperture_stays_accessible() {
        let small = opened_sphere(0.8 * 6.);
        let large = opened_sphere(4. * 6.);
        let a = conservative_wrap(&small, &params(6., 0.2)).unwrap();
        let b = conservative_wrap(&large, &params(6., 0.2)).unwrap();
        assert!(
            ray_inside(&a, DVec3::ZERO),
            "measured 4.8 mm aperture must close"
        );
        assert!(
            !ray_inside(&b, DVec3::ZERO),
            "24 mm aperture must admit exterior access"
        );
        assert_eq!(a.metrics.components, 1);
        assert_eq!(b.metrics.components, 1);
        independently_certify(&small, &a);
        independently_certify(&large, &b);
        eprintln!("WP6 aperture source diameters 4.8 / 24 mm at alpha=6 mm; center inside small=true, large=false");
    }

    #[test]
    fn wp6_face_ambiguity_cycles_and_exact_grid_ties() {
        // Enumerate all cube corner patterns, including checkerboard faces.
        // Every active crossing belongs to exactly one simple local cycle.
        for mask in 1..255u32 {
            let sign = std::array::from_fn(|i| mask & (1 << i) != 0);
            for center in [false, true] {
                let cycles = patch_cycles(sign, [center; 6]).unwrap();
                let mut edges: Vec<_> = cycles.into_iter().flatten().collect();
                edges.sort_unstable();
                let expected: Vec<_> = EDGES
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &(a, b, _))| (sign[a] != sign[b]).then_some(i))
                    .collect();
                assert_eq!(edges, expected, "corner mask {mask}, center {center}");
            }
        }
        let opposite = [true, false, false, false, false, false, false, true];
        assert_eq!(patch_cycles(opposite, [false; 6]).unwrap().len(), 2);
        // A sphere whose radius crosses exact grid nodes exercises zero ties;
        // diagonal touching points exercise near-saddle local patch decisions.
        for delta in [-1e-9, 0., 1e-9] {
            let input = Geometry::from_points(vec![
                DVec3::new(-1., -1., 0.),
                DVec3::new(1. + delta, 1., 0.),
            ]);
            let out = conservative_wrap(&input, &params(2., 0.01)).unwrap();
            independently_certify(&input, &out);
        }
        let input = Geometry::from_points(vec![DVec3::ZERO]);
        // This production choice makes R/h=6 exactly and places the source
        // on grid nodes. A degenerate first extraction must refine or reject.
        let production = conservative_wrap(&input, &params(8., 2. - 3.0f64.sqrt())).unwrap();
        independently_certify(&input, &production);
        let source = Source::new(&input, DVec3::ZERO).unwrap();
        let grid = Grid {
            min: DVec3::splat(-3.),
            h: 1.,
            n: [7; 3],
            stride: [1, 7, 49],
            len: 343,
        };
        let (state, _) = field(&source, &grid, 1.).unwrap();
        assert_eq!(state[grid.index([4, 3, 3])], 1, "d==R is blocked");
        let (pos, tris, _) = contour(&source, &grid, 1., &state).unwrap();
        let m = certify(&source, &pos, &tris, DVec3::ZERO, 2., 0.01, 1.);
        assert_eq!(
            m.open_edges + m.nonmanifold_edges + m.inconsistent_edges + m.nonmanifold_vertices,
            0
        );
        assert!(
            m.degenerate_triangles > 0,
            "coarse R=1,h=1 bypasses the production guard and must reject near-collapsed triangles"
        );
    }

    #[test]
    fn wp6_sphere_closed_certified_distance_band() {
        let input = sphere_geo(30.0, 16, 8);
        let out = conservative_wrap(&input, &params(6.0, 0.2)).expect("valid sphere wrap");
        assert!(!out.tris.is_empty());
        assert_eq!(out.metrics.components, 1);
        assert_eq!(
            out.metrics.open_edges
                + out.metrics.nonmanifold_edges
                + out.metrics.inconsistent_edges
                + out.metrics.nonmanifold_vertices
                + out.metrics.degenerate_triangles,
            0
        );
        assert_eq!(out.metrics.checked_vertices, out.pos.len());
        assert!(
            out.metrics.projection_moves > 0,
            "sphere must attempt admissible bounded shrink projection"
        );
        assert!(out.metrics.distance_min >= 0.2);
        assert!(out.metrics.distance_max <= 6.2);
        assert!(
            out.metrics.distance_mean >= out.metrics.distance_min
                && out.metrics.distance_mean <= out.metrics.distance_max
        );
        independently_certify(&input, &out);
        let e = out.tris.len() * 3 / 2;
        assert_eq!(
            out.pos.len() as isize - e as isize + out.tris.len() as isize,
            2
        );
        assert!(out.tris.iter().all(|t| {
            let [a, b, c] = t.map(|i| out.pos[i as usize]);
            (b - a).cross(c - a).dot((a + b + c) / 3.) > 0.
        }));
    }

    #[test]
    fn wp6_open_plate_winding_points_orphans_and_determinism() {
        let input = plate(0.);
        let out = conservative_wrap(&input, &params(3., 0.01)).unwrap();
        independently_certify(&input, &out);
        let mut reverse = input.clone();
        for p in &mut reverse.prims {
            p.vertices.reverse();
        }
        let again = conservative_wrap(&reverse, &params(3., 0.01)).unwrap();
        assert_eq!(out.pos.len(), again.pos.len());
        assert!(out
            .pos
            .iter()
            .zip(&again.pos)
            .all(|(a, b)| a.distance(*b) < 1e-12));
        independently_certify(&reverse, &again);
        let repeated = conservative_wrap(&input, &params(3., 0.01)).unwrap();
        assert!(out.pos == repeated.pos, "same source gives exact positions");
        assert!(
            out.tris == repeated.tris,
            "same source gives exact connectivity"
        );
        let mut soup = Geometry::default();
        for p in &input.prims {
            let n = soup.points.len() as u32;
            soup.points
                .extend(p.vertices.iter().map(|&i| input.points[i as usize]));
            soup.prims
                .push(vernon_core::Prim::polygon(vec![n, n + 1, n + 2]));
            soup.prims
                .push(vernon_core::Prim::polygon(vec![n + 2, n + 1, n]));
        }
        soup.prims.push(vernon_core::Prim::polygon(vec![0, 0, 0]));
        soup.points.push(DVec3::new(20., 0., 0.));
        let wrapped = conservative_wrap(&soup, &params(3., 0.01)).unwrap();
        assert_eq!(wrapped.metrics.source_points, 1);
        assert_eq!(wrapped.metrics.components, 2);
        independently_certify(&soup, &wrapped);
        let point = Geometry::from_points(vec![DVec3::ZERO]);
        independently_certify(
            &point,
            &conservative_wrap(&point, &params(2., 1e-6)).unwrap(),
        );
        let (points, _) = uv_sphere(5., 12, 6);
        let cloud = Geometry::from_points(points);
        independently_certify(
            &cloud,
            &conservative_wrap(&cloud, &params(3., 0.01)).unwrap(),
        );
    }

    #[test]
    fn wp6_component_separation_and_automatic_translation() {
        let close = merged(&plate(0.), &plate(2.4));
        let far = merged(&plate(0.), &plate(12.));
        let a = conservative_wrap(&close, &params(3., 0.1)).unwrap();
        let b = conservative_wrap(&far, &params(3., 0.1)).unwrap();
        assert_eq!(a.metrics.components, 1);
        assert_eq!(b.metrics.components, 2);
        independently_certify(&close, &a);
        independently_certify(&far, &b);
        let mut sphere = sphere_geo(6., 12, 6);
        let automatic = conservative_wrap(&sphere, &params(0., 0.)).unwrap();
        assert!((automatic.metrics.offset - automatic.metrics.alpha / 30.).abs() < 1e-14);
        independently_certify(&sphere, &automatic);
        for p in &mut sphere.points {
            *p += DVec3::new(1e6, -2e6, 3e6);
        }
        independently_certify(
            &sphere,
            &conservative_wrap(&sphere, &params(3., 0.)).unwrap(),
        );
    }

    #[test]
    fn wp6_rejects_invalid_parameters_and_bounded_grid() {
        assert!(conservative_wrap(&Geometry::default(), &params(1., 0.)).is_err());
        let mut g = plate(0.);
        for p in [
            params(-1., 0.),
            params(f64::NAN, 0.),
            params(1., -1.),
            params(1., f64::INFINITY),
            ConservativeParams {
                alpha: 1e-200,
                offset: 1.,
                max_grid_nodes: 40_000_000,
            },
            ConservativeParams {
                alpha: 1.,
                offset: 0.,
                max_grid_nodes: 8,
            },
        ] {
            assert!(conservative_wrap(&g, &p).is_err());
        }
        g.points[0].x = f64::NAN;
        assert!(conservative_wrap(&g, &params(1., 0.)).is_err());
        assert!(
            conservative_wrap(&Geometry::from_points(vec![DVec3::ZERO]), &params(0., 0.)).is_err()
        );
    }

    #[test]
    fn wp6_rejects_finite_source_numeric_overflow() {
        let triangle = Geometry::from_polygons(
            vec![
                DVec3::ZERO,
                DVec3::new(1e100, 0., 0.),
                DVec3::new(0., 1e100, 0.),
            ],
            vec![vec![0, 1, 2]],
        );
        assert!(
            Source::new(&triangle, DVec3::ZERO).is_err(),
            "unrepresentable triangle area must reject before acceleration"
        );
        let extent = Geometry::from_points(vec![DVec3::splat(-1e308), DVec3::splat(1e308)]);
        assert!(conservative_wrap(&extent, &params(1., 0.)).is_err());
        let tiny = Geometry::from_polygons(
            vec![
                DVec3::ZERO,
                DVec3::new(1e-8, 0., 0.),
                DVec3::new(0., 1e-8, 0.),
            ],
            vec![vec![0, 1, 2]],
        );
        assert!(
            conservative_wrap(&tiny, &params(2e-9, 1e-10)).is_err(),
            "a nonzero triangle below BVH's supported area cannot become just three points"
        );
        for face in [vec![0, 1, 2], vec![0, 1, 1]] {
            let line = Geometry::from_polygons(
                vec![
                    DVec3::ZERO,
                    DVec3::new(10., 0., 0.),
                    DVec3::new(20., 0., 0.),
                ],
                vec![face],
            );
            assert!(
                conservative_wrap(&line, &params(2., 0.2)).is_err(),
                "a line-like closed triangle cannot silently become endpoint balls"
            );
        }
    }

    #[test]
    fn wp6_projection_fallback_retains_certified_baseline() {
        let input = Geometry::from_points(vec![DVec3::ZERO]);
        let out = conservative_wrap(&input, &params(4., 0.01)).unwrap();
        let source = Source::new(&input, DVec3::ZERO).unwrap();
        let h = out.metrics.voxel_size;
        let mut positions = out.pos.clone();
        // These valid cells constrain each vertex against the direction of
        // its source point. No sourceward step is admissible inside the cell.
        let cells: Vec<_> = positions
            .iter()
            .map(|&p| {
                let mut lo = p - DVec3::splat(h * 0.5);
                let mut hi = lo + DVec3::splat(h);
                for a in 0..3 {
                    if p[a] >= 0. {
                        lo[a] = p[a] - h * 1e-6;
                        hi[a] = lo[a] + h;
                    } else {
                        hi[a] = p[a] + h * 1e-6;
                        lo[a] = hi[a] - h;
                    }
                }
                (lo, hi)
            })
            .collect();
        assert_eq!(
            project(&source, &mut positions, &out.tris, &cells, 4., 0.01, h),
            0
        );
        assert!(
            positions == out.pos,
            "failed projection must retain exact certified baseline"
        );
        independently_certify(&input, &out);
    }

    #[test]
    #[ignore = "release benchmark: exact one-million-triangle source"]
    fn wp6_release_million_triangles() {
        if cfg!(debug_assertions) {
            panic!("benchmark requires --release");
        }
        let input = sphere_geo(30., 1000, 501);
        assert_eq!(input.triangles().len(), 1_000_000);
        let timer = std::time::Instant::now();
        let out = conservative_wrap(&input, &params(0., 0.)).expect("million triangle wrap");
        let elapsed = timer.elapsed();
        let m = &out.metrics;
        eprintln!("WP6 full-kernel elapsed={:.6}s source_triangles={} source_points={} output_vertices={} output_triangles={} h={} R={} grid_nodes={} band_nodes={} distance_min={} distance_max={} checked_vertices={} below={} above={} topology={}/{}/{}/{}/{} retries={} projection_moves={}",
            elapsed.as_secs_f64(),m.source_triangles,m.source_points,out.pos.len(),out.tris.len(),m.voxel_size,m.barrier_radius,m.grid_nodes,m.band_nodes,
            m.distance_min,m.distance_max,m.checked_vertices,m.below_offset,m.above_upper_bound,m.open_edges,m.nonmanifold_edges,m.inconsistent_edges,m.nonmanifold_vertices,m.degenerate_triangles,m.refinement_retries,m.projection_moves);
        assert_eq!(m.source_triangles, 1_000_000);
        assert_eq!(m.checked_vertices, out.pos.len());
        assert_eq!(
            m.below_offset
                + m.above_upper_bound
                + m.open_edges
                + m.nonmanifold_edges
                + m.inconsistent_edges
                + m.nonmanifold_vertices
                + m.degenerate_triangles,
            0
        );
        assert!(elapsed.as_secs_f64() < 30., "full kernel took {elapsed:?}");
    }
}
