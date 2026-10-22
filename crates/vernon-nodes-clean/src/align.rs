//! Alignment nodes: pick-pair alignment, automatic principal-axes
//! pre-alignment, ICP refinement (point-to-plane or point-to-point, with
//! trimming and rejection) and a node that applies a stored transform.

use glam::{DMat3, DVec3};
use rayon::prelude::*;
use serde_json::{json, Value};
use vernon_core::{AttribArray, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::kdtree::KdTree;
use crate::linalg::{eigen3, fit_rigid, rodrigues, solve_pinv, Mat, MAXN};
use crate::trimesh::{SignMode, TriMesh};
use crate::util::{
    add, axes_mask, ensure_metric, json_in, json_transform, json_vec3, matrix4, partial_shuffle, pct, pf, pi,
    point_normals, transform_json, Rng, SPEC_ALIGN,
};

pub fn register(reg: &mut Registry) {
    add(reg, pair_info, pair_cook);
    add(reg, prealign_info, prealign_cook);
    add(reg, icp_info, icp_cook);
    add(reg, apply_info, apply_cook);
}

// -------------------------------------------------------------------- pose

/// Similarity transform `x -> s * R * x + t`.
#[derive(Debug, Clone, Copy)]
pub struct Pose {
    pub r: DMat3,
    pub s: f64,
    pub t: DVec3,
}

impl Pose {
    pub const IDENTITY: Pose = Pose {
        r: DMat3::IDENTITY,
        s: 1.0,
        t: DVec3::ZERO,
    };

    pub fn apply(&self, p: DVec3) -> DVec3 {
        self.s * (self.r * p) + self.t
    }

    /// `other` applied after `self`.
    pub fn then(&self, other: &Pose) -> Pose {
        Pose {
            r: other.r * self.r,
            s: other.s * self.s,
            t: other.s * (other.r * self.t) + other.t,
        }
    }

    pub fn inverse(&self) -> Pose {
        let ri = self.r.transpose();
        let si = 1.0 / self.s;
        Pose {
            r: ri,
            s: si,
            t: -(si * (ri * self.t)),
        }
    }

    /// The same transform expressed about a different origin: if `self` acts
    /// on coordinates relative to `c`, the result acts on absolute ones.
    pub fn uncentred(&self, c: DVec3) -> Pose {
        Pose {
            r: self.r,
            s: self.s,
            t: self.t + c - self.s * (self.r * c),
        }
    }

    pub fn from_json(v: &Value) -> Option<Pose> {
        json_transform(v).map(|(r, t, s)| Pose { r, s, t })
    }

    pub fn to_json(&self, kind: &str) -> Value {
        transform_json(kind, self.r, self.t, self.s)
    }

    pub fn apply_to(&self, g: &Geometry) -> Geometry {
        let mut o = g.clone();
        o.transform(matrix4(self.r, self.t, self.s));
        o
    }
}

fn check_scale(ctx: &CookContext, s: f64) -> CookResult<()> {
    if !(s.is_finite() && s > 0.0) {
        return Err(ctx.fail(format!("transform scale {s} must be positive and finite")));
    }
    Ok(())
}

fn geo_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

fn xf_out() -> PortSpec {
    PortSpec::new("transform", DataKind::Json)
}

fn rep_out() -> PortSpec {
    PortSpec::new("report", DataKind::Report)
}

// -------------------------------------------------------------- pair align

fn pair_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.pair_align", "Pick-Pair Align", Stage::Clean)
        .input(PortSpec::new("moving", DataKind::Geometry))
        .input(PortSpec::optional("reference", DataKind::Geometry))
        .input(PortSpec::new("pairs", DataKind::Json))
        .output(geo_out())
        .output(xf_out())
        .output(rep_out())
        .param(
            ParamSpec::bool("fix_scale", "Fix scale", true)
                .help("On: rigid fit (scale stays 1). Off: also estimate a uniform scale."),
        )
        .param(ParamSpec::int("min_pairs", "Minimum pairs", 3, 3, 64).help("The node refuses to run with fewer point pairs."))
        .param(
            ParamSpec::float("rms_limit", "RMS limit", 0.0, 0.0, 10000.0)
                .unit("mm")
                .help("If above 0, the report gates on the fit RMS being at most this value."),
        )
        .help("Aligns the moving geometry to the reference from matching point pairs (list of [moving, reference] points) with a closed-form least-squares fit. Reports RMS and the error of each pair.")
        .emulates("point-pair picking alignment (closed-form rigid fit; optional uniform scale)")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn parse_pairs(v: &Value) -> Result<(Vec<DVec3>, Vec<DVec3>), String> {
    let arr = match v {
        Value::Array(a) => a,
        Value::Object(o) => o
            .get("pairs")
            .and_then(|p| p.as_array())
            .ok_or("pairs object needs a 'pairs' array")?,
        _ => return Err("pairs must be a JSON array".into()),
    };
    let (mut p, mut q) = (Vec::new(), Vec::new());
    for (i, e) in arr.iter().enumerate() {
        let (a, b) = if let Some(pr) = e.as_array() {
            if pr.len() != 2 {
                return Err(format!("pair {i}: expected [moving, reference]"));
            }
            (json_vec3(&pr[0]), json_vec3(&pr[1]))
        } else if e.is_object() {
            let get = |ks: &[&str]| ks.iter().find_map(|k| e.get(*k)).and_then(json_vec3);
            (get(&["moving", "p", "from"]), get(&["reference", "q", "to"]))
        } else {
            (None, None)
        };
        match (a, b) {
            (Some(a), Some(b)) if a.is_finite() && b.is_finite() => {
                p.push(a);
                q.push(b);
            }
            _ => return Err(format!("pair {i} is not two finite [x, y, z] points")),
        }
    }
    Ok((p, q))
}

fn pair_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    ensure_metric(ctx, &g)?;
    let pairs = json_in(inp, 2, "pairs")?;
    let fix_scale = p.bool("fix_scale")?;
    let min_pairs = pi(p, "min_pairs", 3, 64)? as usize;
    let rms_limit = pf(p, "rms_limit", 0.0, 10000.0)?;
    let (pa, qa) = parse_pairs(&pairs).map_err(|e| ctx.fail(e))?;
    if pa.len() < min_pairs {
        return Err(ctx.fail(format!("need at least {min_pairs} point pairs, got {}", pa.len())));
    }
    let fit = fit_rigid(&pa, &qa, None, !fix_scale).map_err(|e| ctx.fail(e.to_string()))?;
    check_scale(ctx, fit.s)?;
    let pose = Pose {
        r: fit.r,
        s: fit.s,
        t: fit.t,
    };
    let errs: Vec<f64> = pa.iter().zip(&qa).map(|(a, b)| (pose.apply(*a) - *b).length()).collect();
    let mut rep = Report::new("Pick-Pair Align");
    let mut m = Metric::new(fit.rms, "mm");
    if rms_limit > 0.0 {
        m = m.with_limit(rms_limit, true);
    }
    rep.metric("rms", m);
    rep.metric("max_pair_error", Metric::new(errs.iter().cloned().fold(0.0, f64::max), "mm"));
    rep.metric("pairs", Metric::new(pa.len() as f64, "pairs"));
    rep.metric("scale", Metric::new(fit.s, ""));
    if pa.len() == 3 {
        rep.note(Severity::Info, "exactly three pairs: the fit has no redundancy to expose a bad pick");
    }
    let mut xf = pose.to_json("pair_align");
    xf["pair_errors"] = json!(errs);
    xf["rms"] = json!(fit.rms);
    Ok(vec![Data::geometry(pose.apply_to(&g)), Data::json(xf), Data::report(rep)])
}

