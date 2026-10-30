//! Part hierarchy: which printed part hangs off which, across which seam.
//!
//! Seams are found from the cap faces the cut nodes leave behind: two parts
//! share a seam when they have caps with one id on one plane, facing each
//! other and overlapping. The strongest seams (largest area first) form a
//! spanning forest rooted at the part with the most surface. The result is
//! stored on the geometry (attribute names below, documented in
//! `docs/specs/slicing-printing.md` section 5b) and read back by [`PartTree`].
//!
//! Pivots and datums stay in assembly coordinates: they say where a part sits
//! on the body, so they do not move when `print.build_fit` poses the part.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use glam::{DVec2, DVec3};
use serde_json::{json, Value};
use vernon_core::{AttribArray, Geometry};

use crate::common::{json_v3, v3_json};
use crate::joints::{find_caps, CapRegion, Frame, JOINT};
use crate::mesh;

/// Prim Int: part id of the parent part, -1 for a root.
pub const PART_PARENT: &str = "part_parent";
/// Prim Vec3, mm: centre of the seam to the parent; bounding-box centre for a root.
pub const PART_PIVOT: &str = "part_pivot";
/// Prim Vec3: unit seam normal pointing from parent to child; zero for a root.
pub const PART_DATUM_NORMAL: &str = "part_datum_normal";
/// Prim Int: cap id of the seam to the parent; 0 for a root.
pub const PART_DATUM_CAP: &str = "part_datum_cap";
/// Detail Str: JSON array with one object per seam, tree links and others.
pub const PART_DATUMS: &str = "part_datums";
/// Detail Int: number of root parts.
pub const PART_ROOTS: &str = "part_roots";

/// Most cap triangles tested per side, so a noisy seam stays linear to check.
const OVERLAP_SAMPLES: usize = 64;

/// A seam between two parts. The normal points from `a` to `b`; `a < b`.
#[derive(Debug, Clone, PartialEq)]
pub struct Datum {
    pub cap: i64,
    pub a: i64,
    pub b: i64,
    pub origin: DVec3,
    pub normal: DVec3,
    /// Area of the smaller of the two caps, mm2.
    pub area_mm2: f64,
    /// True when this seam is a parent link of the tree.
    pub tree: bool,
}

struct Cap {
    region: CapRegion,
    tris: Vec<[DVec3; 3]>,
    area: f64,
    centroid: DVec3,
    min: DVec3,
    max: DVec3,
}

fn cap_data(geo: &Geometry, region: CapRegion, tris_of: &BTreeMap<usize, Vec<[u32; 3]>>) -> Cap {
    let mut tris = Vec::new();
    for pi in &region.prims {
        for t in tris_of.get(pi).into_iter().flatten() {
            tris.push(t.map(|v| geo.points[v as usize]));
        }
    }
    let (mut area, mut moment) = (0.0, DVec3::ZERO);
    let (mut min, mut max) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
    for t in &tris {
        let a = 0.5 * (t[1] - t[0]).cross(t[2] - t[0]).length();
        area += a;
        moment += (t[0] + t[1] + t[2]) / 3.0 * a;
        for q in t {
            min = min.min(*q);
            max = max.max(*q);
        }
    }
    let centroid = if area > 0.0 { moment / area } else { region.origin };
    Cap { region, tris, area, centroid, min, max }
}

fn in_triangle(q: DVec2, t: [DVec2; 3]) -> bool {
    let side = |a: DVec2, b: DVec2| (b - a).perp_dot(q - a);
    let (d0, d1, d2) = (side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0]));
    (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
}

/// True when a sampled triangle centre of `a` lies on a triangle of `b`.
fn centre_on(a: &Cap, b: &Cap, frame: &Frame) -> bool {
    let stride = (a.tris.len() / OVERLAP_SAMPLES).max(1);
    let others: Vec<[DVec2; 3]> = b.tris.iter().map(|t| t.map(|q| frame.p2(q))).collect();
    a.tris.iter().step_by(stride).any(|t| {
        let c = frame.p2((t[0] + t[1] + t[2]) / 3.0);
        others.iter().any(|o| in_triangle(c, *o))
    })
}

