//! Request/result harness for the slicer's post-processing script.
//!
//! Flow: `request_json` lists G-code insertions; `script` renders the post-processing script
//! (text in `script_template.rs`); the slicer, or a [`Runner`] in tests, starts it with the
//! G-code path; `parse_result` strictly validates the result file. [`apply_to_gcode`] is
//! the same set of rules written in Rust, so the generated script can be tested against it
//! without any slicer.
use crate::script_template::{self, DONE_PREFIX, LAYER_MARKER, PROTOCOL_VERSION};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// G-code to insert directly before the start of one layer (layers count from 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insertion {
    pub layer: u32,
    /// One or more G-code lines separated by `\n`.
    pub gcode: String,
}

/// Outcome of one insertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Applied,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacementResult {
    pub index: usize,
    pub status: Status,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BridgeError {
    Io(String),
    Invalid(String),
    Runner(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeError::Io(m) => write!(f, "io: {m}"),
            BridgeError::Invalid(m) => write!(f, "invalid: {m}"),
            BridgeError::Runner(m) => write!(f, "runner: {m}"),
        }
    }
}
impl std::error::Error for BridgeError {}

fn bad<T>(m: impl Into<String>) -> Result<T, BridgeError> {
    Err(BridgeError::Invalid(m.into()))
}

/// Largest layer number a request may name.
pub const MAX_LAYER: u32 = 1_000_000;

fn check_insertion(i: usize, ins: &Insertion) -> Result<(), BridgeError> {
    if ins.layer == 0 || ins.layer > MAX_LAYER {
        return bad(format!("insertion {i}: layer must be in 1..={MAX_LAYER}"));
    }
    if ins.gcode.trim().is_empty() {
        return bad(format!("insertion {i}: no G-code given"));
    }
    for line in ins.gcode.split('\n') {
        if line.chars().any(|c| c.is_control()) || line.chars().count() > 200 {
            return bad(format!("insertion {i}: lines must be printable text of at most 200 characters"));
        }
    }
    Ok(())
}

/// Serialise and validate a request.
pub fn request_json(insertions: &[Insertion]) -> Result<String, BridgeError> {
    let mut items = Vec::with_capacity(insertions.len());
    for (i, ins) in insertions.iter().enumerate() {
        check_insertion(i, ins)?;
        items.push(json!({ "layer": ins.layer, "gcode": ins.gcode }));
    }
    Ok(json!({ "version": PROTOCOL_VERSION, "insertions": items }).to_string())
}

/// Parse a request produced by [`request_json`].
pub fn parse_request(text: &str) -> Result<Vec<Insertion>, BridgeError> {
    let v: Value = serde_json::from_str(text).map_err(|e| BridgeError::Invalid(e.to_string()))?;
    if v.get("version").and_then(Value::as_u64) != Some(PROTOCOL_VERSION as u64) {
        return bad("unsupported request version");
    }
    let Some(list) = v.get("insertions").and_then(Value::as_array) else {
        return bad("missing `insertions` array");
    };
    let mut out = Vec::new();
    for (i, c) in list.iter().enumerate() {
        let (Some(layer), Some(gcode)) = (c.get("layer").and_then(Value::as_u64), c.get("gcode").and_then(Value::as_str)) else {
            return bad(format!("insertion {i}: bad layer or gcode"));
        };
        let ins = Insertion { layer: u32::try_from(layer).unwrap_or(0), gcode: gcode.to_string() };
        check_insertion(i, &ins)?;
        out.push(ins);
    }
    Ok(out)
}

