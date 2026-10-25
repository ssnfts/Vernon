//! Native field extraction after Jakob et al., 2015, section 3.8.
//! The orientation/position solver is in `remesh_fields`; this module builds
//! the lattice graph, contracts zero jumps and walks its oriented edges.

use crate::bvh::TriBvh;
use crate::remesh_fields;
use crate::topology::{edge_key, edge_map, tri_adjacency, weld};
use glam::DVec3;
use std::collections::{BTreeSet, HashMap};
use vernon_core::Geometry;

fn refine(points: &mut Vec<DVec3>, tris: &mut Vec<[u32; 3]>) {
    let mut cache = HashMap::new();
    let mut out = Vec::with_capacity(tris.len() * 4);
    for &[a, b, c] in tris.iter() {
        let mut mid = |a: u32, b: u32| {
            *cache.entry(edge_key(a, b).0).or_insert_with(|| {
                let p = (points[a as usize] + points[b as usize]) * 0.5;
                points.push(p);
                (points.len() - 1) as u32
            })
        };
        let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
        out.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
    }
    *tris = out;
}

fn normal(p: &[DVec3], t: [u32; 3]) -> DVec3 {
    (p[t[1] as usize] - p[t[0] as usize]).cross(p[t[2] as usize] - p[t[0] as usize])
}

/// Remesh sharp-feature-separated patches, using one global target spacing.
pub fn remesh(
    points: &[DVec3],
    tris: &[[u32; 3]],
    edge: f64,
    hard_cos: f64,
    adaptive: f64,
) -> Result<Geometry, String> {
    let mut points = points.to_vec();
    let mut tris: Vec<_> = tris
        .iter()
        .copied()
        .filter(|&t| normal(&points, t).length_squared() > 1e-24)
        .collect();
    if tris.is_empty() {
        return Err("input has no non-degenerate triangles".into());
    }
    // The extraction graph requires a denser sampling than the output lattice.
    // Uniform refinement preserves the given faceted surface exactly.
    for _ in 0..8 {
        let mean = tris
            .iter()
            .map(|t| {
                (0..3)
                    .map(|k| points[t[k] as usize].distance(points[t[(k + 1) % 3] as usize]))
                    .sum::<f64>()
            })
            .sum::<f64>()
            / (3 * tris.len()) as f64;
        if mean <= edge * 0.8 {
            break;
        }
        if tris.len() > 250_000 {
            return Err("input refinement would exceed one million triangles".into());
        }
        refine(&mut points, &mut tris);
    }
    let normals: Vec<_> = tris
        .iter()
        .map(|&t| normal(&points, t).normalize())
        .collect();
    let adj = tri_adjacency(&tris);
    let mut seen = vec![false; tris.len()];
    let mut output = Geometry::new();
    for seed in 0..tris.len() {
        if seen[seed] {
            continue;
        }
        seen[seed] = true;
        let mut patch = vec![seed];
        let mut head = 0;
        while head < patch.len() {
            let i = patch[head];
            head += 1;
            for j in adj[i].iter().flatten().map(|&j| j as usize) {
                if !seen[j] && normals[i].dot(normals[j]) >= hard_cos {
                    seen[j] = true;
                    patch.push(j);
                }
            }
        }
        let mut map = HashMap::new();
        let mut pp = Vec::new();
        let tt: Vec<[u32; 3]> = patch
            .iter()
            .map(|&i| {
                tris[i].map(|v| {
                    *map.entry(v).or_insert_with(|| {
                        pp.push(points[v as usize]);
                        (pp.len() - 1) as u32
                    })
                })
            })
            .collect();
        let g = extract(&pp, &tt, edge * 2.0, adaptive)?;
        let base = output.points.len() as u32;
        output.points.extend(g.points);
        output.prims.extend(g.prims.into_iter().map(|mut p| {
            for v in &mut p.vertices {
                *v += base;
            }
            p
        }));
    }
    let w = weld(&output, edge * 1e-7);
    Ok(Geometry::from_polygons(w.points, w.faces))
}

fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

fn boundary_projection(p: DVec3, points: &[DVec3], edges: &[(u32, u32)]) -> DVec3 {
    let mut best = (f64::INFINITY, p);
    for &(a, b) in edges {
        let a = points[a as usize];
        let d = points[b as usize] - a;
        let q = a + d * ((p - a).dot(d) / d.length_squared().max(1e-30)).clamp(0.0, 1.0);
        let dist = p.distance_squared(q);
        if dist < best.0 {
            best = (dist, q);
        }
    }
    best.1
}