fn caps_meet(a: &Cap, b: &Cap, tol: f64) -> bool {
    if a.region.normal.dot(b.region.normal) > -0.99 {
        return false;
    }
    if (b.centroid - a.centroid).dot(a.region.normal).abs() > tol {
        return false;
    }
    if (a.min - b.max).max_element() > tol || (b.min - a.max).max_element() > tol {
        return false;
    }
    let frame = Frame::new(a.centroid, a.region.normal);
    centre_on(a, b, &frame) || centre_on(b, a, &frame)
}

/// Every seam of `geo`, in a deterministic order. `tree` is not yet set.
pub fn find_seams(geo: &Geometry) -> Vec<Datum> {
    let mut tris_of: BTreeMap<usize, Vec<[u32; 3]>> = BTreeMap::new();
    for (pi, t) in geo.triangulated() {
        tris_of.entry(pi).or_default().push(t);
    }
    let mut by_cap: BTreeMap<i64, Vec<Cap>> = BTreeMap::new();
    for region in find_caps(geo) {
        by_cap.entry(region.cap_id).or_default().push(cap_data(geo, region, &tris_of));
    }
    let tol = 1e-5 * geo.bounds().size().length().max(1.0);
    let mut seams = Vec::new();
    for (&cap, group) in &by_cap {
        for (i, a) in group.iter().enumerate() {
            for b in &group[i + 1..] {
                if a.region.part == b.region.part || !caps_meet(a, b, tol) {
                    continue;
                }
                // find_caps walks parts in order, so `a` has the lower id.
                let small = if a.area <= b.area { a } else { b };
                seams.push(Datum {
                    cap,
                    a: a.region.part,
                    b: b.region.part,
                    origin: small.centroid,
                    normal: a.region.normal,
                    area_mm2: small.area,
                    tree: false,
                });
            }
        }
    }
    seams
}

/// Union-find keyed by part id; returns the set representative.
fn find(sets: &mut BTreeMap<i64, i64>, mut x: i64) -> i64 {
    while sets[&x] != x {
        let up = sets[&sets[&x]];
        sets.insert(x, up);
        x = up;
    }
    x
}

/// Parent links per part from the seams; marks the seams used by the tree.
fn build_forest(parts: &[i64], area: &BTreeMap<i64, f64>, seams: &mut [Datum]) -> BTreeMap<i64, Option<usize>> {
    let mut order: Vec<usize> = (0..seams.len()).collect();
    order.sort_by(|&i, &j| seams[j].area_mm2.total_cmp(&seams[i].area_mm2).then(i.cmp(&j)));
    let mut sets: BTreeMap<i64, i64> = parts.iter().map(|&p| (p, p)).collect();
    let mut links: BTreeMap<i64, Vec<(i64, usize)>> = BTreeMap::new();
    for i in order {
        let (a, b) = (seams[i].a, seams[i].b);
        let (ra, rb) = (find(&mut sets, a), find(&mut sets, b));
        if ra != rb {
            sets.insert(ra, rb);
            seams[i].tree = true;
            links.entry(a).or_default().push((b, i));
            links.entry(b).or_default().push((a, i));
        }
    }
    // Root every component at its part with the most surface, lowest id on a tie.
    let mut by_size = parts.to_vec();
    by_size.sort_by(|p, q| area[q].total_cmp(&area[p]).then(p.cmp(q)));
    let mut parent: BTreeMap<i64, Option<usize>> = BTreeMap::new();
    for root in by_size {
        if parent.contains_key(&root) {
            continue;
        }
        parent.insert(root, None);
        let mut queue = VecDeque::from([root]);
        while let Some(p) = queue.pop_front() {
            for &(q, seam) in links.get(&p).into_iter().flatten() {
                if let std::collections::btree_map::Entry::Vacant(e) = parent.entry(q) {
                    e.insert(Some(seam));
                    queue.push_back(q);
                }
            }
        }
    }
    parent
}

fn datum_json(d: &Datum) -> Value {
    json!({"cap": d.cap, "a": d.a, "b": d.b, "origin": v3_json(d.origin), "normal": v3_json(d.normal), "area_mm2": d.area_mm2, "tree": d.tree})
}

