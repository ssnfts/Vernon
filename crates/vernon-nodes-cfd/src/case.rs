//! Solver case writer and steady-solver runner.

use serde_json::json;
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{
    CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec,
    Registry, Stage,
};

use crate::air::air_in;
use crate::coeffs::{load_case_series, load_case_series_checked};
use crate::foam::{self, CaseSpec, Flavour, FLAVOURS};
use crate::mesh::{
    manifest, mesh_steps, run_owned_chain, script_text, solve_steps, unavailable_output,
};
use crate::types::{BcSet, Domain, PorousZone, RefGeom, SolveEvidence};
use crate::util::{
    case_input_fingerprint, file_in, json_in, placeholder, read_mesh_evidence, stage_case,
    stage_case_optional, write_file, CaseStageKind, FnNode, InitialFieldSource,
};

/// Keep the new owned cases separate from legacy, unmarked writer output.
pub(crate) fn owned_context(ctx: &CookContext) -> CookContext {
    CookContext {
        work_dir: ctx.work_dir.join("native"),
        ..ctx.clone()
    }
}

pub(crate) fn recipe_key(version: &str, ins: &[Option<Data>], p: &Params) -> CookResult<String> {
    fn ordered(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: std::collections::BTreeMap<_, _> =
                    map.into_iter().map(|(k, v)| (k, ordered(v))).collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(ordered).collect())
            }
            other => other,
        }
    }
    let inputs = (1..ins.len())
        .map(|i| json_in(ins, i).map(|v| v.map(|x| x.as_ref().clone())))
        .collect::<CookResult<Vec<_>>>()?;
    // `run` is a one-shot action, not an input: resetting it must not
    // invalidate a completed result.
    let mut params = serde_json::to_value(p)
        .map_err(|e| CookError::Other(format!("cannot encode case recipe: {e}")))?;
    if let Some(map) = params.as_object_mut() {
        map.remove("run");
    }
    serde_json::to_string(&ordered(
        json!({"template": version, "params": params, "json_inputs": inputs}),
    ))
    .map_err(|e| CookError::Other(format!("cannot encode case recipe: {e}")))
}

/// Fixed relative writer paths must not follow links added to an owned cache.
fn owned_file(
    ctx: &CookContext,
    case: &std::path::Path,
    relative: &str,
) -> CookResult<std::path::PathBuf> {
    let rel = std::path::Path::new(relative);
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(ctx.fail("case writer requires a plain relative file path"));
    }
    let path = case.join(rel);
    for part in path.ancestors().take_while(|p| *p != case) {
        match std::fs::symlink_metadata(part) {
            Ok(meta) => {
                let linked = meta.file_type().is_symlink();
                #[cfg(windows)]
                let linked = {
                    use std::os::windows::fs::MetadataExt;
                    linked || meta.file_attributes() & 0x400 != 0
                };
                if linked {
                    return Err(ctx.fail(format!(
                        "case writer refuses a link or reparse point: {}",
                        part.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(ctx.fail(format!(
                    "cannot inspect owned case file {}: {e}",
                    part.display()
                )))
            }
        }
    }
    Ok(path)
}

fn write_setup_file(
    ctx: &CookContext,
    case: &std::path::Path,
    relative: &str,
    text: &str,
) -> CookResult<()> {
    let path = owned_file(ctx, case, relative)?;
    if let Ok(file) = std::fs::File::open(&path) {
        if file.metadata().is_ok_and(|m| m.len() == text.len() as u64) {
            use std::io::Read;
            let mut bytes = Vec::new();
            if file
                .take((text.len() as u64).saturating_add(1))
                .read_to_end(&mut bytes)
                .is_ok()
                && bytes == text.as_bytes()
            {
                return Ok(());
            }
        }
    }
    write_file(&path, text)
}

fn remove_setup_file(ctx: &CookContext, case: &std::path::Path, relative: &str) -> CookResult<()> {
    let path = owned_file(ctx, case, relative)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ctx.fail(format!(
            "cannot remove inactive owned case file {}: {e}",
            path.display()
        ))),
    }
}

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
        .param(ParamSpec::bool("separated_flow", "Separated-flow features", false).help("The kit has a diffuser, a rear wing near stall or tail separation. Steady two-equation RANS misses or smears such separation, so the case is flagged and the force report warns; confirm absolute numbers with a transient detached-eddy run."))
        .param(ParamSpec::bool("limited_laplacian", "Limited Laplacian (non-orthogonality above 70)", false).help("Use a limited Laplacian scheme. It is steadier on meshes whose worst non-orthogonality is above 70 degrees but less accurate."))
        .param(ParamSpec::choice("solver_mode", "Solver recipe", "steady_sst", &["steady_sst", "transient_ddes"]).help("steady_sst writes the steady case the run node solves. transient_ddes writes a detached-eddy case for one flavour; the settings below are starting values, the file is not run by this graph and is not validated for accuracy."))
        .param(ParamSpec::float("end_time_s", "Transient end time", 2.0, 0.0001, 100.0).unit("s").help("Transient recipe only: physical time at which the run stops, in seconds."))
        .param(ParamSpec::float("delta_t_s", "Transient first time step", 1e-4, 1e-7, 1.0).unit("s").help("Transient recipe only: first time step, in seconds. The solver then adjusts it to the Courant limit."))
        .param(ParamSpec::float("max_delta_t_s", "Transient largest time step", 1e-3, 1e-7, 1.0).unit("s").help("Transient recipe only: the adjusted time step never grows beyond this, in seconds."))
        .param(ParamSpec::float("max_co", "Transient Courant limit", 0.5, 0.05, 1.0).help("Transient recipe only: requested largest Courant number. A request, not a measurement or a stability guarantee."))
        .param(ParamSpec::float("write_interval_s", "Transient write interval", 0.05, 0.0001, 100.0).unit("s").help("Transient recipe only: physical time between field write-outs, in seconds."))
        .param(ParamSpec::int("outer_correctors", "Transient outer correctors", 2, 1, 10).help("Transient recipe only: outer pressure-velocity loops per time step."))
        .param(ParamSpec::int("correctors", "Transient pressure correctors", 2, 1, 10).help("Transient recipe only: pressure corrections per outer loop."))
        .help("Writes the two-equation RANS case (k-omega SST, wall functions) by default: initial and boundary fields, transport and turbulence settings, schemes, solver controls, force and force-coefficient monitors, porous and rotating-frame zones. Flag separated-flow features to have the report and the force coefficients warn that steady RANS is only a trend there. The solver recipe setting can instead write a transient detached-eddy case to run outside this graph.")
        .emulates("CFD GUI front end: steady solver case writer")
        .spec("docs/specs/automotive-cfd.md")
        .external("OpenFOAM")
        .implemented()
}

