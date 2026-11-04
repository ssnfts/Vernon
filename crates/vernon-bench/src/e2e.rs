//! End-to-end: the example pipeline, the desktop app start probe and a per-stage CLI smoke.

use std::path::PathBuf;
use std::time::Duration;

use vernon_core::process;

use crate::harness::*;
use crate::tier_b::repo_root;

const TIER: &str = "E";
const PIPELINE: &str = "examples/bodykit_pipeline.vrn";
/// Counts documented in AGENTS.md for the shipped example.
const EXPECT_NODES: usize = 38;
const EXPECT_EXPECTED_GATES: usize = 2;

pub fn run() -> Vec<Case> {
    vec![cook_pipeline(), stage_smoke(), app_probe()]
}

fn cook_pipeline() -> Case {
    run_case("E.pipeline.cook", TIER, "Cook examples/bodykit_pipeline.vrn in-process", "AGENTS.md definition of done: 38 nodes, 0 failing gates, 2 expected (the brief for this suite quoted 40 nodes / 5 expected; the shipped file has 38 / 2)", |ck| {
        let reg = vernon_cli::registry();
        let mut graph = vernon_cli::load_graph(&repo_root().join(PIPELINE), &reg).map_err(|e| format!("{e:#}"))?;
        graph.work_root = Some(scratch("e2e_pipeline"));
        let outcomes = vernon_cli::cook_graph(&mut graph, &reg, None).map_err(|e| format!("{e:#}"))?;
        let failed = outcomes.iter().filter(|o| o.result.is_err()).count();
        let gates = vernon_cli::gate_failures(&outcomes, &graph);
        let unexpected = gates.iter().filter(|g| g.expected.is_none()).count();
        ck.close("nodes cooked", outcomes.len() as f64, EXPECT_NODES as f64, 0.0, 0.0, "nodes");
        ck.close("nodes failed", failed as f64, 0.0, 0.0, 0.0, "nodes");
        ck.close("unexpected failing gates", unexpected as f64, 0.0, 0.0, 0.0, "gates");
        ck.close("expected (documented) gate failures", (gates.len() - unexpected) as f64, EXPECT_EXPECTED_GATES as f64, 0.0, 0.0, "gates");
        Ok(())
    })
}

fn sibling(name: &str) -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let p = me.parent()?.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    p.is_file().then_some(p)
}

fn stage_smoke() -> Case {
    const REF: &str = "scripted `vernon-cli cook --node <last node of each stage>`; exit code 0 required";
    let Some(cli) = sibling("vernon-cli") else {
        return skipped("E.pipeline.stage_smoke", TIER, "Stage-by-stage CLI smoke", REF, "vernon-cli binary not next to vernon-bench (cargo build --release -p vernon-cli)");
    };
    run_case("E.pipeline.stage_smoke", TIER, "Stage-by-stage CLI smoke through the vernon-cli binary", REF, |ck| {
        let text = std::fs::read_to_string(repo_root().join(PIPELINE)).map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let mut last: Vec<(String, String)> = Vec::new();
        let mut entries: Vec<(&String, &serde_json::Value)> = v["nodes"].as_object().ok_or("no nodes")?.iter().collect();
        entries.sort_by_key(|(k, _)| k.parse::<u64>().unwrap_or(0));
        for (_, n) in entries {
            let (ty, name) = (n["type_id"].as_str().unwrap_or(""), n["name"].as_str().unwrap_or(""));
            let stage = ty.split('.').next().unwrap_or("").to_string();
            match last.iter_mut().find(|(s, _)| *s == stage) {
                Some(e) => e.1 = name.to_string(),
                None => last.push((stage, name.to_string())),
            }
        }
        for (stage, node) in last {
            let work = scratch(&format!("e2e_smoke_{stage}"));
            let args: Vec<String> = vec!["cook".into(), repo_root().join(PIPELINE).to_string_lossy().into_owned(), "--node".into(), node.clone(), "--work".into(), work.to_string_lossy().into_owned()];
            let exit = process::run_program(&cli, &args, &repo_root(), &work.join("cli.log"), &process::RunOptions::with_timeout(Duration::from_secs(300))).map_err(|e| e.to_string())?;
            let code = match exit {
                process::Exit::Code(c) => c as f64,
                _ => -1.0,
            };
            ck.close(&format!("stage {stage} (cook --node {node}) exit code"), code, 0.0, 0.0, 0.0, "");
        }
        Ok(())
    })
}

fn app_probe() -> Case {
    const REF: &str = "launch the desktop binary, require it to stay alive for 6 s (window opened), then stop it";
    let app = std::env::var_os("VERNON_APP").map(PathBuf::from).filter(|p| p.is_file()).or_else(|| sibling("vernon"));
    let Some(app) = app else {
        return skipped("E.app.start_probe", TIER, "Desktop app start probe", REF, "app binary not built (cargo build --release -p vernon) and VERNON_APP not set");
    };
    if std::env::var_os("VERNON_BENCH_NO_APP").is_some() {
        return skipped("E.app.start_probe", TIER, "Desktop app start probe", REF, "disabled by VERNON_BENCH_NO_APP (needs a display)");
    }
    run_case("E.app.start_probe", TIER, "Desktop app starts and stays up", REF, |ck| {
        let mut child = std::process::Command::new(&app).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_secs(6));
        let alive = child.try_wait().map_err(|e| e.to_string())?.is_none();
        process::kill_tree(&mut child);
        let _ = child.wait();
        ck.close("app still running after 6 s (1 = yes)", if alive { 1.0 } else { 0.0 }, 1.0, 0.0, 0.0, "bool");
        Ok(())
    })
}