fn datum_from_json(v: &Value) -> Option<Datum> {
    Some(Datum {
        cap: v.get("cap")?.as_i64()?,
        a: v.get("a")?.as_i64()?,
        b: v.get("b")?.as_i64()?,
        origin: json_v3(v.get("origin")?)?,
        normal: json_v3(v.get("normal")?)?,
        area_mm2: v.get("area_mm2")?.as_f64()?,
        tree: v.get("tree")?.as_bool()?,
    })
}

/// Derive the hierarchy of `geo` from its caps and store it on the geometry.
/// Any hierarchy already stored is replaced, since part ids may have changed.
pub fn annotate(geo: &mut Geometry) {
    let ids = mesh::part_ids(geo);
    let parts = mesh::unique_parts(geo);
    let mut area: BTreeMap<i64, f64> = parts.iter().map(|&p| (p, 0.0)).collect();
    for (i, pr) in geo.prims.iter().enumerate() {
        if pr.closed {
            if let Some(total) = area.get_mut(&ids[i]) {
                *total += geo.prim_area(i);
            }
        }
    }
    let mut seams = find_seams(geo);
    let forest = build_forest(&parts, &area, &mut seams);
    // Root pivots: bounding-box centre of the points each part uses.
    let mut prims_of: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (i, &part) in ids.iter().enumerate() {
        prims_of.entry(part).or_default().push(i);
    }
    let mut stamp: BTreeMap<i64, Stamp> = BTreeMap::new();
    for (&part, link) in &forest {
        stamp.insert(
            part,
            match link {
                Some(i) => {
                    let s = &seams[*i];
                    // Subtracting from zero avoids the -0.0 a plain negation leaves on an axis.
                    let (parent, normal) = if s.b == part { (s.a, s.normal) } else { (s.b, DVec3::ZERO - s.normal) };
                    Stamp { parent, pivot: s.origin, normal, cap: s.cap }
                }
                None => Stamp {
                    parent: -1,
                    pivot: mesh::prim_bounds(geo, &prims_of[&part]).center(),
                    normal: DVec3::ZERO,
                    cap: 0,
                },
            },
        );
    }
    write_stamps(geo, &stamp);
    let list = Value::Array(seams.iter().map(datum_json).collect());
    geo.detail_attribs.set(PART_DATUMS, AttribArray::Str(vec![list.to_string()]));
    let roots = forest.values().filter(|l| l.is_none()).count() as i64;
    geo.detail_attribs.set(PART_ROOTS, AttribArray::Int(vec![roots]));
}

struct Stamp {
    parent: i64,
    pivot: DVec3,
    normal: DVec3,
    cap: i64,
}

fn write_stamps(geo: &mut Geometry, stamp: &BTreeMap<i64, Stamp>) {
    let ids = mesh::part_ids(geo);
    let of = |part: &i64| stamp.get(part);
    let parent = ids.iter().map(|p| of(p).map_or(-1, |s| s.parent)).collect();
    let pivot = ids.iter().map(|p| of(p).map_or(DVec3::ZERO, |s| s.pivot)).collect();
    let normal = ids.iter().map(|p| of(p).map_or(DVec3::ZERO, |s| s.normal)).collect();
    let cap = ids.iter().map(|p| of(p).map_or(0, |s| s.cap)).collect();
    geo.prim_attribs.set(PART_PARENT, AttribArray::Int(parent));
    geo.prim_attribs.set(PART_PIVOT, AttribArray::Vec3(pivot));
    geo.prim_attribs.set(PART_DATUM_NORMAL, AttribArray::Vec3(normal));
    geo.prim_attribs.set(PART_DATUM_CAP, AttribArray::Int(cap));
}

/// Copy the hierarchy of `src` onto `dst`, a rebuild of the same parts in which
/// faces were added or moved: every face takes the values of its part.
pub fn carry(src: &Geometry, dst: &mut Geometry) {
    let (Ok(parents), Ok(pivots), Ok(normals), Ok(caps)) = (
        src.prim_attribs.ints(PART_PARENT),
        src.prim_attribs.vec3s(PART_PIVOT),
        src.prim_attribs.vec3s(PART_DATUM_NORMAL),
        src.prim_attribs.ints(PART_DATUM_CAP),
    ) else {
        return;
    };
    let mut stamp = BTreeMap::new();
    for (i, part) in mesh::part_ids(src).into_iter().enumerate() {
        stamp.entry(part).or_insert(Stamp { parent: parents[i], pivot: pivots[i], normal: normals[i], cap: caps[i] });
    }
    write_stamps(dst, &stamp);
    for name in [PART_DATUMS, PART_ROOTS] {
        if let Some(col) = src.detail_attribs.get(name) {
            dst.detail_attribs.set(name, col.clone());
        }
    }
}

