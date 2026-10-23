//! STEP (ISO 10303-21) writer for faceted shells and untrimmed NURBS patches,
//! and the `surface.write_step` node.
//!
//! Units are always millimetres. Faceted meshes are written as planar
//! advanced faces bounded by poly loops; patches as B-spline surface faces
//! bounded by their four border curves (untrimmed).

use std::fmt::Write as _;
use std::path::PathBuf;

use glam::DVec3;
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::nurbs::{compress_knots, NurbsCurve, NurbsSurface, PatchSet};
use crate::topology::{orient, stats, weld};
use crate::util::{check_finite, json_err, json_input, max_abs_coord, node_info, placeholder_note, SPEC_RE};

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
        StepOptions { schema: Schema::Ap214, name: "vernon_surface".into(), max_faces: 500_000 }
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
    pub bbox_min: DVec3,
    pub bbox_max: DVec3,
}

struct Writer {
    body: String,
    next: usize,
    bbox: (DVec3, DVec3),
}

fn real(x: f64) -> String {
    let mut s = format!("{x}");
    if !s.contains('.') {
        s.push('.');
    }
    s
}

fn text(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii() && c != '\'' && !c.is_ascii_control() { c } else { '_' }).collect()
}

impl Writer {
    fn new() -> Self {
        Writer { body: String::new(), next: 1, bbox: (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)) }
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
        self.add(format!("CARTESIAN_POINT('',({},{},{}))", real(p.x), real(p.y), real(p.z)))
    }

    fn direction(&mut self, d: DVec3) -> usize {
        self.add(format!("DIRECTION('',({},{},{}))", real(d.x), real(d.y), real(d.z)))
    }

    fn list(ids: &[usize]) -> String {
        ids.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(",")
    }

    fn curve(&mut self, c: &NurbsCurve) -> usize {
        let cps: Vec<usize> = c.control_points.iter().map(|&p| self.point(p)).collect();
        let (vals, mult) = compress_knots(&c.knots);
        let m = mult.iter().map(usize::to_string).collect::<Vec<_>>().join(",");
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

    /// One untrimmed patch face; returns the ADVANCED_FACE id.
    fn patch_face(&mut self, s: &NurbsSurface) -> usize {
        let surf = self.surface(s);
        let mut oriented = Vec::with_capacity(4);
        for side in 0..4 {
            let c = s.boundary_curve(side);
            let (t0, t1) = c.domain();
            let (a, b) = (c.point(t0), c.point(t1));
            let curve = self.curve(&c);
            let (pa, pb) = (self.point(a), self.point(b));
            let (va, vb) = (self.add(format!("VERTEX_POINT('',#{pa})")), self.add(format!("VERTEX_POINT('',#{pb})")));
            let edge = self.add(format!("EDGE_CURVE('',#{va},#{vb},#{curve},.T.)"));
            let flag = if side < 2 { ".T." } else { ".F." };
            oriented.push(self.add(format!("ORIENTED_EDGE('',*,*,#{edge},{flag})")));
        }
        // loop order: side 0 forward, side 1 forward, side 2 reversed, side 3 reversed
        let lp = self.add(format!("EDGE_LOOP('',({}))", Self::list(&oriented)));
        let bound = self.add(format!("FACE_OUTER_BOUND('',#{lp},.T.)"));
        self.add(format!("ADVANCED_FACE('',(#{bound}),#{surf},.T.)"))
    }

    /// One planar polygon face over shared point ids.
    fn polygon_face(&mut self, pts: &[usize], coords: &[DVec3]) -> usize {
        let mut n = DVec3::ZERO;
        for i in 0..coords.len() {
            let (a, b) = (coords[i], coords[(i + 1) % coords.len()]);
            n += DVec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
        }
        let n = n.normalize_or_zero();
        let n = if n == DVec3::ZERO { DVec3::Z } else { n };
        let xdir = (coords[1] - coords[0]).normalize_or_zero();
        let xdir = if xdir == DVec3::ZERO { n.any_orthonormal_vector() } else { (xdir - n * xdir.dot(n)).normalize_or_zero() };
        let xdir = if xdir == DVec3::ZERO { n.any_orthonormal_vector() } else { xdir };
        let o = self.add(format!("CARTESIAN_POINT('',({},{},{}))", real(coords[0].x), real(coords[0].y), real(coords[0].z)));
        let (dz, dx) = (self.direction(n), self.direction(xdir));
        let ax = self.add(format!("AXIS2_PLACEMENT_3D('',#{o},#{dz},#{dx})"));
        let plane = self.add(format!("PLANE('',#{ax})"));
        let lp = self.add(format!("POLY_LOOP('',({}))", Self::list(pts)));
        let bound = self.add(format!("FACE_OUTER_BOUND('',#{lp},.T.)"));
        self.add(format!("ADVANCED_FACE('',(#{bound}),#{plane},.T.)"))
    }
}

