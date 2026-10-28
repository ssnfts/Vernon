//! STEP (ISO 10303-21) writer for faceted shells and untrimmed NURBS patches,
//! and the `surface.write_step` node.
//!
//! Units are always millimetres. Faceted meshes are written as planar
//! FACE_SURFACE entities bounded by poly loops; patches as B-spline surface faces
//! bounded by their four border curves (untrimmed).
use std::fmt::Write as _;

use glam::DVec3;

use vernon_core::Geometry;

use crate::nurbs::{compress_knots, NurbsCurve, NurbsSurface, PatchSet};

use crate::topology::{orient, weld};

use crate::util::max_abs_coord;

/// Application protocol written into the file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schema {
    Ap214,

    Ap242,
}

/// Writer settings.
#[derive(Debug, Clone)]
pub struct StepOptions {
    pub schema: Schema,

    pub name: String,

    pub max_faces: usize,
}

impl Default for StepOptions {
    fn default() -> Self {
        StepOptions {
            schema: Schema::Ap214,
            name: "vernon_surface".into(),
            max_faces: 500_000,
        }
    }
}

/// Counts of what was written.
#[derive(Debug, Clone, Default)]
pub struct StepSummary {
    pub faces: usize,

    pub patches: usize,

    pub entities: usize,

    pub bytes: usize,

    pub closed_shell: bool,

    pub solid_shells: usize,

    pub open_shells: usize,

    pub shared_edges: usize,

    pub bbox_min: DVec3,

    pub bbox_max: DVec3,
}

struct Writer {
    body: String,

    next: usize,

    bbox: (DVec3, DVec3),
}

fn real(x: f64) -> String {
    // Rust's exponent formatter needs a decimal mantissa for Part 21 REAL.

    let s = format!("{x:e}");

    let (m, e) = s.split_once('e').unwrap();

    format!("{}{}E{}", m, if m.contains('.') { "" } else { "." }, e)
}

fn text(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c == '\'' {
                vec!['\'', '\'']
            } else if c.is_ascii() && !c.is_ascii_control() && c != '\\' {
                vec![c]
            } else {
                vec!['_']
            }
        })
        .collect()
}