/// One part as read back from the attributes.
#[derive(Debug, Clone, PartialEq)]
pub struct PartNode {
    pub part: i64,
    pub label: Option<String>,
    /// Parent in this geometry; `None` for a root.
    pub parent: Option<i64>,
    /// Stored parent that this geometry cannot resolve: the part was extracted
    /// from its assembly, or a later edit left the link in a loop.
    pub unresolved_parent: Option<i64>,
    pub pivot: DVec3,
    pub datum_normal: DVec3,
    pub datum_cap: i64,
    pub faces: usize,
    /// Distinct joint features cut or added on this part.
    pub joint_features: usize,
    pub children: Vec<i64>,
}

/// The part tree of a geometry. `linked` is false when only `part` is present,
/// which leaves a flat list of root parts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartTree {
    pub nodes: BTreeMap<i64, PartNode>,
    pub roots: Vec<i64>,
    pub datums: Vec<Datum>,
    pub linked: bool,
}

impl PartTree {
    /// Read the tree back; `None` when the geometry has no `part` attribute.
    pub fn from_geometry(geo: &Geometry) -> Option<PartTree> {
        let ids = geo.prim_attribs.ints(mesh::PART).ok()?;
        let parents = geo.prim_attribs.ints(PART_PARENT).ok();
        let pivots = geo.prim_attribs.vec3s(PART_PIVOT).ok();
        let normals = geo.prim_attribs.vec3s(PART_DATUM_NORMAL).ok();
        let caps = geo.prim_attribs.ints(PART_DATUM_CAP).ok();
        let labels = geo.prim_attribs.strs("label").ok();
        let joints = geo.prim_attribs.ints(JOINT).ok();
        let mut stored: BTreeMap<i64, i64> = BTreeMap::new();
        let mut nodes: BTreeMap<i64, PartNode> = BTreeMap::new();
        let mut features: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
        for (i, &part) in ids.iter().enumerate() {
            let node = nodes.entry(part).or_insert_with(|| {
                stored.insert(part, parents.map_or(-1, |p| p[i]));
                PartNode {
                    part,
                    label: labels.map(|l| l[i].clone()),
                    parent: None,
                    unresolved_parent: None,
                    pivot: pivots.map_or(DVec3::ZERO, |p| p[i]),
                    datum_normal: normals.map_or(DVec3::ZERO, |n| n[i]),
                    datum_cap: caps.map_or(0, |c| c[i]),
                    faces: 0,
                    joint_features: 0,
                    children: Vec::new(),
                }
            });
            node.faces += 1;
            if let Some(tag) = joints.map(|j| j[i]).filter(|&t| t > 0) {
                features.entry(part).or_default().insert(tag);
            }
        }
        for (part, tags) in features {
            nodes.get_mut(&part).expect("seen above").joint_features = tags.len();
        }
        // Resolve links; a walk that never reaches a root is a loop.
        let limit = nodes.len();
        let reaches_root = |start: i64| {
            let mut at = start;
            for _ in 0..=limit {
                match stored.get(&at) {
                    Some(&up) if up >= 0 && stored.contains_key(&up) => at = up,
                    _ => return true,
                }
            }
            false
        };
        let keys: Vec<i64> = nodes.keys().copied().collect();
        for &part in &keys {
            let up = stored[&part];
            if up < 0 {
                continue;
            }
            if stored.contains_key(&up) && reaches_root(part) {
                nodes.get_mut(&part).expect("key").parent = Some(up);
                nodes.get_mut(&up).expect("checked").children.push(part);
            } else {
                nodes.get_mut(&part).expect("key").unresolved_parent = Some(up);
            }
        }
        let roots = keys.into_iter().filter(|p| nodes[p].parent.is_none()).collect();
        let datums = geo
            .detail_attribs
            .strs(PART_DATUMS)
            .ok()
            .and_then(|s| s.first())
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.as_array().map(|a| a.iter().filter_map(datum_from_json).collect()))
            .unwrap_or_default();
        Some(PartTree { nodes, roots, datums, linked: parents.is_some() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testutil::cook;
    use crate::mesh::testutil::tri_box;
    use crate::nodes_cut::PlaneCut;
    use crate::nodes_fit::BuildFit;
    use crate::nodes_joints::PlugJoint;
    use crate::nodes_split::{Connectivity, GridPartition, PartExtract, PartLabel};
    use vernon_graph::{Data, ParamValue};

    fn geo(d: &Data) -> Geometry {
        (**d.as_geometry().unwrap()).clone()
    }

    /// A 100 mm cube cut at z = 40: part 1 is the taller half.
    fn halves() -> Geometry {
        let cube = tri_box(DVec3::ZERO, DVec3::splat(100.0));
        let set = [("origin", ParamValue::Vec3(DVec3::new(50.0, 50.0, 40.0)))];
        geo(&cook(&PlaneCut, "hier_cut", &[Some(Data::geometry(cube))], &set).unwrap()[0])
    }

    #[test]
    fn plane_cut_links_the_smaller_half_to_the_larger() {
        let g = halves();
        let tree = PartTree::from_geometry(&g).unwrap();
        assert!(tree.linked);
        assert_eq!(tree.roots, vec![1]);
        let child = &tree.nodes[&0];
        assert_eq!(child.parent, Some(1));
        assert_eq!(tree.nodes[&1].children, vec![0]);
        assert!((child.pivot - DVec3::new(50.0, 50.0, 40.0)).length() < 1e-9);
        // Parent is above, so parent to child points down.
        assert!((child.datum_normal + DVec3::Z).length() < 1e-9);
        assert!(child.datum_cap > 0);
        assert!((tree.nodes[&1].pivot - DVec3::new(50.0, 50.0, 70.0)).length() < 1e-9);
        assert_eq!(tree.nodes[&1].datum_normal, DVec3::ZERO);
        assert_eq!(tree.datums.len(), 1);
        assert!((tree.datums[0].area_mm2 - 10_000.0).abs() < 1e-6);
        assert!(tree.datums[0].tree);
        assert_eq!(g.detail_attribs.ints(PART_ROOTS).unwrap(), &[1]);
        g.validate().unwrap();
    }

    #[test]
    fn grid_is_a_spanning_tree_with_only_face_neighbours_as_seams() {
        let slab = tri_box(DVec3::ZERO, DVec3::new(200.0, 200.0, 50.0));
        let set = [
            ("mode", ParamValue::Str("by_count".into())),
            ("count_x", ParamValue::Int(2)),
            ("count_y", ParamValue::Int(2)),
            ("count_z", ParamValue::Int(1)),
        ];
        let out = cook(&GridPartition, "hier_grid", &[Some(Data::geometry(slab))], &set).unwrap();
        let tree = PartTree::from_geometry(out[0].as_geometry().unwrap()).unwrap();
        assert_eq!(tree.nodes.len(), 4);
        assert_eq!(tree.roots.len(), 1);
        // Four cells in a square: four seams, no diagonal ones, three in the tree.
        assert_eq!(tree.datums.len(), 4);
        assert_eq!(tree.datums.iter().filter(|d| d.tree).count(), 3);
        for d in &tree.datums {
            let (a, b) = (d.a, d.b);
            assert!([(0, 1), (0, 2), (1, 3), (2, 3)].contains(&(a, b)), "seam {a}-{b}");
            assert!((d.area_mm2 - 5_000.0).abs() < 1e-6);
        }
        let mut seen = 0;
        let mut stack = tree.roots.clone();
        while let Some(p) = stack.pop() {
            seen += 1;
            stack.extend(&tree.nodes[&p].children);
        }
        assert_eq!(seen, 4);
    }

    #[test]
    fn annotate_is_deterministic() {
        let (a, b) = (halves(), halves());
        assert_eq!(a.detail_attribs.strs(PART_DATUMS).unwrap(), b.detail_attribs.strs(PART_DATUMS).unwrap());
        assert_eq!(a.prim_attribs.ints(PART_PARENT).unwrap(), b.prim_attribs.ints(PART_PARENT).unwrap());
    }

    #[test]
    fn plug_joints_keep_the_links_and_stamp_their_new_faces() {
        let input = halves();
        let out = cook(&PlugJoint, "hier_plug", &[Some(Data::geometry(input.clone()))], &[]).unwrap();
        let g = geo(&out[0]);
        assert!(g.prims.len() > input.prims.len());
        let tree = PartTree::from_geometry(&g).unwrap();
        assert_eq!(tree.roots, vec![1]);
        assert_eq!(tree.nodes[&0].parent, Some(1));
        assert!(tree.nodes[&0].joint_features > 0);
        assert_eq!(tree.nodes[&0].joint_features, tree.nodes[&1].joint_features);
        assert_eq!(tree.datums, PartTree::from_geometry(&input).unwrap().datums);
        // A zero-padded face would read as a child of part 0 with no pivot.
        let parents = g.prim_attribs.ints(PART_PARENT).unwrap();
        for (i, &part) in mesh::part_ids(&g).iter().enumerate() {
            assert_eq!(parents[i], if part == 0 { 1 } else { -1 });
        }
    }

    #[test]
    fn build_fit_keeps_assembly_coordinates_after_moving_parts() {
        let input = halves();
        let before = PartTree::from_geometry(&input).unwrap();
        let out = cook(&BuildFit, "hier_fit", &[Some(Data::geometry(input))], &[]).unwrap();
        let tree = PartTree::from_geometry(out[0].as_geometry().unwrap()).unwrap();
        assert_eq!(tree.nodes[&0].pivot, before.nodes[&0].pivot);
        assert_eq!(tree.nodes[&0].parent, Some(1));
        assert_eq!(tree.datums, before.datums);
    }

    #[test]
    fn labels_show_and_an_extracted_part_reports_its_lost_parent() {
        let out = cook(&PartLabel, "hier_label", &[Some(Data::geometry(halves()))], &[]).unwrap();
        let labelled = geo(&out[0]);
        let tree = PartTree::from_geometry(&labelled).unwrap();
        assert_eq!(tree.nodes[&1].label.as_deref(), Some("P02"));
        let set = [("part", ParamValue::Int(0))];
        let out = cook(&PartExtract, "hier_extract", &[Some(Data::geometry(labelled))], &set).unwrap();
        let tree = PartTree::from_geometry(out[0].as_geometry().unwrap()).unwrap();
        assert_eq!(tree.roots, vec![0]);
        assert_eq!(tree.nodes[&0].parent, None);
        assert_eq!(tree.nodes[&0].unresolved_parent, Some(1));
    }

    #[test]
    fn renumbering_replaces_old_links_and_uncut_parts_are_roots() {
        let mut g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        g.merge(&tri_box(DVec3::splat(50.0), DVec3::splat(10.0)));
        let out = cook(&Connectivity, "hier_conn", &[Some(Data::geometry(g))], &[]).unwrap();
        let tree = PartTree::from_geometry(out[0].as_geometry().unwrap()).unwrap();
        assert_eq!(tree.roots, vec![0, 1]);
        assert!(tree.linked && tree.datums.is_empty());
        assert!((tree.nodes[&1].pivot - DVec3::splat(55.0)).length() < 1e-9);
    }

    #[test]
    fn a_stored_loop_breaks_into_roots_instead_of_hanging() {
        let mut g = halves();
        let ids = mesh::part_ids(&g);
        let looped = ids.iter().map(|&p| 1 - p).collect();
        mesh::set_int_prim_attr(&mut g, PART_PARENT, looped);
        let tree = PartTree::from_geometry(&g).unwrap();
        assert_eq!(tree.roots, vec![0, 1]);
        assert_eq!(tree.nodes[&0].unresolved_parent, Some(1));
    }

    #[test]
    fn plain_part_attribute_reads_as_a_flat_list() {
        let mut g = tri_box(DVec3::ZERO, DVec3::splat(10.0));
        let n = g.prims.len();
        mesh::set_int_prim_attr(&mut g, mesh::PART, vec![3; n]);
        let tree = PartTree::from_geometry(&g).unwrap();
        assert!(!tree.linked);
        assert_eq!(tree.roots, vec![3]);
        assert_eq!(tree.nodes[&3].faces, n);
        assert!(PartTree::from_geometry(&tri_box(DVec3::ZERO, DVec3::ONE)).is_none());
    }
}