fn setup_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let upstream = file_in(ins, 0)?;
    let man = upstream.as_deref().and_then(manifest);
    let mut report = Report::new("Solver case");

    let flavour_name = match p.str("flavour")? {
        "from_mesh" => man
            .as_ref()
            .and_then(|m| m["flavour"].as_str().map(String::from))
            .unwrap_or_else(|| "foundation11".into()),
        other => other.to_string(),
    };
    let flavour =
        Flavour::parse(&flavour_name).ok_or_else(|| ctx.fail("unknown OpenFOAM flavour"))?;
    let rf = match (json_in(ins, 4)?, man.as_ref()) {
        (Some(v), _) => RefGeom::from_json(&v)?,
        (None, Some(m)) => {
            RefGeom::from_json(&m["reference"]).unwrap_or_else(|_| RefGeom::reference_car())
        }
        _ => RefGeom::reference_car(),
    };
    let domain = match (json_in(ins, 5)?, man.as_ref()) {
        (Some(v), _) => Domain::from_json(&v)?,
        (None, Some(m)) => Domain::from_json(&m["domain"]).unwrap_or_else(|_| {
            crate::tunnel::size_domain(&rf, 1.0, 3.0, 6.0, 5.0 / 3.0, rf.half_model, 0.0)
        }),
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
            let out = crate::tunnel::gw_cook(
                ctx,
                &[
                    Some(Data::json(domain.to_json())),
                    Some(Data::json(rf.to_json())),
                    None,
                ],
                &gp,
            )?;
            BcSet::from_json(
                out[0]
                    .as_json()
                    .ok_or_else(|| ctx.fail("ground/wheel defaults failed"))?,
            )?
        }
    };
    let uses_motion =
        bc.ground_mode != "stationary" || (bc.wheel_mode != "static" && !bc.wheels.is_empty());
    if uses_motion
        && (!bc.u_ms.is_finite()
            || (bc.u_ms - u).abs() > 1e-9 * u.abs().max(bc.u_ms.abs()).max(1.0))
    {
        return Err(ctx.fail("ground or wheel motion speed differs from the case inflow speed; update the connected boundary settings"));
    }
    let zones: Vec<PorousZone> = match json_in(ins, 3)? {
        Some(v) => {
            let arr = v
                .get("zones")
                .cloned()
                .unwrap_or_else(|| json!([v.as_ref().clone()]));
            serde_json::from_value(arr)
                .map_err(|e| ctx.fail(format!("not a porous zone list: {e}")))?
        }
        None => vec![],
    };
    for z in &zones {
        if z.bbox_m.is_none() {
            report.note(
                Severity::Warning,
                format!("porous zone '{}' has no volume and is skipped", z.name),
            );
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
        monitor_patches: man
            .as_ref()
            .and_then(|m| m["patches"].as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
    };
    // A transient recipe is refused before any case is created.
    let transient = match p.str("solver_mode")? {
        "steady_sst" => None,
        "transient_ddes" => {
            if flavour != Flavour::OpenCfd(2606) {
                return Err(ctx.fail("the transient recipe is only written for opencfd2606, the release it was run on"));
            }
            if bc.ground_mode == "slip" || bc.wheel_mode == "mrf" || !zones.is_empty() {
                return Err(ctx.fail("the transient recipe supports wall ground and fixed or rotating-wall wheels only: no slip ground, rotating-frame zones or porous zones"));
            }
            let t = foam::TransientControls {
                end_time_s: p.float("end_time_s")?,
                delta_t_s: p.float("delta_t_s")?,
                max_delta_t_s: p.float("max_delta_t_s")?,
                max_co: p.float("max_co")?,
                write_interval_s: p.float("write_interval_s")?,
                outer_correctors: p.int("outer_correctors")?.clamp(0, 1000) as u32,
                correctors: p.int("correctors")?.clamp(0, 1000) as u32,
            };
            t.validate().map_err(|e| ctx.fail(e))?;
            Some(t)
        }
        other => return Err(ctx.fail(format!("unknown solver recipe '{other}'"))),
    };
    if transient.is_none() && spec.window >= spec.n_iter {
        return Err(ctx.fail("averaging window must be shorter than the iteration count"));
    }
    let native = owned_context(ctx);
    let reference_source = reference_diagnostic(
        reference_mesh_input(&native, upstream.as_deref(), man.as_ref(), &domain, &rf),
        &mut report,
    )?;
    let stage = stage_case_optional(
        &native,
        upstream.as_deref(),
        CaseStageKind::CaseSetupInputs,
        &recipe_key("cfd.case_setup/v3", ins, p)?,
        false,
    )?;
    let case = &stage.path;
    // A fresh clone must match the certified builder inputs before this writer
    // replaces any dictionaries. A reused clone has already configured fields;
    // its measured tables and geometry are checked again before publication.
    let reference_clone_matches = match &reference_source {
        Some(source) if !stage.reused => {
            case_input_fingerprint(&native, case)? == source.fingerprint
        }
        Some(_) => true,
        None => false,
    };
    report.metric(
        "owned_case_reused",
        Metric::new(f64::from(stage.reused), "-"),
    );
    if stage.initial_fields == InitialFieldSource::CurrentZero {
        report.note(Severity::Info, "upstream has no immutable initial snapshot; current initial fields were copied before writing the configured fields");
    }
    for (name, text) in [
        ("U", foam::u_field(&spec)),
        ("p", foam::p_field(&spec)),
        ("k", foam::k_field(&spec)),
        ("omega", foam::omega_field(&spec)),
        ("nut", foam::nut_field(&spec)),
    ] {
        write_setup_file(ctx, case, &format!("0/{name}"), &text)?;
        write_setup_file(ctx, case, &format!("0.orig/{name}"), &text)?;
    }
    for name in ["physicalProperties", "transportProperties"] {
        if name != flavour.physical_properties() {
            remove_setup_file(ctx, case, &format!("constant/{name}"))?;
        }
    }
    for name in ["momentumTransport", "turbulenceProperties"] {
        if name != flavour.momentum_transport() {
            remove_setup_file(ctx, case, &format!("constant/{name}"))?;
        }
    }
    write_setup_file(
        ctx,
        case,
        &format!("constant/{}", flavour.physical_properties()),
        &foam::physical_properties(&spec),
    )?;
    write_setup_file(
        ctx,
        case,
        &format!("constant/{}", flavour.momentum_transport()),
        &match &transient {
            Some(_) => foam::ddes_turbulence(&spec),
            None => foam::momentum_transport(&spec),
        },
    )?;
    let (control, schemes, solution) = match &transient {
        Some(t) => (
            foam::ddes_control_dict(&spec, t),
            foam::ddes_fv_schemes(&spec),
            foam::ddes_fv_solution(&spec, t),
        ),
        None => (
            foam::control_dict(&spec),
            foam::fv_schemes(&spec),
            foam::fv_solution(&spec),
        ),
    };
    write_setup_file(ctx, case, "system/controlDict", &control)?;
    write_setup_file(ctx, case, "system/fvSchemes", &schemes)?;
    write_setup_file(ctx, case, "system/fvSolution", &solution)?;
    write_setup_file(
        ctx,
        case,
        "system/decomposeParDict",
        &foam::decompose_par_dict(p.int("np")? as u32),
    )?;
    let topo = foam::topo_set_dict(&spec);
    if let Some(t) = &topo {
        write_setup_file(ctx, case, "system/topoSetDict", t)?;
    } else {
        remove_setup_file(ctx, case, "system/topoSetDict")?;
    }
    let porous = foam::porous_source(&spec);
    for path in ["constant/fvModels", "constant/fvOptions"] {
        if porous.is_none() || path != flavour.fv_models_path() {
            remove_setup_file(ctx, case, path)?;
        }
    }
    if let Some(t) = porous {
        write_setup_file(ctx, case, flavour.fv_models_path(), &t)?;
        report.note(Severity::Info, "porous source layout follows the published class description; confirm on newer flavours with a golden run");
    }
    if let Some(t) = foam::mrf_properties(&spec) {
        write_setup_file(ctx, case, "constant/MRFProperties", &t)?;
    } else {
        remove_setup_file(ctx, case, "constant/MRFProperties")?;
    }
    let np = p.int("np")? as u32;
    let steps = match &transient {
        Some(_) => crate::mesh::transient_steps(np),
        None => solve_steps(flavour, np, true, topo.is_some()),
    };
    write_setup_file(ctx, case, "Allrun.solve", &script_text(&steps))?;

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
        turbulence: format!(
            "{} I={:.3}% nut/nu={:.1}",
            if transient.is_some() {
                "kOmegaSSTDDES"
            } else {
                "kOmegaSST"
            },
            spec.i_pct,
            spec.nut_ratio
        ),
        separated_flow,
    };
    m["flavour"] = json!(flavour.name());
    m["np"] = json!(np);
    m["solver_mode"] = json!(p.str("solver_mode")?);
    m["transient"] = match &transient {
        Some(t) => json!({
            "application": "pimpleFoam", "turbulence_model": "kOmegaSSTDDES", "simulation_type": "LES",
            "delta": "maxDeltaxyz", "time_basis": "seconds", "end_time_s": t.end_time_s, "delta_t_s": t.delta_t_s,
            "max_delta_t_s": t.max_delta_t_s, "max_co": t.max_co, "write_interval_s": t.write_interval_s,
            "outer_correctors": t.outer_correctors, "correctors": t.correctors,
            "evidence": "written by the case writer; no accuracy validation",
        }),
        None => serde_json::Value::Null,
    };
    m["case_setup"] = json!({
        "u_ms": u, "nu": nu, "rho_inf": rho, "aref_m2": spec.aref(), "half_model": domain.half_model,
        "window": spec.window, "n_iter": spec.n_iter, "residual_target": spec.residual,
        "reference": rf.to_json(), "has_topo": topo.is_some(), "recipe": recipe,
    });
    write_setup_file(
        ctx,
        case,
        "vernon_case.json",
        &serde_json::to_string_pretty(&m)
            .map_err(|e| ctx.fail(format!("cannot encode case manifest: {e}")))?,
    )?;

    let binding = reference_diagnostic(
        (|| {
            let Some(source) = &reference_source else {
                return Ok(None);
            };
            if !reference_clone_matches || separated_flow {
                return Ok(None);
            }
            let Some(binding) = crate::reference::match_case_protocol(
                &source.geometry,
                &spec,
                p.str("solver_mode")?,
            ) else {
                return Ok(None);
            };
            let Some(upstream) = upstream.as_deref() else {
                return Ok(None);
            };
            if case_input_fingerprint(&native, upstream)? != source.fingerprint
                || read_mesh_evidence(&native, case)? != source.mesh
                || crate::reference::geometry_binding(&native, case)?.as_ref()
                    != Some(&source.geometry)
                || !reference_mesh_settings_match(&native, case, &m, &domain, &rf)?
            {
                return Ok(None);
            }
            Ok(Some(binding))
        })(),
        &mut report,
    )?;
    crate::reference_record::remove(&native, case)?;
    if let Some(binding) = &binding {
        let expected = case_input_fingerprint(&native, case)?;
        crate::reference_record::write(
            &native,
            case,
            crate::reference_record::Phase::Setup,
            binding,
            &expected,
        )?;
        report.note(Severity::Info, "prepared inputs match the documented published replication; this establishes neither convergence nor physical accuracy");
    }
    report.metric(
        "reference_inputs_bound",
        Metric::new(f64::from(binding.is_some()), "-"),
    );

    report
        .metric("k_inf", Metric::new(spec.k_inf(), "m2/s2"))
        .metric("omega_inf", Metric::new(spec.omega_inf(), "1/s"))
        .metric("u_ms", Metric::new(u, "m/s"))
        .metric("nu", Metric::new(nu, "m2/s"))
        .metric("rho_inf", Metric::new(rho, "kg/m3"))
        .metric("aref_m2", Metric::new(spec.aref(), "m2"))
        .metric(
            "porous_zones",
            Metric::new(
                zones.iter().filter(|z| z.bbox_m.is_some()).count() as f64,
                "count",
            ),
        );
    report.metric(
        "separated_flow_flagged",
        Metric::new(f64::from(separated_flow), "-"),
    );
    if let Some(t) = &transient {
        report
            .metric("end_time_s", Metric::new(t.end_time_s, "s"))
            .metric("delta_t_s", Metric::new(t.delta_t_s, "s"))
            .metric("max_delta_t_s", Metric::new(t.max_delta_t_s, "s"))
            .metric("max_co_requested", Metric::new(t.max_co, "-"));
        report.note(Severity::Warning, "transient detached-eddy case written, not run: the settings are starting values, no achieved Courant number, statistical duration or accuracy is established, and the steady run node refuses this case");
        if domain.half_model {
            report.note(Severity::Warning, "a symmetry plane suppresses asymmetric wake modes; use the full car for a detached-eddy run");
        }
    } else if separated_flow {
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
        report.note(
            Severity::Info,
            "full car: left and right wheel patches both rotate",
        );
    }
    Ok(vec![Data::File(stage.path), Data::report(report)])
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
        .param(ParamSpec::string("wsl_distribution", "Linux distribution", "").help("Linux subsystem runner only: the installed distribution to run in"))
        .param(ParamSpec::string("wsl_user", "Linux user", "").help("Linux subsystem runner only: the account that runs the tools"))
        .param(ParamSpec::string("wsl_environment_file", "Linux environment file", "").help("Linux subsystem runner only: absolute Linux path of the script that sets up the tools' environment"))
        .help("Runs the steady solver in the case directory and reads the force-coefficient, force and residual histories back as a time series. Without the solver it reads whatever results the case already holds.")
        .emulates("automotive wind-tunnel service: solve")
        .spec("docs/specs/automotive-cfd.md")
        .external("OpenFOAM")
}

