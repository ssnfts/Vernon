//! WP9 turbulence diagnostics (test-only). Screening runs on the accepted 12M
//! Fastback mesh that measure k/omega clipping before OpenFOAM bounds the
//! fields. Nothing here feeds a gate, the benchmark or the mesh study.
//!
//! The counter is a coded fvOption. In OpenFOAM v2606 `kOmegaSSTBase.C` calls
//! `solve`, then `fvOptions.correct`, then `bound`, so its `codeCorrect` sees
//! each field after the linear solve and before clipping. It adds no source and
//! no constraint, so the discretised equations are unchanged.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One phase is one solver process. A scheme change only ever happens between
/// processes, by restarting from a written time, never by editing a file that
/// a running solver may re-read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Phase {
    pub start: u32,
    pub end: u32,
    pub schemes: Schemes,
}

/// The fvSchemes a phase runs with. Each is the production file with only the
/// named lines changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Schemes {
    /// The production file unchanged.
    Production,
    /// k/omega convection switched to `bounded Gauss upwind`.
    UpwindTurbulence,
    /// Laplacian and surface-normal-gradient defaults with the explicit
    /// non-orthogonal correction limited to 0.33 (`limited corrected 0.33`).
    LimitedCorrection,
}

impl Schemes {
    pub fn label(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::UpwindTurbulence => "upwind k/omega convection",
            Self::LimitedCorrection => "limited corrected 0.33 (laplacian, snGrad)",
        }
    }

    pub fn apply(self, production: &str) -> Result<String, String> {
        match self {
            Self::Production => Ok(production.to_string()),
            Self::UpwindTurbulence => upwind_turbulence_schemes(production),
            Self::LimitedCorrection => limited_correction_schemes(production),
        }
    }
}

/// Which earlier log a variant must reproduce iteration by iteration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reference {
    /// The stopped Stage 2 run (`logs/solver.log` of the source case).
    Stage2,
    /// The C0 diagnostic (`logs/solver_phase1.log` of the start-source case).
    C0,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Variant {
    /// Real-case mechanics check: upwind 1–2, production 3–4.
    Smoke,
    /// Production numerics from iteration 0 to 1000, instrumented.
    C0,
    /// Spec startup: upwind k/omega 1–500, then production 501–1000.
    D1,
    /// Restart check: production schemes from C0's written iteration 500 for
    /// 501–510, which must reproduce C0's log exactly.
    R0,
    /// D4's restart control: production schemes from C0's iteration 500,
    /// 501–1000. Its departure from C0 is recorded, not asserted.
    R1,
    /// Limited non-orthogonal correction from C0's iteration 500, 501–1000.
    D4,
}

impl Variant {
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "smoke" => Ok(Self::Smoke),
            "c0" => Ok(Self::C0),
            "d1" => Ok(Self::D1),
            "r0" => Ok(Self::R0),
            "r1" => Ok(Self::R1),
            "d4" => Ok(Self::D4),
            other => Err(format!("unknown diagnostic variant '{other}' (smoke, c0, d1, r0, r1, d4)")),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::C0 => "c0",
            Self::D1 => "d1",
            Self::R0 => "r0",
            Self::R1 => "r1",
            Self::D4 => "d4",
        }
    }

    pub fn phases(self) -> Vec<Phase> {
        use Schemes::*;
        let p = |start, end, schemes| Phase { start, end, schemes };
        match self {
            Self::Smoke => vec![p(0, 2, UpwindTurbulence), p(2, 4, Production)],
            Self::C0 => vec![p(0, 1000, Production)],
            Self::D1 => vec![p(0, 500, UpwindTurbulence), p(500, 1000, Production)],
            Self::R0 => vec![p(500, 510, Production)],
            Self::R1 => vec![p(500, 1000, Production)],
            Self::D4 => vec![p(500, 1000, LimitedCorrection)],
        }
    }

    pub fn write_interval(self) -> u32 {
        match self {
            Self::Smoke => 2,
            Self::R0 => 10,
            Self::C0 | Self::D1 | Self::R1 | Self::D4 => 250,
        }
    }

    /// Iterations whose clipping maps are kept permanently (no purging).
    pub fn map_iterations(self) -> Vec<u32> {
        match self {
            Self::Smoke => vec![1, 2, 3, 4],
            Self::C0 | Self::D1 => vec![1, 10, 50, 100, 250, 500, 501, 510, 550, 750, 1000],
            Self::R0 => vec![501, 505, 510],
            Self::R1 | Self::D4 => vec![501, 510, 550, 750, 1000],
        }
    }

    /// Variants that start from C0's written iteration 500 instead of the
    /// Stage 2 case's initial fields.
    pub fn restarts_from_c0(self) -> bool {
        matches!(self, Self::R0 | Self::R1 | Self::D4)
    }

    pub fn reference(self) -> Option<Reference> {
        match self {
            Self::C0 => Some(Reference::Stage2),
            Self::R0 | Self::R1 => Some(Reference::C0),
            _ => None,
        }
    }

    /// Reporting windows for counters and residuals.
    pub fn windows(self) -> Vec<(u32, u32)> {
        let phases = self.phases();
        let (first, last) = (phases[0].start + 1, phases.last().unwrap().end);
        match (first, last) {
            (1, 1000) => vec![(1, 100), (401, 500), (501, 600), (901, 1000), (501, 1000)],
            (501, 1000) => vec![(501, 600), (901, 1000), (501, 1000)],
            _ => vec![(first, last)],
        }
    }

    /// Screening-force windows (never results).
    pub fn force_windows(self) -> Vec<(u32, u32)> {
        let phases = self.phases();
        let (first, last) = (phases[0].start + 1, phases.last().unwrap().end);
        if last == 1000 && first <= 501 { vec![(501, 1000), (751, 1000)] } else { vec![(first, last)] }
    }

    /// Solver-process limit: 40 s per iteration (measured 17) plus 30 minutes
    /// for reading the mesh and writing.
    pub fn phase_timeout_min(phase: &Phase) -> u64 {
        u64::from(phase.end - phase.start) * 40 / 60 + 30
    }
}

