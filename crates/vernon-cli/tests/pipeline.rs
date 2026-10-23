//! Cooks `examples/bodykit_pipeline.vrn` end to end: every node must cook
//! without a panic or an error, and every pipeline stage must hand a Report
//! to the next one.

use std::collections::BTreeSet;
use std::path::PathBuf;

use vernon_graph::Stage;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
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