impl Writer {
    fn new() -> Self {
        Writer {
            body: String::new(),
            next: 1,
            bbox: (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
        }
    }

    fn add(&mut self, entity: impl AsRef<str>) -> usize {
        let id = self.next;

        self.next += 1;

        let _ = writeln!(self.body, "#{id}={};", entity.as_ref());

        id
    }

    fn point(&mut self, p: DVec3) -> usize {
        self.bbox.0 = self.bbox.0.min(p);

        self.bbox.1 = self.bbox.1.max(p);

        self.add(format!(
            "CARTESIAN_POINT('',({},{},{}))",
            real(p.x),
            real(p.y),
            real(p.z)
        ))
    }

    fn direction(&mut self, d: DVec3) -> usize {
        self.add(format!(
            "DIRECTION('',({},{},{}))",
            real(d.x),
            real(d.y),
            real(d.z)
        ))
    }

    fn list(ids: &[usize]) -> String {
        ids.iter()
            .map(|i| format!("#{i}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    fn curve(&mut self, c: &NurbsCurve) -> usize {
        let cps: Vec<usize> = c.control_points.iter().map(|&p| self.point(p)).collect();

        let (vals, mult) = compress_knots(&c.knots);

        let m = mult
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",");

        let k = vals.iter().map(|&v| real(v)).collect::<Vec<_>>().join(",");

        match &c.weights {

            None => self.add(format!("B_SPLINE_CURVE_WITH_KNOTS('',{},({}),.UNSPECIFIED.,.F.,.F.,({m}),({k}),.UNSPECIFIED.)", c.degree, Self::list(&cps))),

            Some(w) => {

                let ws = w.iter().map(|&v| real(v)).collect::<Vec<_>>().join(",");

                self.add(format!("(BOUNDED_CURVE() B_SPLINE_CURVE({},({}),.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS(({m}),({k}),.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE(({ws})) REPRESENTATION_ITEM(''))", c.degree, Self::list(&cps)))

            }

        }
    }

    fn surface(&mut self, s: &NurbsSurface) -> usize {
        let mut rows = Vec::with_capacity(s.nu);

        for i in 0..s.nu {
            let ids: Vec<usize> = (0..s.nv).map(|j| self.point(s.cp(i, j))).collect();

            rows.push(format!("({})", Self::list(&ids)));
        }

        let grid = rows.join(",");

        let (vu, mu) = compress_knots(&s.knots_u);

        let (vv, mv) = compress_knots(&s.knots_v);

        let ints = |v: &[usize]| v.iter().map(usize::to_string).collect::<Vec<_>>().join(",");

        let reals = |v: &[f64]| v.iter().map(|&x| real(x)).collect::<Vec<_>>().join(",");

        match &s.weights {

            None => self.add(format!(

                "B_SPLINE_SURFACE_WITH_KNOTS('',{},{},({grid}),.UNSPECIFIED.,.F.,.F.,.F.,({}),({}),({}),({}),.UNSPECIFIED.)",

                s.degree_u,

                s.degree_v,

                ints(&mu),

                ints(&mv),

                reals(&vu),

                reals(&vv)

            )),

            Some(w) => {

                let wr: Vec<String> = (0..s.nu).map(|i| format!("({})", (0..s.nv).map(|j| real(w[i * s.nv + j])).collect::<Vec<_>>().join(","))).collect();

                self.add(format!(

                    "(BOUNDED_SURFACE() B_SPLINE_SURFACE({},{},({grid}),.UNSPECIFIED.,.F.,.F.,.F.) B_SPLINE_SURFACE_WITH_KNOTS(({}),({}),({}),({}),.UNSPECIFIED.) GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_SURFACE(({})) REPRESENTATION_ITEM('') SURFACE())",

                    s.degree_u,

                    s.degree_v,

                    ints(&mu),

                    ints(&mv),

                    reals(&vu),

                    reals(&vv),

                    wr.join(",")

                ))

            }

        }
    }

    /// One planar polygon face over shared point ids.
    fn polygon_face(&mut self, pts: &[usize], coords: &[DVec3]) -> usize {
        let mut n = DVec3::ZERO;

        for i in 0..coords.len() {
            let (a, b) = (coords[i], coords[(i + 1) % coords.len()]);

            n += DVec3::new(
                (a.y - b.y) * (a.z + b.z),
                (a.z - b.z) * (a.x + b.x),
                (a.x - b.x) * (a.y + b.y),
            );
        }

        let n = n.normalize_or_zero();

        let n = if n == DVec3::ZERO { DVec3::Z } else { n };

        let xdir = (coords[1] - coords[0]).normalize_or_zero();

        let xdir = if xdir == DVec3::ZERO {
            n.any_orthonormal_vector()
        } else {
            (xdir - n * xdir.dot(n)).normalize_or_zero()
        };

        let xdir = if xdir == DVec3::ZERO {
            n.any_orthonormal_vector()
        } else {
            xdir
        };

        let o = self.add(format!(
            "CARTESIAN_POINT('',({},{},{}))",
            real(coords[0].x),
            real(coords[0].y),
            real(coords[0].z)
        ));

        let (dz, dx) = (self.direction(n), self.direction(xdir));

        let ax = self.add(format!("AXIS2_PLACEMENT_3D('',#{o},#{dz},#{dx})"));

        let plane = self.add(format!("PLANE('',#{ax})"));

        let lp = self.add(format!("POLY_LOOP('',({}))", Self::list(pts)));

        let bound = self.add(format!("FACE_OUTER_BOUND('',#{lp},.T.)"));

        self.add(format!("FACE_SURFACE('',(#{bound}),#{plane},.T.)"))
    }
}

struct PatchEdge {
    curve: NurbsCurve,
    a: usize,
    b: usize,
    id: usize,
    uses: Vec<(usize, bool)>,
}

fn vertex(w: &mut Writer, vs: &mut Vec<(DVec3, usize)>, p: DVec3) -> usize {
    if let Some((_, id)) = vs.iter().find(|(q, _)| q.distance(p) <= 1e-9) {
        return *id;
    }

    let cp = w.point(p);
    let id = w.add(format!("VERTEX_POINT('',#{cp})"));
    vs.push((p, id));
    id
}

// Conservative whole-curve equivalence: identical degree, raw knot
// vector and weights, plus Euclidean control polygon within 1e-9 mm.

// Different refinements and partial overlaps deliberately remain separate.

fn curves_equal(a: &NurbsCurve, b: &NurbsCurve) -> bool {
    if a.degree != b.degree
        || a.control_points.len() != b.control_points.len()
        || a.knots.len() != b.knots.len()
    {
        return false;
    }

    // Exact raw knots keep parameter changes conservative. Normalized floats
    // can erase a small parameter difference that is large in world space.
    if a.knots != b.knots {
        return false;
    }

    if a.control_points
        .iter()
        .zip(&b.control_points)
        .any(|(x, y)| x.distance(*y) > 1e-9)
    {
        return false;
    }

    let aw = |i| a.weights.as_ref().map_or(1., |w| w[i]);
    let bw = |i| b.weights.as_ref().map_or(1., |w| w[i]);

    (0..a.control_points.len()).all(|i| aw(i) == bw(i))
}

fn components(links: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut seen = vec![false; links.len()];
    let mut out = Vec::new();
    for root in 0..links.len() {
        if seen[root] {
            continue;
        }
        seen[root] = true;
        let mut comp = vec![root];
        let mut head = 0;
        while head < comp.len() {
            let f = comp[head];
            head += 1;
            for &g in &links[f] {
                if !seen[g] {
                    seen[g] = true;
                    comp.push(g);
                }
            }
        }
        comp.sort_unstable();
        out.push(comp);
    }
    out
}

fn clamped(k: &[f64], degree: usize) -> bool {
    k[..=degree].iter().all(|x| *x == k[0])
        && k[k.len() - degree - 1..]
            .iter()
            .all(|x| *x == k[k.len() - 1])
}

fn patch_volume(s: &NurbsSurface, origin: DVec3) -> f64 {
    // One component-wide origin preserves the sum of signed tetrahedra.
    // Recenter the control net before evaluation as well as the products.
    let mut local = s.clone();
    for p in &mut local.control_points {
        *p -= origin;
    }
    let s = &local;
    let (u0, u1) = s.domain_u();
    let (v0, v1) = s.domain_v();
    let mut volume = 0.;

    for i in 0..8 {
        for j in 0..8 {
            let p = |a: usize, b: usize| {
                s.point(
                    u0 + (u1 - u0) * a as f64 / 8.,
                    v0 + (v1 - v0) * b as f64 / 8.,
                )
            };
            let a = p(i, j);
            let b = p(i + 1, j);
            let c = p(i + 1, j + 1);
            let d = p(i, j + 1);
            volume += (a.dot(b.cross(c)) + a.dot(c.cross(d))) / 6.;
        }
    }
    volume
}

/// Write faceted meshes and/or patch sets into one STEP file body.
pub fn write_step(
    mesh: Option<&Geometry>,
    patches: Option<&PatchSet>,
    opts: &StepOptions,
) -> Result<(String, StepSummary), String> {
    let mut w = Writer::new();

    let mut sum = StepSummary::default();

    let mut items: Vec<usize> = Vec::new();

    // product structure

    let name = text(&opts.name);

    let app = w.add("APPLICATION_CONTEXT('automotive design')");

    let proto = match opts.schema {

        Schema::Ap214 => w.add(format!("APPLICATION_PROTOCOL_DEFINITION('international standard','automotive_design',2000,#{app})")),

        Schema::Ap242 => w.add(format!("APPLICATION_PROTOCOL_DEFINITION('international standard','ap242_managed_model_based_3d_engineering',2020,#{app})")),

    };

    let _ = proto;

    let pctx = w.add(format!("PRODUCT_CONTEXT('',#{app},'mechanical')"));

    let prod = w.add(format!("PRODUCT('{name}','{name}','',(#{pctx}))"));

    let pdf = w.add(format!("PRODUCT_DEFINITION_FORMATION('','',#{prod})"));

    let pdctx = w.add(format!(
        "PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"
    ));

    let pd = w.add(format!("PRODUCT_DEFINITION('design','',#{pdf},#{pdctx})"));

    let pds = w.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));

