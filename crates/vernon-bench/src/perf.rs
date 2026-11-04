//! Performance bench: wall time and peak memory on synthetic meshes. Each measurement runs in a
//! child process (`vernon-bench perf-case <case> <size>`), so peak memory is per case.
//! Targets are stated here, before any measurement; baselines live in `bench/baseline.json`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vernon_core::process;
use vernon_core::{DMat4, DVec3};

use crate::fea::*;
use crate::harness::*;
use crate::meshes::*;
use crate::tier_b::repo_root;

const TIER: &str = "P";
pub const SIZES: [usize; 3] = [10_000, 100_000, 1_000_000];
pub const CASES: [&str; 6] = ["icp", "deviation", "decimate", "flatten", "quad_remesh", "fea_solve"];

/// Wall-time targets in seconds for 10k / 100k / 1M triangles (tetrahedra for `fea_solve`).
fn target_s(case: &str, size: usize) -> f64 {
    let t = match case {
        "icp" => [2.0, 20.0, 200.0],
        "deviation" => [1.0, 10.0, 120.0],
        "decimate" => [1.0, 10.0, 120.0],
        "flatten" => [2.0, 30.0, 300.0],
        "quad_remesh" => [5.0, 60.0, 300.0],
        _ => [2.0, 30.0, 300.0],
    };
    t[SIZES.iter().position(|s| *s == size).unwrap_or(2)]
}
/// Peak-memory targets in MB.
fn target_mb(size: usize) -> f64 {
    match size {
        10_000 => 500.0,
        100_000 => 2_000.0,
        _ => 8_000.0,
    }
}
fn timeout_s(size: usize) -> u64 {
    if size >= 1_000_000 { 600 } else { 300 }
}

pub fn targets_table() -> String {
    let mut s = String::from("| Case | 10k | 100k | 1M | Peak memory |\n|---|---|---|---|---|\n");
    for c in CASES {
        s += &format!("| {c} | {} s | {} s | {} s | {} / {} / {} MB |\n", target_s(c, SIZES[0]), target_s(c, SIZES[1]), target_s(c, SIZES[2]), target_mb(SIZES[0]), target_mb(SIZES[1]), target_mb(SIZES[2]));
    }
    s
}

#[cfg(windows)]
pub fn peak_mb() -> f64 {
    #[repr(C)]
    struct Pmc {
        cb: u32,
        page_faults: u32,
        peak_ws: usize,
        ws: usize,
        a: usize,
        b: usize,
        c: usize,
        d: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(h: isize, p: *mut Pmc, cb: u32) -> i32;
    }
    let mut p = Pmc { cb: std::mem::size_of::<Pmc>() as u32, page_faults: 0, peak_ws: 0, ws: 0, a: 0, b: 0, c: 0, d: 0, pagefile: 0, peak_pagefile: 0 };
    // SAFETY: plain Win32 call with a correctly sized, initialised out-structure.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut p, p.cb) };
    if ok == 0 { f64::NAN } else { p.peak_ws as f64 / 1048576.0 }
}
#[cfg(not(windows))]
pub fn peak_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|t| t.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok()))
        .map(|kb| kb / 1024.0)
        .unwrap_or(f64::NAN)
}

/// Child entry point: build the input (not timed), time the operation, print one JSON line.
pub fn child(case: &str, size: usize) -> Result<(), String> {
    let (wall, actual) = match case {
        "icp" => {
            let reference = heightfield(size, 400.0);
            let mut moving = reference.clone();
            moving.transform(DMat4::from_translation(DVec3::new(1.0, -0.8, 0.5)) * DMat4::from_rotation_z(2f64.to_radians()));
            let t = Instant::now();
            cook("clean.icp_align", &[geo_in(&moving), geo_in(&reference)], &[("max_iterations", i(50))])?;
            (t.elapsed(), reference.triangulated().len())
        }
        "deviation" => {
            let reference = heightfield(size, 400.0);
            let mut compared = reference.clone();
            compared.transform(DMat4::from_translation(DVec3::new(0.0, 0.0, 0.1)));
            let t = Instant::now();
            cook("clean.mesh_distance", &[geo_in(&compared), geo_in(&reference)], &[("signed", b(true))])?;
            (t.elapsed(), reference.triangulated().len())
        }
        "decimate" | "quad_remesh" => {
            let cols = (size as f64).sqrt().round() as i64;
            let g = out_geo(&cook("geo.sphere", &[], &[("radius", f(100.0)), ("rows", i(cols / 2)), ("cols", i(cols))])?, 0)?;
            let t = Instant::now();
            if case == "decimate" {
                cook("clean.decimate", &[geo_in(&g)], &[("mode", s("ratio")), ("ratio", f(0.5)), ("measure", b(false))])?;
            } else {
                cook("surface.quad_remesh", &[geo_in(&g), None], &[("target_quads", i(5000))])?;
            }
            (t.elapsed(), g.triangulated().len())
        }
        "flatten" => {
            let g = heightfield(size, 400.0);
            let t = Instant::now();
            cook("flatten.conformal_map", &[geo_in(&g), None], &[])?;
            (t.elapsed(), g.triangulated().len())
        }
        "fea_solve" => {
            // 60 n^3 linear tetrahedra
            let n = ((size as f64 / 60.0).cbrt().round() as usize).max(1);
            let mesh = mesh_tet4(n);
            let t = Instant::now();
            let o = solve_cantilever(&mesh, 10_000_000)?;
            let _ = o.max_disp();
            (t.elapsed(), mesh.prims.len())
        }
        other => return Err(format!("unknown perf case {other}")),
    };
    println!("{}", json!({"case": case, "size": size, "actual": actual, "wall_s": wall.as_secs_f64(), "peak_mb": peak_mb()}));
    Ok(())
}

