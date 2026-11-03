//! Tier B: open-source reference tools, run as separate processes when present on this PC.
//! Every case is SKIPPED with a reason when its tool or recorded result is missing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use vernon_core::{process, DVec3, Geometry};
use vernon_graph::Data;

use crate::fea::*;
use crate::harness::*;

const TIER: &str = "B";

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.exists())
}

fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for n in names {
            for ext in ["", ".exe"] {
                let p = dir.join(format!("{n}{ext}"));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

pub fn run() -> Vec<Case> {
    vec![cloudcompare(), calculix(), openfoam_existing(), colmap_sample(), slicer_gcode()]
}

/// xorshift, so the synthetic cloud is identical on every machine.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn cloudcompare() -> Case {
    const REF: &str = "CloudCompare CLI (GPL-2.0+, separate process) C2M_DIST; command as in tests/golden/README.md";
    let Some(exe) = env_path("VERNON_CLOUDCOMPARE").or_else(|| on_path(&["CloudCompare"])).or_else(|| Some(PathBuf::from(r"C:\Program Files\CloudCompare\CloudCompare.exe")).filter(|p| p.is_file())) else {
        return skipped("B.cloudcompare.c2m", TIER, "Cloud-to-mesh deviation vs CloudCompare", REF, "CloudCompare not found (set VERNON_CLOUDCOMPARE)");
    };
    run_case("B.cloudcompare.c2m", TIER, "Cloud-to-mesh deviation vs CloudCompare (live run)", REF, |ck| {
        let dir = scratch("b_cloudcompare");
        let reference = out_geo(&cook("geo.sphere", &[], &[("radius", f(50.0)), ("rows", i(32)), ("cols", i(64))])?, 0)?;
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let pts: Vec<DVec3> = (0..3000)
            .map(|_| {
                let (u, v) = (rng.next() * 2.0 - 1.0, rng.next() * std::f64::consts::TAU);
                let s = (1.0 - u * u).sqrt();
                DVec3::new(s * v.cos(), s * v.sin(), u) * (50.0 + (rng.next() - 0.5) * 1.2)
            })
            .collect();
        let compared = Geometry::from_points(pts);
        vernon_io::write(&dir.join("compared.xyz"), &compared, &Default::default()).map_err(|e| e.to_string())?;
        vernon_io::write(&dir.join("reference.obj"), &reference, &Default::default()).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(dir.join("c2m.asc"));
        let args: Vec<String> = "-SILENT -AUTO_SAVE OFF -C_EXPORT_FMT ASC -PREC 9 -ADD_HEADER -O compared.xyz -O reference.obj -C2M_DIST -SAVE_CLOUDS FILE c2m.asc".split(' ').map(String::from).collect();
        let exit = process::run_program(&exe, &args, &dir, &dir.join("cloudcompare.log"), &process::RunOptions::with_timeout(Duration::from_secs(180))).map_err(|e| e.to_string())?;
        if exit != process::Exit::Code(0) || !dir.join("c2m.asc").is_file() {
            return Err(format!("CloudCompare did not produce c2m.asc ({exit:?}); see {}", dir.join("cloudcompare.log").display()));
        }
        let text = std::fs::read_to_string(dir.join("c2m.asc")).map_err(|e| e.to_string())?;
        let theirs: Vec<f64> = text.lines().filter(|l| !l.starts_with("//") && !l.trim().is_empty()).map(|l| l.split_whitespace().nth(3).and_then(|t| t.parse().ok()).unwrap_or(f64::NAN)).collect();
        let d = cook("clean.mesh_distance", &[geo_in(&compared), geo_in(&reference)], &[("signed", b(true))])?;
        let ours = out_geo(&d, 0)?.point_attribs.floats("dist").map_err(|e| e.to_string())?.to_vec();
        if ours.len() != theirs.len() {
            return Err(format!("point count differs: Vernon {}, CloudCompare {}", ours.len(), theirs.len()));
        }
        let worst = ours.iter().zip(&theirs).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
        ck.at_most("worst per-point difference (CloudCompare writes 32-bit floats)", worst, 1e-4, "mm");
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let rms = |v: &[f64]| (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt();
        ck.close("mean signed distance", mean(&ours), mean(&theirs), 0.0, 1e-5, "mm");
        ck.close("RMS distance", rms(&ours), rms(&theirs), 1e-4, 0.0, "mm");
        Ok(())
    })
}

/// How to launch CalculiX: a native executable, or inside WSL.
fn ccx_runner() -> Option<(PathBuf, Vec<String>)> {
    if let Some(p) = env_path("VERNON_CCX").or_else(|| on_path(&["ccx", "ccx_2.21", "ccx_2.20", "ccx_static"])) {
        return Some((p, vec![]));
    }
    let wsl = on_path(&["wsl"])?;
    let probe = scratch("b_ccx_probe").join("probe.log");
    let ok = process::run_program(&wsl, &["-e".into(), "sh".into(), "-c".into(), "command -v ccx".into()], Path::new("."), &probe, &process::RunOptions::with_timeout(Duration::from_secs(60))).ok()?;
    let found = std::fs::read(&probe).map(|b| String::from_utf8_lossy(&b).replace('\0', "").contains("ccx")).unwrap_or(false);
    (ok == process::Exit::Code(0) && found).then(|| (wsl, vec!["-e".into(), "ccx".into()]))
}

fn calculix() -> Case {
    const REF: &str = "CalculiX 2.x (GPL-2.0, separate process, native or inside WSL) on the deck written by Vernon (fea.deck_write)";
    let Some((exe, prefix)) = ccx_runner() else {
        return skipped("B.calculix.cantilever", TIER, "Cantilever FEA vs CalculiX", REF, "CalculiX not found on PATH or in WSL (set VERNON_CCX)");
    };
    run_case("B.calculix.cantilever", TIER, "Cantilever tip deflection and stress: Vernon solver vs CalculiX (C3D10)", REF, move |ck| {
        use vernon_nodes_fea::deck::{write_deck, DeckInput, DeckOptions};
        use vernon_nodes_fea::frd::{dense_tensor6, dense_vec3, parse_frd};
        use vernon_nodes_fea::linalg::von_mises;
        use vernon_nodes_fea::types::OrientationCard;
        let mesh = mesh_tet10(2);
        let (c, id, m, l) = (card(), OrientationCard::identity(), root_mount(&mesh), tip_load(&mesh));
        let ours = solve_cantilever(&mesh, 300_000)?;
        let dir = scratch("b_calculix");
        for ext in ["frd", "dat", "sta", "cvg"] {
            let _ = std::fs::remove_file(dir.join(format!("cantilever.{ext}")));
        }
        let opts = DeckOptions { job: "cantilever".into(), allow_linear_tet: true, ..Default::default() };
        let deck = write_deck(&DeckInput { mesh: &mesh, card: &c, orient: &id, mounts: &m, load: &l, opts: &opts })?;
        std::fs::write(dir.join("cantilever.inp"), &deck.text).map_err(|e| e.to_string())?;
        let (program, mut args, cwd) = if prefix.is_empty() {
            (exe.clone(), vec![], dir.clone())
        } else {
            let mut a = vec!["--cd".to_string(), dir.to_string_lossy().into_owned()];
            a.extend(prefix.clone());
            (exe.clone(), a, PathBuf::from("."))
        };
        args.extend(["-i".into(), "cantilever".into()]);
        let exit = process::run_program(&program, &args, &cwd, &dir.join("ccx.log"), &process::RunOptions::with_timeout(Duration::from_secs(600))).map_err(|e| e.to_string())?;
        if exit != process::Exit::Code(0) {
            return Err(format!("CalculiX exited with {exit:?}; see {}", dir.join("ccx.log").display()));
        }
        let frd = parse_frd(&std::fs::read_to_string(dir.join("cantilever.frd")).map_err(|e| e.to_string())?)?;
        let n = mesh.points.len();
        let needed = vec![true; n];
        let u = dense_vec3(frd.dataset("DISP").ok_or("no DISP")?, n, &needed)?;
        let s = dense_tensor6(frd.dataset("STRESS").ok_or("no STRESS")?, n, &needed)?;
        ck.close("tip deflection, Vernon vs CalculiX", tip_deflection(&mesh, &ours.disp), tip_deflection(&mesh, &u), 0.02, 0.0, "mm");
        let peak = |v: &[[f64; 6]]| nodes_at(&mesh, 0.0).iter().map(|&k| von_mises(&v[k as usize])).fold(0.0, f64::max);
        ck.close("mid-span von Mises, Vernon vs CalculiX", peak(&ours.node_stress), peak(&s), 0.02, 0.0, "MPa");
        ck.close("CalculiX tip deflection vs beam theory", tip_deflection(&mesh, &u), beam_theory(), 0.03, 0.0, "mm");
        Ok(())
    })
}

fn openfoam_existing() -> Case {
    const REF: &str = "OpenFOAM v2606 (GPL-3.0) forceCoeffs output of an existing run; no CFD job is started by the bench";
    let dir = std::env::var_os("VERNON_AHMED_RESULTS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\mabdu\REA\vernon\scratch\wp9-ahmed"));
    let json = dir.join("a25-g3.json");
    if !json.is_file() {
        return skipped("B.openfoam.force_reduction", TIER, "Cd reduction vs OpenFOAM forceCoeffs", REF, "no existing OpenFOAM result (set VERNON_AHMED_RESULTS)");
    }
    run_case("B.openfoam.force_reduction", TIER, "Vernon's reported Cd vs an independent average of OpenFOAM's forceCoeffs history (existing run, read-only)", REF, |ck| {
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&json).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let case = PathBuf::from(v["case"].as_str().ok_or("no case path in result")?);
        let fc = case.join("postProcessing/forceCoeffs1");
        let dat = std::fs::read_dir(&fc).ok().and_then(|d| d.flatten().map(|e| e.path().join("coefficient.dat")).find(|p| p.is_file()));
        let Some(dat) = dat else {
            ck.skip(format!("forceCoeffs history not found under {}", fc.display()));
            return Ok(());
        };
        let text = std::fs::read_to_string(dat).map_err(|e| e.to_string())?;
        let cds: Vec<f64> = text.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_whitespace().nth(1)?.parse().ok()).collect();
        let window = v["window"].as_f64().unwrap_or(1000.0) as usize;
        let tail = &cds[cds.len().saturating_sub(window)..];
        let mean = tail.iter().sum::<f64>() / tail.len() as f64;
        ck.close("Cd: mean of last window in coefficient.dat vs value Vernon reported", mean, v["cd"].as_f64().ok_or("no cd")?, 0.01, 0.0, "");
        ck.note("this checks Vernon's reduction of OpenFOAM output, not the physics of the run (the run did not converge, see A.cfd.ahmed_drag_window)");
        Ok(())
    })
}

