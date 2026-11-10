//! Matched restart arms from a preserved checkpoint of the limited-gradient
//! Stage 2 rerun (WP9 Stage 2 investigation, 2026-11-10). Each arm changes at
//! most one fvSolution setting; the fvSchemes are the rerun's own. Screening
//! evidence only: a restart is never a protocol result.

use std::collections::BTreeMap;
use std::path::Path;

use super::{fnv1a_file, Phase, Schemes};

/// Iterations each arm runs after the checkpoint.
pub const ARM_ITERATIONS: u32 = 500;
/// Restarted iterations excluded from comparisons (restarts are not bit-exact).
pub const ARM_SETTLE: u32 = 100;
/// Open boundaries whose flux must balance (walls and the symmetry plane carry none).
pub const OPEN_PATCHES: [&str; 4] = ["inlet", "outlet", "top", "side"];
/// Fields every rank must hold at the checkpoint time.
pub const CHECKPOINT_FIELDS: [&str; 6] = ["U", "p", "k", "omega", "nut", "phi"];
pub const CHECKPOINT_FILE: &str = "vernon_checkpoint.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm {
    /// Unchanged D5 recipe: the restart control every candidate is compared with.
    Control,
    /// Pressure `relTol` 0.1 -> 0.01.
    PTight,
    /// U `relTol` 0.1 -> 0.01; k and omega unchanged.
    UTight,
    /// k and omega equation relaxation 0.9 -> 0.5; U unchanged.
    TRelax,
    /// U equation relaxation 0.9 -> 0.7; k and omega unchanged.
    URelax,
    /// k and omega `relTol` 0.1 -> 0.01; U unchanged (conditional arm).
    TTight,
}

impl Arm {
    pub const ALL: [Arm; 6] = [Arm::Control, Arm::PTight, Arm::UTight, Arm::TRelax, Arm::URelax, Arm::TTight];

    pub fn id(self) -> &'static str {
        match self {
            Self::Control => "s2c",
            Self::PTight => "ptight",
            Self::UTight => "utight",
            Self::TRelax => "trelax",
            Self::URelax => "urelax",
            Self::TTight => "ttight",
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|a| a.id() == text).ok_or_else(|| format!("unknown checkpoint arm '{text}'"))
    }

    pub fn solution(self) -> Solution {
        match self {
            Self::Control => Solution::Production,
            Self::PTight => Solution::PressureRelTol,
            Self::UTight => Solution::VelocityRelTol,
            Self::TRelax => Solution::TurbulenceRelaxation,
            Self::URelax => Solution::VelocityRelaxation,
            Self::TTight => Solution::TurbulenceRelTol,
        }
    }

    /// One solver process from the checkpoint, the rerun's own schemes.
    pub fn plan(self, checkpoint: u32) -> Plan {
        let (start, end) = (checkpoint, checkpoint + ARM_ITERATIONS);
        let settled = start + ARM_SETTLE + 1;
        let tail = end - 249;
        Plan {
            id: self.id(),
            phases: vec![Phase { start, end, schemes: Schemes::Production }],
            windows: vec![(start + 1, start + ARM_SETTLE), (settled, end), (tail, end)],
            force_windows: vec![(settled, end), (tail, end)],
            map_iterations: [1, 10, 50, 100, 250, 500].iter().map(|i| start + i).collect(),
            write_interval: 250,
            solution: self.solution(),
            monitors: true,
        }
    }
}

/// Everything a diagnostic run needs to know about its iterations.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub id: &'static str,
    pub phases: Vec<Phase>,
    pub windows: Vec<(u32, u32)>,
    pub force_windows: Vec<(u32, u32)>,
    pub map_iterations: Vec<u32>,
    pub write_interval: u32,
    pub solution: Solution,
    /// Per-iteration open-boundary flux and field-range function objects.
    pub monitors: bool,
}

/// The fvSolution an arm runs with: the production file with only the named
/// entries changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Solution {
    Production,
    PressureRelTol,
    VelocityRelTol,
    TurbulenceRelTol,
    TurbulenceRelaxation,
    VelocityRelaxation,
}

