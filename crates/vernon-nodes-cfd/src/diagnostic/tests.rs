use super::*;
use std::path::{Path, PathBuf};

const PROD_SCHEMES: &str = "FoamFile\n{\n    version     2.0;\n    format      ascii;\n    class       dictionary;\n    location    \"system\";\n    object      fvSchemes;\n}\n\nddtSchemes\n{\n    default         steadyState;\n}\n\ngradSchemes\n{\n    default         Gauss linear;\n    grad(U)         cellLimited Gauss linear 1;\n}\n\ndivSchemes\n{\n    default         none;\n    div(phi,U)      bounded Gauss linearUpwind grad(U);\n    div(phi,k)      bounded Gauss limitedLinear 1;\n    div(phi,omega)  bounded Gauss limitedLinear 1;\n    div((nuEff*dev2(T(grad(U))))) Gauss linear;\n}\n\nlaplacianSchemes\n{\n    default         Gauss linear corrected;\n}\n\ninterpolationSchemes\n{\n    default         linear;\n}\n\nsnGradSchemes\n{\n    default         corrected;\n}\n\nwallDist\n{\n    method          meshWave;\n}\n";

const PROD_SOLUTION: &str = "FoamFile\n{\n    version     2.0;\n    format      ascii;\n    class       dictionary;\n    location    \"system\";\n    object      fvSolution;\n}\n\nsolvers\n{\n    p\n    {\n        solver          GAMG;\n        smoother        GaussSeidel;\n        tolerance       1e-10;\n        relTol          0.1;\n    }\n\n    \"(U|k|omega)\"\n    {\n        solver          smoothSolver;\n        smoother        symGaussSeidel;\n        tolerance       1e-10;\n        relTol          0.1;\n    }\n}\n\nSIMPLE\n{\n    consistent      yes;\n    nNonOrthogonalCorrectors 0;\n    residualControl\n    {\n        p               1e-5;\n        U               1e-5;\n        \"(k|omega)\"      1e-5;\n    }\n}\n\nrelaxationFactors\n{\n    equations\n    {\n        U               0.9;\n        \".*\"            0.9;\n    }\n}\n";

// ---------------------------------------------------------------- unit tests

#[test]
fn variants_keep_c0_and_d1_comparable_over_501_to_1000() {
    use Schemes::*;
    let c0 = Variant::C0.phases();
    let d1 = Variant::D1.phases();
    assert_eq!(c0, vec![Phase { start: 0, end: 1000, schemes: Production }]);
    assert_eq!(d1, vec![Phase { start: 0, end: 500, schemes: UpwindTurbulence }, Phase { start: 500, end: 1000, schemes: Production }]);
    assert_eq!(c0.last().unwrap().end, d1.last().unwrap().end);
    let smoke = Variant::Smoke.phases();
    assert!(smoke[0].schemes == UpwindTurbulence && smoke[1].schemes == Production && smoke[0].end == smoke[1].start);
    for v in [Variant::C0, Variant::D1] {
        let maps = v.map_iterations();
        for i in [1, 500, 501, 1000] {
            assert!(maps.contains(&i), "{v:?} keeps the map at {i}");
        }
        for p in v.phases() {
            assert_eq!(p.end % v.write_interval(), 0, "every phase ends on a write time");
        }
    }
    assert!(Variant::parse("d2").is_err());
    assert_eq!(Variant::parse("d1").unwrap(), Variant::D1);
}

#[test]
fn d4_restarts_from_c0_at_500_and_compares_over_501_to_1000() {
    use Schemes::*;
    assert_eq!(Variant::D4.phases(), vec![Phase { start: 500, end: 1000, schemes: LimitedCorrection }]);
    assert_eq!(Variant::R0.phases(), vec![Phase { start: 500, end: 510, schemes: Production }]);
    for v in [Variant::R0, Variant::D4] {
        assert!(v.restarts_from_c0());
        assert_eq!(Variant::parse(v.id()).unwrap(), v);
        for p in v.phases() {
            assert_eq!(p.end % v.write_interval(), 0);
        }
        assert!(v.map_iterations().iter().all(|i| *i > 500), "no map before the restart");
    }
    assert!(!Variant::C0.restarts_from_c0() && !Variant::D1.restarts_from_c0());
    // The D4 window and its maps match C0/D1 over 501–1000, including 751–1000.
    assert_eq!(Variant::D4.windows(), vec![(501, 600), (901, 1000), (501, 1000)]);
    assert!(Variant::C0.windows().contains(&(501, 1000)) && Variant::D1.windows().contains(&(501, 1000)));
    for i in [501, 750, 1000] {
        assert!(Variant::D4.map_iterations().contains(&i));
    }
    assert_eq!(Variant::D4.force_windows(), Variant::C0.force_windows());
    assert_eq!(Variant::R0.windows(), vec![(501, 510)]);
    assert_eq!((Variant::C0.reference(), Variant::R0.reference(), Variant::D4.reference()), (Some(Reference::Stage2), Some(Reference::C0), None));
}

#[test]
fn limited_correction_changes_only_the_laplacian_and_sngrad_defaults() {
    let lim = limited_correction_schemes(PROD_SCHEMES).unwrap();
    let a: Vec<&str> = PROD_SCHEMES.lines().collect();
    let b: Vec<&str> = lim.lines().collect();
    assert_eq!(a.len(), b.len());
    let changed: Vec<(&str, &str)> = a.iter().zip(&b).filter(|(x, y)| x != y).map(|(x, y)| (*x, *y)).collect();
    assert_eq!(changed, vec![(PRODUCTION_LAPLACIAN, LIMITED_LAPLACIAN), (PRODUCTION_SNGRAD, LIMITED_SNGRAD)]);
    assert!(limited_correction_schemes(&lim).is_err(), "already-limited text is refused");
    // The same default line in another block is not touched.
    let moved = PROD_SCHEMES.replace("interpolationSchemes\n{\n    default         linear;", "interpolationSchemes\n{\n    default         corrected;");
    let out = limited_correction_schemes(&moved).unwrap();
    assert!(out.contains("interpolationSchemes\n{\n    default         corrected;"));
    assert!(limited_correction_schemes(&PROD_SCHEMES.replace(PRODUCTION_SNGRAD, "    default         uncorrected;")).is_err());
    assert_eq!(Schemes::Production.apply(PROD_SCHEMES).unwrap(), PROD_SCHEMES);
    assert_eq!(Schemes::LimitedCorrection.apply(PROD_SCHEMES).unwrap(), lim);
    // CRLF text keeps its line endings.
    let crlf = PROD_SCHEMES.replace('\n', "\r\n");
    assert_eq!(limited_correction_schemes(&crlf).unwrap(), lim.replace('\n', "\r\n"));
}