    // units and context

    let lu = w.add("(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.))");

    let au = w.add("(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.))");

    let su = w.add("(NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT())");

    let unc = w.add(format!("UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-06),#{lu},'distance_accuracy_value','confusion accuracy')"));

    let ctx_id = w.add(format!("(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc})) GLOBAL_UNIT_ASSIGNED_CONTEXT((#{lu},#{au},#{su})) REPRESENTATION_CONTEXT('Context3D','3D Context with UNIT and UNCERTAINTY'))"));

    let mut reps: Vec<(&str, usize)> = Vec::new();

    if opts.max_faces == 0 {
        return Err("face limit must be positive".into());
    }

    if let Some(g) = mesh {
        if g.points
            .iter()
            .any(|p| !p.is_finite() || p.abs().max_element() > 1e50)
            || g.prims
                .iter()
                .any(|p| p.vertices.iter().any(|&v| v as usize >= g.points.len()))
        {
            return Err("invalid mesh coordinates or indices".into());
        }

        if g.prims.len() > opts.max_faces {
            return Err("mesh exceeds face limit".into());
        }

        let mut wd = weld(g, 1e-8f64.max(1e-13 * max_abs_coord(&g.points)));

        if wd.faces.is_empty() || wd.faces.len() != g.prims.len() {
            return Err("mesh contains empty, open or degenerate polygons".into());
        }

        let mut split = Vec::new();

        for f in &wd.faces {
            let a = wd.points[f[0] as usize];

            let mut n = DVec3::ZERO;

            for i in 1..f.len() - 1 {
                n += (wd.points[f[i] as usize] - a).cross(wd.points[f[i + 1] as usize] - a);
            }

            if !n.is_finite() || n.length() < 1e-16 {
                return Err("degenerate polygon area".into());
            }

            let normal = n.normalize();

            let planar = f
                .iter()
                .all(|&v| (wd.points[v as usize] - a).dot(normal).abs() <= 1e-8);

            if planar {
                split.push(f.clone());
            } else {
                // A non-planar polygon has no single plane. Its explicit fan is

                // the same tessellation convention used by Geometry::triangles.

                for i in 1..f.len() - 1 {
                    let tri = vec![f[0], f[i], f[i + 1]];

                    if (wd.points[tri[1] as usize] - a)
                        .cross(wd.points[tri[2] as usize] - a)
                        .length()
                        < 1e-16
                    {
                        return Err("degenerate polygon triangle".into());
                    }

                    split.push(tri);
                }
            }

            if split.len() > opts.max_faces {
                return Err("triangulated mesh exceeds face limit".into());
            }
        }

        wd.faces = split;

        orient(&mut wd);

        let em = crate::topology::edge_map(&wd.faces);

        if em
            .values()
            .any(|v| v.len() > 2 || (v.len() == 2 && v[0].1 == v[1].1))
        {
            return Err("nonmanifold or nonorientable mesh".into());
        }

        let mut links = vec![Vec::new(); wd.faces.len()];
        for uses in em.values() {
            if uses.len() == 2 {
                let a = uses[0].0 as usize;
                let b = uses[1].0 as usize;
                links[a].push(b);
                links[b].push(a);
            }
        }

        let pts: Vec<usize> = wd.points.iter().map(|&p| w.point(p)).collect();

        for component in components(&links) {
            let closed = component.iter().all(|&fi| {
                let f = &wd.faces[fi];
                (0..f.len()).all(|i| {
                    em[&crate::topology::edge_key(f[i], f[(i + 1) % f.len()]).0].len() == 2
                })
            });
            let volume = crate::topology::signed_volume(&wd.points, &wd.faces, &component);
            let mut face_ids = Vec::new();

            for &fi in &component {
                let f = &wd.faces[fi];
                let ids: Vec<usize> = f.iter().map(|&i| pts[i as usize]).collect();
                let coords: Vec<DVec3> = f.iter().map(|&i| wd.points[i as usize]).collect();
                face_ids.push(w.polygon_face(&ids, &coords));
            }

            sum.faces += face_ids.len();

            let (kind, item) = if closed && volume > 0. {
                sum.solid_shells += 1;
                let shell = w.add(format!("CLOSED_SHELL('',({}))", Writer::list(&face_ids)));
                (
                    "FACETED_BREP_SHAPE_REPRESENTATION",
                    w.add(format!("FACETED_BREP('{name}',#{shell})")),
                )
            } else {
                sum.open_shells += 1;
                let shell = w.add(format!("OPEN_SHELL('',({}))", Writer::list(&face_ids)));
                (
                    "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
                    w.add(format!("SHELL_BASED_SURFACE_MODEL('{name}',(#{shell}))")),
                )
            };

            reps.push((kind, item));
            items.push(item);
        }
    }

