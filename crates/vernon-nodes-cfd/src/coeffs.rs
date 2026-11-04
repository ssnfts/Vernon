//! Force-coefficient post-processing: reader for the solver's `.dat` histories,
//! window statistics, axle split, and the iteration-convergence gate.
//!
//! The reader discovers columns from the `#` header line (never by index) and
//! accepts the file-name variants different solver releases use.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Registry, Stage};

use crate::air::air_in;
use crate::types::{RefGeom, Series, ref_in};
use crate::util::{FnNode, fin, json_in};

// ------------------------------------------------------------------ .dat reader

/// One parsed `.dat` file: header constants plus numeric columns.
#[derive(Debug, Clone, Default)]
pub struct Dat {
    pub meta: IndexMap<String, f64>,
    /// Column names without the leading time column.
    pub names: Vec<String>,
    pub time: Vec<f64>,
    /// One vector per entry of `names`.
    pub cols: Vec<Vec<f64>>,
}

impl Dat {
    #[cfg(test)]
    pub fn col(&self, name: &str) -> Option<&[f64]> {
        self.names.iter().position(|n| n == name).map(|i| self.cols[i].as_slice())
    }
}

/// `(total_x` -> `total_x`, `total_z)` -> `total_z`, but `Cl(f)` stays whole.
fn clean_token(t: &str) -> String {
    let mut s = t.trim_start_matches('(').to_string();
    while s.ends_with(')') && !s.contains('(') {
        s.pop();
    }
    s
}

fn parse_meta_line(body: &str, meta: &mut IndexMap<String, f64>) {
    let Some((key, val)) = body.split_once(':') else { return };
    let key = key.trim();
    if key.is_empty() || key.contains(char::is_whitespace) {
        return;
    }
    let vals: Vec<f64> = val.replace(['(', ')'], " ").split_whitespace().filter_map(|t| t.parse::<f64>().ok()).collect();
    match vals.as_slice() {
        [v] => {
            meta.insert(key.to_string(), *v);
        }
        [x, y, z] => {
            meta.insert(format!("{key}_x"), *x);
            meta.insert(format!("{key}_y"), *y);
            meta.insert(format!("{key}_z"), *z);
        }
        _ => {}
    }
}

/// Parse a solver history file. Rows whose width differs from the first data
/// row (a half-written last line of a running solver) are skipped; tokens that
/// are not numbers (solver names in the info file) become NaN.
pub fn parse_dat(text: &str) -> Dat {
    let mut meta = IndexMap::new();
    let mut header: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<f64>> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(body) = line.strip_prefix('#') {
            let body = body.trim();
            if body.contains(':') {
                parse_meta_line(body, &mut meta);
            } else if body.split_whitespace().next().is_some_and(|t| t.eq_ignore_ascii_case("time")) {
                header = body.split_whitespace().map(clean_token).filter(|t| !t.is_empty()).collect();
            }
            continue;
        }
        let vals: Vec<f64> = line
            .replace(['(', ')'], " ")
            .split_whitespace()
            .map(|t| t.parse::<f64>().unwrap_or(f64::NAN))
            .collect();
        if vals.first().is_some_and(|t| t.is_finite()) {
            if let Some(first) = rows.first() {
                if first.len() != vals.len() {
                    continue;
                }
            }
            rows.push(vals);
        }
    }
    let width = rows.first().map_or(0, Vec::len);
    let mut names: Vec<String> = header.into_iter().skip(1).collect();
    if names.len() + 1 != width {
        names = (1..width).map(|i| format!("c{i}")).collect();
    }
    let time = rows.iter().map(|r| r[0]).collect();
    let cols = (1..width).map(|j| rows.iter().map(|r| r[j]).collect()).collect();
    Dat { meta, names, time, cols }
}

/// Numeric sub-directories of `base`, sorted by time.
fn time_dirs(base: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(base) else { return vec![] };
    let mut v: Vec<(f64, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let t = e.file_name().to_str()?.parse::<f64>().ok()?;
            e.path().is_dir().then(|| (t, e.path()))
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().map(|x| x.1).collect()
}

/// Join restart segments: a later segment replaces every earlier row at or
/// after its first time.
pub fn merge_restarts(parts: Vec<Dat>) -> Option<Dat> {
    let mut it = parts.into_iter();
    let mut out = it.next()?;
    for next in it {
        let Some(&t0) = next.time.first() else { continue };
        if next.names != out.names {
            continue;
        }
        let keep = out.time.iter().position(|&t| t >= t0).unwrap_or(out.time.len());
        out.time.truncate(keep);
        for c in &mut out.cols {
            c.truncate(keep);
        }
        out.time.extend(next.time);
        for (c, n) in out.cols.iter_mut().zip(next.cols) {
            c.extend(n);
        }
        out.meta.extend(next.meta);
    }
    Some(out)
}

/// Load `postProcessing/<object>/<time>/<file>` over all restart directories.
fn load_object(case: &Path, objects: &[&str], files: &[&str]) -> Option<Dat> {
    for obj in objects {
        let parts: Vec<Dat> = time_dirs(&case.join("postProcessing").join(obj))
            .iter()
            .filter_map(|dir| files.iter().map(|f| dir.join(f)).find(|p| p.is_file()))
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .map(|t| parse_dat(&t))
            .filter(|d| !d.time.is_empty())
            .collect();
        if let Some(d) = merge_restarts(parts) {
            return Some(d);
        }
    }
    None
}

/// Named columns on one time axis.
struct Src {
    time: Vec<f64>,
    cols: Vec<(String, Vec<f64>)>,
    meta: IndexMap<String, f64>,
}

fn coeff_src(d: Dat) -> Src {
    let cols = d.names.iter().cloned().zip(d.cols).collect();
    Src {
        time: d.time,
        cols,
        meta: d.meta,
    }
}

/// Force or moment file: `total_x` -> `Fx`, `pressure_y` -> `Fy_pressure`...
fn vector_src(d: Dat, prefix: &str) -> Src {
    let mut cols = Vec::new();
    let mut named = false;
    for (name, col) in d.names.iter().zip(&d.cols) {
        if let Some((kind, axis)) = name.rsplit_once('_') {
            if matches!(axis, "x" | "y" | "z") && matches!(kind, "total" | "pressure" | "viscous" | "porous") {
                named = true;
                let key = if kind == "total" {
                    format!("{prefix}{axis}")
                } else {
                    format!("{prefix}{axis}_{kind}")
                };
                cols.push((key, col.clone()));
            }
        }
    }
    if !named && d.cols.len() >= 3 {
        // unknown header layout: the first three numbers are the totals
        for (axis, col) in ["x", "y", "z"].iter().zip(&d.cols) {
            cols.push((format!("{prefix}{axis}"), col.clone()));
        }
    }
    Src {
        time: d.time,
        cols,
        meta: d.meta,
    }
}

fn canonical_field(f: &str) -> &str {
    match f {
        "Ux" | "Uy" | "Uz" => "U",
        o => o,
    }
}

/// Initial residuals per field (`res_p`, `res_U` = max over components...).
fn residual_src(d: Dat) -> Src {
    let solver_info = d.names.iter().any(|n| n.ends_with("_initial"));
    let mut fields: IndexMap<String, Vec<f64>> = IndexMap::new();
    for (name, col) in d.names.iter().zip(&d.cols) {
        let field = if solver_info {
            match name.strip_suffix("_initial") {
                Some(f) => f,
                None => continue,
            }
        } else {
            name.as_str()
        };
        let key = format!("res_{}", canonical_field(field));
        match fields.get_mut(&key) {
            Some(existing) => {
                for (e, v) in existing.iter_mut().zip(col) {
                    *e = e.max(*v);
                }
            }
            None => {
                fields.insert(key, col.clone());
            }
        }
    }
    Src {
        time: d.time,
        cols: fields.into_iter().collect(),
        meta: IndexMap::new(),
    }
}

/// Read every available force, moment, coefficient and residual history of a
/// case into one series on their common time axis. An empty series (marked
/// `placeholder`) means the solver has not produced anything yet.
/// Mean pressure and total force along x on one monitored surface over the
/// last `window` rows of its history, in newtons. An absent, short or
/// malformed history is an error, never zero.
pub fn patch_force_x(case: &Path, patch: &str, window: usize) -> Result<(f64, f64), String> {
    let path = case.join("postProcessing").join(format!("forces_{patch}")).join("0").join("force.dat");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let header = text.lines().find(|l| l.contains("total_x")).ok_or("the force history has no named columns")?;
    let names: Vec<&str> = header.trim_start_matches('#').split_whitespace().collect();
    let col = |name: &str| {
        names
            .iter()
            .position(|n| *n == name)
            .ok_or_else(|| format!("the force history has no {name} column"))
    };
    let (total, pressure) = (col("total_x")?, col("pressure_x")?);
    let rows: Vec<Vec<f64>> = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split_whitespace().map(|t| t.parse::<f64>().map_err(|_| format!("bad number {t}"))).collect())
        .collect::<Result<_, _>>()?;
    if window == 0 || rows.len() < window {
        return Err(format!("the force history has {} rows, {window} requested", rows.len()));
    }
    let tail = &rows[rows.len() - window..];
    let mean = |c: usize| -> Result<f64, String> {
        let mut sum = 0.0;
        for r in tail {
            let v = *r.get(c).ok_or("a force row is short")?;
            if !v.is_finite() {
                return Err("a force value is not finite".into());
            }
            sum += v;
        }
        Ok(sum / tail.len() as f64)
    };
    Ok((mean(pressure)?, mean(total)?))
}

pub fn load_case_series(case: &Path) -> Series {
    let coeffs = load_object(case, &["forceCoeffs1", "forceCoeffs"], &["coefficient.dat", "forceCoeffs.dat"]);
    let force = load_object(case, &["forces1", "forces"], &["force.dat", "forces.dat"]);
    let moment = load_object(case, &["forces1", "forces"], &["moment.dat"]);
    let resid =
        load_object(case, &["solverInfo1", "solverInfo"], &["solverInfo.dat"]).or_else(|| load_object(case, &["residuals1", "residuals"], &["residuals.dat"]));
    let mut srcs: Vec<Src> = Vec::new();
    srcs.extend(coeffs.map(coeff_src));
    srcs.extend(force.map(|d| vector_src(d, "F")));
    srcs.extend(moment.map(|d| vector_src(d, "M")));
    srcs.extend(resid.map(residual_src));
    series_from_sources(srcs)
}

/// Fail-closed reader for a caller-verified fresh run. Unlike the legacy
/// reader, this requires coefficient and requested residual time coverage.
pub(crate) fn load_case_series_checked(case: &Path, required_residuals: &[&str]) -> Result<Series, String> {
    load_case_series_checked_with_limits(case, required_residuals, HistoryLimits::default())
}

#[derive(Clone, Copy)]
struct HistoryLimits {
    bytes: usize,
    line: usize,
    rows: usize,
    entries: usize,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            bytes: 64 * 1024 * 1024,
            line: 64 * 1024,
            rows: 100_000,
            entries: 4096,
        }
    }
}

#[derive(Default)]
struct HistoryBudget {
    limits: HistoryLimits,
    bytes: usize,
    rows: usize,
    entries: usize,
}

fn load_case_series_checked_with_limits(case: &Path, required: &[&str], limits: HistoryLimits) -> Result<Series, String> {
    let mut budget = HistoryBudget {
        limits,
        ..HistoryBudget::default()
    };
    check_cancel()?;
    let root = case.join("postProcessing");
    list_checked(&root, &mut budget)?;
    let coeffs = load_object_checked(case, &["forceCoeffs1", "forceCoeffs"], &["coefficient.dat", "forceCoeffs.dat"], &mut budget)?
        .filter(|(_, d)| !d.time.is_empty())
        .ok_or_else(|| format!("no coefficient history with data under {}", root.display()))?;
    let (coeff_path, coeffs) = coeffs;
    for name in ["Cd", "Cl"] {
        if !coeffs.names.iter().any(|n| n == name) {
            return Err(format!("{}: coefficient history has no named {name} column", coeff_path.display()));
        }
    }
    let axis = coeffs.time.clone();

    let monitor =
        load_object_checked(case, &["solverInfo1", "solverInfo"], &["solverInfo.dat"], &mut budget)?.and_then(|(_, d)| monitor_residuals(d, required, &axis));
    let residuals = match monitor {
        Some(src) => src,
        None => {
            let path = case.join("logs").join("solver.log");
            let src = read_residual_log(&path, required, &mut budget)?;
            if !covers(&src.time, &axis) {
                return Err(format!("{}: residual history lacks coverage of the coefficient time axis", path.display()));
            }
            src
        }
    };

    let mut srcs = vec![on_axis(coeff_src(coeffs), &axis), on_axis(residuals, &axis)];
    // Optional histories may run at another cadence: one that does not cover
    // the certified axis is omitted rather than allowed to shrink it.
    for (files, prefix) in [(&["force.dat", "forces.dat"][..], "F"), (&["moment.dat"][..], "M")] {
        if let Some((_, d)) = load_object_checked(case, &["forces1", "forces"], files, &mut budget)? {
            if covers(&d.time, &axis) {
                srcs.push(on_axis(vector_src(d, prefix), &axis));
            }
        }
    }
    Ok(series_from_sources(srcs))
}

fn check_cancel() -> Result<(), String> {
    if vernon_core::process::cancel_requested() {
        return Err("history read cancelled".into());
    }
    Ok(())
}

fn covers(time: &[f64], axis: &[f64]) -> bool {
    let have: HashSet<u64> = time.iter().map(|t| t.to_bits()).collect();
    axis.iter().all(|t| have.contains(&t.to_bits()))
}

/// Restrict a source that covers `axis` to exactly those rows.
fn on_axis(src: Src, axis: &[f64]) -> Src {
    let index: HashMap<u64, usize> = src.time.iter().enumerate().map(|(i, t)| (t.to_bits(), i)).collect();
    let rows: Vec<usize> = axis.iter().map(|t| index[&t.to_bits()]).collect();
    Src {
        time: axis.to_vec(),
        cols: src.cols.into_iter().map(|(n, c)| (n, rows.iter().map(|&i| c[i]).collect())).collect(),
        meta: src.meta,
    }
}

