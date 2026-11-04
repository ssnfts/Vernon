//! `docs/AGENT_GUIDE.md` must match the registry, stay short, and every
//! recipe in it must replay against the real node types.

use std::path::PathBuf;

use serde_json::json;
use vernon_agent_tools::{GraphToolHost, ToolHost};
use vernon_cli::agent_guide::{generate, recipe_calls, RECIPES};

fn committed() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/AGENT_GUIDE.md");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}; run `vernon-cli agent-guide > docs/AGENT_GUIDE.md`", p.display()))
}

#[test]
fn committed_guide_is_up_to_date() {
    let fresh = generate(&vernon_cli::registry()).expect("guide generates");
    assert_eq!(
        committed().replace("\r\n", "\n"),
        fresh,
        "docs/AGENT_GUIDE.md is stale: run `cargo run -p vernon-cli -- agent-guide > docs/AGENT_GUIDE.md`"
    );
}

#[test]
fn guide_fits_the_token_budget() {
    let g = generate(&vernon_cli::registry()).unwrap();
    // About four characters per token; the budget is 2000 tokens.
    assert!(g.len() < 7500, "guide is {} characters (~{} tokens)", g.len(), g.len() / 4);
    assert_eq!(RECIPES.len(), 6);
    for word in ["Conventions", "Gates", "Recipes", "begin_group"] {
        assert!(g.contains(word), "{word}");
    }
}

fn work() -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("vernon_guide_{}_{nanos}", std::process::id()))
}

#[test]
fn every_recipe_replays_against_the_real_registry() {
    for r in RECIPES {
        let mut host = GraphToolHost::new(vernon_cli::registry(), work());
        for (tool, args) in recipe_calls(r).unwrap() {
            host.call(tool, args.clone()).unwrap_or_else(|e| panic!("recipe '{}': {tool} {args} failed: {e}", r.title));
        }
    }
}

#[test]
fn the_cheap_recipes_cook() {
    for (title, last) in [("Scan, align, deviation", "gate"), ("Clean a mesh", "dec"), ("Split for the print bed", "fit")] {
        let r = RECIPES.iter().find(|r| r.title == title).unwrap();
        let mut host = GraphToolHost::new(vernon_cli::registry(), work());
        for (tool, args) in recipe_calls(r).unwrap() {
            host.call(tool, args).unwrap();
        }
        let out = host.call("cook", json!({ "node": last })).unwrap();
        let status = out["cooked"][0]["status"].as_str().unwrap();
        assert_eq!(status, "ok", "{title}: {out}");
    }
}