    if let Some(ps) = patches {
        if ps.units != "mm" {
            return Err("patch units must be mm".into());
        }
        if ps.patches.is_empty() || ps.patches.len() > 256 {
            return Err("patch count must be between 1 and 256".into());
        }

        if sum.faces.saturating_add(ps.patches.len()) > opts.max_faces {
            return Err("combined output exceeds face limit".into());
        }

        let mut vertices: Vec<(DVec3, usize)> = Vec::new();

        let mut edges: Vec<PatchEdge> = Vec::new();

        let mut loops: Vec<Vec<(usize, bool)>> = Vec::new();

        for p in &ps.patches {
            let s = &p.surface;
            if s.nu > 256 || s.nv > 256 || s.degree_u > 16 || s.degree_v > 16 {
                return Err("NURBS dimensions exceed resource limit".into());
            }
            if s.knots_u.first() == s.knots_u.last() || s.knots_v.first() == s.knots_v.last() {
                return Err("empty patch parameter domain".into());
            }
            s.validate().map_err(|e| format!("patch {}: {e}", p.id))?;

            if s.control_points.len() > 65536
                || s.degree_u > 16
                || s.degree_v > 16
                || s.control_points
                    .iter()
                    .any(|p| !p.is_finite() || p.abs().max_element() > 1e50)
                || s.knots_u
                    .iter()
                    .chain(&s.knots_v)
                    .any(|x| !x.is_finite() || x.abs() > 1e100)
                || s.weights.as_ref().is_some_and(|ws| {
                    ws.iter()
                        .any(|x| !x.is_finite() || *x < 1e-100 || *x > 1e100)
                })
            {
                return Err("invalid or excessive NURBS data".into());
            }

            if s.domain_u().1 - s.domain_u().0 < 1e-100 || s.domain_v().1 - s.domain_v().0 < 1e-100
            {
                return Err("patch parameter domain is too small".into());
            }
            if !clamped(&s.knots_u, s.degree_u) || !clamped(&s.knots_v, s.degree_v) {
                return Err("untrimmed STEP patch boundaries require clamped knot vectors".into());
            }

            let mut lp = Vec::new();

            for side in 0..4 {
                let c = s.boundary_curve(side);
                let c = if side >= 2 { c.reversed() } else { c };

                let (a, b) = c.domain();
                let a = c.point(a);
                let b = c.point(b);

                if a.distance(b) < 1e-10 {
                    return Err("collapsed patch boundary is unsupported".into());
                }

                let va = vertex(&mut w, &mut vertices, a);
                let vb = vertex(&mut w, &mut vertices, b);

                let matched = edges.iter().enumerate().find_map(|(i, e)| {
                    if e.a == va && e.b == vb && curves_equal(&e.curve, &c) {
                        Some((i, true))
                    } else if e.a == vb && e.b == va && curves_equal(&e.curve, &c.reversed()) {
                        Some((i, false))
                    } else {
                        None
                    }
                });

                let (idx, forward) = match matched {
                    Some(v) => v,
                    None => {
                        let curve = w.curve(&c);
                        let id = w.add(format!("EDGE_CURVE('',#{va},#{vb},#{curve},.T.)"));
                        edges.push(PatchEdge {
                            curve: c,
                            a: va,
                            b: vb,
                            id,
                            uses: Vec::new(),
                        });
                        (edges.len() - 1, true)
                    }
                };

                edges[idx].uses.push((loops.len(), forward));
                if edges[idx].uses.len() > 2 {
                    return Err("nonmanifold patch edge".into());
                }
                lp.push((idx, forward));
            }
            loops.push(lp);
        }

        sum.shared_edges = edges.iter().filter(|e| e.uses.len() == 2).count();

        let mut links = vec![Vec::new(); loops.len()];
        for e in &edges {
            if e.uses.len() == 2 {
                let a = e.uses[0].0;
                let b = e.uses[1].0;
                links[a].push(b);
                links[b].push(a);
            }
        }

        for comp in components(&links) {
            let mut flips = vec![None; loops.len()];
            flips[comp[0]] = Some(false);
            let mut todo = vec![comp[0]];

            while let Some(f) = todo.pop() {
                for &(ei, dir) in &loops[f] {
                    for &(other, odir) in &edges[ei].uses {
                        if other == f {
                            continue;
                        }
                        let flip = flips[f].unwrap() ^ (dir == odir);
                        if let Some(old) = flips[other] {
                            if old != flip {
                                return Err("nonorientable patch network".into());
                            }
                        } else {
                            flips[other] = Some(flip);
                            todo.push(other);
                        }
                    }
                }
            }

            let closed = comp
                .iter()
                .all(|&f| loops[f].iter().all(|&(e, _)| edges[e].uses.len() == 2));

            let origin = ps.patches[comp[0]].surface.control_points[0];
            let volume: f64 = comp
                .iter()
                .map(|&f| {
                    patch_volume(&ps.patches[f].surface, origin)
                        * if flips[f].unwrap() { -1. } else { 1. }
                })
                .sum();

            if closed && volume < 0. {
                for &f in &comp {
                    flips[f] = Some(!flips[f].unwrap());
                }
            }

            let mut face_ids = Vec::new();

            for &f in &comp {
                let surf = w.surface(&ps.patches[f].surface);
                let flip = flips[f].unwrap();
                let mut entries = loops[f].clone();
                if flip {
                    entries.reverse();
                }

                let mut oes = Vec::new();
                for (e, d) in entries {
                    let edge = edges[e].id;
                    let flag = if d ^ flip { ".T." } else { ".F." };
                    oes.push(w.add(format!("ORIENTED_EDGE('',*,*,#{edge},{flag})")));
                }

                let lp = w.add(format!("EDGE_LOOP('',({}))", Writer::list(&oes)));
                let bound = w.add(format!("FACE_OUTER_BOUND('',#{lp},.T.)"));
                let sense = if flip { ".F." } else { ".T." };
                face_ids.push(w.add(format!("ADVANCED_FACE('',(#{bound}),#{surf},{sense})")));
            }

            let (kind, item) = if closed && volume.abs() > 1e-12 {
                sum.solid_shells += 1;
                let shell = w.add(format!("CLOSED_SHELL('',({}))", Writer::list(&face_ids)));
                (
                    "ADVANCED_BREP_SHAPE_REPRESENTATION",
                    w.add(format!("MANIFOLD_SOLID_BREP('{name}',#{shell})")),
                )
            } else {
                sum.open_shells += 1;
                let shell = w.add(format!("OPEN_SHELL('',({}))", Writer::list(&face_ids)));
                (
                    "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
                    w.add(format!("SHELL_BASED_SURFACE_MODEL('{name}',(#{shell}))")),
                )
            };

            reps.push((kind, item));
            items.push(item);
        }

        sum.faces += ps.patches.len();
        sum.patches = ps.patches.len();
    }

