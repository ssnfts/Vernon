//! Connected islands of a triangle mesh and rule-based island selection.
//!
//! Two triangles belong to the same island when they share an edge (two equal vertex index
//! pairs). Touching at a single vertex does not connect them. A non-manifold edge (three or
//! more triangles) connects all of its triangles. Selection rules return island indices in
//! ascending order; ties for "largest" go to the lower island index.
use glam::DVec3;
use std::collections::HashMap;
use std::fmt;

/// Summary of one island.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandInfo {
    pub area_mm2: f64,
    /// Area-weighted centroid of the triangles (mm).
    pub centroid: DVec3,
    pub bbox_min: DVec3,
    pub bbox_max: DVec3,
    /// Area-weighted mean unit normal; zero when the normals cancel (e.g. a closed shell)
    /// or the island has no area.
    pub mean_normal: DVec3,
}

/// Selection rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rule {
    /// The single island of greatest area.
    Largest,
    /// The `n` islands of greatest area.
    LargestN(usize),
    /// Islands with area strictly above the threshold (mm^2).
    AreaAbove(f64),
    /// Islands whose mean normal has cosine >= `min_cos` with `dir` (`dir` must be non-zero;
    /// islands with a zero mean normal never match).
    FacingDirection { dir: DVec3, min_cos: f64 },
    /// Islands whose centroid lies within `radius_mm` of `p`.
    NearPoint { p: DVec3, radius_mm: f64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IslandError {
    /// A triangle references a vertex that does not exist.
    BadIndex { tri: usize },
    /// A face index of an island is out of range.
    BadFace { face: usize },
}

impl fmt::Display for IslandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IslandError::BadIndex { tri } => write!(f, "triangle {tri} references a missing vertex"),
            IslandError::BadFace { face } => write!(f, "island lists missing face {face}"),
        }
    }
}
impl std::error::Error for IslandError {}

/// Face indices per island, islands ordered by their smallest face index and faces ascending.
pub fn islands(tris: &[[u32; 3]]) -> Vec<Vec<usize>> {
    let mut edge_faces: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            edge_faces.entry((a.min(b), a.max(b))).or_default().push(i);
        }
    }
    let mut seen = vec![false; tris.len()];
    let mut out = Vec::new();
    for s in 0..tris.len() {
        if seen[s] {
            continue;
        }
        seen[s] = true;
        let mut comp = vec![s];
        let mut head = 0;
        while head < comp.len() {
            let t = tris[comp[head]];
            head += 1;
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                for &o in &edge_faces[&(a.min(b), a.max(b))] {
                    if !seen[o] {
                        seen[o] = true;
                        comp.push(o);
                    }
                }
            }
        }
        comp.sort_unstable();
        out.push(comp);
    }
    out
}

/// Area, centroid, bounds and mean normal of each island.
pub fn island_infos(verts: &[DVec3], tris: &[[u32; 3]], isl: &[Vec<usize>]) -> Result<Vec<IslandInfo>, IslandError> {
    let corner = |face: usize| -> Result<[DVec3; 3], IslandError> {
        let t = tris.get(face).ok_or(IslandError::BadFace { face })?;
        let mut p = [DVec3::ZERO; 3];
        for (o, &i) in p.iter_mut().zip(t) {
            *o = *verts.get(i as usize).ok_or(IslandError::BadIndex { tri: face })?;
        }
        Ok(p)
    };
    isl.iter()
        .map(|faces| {
            let (mut area, mut csum, mut nsum) = (0.0, DVec3::ZERO, DVec3::ZERO);
            let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
            for &f in faces {
                let [a, b, c] = corner(f)?;
                let n2 = (b - a).cross(c - a); // twice the area vector
                let ar = 0.5 * n2.length();
                area += ar;
                csum += (a + b + c) / 3.0 * ar;
                nsum += 0.5 * n2;
                for p in [a, b, c] {
                    lo = lo.min(p);
                    hi = hi.max(p);
                }
            }
            let centroid = if area > 0.0 {
                csum / area
            } else if faces.is_empty() {
                DVec3::ZERO
            } else {
                // Degenerate (zero-area) island: fall back to the bounds centre.
                (lo + hi) * 0.5
            };
            if faces.is_empty() {
                lo = DVec3::ZERO;
                hi = DVec3::ZERO;
            }
            // A closed shell sums to ~0 up to rounding; treat that as "no direction".
            let mean_normal = if nsum.length() > 1e-9 * area.max(1e-300) { nsum.normalize() } else { DVec3::ZERO };
            Ok(IslandInfo { area_mm2: area, centroid, bbox_min: lo, bbox_max: hi, mean_normal })
        })
        .collect()
}