#[test]
fn upwind_schemes_change_only_the_two_turbulence_lines() {
    let up = upwind_turbulence_schemes(PROD_SCHEMES).unwrap();
    let a: Vec<&str> = PROD_SCHEMES.lines().collect();
    let b: Vec<&str> = up.lines().collect();
    assert_eq!(a.len(), b.len());
    let changed: Vec<(&str, &str)> = a.iter().zip(&b).filter(|(x, y)| x != y).map(|(x, y)| (*x, *y)).collect();
    assert_eq!(changed, vec![(PRODUCTION_K, UPWIND_K), (PRODUCTION_OMEGA, UPWIND_OMEGA)]);
    assert!(upwind_turbulence_schemes(&up).is_err(), "already-upwind text is refused");
    assert!(upwind_turbulence_schemes(&PROD_SCHEMES.replace(PRODUCTION_K, "")).is_err());
}

#[test]
fn control_dict_changes_only_the_run_window() {
    let source = "FoamFile\n{\n    object controlDict;\n}\napplication     simpleFoam;\nstartFrom       startTime;\nstartTime       0;\nstopAt          endTime;\nendTime         7000;\ndeltaT          1;\nwriteControl    timeStep;\nwriteInterval   1000;\npurgeWrite      2;\nrunTimeModifiable true;\nfunctions\n{\n    yPlus1\n    {\n        writeInterval   1;\n    }\n}\n";
    let out = diagnostic_control_dict(source, 500, 1000, 250).unwrap();
    for want in ["startTime       500;", "endTime         1000;", "writeInterval   250;", "purgeWrite      0;", "runTimeModifiable false;", "        writeInterval   1;", "startFrom       startTime;"] {
        assert!(out.lines().any(|l| l == want), "missing {want}\n{out}");
    }
    assert_eq!(out.lines().count(), source.lines().count());
    assert!(diagnostic_control_dict(&source.replace("purgeWrite      2;\n", ""), 0, 10, 5).is_err());
}

const LOG: &str = "Time = 1\n\nsmoothSolver:  Solving for Ux, Initial residual = 0.5, Final residual = 0.01, No Iterations 3\nGAMG:  Solving for p, Initial residual = 1, Final residual = 0.05, No Iterations 4\nsmoothSolver:  Solving for omega, Initial residual = 0.003, Final residual = 1e-4, No Iterations 2\nbounding omega, min: -12221.702 max: 122664.23 average: 99.263162\nsmoothSolver:  Solving for k, Initial residual = 1, Final residual = 0.06, No Iterations 3\nbounding k, min: -0.0031536787 max: 0.026659016 average: 0.0025152344\nExecutionTime = 19.16 s  ClockTime = 23 s\n\nTime = 2\n\nsmoothSolver:  Solving for Ux, Initial residual = 0.4, Final residual = 0.01, No Iterations 3\nGAMG:  Solving for p, Initial residual = 0.2, Final residual = 0.01, No Iterations 4\nsmoothSolver:  Solving for omega, Initial residual = 0.002, Final residual = 1e-4, No Iterations 2\nsmoothSolver:  Solving for k, Initial residual = 0.5, Final residual = 0.06, No Iterations 3\nbounding k, min: -0.5 max: 1 average: 0.1\nExecutionTime = 36.2 s  ClockTime = 41 s\n\nEnd\n";

#[test]
fn log_parsers_read_bounding_residuals_and_clock_by_iteration() {
    let b = parse_bounding(LOG).unwrap();
    assert_eq!(b.len(), 3);
    assert_eq!(b[&(1, "omega".to_string())].0, -12221.702);
    assert_eq!(b[&(2, "k".to_string())], (-0.5, 1.0, 0.1));
    assert!(!b.contains_key(&(2, "omega".to_string())));
    let r = parse_initial_residuals(LOG);
    assert_eq!(r[&1].iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["Ux", "p", "omega", "k"]);
    assert_eq!(r[&2][1], ("p".to_string(), 0.2));
    let c = parse_clock(LOG);
    assert_eq!((c[&1], c[&2]), (23.0, 41.0));
    assert!(parse_bounding("bounding k, min: -1 max: 1 average: 0\n").is_err(), "a bounding line needs an iteration");
    assert!(parse_bounding(&LOG.replace("bounding k, min: -0.5", "bounding k, min: -0.5\nbounding k, min: -0.5")).is_err());
}

#[test]
fn history_comparison_reports_the_largest_relative_difference() {
    let a = parse_initial_residuals(LOG);
    let same = compare_histories(&a, &a, 1, 2);
    assert_eq!((same.compared_iterations, same.compared_values, same.max_rel_diff, same.first_iteration_over_1e_6, same.missing_iterations), (2, 8, 0.0, None, 0));
    let b = parse_initial_residuals(&LOG.replace("residual = 0.2,", "residual = 0.2002,"));
    let diff = compare_histories(&a, &b, 1, 3);
    assert!((diff.max_rel_diff - 0.0002 / 0.2002).abs() < 1e-12, "{diff:?}");
    assert_eq!((diff.first_iteration_over_1e_6, diff.missing_iterations), (Some(2), 1));
}

fn counter_text(rows: &[&str]) -> String {
    let mut t = String::from("# iter lowerBound min nBelow nNegative nZero volumeBelow volumeNegative region_boxA region_boxC region_other band_lt_0.002 band_lt_0.01 band_lt_0.05 band_ge_0.05 hookLocalMaxSeconds hookSeconds\n");
    for r in rows {
        t += r;
        t += "\n";
    }
    t
}

