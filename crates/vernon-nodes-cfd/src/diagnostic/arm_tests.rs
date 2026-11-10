//! Unit tests of the checkpoint arms (no OpenFOAM needed).

use super::arms::*;
use super::*;
use std::collections::BTreeMap;
use std::path::Path;

/// The production fvSolution exactly as `foam::fv_solution` writes it for the
/// Stage 2 recipe (copied from the rerun case `wp9-solve-fastback-20261110-a`).
const PROD: &str = "FoamFile\n{\n    version     2.0;\n    format      ascii;\n    class       dictionary;\n    location    \"system\";\n    object      fvSolution;\n}\n\nsolvers\n{\n    p\n    {\n        solver          GAMG;\n        smoother        GaussSeidel;\n        tolerance       1e-10;\n        relTol          0.1;\n    }\n\n    Phi\n    {\n        solver          GAMG;\n        smoother        GaussSeidel;\n        tolerance       1e-6;\n        relTol          0.01;\n    }\n\n    \"(U|k|omega)\"\n    {\n        solver          smoothSolver;\n        smoother        symGaussSeidel;\n        tolerance       1e-10;\n        relTol          0.1;\n    }\n}\n\npotentialFlow\n{\n    nNonOrthogonalCorrectors 3;\n}\n\nSIMPLE\n{\n    consistent      yes;\n    nNonOrthogonalCorrectors 0;\n    residualControl\n    {\n        p               1e-5;\n        U               1e-5;\n        \"(k|omega)\"      1e-5;\n    }\n}\n\nrelaxationFactors\n{\n    equations\n    {\n        U               0.9;\n        \".*\"            0.9;\n    }\n}\n";

fn changed_lines<'a>(a: &'a str, b: &'a str) -> Vec<(&'a str, &'a str)> {
    assert_eq!(a.lines().count(), b.lines().count(), "same line count\n{b}");
    a.lines().zip(b.lines()).filter(|(x, y)| x != y).collect()
}

#[test]
fn arms_parse_by_their_own_ids_only() {
    for arm in Arm::ALL {
        assert_eq!(Arm::parse(arm.id()).unwrap(), arm);
        assert!(Variant::parse(arm.id()).is_err(), "{} is not an older variant", arm.id());
    }
    for old in ["smoke", "c0", "d1", "r0", "r1", "d4", "d5", "m1"] {
        assert!(Arm::parse(old).is_err());
    }
    assert_eq!(Arm::ALL.iter().map(|a| a.id()).collect::<Vec<_>>(), ["s2c", "ptight", "utight", "trelax", "urelax", "ttight"]);
}

#[test]
fn every_arm_restarts_from_the_checkpoint_with_the_same_windows_and_one_change() {
    let control = Arm::Control.plan(2000);
    assert_eq!(control.phases, vec![Phase { start: 2000, end: 2500, schemes: Schemes::Production }]);
    assert_eq!(control.windows, vec![(2001, 2100), (2101, 2500), (2251, 2500)]);
    assert_eq!(control.force_windows, vec![(2101, 2500), (2251, 2500)]);
    assert_eq!(control.map_iterations, vec![2001, 2010, 2050, 2100, 2250, 2500]);
    assert_eq!((control.solution, control.monitors, control.id), (Solution::Production, true, "s2c"));
    assert_eq!(2500 % control.write_interval, 0, "the arm ends on a write time");
    for arm in Arm::ALL {
        let p = arm.plan(2000);
        let same = |q: &Plan| (q.phases.clone(), q.windows.clone(), q.force_windows.clone(), q.map_iterations.clone(), q.write_interval, q.monitors);
        assert_eq!(same(&p), same(&control), "{} differs from the control only in fvSolution", arm.id());
        assert_eq!(p.solution == Solution::Production, arm == Arm::Control);
    }
    assert_eq!(Arm::PTight.plan(3000).phases[0].end, 3500, "the checkpoint sets the window");
    // Older variants keep production fvSolution and no monitors.
    for v in [Variant::C0, Variant::R1, Variant::D5, Variant::M1] {
        let p = v.plan();
        assert_eq!((p.phases, p.windows, p.force_windows, p.map_iterations, p.write_interval), (v.phases(), v.windows(), v.force_windows(), v.map_iterations(), v.write_interval()));
        assert_eq!((p.solution, p.monitors), (Solution::Production, false));
    }
}