pub const PRODUCTION_K: &str = "    div(phi,k)      bounded Gauss limitedLinear 1;";
pub const PRODUCTION_OMEGA: &str = "    div(phi,omega)  bounded Gauss limitedLinear 1;";
pub const UPWIND_K: &str = "    div(phi,k)      bounded Gauss upwind;";
pub const UPWIND_OMEGA: &str = "    div(phi,omega)  bounded Gauss upwind;";

/// The production fvSchemes with only the two turbulence convection lines
/// switched to upwind. Anything else in the file stays byte-identical.
pub fn upwind_turbulence_schemes(production: &str) -> Result<String, String> {
    let mut out = String::with_capacity(production.len());
    let (mut k, mut omega) = (0, 0);
    for line in production.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        if body == PRODUCTION_K {
            k += 1;
            out.push_str(UPWIND_K);
            out.push_str(ending);
        } else if body == PRODUCTION_OMEGA {
            omega += 1;
            out.push_str(UPWIND_OMEGA);
            out.push_str(ending);
        } else {
            out.push_str(line);
        }
    }
    if k != 1 || omega != 1 {
        return Err(format!("expected exactly one production k and omega convection line, found {k} and {omega}"));
    }
    Ok(out)
}

pub const PRODUCTION_LAPLACIAN: &str = "    default         Gauss linear corrected;";
pub const PRODUCTION_SNGRAD: &str = "    default         corrected;";
pub const LIMITED_LAPLACIAN: &str = "    default         Gauss linear limited corrected 0.33;";
pub const LIMITED_SNGRAD: &str = "    default         limited corrected 0.33;";

/// The production fvSchemes with only the `default` lines of
/// `laplacianSchemes` and `snGradSchemes` limited to 0.33. The replacement is
/// made only inside those two blocks; anything else stays byte-identical.
pub fn limited_correction_schemes(production: &str) -> Result<String, String> {
    let mut out = String::with_capacity(production.len() + 32);
    let (mut lap, mut sn) = (0, 0);
    let mut block = String::new();
    let mut depth = 0i32;
    for line in production.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        if depth == 0 && !body.trim().is_empty() && !body.contains('{') && !body.contains('}') {
            block = body.trim().to_string();
        }
        if depth == 1 && block == "laplacianSchemes" && body == PRODUCTION_LAPLACIAN {
            lap += 1;
            out.push_str(LIMITED_LAPLACIAN);
            out.push_str(ending);
        } else if depth == 1 && block == "snGradSchemes" && body == PRODUCTION_SNGRAD {
            sn += 1;
            out.push_str(LIMITED_SNGRAD);
            out.push_str(ending);
        } else {
            out.push_str(line);
        }
        depth += body.matches('{').count() as i32 - body.matches('}').count() as i32;
    }
    if lap != 1 || sn != 1 {
        return Err(format!("expected exactly one production laplacian and snGrad default, found {lap} and {sn}"));
    }
    Ok(out)
}

/// The source controlDict with only the run-window keys changed. Function
/// objects (forces, residuals, y+) are kept exactly as in production.
pub fn diagnostic_control_dict(source: &str, start: u32, end: u32, write_interval: u32) -> Result<String, String> {
    let wanted: [(&str, String); 5] = [
        ("startTime", start.to_string()),
        ("endTime", end.to_string()),
        ("writeInterval", write_interval.to_string()),
        ("purgeWrite", "0".into()),
        ("runTimeModifiable", "false".into()),
    ];
    let mut seen = [0usize; 5];
    let mut out = String::with_capacity(source.len());
    let mut depth = 0i32;
    for line in source.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        let mut replaced = false;
        if depth == 0 {
            for (i, (key, value)) in wanted.iter().enumerate() {
                let mut words = body.split_whitespace();
                if words.next() == Some(*key) && body.trim_end().ends_with(';') {
                    seen[i] += 1;
                    out.push_str(&format!("{key:<15} {value};"));
                    out.push_str(ending);
                    replaced = true;
                    break;
                }
            }
        }
        if !replaced {
            out.push_str(line);
        }
        depth += body.matches('{').count() as i32 - body.matches('}').count() as i32;
    }
    for (i, (key, _)) in wanted.iter().enumerate() {
        if seen[i] != 1 {
            return Err(format!("controlDict must contain exactly one top-level '{key}', found {}", seen[i]));
        }
    }
    Ok(out)
}

