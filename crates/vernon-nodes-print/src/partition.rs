//! Bounded deterministic BSP beam search, following the framework of Luo et al.
//! (2012), with finite candidate sampling and explicitly approximate objectives.
use crate::{
    cut::{cut_mesh, Plane},
    fit::{fit_part, FitOptions, Placement},
    mesh,
    orient::{OrientParams, Search, Tris},
};
use glam::DVec3;
use std::{collections::BTreeMap, sync::Arc};
use vernon_core::Geometry;

pub const MAX_PARTS: usize = 64;
pub const MAX_EVALUATIONS: usize = 4096;
const MAX_FACES: usize = 20_000;

pub struct Options {
    pub bed: DVec3,
    pub margin: f64,
    pub beam_width: usize,
    pub weights: [f64; 5],
}

pub struct ResultPlan {
    pub geometry: Geometry,
    pub parts: usize,
    pub cuts: usize,
    pub seam_area: f64,
    pub seam_length: f64,
    pub min_fit_margin: f64,
    pub costs: [f64; 5],
    pub evaluations: usize,
    pub limited: bool,
    pub guide_samples: usize,
}

#[derive(Clone)]
struct Part {
    geo: Arc<Geometry>,
    fit_margin: f64,
    estimate: f64,
}

#[derive(Clone)]
struct State {
    parts: Vec<Part>,
    seam_area: f64,
    seam_length: f64,
    visible: f64,
    fragile: f64,
    cuts: usize,
}