#[test]
fn pressure_rel_tol_changes_only_the_p_solver() {
    let out = Solution::PressureRelTol.apply(PROD).unwrap();
    assert_eq!(changed_lines(PROD, &out), vec![(LOOSE_REL_TOL, TIGHT_REL_TOL)]);
    assert!(out.contains("    p\n    {\n        solver          GAMG;\n        smoother        GaussSeidel;\n        tolerance       1e-10;\n        relTol          0.01;\n    }"));
    assert!(out.contains("        relTol          0.1;\n    }\n}\n\npotentialFlow"), "U/k/omega keep 0.1");
    assert!(Solution::PressureRelTol.apply(&out).is_err(), "already tight is refused");
    assert_eq!(Solution::PressureRelTol.tight_fields(), ["p"]);
    let crlf = PROD.replace('\n', "\r\n");
    assert_eq!(Solution::PressureRelTol.apply(&crlf).unwrap(), out.replace('\n', "\r\n"));
}

const U_BLOCK: &str = "    U\n    {\n        solver          smoothSolver;\n        smoother        symGaussSeidel;\n        tolerance       1e-10;\n        relTol          REL;\n    }\n";
const T_BLOCK: &str = "    \"(k|omega)\"\n    {\n        solver          smoothSolver;\n        smoother        symGaussSeidel;\n        tolerance       1e-10;\n        relTol          REL;\n    }\n";

#[test]
fn velocity_and_turbulence_rel_tol_split_the_shared_solver_entry() {
    for (solution, u, t) in [(Solution::VelocityRelTol, "0.01", "0.1"), (Solution::TurbulenceRelTol, "0.1", "0.01")] {
        let out = solution.apply(PROD).unwrap();
        let split = format!("{}\n{}}}\n", U_BLOCK.replace("REL", u), T_BLOCK.replace("REL", t));
        let combined = "    \"(U|k|omega)\"\n    {\n        solver          smoothSolver;\n        smoother        symGaussSeidel;\n        tolerance       1e-10;\n        relTol          0.1;\n    }\n}\n";
        assert!(!out.contains("(U|k|omega)"));
        assert_eq!(out, PROD.replace(combined, &split), "{solution:?}: only the shared entry is split");
        assert!(solution.apply(&out).is_err(), "a split entry is refused");
        let crlf = PROD.replace('\n', "\r\n");
        assert_eq!(solution.apply(&crlf).unwrap(), out.replace('\n', "\r\n"));
    }
    assert_eq!(Solution::VelocityRelTol.tight_fields(), ["Ux", "Uy", "Uz"]);
    assert_eq!(Solution::TurbulenceRelTol.tight_fields(), ["k", "omega"]);
    let two_rel = PROD.replace("        tolerance       1e-10;\n        relTol          0.1;\n    }\n}", "        tolerance       1e-10;\n        relTol          0.1;\n        relTol          0.1;\n    }\n}");
    assert!(Solution::VelocityRelTol.apply(&two_rel).is_err());
    assert!(Solution::VelocityRelTol.apply(&PROD.replace("\"(U|k|omega)\"", "\"(U|k|omega|nuTilda)\"")).is_err());
}

#[test]
fn turbulence_relaxation_adds_explicit_k_and_omega_after_the_default() {
    let out = Solution::TurbulenceRelaxation.apply(PROD).unwrap();
    assert_eq!(out, PROD.replace(&format!("{DEFAULT_RELAX}\n"), &format!("{DEFAULT_RELAX}\n{K_RELAX}\n{OMEGA_RELAX}\n")));
    assert!(Solution::TurbulenceRelaxation.apply(&out).is_err(), "explicit k is refused");
    assert!(Solution::TurbulenceRelaxation.apply(&PROD.replace(DEFAULT_RELAX, "        \"(k|omega)\"     0.9;")).is_err());
    assert!(Solution::TurbulenceRelaxation.apply(&PROD.replace(DEFAULT_RELAX, "")).is_err());
    assert!(Solution::TurbulenceRelaxation.tight_fields().is_empty());
}

