//! Solver case writer and steady-solver runner.

use serde_json::json;
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::air::air_in;
use crate::coeffs::load_case_series;
use crate::extproc::run_chain;
use crate::foam::{self, CaseSpec, Flavour, FLAVOURS};
use crate::mesh::{manifest, mesh_steps, run_opts, script_text, solve_steps};
use crate::types::{BcSet, Domain, PorousZone, RefGeom};
use crate::util::{case_dir, file_in, json_in, placeholder, write_file, FnNode};

fn flavour_options() -> Vec<&'static str> {
    let mut v = vec!["from_mesh"];
    v.extend_from_slice(FLAVOURS);
    v
}

fn setup_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.case_setup", "Solver Case", Stage::Cfd)
        .input(PortSpec::optional("case", DataKind::File))
        .input(PortSpec::optional("air", DataKind::Json))
        .input(PortSpec::optional("boundary", DataKind::Json))
        .input(PortSpec::optional("porous", DataKind::Json))
        .input(PortSpec::optional("reference", DataKind::Json))
        .input(PortSpec::optional("domain", DataKind::Json))
        .output(PortSpec::new("case", DataKind::File))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice("flavour", "OpenFOAM flavour", "from_mesh", &flavour_options()).help("from_mesh takes the flavour the mesh dictionaries were written for"))
        .param(ParamSpec::float("u_ms", "Speed", 30.0, 10.0, 100.0).unit("m/s").help("Overridden by a wired air state"))
        .param(ParamSpec::float("nu", "Kinematic viscosity", 1.5e-5, 1.4e-5, 1.9e-5).unit("m2/s").help("Kinematic viscosity of the air, used for the Reynolds number and the inlet turbulence values."))
        .param(ParamSpec::float("rho_inf", "Density for force post-processing", 1.225, 0.9, 1.3).unit("kg/m3").help("The solver is incompressible; density only scales the reported forces"))
        .param(ParamSpec::float("turbulence_intensity_pct", "Inlet turbulence intensity", 0.25, 0.05, 1.0).unit("%").help("Turbulence intensity of the incoming flow. Sets the inlet turbulent kinetic energy."))
        .param(ParamSpec::float("nut_ratio", "Inlet viscosity ratio", 10.0, 1.0, 100.0).help("Inlet ratio of turbulent to laminar viscosity. Sets the inlet specific dissipation rate."))
        .param(ParamSpec::int("n_iter", "Max iterations", 3000, 500, 7000).help("Maximum number of steady iterations before the run stops."))
        .param(ParamSpec::int("window", "Averaging window", 1000, 100, 2000).unit("iter").help("Number of final iterations the force coefficients are averaged over."))
        .param(ParamSpec::float("residual_target", "Residual target", 1e-5, 1e-6, 1e-3).help("Residual level at which the run counts as converged and stops."))
        .param(ParamSpec::int("write_interval", "Write interval", 500, 50, 5000).unit("iter").help("Iterations between result write-outs. Larger saves disk, smaller gives a finer restart point."))
        .param(ParamSpec::int("np", "Processes", 8, 1, 512).help("Number of processes the case is decomposed over."))
        .param(ParamSpec::bool("separated_flow", "Separated-flow features", false).help("The kit has a diffuser, a rear wing near stall or tail separation. Steady two-equation RANS misses or smears such separation, so the case is flagged and the force report warns; confirm absolute numbers with a transient detached-eddy run, which this node does not write."))
        .param(ParamSpec::bool("limited_laplacian", "Limited Laplacian (non-orthogonality above 70)", false).help("Use a limited Laplacian scheme. It is steadier on meshes whose worst non-orthogonality is above 70 degrees but less accurate."))
        .help("Writes the steady two-equation RANS case (k-omega SST, wall functions): initial and boundary fields, transport and turbulence settings, schemes, solver controls, force and force-coefficient monitors, porous and rotating-frame zones. Flag separated-flow features to have the report and the force coefficients warn that steady RANS is only a trend there.")
        .emulates("CFD GUI front end: steady solver case writer")
        .spec("docs/specs/automotive-cfd.md")
        .external("OpenFOAM")
        .implemented()
}

