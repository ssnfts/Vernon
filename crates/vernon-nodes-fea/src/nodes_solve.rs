//! Deck writing, the external solver run and the built-in reference solver.

use std::path::{Path, PathBuf};
use std::time::Duration;

use vernon_core::{AttribArray, Metric, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::deck::{write_deck, DeckInput, DeckOptions, ElasticType};
use crate::frd::write_frd;
use crate::material::MaterialCard;
use crate::refsolve::{dat_text, solve, RefInput};
use crate::solver::{find_executable, run_with_timeout, tail, RunStatus};
use crate::types::{fnv_hex, from_data, opt_from_data, to_data, CaseSet, DeckEntry, DeckManifest, MountSet, OrientationCard, RunEntry, SolveManifest};
use crate::util::{fea_info, sanitize, solid_mesh};

fn orientation(inputs: &[Option<Data>], index: usize) -> CookResult<OrientationCard> {
    Ok(opt_from_data::<OrientationCard>(inputs, index, "orientation card")?.unwrap_or_else(OrientationCard::identity))
}

// ------------------------------------------------------------ deck writer

pub struct DeckWrite;

impl NodeOp for DeckWrite {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.deck_write", "Structural Deck Writer")
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::new("material", DataKind::Json))
            .input(PortSpec::new("mounts", DataKind::Json))
            .input(PortSpec::new("cases", DataKind::Json))
            .input(PortSpec::optional("orientation", DataKind::Json))
            .output(PortSpec::new("decks", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("dir", DataKind::File))
            .param(ParamSpec::string("job", "Job name", "vernon").help("Prefix of every deck file; the case name is appended"))
            .param(ParamSpec::choice("elastic", "Material card", "engineering_constants", &["engineering_constants", "ortho"]).help("Engineering constants are the safer card; the orthotropic stiffness card is written from the closed-form terms"))
            .param(ParamSpec::choice("solver", "Equation solver", "auto", &["auto", "spooles", "pardiso", "iterative_cholesky"]).help("Auto takes the first installed solver; thin panels prefer a direct solver"))
            .param(ParamSpec::bool("pressure_as_cload", "Pressure as nodal forces", false).help("Write the face pressures as consistent nodal forces instead of pressure cards"))
            .param(ParamSpec::bool("ip_stress", "Integration-point stresses", true).help("Also print the stress at the integration points to the text output (used for the averaged mount stress)"))
            .param(ParamSpec::float("temperature_c", "Uniform temperature", 23.0, -40.0, 300.0).unit("degC").help("Used when the mesh carries no temp_c attribute"))
            .param(ParamSpec::bool("thermal_strain", "Thermal strain", false).help("Write an expansion coefficient so temperature also causes thermal stress; off keeps temperature to selecting the stiffness only"))
            .param(ParamSpec::float("alpha_per_k", "Expansion coefficient", 7.0e-5, 0.0, 1.0e-3).unit("1/K").help("Thermal expansion coefficient written to the deck, per kelvin."))
            .param(ParamSpec::float("t_ref_c", "Stress-free temperature", 23.0, -40.0, 300.0).unit("degC").help("Temperature at which the part is stress free. Thermal strain is measured from it."))
            .param(ParamSpec::bool("allow_linear_tet", "Allow linear tetrahedra", false).help("Linear tetrahedra are too stiff for structural use"))
            .param(ParamSpec::bool("unit_check", "Unit check", true).help("Refuse decks whose modulus, density, size, pressure or acceleration look like the wrong unit"))
            .help("Writes one solver input deck per load case in the unit system mm, N, tonne, s, MPa: nodes, quadratic tetrahedra, mount sets, material with temperature rows, local material axes, constraints, loads and the result requests.")
            .emulates("finite-element solver input decks (keyword cards)")
            .implemented()
            .external("CalculiX")
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = solid_mesh(ctx, inputs, 0)?;
        let card: MaterialCard = from_data(inputs, 1, "material card")?;
        let mounts: MountSet = from_data(inputs, 2, "mount set")?;
        let cases: CaseSet = from_data(inputs, 3, "case set")?;
        let orient = orientation(inputs, 4)?;
        if mounts.sites.is_empty() {
            return Err(ctx.fail("no mounts: a static study needs constraints"));
        }
        if cases.cases.is_empty() {
            return Err(ctx.fail("the case set is empty"));
        }
        let job = sanitize(params.str("job")?).to_lowercase();
        let solver = match params.str("solver")? {
            "iterative_cholesky" => "iterative cholesky".to_string(),
            s => s.to_string(),
        };
        let dir = ctx.work_dir.join("decks");
        std::fs::create_dir_all(&dir)?;
        let mut man = DeckManifest { dir: dir.to_string_lossy().into_owned(), units: "mm N tonne s MPa".into(), node_count: mesh.points.len(), element_count: mesh.prims.len(), ..Default::default() };
        let mut r = Report::new("Structural decks");
        let mut warnings: Vec<String> = Vec::new();
        for case in &cases.cases {
            let name = sanitize(&case.name).to_lowercase();
            let opts = DeckOptions {
                job: format!("{job}_{name}"),
                elastic: if params.str("elastic")? == "ortho" { ElasticType::Ortho } else { ElasticType::EngineeringConstants },
                solver: solver.clone(),
                pressure_as_cload: params.bool("pressure_as_cload")?,
                ip_stress_print: params.bool("ip_stress")?,
                temperature_c: params.float("temperature_c")?,
                thermal_strain: params.bool("thermal_strain")?.then(|| (params.float("alpha_per_k").unwrap_or(7.0e-5), params.float("t_ref_c").unwrap_or(23.0))),
                allow_linear_tet: params.bool("allow_linear_tet")?,
                unit_check: params.bool("unit_check")?,
            };
            let out = write_deck(&DeckInput { mesh: &mesh, card: &card, orient: &orient, mounts: &mounts, load: &case.loads, opts: &opts }).map_err(|e| ctx.fail(format!("case '{}': {e}", case.name)))?;
            let file = format!("{}.inp", opts.job);
            std::fs::write(dir.join(&file), &out.text)?;
            for w in out.warnings {
                if !warnings.contains(&w) {
                    warnings.push(w);
                }
            }
            man.decks.push(DeckEntry { case: name, job: opts.job.clone(), file, hash: fnv_hex(out.text.as_bytes()), speed_factor: case.speed_factor });
            man.element_type = if mesh.prims[0].vertices.len() == 10 { "C3D10" } else { "C3D4" }.into();
        }
        man.mount_sets = (1..=mounts.sites.len()).map(|k| format!("MOUNT{k}")).collect();
        r.metric("deck_count", Metric::new(man.decks.len() as f64, ""));
        r.metric("nodes", Metric::new(man.node_count as f64, ""));
        r.metric("elements", Metric::new(man.element_count as f64, ""));
        r.metric("mount_count", Metric::new(mounts.sites.len() as f64, ""));
        r.metric("unit_check_passed", Metric::new(if params.bool("unit_check")? { 1.0 } else { 0.0 }, "flag"));
        for w in warnings {
            r.note(Severity::Warning, w);
        }
        if !params.bool("unit_check")? {
            r.note(Severity::Warning, "the unit check is switched off: a modulus in Pa or a length in metres would go unnoticed");
        }
        r.note(Severity::Info, "units are mm, N, tonne, s, MPa; node numbers are point indices plus one");
        Ok(vec![to_data(&man), Data::report(r), Data::File(dir)])
    }
}