/// The rules of the generated script, in Rust: insert each request's lines directly before
/// the n-th layer marker, leave the rest of the file as it is, and add a summary comment.
/// Returns the new text and one result per insertion. A file that already carries the
/// summary comment is left unchanged and every insertion is reported `failed`.
pub fn apply_to_gcode(text: &str, insertions: &[Insertion]) -> (String, Vec<PlacementResult>) {
    let lines: Vec<&str> = text.split('\n').collect();
    let failed = |index: usize, message: String| PlacementResult { index, status: Status::Failed, message };
    if lines.iter().any(|l| l.trim().starts_with(DONE_PREFIX)) {
        let results = (0..insertions.len()).map(|i| failed(i, "file was already processed".into())).collect();
        return (text.to_string(), results);
    }
    let eol = if lines.first().is_some_and(|l| l.ends_with('\r')) { "\r" } else { "" };
    let positions: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.trim() == LAYER_MARKER).map(|(i, _)| i).collect();
    let mut before: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut results = Vec::with_capacity(insertions.len());
    for (index, ins) in insertions.iter().enumerate() {
        match (ins.layer as usize).checked_sub(1).and_then(|l| positions.get(l)) {
            Some(&pos) => {
                before.entry(pos).or_default().push(index);
                results.push(PlacementResult { index, status: Status::Applied, message: String::new() });
            }
            None => results.push(failed(index, format!("layer {} not found (file has {} layers)", ins.layer, positions.len()))),
        }
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        for &index in before.get(&i).into_iter().flatten() {
            let ins = &insertions[index];
            out.push(format!("; vernon: inserted before layer {}{eol}", ins.layer));
            out.extend(ins.gcode.split('\n').map(|g| format!("{g}{eol}")));
        }
        out.push((*line).to_string());
    }
    let applied = results.iter().filter(|r| r.status == Status::Applied).count();
    let summary = format!("{DONE_PREFIX} applied={applied} failed={}", results.len() - applied);
    if out.last().is_some_and(|l| l.is_empty()) {
        out.insert(out.len() - 1, format!("{summary}{eol}"));
    } else {
        out.push(summary);
    }
    (out.join("\n"), results)
}

/// Post-processing script text for the given request/result file paths.
pub fn script(request_path: &Path, result_path: &Path) -> String {
    script_template::render(&request_path.to_string_lossy(), &result_path.to_string_lossy())
}

/// Serialise results in the format the script writes.
pub fn result_json(results: &[PlacementResult]) -> String {
    let items: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "index": r.index,
                "status": if r.status == Status::Applied { "applied" } else { "failed" },
                "message": r.message,
            })
        })
        .collect();
    json!({ "version": PROTOCOL_VERSION, "results": items }).to_string()
}

/// Strictly validate result text against a request of `expected` insertions:
/// exact version, exactly `expected` entries with indices `0..expected` in order,
/// known status strings, string messages, and no unknown fields.
pub fn parse_result(text: &str, expected: usize) -> Result<Vec<PlacementResult>, BridgeError> {
    let v: Value = serde_json::from_str(text).map_err(|e| BridgeError::Invalid(e.to_string()))?;
    let obj = match v.as_object() {
        Some(o) => o,
        None => return bad("result must be a JSON object"),
    };
    if let Some(k) = obj.keys().find(|k| *k != "version" && *k != "results") {
        return bad(format!("unknown top-level field `{k}`"));
    }
    if obj.get("version").and_then(Value::as_u64) != Some(PROTOCOL_VERSION as u64) {
        return bad("unsupported result version");
    }
    let list = match obj.get("results").and_then(Value::as_array) {
        Some(l) => l,
        None => return bad("missing `results` array"),
    };
    if list.len() != expected {
        return bad(format!("expected {expected} results, got {}", list.len()));
    }
    let mut out = Vec::with_capacity(expected);
    for (i, r) in list.iter().enumerate() {
        let o = match r.as_object() {
            Some(o) => o,
            None => return bad(format!("result {i} is not an object")),
        };
        if let Some(k) = o.keys().find(|k| !["index", "status", "message"].contains(&k.as_str())) {
            return bad(format!("result {i}: unknown field `{k}`"));
        }
        if o.get("index").and_then(Value::as_u64) != Some(i as u64) {
            return bad(format!("result {i}: index out of order"));
        }
        let status = match o.get("status").and_then(Value::as_str) {
            Some("applied") => Status::Applied,
            Some("failed") => Status::Failed,
            _ => return bad(format!("result {i}: status must be `applied` or `failed`")),
        };
        let message = match o.get("message").and_then(Value::as_str) {
            Some(m) => m.to_string(),
            None => return bad(format!("result {i}: message must be a string")),
        };
        out.push(PlacementResult { index: i, status, message });
    }
    Ok(out)
}