fn setup_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let upstream = file_in(ins, 0)?;
    let man = upstream.as_deref().and_then(manifest);
    let case = case_dir(ctx, upstream)?;
    let mut report = Report::new("Solver case");

    let flavour_name = match p.str("flavour")? {
        "from_mesh" => man.as_ref().and_then(|m| m["flavour"].as_str().map(String::from)).unwrap_or_else(|| "foundation11".into()),
        other => other.to_string(),
    };
    let flavour = Flavour::parse(&flavour_name).ok_or_else(|| ctx.fail("unknown OpenFOAM flavour"))?;
    let rf = match (json_in(ins, 4)?, man.as_ref()) {
        (Some(v), _) => RefGeom::from_json(&v)?,
        (None, Some(m)) => RefGeom::from_json(&m["reference"]).unwrap_or_else(|_| RefGeom::reference_car()),
        _ => RefGeom::reference_car(),
    };
    let domain = match (json_in(ins, 5)?, man.as_ref()) {
        (Some(v), _) => Domain::from_json(&v)?,
        (None, Some(m)) => Domain::from_json(&m["domain"]).unwrap_or_else(|_| crate::tunnel::size_domain(&rf, 1.0, 3.0, 6.0, 5.0 / 3.0, rf.half_model, 0.0)),
        _ => crate::tunnel::size_domain(&rf, 1.0, 3.0, 6.0, 5.0 / 3.0, rf.half_model, 0.0),
    };
    let air = air_in(ins, 1)?;
    let u = air.u.unwrap_or(p.float("u_ms")?);
    let nu = air.nu.unwrap_or(p.float("nu")?);
    let rho = air.rho.unwrap_or(p.float("rho_inf")?);
    let bc = match json_in(ins, 2)? {
        Some(v) => BcSet::from_json(&v)?,
        None => {
            let mut gp = Params::from_specs(&crate::tunnel::gw_info().params);
            gp.set("u_ms", vernon_graph::ParamValue::Float(u));
            let out = crate::tunnel::gw_cook(ctx, &[None, Some(Data::json(rf.to_json())), None], &gp)?;
            BcSet::from_json(out[0].as_json().ok_or_else(|| ctx.fail("ground/wheel defaults failed"))?)?
        }
    };
    let zones: Vec<PorousZone> = match json_in(ins, 3)? {
        Some(v) => {
            let arr = v.get("zones").cloned().unwrap_or_else(|| json!([v.as_ref().clone()]));
            serde_json::from_value(arr).map_err(|e| ctx.fail(format!("not a porous zone list: {e}")))?
        }
        None => vec![],
    };
    for z in &zones {
        if z.bbox_m.is_none() {
            report.note(Severity::Warning, format!("porous zone '{}' has no volume and is skipped", z.name));
        }
    }
    let spec = CaseSpec {
        flavour,
        domain: &domain,
        rf: &rf,
        bc: &bc,
        zones: &zones,
        u,
        nu,
        rho,
        i_pct: p.float("turbulence_intensity_pct")?,
        nut_ratio: p.float("nut_ratio")?,
        n_iter: p.int("n_iter")? as u32,
        window: p.int("window")? as u32,
        residual: p.float("residual_target")?,
        write_interval: p.int("write_interval")? as u32,
        nonortho_over_70: p.bool("limited_laplacian")?,
    };
    if spec.window >= spec.n_iter {
        return Err(ctx.fail("averaging window must be shorter than the iteration count"));
    }
    for (name, text) in [
        ("U", foam::u_field(&spec)),
        ("p", foam::p_field(&spec)),
        ("k", foam::k_field(&spec)),
        ("omega", foam::omega_field(&spec)),
        ("nut", foam::nut_field(&spec)),
    ] {
        write_file(&case.join("0").join(name), &text)?;
        write_file(&case.join("0.orig").join(name), &text)?;
    }
    write_file(&case.join("constant").join(flavour.physical_properties()), &foam::physical_properties(&spec))?;
    write_file(&case.join("constant").join(flavour.momentum_transport()), &foam::momentum_transport(&spec))?;
    write_file(&case.join("system/controlDict"), &foam::control_dict(&spec))?;
    write_file(&case.join("system/fvSchemes"), &foam::fv_schemes(&spec))?;
    write_file(&case.join("system/fvSolution"), &foam::fv_solution(&spec))?;
    if !case.join("system/decomposeParDict").exists() {
        write_file(&case.join("system/decomposeParDict"), &foam::decompose_par_dict(p.int("np")? as u32))?;
    }
    let topo = foam::topo_set_dict(&spec);
    if let Some(t) = &topo {
        write_file(&case.join("system/topoSetDict"), t)?;
    }
    if let Some(t) = foam::porous_source(&spec) {
        write_file(&case.join(flavour.fv_models_path()), &t)?;
        report.note(Severity::Info, "porous source layout follows the published class description; confirm on newer flavours with a golden run");
    }
    if let Some(t) = foam::mrf_properties(&spec) {
        write_file(&case.join("constant/MRFProperties"), &t)?;
    }
    let np = p.int("np")? as u32;
    let steps = solve_steps(flavour, np, true, topo.is_some());
    write_file(&case.join("Allrun.solve"), &script_text(&steps))?;

    // keep the manifest current
    let mut m = man.unwrap_or_else(|| json!({}));
    let dx0 = m["dx0_m"].as_f64().filter(|d| *d > 0.0);
    let separated_flow = p.bool("separated_flow")?;
    let recipe = crate::types::Recipe {
        dx0_m: dx0,
        base_cells: dx0.map(|d| {
            let n = foam::base_cells(&domain, d);
            (n[0] * n[1] * n[2]) as f64
        }),
        levels: m["plan"].get("levels").map(serde_json::Value::to_string),
        ground_mode: bc.ground_mode.clone(),
        wheel_mode: bc.wheel_mode.clone(),
        yaw_deg: domain.yaw_deg,
        turbulence: format!("kOmegaSST I={:.3}% nut/nu={:.1}", spec.i_pct, spec.nut_ratio),
        separated_flow,
    };
    m["flavour"] = json!(flavour.name());
    m["np"] = json!(np);
    m["case_setup"] = json!({
        "u_ms": u, "nu": nu, "rho_inf": rho, "aref_m2": spec.aref(), "half_model": domain.half_model,
        "window": spec.window, "n_iter": spec.n_iter, "residual_target": spec.residual,
        "reference": rf.to_json(), "has_topo": topo.is_some(), "recipe": recipe,
    });
    write_file(&case.join("vernon_case.json"), &serde_json::to_string_pretty(&m).unwrap_or_default())?;

    report
        .metric("k_inf", Metric::new(spec.k_inf(), "m2/s2"))
        .metric("omega_inf", Metric::new(spec.omega_inf(), "1/s"))
        .metric("u_ms", Metric::new(u, "m/s"))
        .metric("nu", Metric::new(nu, "m2/s"))
        .metric("rho_inf", Metric::new(rho, "kg/m3"))
        .metric("aref_m2", Metric::new(spec.aref(), "m2"))
        .metric("porous_zones", Metric::new(zones.iter().filter(|z| z.bbox_m.is_some()).count() as f64, "count"));
    report.metric("separated_flow_flagged", Metric::new(f64::from(separated_flow), "-"));
    if separated_flow {
        report.note(
            Severity::Warning,
            "separated-flow features are flagged (diffuser, rear wing near stall or tail separation): this case is steady k-omega SST, which misses or smears such separation; use its deltas as a trend and confirm absolute CdA and ClA with a transient detached-eddy (DDES or SBES) run",
        );
    } else {
        report.note(Severity::Info, "steady k-omega SST with wall functions; if the kit has a diffuser, a rear wing near stall or tail separation, flag separated-flow features");
    }
    if domain.half_model {
        report.note(Severity::Info, "half model: forces integrate the y >= 0 half and the reference area is half the full-car frontal area");
    }
    if flavour.uses_foam_run() {
        report.note(Severity::Info, "this flavour runs the steady solver through the modular front end; confirm on a golden run");
    }
    if !bc.wheels.is_empty() && bc.wheel_mode == "rotating" && !domain.half_model {
        report.note(Severity::Info, "full car: left and right wheel patches both rotate");
    }
    Ok(vec![Data::File(case), Data::report(report)])
}