// ------------------------------------------------------------- pre-align

fn prealign_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.auto_prealign", "Auto Pre-Align", Stage::Clean)
        .input(PortSpec::new("moving", DataKind::Geometry))
        .input(PortSpec::new("reference", DataKind::Geometry))
        .output(geo_out())
        .output(xf_out())
        .output(rep_out())
        .param(
            ParamSpec::choice(
                "method",
                "Method",
                "principal_axes",
                &["principal_axes", "centroid", "bbox_center"],
            )
            .help("principal_axes matches the centroid and the three principal directions (all 24 axis orders and signs are scored). centroid and bbox_center only translate."),
        )
        .param(
            ParamSpec::int("sample_limit", "Sample limit", 20000, 1000, 1_000_000)
                .help("Maximum number of points used to score candidates."),
        )
        .param(
            ParamSpec::bool("match_scale", "Match scale", false)
                .help("Also scale the moving geometry so its RMS radius equals the reference's (for clouds of unknown scale)."),
        )
        .help("Coarse starting pose for ICP from centroids and principal axes (PCA). The candidate with the lowest trimmed nearest-neighbour RMS wins. If the reference is a mesh its vertices are used.")
        .emulates("coarse alignment by principal axes (PCA) as used before registration")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn sample_points(g: &Geometry, limit: usize) -> Vec<DVec3> {
    let n = g.points.len();
    if n <= limit {
        return g.points.clone();
    }
    let step = n as f64 / limit as f64;
    (0..limit).map(|i| g.points[((i as f64 * step) as usize).min(n - 1)]).collect()
}

fn centroid_cov(pts: &[DVec3]) -> (DVec3, DMat3) {
    let c = pts.iter().fold(DVec3::ZERO, |a, b| a + *b) / pts.len() as f64;
    let mut cov = DMat3::ZERO;
    for p in pts {
        let d = *p - c;
        cov += DMat3::from_cols(d * d.x, d * d.y, d * d.z);
    }
    (c, cov * (1.0 / pts.len() as f64))
}

/// Right-handed principal frame; columns are the axes, strongest first.
fn principal_frame(pts: &[DVec3]) -> (DVec3, DMat3, [f64; 3]) {
    let (c, cov) = centroid_cov(pts);
    let (vals, vecs) = eigen3(&cov);
    let e2 = vecs[0].cross(vecs[1]).normalize_or_zero();
    (c, DMat3::from_cols(vecs[0], vecs[1], e2), vals)
}

/// The 24 proper signed permutation matrices.
fn cube_rotations() -> Vec<DMat3> {
    let perms = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    let mut out = Vec::new();
    for p in perms {
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                for sz in [-1.0, 1.0] {
                    let signs = [sx, sy, sz];
                    let mut cols = [DVec3::ZERO; 3];
                    for (c, col) in cols.iter_mut().enumerate() {
                        let mut v = [0.0; 3];
                        v[p[c]] = signs[c];
                        *col = DVec3::from_array(v);
                    }
                    let m = DMat3::from_cols(cols[0], cols[1], cols[2]);
                    if m.determinant() > 0.0 {
                        out.push(m);
                    }
                }
            }
        }
    }
    out
}

/// RMS of the smallest `keep` fraction of nearest-neighbour distances.
fn trimmed_rms(src: &[DVec3], pose: &Pose, tree: &KdTree, keep: f64) -> f64 {
    let mut d2: Vec<f64> = src
        .iter()
        .map(|p| tree.nearest(pose.apply(*p)).map_or(f64::INFINITY, |x| x.1))
        .collect();
    d2.sort_by(f64::total_cmp);
    let m = ((d2.len() as f64 * keep).ceil() as usize).clamp(1, d2.len());
    (d2[..m].iter().sum::<f64>() / m as f64).sqrt()
}

fn prealign_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let mv = inputs::geometry(inp, 0)?;
    let rf = inputs::geometry(inp, 1)?;
    ensure_metric(ctx, &mv)?;
    ensure_metric(ctx, &rf)?;
    let method = p.str("method")?.to_string();
    let limit = pi(p, "sample_limit", 1000, 1_000_000)? as usize;
    let match_scale = p.bool("match_scale")?;
    if mv.points.len() < 3 || rf.points.len() < 3 {
        return Err(ctx.fail("both inputs need at least 3 points"));
    }
    let ms = sample_points(&mv, limit);
    let rs = sample_points(&rf, limit);
    let tree = KdTree::new(&rs);
    let score_pts = sample_points(&Geometry::from_points(ms.clone()), 4000);
    let (cm, fm, vm) = principal_frame(&ms);
    let (cr, fr, vr) = principal_frame(&rs);
    let scale = if match_scale {
        let (a, b) = (vm.iter().sum::<f64>(), vr.iter().sum::<f64>());
        if a > 0.0 && b > 0.0 {
            (b / a).sqrt()
        } else {
            1.0
        }
    } else {
        1.0
    };
    let mk = |r: DMat3| -> Pose {
        // x -> cr + s R (x - cm)
        Pose {
            r,
            s: scale,
            t: cr - scale * (r * cm),
        }
    };
    let identity_rms = trimmed_rms(&score_pts, &Pose::IDENTITY, &tree, 0.8);
    let (pose, tested) = match method.as_str() {
        "centroid" => (mk(DMat3::IDENTITY), 1),
        "bbox_center" => {
            let dt = rf.bounds().center() - mv.bounds().center() * scale;
            (
                Pose {
                    r: DMat3::IDENTITY,
                    s: scale,
                    t: dt,
                },
                1,
            )
        }
        _ => {
            let cands = cube_rotations();
            let scored: Vec<(f64, Pose)> = cands
                .par_iter()
                .map(|pm| {
                    let pose = mk(fr * *pm * fm.transpose());
                    (trimmed_rms(&score_pts, &pose, &tree, 0.8), pose)
                })
                .collect();
            let best = scored
                .iter()
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|x| x.1)
                .unwrap_or(Pose::IDENTITY);
            (best, scored.len())
        }
    };
    check_scale(ctx, pose.s)?;
    let after = trimmed_rms(&score_pts, &pose, &tree, 0.8);
    let mut rep = Report::new("Auto Pre-Align");
    rep.metric("rms_before", Metric::new(identity_rms, "mm"));
    rep.metric("rms_after", Metric::new(after, "mm"));
    rep.metric("candidates_tested", Metric::new(tested as f64, ""));
    rep.metric("centroid_offset", Metric::new((cr - cm).length(), "mm"));
    if after > identity_rms {
        rep.note(Severity::Warning, "pre-alignment is worse than the input pose; the shapes may be nearly symmetric or only partly overlapping");
    }
    if vm[2] > 0.0 && vm[1] / vm[0] > 0.85 && method == "principal_axes" {
        rep.note(Severity::Warning, "the principal axes are nearly equal in length, so the axis order is ambiguous; check the result");
    }
    Ok(vec![
        Data::geometry(pose.apply_to(&mv)),
        Data::json(pose.to_json("auto_prealign")),
        Data::report(rep),
    ])
}

