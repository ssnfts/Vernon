//! Cross-stage consistency: the same material choice must give the same
//! numbers in every stage, read through the real nodes rather than the tables
//! behind them. Only this crate links every stage.

use vernon_core::materials::PRINT_MATERIALS;
use vernon_core::{keys, Report};
use vernon_graph::{Graph, NodeId, ParamValue, Registry};

fn report(graph: &mut Graph, reg: &Registry, id: NodeId, output: usize) -> std::sync::Arc<Report> {
    let data = graph.cook_output(reg, id, output).unwrap_or_else(|e| panic!("cook failed: {e}"));
    data.as_report().expect("a report output").clone()
}

fn value(r: &Report, name: &str) -> f64 {
    r.metrics.get(name).unwrap_or_else(|| panic!("{} has no metric {name}", r.title)).value
}

fn string(v: &str) -> ParamValue {
    ParamValue::Str(v.to_string())
}

/// `solar.hdt_check` on a small panel; `material_report` is wired in when given.
fn solar_check(graph: &mut Graph, reg: &Registry, name: &str, annealed: bool, material_report: Option<(NodeId, usize)>) -> NodeId {
    let grid = graph.add_node(reg, "geo.grid").unwrap();
    let hour = graph.add_node(reg, "solar.design_hour").unwrap();
    let balance = graph.add_node(reg, "solar.surface_balance").unwrap();
    graph.connect(grid, 0, balance, 0).unwrap();
    graph.connect(hour, 0, balance, 1).unwrap();
    let check = graph.add_node(reg, "solar.hdt_check").unwrap();
    graph.connect(balance, 0, check, 0).unwrap();
    graph.set_param(check, "material", string(name)).unwrap();
    graph.set_param(check, "annealed", ParamValue::Bool(annealed)).unwrap();
    if let Some((node, output)) = material_report {
        graph.connect(node, output, check, 2).unwrap();
    }
    check
}

#[test]
fn every_stage_reads_heat_deflection_from_the_shared_table() {
    let reg = vernon_cli::registry();
    for m in &PRINT_MATERIALS {
        for annealed in [false, true] {
            let want = m.hdt(annealed);
            let label = format!("{} annealed={annealed}", m.name);
            let mut g = Graph::new();

            let print = g.add_node(&reg, "print.heat_margin").unwrap();
            g.set_param(print, "material", string(m.id)).unwrap();
            g.set_param(print, "annealed", ParamValue::Bool(annealed)).unwrap();
            g.set_param(print, "surface_temp_c", ParamValue::Float(50.0)).unwrap();
            assert_eq!(value(&report(&mut g, &reg, print, 0), keys::HDT_C), want, "print: {label}");

            let check = solar_check(&mut g, &reg, m.name, annealed, None);
            let r = report(&mut g, &reg, check, 1);
            assert_eq!(value(&r, keys::HDT_C), want, "solar table: {label}");
            assert_eq!(value(&r, keys::ANNEALED), f64::from(annealed), "solar table: {label}");

            // The structural stage's card and derating node, then the wired
            // hand-off: the solar check must state the treatment it was given.
            let card = g.add_node(&reg, "fea.material_fdm").unwrap();
            g.set_param(card, "preset", string(m.id)).unwrap();
            let r = report(&mut g, &reg, card, 1);
            assert_eq!(value(&r, keys::HDT_045_C), m.hdt(false), "fea card: {label}");
            assert_eq!(value(&r, keys::ANNEALED), 0.0, "fea card: {label}");

            let derate = g.add_node(&reg, "fea.material_derate").unwrap();
            g.connect(card, 0, derate, 0).unwrap();
            g.set_param(derate, "annealed", ParamValue::Bool(annealed)).unwrap();
            let r = report(&mut g, &reg, derate, 2);
            assert_eq!(value(&r, keys::HDT_045_C), want, "fea derate: {label}");
            assert_eq!(value(&r, keys::ANNEALED), f64::from(annealed), "fea derate: {label}");

            // The check's own flag is deliberately the opposite: the report wins.
            let wired = solar_check(&mut g, &reg, m.name, !annealed, Some((derate, 2)));
            let r = report(&mut g, &reg, wired, 1);
            assert_eq!(value(&r, keys::HDT_C), want, "solar wired: {label}");
            assert_eq!(value(&r, keys::ANNEALED), f64::from(annealed), "solar wired: {label}");
        }
    }
}
