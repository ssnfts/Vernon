//! Result nodes: reading the solver files, safety factors, the mount reaction
//! report, the mesh-convergence study and the hand cross-check.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use glam::DVec3;
use rayon::prelude::*;
use serde_json::json;
use vernon_core::{AttribArray, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::failure::{safety_factor, Criterion, Strengths};
use crate::frd::{dense_tensor6, dense_vec3, parse_dat, parse_frd};
use crate::linalg::{rotate_stress, von_mises, V6};
use crate::material::MaterialCard;
use crate::mesh::{corners, element_centroid, element_volume, mesh_volume};
use crate::study::{gci, is_singular, joint_hand_check, JointInput, Verdict};
use crate::types::{finite, from_data, opt_from_data, to_data, CaseResult, CaseSet, MountSet, OrientationCard, ResultSet, ResultSummary, SolveManifest};
use crate::util::{fea_info, sanitize, solid_mesh};

/// Largest safety factor stored or reported; larger and infinite values are capped so reports and attributes stay finite.
const SF_HARD_CAP: f64 = 1.0e4;

fn v6_from(n: DVec3, s: DVec3) -> V6 {
    [n.x, n.y, n.z, s.x, s.y, s.z]
}

fn split6(t: &V6) -> (DVec3, DVec3) {
    (DVec3::new(t[0], t[1], t[2]), DVec3::new(t[3], t[4], t[5]))
}

fn set_point_vec3(g: &mut vernon_core::Geometry, name: String, v: Vec<DVec3>) {
    g.point_attribs.set(name, AttribArray::Vec3(v));
}

// ----------------------------------------------------------- result reader

pub struct ResultRead;

impl NodeOp for ResultRead {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.result_read", "Structural Result Reader")
            .input(PortSpec::new("results", DataKind::Json))
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::optional("orientation", DataKind::Json))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("fields", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .param(ParamSpec::choice("stress_frame", "Stress frame", "global", &["global", "material"]).help("Store the stresses in the model frame, or rotated into the printed-material frame (needs the orientation)"))
            .param(ParamSpec::bool("use_ip_stress", "Integration-point stress", true).help("Use the stress at the integration points for the averaged mount stress when the text output has it"))
            .param(ParamSpec::string("case", "Case", "").help("Read one load case; empty reads all"))
            .help("Reads the solver result files for every load case onto the solid mesh: displacement, stress and strain-free totals per mount. Point attributes disp_<case>, sig_n_<case> (xx, yy, zz), sig_s_<case> (xy, yz, zx) and vm_mpa_<case>, plus the envelope disp and vm_mpa. A missing or incomplete result is an error, never zeros.")
            .emulates("results readers of finite-element post-processors")
            .implemented()
            .external("CalculiX")
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let man: SolveManifest = from_data(inputs, 0, "solve manifest")?;
        let mesh = solid_mesh(ctx, inputs, 1)?;
        let orient = opt_from_data::<OrientationCard>(inputs, 2, "orientation card")?;
        let material_frame = params.str("stress_frame")? == "material";
        if material_frame && orient.is_none() {
            return Err(ctx.fail("the material stress frame needs the orientation card"));
        }
        let q = orient.as_ref().map(|o| o.axes);
        let want = if params.str("case")?.is_empty() { String::new() } else { sanitize(params.str("case")?).to_lowercase() };
        let n = mesh.points.len();
        let mut used: BTreeSet<usize> = BTreeSet::new();
        for p in &mesh.prims {
            used.extend(p.vertices.iter().map(|&v| v as usize));
        }
        let needed: Vec<bool> = (0..n).map(|i| used.contains(&i)).collect();
        let mut out = (*mesh).clone();
        let mut set = ResultSet { source: man.solver.clone(), frame: if material_frame { "material".into() } else { "global".into() }, cases: vec![] };
        let mut r = Report::new("Structural results");
        let mut env_disp = vec![DVec3::ZERO; n];
        let mut env_vm = vec![0.0f64; n];
        let mesh_vol = mesh_volume(&mesh);
        let mut ip_missing = false;
        for run in man.runs.iter().filter(|x| want.is_empty() || x.case == want) {
            if !matches!(run.status.as_str(), "solved" | "reused") || run.frd.is_empty() {
                return Err(ctx.fail(format!("case '{}' has no results (status {}): run the solver first", run.case, run.status)));
            }
            let frd_path = Path::new(&man.dir).join(&run.frd);
            let text = std::fs::read_to_string(&frd_path).map_err(|e| ctx.fail(format!("cannot read {}: {e}", frd_path.display())))?;
            let frd = parse_frd(&text).map_err(|e| ctx.fail(format!("case '{}': {e}", run.case)))?;
            let ds = frd.dataset("DISP").ok_or_else(|| ctx.fail(format!("case '{}': the result file has no displacement block", run.case)))?;
            let disp = dense_vec3(ds, n, &needed).map_err(|e| ctx.fail(format!("case '{}': {e}", run.case)))?;
            let ss = frd.dataset("STRESS").ok_or_else(|| ctx.fail(format!("case '{}': the result file has no stress block", run.case)))?;
            let mut sig = dense_tensor6(ss, n, &needed).map_err(|e| ctx.fail(format!("case '{}': {e}", run.case)))?;
            let dat = if run.dat.is_empty() { None } else { std::fs::read_to_string(Path::new(&man.dir).join(&run.dat)).ok().map(|t| parse_dat(&t)) };
            if let (true, Some(q)) = (material_frame, q.as_ref()) {
                sig.iter_mut().for_each(|s| *s = rotate_stress(s, q));
            }
            let vm: Vec<f64> = sig.iter().map(von_mises).collect();
            let (sn, ss_): (Vec<DVec3>, Vec<DVec3>) = sig.iter().map(split6).unzip();
            set_point_vec3(&mut out, format!("disp_{}", run.case), disp.clone());
            set_point_vec3(&mut out, format!("sig_n_{}", run.case), sn);
            set_point_vec3(&mut out, format!("sig_s_{}", run.case), ss_);
            out.point_attribs.set(format!("vm_mpa_{}", run.case), AttribArray::Float(vm.clone()));
            for i in 0..n {
                if disp[i].length() > env_disp[i].length() {
                    env_disp[i] = disp[i];
                }
                env_vm[i] = env_vm[i].max(vm[i]);
            }
            // element-mean integration-point stress
            let mut summary = ResultSummary { max_disp_mm: disp.iter().map(|d| d.length()).fold(0.0, f64::max), max_vm_mpa: vm.iter().cloned().fold(0.0, f64::max), ..Default::default() };
            if let Some(d) = &dat {
                summary.rf_totals = d.totals.clone();
                summary.volume_mm3 = d.volume;
                summary.time = d.time;
                if params.bool("use_ip_stress")? {
                    let mut acc: BTreeMap<u32, (V6, f64)> = BTreeMap::new();
                    for (e, _, s) in &d.ip_stress {
                        let a = acc.entry(*e).or_insert(([0.0; 6], 0.0));
                        for (v, x) in a.0.iter_mut().zip(s.iter()) {
                            *v += x;
                        }
                        a.1 += 1.0;
                    }
                    if acc.len() == mesh.prims.len() {
                        let mut ip_n = Vec::with_capacity(acc.len());
                        let mut ip_s = Vec::with_capacity(acc.len());
                        for (_, (mut s, w)) in acc {
                            s.iter_mut().for_each(|v| *v /= w);
                            if let (true, Some(q)) = (material_frame, q.as_ref()) {
                                s = rotate_stress(&s, q);
                            }
                            let (a, b) = split6(&s);
                            ip_n.push(a);
                            ip_s.push(b);
                        }
                        out.prim_attribs.set(format!("ip_n_{}", run.case), AttribArray::Vec3(ip_n));
                        out.prim_attribs.set(format!("ip_s_{}", run.case), AttribArray::Vec3(ip_s));
                    } else {
                        ip_missing = true;
                    }
                }
                if let Some(v) = d.volume {
                    r.metric(format!("{}_volume_error_pct", run.case), Metric::new(100.0 * (v - mesh_vol).abs() / mesh_vol.max(1e-300), "%").with_limit(0.1, true));
                }
            } else if params.bool("use_ip_stress")? {
                ip_missing = true;
            }
            r.metric(format!("{}_max_disp_mm", run.case), Metric::new(finite(summary.max_disp_mm), "mm"));
            r.metric(format!("{}_max_vm_mpa", run.case), Metric::new(finite(summary.max_vm_mpa), "MPa"));
            set.cases.push(CaseResult { name: run.case.clone(), speed_factor: run.speed_factor, summary });
        }
        if set.cases.is_empty() {
            return Err(ctx.fail(if want.is_empty() { "the solve manifest has no cases".to_string() } else { format!("no case named '{want}'") }));
        }
        set_point_vec3(&mut out, "disp".into(), env_disp);
        out.point_attribs.set("vm_mpa", AttribArray::Float(env_vm));
        let max_disp = set.cases.iter().map(|c| c.summary.max_disp_mm).fold(0.0, f64::max);
        let max_vm = set.cases.iter().map(|c| c.summary.max_vm_mpa).fold(0.0, f64::max);
        r.metric("cases_read", Metric::new(set.cases.len() as f64, ""));
        r.metric("max_disp_mm", Metric::new(finite(max_disp), "mm"));
        r.metric("max_vm_mpa", Metric::new(finite(max_vm), "MPa"));
        if ip_missing {
            r.note(Severity::Warning, "integration-point stresses are not available for every case: the mount stress is averaged from nodal values");
        }
        if man.solver == "reference_linear" {
            r.note(Severity::Warning, "white-box placeholder: the results come from the reference solver with four-node tetrahedra");
        }
        r.note(Severity::Info, "peak stresses at constrained nodes are singular: judge a mount by the averaged value");
        Ok(vec![Data::geometry(out), to_data(&set), Data::report(r)])
    }
}