/// Link-free metadata; ordinary absence is `None`, every other failure an error.
fn meta_checked(path: &Path) -> Result<Option<std::fs::Metadata>, String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    #[cfg(windows)]
    let reparse = std::os::windows::fs::MetadataExt::file_attributes(&meta) & 0x400 != 0;
    #[cfg(not(windows))]
    let reparse = false;
    if meta.file_type().is_symlink() || reparse {
        return Err(format!("{}: selected history path is a link", path.display()));
    }
    Ok(Some(meta))
}

/// Entries of a directory that must be a real one; absent means empty.
fn list_checked(dir: &Path, budget: &mut HistoryBudget) -> Result<Vec<PathBuf>, String> {
    let Some(meta) = meta_checked(dir)? else { return Ok(vec![]) };
    if !meta.is_dir() {
        return Err(format!("{}: expected a directory", dir.display()));
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        check_cancel()?;
        budget.entries += 1;
        if budget.entries > budget.limits.entries {
            return Err(format!("{}: directory entry limit of {} exceeded", dir.display(), budget.limits.entries));
        }
        out.push(entry.map_err(|e| format!("{}: {e}", dir.display()))?.path());
    }
    Ok(out)
}

/// Bounded line stream: enforces the byte, line and UTF-8 rules, and polls for
/// cancellation once per line.
fn for_each_line<R: std::io::BufRead>(
    mut reader: R,
    path: &Path,
    budget: &mut HistoryBudget,
    mut visit: impl FnMut(usize, &str, &mut HistoryBudget) -> Result<(), String>,
) -> Result<(), String> {
    let mut number = 0;
    let mut line = Vec::new();
    loop {
        check_cancel()?;
        line.clear();
        let mut ended = false;
        while !ended {
            let chunk = reader.fill_buf().map_err(|e| format!("{}: read failed: {e}", path.display()))?;
            if chunk.is_empty() {
                break;
            }
            let take = chunk.iter().position(|&b| b == b'\n').map_or(chunk.len(), |i| {
                ended = true;
                i + 1
            });
            budget.bytes += take;
            if budget.bytes > budget.limits.bytes {
                return Err(format!("{}: history byte limit of {} exceeded", path.display(), budget.limits.bytes));
            }
            if line.len() + take > budget.limits.line {
                return Err(format!("{}: line {} exceeds the {}-byte limit", path.display(), number + 1, budget.limits.line));
            }
            line.extend_from_slice(&chunk[..take]);
            reader.consume(take);
        }
        if line.is_empty() {
            return Ok(());
        }
        number += 1;
        let text = std::str::from_utf8(&line).map_err(|_| format!("{}: line {number} is not valid UTF-8", path.display()))?;
        visit(number, text.trim(), budget)?;
    }
}

fn count_row(path: &Path, number: usize, budget: &mut HistoryBudget) -> Result<(), String> {
    budget.rows += 1;
    if budget.rows > budget.limits.rows {
        return Err(format!("{}: line {number}: row limit of {} exceeded", path.display(), budget.limits.rows));
    }
    Ok(())
}

fn open_checked(path: &Path) -> Result<Option<std::io::BufReader<std::fs::File>>, String> {
    let Some(meta) = meta_checked(path)? else { return Ok(None) };
    if !meta.is_file() {
        return Err(format!("{}: selected history is not a regular file", path.display()));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(std::io::BufReader::new(file)))
}

fn strictly_after(path: &Path, number: usize, last: Option<f64>, time: f64) -> Result<(), String> {
    if !time.is_finite() || last.is_some_and(|l| time <= l) {
        return Err(format!("{}: line {number}: time {time} is not finite and strictly increasing", path.display()));
    }
    Ok(())
}

/// Strict `.dat` subset: a named Time header, full-width rows, finite numbers.
/// Only solver-name and converged-flag columns may hold text.
fn read_dat_checked(path: &Path, budget: &mut HistoryBudget, out: &mut Dat, header_seen: &mut bool) -> Result<(), String> {
    let Some(reader) = open_checked(path)? else { return Ok(()) };
    budget.rows = 0;
    let mut own_header = false;
    for_each_line(reader, path, budget, |number, line, budget| {
        if line.is_empty() {
            return Ok(());
        }
        if let Some(body) = line.strip_prefix('#') {
            let body = body.trim();
            if body.contains(':') {
                parse_meta_line(body, &mut out.meta);
            } else if body.split_whitespace().next().is_some_and(|t| t.eq_ignore_ascii_case("time")) {
                let names: Vec<String> = body.split_whitespace().skip(1).map(clean_token).filter(|t| !t.is_empty()).collect();
                if *header_seen && names != out.names {
                    return Err(format!("{}: line {number}: columns differ from the earlier segment", path.display()));
                }
                out.cols.resize(names.len(), Vec::new());
                out.names = names;
                *header_seen = true;
                own_header = true;
            }
            return Ok(());
        }
        if !own_header {
            return Err(format!("{}: line {number}: data before a named Time header", path.display()));
        }
        count_row(path, number, budget)?;
        let spaced = line.replace(['(', ')'], " ");
        let tokens: Vec<&str> = spaced.split_whitespace().collect();
        if tokens.len() != out.names.len() + 1 {
            return Err(format!(
                "{}: line {number}: {} values for {} columns",
                path.display(),
                tokens.len(),
                out.names.len() + 1
            ));
        }
        let time = tokens[0].parse::<f64>().unwrap_or(f64::NAN);
        strictly_after(path, number, out.time.last().copied(), time)?;
        out.time.push(time);
        for ((name, col), token) in out.names.iter().zip(&mut out.cols).zip(&tokens[1..]) {
            if name.ends_with("_solver") || name.ends_with("_converged") {
                col.push(f64::NAN);
                continue;
            }
            match token.parse::<f64>() {
                Ok(v) if v.is_finite() => col.push(v),
                _ => return Err(format!("{}: line {number}: invalid value {token:?} for {name}", path.display())),
            }
        }
        Ok(())
    })?;
    if let Some((key, _)) = out.meta.iter().find(|(_, v)| !v.is_finite()) {
        return Err(format!("{}: header constant {key} is not finite", path.display()));
    }
    Ok(())
}

/// First object holding one of `files`, joined over its time directories.
/// Segments of a fresh run may not overlap; restart handling stays legacy.
fn load_object_checked(case: &Path, objects: &[&str], files: &[&str], budget: &mut HistoryBudget) -> Result<Option<(PathBuf, Dat)>, String> {
    let root = case.join("postProcessing");
    for object in objects {
        let mut dirs: Vec<(f64, PathBuf)> = Vec::new();
        for path in list_checked(&root.join(object), budget)? {
            let Some(time) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<f64>().ok()) else {
                continue;
            };
            if time.is_finite() && meta_checked(&path)?.is_some_and(|m| m.is_dir()) {
                dirs.push((time, path));
            }
        }
        dirs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut dat = Dat::default();
        let mut header = false;
        let mut first = None;
        for (_, dir) in &dirs {
            for file in files {
                let path = dir.join(file);
                if meta_checked(&path)?.is_some() {
                    read_dat_checked(&path, budget, &mut dat, &mut header)?;
                    first.get_or_insert(path);
                    break;
                }
            }
        }
        if let Some(path) = first {
            return Ok(Some((path, dat)));
        }
    }
    Ok(None)
}

/// Residuals from a monitor, or `None` when it lacks a required field or time.
fn monitor_residuals(d: Dat, required: &[&str], axis: &[f64]) -> Option<Src> {
    let has = |n: &str| d.names.iter().any(|c| c == &format!("{n}_initial"));
    let complete = required.iter().all(|&r| has(r) || (r == "U" && has("Ux") && has("Uy") && has("Uz")));
    if !complete || !covers(&d.time, axis) {
        return None;
    }
    let src = residual_src(d);
    let wanted: Vec<String> = required.iter().map(|r| format!("res_{r}")).collect();
    Some(Src {
        cols: src.cols.into_iter().filter(|(n, _)| wanted.contains(n)).collect(),
        ..src
    })
}

fn read_residual_log(path: &Path, required: &[&str], budget: &mut HistoryBudget) -> Result<Src, String> {
    let Some(reader) = open_checked(path)? else {
        return Err(format!("{}: required solver log is missing", path.display()));
    };
    residual_log_reader(reader, path, required, budget)
}

/// `<solver>:  Solving for F, Initial residual = a, Final residual = b, No Iterations n`
fn parse_solve(line: &str) -> Option<(&str, f64)> {
    let (solver, rest) = line.split_once("Solving for ")?;
    if !solver.trim_end().ends_with(':') {
        return None;
    }
    let parts: Vec<&str> = rest.split(',').map(str::trim).collect();
    let [field, initial, last, iterations] = parts.as_slice() else { return None };
    let residual = |text: &str, key: &str| {
        let value = text.strip_prefix(key)?.trim().strip_prefix('=')?.trim().parse::<f64>().ok()?;
        (value.is_finite() && value >= 0.0).then_some(value)
    };
    let initial = residual(initial, "Initial residual")?;
    residual(last, "Final residual")?;
    iterations.strip_prefix("No Iterations")?.trim().parse::<u64>().ok()?;
    (!field.is_empty()).then_some((*field, initial))
}

/// Worst initial residual of every required field in every Time block. A
/// missing field in any block, including the last, fails the whole log.
fn residual_log_reader<R: std::io::BufRead>(reader: R, path: &Path, required: &[&str], budget: &mut HistoryBudget) -> Result<Src, String> {
    #[derive(Default)]
    struct Block {
        line: usize,
        worst: HashMap<String, f64>,
    }
    let finish = |block: &Block, cols: &mut Vec<(String, Vec<f64>)>| -> Result<(), String> {
        for (&field, (_, col)) in required.iter().zip(cols.iter_mut()) {
            let parts: &[&str] = if field == "U" && !block.worst.contains_key("U") {
                &["Ux", "Uy", "Uz"]
            } else {
                std::slice::from_ref(&field)
            };
            let mut worst = 0.0f64;
            for part in parts {
                let value = block
                    .worst
                    .get(*part)
                    .ok_or_else(|| format!("{}: line {}: iteration has no {part} residual", path.display(), block.line))?;
                worst = worst.max(*value);
            }
            col.push(worst);
        }
        Ok(())
    };
    let mut time: Vec<f64> = Vec::new();
    let mut cols: Vec<(String, Vec<f64>)> = required.iter().map(|r| (format!("res_{r}"), Vec::new())).collect();
    let mut block: Option<Block> = None;
    budget.rows = 0;
    for_each_line(reader, path, budget, |number, line, budget| {
        if let Some(value) = line.strip_prefix("Time =") {
            if let Some(done) = &block {
                finish(done, &mut cols)?;
            }
            count_row(path, number, budget)?;
            let t = value.trim().parse::<f64>().unwrap_or(f64::NAN);
            strictly_after(path, number, time.last().copied(), t)?;
            time.push(t);
            block = Some(Block {
                line: number,
                worst: HashMap::new(),
            });
        } else if line.contains("Solving for ") {
            let (field, initial) = parse_solve(line).ok_or_else(|| format!("{}: line {number}: malformed solver report", path.display()))?;
            let current = block
                .as_mut()
                .ok_or_else(|| format!("{}: line {number}: solver report before the first Time", path.display()))?;
            let worst = current.worst.entry(field.to_string()).or_insert(initial);
            *worst = worst.max(initial);
        }
        Ok(())
    })?;
    match &block {
        Some(done) => finish(done, &mut cols)?,
        None => return Err(format!("{}: solver log has no Time iterations", path.display())),
    }
    Ok(Src {
        time,
        cols,
        meta: IndexMap::new(),
    })
}

fn series_from_sources(mut srcs: Vec<Src>) -> Series {
    srcs.retain(|s| !s.time.is_empty());
    if srcs.is_empty() {
        return Series {
            placeholder: true,
            ..Series::default()
        };
    }
    let index: Vec<HashMap<u64, usize>> = srcs
        .iter()
        .map(|s| s.time.iter().enumerate().map(|(i, t)| (t.to_bits(), i)).collect())
        .collect();
    let mut common: Vec<f64> = srcs[0].time.clone();
    for idx in &index[1..] {
        common.retain(|t| idx.contains_key(&t.to_bits()));
    }
    if common.is_empty() {
        // sources on disjoint time axes: keep the first one alone
        srcs.truncate(1);
        common = srcs[0].time.clone();
    }
    let mut seen: HashSet<u64> = HashSet::new();
    common.retain(|t| seen.insert(t.to_bits()));
    let mut out = Series {
        time: common.clone(),
        ..Series::default()
    };
    for (src, idx) in srcs.iter().zip(&index) {
        out.meta.extend(src.meta.iter().map(|(k, v)| (k.clone(), *v)));
        for (name, col) in &src.cols {
            let vals: Vec<f64> = common.iter().filter_map(|t| idx.get(&t.to_bits()).map(|&i| col[i])).collect();
            if vals.len() == common.len() && vals.iter().all(|v| v.is_finite()) {
                out.columns.entry(name.clone()).or_insert(vals);
            }
        }
    }
    out
}

// ------------------------------------------------------------------ statistics and physics

/// Statistics over an averaging window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    pub n: usize,
    pub mean: f64,
    /// Sample standard deviation.
    pub sd: f64,
    /// Linear drift over the window as a percentage of |mean| (slope x n / |mean|).
    pub drift_pct: f64,
    pub p05: f64,
    pub p95: f64,
}

// Nearest-rank percentile, the shared convention of `vernon_core::stats`.
use vernon_core::stats::nearest_rank as percentile;

/// Mean, standard deviation, drift and the 5-95% band. `None` for empty or
/// non-finite input.
pub fn window_stats(v: &[f64]) -> Option<Stats> {
    let n = v.len();
    if n == 0 || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let nf = n as f64;
    let mean = v.iter().sum::<f64>() / nf;
    let sd = vernon_core::stats::sample_std(v);
    let centre = (nf - 1.0) / 2.0;
    let (mut sxy, mut sxx) = (0.0, 0.0);
    for (i, x) in v.iter().enumerate() {
        let di = i as f64 - centre;
        sxy += di * (x - mean);
        sxx += di * di;
    }
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let drift_pct = if mean.abs() > 0.0 { slope * nf / mean.abs() * 100.0 } else { 0.0 };
    let mut sorted = v.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(Stats {
        n,
        mean,
        sd,
        drift_pct,
        p05: percentile(&sorted, 0.05),
        p95: percentile(&sorted, 0.95),
    })
}

