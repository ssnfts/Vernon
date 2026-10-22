//! Json hand-off structures between the CFD nodes. All lengths here are metres
//! (OpenFOAM units); geometry on wires stays in millimetres.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use vernon_graph::{CookError, CookResult};

use crate::util::json_in;

/// Reference quantities of the vehicle, in the flow frame (nose at the lowest
/// x, flow toward +x, ground at z = 0, vehicle symmetric about y = 0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefGeom {
    /// Projected frontal area of the FULL car (body + wheels + kit), m2.
    pub a_front_m2: f64,
    pub length_m: f64,
    pub width_m: f64,
    pub height_m: f64,
    pub nose_x_m: f64,
    pub tail_x_m: f64,
    pub x_front_axle_m: f64,
    pub x_rear_axle_m: f64,
    pub track_m: f64,
    pub wheel_radius_m: f64,
    /// Lowest point (negative when the wheels are sunk below the ground).
    pub z_min_m: f64,
    pub half_model: bool,
    /// Patch (surface) names present in the tagged vehicle.
    pub patches: Vec<String>,
    /// Wheel centres in metres, keyed by patch name.
    pub wheel_centres_m: IndexMap<String, [f64; 3]>,
    /// Bounding box of the kit parts (min, max), metres.
    pub kit_bbox_m: Option<[[f64; 3]; 2]>,
}

impl RefGeom {
    /// A road-car sized reference (4.6 m, 2.16 m2) used when nothing is wired.
    pub fn reference_car() -> Self {
        let r = 0.3137;
        let (xf, xr, track) = (0.92, 3.706, 1.57);
        let mut wheel_centres_m = IndexMap::new();
        for (n, x, y) in [
            ("wheel_fl", xf, -track / 2.0),
            ("wheel_fr", xf, track / 2.0),
            ("wheel_rl", xr, -track / 2.0),
            ("wheel_rr", xr, track / 2.0),
        ] {
            wheel_centres_m.insert(n.to_string(), [x, y, r]);
        }
        Self {
            a_front_m2: 2.16,
            length_m: 4.6,
            width_m: 1.85,
            height_m: 1.43,
            nose_x_m: 0.0,
            tail_x_m: 4.6,
            x_front_axle_m: xf,
            x_rear_axle_m: xr,
            track_m: track,
            wheel_radius_m: r,
            z_min_m: 0.0,
            half_model: true,
            patches: vec![
                "body".into(),
                "wheel_fl".into(),
                "wheel_fr".into(),
                "wheel_rl".into(),
                "wheel_rr".into(),
            ],
            wheel_centres_m,
            kit_bbox_m: None,
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_json(v: &Value) -> CookResult<Self> {
        serde_json::from_value(v.clone()).map_err(|e| CookError::Other(format!("not a vehicle reference: {e}")))
    }

    /// Mid-wheelbase x, the default centre of rotation for moments.
    pub fn mid_wheelbase_x(&self) -> f64 {
        0.5 * (self.x_front_axle_m + self.x_rear_axle_m)
    }

    /// Reference area for force coefficients (half of the full car when the
    /// forces only integrate the y >= 0 half).
    pub fn aref_m2(&self, half_model: bool) -> f64 {
        if half_model {
            self.a_front_m2 * 0.5
        } else {
            self.a_front_m2
        }
    }
}

/// Reference input at `idx`, falling back to [`RefGeom::reference_car`].
pub(crate) fn ref_in(inputs: &[Option<vernon_graph::Data>], idx: usize) -> CookResult<(RefGeom, bool)> {
    match json_in(inputs, idx)? {
        Some(v) => Ok((RefGeom::from_json(&v)?, true)),
        None => Ok((RefGeom::reference_car(), false)),
    }
}

/// Virtual-tunnel domain (metres).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Domain {
    pub min_m: [f64; 3],
    pub max_m: [f64; 3],
    pub half_model: bool,
    pub yaw_deg: f64,
    pub blockage_pct: f64,
    pub cross_section_m2: f64,
    pub ref_length_m: f64,
    pub up_l: f64,
    pub down_l: f64,
}

impl Domain {
    pub fn size(&self) -> [f64; 3] {
        [
            self.max_m[0] - self.min_m[0],
            self.max_m[1] - self.min_m[1],
            self.max_m[2] - self.min_m[2],
        ]
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_json(v: &Value) -> CookResult<Self> {
        serde_json::from_value(v.clone()).map_err(|e| CookError::Other(format!("not a tunnel domain: {e}")))
    }

    /// Patch names of the background mesh.
    pub fn patch_names(&self) -> Vec<&'static str> {
        if self.half_model {
            vec!["inlet", "outlet", "top", "side", "ground", "symmetry"]
        } else {
            vec!["inlet", "outlet", "top", "side", "ground"]
        }
    }
}

/// Domain input at `idx` or a default sized from the reference geometry.
pub(crate) fn domain_in(
    inputs: &[Option<vernon_graph::Data>],
    idx: usize,
    rf: &RefGeom,
) -> CookResult<Domain> {
    match json_in(inputs, idx)? {
        Some(v) => Domain::from_json(&v),
        None => Ok(crate::tunnel::size_domain(rf, 1.0, 3.0, 6.0, 5.0 / 3.0, rf.half_model, 0.0)),
    }
}

/// One wheel boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WheelBc {
    pub name: String,
    pub centre_m: [f64; 3],
}