    sum.closed_shell = sum.solid_shells > 0 && sum.open_shells == 0;

    if items.is_empty() {
        return Err("nothing to write: connect a mesh or a patch set".into());
    }

    let ax_o = w.add("CARTESIAN_POINT('',(0.,0.,0.))");

    let (dz, dx) = (
        w.add("DIRECTION('',(0.,0.,1.))"),
        w.add("DIRECTION('',(1.,0.,0.))"),
    );

    let ax = w.add(format!("AXIS2_PLACEMENT_3D('',#{ax_o},#{dz},#{dx})"));

    for (kind, item) in reps {
        let rep = w.add(format!("{kind}('{name}',(#{item},#{ax}),#{ctx_id})"));
        w.add(format!("SHAPE_DEFINITION_REPRESENTATION(#{pds},#{rep})"));
    }

    let schema = match opts.schema {
        Schema::Ap214 => "AUTOMOTIVE_DESIGN { 1 0 10303 214 3 1 1 }",

        Schema::Ap242 => "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF { 1 0 10303 442 1 1 4 }",
    };

    let mut out = String::with_capacity(w.body.len() + 600);

    out.push_str("ISO-10303-21;\nHEADER;\n");

    let _ = writeln!(
        out,
        "FILE_DESCRIPTION(('Vernon surface export, units millimetre'),'2;1');"
    );