/// `bounding <field>, min: <min> max: <max> average: <avg>` per iteration.
pub type Bounding = BTreeMap<(u32, String), (f64, f64, f64)>;

fn iteration_of(line: &str) -> Option<u32> {
    let rest = line.strip_prefix("Time = ")?;
    let value: f64 = rest.trim().parse().ok()?;
    (value >= 0.0 && value.fract() == 0.0).then_some(value as u32)
}

pub fn parse_bounding(log: &str) -> Result<Bounding, String> {
    let mut out = Bounding::new();
    let mut iteration = None;
    for line in log.lines() {
        if let Some(i) = iteration_of(line) {
            iteration = Some(i);
            continue;
        }
        let Some(rest) = line.strip_prefix("bounding ") else { continue };
        let i = iteration.ok_or("bounding line before any Time line")?;
        let (field, rest) = rest.split_once(", min: ").ok_or_else(|| format!("bad bounding line: {line}"))?;
        let (min, rest) = rest.split_once(" max: ").ok_or_else(|| format!("bad bounding line: {line}"))?;
        let (max, avg) = rest.split_once(" average: ").ok_or_else(|| format!("bad bounding line: {line}"))?;
        let num = |s: &str| s.trim().parse::<f64>().map_err(|_| format!("bad number in: {line}"));
        if out.insert((i, field.to_string()), (num(min)?, num(max)?, num(avg)?)).is_some() {
            return Err(format!("two bounding lines for {field} at iteration {i}"));
        }
    }
    Ok(out)
}

/// Initial residuals in log order for every iteration: (`Ux`, 1e-3), ...
pub fn parse_initial_residuals(log: &str) -> BTreeMap<u32, Vec<(String, f64)>> {
    let mut out: BTreeMap<u32, Vec<(String, f64)>> = BTreeMap::new();
    let mut iteration = None;
    for line in log.lines() {
        if let Some(i) = iteration_of(line) {
            iteration = Some(i);
            continue;
        }
        let (Some(i), Some(pos)) = (iteration, line.find("Solving for ")) else { continue };
        let rest = &line[pos + "Solving for ".len()..];
        let Some((field, rest)) = rest.split_once(", Initial residual = ") else { continue };
        let Some((value, _)) = rest.split_once(',') else { continue };
        if let Ok(v) = value.trim().parse::<f64>() {
            out.entry(i).or_default().push((field.to_string(), v));
        }
    }
    out
}