pub const LOOSE_REL_TOL: &str = "        relTol          0.1;";
pub const TIGHT_REL_TOL: &str = "        relTol          0.01;";
pub const COMBINED_SOLVER: &str = "    \"(U|k|omega)\"";
pub const TURBULENCE_SOLVER: &str = "    \"(k|omega)\"";
pub const PRODUCTION_U_RELAX: &str = "        U               0.9;";
pub const REDUCED_U_RELAX: &str = "        U               0.7;";
pub const DEFAULT_RELAX: &str = "        \".*\"            0.9;";
pub const K_RELAX: &str = "        k               0.5;";
pub const OMEGA_RELAX: &str = "        omega           0.5;";

impl Solution {
    pub fn label(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::PressureRelTol => "p relTol 0.01",
            Self::VelocityRelTol => "U relTol 0.01 (k, omega 0.1)",
            Self::TurbulenceRelTol => "k, omega relTol 0.01 (U 0.1)",
            Self::TurbulenceRelaxation => "k, omega equation relaxation 0.5",
            Self::VelocityRelaxation => "U equation relaxation 0.7",
        }
    }

    pub fn apply(self, production: &str) -> Result<String, String> {
        match self {
            Self::Production => Ok(production.to_string()),
            Self::PressureRelTol => pressure_rel_tol(production),
            Self::VelocityRelTol => split_velocity_turbulence(production, true),
            Self::TurbulenceRelTol => split_velocity_turbulence(production, false),
            Self::TurbulenceRelaxation => turbulence_relaxation(production),
            Self::VelocityRelaxation => velocity_relaxation(production),
        }
    }

    /// Fields whose every linear solve must end at or below relTol 0.01.
    pub fn tight_fields(self) -> &'static [&'static str] {
        match self {
            Self::PressureRelTol => &["p"],
            Self::VelocityRelTol => &["Ux", "Uy", "Uz"],
            Self::TurbulenceRelTol => &["k", "omega"],
            _ => &[],
        }
    }
}

/// Each line (with its ending) and the names of the dictionaries around it,
/// outermost first. A key is the last bare line before a lone `{`.
fn with_paths(text: &str) -> Result<Vec<(Vec<String>, &str)>, String> {
    let mut out = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut key: Option<String> = None;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']).trim();
        out.push((stack.clone(), line));
        if body == "{" {
            stack.push(key.take().unwrap_or_default());
        } else if body == "}" {
            stack.pop().ok_or("unbalanced '}'")?;
        } else if body.contains('{') || body.contains('}') {
            return Err(format!("single-line dictionary not supported here: '{body}'"));
        } else if !body.is_empty() && !body.ends_with(';') && !body.starts_with("//") {
            key = Some(body.to_string());
        }
    }
    if !stack.is_empty() {
        return Err("unbalanced '{'".into());
    }
    Ok(out)
}

fn ending(line: &str) -> &str {
    &line[line.trim_end_matches(['\r', '\n']).len()..]
}

