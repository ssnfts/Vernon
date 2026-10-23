//! Clean-room guard for the whole product: no vendor or product name from
//! `vernon_core::clean_room` may reach anything a user reads. Covers every
//! registered node (labels, help, ports, parameter names, labels, units,
//! tooltips, text defaults and choice options), the report text and error
//! messages of the cooked demo pipeline, and the CLI node listing.

use std::path::PathBuf;

use vernon_core::clean_room::find_protected;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn no_node_shows_a_vendor_name() {
    let reg = vernon_cli::registry();
    let mut bad = Vec::new();
    let mut checked = 0;
    for info in reg.types() {
        for s in info.visible_strings() {
            checked += 1;
            if let Some(n) = find_protected(&s) {
                bad.push(format!("{}: '{s}' names {n}", info.id));
            }
        }
    }
    assert!(checked > 5000, "only {checked} strings scanned");
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn cooked_demo_text_and_the_cli_listing_are_clean() {
    let reg = vernon_cli::registry();
    let mut graph = vernon_cli::load_graph(&repo().join("examples/bodykit_pipeline.vrn"), &reg).unwrap();
    let work = std::env::temp_dir().join(format!("vernon-cli-cleanroom-{}", std::process::id()));
    graph.work_root = Some(work.clone());
    let outcomes = vernon_cli::cook_graph(&mut graph, &reg, None).unwrap();

    let mut bad = Vec::new();
    let mut check = |what: &str, text: &str| {
        if let Some(n) = find_protected(text) {
            bad.push(format!("{what}: '{text}' names {n}"));
        }
    };
    for o in &outcomes {
        check(&format!("{} name", o.name), &o.name);
        match &o.result {
            Err(e) => check(&format!("{} error", o.name), e),
            Ok(_) => {
                for r in o.reports() {
                    check(&format!("{} title", o.name), &r.title);
                    for (name, m) in &r.metrics {
                        check(&format!("{} metric", o.name), name);
                        check(&format!("{} unit of {name}", o.name), &m.unit);
                    }
                    for (_, text) in &r.notes {
                        check(&format!("{} note", o.name), text);
                    }
                }
            }
        }
    }
    check("cook listing", &vernon_cli::format_outcomes(&outcomes));
    let gates = vernon_cli::gate_failures(&outcomes, &graph);
    check("gate summary", &vernon_cli::format_gates(&gates));
    check("node listing", &vernon_cli::list_text(&reg, false));
    let _ = std::fs::remove_dir_all(&work);
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn the_listing_names_emulated_tools_only_on_request() {
    let reg = vernon_cli::registry();
    assert!(!vernon_cli::list_text(&reg, false).contains("emulates:"));
    assert!(vernon_cli::list_text(&reg, true).contains("emulates:"));
}
