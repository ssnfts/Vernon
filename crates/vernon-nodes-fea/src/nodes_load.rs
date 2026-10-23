//! Load nodes: aero pressure on the solid mesh, bump (inertial) load, and the
//! named load-case set that the deck writer turns into one deck per case.

use std::collections::BTreeMap;

use glam::DVec3;
use vernon_core::{Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::loads::{accel_field, bin_pressures, dynamic_pressure_pa, face_geometry, field_pressures, mesh_mass_tonne, nearest_node, net_force_pressures, pressure_resultant};
use crate::material::MaterialCard;
use crate::types::{finite, from_data, opt_from_data, to_data, CaseSet, FacePressure, Gravity, LoadCase, LoadSet, MountSet, NodalForce};
use crate::util::{fea_info, sanitize, solid_mesh};

/// Downforce, drag and design dynamic pressure read from an aero-loads table.
struct CfdDesign {
    downforce_n: f64,
    drag_n: f64,
    q_pa: f64,
}

/// Read the design row of the aero-loads table of the aerodynamics stage, if connected.
fn cfd_design(ctx: &CookContext, inputs: &[Option<Data>], index: usize) -> CookResult<Option<CfdDesign>> {
    let Some(v) = inputs.get(index).and_then(|d| d.as_ref()).and_then(|d| d.as_json()) else { return Ok(None) };
    if v.get("placeholder").and_then(|p| p.as_bool()).unwrap_or(false) {
        return Err(ctx.fail("the aero loads carry no solver data yet: run the aerodynamic study first or disconnect the input"));
    }
    let d = v.get("design").ok_or_else(|| ctx.fail("the aero-loads input has no design row"))?;
    let num = |k: &str| d.get(k).and_then(|x| x.as_f64());
    let downforce_n = num("design_downforce_n").or_else(|| num("downforce_n")).ok_or_else(|| ctx.fail("the aero-loads input has no design downforce"))?;
    Ok(Some(CfdDesign { downforce_n, drag_n: num("drag_n").unwrap_or(0.0), q_pa: num("design_q_pa").or_else(|| num("q_pa")).unwrap_or(0.0) }))
}

/// Sum several pressures that act on the same element face.
fn merge_face_pressures(ps: Vec<FacePressure>) -> Vec<FacePressure> {
    let mut m: BTreeMap<(u32, u8), f64> = BTreeMap::new();
    for p in ps {
        *m.entry((p.elem, p.face)).or_insert(0.0) += p.p_mpa;
    }
    m.into_iter().map(|((elem, face), p_mpa)| FacePressure { elem, face, p_mpa }).collect()
}

// ------------------------------------------------------------- aero load

pub struct LoadAero;

impl NodeOp for LoadAero {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.load_aero", "Aero Load")
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::optional("surface", DataKind::Geometry))
            .input(PortSpec::optional("aero_loads", DataKind::Json))
            .output(PortSpec::new("loads", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("mode", "Mode", "net_force", &["net_force", "field"]).help("Uniform pressure on the faces that look into the load direction, or a pressure field mapped from a surface carrying the point attribute cp"))
            .param(ParamSpec::string("name", "Load name", "aero").help("Name of the load set"))
            .param(ParamSpec::float("cla_m2", "Force area", 0.5, 0.0, 10.0).unit("m2").help("Downforce area ClA (or drag area CdA for a drag load); used when no aero-loads table is connected"))
            .param(ParamSpec::choice("quantity", "Table quantity", "downforce", &["downforce", "drag"]).help("Which number of a connected aero-loads table to apply; set the direction to match"))
            .param(ParamSpec::float("v_kmh", "Speed", 200.0, 30.0, 400.0).unit("km/h"))
            .param(ParamSpec::float("rho", "Air density", 1.11, 0.9, 1.3).unit("kg/m3").help("Hot-day site density"))
            .param(ParamSpec::vec3("direction", "Force direction", DVec3::new(0.0, 0.0, -1.0)).help("Direction of the net force on the part; downforce pushes along -Z"))
            .param(ParamSpec::float("min_cos", "Facing threshold", 0.05, 0.0, 0.9).help("Faces whose normal is within this cosine of edge-on are left unloaded (net_force mode)"))
            .param(ParamSpec::float("cp_in", "Inside pressure coefficient", 0.0, -2.0, 2.0).help("Pressure coefficient on the inside of the part (field mode)"))
            .param(ParamSpec::int("bins", "Pressure classes", 48, 0, 64).help("Faces are grouped into this many pressure classes (0 keeps one pressure per face); the integral is preserved"))
            .param(ParamSpec::float("snap_mm", "Mapping distance", 20.0, 0.1, 1000.0).unit("mm").help("Faces farther than this from the pressure surface stay unloaded (field mode)"))
            .param(ParamSpec::float("tol_pct", "Conservation tolerance", 1.0, 0.01, 50.0).unit("%").help("Allowed difference between the applied and the requested resultant"))
            .param(ParamSpec::bool("strict", "Stop on failure", true).help("Stop when the conservation check fails instead of only flagging it in the report"))
            .help("Aero pressure on the faces of the solid mesh. The net-force mode spreads q * ClA over the faces looking into the load direction; the field mode maps a pressure-coefficient field from a surface and checks that the applied resultant matches the surface integral. Positive pressure pushes into the body, suction pulls out.")
            .emulates("pressure loads on a static study from wind-tunnel or flow-solver results")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = solid_mesh(ctx, inputs, 0)?;
        let design = cfd_design(ctx, inputs, 2)?;
        let tol = params.float("tol_pct")?;
        let q_param = dynamic_pressure_pa(params.float("rho")?, params.float("v_kmh")?);
        let q = design.as_ref().filter(|d| d.q_pa > 0.0).map_or(q_param, |d| d.q_pa);
        let mut r = Report::new("Aero load");
        r.metric("dynamic_pressure_pa", Metric::new(q, "Pa"));
        let mut notes: Vec<String> = Vec::new();
        let (mut ps, target, axis_dir) = if params.str("mode")? == "field" {
            let surf = inputs::opt_geometry(inputs, 1).ok_or_else(|| ctx.fail("field mode needs a pressure surface with a point attribute cp"))?;
            let f = field_pressures(&g, &surf, q, params.float("cp_in")?, params.float("snap_mm")?).map_err(|e| ctx.fail(e))?;
            r.metric("loaded_faces", Metric::new(f.loaded_faces as f64, ""));
            r.metric("loaded_area_mm2", Metric::new(f.loaded_area_mm2, "mm2"));
            (f.pressures, f.resultant_surface, None)
        } else {
            let d = params.vec3("direction")?.normalize_or_zero();
            let force = match &design {
                Some(c) if params.str("quantity")? == "drag" => c.drag_n,
                Some(c) => c.downforce_n,
                None => q * params.float("cla_m2")?,
            };
            if force == 0.0 {
                return Err(ctx.fail("the requested force is zero"));
            }
            let nf = net_force_pressures(&g, d, force, params.float("min_cos")?).map_err(|e| ctx.fail(e))?;
            r.metric("projected_area_mm2", Metric::new(nf.projected_area_mm2, "mm2"));
            r.metric("loaded_faces", Metric::new(nf.pressures.len() as f64, ""));
            r.metric("pressure_mpa", Metric::new(nf.p_mpa, "MPa"));
            notes.push(format!("net force {force:.1} N spread over {:.0} mm2 of projected area", nf.projected_area_mm2));
            (merge_face_pressures(nf.pressures), d * force.abs(), Some(d))
        };
        bin_pressures(&g, &mut ps, params.int("bins")?.max(0) as usize);
        let res = pressure_resultant(&g, &ps);
        // the applied resultant against the requested one: along the force direction in net-force mode, per axis in field mode
        let abs_force: f64 = ps.iter().map(|p| p.p_mpa.abs() * face_geometry(&g, p.elem as usize, p.face).0).sum();
        let err = match axis_dir {
            Some(d) => 100.0 * (res.dot(d) - target.dot(d)).abs() / target.length().max(1e-300),
            None => {
                let e = (res - target).abs();
                100.0 * e.max_element() / target.length().max(1e-3 * abs_force).max(1e-300)
            }
        };
        r.metric("force_requested_n", Metric::new(target.length(), "N"));
        for (k, v) in [("x", res.x), ("y", res.y), ("z", res.z)] {
            r.metric(format!("resultant_{k}_n"), Metric::new(finite(v), "N"));
        }
        r.metric("conservation_error_pct", Metric::new(finite(err), "%").with_limit(tol, true));
        r.metric("max_pressure_mpa", Metric::new(ps.iter().map(|p| p.p_mpa.abs()).fold(0.0, f64::max), "MPa"));
        if err > tol && params.bool("strict")? {
            return Err(ctx.fail(format!("the applied resultant differs from the requested one by {err:.2} % (limit {tol} %); check the mapping distance and the pressure surface")));
        }
        for n in &notes {
            r.note(Severity::Info, n.clone());
        }
        r.note(Severity::Info, "positive pressure pushes into the body, negative pressure (suction) pulls out");
        let set = LoadSet { name: sanitize(params.str("name")?), pressures: ps, resultant_n: res.into(), notes, ..Default::default() };
        Ok(vec![to_data(&set), Data::report(r)])
    }
}

