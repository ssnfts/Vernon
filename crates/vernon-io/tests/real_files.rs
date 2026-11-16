//! Real-file checks. Sample files live in `scratch/real-files/` (git-ignored,
//! see "Real-file checks" in `docs/VALIDATION.md`). Each test skips quietly
//! when its file is absent and is `#[ignore]`d in normal runs.
use std::path::PathBuf;
use vernon_io::{read, read_las_bytes, FileUnit, ReadOptions};

fn sample(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scratch/real-files").join(name);
    p.exists().then_some(p)
}

fn bounds(g: &vernon_core::Geometry) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::MAX; 3];
    let mut hi = [f64::MIN; 3];
    for p in &g.points {
        for (i, v) in [p.x, p.y, p.z].into_iter().enumerate() {
            lo[i] = lo[i].min(v);
            hi[i] = hi[i].max(v);
        }
    }
    (lo, hi)
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// `pump.e57` (Leica ScanStation, 5 scans, 2.88 M records of which the
/// invalid ones are dropped). Header values read from the file's own XML.
#[test]
#[ignore = "needs real sample files"]
fn pump_e57_matches_its_xml() {
    let Some(p) = sample("pump.e57") else { return };
    let g = read(&p, &ReadOptions::default()).unwrap();
    let records: usize = 420_838 + 370_530 + 389_576 + 258_020 + 1_440_000;
    assert!(g.points.len() > 1_000_000 && g.points.len() < records, "{}", g.points.len());
    // Scan 1 local xMin -0.005631 m, pose translation x -3.028748 m, identity-ish
    // rotation: world xMin about -3034.4 mm. Millimetres, not metres.
    let (lo, hi) = bounds(&g);
    assert!(near(lo[0], -3034.4, 1.0), "{lo:?}");
    assert!(hi[0] < 2000.0 && hi[1] < 0.0 && lo[1] > -6000.0, "{lo:?} {hi:?}");
    // The 1.44 M-record scan holds -2147.483649 m sentinels: none may survive.
    assert!(lo.iter().chain(hi.iter()).all(|v| v.abs() < 10_000.0), "{lo:?} {hi:?}");
    // intensity limits 0..1 (ScaledInteger 0..32767 x 3.0518509e-5); colour 0..255 -> 0..1.
    let i = g.point_attribs.floats("intensity").unwrap();
    assert!(i.iter().all(|v| (0.0..=1.0 + 1e-9).contains(v)));
    assert!(g.point_attribs.get("Cd").is_some());
}

/// `vegetation_1_3.las`: LAS 1.3, point format 1, 10 683 points, scale 0.001,
/// header bounds x -98451.205..-98447.447, y -55975.417..-55969.405,
/// z -81460.091..-81455.203, no coordinate-system record (read as metres).
#[test]
#[ignore = "needs real sample files"]
fn vegetation_las_matches_its_header() {
    let Some(p) = sample("vegetation_1_3.las") else { return };
    let g = read(&p, &ReadOptions::default()).unwrap();
    assert_eq!(g.points.len(), 10_683);
    let (lo, hi) = bounds(&g);
    for (got, want) in [(lo[0], -98451205.0), (hi[0], -98447447.0), (lo[1], -55975417.0), (hi[1], -55969405.0), (lo[2], -81460091.0), (hi[2], -81455203.0)] {
        assert!(near(got, want, 0.5), "{got} vs {want}");
    }
    assert!(g.point_attribs.get("intensity").is_some() && g.point_attribs.get("classification").is_some());
    assert_eq!(g.detail_attribs.strs("las_unit_source").unwrap(), &["assumed_metres".to_string()]);
}

/// `simple1_4.las`: LAS 1.4, format 7, 22 600 points, EPSG:4326 geographic
/// (degrees) with metre vertical units: refused until a unit is named.
#[test]
#[ignore = "needs real sample files"]
fn simple_las_1_4_is_geographic_and_refused_without_a_unit() {
    let Some(p) = sample("simple1_4.las") else { return };
    let e = read(&p, &ReadOptions::default()).unwrap_err().to_string();
    assert!(e.contains("geographic"), "{e}");
    let g = read_las_bytes(&std::fs::read(&p).unwrap(), Some(FileUnit::Millimetre)).unwrap();
    assert_eq!(g.points.len(), 22_600);
    let (lo, hi) = bounds(&g);
    // header z range 44..254 (scale 0.001), read as given with unit mm.
    assert!(near(lo[2], 44.0, 1e-6) && near(hi[2], 254.0, 1e-6), "{lo:?} {hi:?}");
    assert!(g.point_attribs.get("Cd").is_some());
}