fn body(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

fn at(path: &[String], want: &[&str]) -> bool {
    path.len() == want.len() && path.iter().zip(want).all(|(a, b)| a == b)
}

fn pressure_rel_tol(production: &str) -> Result<String, String> {
    let mut out = String::with_capacity(production.len());
    let mut n = 0;
    for (path, line) in with_paths(production)? {
        if at(&path, &["solvers", "p"]) && body(line) == LOOSE_REL_TOL {
            n += 1;
            out.push_str(TIGHT_REL_TOL);
            out.push_str(ending(line));
        } else {
            out.push_str(line);
        }
    }
    if n != 1 {
        return Err(format!("expected exactly one 'relTol 0.1' in solvers/p, found {n}"));
    }
    Ok(out)
}

/// Splits `"(U|k|omega)"` into `U` and `"(k|omega)"` with identical settings
/// except the tightened relTol of one of them.
fn split_velocity_turbulence(production: &str, tighten_u: bool) -> Result<String, String> {
    let lines = with_paths(production)?;
    for (path, line) in &lines {
        let b = body(line);
        if path.len() == 1 && path[0] == "solvers" && (b == "    U" || b == TURBULENCE_SOLVER) {
            return Err(format!("solvers already has a separate entry '{}'", b.trim()));
        }
    }
    let starts: Vec<usize> = lines.iter().enumerate().filter(|(_, (p, l))| at(p, &["solvers"]) && body(l) == COMBINED_SOLVER).map(|(i, _)| i).collect();
    if starts.len() != 1 {
        return Err(format!("expected exactly one {} solver entry, found {}", COMBINED_SOLVER.trim(), starts.len()));
    }
    let s = starts[0];
    let key = COMBINED_SOLVER.trim().to_string();
    if lines.get(s + 1).map(|l| body(l.1).trim()) != Some("{") {
        return Err("the combined solver entry must open with '{' on the next line".into());
    }
    let close = (s + 2..lines.len())
        .find(|i| body(lines[*i].1).trim() == "}" && lines[*i].0.len() == 2 && lines[*i].0[1] == key)
        .ok_or("the combined solver entry is not closed")?;
    if (s + 2..close).any(|i| lines[i].0.len() != 2) {
        return Err("nested dictionaries inside the combined solver entry are not supported".into());
    }
    let entries: Vec<&str> = (s + 2..close).map(|i| lines[i].1).collect();
    if entries.iter().filter(|l| body(l) == LOOSE_REL_TOL).count() != 1 {
        return Err("the combined solver entry must have exactly one 'relTol 0.1'".into());
    }
    let nl = ending(lines[s].1);
    let nl = if nl.is_empty() { "\n" } else { nl };
    let emit = |name: &str, tight: bool, out: &mut String| {
        out.push_str(name);
        out.push_str(nl);
        out.push_str(lines[s + 1].1);
        for e in &entries {
            if tight && body(e) == LOOSE_REL_TOL {
                out.push_str(TIGHT_REL_TOL);
                out.push_str(ending(e));
            } else {
                out.push_str(e);
            }
        }
        out.push_str(body(lines[close].1));
        out.push_str(nl);
    };
    let mut out = String::with_capacity(production.len() + 256);
    for (_, line) in &lines[..s] {
        out.push_str(line);
    }
    emit("    U", tighten_u, &mut out);
    out.push_str(nl);
    emit(TURBULENCE_SOLVER, !tighten_u, &mut out);
    for (_, line) in &lines[close + 1..] {
        out.push_str(line);
    }
    Ok(out)
}

fn turbulence_relaxation(production: &str) -> Result<String, String> {
    let mut out = String::with_capacity(production.len() + 64);
    let mut n = 0;
    for (path, line) in with_paths(production)? {
        let in_equations = at(&path, &["relaxationFactors", "equations"]);
        let first = body(line).split_whitespace().next().unwrap_or("");
        if in_equations && (first == "k" || first == "omega" || first.contains("k|") || first.contains("|omega")) {
            return Err(format!("equations already relax a turbulence field: '{}'", body(line).trim()));
        }
        out.push_str(line);
        if in_equations && body(line) == DEFAULT_RELAX {
            n += 1;
            let nl = if ending(line).is_empty() { "\n" } else { ending(line) };
            out.push_str(K_RELAX);
            out.push_str(nl);
            out.push_str(OMEGA_RELAX);
            out.push_str(nl);
        }
    }
    if n != 1 {
        return Err(format!("expected exactly one default equation relaxation 0.9, found {n}"));
    }
    Ok(out)
}

fn velocity_relaxation(production: &str) -> Result<String, String> {
    let mut out = String::with_capacity(production.len());
    let mut n = 0;
    for (path, line) in with_paths(production)? {
        if at(&path, &["relaxationFactors", "equations"]) && body(line) == PRODUCTION_U_RELAX {
            n += 1;
            out.push_str(REDUCED_U_RELAX);
            out.push_str(ending(line));
        } else {
            out.push_str(line);
        }
    }
    if n != 1 {
        return Err(format!("expected exactly one U equation relaxation 0.9, found {n}"));
    }
    Ok(out)
}

// ----------------------------------------------------------- monitors

fn monitor_objects(open_patches: &[&str]) -> String {
    let mut o = String::new();
    for p in open_patches {
        o += &format!("\n    wp9Flux_{p}\n    {{\n        type            surfaceFieldValue;\n        libs            (fieldFunctionObjects);\n        writeControl    timeStep;\n        writeInterval   1;\n        log             false;\n        writeFields     false;\n        regionType      patch;\n        name            {p};\n        operation       sum;\n        fields          (phi);\n    }}\n");
    }
    o += "\n    wp9Range\n    {\n        type            fieldMinMax;\n        libs            (fieldFunctionObjects);\n        writeControl    timeStep;\n        writeInterval   1;\n        log             false;\n        mode            magnitude;\n        location        false;\n        fields          (U p k omega nut);\n    }\n";
    o
}

/// The controlDict with read-only per-iteration monitors added at the end of
/// its top-level `functions` block: the summed flux through each open patch
/// and the magnitude range of U, p, k, omega and nut. Nothing else changes.
pub fn with_monitors(control: &str, open_patches: &[&str]) -> Result<String, String> {
    if control.contains("wp9Flux_") || control.contains("wp9Range") {
        return Err("controlDict already has WP9 monitors".into());
    }
    let lines = with_paths(control)?;
    let mut closes = Vec::new();
    for (i, (path, line)) in lines.iter().enumerate() {
        if at(path, &["functions"]) && body(line).trim() == "}" {
            closes.push(i);
        }
    }
    if closes.len() != 1 {
        return Err(format!("expected exactly one top-level functions block, found {}", closes.len()));
    }
    let mut out = String::with_capacity(control.len() + 2048);
    for (i, (_, line)) in lines.iter().enumerate() {
        if i == closes[0] {
            let objects = monitor_objects(open_patches);
            if line.ends_with("\r\n") {
                out.push_str(&objects.replace('\n', "\r\n"));
            } else {
                out.push_str(&objects);
            }
        }
        out.push_str(line);
    }
    Ok(out)
}

/// Column names and the rows keyed by iteration.
pub type DatTable = (Vec<String>, BTreeMap<u32, Vec<f64>>);

/// Rows of a function-object .dat file: iteration -> values, with the column
/// names of its last `# Time ...` header. A repeated iteration is refused.
pub fn parse_dat(text: &str) -> Result<DatTable, String> {
    let mut names = Vec::new();
    let mut rows = BTreeMap::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix('#') {
            let h = h.trim();
            if h.starts_with("Time") {
                names = h.split_whitespace().skip(1).map(str::to_string).collect();
            }
            continue;
        }
        let v: Vec<&str> = line.split_whitespace().collect();
        if v.is_empty() {
            continue;
        }
        let t: f64 = v[0].parse().map_err(|_| format!("bad time in: {line}"))?;
        if t < 0.0 || t.fract() != 0.0 {
            return Err(format!("non-integer iteration in: {line}"));
        }
        let vals: Vec<f64> = v[1..].iter().map(|s| s.parse::<f64>().map_err(|_| format!("bad number '{s}' in: {line}"))).collect::<Result<_, _>>()?;
        if !names.is_empty() && vals.len() != names.len() {
            return Err(format!("row has {} values, header {}: {line}", vals.len(), names.len()));
        }
        if rows.insert(t as u32, vals).is_some() {
            return Err(format!("iteration {t} appears twice"));
        }
    }
    if names.is_empty() {
        return Err("no '# Time' header".into());
    }
    Ok((names, rows))
}