/// Write faceted meshes and/or patch sets into one STEP file body.
pub fn write_step(mesh: Option<&Geometry>, patches: Option<&PatchSet>, opts: &StepOptions) -> Result<(String, StepSummary), String> {
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
    let pdctx = w.add(format!("PRODUCT_DEFINITION_CONTEXT('part definition',#{app},'design')"));
    let pd = w.add(format!("PRODUCT_DEFINITION('design','',#{pdf},#{pdctx})"));
    let pds = w.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{pd})"));
    // units and context
    let lu = w.add("(LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.))");
    let au = w.add("(NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.))");
    let su = w.add("(NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT())");
    let unc = w.add(format!("UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-06),#{lu},'distance_accuracy_value','confusion accuracy')"));
    let ctx_id = w.add(format!("(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{unc})) GLOBAL_UNIT_ASSIGNED_CONTEXT((#{lu},#{au},#{su})) REPRESENTATION_CONTEXT('Context3D','3D Context with UNIT and UNCERTAINTY'))"));

    if let Some(g) = mesh {
        let mut wd = weld(g, 1e-6f64.max(1.2e-7 * max_abs_coord(&g.points)));
        if wd.faces.is_empty() {
            return Err("the mesh has no usable faces".into());
        }
        if wd.faces.len() > opts.max_faces {
            return Err(format!("{} faces are too many for a faceted STEP file (limit {}); decimate the mesh or raise the limit", wd.faces.len(), opts.max_faces));
        }
        orient(&mut wd);
        let st = stats(&wd);
        let pts: Vec<usize> = wd.points.iter().map(|&p| w.point(p)).collect();
        let mut face_ids = Vec::with_capacity(wd.faces.len());
        for f in &wd.faces {
            let ids: Vec<usize> = f.iter().map(|&v| pts[v as usize]).collect();
            let coords: Vec<DVec3> = f.iter().map(|&v| wd.points[v as usize]).collect();
            face_ids.push(w.polygon_face(&ids, &coords));
        }
        sum.faces += face_ids.len();
        if st.closed && st.volume > 0.0 {
            let shell = w.add(format!("CLOSED_SHELL('',({}))", Writer::list(&face_ids)));
            items.push(w.add(format!("FACETED_BREP('{name}',#{shell})")));
            sum.closed_shell = true;
        } else {
            let shell = w.add(format!("OPEN_SHELL('',({}))", Writer::list(&face_ids)));
            items.push(w.add(format!("SHELL_BASED_SURFACE_MODEL('{name}',(#{shell}))")));
        }
    }
    if let Some(ps) = patches {
        if ps.patches.is_empty() {
            return Err("the patch set is empty".into());
        }
        let mut face_ids = Vec::with_capacity(ps.patches.len());
        for p in &ps.patches {
            p.surface.validate().map_err(|e| format!("patch {}: {e}", p.id))?;
            face_ids.push(w.patch_face(&p.surface));
        }
        sum.patches += face_ids.len();
        sum.faces += face_ids.len();
        let shell = w.add(format!("OPEN_SHELL('',({}))", Writer::list(&face_ids)));
        items.push(w.add(format!("SHELL_BASED_SURFACE_MODEL('{name}_patches',(#{shell}))")));
    }
    if items.is_empty() {
        return Err("nothing to write: connect a mesh or a patch set".into());
    }
    let rep_type = if mesh.is_some() && patches.is_none() && sum.closed_shell { "FACETED_BREP_SHAPE_REPRESENTATION" } else { "MANIFOLD_SURFACE_SHAPE_REPRESENTATION" };
    let ax_o = w.add("CARTESIAN_POINT('',(0.,0.,0.))");
    let (dz, dx) = (w.add("DIRECTION('',(0.,0.,1.))"), w.add("DIRECTION('',(1.,0.,0.))"));
    let ax = w.add(format!("AXIS2_PLACEMENT_3D('',#{ax_o},#{dz},#{dx})"));
    let rep = w.add(format!("{rep_type}('{name}',({},#{ax}),#{ctx_id})", Writer::list(&items)));
    w.add(format!("SHAPE_DEFINITION_REPRESENTATION(#{pds},#{rep})"));

    let schema = match opts.schema {
        Schema::Ap214 => "AUTOMOTIVE_DESIGN { 1 0 10303 214 3 1 1 }",
        Schema::Ap242 => "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF { 1 0 10303 442 1 1 4 }",
    };
    let mut out = String::with_capacity(w.body.len() + 600);
    out.push_str("ISO-10303-21;\nHEADER;\n");
    let _ = writeln!(out, "FILE_DESCRIPTION(('Vernon surface export, units millimetre'),'2;1');");
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