// ------------------------------------------------------------------ steady solve

fn solve_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.solve_steady", "Run Steady Solver", Stage::Cfd)
        .input(PortSpec::new("case", DataKind::File))
        .output(PortSpec::new("case", DataKind::File))
        .output(PortSpec::new("history", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::bool("run", "Run now", false).help("Starts the solver once and switches itself off again, so a re-cook never relaunches hours of CPU. Results the case holds for these exact inputs are always read.").action())
        .param(ParamSpec::int("np", "Processes", 8, 1, 512).help("Number of processes the run is decomposed over."))
        .param(ParamSpec::bool("potential_init", "Initialise with a potential-flow solution", true).help("Start from a potential-flow solution instead of uniform flow. It usually speeds up the first iterations."))
        .param(ParamSpec::choice("runner", "Runner", "native", &["native", "wsl", "docker"]).help("Where the solver programs run: directly on this machine, in a Linux subsystem, or in a container."))
        .param(ParamSpec::string("image", "Container image", "").help("Docker runner only"))
        .param(ParamSpec::path("bin_dir", "Tool directory", &[], false).help("Folder with the OpenFOAM executables; empty searches the PATH"))
        .param(ParamSpec::int("timeout_min", "Time limit", 720, 0, 100_000).unit("min").help("The run (and every process it started) is stopped after this long; 0 = one day"))
        .help("Runs the steady solver in the case directory and reads the force-coefficient, force and residual histories back as a time series. Without the solver it reads whatever results the case already holds.")
        .emulates("automotive wind-tunnel service: solve")
        .spec("docs/specs/automotive-cfd.md")
        .external("OpenFOAM")
}