fn colmap_sample() -> Case {
    const REF: &str = "COLMAP 4.2.1 (BSD-3-Clause) sparse model recorded from the WP8 public sample run (south-building)";
    let dir = std::env::var_os("VERNON_COLMAP_MODEL").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Users\mabdu\REA\vernon\scratch\continuation\runs\wp8-real-20261030-b\temp\vernon-wp8-real-63600-1793347686143423200\sparse\sparse\0"));
    if !dir.join("points3D.txt").is_file() {
        return skipped("B.colmap.sample_model", TIER, "COLMAP sample model ingest", REF, "no recorded COLMAP model (set VERNON_COLMAP_MODEL); COLMAP is not re-run by the bench, it needs a GPU and hours");
    }
    run_case("B.colmap.sample_model", TIER, "Vernon reads the recorded COLMAP sparse model faithfully and it is of usable quality", REF, |ck| {
        let text = std::fs::read_to_string(dir.join("points3D.txt")).map_err(|e| e.to_string())?;
        let rows: Vec<&str> = text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).collect();
        let pts = vernon_nodes_capture::colmap_model::parse_points3d_txt(&text)?;
        ck.close("point count read by Vernon vs data lines in file", pts.len() as f64, rows.len() as f64, 0.0, 0.0, "");
        let mean_err = pts.iter().map(|p| p.error).sum::<f64>() / pts.len().max(1) as f64;
        ck.at_most("mean reprojection error reported by COLMAP", mean_err, 1.0, "px");
        let short = pts.iter().filter(|p| p.track_len < 2).count();
        ck.close("points with a track shorter than 2 views", short as f64, 0.0, 0.0, 0.0, "");
        ck.at_least("sparse points reconstructed", pts.len() as f64, 1000.0, "points");
        Ok(())
    })
}

