//! Native local cross/position fields, independently implemented from Jakob et al.
//! (2015), sections 3.2--3.5 and Appendix A.
//! https://igl.ethz.ch/projects/instant-meshes/instant-meshes-SA-2015-jakob-et-al-compressed.pdf
//!
//! This module operates in each sample's 3D tangent plane. It does not construct
//! a global UV map. Varying spacing is a local extension of the paper's uniform
//! lattice; it allows integer-jump defects rather than guaranteeing an integral
//! globally consistent adaptive parameterization.

use glam::DVec3;

/// A representative of a four-fold tangent direction and square position lattice.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub direction: DVec3,
    pub position: DVec3,
}

struct Level {
    points: Vec<DVec3>,
    normals: Vec<DVec3>,
    edges: Vec<(u32, u32)>,
    neighbors: Vec<Vec<usize>>,
    constraints: Vec<Option<DVec3>>,
    anchors: Vec<Option<DVec3>>,
    spacing: Vec<f64>,
    mass: Vec<f64>,
    seeds: Vec<DVec3>,
    parent: Vec<usize>,
}

fn unit_normal(n: DVec3) -> DVec3 {
    if n.is_finite() && n.length_squared() > 1e-24 {
        n.normalize()
    } else {
        DVec3::Z
    }
}

fn tangent(v: DVec3, n: DVec3) -> DVec3 {
    (v - n * v.dot(n)).normalize_or_zero()
}

fn fallback_tangent(n: DVec3) -> DVec3 {
    let a = n.abs();
    let axis = if a.x <= a.y && a.x <= a.z {
        DVec3::X
    } else if a.y <= a.z {
        DVec3::Y
    } else {
        DVec3::Z
    };
    tangent(axis, n)
}

fn safe_spacing(h: f64) -> f64 {
    if h.is_finite() && h > 1e-12 {
        h
    } else {
        1e-12
    }
}

fn project(p: DVec3, v: DVec3, n: DVec3) -> DVec3 {
    p - n * (p - v).dot(n)
}

fn quarter(v: DVec3, n: DVec3, k: usize) -> DVec3 {
    match k % 4 {
        0 => v,
        1 => n.cross(v),
        2 => -v,
        _ => -n.cross(v),
    }
}

fn nearest_direction(reference: DVec3, direction: DVec3, normal: DVec3) -> DVec3 {
    let mut best = direction;
    let mut score = reference.dot(best);
    for k in 1..4 {
        let d = quarter(direction, normal, k);
        let s = reference.dot(d);
        if s > score + 1e-12 {
            best = d;
            score = s;
        }
    }
    best
}

fn adjacency(count: usize, edges: &[(u32, u32)]) -> Vec<Vec<usize>> {
    let mut neighbors = vec![Vec::new(); count];
    for &(a, b) in edges {
        neighbors[a as usize].push(b as usize);
        neighbors[b as usize].push(a as usize);
    }
    for list in &mut neighbors {
        list.sort_unstable();
        list.dedup();
    }
    neighbors
}

fn canonical_edges(count: usize, edges: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut edges: Vec<_> = edges
        .iter()
        .copied()
        .filter(|&(a, b)| a != b && (a as usize) < count && (b as usize) < count)
        .map(|(a, b)| if a < b { (a, b) } else { (b, a) })
        .collect();
    edges.sort_unstable();
    edges.dedup();
    edges
}