/// Coefficient from a force: `F / (q Aref)`.
pub fn coefficient_from_force(force_n: f64, q_pa: f64, aref_m2: f64) -> f64 {
    force_n / (q_pa * aref_m2)
}

/// Split the vertical force over the axles from the pitch moment about
/// `x_c` of the VERTICAL forces alone: `Ff + Fr = Fz` and
/// `Ff (xf - xc) + Fr (xr - xc) = -My` (`My = -sum (x_i - xc) Fz_i`).
/// A solver moment also holds the drag term; remove it first with
/// [`vertical_pitch_moment`]. `None` when the axles coincide.
pub fn axle_split(fz: f64, my: f64, x_c: f64, x_f: f64, x_r: f64) -> Option<(f64, f64)> {
    let det = x_f - x_r;
    if det.abs() < 1e-9 {
        return None;
    }
    let ff = (-my - fz * (x_r - x_c)) / det;
    Some((ff, fz - ff))
}

/// Pitch moment of the vertical forces alone. The solver's `My` about the
/// centre of rotation `(xc, ., zc)` is `sum (z_i - zc) Fx_i - sum (x_i - xc) Fz_i`
/// (the y part of r x F); with the drag acting along a line at height
/// `z_drag` the first sum is `(z_drag - zc) Fx`.
pub fn vertical_pitch_moment(my: f64, fx: f64, z_drag: f64, z_c: f64) -> f64 {
    my - (z_drag - z_c) * fx
}

/// Front share of the vertical load in percent; `None` when the axle forces
/// have opposite signs (some downforce, some lift) or the total is zero.
pub fn balance_front_pct(ff: f64, fr: f64) -> Option<f64> {
    let total = ff + fr;
    (ff * fr >= 0.0 && total.abs() > 1e-9).then(|| ff / total * 100.0)
}

/// Solve provenance and strict convergence diagnostics bound to the exact
/// source and averaging window used for a force result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForceQualification {
    pub version: u32,
    pub solve: crate::types::SolveEvidence,
    /// Exactly the column used for the coefficient source: Fx or Cd.
    pub column: String,
    pub window: usize,
    pub samples: usize,
    pub convergence: Convergence,
}

/// Force-and-moment result of one run: the hand-off to convergence, mesh
/// study, A/B comparison and site loads. Coefficients refer to the FULL car.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ForceResult {
    /// True when solver data is absent or unusable. Invalid measured values
    /// may remain for diagnostics; consumers must not use a flagged result.
    pub placeholder: bool,
    /// `forces` (derived from force/moment histories) or `coefficients`.
    pub source: String,
    pub u_ms: f64,
    pub rho: f64,
    pub q_pa: f64,
    pub a_front_m2: f64,
    /// Reference area the coefficients refer to (half of the full area for a half model).
    pub aref_m2: f64,
    pub half_model: bool,
    pub cd: f64,
    pub cd_sd: f64,
    pub cl: f64,
    pub cl_sd: f64,
    pub cda: f64,
    pub cda_sd: f64,
    pub cla: f64,
    pub cla_sd: f64,
    pub cd_drift_pct: f64,
    pub cd_p05: f64,
    pub cd_p95: f64,
    /// Full-car axle vertical forces, N (negative = downforce).
    pub front_force_n: Option<f64>,
    pub rear_force_n: Option<f64>,
    pub balance_front_pct: Option<f64>,
    pub balance_sd_pct: Option<f64>,
    pub window: usize,
    pub samples: usize,
    pub last_iter: f64,
    /// Mesh, ground/wheel, yaw and turbulence settings of the run, when known.
    pub recipe: Option<crate::types::Recipe>,
    /// Grid-convergence index of this (fine-grid) result in percent, set by
    /// the mesh study.
    pub mesh_gci_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualification: Option<ForceQualification>,
}

impl ForceResult {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_json(v: &Value) -> CookResult<Self> {
        serde_json::from_value(v.clone()).map_err(|e| CookError::Other(format!("not a force result: {e}")))
    }

    /// Value of a named quantity (`cd`, `cl`, `cda`, `cla`) and its iteration spread.
    pub fn quantity(&self, name: &str) -> Option<(f64, f64)> {
        match name {
            "cd" => Some((self.cd, self.cd_sd)),
            "cl" => Some((self.cl, self.cl_sd)),
            "cda" => Some((self.cda, self.cda_sd)),
            "cla" => Some((self.cla, self.cla_sd)),
            _ => None,
        }
    }
}

fn usable_drag(r: &ForceResult) -> bool {
    !r.placeholder && r.cd.is_finite() && r.cd > 0.0 && r.cda.is_finite() && r.cda > 0.0
}

/// A force result input at `idx`; an absent input is a clean error.
pub(crate) fn force_in(inputs: &[Option<Data>], idx: usize, name: &str) -> CookResult<ForceResult> {
    match json_in(inputs, idx)? {
        Some(v) => ForceResult::from_json(&v),
        None => Err(CookError::MissingInput { index: idx, name: name.into() }),
    }
}

/// Settings for [`compute_forces`].
pub struct ForceSpec<'a> {
    pub rf: &'a RefGeom,
    pub half_model: bool,
    pub window: usize,
    pub rho: f64,
    /// Speed from a wired air state, preferred over everything else.
    pub u_wire: Option<f64>,
    pub u_param: f64,
    /// Centre of rotation override for the axle split.
    pub cofr_x: Option<f64>,
    /// Height (solver frame, m) of the drag's line of action, whose pitch
    /// moment is removed before the axle split. `None`: mid-height of the
    /// vehicle reference (lowest point + height / 2).
    pub drag_z: Option<f64>,
}

/// Result plus the notes the node should show.
pub struct Computed {
    pub result: ForceResult,
    pub notes: Vec<(Severity, String)>,
    /// |file Aref / expected Aref - 1| in percent.
    pub aref_mismatch_pct: f64,
    /// Difference from the solver's own drag coefficient in percent, when a
    /// force history also holds one. A wrong density, speed or half-model
    /// setting shows up here.
    pub solver_cd_mismatch_pct: Option<f64>,
}

fn tail(v: &[f64], w: usize) -> &[f64] {
    &v[v.len() - w..]
}

/// Cd, Cl, CdA, ClA and axle balance over the last `window` iterations.
pub fn compute_forces(series: &Series, s: &ForceSpec) -> Result<Computed, String> {
    let n = series.len();
    if n == 0 || series.placeholder {
        return Err("no solver history".into());
    }
    let mut notes = Vec::new();
    let w = s.window.min(n).max(1);
    if w < s.window {
        notes.push((
            Severity::Warning,
            format!("history has {n} samples, shorter than the {} sample window; averaging all of it", s.window),
        ));
    }
    let meta_u = series.meta.get("magUInf").copied().filter(|v| *v > 0.0);
    let u = s.u_wire.or(meta_u).unwrap_or(s.u_param);
    if let (Some(a), Some(b)) = (s.u_wire, meta_u) {
        if (a / b - 1.0).abs() > 0.01 {
            notes.push((
                Severity::Warning,
                format!("wired speed {a:.2} m/s differs from the speed in the solver history ({b:.2} m/s)"),
            ));
        }
    }
    let a_full = s.rf.a_front_m2;
    let aref = s.rf.aref_m2(s.half_model);
    if !(a_full > 0.0 && u > 0.0 && s.rho > 0.0) {
        return Err("reference area, speed and density must be positive".into());
    }
    let q = 0.5 * s.rho * u * u;
    let scale = if s.half_model { 2.0 } else { 1.0 };
    let mut aref_mismatch_pct = 0.0;
    let (source, cd_i, cl_i): (&str, Vec<f64>, Vec<f64>) = if let (Some(fx), Some(fz)) = (series.col("Fx"), series.col("Fz")) {
        if let Some(&a_file) = series.meta.get("Aref").filter(|a| **a > 0.0) {
            aref_mismatch_pct = (a_file / aref - 1.0).abs() * 100.0;
            if aref_mismatch_pct > 1.0 {
                notes.push((
                    Severity::Error,
                    format!("reference area in the solver history ({a_file:.4} m2) differs from the expected {aref:.4} m2 (half model: A_front / 2); check the half-model setting and the wired reference"),
                ));
            }
        }
        (
            "forces",
            fx.iter().map(|f| coefficient_from_force(scale * f, q, a_full)).collect(),
            fz.iter().map(|f| coefficient_from_force(scale * f, q, a_full)).collect(),
        )
    } else if let (Some(cd), Some(cl)) = (series.col("Cd"), series.col("Cl")) {
        let mut ratio = 1.0;
        match series.meta.get("Aref") {
            Some(&a_file) if a_file > 0.0 => {
                ratio = a_file / aref;
                aref_mismatch_pct = (ratio - 1.0).abs() * 100.0;
                if aref_mismatch_pct > 1.0 {
                    notes.push((
                        Severity::Error,
                        format!(
                            "reference area in the solver history ({a_file:.4} m2) differs from the expected {aref:.4} m2 (half model: A_front / 2); coefficients were rescaled"
                        ),
                    ));
                } else {
                    ratio = 1.0;
                }
            }
            _ => notes.push((
                Severity::Warning,
                "reference area not recorded in the solver history; assuming it matches the vehicle reference".into(),
            )),
        }
        ("coefficients", cd.iter().map(|c| c * ratio).collect(), cl.iter().map(|c| c * ratio).collect())
    } else {
        return Err("history has neither Fx/Fz nor Cd/Cl columns".into());
    };

    let cd = window_stats(tail(&cd_i, w)).ok_or("non-finite drag history")?;
    let cl = window_stats(tail(&cl_i, w)).ok_or("non-finite lift history")?;
    let solver_cd_mismatch_pct = (source == "forces")
        .then(|| series.col("Cd").and_then(|c| window_stats(tail(c, w))))
        .flatten()
        .filter(|solver| solver.mean != 0.0)
        .map(|solver| (cd.mean / solver.mean - 1.0).abs() * 100.0);
    if let Some(m) = solver_cd_mismatch_pct.filter(|m| *m > 1.0) {
        notes.push((
            Severity::Error,
            format!("drag from the forces differs by {m:.1} % from the solver's own coefficient: the density, speed, reference area or half-model setting here is not the run's"),
        ));
    }
    let mut res = ForceResult {
        placeholder: false,
        source: source.into(),
        u_ms: u,
        rho: s.rho,
        q_pa: q,
        a_front_m2: a_full,
        aref_m2: aref,
        half_model: s.half_model,
        cd: cd.mean,
        cd_sd: cd.sd,
        cl: cl.mean,
        cl_sd: cl.sd,
        cda: cd.mean * a_full,
        cda_sd: cd.sd * a_full,
        cla: cl.mean * a_full,
        cla_sd: cl.sd * a_full,
        cd_drift_pct: cd.drift_pct,
        cd_p05: cd.p05,
        cd_p95: cd.p95,
        window: w,
        samples: n,
        last_iter: series.time[n - 1],
        ..ForceResult::default()
    };
    if !usable_drag(&res) {
        res.placeholder = true;
        notes.push((
            Severity::Error,
            "drag coefficient and drag area must be finite and positive; these values are not a usable aero result".into(),
        ));
    }

    // axle split
    let x_recorded = series.meta.get("CofR_x").copied().unwrap_or_else(|| s.rf.mid_wheelbase_x());
    let x_c = s.cofr_x.unwrap_or(x_recorded);
    let z_c = series.meta.get("CofR_z").copied().unwrap_or(0.0);
    let z_drag = s.drag_z.or_else(|| (s.rf.height_m > 0.0).then_some(s.rf.z_min_m + 0.5 * s.rf.height_m));
    if let (Some(fz), Some(my), Some(fx)) = (series.col("Fz"), series.col("My"), series.col("Fx")) {
        match z_drag {
            Some(z) => notes.push((
                Severity::Info,
                format!("drag pitch moment removed with the drag acting at z = {z:.3} m (centre of rotation z = {z_c:.3} m); the balance is the split of the vertical aero force"),
            )),
            None => notes.push((Severity::Warning, "vehicle height unknown: the axle split still holds the drag's pitch moment".into())),
        }
        let (mut ff_v, mut fr_v, mut bal_v) = (Vec::new(), Vec::new(), Vec::new());
        for ((fz_i, my_i), fx_i) in tail(fz, w).iter().zip(tail(my, w)).zip(tail(fx, w)) {
            // From My = (z-z_c) Fx - (x-x_c) Fz: moving the x origin
            // transports the recorded moment by +(x_new-x_old) Fz.
            let my_at_origin = my_i + (x_c - x_recorded) * fz_i;
            let my_v = z_drag.map_or(my_at_origin, |z| vertical_pitch_moment(my_at_origin, *fx_i, z, z_c));
            if let Some((ff, fr)) = axle_split(scale * fz_i, scale * my_v, x_c, s.rf.x_front_axle_m, s.rf.x_rear_axle_m) {
                ff_v.push(ff);
                fr_v.push(fr);
                if let Some(b) = balance_front_pct(ff, fr) {
                    bal_v.push(b);
                }
            }
        }
        if let (Some(ff), Some(fr)) = (window_stats(&ff_v), window_stats(&fr_v)) {
            res.front_force_n = Some(ff.mean);
            res.rear_force_n = Some(fr.mean);
            match balance_front_pct(ff.mean, fr.mean) {
                Some(b) => {
                    res.balance_front_pct = Some(b);
                    res.balance_sd_pct = window_stats(&bal_v).map(|st| st.sd);
                }
                None => notes.push((
                    Severity::Info,
                    "front and rear vertical forces have opposite signs; balance is not defined, axle forces reported".into(),
                )),
            }
        }
    } else if let (Some(f), Some(r)) = (series.col("Cl(f)"), series.col("Cl(r)")) {
        if let (Some(fs), Some(rs)) = (window_stats(tail(f, w)), window_stats(tail(r, w))) {
            res.balance_front_pct = balance_front_pct(fs.mean, rs.mean);
            notes.push((
                Severity::Info,
                "axle split taken from the solver's front/rear lift coefficients (these hold the drag's pitch moment about the centre of rotation)".into(),
            ));
        }
    }
    let mut computed = Computed {
        result: res,
        notes,
        aref_mismatch_pct,
        solver_cd_mismatch_pct,
    };
    match derive_force_qualification(series, &computed) {
        Ok(qualification) => computed.result.qualification = qualification,
        Err(why) => computed.notes.push((Severity::Warning, format!("force qualification unavailable: {why}"))),
    }
    Ok(computed)
}