#[test]
fn counter_rows_must_agree_with_openfoam_bounding() {
    let text = counter_text(&["1 1e-15 -0.0031536787 120 104 0 0.015 0.013 96 8 16 0 0 8 112 0.001 0.001", "2 1e-15 -0.5 3 3 0 1 1 0 0 3 0 0 0 3 0.001 0.001"]);
    let (regions, rows) = parse_counter(&text).unwrap();
    assert_eq!(regions, ["boxA", "boxC", "other"]);
    assert_eq!(rows[0].bands, [0, 0, 8, 112]);
    let b = parse_bounding(LOG).unwrap();
    check_counter("k", &rows, &b, 1, 2).unwrap();
    // omega bounded at 1 only: rows that count nothing at 2 must match that.
    let omega = counter_text(&["1 1e-15 -12221.702 5 5 0 1 1 0 0 5 0 0 0 5 0 0", "2 1e-15 3 0 0 0 0 0 0 0 0 0 0 0 0 0 0"]);
    check_counter("omega", &parse_counter(&omega).unwrap().1, &b, 1, 2).unwrap();
    let wrong_min = counter_text(&["1 1e-15 -0.004 120 104 0 0.015 0.013 96 8 16 0 0 8 112 0 0", "2 1e-15 -0.5 3 3 0 1 1 0 0 3 0 0 0 3 0 0"]);
    assert!(check_counter("k", &parse_counter(&wrong_min).unwrap().1, &b, 1, 2).unwrap_err().contains("log min"));
    let missed = counter_text(&["1 1e-15 -0.0031536787 120 104 0 0.015 0.013 96 8 16 0 0 8 112 0 0", "2 1e-15 3 0 0 0 0 0 0 0 0 0 0 0 0 0 0"]);
    assert!(check_counter("k", &parse_counter(&missed).unwrap().1, &b, 1, 2).unwrap_err().contains("counter saw nothing"));
    let extra = counter_text(&["1 1e-15 -12221.702 5 5 0 1 1 0 0 5 0 0 0 5 0 0", "2 1e-15 -1 1 1 0 0 0 0 0 1 0 0 0 1 0 0"]);
    assert!(check_counter("omega", &parse_counter(&extra).unwrap().1, &b, 1, 2).unwrap_err().contains("did not bound"));
    let gap = counter_text(&["2 1e-15 -0.5 3 3 0 1 1 0 0 3 0 0 0 3 0 0"]);
    assert!(check_counter("k", &parse_counter(&gap).unwrap().1, &b, 1, 2).is_err());
    let unbalanced = counter_text(&["1 1e-15 -0.0031536787 120 104 0 0.015 0.013 96 8 15 0 0 8 112 0 0", "2 1e-15 -0.5 3 3 0 1 1 0 0 3 0 0 0 3 0 0"]);
    assert!(check_counter("k", &parse_counter(&unbalanced).unwrap().1, &b, 1, 2).unwrap_err().contains("add up"));
    let zero_floor = counter_text(&["1 0 -0.0031536787 120 104 0 0.015 0.013 96 8 16 0 0 8 112 0 0", "2 1e-15 -0.5 3 3 0 1 1 0 0 3 0 0 0 3 0 0"]);
    assert!(check_counter("k", &parse_counter(&zero_floor).unwrap().1, &b, 1, 2).is_err());
    assert!(parse_counter(&(text.clone() + "# iter other header\n")).is_err(), "a different header mid-file is refused");
}

#[test]
fn stl_bounds_and_regions() {
    let stl = "solid w\n facet normal 0 0 1\n  outer loop\n   vertex 1 2 3\n   vertex 4 -5 6\n   vertex 0.5 0 -1\n  endloop\n endfacet\nendsolid w\n";
    assert_eq!(stl_bounds(stl), Some(([0.5, -5.0, -1.0], [4.0, 2.0, 6.0])));
    assert_eq!(stl_bounds("solid x\nendsolid x\n"), None);
    let body = ([0.0, 0.0, 0.15], [4.6, 0.9, 1.4]);
    let front = ([0.6, 0.6, 0.0], [1.3, 0.85, 0.65]);
    let rear = ([3.4, 0.6, 0.0], [4.1, 0.85, 0.65]);
    let r = case_regions(body, front, rear);
    let names: Vec<&str> = r.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(names, ["wheel_fr", "wheel_rr", "ground_contact", "underbody_band", "body_near", "near_wake"]);
    assert!((r[0].1[0] - 0.55).abs() < 1e-12 && (r[2].2[2] - 0.05).abs() < 1e-12 && (r[3].2[2] - 0.25).abs() < 1e-12);
    assert!((r[5].1[0] - 4.7).abs() < 1e-12 && (r[5].2[0] - 9.2).abs() < 1e-12);
    let text = fv_options(&r, &[1, 10], None);
    assert!(text.contains("regionOrder     (wheel_fr wheel_rr ground_contact underbody_band body_near near_wake);"));
    assert!(text.contains("mapIterations   (1 10);") && !text.contains("wp9Inject"));
    assert!(text.contains("codeAddSup") && text.contains("codeConstrain"), "empty hooks must be explicit or OpenFOAM substitutes NotImplemented");
    // The counter's code must not depend on the case: identical text compiles once.
    let other = fv_options(&case_regions(front, body, rear), &[1, 10], None);
    let code = |t: &str| t[t.find("codeOptions").unwrap()..].to_string();
    assert_eq!(code(&text), code(&other));
}

// ------------------------------------------------------- fixture (real tools)

const DISTRO: &str = "Vernon-Ubuntu-24.04";
const USER: &str = "vernon";
const A: ([f64; 3], [f64; 3]) = ([0.6, 0.1, 0.1], [0.9, 0.3, 0.3]);
const B: ([f64; 3], [f64; 3]) = ([1.2, 0.2, 0.2], [1.4, 0.3, 0.3]);
const C: ([f64; 3], [f64; 3]) = ([1.6, 0.0, 0.2], [1.8, 0.05, 0.3]);
const CELL_VOLUME: f64 = 0.05 * 0.05 * 0.05;

fn injection() -> Injection {
    Injection { a: A, b: B, c: C, a_k: -0.25, c_k: -0.5, a_omega: -3.0, c_omega: -6.0, at: 1 }
}

fn fixture_regions() -> Vec<Region> {
    vec![("boxA".into(), A.0, A.1), ("boxC".into(), C.0, C.1)]
}