/// `surface.write_step`
pub struct WriteStepNode;

impl NodeOp for WriteStepNode {
    fn info(&self) -> NodeTypeInfo {
        node_info("surface.write_step", "Write STEP", SPEC_RE)
            .input(PortSpec::optional("patches", DataKind::Json))
            .input(PortSpec::optional("mesh", DataKind::Geometry))
            .output(PortSpec::new("file", DataKind::File))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("schema", "Schema", "ap214", &["ap214", "ap242"]).help("Application protocol named in the file header."))
            .param(ParamSpec::string("name", "Part name", "vernon_surface").help("Name stored in the file and used for the file name."))
            .param(ParamSpec::path("save_to", "Save to", &["stp", "step"], true).help("Optional extra copy written to this path after the file is created."))
            .param(ParamSpec::int("max_faces", "Face limit", 500_000, 100, 50_000_000).help("Refuse faceted meshes with more faces than this."))
            .help("Writes a STEP file in millimetres from a patch set (untrimmed surface faces) and/or a mesh (faceted shell; a closed shell becomes a solid). Border curves of patches are written as edges; trimmed faces and shared edges between patches are not written yet.")
            .emulates("STEP export of surfaces and solids")
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = vernon_graph::node::inputs::opt_geometry(inputs, 1);
        let pj = json_input(inputs, 0);
        if let Some(m) = &mesh {
            check_finite(ctx, m)?;
        }
        let ps = match pj {
            Some(j) => Some(PatchSet::from_json(&j).map_err(|e| json_err(ctx, "patch set input", e))?),
            None => None,
        };
        if let Some(p) = &ps {
            for patch in &p.patches {
                if patch.surface.control_points.iter().any(|c| !c.is_finite()) {
                    return Err(ctx.fail("a patch has non-finite control points"));
                }
            }
        }
        let opts = StepOptions {
            schema: if params.str("schema")? == "ap242" { Schema::Ap242 } else { Schema::Ap214 },
            name: params.str("name")?.trim().to_string(),
            max_faces: params.float("max_faces")? as usize,
        };
        if opts.name.is_empty() {
            return Err(ctx.fail("the part name must not be empty"));
        }
        let (content, sum) = write_step(mesh.as_deref(), ps.as_ref(), &opts).map_err(|e| ctx.fail(e))?;
        std::fs::create_dir_all(&ctx.work_dir).map_err(|e| ctx.fail(format!("cannot create the work directory: {e}")))?;
        let file_name: String = opts.name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
        let path = ctx.work_dir.join(format!("{file_name}.stp"));
        write_atomic(&path, &content).map_err(|e| ctx.fail(format!("cannot write {}: {e}", path.display())))?;
        let mut report = Report::new("STEP export");
        report.metric("faces", Metric::new(sum.faces as f64, "count"));
        report.metric("patches", Metric::new(sum.patches as f64, "count"));
        report.metric("entities", Metric::new(sum.entities as f64, "count"));
        report.metric("file_bytes", Metric::new(sum.bytes as f64, "bytes"));
        report.metric("closed_shell", Metric::new(f64::from(sum.closed_shell), ""));
        let size = sum.bbox_max - sum.bbox_min;
        report.metric("bbox_x_mm", Metric::new(size.x, "mm"));
        report.metric("bbox_y_mm", Metric::new(size.y, "mm"));
        report.metric("bbox_z_mm", Metric::new(size.z, "mm"));
        if sum.patches > 0 {
            report.note(Severity::Info, "patches are written untrimmed, one face per patch, without shared edges");
        }
        placeholder_note(&mut report, "trimmed faces and shared edge topology between patches (a boundary-representation kernel); this writer emits a faceted shell and untrimmed patch faces");
        let save = params.str("save_to")?.trim().to_string();
        if !save.is_empty() {
            let target = PathBuf::from(&save);
            write_atomic(&target, &content).map_err(|e| ctx.fail(format!("cannot save to {save}: {e}")))?;
        }
        Ok(vec![Data::File(path), Data::report(report)])
    }
}