/// Indices (ascending) of the islands chosen by `rule`.
pub fn select(info: &[IslandInfo], rule: Rule) -> Vec<usize> {
    let by_area_desc = || {
        let mut idx: Vec<usize> = (0..info.len()).collect();
        idx.sort_by(|&a, &b| info[b].area_mm2.total_cmp(&info[a].area_mm2).then(a.cmp(&b)));
        idx
    };
    let mut out: Vec<usize> = match rule {
        Rule::Largest => by_area_desc().into_iter().take(1).collect(),
        Rule::LargestN(n) => by_area_desc().into_iter().take(n).collect(),
        Rule::AreaAbove(t) => (0..info.len()).filter(|&i| info[i].area_mm2 > t).collect(),
        Rule::FacingDirection { dir, min_cos } => match dir.try_normalize() {
            Some(d) => (0..info.len())
                .filter(|&i| info[i].mean_normal != DVec3::ZERO && info[i].mean_normal.dot(d) >= min_cos)
                .collect(),
            None => Vec::new(),
        },
        Rule::NearPoint { p, radius_mm } => {
            (0..info.len()).filter(|&i| info[i].centroid.distance(p) <= radius_mm).collect()
        }
    };
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit cube scaled by `s`, at `o`, with vertex ids starting at `base`.
    fn cube(o: DVec3, s: f64, base: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let v = (0..8)
            .map(|i| o + DVec3::new((i & 1) as f64, ((i >> 1) & 1) as f64, ((i >> 2) & 1) as f64) * s)
            .collect();
        let t = [
            [0, 1, 5], [0, 5, 4], [2, 7, 3], [2, 6, 7], [0, 4, 6], [0, 6, 2],
            [1, 3, 7], [1, 7, 5], [0, 2, 3], [0, 3, 1], [4, 5, 7], [4, 7, 6],
        ]
        .map(|t: [u32; 3]| t.map(|i| i + base))
        .to_vec();
        (v, t)
    }

    /// Quad with origin `o`, edges `u` and `v` (normal u x v); two triangles.
    fn quad(o: DVec3, u: DVec3, v: DVec3, verts: &mut Vec<DVec3>, tris: &mut Vec<[u32; 3]>) {
        let b = verts.len() as u32;
        verts.extend([o, o + u, o + u + v, o + v]);
        tris.push([b, b + 1, b + 2]);
        tris.push([b, b + 2, b + 3]);
    }

    fn scene() -> (Vec<DVec3>, Vec<[u32; 3]>) {
        let (mut v, mut t) = (Vec::new(), Vec::new());
        // P: 2x2 facing +z (area 4); Q: 1x1 facing +x (area 1); R: 3x3 facing +z (area 9).
        quad(DVec3::ZERO, DVec3::new(2.0, 0.0, 0.0), DVec3::new(0.0, 2.0, 0.0), &mut v, &mut t);
        quad(DVec3::new(10.0, 0.0, 0.0), DVec3::Y, DVec3::Z, &mut v, &mut t);
        quad(DVec3::new(20.0, 0.0, 0.0), DVec3::new(3.0, 0.0, 0.0), DVec3::new(0.0, 3.0, 0.0), &mut v, &mut t);
        (v, t)
    }

    #[test]
    fn island_select_two_disjoint_cubes() {
        let (mut v, mut t) = cube(DVec3::ZERO, 1.0, 0);
        let (v2, t2) = cube(DVec3::new(5.0, 0.0, 0.0), 1.0, 8);
        v.extend(v2);
        t.extend(t2);
        let isl = islands(&t);
        assert_eq!(isl.len(), 2);
        assert_eq!(isl[0], (0..12).collect::<Vec<_>>());
        assert_eq!(isl[1], (12..24).collect::<Vec<_>>());
        let info = island_infos(&v, &t, &isl).unwrap();
        assert!((info[0].area_mm2 - 6.0).abs() < 1e-12);
        assert!(info[0].mean_normal == DVec3::ZERO, "closed shell has no mean direction");
        assert!((info[1].centroid - DVec3::new(5.5, 0.5, 0.5)).length() < 1e-12);
    }

    #[test]
    fn island_select_shared_vertex_only_stays_two_islands() {
        // Second cube reuses the first cube's vertex 7 as its own vertex 0.
        let (mut v, mut t) = cube(DVec3::ZERO, 1.0, 0);
        let (v2, t2) = cube(DVec3::ONE, 1.0, 0);
        let remap = |i: u32| if i == 0 { 7 } else { i + 7 };
        v.extend(v2.iter().skip(1));
        t.extend(t2.iter().map(|tr| tr.map(remap)));
        assert_eq!(v.len(), 15);
        assert_eq!(islands(&t).len(), 2);
    }

    #[test]
    fn island_select_empty_mesh_has_no_islands() {
        assert!(islands(&[]).is_empty());
        assert!(island_infos(&[], &[], &[]).unwrap().is_empty());
        assert!(select(&[], Rule::Largest).is_empty());
    }

    #[test]
    fn island_select_info_values_by_hand() {
        let (v, t) = scene();
        let isl = islands(&t);
        assert_eq!(isl, vec![vec![0, 1], vec![2, 3], vec![4, 5]]);
        let info = island_infos(&v, &t, &isl).unwrap();
        assert!((info[0].area_mm2 - 4.0).abs() < 1e-12);
        assert!((info[1].area_mm2 - 1.0).abs() < 1e-12);
        assert!((info[2].area_mm2 - 9.0).abs() < 1e-12);
        assert!((info[0].centroid - DVec3::new(1.0, 1.0, 0.0)).length() < 1e-12);
        assert!((info[1].mean_normal - DVec3::X).length() < 1e-12);
        assert_eq!((info[2].bbox_min, info[2].bbox_max), (DVec3::new(20.0, 0.0, 0.0), DVec3::new(23.0, 3.0, 0.0)));
    }

    #[test]
    fn island_select_each_rule_picks_the_expected_island() {
        let (v, t) = scene();
        let info = island_infos(&v, &t, &islands(&t)).unwrap();
        assert_eq!(select(&info, Rule::Largest), vec![2]);
        assert_eq!(select(&info, Rule::LargestN(2)), vec![0, 2]);
        assert_eq!(select(&info, Rule::LargestN(10)), vec![0, 1, 2]);
        assert!(select(&info, Rule::LargestN(0)).is_empty());
        assert_eq!(select(&info, Rule::AreaAbove(3.0)), vec![0, 2]);
        assert_eq!(select(&info, Rule::AreaAbove(4.0)), vec![2], "strictly above");
        assert_eq!(select(&info, Rule::FacingDirection { dir: DVec3::new(5.0, 0.0, 0.0), min_cos: 0.9 }), vec![1]);
        assert_eq!(select(&info, Rule::FacingDirection { dir: DVec3::Z, min_cos: 0.99 }), vec![0, 2]);
        assert!(select(&info, Rule::FacingDirection { dir: DVec3::ZERO, min_cos: 0.0 }).is_empty());
        assert_eq!(select(&info, Rule::NearPoint { p: DVec3::new(10.0, 0.5, 0.5), radius_mm: 1.0 }), vec![1]);
        assert_eq!(select(&info, Rule::NearPoint { p: DVec3::new(1.0, 1.0, 0.0), radius_mm: 0.5 }), vec![0]);
        assert!(select(&info, Rule::NearPoint { p: DVec3::new(100.0, 0.0, 0.0), radius_mm: 1.0 }).is_empty());
    }

    #[test]
    fn island_select_largest_ties_go_to_the_lower_index() {
        let (mut v, mut t) = (Vec::new(), Vec::new());
        quad(DVec3::ZERO, DVec3::X, DVec3::Y, &mut v, &mut t);
        quad(DVec3::new(5.0, 0.0, 0.0), DVec3::X, DVec3::Y, &mut v, &mut t);
        let info = island_infos(&v, &t, &islands(&t)).unwrap();
        assert_eq!(select(&info, Rule::Largest), vec![0]);
    }

    #[test]
    fn island_select_bad_indices_are_errors() {
        let v = vec![DVec3::ZERO, DVec3::X, DVec3::Y];
        assert_eq!(
            island_infos(&v, &[[0, 1, 9]], &[vec![0]]).unwrap_err(),
            IslandError::BadIndex { tri: 0 }
        );
        assert_eq!(island_infos(&v, &[[0, 1, 2]], &[vec![3]]).unwrap_err(), IslandError::BadFace { face: 3 });
    }
}