/// Ground + wheel treatment (output of `cfd.ground_wheels`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BcSet {
    pub ground_mode: String,
    pub wheel_mode: String,
    pub u_ms: f64,
    /// Signed angular speed about `axis` giving a rolling tyre, rad/s.
    pub omega_rad_s: f64,
    pub r_dyn_m: f64,
    pub axis: [f64; 3],
    pub wheel_width_m: f64,
    pub belt_start_x_m: Option<f64>,
    pub wheels: Vec<WheelBc>,
}

impl BcSet {
    pub fn default_moving(u: f64) -> Self {
        Self {
            ground_mode: "moving".into(),
            wheel_mode: "rotating".into(),
            u_ms: u,
            omega_rad_s: 0.0,
            r_dyn_m: 0.0,
            axis: [0.0, 1.0, 0.0],
            wheel_width_m: 0.25,
            belt_start_x_m: None,
            wheels: vec![],
        }
    }

    pub fn from_json(v: &Value) -> CookResult<Self> {
        serde_json::from_value(v.clone()).map_err(|e| CookError::Other(format!("not a ground/wheel set: {e}")))
    }
}

/// Darcy-Forchheimer porous zone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PorousZone {
    pub name: String,
    pub bbox_m: Option<[[f64; 3]; 2]>,
    pub flow_dir: [f64; 3],
    /// Viscous coefficients in the zone frame (flow, lateral, lateral), 1/m2.
    pub d: [f64; 3],
    /// Inertial coefficients, 1/m.
    pub f: [f64; 3],
}

/// Time series table: one `time` column and named columns of equal length.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub time: Vec<f64>,
    pub columns: IndexMap<String, Vec<f64>>,
    /// Header constants (Aref, lRef, magUInf...).
    pub meta: IndexMap<String, f64>,
    /// True for the empty series a white-box node emits without solver data.
    pub placeholder: bool,
}

impl Series {
    pub fn len(&self) -> usize {
        self.time.len()
    }

    pub fn is_empty(&self) -> bool {
        self.time.is_empty()
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn from_json(v: &Value) -> CookResult<Self> {
        let s: Series =
            serde_json::from_value(v.clone()).map_err(|e| CookError::Other(format!("not a time series: {e}")))?;
        for (k, c) in &s.columns {
            if c.len() != s.time.len() {
                return Err(CookError::Other(format!(
                    "series column '{k}' has {} rows, time has {}",
                    c.len(),
                    s.time.len()
                )));
            }
        }
        Ok(s)
    }

    pub fn col(&self, name: &str) -> Option<&[f64]> {
        self.columns.get(name).map(Vec::as_slice)
    }
}
