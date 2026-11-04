//! Tier C: golden slots for licensed commercial tools. The owner records outputs from their own
//! licensed copy (CLEAN_ROOM.md, allowed source 3); the harness compares them within stated tolerances.
//!
//! Layout: `bench/golden/<tool-slug>/golden.json` (schema in `bench/golden/README.md`). With no
//! `golden.json` the slot reports SKIPPED(no golden). Vendor install files are never opened.

use serde_json::Value;

use crate::harness::*;
use crate::tier_b::repo_root;

const TIER: &str = "C";

/// Tools the owner may hold licences for. One slot folder each.
pub const SLOTS: &[(&str, &str)] = &[
    ("rhino", "Rhino (mesh tools, QuadRemesh, SubD)"),
    ("fusion", "Fusion (mesh to solid)"),
    ("quicksurface", "reverse-engineering surfacing"),
    ("geomagic-designx", "reverse-engineering surfacing"),
    ("zeiss-inspect", "scan deviation and inspection"),
    ("airshaper", "road-vehicle CFD service"),
    ("simscale", "CFD / FEA service"),
    ("ansys", "FEA / CFD solver"),
    ("optimumlap", "point-mass lap simulation"),
    ("ladybug-tools", "solar and radiation analysis"),
];

pub fn run() -> Vec<Case> {
    let root = repo_root().join("bench/golden");
    SLOTS.iter().map(|(slug, what)| slot(&root, slug, what)).collect()
}

fn slot(root: &std::path::Path, slug: &str, what: &str) -> Case {
    let id = format!("C.{slug}");
    let title = format!("Golden slot: {what}");
    let file = root.join(slug).join("golden.json");
    if !file.is_file() {
        return skipped(&id, TIER, &title, "owner-recorded output of a licensed copy", "no golden");
    }
    run_case(&id, TIER, &title, "owner-recorded output of a licensed copy", |ck| {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).map_err(|e| e.to_string())?).map_err(|e| format!("golden.json: {e}"))?;
        let tool = v["tool"].as_str().unwrap_or("?");
        let version = v["tool_version"].as_str().unwrap_or("?");
        ck.note(format!("recorded from {tool} {version} on {}", v["recorded_on"].as_str().unwrap_or("?")));
        let cases = v["cases"].as_array().ok_or("golden.json has no 'cases' array")?;
        let mut done = 0;
        for c in cases {
            let name = c["name"].as_str().unwrap_or("?");
            let node = c["node"].as_str().ok_or_else(|| format!("{name}: no node"))?;
            let mut inputs = Vec::new();
            for rel in c["inputs"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                let g = vernon_io::read(&root.join(slug).join(rel), &Default::default()).map_err(|e| format!("{name}: {rel}: {e}"))?;
                inputs.push(geo_in(&g));
            }
            let mut set: Vec<(String, vernon_graph::ParamValue)> = Vec::new();
            for (k, pv) in c["params"].as_object().into_iter().flatten() {
                let val = match pv {
                    Value::Bool(x) => b(*x),
                    Value::Number(n) if n.is_i64() => i(n.as_i64().unwrap()),
                    Value::Number(n) => f(n.as_f64().unwrap_or(0.0)),
                    Value::String(x) => s(x),
                    _ => return Err(format!("{name}: unsupported parameter type for '{k}'")),
                };
                set.push((k.clone(), val));
            }
            let set_ref: Vec<(&str, vernon_graph::ParamValue)> = set.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            let out = cook(node, &inputs, &set_ref)?;
            let rep = out_report(&out)?;
            for exp in c["expected"].as_array().into_iter().flatten() {
                let metric_name = exp["metric"].as_str().ok_or_else(|| format!("{name}: expected entry without metric"))?;
                let want = exp["value"].as_f64().ok_or_else(|| format!("{name}/{metric_name}: no value"))?;
                let rel = exp["tol_rel"].as_f64().unwrap_or(0.0);
                let abs = exp["tol_abs"].as_f64().unwrap_or(0.0);
                if rel == 0.0 && abs == 0.0 {
                    return Err(format!("{name}/{metric_name}: a tolerance is required"));
                }
                let got = metric(&rep, metric_name)?;
                ck.close(&format!("{name}: {metric_name}"), got, want, rel, abs, exp["unit"].as_str().unwrap_or(""));
                done += 1;
            }
        }
        if done == 0 {
            return Err("golden.json lists no expected values".into());
        }
        Ok(())
    })
}
