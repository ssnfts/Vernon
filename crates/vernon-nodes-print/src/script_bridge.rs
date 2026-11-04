//! Request/result harness for driving a slicer plug-in script.
//!
//! Flow: `request_json` lists connector placements; `script` renders the plug-in script
//! (text in `script_template.rs`); a [`Runner`] executes it; `parse_result` strictly
//! validates the result file. Tests use [`StubRunner`], so no slicer is ever needed.
use crate::script_template::{self, PROTOCOL_VERSION};
use serde_json::{json, Value};
use std::fmt;
use std::path::Path;

/// Connector kinds the request may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Dowel,
    Plug,
    Snap,
    Dovetail,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Dowel => "dowel",
            Kind::Plug => "plug",
            Kind::Snap => "snap",
            Kind::Dovetail => "dovetail",
        }
    }
    fn parse(s: &str) -> Option<Kind> {
        [Kind::Dowel, Kind::Plug, Kind::Snap, Kind::Dovetail].into_iter().find(|k| k.as_str() == s)
    }
}

/// One connector to place (millimetres, Z up).
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub position: [f64; 3],
    pub normal: [f64; 3],
    pub kind: Kind,
    pub diameter_mm: f64,
    pub depth_mm: f64,
}

/// Outcome of one placement.
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