fn baseline_path() -> std::path::PathBuf {
    repo_root().join("bench/baseline.json")
}

pub fn run(sizes: &[usize], update_baseline: bool) -> Vec<Case> {
    let me = std::env::current_exe().expect("current exe");
    let baseline: Value = std::fs::read_to_string(baseline_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    let mut new_baseline = baseline.clone();
    let mut out = Vec::new();
    for case in CASES {
        for size in SIZES {
            let id = format!("P.{case}.{}k", size / 1000);
            let title = format!("{case} at {} triangles (tetrahedra for fea_solve)", size);
            let reference = format!("target {} s and {} MB, stated before measuring; baseline in bench/baseline.json", target_s(case, size), target_mb(size));
            if !sizes.contains(&size) {
                out.push(skipped(&id, TIER, &title, &reference, "size not requested (quick mode)"));
                continue;
            }
            let work = scratch(&format!("perf_{case}_{size}"));
            let log = work.join("child.log");
            let t = Instant::now();
            let exit = process::run_program(&me, &["perf-case".into(), case.into(), size.to_string()], &work, &log, &process::RunOptions::with_timeout(Duration::from_secs(timeout_s(size))));
            let wall_total = t.elapsed().as_secs_f64();
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let line: Option<Value> = text.lines().rev().find(|l| l.starts_with('{')).and_then(|l| serde_json::from_str(l).ok());
            let mut c = run_case(&id, TIER, &title, &reference, |ck| {
                match (&exit, &line) {
                    (Ok(process::Exit::TimedOut), _) => return Err(format!("timed out after {} s (target {} s)", timeout_s(size), target_s(case, size))),
                    (_, Some(v)) => {
                        let (wall, mb) = (v["wall_s"].as_f64().unwrap_or(f64::NAN), v["peak_mb"].as_f64().unwrap_or(f64::NAN));
                        ck.note(format!("actual input size {} ; wall {wall:.2} s ; peak {mb:.0} MB", v["actual"]));
                        ck.at_most("wall time", wall, target_s(case, size), "s");
                        ck.at_most("peak memory", mb, target_mb(size), "MB");
                        if let Some(b) = baseline.get(&id) {
                            let (bw, bm) = (b["wall_s"].as_f64().unwrap_or(0.0), b["peak_mb"].as_f64().unwrap_or(0.0));
                            ck.at_most("wall time vs baseline (1.5x + 0.25 s)", wall, 1.5 * bw + 0.25, "s");
                            ck.at_most("peak memory vs baseline (1.3x + 20 MB)", mb, 1.3 * bm + 20.0, "MB");
                        } else {
                            ck.note("no baseline stored for this case yet");
                        }
                    }
                    _ => return Err(format!("child failed ({exit:?}): {}", process::tail(&log, 3).replace('\n', " | "))),
                }
                Ok(())
            });
            c.ms = wall_total * 1e3;
            if let Some(v) = &line {
                if update_baseline {
                    new_baseline[&id] = json!({"wall_s": v["wall_s"], "peak_mb": v["peak_mb"]});
                }
            }
            out.push(c);
        }
    }
    if update_baseline {
        let _ = std::fs::write(baseline_path(), serde_json::to_string_pretty(&new_baseline).unwrap_or_default());
    }
    out
}