fn volume(g: &Geometry) -> f64 {
    let origin = g.points.first().copied().unwrap_or(DVec3::ZERO);
    g.triangles()
        .into_iter()
        .map(|(_, t)| {
            let [a, b, c] = t.map(|v| g.points[v as usize] - origin);
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

fn oriented(g: &Geometry) -> bool {
    let mut edges: BTreeMap<(u32, u32), (usize, i64)> = BTreeMap::new();
    for p in g.prims.iter().filter(|p| p.closed) {
        if p.vertices.len() < 3 {
            return false;
        }
        for i in 0..p.vertices.len() {
            let a = p.vertices[i];
            let b = p.vertices[(i + 1) % p.vertices.len()];
            if a == b {
                return false;
            }
            let (key, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
            let e = edges.entry(key).or_default();
            e.0 += 1;
            e.1 += sign;
        }
    }
    !edges.is_empty()
        && edges
            .values()
            .all(|&(count, balance)| count == 2 && balance == 0)
}

fn fit_options(o: &Options) -> FitOptions {
    FitOptions {
        bed: o.bed,
        margin: o.margin,
        search: Search::Axis,
        z_spin: true,
        allow_scaling: false,
        min_scale: 1.0,
        placement: Placement::Centre,
        orient: OrientParams {
            layer_h: 0.2,
            theta_o_deg: 45.0,
            loads: vec![],
            k: 1.0,
            weights: [0.0; 4],
        },
    }
}

fn part(g: Geometry, o: &Options) -> Result<Part, String> {
    let r =
        fit_part(&g.points, &Tris::new(&g), &fit_options(o)).ok_or("part has no usable faces")?;
    let ext = g.bounds().size();
    let usable = o.bed - DVec3::splat(2.0 * o.margin);
    // Search heuristic only: a box estimate is not a minimum for arbitrary shapes.
    let estimate = if r.fits {
        1.0
    } else {
        [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ]
        .iter()
        .map(|p| {
            (0..3)
                .map(|i| (ext[p[i]] / usable[i] - 1e-9).ceil().max(1.0))
                .product::<f64>()
        })
        .fold(f64::INFINITY, f64::min)
        .max(2.0)
    };
    Ok(Part {
        geo: Arc::new(g),
        fit_margin: r.margin,
        estimate,
    })
}

fn costs(s: &State, o: &Options) -> [f64; 5] {
    let count: f64 = s.parts.iter().map(|p| p.estimate).sum();
    let used: f64 = s.parts.iter().map(|p| volume(&p.geo).abs()).sum();
    let u = o.bed - DVec3::splat(2.0 * o.margin);
    let raw = [
        count,
        s.seam_area,
        s.visible,
        s.fragile,
        (count * u.x * u.y * u.z - used).max(0.0),
    ];
    std::array::from_fn(|i| raw[i] * o.weights[i])
}

fn rank(s: &State, o: &Options) -> f64 {
    costs(s, o).iter().sum()
}

fn canonical(n: DVec3) -> Option<DVec3> {
    if !n.is_finite() || n.length() < 1e-10 {
        return None;
    }
    let n = n.normalize();
    Some(
        if n.x < -1e-9 || (n.x.abs() < 1e-9 && (n.y < -1e-9 || (n.y.abs() < 1e-9 && n.z < 0.0))) {
            -n
        } else {
            n
        },
    )
}

fn normals(g: &Geometry) -> Vec<DVec3> {
    let mut v = vec![DVec3::X, DVec3::Y, DVec3::Z];
    // A coarse uniform subset, augmented by source face directions. Never
    // claim the paper's 129-direction sampling or its objective approximations.
    for q in [
        DVec3::new(1., 1., 0.),
        DVec3::new(1., -1., 0.),
        DVec3::new(1., 0., 1.),
        DVec3::new(1., 0., -1.),
        DVec3::new(0., 1., 1.),
        DVec3::new(0., 1., -1.),
    ] {
        v.push(q.normalize());
    }
    for i in 0..g.prims.len() {
        if let Some(n) = canonical(g.prim_normal(i)) {
            if !v.iter().any(|q| q.dot(n) > 1.0 - 1e-8) {
                v.push(n);
            }
            if v.len() == 16 {
                break;
            }
        }
    }
    v
}

fn planes(g: &Geometry, ns: &[DVec3], guide: &[DVec3], o: &Options) -> Vec<Plane> {
    let mut out = Vec::new();
    let usable = (o.bed - DVec3::splat(2.0 * o.margin)).max_element();
    for &n in ns {
        let (lo, hi) = g
            .points
            .iter()
            .map(|p| p.dot(n))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                (a.min(v), b.max(v))
            });
        let span = hi - lo;
        if span <= 1e-8 {
            continue;
        }
        let k = (span / usable).ceil().clamp(2.0, MAX_PARTS as f64);
        let mut ds = vec![lo + span * 0.5, lo + span / k, hi - span / k];
        for &p in guide.iter().take(16) {
            ds.push(p.dot(n));
        }
        for d in ds {
            if d <= lo + span * 1e-6 || d >= hi - span * 1e-6 {
                continue;
            }
            if out.iter().any(|p: &Plane| {
                p.normal.dot(n) > 1. - 1e-8 && (p.origin.dot(n) - d).abs() < span * 1e-8
            }) {
                continue;
            }
            out.push(Plane {
                origin: n * d,
                normal: n,
            });
        }
    }
    out
}

// Boundary edges of the newly added cap give actual seam length; cap internal
// triangulation edges cancel. Curvature is a local source-face normal proxy.
fn seam(g: &Geometry, cap: i64, guide: &[DVec3]) -> (f64, f64, f64) {
    let caps = g.prim_attribs.ints(mesh::CAP).unwrap();
    let mut edges: BTreeMap<(u32, u32), (usize, DVec3, DVec3)> = BTreeMap::new();
    for (i, p) in g.prims.iter().enumerate().filter(|(i, _)| caps[*i] == cap) {
        let _ = i;
        for k in 0..p.vertices.len() {
            let a = p.vertices[k];
            let b = p.vertices[(k + 1) % p.vertices.len()];
            let key = if a < b { (a, b) } else { (b, a) };
            let e = edges
                .entry(key)
                .or_insert((0, g.points[a as usize], g.points[b as usize]));
            e.0 += 1;
        }
    }
    let diag = g.bounds().size().length().max(1e-9);
    let mut body_edges: BTreeMap<(u32, u32), Vec<DVec3>> = BTreeMap::new();
    for (i, p) in g.prims.iter().enumerate().filter(|(i, _)| caps[*i] != cap) {
        for k in 0..p.vertices.len() {
            let a = p.vertices[k];
            let b = p.vertices[(k + 1) % p.vertices.len()];
            let key = if a < b { (a, b) } else { (b, a) };
            body_edges.entry(key).or_default().push(g.prim_normal(i));
        }
    }
    let mut crease: BTreeMap<u32, f64> = BTreeMap::new();
    for ((a, b), ns) in body_edges {
        if ns.len() == 2 {
            let angle = ns[0].dot(ns[1]).clamp(-1., 1.).acos() / std::f64::consts::PI;
            for v in [a, b] {
                let e = crease.entry(v).or_default();
                *e = e.max(angle);
            }
        }
    }
    let mut len = 0.;
    let mut visible = 0.;
    for ((ia, ib), (n, a, b)) in edges {
        if n == 1 {
            let l = a.distance(b);
            len += l;
            let mid = (a + b) * 0.5;
            let guide_distance = guide
                .iter()
                .map(|p| mid.distance(*p))
                .fold(f64::INFINITY, f64::min);
            // Nearby guide samples and actual adjacent-face dihedral angles reduce
            // the visibility objective. This is a mesh crease proxy, not curvature fitting.
            let guide_factor = if guide_distance.is_finite() {
                (guide_distance / (0.1 * diag)).clamp(0., 1.)
            } else {
                1.
            };
            let curvature = crease
                .get(&ia)
                .copied()
                .unwrap_or(0.)
                .max(crease.get(&ib).copied().unwrap_or(0.));
            visible += l * guide_factor * (1. - 0.5 * curvature);
        }
    }

    (len, visible, diag)
}

pub fn search(geo: &Geometry, guide: Option<&Geometry>, o: &Options) -> Result<ResultPlan, String> {
    geo.validate().map_err(|e| e.to_string())?;
    if geo.prims.is_empty()
        || geo.prims.len() > MAX_FACES
        || geo.points.len() > 200_000
        || geo.prims.len().saturating_mul(o.beam_width) > 160_000
        || geo
            .points
            .iter()
            .any(|p| !p.is_finite() || p.abs().max_element() > 1e12)
    {
        return Err("partition requires finite nonempty geometry within 20000 faces, 200000 points and 160000 face-times-beam budget".into());
    }
    if !o.bed.is_finite()
        || o.bed.max_element() > 1e12
        || !o.margin.is_finite()
        || o.margin < 0.
        || (o.bed - DVec3::splat(2. * o.margin)).min_element() <= 0.
        || o.weights
            .iter()
            .any(|w| !w.is_finite() || !(0.0..=1e6).contains(w))
        || !(1..=64).contains(&o.beam_width)
    {
        return Err("invalid bed, margin, weights or beam width".into());
    }
    let mut guides = Vec::new();
    if let Some(g) = guide {
        g.validate().map_err(|e| e.to_string())?;
        if g.points.iter().any(|p| !p.is_finite()) {
            return Err("guide points must be finite".into());
        }
        guides.extend(g.points.iter().copied().take(16));
    }
    let mut start = Vec::new();
    let base_cap = geo
        .prim_attribs
        .ints(mesh::CAP)
        .ok()
        .and_then(|v| v.iter().max().copied())
        .unwrap_or(0)
        .max(0);
    if base_cap > i64::MAX - MAX_PARTS as i64 - 1 {
        return Err("existing cap identifiers leave no room for partition cuts".into());
    }
    for id in mesh::unique_parts(geo) {
        let g = mesh::extract_part(geo, id);
        let stats = mesh::edge_stats(&g);
        if stats.open > 0 || stats.non_manifold > 0 || volume(&g) <= 1e-12 || !oriented(&g) {
            return Err("partition requires consistently outward closed manifold parts with positive volume".into());
        }
        start.push(part(g, o)?);
    }
    if start.is_empty() || start.len() > MAX_PARTS {
        return Err("partition has no solids or too many existing parts".into());
    }
    if start.iter().map(|p| p.estimate).sum::<f64>() > MAX_PARTS as f64 * 4. {
        return Err("partition exceeds bounded search size; check geometry and bed units".into());
    }
    let ns = normals(geo);
    let mut beam = vec![State {
        parts: start,
        seam_area: 0.,
        seam_length: 0.,
        visible: 0.,
        fragile: 0.,
        cuts: 0,
    }];
    let mut best: Option<State> = None;
    let mut evaluations = 0;
    for _ in 0..MAX_PARTS {
        let mut next = Vec::new();
        for state in beam {
            if state.parts.iter().all(|p| p.fit_margin >= -1e-9) {
                if best.as_ref().map_or(true, |b| rank(&state, o) < rank(b, o)) {
                    best = Some(state.clone());
                }
                next.push(state);
                continue;
            }
            if state.parts.len() >= MAX_PARTS {
                continue;
            }
            let target = state
                .parts
                .iter()
                .enumerate()
                .filter(|(_, p)| p.fit_margin < -1e-9)
                .max_by(|(ia, a), (ib, b)| {
                    a.estimate.total_cmp(&b.estimate).then_with(|| ib.cmp(ia))
                })
                .unwrap()
                .0;
            let source = &state.parts[target].geo;
            for plane in planes(source, &ns, &guides, o) {
                if evaluations >= MAX_EVALUATIONS {
                    break;
                }
                evaluations += 1;
                let cap = base_cap + state.cuts as i64 + 1;
                let Ok(cut) = cut_mesh(source, &plane, 1e-8, true, cap) else {
                    continue;
                };
                if cut.lower.prims.is_empty() || cut.upper.prims.is_empty() || cut.cap_area <= 1e-10
                {
                    continue;
                }
                let valid = [&cut.lower, &cut.upper].iter().all(|g| {
                    let s = mesh::edge_stats(g);
                    s.open == 0 && s.non_manifold == 0 && oriented(g) && volume(g) > 1e-12
                });
                if !valid {
                    continue;
                }
                let (length, visible, _) = seam(&cut.lower, cap, &guides);
                let radius = if length > 0. {
                    2. * cut.cap_area / length
                } else {
                    0.
                };
                let Ok(a) = part(cut.lower, o) else {
                    continue;
                };
                let Ok(b) = part(cut.upper, o) else {
                    continue;
                };
                let mut candidate = state.clone();
                candidate.parts.remove(target);
                candidate.parts.push(a);
                candidate.parts.push(b);
                if candidate
                    .parts
                    .iter()
                    .map(|p| p.geo.prims.len())
                    .sum::<usize>()
                    > 80_000
                {
                    continue;
                }
                candidate.seam_area += cut.cap_area;
                candidate.seam_length += length;
                candidate.visible += visible;
                candidate.fragile += f64::from(radius < 2.0);
                candidate.cuts += 1;
                next.push(candidate);
                // Bound live geometry instead of materializing the whole frontier.
                next.sort_by(|a, b| {
                    rank(a, o)
                        .total_cmp(&rank(b, o))
                        .then(a.parts.len().cmp(&b.parts.len()))
                });
                next.truncate(o.beam_width);
            }
        }
        if next.is_empty() {
            break;
        }
        next.sort_by(|a, b| {
            rank(a, o)
                .total_cmp(&rank(b, o))
                .then(a.parts.len().cmp(&b.parts.len()))
        });
        next.truncate(o.beam_width);
        for state in &next {
            if state.parts.iter().all(|p| p.fit_margin >= -1e-9)
                && best.as_ref().map_or(true, |b| rank(state, o) < rank(b, o))
            {
                best = Some(state.clone());
            }
        }
        if evaluations >= MAX_EVALUATIONS
            || next
                .iter()
                .all(|s| s.parts.iter().all(|p| p.fit_margin >= -1e-9))
        {
            break;
        }
        beam = next;
    }
    let chosen = best.ok_or(
        "bounded partition search found no fitting plan; increase bed size or pre-split the body",
    )?;
    let min_fit_margin = chosen
        .parts
        .iter()
        .map(|p| p.fit_margin)
        .fold(f64::INFINITY, f64::min);
    let cost = costs(&chosen, o);
    let mut geometry = Geometry::new();
    for (id, p) in chosen.parts.iter().enumerate() {
        let mut g = (*p.geo).clone();
        let n = g.prims.len();
        mesh::set_int_prim_attr(&mut g, mesh::PART, vec![id as i64; n]);
        geometry.merge(&g);
    }
    Ok(ResultPlan {
        geometry,
        parts: chosen.parts.len(),
        cuts: chosen.cuts,
        seam_area: chosen.seam_area,
        seam_length: chosen.seam_length,
        min_fit_margin,
        costs: cost,
        evaluations,
        limited: evaluations >= MAX_EVALUATIONS,
        guide_samples: guides.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::testutil::tri_box;
    fn opts() -> Options {
        Options {
            bed: DVec3::splat(350.),
            margin: 0.,
            beam_width: 4,
            weights: [1., 0., 1., 1., 0.],
        }
    }
    #[test]
    fn wp5_partition_fitting_body_needs_no_cuts() {
        let g = tri_box(DVec3::new(500., 20., 0.), DVec3::splat(100.));
        let r = search(&g, None, &opts()).unwrap();
        assert_eq!((r.parts, r.cuts, r.evaluations), (1, 0, 0));
        assert_eq!(r.geometry.bounds(), g.bounds());
        assert_eq!(r.geometry.prims.len(), g.prims.len());
        assert!((volume(&r.geometry) - volume(&g)).abs() < 1e-8);
        assert_eq!(r.min_fit_margin, 250.);
    }
    #[test]
    fn wp5_partition_is_deterministic_and_guides_affect_cost() {
        let g = tri_box(DVec3::ZERO, DVec3::new(600., 100., 100.));
        let guide = Geometry::from_points(vec![
            DVec3::new(280., 0., 50.),
            DVec3::new(280., 100., 50.),
            DVec3::new(280., 50., 0.),
            DVec3::new(280., 50., 100.),
        ]);
        let a = search(&g, Some(&guide), &opts()).unwrap();
        let b = search(&g, Some(&guide), &opts()).unwrap();
        let c = search(&g, None, &opts()).unwrap();
        assert_eq!(a.geometry, b.geometry);
        assert_eq!(a.costs, b.costs);
        assert_eq!(a.evaluations, b.evaluations);
        assert_eq!(a.guide_samples, 4);
        assert!(a.costs[2] < c.costs[2]);
        assert!(a.geometry.points.iter().any(|p| (p.x - 280.).abs() < 1e-8));
    }
    #[test]
    fn wp5_partition_rejects_invalid_and_excessive_data() {
        let g = tri_box(DVec3::ZERO, DVec3::splat(100.));
        for bad in [DVec3::splat(f64::NAN), DVec3::splat(1e300)] {
            let mut data = g.clone();
            data.points[0] = bad;
            assert!(search(&data, None, &opts()).is_err());
        }
        for bed in [
            DVec3::ZERO,
            DVec3::splat(f64::NAN),
            DVec3::splat(1e-100),
            DVec3::splat(1e300),
        ] {
            let mut o = opts();
            o.bed = bed;
            assert!(search(&g, None, &o).is_err());
        }
        let mut o = opts();
        o.beam_width = 0;
        assert!(search(&g, None, &o).is_err());
        let mut open = g.clone();
        open.prims.pop();
        assert!(search(&open, None, &opts()).is_err());
        let mut winding = g.clone();
        winding.prims[0].vertices.reverse();
        assert!(search(&winding, None, &opts()).is_err());
        let mut caps = g.clone();
        let n = caps.prims.len();
        mesh::set_int_prim_attr(&mut caps, mesh::CAP, vec![i64::MAX; n]);
        assert!(search(&caps, None, &opts()).is_err());
    }
    #[test]
    fn wp5_partition_connector_proxy_and_existing_caps() {
        let mut g = tri_box(DVec3::ZERO, DVec3::new(600., 1., 1.));
        let n = g.prims.len();
        mesh::set_int_prim_attr(&mut g, mesh::CAP, vec![5; n]);
        let r = search(&g, None, &opts()).unwrap();
        assert!(r.costs[3] > 0.);
        assert!(r.seam_length > 0.);
        assert!(r
            .geometry
            .prim_attribs
            .ints(mesh::CAP)
            .unwrap()
            .iter()
            .any(|&v| v > 5));
        assert_eq!(mesh::open_edges_by_part(&r.geometry), 0);
    }
}