// ----------------------------------------------------------- external run

pub struct SolveExternal;

const CCX_NAMES: [&str; 5] = ["ccx", "ccx_2.21", "ccx_2.20", "ccx_static", "ccx_MT"];

fn stamp_path(run_dir: &Path, job: &str) -> PathBuf {
    run_dir.join(format!("{job}.deckhash"))
}

impl NodeOp for SolveExternal {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.solve_external", "Run Structural Solver")
            .input(PortSpec::new("decks", DataKind::Json))
            .output(PortSpec::new("results", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("dir", DataKind::File))
            .param(ParamSpec::bool("run", "Run solver", false).help("Run the executable below once on every deck; switches itself off again. Results already computed from an identical deck are reused").action())
            .param(ParamSpec::path("exe_path", "Solver executable", &["exe", ""], false).help("Empty searches the program path"))
            .param(ParamSpec::int("threads", "Threads", 0, 0, 256).help("0 leaves the solver default"))
            .param(ParamSpec::float("timeout_s", "Timeout", 3600.0, 1.0, 604800.0).unit("s").help("Longest the solver may run before it is stopped, in seconds."))
            .help("Runs the open-source finite-element solver as a separate process on each deck, one folder per deck hash so old results can never be mistaken for new ones. Without the solver the decks stay on disk and the report fails the solved-cases check.")
            .emulates("running a finite-element solver on a written input deck")
            .implemented()
            .external("CalculiX")
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let man: DeckManifest = from_data(inputs, 0, "deck manifest")?;
        let deck_dir = PathBuf::from(&man.dir);
        let runs_dir = ctx.work_dir.join("runs");
        std::fs::create_dir_all(&runs_dir)?;
        let want_run = params.bool("run")?;
        let exe = find_executable(params.str("exe_path")?, &CCX_NAMES);
        let mut r = Report::new("Structural solver");
        let mut sm = SolveManifest { dir: runs_dir.to_string_lossy().into_owned(), solver: "external".into(), element_type: man.element_type.clone(), mount_sets: man.mount_sets.clone(), runs: vec![] };
        let threads = params.int("threads")?;
        let envs: Vec<(String, String)> = if threads > 0 { vec![("OMP_NUM_THREADS".into(), threads.to_string()), ("CCX_NPROC_EQUATION_SOLVER".into(), threads.to_string())] } else { vec![] };
        if let (true, Some(e)) = (want_run, &exe) {
            let log = ctx.work_dir.join("solver_version.log");
            if let Ok(RunStatus::Exited(_)) = run_with_timeout(e, &["-v".to_string()], &ctx.work_dir, &log, Duration::from_secs(20), &[]) {
                if let Some(line) = std::fs::read_to_string(&log).ok().and_then(|t| t.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string())) {
                    r.note(Severity::Info, format!("solver: {line}"));
                }
            }
        }
        let mut solved = 0usize;
        for d in &man.decks {
            let run_dir = runs_dir.join(&d.hash);
            std::fs::create_dir_all(&run_dir)?;
            let src = deck_dir.join(&d.file);
            let text = std::fs::read_to_string(&src).map_err(|e| ctx.fail(format!("cannot read deck {}: {e}", src.display())))?;
            if fnv_hex(text.as_bytes()) != d.hash {
                return Err(ctx.fail(format!("deck {} changed after it was written: write the decks again", d.file)));
            }
            std::fs::write(run_dir.join(format!("{}.inp", d.job)), &text)?;
            let frd = run_dir.join(format!("{}.frd", d.job));
            let dat = run_dir.join(format!("{}.dat", d.job));
            let stamp = stamp_path(&run_dir, &d.job);
            let cached = frd.is_file() && std::fs::read_to_string(&stamp).is_ok_and(|s| s.trim() == d.hash);
            let mut status = "not_run";
            if cached {
                status = "reused";
            } else if want_run {
                match &exe {
                    None => status = "solver_missing",
                    Some(e) => {
                        // never let a result of an older deck survive
                        for ext in ["frd", "dat", "sta", "cvg", "12d", "deckhash"] {
                            let _ = std::fs::remove_file(run_dir.join(format!("{}.{ext}", d.job)));
                        }
                        let log = run_dir.join(format!("{}.log", d.job));
                        let st = run_with_timeout(e, &["-i".to_string(), d.job.clone()], &run_dir, &log, Duration::from_secs_f64(params.float("timeout_s")?), &envs)?;
                        match st {
                            RunStatus::Exited(0) if frd.is_file() => {
                                std::fs::write(&stamp, &d.hash)?;
                                status = "solved";
                            }
                            RunStatus::TimedOut => return Err(CookError::External { tool: "CalculiX".into(), message: format!("timed out on case '{}'", d.case) }),
                            RunStatus::Cancelled => return Err(CookError::External { tool: "CalculiX".into(), message: format!("stopped on case '{}' (cook cancelled)", d.case) }),
                            RunStatus::Exited(code) => {
                                return Err(CookError::External { tool: "CalculiX".into(), message: format!("case '{}' ended with exit code {code}:\n{}", d.case, tail(&log, 15)) });
                            }
                        }
                    }
                }
            }
            if matches!(status, "solved" | "reused") {
                solved += 1;
            }
            let rel = |p: &Path, present: bool| if present { format!("{}/{}", d.hash, p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) } else { String::new() };
            sm.runs.push(RunEntry {
                case: d.case.clone(),
                job: d.job.clone(),
                status: status.into(),
                frd: rel(&frd, frd.is_file()),
                dat: rel(&dat, dat.is_file()),
                log: rel(&run_dir.join(format!("{}.log", d.job)), run_dir.join(format!("{}.log", d.job)).is_file()),
                hash: d.hash.clone(),
                speed_factor: d.speed_factor,
            });
        }
        r.metric("cases_total", Metric::new(man.decks.len() as f64, ""));
        r.metric("cases_solved", Metric::new(solved as f64, "").with_limit(man.decks.len() as f64, false));
        if !want_run && solved < man.decks.len() {
            r.note(Severity::Warning, "the solver was not run: switch 'Run solver' on; the decks are in the work folder");
        }
        if want_run && exe.is_none() {
            r.note(Severity::Warning, "solver executable not found: set its path or add it to the program path; the decks are in the work folder");
        }
        if solved > 0 && solved < man.decks.len() {
            r.note(Severity::Warning, "some cases have no results");
        }
        Ok(vec![to_data(&sm), Data::report(r), Data::File(runs_dir)])
    }
}