fn finite3(v: [f64; 3]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// Serialise and validate a request. Normals must be non-zero, sizes positive.
pub fn request_json(placements: &[Placement]) -> Result<String, BridgeError> {
    let mut items = Vec::with_capacity(placements.len());
    for (i, p) in placements.iter().enumerate() {
        let n2: f64 = p.normal.iter().map(|x| x * x).sum();
        if !finite3(p.position) || !finite3(p.normal) || n2 < 1e-12 {
            return bad(format!("connector {i}: position/normal must be finite and normal non-zero"));
        }
        if !(p.diameter_mm.is_finite() && p.diameter_mm > 0.0 && p.depth_mm.is_finite() && p.depth_mm > 0.0) {
            return bad(format!("connector {i}: diameter and depth must be positive"));
        }
        items.push(json!({
            "kind": p.kind.as_str(),
            "position": p.position,
            "normal": p.normal,
            "diameter_mm": p.diameter_mm,
            "depth_mm": p.depth_mm,
        }));
    }
    Ok(json!({ "version": PROTOCOL_VERSION, "connectors": items }).to_string())
}

fn arr3(v: &Value, what: &str) -> Result<[f64; 3], BridgeError> {
    let a = v.as_array().filter(|a| a.len() == 3);
    let a = match a {
        Some(a) => a,
        None => return bad(format!("{what} must be an array of 3 numbers")),
    };
    let mut out = [0.0; 3];
    for (o, x) in out.iter_mut().zip(a) {
        *o = match x.as_f64() {
            Some(f) => f,
            None => return bad(format!("{what} has a non-number")),
        };
    }
    Ok(out)
}

/// Parse a request produced by [`request_json`] (used by runners).
pub fn parse_request(text: &str) -> Result<Vec<Placement>, BridgeError> {
    let v: Value = serde_json::from_str(text).map_err(|e| BridgeError::Invalid(e.to_string()))?;
    if v.get("version").and_then(Value::as_u64) != Some(PROTOCOL_VERSION as u64) {
        return bad("unsupported request version");
    }
    let list = match v.get("connectors").and_then(Value::as_array) {
        Some(l) => l,
        None => return bad("missing `connectors` array"),
    };
    let mut out = Vec::new();
    for (i, c) in list.iter().enumerate() {
        let kind = c.get("kind").and_then(Value::as_str).and_then(Kind::parse);
        let (Some(kind), Some(d), Some(h)) = (
            kind,
            c.get("diameter_mm").and_then(Value::as_f64),
            c.get("depth_mm").and_then(Value::as_f64),
        ) else {
            return bad(format!("connector {i}: bad kind, diameter or depth"));
        };
        out.push(Placement {
            position: arr3(c.get("position").unwrap_or(&Value::Null), "position")?,
            normal: arr3(c.get("normal").unwrap_or(&Value::Null), "normal")?,
            kind,
            diameter_mm: d,
            depth_mm: h,
        });
    }
    Ok(out)
}

/// Plug-in script text for the given request/result file paths.
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

/// Strictly validate result text against a request of `expected` connectors:
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

/// Executes the script. Real implementations spawn the slicer; tests use [`StubRunner`].
pub trait Runner {
    /// Run `script_path`, which must leave a result file at `result_path`.
    fn run(&self, script_path: &Path, request_path: &Path, result_path: &Path) -> Result<(), BridgeError>;
}

/// Test double: reads the request file and writes a result without any slicer.
/// Connectors whose depth exceeds `max_depth_mm` are reported `failed`.
#[derive(Debug, Clone)]
pub struct StubRunner {
    pub max_depth_mm: f64,
}

impl Runner for StubRunner {
    fn run(&self, _script: &Path, request_path: &Path, result_path: &Path) -> Result<(), BridgeError> {
        let text = std::fs::read_to_string(request_path)
            .map_err(|e| BridgeError::Io(format!("{}: {e}", request_path.display())))?;
        let results: Vec<PlacementResult> = parse_request(&text)?
            .iter()
            .enumerate()
            .map(|(index, p)| {
                if p.depth_mm > self.max_depth_mm {
                    PlacementResult { index, status: Status::Failed, message: "depth too large".into() }
                } else {
                    PlacementResult { index, status: Status::Applied, message: String::new() }
                }
            })
            .collect();
        std::fs::write(result_path, result_json(&results)).map_err(|e| BridgeError::Io(e.to_string()))
    }
}

/// Write request and script into `dir`, run, and return the validated results.
pub fn execute(runner: &dyn Runner, dir: &Path, placements: &[Placement]) -> Result<Vec<PlacementResult>, BridgeError> {
    let req = dir.join("request.json");
    let res = dir.join("result.json");
    let scr = dir.join("bridge_script.py");
    // A stale result must never be mistaken for a fresh one.
    if res.exists() {
        std::fs::remove_file(&res).map_err(|e| BridgeError::Io(e.to_string()))?;
    }
    std::fs::write(&req, request_json(placements)?).map_err(|e| BridgeError::Io(e.to_string()))?;
    std::fs::write(&scr, script(&req, &res)).map_err(|e| BridgeError::Io(e.to_string()))?;
    runner.run(&scr, &req, &res)?;
    read_result(&res, placements.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(kind: Kind, depth: f64) -> Placement {
        Placement { position: [1.0, 2.0, 3.0], normal: [0.0, 0.0, 1.0], kind, diameter_mm: 5.0, depth_mm: depth }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("vernon_script_bridge_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn script_bridge_round_trip_with_stub() {
        let dir = tmp("rt");
        let ps = [p(Kind::Dowel, 8.0), p(Kind::Snap, 30.0), p(Kind::Plug, 2.0)];
        let r = execute(&StubRunner { max_depth_mm: 10.0 }, &dir, &ps).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].status, Status::Applied);
        assert_eq!((r[1].status, r[1].message.as_str()), (Status::Failed, "depth too large"));
        assert_eq!(r[2].index, 2);
        let script = std::fs::read_to_string(dir.join("bridge_script.py")).unwrap();
        assert!(script.contains("request.json") && script.contains("add_connector"));
    }

    #[test]
    fn script_bridge_request_round_trips() {
        let ps = vec![p(Kind::Dovetail, 4.5)];
        assert_eq!(parse_request(&request_json(&ps).unwrap()).unwrap(), ps);
    }

    #[test]
    fn script_bridge_rejects_bad_requests() {
        let mut bad_n = p(Kind::Plug, 1.0);
        bad_n.normal = [0.0; 3];
        assert!(request_json(&[bad_n]).is_err());
        assert!(request_json(&[p(Kind::Plug, 0.0)]).is_err());
        let mut nan = p(Kind::Plug, 1.0);
        nan.position[0] = f64::NAN;
        assert!(request_json(&[nan]).is_err());
    }

    #[test]
    fn script_bridge_malformed_results_rejected() {
        let ok = r#"{"version":1,"results":[{"index":0,"status":"applied","message":""}]}"#;
        assert!(parse_result(ok, 1).is_ok());
        for text in [
            "not json",
            "[]",
            r#"{"version":2,"results":[]}"#,
            r#"{"version":1}"#,
            r#"{"version":1,"results":[]}"#,
            r#"{"version":1,"results":[{"index":1,"status":"applied","message":""}]}"#,
            r#"{"version":1,"results":[{"index":0,"status":"maybe","message":""}]}"#,
            r#"{"version":1,"results":[{"index":0,"status":"applied","message":5}]}"#,
            r#"{"version":1,"results":[{"index":0,"status":"applied","message":"","extra":1}]}"#,
            r#"{"version":1,"results":[],"extra":1}"#,
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
            fn run(&self, _: &Path, _: &Path, _: &Path) -> Result<(), BridgeError> {
                Ok(())
            }
        }
        let dir = tmp("lazy");
        // Even if an old result exists, `execute` removes it first.
        std::fs::write(dir.join("result.json"), result_json(&[])).unwrap();
        assert!(matches!(execute(&Lazy, &dir, &[p(Kind::Snap, 1.0)]), Err(BridgeError::Io(_))));
    }
}