#[test]
fn velocity_relaxation_changes_only_the_u_equation_factor() {
    let out = Solution::VelocityRelaxation.apply(PROD).unwrap();
    assert_eq!(changed_lines(PROD, &out), vec![(PRODUCTION_U_RELAX, REDUCED_U_RELAX)]);
    assert!(out.contains("        U               1e-5;"), "residualControl untouched");
    assert!(Solution::VelocityRelaxation.apply(&out).is_err());
    assert_eq!(Solution::Production.apply(PROD).unwrap(), PROD);
}

const CONTROL: &str = "FoamFile\n{\n    object      controlDict;\n}\n\napplication     simpleFoam;\nendTime         7000;\n\nfunctions\n{\n    forces1\n    {\n        type            forces;\n        patches         (\"body.*\");\n    }\n\n    yPlus1\n    {\n        type            yPlus;\n    }\n}\n";

#[test]
fn monitors_are_appended_inside_the_functions_block_only() {
    let out = with_monitors(CONTROL, &OPEN_PATCHES).unwrap();
    // Every original line survives in order; only new lines are added.
    let mut it = out.lines();
    for line in CONTROL.lines() {
        assert!(it.any(|l| l == line), "kept: {line}");
    }
    let tail = &out[out.find("    yPlus1").unwrap()..];
    for p in OPEN_PATCHES {
        assert!(tail.contains(&format!("    wp9Flux_{p}\n    {{\n        type            surfaceFieldValue;")), "{p}");
        assert!(tail.contains(&format!("        name            {p};\n        operation       sum;\n        fields          (phi);")));
    }
    assert!(tail.contains("    wp9Range\n    {\n        type            fieldMinMax;"));
    assert!(out.ends_with("        fields          (U p k omega nut);\n    }\n}\n"), "inserted before the block's closing brace");
    assert!(with_monitors(&out, &OPEN_PATCHES).is_err(), "monitors are added once");
    assert!(with_monitors(&CONTROL.replace("functions\n", "notFunctions\n"), &OPEN_PATCHES).is_err());
    let crlf = CONTROL.replace('\n', "\r\n");
    assert_eq!(with_monitors(&crlf, &OPEN_PATCHES).unwrap(), out.replace('\n', "\r\n"));
}

#[test]
fn dat_rows_and_flux_balance() {
    let text = "# Region type : patch inlet\n# Faces  : 100\n# Time          \tsum(phi)\n2001\t-1.5\n2002\t-1.5\n";
    let (names, rows) = parse_dat(text).unwrap();
    assert_eq!((names, rows[&2001].clone()), (vec!["sum(phi)".to_string()], vec![-1.5]));
    assert!(parse_dat(&format!("{text}2002\t-1.5\n")).is_err(), "a repeated iteration is refused");
    assert!(parse_dat("# Time a b\n1 2\n").is_err(), "short row");
    assert!(parse_dat("1 2\n").is_err(), "no header");
    let col = |v: &[(u32, f64)]| v.iter().copied().collect::<BTreeMap<u32, f64>>();
    let fluxes = vec![("inlet", col(&[(1, -2.0), (2, -2.0)])), ("outlet", col(&[(1, 1.9), (2, 2.0)])), ("top", col(&[(1, 0.08), (2, 0.0)])), ("side", col(&[(1, 0.0), (2, 0.001)]))];
    let b = flux_balance(&fluxes, 1, 2).unwrap();
    assert!((b[&1] + 0.01).abs() < 1e-12 && (b[&2] - 0.0005).abs() < 1e-12, "{b:?}");
    assert!(flux_balance(&fluxes, 1, 3).is_err(), "a missing iteration is refused");
    let reversed = vec![("inlet", col(&[(1, 2.0)])), ("outlet", col(&[(1, -2.0)]))];
    assert!(flux_balance(&reversed, 1, 1).is_err(), "the inlet must be an inflow");
}

const SOLVES: &str = "Time = 11\n\nsmoothSolver:  Solving for Ux, Initial residual = 0.001, Final residual = 8e-05, No Iterations 6\nGAMG:  Solving for p, Initial residual = 0.002, Final residual = 1.9e-05, No Iterations 9\nExecutionTime = 1 s  ClockTime = 1 s\nTime = 12\n\nsmoothSolver:  Solving for Ux, Initial residual = 0.001, Final residual = 9e-05, No Iterations 5\nGAMG:  Solving for p, Initial residual = 0.002, Final residual = 1e-11, No Iterations 30\nExecutionTime = 2 s  ClockTime = 2 s\n";

