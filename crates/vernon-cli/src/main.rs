//! `vernon-cli list [--markdown] [--docs]` and
//! `vernon-cli cook <file.vrn> [--node NAME] [--out mesh.obj] [--work DIR] [--ignore-gates]`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};

const USAGE: &str = "usage:
  vernon-cli list [--markdown] [--docs]
  vernon-cli info <node.type> [...]
  vernon-cli cook <file.vrn> [--node NAME] [--out mesh.obj] [--work DIR] [--ignore-gates]

list prints the public tool behaviours each node re-implements only with
--docs (they are documentation, not part of the user interface).

cook exits with 1 when a node fails and 3 when a report limit fails that
the scene does not list under the node's expect_fail (--ignore-gates: 0).";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Vec<String>) -> Result<ExitCode> {
    let reg = vernon_cli::registry();
    match args.first().map(String::as_str) {
        Some("list") => {
            let (mut md, mut docs) = (false, false);
            for a in &args[1..] {
                match a.as_str() {
                    "--markdown" => md = true,
                    "--docs" => docs = true,
                    other => bail!("unknown option '{other}'\n{USAGE}"),
                }
            }
            if md {
                print!("{}", vernon_cli::list_markdown(&reg));
            } else {
                print!("{}", vernon_cli::list_text(&reg, docs));
            }
            Ok(ExitCode::SUCCESS)
        }
        Some("info") => {
            if args.len() < 2 {
                bail!("info needs a node type id\n{USAGE}");
            }
            for id in &args[1..] {
                let info = reg
                    .info(id)
                    .with_context(|| format!("unknown node type '{id}'"))?;
                print!("{}", vernon_cli::format_info(info));
            }
            Ok(ExitCode::SUCCESS)
        }
        Some("cook") => {
            let mut file = None::<PathBuf>;
            let mut node = None::<String>;
            let mut out = None::<PathBuf>;
            let mut work = None::<PathBuf>;
            let mut ignore_gates = false;
            let mut it = args.iter().skip(1);
            while let Some(a) = it.next() {
                let mut value = |flag: &str| {
                    it.next()
                        .cloned()
                        .with_context(|| format!("{flag} needs a value"))
                };
                match a.as_str() {
                    "--node" => node = Some(value("--node")?),
                    "--out" => out = Some(value("--out")?.into()),
                    "--work" => work = Some(value("--work")?.into()),
                    "--ignore-gates" => ignore_gates = true,
                    f if f.starts_with("--") => bail!("unknown option '{f}'\n{USAGE}"),
                    f if file.is_none() => file = Some(f.into()),
                    f => bail!("unexpected argument '{f}'\n{USAGE}"),
                }
            }
            let Some(file) = file else {
                bail!("cook needs a .vrn file\n{USAGE}")
            };
            let mut graph = vernon_cli::load_graph(&file, &reg)?;
            graph.work_root = Some(work.unwrap_or_else(|| vernon_cli::default_work_root(&file)));
            let outcomes = vernon_cli::cook_graph(&mut graph, &reg, node.as_deref())?;
            print!("{}", vernon_cli::format_outcomes(&outcomes));
            let gates = vernon_cli::gate_failures(&outcomes, &graph);
            print!("{}", vernon_cli::format_gates(&gates));
            if let Some(out) = out {
                let (name, data) = vernon_cli::export_geometry(&outcomes, &graph, node.as_deref())
                    .context("no cooked node produced geometry to write")?;
                vernon_cli::write_geometry(&data, &out)?;
                println!("wrote {} from {name}", out.display());
            }
            let failed = outcomes.iter().any(|o| o.result.is_err());
            let gate_failed = gates.iter().any(|g| g.expected.is_none());
            Ok(if failed {
                ExitCode::FAILURE
            } else if gate_failed && !ignore_gates {
                ExitCode::from(3)
            } else {
                ExitCode::SUCCESS
            })
        }
        Some("-h" | "--help" | "help") | None => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => bail!("unknown command '{other}'\n{USAGE}"),
    }
}
