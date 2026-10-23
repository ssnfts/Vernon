//! Plane sections of a mesh as ordered polylines (`surface.section_curves`).

use std::collections::HashMap;

use glam::DVec3;
use vernon_core::{AttribArray, Geometry, Metric, Report};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::topology::{edge_key, weld};
use crate::util::{bbox_diagonal, check_finite, node_info, polylines_geometry, positive, SPEC_RE};

/// One chained section outline.
#[derive(Debug, Clone)]
pub struct Loop {
    pub points: Vec<DVec3>,
    /// `true` when the outline returns to its start (the first point is not repeated).
    pub closed: bool,
}

/// Intersect `geo` with the plane through `origin` with normal `normal` and
/// chain the crossing segments into open or closed loops.
///
/// Vertices on the plane (within `1e-9 x bbox diagonal`) count as the positive
/// side, which keeps the crossing rule consistent and avoids double hits.
pub fn section_mesh(geo: &Geometry, origin: DVec3, normal: DVec3) -> Vec<Loop> {
    let n = normal.normalize_or_zero();
    if n == DVec3::ZERO {
        return vec![];
    }
    let diag = bbox_diagonal(geo);
    let eps = 1e-9 * diag.max(1e-12);
    let w = weld(geo, 1e-7 * diag.max(1e-9));
    let d: Vec<f64> = w.points.iter().map(|p| n.dot(*p - origin)).collect();
    let positive = |i: u32| d[i as usize] >= -eps;

    let mut crossing_id: HashMap<(u32, u32), usize> = HashMap::new();
    let mut crossing_pt: Vec<DVec3> = Vec::new();
    let mut segments: Vec<(usize, usize)> = Vec::new();
    for ring in &w.faces {
        let mut hits: Vec<usize> = Vec::new();
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            if positive(a) == positive(b) {
                continue;
            }
            let key = edge_key(a, b).0;
            let id = *crossing_id.entry(key).or_insert_with(|| {
                let (pa, pb) = (w.points[key.0 as usize], w.points[key.1 as usize]);
                let (da, db) = (d[key.0 as usize], d[key.1 as usize]);
                let t = if (da - db).abs() > 0.0 { (da / (da - db)).clamp(0.0, 1.0) } else { 0.5 };
                crossing_pt.push(pa + (pb - pa) * t);
                crossing_pt.len() - 1
            });
            hits.push(id);
        }
        for pair in hits.chunks(2) {
            if pair.len() == 2 && pair[0] != pair[1] {
                segments.push((pair[0], pair[1]));
            }
        }
    }

    let mut at: Vec<Vec<usize>> = vec![Vec::new(); crossing_pt.len()];
    for (si, &(a, b)) in segments.iter().enumerate() {
        at[a].push(si);
        at[b].push(si);
    }
    let mut used = vec![false; segments.len()];
    let walk = |start: usize, used: &mut Vec<bool>| -> Vec<usize> {
        let mut seq = vec![start];
        let mut cur = start;
        while let Some(&si) = at[cur].iter().find(|&&s| !used[s]) {
            used[si] = true;
            let (a, b) = segments[si];
            cur = if a == cur { b } else { a };
            seq.push(cur);
            if cur == start {
                break;
            }
        }
        seq
    };
    let mut loops = Vec::new();
    // open chains first, starting from degree-1 crossings
    for c in 0..crossing_pt.len() {
        if at[c].len() == 1 && !used[at[c][0]] {
            let seq = walk(c, &mut used);
            loops.push((seq, false));
        }
    }
    for s in 0..segments.len() {
        if !used[s] {
            let start = segments[s].0;
            let seq = walk(start, &mut used);
            let closed = seq.len() > 2 && seq.first() == seq.last();
            loops.push((seq, closed));
        }
    }
    loops
        .into_iter()
        .map(|(mut seq, closed)| {
            if closed {
                seq.pop();
            }
            let mut points: Vec<DVec3> = seq.iter().map(|&i| crossing_pt[i]).collect();
            if closed && signed_area(&points, n) < 0.0 {
                points.reverse();
            }
            Loop { points, closed }
        })
        .collect()
}

/// Signed area of a closed loop about unit normal `n` (positive = counter-clockwise).
pub fn signed_area(pts: &[DVec3], n: DVec3) -> f64 {
    let mut a = DVec3::ZERO;
    for i in 0..pts.len() {
        a += pts[i].cross(pts[(i + 1) % pts.len()]);
    }
    0.5 * a.dot(n)
}

pub fn loop_length(l: &Loop) -> f64 {
    let n = l.points.len();
    let mut s: f64 = l.points.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    if l.closed && n > 1 {
        s += (l.points[0] - l.points[n - 1]).length();
    }
    s
}

/// `surface.section_curves`
pub struct SectionCurvesNode;