fn solve_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let case = file_in(ins, 0)?.ok_or(CookError::MissingInput {
        index: 0,
        name: "case".into(),
    })?;
    if !case.join("system/controlDict").exists() {
        return Err(ctx.fail("case has no system/controlDict; connect the solver case writer"));
    }
    let man = manifest(&case);
    if man
        .as_ref()
        .is_some_and(|m| m["solver_mode"] == "transient_ddes")
    {
        return Err(ctx.fail("this case holds the transient detached-eddy recipe; the steady solver must not run it and its results are not steady histories"));
    }
    let flavour = man
        .as_ref()
        .and_then(|m| m["flavour"].as_str().and_then(Flavour::parse))
        .unwrap_or(Flavour::Foundation(11));
    let mut report = Report::new("Steady solve");
    let run = p.bool("run")?;
    let native = owned_context(ctx);
    let key = recipe_key("cfd.solve_steady/v2", ins, p)?;
    if run {
        let (published, series) = solve_requested(&native, &case, &key, flavour, p, &mut report)?;
        let certified = series.is_some();
        report_external_run(&mut report, true, certified);
        let mut series = series.unwrap_or_default();
        series.recipe = man
            .as_ref()
            .and_then(|m| serde_json::from_value(m["case_setup"]["recipe"].clone()).ok());
        report
            .metric("iterations_read", Metric::new(series.len() as f64, "count"))
            .metric(
                "solve_done",
                Metric::new(f64::from(certified), "-").with_limit(1.0, false),
            );
        return Ok(vec![
            Data::File(published),
            Data::json(series.to_json()),
            Data::report(report),
        ]);
    }
    // After a certified run switches `run` off again, its own results stay valid.
    if let Some(owned) = certified_owned(&native, &case, &key) {
        if let Ok(mut series) = load_case_series_checked(&owned, REQUIRED_RESIDUALS) {
            series.solve_evidence = measured_solve_evidence(&native, &owned).ok();
            series.recipe = man
                .as_ref()
                .and_then(|m| serde_json::from_value(m["case_setup"]["recipe"].clone()).ok());
            report_external_run(&mut report, false, false);
            report
                .metric("iterations_read", Metric::new(series.len() as f64, "count"))
                .metric("solve_done", Metric::new(1.0, "-").with_limit(1.0, false));
            return Ok(vec![
                Data::File(owned),
                Data::json(series.to_json()),
                Data::report(report),
            ]);
        }
    }
    let stamp = case_stamp(ctx, &case)?;
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
    series.recipe = man
        .as_ref()
        .and_then(|m| serde_json::from_value(m["case_setup"]["recipe"].clone()).ok());
    let done = !series.is_empty();
    if !done {
        placeholder(
            &mut report,
            "run the potential-flow initialisation and the steady solver, then read the force-coefficient and residual histories (run is off or the tools were not found)",
        );
    }
    report
        .metric("iterations_read", Metric::new(series.len() as f64, "count"))
        .metric(
            "solve_done",
            Metric::new(f64::from(done), "-").with_limit(1.0, false),
        );
    let _ = (mesh_steps, ctx);
    Ok(vec![
        Data::File(case),
        Data::json(series.to_json()),
        Data::report(report),
    ])
}

fn report_external_run(report: &mut Report, run: bool, succeeded: bool) {
    report
        .metric("external_run_requested", Metric::new(f64::from(run), "-"))
        .metric(
            "external_run_succeeded",
            Metric::new(f64::from(run && succeeded), "-"),
        )
        .metric(
            "external_run_failed",
            Metric::new(f64::from(run && !succeeded), "-"),
        );
}

/// Residuals of the k-omega SST case this node runs.
const REQUIRED_RESIDUALS: &[&str] = &["p", "U", "k", "omega"];

/// One requested solve in a new owned generation. Only a clean exit plus a
/// complete fresh history publishes the case; anything else publishes neither
/// the case nor the upstream's old results.
fn solve_requested(
    native: &CookContext,
    case: &std::path::Path,
    key: &str,
    flavour: Flavour,
    p: &Params,
    report: &mut Report,
) -> CookResult<(std::path::PathBuf, Option<crate::types::Series>)> {
    let refused = unavailable_output(native);
    let reference_source = reference_diagnostic(
        checked_reference_inputs(native, case, crate::reference_record::Phase::Setup),
        report,
    )?;
    let owned = match stage_case(native, case, CaseStageKind::SolverInputs, key, true) {
        Ok(stage) => stage.path,
        Err(e) => {
            report.metric("steps_completed", Metric::new(0.0, "count"));
            report.note(Severity::Error, format!("the case cannot be solved: {e}"));
            return Ok((refused, None));
        }
    };
    let reference_binding = match &reference_source {
        Some((binding, expected))
            if case_input_fingerprint(native, &owned)? == *expected
                && reference_extra_physics_absent(native, &owned)? =>
        {
            Some(binding.clone())
        }
        _ => None,
    };
    for field in ["U", "p", "k", "omega", "nut"] {
        let path = owned_file(native, &owned, &format!("0/{field}"))?;
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() > 0) {
            report.metric("steps_completed", Metric::new(0.0, "count"));
            report.note(Severity::Error, format!("the steady case needs a nonempty initial field 0/{field}; connect the solver case writer before running"));
            return Ok((refused, None));
        }
    }
    let np = p.int("np")? as u32;
    write_file(
        &owned.join("system/decomposeParDict"),
        &foam::decompose_par_dict(np),
    )?;
    // Capture AFTER this node's known decomposition rewrite, BEFORE external
    // programs. Never substitute an arbitrary final hash as reference proof.
    let reference_expected = reference_binding
        .map(|binding| {
            case_input_fingerprint(native, &owned).map(|fingerprint| (binding, fingerprint))
        })
        .transpose()?;
    // Zone construction may legitimately add cellZones, while the measured
    // five-table mesh identity must survive the solve unchanged.
    let mesh_before = read_mesh_evidence(native, &owned).ok();
    let steps = solve_steps(
        flavour,
        np,
        p.bool("potential_init")?,
        owned.join("system/topoSetDict").exists(),
    );
    let res = run_owned_chain(native, p, &owned, &steps)?;
    report.metric(
        "steps_completed",
        Metric::new(res.completed.len() as f64, "count"),
    );
    if let Some(f) = res.failed {
        report.note(Severity::Error, format!("solver run stopped: {f}"));
        return Ok((refused, None));
    }
    match load_case_series_checked(&owned, REQUIRED_RESIDUALS) {
        Ok(mut series) if !series.is_empty() => {
            let stamp = case_stamp(native, &owned)?;
            write_file(&owned.join(RESULTS_STAMP), &stamp)?;
            if let Some((binding, expected)) = reference_expected {
                reference_diagnostic(
                    (|| {
                        if stamp != expected {
                            return Err(native.fail("physical inputs changed during the solve; replication proof is withheld"));
                        }
                        crate::reference_record::write(
                            native,
                            &owned,
                            crate::reference_record::Phase::Solved,
                            &binding,
                            &expected,
                        )?;
                        Ok(Some(()))
                    })(),
                    report,
                )?;
            }
            series.solve_evidence = measured_solve_evidence(native, &owned)
                .ok()
                .filter(|e| Some(&e.mesh) == mesh_before.as_ref());
            if series.solve_evidence.is_none() {
                report.note(Severity::Warning, "the history is readable, but matching measured mesh evidence is absent; it cannot qualify a benchmark");
            }
            Ok((owned, Some(series)))
        }
        Ok(_) => {
            report.note(Severity::Error, "the solver wrote no history");
            Ok((refused, None))
        }
        Err(why) => {
            report.note(
                Severity::Error,
                format!("the solver exited cleanly but its results cannot be certified: {why}"),
            );
            Ok((refused, None))
        }
    }
}

/// The owned case of an earlier certified run of exactly these inputs.
fn certified_owned(
    native: &CookContext,
    case: &std::path::Path,
    key: &str,
) -> Option<std::path::PathBuf> {
    let owned = native.work_dir.join("case");
    let stamp = std::fs::read_to_string(owned.join(RESULTS_STAMP)).ok()?;
    let stage = stage_case(native, case, CaseStageKind::SolverInputs, key, false).ok()?;
    let same = stage.reused && case_stamp(native, &stage.path).ok()? == stamp.trim();
    same.then_some(stage.path)
}

/// File in a case recording which inputs its results were solved for.
const RESULTS_STAMP: &str = "vernon_results.stamp";

struct ReferenceMeshInput {
    fingerprint: String,
    geometry: crate::reference::GeometryBinding,
    mesh: crate::types::MeshEvidence,
}

fn reference_diagnostic<T>(
    result: CookResult<Option<T>>,
    report: &mut Report,
) -> CookResult<Option<T>> {
    if vernon_core::process::cancel_requested() {
        return Err(CookError::Other("reference qualification cancelled".into()));
    }
    match result {
        Ok(value) => Ok(value),
        Err(why) => {
            report.note(
                Severity::Warning,
                format!("reference inputs cannot be qualified: {why}"),
            );
            Ok(None)
        }
    }
}