/// `ExecutionTime = a s  ClockTime = b s` per iteration (clock seconds).
pub fn parse_clock(log: &str) -> BTreeMap<u32, f64> {
    let mut out = BTreeMap::new();
    let mut iteration = None;
    for line in log.lines() {
        if let Some(i) = iteration_of(line) {
            iteration = Some(i);
        } else if let (Some(i), Some(pos)) = (iteration, line.find("ClockTime = ")) {
            if let Some(v) = line[pos + 12..].split_whitespace().next().and_then(|v| v.parse().ok()) {
                out.insert(i, v);
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub compared_iterations: usize,
    pub compared_values: usize,
    pub max_rel_diff: f64,
    pub first_iteration_over_1e_6: Option<u32>,
    pub missing_iterations: usize,
}

/// Iteration-by-iteration comparison of two histories over `first..=last`.
pub fn compare_histories(
    a: &BTreeMap<u32, Vec<(String, f64)>>,
    b: &BTreeMap<u32, Vec<(String, f64)>>,
    first: u32,
    last: u32,
) -> Comparison {
    let mut c = Comparison { compared_iterations: 0, compared_values: 0, max_rel_diff: 0.0, first_iteration_over_1e_6: None, missing_iterations: 0 };
    for i in first..=last {
        let (Some(x), Some(y)) = (a.get(&i), b.get(&i)) else {
            c.missing_iterations += 1;
            continue;
        };
        if x.len() != y.len() || x.iter().zip(y).any(|(p, q)| p.0 != q.0) {
            c.max_rel_diff = f64::INFINITY;
            c.first_iteration_over_1e_6.get_or_insert(i);
            continue;
        }
        c.compared_iterations += 1;
        for (p, q) in x.iter().zip(y) {
            c.compared_values += 1;
            let scale = p.1.abs().max(q.1.abs());
            let rel = if scale == 0.0 { 0.0 } else { (p.1 - q.1).abs() / scale };
            if rel > c.max_rel_diff {
                c.max_rel_diff = rel;
            }
            if rel > 1e-6 {
                c.first_iteration_over_1e_6.get_or_insert(i);
            }
        }
    }
    c
}

#[derive(Debug, Clone, PartialEq)]
pub struct CounterRow {
    pub iteration: u32,
    pub lower_bound: f64,
    pub min: f64,
    pub below: u64,
    pub negative: u64,
    pub zero: u64,
    pub volume_below: f64,
    pub volume_negative: f64,
    pub regions: Vec<u64>,
    pub bands: [u64; 4],
    /// The counter's own work, before communication; maximum over ranks.
    pub hook_local_max_seconds: f64,
    /// Master's wall time in the hook, including waits in its reductions.
    pub hook_seconds: f64,
}

/// Region names from the header, then one row per iteration.
pub fn parse_counter(text: &str) -> Result<(Vec<String>, Vec<CounterRow>), String> {
    let mut lines = text.lines();
    let header = lines.next().ok_or("empty counter file")?;
    let names: Vec<&str> = header.strip_prefix("# ").ok_or("counter header missing")?.split_whitespace().collect();
    let fixed = ["iter", "lowerBound", "min", "nBelow", "nNegative", "nZero", "volumeBelow", "volumeNegative"];
    if names.len() < fixed.len() + 7 || names[..fixed.len()] != fixed {
        return Err(format!("unexpected counter header: {header}"));
    }
    let regions: Vec<String> = names[fixed.len()..names.len() - 6]
        .iter()
        .map(|n| n.strip_prefix("region_").map(str::to_string).ok_or_else(|| format!("bad region column {n}")))
        .collect::<Result<_, _>>()?;
    if names[names.len() - 6..] != ["band_lt_0.002", "band_lt_0.01", "band_lt_0.05", "band_ge_0.05", "hookLocalMaxSeconds", "hookSeconds"] {
        return Err(format!("unexpected counter tail: {header}"));
    }
    let mut rows = Vec::new();
    for line in lines {
        if line.starts_with('#') {
            if line != header {
                return Err("a later process wrote a different counter header".into());
            }
            continue;
        }
        let v: Vec<&str> = line.split_whitespace().collect();
        if v.len() != names.len() {
            return Err(format!("counter row has {} columns, header {}: {line}", v.len(), names.len()));
        }
        let f = |s: &str| s.parse::<f64>().map_err(|_| format!("bad number '{s}' in: {line}"));
        let u = |s: &str| s.parse::<u64>().map_err(|_| format!("bad count '{s}' in: {line}"));
        let n = regions.len();
        rows.push(CounterRow {
            iteration: v[0].parse().map_err(|_| format!("bad iteration in: {line}"))?,
            lower_bound: f(v[1])?,
            min: f(v[2])?,
            below: u(v[3])?,
            negative: u(v[4])?,
            zero: u(v[5])?,
            volume_below: f(v[6])?,
            volume_negative: f(v[7])?,
            regions: v[8..8 + n].iter().map(|s| u(s)).collect::<Result<_, _>>()?,
            bands: [u(v[8 + n])?, u(v[9 + n])?, u(v[10 + n])?, u(v[11 + n])?],
            hook_local_max_seconds: f(v[12 + n])?,
            hook_seconds: f(v[13 + n])?,
        });
    }
    Ok((regions, rows))
}

/// Instrumentation self-check for one field: exactly one counter row per
/// iteration in `first..=last`, rows sum consistently, and the counter agrees
/// with OpenFOAM's own `bounding` output (which prints the pre-clip minimum
/// whenever, and only when, some value is below the floor).
pub fn check_counter(field: &str, rows: &[CounterRow], bounding: &Bounding, first: u32, last: u32) -> Result<(), String> {
    let iterations: Vec<u32> = rows.iter().map(|r| r.iteration).collect();
    let expected: Vec<u32> = (first..=last).collect();
    if iterations != expected {
        return Err(format!("{field}: counter iterations are not exactly {first}..={last} once each"));
    }
    for r in rows {
        let bound = bounding.get(&(r.iteration, field.to_string()));
        if r.regions.iter().sum::<u64>() != r.below || r.bands.iter().sum::<u64>() != r.below || r.negative + r.zero > r.below {
            return Err(format!("{field} {}: region, band or sign counts do not add up", r.iteration));
        }
        if !(r.lower_bound > 0.0 && r.lower_bound.is_finite()) {
            return Err(format!("{field} {}: lower bound {} is not a positive floor", r.iteration, r.lower_bound));
        }
        match (bound, r.below > 0) {
            (Some((log_min, _, _)), true) => {
                let scale = log_min.abs().max(r.min.abs()).max(f64::MIN_POSITIVE);
                // The log prints about 8 significant digits.
                if (log_min - r.min).abs() / scale > 1e-6 {
                    return Err(format!("{field} {}: counter min {} but log min {log_min}", r.iteration, r.min));
                }
            }
            (None, false) => {}
            (Some(_), false) => return Err(format!("{field} {}: OpenFOAM bounded the field but the counter saw nothing below the floor", r.iteration)),
            (None, true) => return Err(format!("{field} {}: counter saw {} cells below the floor but OpenFOAM did not bound", r.iteration, r.below)),
        }
    }
    Ok(())
}

/// Axis-aligned bounds of the `vertex` lines of an ASCII STL.
pub fn stl_bounds(text: &str) -> Option<([f64; 3], [f64; 3])> {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    let mut n = 0usize;
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("vertex ") else { continue };
        let mut it = rest.split_whitespace().map(|s| s.parse::<f64>());
        let (Some(Ok(x)), Some(Ok(y)), Some(Ok(z))) = (it.next(), it.next(), it.next()) else { return None };
        for (i, v) in [x, y, z].into_iter().enumerate() {
            lo[i] = lo[i].min(v);
            hi[i] = hi[i].max(v);
        }
        n += 1;
    }
    (n >= 3 && lo.iter().chain(&hi).all(|v| v.is_finite())).then_some((lo, hi))
}

pub type Region = (String, [f64; 3], [f64; 3]);

/// Classification boxes in priority order (first match wins; the rest is
/// `other`). They are descriptive bins for locating clipped cells, not claims
/// about the flow. Ground height is the lowest wheel point (wheels touch it).
pub fn case_regions(body: ([f64; 3], [f64; 3]), front: ([f64; 3], [f64; 3]), rear: ([f64; 3], [f64; 3])) -> Vec<Region> {
    let grow = |b: ([f64; 3], [f64; 3]), d: f64| ([b.0[0] - d, b.0[1] - d, b.0[2] - d], [b.1[0] + d, b.1[1] + d, b.1[2] + d]);
    let ground = front.0[2].min(rear.0[2]);
    let (blo, bhi) = body;
    let length = bhi[0] - blo[0];
    let (flo, fhi) = grow(front, 0.05);
    let (rlo, rhi) = grow(rear, 0.05);
    let (nlo, nhi) = grow(body, 0.10);
    vec![
        ("wheel_fr".into(), flo, fhi),
        ("wheel_rr".into(), rlo, rhi),
        ("ground_contact".into(), [nlo[0], nlo[1], ground - 1.0], [nhi[0], nhi[1], ground + 0.05]),
        ("underbody_band".into(), [nlo[0], nlo[1], ground - 1.0], [nhi[0], nhi[1], blo[2] + 0.10]),
        ("body_near".into(), nlo, nhi),
        ("near_wake".into(), [bhi[0] + 0.10, blo[1] - 0.30, ground - 1.0], [bhi[0] + length, bhi[1] + 0.30, bhi[2] + 0.30]),
    ]
}

fn vec3(v: [f64; 3]) -> String {
    format!("({:.9} {:.9} {:.9})", v[0], v[1], v[2])
}

/// Values the fixture injects after the solve and before bounding.
#[derive(Debug, Clone, Copy)]
pub struct Injection {
    pub a: ([f64; 3], [f64; 3]),
    pub b: ([f64; 3], [f64; 3]),
    pub c: ([f64; 3], [f64; 3]),
    pub a_k: f64,
    pub c_k: f64,
    pub a_omega: f64,
    pub c_omega: f64,
    /// The only iteration at which values are injected.
    pub at: u32,
}

const FOAM_HEADER: &str = "FoamFile\n{\n    version     2.0;\n    format      ascii;\n    class       dictionary;\n    location    \"system\";\n    object      fvOptions;\n}\n\n";

const TURBULENCE_BUILD: &str = r##"
        codeOptions
        #{
            -I$(LIB_SRC)/finiteVolume/lnInclude \
            -I$(LIB_SRC)/meshTools/lnInclude \
            -I$(LIB_SRC)/TurbulenceModels/turbulenceModels/lnInclude \
            -I$(LIB_SRC)/TurbulenceModels/incompressible/lnInclude \
            -I$(LIB_SRC)/transportModels
        #};

        codeLibs
        #{
            -lfiniteVolume -lmeshTools -lturbulenceModels -lincompressibleTurbulenceModels
        #};
"##;

const NO_SOURCE: &str = r##"
        codeAddSup
        #{
            // No source: the equation is not modified.
        #};

        codeAddSupRho
        #{
            // No source: the equation is not modified.
        #};

        codeConstrain
        #{
            // No constraint: the equation is not modified.
        #};
"##;

const INJECT_CORRECT: &str = r##"
        codeInclude
        #{
            #include "turbulentTransportModel.H"
        #};

        codeCorrect
        #{
            if (!coeffs_.get<labelList>("injectIterations").found(label(mesh_.time().value() + 0.5)))
            {
                return;
            }
            const incompressible::RASModel& wp9Ras =
                mesh_.lookupObject<incompressible::RASModel>(Foam::turbulenceModel::propertiesName);
            const bool wp9IsK = (fld.name() == "k");
            const scalar wp9Floor = wp9IsK ? wp9Ras.kMin().value() : wp9Ras.omegaMin().value();
            const boundBox wp9A(coeffs_.get<point>("aMin"), coeffs_.get<point>("aMax"));
            const boundBox wp9B(coeffs_.get<point>("bMin"), coeffs_.get<point>("bMax"));
            const boundBox wp9C(coeffs_.get<point>("cMin"), coeffs_.get<point>("cMax"));
            const scalar wp9AValue = coeffs_.get<scalar>(wp9IsK ? "aValueK" : "aValueOmega");
            const scalar wp9CValue = coeffs_.get<scalar>(wp9IsK ? "cValueK" : "cValueOmega");
            const vectorField& wp9Centres = mesh_.C().primitiveField();
            scalarField& wp9Values = fld.primitiveFieldRef();
            forAll(wp9Values, celli)
            {
                if (wp9A.contains(wp9Centres[celli])) { wp9Values[celli] = wp9AValue; }
                else if (wp9C.contains(wp9Centres[celli])) { wp9Values[celli] = wp9CValue; }
                else if (wp9B.contains(wp9Centres[celli])) { wp9Values[celli] = 0.5*wp9Floor; }
            }
        #};
"##;

/// The counter's C++ (identical text for the fixture and the real case, so the
/// library compiled on the fixture is the one the real case loads).
const COUNTER_CORRECT: &str = r##"
        codeInclude
        #{
            #include "wallDist.H"
            #include "turbulentTransportModel.H"
            #include "OSspecific.H"
            #include <algorithm>
            #include <array>
            #include <chrono>
            #include <fstream>
            #include <vector>
        #};

        codeCorrect
        #{
            const auto wp9Start = std::chrono::steady_clock::now();
            const Time& wp9Time = mesh_.time();
            const label wp9Iter = label(wp9Time.value() + 0.5);
            const word wp9Field = fld.name();
            const incompressible::RASModel& wp9Ras =
                mesh_.lookupObject<incompressible::RASModel>(Foam::turbulenceModel::propertiesName);
            const scalar wp9Floor =
                wp9Field == "k" ? wp9Ras.kMin().value()
              : wp9Field == "omega" ? wp9Ras.omegaMin().value()
              : -GREAT;

            static bool wp9Ready = false;
            static wordList wp9Order;
            static List<boundBox> wp9Boxes;
            static labelList wp9Maps;
            static HashTable<scalarField> wp9Counts;
            static HashTable<scalarField> wp9Mins;
            if (!wp9Ready)
            {
                wp9Order = coeffs_.get<wordList>("regionOrder");
                const dictionary& wp9RegionDict = coeffs_.subDict("regions");
                wp9Boxes.setSize(wp9Order.size());
                forAll(wp9Order, r)
                {
                    const dictionary& d = wp9RegionDict.subDict(wp9Order[r]);
                    wp9Boxes[r] = boundBox(d.get<point>("min"), d.get<point>("max"));
                }
                wp9Maps = coeffs_.get<labelList>("mapIterations");
                wp9Ready = true;
            }

            const scalarField& wp9Values = fld.primitiveField();
            const scalarField& wp9Volumes = mesh_.V().field();
            const vectorField& wp9Centres = mesh_.C().primitiveField();
            const scalarField& wp9Wall = wallDist::New(mesh_).y().primitiveField();
            if (!wp9Counts.found(wp9Field))
            {
                wp9Counts.insert(wp9Field, scalarField(wp9Values.size(), Zero));
                wp9Mins.insert(wp9Field, scalarField(wp9Values.size(), Zero));
            }
            scalarField& wp9Count = wp9Counts[wp9Field];
            scalarField& wp9Min = wp9Mins[wp9Field];

            const label wp9NRegions = wp9Order.size();
            labelList wp9RegionCounts(wp9NRegions + 1, Zero);
            labelList wp9BandCounts(4, Zero);
            label wp9Below = 0, wp9Negative = 0, wp9Zero = 0;
            scalar wp9VolBelow = 0, wp9VolNegative = 0, wp9Lowest = GREAT;
            std::vector<label> wp9Clipped;
            std::vector<label> wp9ClippedRegion;
            forAll(wp9Values, celli)
            {
                const scalar v = wp9Values[celli];
                wp9Lowest = min(wp9Lowest, v);
                if (v < wp9Floor)
                {
                    ++wp9Below;
                    wp9VolBelow += wp9Volumes[celli];
                    if (v < 0) { ++wp9Negative; wp9VolNegative += wp9Volumes[celli]; }
                    else if (v == 0) { ++wp9Zero; }
                    label region = wp9NRegions;
                    for (label r = 0; r < wp9NRegions; ++r)
                    {
                        if (wp9Boxes[r].contains(wp9Centres[celli])) { region = r; break; }
                    }
                    ++wp9RegionCounts[region];
                    const scalar y = wp9Wall[celli];
                    ++wp9BandCounts[y < 0.002 ? 0 : y < 0.01 ? 1 : y < 0.05 ? 2 : 3];
                    wp9Count[celli] += 1;
                    wp9Min[celli] = min(wp9Min[celli], v);
                    wp9Clipped.push_back(celli);
                    wp9ClippedRegion.push_back(region);
                }
            }
            // Local work only, before any communication (a reduction would
            // also wait for the slowest rank's solve); the maximum over ranks.
            scalar wp9LocalSeconds =
                std::chrono::duration<double>(std::chrono::steady_clock::now() - wp9Start).count();
            reduce(wp9LocalSeconds, maxOp<scalar>());
            reduce(wp9Below, sumOp<label>());
            reduce(wp9Negative, sumOp<label>());
            reduce(wp9Zero, sumOp<label>());
            reduce(wp9VolBelow, sumOp<scalar>());
            reduce(wp9VolNegative, sumOp<scalar>());
            reduce(wp9Lowest, minOp<scalar>());
            forAll(wp9RegionCounts, r) { reduce(wp9RegionCounts[r], sumOp<label>()); }
            forAll(wp9BandCounts, b) { reduce(wp9BandCounts[b], sumOp<label>()); }

            const bool wp9IsMap = wp9Maps.found(wp9Iter);
            List<scalarField> wp9Worst(Pstream::nProcs());
            if (wp9IsMap)
            {
                std::vector<std::size_t> order(wp9Clipped.size());
                for (std::size_t j = 0; j < order.size(); ++j) { order[j] = j; }
                std::sort(order.begin(), order.end(), [&](std::size_t a, std::size_t b)
                {
                    return wp9Values[wp9Clipped[a]] < wp9Values[wp9Clipped[b]];
                });
                const std::size_t keep = std::min<std::size_t>(order.size(), 20);
                scalarField local(9*keep);
                for (std::size_t j = 0; j < keep; ++j)
                {
                    const label c = wp9Clipped[order[j]];
                    local[9*j + 0] = wp9Values[c];
                    local[9*j + 1] = wp9Centres[c].x();
                    local[9*j + 2] = wp9Centres[c].y();
                    local[9*j + 3] = wp9Centres[c].z();
                    local[9*j + 4] = wp9Wall[c];
                    local[9*j + 5] = wp9ClippedRegion[order[j]];
                    local[9*j + 6] = wp9Volumes[c];
                    local[9*j + 7] = Pstream::myProcNo();
                    local[9*j + 8] = c;
                }
                wp9Worst[Pstream::myProcNo()] = local;
                Pstream::gatherList(wp9Worst);

                volScalarField wp9CountField
                (
                    IOobject("clipCount_" + wp9Field, wp9Time.timeName(), mesh_, IOobject::NO_READ, IOobject::NO_WRITE, false),
                    mesh_,
                    dimensionedScalar(dimless, Zero)
                );
                wp9CountField.primitiveFieldRef() = wp9Count;
                wp9CountField.write();
                volScalarField wp9MinField
                (
                    IOobject("clipMin_" + wp9Field, wp9Time.timeName(), mesh_, IOobject::NO_READ, IOobject::NO_WRITE, false),
                    mesh_,
                    dimensionedScalar(fld.dimensions(), Zero)
                );
                wp9MinField.primitiveFieldRef() = wp9Min;
                wp9MinField.write();
                wp9Count = 0;
                wp9Min = 0;
            }

            if (Pstream::master())
            {
                const double wp9Seconds =
                    std::chrono::duration<double>(std::chrono::steady_clock::now() - wp9Start).count();
                const fileName wp9Dir = wp9Time.globalPath()/"postProcessing"/"wp9ClipCounter";
                mkDir(wp9Dir);
                const fileName wp9Path = wp9Dir/("clip_" + wp9Field + ".dat");
                const bool wp9Fresh = !isFile(wp9Path);
                std::ofstream os(wp9Path.c_str(), std::ios::app);
                os.precision(17);
                if (wp9Fresh)
                {
                    os << "# iter lowerBound min nBelow nNegative nZero volumeBelow volumeNegative";
                    forAll(wp9Order, r) { os << " region_" << wp9Order[r].c_str(); }
                    os << " region_other band_lt_0.002 band_lt_0.01 band_lt_0.05 band_ge_0.05 hookLocalMaxSeconds hookSeconds\n";
                }
                os << wp9Iter << ' ' << wp9Floor << ' ' << wp9Lowest << ' ' << wp9Below << ' ' << wp9Negative << ' '
                   << wp9Zero << ' ' << wp9VolBelow << ' ' << wp9VolNegative;
                forAll(wp9RegionCounts, r) { os << ' ' << wp9RegionCounts[r]; }
                forAll(wp9BandCounts, b) { os << ' ' << wp9BandCounts[b]; }
                os << ' ' << wp9LocalSeconds << ' ' << wp9Seconds << '\n';

                if (wp9IsMap)
                {
                    std::vector<std::array<double, 9>> all;
                    forAll(wp9Worst, p)
                    {
                        const scalarField& w = wp9Worst[p];
                        for (label j = 0; 9*j + 8 < w.size(); ++j)
                        {
                            std::array<double, 9> e;
                            for (int k = 0; k < 9; ++k) { e[k] = w[9*j + k]; }
                            all.push_back(e);
                        }
                    }
                    std::sort(all.begin(), all.end(), [](const std::array<double, 9>& a, const std::array<double, 9>& b) { return a[0] < b[0]; });
                    if (all.size() > 20) { all.resize(20); }
                    const fileName wp9WorstPath = wp9Dir/("clip_" + wp9Field + "_worst.dat");
                    const bool wp9WorstFresh = !isFile(wp9WorstPath);
                    std::ofstream ws(wp9WorstPath.c_str(), std::ios::app);
                    ws.precision(17);
                    if (wp9WorstFresh) { ws << "# iter value x y z wallDistance region volume processor cell\n"; }
                    for (const auto& e : all)
                    {
                        const label region = label(e[5]);
                        ws << wp9Iter << ' ' << e[0] << ' ' << e[1] << ' ' << e[2] << ' ' << e[3] << ' ' << e[4] << ' '
                           << (region < wp9NRegions ? wp9Order[region].c_str() : "other") << ' ' << e[6] << ' '
                           << label(e[7]) << ' ' << label(e[8]) << '\n';
                    }
                }
            }
        #};
"##;

/// system/fvOptions: optionally the fixture injector (first, so it runs
/// before the counter), then the counter.
pub fn fv_options(regions: &[Region], map_iterations: &[u32], inject: Option<&Injection>) -> String {
    let mut o = String::from(FOAM_HEADER);
    if let Some(i) = inject {
        o += "wp9Inject\n{\n    type            scalarCodedSource;\n    name            wp9Inject;\n    scalarCodedSourceCoeffs\n    {\n";
        o += "        selectionMode   all;\n        fields          (k omega);\n";
        for (key, b) in [("a", i.a), ("b", i.b), ("c", i.c)] {
            o += &format!("        {key}Min            {};\n        {key}Max            {};\n", vec3(b.0), vec3(b.1));
        }
        o += &format!("        aValueK         {:e};\n        cValueK         {:e};\n        aValueOmega     {:e};\n        cValueOmega     {:e};\n", i.a_k, i.c_k, i.a_omega, i.c_omega);
        o += &format!("        injectIterations ({});\n", i.at);
        o += TURBULENCE_BUILD;
        o += INJECT_CORRECT;
        o += NO_SOURCE;
        o += "    }\n}\n\n";
    }
    o += "wp9ClipCounter\n{\n    type            scalarCodedSource;\n    name            wp9ClipCounter;\n    scalarCodedSourceCoeffs\n    {\n";
    o += "        selectionMode   all;\n        fields          (k omega);\n";
    let maps: Vec<String> = map_iterations.iter().map(u32::to_string).collect();
    o += &format!("        mapIterations   ({});\n", maps.join(" "));
    let names: Vec<&str> = regions.iter().map(|r| r.0.as_str()).collect();
    o += &format!("        regionOrder     ({});\n        regions\n        {{\n", names.join(" "));
    for (name, lo, hi) in regions {
        o += &format!("            {name}\n            {{\n                min {};\n                max {};\n            }}\n", vec3(*lo), vec3(*hi));
    }
    o += "        }\n";
    o += TURBULENCE_BUILD;
    o += COUNTER_CORRECT;
    o += NO_SOURCE;
    o += "    }\n}\n";
    o
}

/// FNV-1a 64 over a file's bytes, streamed.
pub fn fnv1a_file(path: &Path) -> std::io::Result<u64> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hash);
        }
        for b in &buf[..n] {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Recursive copy of ordinary files and directories; returns (files, bytes).
/// Links (OpenFOAM's `lnInclude` symlinks in a compile tree) are not copied:
/// only the sources and the built library are needed to reuse a compile.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<(u64, u64)> {
    std::fs::create_dir_all(to)?;
    let (mut files, mut bytes) = (0, 0);
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            let (f, b) = copy_tree(&entry.path(), &target)?;
            files += f;
            bytes += b;
        } else if kind.is_file() {
            bytes += std::fs::copy(entry.path(), &target)?;
            files += 1;
        }
    }
    Ok((files, bytes))
}

/// Remove every link or special entry below `dir` (the run's cleanup scanner
/// refuses reparse points under its temp tree). Returns how many were removed.
pub fn remove_links(dir: &Path) -> std::io::Result<u64> {
    let mut removed = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            removed += remove_links(&entry.path())?;
        } else if !kind.is_file() {
            std::fs::remove_file(entry.path()).or_else(|_| std::fs::remove_dir(entry.path()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn processor_dirs(case: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out: Vec<(u32, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(case)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(n) = name.strip_prefix("processor").and_then(|n| n.parse::<u32>().ok()) {
            out.push((n, entry.path()));
        }
    }
    out.sort();
    Ok(out.into_iter().map(|p| p.1).collect())
}

#[cfg(test)]
mod tests;
