//! Geometry preparation and domain checks for the DoMINO surrogate.
//!
//! Every number in the "checkpoint contract" block comes from the installed
//! `nvidia/domino_drivaerml` checkpoint (`config.yaml` and `scaling_factors.pkl`,
//! revision recorded in the download manifest). The tolerances in the checks are
//! Vernon's own conservative heuristics, not NVIDIA specifications.

use std::collections::HashMap;

use glam::DVec3;
use vernon_core::report::Severity;
use vernon_core::Geometry;

// ---- checkpoint contract (metres, +x is the flow direction, z up) ----
/// `data.bounding_box_surface` of the checkpoint config.
pub const SURFACE_BOX_MIN: DVec3 = DVec3::new(-1.5, -1.4, -0.32);
pub const SURFACE_BOX_MAX: DVec3 = DVec3::new(5.0, 1.4, 1.4);
/// `data.bounding_box` (volume domain) of the checkpoint config.
pub const VOLUME_BOX_MIN: DVec3 = DVec3::new(-3.5, -2.25, -0.32);
pub const VOLUME_BOX_MAX: DVec3 = DVec3::new(8.5, 2.25, 3.0);
/// Extreme surface coordinates seen in training (`stl_centers` min/max of the scaling factors).
pub const TRAIN_MIN: DVec3 = DVec3::new(-0.942573, -1.1293833, -0.31963632);
pub const TRAIN_MAX: DVec3 = DVec3::new(4.1327467, 1.1293833, 1.2305567);
/// The only operating point in the checkpoint's `variables.global_parameters`.
pub const INLET_SPEED_MPS: f64 = 30.0;
pub const AIR_DENSITY: f64 = 1.205;
/// Largest surface mesh verified to run on a 16 GB GPU (19,800 triangles, 6.35 GB peak); 33,540 failed. The model builds all-pairs distances, so memory grows with the square of the triangle count.
pub const MAX_TRIANGLES: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Use the coordinates as they are (mm in the scene, converted to metres).
    AsIs,
    /// Translate only: nose to the training frame's front, centred on y = 0, wheels on the training ground plane.
    SnapToTrainingFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// Inside the training coordinate span and not obviously small. Still not validated.
    WithinTrainingSpan,
    /// Inside the checkpoint's bounding box but outside what it saw in training.
    OutOfDistribution,
    /// Outside the checkpoint's bounding box: inference is refused.
    Outside,
}

impl Domain {
    pub fn label(self) -> &'static str {
        match self {
            Domain::WithinTrainingSpan => "within training span (not validated)",
            Domain::OutOfDistribution => "OUT OF TRAINING DISTRIBUTION",
            Domain::Outside => "OUTSIDE MODEL DOMAIN",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub id: &'static str,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PrepOptions {
    pub placement: Placement,
    /// Run even though the geometry is outside the checkpoint's bounding box (results are labelled out of domain).
    pub allow_outside: bool,
    pub max_triangles: usize,
}

impl Default for PrepOptions {
    fn default() -> Self {
        PrepOptions { placement: Placement::AsIs, allow_outside: false, max_triangles: MAX_TRIANGLES }
    }
}

#[derive(Debug, Clone)]
pub struct Prepared {
    /// Triangles in the model frame, metres. Order follows `Geometry::triangles()`.
    pub tris: Vec<[DVec3; 3]>,
    /// Index of the source primitive of each triangle.
    pub tri_prim: Vec<usize>,
    /// Added to (scene mm x 0.001) to reach the model frame.
    pub offset_m: DVec3,
    pub min_m: DVec3,
    pub max_m: DVec3,
    pub domain: Domain,
    pub checks: Vec<Check>,
    pub open_edges: usize,
    pub non_manifold_edges: usize,
    pub degenerate: usize,
    pub components: usize,
}

impl Prepared {
    pub fn extents_m(&self) -> DVec3 {
        self.max_m - self.min_m
    }

    /// First blocking problem, if inference must not run.
    pub fn blocking(&self) -> Option<&Check> {
        self.checks.iter().find(|c| c.severity == Severity::Error)
    }