fn coarsen(fine: &Level) -> Option<(Level, Vec<usize>)> {
    let count = fine.points.len();
    let mut candidates = Vec::with_capacity(fine.edges.len());
    for &(a, b) in &fine.edges {
        let (a, b) = (a as usize, b as usize);
        let agreement = fine.normals[a].dot(fine.normals[b]);
        // Preserve opposing sheets and the position of constrained curves.
        if agreement < 0.25 || fine.constraints[a].is_some() != fine.constraints[b].is_some() {
            continue;
        }
        if let (Some(ca), Some(cb)) = (fine.constraints[a], fine.constraints[b]) {
            if ca.dot(cb).abs() < 0.9 {
                continue;
            }
        }
        let balance = (fine.mass[a] / fine.mass[b]).min(fine.mass[b] / fine.mass[a]);
        // Resolve floating-point ties identically under rigid motion.
        let score = (agreement * balance * 1e9).round() as i64;
        candidates.push((score, a, b));
    }
    candidates.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut mate = vec![usize::MAX; count];
    let mut merges = 0;
    for (_, a, b) in candidates {
        if mate[a] == usize::MAX && mate[b] == usize::MAX {
            mate[a] = b;
            mate[b] = a;
            merges += 1;
        }
    }
    if merges == 0 {
        return None;
    }
    let mut coarse = Level {
        points: Vec::new(),
        normals: Vec::new(),
        edges: Vec::new(),
        neighbors: Vec::new(),
        constraints: Vec::new(),
        anchors: Vec::new(),
        spacing: Vec::new(),
        mass: Vec::new(),
        seeds: Vec::new(),
        parent: Vec::new(),
    };
    let mut parent = vec![0; count];
    for a in 0..count {
        let b = mate[a];
        if b < a {
            continue;
        }
        let id = coarse.points.len();
        parent[a] = id;
        if b == usize::MAX {
            coarse.points.push(fine.points[a]);
            coarse.normals.push(fine.normals[a]);
            coarse.spacing.push(fine.spacing[a]);
            coarse.mass.push(fine.mass[a]);
            coarse.constraints.push(fine.constraints[a]);
            coarse.anchors.push(fine.anchors[a]);
            coarse.seeds.push(fine.seeds[a]);
            continue;
        }
        parent[b] = id;
        let wa = fine.mass[a];
        let wb = fine.mass[b];
        let w = wa + wb;
        let n = unit_normal(fine.normals[a] * wa + fine.normals[b] * wb);
        let seed_a = tangent(fine.seeds[a], n);
        let seed_b = tangent(fine.seeds[b], n);
        let seed = tangent(seed_a * wa + nearest_direction(seed_a, seed_b, n) * wb, n);
        let constraint = match (fine.constraints[a], fine.constraints[b]) {
            (Some(ca), Some(cb)) => Some(tangent(
                ca * wa + cb * (if ca.dot(cb) < 0.0 { -wb } else { wb }),
                n,
            )),
            (Some(c), None) | (None, Some(c)) => Some(tangent(c, n)),
            (None, None) => None,
        }
        .filter(|c| c.length_squared() > 0.5);
        coarse
            .points
            .push((fine.points[a] * wa + fine.points[b] * wb) / w);
        coarse.normals.push(n);
        coarse
            .spacing
            .push((fine.spacing[a] * wa + fine.spacing[b] * wb) / w);
        coarse.mass.push(w);
        coarse.constraints.push(constraint);
        let center = (fine.points[a] * wa + fine.points[b] * wb) / w;
        coarse
            .anchors
            .push(match (fine.anchors[a], fine.anchors[b]) {
                (Some(pa), Some(pb)) => Some(
                    if pa.distance_squared(center) <= pb.distance_squared(center) {
                        pa
                    } else {
                        pb
                    },
                ),
                (pa, pb) => pa.or(pb),
            });
        coarse.seeds.push(if seed.length_squared() > 0.5 {
            seed
        } else {
            fallback_tangent(n)
        });
    }
    let mapped: Vec<_> = fine
        .edges
        .iter()
        .map(|&(a, b)| (parent[a as usize] as u32, parent[b as usize] as u32))
        .collect();
    coarse.edges = canonical_edges(coarse.points.len(), &mapped);
    coarse.neighbors = adjacency(coarse.points.len(), &coarse.edges);
    Some((coarse, parent))
}