fn write_atomic(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{cook, f, grid, report_of, s, some};

    fn count(content: &str, kind: &str) -> usize {
        content.lines().filter(|l| l.split_once('=').is_some_and(|(_, r)| r.starts_with(&format!("{kind}(")))).count()
    }

    fn points(content: &str) -> Vec<DVec3> {
        content
            .lines()
            .filter_map(|l| {
                let r = l.split_once('=')?.1;
                let inner = r.strip_prefix("CARTESIAN_POINT('',(")?.strip_suffix("));")?;
                let v: Vec<f64> = inner.split(',').map(|x| x.parse().unwrap()).collect();
                (v.len() == 3).then(|| DVec3::new(v[0], v[1], v[2]))
            })
            .collect()
    }

    fn sheet(x0: f64) -> NurbsSurface {
        let mut pts = Vec::new();
        for i in 0..4 {
            for j in 0..4 {
                pts.push(DVec3::new(x0 + 10.0 * i as f64 / 3.0, 10.0 * j as f64 / 3.0, ((i * j) as f64).sin()));
            }
        }
        NurbsSurface::clamped_uniform(3, 3, 4, 4, pts)
    }

    #[test]
    fn faceted_box_is_a_closed_brep_in_millimetres() {
        let g = Geometry::make_box(DVec3::new(10.0, 20.0, 30.0));
        let (txt, sum) = write_step(Some(&g), None, &StepOptions::default()).unwrap();
        assert!(txt.starts_with("ISO-10303-21;\n"));
        assert!(txt.trim_end().ends_with("END-ISO-10303-21;"));
        assert!(txt.contains("SI_UNIT(.MILLI.,.METRE.)"));
        assert!(txt.contains("FILE_SCHEMA(('AUTOMOTIVE_DESIGN"));
        assert_eq!(count(&txt, "ADVANCED_FACE"), 6);
        assert_eq!(count(&txt, "FACETED_BREP"), 1);
        assert_eq!(count(&txt, "CLOSED_SHELL"), 1);
        assert!(sum.closed_shell);
        assert!(txt.is_ascii());
        assert!((sum.bbox_max - sum.bbox_min - DVec3::new(10.0, 20.0, 30.0)).length() < 1e-9);
    }

    #[test]
    fn open_mesh_is_an_open_shell_and_ap242_changes_the_header() {
        let g = grid(2, 2, 10.0, 10.0, |_, _| 0.0);
        let (txt, sum) = write_step(Some(&g), None, &StepOptions { schema: Schema::Ap242, ..Default::default() }).unwrap();
        assert!(txt.contains("AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF"));
        assert_eq!(count(&txt, "OPEN_SHELL"), 1);
        assert_eq!(count(&txt, "FACETED_BREP"), 0);
        assert!(!sum.closed_shell);
        assert_eq!(count(&txt, "ADVANCED_FACE"), 4);
    }

    #[test]
    fn two_patches_give_two_faces_and_the_input_bounding_box() {
        // spec test 15 (our own parse instead of an external reader)
        let ps = PatchSet::new(vec![sheet(0.0), sheet(10.0)]);
        let (txt, sum) = write_step(None, Some(&ps), &StepOptions::default()).unwrap();
        assert_eq!(count(&txt, "ADVANCED_FACE"), 2);
        assert_eq!(count(&txt, "B_SPLINE_SURFACE_WITH_KNOTS"), 2);
        assert_eq!(count(&txt, "EDGE_CURVE"), 8);
        let cps: Vec<DVec3> = ps.patches.iter().flat_map(|p| p.surface.control_points.clone()).collect();
        let (lo, hi) = cps.iter().fold((DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)), |(l, h), p| (l.min(*p), h.max(*p)));
        assert!((sum.bbox_min - lo).length() < 1e-4 && (sum.bbox_max - hi).length() < 1e-4);
        // distinct knots plus multiplicities: (4,4) and (0,1)
        assert!(txt.contains("(4,4),(4,4),(0.,1.),(0.,1.)"), "{}", txt.lines().find(|l| l.contains("B_SPLINE_SURFACE_WITH_KNOTS")).unwrap());
        // all CARTESIAN_POINT entries parse as three numbers
        assert!(points(&txt).len() > 32);
    }

    #[test]
    fn rational_patches_use_the_complex_entity() {
        let mut s0 = sheet(0.0);
        s0.weights = Some(vec![1.0; 16]);
        let (txt, _) = write_step(None, Some(&PatchSet::new(vec![s0])), &StepOptions::default()).unwrap();
        assert!(txt.contains("RATIONAL_B_SPLINE_SURFACE"));
    }

    #[test]
    fn node_writes_a_file_and_recreates_it() {
        let g = Geometry::make_box(DVec3::splat(10.0));
        let out = cook("surface.write_step", &[None, some(&g)], &[("name", s("box part"))]).unwrap();
        let path = out[0].as_file().unwrap().clone();
        assert!(path.exists());
        assert!(path.file_name().unwrap().to_str().unwrap().starts_with("box_part"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("FACETED_BREP"));
        std::fs::remove_file(&path).unwrap();
        let again = cook("surface.write_step", &[None, some(&g)], &[("name", s("box part"))]).unwrap();
        assert!(again[0].as_file().unwrap().exists());
        assert_eq!(report_of(&out[1]).metrics["faces"].value, 6.0);
        // an unwritable extra copy is an error, not a panic
        let bad = cook("surface.write_step", &[None, some(&g)], &[("save_to", s("Z:/no/such/dir/out.stp"))]);
        assert!(bad.is_err());
        // limits and empty inputs
        assert!(cook("surface.write_step", &[None, None], &[]).is_err());
        assert!(cook("surface.write_step", &[None, some(&g)], &[("max_faces", f(100.0)), ("name", s(""))]).is_err());
    }

    #[test]
    fn non_finite_coordinates_are_rejected() {
        let mut g = Geometry::make_box(DVec3::splat(10.0));
        g.points[0].x = f64::NAN;
        assert!(cook("surface.write_step", &[None, some(&g)], &[]).is_err());
    }
}