// ------------------------------------------------------------ safety factor

pub struct SafetyFactor;

/// Elements whose centroid lies within `radius` of `center`, or the nearest one.
fn sphere_elements(mesh: &vernon_core::Geometry, centroids: &[DVec3], center: DVec3, radius: f64) -> (Vec<usize>, bool) {
    let inside: Vec<usize> = (0..centroids.len()).filter(|&e| centroids[e].distance(center) <= radius).collect();
    if !inside.is_empty() {
        return (inside, false);
    }
    let nearest = (0..mesh.prims.len()).min_by(|&a, &b| centroids[a].distance_squared(center).total_cmp(&centroids[b].distance_squared(center)));
    (nearest.into_iter().collect(), true)
}

struct Gate {
    cases: Vec<String>,
    /// Node safety factor per criterion (minimum over the cases).
    sf: [Vec<f64>; 4],
}

impl NodeOp for SafetyFactor {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.safety_factor", "Safety Factor Map")
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::new("fields", DataKind::Json))
            .input(PortSpec::new("material", DataKind::Json))
            .input(PortSpec::optional("orientation", DataKind::Json))
            .input(PortSpec::optional("mounts", DataKind::Json))
            .output(PortSpec::new("mesh", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("table", DataKind::Json))
            .param(ParamSpec::choice("criterion", "Criterion", "tsai_wu", &["von_mises", "max_principal", "tsai_wu", "interlayer"]).help("Failure criterion that is gated; all four are written to the mesh"))
            .param(ParamSpec::float("sf_target", "Safety-factor target", 3.0, 1.0, 10.0).help("Engineering judgement of 3 to 4 for printed exterior aero parts; no standard exists"))
            .param(ParamSpec::choice("allowable", "Allowable", "ultimate", &["ultimate", "yield"]).help("Strength that the stress is compared with"))
            .param(ParamSpec::float("yield_fraction", "Yield over ultimate", 0.8, 0.1, 1.0).help("Used with the yield allowable; placeholder until coupon data replaces it"))
            .param(ParamSpec::float("avg_radius_mm", "Averaging radius", 0.0, 0.0, 30.0).unit("mm").help("Radius of the sphere around a mount over which stress is averaged; 0 takes the value of the mount"))
            .param(ParamSpec::float("sf_cap", "Safety-factor cap", 100.0, 1.0, 10000.0).help("Larger values (and no-load points) are stored as this value so the field stays finite"))
            .param(ParamSpec::float("temperature_c", "Uniform temperature", 23.0, -40.0, 300.0).unit("degC").help("Used when the mesh has no temp_c attribute"))
            .param(ParamSpec::float("design_speed_kmh", "Design speed", 200.0, 30.0, 400.0).unit("km/h").help("Speed of the aero case; the speed limit scales with the square root of the safety factor"))
            .help("Safety factor per point from the stress, the direction-dependent strengths and the temperature retention, for four criteria. The gate is the lower of the free-field minimum and the minimum over mounts of the sphere-averaged stress, because the raw peak at a constrained patch is a singularity; the raw peak is reported beside it.")
            .emulates("safety-factor results of static stress studies and strength-ratio failure criteria for composites")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = solid_mesh(ctx, inputs, 0)?;
        let rs: ResultSet = from_data(inputs, 1, "result set")?;
        let card: MaterialCard = from_data(inputs, 2, "material card")?;
        let orient = opt_from_data::<OrientationCard>(inputs, 3, "orientation card")?.unwrap_or_else(OrientationCard::identity);
        let mounts: MountSet = opt_from_data(inputs, 4, "mount set")?.unwrap_or_default();
        let crit = Criterion::parse(params.str("criterion")?).ok_or_else(|| ctx.fail("unknown criterion"))?;
        let ci = Criterion::ALL.iter().position(|c| *c == crit).unwrap_or(0);
        let (target, cap) = (params.float("sf_target")?, params.float("sf_cap")?.min(SF_HARD_CAP));
        let frac = if params.str("allowable")? == "yield" { params.float("yield_fraction")? } else { 1.0 };
        if rs.cases.is_empty() {
            return Err(ctx.fail("the result set has no cases"));
        }
        let n = mesh.points.len();
        let q = if rs.frame == "material" || card.mode == "isotropic_z" { None } else { Some(orient.axes) };
        let temps: Vec<f64> = match mesh.point_attribs.floats("temp_c") {
            Ok(t) => t.to_vec(),
            Err(_) => vec![params.float("temperature_c")?; n],
        };
        let r_node: Vec<f64> = temps.iter().map(|&t| card.retention_at(t)).collect();
        let str_node: Vec<Strengths> = r_node.iter().map(|&r| Strengths::from_card(&card, r, frac)).collect();
        let names: Vec<String> = rs.cases.iter().map(|c| c.name.clone()).collect();
        // stress per case and node
        let mut stress: Vec<Vec<V6>> = Vec::new();
        for c in &names {
            let nn = mesh.point_attribs.vec3s(&format!("sig_n_{c}")).map_err(|_| ctx.fail(format!("the mesh has no attribute sig_n_{c}: read the results first")))?;
            let ns = mesh.point_attribs.vec3s(&format!("sig_s_{c}")).map_err(|_| ctx.fail(format!("the mesh has no attribute sig_s_{c}: read the results first")))?;
            stress.push((0..n).map(|i| v6_from(nn[i], ns[i])).collect());
        }
        let to_mat = |s: &V6| q.as_ref().map_or(*s, |q| rotate_stress(s, q));
        let capv = |v: f64| if v.is_finite() { v.min(cap) } else { cap };
        // node safety factors: minimum over cases, per criterion and per case for the chosen one
        let per_case_sel: Vec<Vec<f64>> = stress
            .par_iter()
            .map(|sc| (0..n).into_par_iter().map(|i| capv(safety_factor(crit, &to_mat(&sc[i]), &str_node[i]))).collect())
            .collect();
        let sf_all: [Vec<f64>; 4] = std::array::from_fn(|k| {
            let c = Criterion::ALL[k];
            (0..n)
                .into_par_iter()
                .map(|i| stress.iter().map(|sc| capv(safety_factor(c, &to_mat(&sc[i]), &str_node[i]))).fold(f64::INFINITY, f64::min))
                .collect()
        });
        let gate = Gate { cases: names.clone(), sf: sf_all };
        let sel = &gate.sf[ci];

        // mounts: raw peak and sphere-averaged stress
        let centroids: Vec<DVec3> = (0..mesh.prims.len()).map(|e| element_centroid(&mesh, e)).collect();
        let vols: Vec<f64> = (0..mesh.prims.len()).map(|e| element_volume(&mesh, e).abs()).collect();
        let mut in_sphere = vec![false; n];
        let mut mount_rows = Vec::new();
        let mut r = Report::new("Safety factor");
        let (mut sf_mount_min, mut sf_peak_mount_min, mut vm_avg_max) = (f64::INFINITY, f64::INFINITY, 0.0f64);
        let mut mount_case_min = vec![f64::INFINITY; gate.cases.len()];
        let mut mount_crit_min = [f64::INFINITY; 4];
        for site in &mounts.sites {
            let radius = match params.float("avg_radius_mm")? {
                v if v > 0.0 => v,
                _ => site.avg_radius_mm.max(site.bolt_d_mm).max(1e-3),
            };
            let center = DVec3::from(site.center);
            let mut peak = f64::INFINITY;
            let mut any = false;
            for i in 0..n {
                if mesh.points[i].distance(center) <= radius {
                    in_sphere[i] = true;
                    peak = peak.min(sel[i]);
                    any = true;
                }
            }
            if !any {
                let k = (0..n).min_by(|&a, &b| mesh.points[a].distance_squared(center).total_cmp(&mesh.points[b].distance_squared(center))).unwrap_or(0);
                peak = sel[k];
                in_sphere[k] = true;
            }
            let (els, fallback) = sphere_elements(&mesh, &centroids, center, radius);
            if fallback {
                r.note(Severity::Warning, format!("{}: no element centroid within {radius} mm; the nearest element is used", site.name));
            }
            let wsum: f64 = els.iter().map(|&e| vols[e]).sum::<f64>().max(1e-300);
            let mut r_min = 1.0f64;
            for &e in &els {
                for c in corners(&mesh, e) {
                    r_min = r_min.min(r_node[c as usize]);
                }
            }
            let st = Strengths::from_card(&card, r_min, frac);
            let mut avg_sf_sel = f64::INFINITY;
            let mut gov = String::new();
            let mut crit_sf = [f64::INFINITY; 4];
            let mut vm_avg = 0.0f64;
            for (k, cname) in gate.cases.iter().enumerate() {
                let ipn = mesh.prim_attribs.vec3s(&format!("ip_n_{cname}")).ok();
                let ips = mesh.prim_attribs.vec3s(&format!("ip_s_{cname}")).ok();
                let mut s = [0.0; 6];
                for &e in &els {
                    let t = match (ipn, ips) {
                        (Some(a), Some(b)) => v6_from(a[e], b[e]),
                        _ => {
                            let cs = corners(&mesh, e);
                            let mut m = [0.0; 6];
                            for c in cs {
                                for (i, v) in m.iter_mut().enumerate() {
                                    *v += stress[k][c as usize][i] / 4.0;
                                }
                            }
                            m
                        }
                    };
                    for i in 0..6 {
                        s[i] += t[i] * vols[e] / wsum;
                    }
                }
                let sm = to_mat(&s);
                vm_avg = vm_avg.max(von_mises(&s));
                for (j, c) in Criterion::ALL.iter().enumerate() {
                    let v = capv(safety_factor(*c, &sm, &st));
                    crit_sf[j] = crit_sf[j].min(v);
                    if j == ci {
                        mount_case_min[k] = mount_case_min[k].min(v);
                        if v < avg_sf_sel {
                            avg_sf_sel = v;
                            gov = cname.clone();
                        }
                    }
                }
            }
            for j in 0..4 {
                mount_crit_min[j] = mount_crit_min[j].min(crit_sf[j]);
            }
            sf_mount_min = sf_mount_min.min(avg_sf_sel);
            sf_peak_mount_min = sf_peak_mount_min.min(peak);
            vm_avg_max = vm_avg_max.max(vm_avg);
            r.metric(format!("sf_{}", site.name), Metric::new(avg_sf_sel, "").with_limit(target, false));
            r.metric(format!("sf_peak_{}", site.name), Metric::new(peak, ""));
            mount_rows.push(json!({
                "name": site.name, "radius_mm": radius, "elements": els.len(), "sf_avg": avg_sf_sel, "sf_peak": peak,
                "vm_avg_mpa": vm_avg, "governing_case": gov, "sf_by_criterion": {
                    "von_mises": crit_sf[0], "max_principal": crit_sf[1], "tsai_wu": crit_sf[2], "interlayer": crit_sf[3] }
            }));
        }
        // free field: nodes outside every mount sphere
        let field_min = |v: &[f64]| (0..n).filter(|&i| !in_sphere[i]).map(|i| v[i]).fold(f64::INFINITY, f64::min);
        let field = field_min(sel);
        let gate_value = field.min(sf_mount_min);
        let peak_min = sel.iter().cloned().fold(f64::INFINITY, f64::min);
        let (worst_node, _) = sel.iter().enumerate().fold((0usize, f64::INFINITY), |a, (i, &v)| if v < a.1 { (i, v) } else { a });
        let capf = |v: f64| if v.is_finite() { v } else { cap };
        r.metric("sf_min", Metric::new(capf(gate_value), "").with_limit(target, false));
        r.metric("sf_field_min", Metric::new(capf(field), ""));
        r.metric("sf_peak_min", Metric::new(capf(peak_min), ""));
        if !mounts.sites.is_empty() {
            r.metric("sf_mount_min", Metric::new(capf(sf_mount_min), "").with_limit(target, false));
            r.metric("sf_peak_mount_min", Metric::new(capf(sf_peak_mount_min), ""));
            r.metric("mount_vm_avg_mpa", Metric::new(vm_avg_max, "MPa"));
        }
        let mut by_crit = serde_json::Map::new();
        for (j, c) in Criterion::ALL.iter().enumerate() {
            let g = field_min(&gate.sf[j]).min(mount_crit_min[j]);
            r.metric(format!("sf_min_{}", c.name()), Metric::new(capf(g), ""));
            by_crit.insert(c.name().into(), json!(capf(g)));
        }
        r.metric("max_disp_mm", Metric::new(finite(rs.cases.iter().map(|c| c.summary.max_disp_mm).fold(0.0, f64::max)), "mm"));
        r.metric("max_vm_mpa", Metric::new(finite(rs.cases.iter().map(|c| c.summary.max_vm_mpa).fold(0.0, f64::max)), "MPa"));
        // per case gate value and the speed at which an aero-only case reaches the target
        let mut case_rows = Vec::new();
        let mut governing = (String::new(), f64::INFINITY);
        let mut speed_limit = f64::INFINITY;
        let v0 = params.float("design_speed_kmh")?;
        for (k, c) in rs.cases.iter().enumerate() {
            let f = (0..n).filter(|&i| !in_sphere[i]).map(|i| per_case_sel[k][i]).fold(f64::INFINITY, f64::min);
            let v = f.min(mount_case_min[k]);
            if v < governing.1 {
                governing = (c.name.clone(), v);
            }
            if c.name.starts_with("aero") && !c.name.contains("bump") && v.is_finite() {
                speed_limit = speed_limit.min(v0 * c.speed_factor * (v / target).sqrt());
            }
            case_rows.push(json!({"name": c.name, "sf_min": capf(v), "speed_factor": c.speed_factor}));
        }
        if speed_limit.is_finite() {
            r.metric("speed_limit_kmh", Metric::new(speed_limit, "km/h"));
        }
        let wp = mesh.points[worst_node];
        r.note(Severity::Info, format!("governing case: {}; lowest raw safety factor at ({:.1}, {:.1}, {:.1}) mm", governing.0, wp.x, wp.y, wp.z));
        r.note(Severity::Info, format!("criterion {}; target {target}; the von Mises value overstates the safety of printed parts because it ignores the weak stacking direction", crit.name()));
        if rs.source == "reference_linear" {
            r.metric("estimate_only", Metric::new(1.0, "flag").with_limit(0.0, true));
            r.note(Severity::Warning, "white-box placeholder: the stresses come from the reference solver with four-node tetrahedra, so this safety factor cannot pass a gate");
        }
        if card.retention.as_ref().is_some_and(|t| t.uncalibrated) {
            r.note(Severity::Warning, "the temperature retention curve is uncalibrated");
        }
        let mut out = (*mesh).clone();
        let cap_vec = |v: &Vec<f64>| AttribArray::Float(v.iter().map(|&x| capv(x)).collect());
        out.point_attribs.set("sf", cap_vec(sel));
        for (j, c) in Criterion::ALL.iter().enumerate() {
            out.point_attribs.set(format!("sf_{}", c.name()), cap_vec(&gate.sf[j]));
        }
        let table = json!({
            "criterion": crit.name(), "sf_target": target, "sf_min": capf(gate_value), "governing_case": governing.0,
            "by_criterion": by_crit, "mounts": mount_rows, "cases": case_rows, "source": rs.source,
        });
        Ok(vec![Data::geometry(out), Data::report(r), Data::json(table)])
    }
}