impl NodeOp for SectionCurvesNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.section_curves", "Mesh Section Curves", SPEC_RE)
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("sections", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("mode", "Mode", "plane", &["plane", "planes_along_axis"]).help("One cutting plane, or a stack of parallel planes."))
            .param(ParamSpec::vec3("origin", "Origin", DVec3::ZERO).unit("mm").help("A point on the first cutting plane."))
            .param(ParamSpec::vec3("normal", "Normal", DVec3::X).help("Plane normal; stations run along it."))
            .param(ParamSpec::int("count", "Plane count", 1, 1, 500).help("Number of parallel planes (stack mode)."))
            .param(ParamSpec::float("spacing_mm", "Spacing", 25.0, 0.1, 1000.0).unit("mm").help("Distance between parallel planes (stack mode)."))
            .param(ParamSpec::bool("from_bounds", "Start at bounds", false).help("Place the first plane half a spacing inside the mesh bounds along the normal."))
            .help("Cuts the mesh with one plane or a stack of parallel planes and returns the outlines as ordered polylines, one section id per plane. Closed outlines repeat their first point.")
            .emulates("Fusion mesh section sketch")
            .emulates("Design X mesh sketch")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = vernon_graph::node::inputs::geometry(inputs, 0)?;
        check_finite(ctx, &mesh)?;
        let normal = params.vec3("normal")?;
        if normal.length_squared() < 1e-24 {
            return Err(ctx.fail("section plane normal must not be zero"));
        }
        let nhat = normal.normalize();
        let origin = params.vec3("origin")?;
        let mode = params.str("mode")?.to_string();
        let count = if mode == "plane" { 1 } else { params.int("count")?.clamp(1, 500) as usize };
        let spacing = positive(ctx, "spacing_mm", params.float("spacing_mm")?)?;
        let b = mesh.bounds();
        let tmin = if b.is_empty() {
            0.0
        } else {
            (0..8)
                .map(|i| DVec3::new(if i & 1 == 0 { b.min.x } else { b.max.x }, if i & 2 == 0 { b.min.y } else { b.max.y }, if i & 4 == 0 { b.min.z } else { b.max.z }).dot(nhat))
                .fold(f64::INFINITY, f64::min)
        };

        let mut lines: Vec<(Vec<DVec3>, bool)> = Vec::new();
        let (mut sec_ids, mut loop_ids, mut closed_flags) = (Vec::new(), Vec::new(), Vec::new());
        let mut total_len = 0.0;
        let (mut closed_n, mut open_n) = (0usize, 0usize);
        let mut nonempty_sections = 0usize;
        for k in 0..count {
            let plane_origin = if params.bool("from_bounds")? && mode != "plane" {
                nhat * (tmin + 0.5 * spacing + k as f64 * spacing)
            } else {
                origin + nhat * (k as f64 * spacing)
            };
            let loops = section_mesh(&mesh, plane_origin, nhat);
            if !loops.is_empty() {
                nonempty_sections += 1;
            }
            for (li, l) in loops.iter().enumerate() {
                total_len += loop_length(l);
                if l.closed {
                    closed_n += 1;
                } else {
                    open_n += 1;
                }
                lines.push((l.points.clone(), l.closed));
                sec_ids.push(k as i64);
                loop_ids.push(li as i64);
                closed_flags.push(i64::from(l.closed));
            }
        }
        let mut geo = polylines_geometry(&lines);
        // polylines_geometry skips lines with < 2 points; keep attribs aligned
        let kept: Vec<usize> = lines.iter().enumerate().filter(|(_, (p, _))| p.len() >= 2).map(|(i, _)| i).collect();
        geo.prim_attribs.set("section_id", AttribArray::Int(kept.iter().map(|&i| sec_ids[i]).collect()));
        geo.prim_attribs.set("loop_id", AttribArray::Int(kept.iter().map(|&i| loop_ids[i]).collect()));
        geo.prim_attribs.set("loop_closed", AttribArray::Int(kept.iter().map(|&i| closed_flags[i]).collect()));

        let mut report = Report::new("Mesh section curves");
        report.metric("section_count", Metric::new(count as f64, "count"));
        report.metric("sections_with_curves", Metric::new(nonempty_sections as f64, "count"));
        report.metric("loop_count", Metric::new((closed_n + open_n) as f64, "count"));
        report.metric("closed_loop_count", Metric::new(closed_n as f64, "count"));
        report.metric("open_loop_count", Metric::new(open_n as f64, "count"));
        report.metric("total_length_mm", Metric::new(total_len, "mm"));
        if closed_n + open_n == 0 {
            report.note(vernon_core::Severity::Warning, "the cutting planes do not intersect the mesh");
        }
        Ok(vec![Data::geometry(geo), Data::report(report)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{sphere, tube};

    #[test]
    fn sphere_equator_is_one_circle() {
        let g = sphere(50.0, 64, 32);
        let loops = section_mesh(&g, DVec3::ZERO, DVec3::Z);
        assert_eq!(loops.len(), 1);
        assert!(loops[0].closed);
        // a UV sphere with an even band count has a vertex ring on the equator: plane through mesh vertices
        for p in &loops[0].points {
            assert!(p.z.abs() < 1e-9);
            assert!(((p.x * p.x + p.y * p.y).sqrt() - 50.0).abs() < 0.03);
        }
        assert!(signed_area(&loops[0].points, DVec3::Z) > 0.0);
    }

    #[test]
    fn plane_off_equator_gives_one_loop_without_double_hits() {
        let g = sphere(50.0, 64, 32);
        let loops = section_mesh(&g, DVec3::new(0.0, 0.0, 10.0), DVec3::Z);
        assert_eq!(loops.len(), 1);
        let pts = &loops[0].points;
        for i in 0..pts.len() {
            assert!((pts[i] - pts[(i + 1) % pts.len()]).length() > 1e-6, "duplicate point");
        }
    }

    #[test]
    fn hollow_cylinder_gives_two_loops() {
        let g = tube(25.0, 20.0, 100.0, 48);
        let loops = section_mesh(&g, DVec3::new(0.0, 0.0, 0.0), DVec3::Z);
        assert_eq!(loops.len(), 2);
        assert!(loops.iter().all(|l| l.closed));
    }

    #[test]
    fn open_mesh_gives_open_chain() {
        let g = crate::testutil::grid(10, 10, 100.0, 100.0, |_, _| 0.0);
        let loops = section_mesh(&g, DVec3::new(50.0, 0.0, 0.0), DVec3::X);
        assert_eq!(loops.len(), 1);
        assert!(!loops[0].closed);
        assert!((loop_length(&loops[0]) - 100.0).abs() < 1e-6);
    }
}