fn foam_file(class: &str, location: &str, object: &str, body: &str) -> String {
    format!("FoamFile\n{{\n    version     2.0;\n    format      ascii;\n    class       {class};\n    location    \"{location}\";\n    object      {object};\n}}\n\n{body}")
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn fixture_control(start: u32, end: u32, every: u32) -> String {
    foam_file("dictionary", "system", "controlDict", &format!("application     simpleFoam;\nstartFrom       startTime;\nstartTime       {start};\nstopAt          endTime;\nendTime         {end};\ndeltaT          1;\nwriteControl    timeStep;\nwriteInterval   {every};\npurgeWrite      0;\nwriteFormat     ascii;\nwritePrecision  17;\nwriteCompression off;\ntimeFormat      general;\ntimePrecision   6;\nrunTimeModifiable false;\n"))
}

/// A 2 x 0.5 x 0.5 m channel of 40 x 10 x 10 cubes (0.05 m) with walls on
/// four sides, k-omega SST and the production schemes/solution settings.
fn fixture_case(dir: &Path, options: Option<&str>, upwind: bool, end: u32, every: u32) {
    let _ = std::fs::remove_dir_all(dir);
    write(&dir.join("system/blockMeshDict"), &foam_file("dictionary", "system", "blockMeshDict", "convertToMeters 1;\nvertices\n(\n    (0 0 0) (2 0 0) (2 0.5 0) (0 0.5 0)\n    (0 0 0.5) (2 0 0.5) (2 0.5 0.5) (0 0.5 0.5)\n);\nblocks\n(\n    hex (0 1 2 3 4 5 6 7) (40 10 10) simpleGrading (1 1 1)\n);\nedges\n(\n);\nboundary\n(\n    inlet { type patch; faces ((0 4 7 3)); }\n    outlet { type patch; faces ((1 2 6 5)); }\n    walls { type wall; faces ((0 1 5 4) (3 7 6 2) (0 3 2 1) (4 5 6 7)); }\n);\nmergePatchPairs\n(\n);\n"));
    write(&dir.join("system/controlDict"), &fixture_control(0, end, every));
    let schemes = if upwind { upwind_turbulence_schemes(PROD_SCHEMES).unwrap() } else { PROD_SCHEMES.to_string() };
    write(&dir.join("system/fvSchemes"), &schemes);
    write(&dir.join("system/fvSolution"), PROD_SOLUTION);
    write(&dir.join("system/decomposeParDict"), &foam_file("dictionary", "system", "decomposeParDict", "numberOfSubdomains 4;\nmethod          simple;\nsimpleCoeffs\n{\n    n           (4 1 1);\n    delta       0.001;\n}\n"));
    if let Some(o) = options {
        write(&dir.join("system/fvOptions"), o);
    }
    write(&dir.join("constant/transportProperties"), &foam_file("dictionary", "constant", "transportProperties", "transportModel  Newtonian;\nnu              1e-05;\n"));
    write(&dir.join("constant/turbulenceProperties"), &foam_file("dictionary", "constant", "turbulenceProperties", "simulationType  RAS;\nRAS\n{\n    model           kOmegaSST;\n    turbulence      on;\n    printCoeffs     off;\n}\n"));
    let field = |class: &str, name: &str, dims: &str, internal: &str, inlet: &str, outlet: &str, walls: &str| {
        write(&dir.join("0").join(name), &foam_file(class, "0", name, &format!("dimensions      {dims};\ninternalField   {internal};\nboundaryField\n{{\n    inlet {{ {inlet} }}\n    outlet {{ {outlet} }}\n    walls {{ {walls} }}\n}}\n")));
    };
    field("volVectorField", "U", "[0 1 -1 0 0 0 0]", "uniform (1 0 0)", "type fixedValue; value uniform (1 0 0);", "type zeroGradient;", "type noSlip;");
    field("volScalarField", "p", "[0 2 -2 0 0 0 0]", "uniform 0", "type zeroGradient;", "type fixedValue; value uniform 0;", "type zeroGradient;");
    field("volScalarField", "k", "[0 2 -2 0 0 0 0]", "uniform 0.01", "type fixedValue; value uniform 0.01;", "type zeroGradient;", "type kqRWallFunction; value uniform 0.01;");
    field("volScalarField", "omega", "[0 0 -1 0 0 0 0]", "uniform 10", "type fixedValue; value uniform 10;", "type zeroGradient;", "type omegaWallFunction; value uniform 10;");
    field("volScalarField", "nut", "[0 2 -1 0 0 0 0]", "uniform 0", "type calculated; value uniform 0;", "type calculated; value uniform 0;", "type nutkWallFunction; value uniform 0;");
}

fn var(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("set {k}"))
}