fn reference_mesh_input(
    ctx: &CookContext,
    case: Option<&std::path::Path>,
    manifest: Option<&serde_json::Value>,
    domain: &Domain,
    reference: &RefGeom,
) -> CookResult<Option<ReferenceMeshInput>> {
    let (Some(case), Some(manifest)) = (case, manifest) else {
        return Ok(None);
    };
    let Some(fingerprint) = crate::mesh::certified_input_fingerprint(ctx, case)? else {
        return Ok(None);
    };
    if !reference_mesh_settings_match(ctx, case, manifest, domain, reference)? {
        return Ok(None);
    }
    let Some(geometry) = crate::reference::geometry_binding(ctx, case)? else {
        return Ok(None);
    };
    let mesh = read_mesh_evidence(ctx, case)?;
    Ok(Some(ReferenceMeshInput {
        fingerprint,
        geometry,
        mesh,
    }))
}

fn reference_extra_physics_absent(ctx: &CookContext, case: &std::path::Path) -> CookResult<bool> {
    for relative in [
        "constant/dynamicMeshDict",
        "constant/MRFProperties",
        "constant/fvOptions",
        "system/fvOptions",
        "constant/fvModels",
        "system/fvModels",
        "system/topoSetDict",
    ] {
        let path = owned_file(ctx, case, relative)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(ctx.fail(format!("cannot inspect reference physics: {e}"))),
        }
    }
    Ok(true)
}

/// The reference describes the domain and surfaces used for the actual mesh,
/// rather than a different JSON override supplied only to the case writer.
fn reference_mesh_settings_match(
    ctx: &CookContext,
    case: &std::path::Path,
    manifest: &serde_json::Value,
    domain: &Domain,
    reference: &RefGeom,
) -> CookResult<bool> {
    let (Ok(recorded_domain), Ok(recorded_reference)) = (
        Domain::from_json(&manifest["domain"]),
        RefGeom::from_json(&manifest["reference"]),
    ) else {
        return Ok(false);
    };
    if !reference_json_equivalent(&recorded_domain.to_json(), &domain.to_json())
        || !reference_json_equivalent(&recorded_reference.to_json(), &reference.to_json())
        || !reference_extra_physics_absent(ctx, case)?
    {
        return Ok(false);
    }
    let Some(dx) = manifest["dx0_m"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.0)
    else {
        return Ok(false);
    };
    let actual = crate::mesh::mesh_text(case, "system/blockMeshDict", 1024 * 1024)
        .map_err(|e| ctx.fail(e))?;
    Ok(actual == foam::block_mesh_dict(domain, dx)
        && crate::mesh::reference_meshing_recipe_matches(ctx, case, manifest)?)
}

/// JSON file roundtrips can change the last bit of an f64. Only numerical
/// identity gets a roundoff tolerance; keys, names, flags and shapes stay exact.
fn reference_json_equivalent(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => match (a.as_f64(), b.as_f64()) {
            (Some(a), Some(b)) => {
                a.is_finite()
                    && b.is_finite()
                    && (a - b).abs() <= 1e-12 + 1e-9 * a.abs().max(b.abs())
            }
            _ => false,
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(a, b)| reference_json_equivalent(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, a)| b.get(key).is_some_and(|b| reference_json_equivalent(a, b)))
        }
        _ => a == b,
    }
}

/// Admission combines independently checked input, phase, geometry and profile.
/// A readable sidecar or a legacy scope label cannot provide these verdicts.
fn admit_reference_record(
    record: &crate::reference_record::Record,
    geometry: &crate::reference::GeometryBinding,
    phase: crate::reference_record::Phase,
    supported: bool,
) -> Option<crate::reference::ReferenceBinding> {
    (record.version == 1
        && record.phase == phase
        && supported
        && record.binding.geometry == *geometry)
        .then(|| record.binding.clone())
}

fn checked_reference_inputs(
    ctx: &CookContext,
    case: &std::path::Path,
    phase: crate::reference_record::Phase,
) -> CookResult<Option<(crate::reference::ReferenceBinding, String)>> {
    let Some(record) = crate::reference_record::read(ctx, case, phase)? else {
        return Ok(None);
    };
    if !crate::reference::binding_supported(&record.binding)
        || !reference_extra_physics_absent(ctx, case)?
    {
        return Ok(None);
    }
    let Some(geometry) = crate::reference::geometry_binding(ctx, case)? else {
        return Ok(None);
    };
    Ok(admit_reference_record(&record, &geometry, phase, true)
        .map(|binding| (binding, record.input_fingerprint)))
}

/// Only called for this node's successful or certified cached owned solve.
/// A readable imported history never acquires proof from its filename or preset.
fn measured_solve_evidence(ctx: &CookContext, case: &std::path::Path) -> CookResult<SolveEvidence> {
    let mesh = read_mesh_evidence(ctx, case)?;
    let input_fingerprint = case_stamp(ctx, case)?;
    let recorded = std::fs::read_to_string(case.join(RESULTS_STAMP))
        .map_err(|e| ctx.fail(format!("the solve has no input stamp: {e}")))?;
    if recorded.trim() != input_fingerprint {
        return Err(ctx.fail("the solve stamp does not match current inputs"));
    }
    let reference_binding =
        checked_reference_inputs(ctx, case, crate::reference_record::Phase::Solved)?
            .map(|(binding, _)| binding);
    Ok(SolveEvidence {
        version: 1,
        mesh,
        input_fingerprint,
        reference_scope: reference_binding
            .as_ref()
            .map(|binding| binding.profile.clone()),
        reference_binding,
    })
}