/// Signed net outflow through the open patches divided by the inflow
/// magnitude, per iteration. `fluxes` holds each patch's summed phi.
pub fn flux_balance(fluxes: &[(&str, BTreeMap<u32, f64>)], first: u32, last: u32) -> Result<BTreeMap<u32, f64>, String> {
    let inlet = fluxes.iter().find(|f| f.0 == "inlet").ok_or("no inlet flux")?;
    let mut out = BTreeMap::new();
    for i in first..=last {
        let inflow = *inlet.1.get(&i).ok_or_else(|| format!("inlet flux missing at {i}"))?;
        if !(inflow < 0.0 && inflow.is_finite()) {
            return Err(format!("inlet flux at {i} is {inflow}, expected a finite inflow (negative phi)"));
        }
        let mut net = 0.0;
        for (name, rows) in fluxes {
            net += *rows.get(&i).ok_or_else(|| format!("{name} flux missing at {i}"))?;
        }
        out.insert(i, net / inflow.abs());
    }
    Ok(out)
}

// ----------------------------------------------------------- lookup proof

/// (iteration, initial, final, linear iterations) of every solve of `field`.
pub fn parse_solves(log: &str, field: &str) -> Vec<(u32, f64, f64, u32)> {
    let mut out = Vec::new();
    let mut iteration = None;
    let needle = format!("Solving for {field}, Initial residual = ");
    for line in log.lines() {
        if let Some(rest) = line.strip_prefix("Time = ") {
            iteration = rest.trim().parse::<f64>().ok().map(|v| v as u32);
            continue;
        }
        let (Some(i), Some(pos)) = (iteration, line.find(&needle)) else { continue };
        let rest = &line[pos + needle.len()..];
        let mut parts = rest.split(',');
        let initial = parts.next().and_then(|s| s.trim().parse::<f64>().ok());
        let fin = parts.next().and_then(|s| s.trim().strip_prefix("Final residual = ")).and_then(|s| s.trim().parse::<f64>().ok());
        let n = parts.next().and_then(|s| s.trim().strip_prefix("No Iterations ")).and_then(|s| s.trim().parse::<u32>().ok());
        if let (Some(a), Some(b), Some(n)) = (initial, fin, n) {
            out.push((i, a, b, n));
        }
    }
    out
}