// ------------------------------------------------------------- bump load

pub struct LoadBump;

impl NodeOp for LoadBump {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.load_bump", "Bump Load")
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::optional("material", DataKind::Json))
            .output(PortSpec::new("loads", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::string("name", "Load name", "bump"))
            .param(ParamSpec::float("bump_g", "Bump acceleration", 4.0, 0.0, 10.0).unit("g").help("Engineering rule of thumb of 3 to 5 g; not a standard value"))
            .param(ParamSpec::vec3("axis", "Acceleration direction", DVec3::new(0.0, 0.0, -1.0)).help("Direction of the body-force field on the part (inertia opposes the chassis acceleration)"))
            .param(ParamSpec::bool("include_gravity", "Add gravity", true).help("Add 1 g along -Z to the bump"))
            .param(ParamSpec::float("density_g_cm3", "Density", 1.24, 0.1, 10.0).unit("g/cm3").help("Used when no material card is connected"))
            .param(ParamSpec::float("attached_mass_kg", "Attached mass", 0.0, 0.0, 50.0).unit("kg").help("A lamp, blade or bracket carried by the part"))
            .param(ParamSpec::vec3("attach_point", "Attachment point", DVec3::ZERO).unit("mm").help("The mesh node nearest to this point carries the attached mass"))
            .help("Inertial load of a bump: a body acceleration on every element plus the attached mass applied as a force at its attachment node. The acceleration field is 9810 mm/s2 per g in the deck's unit system.")
            .emulates("gravity and acceleration loads of a static study")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = solid_mesh(ctx, inputs, 0)?;
        let rho = match opt_from_data::<MaterialCard>(inputs, 1, "material card")? {
            Some(c) => c.density_t_mm3,
            None => params.float("density_g_cm3")? * 1e-9,
        };
        let a = accel_field(params.float("bump_g")?, params.vec3("axis")?, params.bool("include_gravity")?);
        let mass_t = mesh_mass_tonne(&g, rho);
        let m_att_t = params.float("attached_mass_kg")? * 1e-3;
        let mut set = LoadSet { name: sanitize(params.str("name")?), ..Default::default() };
        let mut res = a * mass_t;
        if a.length() > 0.0 {
            set.gravity = Some(Gravity { accel_mm_s2: a.length(), dir: a.normalize().into() });
        }
        if m_att_t > 0.0 {
            let node = nearest_node(&g, params.vec3("attach_point")?).ok_or_else(|| ctx.fail("the mesh has no nodes"))?;
            let f = a * m_att_t;
            set.forces.push(NodalForce { node, f_n: f.into() });
            res += f;
            set.notes.push(format!("attached mass {} kg carried by node {}", params.float("attached_mass_kg")?, node + 1));
        }
        set.resultant_n = res.into();
        let mut r = Report::new("Bump load");
        r.metric("part_mass_kg", Metric::new(mass_t * 1e3, "kg"));
        r.metric("acceleration_g", Metric::new(a.length() / crate::loads::G_MM_S2, "g"));
        r.metric("body_force_n", Metric::new(a.length() * mass_t, "N"));
        r.metric("attached_mass_kg", Metric::new(m_att_t * 1e3, "kg"));
        r.metric("resultant_n", Metric::new(res.length(), "N"));
        if set.is_empty() {
            r.note(Severity::Warning, "the bump acceleration is zero: the load set is empty");
        }
        r.note(Severity::Info, "the 3 to 5 g bump value is an engineering rule of thumb, not a standard");
        Ok(vec![to_data(&set), Data::report(r)])
    }
}