// ----------------------------------------------------------- mount report

pub struct MountReport;

impl NodeOp for MountReport {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.mount_report", "Mount Reaction Report")
            .input(PortSpec::new("fields", DataKind::Json))
            .input(PortSpec::new("cases", DataKind::Json))
            .input(PortSpec::optional("mounts", DataKind::Json))
            .input(PortSpec::optional("table", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("table", DataKind::Json))
            .param(ParamSpec::float("tol", "Equilibrium tolerance", 1.0e-3, 1.0e-6, 0.1).help("Allowed ratio of the force imbalance to the applied load, per axis"))
            .param(ParamSpec::float("sf_target", "Safety-factor target", 3.0, 1.0, 10.0).help("Limit applied to the safety factor at each mount"))
            .help("Reaction force at every mount for every load case, the equilibrium check (reactions plus applied loads must vanish) and, when the safety-factor table is connected, the safety factor at each mount against the target.")
            .emulates("reaction-force output of static stress studies")
            .implemented()
    }

    fn cook(&self, _ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let rs: ResultSet = from_data(inputs, 0, "result set")?;
        let cases: CaseSet = from_data(inputs, 1, "case set")?;
        let mounts: Option<MountSet> = opt_from_data(inputs, 2, "mount set")?;
        let table = inputs.get(3).and_then(|d| d.as_ref()).and_then(|d| d.as_json()).map(|v| (**v).clone());
        let tol = params.float("tol")?;
        let target = params.float("sf_target")?;
        let mount_name = |key: &str| -> String {
            let k = key.trim_start_matches("MOUNT").parse::<usize>().unwrap_or(0);
            mounts.as_ref().and_then(|m| m.sites.get(k.wrapping_sub(1))).map_or_else(|| key.to_lowercase(), |s| s.name.clone())
        };
        let mut r = Report::new("Mount reactions");
        let mut rows = Vec::new();
        let mut worst = 0.0f64;
        let mut checked = 0usize;
        for c in &rs.cases {
            let Some(lc) = cases.cases.iter().find(|x| sanitize(&x.name).to_lowercase() == c.name) else {
                r.note(Severity::Warning, format!("case '{}' is not in the case set; its equilibrium is not checked", c.name));
                continue;
            };
            let totals = &c.summary.rf_totals;
            let mut sum = DVec3::ZERO;
            let mut per_mount = serde_json::Map::new();
            for (k, f) in totals.iter().filter(|(k, _)| k.starts_with("MOUNT")) {
                let f = DVec3::from(*f);
                sum += f;
                r.metric(format!("reaction_{}_{}_n", c.name, mount_name(k)), Metric::new(f.length(), "N"));
                per_mount.insert(mount_name(k), json!([f.x, f.y, f.z]));
            }
            let expected = mounts.as_ref().map_or(per_mount.len(), |m| m.sites.len());
            let applied = DVec3::from(lc.loads.resultant_n);
            let mut err = f64::NAN;
            if per_mount.is_empty() || per_mount.len() < expected {
                r.note(Severity::Warning, format!("case '{}': reaction totals are missing for {} mounts, so the equilibrium is not checked", c.name, expected - per_mount.len()));
            } else if applied.length() > 1e-12 {
                let e = (sum + applied).abs();
                err = e.max_element() / applied.length();
                worst = worst.max(err);
                checked += 1;
                r.metric(format!("equilibrium_error_{}", c.name), Metric::new(err, "").with_limit(tol, true));
            } else {
                r.note(Severity::Info, format!("case '{}' has no applied load", c.name));
            }
            rows.push(json!({
                "case": c.name, "applied_n": [applied.x, applied.y, applied.z], "reaction_sum_n": [sum.x, sum.y, sum.z],
                "reactions_n": per_mount, "equilibrium_error": if err.is_finite() { json!(err) } else { json!(null) },
            }));
        }
        if checked == 0 {
            r.metric("equilibrium_checked", Metric::new(0.0, "cases").with_limit(1.0, false));
        } else {
            r.metric("worst_equilibrium_error", Metric::new(worst, "").with_limit(tol, true));
        }
        let mut sf_rows = Vec::new();
        if let Some(t) = &table {
            if let Some(ms) = t.get("mounts").and_then(|m| m.as_array()) {
                for m in ms {
                    let name = m.get("name").and_then(|x| x.as_str()).unwrap_or("mount");
                    if let Some(sf) = m.get("sf_avg").and_then(|x| x.as_f64()) {
                        r.metric(format!("sf_{name}"), Metric::new(sf, "").with_limit(target, false));
                        sf_rows.push(json!({"name": name, "sf_avg": sf, "sf_peak": m.get("sf_peak")}));
                    }
                }
            }
        }
        if rs.source == "reference_linear" {
            r.note(Severity::Warning, "white-box placeholder: the reactions come from the reference solver");
        }
        Ok(vec![Data::report(r), Data::json(json!({"cases": rows, "mount_sf": sf_rows, "tolerance": tol}))])
    }
}