/// Proof from the solver's own output that each field used the intended
/// relTol: a tight field ends every solve at or below 0.01 of its initial
/// residual (or the 1e-10 absolute tolerance); a loose field ends every solve
/// at or below 0.1 and typically well above 0.01 (median ratio > 0.02).
pub fn check_rel_tol(log: &str, field: &str, tight: bool) -> Result<serde_json::Value, String> {
    let solves = parse_solves(log, field);
    if solves.is_empty() {
        return Err(format!("{field}: no solves in the log"));
    }
    let target = if tight { 0.01 } else { 0.1 };
    let mut ratios = Vec::with_capacity(solves.len());
    for (i, a, b, _) in &solves {
        if *b <= 1e-10 {
            continue;
        }
        let r = b / a;
        if r > target * (1.0 + 1e-6) {
            return Err(format!("{field} at {i}: final/initial {r:.4} exceeds relTol {target}"));
        }
        ratios.push(r);
    }
    ratios.sort_by(f64::total_cmp);
    let median = ratios.get(ratios.len() / 2).copied().unwrap_or(0.0);
    if !tight && median <= 0.02 {
        return Err(format!("{field}: median final/initial {median:.4} is not that of relTol 0.1"));
    }
    let mean_iterations = solves.iter().map(|s| f64::from(s.3)).sum::<f64>() / solves.len() as f64;
    Ok(serde_json::json!({ "field": field, "rel_tol": target, "solves": solves.len(), "median_ratio": median, "max_ratio": ratios.last(), "mean_linear_iterations": mean_iterations }))
}

// ----------------------------------------------------------- checkpoint

