//! Cooks `examples/bodykit_pipeline.vrn` end to end: every node must cook
//! without a panic or an error, and every pipeline stage must hand a Report
//! to the next one.

use std::collections::BTreeSet;
use std::path::PathBuf;

use vernon_graph::Stage;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The airflow results reach the performance stage through the registered
/// nodes: the measured CdA, the lift sign flipped to downforce, and the same
/// air state in the site loads. The histories are recorded, not solved here;
/// the real solver chain is covered by the ignored tests in the airflow crate.
#[test]
fn airflow_force_report_reaches_the_vehicle_and_the_site_loads() {
    use std::sync::Arc;
    use vernon_graph::{CookContext, Data, ParamValue, Params};

    let reg = vernon_cli::registry();
    let work = std::env::temp_dir().join(format!("vernon-cli-handoff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let cook = |id: &str, ins: Vec<Option<Data>>, set: &[(&str, ParamValue)]| -> Vec<Data> {
        let mut p = Params::from_specs(&reg.info(id).unwrap().params);
        for (k, v) in set {
            p.set(k, v.clone());
        }
        let dir = work.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = CookContext { node_name: id.into(), work_dir: dir, frame: 0.0 };
        reg.get(id).unwrap().cook(&ctx, &ins, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
    };
    let j = |d: &Data| Some(Data::Json(Arc::clone(d.as_json().unwrap())));

    let air = cook("cfd.air_state", vec![], &[("t_c", ParamValue::Float(45.0)), ("rh_pct", ParamValue::Float(30.0))]);
    let case = cook("cfd.case_setup", vec![None, j(&air[0]), None, None, None, None], &[]);
    let case_dir = case[0].as_file().unwrap().clone();
    let manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(case_dir.join("vernon_case.json")).unwrap()).unwrap();
    let (a_ref, half) = (manifest["case_setup"]["aref_m2"].as_f64().unwrap(), manifest["case_setup"]["half_model"].as_bool().unwrap());
    let reference = Some(Data::json(manifest["case_setup"]["reference"].clone()));

    let mk = |sub: &str, file: &str, text: String| {
        let d = case_dir.join("postProcessing").join(sub).join("0");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(file), text).unwrap();
    };
    let (cd, cl) = (0.32, -0.25);
    let mut coeff = format!("# magUInf : 30\n# Aref : {a_ref}\n# Time Cd Cl\n");
    let mut info = String::from("# Time Ux_initial p_initial k_initial omega_initial\n");
    for i in 1..=1500 {
        coeff += &format!("{i} {cd} {cl}\n");
        info += &format!("{i} 1e-6 2e-6 1e-6 1e-6\n");
    }
    mk("forceCoeffs1", "coefficient.dat", coeff);
    mk("solverInfo1", "solverInfo.dat", info);

    let solve = cook("cfd.solve_steady", vec![Some(case[0].clone())], &[]);
    let forces = cook("cfd.force_coeffs", vec![j(&solve[1]), reference, j(&air[0])], &[("half_model", ParamValue::Bool(half))]);
    let report = forces[1].as_report().unwrap();
    assert!(report.passes(), "{report:?}");
    let (cda, cla) = (report.metrics["CdA"].value, report.metrics["ClA"].value);
    assert!(cda > 0.0 && cla < 0.0, "{cda} {cla}");

    // Manual values that must lose to the connected result.
    let vehicle = cook("perf.vehicle", vec![Some(forces[1].clone())], &[("cda_m2", ParamValue::Float(2.5)), ("cla_m2", ParamValue::Float(3.0))]);
    let v = vehicle[1].as_report().unwrap();
    assert!((v.metrics["CdA"].value - cda).abs() < 1e-12, "{v:?}");
    assert!((v.metrics["downforce_area"].value + cla).abs() < 1e-12, "{v:?}");
    assert_eq!(v.metrics["aero_from_solver"].value, 1.0);

    let loads = cook("perf.aero_loads", vec![j(&air[0]), j(&vehicle[0])], &[]);
    let rho = air[0].as_json().unwrap()["rho"].as_f64().unwrap();
    assert!((rho - 1.098).abs() < 0.002, "{rho}");
    assert!(loads.iter().any(|d| d.as_report().is_some()));
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn bodykit_pipeline_cooks_every_node_and_every_stage_reports() {
    let reg = vernon_cli::registry();
    let vrn = repo().join("examples/bodykit_pipeline.vrn");
    let mut graph = vernon_cli::load_graph(&vrn, &reg).expect("example loads");
    let work = std::env::temp_dir().join(format!("vernon-cli-test-{}", std::process::id()));
    graph.work_root = Some(work.clone());

    let outcomes = vernon_cli::cook_graph(&mut graph, &reg, None).expect("cook runs");
    assert_eq!(outcomes.len(), graph.nodes.len());
    let failed: Vec<String> = outcomes
        .iter()
        .filter_map(|o| o.result.as_ref().err().map(|e| format!("{}: {e}", o.name)))
        .collect();
    assert!(failed.is_empty(), "nodes failed:\n{}", failed.join("\n"));

    let reporting: BTreeSet<Stage> = outcomes
        .iter()
        .filter(|o| o.reports().next().is_some())
        .filter_map(|o| o.stage)
        .collect();
    for stage in Stage::ALL {
        assert!(
            reporting.contains(&stage),
            "stage {} emitted no report",
            stage.label()
        );
    }

    // Spot-check the numeric hand-offs the example is built to show.
    let metric = |node: &str, name: &str| -> f64 {
        let o = outcomes
            .iter()
            .find(|o| o.name == node)
            .unwrap_or_else(|| panic!("no node {node}"));
        o.reports()
            .find_map(|r| r.metrics.get(name).map(|m| m.value))
            .unwrap_or_else(|| panic!("{node} has no metric {name}"))
    };
    assert!(metric("icp_align", "rms_all_after") < metric("icp_align", "rms_all_before"));
    assert!(metric("deviation_stats", "p95_abs") < 0.2);
    assert!(metric("bed_fit", "fail_count") == 0.0);
    assert!(metric("top_speed", "v_max") > 100.0);

    // One air state flows through: the airflow stage's density (the hot,
    // humid 45 degC / 30 % site day, not the 1.225 standard atmosphere)
    // reaches the performance nodes, and the structural aero load uses the
    // very same design dynamic pressure and downforce the performance table
    // reports.
    let rho = metric("air", "rho");
    assert!((rho - 1.098).abs() < 0.002, "{rho}");
    assert_eq!(metric("aero_loads", "rho"), rho);
    assert_eq!(metric("top_speed", "rho"), rho);
    let q = metric("aero_loads", "design_q");
    assert!((q - 0.5 * rho * (200.0f64 / 3.6).powi(2)).abs() < 1e-6, "{q}");
    assert_eq!(metric("aero_load", "dynamic_pressure_pa"), q);
    let down = metric("aero_loads", "downforce_design");
    assert!((metric("aero_load", "force_requested_n") - down).abs() < 1e-9 * down.max(1.0));

    // The CFD case uses that same air: its report, and the density actually
    // written into the solver's force post-processing.
    assert_eq!(metric("cfd_case", "rho_inf"), rho);
    let case = outcomes.iter().find(|o| o.name == "cfd_case").unwrap();
    let case_dir = case.result.as_ref().unwrap().iter().find_map(|d| d.as_file().cloned()).expect("case directory");
    let control = std::fs::read_to_string(case_dir.join("system/controlDict")).expect("controlDict");
    let written: Vec<f64> = control
        .lines()
        .filter_map(|l| l.trim().strip_prefix("rhoInf"))
        .map(|v| v.trim().trim_end_matches(';').parse().expect("rhoInf is a number"))
        .collect();
    assert!(!written.is_empty(), "controlDict has no rhoInf");
    assert!(written.iter().all(|w| (w - rho).abs() < 1e-3), "{written:?} vs {rho}");

    // The solar stage is set up separately, so check it agrees with that air.
    let air = &graph.nodes[&graph.find("air").unwrap()];
    assert_eq!(metric("surface_temp", "ambient_c"), air.params.float("t_c").unwrap());

    // The heat check's limit is the structural material's, and it says so.
    assert_eq!(metric("hdt_check", vernon_core::keys::HDT_C), vernon_core::materials::ASA.hdt_045_c);
    assert_eq!(metric("hdt_check", vernon_core::keys::HDT_C), metric("material", vernon_core::keys::HDT_045_C));
    assert_eq!(metric("hdt_check", vernon_core::keys::ANNEALED), 0.0);
    assert_eq!(metric("hdt_check", vernon_core::keys::HDT_C), metric("derate", vernon_core::keys::HDT_045_C));

    // The structural deck runs at the temperature the solar stage computes.
    // The solar mesh is the bare panel and the solid its thickened fit, so
    // first check every solid node found the panel.
    assert_eq!(metric("temp_map", "unmapped_nodes"), 0.0);
    let (t_min, t_max) = (metric("surface_temp", "min_temp_c"), metric("surface_temp", "max_temp_c"));
    assert!(t_min > metric("surface_temp", "ambient_c") + 20.0, "{t_min}");
    let deck = outcomes.iter().find(|o| o.name == "fea_deck").unwrap();
    let deck_dir = deck.result.as_ref().unwrap().iter().find_map(|d| d.as_file().cloned()).expect("deck directory");
    let decks: Vec<PathBuf> = std::fs::read_dir(&deck_dir)
        .expect("deck directory exists")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "inp"))
        .collect();
    assert_eq!(decks.len() as f64, metric("fea_deck", "deck_count"));
    for path in &decks {
        let text = std::fs::read_to_string(path).unwrap();
        // Rows of a keyword card, up to the next card.
        let card = |prefix: &str| -> Vec<&str> {
            text.lines().skip_while(|l| !l.starts_with(prefix)).skip(1).take_while(|l| !l.starts_with('*')).collect()
        };
        let temps: Vec<f64> = card("*TEMPERATURE")
            .iter()
            .map(|l| l.split(',').nth(1).unwrap().parse().expect("node temperature"))
            .collect();
        assert_eq!(temps.len() as f64, metric("fea_deck", "nodes"), "{}", path.display());
        assert!(temps.iter().all(|t| (t_min - 1e-3..=t_max + 1e-3).contains(t)), "{}: outside {t_min}..{t_max}", path.display());

        // The material card has temperature rows spanning that temperature,
        // and the stiffness it gives there is the derated one. Each row is
        // two lines: the constants, then the last constant and the temperature.
        let rows: Vec<(f64, f64)> = card("*ELASTIC")
            .chunks(2)
            .map(|r| (r[1].split(',').nth(1).unwrap().parse().unwrap(), r[0].split(',').next().unwrap().parse().unwrap()))
            .collect();
        assert_eq!(rows.len() as f64, metric("derate", "table_points"), "{}", path.display());
        let upper = rows.iter().position(|r| r.0 >= t_max).expect("table reaches the panel temperature");
        assert!(upper > 0);
        let ((t0, e0), (t1, e1)) = (rows[upper - 1], rows[upper]);
        let retained = (e0 + (t_max - t0) / (t1 - t0) * (e1 - e0)) / rows[0].1;
        assert!(retained > metric("derate", "retention_85c") && retained < metric("derate", "retention_60c"), "{retained}");
    }

    // The stages keep their own deviation names; the shared aliases find each.
    let deviation = |node: &str| {
        let o = outcomes.iter().find(|o| o.name == node).unwrap_or_else(|| panic!("no node {node}"));
        o.reports().find_map(|r| r.first_metric(&vernon_core::keys::DEVIATION_P95_ALIASES))
    };
    assert_eq!(deviation("deviation_stats").map(|(k, _)| k), Some(vernon_core::keys::P95_ABS));
    assert_eq!(deviation("surface_deviation").map(|(k, _)| k), Some(vernon_core::keys::DEV_P95_MM));

    // Every gate passes except the two the example marks as expected.
    let gates = vernon_cli::gate_failures(&outcomes, &graph);
    let open: Vec<_> = gates.iter().filter(|g| g.expected.is_none()).collect();
    assert!(open.is_empty(), "unexpected gate failures: {open:#?}");
    let expected: BTreeSet<_> = gates.iter().map(|g| (g.node.as_str(), g.metric.as_deref().unwrap_or(""))).collect();
    assert_eq!(
        expected,
        BTreeSet::from([
            ("vehicle_prep", "wheels_found"),
            ("tet_mesh", "placeholder"),
            ("cfd_solve", "solve_done"),
            ("cfd_forces", "data_available"),
            ("vehicle", "aero_from_solver"),
        ])
    );

    // File hand-offs exist on disk.
    for node in ["package_3mf", "cfd_case", "fea_deck"] {
        let o = outcomes.iter().find(|o| o.name == node).unwrap();
        let file = o
            .result
            .as_ref()
            .unwrap()
            .iter()
            .find_map(|d| d.as_file().cloned());
        let file = file.unwrap_or_else(|| panic!("{node} produced no file"));
        assert!(file.exists(), "{node}: {} missing", file.display());
    }

    // The display node exports to OBJ.
    let (_, geo) = vernon_cli::export_geometry(&outcomes, &graph, None).expect("display geometry");
    let obj = work.join("display.obj");
    vernon_cli::write_geometry(&geo, &obj).expect("obj written");
    assert!(std::fs::metadata(&obj).unwrap().len() > 0);

    let text = vernon_cli::format_outcomes(&outcomes);
    assert!(text.contains("0 failed"), "{text}");
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn parse_fills_defaults_and_rejects_unknown_types() {
    let reg = vernon_cli::registry();
    let g = vernon_cli::parse_graph(
        r#"{ "nodes": { "0": { "type_id": "geo.box", "name": "b" } } }"#,
        &reg,
    )
    .unwrap();
    let node = &g.nodes[&0];
    let info = reg.info("geo.box").unwrap();
    assert_eq!(node.params.0.len(), info.params.len());
    assert!(vernon_cli::parse_graph(
        r#"{ "nodes": { "0": { "type_id": "nope.x", "name": "n" } } }"#,
        &reg
    )
    .is_err());
}