    let _ = writeln!(out, "FILE_NAME('{name}.stp','1970-01-01T00:00:00',('Vernon'),('Vernon'),'Vernon STEP writer','Vernon','');");

    let _ = writeln!(out, "FILE_SCHEMA(('{schema}'));");

    out.push_str("ENDSEC;\nDATA;\n");

    out.push_str(&w.body);

    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");

    sum.entities = w.next - 1;

    sum.bytes = out.len();

    sum.bbox_min = w.bbox.0;

    sum.bbox_max = w.bbox.1;

    Ok((out, sum))
}

#[cfg(test)]
mod tests {

    use super::*;

    fn count(s: &str, k: &str) -> usize {
        s.lines()
            .filter(|l| {
                l.split_once('=')
                    .is_some_and(|(_, r)| r.starts_with(&format!("{k}(")))
            })
            .count()
    }

    #[test]
    fn cube_facets_are_face_surface() {
        let g = Geometry::make_box(DVec3::splat(10.));
        let (t, s) = write_step(Some(&g), None, &StepOptions::default()).unwrap();
        crate::step_validate::validate(&t).unwrap();
        assert_eq!(count(&t, "FACE_SURFACE"), 6);
        assert_eq!(count(&t, "ADVANCED_FACE"), 0);
        assert!(s.closed_shell);
    }

    #[test]
    fn adjacent_patch_edges_share_vertices_and_curves() {
        let sheet = |x: f64| {
            NurbsSurface::clamped_uniform(
                1,
                1,
                2,
                2,
                vec![
                    DVec3::new(x, 0., 0.),
                    DVec3::new(x, 1., 0.),
                    DVec3::new(x + 1., 0., 0.),
                    DVec3::new(x + 1., 1., 0.),
                ],
            )
        };
        let ps = PatchSet::new(vec![sheet(0.), sheet(1.)]);
        let (t, _) = write_step(None, Some(&ps), &StepOptions::default()).unwrap();
        assert_eq!(count(&t, "VERTEX_POINT"), 6);
        assert_eq!(count(&t, "EDGE_CURVE"), 7);
        assert_eq!(count(&t, "OPEN_SHELL"), 1);
    }