#[test]
fn rel_tol_is_proven_from_the_solver_output() {
    assert_eq!(parse_solves(SOLVES, "Ux"), vec![(11, 0.001, 8e-05, 6), (12, 0.001, 9e-05, 5)]);
    let loose = check_rel_tol(SOLVES, "Ux", false).unwrap();
    assert_eq!(loose["solves"], 2);
    assert!(check_rel_tol(SOLVES, "Ux", true).unwrap_err().contains("exceeds relTol 0.01"));
    // p ended at 0.0095 of its initial, then below the 1e-10 tolerance.
    let tight = check_rel_tol(SOLVES, "p", true).unwrap();
    assert_eq!(tight["solves"], 2);
    assert!(check_rel_tol(SOLVES, "p", false).unwrap_err().contains("median"), "a loose field must not look tight");
    assert!(check_rel_tol(SOLVES, "k", false).is_err(), "no solves");
    assert!(check_rel_tol(&SOLVES.replace("8e-05", "0.0002"), "Ux", false).unwrap_err().contains("exceeds relTol 0.1"));
}

fn fake_checkpoint(root: &Path, time: u32) {
    for n in 0..16 {
        let rank = root.join(format!("processor{n}"));
        for f in ["points", "faces", "owner", "neighbour", "boundary"] {
            write_text(&rank.join("constant/polyMesh").join(f), &format!("{n} {f}"));
        }
        for f in CHECKPOINT_FIELDS {
            write_text(&rank.join(time.to_string()).join(f), &format!("{n} {f} at {time}"));
        }
        write_text(&rank.join(format!("{time}/uniform/time")), "time");
        write_text(&rank.join("1000/U"), "an older write is not part of the checkpoint");
    }
    for f in ["system/controlDict", "system/fvSchemes", "system/fvSolution", "system/decomposeParDict", "constant/transportProperties", "constant/turbulenceProperties", "logs/solver.log", "constant/triSurface/body.stl"] {
        write_text(&root.join(f), f);
    }
}

fn write_text(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn a_preserved_checkpoint_is_complete_and_tamper_evident() {
    let root = std::env::temp_dir().join(format!("vernon-wp9-checkpoint-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (from, to) = (root.join("from"), root.join("to"));
    fake_checkpoint(&from, 2000);
    let files = checkpoint_files(&from, 2000).unwrap();
    assert_eq!(files.len(), 16 * (5 + 6 + 1) + 8);
    assert!(files.iter().all(|f| !f.contains("/1000/")), "only the chosen time");
    let doc = preserve_checkpoint(&from, &to, 2000, serde_json::json!({ "run": "test" })).unwrap();
    assert_eq!((doc["time"].as_u64(), doc["file_count"].as_u64()), (Some(2000), Some(files.len() as u64)));
    assert_eq!(verify_checkpoint(&to).unwrap(), 2000);
    assert!(preserve_checkpoint(&from, &to, 2000, serde_json::Value::Null).is_err(), "never into a non-empty directory");
    // A changed, missing or extra file is refused.
    write_text(&to.join("processor3/2000/omega"), "changed");
    assert!(verify_checkpoint(&to).unwrap_err().contains("changed"));
    write_text(&to.join("processor3/2000/omega"), "3 omega at 2000");
    assert_eq!(verify_checkpoint(&to).unwrap(), 2000);
    write_text(&to.join("processor3/2000/extra"), "x");
    assert!(verify_checkpoint(&to).is_err());
    std::fs::remove_file(to.join("processor3/2000/extra")).unwrap();
    std::fs::remove_file(to.join("processor7/2000/nut")).unwrap();
    assert!(verify_checkpoint(&to).unwrap_err().contains("nut"));
    assert!(checkpoint_files(&from, 3000).is_err(), "a time that was not written");
    std::fs::remove_dir_all(from.join("processor15")).unwrap();
    assert!(checkpoint_files(&from, 2000).unwrap_err().contains("16"));
    let _ = std::fs::remove_dir_all(&root);
}