/// Read and validate a result file; a missing or unreadable file is an error.
pub fn read_result(path: &Path, expected: usize) -> Result<Vec<PlacementResult>, BridgeError> {
    let text = std::fs::read_to_string(path).map_err(|e| BridgeError::Io(format!("{}: {e}", path.display())))?;
    parse_result(&text, expected)
}

/// Starts the script on a G-code file. Real runs are done by the slicer itself; tests use
/// [`PythonRunner`] or [`ReferenceRunner`].
pub trait Runner {
    /// Run `script_path` on `gcode_path`; the script leaves a result file at `result_path`.
    fn run(&self, script_path: &Path, gcode_path: &Path, request_path: &Path, result_path: &Path) -> Result<(), BridgeError>;
}

/// Runs the generated script with a Python 3 interpreter, the way the slicer does.
#[derive(Debug, Clone)]
pub struct PythonRunner {
    pub python: PathBuf,
}

impl Runner for PythonRunner {
    fn run(&self, script: &Path, gcode: &Path, _request: &Path, _result: &Path) -> Result<(), BridgeError> {
        let out = std::process::Command::new(&self.python)
            .arg("-I")
            .arg(script)
            .arg(gcode)
            .output()
            .map_err(|e| BridgeError::Runner(format!("{}: {e}", self.python.display())))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(BridgeError::Runner(format!("script exited with {}: {}", out.status, String::from_utf8_lossy(&out.stderr).trim())))
        }
    }
}

/// Applies [`apply_to_gcode`] and writes the result file; needs no interpreter.
#[derive(Debug, Clone, Default)]
pub struct ReferenceRunner;

impl Runner for ReferenceRunner {
    fn run(&self, _script: &Path, gcode: &Path, request_path: &Path, result_path: &Path) -> Result<(), BridgeError> {
        let io = |p: &Path, e: std::io::Error| BridgeError::Io(format!("{}: {e}", p.display()));
        let req = std::fs::read_to_string(request_path).map_err(|e| io(request_path, e))?;
        let text = std::fs::read_to_string(gcode).map_err(|e| io(gcode, e))?;
        let (new_text, results) = apply_to_gcode(&text, &parse_request(&req)?);
        if new_text != text {
            std::fs::write(gcode, new_text).map_err(|e| io(gcode, e))?;
        }
        std::fs::write(result_path, result_json(&results)).map_err(|e| io(result_path, e))
    }
}