    /// Scene millimetres of a model-frame point (metres).
    pub fn to_scene_mm(&self, p_m: DVec3) -> DVec3 {
        (p_m - self.offset_m) * 1000.0
    }
}

fn weld_key(p: DVec3) -> [i64; 3] {
    let q = 1e6; // 1 micrometre in metres
    [(p.x * q).round() as i64, (p.y * q).round() as i64, (p.z * q).round() as i64]
}

/// Edge statistics of a triangle soup: (open edges, non-manifold edges, connected components).
pub fn topology(tris: &[[DVec3; 3]]) -> (usize, usize, usize) {
    let mut ids: HashMap<[i64; 3], usize> = HashMap::new();
    let mut tri_ids: Vec<[usize; 3]> = Vec::with_capacity(tris.len());
    for t in tris {
        let mut r = [0usize; 3];
        for (k, p) in t.iter().enumerate() {
            let n = ids.len();
            r[k] = *ids.entry(weld_key(*p)).or_insert(n);
        }
        tri_ids.push(r);
    }
    let mut edges: HashMap<(usize, usize), u32> = HashMap::new();
    let mut parent: Vec<usize> = (0..ids.len()).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for r in &tri_ids {
        for k in 0..3 {
            let (a, b) = (r[k], r[(k + 1) % 3]);
            if a == b {
                continue;
            }
            *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent[ra] = rb;
            }
        }
    }
    let open = edges.values().filter(|&&c| c == 1).count();
    let nm = edges.values().filter(|&&c| c > 2).count();
    let used: std::collections::HashSet<usize> = tri_ids.iter().flatten().copied().collect();
    let comps: std::collections::HashSet<usize> = used.iter().map(|&v| find(&mut parent, v)).collect();
    (open, nm, comps.len())
}