    #[test]
    fn disconnected_boxes_have_separate_typed_representations() {
        let mut g = Geometry::make_box(DVec3::splat(2.));
        let mut b = g.clone();
        for p in &mut b.points {
            p.x += 10.;
        }
        let offset = g.points.len() as u32;
        g.points.extend(b.points);
        for mut p in b.prims {
            for v in &mut p.vertices {
                *v += offset;
            }
            g.prims.push(p);
        }
        let (t, s) = write_step(Some(&g), None, &StepOptions::default()).unwrap();
        assert_eq!(count(&t, "CLOSED_SHELL"), 2);
        assert_eq!(count(&t, "FACETED_BREP_SHAPE_REPRESENTATION"), 2);
        assert_eq!(s.solid_shells, 2);
    }

    #[test]
    fn closed_cube_patch_network_is_an_advanced_solid() {
        let g = Geometry::make_box(DVec3::splat(2.));
        let surfaces = g
            .prims
            .iter()
            .map(|f| {
                let p: Vec<_> = f.vertices.iter().map(|&i| g.points[i as usize]).collect();
                NurbsSurface::clamped_uniform(1, 1, 2, 2, vec![p[0], p[3], p[1], p[2]])
            })
            .collect();
        let ps = PatchSet::new(surfaces);
        let (t, s) = write_step(None, Some(&ps), &StepOptions::default()).unwrap();
        crate::step_validate::validate(&t).unwrap();
        assert_eq!(count(&t, "EDGE_CURVE"), 12);
        assert_eq!(count(&t, "VERTEX_POINT"), 8);
        assert_eq!(count(&t, "MANIFOLD_SOLID_BREP"), 1);
        assert_eq!(count(&t, "ADVANCED_BREP_SHAPE_REPRESENTATION"), 1);
        assert_eq!(s.shared_edges, 12);
        assert!(s.closed_shell);
    }

    #[test]
    fn nonplanar_quad_triangulates_and_combined_limit_is_actual() {
        let mut g = Geometry {
            points: vec![DVec3::ZERO, DVec3::X, DVec3::new(1., 1., 0.2), DVec3::Y],
            ..Default::default()
        };
        g.prims.push(vernon_core::Prim::polygon(vec![0, 1, 2, 3]));
        let (t, s) = write_step(Some(&g), None, &StepOptions::default()).unwrap();
        assert_eq!(count(&t, "FACE_SURFACE"), 2);
        assert_eq!(s.faces, 2);
        assert!(write_step(
            Some(&g),
            None,
            &StepOptions {
                max_faces: 1,
                ..Default::default()
            }
        )
        .is_err());
    }

