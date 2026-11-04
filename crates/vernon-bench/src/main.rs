//! `vernon-bench run [--quick] [--no-perf] [--update-baseline] [--out DIR]` runs the whole suite and
//! writes `REPORT.md` and `results.json`. Statuses are only PASS, FAIL and SKIPPED(reason).

mod e2e;
mod fea;
mod harness;
mod meshes;
mod perf;
mod report;
mod tier_a;
mod tier_b;
mod tier_c;

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("perf-case") if args.len() == 3 => match perf::child(&args[1], args[2].parse().unwrap_or(0)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Some("run") | None => run(&args),
        _ => {
            eprintln!("usage: vernon-bench run [--quick] [--no-perf] [--update-baseline] [--out DIR]");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> ExitCode {
    let flag = |f: &str| args.iter().any(|a| a == f);
    let out: PathBuf = args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)).map(PathBuf::from).unwrap_or_else(|| tier_b::repo_root().join("bench/results"));
    let mut cases = Vec::new();
    for (name, f) in [("Tier A", tier_a::run as fn() -> Vec<harness::Case>), ("Tier B", tier_b::run), ("Tier C", tier_c::run), ("End to end", e2e::run)] {
        eprintln!("== {name}");
        for c in f() {
            eprintln!("{:8} {} {}", c.status.word(), c.id, c.status.detail());
            cases.push(c);
        }
    }
    if flag("--no-perf") {
        eprintln!("== performance skipped (--no-perf)");
    } else {
        eprintln!("== Performance");
        let sizes: Vec<usize> = if flag("--quick") { perf::SIZES[..2].to_vec() } else { perf::SIZES.to_vec() };
        for c in perf::run(&sizes, flag("--update-baseline")) {
            eprintln!("{:8} {} {}", c.status.word(), c.id, c.status.detail());
            cases.push(c);
        }
    }
    if let Err(e) = report::write(&out, &cases) {
        eprintln!("could not write the report: {e}");
        return ExitCode::from(2);
    }
    let fails = cases.iter().filter(|c| matches!(c.status, harness::Status::Fail(_))).count();
    eprintln!("report: {}", out.join("REPORT.md").display());
    if fails > 0 { ExitCode::from(1) } else { ExitCode::SUCCESS }
}