/// Split a closed boundary walk at repeated vertices. A singularity may leave
/// two missing cells touching at one vertex, rather than one simple polygon.
fn simple_cycles(walk: Vec<u32>) -> Vec<Vec<u32>> {
    let mut pending = vec![walk];
    let mut result = Vec::new();
    while let Some(ring) = pending.pop() {
        let mut seen = HashMap::new();
        let mut split = None;
        for (i, &v) in ring.iter().enumerate() {
            if let Some(j) = seen.insert(v, i) {
                split = Some((j, i));
                break;
            }
        }
        if let Some((a, b)) = split {
            pending.push(ring[a..b].to_vec());
            pending.push(ring[..a].iter().chain(&ring[b..]).copied().collect());
        } else if ring.len() >= 3 {
            result.push(ring);
        }
    }
    result
}

/// Shared arc-length samples between geometric corners, independent of each
/// feature patch's field phase and interior triangulation.
fn boundary_anchors(
    points: &[DVec3],
    edges: &[(u32, u32)],
    neighbors: &[Vec<usize>],
    corners: &[bool],
    spacing: f64,
) -> (Vec<DVec3>, Vec<DVec3>) {
    let mut used = BTreeSet::new();
    let mut coarse = Vec::new();
    let mut fine = Vec::new();
    let starts = corners
        .iter()
        .enumerate()
        .filter(|(_, c)| **c)
        .map(|(i, _)| i)
        .chain(0..points.len());
    for a in starts {
        for &b in &neighbors[a] {
            if used.contains(&edge_key(a as u32, b as u32).0) {
                continue;
            }
            let mut chain = vec![a];
            let (mut prev, mut cur) = (a, b);
            loop {
                used.insert(edge_key(prev as u32, cur as u32).0);
                chain.push(cur);
                if cur == a || corners[cur] || neighbors[cur].len() != 2 {
                    break;
                }
                let next = neighbors[cur].iter().copied().find(|&v| v != prev).unwrap();
                prev = cur;
                cur = next;
                if chain.len() > edges.len() + 1 {
                    break;
                }
            }
            // A cornerless closed seam has no natural first vertex. Patch-local
            // indexing must not choose its arc-length phase or traversal order.
            if chain.first() == chain.last() && !chain.iter().any(|&v| corners[v]) {
                chain.pop();
                let compare = |a: usize, b: usize| {
                    points[a]
                        .x
                        .total_cmp(&points[b].x)
                        .then(points[a].y.total_cmp(&points[b].y))
                        .then(points[a].z.total_cmp(&points[b].z))
                };
                let first = (0..chain.len())
                    .min_by(|&a, &b| compare(chain[a], chain[b]))
                    .unwrap();
                chain.rotate_left(first);
                if compare(chain[1], *chain.last().unwrap()).is_gt() {
                    chain[1..].reverse();
                }
                chain.push(chain[0]);
            }
            let length = chain
                .windows(2)
                .map(|p| points[p[0]].distance(points[p[1]]))
                .sum::<f64>();
            let count = (length / spacing).round().max(1.0) as usize;
            let mut segment = 0;
            let mut start = 0.0;
            for k in 0..=count * 2 {
                let distance = length * k as f64 / (count * 2) as f64;
                while segment + 2 < chain.len()
                    && start + points[chain[segment]].distance(points[chain[segment + 1]])
                        < distance
                {
                    start += points[chain[segment]].distance(points[chain[segment + 1]]);
                    segment += 1;
                }
                let p = points[chain[segment]];
                let q = points[chain[segment + 1]];
                let value = if k == count * 2 {
                    points[*chain.last().unwrap()]
                } else {
                    p.lerp(
                        q,
                        ((distance - start) / p.distance(q).max(1e-30)).clamp(0.0, 1.0),
                    )
                };
                fine.push(value);
                if k % 2 == 0 {
                    coarse.push(value);
                }
            }
        }
    }
    (coarse, fine)
}

fn nearest_anchor(p: DVec3, anchors: &[DVec3]) -> DVec3 {
    anchors
        .iter()
        .copied()
        .min_by(|a, b| a.distance_squared(p).total_cmp(&b.distance_squared(p)))
        .unwrap_or(p)
}

