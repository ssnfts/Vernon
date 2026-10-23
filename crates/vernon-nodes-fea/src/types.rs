//! Plain-data hand-off types that travel between the structural nodes as
//! `Data::Json`, plus helpers to read them from node inputs.
//!
//! Units: mm, N, tonne, s, MPa (see `docs/specs/structural-fea.md` section 1).
//! Node numbers are 0-based point indices of the solid-mesh `Geometry`; the
//! deck uses index + 1.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use vernon_core::{Metric, Report, Severity};
use vernon_graph::{CookError, CookResult, Data};

/// Pressure on one element face. Positive pushes into the body (solver
/// convention), negative is suction.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FacePressure {
    pub elem: u32,
    /// Solver face label 1..4.
    pub face: u8,
    pub p_mpa: f64,
}

/// A concentrated force on one node (N).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NodalForce {
    pub node: u32,
    pub f_n: [f64; 3],
}

/// Body acceleration field applied to every element (mm/s2).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Gravity {
    pub accel_mm_s2: f64,
    /// Unit direction of the acceleration field.
    pub dir: [f64; 3],
}

/// Everything applied in one load case.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadSet {
    pub name: String,
    pub pressures: Vec<FacePressure>,
    pub forces: Vec<NodalForce>,
    pub gravity: Option<Gravity>,
    /// Resultant of everything applied (N), used by the equilibrium gate.
    pub resultant_n: [f64; 3],
    pub notes: Vec<String>,
}

impl LoadSet {
    pub fn is_empty(&self) -> bool {
        self.pressures.is_empty() && self.forces.is_empty() && self.gravity.is_none()
    }

    /// Scale the surface loads (pressure and nodal forces) by `k`; the
    /// body acceleration is left alone. Used for speed sweeps (k = (v/v0)^2).
    pub fn scale_surface(&self, k: f64) -> LoadSet {
        let mut s = self.clone();
        for p in &mut s.pressures {
            p.p_mpa *= k;
        }
        for f in &mut s.forces {
            for c in &mut f.f_n {
                *c *= k;
            }
        }
        // Without a body acceleration the resultant is purely a surface load and scales too.
        if s.gravity.is_none() {
            for c in &mut s.resultant_n {
                *c *= k;
            }
        }
        s
    }

    /// Append another load set (same mesh). Gravity fields add as vectors.
    pub fn merge(&mut self, other: &LoadSet) {
        self.pressures.extend_from_slice(&other.pressures);
        self.forces.extend_from_slice(&other.forces);
        self.gravity = match (self.gravity, other.gravity) {
            (Some(a), Some(b)) => {
                let v = glam::DVec3::from(a.dir) * a.accel_mm_s2 + glam::DVec3::from(b.dir) * b.accel_mm_s2;
                let m = v.length();
                Some(Gravity { accel_mm_s2: m, dir: if m > 0.0 { (v / m).into() } else { [0.0, 0.0, -1.0] } })
            }
            (a, b) => a.or(b),
        };
        for i in 0..3 {
            self.resultant_n[i] += other.resultant_n[i];
        }
        self.notes.extend(other.notes.iter().cloned());
    }
}

/// One named load case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadCase {
    pub name: String,
    /// Speed relative to the design speed (aero pressure scales with its square).
    pub speed_factor: f64,
    pub loads: LoadSet,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CaseSet {
    pub cases: Vec<LoadCase>,
}

/// One bolted attachment of a printed part.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MountSite {
    pub name: String,
    pub center: [f64; 3],
    /// Unit bolt axis.
    pub axis: [f64; 3],
    /// Surface node indices of the contact patch (0-based).
    pub nodes: Vec<u32>,
    /// `fixed_patch`, `rigid_washer` or `spring_pad`.
    pub model: String,
    pub r_w_mm: f64,
    pub avg_radius_mm: f64,
    pub bolt_d_mm: f64,
    pub pad_e_mpa: f64,
    pub pad_t_mm: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MountSet {
    pub sites: Vec<MountSite>,
}

/// Material frame of one printed part: rows of `axes` are the material axes
/// 1, 2, 3 (3 = layer stacking) expressed in the model frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrientationCard {
    pub name: String,
    pub axes: [[f64; 3]; 3],
    pub origin: [f64; 3],
    pub rot_deg: [f64; 3],
}

impl OrientationCard {
    pub fn identity() -> Self {
        Self { name: "OR1".into(), axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], origin: [0.0; 3], rot_deg: [0.0; 3] }
    }

    /// Points `a` (on material axis 1) and `b` (in the 1-2 plane) as written on the card.
    pub fn ab(&self) -> ([f64; 3], [f64; 3]) {
        let o = self.origin;
        let a = [o[0] + self.axes[0][0], o[1] + self.axes[0][1], o[2] + self.axes[0][2]];
        let b = [o[0] + self.axes[1][0], o[1] + self.axes[1][1], o[2] + self.axes[1][2]];
        (a, b)
    }
}

fn one() -> f64 {
    1.0
}

/// One deck written for one load case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeckEntry {
    pub case: String,
    pub job: String,
    pub file: String,
    pub hash: String,
    /// Speed relative to the design speed of the case (1 = design speed).
    #[serde(default = "one")]
    pub speed_factor: f64,
}

/// What `fea.deck_write` tells the later nodes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeckManifest {
    pub dir: String,
    pub decks: Vec<DeckEntry>,
    pub element_type: String,
    pub units: String,
    pub node_count: usize,
    pub element_count: usize,
    /// Node-set names of the mounts, in deck order.
    pub mount_sets: Vec<String>,
}