/// Bounded, fail-closed provenance of all selected immutable case inputs.
/// This includes the complete physical settings, mesh and 0.orig snapshot;
/// current working fields and result histories are excluded when 0.orig exists.
fn case_stamp(ctx: &CookContext, case: &std::path::Path) -> CookResult<String> {
    case_input_fingerprint(ctx, case)
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

    fn setup(
        ins: &[Option<Data>],
        set: &[(&str, ParamValue)],
        name: &str,
    ) -> (std::path::PathBuf, Report) {
        let mut p = Params::from_specs(&setup_info().params);
        for (k, v) in set {
            p.set(k, v.clone());
        }
        let out = setup_cook(&ctx(name), ins, &p).unwrap();
        (
            out[0].as_file().unwrap().clone(),
            out[1].as_report().unwrap().as_ref().clone(),
        )
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

    fn ddes(extra: &[(&str, ParamValue)], name: &str) -> (std::path::PathBuf, Report) {
        let mut set = vec![
            ("flavour", ParamValue::Str("opencfd2606".into())),
            ("solver_mode", ParamValue::Str("transient_ddes".into())),
        ];
        set.extend(extra.iter().cloned());
        setup(&[], &set, name)
    }

    #[test]
    fn default_recipe_stays_steady() {
        let (case, _) = setup(
            &[],
            &[("flavour", ParamValue::Str("opencfd2606".into()))],
            "mode-default",
        );
        let turb = read(&case, "constant/turbulenceProperties");
        assert!(
            turb.contains("simulationType  RAS")
                && turb.contains("kOmegaSST;")
                && !turb.contains("DDES")
        );
        assert!(read(&case, "system/controlDict").contains("application     simpleFoam"));
        assert!(
            read(&case, "system/fvSolution").contains("SIMPLE")
                && !read(&case, "system/fvSolution").contains("PIMPLE")
        );
        assert!(read(&case, "system/fvSchemes").contains("steadyState"));
        assert!(
            read(&case, "Allrun.solve").contains("simpleFoam")
                && !read(&case, "Allrun.solve").contains("pimpleFoam")
        );
    }

    #[test]
    fn steady_recipe_limits_the_velocity_and_turbulence_gradients() {
        // WP9 diagnostic D5 (2026-11-09): unlimited grad(k)/grad(omega) on the
        // rough near-wall layer cells drove most of the k/omega clipping.
        let (case, _) = setup(
            &[],
            &[("flavour", ParamValue::Str("opencfd2606".into()))],
            "steady-gradients",
        );
        let schemes = read(&case, "system/fvSchemes");
        assert!(schemes.contains(
            "gradSchemes\n{\n    default         Gauss linear;\n    grad(U)         cellLimited Gauss linear 1;\n    grad(k)         cellLimited Gauss linear 1;\n    grad(omega)     cellLimited Gauss linear 1;\n}\n"
        ), "{schemes}");
        // The convection schemes are unchanged.
        assert!(schemes.contains("    div(phi,k)      bounded Gauss limitedLinear 1;\n    div(phi,omega)  bounded Gauss limitedLinear 1;\n"));
    }

    #[test]
    fn transient_recipe_writes_ddes_and_maps_its_settings_exactly() {
        let (case, r) = ddes(
            &[
                ("end_time_s", ParamValue::Float(1.5)),
                ("delta_t_s", ParamValue::Float(2e-4)),
                ("max_delta_t_s", ParamValue::Float(4e-4)),
                ("max_co", ParamValue::Float(0.8)),
                ("write_interval_s", ParamValue::Float(0.25)),
                ("outer_correctors", ParamValue::Int(3)),
                ("correctors", ParamValue::Int(4)),
                // Steady limits must neither apply to this recipe nor reach its files.
                ("n_iter", ParamValue::Int(500)),
                ("window", ParamValue::Int(2000)),
            ],
            "ddes-map",
        );
        let turb = read(&case, "constant/turbulenceProperties");
        assert!(
            turb.contains("simulationType  LES")
                && turb.contains("LESModel        kOmegaSSTDDES;")
                && turb.contains("delta           maxDeltaxyz;")
        );
        assert!(!turb.contains("RAS") && !turb.to_ascii_lowercase().contains("sbes"));
        let control = read(&case, "system/controlDict");
        for want in [
            "application     pimpleFoam;",
            "endTime         1.5;",
            "deltaT          0.0002;",
            "maxCo           0.8;",
            "maxDeltaT       0.0004;",
            "writeInterval   0.25;",
            "adjustTimeStep  yes;",
            "writeControl    adjustableRunTime;",
            "forceCoeffs1",
        ] {
            assert!(control.contains(want), "{want} missing from\n{control}");
        }
        let solution = read(&case, "system/fvSolution");
        assert!(
            solution.contains("PIMPLE")
                && !solution.contains("SIMPLE")
                && !solution.contains("relaxationFactors")
        );
        assert!(solution.contains("pFinal") && solution.contains("(U|k|omega)Final"));
        assert!(
            solution.contains("nOuterCorrectors 3;") && solution.contains("nCorrectors     4;")
        );
        let schemes = read(&case, "system/fvSchemes");
        assert!(schemes.contains("default         backward;") && !schemes.contains("steadyState"));
        let script = read(&case, "Allrun.solve");
        assert!(
            script.contains("pimpleFoam")
                && !script.contains("simpleFoam")
                && !script.contains("potentialFoam")
        );
        for f in [
            "0/U",
            "0/p",
            "0/k",
            "0/omega",
            "0/nut",
            "0.orig/U",
            "0.orig/nut",
        ] {
            assert!(!read(&case, f).is_empty(), "{f}");
        }
        for f in [
            "constant/turbulenceProperties",
            "system/controlDict",
            "system/fvSchemes",
            "system/fvSolution",
        ] {
            foam::check_syntax(&read(&case, f)).unwrap_or_else(|e| panic!("{f}: {e}"));
        }
        let m = manifest(&case).unwrap();
        assert_eq!(m["solver_mode"], "transient_ddes");
        assert_eq!(m["transient"]["end_time_s"], 1.5);
        assert_eq!(r.metrics["max_co_requested"].value, 0.8);
        assert!(
            r.notes
                .iter()
                .any(|(s, t)| *s == Severity::Warning && t.contains("not run")),
            "{r:?}"
        );
        assert!(!r.metrics.contains_key("solve_done"));
    }

    #[test]
    fn transient_recipe_rejects_bad_settings_before_writing_anything() {
        let bad: [(&str, &[(&str, ParamValue)]); 7] = [
            ("zero end", &[("end_time_s", ParamValue::Float(0.0))]),
            (
                "infinite co",
                &[("max_co", ParamValue::Float(f64::INFINITY))],
            ),
            ("nan step", &[("delta_t_s", ParamValue::Float(f64::NAN))]),
            (
                "step above max",
                &[
                    ("delta_t_s", ParamValue::Float(0.01)),
                    ("max_delta_t_s", ParamValue::Float(0.001)),
                ],
            ),
            (
                "max above end",
                &[
                    ("end_time_s", ParamValue::Float(0.1)),
                    ("max_delta_t_s", ParamValue::Float(0.5)),
                    ("delta_t_s", ParamValue::Float(0.1)),
                    ("write_interval_s", ParamValue::Float(0.05)),
                ],
            ),
            (
                "interval above end",
                &[
                    ("end_time_s", ParamValue::Float(0.1)),
                    ("write_interval_s", ParamValue::Float(0.5)),
                ],
            ),
            ("no correctors", &[("correctors", ParamValue::Int(0))]),
        ];
        for (why, extra) in bad {
            let c = ctx("ddes-bad");
            let mut p = Params::from_specs(&setup_info().params);
            p.set("flavour", ParamValue::Str("opencfd2606".into()));
            p.set("solver_mode", ParamValue::Str("transient_ddes".into()));
            for (k, v) in extra {
                p.set(k, v.clone());
            }
            assert!(setup_cook(&c, &[], &p).is_err(), "{why} must be refused");
            assert!(
                !c.work_dir.join("native").join("case").exists(),
                "{why} left a case behind"
            );
        }
        // Only the release it was run on.
        let mut p = Params::from_specs(&setup_info().params);
        p.set("flavour", ParamValue::Str("foundation13".into()));
        p.set("solver_mode", ParamValue::Str("transient_ddes".into()));
        assert!(setup_cook(&ctx("ddes-flavour"), &[], &p).is_err());
    }

    #[test]
    fn steady_solver_refuses_the_transient_recipe_without_starting_anything() {
        let (case, _) = ddes(&[], "ddes-refuse");
        let c = ctx("ddes-solve");
        let mut p = Params::from_specs(&solve_info().params);
        p.set("run", ParamValue::Bool(true));
        p.set(
            "bin_dir",
            ParamValue::Str(c.work_dir.join("nowhere").display().to_string()),
        );
        let e = solve_cook(&c, &[Some(Data::File(case))], &p).unwrap_err();
        assert!(e.to_string().contains("transient"), "{e}");
        assert!(!c.work_dir.join("native").exists(), "nothing may be staged");
    }

    #[test]
    fn switching_recipe_leaves_nothing_of_the_other_one() {
        let c = ctx("ddes-switch");
        let mut p = Params::from_specs(&setup_info().params);
        p.set("flavour", ParamValue::Str("opencfd2606".into()));
        p.set("solver_mode", ParamValue::Str("transient_ddes".into()));
        let t = setup_cook(&c, &[], &p).unwrap()[0]
            .as_file()
            .unwrap()
            .clone();
        assert!(read(&t, "system/fvSolution").contains("PIMPLE"));
        p.set("solver_mode", ParamValue::Str("steady_sst".into()));
        let s = setup_cook(&c, &[], &p).unwrap()[0]
            .as_file()
            .unwrap()
            .clone();
        assert!(
            read(&s, "system/fvSolution").contains("SIMPLE")
                && !read(&s, "system/fvSolution").contains("PIMPLE")
        );
        assert!(
            read(&s, "constant/turbulenceProperties").contains("RAS")
                && !read(&s, "Allrun.solve").contains("pimpleFoam")
        );
        assert_eq!(manifest(&s).unwrap()["solver_mode"], "steady_sst");
        assert!(manifest(&s).unwrap()["transient"].is_null());
    }

    #[test]
    fn separated_flow_flag_warns_and_is_recorded_in_the_recipe() {
        let (case, r) = setup(
            &[],
            &[("separated_flow", ParamValue::Bool(true))],
            "sepflow-on",
        );
        assert_eq!(r.metrics["separated_flow_flagged"].value, 1.0);
        assert!(
            r.notes.iter().any(|(s, m)| *s == Severity::Warning
                && m.contains("separated-flow features are flagged")),
            "{r:?}"
        );
        let man: serde_json::Value =
            serde_json::from_str(&read(&case, "vernon_case.json")).unwrap();
        assert_eq!(man["case_setup"]["recipe"]["separated_flow"], true);
        let (case, r) = setup(&[], &[], "sepflow-off");
        assert_eq!(r.metrics["separated_flow_flagged"].value, 0.0);
        assert!(!r
            .notes
            .iter()
            .any(|(_, m)| m.contains("features are flagged")));
        let man: serde_json::Value =
            serde_json::from_str(&read(&case, "vernon_case.json")).unwrap();
        assert_eq!(man["case_setup"]["recipe"]["separated_flow"], false);
    }

    #[test]
    fn every_file_parses_for_every_flavour() {
        for fl in FLAVOURS {
            let (case, _) = setup(
                &[],
                &[("flavour", ParamValue::Str((*fl).into()))],
                &format!("fl-{fl}"),
            );
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
                foam::check_syntax(&read(&case, &format!("constant/{rel}")))
                    .unwrap_or_else(|e| panic!("{fl} {rel}: {e}"));
            }
            let ctl = read(&case, "system/controlDict");
            assert_eq!(ctl.contains("foamRun"), f.uses_foam_run(), "{fl}");
            assert!(
                ctl.contains("libs            (forces)")
                    || ctl.contains("libs            (\"libforces.so\")"),
                "{fl}"
            );
        }
    }

    #[test]
    fn boundary_conditions_follow_the_table() {
        let (case, _) = setup(&[], &[], "bc");
        let u = read(&case, "0/U");
        assert!(u.contains("type fixedValue;\n        value uniform (30 0 0);"));
        assert!(u.contains("inletOutlet"));
        assert!(u.contains("symmetryPlane"));
        assert!(u.contains("\"wheel_fr.*\""));
        assert!(!u.contains("\"wheel_fl.*\""));
        assert!(u.contains("rotatingWallVelocity"));
        assert!(u.contains("origin (0.92 0.785 0.3137)"), "{u}");
        let omega = (30.0f64 / 0.3137).round();
        assert!(
            u.contains(&format!("omega {}", -omega)) || u.contains("omega -95.6"),
            "{u}"
        );
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
        let dom = crate::tunnel::size_domain(
            &RefGeom::reference_car(),
            1.0,
            3.0,
            6.0,
            5.0 / 3.0,
            false,
            0.0,
        );
        let (case, r) = setup(
            &[
                None,
                None,
                None,
                None,
                None,
                Some(Data::json(dom.to_json())),
            ],
            &[],
            "fc-full",
        );
        assert!((r.metrics["aref_m2"].value - 2.16).abs() < 1e-9);
        assert!(!read(&case, "0/U").contains("symmetryPlane"));
        assert!(read(&case, "0/U").contains("\"wheel_fl.*\""), "a full domain needs both left wheel boundaries even when the default reference was half-model metadata");
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
        let (case, _) = setup(
            &[None, None, None, Some(Data::json(porous))],
            &[("flavour", ParamValue::Str("opencfd2312".into()))],
            "porous2",
        );
        assert!(case.join("constant/fvOptions").exists());
        // a zone without volume is skipped with a warning
        let mut gp = Params::from_specs(&crate::tunnel::gw_info().params);
        gp.set("wheel_mode", ParamValue::Str("mrf".into()));
        let bc = crate::tunnel::gw_cook(&ctx("gw"), &[], &gp).unwrap();
        let (case, _) = setup(&[None, None, Some(bc[0].clone())], &[], "mrf");
        let mrf = read(&case, "constant/MRFProperties");
        foam::check_syntax(&mrf).unwrap();
        assert!(mrf.contains("MRF_wheel_fr"));
        assert!(!mrf.contains("MRF_wheel_fl"));
        let topo = read(&case, "system/topoSetDict");
        foam::check_syntax(&topo).unwrap();
        assert!(topo.contains("cylinderToCell"));
        assert!(!read(&case, "0/U").contains("rotatingWallVelocity"));
    }

    #[test]
    fn writing_is_deterministic_and_consumer_owned() {
        let (case, _) = setup(&[], &[], "det");
        let a = read(&case, "system/fvSchemes");
        let out = setup_cook(
            &ctx("det2"),
            &[Some(Data::File(case.clone()))],
            &Params::from_specs(&setup_info().params),
        )
        .unwrap();
        assert_ne!(out[0].as_file().unwrap(), &case);
        assert_eq!(a, read(out[0].as_file().unwrap(), "system/fvSchemes"));
        assert_eq!(a, read(&case, "system/fvSchemes"));
        let m = manifest(&case).unwrap();
        assert_eq!(m["case_setup"]["aref_m2"], 1.08);
    }

    #[test]
    fn wp9_setup_owns_output_without_mutating_upstream() {
        let (source, _) = setup(&[], &[], "wp9-setup-source");
        let original = read(&source, "system/controlDict");
        let original_u = read(&source, "0.orig/U");
        // A case setup operation must preserve a supplied mesh without requiring
        // one: users can connect this writer before or after volume meshing.
        std::fs::create_dir_all(source.join("constant/polyMesh")).unwrap();
        std::fs::write(
            source.join("constant/polyMesh/points"),
            "owned upstream mesh",
        )
        .unwrap();
        let consumer = ctx("wp9-setup-consumer");
        let mut p = Params::from_specs(&setup_info().params);
        p.set("u_ms", ParamValue::Float(42.0));
        let output = setup_cook(&consumer, &[Some(Data::File(source.clone()))], &p).unwrap();
        let case = output[0].as_file().unwrap();
        assert!(
            case.starts_with(&consumer.work_dir),
            "output escaped the node work directory: {case:?}"
        );
        assert_ne!(case, &source);
        assert_eq!(original, read(&source, "system/controlDict"));
        assert_eq!(original_u, read(&source, "0.orig/U"));
        assert_eq!(
            read(case, "constant/polyMesh/points"),
            "owned upstream mesh"
        );
        assert!(read(case, "0/U").contains("42"));
    }

    #[test]
    fn wp9_setup_replaces_disabled_optional_physics_and_flavour_files() {
        let context = ctx("wp9-setup-changing-recipe");
        let p = Params::from_specs(&setup_info().params);
        let mut gp = Params::from_specs(&crate::tunnel::gw_info().params);
        gp.set("wheel_mode", ParamValue::Str("mrf".into()));
        let boundary = crate::tunnel::gw_cook(&ctx("wp9-setup-mrf-bc"), &[], &gp).unwrap();
        let inputs = [
            None,
            None,
            Some(boundary[0].clone()),
            Some(Data::json(crate::tunnel::porous_cook_for_case_tests())),
        ];
        let initial = setup_cook(&ctx("wp9-setup-old-physics-source"), &inputs, &p).unwrap();
        let first = initial[0].as_file().unwrap();
        assert!(first.join("constant/MRFProperties").is_file());
        assert!(first.join("constant/fvModels").is_file());
        assert!(first.join("system/topoSetDict").is_file());
        let mut changed = p.clone();
        changed.set("flavour", ParamValue::Str("opencfd2312".into()));
        changed.set("np", ParamValue::Int(3));
        let output = setup_cook(&context, &[Some(initial[0].clone())], &changed).unwrap();
        let current = output[0].as_file().unwrap();
        assert!(
            !current.join("constant/MRFProperties").exists(),
            "disabled rotating-frame source remained active"
        );
        assert!(
            !current.join("constant/fvModels").exists(),
            "disabled porous source remained active"
        );
        assert!(
            !current.join("system/topoSetDict").exists(),
            "disabled zone selection remained active"
        );
        assert!(
            !current.join("constant/physicalProperties").exists(),
            "previous flavour properties survived"
        );
        assert!(current.join("constant/transportProperties").is_file());
        assert!(read(current, "system/decomposeParDict").contains("numberOfSubdomains 3;"));
        assert!(
            first.join("constant/MRFProperties").is_file(),
            "upstream source was changed"
        );
    }

    #[test]
    fn wp9_setup_preserves_legacy_unmarked_output() {
        let context = ctx("wp9-setup-legacy-output");
        let legacy = context.work_dir.join("case");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("user-note.txt"), "preserve this legacy folder").unwrap();
        let output = setup_cook(&context, &[], &Params::from_specs(&setup_info().params)).unwrap();
        assert_ne!(
            output[0].as_file().unwrap(),
            &legacy,
            "must use a new owned namespace rather than adopt legacy output"
        );
        assert_eq!(
            read(&legacy, "user-note.txt"),
            "preserve this legacy folder"
        );
    }

    #[test]
    fn wp9_solve_provenance_covers_physics_mesh_and_initial_snapshot() {
        let (case, _) = setup(&[], &[], "wp9-solve-stamp");
        let context = CookContext {
            node_name: "provenance-only".into(),
            work_dir: case.join("must-stay-missing"),
            frame: 0.0,
        };
        for relative in [
            "constant/physicalProperties",
            "constant/MRFProperties",
            "constant/polyMesh/points",
            "0.orig/U",
        ] {
            let before = case_stamp(&context, &case).unwrap();
            let path = case.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut contents = std::fs::read_to_string(&path).unwrap_or_default();
            contents.push_str("\n// changed immutable solver input\n");
            std::fs::write(&path, contents).unwrap();
            assert_ne!(
                before,
                case_stamp(&context, &case).unwrap(),
                "result provenance ignored {relative}"
            );
        }
        let before = case_stamp(&context, &case).unwrap();
        std::fs::write(case.join("0/U"), "solver-modified working initial field").unwrap();
        std::fs::create_dir_all(case.join("postProcessing/solverInfo1/0")).unwrap();
        std::fs::write(
            case.join("postProcessing/solverInfo1/0/solverInfo.dat"),
            "new solver output",
        )
        .unwrap();
        assert_eq!(
            before,
            case_stamp(&context, &case).unwrap(),
            "mutable outputs changed immutable-input provenance"
        );
        assert!(
            !context.work_dir.exists(),
            "read-only provenance created a work directory"
        );
    }

    #[test]
    fn wp9_driver_action_reset_preserves_input_recipe() {
        let mut p = Params::from_specs(&solve_info().params);
        let off = recipe_key("steady-inputs/v2", &[], &p).unwrap();
        p.set("run", ParamValue::Bool(true));
        assert_eq!(
            off,
            recipe_key("steady-inputs/v2", &[], &p).unwrap(),
            "resetting the one-shot action invalidates a completed result"
        );
        p.set("np", ParamValue::Int(3));
        assert_ne!(
            off,
            recipe_key("steady-inputs/v2", &[], &p).unwrap(),
            "actual file-affecting inputs must remain in the recipe"
        );
    }

    #[test]
    fn wp9_failed_solve_does_not_publish_old_history_or_upstream_case() {
        let (source, _) = setup(&[], &[], "wp9-solve-failed-source");
        std::fs::create_dir_all(source.join("constant/polyMesh")).unwrap();
        // Deliberately opaque unit fixtures: no CFD program will be found/run.
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            std::fs::write(
                source.join("constant/polyMesh").join(name),
                "unit fixture mesh input",
            )
            .unwrap();
        }
        let coefficients = source.join("postProcessing/forceCoeffs1/0");
        std::fs::create_dir_all(&coefficients).unwrap();
        std::fs::write(
            coefficients.join("coefficient.dat"),
            "# Time Cd Cl\n1 0.3 -0.1\n2 0.3 -0.1\n",
        )
        .unwrap();
        assert_eq!(
            load_case_series(&source).len(),
            2,
            "the stale-results fixture must really load"
        );
        let original_control = read(&source, "system/controlDict");
        let original_decomposition = read(&source, "system/decomposeParDict");
        let mut p = Params::from_specs(&solve_info().params);
        p.set("run", ParamValue::Bool(true));
        p.set("np", ParamValue::Int(1));
        p.set("potential_init", ParamValue::Bool(false));
        p.set(
            "bin_dir",
            ParamValue::Str(source.join("tool-does-not-exist").display().to_string()),
        );
        let out = solve_cook(
            &ctx("wp9-solve-failed-consumer"),
            &[Some(Data::File(source.clone()))],
            &p,
        )
        .unwrap();
        let history = crate::types::Series::from_json(out[1].as_json().unwrap()).unwrap();
        assert!(
            history.is_empty(),
            "a failed requested solve published old upstream coefficients"
        );
        assert!(
            !out[0].as_file().unwrap().exists(),
            "failed result must not publish a usable case File"
        );
        assert!(!out[2].as_report().unwrap().passes());
        assert_eq!(read(&source, "system/controlDict"), original_control);
        assert_eq!(
            read(&source, "system/decomposeParDict"),
            original_decomposition
        );
        assert!(
            !source.join("logs").exists(),
            "upstream case received execution logs"
        );
        assert!(!source.join(RESULTS_STAMP).exists());
    }

    #[test]
    fn wp9_solve_exit_zero_without_fresh_results_cannot_certify_old_data() {
        let (source, _) = setup(&[], &[], "wp9-solve-zero-output-source");
        std::fs::create_dir_all(source.join("constant/polyMesh")).unwrap();
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            std::fs::write(
                source.join("constant/polyMesh").join(name),
                "unit fixture mesh input",
            )
            .unwrap();
        }
        let coefficients = source.join("postProcessing/forceCoeffs1/0");
        std::fs::create_dir_all(&coefficients).unwrap();
        std::fs::write(
            coefficients.join("coefficient.dat"),
            "# Time Cd Cl\n1 0.3 -0.1\n2 0.3 -0.1\n",
        )
        .unwrap();
        assert_eq!(load_case_series(&source).len(), 2);
        let tools = ctx("wp9-solve-zero-output-fixture").work_dir;
        #[cfg(windows)]
        std::fs::write(
            tools.join("simpleFoam.cmd"),
            "@echo off\r\necho End\r\nexit /b 0\r\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tool = tools.join("simpleFoam");
            std::fs::write(&tool, "#!/bin/sh\nprintf 'End\\n'\nexit 0\n").unwrap();
            std::fs::set_permissions(tool, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut p = Params::from_specs(&solve_info().params);
        p.set("run", ParamValue::Bool(true));
        p.set("np", ParamValue::Int(1));
        p.set("potential_init", ParamValue::Bool(false));
        p.set("bin_dir", ParamValue::Str(tools.display().to_string()));
        let out = solve_cook(
            &ctx("wp9-solve-zero-output-consumer"),
            &[Some(Data::File(source.clone()))],
            &p,
        )
        .unwrap();
        let report = out[2].as_report().unwrap();
        assert_eq!(
            report.metrics["steps_completed"].value, 1.0,
            "negative fixture must really run and exit zero"
        );
        assert!(
            crate::types::Series::from_json(out[1].as_json().unwrap())
                .unwrap()
                .is_empty(),
            "zero-output process certified old coefficients"
        );
        assert!(!report.passes());
        assert!(!out[0].as_file().unwrap().exists());
        assert!(
            !source.join(RESULTS_STAMP).exists(),
            "old input was falsely stamped as freshly solved"
        );
    }

    #[test]
    fn rejects_window_longer_than_the_run() {
        let mut p = Params::from_specs(&setup_info().params);
        p.set("n_iter", ParamValue::Int(500));
        p.set("window", ParamValue::Int(600));
        assert!(setup_cook(&ctx("win"), &[], &p).is_err());
    }

    // These opaque tools exercise provenance plumbing only, never CFD accuracy.
    fn evidence_solve_fixture(measured: bool) -> (CookContext, std::path::PathBuf, Params) {
        let (source, _) = setup(&[], &[], "solve-evidence-source");
        std::fs::create_dir_all(source.join("constant/polyMesh")).unwrap();
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            std::fs::write(
                source.join("constant/polyMesh").join(name),
                "opaque unit table\n",
            )
            .unwrap();
        }
        if measured {
            let owner = CookContext {
                work_dir: source.parent().unwrap().to_path_buf(),
                ..ctx("evidence-owner")
            };
            crate::util::write_mesh_evidence(&owner, &source, 320, 0.0, 0.0).unwrap();
        }
        std::fs::write(source.join("system/topoSetDict"), "opaque zone input\n").unwrap();
        let context = ctx("solve-evidence-consumer");
        let tools = context.work_dir.join("unit-tools");
        std::fs::create_dir_all(&tools).unwrap();
        let mut log = String::new();
        for time in 1..=4 {
            log.push_str(&format!("Time = {time}\n"));
            for field in ["p", "U", "k", "omega"] {
                log.push_str(&format!("smoothSolver: Solving for {field}, Initial residual = 1e-7, Final residual = 1e-8, No Iterations 1\n"));
            }
        }
        log.push_str("End\n");
        for name in ["topoSet", "simpleFoam"] {
            let text = if cfg!(windows) {
                if name == "topoSet" {
                    "@echo off\r\necho opaque generated zone > constant\\polyMesh\\cellZones\r\nexit /b 0\r\n".to_string()
                } else {
                    let mut text = "@echo off\r\nmkdir postProcessing\\forceCoeffs1\\0\r\n(echo # Time Cd Cl&echo 1 0.3 -0.1&echo 2 0.3 -0.1&echo 3 0.3 -0.1&echo 4 0.3 -0.1) > postProcessing\\forceCoeffs1\\0\\coefficient.dat\r\n".to_string();
                    for line in log.lines() {
                        text.push_str(&format!("echo {line}\r\n"));
                    }
                    text.push_str("exit /b 0\r\n");
                    text
                }
            } else if name == "topoSet" {
                "#!/bin/sh\nset -e\nprintf 'opaque generated zone\\n' > constant/polyMesh/cellZones\n".to_string()
            } else {
                format!("#!/bin/sh\nset -e\nmkdir -p postProcessing/forceCoeffs1/0\ncat > postProcessing/forceCoeffs1/0/coefficient.dat <<'DATA'\n# Time Cd Cl\n1 0.3 -0.1\n2 0.3 -0.1\n3 0.3 -0.1\n4 0.3 -0.1\nDATA\ncat <<'LOG'\n{log}LOG\n")
            };
            let path = tools.join(if cfg!(windows) {
                format!("{name}.cmd")
            } else {
                name.into()
            });
            std::fs::write(&path, text).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let mut p = Params::from_specs(&solve_info().params);
        p.set("run", ParamValue::Bool(true));
        p.set("np", ParamValue::Int(1));
        p.set("potential_init", ParamValue::Bool(false));
        p.set("bin_dir", ParamValue::Str(tools.display().to_string()));
        (context, source, p)
    }

    #[test]
    fn wp9_solve_evidence_is_bound_to_final_inputs_and_survives_action_reset() {
        let (context, source, mut p) = evidence_solve_fixture(true);
        let inputs = [Some(Data::File(source.clone()))];
        let out = solve_cook(&context, &inputs, &p).unwrap();
        assert!(out[2].as_report().unwrap().passes(), "{:?}", out[2]);
        let owned = out[0].as_file().unwrap();
        let s = crate::types::Series::from_json(out[1].as_json().unwrap()).unwrap();
        assert_eq!(s.len(), 4);
        let evidence = s
            .solve_evidence
            .as_ref()
            .expect("fresh successful measured solve lacks provenance");
        assert_eq!(evidence.mesh.cells, 320);
        assert_eq!(evidence.version, 1);
        assert!(
            evidence.reference_scope.is_none(),
            "a selected case cannot establish reference identity"
        );
        assert_eq!(
            evidence.input_fingerprint,
            case_stamp(&owned_context(&context), owned).unwrap()
        );
        assert_eq!(
            read(owned, RESULTS_STAMP).trim(),
            evidence.input_fingerprint
        );
        assert!(owned.join("constant/polyMesh/cellZones").exists());
        assert!(!source.join("constant/polyMesh/cellZones").exists());
        p.set("run", ParamValue::Bool(false));
        let cached = solve_cook(&context, &inputs, &p).unwrap();
        let cached_series = crate::types::Series::from_json(cached[1].as_json().unwrap()).unwrap();
        assert_eq!(cached_series.solve_evidence, s.solve_evidence);
        assert_eq!(cached[0].as_file(), Some(owned));
        std::fs::write(
            owned.join("constant/polyMesh/points"),
            "changed base table\n",
        )
        .unwrap();
        let invalid = solve_cook(&context, &inputs, &p).unwrap();
        assert!(
            crate::types::Series::from_json(invalid[1].as_json().unwrap())
                .unwrap()
                .solve_evidence
                .is_none()
        );
    }

    #[test]
    fn wp9_unmeasured_solve_stays_readable_without_qualification() {
        let (context, source, p) = evidence_solve_fixture(false);
        let out = solve_cook(&context, &[Some(Data::File(source))], &p).unwrap();
        assert!(out[2].as_report().unwrap().passes());
        let s = crate::types::Series::from_json(out[1].as_json().unwrap()).unwrap();
        assert_eq!(s.len(), 4);
        assert!(s.solve_evidence.is_none());
    }

    // Only dictionary generation: no CAD asset, physical mesh or external tool.
    fn reference_mesh_dictionary_fixture() -> (
        CookContext,
        std::path::PathBuf,
        serde_json::Value,
        Domain,
        RefGeom,
    ) {
        let context = ctx("reference-mesh-settings");
        let reference = RefGeom::reference_car();
        let domain = crate::tunnel::size_domain(&reference, 1.0, 3.0, 6.0, 5.0 / 3.0, true, 0.0);
        let plan_params = Params::from_specs(&crate::tunnel::plan_info().params);
        let plan = crate::tunnel::plan_cook(
            &context,
            &[
                Some(Data::json(reference.to_json())),
                Some(Data::json(domain.to_json())),
            ],
            &plan_params,
        )
        .unwrap();
        let mut plan = plan[0].as_json().unwrap().as_ref().clone();
        plan["patches"] = json!(["body", "wheel_fr", "wheel_rr"]);
        let mut registry = Registry::new();
        crate::mesh::register(&mut registry);
        let op = registry.get("cfd.mesh_setup").unwrap();
        let mut params = Params::from_specs(&op.info().params);
        params.set("flavour", ParamValue::Str("opencfd2606".into()));
        let result = op
            .cook(
                &context,
                &[Some(Data::json(plan)), Some(Data::json(domain.to_json()))],
                &params,
            )
            .unwrap();
        let case = result[0].as_file().unwrap().to_path_buf();
        std::fs::create_dir_all(case.join("constant")).unwrap();
        let manifest = manifest(&case).unwrap();
        (context, case, manifest, domain, reference)
    }

    #[test]
    fn wp9_reference_mesh_settings_bind_overrides_and_the_background_dictionary() {
        let (context, case, manifest, domain, reference) = reference_mesh_dictionary_fixture();
        let dictionary = read(&case, "system/blockMeshDict");
        assert!(
            reference_mesh_settings_match(&context, &case, &manifest, &domain, &reference).unwrap()
        );
        let mut changed_domain = domain.clone();
        changed_domain.max_m[0] += 1.0;
        assert!(!reference_mesh_settings_match(
            &context,
            &case,
            &manifest,
            &changed_domain,
            &reference
        )
        .unwrap());
        let mut changed_reference = reference.clone();
        changed_reference.patches.pop();
        assert!(!reference_mesh_settings_match(
            &context,
            &case,
            &manifest,
            &domain,
            &changed_reference
        )
        .unwrap());
        std::fs::write(
            case.join("system/blockMeshDict"),
            format!("{dictionary}\n// edited after mesh generation\n"),
        )
        .unwrap();
        assert!(
            !reference_mesh_settings_match(&context, &case, &manifest, &domain, &reference)
                .unwrap()
        );
        std::fs::write(case.join("system/blockMeshDict"), dictionary).unwrap();
        for extra in [
            "constant/dynamicMeshDict",
            "constant/MRFProperties",
            "constant/fvOptions",
            "system/fvOptions",
            "constant/fvModels",
            "system/fvModels",
            "system/topoSetDict",
        ] {
            std::fs::write(case.join(extra), "additional physics\n").unwrap();
            assert!(
                !reference_mesh_settings_match(&context, &case, &manifest, &domain, &reference)
                    .unwrap(),
                "accepted {extra}"
            );
            std::fs::remove_file(case.join(extra)).unwrap();
        }
    }

    #[test]
    fn wp9_reference_mesh_settings_reject_legacy_recipe_and_an_omitted_surface() {
        let (context, case, manifest, domain, reference) = reference_mesh_dictionary_fixture();
        let mut legacy = manifest.clone();
        legacy.as_object_mut().unwrap().remove("meshing_recipe");
        assert!(
            !reference_mesh_settings_match(&context, &case, &legacy, &domain, &reference).unwrap(),
            "legacy metadata cannot prove which surfaces drove the mesh"
        );
        let original = read(&case, "system/snappyHexMeshDict");
        std::fs::write(
            case.join("system/snappyHexMeshDict"),
            original.replace("wheel_rr", "omitted_rear"),
        )
        .unwrap();
        assert!(
            !reference_mesh_settings_match(&context, &case, &manifest, &domain, &reference)
                .unwrap(),
            "an edited meshing dictionary kept full-vehicle reference admission"
        );
    }

    #[test]
    fn wp9_case_setup_removes_unqualified_reference_on_reused_owned_case() {
        let context = ctx("reference-unqualified-reuse");
        let params = Params::from_specs(&setup_info().params);
        let first = setup_cook(&context, &[], &params).unwrap();
        let case = first[0].as_file().unwrap();
        let sidecar = case.join(crate::reference_record::FILE_NAME);
        std::fs::write(&sidecar, b"stale or forged reference envelope").unwrap();
        let again = setup_cook(&context, &[], &params).unwrap();
        assert_eq!(again[0].as_file(), Some(case));
        assert!(
            !sidecar.exists(),
            "an unqualified setup retained a trusted-looking old record"
        );
    }

    #[test]
    fn wp9_reference_admission_requires_phase_geometry_and_supported_profile() {
        use crate::reference::{GeometryBinding, ReferenceBinding};
        use crate::reference_record::{Phase, Record};
        // Synthetic envelope policy fixture, never registered production CAD.
        let geometry = GeometryBinding {
            version: 1,
            candidate: "drivaer_fastback".into(),
            preparation_revision: "synthetic-envelope-fixture".into(),
            tri_surface_fingerprint: "0123456789abcdef".into(),
        };
        let binding = ReferenceBinding {
            version: 1,
            profile: "synthetic-envelope-fixture".into(),
            geometry: geometry.clone(),
            u_ms: 16.0,
            rho: 1.184,
            nu: 1.567e-5,
            full_area_m2: 2.16,
            reference_length_m: 4.6,
            half_model: true,
            ground_mode: "moving".into(),
            wheel_mode: "rotating".into(),
            wheel_omega_rad_s: -51.0,
        };
        let mut record = Record {
            version: 1,
            phase: Phase::Setup,
            binding: binding.clone(),
            input_fingerprint: "fedcba9876543210".into(),
        };
        assert!(admit_reference_record(&record, &geometry, Phase::Setup, false).is_none());
        assert!(admit_reference_record(&record, &geometry, Phase::Solved, true).is_none());
        let mut other = geometry.clone();
        other.candidate = "drivaer_estate".into();
        assert!(admit_reference_record(&record, &other, Phase::Setup, true).is_none());
        other = geometry.clone();
        other.tri_surface_fingerprint = "0000000000000000".into();
        assert!(admit_reference_record(&record, &other, Phase::Setup, true).is_none());
        other = geometry.clone();
        other.preparation_revision = "different-preparation".into();
        assert!(admit_reference_record(&record, &other, Phase::Setup, true).is_none());
        record.version = 2;
        assert!(admit_reference_record(&record, &geometry, Phase::Setup, true).is_none());
        record.version = 1;
        assert_eq!(
            admit_reference_record(&record, &geometry, Phase::Setup, true),
            Some(binding.clone())
        );
        record.phase = Phase::Solved;
        assert_eq!(
            admit_reference_record(&record, &geometry, Phase::Solved, true),
            Some(binding)
        );
    }

    #[test]
    fn review_boundary_motion_speed_must_match_the_case() {
        for (ground, wheel) in [("moving", "static"), ("stationary", "rotating")] {
            let mut gp = Params::from_specs(&crate::tunnel::gw_info().params);
            gp.set("u_ms", ParamValue::Float(40.0));
            gp.set("ground_mode", ParamValue::Str(ground.into()));
            gp.set("wheel_mode", ParamValue::Str(wheel.into()));
            let boundary = crate::tunnel::gw_cook(&ctx("review-bc-source"), &[], &gp).unwrap();
            let inputs = [None, None, Some(boundary[0].clone())];
            let context = ctx("review-bc-speed");
            let mut p = Params::from_specs(&setup_info().params);
            assert!(
                setup_cook(&context, &inputs, &p).is_err(),
                "{ground}/{wheel}:40m/s motion silently paired with30m/s inflow"
            );
            assert!(
                !context.work_dir.join("native").exists(),
                "invalid motion wrote a partial case"
            );
            p.set("u_ms", ParamValue::Float(40.0));
            assert!(setup_cook(&context, &inputs, &p).is_ok());
        }
    }

    #[test]
    fn review_steady_run_requires_initial_fields_before_spawning() {
        let context = ctx("review-steady-preflight");
        let source = context.work_dir.join("mesh-only-source");
        std::fs::create_dir_all(source.join("system")).unwrap();
        std::fs::write(
            source.join("system/controlDict"),
            "application simpleFoam;\n",
        )
        .unwrap();
        std::fs::create_dir_all(source.join("constant/polyMesh")).unwrap();
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            std::fs::write(
                source.join("constant/polyMesh").join(name),
                "opaque unit mesh fixture",
            )
            .unwrap();
        }
        let tools = context.work_dir.join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        #[cfg(windows)]
        std::fs::write(
            tools.join("simpleFoam.cmd"),
            "@echo off\r\necho ran > unexpectedly-started.txt\r\nexit /b 0\r\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tool = tools.join("simpleFoam");
            std::fs::write(
                &tool,
                "#!/bin/sh\nprintf ran > unexpectedly-started.txt\nexit 0\n",
            )
            .unwrap();
            std::fs::set_permissions(tool, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut p = Params::from_specs(&solve_info().params);
        p.set("run", ParamValue::Bool(true));
        p.set("np", ParamValue::Int(1));
        p.set("potential_init", ParamValue::Bool(false));
        p.set("bin_dir", ParamValue::Str(tools.display().to_string()));
        let _result = solve_cook(&context, &[Some(Data::File(source.clone()))], &p);
        assert!(
            !context
                .work_dir
                .join("native/case/unexpectedly-started.txt")
                .exists(),
            "the solver was launched for a mesh-only case without initial fields"
        );
        assert!(!source.join("unexpectedly-started.txt").exists());
    }

    #[test]
    fn solve_reads_existing_results_and_fails_closed_without() {
        let (case, _) = setup(&[], &[], "solve");
        let p = Params::from_specs(&solve_info().params);
        let out = solve_cook(&ctx("solve-run"), &[Some(Data::File(case.clone()))], &p).unwrap();
        let r = out[2].as_report().unwrap();
        assert!(!r.passes());
        assert!(r
            .notes
            .iter()
            .any(|(_, t)| t.starts_with("white-box placeholder:")));
        assert!(crate::types::Series::from_json(out[1].as_json().unwrap())
            .unwrap()
            .is_empty());
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
        assert!(r
            .notes
            .iter()
            .any(|(s, t)| *s == Severity::Warning && t.contains("not produced by a run here")));
        let s = crate::types::Series::from_json(out[1].as_json().unwrap()).unwrap();
        assert_eq!(s.len(), 50);
        assert_eq!(s.meta["Aref"], 1.08);
        // stamped for these inputs: read without a warning
        std::fs::write(
            case.join(RESULTS_STAMP),
            case_stamp(&ctx("solve-import-stamp"), &case).unwrap(),
        )
        .unwrap();
        let out = solve_cook(&ctx("solve-run"), &[Some(Data::File(case.clone()))], &p).unwrap();
        assert!(out[2]
            .as_report()
            .unwrap()
            .notes
            .iter()
            .all(|(s, _)| *s != Severity::Warning));
        // the case is rewritten for other inputs: the old results are not read
        let dict = read(&case, "system/controlDict");
        std::fs::write(
            case.join("system/controlDict"),
            dict + "\n// other inputs\n",
        )
        .unwrap();
        let out = solve_cook(&ctx("solve-run"), &[Some(Data::File(case.clone()))], &p).unwrap();
        assert!(crate::types::Series::from_json(out[1].as_json().unwrap())
            .unwrap()
            .is_empty());
        assert!(out[2]
            .as_report()
            .unwrap()
            .notes
            .iter()
            .any(|(_, t)| t.contains("solved for other inputs")));
        // missing tool with run on: reported, never a fake success
        let mut p2 = p.clone();
        p2.set("run", ParamValue::Bool(true));
        p2.set(
            "bin_dir",
            ParamValue::Str(case.join("nowhere").display().to_string()),
        );
        let out = solve_cook(&ctx("solve-run"), &[Some(Data::File(case))], &p2).unwrap();
        assert!(out[2]
            .as_report()
            .unwrap()
            .notes
            .iter()
            .any(|(s, _)| *s == Severity::Error));
        assert!(solve_cook(&ctx("solve-run"), &[], &p).is_err());
    }
}