/// Projected local quality improvement. Every accepted move raises the worst
/// Jacobian in its incident faces, so it cannot trade one inversion for another.
fn improve_quads(points: &mut [DVec3], faces: &[Vec<u32>], pinned: &[bool], surface: &TriBvh) {
    let mut incident = vec![Vec::new(); points.len()];
    let mut neighbors = vec![BTreeSet::new(); points.len()];
    for (fi, f) in faces.iter().enumerate() {
        for k in 0..4 {
            let a = f[k] as usize;
            let b = f[(k + 1) % 4] as usize;
            incident[a].push(fi);
            neighbors[a].insert(b);
            neighbors[b].insert(a);
        }
    }
    let quality = |points: &[DVec3], fs: &[usize]| {
        fs.iter()
            .map(|&fi| {
                let f = &faces[fi];
                crate::quad::quad_corner_jacobians(&[
                    points[f[0] as usize],
                    points[f[1] as usize],
                    points[f[2] as usize],
                    points[f[3] as usize],
                ])
                .into_iter()
                .fold(1.0, f64::min)
            })
            .fold(1.0, f64::min)
    };
    for _ in 0..12 {
        let mut changed = false;
        for i in 0..points.len() {
            if pinned[i] || neighbors[i].is_empty() {
                continue;
            }
            let old = points[i];
            let old_quality = quality(points, &incident[i]);
            if old_quality >= 0.15 {
                continue;
            }
            let avg =
                neighbors[i].iter().map(|&j| points[j]).sum::<DVec3>() / neighbors[i].len() as f64;
            let mut best = old;
            let mut best_quality = old_quality;
            for factor in [0.25, 0.5, 1.0] {
                points[i] = surface.closest(old.lerp(avg, factor)).unwrap().point;
                let q = quality(points, &incident[i]);
                if q > best_quality + 1e-10 {
                    best_quality = q;
                    best = points[i];
                }
            }
            points[i] = best;
            changed |= best_quality > old_quality + 1e-10;
        }
        if !changed {
            break;
        }
    }
}