/// Deterministic extrinsic 4-RoSy and local PoSy fields on a surface graph.
///
/// The source must be sampled more finely than the requested lattice. A bounded
/// greedy hierarchy propagates long-range phase/orientation; local GS sweeps
/// refine it. This does not guarantee a globally integrable field or optimum.
pub fn solve(
    points: &[DVec3],
    normals: &[DVec3],
    edges: &[(u32, u32)],
    constraints: &[Option<DVec3>],
    spacing: &[f64],
    anchors: Option<&[Option<DVec3>]>,
) -> Vec<Field> {
    if points.is_empty() {
        return Vec::new();
    }
    let count = points.len();
    let normals: Vec<_> = (0..count)
        .map(|i| unit_normal(normals.get(i).copied().unwrap_or(DVec3::Z)))
        .collect();
    let edges = canonical_edges(count, edges);
    let neighbors = adjacency(count, &edges);
    let constraints: Vec<_> = (0..count)
        .map(|i| {
            constraints
                .get(i)
                .copied()
                .flatten()
                .map(|c| tangent(c, normals[i]))
                .filter(|c| c.is_finite() && c.length_squared() > 0.5)
        })
        .collect();
    let seeds: Vec<_> = (0..count)
        .map(|i| {
            constraints[i].unwrap_or_else(|| {
                neighbors[i]
                    .iter()
                    .map(|&j| tangent(points[j] - points[i], normals[i]))
                    .find(|d| d.is_finite() && d.length_squared() > 0.5)
                    .unwrap_or_else(|| fallback_tangent(normals[i]))
            })
        })
        .collect();
    let mut levels = vec![Level {
        points: points.to_vec(),
        normals,
        edges,
        neighbors,
        constraints,
        anchors: (0..count)
            .map(|i| anchors.and_then(|a| a.get(i)).copied().flatten())
            .collect(),
        seeds,
        spacing: (0..count)
            .map(|i| safe_spacing(spacing.get(i).copied().unwrap_or(1.0)))
            .collect(),
        mass: vec![1.0; count],
        parent: Vec::new(),
    }];
    // Retain a few coarse samples rather than seed a collapsed plane from a
    // world axis. Geometry-derived seeds and deterministic ties preserve pose.
    while levels.len() < 20 && levels.last().unwrap().points.len() > 8 {
        let Some((coarse, parent)) = coarsen(levels.last().unwrap()) else {
            break;
        };
        levels.last_mut().unwrap().parent = parent;
        levels.push(coarse);
    }
    let mut directions: Vec<Vec<DVec3>> = (0..levels.len()).map(|_| Vec::new()).collect();
    for l in (0..levels.len()).rev() {
        let level = &levels[l];
        let mut values: Vec<_> = (0..level.points.len())
            .map(|i| {
                let d = if l + 1 < levels.len() {
                    tangent(directions[l + 1][level.parent[i]], level.normals[i])
                } else {
                    level.seeds[i]
                };
                if d.length_squared() > 0.5 {
                    d
                } else {
                    level.seeds[i]
                }
            })
            .collect();
        smooth_orientations(level, &mut values);
        directions[l] = values;
    }
    let mut fields: Vec<Field> = Vec::new();
    for l in (0..levels.len()).rev() {
        let level = &levels[l];
        let mut fine: Vec<_> = (0..level.points.len())
            .map(|i| {
                let p = if l + 1 < levels.len() {
                    fields[level.parent[i]].position
                } else {
                    level.points[i]
                };
                let mut f = Field {
                    direction: directions[l][i],
                    position: project(p, level.points[i], level.normals[i]),
                };
                constrain_position(level, i, &mut f);
                f
            })
            .collect();
        smooth_positions(level, &mut fine);
        fields = fine;
    }
    fields
}