// -------------------------------------------------------- reference solver

pub struct SolveLinear;

impl NodeOp for SolveLinear {
    fn info(&self) -> NodeTypeInfo {
        fea_info("fea.solve_linear", "Reference Linear Solver")
            .input(PortSpec::new("mesh", DataKind::Geometry))
            .input(PortSpec::new("material", DataKind::Json))
            .input(PortSpec::new("mounts", DataKind::Json))
            .input(PortSpec::new("cases", DataKind::Json))
            .input(PortSpec::optional("orientation", DataKind::Json))
            .output(PortSpec::new("results", DataKind::Json))
            .output(PortSpec::new("report", DataKind::Report))
            .output(PortSpec::new("dir", DataKind::File))
            .output(PortSpec::new("fields", DataKind::Geometry))
            .param(ParamSpec::float("tol", "Tolerance", 1.0e-10, 1.0e-14, 1.0e-4).help("Relative residual of the conjugate-gradient solve"))
            .param(ParamSpec::int("max_dof", "Largest model", 60000, 1000, 300000).help("Largest number of unknowns this solver accepts"))
            .help("Built-in linear static solver for four-node and ten-node tetrahedra with orthotropic printed material, used to run the chain without the external solver and to check it. Writes the same result files as the external solver, plus the mesh with disp_<case> on the points and vm_mpa_<case> on the elements. Four-node tetrahedra are stiff in bending and need a fine mesh, so safety factors from them stay estimates that cannot pass a gate. Thermal expansion is not modelled, and the temperature comes only from a temp_c point attribute (23 degC without one), so safety factors on a model with temp_c stay estimates too; use the external solver for those.")
            .emulates("linear static solution of a tetrahedral model")
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let mesh = solid_mesh(ctx, inputs, 0)?;
        let card: MaterialCard = from_data(inputs, 1, "material card")?;
        let mounts: MountSet = from_data(inputs, 2, "mount set")?;
        let cases: CaseSet = from_data(inputs, 3, "case set")?;
        let orient = orientation(inputs, 4)?;
        card.check_stability().map_err(|e| ctx.fail(format!("material constants rejected: {e}")))?;
        let root = ctx.work_dir.join("reference");
        std::fs::create_dir_all(&root)?;
        let mut r = Report::new("Reference linear solution");
        let element_type = if mesh.prims.first().is_some_and(|p| p.vertices.len() == 10) { "C3D10" } else { "C3D4" };
        let mut fields = (*mesh).clone();
        let mut sm = SolveManifest { dir: root.to_string_lossy().into_owned(), solver: "reference_linear".into(), element_type: element_type.into(), mount_sets: (1..=mounts.sites.len()).map(|k| format!("MOUNT{k}")).collect(), runs: vec![] };
        for case in &cases.cases {
            let name = sanitize(&case.name).to_lowercase();
            // Same refusal as the deck writer: a wrong-unit model must not reach a gate.
            let (errors, warnings) = crate::deck::unit_gate(&mesh, &card, &case.loads);
            if !errors.is_empty() {
                return Err(ctx.fail(format!("case '{}': {}", case.name, errors.join("; "))));
            }
            for w in warnings {
                if !r.notes.iter().any(|(_, t)| *t == w) {
                    r.note(Severity::Warning, w);
                }
            }
            let out = solve(&RefInput { mesh: &mesh, card: &card, orient: &orient, mounts: &mounts, load: &case.loads, max_dof: params.int("max_dof")? as usize, tol: params.float("tol")? }).map_err(|e| ctx.fail(format!("case '{}': {e}", case.name)))?;
            let dir = root.join(&name);
            std::fs::create_dir_all(&dir)?;
            let job = format!("reference_{name}");
            std::fs::write(dir.join(format!("{job}.frd")), write_frd(&mesh.points, Some(&out.disp), Some(&out.node_stress)))?;
            let dat = dat_text(&out);
            std::fs::write(dir.join(format!("{job}.dat")), &dat)?;
            r.metric(format!("{name}_max_disp_mm"), Metric::new(out.max_disp(), "mm"));
            r.metric(format!("{name}_max_vm_mpa"), Metric::new(out.max_vm(), "MPa"));
            let elem_vm = out.elem_vm();
            r.metric(format!("{name}_max_elem_vm_mpa"), Metric::new(elem_vm.iter().cloned().fold(0.0, f64::max), "MPa"));
            fields.point_attribs.set(format!("disp_{name}"), AttribArray::Vec3(out.disp.clone()));
            fields.prim_attribs.set(format!("vm_mpa_{name}"), AttribArray::Float(elem_vm));
            r.metric(format!("{name}_iterations"), Metric::new(out.iterations as f64, ""));
            r.metric(format!("{name}_residual"), Metric::new(out.residual, ""));
            sm.runs.push(RunEntry { case: name.clone(), job: job.clone(), status: "solved".into(), frd: format!("{name}/{job}.frd"), dat: format!("{name}/{job}.dat"), log: String::new(), hash: fnv_hex(dat.as_bytes()), speed_factor: case.speed_factor });
        }
        r.note(Severity::Info, format!("{} load cases solved with {element_type} elements", sm.runs.len()));
        if mesh.point_attribs.contains("temp_c") {
            r.note(Severity::Warning, "temperatures soften the material here but thermal expansion is not modelled, so restrained parts miss their thermal stress");
        }
        if element_type == "C3D4" {
            r.note(Severity::Warning, "four-node tetrahedra are stiff in bending: deflections and stresses come out low unless the mesh is fine");
        }
        Ok(vec![to_data(&sm), Data::report(r), Data::File(root), Data::geometry(fields)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{box_tets, tet4_to_tet10};
    use crate::nodes_load::{LoadAero, LoadBump, LoadCases};
    use crate::nodes_mesh::MountSites;
    use crate::util::testing::{ctx, defaults};
    use glam::DVec3;
    use vernon_graph::ParamValue;

    fn j<T: serde::Serialize>(v: &T) -> Data {
        to_data(v)
    }

    /// Plate with a mount patch and the four load cases.
    struct Chain {
        mesh: Data,
        mat: Data,
        mounts: Data,
        cases: Data,
    }

    fn chain(tag: &str) -> Chain {
        let c = ctx(tag);
        let mesh = tet4_to_tet10(&box_tets(DVec3::new(60.0, 40.0, 6.0), [12, 8, 2])).unwrap();
        let mut p = defaults(&MountSites);
        p.set("site_a", ParamValue::Vec3(DVec3::new(-25.0, 0.0, 3.0)));
        p.set("model", ParamValue::Str("fixed_patch".into()));
        let ms = MountSites.cook(&c, &[Some(Data::geometry(mesh.clone()))], &p).unwrap();
        let aero = LoadAero.cook(&c, &[Some(Data::geometry(mesh.clone()))], &defaults(&LoadAero)).unwrap();
        let bump = LoadBump.cook(&c, &[Some(Data::geometry(mesh.clone()))], &defaults(&LoadBump)).unwrap();
        let cases = LoadCases.cook(&c, &[Some(aero[0].clone()), Some(bump[0].clone()), Some(ms[0].clone())], &defaults(&LoadCases)).unwrap();
        let card = crate::nodes_material::build_card(&defaults(&crate::nodes_material::MaterialFdm), &Default::default()).unwrap().0;
        Chain { mesh: Data::geometry(mesh), mat: j(&card), mounts: ms[0].clone(), cases: cases[0].clone() }
    }

    fn deck_inputs(c: &Chain) -> Vec<Option<Data>> {
        vec![Some(c.mesh.clone()), Some(c.mat.clone()), Some(c.mounts.clone()), Some(c.cases.clone())]
    }

    #[test]
    fn one_deck_per_case_with_the_documented_cards() {
        let c = chain("deck1");
        let x = ctx("deck1w");
        let out = DeckWrite.cook(&x, &deck_inputs(&c), &defaults(&DeckWrite)).unwrap();
        let man: DeckManifest = serde_json::from_value((**out[0].as_json().unwrap()).clone()).unwrap();
        assert_eq!(man.decks.len(), 4);
        assert_eq!(man.element_type, "C3D10");
        assert_eq!(man.mount_sets, ["MOUNT1"]);
        let text = std::fs::read_to_string(Path::new(&man.dir).join(&man.decks[0].file)).unwrap();
        for card in ["*HEADING", "*ELEMENT,TYPE=C3D10,ELSET=EALL", "*ORIENTATION,NAME=OR1", "*ELASTIC,TYPE=ENGINEERING CONSTANTS", "*SOLID SECTION,ELSET=EALL,MATERIAL=PRINT,ORIENTATION=OR1", "*BOUNDARY", "*STEP", "*DSLOAD", "*NODE FILE", "*END STEP"] {
            assert!(text.contains(card), "missing {card}");
        }
        assert_eq!(man.decks[0].hash, fnv_hex(text.as_bytes()));
        // the bump case is a gravity load with no surface
        let bump = std::fs::read_to_string(Path::new(&man.dir).join(&man.decks[2].file)).unwrap();
        assert!(bump.contains("EALL,GRAV,"), "{bump}");
        assert!(!bump.contains("*DSLOAD"));
        // writing twice gives identical hashes
        let again = DeckWrite.cook(&x, &deck_inputs(&c), &defaults(&DeckWrite)).unwrap();
        let man2: DeckManifest = serde_json::from_value((**again[0].as_json().unwrap()).clone()).unwrap();
        assert_eq!(man.decks.iter().map(|d| &d.hash).collect::<Vec<_>>(), man2.decks.iter().map(|d| &d.hash).collect::<Vec<_>>());
    }

    #[test]
    fn the_unit_gate_stops_a_modulus_in_pascals() {
        let mut c = chain("deck2");
        let mut card: MaterialCard = serde_json::from_value((**c.mat.as_json().unwrap()).clone()).unwrap();
        card.e1 *= 1.0e6;
        card.e2 *= 1.0e6;
        card.e3 *= 1.0e6;
        c.mat = j(&card);
        let err = DeckWrite.cook(&ctx("deck2w"), &deck_inputs(&c), &defaults(&DeckWrite)).unwrap_err().to_string();
        assert!(err.contains("unit gate"), "{err}");
    }

    #[test]
    fn deck_writer_needs_mounts() {
        let c = chain("deck3");
        let mut ins = deck_inputs(&c);
        ins[2] = Some(j(&MountSet::default()));
        assert!(DeckWrite.cook(&ctx("deck3w"), &ins, &defaults(&DeckWrite)).is_err());
    }

    #[test]
    fn without_the_solver_the_report_fails_and_nothing_is_invented() {
        let c = chain("ext1");
        let x = ctx("ext1w");
        let decks = DeckWrite.cook(&x, &deck_inputs(&c), &defaults(&DeckWrite)).unwrap();
        let out = SolveExternal.cook(&x, &[Some(decks[0].clone())], &defaults(&SolveExternal)).unwrap();
        let sm: SolveManifest = serde_json::from_value((**out[0].as_json().unwrap()).clone()).unwrap();
        assert!(sm.runs.iter().all(|r| r.status == "not_run" && r.frd.is_empty()));
        let rep = out[1].as_report().unwrap();
        assert!(!rep.passes());
        assert_eq!(rep.metrics["cases_solved"].value, 0.0);
        // asking to run with a missing executable is reported, not an error
        let mut p = defaults(&SolveExternal);
        p.set("run", ParamValue::Bool(true));
        p.set("exe_path", ParamValue::Str("Z:/definitely/not/here/ccx.exe".into()));
        let out = SolveExternal.cook(&x, &[Some(decks[0].clone())], &p).unwrap();
        let sm: SolveManifest = serde_json::from_value((**out[0].as_json().unwrap()).clone()).unwrap();
        assert!(sm.runs.iter().all(|r| r.status == "solver_missing"));
    }

    #[test]
    fn results_of_an_identical_deck_are_reused_and_a_changed_deck_is_not() {
        let c = chain("ext2");
        let x = ctx("ext2w");
        let decks = DeckWrite.cook(&x, &deck_inputs(&c), &defaults(&DeckWrite)).unwrap();
        let man: DeckManifest = serde_json::from_value((**decks[0].as_json().unwrap()).clone()).unwrap();
        let d = &man.decks[0];
        let run_dir = x.work_dir.join("runs").join(&d.hash);
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join(format!("{}.frd", d.job)), " 9999\n").unwrap();
        std::fs::write(stamp_path(&run_dir, &d.job), &d.hash).unwrap();
        let out = SolveExternal.cook(&x, &[Some(decks[0].clone())], &defaults(&SolveExternal)).unwrap();
        let sm: SolveManifest = serde_json::from_value((**out[0].as_json().unwrap()).clone()).unwrap();
        assert_eq!(sm.runs[0].status, "reused");
        assert_eq!(sm.runs[1].status, "not_run");
        // tampering with a deck after it was written is refused
        std::fs::write(Path::new(&man.dir).join(&d.file), "*HEADING\nother\n").unwrap();
        assert!(SolveExternal.cook(&x, &[Some(decks[0].clone())], &defaults(&SolveExternal)).is_err());
    }

    #[test]
    fn reference_solver_writes_readable_files() {
        let c = chain("lin1");
        let x = ctx("lin1w");
        let out = SolveLinear.cook(&x, &deck_inputs(&c), &defaults(&SolveLinear)).unwrap();
        let sm: SolveManifest = serde_json::from_value((**out[0].as_json().unwrap()).clone()).unwrap();
        assert_eq!(sm.solver, "reference_linear");
        assert_eq!(sm.element_type, "C3D10");
        assert_eq!(sm.runs.len(), 4);
        // displacement on the points, von Mises per element
        let g = out[3].as_geometry().unwrap();
        assert!(g.point_attribs.contains("disp_aero"));
        let vm = g.prim_attribs.floats("vm_mpa_aero").unwrap();
        assert_eq!(vm.len(), g.prims.len());
        assert!(vm.iter().any(|v| *v > 0.0));
        for r in &sm.runs {
            assert!(Path::new(&sm.dir).join(&r.frd).is_file() && Path::new(&sm.dir).join(&r.dat).is_file());
        }
        let rep = out[1].as_report().unwrap();
        assert!(rep.passes(), "{:?}", rep.metrics);
        assert!(rep.notes.iter().all(|(_, t)| !t.starts_with("white-box placeholder:")));
        assert!(rep.metrics["aero_max_disp_mm"].value > 0.0);
        // the overspeed case deflects 1.25^2 times more (linear model)
        let ratio = rep.metrics["aero_overspeed_max_disp_mm"].value / rep.metrics["aero_max_disp_mm"].value;
        assert!((ratio - 1.5625).abs() < 1e-6, "{ratio}");
    }
}