fn extract(
    points: &[DVec3],
    tris: &[[u32; 3]],
    spacing: f64,
    adaptive: f64,
) -> Result<Geometry, String> {
    let bvh = TriBvh::new(points.to_vec(), tris);
    let mut normals = vec![DVec3::ZERO; points.len()];
    for &t in tris {
        let n = normal(points, t);
        for v in t {
            normals[v as usize] += n;
        }
    }
    for n in &mut normals {
        *n = n.normalize_or_zero();
    }
    let faces: Vec<_> = tris.iter().map(|t| t.to_vec()).collect();
    let em = edge_map(&faces);
    if em.values().any(|f| f.len() > 2) {
        return Err("non-manifold input edge".into());
    }
    let mut edges: Vec<_> = em.keys().copied().collect();
    edges.sort_unstable();
    let boundary: Vec<_> = edges.iter().copied().filter(|e| em[e].len() == 1).collect();
    let mut bn = vec![Vec::new(); points.len()];
    let mut adjacency = vec![Vec::new(); points.len()];
    for &(a, b) in &edges {
        adjacency[a as usize].push(b as usize);
        adjacency[b as usize].push(a as usize);
    }
    for &(a, b) in &boundary {
        bn[a as usize].push(b as usize);
        bn[b as usize].push(a as usize);
    }
    let mut corners = vec![false; points.len()];
    let constraints: Vec<_> = bn
        .iter()
        .enumerate()
        .map(|(i, ns)| {
            if ns.is_empty() {
                return None;
            }
            if ns.len() == 2 {
                let a = (points[ns[0]] - points[i]).normalize_or_zero();
                let b = (points[ns[1]] - points[i]).normalize_or_zero();
                corners[i] = a.dot(b) > -0.8;
                Some(if corners[i] {
                    a
                } else {
                    (points[ns[1]] - points[ns[0]]).normalize_or_zero()
                })
            } else {
                corners[i] = true;
                Some((points[ns[0]] - points[i]).normalize_or_zero())
            }
        })
        .collect();
    let local_spacing: Vec<_> = adjacency
        .iter()
        .enumerate()
        .map(|(i, ns)| {
            let curvature = ns
                .iter()
                .map(|&j| {
                    (normals[i] - normals[j]).length() / points[i].distance(points[j]).max(1e-12)
                })
                .fold(0.0, f64::max);
            spacing
                / (1.0 + adaptive * spacing * curvature)
                    .sqrt()
                    .clamp(1.0, 2.0)
        })
        .collect();
    let (boundary_coarse, boundary_fine) =
        boundary_anchors(points, &boundary, &bn, &corners, spacing);
    let anchors: Vec<_> = (0..points.len())
        .map(|i| {
            if corners[i] {
                Some(points[i])
            } else if !bn[i].is_empty() {
                Some(nearest_anchor(points[i], &boundary_coarse))
            } else {
                None
            }
        })
        .collect();
    let mut field = remesh_fields::solve(
        points,
        &normals,
        &edges,
        &constraints,
        &local_spacing,
        Some(&anchors),
    );
    for i in 0..points.len() {
        if corners[i] {
            field[i].position = points[i];
        } else if !bn[i].is_empty() {
            field[i].position = nearest_anchor(points[i], &boundary_coarse);
        }
    }
    let mut graph = vec![BTreeSet::new(); points.len()];
    let mut candidates = Vec::new();
    for &(a, b) in &edges {
        let (a, b) = (a as usize, b as usize);
        let (jump, dist) = remesh_fields::jump(
            points[a],
            normals[a],
            &field[a],
            local_spacing[a],
            points[b],
            normals[b],
            &field[b],
            local_spacing[b],
        );
        let len = jump[0].abs() + jump[1].abs();
        if len == 0 {
            candidates.push((dist, a, b));
        } else if len == 1 {
            graph[a].insert(b);
            graph[b].insert(a);
        }
    }
    candidates.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut parent: Vec<_> = (0..points.len()).collect();
    let mut corner: Vec<_> = corners
        .iter()
        .enumerate()
        .map(|(i, &c)| c.then_some(i))
        .collect();
    for (_, a, b) in candidates {
        let a = root(&mut parent, a);
        let b = root(&mut parent, b);
        if a == b || graph[a].contains(&b) || (corner[a].is_some() && corner[b].is_some()) {
            continue;
        }
        let (a, b) = (a.min(b), a.max(b));
        parent[b] = a;
        corner[a] = corner[a].or(corner[b]);
        let neighbors = std::mem::take(&mut graph[b]);
        for n in neighbors {
            graph[n].remove(&b);
            if n != a {
                graph[n].insert(a);
                graph[a].insert(n);
            }
        }
    }
    let mut sums = vec![DVec3::ZERO; points.len()];
    let mut nsums = sums.clone();
    let mut weights = vec![0.0; points.len()];
    let mut counts = vec![0usize; points.len()];
    let mut border = vec![false; points.len()];
    for i in 0..points.len() {
        let r = root(&mut parent, i);
        let weight = (-points[i].distance_squared(field[i].position)
            / (local_spacing[i] / 3.0).powi(2))
        .exp()
        .max(1e-12);
        sums[r] += field[i].position * weight;
        nsums[r] += normals[i] * weight;
        weights[r] += weight;
        counts[r] += 1;
        border[r] |= !bn[i].is_empty();
    }
    let mut map = vec![usize::MAX; points.len()];
    let cluster_count = (0..points.len())
        .filter(|&r| parent[r] == r && graph[r].len() >= 2)
        .count()
        .max(1);
    let min_samples = points.len() as f64 / cluster_count as f64 / 10.0;
    for r in 0..points.len() {
        if parent[r] == r && !border[r] && (counts[r] as f64) < min_samples {
            for n in std::mem::take(&mut graph[r]) {
                graph[n].remove(&r);
            }
        }
    }
    // Interior graph vertices of valence one/two cannot be regular surface
    // vertices. Suppress dangling branches and redundant two-edge paths before
    // the angular walk; otherwise a folded path can consume a boundary cell.
    let mut pending: Vec<_> = (0..points.len())
        .filter(|&r| parent[r] == r && !border[r] && graph[r].len() <= 2)
        .collect();
    while let Some(r) = pending.pop() {
        if border[r] || graph[r].is_empty() || graph[r].len() > 2 {
            continue;
        }
        let neighbors: Vec<_> = std::mem::take(&mut graph[r]).into_iter().collect();
        for &n in &neighbors {
            graph[n].remove(&r);
        }
        if neighbors.len() == 2 {
            graph[neighbors[0]].insert(neighbors[1]);
            graph[neighbors[1]].insert(neighbors[0]);
        }
        for n in neighbors {
            if !border[n] && graph[n].len() <= 2 {
                pending.push(n);
            }
        }
    }
    let mut positions = Vec::new();
    let mut nn = Vec::new();
    let mut on_boundary = Vec::new();
    for r in 0..points.len() {
        if parent[r] != r || graph[r].len() < 2 {
            continue;
        }
        // Section 3.8 removes tiny unsupported clusters left by conflicting
        // jumps. Preserve source boundary anchors even when sparsely sampled.
        let avg = sums[r] / weights[r];
        let p = if let Some(i) = corner[r] {
            points[i]
        } else if border[r] {
            nearest_anchor(avg, &boundary_coarse)
        } else {
            bvh.closest(avg).unwrap().point
        };
        map[r] = positions.len();
        positions.push(p);
        nn.push(nsums[r].normalize_or_zero());
        on_boundary.push(border[r]);
    }
    let mut rings = vec![Vec::new(); positions.len()];
    for r in 0..points.len() {
        if map[r] == usize::MAX {
            continue;
        }
        let i = map[r];
        let q = field[r].direction;
        let t = nn[i].cross(q).normalize_or_zero();
        let mut links: Vec<_> = graph[r]
            .iter()
            .copied()
            .filter(|&j| map[j] != usize::MAX)
            .map(|j| map[j])
            .collect();
        links.sort_by(|&a, &b| {
            let da = positions[a] - positions[i];
            let db = positions[b] - positions[i];
            da.dot(t)
                .atan2(da.dot(q))
                .total_cmp(&db.dot(t).atan2(db.dot(q)))
                .then(a.cmp(&b))
        });
        rings[i] = links;
    }
    let mut visited = BTreeSet::new();
    let mut polygons = Vec::new();
    for a in 0..rings.len() {
        for &b in &rings[a] {
            if visited.contains(&(a, b)) {
                continue;
            }
            let (mut u, mut v) = (a, b);
            let mut poly = Vec::new();
            let mut closed = false;
            for _ in 0..128 {
                if !visited.insert((u, v)) {
                    break;
                }
                poly.push(u as u32);
                let Some(k) = rings[v].iter().position(|&n| n == u) else {
                    break;
                };
                let next = rings[v][(k + rings[v].len() - 1) % rings[v].len()];
                u = v;
                v = next;
                if (u, v) == (a, b) {
                    closed = true;
                    break;
                }
            }
            if !closed {
                continue;
            }
            for poly in simple_cycles(poly) {
                if poly.len() > 32 {
                    continue;
                }
                let origin = positions[poly[0] as usize];
                let mut n = DVec3::ZERO;
                let mut avg = DVec3::ZERO;
                for k in 0..poly.len() {
                    let i = poly[k] as usize;
                    let j = poly[(k + 1) % poly.len()] as usize;
                    n += (positions[i] - origin).cross(positions[j] - origin);
                    avg += nn[i];
                }
                if n.dot(avg) > spacing * spacing * 1e-10 {
                    polygons.push(poly);
                }
            }
        }
    }
    if polygons.is_empty() {
        return Err("the position field produced no closed cells".into());
    }
    // At a field singularity a half-edge walk can revisit a vertex and is
    // rejected above. Close only small, simple interior gaps left by those
    // walks; a true input boundary must never be capped.
    let pem = edge_map(&polygons);
    for mut ring in crate::topology::boundary_loops(&polygons)
        .into_iter()
        .flat_map(simple_cycles)
    {
        if ring.len() > 16
            || (0..ring.len()).any(|k| {
                !matches!(pem.get(&edge_key(ring[k], ring[(k + 1) % ring.len()]).0), Some(fs) if fs.len() == 1)
            })
        {
            continue;
        }
        let mut unique = ring.clone();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() != ring.len() {
            continue;
        }
        if !boundary.is_empty()
            && ring.iter().any(|&v| {
                let p = positions[v as usize];
                p.distance(boundary_projection(p, points, &boundary)) < spacing * 0.25
            })
        {
            continue;
        }
        let center = ring.iter().map(|&v| positions[v as usize]).sum::<DVec3>() / ring.len() as f64;
        if ring
            .iter()
            .any(|&v| positions[v as usize].distance(center) > spacing * 2.0)
        {
            continue;
        }
        ring.reverse();
        polygons.push(ring);
    }
    // Only convex cells admit the centroid quad fan below. Triangulate
    // concave singularity cells first, sharing their original boundary edges.
    let mut convex = Vec::new();
    for ring in polygons {
        let n = ring
            .iter()
            .enumerate()
            .map(|(k, &v)| {
                (positions[v as usize] - positions[ring[0] as usize]).cross(
                    positions[ring[(k + 1) % ring.len()] as usize] - positions[ring[0] as usize],
                )
            })
            .sum::<DVec3>()
            .normalize_or_zero();
        let valid = (0..ring.len()).all(|k| {
            let a = positions[ring[k] as usize];
            let b = positions[ring[(k + 1) % ring.len()] as usize];
            let c = positions[ring[(k + 2) % ring.len()] as usize];
            (b - a).cross(c - b).dot(n) > spacing * spacing * 1e-10
        });
        if valid {
            convex.push(ring);
        } else {
            convex.extend(
                crate::util::triangulate_ring(&positions, &ring)
                    .iter()
                    .map(|t| t.to_vec()),
            );
        }
    }
    let coarse = Geometry::from_polygons(positions, convex);
    // Each graph cell becomes a ring of quads (paper's pure-quad option),
    // with shared edge midpoints and projection back to the reference.
    let mut pp = coarse.points.clone();
    let mut pinned = on_boundary.clone();
    let mut midpoints = HashMap::new();
    let mut quads = Vec::new();
    let cem = edge_map(
        &coarse
            .prims
            .iter()
            .map(|p| p.vertices.clone())
            .collect::<Vec<_>>(),
    );
    for p in &coarse.prims {
        let f = &p.vertices;
        let centroid = f.iter().map(|&v| pp[v as usize]).sum::<DVec3>() / f.len() as f64;
        let center = pp.len() as u32;
        pp.push(bvh.closest(centroid).unwrap().point);
        pinned.push(false);
        let mut mids = Vec::new();
        for k in 0..f.len() {
            let e = edge_key(f[k], f[(k + 1) % f.len()]).0;
            let mid = *midpoints.entry(e).or_insert_with(|| {
                let avg = (pp[e.0 as usize] + pp[e.1 as usize]) * 0.5;
                let p =
                    if cem[&e].len() == 1 && on_boundary[e.0 as usize] && on_boundary[e.1 as usize]
                    {
                        nearest_anchor(avg, &boundary_fine)
                    } else {
                        bvh.closest(avg).unwrap().point
                    };
                pp.push(p);
                pinned.push(
                    cem[&e].len() == 1 && on_boundary[e.0 as usize] && on_boundary[e.1 as usize],
                );
                (pp.len() - 1) as u32
            });
            mids.push(mid);
        }
        for k in 0..f.len() {
            quads.push(vec![
                f[k],
                mids[k],
                center,
                mids[(k + f.len() - 1) % f.len()],
            ]);
        }
    }
    // Discard unused graph vertices; unused samples must not inflate metrics.
    let mut used = vec![false; pp.len()];
    for f in &quads {
        for &v in f {
            used[v as usize] = true;
        }
    }
    let mut remap = vec![0; pp.len()];
    let mut compact = Vec::new();
    let mut fixed = Vec::new();
    for (i, &p) in pp.iter().enumerate() {
        if used[i] {
            remap[i] = compact.len() as u32;
            compact.push(p);
            fixed.push(pinned[i]);
        }
    }
    for f in &mut quads {
        for v in f {
            *v = remap[*v as usize];
        }
    }
    improve_quads(&mut compact, &quads, &fixed, &bvh);
    Ok(Geometry::from_polygons(compact, quads))
}