fn run(case: &Path, env_file: &str, steps: &[(&str, &[&str])], timeout_min: u64) -> Result<(), String> {
    let wsl = crate::extproc::WslOpts { distribution: DISTRO.into(), user: USER.into(), environment_file: env_file.into() };
    let opts = crate::extproc::RunOpts { runner: "wsl".into(), image: String::new(), bin_dir: String::new(), timeout_min };
    let control = case.parent().unwrap().join(format!(".wsl-control-{}", case.file_name().unwrap().to_string_lossy()));
    std::fs::create_dir_all(&control).map_err(|e| e.to_string())?;
    let steps: Vec<(String, Vec<String>)> = steps.iter().map(|(n, a)| (n.to_string(), a.iter().map(|s| s.to_string()).collect())).collect();
    let res = crate::extproc::run_chain_with_wsl(&opts, &wsl, case, &control, &steps);
    match res.failed {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

fn log(case: &Path, step: &str) -> String {
    std::fs::read_to_string(case.join("logs").join(format!("{step}.log"))).unwrap_or_default()
}

fn fixture_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("vernon-wp9-fixture-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn counter_rows(case: &Path, field: &str) -> (Vec<String>, Vec<CounterRow>) {
    let text = std::fs::read_to_string(case.join(format!("postProcessing/wp9ClipCounter/clip_{field}.dat"))).unwrap_or_else(|e| panic!("counter output for {field}: {e}"));
    parse_counter(&text).unwrap()
}

fn assert_injected_counts(case: &Path, log_text: &str, iterations: u32) {
    let bounding = parse_bounding(log_text).unwrap();
    for (field, a_value, c_value) in [("k", -0.25, -0.5), ("omega", -3.0, -6.0)] {
        let (regions, rows) = counter_rows(case, field);
        assert_eq!(regions, ["boxA", "boxC", "other"]);
        // Every iteration: the counter agrees with OpenFOAM's own bounding line
        // (iterations 2.. carry whatever clipping the injection provoked).
        check_counter(field, &rows, &bounding, 1, iterations).unwrap();
        // The injection iteration: exact known counts.
        for r in rows.iter().filter(|r| r.iteration == 1) {
            assert_eq!(r.lower_bound, 1e-15, "{field}: the floor is the model's (SMALL)");
            assert_eq!((r.negative, r.zero, r.below), (104, 0, 120), "{field} {}: 96 + 8 negative, 16 positive but below the floor", r.iteration);
            assert_eq!(r.regions, [96, 8, 16], "{field} {}", r.iteration);
            assert_eq!(r.bands, [0, 0, 8, 112], "{field} {}: box C is 0.025 m from a wall, the rest >= 0.125 m", r.iteration);
            assert!((r.volume_below - 120.0 * CELL_VOLUME).abs() < 1e-12 && (r.volume_negative - 104.0 * CELL_VOLUME).abs() < 1e-12, "{field} volumes {r:?}");
            assert_eq!(r.min, c_value, "{field}: pre-clip minimum is the injected box-C value");
            let logged = bounding[&(r.iteration, field.to_string())].0;
            assert_eq!(logged, c_value, "{field}: OpenFOAM's bounding line prints the same pre-clip minimum");
        }
        // Worst cells at map iterations: the 8 box-C cells first, then box A.
        let worst = std::fs::read_to_string(case.join(format!("postProcessing/wp9ClipCounter/clip_{field}_worst.dat"))).unwrap();
        let first_map: Vec<Vec<&str>> = worst.lines().filter(|l| l.starts_with("1 ")).map(|l| l.split_whitespace().collect()).collect();
        assert_eq!(first_map.len(), 20, "{field}: twenty worst cells at iteration 1");
        for (j, w) in first_map.iter().enumerate() {
            let (value, x, y, z): (f64, f64, f64, f64) = (w[1].parse().unwrap(), w[2].parse().unwrap(), w[3].parse().unwrap(), w[4].parse().unwrap());
            let (want_value, region, bx) = if j < 8 { (c_value, "boxC", C) } else { (a_value, "boxA", A) };
            assert_eq!((value, w[6]), (want_value, region), "{field} worst #{j}");
            assert!(x > bx.0[0] && x < bx.1[0] && y > bx.0[1] && y < bx.1[1] && z > bx.0[2] && z < bx.1[2], "{field} worst #{j} at ({x}, {y}, {z})");
        }
    }
}

/// The counter on known values: global counts, volumes, regions, wall bands,
/// floor and locations, in serial and on 4 MPI ranks.
#[test]
#[ignore = "needs WSL and OpenFOAM"]
fn fixture_counter_measures_known_values_before_bounding() {
    let env = var("VERNON_DIAG_FIXTURE_ENV");
    let root = fixture_root("inject");
    let options = fv_options(&fixture_regions(), &[1, 3], Some(&injection()));
    let serial = root.join("serial");
    fixture_case(&serial, Some(&options), false, 3, 3);
    run(&serial, &env, &[("blockMesh", &["blockMesh"]), ("solver", &["simpleFoam"])], 30).unwrap();
    let serial_log = log(&serial, "solver");
    assert!(serial_log.contains("\nEnd"), "{serial_log}");
    assert_injected_counts(&serial, &serial_log, 3);
    let parallel = root.join("parallel");
    fixture_case(&parallel, Some(&options), false, 3, 3);
    copy_tree(&serial.join("dynamicCode"), &parallel.join("dynamicCode")).unwrap();
    run(&parallel, &env, &[("blockMesh", &["blockMesh"]), ("decomposePar", &["decomposePar", "-force"]), ("solver", &["mpirun", "-np", "4", "simpleFoam", "-parallel"])], 30).unwrap();
    let parallel_log = log(&parallel, "solver");
    assert!(parallel_log.contains("\nEnd") && parallel_log.contains("nProcs : 4"), "{parallel_log}");
    assert_injected_counts(&parallel, &parallel_log, 3);
    // The same global counts whatever the decomposition (at the injection
    // iteration; later iterations solve on different partitions).
    for field in ["k", "omega"] {
        let strip = |rows: Vec<CounterRow>| rows.into_iter().filter(|r| r.iteration == 1).map(|r| (r.iteration, r.below, r.negative, r.regions, r.bands, r.min)).collect::<Vec<_>>();
        assert_eq!(strip(counter_rows(&serial, field).1), strip(counter_rows(&parallel, field).1));
    }
    // Maps were written on every rank at both map iterations.
    for p in 0..4 {
        for t in [1, 3] {
            for f in ["clipCount_k", "clipMin_k", "clipCount_omega", "clipMin_omega"] {
                assert!(parallel.join(format!("processor{p}/{t}/{f}")).is_file(), "processor{p}/{t}/{f}");
            }
        }
    }
    println!("WP9_FIXTURE_COUNTER_PROVEN ranks=1,4 iterations=3 negative=104 below_floor=120");
}

fn same_files(a: &Path, b: &Path, names: &[&str]) {
    for n in names {
        let x = std::fs::read(a.join(n)).unwrap_or_else(|e| panic!("{}: {e}", a.join(n).display()));
        let y = std::fs::read(b.join(n)).unwrap_or_else(|e| panic!("{}: {e}", b.join(n).display()));
        assert!(x == y, "{n} differs between {} and {}", a.display(), b.display());
    }
}

const FIELDS: [&str; 6] = ["U", "p", "k", "omega", "nut", "phi"];

/// The counter does not change the solution: with and without it, five
/// iterations write identical fields and identical residual histories.
#[test]
#[ignore = "needs WSL and OpenFOAM"]
fn fixture_counter_leaves_the_solution_unchanged() {
    let env = var("VERNON_DIAG_FIXTURE_ENV");
    let root = fixture_root("unchanged");
    let plain = root.join("plain");
    let counted = root.join("counted");
    fixture_case(&plain, None, false, 5, 5);
    fixture_case(&counted, Some(&fv_options(&fixture_regions(), &[5], None)), false, 5, 5);
    for case in [&plain, &counted] {
        run(case, &env, &[("blockMesh", &["blockMesh"]), ("solver", &["simpleFoam"])], 30).unwrap();
        assert!(log(case, "solver").contains("\nEnd"));
    }
    same_files(&plain.join("5"), &counted.join("5"), &FIELDS);
    let (a, b) = (parse_initial_residuals(&log(&plain, "solver")), parse_initial_residuals(&log(&counted, "solver")));
    let c = compare_histories(&a, &b, 1, 5);
    assert_eq!((c.compared_iterations, c.max_rel_diff), (5, 0.0), "{c:?}");
    assert_eq!(counter_rows(&counted, "k").1.len(), 5);
    println!("WP9_FIXTURE_UNCHANGED_PROVEN iterations=5 fields=identical");
}

/// The scheme switch is two solver processes. Proven here: (1) a restart from
/// a written time reproduces the continuous run exactly; (2) the switched run
/// equals the pure-upwind run up to the switch and differs from it after.
#[test]
#[ignore = "needs WSL and OpenFOAM"]
fn fixture_restart_switch_uses_upwind_then_production() {
    let env = var("VERNON_DIAG_FIXTURE_ENV");
    let root = fixture_root("switch");
    let options = fv_options(&fixture_regions(), &[2, 3], None);
    let solve: &[(&str, &[&str])] = &[("solver", &["simpleFoam"])];
    let mesh_and_solve: &[(&str, &[&str])] = &[("blockMesh", &["blockMesh"]), ("solver", &["simpleFoam"])];
    // Continuous production 0..4, and production 0..2 then restart 2..4.
    let continuous = root.join("continuous");
    fixture_case(&continuous, Some(&options), false, 4, 2);
    run(&continuous, &env, mesh_and_solve, 30).unwrap();
    let restarted = root.join("restarted");
    fixture_case(&restarted, Some(&options), false, 2, 2);
    copy_tree(&continuous.join("dynamicCode"), &restarted.join("dynamicCode")).unwrap();
    run(&restarted, &env, mesh_and_solve, 30).unwrap();
    write(&restarted.join("system/controlDict"), &fixture_control(2, 4, 2));
    std::fs::rename(restarted.join("logs/solver.log"), restarted.join("logs/solver_phase1.log")).unwrap();
    run(&restarted, &env, solve, 30).unwrap();
    same_files(&continuous.join("4"), &restarted.join("4"), &FIELDS);
    let cont = parse_initial_residuals(&log(&continuous, "solver"));
    let rest = parse_initial_residuals(&log(&restarted, "solver"));
    assert_eq!(rest.keys().copied().collect::<Vec<_>>(), [3, 4], "the restart runs iterations 3 and 4 only");
    let c = compare_histories(&cont, &rest, 3, 4);
    assert_eq!((c.compared_iterations, c.max_rel_diff), (2, 0.0), "restart reproduces the continuous run: {c:?}");
    // Pure upwind 0..4 against upwind 0..2 then production 2..4.
    let upwind = root.join("upwind");
    fixture_case(&upwind, Some(&options), true, 4, 2);
    copy_tree(&continuous.join("dynamicCode"), &upwind.join("dynamicCode")).unwrap();
    run(&upwind, &env, mesh_and_solve, 30).unwrap();
    let switched = root.join("switched");
    fixture_case(&switched, Some(&options), true, 2, 2);
    copy_tree(&continuous.join("dynamicCode"), &switched.join("dynamicCode")).unwrap();
    run(&switched, &env, mesh_and_solve, 30).unwrap();
    let phase1 = log(&switched, "solver");
    std::fs::rename(switched.join("logs/solver.log"), switched.join("logs/solver_phase1.log")).unwrap();
    write(&switched.join("system/controlDict"), &fixture_control(2, 4, 2));
    write(&switched.join("system/fvSchemes"), PROD_SCHEMES);
    run(&switched, &env, solve, 30).unwrap();
    let phase2 = log(&switched, "solver");
    let up = parse_initial_residuals(&log(&upwind, "solver"));
    let first = compare_histories(&up, &parse_initial_residuals(&phase1), 1, 2);
    assert_eq!((first.compared_iterations, first.max_rel_diff), (2, 0.0), "iterations 1-2 use upwind: {first:?}");
    let after = compare_histories(&up, &parse_initial_residuals(&phase2), 3, 4);
    assert!(after.compared_iterations == 2 && after.max_rel_diff > 1e-6, "iterations 3-4 no longer use upwind: {after:?}");
    // The same counter file runs on across the restart: 1..4 once each.
    let (_, rows) = counter_rows(&switched, "k");
    assert_eq!(rows.iter().map(|r| r.iteration).collect::<Vec<_>>(), [1, 2, 3, 4]);
    println!("WP9_FIXTURE_SWITCH_PROVEN restart=exact upwind_through=2 production_from=3 after_diff={:e}", after.max_rel_diff);
}

// ------------------------------------------------------------ the real case

fn bytes_of(dir: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            total += if p.is_dir() { bytes_of(&p) } else { e.metadata().map(|m| m.len()).unwrap_or(0) };
        }
    }
    total
}