// ------------------------------------------------------------- mesh study

pub struct MeshStudy;

fn verdict_code(v: Verdict) -> f64 {
    match v {
        Verdict::Pass => 0.0,
        Verdict::TooCoarse => 1.0,
        Verdict::NonMonotone => 2.0,
        Verdict::Diverging => 3.0,
    }
}

impl NodeOp for MeshStudy {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.mesh_study", "Structural Mesh Convergence")
            .input(PortSpec::new("fine", DataKind::Report))
            .input(PortSpec::new("medium", DataKind::Report))
            .input(PortSpec::new("coarse", DataKind::Report))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("table", DataKind::Json))
            .param(ParamSpec::float("r", "Refinement ratio", 2.0, 1.1, 4.0).help("Edge-length ratio between successive meshes; 1.5 or more"))
            .param(ParamSpec::float("gci_max_pct", "Index limit", 5.0, 0.1, 50.0).unit("%").help("Largest accepted grid convergence index of the fine mesh"))
            .param(ParamSpec::string("metrics", "Metrics", "max_disp_mm,mount_vm_avg_mpa").help("Comma-separated report metrics to compare across the three meshes; use averaged mount values, never the raw peak"))
            .param(ParamSpec::string("peak_metric", "Peak metric", "sf_peak_mount_min").help("Report metric holding the raw peak safety factor at the mounts (singularity check); empty skips it"))
            .param(ParamSpec::string("avg_metric", "Averaged metric", "sf_mount_min").help("Report metric holding the averaged safety factor at the mounts"))
            .help("Grid convergence index of chosen result metrics from three meshes (fine, medium, coarse) with a constant refinement ratio: observed order, extrapolated value, index and a monotone-convergence verdict. A mount is flagged singular when its raw peak moves by more than 20 percent while the averaged value moves by less than 5 percent.")
            .emulates("grid convergence index of Roache for mesh refinement studies")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let reps = [inputs::report(inputs, 0)?, inputs::report(inputs, 1)?, inputs::report(inputs, 2)?];
        let (rr, lim) = (params.float("r")?, params.float("gci_max_pct")?);
        let get = |name: &str| -> CookResult<[f64; 3]> {
            let mut v = [0.0; 3];
            for (k, rep) in reps.iter().enumerate() {
                v[k] = rep.metrics.get(name).map(|m| m.value).ok_or_else(|| ctx.fail(format!("metric '{name}' is missing from report {}", ["fine", "medium", "coarse"][k])))?;
            }
            Ok(v)
        };
        let mut r = Report::new("Mesh convergence");
        let mut rows = Vec::new();
        let mut all_pass = true;
        for m in params.str("metrics")?.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let [f1, f2, f3] = get(m)?;
            let g = gci(f1, f2, f3, rr, lim);
            all_pass &= g.verdict == Verdict::Pass;
            if let Some(p) = g.p {
                r.metric(format!("{m}_order"), Metric::new(finite(p), ""));
            }
            r.metric(format!("{m}_extrapolated"), Metric::new(finite(g.f_ext), ""));
            r.metric(format!("{m}_gci_pct"), Metric::new(finite(g.gci_fine_pct), "%").with_limit(lim, true));
            r.metric(format!("{m}_converged"), Metric::new(if g.verdict == Verdict::Pass { 1.0 } else { 0.0 }, "flag").with_limit(1.0, false));
            r.metric(format!("{m}_verdict"), Metric::new(verdict_code(g.verdict), "code"));
            if g.verdict != Verdict::Pass {
                r.note(Severity::Warning, format!("{m}: {}", g.verdict.label()));
            }
            rows.push(json!({"metric": m, "values": [f1, f2, f3], "order": g.p, "extrapolated": g.f_ext, "gci_pct": g.gci_fine_pct, "verdict": g.verdict.label()}));
        }
        if rows.is_empty() {
            return Err(ctx.fail("no metric chosen"));
        }
        let (pm, am) = (params.str("peak_metric")?.trim(), params.str("avg_metric")?.trim());
        if !pm.is_empty() && !am.is_empty() {
            if let (Ok(p), Ok(a)) = (get(pm), get(am)) {
                // the medium and fine meshes are the last two levels of refinement
                let singular = is_singular(p[1], p[0], a[1], a[0]);
                r.metric("singular", Metric::new(if singular { 1.0 } else { 0.0 }, "flag"));
                if singular {
                    r.note(Severity::Warning, "the raw peak at a mount keeps changing with the mesh while the averaged value is steady: a stress singularity, judge the mount by the averaged value");
                }
            }
        }
        r.metric("all_converged", Metric::new(if all_pass { 1.0 } else { 0.0 }, "flag").with_limit(1.0, false));
        Ok(vec![Data::report(r), Data::json(json!({"refinement_ratio": rr, "gci_limit_pct": lim, "metrics": rows}))])
    }
}