/// Prepare `geo` (scene millimetres) for DoMINO. Never runs the model.
pub fn prepare(geo: &Geometry, opts: &PrepOptions) -> Result<Prepared, String> {
    let raw = geo.triangles();
    if raw.len() < 8 {
        return Err(format!("need at least 8 triangles, the geometry has {}", raw.len()));
    }
    if raw.len() > opts.max_triangles {
        return Err(format!(
            "{} triangles exceed the {} the surface model has been checked with; decimate the mesh first",
            raw.len(),
            opts.max_triangles
        ));
    }
    let mut tris = Vec::with_capacity(raw.len());
    let mut tri_prim = Vec::with_capacity(raw.len());
    for (pi, t) in &raw {
        let mut tri = [DVec3::ZERO; 3];
        for k in 0..3 {
            let p = geo.points.get(t[k] as usize).copied().ok_or("triangle refers to a missing point")?;
            if !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite()) {
                return Err("geometry has non-finite coordinates".into());
            }
            tri[k] = p * 0.001;
        }
        tris.push(tri);
        tri_prim.push(*pi);
    }
    let bounds = |tris: &[[DVec3; 3]]| {
        let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for t in tris {
            for p in t {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
        }
        (lo, hi)
    };
    let (lo0, hi0) = bounds(&tris);
    let offset = match opts.placement {
        Placement::AsIs => DVec3::ZERO,
        Placement::SnapToTrainingFrame => {
            DVec3::new(TRAIN_MIN.x - lo0.x, -(lo0.y + hi0.y) * 0.5, TRAIN_MIN.z - lo0.z)
        }
    };
    if offset != DVec3::ZERO {
        for t in &mut tris {
            for p in t.iter_mut() {
                *p += offset;
            }
        }
    }
    let (lo, hi) = bounds(&tris);
    let ext = hi - lo;
    let degenerate = tris.iter().filter(|t| (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5 < 1e-10).count();
    let (open, nm, comps) = topology(&tris);
    let mut checks: Vec<Check> = Vec::new();
    let mut push = |id: &'static str, severity: Severity, message: String| checks.push(Check { id, severity, message });

    // Domain, from the checkpoint's own bounding box and training span.
    let outside_box = lo.cmplt(SURFACE_BOX_MIN).any() || hi.cmpgt(SURFACE_BOX_MAX).any();
    let tol = DVec3::splat(0.02);
    let outside_train = lo.cmplt(TRAIN_MIN - tol).any() || hi.cmpgt(TRAIN_MAX + tol).any();
    let train_ext = TRAIN_MAX - TRAIN_MIN;
    let small = (ext / train_ext).min_element() < 0.7;
    let domain = if outside_box {
        Domain::Outside
    } else if outside_train || small {
        Domain::OutOfDistribution
    } else {
        Domain::WithinTrainingSpan
    };
    match domain {
        Domain::Outside => {
            let sev = if opts.allow_outside { Severity::Warning } else { Severity::Error };
            push(
                "outside_domain",
                sev,
                format!(
                    "geometry spans x {:.2}..{:.2} m, y {:.2}..{:.2} m, z {:.2}..{:.2} m, outside the checkpoint's surface box x {}..{}, y {}..{}, z {}..{} m (flow is +x, z up, nose toward -x). {}",
                    lo.x, hi.x, lo.y, hi.y, lo.z, hi.z,
                    SURFACE_BOX_MIN.x, SURFACE_BOX_MAX.x, SURFACE_BOX_MIN.y, SURFACE_BOX_MAX.y, SURFACE_BOX_MIN.z, SURFACE_BOX_MAX.z,
                    if opts.allow_outside { "Running anyway because 'allow outside domain' is on; the result is not meaningful." } else { "Use 'snap to training frame' or move/scale the geometry, or turn on 'allow outside domain' to run anyway." }
                ),
            );
        }
        Domain::OutOfDistribution => push(
            "out_of_distribution",
            Severity::Warning,
            format!(
                "geometry (extent {:.2} x {:.2} x {:.2} m) is outside or much smaller than the car bodies the checkpoint was trained on ({:.2} x {:.2} x {:.2} m); predictions are extrapolation",
                ext.x, ext.y, ext.z, train_ext.x, train_ext.y, train_ext.z
            ),
        ),
        Domain::WithinTrainingSpan => push(
            "within_span",
            Severity::Info,
            "geometry is inside the training coordinate span; the model was only trained on 500 morphed variants of one notchback car (DrivAerML), so other shapes can still be out of distribution".into(),
        ),
    }
    if !(ext.x >= ext.y && ext.x >= ext.z) {
        push(
            "axis_order",
            Severity::Warning,
            "the x extent is not the longest; the model expects a car with its length along the flow axis +x".into(),
        );
    }
    if open > 0 {
        push(
            "not_watertight",
            Severity::Warning,
            format!("{open} open edge(s): the mesh is not watertight; the signed-distance inputs and interior masking are unreliable"),
        );
    }
    if nm > 0 {
        push("non_manifold", Severity::Warning, format!("{nm} non-manifold edge(s)"));
    }
    if degenerate > 0 {
        push("degenerate", Severity::Warning, format!("{degenerate} degenerate triangle(s) (area < 1e-10 m2)"));
    }
    if comps > 1 {
        push("components", Severity::Info, format!("{comps} separate pieces; the model expects one car body"));
    }
    Ok(Prepared { tris, tri_prim, offset_m: offset, min_m: lo, max_m: hi, domain, checks, open_edges: open, non_manifold_edges: nm, degenerate, components: comps })
}

/// Binary STL of the prepared triangles (metres, float32), the format the bridge loads.
pub fn stl_bytes(tris: &[[DVec3; 3]]) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    out.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for t in tris {
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        for v in [n, t[0], t[1], t[2]] {
            for c in [v.x, v.y, v.z] {
                out.extend_from_slice(&(c as f32).to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

/// Ray-parity test along +y; valid for watertight meshes only.
pub fn inside_mesh(tris: &[[DVec3; 3]], p: DVec3) -> bool {
    let mut hits = 0u32;
    for t in tris {
        // Project to the xz plane and test whether (p.x, p.z) is inside the triangle, then the y of the hit.
        let (a, b, c) = (t[0], t[1], t[2]);
        let d = (b.z - c.z) * (a.x - c.x) + (c.x - b.x) * (a.z - c.z);
        if d.abs() < 1e-14 {
            continue;
        }
        let l1 = ((b.z - c.z) * (p.x - c.x) + (c.x - b.x) * (p.z - c.z)) / d;
        let l2 = ((c.z - a.z) * (p.x - c.x) + (a.x - c.x) * (p.z - c.z)) / d;
        let l3 = 1.0 - l1 - l2;
        // half-open rule avoids double counting shared edges
        let eps = 1e-12;
        if l1 >= -eps && l2 >= -eps && l3 >= -eps && (l1 > eps || l2 > eps || l3 > eps) {
            let y = l1 * a.y + l2 * b.y + l3 * c.y;
            if y > p.y {
                hits += 1;
            }
        }
    }
    hits % 2 == 1
}

/// Query points of a y = const slice through the volume domain, in the model frame.
/// Returns the kept points and, for each, its index in the full `nx * nz` grid;
/// points inside a watertight body are dropped.
pub fn slice_points(
    prep: &Prepared,
    y_m: f64,
    x_range: (f64, f64),
    z_range: (f64, f64),
    nx: usize,
    nz: usize,
) -> (Vec<DVec3>, Vec<usize>, bool) {
    let mask = prep.open_edges == 0 && prep.non_manifold_edges == 0;
    let mut pts = Vec::new();
    let mut idx = Vec::new();
    for k in 0..nz {
        for i in 0..nx {
            let fx = if nx > 1 { i as f64 / (nx - 1) as f64 } else { 0.5 };
            let fz = if nz > 1 { k as f64 / (nz - 1) as f64 } else { 0.5 };
            let p = DVec3::new(x_range.0 + fx * (x_range.1 - x_range.0), y_m, z_range.0 + fz * (z_range.1 - z_range.0));
            let near = p.cmpge(prep.min_m).all() && p.cmple(prep.max_m).all();
            if mask && near && inside_mesh(&prep.tris, p) {
                continue;
            }
            pts.push(p);
            idx.push(k * nx + i);
        }
    }
    (pts, idx, mask)
}