fn smooth_orientations(level: &Level, values: &mut [DVec3]) {
    for sweep in 0..8 {
        for ordinal in 0..values.len() {
            let i = if sweep % 2 == 0 {
                ordinal
            } else {
                values.len() - 1 - ordinal
            };
            let n = level.normals[i];
            let mut sum = DVec3::ZERO;
            for &j in &level.neighbors[i] {
                let mut best = -f64::INFINITY;
                let mut contribution = values[j];
                // Search both fourfold symmetry sets in ambient 3D, then undo
                // i's chosen quarter turn before accumulating in its gauge.
                for a in 0..4 {
                    let da = quarter(values[i], n, a);
                    for b in 0..4 {
                        let db = quarter(values[j], level.normals[j], b);
                        let score = da.dot(db);
                        if score > best + 1e-12 {
                            best = score;
                            contribution = quarter(db, n, (4 - a) % 4);
                        }
                    }
                }
                sum += contribution;
                let direction = tangent(sum, n);
                if direction.length_squared() > 0.5 {
                    values[i] = direction;
                }
            }
            if let Some(c) = level.constraints[i] {
                values[i] = nearest_direction(values[i], c, n);
            }
        }
    }
}

fn stable_floor(x: f64) -> f64 {
    let nearest = x.round();
    if (x - nearest).abs() < 1e-10 {
        nearest
    } else {
        x.floor()
    }
}

fn recenter(v: DVec3, n: DVec3, h: f64, field: &mut Field) {
    let u = field.direction;
    let w = n.cross(u);
    let d = (v - field.position) / h;
    field.position += h * (u * stable_floor(d.dot(u) + 0.5) + w * stable_floor(d.dot(w) + 0.5));
}

fn constrain_position(level: &Level, i: usize, field: &mut Field) {
    if let Some(p) = level.anchors[i] {
        field.position = p;
        return;
    }
    let v = level.points[i];
    let n = level.normals[i];
    field.position = project(field.position, v, n);
    if let Some(c) = level.constraints[i] {
        field.position = v + c * (field.position - v).dot(c);
    }
    recenter(v, n, level.spacing[i], field);
}

fn smooth_positions(level: &Level, fields: &mut [Field]) {
    for sweep in 0..10 {
        for ordinal in 0..fields.len() {
            let i = if sweep % 2 == 0 {
                ordinal
            } else {
                fields.len() - 1 - ordinal
            };
            let mut sum = DVec3::ZERO;
            let mut weight = 0.0;
            for &j in &level.neighbors[i] {
                let pair = compatible_pair(
                    level.points[i],
                    level.normals[i],
                    &fields[i],
                    level.spacing[i],
                    level.points[j],
                    level.normals[j],
                    &fields[j],
                    level.spacing[j],
                );
                // Translate j's representative into i's current lattice gauge.
                sum += fields[j].position + pair.shift_b - pair.shift_a;
                weight += 1.0;
                fields[i].position = project(sum / weight, level.points[i], level.normals[i]);
            }
            constrain_position(level, i, &mut fields[i]);
        }
    }
}

struct Pair {
    index_a: [i32; 2],
    index_b: [i32; 2],
    shift_a: DVec3,
    shift_b: DVec3,
}

fn intermediate(a: DVec3, na: DVec3, b: DVec3, nb: DVec3) -> DVec3 {
    let c = na.dot(nb).clamp(-1.0, 1.0);
    let d = b - a;
    let denominator = 1.0 - c * c + 1e-4;
    let la = 2.0 * (na + c * nb).dot(d) / denominator;
    let lb = -2.0 * (nb + c * na).dot(d) / denominator;
    (a + b) * 0.5 - (na * la + nb * lb) * 0.25
}