fn derive_force_qualification(series: &Series, computed: &Computed) -> Result<Option<ForceQualification>, String> {
    let Some(solve) = &series.solve_evidence else { return Ok(None) };
    if series.time.iter().any(|t| !t.is_finite() || *t < 0.0) || series.time.windows(2).any(|pair| pair[1] <= pair[0]) {
        return Err("time history must be finite, nonnegative and strictly increasing for force qualification".into());
    }
    let mesh = &solve.mesh;
    let fingerprint = |s: &str| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit());
    if solve.version != 1 || mesh.version != 1 {
        return Err("unsupported solve or mesh evidence version".into());
    }
    if mesh.cells == 0 || !fingerprint(&solve.input_fingerprint) || !fingerprint(&mesh.mesh_fingerprint) {
        return Err("invalid solve or mesh evidence identity".into());
    }
    if !(0.0..70.0).contains(&mesh.max_nonortho) || !(0.0..4.0).contains(&mesh.max_skew) {
        return Err("mesh evidence quality is non-finite or outside the required limits".into());
    }
    for name in ["Aref", "magUInf"] {
        if !series.meta.get(name).is_some_and(|v| v.is_finite() && *v > 0.0) {
            return Err(format!("recorded normalization '{name}' must be finite and positive"));
        }
    }
    let result = &computed.result;
    let recorded_speed = series.meta["magUInf"];
    if !result.u_ms.is_finite() || (result.u_ms / recorded_speed - 1.0).abs() > 0.01 {
        return Err("speed used for force normalization does not match the recorded speed".into());
    }
    if !force_report(computed).passes() {
        return Ok(None);
    }
    let window = result.window;
    if window < 2 || result.samples / window < 2 {
        return Ok(None);
    }
    let column = match result.source.as_str() {
        "forces" => "Fx",
        "coefficients" => "Cd",
        _ => return Err("unrecognized coefficient source".into()),
    };
    let convergence = iteration_gate(
        series,
        column,
        &ConvSpec {
            window,
            tol_pct: 0.2,
            drift_limit_pct: 0.1,
            sd_limit_pct: 0.5,
            residual_target: 1e-5,
            require_residuals: true,
        },
    )?;
    if convergence.window != window || convergence.iterations != result.samples {
        return Err("convergence source or window does not match the coefficient result".into());
    }
    Ok(Some(ForceQualification {
        version: 1,
        solve: solve.clone(),
        column: column.into(),
        window,
        samples: result.samples,
        convergence,
    }))
}

fn placeholder_result(s: &ForceSpec) -> ForceResult {
    ForceResult {
        placeholder: true,
        source: "none".into(),
        u_ms: s.u_wire.unwrap_or(s.u_param),
        rho: s.rho,
        a_front_m2: s.rf.a_front_m2,
        aref_m2: s.rf.aref_m2(s.half_model),
        half_model: s.half_model,
        ..ForceResult::default()
    }
}

// ------------------------------------------------------------------ cfd.force_coeffs

fn force_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.force_coeffs", "Force Coefficients", Stage::Cfd)
        .input(PortSpec::new("history", DataKind::Json))
        .input(PortSpec::optional("reference", DataKind::Json))
        .input(PortSpec::optional("air", DataKind::Json))
        .output(PortSpec::new("result", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::int("window", "Averaging window", 1000, 100, 2000).unit("iter").help("Last N iterations averaged; the spread over this window is the iteration uncertainty"))
        .param(ParamSpec::bool("half_model", "Half-car forces", true).help("Forces were integrated over the y >= 0 half only; the reference area is then half the full frontal area"))
        .param(ParamSpec::float("rho_inf", "Density of the run", 1.225, 0.9, 1.3).unit("kg/m3").help("Overridden by a wired air state"))
        .param(ParamSpec::float("u_ms", "Speed of the run", 30.0, 10.0, 100.0).unit("m/s").help("Overridden by a wired air state; otherwise the speed recorded in the history is used"))
        .param(ParamSpec::bool("custom_cofr", "Custom centre of rotation", false).help("Off: the centre of rotation recorded by the solver, else mid-wheelbase"))
        .param(ParamSpec::float("cofr_x_m", "Centre of rotation x", 0.0, -10.0, 20.0).unit("m").help("Centre of rotation for the moment coefficients: x position in metres from the nose."))
        .param(ParamSpec::bool("custom_drag_height", "Custom drag height", false).help("Off: the drag acts at mid-height of the vehicle (lowest point + height / 2)"))
        .param(ParamSpec::float("drag_height_m", "Drag line height", 0.45, -1.0, 5.0).unit("m").help("Height of the drag's line of action in the solver frame; its pitch moment is removed before the axle split"))
        .help("Drag and lift coefficients, CdA and ClA, front/rear axle split and aero balance with their iteration spread, from the solver's force histories. Checks the reference area so a half-car run can never be normalised by the full area.")
        .emulates("automotive wind-tunnel service: drag and lift coefficients")
        .emulates("cloud CFD service: force post-processing")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn force_report(c: &Computed) -> Report {
    let r = &c.result;
    let mut rep = Report::new("Force coefficients");
    rep.note(Severity::Info, vernon_core::stats::CONVENTION);
    rep.metric("data_available", Metric::new(f64::from(usable_drag(r)), "-").with_limit(1.0, false))
        .metric("Cd", Metric::new(r.cd, "-"))
        .metric("Cl", Metric::new(r.cl, "-"))
        .metric("CdA", Metric::new(r.cda, "m2"))
        .metric("ClA", Metric::new(r.cla, "m2"))
        .metric("CdA_sd", Metric::new(r.cda_sd, "m2"))
        .metric("Cd_drift_pct", Metric::new(r.cd_drift_pct, "%"))
        .metric("aref_mismatch_pct", Metric::new(c.aref_mismatch_pct, "%").with_limit(1.0, true))
        .metric("window", Metric::new(r.window as f64, "iter"));
    if let Some(qualification) = &r.qualification {
        let qualified = qualification.convergence.converged;
        rep.metric(
            vernon_core::keys::CFD_ITERATION_QUALIFIED,
            Metric::new(f64::from(qualified), "-").with_limit(1.0, false),
        );
        if !qualified {
            rep.note(
                Severity::Error,
                "the force history has not passed the required iteration convergence checks; raw coefficients remain diagnostic values",
            );
        }
    }
    if let Some(m) = c.solver_cd_mismatch_pct {
        rep.metric("solver_cd_mismatch_pct", Metric::new(m, "%").with_limit(1.0, true));
    }
    if let Some(b) = r.balance_front_pct {
        rep.metric("balance_front_pct", Metric::new(b, "%"));
    }
    if let Some(f) = r.front_force_n {
        rep.metric("front_force", Metric::new(f, "N"));
    }
    if let Some(f) = r.rear_force_n {
        rep.metric("rear_force", Metric::new(f, "N"));
    }
    for (sev, text) in &c.notes {
        rep.note(*sev, text.clone());
    }
    rep.note(
        Severity::Info,
        "downforce is less trustworthy than drag in steady RANS; trust only A/B deltas on a validated recipe",
    );
    if c.result.recipe.as_ref().is_some_and(|r| r.separated_flow) {
        rep.metric("separated_flow_flagged", Metric::new(1.0, "-"));
        rep.note(
            Severity::Warning,
            "the case is flagged with separated-flow features (diffuser, rear wing near stall or tail separation): steady k-omega SST misses or smears that separation, so read these coefficients as a trend and confirm them with a transient detached-eddy (DDES or SBES) run",
        );
    }
    rep
}

fn force_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let series = match json_in(ins, 0)? {
        Some(v) => Series::from_json(&v)?,
        None => {
            return Err(CookError::MissingInput {
                index: 0,
                name: "history".into(),
            });
        }
    };
    let (rf, _) = ref_in(ins, 1)?;
    let air = air_in(ins, 2)?;
    let rho = air.rho.unwrap_or(p.float("rho_inf")?);
    let spec = ForceSpec {
        rf: &rf,
        half_model: p.bool("half_model")?,
        window: p.int("window")?.max(1) as usize,
        rho,
        u_wire: air.u,
        u_param: p.float("u_ms")?,
        cofr_x: p.bool("custom_cofr")?.then(|| p.float("cofr_x_m")).transpose()?,
        drag_z: p.bool("custom_drag_height")?.then(|| p.float("drag_height_m")).transpose()?,
    };
    match compute_forces(&series, &spec) {
        Ok(mut c) => {
            c.result.recipe = series.recipe.clone();
            fin(ctx, "CdA", c.result.cda)?;
            fin(ctx, "ClA", c.result.cla)?;
            Ok(vec![Data::json(c.result.to_json()), Data::report(force_report(&c))])
        }
        Err(e) if series.is_empty() || series.placeholder => {
            let mut rep = Report::new("Force coefficients");
            rep.metric("data_available", Metric::new(0.0, "-").with_limit(1.0, false));
            rep.note(
                Severity::Warning,
                format!("{e}: connect a solved case or a recorded force history; the numbers on this wire are not results"),
            );
            Ok(vec![Data::json(placeholder_result(&spec).to_json()), Data::report(rep)])
        }
        Err(e) => Err(ctx.fail(e)),
    }
}

// ------------------------------------------------------------------ cfd.run_convergence

/// Thresholds of the iteration gate.
#[derive(Debug, Clone)]
pub struct ConvSpec {
    pub window: usize,
    pub tol_pct: f64,
    pub drift_limit_pct: f64,
    pub sd_limit_pct: f64,
    pub residual_target: f64,
    pub require_residuals: bool,
}

/// Outcome of the iteration gate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Convergence {
    pub converged: bool,
    pub enough_iterations: bool,
    pub iterations: usize,
    pub window: usize,
    pub mean_prev: f64,
    pub mean_last: f64,
    pub window_diff_pct: f64,
    pub drift_pct: f64,
    pub sd_pct: f64,
    pub unsteady: bool,
    /// RMS initial residual over the last window, per field.
    pub residuals: IndexMap<String, f64>,
    pub residuals_ok: bool,
    pub residuals_checked: bool,
}

fn pick_column<'a>(series: &'a Series, column: &str) -> Result<&'a [f64], String> {
    let order: &[&str] = match column {
        "auto" => &["Cd", "Fx"],
        "Cd" => &["Cd"],
        "Cl" => &["Cl"],
        "Fx" => &["Fx"],
        "Fz" => &["Fz"],
        _ => &[],
    };
    order
        .iter()
        .find_map(|name| series.col(name))
        .ok_or_else(|| format!("history has no '{column}' column"))
}

/// Iteration-stationarity gate (spec 3.9): residuals below target, two
/// adjacent windows agree, drift small. Scatter only raises the `unsteady` flag.
pub fn iteration_gate(series: &Series, column: &str, c: &ConvSpec) -> Result<Convergence, String> {
    let col = pick_column(series, column)?;
    let n = col.len();
    if n != series.time.len() {
        return Err("monitored column does not cover the time history".into());
    }
    let w = c.window.max(2);
    let enough = n >= 2 * w;
    let eff = if enough { w } else { n / 2 };
    if eff < 2 {
        return Err("history too short to compare two windows".into());
    }
    let w2 = window_stats(&col[n - eff..]).ok_or("non-finite history")?;
    let w1 = window_stats(&col[n - 2 * eff..n - eff]).ok_or("non-finite history")?;
    let mean_ref = w2.mean.abs().max(f64::MIN_POSITIVE);
    let diff = (w2.mean - w1.mean).abs() / mean_ref * 100.0;
    let drift = w1.drift_pct.abs().max(w2.drift_pct.abs());
    let sd_pct = w2.sd / mean_ref * 100.0;
    let mut residuals = IndexMap::new();
    for (name, r) in &series.columns {
        if let Some(field) = name.strip_prefix("res_") {
            if r.len() != n {
                return Err(format!("residual column '{name}' has {} rows, expected {n}", r.len()));
            }
            if r.iter().any(|v| !v.is_finite() || *v < 0.0) {
                return Err(format!("residual column '{name}' contains a non-finite or negative value"));
            }
            let last = &r[n - eff..];
            let maximum = last.iter().copied().fold(0.0_f64, f64::max);
            let rms = if maximum == 0.0 {
                0.0
            } else {
                // Scaling first keeps a finite residual from overflowing
                // solely because RMS squares it.
                maximum * (last.iter().map(|x| (x / maximum).powi(2)).sum::<f64>() / last.len() as f64).sqrt()
            };
            residuals.insert(field.to_string(), rms);
        }
    }
    let residuals_checked = ["p", "U", "k", "omega"].iter().all(|field| residuals.contains_key(*field));
    let residuals_ok = (residuals_checked || !c.require_residuals) && residuals.values().all(|r| *r <= c.residual_target);
    let converged = enough && residuals_ok && diff <= c.tol_pct && drift <= c.drift_limit_pct;
    Ok(Convergence {
        converged,
        enough_iterations: enough,
        iterations: n,
        window: eff,
        mean_prev: w1.mean,
        mean_last: w2.mean,
        window_diff_pct: diff,
        drift_pct: drift,
        sd_pct,
        unsteady: sd_pct > c.sd_limit_pct,
        residuals,
        residuals_ok,
        residuals_checked,
    })
}