fn window_mean(rows: &[CounterRow], first: u32, last: u32, f: impl Fn(&CounterRow) -> f64) -> Option<f64> {
    let v: Vec<f64> = rows.iter().filter(|r| r.iteration >= first && r.iteration <= last).map(f).collect();
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

fn rms_initial(res: &BTreeMap<u32, Vec<(String, f64)>>, field: &str, first: u32, last: u32) -> Option<f64> {
    let v: Vec<f64> = res
        .range(first..=last)
        .filter_map(|(_, list)| {
            if field == "U" {
                list.iter().filter(|x| x.0.starts_with('U')).map(|x| x.1).reduce(f64::max)
            } else {
                list.iter().find(|x| x.0 == field).map(|x| x.1)
            }
        })
        .collect();
    (!v.is_empty()).then(|| (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt())
}

fn force_window(case: &Path, first: u32, last: u32) -> serde_json::Value {
    // Each phase writes under its own start-time directory.
    let mut all = String::new();
    if let Ok(dirs) = std::fs::read_dir(case.join("postProcessing/forceCoeffs1")) {
        let mut d: Vec<PathBuf> = dirs.flatten().map(|e| e.path()).collect();
        d.sort_by_key(|p| p.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u32>().ok()).unwrap_or(0));
        for p in d {
            all += &std::fs::read_to_string(p.join("coefficient.dat")).unwrap_or_default();
        }
    }
    let (mut cd, mut cl) = (Vec::new(), Vec::new());
    for line in all.lines().filter(|l| !l.starts_with('#')) {
        let v: Vec<f64> = line.split_whitespace().filter_map(|s| s.parse().ok()).collect();
        if v.len() >= 5 && v[0] >= f64::from(first) && v[0] <= f64::from(last) {
            cd.push(v[1]);
            cl.push(v[4]);
        }
    }
    let stats = |x: &[f64]| {
        let n = x.len() as f64;
        let m = x.iter().sum::<f64>() / n;
        (m, (x.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / n).sqrt())
    };
    if cd.is_empty() {
        return serde_json::json!({ "first": first, "last": last, "samples": 0 });
    }
    let (cd_mean, cd_sd) = stats(&cd);
    let (cl_mean, cl_sd) = stats(&cl);
    serde_json::json!({ "first": first, "last": last, "samples": cd.len(), "cd_mean": cd_mean, "cd_sd": cd_sd, "cl_mean": cl_mean, "cl_sd": cl_sd })
}

/// One diagnostic variant on the accepted 12M Fastback mesh, copied from the
/// stopped Stage 2 case. Screening evidence only: never acceptance.
#[test]
#[ignore = "needs WSL, OpenFOAM and the stopped Stage 2 case"]
fn wp9_turbulence_diagnostic() {
    let variant = Variant::parse(&var("VERNON_DIAG_VARIANT")).unwrap();
    let source = PathBuf::from(var("VERNON_DIAG_SOURCE"));
    let out_path = PathBuf::from(var("VERNON_DIAG_OUT"));
    let solve_env = var("VERNON_WSL_ENV_FILE");
    let fixture_env = var("VERNON_DIAG_FIXTURE_ENV");
    assert_eq!((var("VERNON_WSL_DISTRO").as_str(), var("VERNON_WSL_USER").as_str()), (DISTRO, USER));
    let phases = variant.phases();
    // R0 and D4 start from C0's written iteration 500 on the same mesh.
    let start_source = if variant.restarts_from_c0() { PathBuf::from(var("VERNON_DIAG_START_SOURCE")) } else { source.clone() };
    let first = phases[0].start + 1;
    let work = std::env::temp_dir().join(format!("vernon-wp9-diag-{}-{}", variant.id(), std::process::id()));
    let case = work.join("case");
    std::fs::create_dir_all(&case).unwrap();

    // 1. Case assembly: the production system/ and constant/ settings, the
    //    16-way decomposed mesh and the start fields, copied and fingerprinted.
    let started = std::time::Instant::now();
    let start_time = phases[0].start.to_string();
    std::fs::create_dir_all(case.join("system")).unwrap();
    std::fs::create_dir_all(case.join("constant")).unwrap();
    for name in ["controlDict", "fvSchemes", "fvSolution", "decomposeParDict"] {
        std::fs::copy(source.join("system").join(name), case.join("system").join(name)).unwrap_or_else(|e| panic!("copy system/{name}: {e}"));
    }
    for name in ["transportProperties", "turbulenceProperties"] {
        std::fs::copy(source.join("constant").join(name), case.join("constant").join(name)).unwrap();
    }
    let processors = processor_dirs(&source).unwrap();
    assert_eq!(processors.len(), 16, "the accepted case is decomposed 16 ways");
    let mut mesh_hash: u64 = 0;
    let (mut files, mut bytes) = (0u64, 0u64);
    for p in &processors {
        let target = case.join(p.file_name().unwrap());
        let (f, b) = copy_tree(&p.join("constant/polyMesh"), &target.join("constant/polyMesh")).unwrap();
        let state = start_source.join(p.file_name().unwrap());
        let (f2, b2) = copy_tree(&state.join(&start_time), &target.join(&start_time)).unwrap();
        assert!(target.join(&start_time).join("U").is_file() && target.join(&start_time).join("phi").is_file(), "start fields at {start_time}");
        files += f + f2;
        bytes += b + b2;
        for name in ["points", "faces", "owner", "neighbour", "boundary"] {
            let h = fnv1a_file(&target.join("constant/polyMesh").join(name)).unwrap();
            assert_eq!(h, fnv1a_file(&p.join("constant/polyMesh").join(name)).unwrap(), "copied {name} matches the source");
            assert_eq!(h, fnv1a_file(&state.join("constant/polyMesh").join(name)).unwrap(), "the start state was computed on the same {name}");
            mesh_hash = mesh_hash.rotate_left(7) ^ h;
        }
    }
    let copy_seconds = started.elapsed().as_secs_f64();
    let production_schemes = std::fs::read_to_string(source.join("system/fvSchemes")).unwrap();
    let source_control = std::fs::read_to_string(source.join("system/controlDict")).unwrap();

    // 2. Classification boxes from the case's own surfaces; counter options.
    let bounds = |name: &str| stl_bounds(&std::fs::read_to_string(source.join(format!("constant/triSurface/{name}.stl"))).unwrap()).unwrap_or_else(|| panic!("{name}.stl bounds"));
    let regions = case_regions(bounds("body"), bounds("wheel_fr"), bounds("wheel_rr"));
    let options = fv_options(&regions, &variant.map_iterations(), None);
    std::fs::write(case.join("system/fvOptions"), &options).unwrap();

    // 3. Compile the counter before the timed solve, on the fixture with the
    //    identical fvOptions text, then hand the library to the real case.
    let compile_case = work.join("compile");
    fixture_case(&compile_case, Some(&options), false, 1, 1);
    let compile_started = std::time::Instant::now();
    run(&compile_case, &fixture_env, &[("blockMesh", &["blockMesh"]), ("solver", &["simpleFoam"])], 30).unwrap();
    let compile_seconds = compile_started.elapsed().as_secs_f64();
    let compile_log = log(&compile_case, "solver");
    assert!(compile_log.contains("\nEnd"), "{compile_log}");
    let links_removed = remove_links(&compile_case).unwrap();
    copy_tree(&compile_case.join("dynamicCode"), &case.join("dynamicCode")).unwrap();

    // 4. Phases: one solver process each, schemes fixed for its lifetime.
    let mut phase_rows = Vec::new();
    let mut full_log = String::new();
    for (n, phase) in phases.iter().enumerate() {
        let schemes = &phase.schemes.apply(&production_schemes).unwrap();
        std::fs::write(case.join("system/fvSchemes"), schemes).unwrap();
        std::fs::write(case.join("system/controlDict"), diagnostic_control_dict(&source_control, phase.start, phase.end, variant.write_interval()).unwrap()).unwrap();
        let step = format!("solver_phase{}", n + 1);
        let t = std::time::Instant::now();
        let result = run(&case, &solve_env, &[(step.as_str(), &["mpirun", "-np", "16", "simpleFoam", "-parallel"])], Variant::phase_timeout_min(phase));
        let wall = t.elapsed().as_secs_f64();
        let text = log(&case, &step);
        result.unwrap_or_else(|e| panic!("{step}: {e}\n{}", text.lines().rev().take(30).collect::<Vec<_>>().join("\n")));
        let iterations: Vec<u32> = text.lines().filter_map(iteration_of).collect();
        let expected: Vec<u32> = (phase.start + 1..=phase.end).collect();
        assert_eq!(iterations, expected, "{step} runs exactly its window");
        assert!(text.contains("\nEnd"), "{step} reached End");
        for marker in ["Invoking wmake", "Creating new library"] {
            assert!(!text.contains(marker), "{step} must load the precompiled counter, not compile ('{marker}')");
        }
        assert!(text.contains("wp9ClipCounter"), "{step} names the counter");
        for p in 0..16 {
            assert!(case.join(format!("processor{p}/{}/U", phase.end)).is_file(), "{step} wrote its end time on processor{p}");
        }
        let clock = parse_clock(&text);
        let spi = match (clock.get(&(phase.start + 1)), clock.get(&phase.end)) {
            (Some(a), Some(b)) if phase.end > phase.start + 1 => (b - a) / f64::from(phase.end - phase.start - 1),
            _ => f64::NAN,
        };
        phase_rows.push(serde_json::json!({
            "phase": n + 1, "start": phase.start, "end": phase.end,
            "schemes": phase.schemes.label(),
            "turbulence_convection": if phase.schemes == Schemes::UpwindTurbulence { "bounded Gauss upwind" } else { "bounded Gauss limitedLinear 1" },
            "laplacian_default": if phase.schemes == Schemes::LimitedCorrection { "Gauss linear limited corrected 0.33" } else { "Gauss linear corrected" },
            "sngrad_default": if phase.schemes == Schemes::LimitedCorrection { "limited corrected 0.33" } else { "corrected" },
            "fvSchemes_fnv1a64": format!("{:016x}", { let mut h: u64 = 0xcbf2_9ce4_8422_2325; for b in schemes.bytes() { h ^= u64::from(b); h = h.wrapping_mul(0x0100_0000_01b3); } h }),
            "wall_seconds": wall, "seconds_per_iteration": spi, "log": case.join("logs").join(format!("{step}.log")),
        }));
        full_log += &text;
    }
    let last = phases.last().unwrap().end;

    // 5. Instrumentation checks against OpenFOAM's own bounding output.
    let bounding = parse_bounding(&full_log).unwrap();
    let mut counters = serde_json::Map::new();
    for field in ["k", "omega"] {
        let (names, rows) = counter_rows(&case, field);
        check_counter(field, &rows, &bounding, first, last).unwrap_or_else(|e| panic!("instrumentation: {e}"));
        let mut w = Vec::new();
        for (a, b) in variant.windows() {
            w.push(serde_json::json!({
                "first": a, "last": b,
                "iterations_with_clipping": rows.iter().filter(|r| r.iteration >= a && r.iteration <= b && r.below > 0).count(),
                "mean_cells_below_floor": window_mean(&rows, a, b, |r| r.below as f64),
                "mean_cells_negative": window_mean(&rows, a, b, |r| r.negative as f64),
                "mean_volume_below_m3": window_mean(&rows, a, b, |r| r.volume_below),
                "lowest_min": rows.iter().filter(|r| r.iteration >= a && r.iteration <= b).map(|r| r.min).fold(f64::INFINITY, f64::min),
                "mean_region_cells": names.iter().enumerate().map(|(i, n)| (n.clone(), serde_json::json!(window_mean(&rows, a, b, |r| r.regions[i] as f64)))).collect::<serde_json::Map<_, _>>(),
                "mean_wall_band_cells": (0..4).map(|i| window_mean(&rows, a, b, |r| r.bands[i] as f64)).collect::<Vec<_>>(),
            }));
        }
        counters.insert(field.into(), serde_json::json!({
            "floor": rows[0].lower_bound, "rows": rows.len(), "windows": w,
            "mean_hook_seconds": window_mean(&rows, first, last, |r| r.hook_seconds),
            "mean_hook_local_max_seconds": window_mean(&rows, first, last, |r| r.hook_local_max_seconds),
            "file": case.join(format!("postProcessing/wp9ClipCounter/clip_{field}.dat")),
            "worst_cells": case.join(format!("postProcessing/wp9ClipCounter/clip_{field}_worst.dat")),
        }));
    }
    for i in variant.map_iterations() {
        for f in ["clipCount_k", "clipMin_k", "clipCount_omega", "clipMin_omega"] {
            assert!(case.join(format!("processor0/{i}/{f}")).is_file(), "map {f} kept at {i}");
        }
    }

    // 6. Residuals and screening forces; reproduction of the reference run
    //    (Stage 2 for C0, C0 for the R0 restart check).
    let residuals = parse_initial_residuals(&full_log);
    let residual_rms: Vec<serde_json::Value> = variant.windows().iter().map(|(a, b)| serde_json::json!({
        "first": a, "last": b, "p": rms_initial(&residuals, "p", *a, *b), "U": rms_initial(&residuals, "U", *a, *b),
        "k": rms_initial(&residuals, "k", *a, *b), "omega": rms_initial(&residuals, "omega", *a, *b),
    })).collect();
    let source_log = std::fs::read_to_string(source.join("logs/solver.log")).unwrap_or_default();
    let reference_log_path = match variant.reference() {
        Some(Reference::C0) => Some(start_source.join("logs/solver_phase1.log")),
        Some(Reference::Stage2) => Some(source.join("logs/solver.log")),
        None => None,
    };
    let reproduction = if let Some(reference_path) = &reference_log_path {
        let reference_log = std::fs::read_to_string(reference_path).unwrap_or_default();
        let c = compare_histories(&parse_initial_residuals(&reference_log), &residuals, first, last);
        let sb = parse_bounding(&reference_log).unwrap_or_default();
        let mut bounding_max_rel = 0.0f64;
        let mut bounding_compared = 0usize;
        for ((i, f), (min, _, _)) in bounding.iter() {
            if let Some((smin, _, _)) = sb.get(&(*i, f.clone())) {
                bounding_compared += 1;
                let s = min.abs().max(smin.abs()).max(f64::MIN_POSITIVE);
                bounding_max_rel = bounding_max_rel.max((min - smin).abs() / s);
            } else {
                bounding_max_rel = f64::INFINITY;
            }
        }
        serde_json::json!({
            "against": reference_path, "compared_iterations": c.compared_iterations, "compared_values": c.compared_values,
            "missing_iterations": c.missing_iterations, "max_rel_diff_initial_residuals": c.max_rel_diff,
            "first_iteration_over_1e_6": c.first_iteration_over_1e_6, "bounding_lines_compared": bounding_compared,
            "max_rel_diff_bounding_min": bounding_max_rel,
        })
    } else {
        serde_json::Value::Null
    };
    let source_clock = parse_clock(&source_log);
    if variant == Variant::R0 {
        // The restart from C0's written 500 must continue C0 exactly, so any
        // D4 difference comes from its schemes, not from the restart.
        let r = &reproduction;
        assert_eq!((r["compared_iterations"].as_u64(), r["missing_iterations"].as_u64()), (Some(u64::from(last - first + 1)), Some(0)), "R0 compares every iteration: {r}");
        assert!(r["max_rel_diff_initial_residuals"].as_f64().is_some_and(|d| d <= 1e-6), "R0 reproduces C0's residuals: {r}");
        assert!(r["max_rel_diff_bounding_min"].as_f64().is_some_and(|d| d <= 1e-6) && r["bounding_lines_compared"].as_u64() > Some(0), "R0 reproduces C0's bounding: {r}");
    }
    // Same iterations as the phase's own measurement (first+1 .. last).
    let source_spi = match (source_clock.get(&(first + 1)), source_clock.get(&last)) {
        (Some(a), Some(b)) if last > first + 1 => Some((b - a) / f64::from(last - first - 1)),
        _ => None,
    };
    let forces: Vec<serde_json::Value> = variant.force_windows().iter().map(|(a, b)| force_window(&case, *a, *b)).collect();

    let row = serde_json::json!({
        "scope": "turbulence_diagnostic", "variant": variant.id(), "diagnostic_only": true,
        "acceptance_evidence": false, "physical_validation": false,
        "source_case": source, "mesh_fingerprint_fnv": format!("{mesh_hash:016x}"), "copied_files": files, "copied_bytes": bytes,
        "copy_seconds": copy_seconds, "compile_seconds_on_fixture": compile_seconds, "compile_links_removed": links_removed, "np": 16,
        "phases": phase_rows, "iterations": last, "regions": regions.iter().map(|r| serde_json::json!({"name": r.0, "min": r.1, "max": r.2})).collect::<Vec<_>>(),
        "start_source_case": start_source, "start_time": phases[0].start,
        "counters": counters, "residual_rms": residual_rms,
        "reproduction_of_stage2": if variant.reference() == Some(Reference::Stage2) { reproduction.clone() } else { serde_json::Value::Null },
        "reproduction_of_c0": if variant.reference() == Some(Reference::C0) { reproduction } else { serde_json::Value::Null },
        "stage2_seconds_per_iteration_same_window": source_spi,
        "screening_forces_not_results": forces, "case_bytes": bytes_of(&case), "case": case,
    });
    std::fs::write(&out_path, serde_json::to_string_pretty(&row).unwrap()).unwrap();
    let back: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(back["variant"], variant.id());
    assert_eq!(back["iterations"], last);
    // Own line: --nocapture output follows cargo's "test name ... " prefix.
    println!("\nWP9_DIAGNOSTIC_COMPLETE variant={} iterations={} phases={} np=16 instrumentation=checked acceptance_evidence=0", variant.id(), last, phases.len());
}