// --------------------------------------------------------------------- ICP

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorMetric {
    PointToPoint,
    PointToPlane,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reject {
    None,
    WorstPct(f64),
    Sigma(f64),
}

#[derive(Debug, Clone)]
pub struct IcpParams {
    pub metric: ErrorMetric,
    pub max_iterations: usize,
    pub rms_delta: f64,
    pub overlap: f64,
    pub sample_limit: usize,
    pub max_pair_dist: f64,
    pub reject: Reject,
    pub normal_angle_deg: f64,
    pub adjust_scale: bool,
    pub lock_rot: [bool; 3],
    pub lock_trans: [bool; 3],
    pub seed: u64,
}

impl Default for IcpParams {
    fn default() -> Self {
        IcpParams {
            metric: ErrorMetric::PointToPlane,
            max_iterations: 50,
            rms_delta: 1e-6,
            overlap: 1.0,
            sample_limit: 50_000,
            max_pair_dist: 0.0,
            reject: Reject::None,
            normal_angle_deg: 45.0,
            adjust_scale: false,
            lock_rot: [false; 3],
            lock_trans: [false; 3],
            seed: 1,
        }
    }
}

/// What the moving points are matched against.
pub enum RefIndex {
    Mesh(TriMesh),
    Cloud {
        tree: KdTree,
        pts: Vec<DVec3>,
        normals: Vec<DVec3>,
    },
}

/// Plane-fit normals for a cloud (sign arbitrary).
pub fn pca_normals(pts: &[DVec3], tree: &KdTree, k: usize) -> Vec<DVec3> {
    pts.par_iter()
        .map_init(Vec::new, |buf, q| {
            tree.knn(*q, k + 1, buf);
            let c = buf.iter().map(|e| pts[e.1 as usize]).fold(DVec3::ZERO, |a, b| a + b) / buf.len().max(1) as f64;
            let mut cov = DMat3::ZERO;
            for e in buf.iter() {
                let d = pts[e.1 as usize] - c;
                cov += DMat3::from_cols(d * d.x, d * d.y, d * d.z);
            }
            eigen3(&cov).1[2]
        })
        .collect()
}

impl RefIndex {
    /// Build from geometry, shifted by `-shift`. Meshes use closest-point on
    /// triangles; clouds use a k-d tree and stored or estimated normals.
    pub fn build(g: &Geometry, shift: DVec3) -> Option<RefIndex> {
        let pts: Vec<DVec3> = g.points.iter().map(|p| *p - shift).collect();
        if !g.prims.is_empty() {
            let tris: Vec<(usize, [u32; 3])> = g.triangles();
            if !tris.is_empty() {
                let t: Vec<[u32; 3]> = tris.iter().map(|x| x.1).collect();
                let src: Vec<u32> = tris.iter().map(|x| x.0 as u32).collect();
                return Some(RefIndex::Mesh(TriMesh::new(pts, t, src)));
            }
        }
        if pts.is_empty() {
            return None;
        }
        let tree = KdTree::new(&pts);
        let normals = match point_normals(g) {
            Some(n) => n.to_vec(),
            None => pca_normals(&pts, &tree, 12),
        };
        Some(RefIndex::Cloud { tree, pts, normals })
    }

    /// Closest reference point, its normal and the distance, within `max_d2`.
    pub fn closest(&self, p: DVec3, max_d2: f64) -> Option<(DVec3, DVec3, f64)> {
        match self {
            RefIndex::Mesh(m) => m.closest(p, max_d2, SignMode::None).map(|h| (h.point, m.tri_n[h.tri as usize], h.dist)),
            RefIndex::Cloud { tree, pts, normals } => tree
                .nearest_within(p, max_d2)
                .map(|(i, d2)| (pts[i as usize], normals[i as usize], d2.sqrt())),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct IcpResult {
    pub pose: Option<Pose>,
    pub iterations: usize,
    pub rms_history: Vec<f64>,
    pub rms_pairs: f64,
    pub rms_all_before: f64,
    pub rms_all_after: f64,
    pub cond: f64,
    pub converged: bool,
    pub pairs_used: usize,
    pub sampled: usize,
    pub free_directions: Vec<String>,
    pub notes: Vec<(Severity, String)>,
}

struct Pair {
    p: DVec3,
    q: DVec3,
    n: DVec3,
    d: f64,
}

fn dir_name(v: &[f64; MAXN], n: usize) -> String {
    let mut best = 0;
    for i in 1..n {
        if v[i].abs() > v[best].abs() {
            best = i;
        }
    }
    let axis = ["x", "y", "z"];
    match best {
        0..=2 => format!("rotation about {}", axis[best]),
        3..=5 => format!("translation along {}", axis[best - 3]),
        _ => "scale".to_string(),
    }
}

fn rms_all(src: &[DVec3], pose: &Pose, rf: &RefIndex) -> f64 {
    let step = (src.len() / 2_000_000).max(1);
    let (sum, cnt) = src
        .par_iter()
        .step_by(step)
        .map(|p| rf.closest(pose.apply(*p), f64::INFINITY).map_or((0.0, 0usize), |h| (h.2 * h.2, 1usize)))
        .reduce(|| (0.0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    if cnt == 0 {
        0.0
    } else {
        (sum / cnt as f64).sqrt()
    }
}

/// Run ICP with `src` (moving points) and `src_n` (optional unit normals)
/// against `rf`. Both must already be expressed relative to a common origin
/// near the reference centre; `scale_len` is a characteristic length used to
/// balance rotation and translation unknowns.
pub fn run_icp(src: &[DVec3], src_n: Option<&[DVec3]>, rf: &RefIndex, prm: &IcpParams, scale_len: f64) -> IcpResult {
    let mut res = IcpResult::default();
    let n = src.len();
    let l = scale_len.max(1e-9);
    let mut pose = Pose::IDENTITY;
    let mut perm: Vec<u32> = (0..n as u32).collect();
    let mut rng = Rng::new(prm.seed);
    let cap2 = if prm.max_pair_dist > 0.0 {
        prm.max_pair_dist * prm.max_pair_dist
    } else {
        f64::INFINITY
    };
    let cos_thr = if src_n.is_some() && prm.normal_angle_deg < 89.999 {
        Some(prm.normal_angle_deg.to_radians().cos())
    } else {
        None
    };
    res.rms_all_before = rms_all(src, &pose, rf);
    let free: Vec<usize> = {
        let mut f = Vec::new();
        for i in 0..3 {
            if !prm.lock_rot[i] {
                f.push(i);
            }
        }
        for i in 0..3 {
            if !prm.lock_trans[i] {
                f.push(3 + i);
            }
        }
        if prm.adjust_scale {
            f.push(6);
        }
        f
    };
    let locked = prm.lock_rot.iter().chain(prm.lock_trans.iter()).any(|&b| b);
    let use_horn = prm.metric == ErrorMetric::PointToPoint && !locked;
    let mut prev_rms = f64::INFINITY;
    let mut degenerate = false;
    let mut cond_max = 1.0f64;
    let mut stopped_by_rule = false;

    // Matching, compatibility filter, rejection and trimming for one pose.
    let select = |pose: &Pose, idx: &[u32]| -> Vec<Pair> {
        let m = idx.len();
        let mut pairs: Vec<Pair> = idx
            .par_iter()
            .filter_map(|&i| {
                let i = i as usize;
                let p = pose.apply(src[i]);
                let (q, nq, d) = rf.closest(p, cap2)?;
                if let (Some(th), Some(sn)) = (cos_thr, src_n) {
                    if (pose.r * sn[i]).dot(nq).abs() < th {
                        return None;
                    }
                }
                Some(Pair { p, q, n: nq, d })
            })
            .collect();
        match prm.reject {
            Reject::None => {}
            Reject::WorstPct(pc) => {
                if !pairs.is_empty() {
                    pairs.sort_by(|a, b| a.d.total_cmp(&b.d));
                    let keep = ((pairs.len() as f64) * (1.0 - pc / 100.0)).ceil() as usize;
                    pairs.truncate(keep.max(1));
                }
            }
            Reject::Sigma(k) => {
                let cnt = pairs.len().max(1) as f64;
                let mu = pairs.iter().map(|p| p.d).sum::<f64>() / cnt;
                let sd = (pairs.iter().map(|p| (p.d - mu) * (p.d - mu)).sum::<f64>() / cnt).sqrt();
                let thr = mu + k * sd;
                pairs.retain(|p| p.d <= thr + 1e-12);
            }
        }
        if prm.overlap < 1.0 {
            pairs.sort_by(|a, b| a.d.total_cmp(&b.d));
            let keep = ((m as f64) * prm.overlap).ceil() as usize;
            pairs.truncate(keep.max(1));
        }
        pairs
    };
    let pair_rms = |pairs: &[Pair]| -> f64 {
        if pairs.is_empty() {
            0.0
        } else {
            (pairs.iter().map(|p| p.d * p.d).sum::<f64>() / pairs.len() as f64).sqrt()
        }
    };

    for it in 0..prm.max_iterations.max(1) {
        let m = n.min(prm.sample_limit.max(10));
        if m < n {
            partial_shuffle(&mut perm, m, &mut rng);
        }
        let idx: &[u32] = &perm[..m];
        res.sampled = m;
        let pairs = select(&pose, idx);
        let need = if use_horn { 3 } else { free.len().max(3) };
        let rms = pair_rms(&pairs);
        res.rms_pairs = rms;
        res.pairs_used = pairs.len();
        if pairs.len() < need {
            res.notes.push((
                Severity::Warning,
                format!(
                    "only {} correspondences found (need {need}); the pose was left unchanged. Increase max pair distance or improve the starting pose",
                    pairs.len()
                ),
            ));
            degenerate = true;
            res.rms_history.push(rms);
            res.iterations = it;
            break;
        }
        res.rms_history.push(rms);
        res.iterations = it + 1;
        if it > 0 && (rms - prev_rms).abs() < prm.rms_delta {
            stopped_by_rule = true;
            // still evaluate the conditioning of this final system below
        }
        prev_rms = rms;

        // ---- solve for the incremental transform
        let (dr, dt, ds): (DMat3, DVec3, f64);
        if use_horn {
            let a: Vec<DVec3> = pairs.iter().map(|x| x.p).collect();
            let b: Vec<DVec3> = pairs.iter().map(|x| x.q).collect();
            match fit_rigid(&a, &b, None, prm.adjust_scale) {
                Ok(f) => {
                    dr = f.r;
                    dt = f.t;
                    ds = f.s;
                }
                Err(e) => {
                    res.notes.push((Severity::Warning, format!("degenerate correspondences: {e}")));
                    degenerate = true;
                    cond_max = 1e30;
                    break;
                }
            }
        } else {
            let nu = free.len();
            let mut ata: Mat = [[0.0; MAXN]; MAXN];
            let mut atb = [0.0; MAXN];
            let axes = [DVec3::X, DVec3::Y, DVec3::Z];
            let mut add_row = |p: DVec3, nrm: DVec3, b: f64| {
                let c = p.cross(nrm) / l;
                let full = [c.x, c.y, c.z, nrm.x, nrm.y, nrm.z, nrm.dot(p) / l];
                let mut row = [0.0; MAXN];
                for (k, &fi) in free.iter().enumerate() {
                    row[k] = full[fi];
                }
                for r in 0..nu {
                    for cc in 0..nu {
                        ata[r][cc] += row[r] * row[cc];
                    }
                    atb[r] += row[r] * b;
                }
            };
            for pr in &pairs {
                match prm.metric {
                    ErrorMetric::PointToPlane => add_row(pr.p, pr.n, pr.n.dot(pr.q - pr.p)),
                    ErrorMetric::PointToPoint => {
                        for ax in axes {
                            add_row(pr.p, ax, ax.dot(pr.q - pr.p));
                        }
                    }
                }
            }
            let sol = solve_pinv(nu, &ata, &atb, 1e8);
            cond_max = cond_max.max(sol.cond);
            if sol.cond > 1e8 {
                degenerate = true;
                for f in &sol.free {
                    let mut full = [0.0; MAXN];
                    for (k, &fi) in free.iter().enumerate() {
                        full[fi] = f[k];
                    }
                    let name = dir_name(&full, 7);
                    if !res.free_directions.contains(&name) {
                        res.free_directions.push(name);
                    }
                }
            }
            let mut x = [0.0; 7];
            for (k, &fi) in free.iter().enumerate() {
                x[fi] = sol.x[k];
            }
            dr = rodrigues(DVec3::new(x[0], x[1], x[2]) / l);
            dt = DVec3::new(x[3], x[4], x[5]);
            ds = 1.0 + x[6] / l;
        }
        if !(ds.is_finite() && ds > 0.0 && dt.is_finite()) {
            res.notes.push((Severity::Warning, "the update step was not finite; stopping".into()));
            degenerate = true;
            break;
        }
        let step_rot = (dr - DMat3::IDENTITY).to_cols_array().iter().fold(0.0f64, |a, b| a.max(b.abs()));
        let step_t = dt.length();
        pose = pose.then(&Pose { r: dr, s: ds, t: dt });
        if stopped_by_rule || (step_rot < 1e-9 && step_t < 1e-9 && (ds - 1.0).abs() < 1e-9) {
            stopped_by_rule = true;
            break;
        }
    }
    res.cond = cond_max;
    res.converged = stopped_by_rule && !degenerate;
    res.rms_all_after = rms_all(src, &pose, rf);
    // final pair statistics with the final pose, selected exactly like in the loop
    {
        let m = n.min(prm.sample_limit.max(10));
        let pairs = select(&pose, &perm[..m]);
        if !pairs.is_empty() {
            res.rms_pairs = pair_rms(&pairs);
            res.pairs_used = pairs.len();
        }
    }
    res.pose = Some(pose);
    res
}

fn icp_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.icp_align", "ICP Align", Stage::Clean)
        .input(PortSpec::new("moving", DataKind::Geometry))
        .input(PortSpec::new("reference", DataKind::Geometry))
        .output(geo_out())
        .output(xf_out())
        .output(rep_out())
        .param(ParamSpec::choice("metric", "Error metric", "point_to_plane", &["point_to_plane", "point_to_point"]).help("Point-to-plane converges faster on smooth surfaces; point-to-point is the classic form."))
        .param(ParamSpec::int("max_iterations", "Max iterations", 50, 1, 1000).help("Upper bound on iterations."))
        .param(ParamSpec::float("rms_delta", "RMS change to stop", 1e-6, 1e-10, 1.0).unit("mm").help("Stop when the RMS changes by less than this between iterations."))
        .param(ParamSpec::float("overlap", "Overlap", 1.0, 0.1, 1.0).help("Expected share of the moving points that overlap the reference; the worst remainder is trimmed each iteration."))
        .param(ParamSpec::int("sample_limit", "Sample limit", 50000, 1000, 5_000_000).help("Points drawn at random from the moving geometry per iteration; at or above its size every point is used."))
        .param(ParamSpec::float("max_pair_dist", "Max pair distance", 0.0, 0.0, 10000.0).unit("mm").help("0 = off. Pairs farther apart than this are ignored. A tight value can hide misfit: check the excluded share in the report."))
        .param(ParamSpec::choice("reject", "Reject", "none", &["none", "worst_pct", "sigma"]).help("Extra pair rejection: drop the worst percentage, or pairs beyond mean + k sigma."))
        .param(ParamSpec::float("reject_pct", "Reject worst", 10.0, 0.0, 90.0).unit("%").help("Used when reject is worst_pct."))
        .param(ParamSpec::float("reject_sigma", "Reject sigma", 2.5, 0.5, 10.0).help("Used when reject is sigma."))
        .param(ParamSpec::float("normal_angle", "Normal angle limit", 45.0, 0.0, 90.0).unit("deg").help("Pairs whose normals differ by more than this are ignored (needs normals on the moving points; 90 = off)."))
        .param(ParamSpec::bool("adjust_scale", "Adjust scale", false).help("Also estimate a uniform scale factor."))
        .param(ParamSpec::string("lock_rotation", "Lock rotation axes", "").help("Letters of axes whose rotation is locked, e.g. 'xy'. Empty = free."))
        .param(ParamSpec::string("lock_translation", "Lock translation axes", "").help("Letters of axes whose translation is locked, e.g. 'z'. Empty = free."))
        .param(ParamSpec::choice("fit_mode", "Fit mode", "global", &["global", "local"]).help("local limits pairs to a distance gate (5 % of the reference size when max pair distance is 0)."))
        .param(ParamSpec::float("rms_limit", "RMS limit", 0.0, 0.0, 10000.0).unit("mm").help("If above 0, the report gates on the final RMS."))
        .param(ParamSpec::int("seed", "Seed", 1, 0, 1_000_000_000).help("Seed for the random sampling."))
        .help("Iterative closest point refinement from the current pose. Needs a starting pose within the feature size of the surface. Reports RMS per iteration, conditioning (sliding degeneracy), and how many points were excluded.")
        .emulates("ICP registration (point-to-plane, trimmed, with distance gate) as in point-cloud comparison tools")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn icp_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let mv = inputs::geometry(inp, 0)?;
    let rf = inputs::geometry(inp, 1)?;
    ensure_metric(ctx, &mv)?;
    ensure_metric(ctx, &rf)?;
    if mv.points.len() < 6 {
        return Err(ctx.fail("the moving geometry needs at least 6 points"));
    }
    let bb = rf.bounds();
    let c = bb.center();
    let diag = bb.size().length().max(1e-9);
    let mut prm = IcpParams {
        metric: if p.str("metric")? == "point_to_point" {
            ErrorMetric::PointToPoint
        } else {
            ErrorMetric::PointToPlane
        },
        max_iterations: pi(p, "max_iterations", 1, 1000)? as usize,
        rms_delta: pf(p, "rms_delta", 1e-10, 1.0)?,
        overlap: pf(p, "overlap", 0.1, 1.0)?,
        sample_limit: pi(p, "sample_limit", 1000, 5_000_000)? as usize,
        max_pair_dist: pf(p, "max_pair_dist", 0.0, 10000.0)?,
        reject: match p.str("reject")? {
            "worst_pct" => Reject::WorstPct(pf(p, "reject_pct", 0.0, 90.0)?),
            "sigma" => Reject::Sigma(pf(p, "reject_sigma", 0.5, 10.0)?),
            _ => Reject::None,
        },
        normal_angle_deg: pf(p, "normal_angle", 0.0, 90.0)?,
        adjust_scale: p.bool("adjust_scale")?,
        lock_rot: axes_mask(p.str("lock_rotation")?),
        lock_trans: axes_mask(p.str("lock_translation")?),
        seed: pi(p, "seed", 0, 1_000_000_000)? as u64,
    };
    let mut rep = Report::new("ICP Align");
    if p.str("fit_mode")? == "local" && prm.max_pair_dist <= 0.0 {
        prm.max_pair_dist = 0.05 * diag;
        rep.note(
            Severity::Info,
            format!("local fit: pair distance gate set to {:.3} mm (5 % of the reference size)", prm.max_pair_dist),
        );
    }
    let rf_idx = RefIndex::build(&rf, c).ok_or_else(|| ctx.fail("the reference has no points or faces"))?;
    let src: Vec<DVec3> = mv.points.iter().map(|q| *q - c).collect();
    let src_n = point_normals(&mv);
    if src_n.is_none() && prm.normal_angle_deg < 89.999 {
        rep.note(Severity::Info, "the moving points have no normals, so the normal-angle limit is not applied");
    }
    let res = run_icp(&src, src_n, &rf_idx, &prm, diag * 0.5);
    let pose_c = res.pose.unwrap_or(Pose::IDENTITY);
    let pose = pose_c.uncentred(c);
    check_scale(ctx, pose.s)?;
    let mut limit_metric = Metric::new(res.rms_pairs, "mm");
    let rms_limit = pf(p, "rms_limit", 0.0, 10000.0)?;
    if rms_limit > 0.0 {
        limit_metric = limit_metric.with_limit(rms_limit, true);
    }
    rep.metric("rms_pairs", limit_metric);
    rep.metric("rms_all_before", Metric::new(res.rms_all_before, "mm"));
    rep.metric("rms_all_after", Metric::new(res.rms_all_after, "mm"));
    rep.metric("iterations", Metric::new(res.iterations as f64, ""));
    rep.metric("pairs_used", Metric::new(res.pairs_used as f64, "pairs"));
    let excluded = 100.0 - pct(res.pairs_used, res.sampled.max(1));
    rep.metric("excluded_pairs", Metric::new(excluded.max(0.0), "%"));
    rep.metric("condition_number", Metric::new(res.cond, ""));
    rep.metric("converged", Metric::new(if res.converged { 1.0 } else { 0.0 }, "bool").with_limit(1.0, false));
    if prm.adjust_scale {
        rep.metric("scale", Metric::new(pose.s, ""));
    }
    if excluded > 50.0 {
        rep.note(
            Severity::Warning,
            format!("{excluded:.0} % of the sampled points were excluded; a tight pair distance, trimming or normal limit can hide misfit"),
        );
    }
    if res.cond > 1e8 {
        let dirs = if res.free_directions.is_empty() {
            "unknown".to_string()
        } else {
            res.free_directions.join(", ")
        };
        rep.note(
            Severity::Warning,
            format!(
                "sliding degeneracy (condition number {:.2e}): the surface does not constrain {dirs}. The pose is not claimed to be converged",
                res.cond
            ),
        );
    }
    if !res.converged && res.cond <= 1e8 && res.pairs_used > 0 {
        rep.note(
            Severity::Warning,
            format!("did not meet the RMS-change stop rule within {} iterations", prm.max_iterations),
        );
    }
    for (s, t) in &res.notes {
        rep.note(*s, t.clone());
    }
    let mut out = pose.apply_to(&mv);
    out.detail_attribs.set("icp_rms_before", AttribArray::Float(vec![res.rms_all_before]));
    out.detail_attribs.set("icp_rms_after", AttribArray::Float(vec![res.rms_all_after]));
    out.detail_attribs.set("icp_iterations", AttribArray::Int(vec![res.iterations as i64]));
    let mut xf = pose.to_json("icp");
    xf["icp"] = json!({
        "iterations": res.iterations,
        "rms_per_iteration": res.rms_history,
        "rms_pairs": res.rms_pairs,
        "rms_all_before": res.rms_all_before,
        "rms_all_after": res.rms_all_after,
        "condition_number": res.cond,
        "converged": res.converged,
        "free_directions": res.free_directions,
        "excluded_percent": excluded.max(0.0),
    });
    Ok(vec![Data::geometry(out), Data::json(xf), Data::report(rep)])
}

// ---------------------------------------------------------- apply transform

fn apply_info() -> NodeTypeInfo {
    NodeTypeInfo::new("clean.apply_transform", "Apply Stored Transform", Stage::Clean)
        .input(PortSpec::new("geometry", DataKind::Geometry))
        .input(PortSpec::new("transform", DataKind::Json))
        .output(geo_out())
        .param(ParamSpec::bool("invert", "Invert", false).help("Apply the inverse of the stored transform."))
        .help("Moves geometry by a transform produced by an align node, so one alignment can be reused on another scan, or undone.")
        .emulates("re-applying a stored registration matrix")
        .spec(SPEC_ALIGN)
        .implemented()
}

fn apply_cook(ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let g = inputs::geometry(inp, 0)?;
    let xf = json_in(inp, 1, "transform")?;
    let mut pose = Pose::from_json(&xf).ok_or_else(|| CookError::Other("transform input is not a 'transform.v1' record".into()))?;
    check_scale(ctx, pose.s)?;
    if p.bool("invert")? {
        pose = pose.inverse();
    }
    Ok(vec![Data::geometry(pose.apply_to(&g))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::*;

    fn pose(rot_deg: f64, axis: DVec3, t: DVec3) -> Pose {
        Pose {
            r: DMat3::from_axis_angle(axis.normalize(), rot_deg.to_radians()),
            s: 1.0,
            t,
        }
    }

    fn pairs_json(p: &[DVec3], q: &[DVec3]) -> Option<Data> {
        json(Value::Array(
            p.iter().zip(q).map(|(a, b)| json!([a.to_array(), b.to_array()])).collect(),
        ))
    }

    #[test]
    fn pose_algebra() {
        let a = pose(30.0, DVec3::new(1.0, 2.0, 3.0), DVec3::new(1.0, -2.0, 3.0));
        let b = pose(-12.0, DVec3::Z, DVec3::new(-4.0, 0.5, 9.0));
        let x = DVec3::new(7.0, -1.0, 2.0);
        assert!((a.then(&b).apply(x) - b.apply(a.apply(x))).length() < 1e-9);
        assert!((a.inverse().apply(a.apply(x)) - x).length() < 1e-9);
        let c = DVec3::new(100.0, 50.0, -20.0);
        let u = a.uncentred(c);
        assert!((u.apply(x) - (a.apply(x - c) + c)).length() < 1e-9);
        let back = Pose::from_json(&a.to_json("t")).unwrap();
        assert!((back.apply(x) - a.apply(x)).length() < 1e-9);
    }

    #[test]
    fn pair_align_golden() {
        let p = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(100.0, 0.0, 0.0),
            DVec3::new(0.0, 50.0, 0.0),
            DVec3::new(0.0, 0.0, 25.0),
        ];
        let tr = pose(10.0, DVec3::Z, DVec3::new(5.0, -3.0, 2.0));
        let q: Vec<DVec3> = p.iter().map(|x| tr.apply(*x)).collect();
        let g = Geometry::from_points(p.clone());
        let out = run(pair_info, pair_cook, vec![geo(g), None, pairs_json(&p, &q)], &[]).unwrap();
        let o = out_geo(&out[0]);
        for (a, b) in o.points.iter().zip(&q) {
            assert!((*a - *b).length() < 1e-9);
        }
        let r = out_report(&out[2]);
        assert!(r.metrics["rms"].value < 1e-9);
        let back = Pose::from_json(out_json(&out[1])).unwrap();
        assert!((back.apply(p[1]) - q[1]).length() < 1e-9);
    }

    #[test]
    fn pair_align_errors_are_typed() {
        let p = vec![DVec3::ZERO, DVec3::X, DVec3::Y * 2.0];
        let g = || geo(Geometry::from_points(p.clone()));
        assert!(run(pair_info, pair_cook, vec![g(), None, pairs_json(&p[..2], &p[..2])], &[]).is_err());
        let line = vec![DVec3::ZERO, DVec3::X, DVec3::X * 2.0];
        assert!(run(pair_info, pair_cook, vec![g(), None, pairs_json(&line, &line)], &[]).is_err());
        assert!(run(pair_info, pair_cook, vec![g(), None, json(json!("nope"))], &[]).is_err());
        assert!(run(pair_info, pair_cook, vec![g(), None, json(json!([[[0, 0, 0]]]))], &[]).is_err());
    }

    #[test]
    fn pair_align_scale_option() {
        let p = vec![
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(100.0, 0.0, 0.0),
            DVec3::new(0.0, 50.0, 0.0),
            DVec3::new(0.0, 0.0, 25.0),
        ];
        let q: Vec<DVec3> = p.iter().map(|x| *x * 1.02).collect();
        let g = Geometry::from_points(p.clone());
        let free = run(pair_info, pair_cook, vec![geo(g.clone()), None, pairs_json(&p, &q)], &[("fix_scale", b(false))]).unwrap();
        let fixed = run(pair_info, pair_cook, vec![geo(g), None, pairs_json(&p, &q)], &[]).unwrap();
        assert!((out_report(&free[2]).metrics["scale"].value - 1.02).abs() < 1e-6);
        assert!(out_report(&fixed[2]).metrics["rms"].value > out_report(&free[2]).metrics["rms"].value);
    }

    #[test]
    fn prealign_recovers_a_large_rotation() {
        let reference = ellipsoid_cloud(120.0, 60.0, 40.0, 3_000, 7);
        let tr = pose(90.0, DVec3::new(0.3, 0.2, 1.0), DVec3::new(300.0, -120.0, 50.0));
        let mut moving = tr.apply_to(&reference);
        moving.point_attribs.remove("N");
        let out = run(prealign_info, prealign_cook, vec![geo(moving), geo(reference.clone())], &[]).unwrap();
        let r = out_report(&out[2]);
        assert!(r.metrics["rms_before"].value > 100.0);
        assert!(r.metrics["rms_after"].value < 1e-3, "{}", r.metrics["rms_after"].value);
        let o = out_geo(&out[0]);
        let max_err = o
            .points
            .iter()
            .zip(&reference.points)
            .map(|(a, b)| (*a - *b).length())
            .fold(0.0, f64::max);
        assert!(max_err < 1e-3, "{max_err}");
    }

    #[test]
    fn prealign_translation_only_methods() {
        let reference = ellipsoid_cloud(120.0, 60.0, 40.0, 1_500, 7);
        let moving = pose(0.0, DVec3::Z, DVec3::new(30.0, 0.0, 0.0)).apply_to(&reference);
        for method in ["centroid", "bbox_center"] {
            let out = run(prealign_info, prealign_cook, vec![geo(moving.clone()), geo(reference.clone())], &[("method", s(method))]).unwrap();
            let o = out_geo(&out[0]);
            let d = (o.points[0] - reference.points[0]).length();
            assert!(d < 2.0, "{method}: {d}");
        }
    }

    fn ellipsoid_case() -> (Geometry, Geometry, Pose) {
        let reference = ellipsoid_cloud(120.0, 60.0, 40.0, 20_000, 11);
        let tr = pose(3.0, DVec3::new(1.0, 2.0, 3.0), DVec3::new(2.0, -1.0, 0.5));
        let moving = tr.apply_to(&reference);
        (reference, moving, tr)
    }

    #[test]
    fn golden_4_icp_ellipsoid_point_to_plane() {
        let (reference, moving, _tr) = ellipsoid_case();
        let out = run(icp_info, icp_cook, vec![geo(moving), geo(reference.clone())], &[]).unwrap();
        let o = out_geo(&out[0]);
        let max_err = o
            .points
            .iter()
            .zip(&reference.points)
            .map(|(a, b)| (*a - *b).length())
            .fold(0.0, f64::max);
        assert!(max_err < 0.01, "max error {max_err}");
        let r = out_report(&out[2]);
        assert!(r.metrics["rms_pairs"].value < 0.005, "{}", r.metrics["rms_pairs"].value);
        assert!(r.metrics["iterations"].value < 30.0, "{}", r.metrics["iterations"].value);
        assert_eq!(r.metrics["converged"].value, 1.0);
        assert!(r.metrics["rms_all_before"].value > 1.0);
        assert!(r.metrics["condition_number"].value < 1e8);
        // transform record carries the per-iteration history
        let xf = out_json(&out[1]);
        assert!(xf["icp"]["rms_per_iteration"].as_array().unwrap().len() >= 2);
        // detail attributes are available to later stages
        assert!(o.detail_attribs.floats("icp_rms_after").is_ok());
    }

    #[test]
    fn icp_point_to_point_matches() {
        let (reference, moving, _) = ellipsoid_case();
        let out = run(
            icp_info,
            icp_cook,
            vec![geo(moving), geo(reference.clone())],
            &[("metric", s("point_to_point")), ("max_iterations", i(200))],
        )
        .unwrap();
        let o = out_geo(&out[0]);
        let max_err = o.points.iter().zip(&reference.points).map(|(a, b)| (*a - *b).length()).fold(0.0, f64::max);
        assert!(max_err < 0.05, "max error {max_err}");
    }

    #[test]
    fn icp_against_a_mesh_reference() {
        // ellipsoid mesh from a scaled UV sphere
        let (pos, tris) = uv_sphere(1.0, 64, 32);
        let pos: Vec<DVec3> = pos.iter().map(|p| DVec3::new(p.x * 120.0, p.y * 60.0, p.z * 40.0)).collect();
        let n = tris.len();
        let mesh = Geometry {
            points: pos,
            prims: tris.iter().map(|t| vernon_core::Prim::polygon(t.to_vec())).collect(),
            ..Default::default()
        };
        let _ = n;
        let tr = pose(2.0, DVec3::new(0.0, 0.0, 1.0), DVec3::new(3.0, -2.0, 1.0));
        let moving = tr.apply_to(&ellipsoid_cloud(120.0, 60.0, 40.0, 8_000, 5));
        let out = run(icp_info, icp_cook, vec![geo(moving), geo(mesh)], &[("normal_angle", f(90.0))]).unwrap();
        let r = out_report(&out[2]);
        assert!(r.metrics["rms_all_after"].value < 0.3, "{}", r.metrics["rms_all_after"].value);
        assert!(r.metrics["rms_all_after"].value < r.metrics["rms_all_before"].value * 0.1);
    }

    #[test]
    fn golden_5_flat_plate_slide_is_flagged() {
        // reference: 200 x 100 plate mesh; moving: same plate sampled as points, slid 5 mm in X
        let reference = Geometry::from_polygons(
            vec![
                DVec3::new(-100.0, -50.0, 0.0),
                DVec3::new(100.0, -50.0, 0.0),
                DVec3::new(100.0, 50.0, 0.0),
                DVec3::new(-100.0, 50.0, 0.0),
            ],
            vec![vec![0, 1, 2, 3]],
        );
        let mut pts = Vec::new();
        for j in 0..=20 {
            for i in 0..=40 {
                pts.push(DVec3::new(-100.0 + i as f64 * 5.0 + 5.0, -50.0 + j as f64 * 5.0, 0.0));
            }
        }
        // drop the points that slid off the plate so every point has a partner
        pts.retain(|p| p.x <= 100.0);
        let out = run(icp_info, icp_cook, vec![geo(Geometry::from_points(pts)), geo(reference)], &[("normal_angle", f(90.0))]).unwrap();
        let r = out_report(&out[2]);
        assert!(r.metrics["condition_number"].value > 1e8);
        assert_eq!(r.metrics["converged"].value, 0.0);
        let msg: String = r.notes.iter().map(|n| n.1.clone()).collect::<Vec<_>>().join(" ");
        assert!(msg.contains("translation along") || msg.contains("rotation about"), "{msg}");
        assert!(msg.contains("sliding degeneracy"));
        // and the pose did not invent a slide
        let xf = out_json(&out[1]);
        assert!(!xf["icp"]["free_directions"].as_array().unwrap().is_empty());
    }

    #[test]
    fn golden_6_partial_overlap_is_trimmed() {
        let reference = ellipsoid_cloud(120.0, 60.0, 40.0, 10_000, 21);
        let mut moving = pose(1.0, DVec3::new(0.2, 0.5, 1.0), DVec3::new(1.5, -1.0, 0.5)).apply_to(&reference);
        // a 20 % patch displaced 30 mm out of the surface
        let mut rng = Rng::new(77);
        for q in moving.points.iter_mut() {
            if rng.next_f64() < 0.2 {
                *q += q.normalize() * 30.0;
            }
        }
        moving.point_attribs.remove("N");
        let trimmed = run(icp_info, icp_cook, vec![geo(moving.clone()), geo(reference.clone())], &[("overlap", f(0.8))]).unwrap();
        let plain = run(icp_info, icp_cook, vec![geo(moving), geo(reference)], &[("overlap", f(1.0))]).unwrap();
        let rt = out_report(&trimmed[2]).metrics["rms_pairs"].value;
        let rp = out_report(&plain[2]).metrics["rms_pairs"].value;
        assert!(rt < 0.5, "trimmed rms {rt}");
        assert!(rp > 0.5, "untrimmed rms {rp}");
    }

    #[test]
    fn golden_7_tight_distance_gate_is_reported() {
        let reference = ellipsoid_cloud(120.0, 60.0, 40.0, 5_000, 21);
        let moving = pose(0.0, DVec3::Z, DVec3::new(20.0, 0.0, 0.0)).apply_to(&reference);
        let out = run(icp_info, icp_cook, vec![geo(moving.clone()), geo(reference)], &[("max_pair_dist", f(0.01))]).unwrap();
        let r = out_report(&out[2]);
        assert!(r.metrics["excluded_pairs"].value > 50.0);
        assert_eq!(r.metrics["converged"].value, 0.0);
        let o = out_geo(&out[0]);
        assert!((o.points[0] - moving.points[0]).length() < 0.01);
        assert!(r.notes.iter().any(|n| n.0 == Severity::Warning && n.1.contains("excluded")));
    }

    #[test]
    fn icp_scale_and_locks() {
        let reference = ellipsoid_cloud(120.0, 60.0, 40.0, 8_000, 31);
        // scaled copy
        let mut moving = reference.clone();
        for q in moving.points.iter_mut() {
            *q *= 1.02;
        }
        moving.point_attribs.remove("N");
        let out = run(
            icp_info,
            icp_cook,
            vec![geo(moving.clone()), geo(reference.clone())],
            &[("adjust_scale", b(true)), ("max_iterations", i(100))],
        )
        .unwrap();
        let sc = out_report(&out[2]).metrics["scale"].value;
        assert!((sc - 1.0 / 1.02).abs() < 2e-3, "scale {sc}");
        // locked translation in z: a z-offset must not be fixed
        let moved = pose(0.0, DVec3::Z, DVec3::new(0.0, 0.0, 3.0)).apply_to(&reference);
        let out = run(
            icp_info,
            icp_cook,
            vec![geo(moved.clone()), geo(reference)],
            &[("lock_translation", s("z")), ("lock_rotation", s("xyz")), ("normal_angle", f(90.0))],
        )
        .unwrap();
        let o = out_geo(&out[0]);
        assert!((o.points[0].z - moved.points[0].z).abs() < 1e-9, "z moved");
    }

    #[test]
    fn icp_rejection_modes_run() {
        let (reference, moving, _) = ellipsoid_case();
        for (mode, over) in [("worst_pct", f(10.0)), ("sigma", f(2.5))] {
            let key = if mode == "worst_pct" { "reject_pct" } else { "reject_sigma" };
            let out = run(
                icp_info,
                icp_cook,
                vec![geo(moving.clone()), geo(reference.clone())],
                &[("reject", s(mode)), (key, over)],
            )
            .unwrap();
            assert!(out_report(&out[2]).metrics["rms_all_after"].value < 0.05);
        }
    }

    #[test]
    fn pair_then_icp_composes_once() {
        // K46: coarse transform must not be applied twice
        let reference = ellipsoid_cloud(120.0, 60.0, 40.0, 6_000, 41);
        let truth = pose(15.0, DVec3::new(1.0, 1.0, 1.0), DVec3::new(40.0, -30.0, 10.0));
        let moving = truth.apply_to(&reference);
        // three approximate pairs (moving point -> reference point), 0.4 mm of pick noise
        let idx = [0usize, 1000, 2000, 3000];
        let pm: Vec<DVec3> = idx.iter().map(|&k| moving.points[k]).collect();
        let pr: Vec<DVec3> = idx.iter().enumerate().map(|(n, &k)| reference.points[k] + DVec3::splat(0.4 * (n as f64 - 1.5) / 1.5)).collect();
        let coarse = run(pair_info, pair_cook, vec![geo(moving), None, pairs_json(&pm, &pr)], &[]).unwrap();
        let fine = run(
            icp_info,
            icp_cook,
            vec![coarse[0].clone().into(), geo(reference.clone())],
            &[],
        )
        .unwrap();
        let o = out_geo(&fine[0]);
        let max_err = o.points.iter().zip(&reference.points).map(|(a, b)| (*a - *b).length()).fold(0.0, f64::max);
        assert!(max_err < 0.02, "{max_err}");
    }

    #[test]
    fn apply_transform_round_trip() {
        let g = ellipsoid_cloud(50.0, 40.0, 30.0, 200, 1);
        let tr = pose(33.0, DVec3::new(0.0, 1.0, 1.0), DVec3::new(10.0, 20.0, 30.0));
        let fwd = run(apply_info, apply_cook, vec![geo(g.clone()), json(tr.to_json("x"))], &[]).unwrap();
        let back = run(apply_info, apply_cook, vec![fwd[0].clone().into(), json(tr.to_json("x"))], &[("invert", b(true))]).unwrap();
        let o = out_geo(&back[0]);
        for (a, b) in o.points.iter().zip(&g.points) {
            assert!((*a - *b).length() < 1e-9);
        }
        assert!(run(apply_info, apply_cook, vec![geo(g), json(json!({"foo": 1}))], &[]).is_err());
    }
}