fn conv_info() -> NodeTypeInfo {
    NodeTypeInfo::new("cfd.run_convergence", "Run Convergence", Stage::Cfd)
        .input(PortSpec::new("history", DataKind::Json))
        .output(PortSpec::new("status", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::choice("column", "Monitored quantity", "auto", &["auto", "Cd", "Cl", "Fx", "Fz"]).help("auto: drag coefficient, else drag force"))
        .param(ParamSpec::int("window", "Window", 1000, 100, 2000).unit("iter").help("Number of final iterations the drift of each coefficient is measured over."))
        .param(ParamSpec::float("tol_pct", "Window-to-window change", 0.2, 0.01, 2.0).unit("%").help("Mean of the last window against the one before"))
        .param(ParamSpec::float("drift_limit_pct", "Drift per window", 0.1, 0.01, 2.0).unit("%").help("Largest acceptable drift of a coefficient over the window, in percent. A larger drift marks the run as not converged."))
        .param(ParamSpec::float("sd_limit_pct", "Scatter flag level", 0.5, 0.05, 5.0).unit("%").help("Above this the result is flagged unsteady: report the spread, not just the mean"))
        .param(ParamSpec::float("residual_target", "Residual target", 1e-5, 1e-6, 1e-3).help("RMS initial residual of every field over the last window"))
        .param(ParamSpec::bool("require_residuals", "Require residual history", true).help("Complete p, U, k and omega residual histories are required to certify convergence"))
        .help("Iteration stationarity gate: residuals below target, the last two averaging windows agree, drift is small. Large scatter flags the run as unsteady instead of failing it.")
        .emulates("automotive wind-tunnel service: convergence check")
        .spec("docs/specs/automotive-cfd.md")
        .implemented()
}

fn conv_cook(ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let series = match json_in(ins, 0)? {
        Some(v) => Series::from_json(&v)?,
        None => {
            return Err(CookError::MissingInput {
                index: 0,
                name: "history".into(),
            });
        }
    };
    let spec = ConvSpec {
        window: p.int("window")?.max(2) as usize,
        tol_pct: p.float("tol_pct")?,
        drift_limit_pct: p.float("drift_limit_pct")?,
        sd_limit_pct: p.float("sd_limit_pct")?,
        residual_target: p.float("residual_target")?,
        require_residuals: p.bool("require_residuals")?,
    };
    let mut rep = Report::new("Run convergence");
    if series.is_empty() || series.placeholder {
        rep.metric("converged", Metric::new(0.0, "-").with_limit(1.0, false));
        rep.note(Severity::Warning, "no solver history yet: nothing to certify");
        let st = Convergence::default();
        return Ok(vec![Data::json(serde_json::to_value(st).unwrap_or(Value::Null)), Data::report(rep)]);
    }
    let g = iteration_gate(&series, p.str("column")?, &spec).map_err(|e| ctx.fail(e))?;
    rep.metric("converged", Metric::new(f64::from(g.converged), "-").with_limit(1.0, false))
        .metric("enough_iterations", Metric::new(f64::from(g.enough_iterations), "-").with_limit(1.0, false))
        .metric("window_diff_pct", Metric::new(g.window_diff_pct, "%").with_limit(spec.tol_pct, true))
        .metric("drift_pct", Metric::new(g.drift_pct, "%").with_limit(spec.drift_limit_pct, true))
        .metric("sd_pct", Metric::new(g.sd_pct, "%"))
        .metric("iterations", Metric::new(g.iterations as f64, "iter"));
    for (field, r) in &g.residuals {
        rep.metric(format!("res_{field}"), Metric::new(*r, "-").with_limit(spec.residual_target, true));
    }
    if !g.enough_iterations {
        rep.note(
            Severity::Warning,
            format!("only {} iterations: fewer than two windows of {}", g.iterations, spec.window),
        );
    }
    if !g.residuals_checked {
        let sev = if spec.require_residuals { Severity::Error } else { Severity::Warning };
        rep.note(
            sev,
            "incomplete residual history: p, U, k and omega are required; run the case with the solver-information monitor",
        );
    }
    if g.unsteady {
        rep.note(Severity::Warning, "unsteady: scatter is above the flag level; report mean +/- spread");
    }
    let status = json!({
        "converged": g.converged, "unsteady": g.unsteady, "iterations": g.iterations, "window": g.window,
        "window_diff_pct": g.window_diff_pct, "drift_pct": g.drift_pct, "sd_pct": g.sd_pct,
        "mean_last": g.mean_last, "residuals_ok": g.residuals_ok,
    });
    Ok(vec![Data::json(status), Data::report(rep)])
}

pub(crate) fn register(reg: &mut Registry) {
    reg.add(FnNode::new(force_info, force_cook));
    reg.add(FnNode::new(conv_info, conv_cook));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::*;
    use vernon_graph::ParamValue;

    const COEFF: &str = "# Force coefficients\n# magUInf      : 30\n# lRef         : 4.6\n# Aref         : 1.08\n# CofR         : (2.313 0 0)\n# liftDir      : (0 0 1)\n#\n# Time          Cd              Cd(f)           Cd(r)           Cl              Cl(f)           Cl(r)\n1 0.31 0.15 0.16 -0.2 -0.1 -0.1\n2 0.32 0.16 0.16 -0.21 -0.11 -0.10\n3 0.32";

    #[test]
    fn parses_coefficient_header_by_name() {
        let d = parse_dat(COEFF);
        assert_eq!(d.names, ["Cd", "Cd(f)", "Cd(r)", "Cl", "Cl(f)", "Cl(r)"]);
        assert_eq!(d.time, [1.0, 2.0]);
        assert_eq!(d.col("Cl").unwrap(), [-0.2, -0.21]);
        assert_eq!(d.meta["Aref"], 1.08);
        assert_eq!(d.meta["CofR_x"], 2.313);
        assert_eq!(d.meta["magUInf"], 30.0);
    }

    #[test]
    fn parses_grouped_force_header() {
        let t = "# Force\n# CofR : (0 0 0)\n# Time  (total_x total_y total_z) (pressure_x pressure_y pressure_z)\n1 ((10 0 -5) (8 0 -6))\n2 ((11 0 -5.5) (9 0 -6.5))\n";
        let d = parse_dat(t);
        assert_eq!(d.names, ["total_x", "total_y", "total_z", "pressure_x", "pressure_y", "pressure_z"]);
        let s = vector_src(d, "F");
        let names: Vec<_> = s.cols.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["Fx", "Fy", "Fz", "Fx_pressure", "Fy_pressure", "Fz_pressure"]);
        assert_eq!(s.cols[2].1, [-5.0, -5.5]);
    }

    #[test]
    fn unknown_header_falls_back_to_first_three_columns() {
        let d = parse_dat("# Time forces(pressure viscous)\n1 10 0 -5 8 0 -6\n2 11 0 -6 9 0 -7\n");
        let s = vector_src(d, "M");
        assert_eq!(s.cols[0].0, "Mx");
        assert_eq!(s.cols[1].1, [0.0, 0.0]);
        assert_eq!(s.cols[2].1, [-5.0, -6.0]);
    }

    #[test]
    fn statistics_of_a_ramp() {
        let v: Vec<f64> = (0..100).map(|i| 1.0 + 0.001 * f64::from(i)).collect();
        let s = window_stats(&v).unwrap();
        assert!((s.mean - 1.0495).abs() < 1e-12);
        // slope 0.001 per sample over 100 samples = 0.1 on a 1.0495 mean
        assert!((s.drift_pct - 100.0 * 0.1 / 1.0495).abs() < 1e-9, "{}", s.drift_pct);
        assert!(s.p05 < s.mean && s.p95 > s.mean);
        assert!(window_stats(&[]).is_none());
        assert!(window_stats(&[1.0, f64::NAN]).is_none());
        assert_eq!(window_stats(&[2.0]).unwrap().sd, 0.0);
    }

    #[test]
    fn golden_axle_split_g16_g17() {
        let (ff, fr) = axle_split(-856.5, 193.7, 0.0, 1.35, -1.35).unwrap();
        assert!((ff + 500.0).abs() < 0.2 && (fr + 356.5).abs() < 0.2, "{ff} {fr}");
        let b = balance_front_pct(ff, fr).unwrap();
        assert!((b - 58.4).abs() < 0.1, "{b}");
        let (ff, fr) = axle_split(-856.5, 0.0, 0.0, 1.35, -1.35).unwrap();
        assert!((ff + 428.2).abs() < 0.1 && (fr + 428.2).abs() < 0.1);
        assert!(axle_split(-1.0, 0.0, 0.0, 1.0, 1.0).is_none());
        // Golden with drag: the toolchain example (rho 1.11, 200 km/h,
        // CdA 0.64 -> Fx = 1096 N) acting 0.45 m above a ground-level
        // centre of rotation adds 493 N m to the solver's pitch moment.
        let fx = 0.5 * 1.11 * (200.0f64 / 3.6).powi(2) * 0.64;
        assert!((fx - 1096.3).abs() < 0.2, "{fx}");
        let my_solver = 193.7 + 0.45 * fx;
        let (ff, fr) = axle_split(-856.5, vertical_pitch_moment(my_solver, fx, 0.45, 0.0), 0.0, 1.35, -1.35).unwrap();
        assert!((balance_front_pct(ff, fr).unwrap() - 58.4).abs() < 0.1);
        // Without the correction the balance reads about 80 % front.
        let (ff, fr) = axle_split(-856.5, my_solver, 0.0, 1.35, -1.35).unwrap();
        assert!(balance_front_pct(ff, fr).unwrap() > 75.0);
        // The same through compute_forces (full car, CofR z = 0).
        let mut rf = RefGeom::reference_car();
        rf.x_front_axle_m = 1.35;
        rf.x_rear_axle_m = -1.35;
        let mut s = Series {
            time: (1..=10).map(f64::from).collect(),
            ..Series::default()
        };
        s.columns.insert("Fx".into(), vec![fx; 10]);
        s.columns.insert("Fz".into(), vec![-856.5; 10]);
        s.columns.insert("My".into(), vec![my_solver; 10]);
        s.meta.insert("CofR_x".into(), 0.0);
        s.meta.insert("CofR_z".into(), 0.0);
        let spec = ForceSpec {
            rf: &rf,
            half_model: false,
            window: 10,
            rho: 1.11,
            u_wire: Some(200.0 / 3.6),
            u_param: 30.0,
            cofr_x: None,
            drag_z: Some(0.45),
        };
        let c = compute_forces(&s, &spec).unwrap().result;
        assert!((c.balance_front_pct.unwrap() - 58.4).abs() < 0.1, "{:?}", c.balance_front_pct);
        assert!((c.front_force_n.unwrap() + 500.0).abs() < 0.3);
        assert!(balance_front_pct(-100.0, 20.0).is_none());
    }

    #[test]
    fn golden_half_model_reference_area_g18() {
        // half-model force 548 N at q 1713 Pa, full frontal area 2.0 m2
        assert!((coefficient_from_force(548.0, 1713.0, 1.0) - 0.320).abs() < 0.001);
        assert!((coefficient_from_force(548.0, 1713.0, 2.0) - 0.160).abs() < 0.001);
        let mut rf = RefGeom::reference_car();
        rf.a_front_m2 = 2.0;
        // history normalised by the FULL area although only the half was integrated
        let mut s = Series {
            time: vec![1.0, 2.0, 3.0],
            ..Series::default()
        };
        s.columns.insert("Cd".into(), vec![0.160; 3]);
        s.columns.insert("Cl".into(), vec![-0.1; 3]);
        s.meta.insert("Aref".into(), 2.0);
        let spec = ForceSpec {
            rf: &rf,
            half_model: true,
            window: 100,
            rho: 1.225,
            u_wire: None,
            u_param: 30.0,
            cofr_x: None,
            drag_z: None,
        };
        let c = compute_forces(&s, &spec).unwrap();
        assert!((c.aref_mismatch_pct - 100.0).abs() < 1e-9);
        assert!(c.notes.iter().any(|(sev, _)| *sev == Severity::Error));
        assert!((c.result.cd - 0.320).abs() < 1e-9, "rescaled to the half-model area: {}", c.result.cd);
        assert!(!force_report(&c).passes(), "the wrong area must fail the gate");
        // correct area passes
        s.meta.insert("Aref".into(), 1.0);
        s.columns.insert("Cd".into(), vec![0.320; 3]);
        let c = compute_forces(&s, &spec).unwrap();
        assert!(force_report(&c).passes());
        assert!((c.result.cda - 0.64).abs() < 1e-9);
    }

    fn force_series(n: usize, fx: f64, fz: f64, my: f64) -> Series {
        let mut s = Series {
            time: (1..=n).map(|i| i as f64).collect(),
            ..Series::default()
        };
        s.columns
            .insert("Fx".into(), (0..n).map(|i| fx * (1.0 + 0.001 * ((i % 3) as f64 - 1.0))).collect());
        s.columns.insert("Fz".into(), vec![fz; n]);
        s.columns.insert("My".into(), vec![my; n]);
        s
    }

    #[test]
    fn forces_to_coefficients_and_balance() {
        let mut rf = RefGeom::reference_car();
        rf.x_front_axle_m = 1.35;
        rf.x_rear_axle_m = -1.35;
        rf.a_front_m2 = 2.0;
        // half-model forces; q = 1713 Pa
        // solver moment = vertical part + drag at 0.45 m about a ground-level CofR
        let mut s = force_series(300, 548.0, -428.25, 0.0);
        let my: Vec<f64> = s.columns["Fx"].iter().map(|fx| 96.85 + 0.45 * fx).collect();
        s.columns.insert("My".into(), my);
        s.meta.insert("CofR_x".into(), 0.0);
        s.meta.insert("CofR_z".into(), 0.0);
        let q = 0.5 * 1.11 * (200.0f64 / 3.6).powi(2);
        let spec = ForceSpec {
            rf: &rf,
            half_model: true,
            window: 100,
            rho: 1.11,
            u_wire: Some(200.0 / 3.6),
            u_param: 30.0,
            cofr_x: None,
            drag_z: Some(0.45),
        };
        let c = compute_forces(&s, &spec).unwrap().result;
        assert!((c.q_pa - q).abs() < 1e-9);
        assert!((c.cd - 2.0 * 548.0 / (q * 2.0)).abs() < 1e-3, "{}", c.cd);
        assert!((c.cda - 0.64).abs() < 0.002, "{}", c.cda);
        assert!((c.cla + 0.5).abs() < 0.002, "{}", c.cla);
        assert!(c.cd_sd > 0.0 && c.cda_sd > 0.0);
        assert!((c.balance_front_pct.unwrap() - 58.4).abs() < 0.1);
        assert!((c.front_force_n.unwrap() + 500.0).abs() < 0.3);
        assert_eq!(c.window, 100);
        assert_eq!(c.source, "forces");
        // the same history as a full-car run gives half the force coefficients
        let full = compute_forces(&s, &ForceSpec { half_model: false, ..spec }).unwrap().result;
        assert!((full.cd * 2.0 - c.cd).abs() < 1e-9);
    }

    #[test]
    fn review_moment_reference_translation_preserves_physical_axle_loads() {
        let mut rf = RefGeom::reference_car();
        rf.x_front_axle_m = 1.35;
        rf.x_rear_axle_m = -1.35;
        rf.a_front_m2 = 2.0;
        let (front_full, rear_full, drag_full) = (-500.0, -356.5, 1100.0);
        let (x_recorded, z_recorded, z_drag) = (0.3, 0.2, 0.8);
        for half in [false, true] {
            let divisor = if half { 2.0 } else { 1.0 };
            let (front, rear, drag) = (front_full / divisor, rear_full / divisor, drag_full / divisor);
            // Original point-force equilibrium, with the moment measured about
            // the recorded origin and the drag acting above that origin.
            let my = -(rf.x_front_axle_m - x_recorded) * front - (rf.x_rear_axle_m - x_recorded) * rear + (z_drag - z_recorded) * drag;
            let mut history = Series {
                time: (1..=8).map(f64::from).collect(),
                ..Series::default()
            };
            history.columns.insert("Fx".into(), vec![drag; 8]);
            history.columns.insert("Fz".into(), vec![front + rear; 8]);
            history.columns.insert("My".into(), vec![my; 8]);
            history.meta.insert("CofR_x".into(), x_recorded);
            history.meta.insert("CofR_z".into(), z_recorded);
            let spec = ForceSpec {
                rf: &rf,
                half_model: half,
                window: 8,
                rho: 1.225,
                u_wire: None,
                u_param: 30.0,
                cofr_x: None,
                drag_z: Some(z_drag),
            };
            let baseline = compute_forces(&history, &spec).unwrap().result;
            for custom_x in [None, Some(-0.7), Some(x_recorded), Some(0.9)] {
                let actual = compute_forces(&history, &ForceSpec { cofr_x: custom_x, ..spec }).unwrap().result;
                let ff = actual.front_force_n.expect("front axle load");
                let fr = actual.rear_force_n.expect("rear axle load");
                assert!((ff - front_full).abs() < 1e-9, "half={half}, x={custom_x:?}, front={ff}");
                assert!((fr - rear_full).abs() < 1e-9, "half={half}, x={custom_x:?}, rear={fr}");
                assert!((ff + fr - front_full - rear_full).abs() < 1e-9);
                assert!((actual.balance_front_pct.unwrap() - baseline.balance_front_pct.unwrap()).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn separated_flow_flag_reaches_the_force_report() {
        let mut rf = RefGeom::reference_car();
        rf.a_front_m2 = 2.0;
        let s = force_series(300, 548.0, -428.25, 0.0);
        let spec = ForceSpec {
            rf: &rf,
            half_model: true,
            window: 100,
            rho: 1.11,
            u_wire: Some(200.0 / 3.6),
            u_param: 30.0,
            cofr_x: None,
            drag_z: None,
        };
        let mut c = compute_forces(&s, &spec).unwrap();
        let plain = force_report(&c);
        assert!(!plain.notes.iter().any(|(_, m)| m.contains("separated-flow")));
        assert!(!plain.metrics.contains_key("separated_flow_flagged"));
        c.result.recipe = Some(crate::types::Recipe {
            separated_flow: true,
            ..Default::default()
        });
        let flagged = force_report(&c);
        assert!(
            flagged
                .notes
                .iter()
                .any(|(sev, m)| *sev == Severity::Warning && m.contains("separated-flow") && m.contains("DDES")),
            "{flagged:?}"
        );
        assert_eq!(flagged.metrics["separated_flow_flagged"].value, 1.0);
    }

    #[test]
    fn load_series_from_case_directory_with_restart() {
        let case = tmp("coeffs-load");
        let mk = |sub: &str, file: &str, text: &str| {
            let d = case.join("postProcessing").join(sub);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(file), text).unwrap();
        };
        mk(
            "forceCoeffs1/0",
            "coefficient.dat",
            "# Aref : 1.08\n# magUInf : 30\n# Time Cd Cl\n1 0.30 -0.1\n2 0.31 -0.1\n3 0.32 -0.1\n4 0.33 -0.1\n",
        );
        // restart from iteration 3 replaces the earlier rows
        mk("forceCoeffs1/3", "forceCoeffs.dat", "# Time Cd Cl\n3 0.40 -0.2\n4 0.41 -0.2\n5 0.42 -0.2\n");
        mk(
            "forces1/0",
            "force.dat",
            "# CofR : (1.5 0 0)\n# Time (total_x total_y total_z)\n1 (5 0 -3)\n2 (5 0 -3)\n3 (5 0 -3)\n4 (5 0 -3)\n5 (5 0 -3)\n",
        );
        mk(
            "solverInfo1/0",
            "solverInfo.dat",
            "# Time Ux_solver Ux_initial Ux_final Uy_initial p_solver p_initial\n1 smoothSolver 1e-2 1e-4 5e-3 GAMG 1e-1\n2 smoothSolver 1e-3 1e-4 4e-3 GAMG 1e-2\n3 smoothSolver 1e-4 1e-4 3e-3 GAMG 1e-3\n4 smoothSolver 1e-5 1e-4 2e-3 GAMG 1e-4\n5 smoothSolver 1e-5 1e-4 1e-3 GAMG 1e-5\n",
        );
        let s = load_case_series(&case);
        assert_eq!(s.time, [1.0, 2.0, 3.0, 4.0, 5.0], "common time axis of all sources");
        assert_eq!(
            s.col("Cd").unwrap(),
            [0.30, 0.31, 0.40, 0.41, 0.42],
            "restart replaces rows from its first time on"
        );
        assert_eq!(s.col("Fz").unwrap(), [-3.0; 5]);
        // U residual = worst component; solver-name columns are dropped
        assert_eq!(s.col("res_U").unwrap(), [1e-2, 4e-3, 3e-3, 2e-3, 1e-3]);
        assert_eq!(s.col("res_p").unwrap(), [1e-1, 1e-2, 1e-3, 1e-4, 1e-5]);
        assert!(s.col("Ux_solver").is_none());
        assert_eq!(s.meta["CofR_x"], 1.5);
        assert_eq!(s.meta["magUInf"], 30.0);
        assert!(!s.placeholder);
        assert!(load_case_series(&tmp("coeffs-empty")).placeholder);
    }

    const SST_RESIDUALS: &[&str] = &["p", "U", "k", "omega"];

    fn checked_write(case: &Path, file: &str, text: impl AsRef<[u8]>) {
        let path = case.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn checked_case(name: &str) -> PathBuf {
        let case = tmp(name);
        checked_write(
            &case,
            "postProcessing/forceCoeffs1/0/coefficient.dat",
            "# Aref : 1.08\n# Time Cd Cl\n1 0.31 -0.1\n2 0.32 -0.2\n",
        );
        case
    }

    // Original small fixture using the public solver-log text interface.
    fn checked_log_block(t: usize, scale: f64) -> String {
        let solve = |field: &str, r: f64| {
            format!(
                "smoothSolver:  Solving for {field}, Initial residual = {}, Final residual = 1e-9, No Iterations 2\n",
                r * scale
            )
        };
        let mut out = format!("Time = {t}\n\n");
        for (field, r) in [
            ("Ux", 0.01),
            ("Uy", 0.03),
            ("Uz", 0.02),
            ("p", 0.04),
            ("p", 0.001),
            ("k", 0.005),
            ("omega", 0.006),
        ] {
            out.push_str(&solve(field, r));
        }
        out.push_str("ExecutionTime = 0.1 s  ClockTime = 1 s\n\n");
        out
    }

    fn checked_log() -> String {
        format!("Starting time loop\n{}{}End\n", checked_log_block(1, 1.0), checked_log_block(2, 0.1))
    }

    fn checked_info() -> &'static str {
        "# Solver information\n# Time Ux_solver Ux_initial Ux_final Uy_initial Uz_initial p_initial k_initial omega_initial\n1 smoothSolver 0.01 1e-9 0.03 0.02 0.04 0.005 0.006\n2 smoothSolver 0.001 1e-9 0.003 0.002 0.004 0.0005 0.0006\n"
    }

    fn checked_error(case: &Path, fragment: &str) {
        let error = load_case_series_checked(case, SST_RESIDUALS).expect_err("incomplete or invalid fresh-run evidence must fail");
        assert!(error.contains(fragment), "expected {fragment:?} in {error:?}");
    }

    #[test]
    fn checked_complete_current_log_keeps_coefficients_and_worst_residuals() {
        let case = checked_case("checked-log");
        let mut log = checked_log();
        log = log.replace(
            "smoothSolver:  Solving for Uy, Initial residual = 0.03",
            "smoothSolver:  Solving for Uy, Initial residual = 0.07",
        );
        checked_write(&case, "logs/solver.log", log);
        let s = load_case_series_checked(&case, SST_RESIDUALS).unwrap();
        assert_eq!(s.time, [1.0, 2.0]);
        assert_eq!(s.col("Cd").unwrap(), [0.31, 0.32]);
        assert_eq!(s.col("Cl").unwrap(), [-0.1, -0.2]);
        assert_eq!(s.col("res_U").unwrap(), [0.07, 0.003]);
        assert_eq!(
            s.col("res_p").unwrap(),
            [0.04, 0.004],
            "the later pressure correction must not hide the worse first solve"
        );
        assert_eq!(s.col("res_k").unwrap(), [0.005, 0.0005]);
        assert_eq!(s.col("res_omega").unwrap(), [0.006, 0.0006000000000000001]);
        assert!(!s.placeholder);
    }

    #[test]
    fn checked_complete_solver_info_is_preferred_without_reading_unused_log() {
        let case = checked_case("checked-info");
        checked_write(&case, "postProcessing/solverInfo1/0/solverInfo.dat", checked_info());
        checked_write(&case, "logs/solver.log", "Time = NaN\ninvalid unused log\n");
        let s = load_case_series_checked(&case, SST_RESIDUALS).unwrap();
        assert_eq!(s.time, [1.0, 2.0]);
        assert_eq!(s.col("res_U").unwrap(), [0.03, 0.003]);
        assert_eq!(s.col("res_omega").unwrap(), [0.006, 0.0006]);
    }

    #[test]
    fn checked_optional_force_cadence_cannot_shrink_required_history() {
        let case = checked_case("checked-optional-cadence");
        checked_write(&case, "logs/solver.log", checked_log());
        checked_write(&case, "postProcessing/forces1/0/force.dat", "# Time (total_x total_y total_z)\n1 (5 0 -3)\n");
        let s = load_case_series_checked(&case, SST_RESIDUALS).unwrap();
        assert_eq!(s.time, [1.0, 2.0]);
        assert_eq!(s.col("res_p").unwrap(), [0.04, 0.004]);
    }

    #[test]
    fn checked_incomplete_solver_info_uses_whole_complete_current_log() {
        for (name, monitor) in [
            ("checked-info-fields", "# Time p_initial Ux_initial\n1 0.9 0.8\n2 0.9 0.8\n"),
            (
                "checked-info-times",
                "# Time Ux_initial Uy_initial Uz_initial p_initial k_initial omega_initial\n1 0.9 0.9 0.9 0.9 0.9 0.9\n",
            ),
        ] {
            let case = checked_case(name);
            checked_write(&case, "postProcessing/solverInfo1/0/solverInfo.dat", monitor);
            checked_write(&case, "logs/solver.log", checked_log());
            let s = load_case_series_checked(&case, SST_RESIDUALS).unwrap();
            assert_eq!(s.col("res_p").unwrap(), [0.04, 0.004], "do not blend a monitor row with a log row");
            assert_eq!(s.col("res_U").unwrap(), [0.03, 0.003]);
        }
    }

    #[test]
    fn checked_force_only_or_legacy_logs_cannot_certify_a_run() {
        let case = checked_case("checked-no-log");
        checked_write(&case, "log.simpleFoam", checked_log());
        checked_write(&case, "solver.log", checked_log());
        checked_write(&case, "logs/Ux_0", "1 0.01\n2 0.001\n");
        checked_error(&case, "logs");
    }

    #[test]
    fn checked_missing_required_field_or_velocity_component_fails() {
        for field in ["omega", "Uy", "Uz"] {
            let case = checked_case(&format!("checked-missing-{field}"));
            let log: String = checked_log()
                .lines()
                .filter(|l| !l.contains(&format!("Solving for {field},")))
                .map(|l| format!("{l}\n"))
                .collect();
            checked_write(&case, "logs/solver.log", log);
            checked_error(&case, field);
        }
    }

    #[test]
    fn checked_explicit_scalar_u_log_is_accepted_without_inventing_components() {
        let case = checked_case("checked-scalar-U");
        let log: String = checked_log()
            .lines()
            .filter(|l| !l.contains("Solving for Uy,") && !l.contains("Solving for Uz,"))
            .map(|l| format!("{}\n", l.replace("Solving for Ux,", "Solving for U,")))
            .collect();
        checked_write(&case, "logs/solver.log", log);
        let s = load_case_series_checked(&case, SST_RESIDUALS).unwrap();
        assert_eq!(s.col("res_U").unwrap(), [0.01, 0.001]);
    }

    #[test]
    fn checked_disjoint_or_partial_time_axes_fail_without_force_only_fallback() {
        for (name, log) in [
            ("checked-disjoint", format!("{}{}", checked_log_block(3, 1.0), checked_log_block(4, 0.1))),
            ("checked-partial", checked_log_block(1, 1.0)),
        ] {
            let case = checked_case(name);
            checked_write(&case, "logs/solver.log", log);
            checked_error(&case, "coverage");
        }
    }

    #[test]
    fn checked_malformed_and_nonfinite_solver_reports_fail_with_line_context() {
        let line = "smoothSolver:  Solving for Ux, Initial residual = 0.01, Final residual = 1e-9, No Iterations 2";
        let bad = [
            line.replace("0.01", "NaN"),
            line.replace("0.01", "inf"),
            line.replace("0.01", "-0.01"),
            line.replace("1e-9", "NaN"),
            line.replace("No Iterations 2", "No Iterations two"),
            line.replace(", Final residual = 1e-9", ""),
        ];
        for (i, bad) in bad.iter().enumerate() {
            let case = checked_case(&format!("checked-malformed-{i}"));
            checked_write(&case, "logs/solver.log", checked_log().replacen(line, bad, 1));
            let error = load_case_series_checked(&case, SST_RESIDUALS).unwrap_err();
            assert!(error.contains("solver.log") && error.contains("line"), "{error}");
        }
    }

    #[test]
    fn checked_time_and_incomplete_last_iteration_are_not_silently_dropped() {
        for (i, log) in [
            checked_log().replace("Time = 1\n", ""),
            checked_log().replace("Time = 2", "Time = 1"),
            checked_log().replace("Time = 2", "Time = 0.5"),
            checked_log().replace("Time = 2", "Time = NaN"),
            format!(
                "{}Time = 2\nsmoothSolver: Solving for Ux, Initial residual = 0.1, Final residual = 0.01, No Iterations 1\n",
                checked_log_block(1, 1.0)
            ),
        ]
        .iter()
        .enumerate()
        {
            let case = checked_case(&format!("checked-time-{i}"));
            checked_write(&case, "logs/solver.log", log);
            checked_error(&case, "solver.log");
        }
    }

    #[test]
    fn checked_coefficients_must_be_named_complete_and_finite() {
        for (i, data) in [
            "# Time Cd Cl\n",
            "# Time Cd Cl\n1 0.31\n",
            "# Time Cd Cl\n1 NaN -0.1\n",
            "# Time Cd Cl\n1 0.31 inf\n",
            "# Time foo Cl\n1 0.31 -0.1\n",
        ]
        .iter()
        .enumerate()
        {
            let case = checked_case(&format!("checked-coeff-{i}"));
            checked_write(&case, "postProcessing/forceCoeffs1/0/coefficient.dat", data);
            checked_write(&case, "logs/solver.log", checked_log());
            checked_error(&case, "coefficient");
        }
    }

    #[test]
    fn checked_invalid_monitor_numeric_data_does_not_get_hidden_by_fallback() {
        let case = checked_case("checked-invalid-monitor");
        checked_write(&case, "postProcessing/solverInfo1/0/solverInfo.dat", checked_info().replacen("0.03", "NaN", 1));
        checked_write(&case, "logs/solver.log", checked_log());
        checked_error(&case, "solverInfo.dat");
    }

    #[test]
    fn checked_history_limits_are_enforced_before_unbounded_work() {
        let case = checked_case("checked-limits");
        checked_write(&case, "logs/solver.log", checked_log());
        let defaults = HistoryLimits::default();
        for (limits, label) in [
            (HistoryLimits { bytes: 16, ..defaults }, "byte"),
            (HistoryLimits { line: 20, ..defaults }, "line"),
            (HistoryLimits { rows: 1, ..defaults }, "row"),
            (HistoryLimits { entries: 1, ..defaults }, "entr"),
        ] {
            let error = load_case_series_checked_with_limits(&case, SST_RESIDUALS, limits).unwrap_err();
            assert!(error.contains(label), "{label}: {error}");
        }
    }

    #[test]
    fn checked_stream_read_errors_and_invalid_utf8_are_explicit() {
        struct BrokenRead(std::io::Cursor<Vec<u8>>);
        impl std::io::Read for BrokenRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected read fault"))
            }
        }
        impl std::io::BufRead for BrokenRead {
            fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
                if self.0.position() == self.0.get_ref().len() as u64 {
                    Err(std::io::Error::other("injected read fault"))
                } else {
                    std::io::BufRead::fill_buf(&mut self.0)
                }
            }
            fn consume(&mut self, n: usize) {
                std::io::BufRead::consume(&mut self.0, n);
            }
        }
        let error = residual_log_reader(
            BrokenRead(std::io::Cursor::new(checked_log_block(1, 1.0).into_bytes())),
            Path::new("solver.log"),
            SST_RESIDUALS,
            &mut HistoryBudget::default(),
        )
        .err()
        .expect("read fault must propagate");
        assert!(error.contains("injected read fault"), "{error}");
        let case = checked_case("checked-invalid-utf8");
        checked_write(&case, "logs/solver.log", [0xff, b'\n']);
        checked_error(&case, "UTF-8");
    }

    #[test]
    fn checked_cancellation_and_special_files_fail_without_panicking() {
        use std::sync::{Arc, atomic::AtomicBool};
        let case = checked_case("checked-cancel");
        checked_write(&case, "logs/solver.log", checked_log());
        let error = vernon_core::process::with_cancel(Arc::new(AtomicBool::new(true)), || load_case_series_checked(&case, SST_RESIDUALS)).unwrap_err();
        assert!(error.contains("cancel"), "{error}");
        let special = checked_case("checked-special");
        std::fs::create_dir_all(special.join("logs/solver.log")).unwrap();
        checked_error(&special, "regular");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn checked_selected_symlink_is_rejected() {
        let case = checked_case("checked-link");
        let target = case.join("outside.log");
        std::fs::write(&target, checked_log()).unwrap();
        std::fs::create_dir_all(case.join("logs")).unwrap();
        #[cfg(unix)]
        let result = std::os::unix::fs::symlink(&target, case.join("logs/solver.log"));
        #[cfg(windows)]
        let result = std::os::windows::fs::symlink_file(&target, case.join("logs/solver.log"));
        if let Err(error) = result {
            // Windows reports a missing symlink privilege as raw error 1314.
            if error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(1314) {
                eprintln!("SKIP selected symlink fixture: OS denied link creation: {error}");
                return;
            }
            panic!("symlink fixture: {error}");
        }
        checked_error(&case, "link");
    }

    #[test]
    fn force_coeffs_node_flags_missing_history_without_inventing_numbers() {
        let ctx = ctx("fc-empty");
        let p = Params::from_specs(&force_info().params);
        let empty = Series {
            placeholder: true,
            ..Series::default()
        };
        let out = force_cook(&ctx, &[Some(Data::json(empty.to_json()))], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(!r.passes());
        let res = ForceResult::from_json(out[0].as_json().unwrap()).unwrap();
        assert!(res.placeholder && res.cda == 0.0);
        assert!(force_cook(&ctx, &[], &p).is_err());
        // history without usable columns is a clean error, not a panic
        let mut odd = Series {
            time: vec![1.0],
            ..Series::default()
        };
        odd.columns.insert("foo".into(), vec![1.0]);
        assert!(force_cook(&ctx, &[Some(Data::json(odd.to_json()))], &p).is_err());
    }

    #[test]
    fn force_coeffs_node_cooks_a_history() {
        let ctx = ctx("fc-run");
        let mut p = Params::from_specs(&force_info().params);
        p.set("window", ParamValue::Int(100));
        p.set("half_model", ParamValue::Bool(false));
        let s = force_series(250, 700.0, -450.0, 0.0);
        let out = force_cook(&ctx, &[Some(Data::json(s.to_json()))], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        let res = ForceResult::from_json(out[0].as_json().unwrap()).unwrap();
        assert!(res.cda > 0.0 && res.cla < 0.0 && !res.placeholder);
        assert!(r.metrics.contains_key("balance_front_pct"));
    }

    #[test]
    fn review_nonpositive_drag_node_report_and_json_are_both_unusable() {
        let ctx = ctx("force-invalid-drag");
        let mut params = Params::from_specs(&force_info().params);
        params.set("half_model", ParamValue::Bool(false));
        params.set("window", ParamValue::Int(100));
        for coefficient_input in [false, true] {
            for drag in [0.0, -0.2] {
                let history = if coefficient_input {
                    let mut history = Series {
                        time: (1..=200).map(f64::from).collect(),
                        ..Series::default()
                    };
                    history.columns.insert("Cd".into(), vec![drag; 200]);
                    history.columns.insert("Cl".into(), vec![-0.1; 200]);
                    history
                } else {
                    force_series(200, drag * 1000.0, -450.0, 0.0)
                };
                let out = force_cook(&ctx, &[Some(Data::json(history.to_json()))], &params).unwrap();
                let report = out[1].as_report().unwrap();
                assert_eq!(
                    report.metrics["data_available"].value, 0.0,
                    "coefficient_input={coefficient_input}, drag={drag}"
                );
                assert!(!report.passes(), "nonpositive drag cannot be available: {report:?}");
                assert!(
                    report
                        .notes
                        .iter()
                        .any(|(severity, message)| *severity == Severity::Error && message.contains("drag")),
                    "{report:?}"
                );
                let result = ForceResult::from_json(out[0].as_json().unwrap()).unwrap();
                assert!(result.placeholder, "JSON consumers must also see unusable data: {result:?}");
                assert!(result.cd <= 0.0 && result.cda <= 0.0, "retain the raw drag values for diagnosis");
            }
        }
    }

    #[test]
    fn review_positive_drag_with_downforce_stays_available() {
        let ctx = ctx("force-valid-downforce");
        let mut params = Params::from_specs(&force_info().params);
        params.set("half_model", ParamValue::Bool(false));
        params.set("window", ParamValue::Int(100));
        let mut history = Series {
            time: (1..=200).map(f64::from).collect(),
            ..Series::default()
        };
        history.columns.insert("Cd".into(), vec![0.3; 200]);
        history.columns.insert("Cl".into(), vec![-0.2; 200]);
        let out = force_cook(&ctx, &[Some(Data::json(history.to_json()))], &params).unwrap();
        let report = out[1].as_report().unwrap();
        let result = ForceResult::from_json(out[0].as_json().unwrap()).unwrap();
        assert!(report.passes(), "{report:?}");
        assert_eq!(report.metrics["data_available"].value, 1.0);
        assert!(!result.placeholder && result.cd > 0.0 && result.cla < 0.0);
    }

    fn conv_series(n: usize, drift: f64, resid: f64) -> Series {
        let mut s = Series {
            time: (1..=n).map(|i| i as f64).collect(),
            ..Series::default()
        };
        s.columns
            .insert("Cd".into(), (0..n).map(|i| 0.3 + drift * i as f64 + 1e-5 * ((i % 4) as f64 - 1.5)).collect());
        s.columns.insert("res_p".into(), vec![resid; n]);
        s.columns.insert("res_U".into(), vec![resid * 0.5; n]);
        s.columns.insert("res_k".into(), vec![resid * 0.75; n]);
        s.columns.insert("res_omega".into(), vec![resid * 0.25; n]);
        s
    }

    fn conv_spec(window: usize) -> ConvSpec {
        ConvSpec {
            window,
            tol_pct: 0.2,
            drift_limit_pct: 0.1,
            sd_limit_pct: 0.5,
            residual_target: 1e-5,
            require_residuals: true,
        }
    }

    #[test]
    fn iteration_gate_passes_a_flat_converged_run() {
        let g = iteration_gate(&conv_series(400, 0.0, 5e-6), "auto", &conv_spec(100)).unwrap();
        assert!(g.converged && g.enough_iterations && !g.unsteady, "{g:?}");
        assert!(g.residuals["p"] <= 1e-5);
    }

    #[test]
    fn review_convergence_requires_every_sst_residual_field() {
        for field in ["p", "U", "k", "omega"] {
            let mut history = conv_series(400, 0.0, 5e-6);
            history.columns.shift_remove(&format!("res_{field}"));
            let gate = iteration_gate(&history, "auto", &conv_spec(100)).unwrap();
            assert!(!gate.converged && !gate.residuals_ok && !gate.residuals_checked, "missing {field}: {gate:?}");
        }
        let mut only_p_u = conv_series(400, 0.0, 5e-6);
        only_p_u.columns.shift_remove("res_k");
        only_p_u.columns.shift_remove("res_omega");
        let gate = iteration_gate(&only_p_u, "auto", &conv_spec(100)).unwrap();
        assert!(!gate.converged && !gate.residuals_checked, "p/U alone is not complete SST evidence");
        let complete = iteration_gate(&conv_series(400, 0.0, 5e-6), "auto", &conv_spec(100)).unwrap();
        assert!(complete.converged && complete.residuals_checked);
        assert_eq!(complete.residuals.len(), 4);
    }

    #[test]
    fn review_convergence_checks_turbulence_residual_targets() {
        for field in ["k", "omega"] {
            let mut history = conv_series(400, 0.0, 5e-6);
            history.columns.insert(format!("res_{field}"), vec![2e-5; 400]);
            let gate = iteration_gate(&history, "auto", &conv_spec(100)).unwrap();
            assert!(gate.residuals_checked && !gate.residuals_ok && !gate.converged, "{field}: {gate:?}");
        }
    }

    #[test]
    fn review_convergence_rejects_malformed_present_residuals_even_if_waived() {
        for require in [true, false] {
            for field in ["p", "U", "k", "omega", "extra"] {
                for value in [f64::NAN, f64::INFINITY, -1e-8] {
                    for index in [0, 399] {
                        let mut history = conv_series(400, 0.0, 5e-6);
                        history.columns.entry(format!("res_{field}")).or_insert_with(|| vec![5e-6; 400])[index] = value;
                        let spec = ConvSpec {
                            require_residuals: require,
                            ..conv_spec(100)
                        };
                        let error = iteration_gate(&history, "auto", &spec).expect_err("invalid residual evidence must not be dropped or waived");
                        assert!(error.contains(field), "{error}");
                    }
                }
            }
        }
    }

    #[test]
    fn review_convergence_rejects_residual_shape_errors_without_panic() {
        for len in [0, 10, 399, 401] {
            let mut history = conv_series(400, 0.0, 5e-6);
            history.columns.insert("res_k".into(), vec![5e-6; len]);
            let error = iteration_gate(&history, "auto", &conv_spec(100)).expect_err("residuals must cover the whole monitored history");
            assert!(error.contains("res_k"), "{error}");
        }
    }

    #[test]
    fn review_convergence_rms_handles_zero_and_large_finite_values() {
        let zero = iteration_gate(&conv_series(400, 0.0, 0.0), "auto", &conv_spec(100)).unwrap();
        assert!(zero.converged && zero.residuals.values().all(|v| *v == 0.0));
        let mut large = conv_series(400, 0.0, 5e-6);
        large.columns.insert("res_k".into(), vec![1e300; 400]);
        let gate = iteration_gate(&large, "auto", &conv_spec(100)).unwrap();
        assert!(gate.residuals["k"].is_finite(), "finite input should yield a finite RMS: {gate:?}");
        assert!((gate.residuals["k"] / 1e300 - 1.0).abs() < 1e-12);
        assert!(!gate.residuals_ok && !gate.converged);
    }

    #[test]
    fn review_convergence_waiver_allows_only_missing_residual_evidence() {
        let waived = ConvSpec {
            require_residuals: false,
            ..conv_spec(100)
        };
        let mut partial = conv_series(400, 0.0, 5e-6);
        partial.columns.shift_remove("res_k");
        partial.columns.shift_remove("res_omega");
        assert!(iteration_gate(&partial, "auto", &waived).unwrap().converged);
        partial.columns.insert("res_p".into(), vec![2e-5; 400]);
        assert!(
            !iteration_gate(&partial, "auto", &waived).unwrap().converged,
            "present residual targets are not waived"
        );
    }

    fn qualification_history(n: usize) -> Series {
        let mut history = conv_series(n, 0.0, 5e-6);
        history.columns.insert("Cl".into(), vec![-0.1; n]);
        history.meta.insert("Aref".into(), 2.0);
        history.meta.insert("magUInf".into(), 30.0);
        history.solve_evidence = Some(crate::types::SolveEvidence {
            version: 1,
            mesh: crate::types::MeshEvidence {
                version: 1,
                cells: 12_345_678,
                mesh_fingerprint: "0123456789abcdef".into(),
                max_nonortho: 62.5,
                max_skew: 2.25,
            },
            input_fingerprint: "fedcba9876543210".into(),
            reference_scope: None,
            reference_binding: None,
        });
        history
    }

    fn qualification_compute(history: &Series, window: usize) -> Computed {
        let mut reference = RefGeom::reference_car();
        reference.a_front_m2 = 2.0;
        compute_forces(
            history,
            &ForceSpec {
                rf: &reference,
                half_model: false,
                window,
                rho: 1.225,
                u_wire: None,
                u_param: 30.0,
                cofr_x: None,
                drag_z: None,
            },
        )
        .expect("raw diagnostic coefficients remain readable")
    }

    #[test]
    fn qualification_complete_coefficients_bind_strict_gate_and_solve_evidence() {
        let history = qualification_history(2000);
        let computed = qualification_compute(&history, 1000);
        let proof = computed
            .result
            .qualification
            .as_ref()
            .expect("valid measured normalized history needs evidence");
        assert_eq!(proof.version, 1);
        assert_eq!(proof.solve, history.solve_evidence.clone().unwrap());
        assert_eq!(proof.column, "Cd");
        assert_eq!((proof.window, proof.samples), (1000, 2000));
        assert_eq!(proof.convergence, iteration_gate(&history, "Cd", &conv_spec(1000)).unwrap());
        assert!(proof.convergence.converged);
        assert!(proof.solve.reference_scope.is_none());
        let json = computed.result.to_json();
        assert_eq!(ForceResult::from_json(&json).unwrap(), computed.result);
        assert!(json["qualification"]["solve"].get("reference_scope").is_none());
    }

    #[test]
    fn qualification_force_source_uses_fx_even_when_auto_would_pick_cd() {
        let q_area = 0.5 * 1.225 * 30.0 * 30.0 * 2.0;
        for force_drifts in [false, true] {
            let mut history = qualification_history(200);
            history
                .columns
                .insert("Cd".into(), (0..200).map(|i| if !force_drifts && i < 100 { 0.2 } else { 0.3 }).collect());
            history.columns.insert(
                "Fx".into(),
                (0..200).map(|i| q_area * if force_drifts && i < 100 { 0.2 } else { 0.3 }).collect(),
            );
            history.columns.insert("Fz".into(), vec![-0.1 * q_area; 200]);
            let computed = qualification_compute(&history, 100);
            assert_eq!(
                force_report(&computed).passes(),
                !force_drifts,
                "normalization agrees, but iteration qualification must gate the report"
            );
            let proof = computed.result.qualification.as_ref().expect("force-source evidence");
            assert_eq!(proof.column, "Fx");
            assert_eq!(proof.convergence, iteration_gate(&history, "Fx", &conv_spec(100)).unwrap());
            assert_eq!(proof.convergence.converged, !force_drifts);
            assert_ne!(
                proof.convergence.converged,
                iteration_gate(&history, "auto", &conv_spec(100)).unwrap().converged
            );
        }
    }

    #[test]
    fn qualification_requires_two_actual_coefficient_windows() {
        let boundary = qualification_compute(&qualification_history(20), 10);
        let proof = boundary.result.qualification.as_ref().expect("small smoke window with complete evidence");
        assert_eq!((proof.window, proof.convergence.window, proof.samples), (10, 10, 20));
        for (samples, requested) in [(19, 10), (8, 10), (2, 1)] {
            let computed = qualification_compute(&qualification_history(samples), requested);
            assert!(
                computed.result.qualification.is_none(),
                "cannot bind a shortened gate to a different coefficient window"
            );
            assert!(!computed.result.placeholder && computed.result.cd > 0.0);
        }
    }

    #[test]
    fn qualification_retains_nonconverged_diagnostics_without_waiving_sst() {
        for bad_field in ["k", "omega"] {
            let mut history = qualification_history(200);
            history.columns.insert(format!("res_{bad_field}"), vec![2e-5; 200]);
            let computed = qualification_compute(&history, 100);
            let proof = computed.result.qualification.as_ref().expect("retain non-converged diagnostics");
            assert!(!proof.convergence.converged && !proof.convergence.residuals_ok);
            assert!(proof.convergence.residuals[bad_field] > 1e-5);
        }
        let mut missing = qualification_history(200);
        missing.columns.shift_remove("res_omega");
        let computed = qualification_compute(&missing, 100);
        let proof = computed.result.qualification.as_ref().expect("missing-field diagnostic bound to this solve");
        assert!(!proof.convergence.converged && !proof.convergence.residuals_checked);
        assert!(!proof.convergence.residuals.contains_key("omega"));
    }

    #[test]
    fn qualification_malformed_residual_diagnosed_while_raw_coefficients_remain() {
        for invalid in [f64::NAN, f64::INFINITY, -0.01] {
            let mut history = qualification_history(200);
            history.columns.get_mut("res_k").unwrap()[0] = invalid;
            let computed = qualification_compute(&history, 100);
            assert!(computed.result.qualification.is_none());
            assert!(!computed.result.placeholder && computed.result.cd > 0.0);
            assert!(
                computed.notes.iter().any(|(_, note)| note.contains("qualification") && note.contains("res_k")),
                "{:?}",
                computed.notes
            );
        }
    }

    #[test]
    fn qualification_missing_or_invalid_provenance_never_invents_proof() {
        let mut variants = vec![qualification_history(200); 9];
        variants[0].solve_evidence = None;
        variants[1].solve_evidence.as_mut().unwrap().version = 2;
        variants[2].solve_evidence.as_mut().unwrap().mesh.version = 2;
        variants[3].solve_evidence.as_mut().unwrap().mesh.cells = 0;
        variants[4].solve_evidence.as_mut().unwrap().input_fingerprint = "bad".into();
        variants[5].solve_evidence.as_mut().unwrap().mesh.mesh_fingerprint = "gggggggggggggggg".into();
        variants[6].solve_evidence.as_mut().unwrap().mesh.max_nonortho = 70.0;
        variants[7].solve_evidence.as_mut().unwrap().mesh.max_skew = 4.0;
        variants[8].solve_evidence.as_mut().unwrap().mesh.max_skew = f64::NAN;
        for history in variants {
            let computed = qualification_compute(&history, 100);
            assert!(computed.result.qualification.is_none());
            assert!(!computed.result.placeholder && computed.result.cd > 0.0);
        }
    }

    #[test]
    fn qualification_requires_recorded_positive_normalization_metadata() {
        for name in ["Aref", "magUInf"] {
            for value in [None, Some(0.0), Some(-1.0), Some(f64::NAN)] {
                let mut history = qualification_history(200);
                match value {
                    Some(value) => {
                        history.meta.insert(name.into(), value);
                    }
                    None => {
                        history.meta.shift_remove(name);
                    }
                }
                let computed = qualification_compute(&history, 100);
                assert!(computed.result.qualification.is_none(), "{name}={value:?} is not measured normalization");
                assert!(computed.result.cd > 0.0);
            }
        }
        let mut wrong_area = qualification_history(200);
        wrong_area.meta.insert("Aref".into(), 4.0);
        let computed = qualification_compute(&wrong_area, 100);
        assert!(!force_report(&computed).passes() && computed.result.qualification.is_none());
    }

    #[test]
    fn qualification_legacy_force_json_defaults_to_absent_evidence() {
        let legacy = serde_json::json!({"placeholder": false, "source": "coefficients", "cd": 0.3, "cda": 0.6});
        let parsed = ForceResult::from_json(&legacy).unwrap();
        assert!(parsed.qualification.is_none());
        assert!(parsed.to_json().get("qualification").is_none());
    }

    #[test]
    fn qualification_report_nonconverged_proof_fails_without_hiding_raw_data() {
        let mut history = qualification_history(200);
        history.columns.insert("res_k".into(), vec![2e-5; 200]);
        let computed = qualification_compute(&history, 100);
        assert!(!computed.result.qualification.as_ref().unwrap().convergence.converged);
        assert!(!computed.result.placeholder && computed.result.cd > 0.0);
        let report = force_report(&computed);
        assert_eq!(report.metrics["data_available"].value, 1.0);
        assert!(
            !report.passes(),
            "an explicitly nonconverged solve must not pass to Report consumers: {report:?}"
        );
        let metric = &report.metrics[vernon_core::keys::CFD_ITERATION_QUALIFIED];
        assert_eq!(metric.value, 0.0);
        assert_eq!(metric.passes(), Some(false));
        assert!(
            report
                .notes
                .iter()
                .any(|(severity, note)| *severity == Severity::Error && note.contains("converg")),
            "{report:?}"
        );
    }

    #[test]
    fn qualification_report_converged_proof_has_explicit_passing_metric() {
        let computed = qualification_compute(&qualification_history(200), 100);
        assert!(computed.result.qualification.as_ref().unwrap().convergence.converged);
        let report = force_report(&computed);
        assert!(report.passes(), "{report:?}");
        assert_eq!(report.metrics["data_available"].value, 1.0);
        let metric = &report.metrics[vernon_core::keys::CFD_ITERATION_QUALIFIED];
        assert_eq!(metric.value, 1.0);
        assert_eq!(metric.passes(), Some(true));
    }

    #[test]
    fn qualification_report_absent_legacy_proof_does_not_invent_a_gate() {
        let mut history = qualification_history(200);
        history.solve_evidence = None;
        let computed = qualification_compute(&history, 100);
        assert!(computed.result.qualification.is_none());
        let report = force_report(&computed);
        assert!(report.passes(), "legacy diagnostic behavior stays readable: {report:?}");
        assert_eq!(report.metrics["data_available"].value, 1.0);
        assert!(!report.metrics.contains_key(vernon_core::keys::CFD_ITERATION_QUALIFIED));
    }

    #[test]
    fn qualification_invalid_time_axis_cannot_manufacture_iteration_evidence() {
        for (label, index, value) in [
            ("duplicate", 199, 199.0),
            ("decreasing", 199, 150.0),
            ("negative", 0, -1.0),
            ("nonfinite", 0, f64::NAN),
            ("infinite", 199, f64::INFINITY),
        ] {
            let mut history = qualification_history(200);
            history.time[index] = value;
            let computed = qualification_compute(&history, 100);
            assert!(
                computed.result.qualification.is_none(),
                "{label} times cannot prove 200 distinct measured iterations"
            );
            assert!(!computed.result.placeholder && computed.result.cd > 0.0, "raw diagnostics remain readable");
            assert!(
                computed.notes.iter().any(|(_, note)| note.contains("qualification") && note.contains("time")),
                "{label}: {:?}",
                computed.notes
            );
        }
    }

    #[test]
    fn iteration_gate_fails_drift_residuals_and_short_runs() {
        // 0.0001 per iteration on 0.3: 100 samples drift by about 3%
        let g = iteration_gate(&conv_series(400, 1e-4, 5e-6), "auto", &conv_spec(100)).unwrap();
        assert!(!g.converged && g.window_diff_pct > 0.2 && g.drift_pct > 0.1, "{g:?}");
        let g = iteration_gate(&conv_series(400, 0.0, 5e-5), "auto", &conv_spec(100)).unwrap();
        assert!(!g.converged && !g.residuals_ok);
        let g = iteration_gate(&conv_series(150, 0.0, 5e-6), "auto", &conv_spec(100)).unwrap();
        assert!(!g.converged && !g.enough_iterations);
        assert!(iteration_gate(&conv_series(400, 0.0, 5e-6), "Fz", &conv_spec(100)).is_err());
        assert!(iteration_gate(&conv_series(3, 0.0, 5e-6), "auto", &conv_spec(100)).is_err());
    }

    #[test]
    fn scatter_flags_unsteady_without_failing() {
        let mut s = conv_series(400, 0.0, 5e-6);
        let cd = s.columns.get_mut("Cd").unwrap();
        for (i, v) in cd.iter_mut().enumerate() {
            *v = 0.3 + if i % 2 == 0 { 0.003 } else { -0.003 };
        }
        let g = iteration_gate(&s, "auto", &conv_spec(100)).unwrap();
        assert!(g.unsteady && g.converged, "{g:?}");
    }

    #[test]
    fn missing_residuals_fail_unless_waived() {
        let mut s = conv_series(400, 0.0, 5e-6);
        s.columns.retain(|k, _| !k.starts_with("res_"));
        let g = iteration_gate(&s, "auto", &conv_spec(100)).unwrap();
        assert!(!g.converged && !g.residuals_checked);
        let waived = ConvSpec {
            require_residuals: false,
            ..conv_spec(100)
        };
        assert!(iteration_gate(&s, "auto", &waived).unwrap().converged);
    }

    #[test]
    fn convergence_node_reports_limits() {
        let ctx = ctx("conv");
        let mut p = Params::from_specs(&conv_info().params);
        p.set("window", ParamValue::Int(100));
        let out = conv_cook(&ctx, &[Some(Data::json(conv_series(400, 0.0, 5e-6).to_json()))], &p).unwrap();
        let r = out[1].as_report().unwrap();
        assert!(r.passes(), "{r:?}");
        assert_eq!(out[0].as_json().unwrap()["converged"], true);
        let out = conv_cook(&ctx, &[Some(Data::json(conv_series(400, 1e-4, 5e-6).to_json()))], &p).unwrap();
        assert!(!out[1].as_report().unwrap().passes());
        let none = Series {
            placeholder: true,
            ..Series::default()
        };
        let out = conv_cook(&ctx, &[Some(Data::json(none.to_json()))], &p).unwrap();
        assert!(!out[1].as_report().unwrap().passes());
        assert!(conv_cook(&ctx, &[], &p).is_err());
    }
}