/// Every file a restart from `time` needs, relative to the case: each rank's
/// mesh and its `time` directory (fields and `uniform/`), and the case's
/// `system/`, `constant/` settings and logs. Sorted.
pub fn checkpoint_files(case: &Path, time: u32) -> Result<Vec<String>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for e in entries {
            let e = e.map_err(|e| e.to_string())?;
            let kind = e.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() {
                walk(root, &e.path(), out)?;
            } else if kind.is_file() {
                let rel = e.path().strip_prefix(root).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
                out.push(rel);
            } else {
                return Err(format!("link or special file in a checkpoint: {}", e.path().display()));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    let ranks = super::processor_dirs(case).map_err(|e| e.to_string())?;
    if ranks.len() != 16 {
        return Err(format!("expected 16 processor directories, found {}", ranks.len()));
    }
    for (n, rank) in ranks.iter().enumerate() {
        if rank.file_name().and_then(|f| f.to_str()) != Some(format!("processor{n}").as_str()) {
            return Err(format!("processor directories are not processor0..15: {}", rank.display()));
        }
        for sub in ["constant/polyMesh".to_string(), time.to_string()] {
            let d = rank.join(&sub);
            if !d.is_dir() {
                return Err(format!("missing {}", d.display()));
            }
            walk(case, &d, &mut out)?;
        }
        for f in CHECKPOINT_FIELDS {
            if !rank.join(time.to_string()).join(f).is_file() {
                return Err(format!("processor{n}/{time} has no {f}"));
            }
        }
    }
    for f in ["system/controlDict", "system/fvSchemes", "system/fvSolution", "system/decomposeParDict", "constant/transportProperties", "constant/turbulenceProperties", "logs/solver.log"] {
        if !case.join(f).is_file() {
            return Err(format!("missing {f}"));
        }
        out.push(f.to_string());
    }
    let tri = case.join("constant/triSurface");
    if tri.is_dir() {
        walk(case, &tri, &mut out)?;
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// Copy a stopped run's checkpoint to an empty directory and write its
/// manifest: every file is hashed in the source and in the copy, and the two
/// must agree. The source is only read.
pub fn preserve_checkpoint(from: &Path, to: &Path, time: u32, provenance: serde_json::Value) -> Result<serde_json::Value, String> {
    if to.exists() && std::fs::read_dir(to).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("{} is not empty", to.display()));
    }
    let files = checkpoint_files(from, time)?;
    let mut hashes = serde_json::Map::new();
    let mut bytes = 0u64;
    for rel in &files {
        let (src, dst) = (from.join(rel), to.join(rel));
        std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
        let before = fnv1a_file(&src).map_err(|e| format!("{rel}: {e}"))?;
        bytes += std::fs::copy(&src, &dst).map_err(|e| format!("{rel}: {e}"))?;
        let (after, copied) = (fnv1a_file(&src).map_err(|e| e.to_string())?, fnv1a_file(&dst).map_err(|e| e.to_string())?);
        if before != after || after != copied {
            return Err(format!("{rel} changed while it was copied, or the copy differs"));
        }
        hashes.insert(rel.clone(), serde_json::json!(format!("{copied:016x}")));
    }
    let doc = serde_json::json!({
        "scope": "wp9_stage2_checkpoint", "diagnostic_only": true, "time": time,
        "source_case": from, "file_count": files.len(), "bytes": bytes,
        "hash": "fnv1a64", "provenance": provenance, "files": hashes,
    });
    std::fs::write(to.join(CHECKPOINT_FILE), serde_json::to_string_pretty(&doc).unwrap()).map_err(|e| e.to_string())?;
    verify_checkpoint(to)?;
    Ok(doc)
}

/// Re-hash every file named by the copy's `vernon_checkpoint.json` and
/// return the checkpoint time. Any missing or changed file is refused.
pub fn verify_checkpoint(case: &Path) -> Result<u32, String> {
    let text = std::fs::read_to_string(case.join(CHECKPOINT_FILE)).map_err(|e| format!("{CHECKPOINT_FILE}: {e}"))?;
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{CHECKPOINT_FILE}: {e}"))?;
    let time = doc["time"].as_u64().and_then(|t| u32::try_from(t).ok()).ok_or("checkpoint time missing")?;
    let files = doc["files"].as_object().ok_or("checkpoint file list missing")?;
    let wanted = checkpoint_files(case, time)?;
    let listed: Vec<&String> = files.keys().collect();
    if listed.len() != wanted.len() || listed.iter().zip(&wanted).any(|(a, b)| *a != b) {
        return Err("the checkpoint holds files other than those listed".into());
    }
    for (rel, digest) in files {
        let got = format!("{:016x}", fnv1a_file(&case.join(rel)).map_err(|e| format!("{rel}: {e}"))?);
        if digest.as_str() != Some(got.as_str()) {
            return Err(format!("{rel} changed since it was preserved"));
        }
    }
    Ok(time)
}