// -------------------------------------------------------------- hand check

pub struct HandCheck;

impl NodeOp for HandCheck {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.hand_check", "Bearing and Shear-out Check")
            .input(PortSpec::new("material", DataKind::Json))
            .input(PortSpec::optional("mounts", DataKind::Json))
            .input(PortSpec::optional("fe_report", DataKind::Report))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("table", DataKind::Json))
            .param(ParamSpec::float("load_n", "Load per bolt", 0.0, 0.0, 100000.0).unit("N").help("0 computes q * ClA divided by the number of mounts"))
            .param(ParamSpec::float("cla_m2", "Force area", 0.5, 0.0, 10.0).unit("m2").help("Force area, the lift coefficient times reference area, of the kit being checked."))
            .param(ParamSpec::float("v_kmh", "Speed", 200.0, 30.0, 400.0).unit("km/h").help("Speed the hand check is evaluated at. The load scales with the square of speed."))
            .param(ParamSpec::float("rho", "Air density", 1.11, 0.9, 1.3).unit("kg/m3").help("Air density used for the dynamic pressure."))
            .param(ParamSpec::int("n_mounts", "Number of mounts", 0, 0, 64).help("0 uses the connected mount set, or 2"))
            .param(ParamSpec::float("axial_fraction", "Axial fraction", 0.0, 0.0, 1.0).help("Share of the load along the bolt axis (pull-through); the rest is in the plate plane"))
            .param(ParamSpec::float("bolt_d_mm", "Bolt diameter", 0.0, 0.0, 30.0).unit("mm").help("0 takes the mount value, or 5"))
            .param(ParamSpec::float("plate_t_mm", "Plate thickness", 4.0, 0.5, 50.0).unit("mm").help("Thickness of the plate under the mount, in millimetres."))
            .param(ParamSpec::float("edge_mm", "Edge distance", 12.0, 1.0, 100.0).unit("mm").help("From the hole centre to the free edge"))
            .param(ParamSpec::float("head_d_mm", "Head diameter", 10.0, 2.0, 60.0).unit("mm").help("Diameter of the bolt head or washer that loads the plate, in millimetres."))
            .param(ParamSpec::bool("load_through_stack", "In-plane load through the stack", false).help("The in-plane load pushes through the layer-stacking direction (bolt axis lies in the print plane)"))
            .param(ParamSpec::bool("plate_flat", "Layers parallel to the plate", true).help("On when the print layers lie parallel to the plate. It picks which strength allowable the pull-through check uses."))
            .param(ParamSpec::float("sf_target", "Safety-factor target", 3.0, 1.0, 10.0).help("Safety factor the hand check must reach. The check passes when the computed factor is at least this."))
            .param(ParamSpec::float("temperature_c", "Temperature", 23.0, -40.0, 300.0).unit("degC").help("Strengths are reduced by the retention at this temperature"))
            .help("Hand calculation of a bolted joint in a printed plate: bearing stress F / (d t), pull-through shear F / (pi D t) and shear-out F / (2 (e - d/2) t), each against the directional strength. Compared with the finite-element safety factor at the mounts, which should agree within a factor of 2.")
            .emulates("standard bolted-joint hand calculations (bearing, pull-through, shear-out)")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let card: MaterialCard = from_data(inputs, 0, "material card")?;
        let mounts: Option<MountSet> = opt_from_data(inputs, 1, "mount set")?;
        let n_mounts = match params.int("n_mounts")? {
            0 => mounts.as_ref().map_or(2, |m| m.sites.len().max(1)),
            k => k as usize,
        };
        let q = crate::loads::dynamic_pressure_pa(params.float("rho")?, params.float("v_kmh")?);
        let load = match params.float("load_n")? {
            v if v > 0.0 => v,
            _ => q * params.float("cla_m2")? / n_mounts as f64,
        };
        let bolt = match params.float("bolt_d_mm")? {
            v if v > 0.0 => v,
            _ => mounts.as_ref().and_then(|m| m.sites.first()).map_or(5.0, |s| s.bolt_d_mm),
        };
        let k = card.retention_at(params.float("temperature_c")?);
        let j = JointInput {
            load_n: load,
            axial_fraction: params.float("axial_fraction")?,
            bolt_d_mm: bolt,
            plate_t_mm: params.float("plate_t_mm")?,
            edge_mm: params.float("edge_mm")?,
            head_d_mm: params.float("head_d_mm")?,
            xt: card.xt * k,
            zt: card.zt * k,
            s12: card.s12 * k,
            s13: card.s13 * k,
            load_through_stack: params.bool("load_through_stack")?,
            plate_flat: params.bool("plate_flat")?,
        };
        let res = joint_hand_check(&j).map_err(|e| ctx.fail(e))?;
        let target = params.float("sf_target")?;
        let cp = |v: f64| finite(v.min(SF_HARD_CAP));
        let mut r = Report::new("Joint hand check");
        r.metric("load_per_bolt_n", Metric::new(load, "N"));
        r.metric("bearing_mpa", Metric::new(res.bearing_mpa, "MPa"));
        r.metric("bearing_sf", Metric::new(cp(res.bearing_sf), "").with_limit(target, false));
        r.metric("pullthrough_mpa", Metric::new(res.pullthrough_mpa, "MPa"));
        r.metric("pullthrough_sf", Metric::new(cp(res.pullthrough_sf), "").with_limit(target, false));
        r.metric("shearout_mpa", Metric::new(res.shearout_mpa, "MPa"));
        r.metric("shearout_sf", Metric::new(cp(res.shearout_sf), "").with_limit(target, false));
        r.metric("hand_sf_min", Metric::new(cp(res.min_sf), "").with_limit(target, false));
        let mut ratio = None;
        if let Some(fe) = inputs::opt_report(inputs, 2) {
            if let Some(m) = fe.metrics.get("sf_mount_min").or_else(|| fe.metrics.get("sf_min")) {
                let ratio_v = res.min_sf.min(SF_HARD_CAP) / m.value.max(1e-12);
                let worst = ratio_v.max(1.0 / ratio_v.max(1e-12));
                r.metric("hand_over_fe", Metric::new(finite(ratio_v), ""));
                if worst > 2.0 {
                    r.note(Severity::Warning, format!("the hand and finite-element safety factors differ by a factor of {worst:.1}: check the mount model, the load path and the mesh"));
                }
                ratio = Some(ratio_v);
            }
        }
        r.note(Severity::Info, "load through the stacking direction is checked against the weaker stacking-direction strength; orient aero loads in the print plane");
        let table = json!({
            "load_n": load, "bolt_d_mm": bolt, "retention": k, "hand_over_fe": ratio,
            "bearing": {"mpa": res.bearing_mpa, "sf": cp(res.bearing_sf)},
            "pullthrough": {"mpa": res.pullthrough_mpa, "sf": cp(res.pullthrough_sf)},
            "shearout": {"mpa": res.shearout_mpa, "sf": cp(res.shearout_sf)},
        });
        Ok(vec![Data::report(r), Data::json(table)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{box_tets, tet4_to_tet10};
    use crate::nodes_load::{LoadAero, LoadBump, LoadCases};
    use crate::nodes_material::{build_card, MaterialFdm};
    use crate::nodes_mesh::MountSites;
    use crate::nodes_solve::{DeckWrite, SolveLinear};
    use crate::util::testing::{ctx, defaults};
    use vernon_graph::ParamValue;

    fn card_data(preset: &str) -> (Data, MaterialCard) {
        let mut p = defaults(&MaterialFdm);
        p.set("preset", ParamValue::Str(preset.into()));
        let c = build_card(&p, &Default::default()).unwrap().0;
        (to_data(&c), c)
    }

    /// Reference-solver run of the plate chain; returns (result mesh, result set, mount set, cases, card).
    fn run(tag: &str) -> (Data, Data, Data, Data, Data) {
        let c = ctx(tag);
        let mesh = tet4_to_tet10(&box_tets(DVec3::new(60.0, 40.0, 6.0), [12, 8, 2])).unwrap();
        let mut mp = defaults(&MountSites);
        mp.set("site_a", ParamValue::Vec3(DVec3::new(-25.0, 0.0, 3.0)));
        mp.set("model", ParamValue::Str("fixed_patch".into()));
        let ms = MountSites.cook(&c, &[Some(Data::geometry(mesh.clone()))], &mp).unwrap();
        let aero = LoadAero.cook(&c, &[Some(Data::geometry(mesh.clone()))], &defaults(&LoadAero)).unwrap();
        let (card, _) = card_data("ppa_cf");
        let bump = LoadBump.cook(&c, &[Some(Data::geometry(mesh.clone())), Some(card.clone())], &defaults(&LoadBump)).unwrap();
        let cases = LoadCases.cook(&c, &[Some(aero[0].clone()), Some(bump[0].clone()), Some(ms[0].clone())], &defaults(&LoadCases)).unwrap();
        let ins = vec![Some(Data::geometry(mesh.clone())), Some(card.clone()), Some(ms[0].clone()), Some(cases[0].clone())];
        let sol = SolveLinear.cook(&c, &ins, &defaults(&SolveLinear)).unwrap();
        let rd = ResultRead.cook(&c, &[Some(sol[0].clone()), Some(Data::geometry(mesh))], &defaults(&ResultRead)).unwrap();
        (rd[0].clone(), rd[1].clone(), ms[0].clone(), cases[0].clone(), card)
    }

    #[test]
    fn result_reader_fills_attributes_for_every_case() {
        let (mesh, fields, _, _, _) = run("rr1");
        let g = mesh.as_geometry().unwrap();
        for case in ["aero", "aero_overspeed", "bump", "aero_bump"] {
            for p in ["disp", "sig_n", "sig_s"] {
                assert!(g.point_attribs.contains(&format!("{p}_{case}")), "{p}_{case}");
            }
            assert!(g.prim_attribs.contains(&format!("ip_n_{case}")));
        }
        assert!(g.point_attribs.contains("disp") && g.point_attribs.contains("vm_mpa"));
        g.validate().unwrap();
        let rs: ResultSet = serde_json::from_value((**fields.as_json().unwrap()).clone()).unwrap();
        assert_eq!(rs.cases.len(), 4);
        assert_eq!(rs.source, "reference_linear");
    }

    #[test]
    fn unsolved_cases_are_an_error_not_zeros() {
        let c = ctx("rr2");
        let man = SolveManifest { dir: c.work_dir.to_string_lossy().into_owned(), solver: "external".into(), runs: vec![crate::types::RunEntry { case: "aero".into(), job: "j".into(), status: "not_run".into(), frd: String::new(), dat: String::new(), log: String::new(), hash: "x".into(), speed_factor: 1.0 }], ..Default::default() };
        let mesh = tet4_to_tet10(&box_tets(DVec3::splat(10.0), [1, 1, 1])).unwrap();
        let e = ResultRead.cook(&c, &[Some(to_data(&man)), Some(Data::geometry(mesh))], &defaults(&ResultRead)).unwrap_err().to_string();
        assert!(e.contains("no results"), "{e}");
    }

    #[test]
    fn material_frame_needs_the_orientation() {
        let c = ctx("rr3b");
        let mut p = defaults(&ResultRead);
        p.set("stress_frame", ParamValue::Str("material".into()));
        let man = SolveManifest::default();
        let mesh = tet4_to_tet10(&box_tets(DVec3::splat(10.0), [1, 1, 1])).unwrap();
        assert!(ResultRead.cook(&c, &[Some(to_data(&man)), Some(Data::geometry(mesh))], &p).is_err());
    }

    #[test]
    fn safety_factor_gates_the_mount_average_and_marks_the_reference_solver() {
        let (mesh, fields, mounts, _, card) = run("sf1");
        let c = ctx("sf1s");
        let out = SafetyFactor.cook(&c, &[Some(mesh), Some(fields), Some(card), None, Some(mounts)], &defaults(&SafetyFactor)).unwrap();
        let g = out[0].as_geometry().unwrap();
        assert!(g.point_attribs.contains("sf") && g.point_attribs.contains("sf_von_mises") && g.point_attribs.contains("sf_tsai_wu"));
        g.validate().unwrap();
        assert!(g.point_attribs.floats("sf").unwrap().iter().all(|v| v.is_finite() && *v > 0.0));
        let rep = out[1].as_report().unwrap();
        for k in ["sf_min", "sf_field_min", "sf_mount_min", "sf_peak_mount_min", "sf_mount1", "mount_vm_avg_mpa", "max_disp_mm"] {
            assert!(rep.metrics.contains_key(k), "{k}");
            assert!(rep.metrics[k].value.is_finite());
        }
        assert!(rep.metrics["sf_min"].value <= rep.metrics["sf_mount_min"].value + 1e-9);
        assert!(rep.metrics["sf_min"].value <= rep.metrics["sf_field_min"].value + 1e-9);
        assert!(!rep.passes(), "the reference solver must not pass a gate");
        assert!(rep.metrics.contains_key("estimate_only"));
        let t = out[2].as_json().unwrap();
        assert_eq!(t["mounts"].as_array().unwrap().len(), 1);
        assert_eq!(t["cases"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn overspeed_scales_the_aero_safety_factor_with_the_inverse_square() {
        let (mesh, fields, mounts, _, card) = run("sf2");
        let c = ctx("sf2s");
        let out = SafetyFactor.cook(&c, &[Some(mesh), Some(fields), Some(card), None, Some(mounts)], &defaults(&SafetyFactor)).unwrap();
        let t = out[2].as_json().unwrap();
        let sf = |name: &str| t["cases"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap()["sf_min"].as_f64().unwrap();
        // tsai-wu is linear in the load scale: SF(1.25 v) = SF(v) / 1.5625 (no temperature change)
        assert!((sf("aero_overspeed") * 1.5625 / sf("aero") - 1.0).abs() < 1e-4, "{} {}", sf("aero"), sf("aero_overspeed"));
        let rep = out[1].as_report().unwrap();
        assert!(rep.metrics["speed_limit_kmh"].value > 0.0);
    }

    #[test]
    fn missing_stress_attributes_are_an_error() {
        let (mesh, fields, _, _, card) = run("sf3");
        let mut g = (**mesh.as_geometry().unwrap()).clone();
        g.point_attribs.remove("sig_n_aero");
        let e = SafetyFactor.cook(&ctx("sf3s"), &[Some(Data::geometry(g)), Some(fields), Some(card), None, None], &defaults(&SafetyFactor)).unwrap_err().to_string();
        assert!(e.contains("sig_n_aero"), "{e}");
    }

    #[test]
    fn g7_criteria_through_the_node_on_a_one_element_state() {
        // a single stressed point: sx = 10, sz = 5, txz = 3 with the PLA-like strengths of the spec
        let mut card = MaterialCard::trans_iso("t", 3000.0, 2000.0, 0.35, 0.30, 800.0);
        card.xt = 50.0;
        card.xc = 60.0;
        card.zt = 27.5;
        card.zc = 60.0;
        card.s12 = 30.0;
        card.s13 = 20.0;
        card.s23 = 20.0;
        let mesh = tet4_to_tet10(&box_tets(DVec3::splat(10.0), [1, 1, 1])).unwrap();
        let n = mesh.points.len();
        let mut g = mesh.clone();
        g.point_attribs.set("sig_n_aero", AttribArray::Vec3(vec![DVec3::new(10.0, 0.0, 5.0); n]));
        g.point_attribs.set("sig_s_aero", AttribArray::Vec3(vec![DVec3::new(0.0, 0.0, 3.0); n]));
        let rs = ResultSet { source: "external".into(), frame: "material".into(), cases: vec![CaseResult { name: "aero".into(), speed_factor: 1.0, summary: ResultSummary::default() }] };
        let run = |crit: &str| {
            let mut p = defaults(&SafetyFactor);
            p.set("criterion", ParamValue::Str(crit.into()));
            let out = SafetyFactor.cook(&ctx("g7"), &[Some(Data::geometry(g.clone())), Some(to_data(&rs)), Some(to_data(&card)), None, None], &p).unwrap();
            out[1].as_report().unwrap().metrics["sf_min"].value
        };
        assert!((run("tsai_wu") - 3.381).abs() < 5e-3, "{}", run("tsai_wu"));
        assert!((run("von_mises") - 4.951).abs() < 5e-3, "{}", run("von_mises"));
        assert!((run("interlayer") - 4.24).abs() < 0.01, "{}", run("interlayer"));
        // zero stress is capped, not infinite
        g.point_attribs.set("sig_n_aero", AttribArray::Vec3(vec![DVec3::ZERO; n]));
        g.point_attribs.set("sig_s_aero", AttribArray::Vec3(vec![DVec3::ZERO; n]));
        let out = SafetyFactor.cook(&ctx("g7b"), &[Some(Data::geometry(g)), Some(to_data(&rs)), Some(to_data(&card)), None, None], &defaults(&SafetyFactor)).unwrap();
        assert_eq!(out[1].as_report().unwrap().metrics["sf_min"].value, 100.0);
    }

    #[test]
    fn mount_report_balances_reactions_and_loads() {
        let (_, fields, mounts, cases, _) = run("mr1");
        let out = MountReport.cook(&ctx("mr1r"), &[Some(fields), Some(cases), Some(mounts), None], &defaults(&MountReport)).unwrap();
        let rep = out[0].as_report().unwrap();
        for c in ["aero", "aero_overspeed", "bump", "aero_bump"] {
            let m = &rep.metrics[&format!("equilibrium_error_{c}")];
            assert!(m.value < 1e-3, "{c}: {}", m.value);
        }
        assert!(rep.metrics["reaction_aero_mount1_n"].value > 100.0);
        assert!(rep.metrics["worst_equilibrium_error"].value < 1e-3);
    }

    #[test]
    fn mount_report_flags_a_missing_reaction() {
        let rs = ResultSet { source: "external".into(), frame: "global".into(), cases: vec![CaseResult { name: "aero".into(), speed_factor: 1.0, summary: ResultSummary::default() }] };
        let mut ls = crate::types::LoadSet { name: "aero".into(), ..Default::default() };
        ls.resultant_n = [0.0, 0.0, -100.0];
        let cs = CaseSet { cases: vec![crate::types::LoadCase { name: "aero".into(), speed_factor: 1.0, loads: ls }] };
        let out = MountReport.cook(&ctx("mr2"), &[Some(to_data(&rs)), Some(to_data(&cs)), None, None], &defaults(&MountReport)).unwrap();
        let rep = out[0].as_report().unwrap();
        assert!(!rep.passes());
        assert_eq!(rep.metrics["equilibrium_checked"].value, 0.0);
    }

    #[test]
    fn g10_convergence_through_the_node() {
        let mk = |v: f64| {
            let mut r = Report::new("x");
            r.metric("max_disp_mm", Metric::new(v, "mm"));
            r.metric("sf_peak_mount_min", Metric::new(v, ""));
            r.metric("sf_mount_min", Metric::new(1.0, ""));
            Data::report(r)
        };
        let mut p = defaults(&MeshStudy);
        p.set("metrics", ParamValue::Str("max_disp_mm".into()));
        let out = MeshStudy.cook(&ctx("ms1"), &[Some(mk(1.6120)), Some(mk(1.6060)), Some(mk(1.5800))], &p).unwrap();
        let r = out[0].as_report().unwrap();
        assert!((r.metrics["max_disp_mm_order"].value - 2.115).abs() < 5e-3, "{}", r.metrics["max_disp_mm_order"].value);
        assert!((r.metrics["max_disp_mm_extrapolated"].value - 1.6138).abs() < 5e-4);
        assert!(r.metrics["max_disp_mm_gci_pct"].value < 0.2);
        assert!(r.passes(), "{:?}", r.metrics);
        // a non-monotone triplet fails
        let out = MeshStudy.cook(&ctx("ms2"), &[Some(mk(1.60)), Some(mk(1.61)), Some(mk(1.58))], &p).unwrap();
        let r = out[0].as_report().unwrap();
        assert_eq!(r.metrics["max_disp_mm_verdict"].value, 2.0);
        assert!(!r.passes());
        // singular: the raw peak moves by 30 percent while the average is steady
        let mut q = defaults(&MeshStudy);
        q.set("metrics", ParamValue::Str("sf_mount_min".into()));
        let peak = |v: f64, a: f64| {
            let mut r = Report::new("x");
            r.metric("sf_peak_mount_min", Metric::new(v, ""));
            r.metric("sf_mount_min", Metric::new(a, ""));
            Data::report(r)
        };
        let out = MeshStudy.cook(&ctx("ms3"), &[Some(peak(2.0, 4.0)), Some(peak(2.8, 4.02)), Some(peak(3.4, 4.05))], &q).unwrap();
        assert_eq!(out[0].as_report().unwrap().metrics["singular"].value, 1.0);
    }

    #[test]
    fn g8_hand_check_passes_in_plane_and_fails_through_the_stack() {
        let (card, _) = card_data("ppa_cf");
        let mut p = defaults(&HandCheck);
        let out = HandCheck.cook(&ctx("hc1"), &[Some(card.clone()), None, None], &p).unwrap();
        let r = out[0].as_report().unwrap();
        assert!((r.metrics["load_per_bolt_n"].value - 428.24).abs() < 0.01);
        assert!((r.metrics["bearing_mpa"].value - 21.41).abs() < 0.01);
        assert!(r.metrics["hand_sf_min"].value > 3.0 && r.passes(), "{:?}", r.metrics);
        p.set("load_through_stack", ParamValue::Bool(true));
        let out = HandCheck.cook(&ctx("hc2"), &[Some(card), None, None], &p).unwrap();
        let r = out[0].as_report().unwrap();
        assert!(r.metrics["bearing_sf"].value < 3.0 && !r.passes());
        // 250 km/h: 669 N per mount
        let (card, _) = card_data("ppa_cf");
        let mut p = defaults(&HandCheck);
        p.set("v_kmh", ParamValue::Float(250.0));
        let out = HandCheck.cook(&ctx("hc3"), &[Some(card), None, None], &p).unwrap();
        assert!((out[0].as_report().unwrap().metrics["load_per_bolt_n"].value - 669.1).abs() < 0.2);
    }

    #[test]
    fn hand_check_warns_when_the_fe_result_is_far_away() {
        let (card, _) = card_data("ppa_cf");
        let mut fe = Report::new("fe");
        fe.metric("sf_mount_min", Metric::new(0.5, ""));
        let out = HandCheck.cook(&ctx("hc4"), &[Some(card), None, Some(Data::report(fe))], &defaults(&HandCheck)).unwrap();
        let r = out[0].as_report().unwrap();
        assert!(r.notes.iter().any(|(s, t)| *s == Severity::Warning && t.contains("factor")));
    }

    #[test]
    fn deck_writer_and_reference_solver_use_the_same_case_names() {
        // the case names in the deck manifest and in the result set agree, so the report can match them
        let (_, fields, _, cases, card) = run("names");
        let rs: ResultSet = serde_json::from_value((**fields.as_json().unwrap()).clone()).unwrap();
        let cs: CaseSet = serde_json::from_value((**cases.as_json().unwrap()).clone()).unwrap();
        let mesh = tet4_to_tet10(&box_tets(DVec3::new(60.0, 40.0, 6.0), [12, 8, 2])).unwrap();
        let mut mp = defaults(&MountSites);
        mp.set("site_a", ParamValue::Vec3(DVec3::new(-25.0, 0.0, 3.0)));
        let ms = MountSites.cook(&ctx("names2"), &[Some(Data::geometry(mesh.clone()))], &mp).unwrap();
        let d = DeckWrite.cook(&ctx("names3"), &[Some(Data::geometry(mesh)), Some(card), Some(ms[0].clone()), Some(cases)], &defaults(&DeckWrite)).unwrap();
        let man: crate::types::DeckManifest = serde_json::from_value((**d[0].as_json().unwrap()).clone()).unwrap();
        let a: Vec<&str> = rs.cases.iter().map(|c| c.name.as_str()).collect();
        let b: Vec<String> = man.decks.iter().map(|d| d.case.clone()).collect();
        let c: Vec<String> = cs.cases.iter().map(|c| sanitize(&c.name).to_lowercase()).collect();
        assert_eq!(a, b.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(b, c);
    }
}