fn solve_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let case = file_in(ins, 0)?.ok_or(CookError::MissingInput { index: 0, name: "case".into() })?;
    if !case.join("system/controlDict").exists() {
        return Err(ctx.fail("case has no system/controlDict; connect the solver case writer"));
    }
    let man = manifest(&case);
    let flavour = man
        .as_ref()
        .and_then(|m| m["flavour"].as_str().and_then(Flavour::parse))
        .unwrap_or(Flavour::Foundation(11));
    let mut report = Report::new("Steady solve");
    let stamp = case_stamp(&case);
    if p.bool("run")? {
        let topo = case.join("system/topoSetDict").exists();
        let steps = solve_steps(flavour, p.int("np")? as u32, p.bool("potential_init")?, topo);
        let res = run_chain(&run_opts(p)?, &case, &steps);
        report.metric("steps_completed", Metric::new(res.completed.len() as f64, "count"));
        match res.failed {
            Some(f) => {
                report.note(Severity::Error, format!("solver run stopped: {f}"));
            }
            None => write_file(&case.join(RESULTS_STAMP), &stamp)?,
        }
    }
    let series = match std::fs::read_to_string(case.join(RESULTS_STAMP)) {
        Ok(s) if s.trim() == stamp => load_case_series(&case),
        Ok(_) => {
            report.note(Severity::Warning, "the results in the case were solved for other inputs and are not read; run the solver again");
            Default::default()
        }
        Err(_) => {
            let s = load_case_series(&case);
            if !s.is_empty() {
                report.note(Severity::Warning, "the results in the case were not produced by a run here; check they belong to these inputs");
            }
            s
        }
    };
    let mut series = series;
    series.recipe = man.as_ref().and_then(|m| serde_json::from_value(m["case_setup"]["recipe"].clone()).ok());
    let done = !series.is_empty();
    if !done {
        placeholder(
            &mut report,
            "run the potential-flow initialisation and the steady solver, then read the force-coefficient and residual histories (run is off or the tools were not found)",
        );
    }
    report
        .metric("iterations_read", Metric::new(series.len() as f64, "count"))
        .metric("solve_done", Metric::new(f64::from(done), "-").with_limit(1.0, false));
    let _ = (mesh_steps, ctx);
    Ok(vec![Data::File(case), Data::json(series.to_json()), Data::report(report)])
}

/// File in a case recording which inputs its results were solved for.
const RESULTS_STAMP: &str = "vernon_results.stamp";