/// Every parameter carries a tooltip: the numeric limits and gates are the ones a user
/// has to set, and an empty tooltip hides the unit and what happens on a miss.
#[test]
fn every_parameter_has_help_text() {
    let reg = vernon_cli::registry();
    let mut missing = Vec::new();
    for info in reg.types() {
        for p in &info.params {
            if p.help.trim().len() < 8 {
                missing.push(format!("{}.{}", info.id, p.name));
            }
        }
    }
    assert!(missing.is_empty(), "{} parameters have no help text:
{}", missing.len(), missing.join("
"));
}

#[test]
fn nodes_md_is_up_to_date() {
    let reg = vernon_cli::registry();
    let md = vernon_cli::list_markdown(&reg);
    for info in reg.types() {
        assert!(
            md.contains(&format!("`{}`", info.id)),
            "{} missing from catalogue",
            info.id
        );
    }
    let on_disk =
        std::fs::read_to_string(repo().join("docs/NODES.md")).expect("docs/NODES.md exists");
    assert_eq!(
        on_disk.replace("\r\n", "\n"),
        md,
        "docs/NODES.md is stale: run `vernon-cli list --markdown > docs/NODES.md`"
    );
}

#[test]
fn cli_binary_lists_and_cooks() {
    let exe = env!("CARGO_BIN_EXE_vernon-cli");
    let out = std::process::Command::new(exe)
        .arg("list")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("clean.icp_align"));
    let bad = std::process::Command::new(exe)
        .arg("frobnicate")
        .output()
        .unwrap();
    assert!(!bad.status.success());
}