// ------------------------------------------------------------ load cases

pub struct LoadCases;

impl NodeOp for LoadCases {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.load_cases", "Load Case Set")
            .input(PortSpec::new("aero", DataKind::Json))
            .input(PortSpec::optional("bump", DataKind::Json))
            .input(PortSpec::optional("mounts", DataKind::Json))
            .output(PortSpec::new("cases", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::bool("case_aero", "Aero at design speed", true))
            .param(ParamSpec::bool("case_overspeed", "Aero at over-speed", true))
            .param(ParamSpec::float("overspeed_factor", "Over-speed ratio", 1.25, 1.0, 2.0).help("Speed relative to the design speed; the aero pressure scales with its square"))
            .param(ParamSpec::bool("case_bump", "Bump with gravity", true))
            .param(ParamSpec::bool("case_combined", "Aero and bump together", true))
            .help("Builds the named load cases from the aero and bump loads: aero at design speed, aero at a higher speed, bump with gravity, and aero plus bump. The safety factor is the minimum over the cases. No extra load factor is added; the safety-factor target already carries the margin.")
            .emulates("load-case and step managers of finite-element front ends")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let aero: LoadSet = from_data(inputs, 0, "aero load set")?;
        let bump: Option<LoadSet> = opt_from_data(inputs, 1, "bump load set")?;
        let mounts: Option<MountSet> = opt_from_data(inputs, 2, "mount set")?;
        let k = params.float("overspeed_factor")?;
        let mut cases = Vec::new();
        let mut r = Report::new("Load cases");
        if params.bool("case_aero")? {
            cases.push(LoadCase { name: "aero".into(), speed_factor: 1.0, loads: aero.clone() });
        }
        if params.bool("case_overspeed")? {
            cases.push(LoadCase { name: "aero_overspeed".into(), speed_factor: k, loads: aero.scale_surface(k * k) });
        }
        match &bump {
            Some(b) => {
                if params.bool("case_bump")? {
                    cases.push(LoadCase { name: "bump".into(), speed_factor: 1.0, loads: b.clone() });
                }
                if params.bool("case_combined")? {
                    let mut l = aero.clone();
                    l.merge(b);
                    cases.push(LoadCase { name: "aero_bump".into(), speed_factor: 1.0, loads: l });
                }
            }
            None => {
                if params.bool("case_bump")? || params.bool("case_combined")? {
                    r.note(Severity::Warning, "no bump load connected: the bump cases were skipped");
                }
            }
        }
        cases.retain(|c| !c.loads.is_empty());
        if cases.is_empty() {
            return Err(ctx.fail("no load case is left: enable a case and connect a non-empty load"));
        }
        r.metric("case_count", Metric::new(cases.len() as f64, ""));
        for c in &cases {
            let pmax = c.loads.pressures.iter().map(|p| p.p_mpa.abs()).fold(0.0, f64::max);
            r.metric(format!("{}_max_pressure_mpa", c.name), Metric::new(pmax, "MPa"));
            r.metric(format!("{}_resultant_n", c.name), Metric::new(DVec3::from(c.loads.resultant_n).length(), "N"));
        }
        if let Some(m) = &mounts {
            r.metric("mount_count", Metric::new(m.sites.len() as f64, "").with_limit(1.0, false));
        } else {
            r.note(Severity::Warning, "no mounts connected: the deck writer needs at least one constrained patch");
        }
        let names: Vec<&str> = cases.iter().map(|c| c.name.as_str()).collect();
        r.note(Severity::Info, format!("cases: {}; the safety factor is the minimum over them", names.join(", ")));
        Ok(vec![to_data(&CaseSet { cases }), Data::report(r)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loads::G_MM_S2;
    use crate::mesh::{box_tets, tet4_to_tet10};
    use crate::util::testing::{ctx, defaults};
    use vernon_core::Geometry;
    use vernon_graph::ParamValue;

    fn slab() -> Data {
        Data::geometry(tet4_to_tet10(&box_tets(DVec3::new(60.0, 40.0, 6.0), [12, 8, 2])).unwrap())
    }

    fn ls(d: &Data) -> LoadSet {
        serde_json::from_value((**d.as_json().unwrap()).clone()).unwrap()
    }

    #[test]
    fn g1_net_force_matches_q_times_cla() {
        // 200 km/h, rho 1.11, ClA 0.5 m2 -> 856.5 N along -Z
        let mut p = defaults(&LoadAero);
        p.set("cla_m2", ParamValue::Float(0.5));
        let out = LoadAero.cook(&ctx("aero"), &[Some(slab())], &p).unwrap();
        let rep = out[1].as_report().unwrap();
        assert!((rep.metrics["dynamic_pressure_pa"].value - 1712.96).abs() < 0.01);
        assert!((rep.metrics["force_requested_n"].value - 856.48).abs() < 0.01, "{}", rep.metrics["force_requested_n"].value);
        let set = ls(&out[0]);
        assert!((set.resultant_n[2] + 856.48).abs() < 0.01, "{:?}", set.resultant_n);
        assert!(rep.passes(), "{:?}", rep.metrics);
        assert!(set.pressures.iter().all(|f| f.p_mpa > 0.0));
    }

    #[test]
    fn speed_scaling_of_the_force_is_quadratic() {
        let f = |v: f64| {
            let mut p = defaults(&LoadAero);
            p.set("v_kmh", ParamValue::Float(v));
            let out = LoadAero.cook(&ctx("aero2"), &[Some(slab())], &p).unwrap();
            ls(&out[0]).resultant_n[2]
        };
        assert!((f(250.0) / f(200.0) - 1.5625).abs() < 1e-9);
    }

    #[test]
    fn field_mode_conserves_a_uniform_cp() {
        let g = tet4_to_tet10(&box_tets(DVec3::new(60.0, 40.0, 6.0), [12, 8, 2])).unwrap();
        // pressure surface: the top face of the slab with cp = -1 (suction)
        let mut surf = Geometry::make_box(DVec3::new(60.0, 40.0, 6.0));
        let n = surf.points.len();
        surf.point_attribs.set("cp", vernon_core::AttribArray::Float(vec![-1.0; n]));
        let mut p = defaults(&LoadAero);
        p.set("mode", ParamValue::Str("field".into()));
        let out = LoadAero.cook(&ctx("aero3"), &[Some(Data::geometry(g)), Some(Data::geometry(surf))], &p).unwrap();
        let set = ls(&out[0]);
        // suction on the whole closed box integrates to zero net force; every face pulls out
        assert!(set.pressures.iter().all(|f| f.p_mpa < 0.0));
        assert!(out[1].as_report().unwrap().metrics["conservation_error_pct"].value <= 1.0);
    }

    #[test]
    fn missing_surface_in_field_mode_is_an_error() {
        let mut p = defaults(&LoadAero);
        p.set("mode", ParamValue::Str("field".into()));
        assert!(LoadAero.cook(&ctx("aero4"), &[Some(slab())], &p).is_err());
    }

    #[test]
    fn placeholder_aero_table_is_refused() {
        let j = Data::json(serde_json::json!({"placeholder": true, "rows": []}));
        assert!(LoadAero.cook(&ctx("aero5"), &[Some(slab()), None, Some(j)], &defaults(&LoadAero)).is_err());
    }

    #[test]
    fn aero_table_sets_the_force() {
        let j = Data::json(serde_json::json!({"placeholder": false, "design": {"design_downforce_n": 1000.0, "drag_n": 300.0, "design_q_pa": 2000.0}}));
        let out = LoadAero.cook(&ctx("aero6"), &[Some(slab()), None, Some(j)], &defaults(&LoadAero)).unwrap();
        assert!((ls(&out[0]).resultant_n[2] + 1000.0).abs() < 1e-6);
    }

    #[test]
    fn bump_load_weighs_four_g_plus_gravity() {
        let g = tet4_to_tet10(&box_tets(DVec3::new(60.0, 40.0, 6.0), [6, 4, 1])).unwrap();
        let mut p = defaults(&LoadBump);
        p.set("attached_mass_kg", ParamValue::Float(0.5));
        p.set("attach_point", ParamValue::Vec3(DVec3::new(30.0, 20.0, 3.0)));
        let out = LoadBump.cook(&ctx("bump"), &[Some(Data::geometry(g))], &p).unwrap();
        let set = ls(&out[0]);
        let gr = set.gravity.unwrap();
        assert!((gr.accel_mm_s2 - 5.0 * G_MM_S2).abs() < 1e-6 && gr.dir[2] == -1.0);
        let m_part = 60.0 * 40.0 * 6.0 * 1.24e-9;
        let expect = -(m_part + 0.5e-3) * 5.0 * G_MM_S2;
        assert!((set.resultant_n[2] - expect).abs() < 1e-6 * expect.abs(), "{} vs {expect}", set.resultant_n[2]);
        assert_eq!(set.forces.len(), 1);
        assert!((out[1].as_report().unwrap().metrics["part_mass_kg"].value - m_part * 1e3).abs() < 1e-9);
    }

    #[test]
    fn four_cases_with_quadratic_overspeed() {
        let aero = LoadAero.cook(&ctx("lc1"), &[Some(slab())], &defaults(&LoadAero)).unwrap();
        let bump = LoadBump.cook(&ctx("lc2"), &[Some(slab())], &defaults(&LoadBump)).unwrap();
        let out = LoadCases.cook(&ctx("lc3"), &[Some(aero[0].clone()), Some(bump[0].clone()), None], &defaults(&LoadCases)).unwrap();
        let cs: CaseSet = serde_json::from_value((**out[0].as_json().unwrap()).clone()).unwrap();
        let names: Vec<&str> = cs.cases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["aero", "aero_overspeed", "bump", "aero_bump"]);
        let a = cs.cases[0].loads.pressures[0].p_mpa;
        let b = cs.cases[1].loads.pressures[0].p_mpa;
        assert!((b / a - 1.5625).abs() < 1e-9);
        // the combined case carries the pressure and the body acceleration
        assert!(cs.cases[3].loads.gravity.is_some() && !cs.cases[3].loads.pressures.is_empty());
        let rep = out[1].as_report().unwrap();
        assert_eq!(rep.metrics["case_count"].value, 4.0);
    }

    #[test]
    fn without_a_bump_load_only_aero_cases_remain() {
        let aero = LoadAero.cook(&ctx("lc4"), &[Some(slab())], &defaults(&LoadAero)).unwrap();
        let out = LoadCases.cook(&ctx("lc5"), &[Some(aero[0].clone()), None, None], &defaults(&LoadCases)).unwrap();
        assert_eq!(out[1].as_report().unwrap().metrics["case_count"].value, 2.0);
    }
}