/// Stable FNV-1a hash of what a solve depends on: the case manifest, the
/// `system` dictionaries and the surface files under `constant/triSurface`
/// (not the generated feature-edge files). Files the solver writes (time
/// folders, `0/` after initialisation, `postProcessing`) are left out, so
/// the stamp of a case does not change by solving it.
fn case_stamp(case: &std::path::Path) -> String {
    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    collect(&p, out);
                } else {
                    out.push(p);
                }
            }
        }
    }
    let mut files = vec![case.join("vernon_case.json")];
    collect(&case.join("system"), &mut files);
    collect(&case.join("constant").join("triSurface"), &mut files);
    files.retain(|f| f.extension().is_none_or(|e| e != "eMesh"));
    files.sort();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    for f in &files {
        feed(f.strip_prefix(case).unwrap_or(f).to_string_lossy().replace('\\', "/").as_bytes());
        feed(&std::fs::read(f).unwrap_or_default());
    }
    format!("{h:016x}")
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(setup_info, setup_cook));
    reg.add(FnNode::new(solve_info, solve_cook));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::*;
    use std::path::Path;
    use vernon_graph::ParamValue;

    fn read(case: &Path, rel: &str) -> String {
        std::fs::read_to_string(case.join(rel)).unwrap_or_else(|_| panic!("missing {rel}"))
    }

    fn setup(ins: &[Option<Data>], set: &[(&str, ParamValue)], name: &str) -> (std::path::PathBuf, Report) {
        let mut p = Params::from_specs(&setup_info().params);
        for (k, v) in set {
            p.set(k, v.clone());
        }
        let out = setup_cook(&ctx(name), ins, &p).unwrap();
        (out[0].as_file().unwrap().clone(), out[1].as_report().unwrap().as_ref().clone())
    }

    #[test]
    fn g8_inlet_turbulence_values() {
        let (case, r) = setup(&[], &[("u_ms", ParamValue::Float(40.0))], "g8");
        assert!((r.metrics["k_inf"].value - 0.015).abs() < 1e-9);
        assert!((r.metrics["omega_inf"].value - 100.0).abs() < 1e-6);
        let k = read(&case, "0/k");
        assert!(k.contains("uniform 0.015;"), "{k}");
        assert!(read(&case, "0/omega").contains("uniform 100;"));
    }

    #[test]
    fn separated_flow_flag_warns_and_is_recorded_in_the_recipe() {
        let (case, r) = setup(&[], &[("separated_flow", ParamValue::Bool(true))], "sepflow-on");
        assert_eq!(r.metrics["separated_flow_flagged"].value, 1.0);
        assert!(r.notes.iter().any(|(s, m)| *s == Severity::Warning && m.contains("separated-flow features are flagged")), "{r:?}");
        let man: serde_json::Value = serde_json::from_str(&read(&case, "vernon_case.json")).unwrap();
        assert_eq!(man["case_setup"]["recipe"]["separated_flow"], true);
        let (case, r) = setup(&[], &[], "sepflow-off");
        assert_eq!(r.metrics["separated_flow_flagged"].value, 0.0);
        assert!(!r.notes.iter().any(|(_, m)| m.contains("features are flagged")));
        let man: serde_json::Value = serde_json::from_str(&read(&case, "vernon_case.json")).unwrap();
        assert_eq!(man["case_setup"]["recipe"]["separated_flow"], false);
    }

    #[test]
    fn every_file_parses_for_every_flavour() {
        for fl in FLAVOURS {
            let (case, _) = setup(&[], &[("flavour", ParamValue::Str((*fl).into()))], &format!("fl-{fl}"));
            let f = Flavour::parse(fl).unwrap();
            for rel in [
                "0/U",
                "0/p",
                "0/k",
                "0/omega",
                "0/nut",
                "system/controlDict",
                "system/fvSchemes",
                "system/fvSolution",
                "system/decomposeParDict",
            ] {
                foam::check_syntax(&read(&case, rel)).unwrap_or_else(|e| panic!("{fl} {rel}: {e}"));
            }
            for rel in [f.physical_properties(), f.momentum_transport()] {
                foam::check_syntax(&read(&case, &format!("constant/{rel}"))).unwrap_or_else(|e| panic!("{fl} {rel}: {e}"));
            }
            let ctl = read(&case, "system/controlDict");
            assert_eq!(ctl.contains("foamRun"), f.uses_foam_run(), "{fl}");
            assert!(ctl.contains("libs            (forces)") || ctl.contains("libs            (\"libforces.so\")"), "{fl}");
        }
    }

    #[test]
    fn boundary_conditions_follow_the_table() {
        let (case, _) = setup(&[], &[], "bc");
        let u = read(&case, "0/U");
        assert!(u.contains("type fixedValue;\n        value uniform (30 0 0);"));
        assert!(u.contains("inletOutlet"));
        assert!(u.contains("symmetryPlane"));
        assert!(u.contains("\"wheel_fl.*\""));
        assert!(u.contains("rotatingWallVelocity"));
        assert!(u.contains("origin (0.92 -0.785 0.3137)"), "{u}");
        let omega = (30.0f64 / 0.3137).round();
        assert!(u.contains(&format!("omega {}", -omega)) || u.contains("omega -95.6"), "{u}");
        assert!(read(&case, "0/p").contains("fixedValue;\n        value uniform 0;"));
        assert!(read(&case, "0/nut").contains("nutUSpaldingWallFunction"));
        assert!(read(&case, "0/k").contains("kqRWallFunction"));
        assert!(read(&case, "0/omega").contains("omegaWallFunction"));
        assert!(read(&case, "0.orig/U").len() > 100);
    }

    #[test]
    fn force_coeffs_use_half_area_and_density() {
        let (case, r) = setup(&[], &[], "fc");
        let ctl = read(&case, "system/controlDict");
        assert!(ctl.contains("Aref            1.08;"), "{ctl}");
        assert!(ctl.contains("rhoInf          1.225;"));
        assert!(ctl.contains("magUInf         30;"));
        assert!(ctl.contains("porosity        no;"));
        assert!(!ctl.contains("ground"));
        assert!(ctl.contains("endTime         3000;"));
        assert!((r.metrics["aref_m2"].value - 1.08).abs() < 1e-9);
        let sol = read(&case, "system/fvSolution");
        assert!(sol.contains("residualControl") && sol.contains("consistent      yes;"));
        // full car uses the whole area
        let dom = crate::tunnel::size_domain(&RefGeom::reference_car(), 1.0, 3.0, 6.0, 5.0 / 3.0, false, 0.0);
        let (case, r) = setup(&[None, None, None, None, None, Some(Data::json(dom.to_json()))], &[], "fc-full");
        assert!((r.metrics["aref_m2"].value - 2.16).abs() < 1e-9);
        assert!(!read(&case, "0/U").contains("symmetryPlane"));
    }

    #[test]
    fn porous_and_mrf_files() {
        let porous = crate::tunnel::porous_cook_for_case_tests();
        let (case, r) = setup(&[None, None, None, Some(Data::json(porous))], &[], "porous");
        assert_eq!(r.metrics["porous_zones"].value, 1.0);
        let fvm = read(&case, "constant/fvModels");
        foam::check_syntax(&fvm).unwrap();
        assert!(fvm.contains("DarcyForchheimer") && fvm.contains("cellZone        radiator;"));
        assert!(read(&case, "system/topoSetDict").contains("boxToCell"));
        assert!(read(&case, "system/controlDict").contains("porosity        yes;"));
        // older flavours use fvOptions
        let porous = crate::tunnel::porous_cook_for_case_tests();
        let (case, _) = setup(&[None, None, None, Some(Data::json(porous))], &[("flavour", ParamValue::Str("opencfd2312".into()))], "porous2");
        assert!(case.join("constant/fvOptions").exists());
        // a zone without volume is skipped with a warning
        let mut gp = Params::from_specs(&crate::tunnel::gw_info().params);
        gp.set("wheel_mode", ParamValue::Str("mrf".into()));
        let bc = crate::tunnel::gw_cook(&ctx("gw"), &[], &gp).unwrap();
        let (case, _) = setup(&[None, None, Some(bc[0].clone())], &[], "mrf");
        let mrf = read(&case, "constant/MRFProperties");
        foam::check_syntax(&mrf).unwrap();
        assert!(mrf.contains("MRF_wheel_fl"));
        let topo = read(&case, "system/topoSetDict");
        foam::check_syntax(&topo).unwrap();
        assert!(topo.contains("cylinderToCell"));
        assert!(!read(&case, "0/U").contains("rotatingWallVelocity"));
    }

    #[test]
    fn writing_is_deterministic_and_in_place() {
        let (case, _) = setup(&[], &[], "det");
        let a = read(&case, "system/fvSchemes");
        let out = setup_cook(&ctx("det2"), &[Some(Data::File(case.clone()))], &Params::from_specs(&setup_info().params)).unwrap();
        assert_eq!(out[0].as_file().unwrap(), &case);
        assert_eq!(a, read(&case, "system/fvSchemes"));
        let m = manifest(&case).unwrap();
        assert_eq!(m["case_setup"]["aref_m2"], 1.08);
    }

    #[test]
    fn rejects_window_longer_than_the_run() {
        let mut p = Params::from_specs(&setup_info().params);
        p.set("n_iter", ParamValue::Int(500));
        p.set("window", ParamValue::Int(600));
        assert!(setup_cook(&ctx("win"), &[], &p).is_err());
    }

    #[test]
    fn solve_reads_existing_results_and_fails_closed_without() {
        let (case, _) = setup(&[], &[], "solve");
        let p = Params::from_specs(&solve_info().params);
        let out = solve_cook(&ctx("solve-run"), &[Some(Data::File(case.clone()))], &p).unwrap();
        let r = out[2].as_report().unwrap();
        assert!(!r.passes());
        assert!(r.notes.iter().any(|(_, t)| t.starts_with("white-box placeholder:")));
        assert!(crate::types::Series::from_json(out[1].as_json().unwrap()).unwrap().is_empty());
        // drop a recorded coefficient history into the case
        let dir = case.join("postProcessing/forceCoeffs1/0");
        std::fs::create_dir_all(&dir).unwrap();
        let mut dat = String::from("# Force coefficients\n# magUInf      : 30\n# Aref         : 1.08\n# Time         Cd              Cl\n");
        for i in 1..=50 {
            dat += &format!("{i} {} {}\n", 0.3 + 1e-4 * f64::from(i % 3), -0.2);
        }
        std::fs::write(dir.join("coefficient.dat"), dat).unwrap();
        let out = solve_cook(&ctx("solve-run"), &[Some(Data::File(case.clone()))], &p).unwrap();
        let r = out[2].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        assert!(r.notes.iter().any(|(s, t)| *s == Severity::Warning && t.contains("not produced by a run here")));
        let s = crate::types::Series::from_json(out[1].as_json().unwrap()).unwrap();
        assert_eq!(s.len(), 50);
        assert_eq!(s.meta["Aref"], 1.08);
        // stamped for these inputs: read without a warning
        std::fs::write(case.join(RESULTS_STAMP), case_stamp(&case)).unwrap();
        let out = solve_cook(&ctx("solve-run"), &[Some(Data::File(case.clone()))], &p).unwrap();
        assert!(out[2].as_report().unwrap().notes.iter().all(|(s, _)| *s != Severity::Warning));
        // the case is rewritten for other inputs: the old results are not read
        let dict = read(&case, "system/controlDict");
        std::fs::write(case.join("system/controlDict"), dict + "\n// other inputs\n").unwrap();
        let out = solve_cook(&ctx("solve-run"), &[Some(Data::File(case.clone()))], &p).unwrap();
        assert!(crate::types::Series::from_json(out[1].as_json().unwrap()).unwrap().is_empty());
        assert!(out[2].as_report().unwrap().notes.iter().any(|(_, t)| t.contains("solved for other inputs")));
        // missing tool with run on: reported, never a fake success
        let mut p2 = p.clone();
        p2.set("run", ParamValue::Bool(true));
        p2.set("bin_dir", ParamValue::Str(case.join("nowhere").display().to_string()));
        let out = solve_cook(&ctx("solve-run"), &[Some(Data::File(case))], &p2).unwrap();
        assert!(out[2].as_report().unwrap().notes.iter().any(|(s, _)| *s == Severity::Error));
        assert!(solve_cook(&ctx("solve-run"), &[], &p).is_err());
    }
}