fn compatible_pair(
    a: DVec3,
    na: DVec3,
    fa: &Field,
    ha: f64,
    b: DVec3,
    nb: DVec3,
    fb: &Field,
    hb: f64,
) -> Pair {
    let ua = fa.direction;
    let va = na.cross(ua);
    // Independently represented crosses may differ by a quarter turn while
    // describing the same lattice. Give both bases corresponding axis names.
    let mut ub = fb.direction;
    let mut vb = nb.cross(ub);
    let mut agreement = ua.dot(ub) + va.dot(vb);
    for k in 1..4 {
        let u = quarter(fb.direction, nb, k);
        let v = nb.cross(u);
        let score = ua.dot(u) + va.dot(v);
        if score > agreement + 1e-12 {
            ub = u;
            vb = v;
            agreement = score;
        }
    }
    let q = intermediate(a, na, b, nb);
    let ca = (q - fa.position) / ha;
    let cb = (q - fb.position) / hb;
    let base_a = [
        stable_floor(ca.dot(ua)) as i32,
        stable_floor(ca.dot(va)) as i32,
    ];
    let base_b = [
        stable_floor(cb.dot(ub)) as i32,
        stable_floor(cb.dot(vb)) as i32,
    ];
    let mut best = f64::INFINITY;
    let mut proximity = f64::INFINITY;
    let mut result = Pair {
        index_a: [0; 2],
        index_b: [0; 2],
        shift_a: DVec3::ZERO,
        shift_b: DVec3::ZERO,
    };
    let tolerance = 1e-12 * ha.max(hb).powi(2);
    for x in 0..4 {
        let ia = [
            base_a[0].saturating_add(x & 1),
            base_a[1].saturating_add(x >> 1),
        ];
        let sa = ha * (ua * ia[0] as f64 + va * ia[1] as f64);
        let pa = fa.position + sa;
        for y in 0..4 {
            let ib = [
                base_b[0].saturating_add(y & 1),
                base_b[1].saturating_add(y >> 1),
            ];
            let sb = hb * (ub * ib[0] as f64 + vb * ib[1] as f64);
            let pb = fb.position + sb;
            let error = pa.distance_squared(pb);
            let near = pa.distance_squared(q) + pb.distance_squared(q);
            if error < best - tolerance
                || ((error - best).abs() <= tolerance && near < proximity - tolerance)
            {
                best = error;
                proximity = near;
                result = Pair {
                    index_a: ia,
                    index_b: ib,
                    shift_a: sa,
                    shift_b: sb,
                };
            }
        }
    }
    result
}