#[cfg(test)]
mod boundary_tests {
    use super::*;

    #[test]
    fn closed_seam_anchors_ignore_local_indexing() {
        let original: Vec<_> = (0..16)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / 16.0;
                DVec3::new(a.cos() * 10.0, a.sin() * 10.0, 3.0)
            })
            .collect();
        let sample = |points: &[DVec3]| {
            let edges: Vec<_> = (0..16).map(|i| (i as u32, ((i + 1) % 16) as u32)).collect();
            let neighbors: Vec<_> = (0..16).map(|i| vec![(i + 15) % 16, (i + 1) % 16]).collect();
            boundary_anchors(points, &edges, &neighbors, &[false; 16], 5.2)
        };
        let reference = sample(&original);
        for reverse in [false, true] {
            for offset in 0..16 {
                let points: Vec<_> = (0..16)
                    .map(|i| {
                        original[if reverse {
                            (offset + 16 - i) % 16
                        } else {
                            (offset + i) % 16
                        }]
                    })
                    .collect();
                let actual = sample(&points);
                for (a, b) in [(&reference.0, &actual.0), (&reference.1, &actual.1)] {
                    assert_eq!(a.len(), b.len());
                    assert!(
                        a.iter().all(|p| b.iter().any(|q| p.distance(*q) < 1e-10)),
                        "seam phase changed: reverse={reverse}, offset={offset}"
                    );
                }
            }
        }
    }
}