/// One solver run (or one reference-solver run) of one load case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEntry {
    pub case: String,
    pub job: String,
    /// `solved`, `reused`, `not_run`, `solver_missing`.
    pub status: String,
    /// Result files relative to `SolveManifest::dir` (empty when absent).
    pub frd: String,
    pub dat: String,
    pub log: String,
    pub hash: String,
    #[serde(default = "one")]
    pub speed_factor: f64,
}

/// What `fea.solve_external` and `fea.solve_linear` tell `fea.result_read`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SolveManifest {
    pub dir: String,
    /// `external` (the open-source solver) or `reference_linear` (the built-in white-box solver).
    pub solver: String,
    pub element_type: String,
    pub mount_sets: Vec<String>,
    pub runs: Vec<RunEntry>,
}

/// Totals of one load case read from the result files.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CaseResult {
    pub name: String,
    pub speed_factor: f64,
    pub summary: ResultSummary,
}

/// Everything `fea.result_read` found; the field data itself travels as
/// attributes on the solid mesh (`disp_<case>`, `sig_n_<case>`, `sig_s_<case>`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResultSet {
    /// `external` or `reference_linear`.
    pub source: String,
    /// Frame of the stored stresses: `global` or `material`.
    pub frame: String,
    pub cases: Vec<CaseResult>,
}

/// Totals read back from the solver text output.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResultSummary {
    /// Reaction force totals per node set (N).
    pub rf_totals: std::collections::BTreeMap<String, [f64; 3]>,
    pub max_disp_mm: f64,
    pub max_vm_mpa: f64,
    pub volume_mm3: Option<f64>,
    pub time: f64,
}

/// Serialise a value as `Data::Json`.
pub fn to_data<T: Serialize>(v: &T) -> Data {
    Data::json(serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
}

fn json_of(inputs: &[Option<Data>], index: usize) -> Option<&serde_json::Value> {
    inputs.get(index).and_then(|d| d.as_ref()).and_then(|d| d.as_json()).map(|a| a.as_ref())
}

/// Read a required JSON input as `T`.
pub fn from_data<T: DeserializeOwned>(inputs: &[Option<Data>], index: usize, what: &str) -> CookResult<T> {
    let v = match inputs.get(index).and_then(|d| d.as_ref()) {
        Some(d) => d.as_json().ok_or(CookError::WrongInput { index, expected: vernon_graph::DataKind::Json, found: d.kind() })?,
        None => return Err(CookError::MissingInput { index, name: what.into() }),
    };
    serde_json::from_value((**v).clone()).map_err(|e| CookError::Other(format!("input {index} is not a valid {what}: {e}")))
}

/// Read an optional JSON input as `T` (an unconnected port gives `None`).
pub fn opt_from_data<T: DeserializeOwned>(inputs: &[Option<Data>], index: usize, what: &str) -> CookResult<Option<T>> {
    match json_of(inputs, index) {
        None => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| CookError::Other(format!("input {index} is not a valid {what}: {e}"))),
    }
}

/// A report metric with a pass limit where larger is better.
pub fn metric_min(value: f64, unit: &str, limit: f64) -> Metric {
    Metric::new(finite(value), unit).with_limit(limit, false)
}

/// A report metric with a pass limit where smaller is better.
pub fn metric_max(value: f64, unit: &str, limit: f64) -> Metric {
    Metric::new(finite(value), unit).with_limit(limit, true)
}

/// Non-finite numbers would turn into `null` in JSON; keep reports finite.
pub fn finite(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else if v.is_sign_negative() {
        -1e300
    } else {
        1e300
    }
}

/// Start a report for a white-box placeholder: a warning note plus a failing
/// `placeholder` metric so the result cannot pass a gate silently.
pub fn placeholder_report(title: &str, what: &str) -> Report {
    let mut r = Report::new(title);
    r.note(Severity::Warning, format!("white-box placeholder: {what}"));
    r.metric("placeholder", Metric::new(1.0, "flag").with_limit(0.0, true));
    r
}

/// FNV-1a 64-bit hash as 16 hex digits (stable across builds, unlike `DefaultHasher`).
pub fn fnv_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_stable() {
        assert_eq!(fnv_hex(b""), "cbf29ce484222325");
        assert_eq!(fnv_hex(b"a"), "af63dc4c8601ec8c");
    }

    #[test]
    fn json_round_trip_and_missing_input() {
        let ls = LoadSet { name: "x".into(), resultant_n: [1.0, 2.0, 3.0], ..Default::default() };
        let d = to_data(&ls);
        let back: LoadSet = from_data(&[Some(d)], 0, "load set").unwrap();
        assert_eq!(back, ls);
        assert!(from_data::<LoadSet>(&[None], 0, "load set").is_err());
        assert!(opt_from_data::<LoadSet>(&[None], 0, "load set").unwrap().is_none());
    }

    #[test]
    fn merge_adds_gravity_vectors() {
        let mut a = LoadSet { gravity: Some(Gravity { accel_mm_s2: 9810.0, dir: [0.0, 0.0, -1.0] }), ..Default::default() };
        let b = LoadSet { gravity: Some(Gravity { accel_mm_s2: 4.0 * 9810.0, dir: [0.0, 0.0, -1.0] }), ..Default::default() };
        a.merge(&b);
        let g = a.gravity.unwrap();
        assert!((g.accel_mm_s2 - 5.0 * 9810.0).abs() < 1e-6 && g.dir[2] == -1.0);
    }
}