    #[test]
    fn mixed_mesh_and_rational_patch_get_separate_representation_types() {
        let g = Geometry::make_box(DVec3::splat(2.));
        let mut p = NurbsSurface::clamped_uniform(
            1,
            1,
            2,
            2,
            vec![DVec3::ZERO, DVec3::Y, DVec3::X, DVec3::X + DVec3::Y],
        );
        p.weights = Some(vec![1., 2., 3., 4.]);
        let ps = PatchSet::new(vec![p]);
        let (t, s) = write_step(
            Some(&g),
            Some(&ps),
            &StepOptions {
                schema: Schema::Ap242,
                name: "O'Brien\\é".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(t.is_ascii());
        assert!(t.contains("O''Brien__"));
        crate::step_validate::validate(&t).unwrap();
        assert!(t.contains("RATIONAL_B_SPLINE_SURFACE"));
        assert_eq!(count(&t, "FACETED_BREP_SHAPE_REPRESENTATION"), 1);
        assert_eq!(count(&t, "MANIFOLD_SURFACE_SHAPE_REPRESENTATION"), 1);
        assert_eq!(s.open_shells, 1);
        assert_eq!(s.solid_shells, 1);
        assert!(!s.closed_shell);
        assert!(write_step(
            Some(&g),
            Some(&ps),
            &StepOptions {
                max_faces: 6,
                ..Default::default()
            }
        )
        .is_err());
    }

    #[test]
    fn invalid_nonmanifold_and_excessive_input_are_errors() {
        let mut g = Geometry::make_box(DVec3::splat(2.));
        g.prims.push(g.prims[0].clone());
        assert!(write_step(Some(&g), None, &StepOptions::default()).is_err());
        g.prims.pop();
        g.prims[0].vertices[0] = 1000;
        assert!(write_step(Some(&g), None, &StepOptions::default()).is_err());
        let s = NurbsSurface::clamped_uniform(
            1,
            1,
            2,
            2,
            vec![DVec3::ZERO, DVec3::Y, DVec3::X, DVec3::X + DVec3::Y],
        );
        assert!(write_step(
            None,
            Some(&PatchSet::new(vec![s; 257])),
            &StepOptions::default()
        )
        .is_err());
    }

    #[test]
    fn scientific_real_mantissa_is_valid() {
        assert_eq!(real(1e-30), "1.E-30");
        assert_eq!(real(1e30), "1.E30");
    }

    #[test]
    fn extreme_finite_knots_are_rejected_before_evaluation() {
        let mut p = NurbsSurface::clamped_uniform(
            1,
            1,
            2,
            2,
            vec![DVec3::ZERO, DVec3::Y, DVec3::X, DVec3::X + DVec3::Y],
        );
        p.knots_u = vec![-1e308, -1e308, 1e308, 1e308];
        assert!(write_step(None, Some(&PatchSet::new(vec![p])), &StepOptions::default()).is_err());
    }
    fn flat_sheet(x0: f64, x1: f64, y1: f64) -> NurbsSurface {
        NurbsSurface::clamped_uniform(
            1,
            1,
            2,
            2,
            vec![
                DVec3::new(x0, 0., 0.),
                DVec3::new(x0, y1, 0.),
                DVec3::new(x1, 0., 0.),
                DVec3::new(x1, y1, 0.),
            ],
        )
    }
    #[test]
    fn reversed_patch_boundary_has_consistent_shared_orientation() {
        let ps = PatchSet::new(vec![flat_sheet(0., 1., 1.), flat_sheet(2., 1., 1.)]);
        let (t, s) = write_step(None, Some(&ps), &StepOptions::default()).unwrap();
        crate::step_validate::validate(&t).unwrap();
        assert_eq!(count(&t, "EDGE_CURVE"), 7);
        assert_eq!(count(&t, "VERTEX_POINT"), 6);
        assert_eq!(s.shared_edges, 1);
        assert_eq!(s.open_shells, 1);
        assert!(t
            .lines()
            .any(|l| l.contains("ADVANCED_FACE") && l.ends_with(",.F.);")));
    }
    #[test]
    fn partial_seams_and_claimed_closed_flag_do_not_close_independent_patches() {
        let mut ps = PatchSet::new(vec![flat_sheet(0., 1., 1.), flat_sheet(1., 2., 2.)]);
        ps.closed = true;
        ps.solid = true;
        let (t, s) = write_step(None, Some(&ps), &StepOptions::default()).unwrap();
        crate::step_validate::validate(&t).unwrap();
        assert_eq!(s.shared_edges, 0);
        assert_eq!(s.open_shells, 2);
        assert_eq!(count(&t, "OPEN_SHELL"), 2);
        assert!(!s.closed_shell);
    }
    #[test]
    fn non_mm_patch_units_are_rejected() {
        let mut ps = PatchSet::new(vec![flat_sheet(0., 1., 1.)]);
        ps.units = "metre".into();
        assert!(write_step(None, Some(&ps), &StepOptions::default()).is_err());
    }
    #[test]
    fn translated_cube_patch_volume_remains_local_and_positive() {
        for offset in [1e8, 1e10] {
            let mut g = Geometry::make_box(DVec3::splat(10.));
            for p in &mut g.points {
                *p += DVec3::splat(offset);
            }
            let surfaces: Vec<_> = g
                .prims
                .iter()
                .map(|f| {
                    let p: Vec<_> = f.vertices.iter().map(|&i| g.points[i as usize]).collect();
                    NurbsSurface::clamped_uniform(1, 1, 2, 2, vec![p[0], p[3], p[1], p[2]])
                })
                .collect();
            let volume: f64 = surfaces
                .iter()
                .map(|s| patch_volume(s, surfaces[0].control_points[0]))
                .sum();
            assert!(
                (volume - 1000.).abs() < 1e-4,
                "offset {offset}: volume {volume}"
            );
            let (t, sum) = write_step(
                None,
                Some(&PatchSet::new(surfaces)),
                &StepOptions::default(),
            )
            .unwrap();
            crate::step_validate::validate(&t).unwrap();
            assert_eq!(sum.solid_shells, 1);
            assert_eq!(sum.open_shells, 0);
            assert!(!t
                .lines()
                .any(|l| l.contains("ADVANCED_FACE") && l.ends_with(",.F.);")));
        }
    }
    #[test]
    fn large_scale_nearby_rational_weights_must_not_merge_different_curves() {
        let mut a = NurbsCurve::clamped_uniform(
            vec![
                DVec3::ZERO,
                DVec3::new(1e12, 1e12, 0.),
                DVec3::new(2e12, 0., 0.),
            ],
            2,
        );
        a.weights = Some(vec![1., 1., 1.]);
        let mut b = a.clone();
        b.weights.as_mut().unwrap()[1] += 5e-13;
        assert!(a.point(0.5).distance(b.point(0.5)) > 0.01);
        assert!(!curves_equal(&a, &b));
    }
}