/// Write request and script into `dir`, run on `gcode`, and return the validated results.
pub fn execute(runner: &dyn Runner, dir: &Path, gcode: &Path, insertions: &[Insertion]) -> Result<Vec<PlacementResult>, BridgeError> {
    let req = dir.join("request.json");
    let res = dir.join("result.json");
    let scr = dir.join("bridge_script.py");
    // A stale result must never be mistaken for a fresh one.
    if res.exists() {
        std::fs::remove_file(&res).map_err(|e| BridgeError::Io(e.to_string()))?;
    }
    std::fs::write(&req, request_json(insertions)?).map_err(|e| BridgeError::Io(e.to_string()))?;
    std::fs::write(&scr, script(&req, &res)).map_err(|e| BridgeError::Io(e.to_string()))?;
    runner.run(&scr, gcode, &req, &res)?;
    read_result(&res, insertions.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ins(layer: u32, gcode: &str) -> Insertion {
        Insertion { layer, gcode: gcode.into() }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vernon_script_bridge_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const GCODE: &str = "; start\nG28\n;LAYER_CHANGE\n;Z:0.2\nG1 X1 Y1 E1\n;LAYER_CHANGE\n;Z:0.4\nG1 X2 Y2 E1\n;LAYER_CHANGE\n;Z:0.6\nG1 X3 Y3 E1\nM84\n";

    fn requests() -> Vec<Insertion> {
        vec![ins(2, "M117 layer 2\nM600"), ins(3, "M106 S255"), ins(9, "M0"), ins(2, "G4 P100")]
    }

    #[test]
    fn script_bridge_reference_inserts_before_the_layer_marker() {
        let (out, r) = apply_to_gcode(GCODE, &requests());
        let lines: Vec<&str> = out.lines().collect();
        let at = lines.iter().position(|l| *l == "; vernon: inserted before layer 2").unwrap();
        assert_eq!(lines[at + 1..at + 3], ["M117 layer 2", "M600"]);
        // The second insertion of the same layer follows the first and precedes the marker.
        assert_eq!(lines[at + 3..at + 6], ["; vernon: inserted before layer 2", "G4 P100", ";LAYER_CHANGE"]);
        assert_eq!(r.iter().map(|x| x.status).collect::<Vec<_>>(), [Status::Applied, Status::Applied, Status::Failed, Status::Applied]);
        assert_eq!(r[2].message, "layer 9 not found (file has 3 layers)");
        assert_eq!(*lines.last().unwrap(), "; vernon_post_process: applied=3 failed=1");
        assert!(out.ends_with('\n'), "a trailing newline is kept");
        // Every original line is still there, in order.
        let orig: Vec<&str> = GCODE.lines().collect();
        let kept: Vec<&str> = lines.iter().copied().filter(|l| orig.contains(l) && !l.starts_with("; vernon")).collect();
        assert_eq!(kept, orig);
    }

    #[test]
    fn script_bridge_reference_is_not_applied_twice() {
        let (once, _) = apply_to_gcode(GCODE, &requests());
        let (twice, r) = apply_to_gcode(&once, &requests());
        assert_eq!(once, twice);
        assert!(r.iter().all(|x| x.status == Status::Failed && x.message == "file was already processed"));
    }

    #[test]
    fn script_bridge_reference_keeps_windows_line_endings() {
        let crlf = GCODE.replace('\n', "\r\n");
        let (out, _) = apply_to_gcode(&crlf, &[ins(1, "M117 hi")]);
        assert!(out.lines().all(|l| !l.contains('\r')));
        assert_eq!(out.matches("\r\n").count(), out.matches('\n').count(), "no bare line feeds");
        assert!(out.contains("M117 hi\r\n"));
    }

    #[test]
    fn script_bridge_round_trip_with_reference_runner() {
        let dir = tmp("rt");
        let gcode = dir.join("job.gcode");
        std::fs::write(&gcode, GCODE).unwrap();
        let r = execute(&ReferenceRunner, &dir, &gcode, &requests()).unwrap();
        assert_eq!(r.len(), 4);
        assert_eq!((r[2].status, r[2].index), (Status::Failed, 2));
        let out = std::fs::read_to_string(&gcode).unwrap();
        assert!(out.contains("M600") && out.contains("; vernon_post_process: applied=3 failed=1"));
        let script = std::fs::read_to_string(dir.join("bridge_script.py")).unwrap();
        assert!(script.contains("request.json") && script.contains("sys.argv"));
    }

    /// First interpreter that answers `--version`, or None. The test below is skipped without one.
    fn python() -> Option<PathBuf> {
        ["python", "python3", "py"]
            .iter()
            .map(PathBuf::from)
            .find(|p| std::process::Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success()))
    }

    /// The generated script itself, run by an interpreter on a small G-code file, must give
    /// byte-for-byte what the Rust rules give. No slicer is involved.
    #[test]
    fn script_bridge_generated_script_matches_the_reference() {
        let Some(python) = python() else {
            eprintln!("skipped: no Python 3 interpreter on the search path");
            return;
        };
        for (name, text) in [("lf", GCODE.to_string()), ("crlf", GCODE.replace('\n', "\r\n")), ("nonl", GCODE.trim_end().to_string())] {
            let dir = tmp(&format!("py_{name}"));
            let want_dir = tmp(&format!("ref_{name}"));
            let (got, want) = (dir.join("job.gcode"), want_dir.join("job.gcode"));
            std::fs::write(&got, &text).unwrap();
            std::fs::write(&want, &text).unwrap();
            let a = execute(&PythonRunner { python: python.clone() }, &dir, &got, &requests()).unwrap();
            let b = execute(&ReferenceRunner, &want_dir, &want, &requests()).unwrap();
            assert_eq!(a, b, "{name}: results differ");
            assert_eq!(std::fs::read(&got).unwrap(), std::fs::read(&want).unwrap(), "{name}: G-code differs");
            // Running it again leaves the file alone and reports every insertion as failed.
            let again = execute(&PythonRunner { python: python.clone() }, &dir, &got, &requests()).unwrap();
            assert!(again.iter().all(|x| x.status == Status::Failed));
            assert_eq!(std::fs::read(&got).unwrap(), std::fs::read(&want).unwrap());
        }
    }

    #[test]
    fn script_bridge_request_round_trips() {
        let ps = vec![ins(4, "M600"), ins(7, "M117 a\nM117 b")];
        assert_eq!(parse_request(&request_json(&ps).unwrap()).unwrap(), ps);
    }

    #[test]
    fn script_bridge_rejects_bad_requests() {
        assert!(request_json(&[ins(0, "M600")]).is_err());
        assert!(request_json(&[ins(MAX_LAYER + 1, "M600")]).is_err());
        assert!(request_json(&[ins(1, "  ")]).is_err());
        assert!(request_json(&[ins(1, "M600\u{7}")]).is_err());
        assert!(request_json(&[ins(1, &"G".repeat(201))]).is_err());
        assert!(parse_request(r#"{"version":1,"insertions":[]}"#).is_err());
    }

    #[test]
    fn script_bridge_malformed_results_rejected() {
        let ok = r#"{"version":2,"results":[{"index":0,"status":"applied","message":""}]}"#;
        assert!(parse_result(ok, 1).is_ok());
        for text in [
            "not json",
            "[]",
            r#"{"version":1,"results":[]}"#,
            r#"{"version":2}"#,
            r#"{"version":2,"results":[]}"#,
            r#"{"version":2,"results":[{"index":1,"status":"applied","message":""}]}"#,
            r#"{"version":2,"results":[{"index":0,"status":"maybe","message":""}]}"#,
            r#"{"version":2,"results":[{"index":0,"status":"applied","message":5}]}"#,
            r#"{"version":2,"results":[{"index":0,"status":"applied","message":"","extra":1}]}"#,
            r#"{"version":2,"results":[],"extra":1}"#,
        ] {
            assert!(parse_result(text, 1).is_err(), "accepted: {text}");
        }
    }

    #[test]
    fn script_bridge_missing_file_rejected() {
        let dir = tmp("missing");
        let e = read_result(&dir.join("nope.json"), 0).unwrap_err();
        assert!(matches!(e, BridgeError::Io(_)));
    }

    #[test]
    fn script_bridge_runner_that_writes_nothing_is_an_error() {
        struct Lazy;
        impl Runner for Lazy {
            fn run(&self, _: &Path, _: &Path, _: &Path, _: &Path) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        let dir = tmp("lazy");
        // Even if an old result exists, `execute` removes it first.
        std::fs::write(dir.join("result.json"), result_json(&[])).unwrap();
        assert!(matches!(execute(&Lazy, &dir, &dir.join("x.gcode"), &[ins(1, "M600")]), Err(BridgeError::Io(_))));
    }
}