fn slicer_gcode() -> Case {
    const REF: &str = "OrcaSlicer (AGPL-3.0) / PrusaSlicer, separate process; G-code accepted by print.flavour_check";
    let orca = env_path("VERNON_SLICER").or_else(|| on_path(&["orca-slicer", "prusa-slicer-console", "prusa-slicer"])).or_else(|| Some(PathBuf::from(r"C:\Program Files\OrcaSlicer\orca-slicer.exe")).filter(|p| p.is_file()));
    let Some(exe) = orca else {
        return skipped("B.slicer.flavour_check", TIER, "Slicer G-code accepted by flavour_check", REF, "no slicer found (set VERNON_SLICER)");
    };
    run_case("B.slicer.flavour_check", TIER, "G-code from a real slicer run is accepted by print.flavour_check", REF, |ck| {
        let dir = scratch("b_slicer");
        let part = repo_root().join("tests/golden/orcaslicer/box_3mf/part.3mf");
        for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
            if e.path().extension().is_some_and(|x| x == "gcode") {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let args: Vec<String> = vec!["--slice".into(), "0".into(), "--outputdir".into(), dir.to_string_lossy().into_owned(), part.to_string_lossy().into_owned()];
        let exit = process::run_program(&exe, &args, &dir, &dir.join("slicer.log"), &process::RunOptions::with_timeout(Duration::from_secs(180))).map_err(|e| e.to_string())?;
        let gcode = std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "gcode"));
        let Some(gcode) = gcode else {
            ck.skip(format!("the slicer produced no G-code (exit {exit:?}); command-line slicing needs a full exported printer/process/filament profile set, see tests/golden/README.md"));
            return Ok(());
        };
        let mut best = None;
        for flavour in ["marlin2", "klipper"] {
            let o = cook("print.flavour_check", &[Some(Data::File(gcode.clone()))], &[("flavour", s(flavour))])?;
            let v = o.first().and_then(|d| d.as_json()).map(|j| j.as_array().map(|a| a.len()).unwrap_or(0)).unwrap_or(usize::MAX);
            ck.note(format!("{flavour}: {v} violation(s)"));
            best = Some(best.map_or(v, |b: usize| b.min(v)));
        }
        ck.close("violations in the best-matching dialect", best.unwrap_or(usize::MAX) as f64, 0.0, 0.0, 0.0, "count");
        Ok(())
    })
}