/// A failing report limit gives a non-zero exit code unless the scene
/// marks it as expected (or `--ignore-gates` is given).
#[test]
fn cli_exit_code_follows_the_gates() {
    let exe = env!("CARGO_BIN_EXE_vernon-cli");
    let dir = std::env::temp_dir().join(format!("vernon-cli-gates-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let scene = |expect: &str| {
        format!(
            r#"{{ "nodes": {{
                "0": {{ "type_id": "geo.car_panel", "name": "panel" }},
                "1": {{ "type_id": "cfd.vehicle_prep", "name": "prep", "inputs": [[0, 0], null, null]{expect} }} }} }}"#
        )
    };
    let run = |text: String, extra: &[&str]| {
        let vrn = dir.join("gates.vrn");
        std::fs::write(&vrn, text).unwrap();
        let out = std::process::Command::new(exe)
            .arg("cook")
            .arg(&vrn)
            .arg("--work")
            .arg(dir.join("work"))
            .args(extra)
            .output()
            .unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let (code, text) = run(scene(""), &[]);
    assert_eq!(code, Some(3), "{text}");
    assert!(text.contains("gates: 1 failing, 0 expected") && text.contains("[FAIL] prep / wheels_found"), "{text}");
    assert_eq!(run(scene(""), &["--ignore-gates"]).0, Some(0));
    let (code, text) = run(scene(r#", "expect_fail": { "wheels_found": "panel only" }"#), &[]);
    assert_eq!(code, Some(0), "{text}");
    assert!(text.contains("[expected] prep / wheels_found") && text.contains("panel only"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Metric names are snake case (the drag and lift areas keep their CdA and
/// ClA symbols) and units use the canonical spellings of `vernon_core::units`,
/// whichever stage wrote the report.
#[test]
fn demo_metric_names_and_units_follow_the_conventions() {
    let reg = vernon_cli::registry();
    let vrn = repo().join("examples/bodykit_pipeline.vrn");
    let mut graph = vernon_cli::load_graph(&vrn, &reg).expect("example loads");
    let work = std::env::temp_dir().join(format!("vernon-cli-names-{}", std::process::id()));
    graph.work_root = Some(work.clone());
    let outcomes = vernon_cli::cook_graph(&mut graph, &reg, None).expect("cook runs");
    let legacy = ["mm2", "mm3", "m2", "m3", "degC", "deg C", "deg", "-", "flag", "bool", "ratio", "W/m2", "kg/m3", "kWh/m2"];
    let mut bad = Vec::new();
    let mut seen = 0;
    for o in &outcomes {
        for r in o.reports() {
            for (name, m) in &r.metrics {
                seen += 1;
                let symbol = matches!(name.as_str(), "CdA" | "ClA");
                if !symbol && !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.') {
                    bad.push(format!("{}: metric name '{name}'", o.name));
                }
                if legacy.contains(&m.unit.as_str()) {
                    bad.push(format!("{}: {name} has unit '{}'", o.name, m.unit));
                }
                if m.value.is_sign_negative() && m.value == 0.0 {
                    bad.push(format!("{}: {name} is negative zero", o.name));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    assert!(seen > 100, "only {seen} metrics");
    assert!(bad.is_empty(), "{bad:#?}");
}