/// Integer displacement from `fa` to `fb` in `fa`'s quarter-turn gauge, plus
/// squared distance of their canonical position representatives. This distance
/// orders zero-jump contractions as in section 3.8. Zero denotes a candidate
/// merge; a displacement of L1 norm one denotes a candidate output edge.
pub fn jump(
    a: DVec3,
    na: DVec3,
    fa: &Field,
    ha: f64,
    b: DVec3,
    nb: DVec3,
    fb: &Field,
    hb: f64,
) -> ([i32; 2], f64) {
    let pair = compatible_pair(
        a,
        unit_normal(na),
        fa,
        safe_spacing(ha),
        b,
        unit_normal(nb),
        fb,
        safe_spacing(hb),
    );
    (
        [
            pair.index_a[0].saturating_sub(pair.index_b[0]),
            pair.index_a[1].saturating_sub(pair.index_b[1]),
        ],
        fa.position.distance_squared(fb.position),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DQuat;

    fn grid(side: usize) -> (Vec<DVec3>, Vec<DVec3>, Vec<(u32, u32)>, Vec<Option<DVec3>>) {
        let mut points = Vec::new();
        let mut edges = Vec::new();
        let mut constraints = Vec::new();
        for y in 0..side {
            for x in 0..side {
                points.push(DVec3::new(
                    x as f64 / (side - 1) as f64,
                    y as f64 / (side - 1) as f64,
                    0.0,
                ));
                let i = (y * side + x) as u32;
                if x + 1 < side {
                    edges.push((i, i + 1));
                }
                if y + 1 < side {
                    edges.push((i, i + side as u32));
                }
                if x + 1 < side && y + 1 < side {
                    edges.push((i, i + side as u32 + 1));
                }
                constraints.push(if y == 0 || y + 1 == side {
                    Some(DVec3::X)
                } else if x == 0 || x + 1 == side {
                    Some(DVec3::Y)
                } else {
                    None
                });
            }
        }
        let normals = vec![DVec3::Z; points.len()];
        (points, normals, edges, constraints)
    }

    #[test]
    fn equivalent_planar_lattices_respect_quarter_turn_gauges() {
        let a = Field {
            direction: DVec3::X,
            position: DVec3::ZERO,
        };
        let b = Field {
            direction: DVec3::Y,
            position: DVec3::X,
        };
        let (d, _) = jump(
            DVec3::new(0.1, 0.1, 0.0),
            DVec3::Z,
            &a,
            1.0,
            DVec3::new(0.9, 0.1, 0.0),
            DVec3::Z,
            &b,
            1.0,
        );
        assert_eq!(d, [1, 0]);
        let b = Field {
            direction: -DVec3::X,
            position: DVec3::ZERO,
        };
        let (d, e) = jump(
            DVec3::new(0.1, 0.1, 0.0),
            DVec3::Z,
            &a,
            1.0,
            DVec3::new(0.2, 0.1, 0.0),
            DVec3::Z,
            &b,
            1.0,
        );
        assert_eq!(d, [0, 0]);
        assert!(e < 1e-20);
    }

    #[test]
    fn solved_fields_are_rigid_rotation_equivariant() {
        let (p, n, e, c) = grid(11);
        let h = vec![0.2; p.len()];
        let f = solve(&p, &n, &e, &c, &h, None);
        let rot = DQuat::from_axis_angle(DVec3::new(1.0, 2.0, 3.0).normalize(), 0.73);
        let offset = DVec3::new(2.0, -4.0, 5.0);
        let rp: Vec<_> = p.iter().map(|&v| rot * v + offset).collect();
        let rn: Vec<_> = n.iter().map(|&v| rot * v).collect();
        let rc: Vec<_> = c.iter().map(|v| v.map(|d| rot * d)).collect();
        let rf = solve(&rp, &rn, &e, &rc, &h, None);
        for (a, b) in f.iter().zip(&rf) {
            let desired = rot * a.direction;
            let t = rn[0].cross(b.direction);
            assert!(desired.dot(b.direction).abs().max(desired.dot(t).abs()) > 1.0 - 1e-7);
            // A position field is defined modulo its lattice, so compare its
            // nearest equivalent to the transformed original representative.
            let expected = rot * a.position + offset;
            let delta = expected - b.position;
            let du = delta.dot(b.direction) / 0.2;
            let dv = delta.dot(t) / 0.2;
            assert!((du - du.round()).abs() < 1e-5, "phase u: {du}");
            assert!((dv - dv.round()).abs() < 1e-5, "phase v: {dv}");
        }
    }

    #[test]
    fn multilevel_dense_sampling_has_many_zero_jumps() {
        for side in [9, 33] {
            let (p, n, e, c) = grid(side);
            let h = vec![0.25; p.len()];
            let fields = solve(&p, &n, &e, &c, &h, None);
            let mut zeros = 0;
            for &(a, b) in &e {
                let (a, b) = (a as usize, b as usize);
                let (d, _) = jump(p[a], n[a], &fields[a], h[a], p[b], n[b], &fields[b], h[b]);
                zeros += usize::from(d == [0, 0]);
            }
            let fraction = zeros as f64 / e.len() as f64;
            assert!(
                fraction > if side == 9 { 0.15 } else { 0.65 },
                "side={side}, zero fraction={fraction}"
            );
            for (i, f) in fields.iter().enumerate() {
                assert!(f.position.is_finite() && f.direction.is_finite());
                assert!((f.direction.length() - 1.0).abs() < 1e-9);
                assert!(n[i].dot(f.position - p[i]).abs() < 1e-9);
            }
        }
    }
}
