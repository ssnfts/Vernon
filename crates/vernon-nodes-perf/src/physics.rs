//! Closed-form vehicle physics for stage 8: air density, aero loads, tyre grip with
//! the friction ellipse, drive force, corner speed limit and top speed.
//!
//! SI units throughout (m, s, kg, N, Pa, W). `cla_m2` is positive for downforce.
//! Formulas follow `docs/specs/performance-validation.md` sections 2.3 and 3.1-3.4.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Standard gravity, m/s2.
pub const G: f64 = vernon_core::units::STANDARD_GRAVITY_M_S2;
/// Gas constants and the saturation pressure live in `vernon_core::air`, shared
/// with the airflow stage so both stages compute the same density.
pub use vernon_core::air::{sat_pressure_pa, R_DRY, R_VAPOUR};

/// Moist-air density from temperature (C), pressure (Pa), relative humidity (0..1).
pub fn air_density(theta_c: f64, p_pa: f64, rh: f64, r_dry: f64) -> f64 {
    vernon_core::air::moist_air_density(theta_c, p_pa, rh, r_dry)
}

/// Dynamic pressure, Pa.
pub fn dyn_pressure(rho: f64, v: f64) -> f64 {
    0.5 * rho * v * v
}

/// Aero power `1/2 rho CdA v^3`, W.
pub fn aero_power(rho: f64, cda: f64, v: f64) -> f64 {
    0.5 * rho * cda * v * v * v
}

/// Braking deceleration (aero formula of the toolchain note, no rolling term), m/s2.
pub fn braking_decel(mu: f64, rho: f64, cda: f64, cla: f64, mass: f64, v: f64) -> f64 {
    mu * (G + 0.5 * rho * cla * v * v / mass) + 0.5 * rho * cda * v * v / mass
}

/// Steady corner speed `sqrt(mu g R / (1 - mu rho ClA R / 2m))`; infinite when downforce
/// outgrows the weight term (no speed limit).
pub fn corner_speed(mu: f64, rho: f64, cla: f64, mass: f64, radius: f64) -> f64 {
    let k = mu * rho * cla * radius / (2.0 * mass);
    if k >= 1.0 {
        f64::INFINITY
    } else {
        (mu * G * radius / (1.0 - k)).sqrt()
    }
}

/// Front aero balance in percent from the front and rear lift magnitudes.
pub fn aero_balance_pct(l_front: f64, l_rear: f64) -> f64 {
    let t = l_front + l_rear;
    if t.abs() < 1e-12 {
        50.0
    } else {
        100.0 * l_front / t
    }
}

/// Duct mass flow `rho V A`, kg/s.
pub fn duct_mass_flow(rho: f64, v: f64, area: f64) -> f64 {
    rho * v * area
}

/// Air state handed between nodes (`env.v1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Env {
    pub rho: f64,
    pub t_c: f64,
    pub p_pa: f64,
    pub rh: f64,
}

impl Env {
    pub fn from_state(t_c: f64, p_pa: f64, rh: f64, r_dry: f64) -> Self {
        Self::from_core(vernon_core::air::AirState::from_state(t_c, p_pa, rh, r_dry))
    }

    /// An environment known only by its density.
    pub fn from_rho(rho: f64) -> Self {
        Self::from_core(vernon_core::air::AirState::from_rho(rho))
    }

    fn from_core(a: vernon_core::air::AirState) -> Self {
        Self { rho: a.rho, t_c: a.t_c, p_pa: a.p_pa, rh: a.rh }
    }

    /// The shared air-state fields (`vernon_core::air`) plus `kind` and the
    /// older `T_c` spelling of the temperature.
    pub fn to_json(&self) -> Value {
        let a = vernon_core::air::AirState { rho: self.rho, t_c: self.t_c, p_pa: self.p_pa, rh: self.rh };
        let mut m = serde_json::Map::new();
        m.insert("kind".into(), json!("env.v1"));
        for (k, v) in a.fields() {
            m.insert(k.into(), json!(v));
        }
        m.insert("T_c".into(), json!(self.t_c));
        Value::Object(m)
    }

    /// Reads `env.v1` and also the airflow stage's air state (`t_c`,
    /// `rh_pct` in percent), so either can feed the performance nodes.
    pub fn from_json(v: &Value) -> Result<Self, String> {
        vernon_core::air::AirState::from_lookup(|k| v.get(k).and_then(Value::as_f64))
            .map(Self::from_core)
            .map_err(|e| e.replacen("air state", "env", 1))
    }
}

/// Vehicle description (`vehicle.v1`). Defaults are Vernon-chosen, see spec section 2.1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vehicle {
    pub mass_kg: f64,
    pub rot_inertia_frac: f64,
    pub power_w: f64,
    pub power_density_exponent: f64,
    pub rho_ref: f64,
    /// Optional engine torque table, `[rpm, N m]` sorted by rpm.
    pub engine_curve: Vec<[f64; 2]>,
    pub gear_ratios: Vec<f64>,
    pub final_drive: f64,
    pub wheel_radius_m: f64,
    pub driveline_eff: f64,
    pub f_drive_max_n: f64,
    pub cda_m2: f64,
    /// Positive = downforce.
    pub cla_m2: f64,
    /// Additive calibration at the reference setting; never mutates the CFD numbers.
    pub cda_offset_m2: f64,
    pub mu_x: f64,
    pub mu_y: f64,
    pub mu_drop_per_kn: f64,
    pub crr: f64,
    pub rr_includes_downforce: bool,
    pub brake_max_g: f64,
    pub v_cap_ms: f64,
    pub aero_balance_front: f64,
}

impl Default for Vehicle {
    fn default() -> Self {
        Self {
            mass_kg: 1400.0,
            rot_inertia_frac: 0.0,
            power_w: 150_000.0,
            power_density_exponent: 0.0,
            rho_ref: 1.225,
            engine_curve: Vec::new(),
            gear_ratios: Vec::new(),
            final_drive: 3.5,
            wheel_radius_m: 0.32,
            driveline_eff: 0.92,
            f_drive_max_n: 0.0,
            cda_m2: 0.64,
            cla_m2: 0.0,
            cda_offset_m2: 0.0,
            mu_x: 1.0,
            mu_y: 1.0,
            mu_drop_per_kn: 0.0,
            crr: 0.012,
            rr_includes_downforce: true,
            brake_max_g: 0.0,
            v_cap_ms: 120.0,
            aero_balance_front: 0.5,
        }
    }
}

/// Result of a drive-force evaluation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drive {
    pub force: f64,
    /// 1-based gear, 0 when the simple power model is used.
    pub gear: u32,
    pub rpm: f64,
}

/// Forces and grip limits at one operating point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grip {
    pub drag: f64,
    pub down: f64,
    pub normal: f64,
    pub rolling: f64,
    /// Lateral capacity `mu_y N`.
    pub fy_cap: f64,
    /// Longitudinal capacity left by the friction ellipse.
    pub fx_max: f64,
}

impl Vehicle {
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let veh: Vehicle = serde_json::from_value(v.clone()).map_err(|e| format!("vehicle: {e}"))?;
        veh.validate()?;
        Ok(veh)
    }

    pub fn to_json(&self) -> Value {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Value::Object(m) = &mut v {
            m.insert("kind".into(), json!("vehicle.v1"));
        }
        v
    }

    pub fn validate(&self) -> Result<(), String> {
        let pos = |name: &str, x: f64| -> Result<(), String> {
            if x.is_finite() && x > 0.0 {
                Ok(())
            } else {
                Err(format!("vehicle: {name} must be positive, got {x}"))
            }
        };
        let nonneg = |name: &str, x: f64| -> Result<(), String> {
            if x.is_finite() && x >= 0.0 {
                Ok(())
            } else {
                Err(format!("vehicle: {name} must be zero or positive, got {x}"))
            }
        };
        pos("mass_kg", self.mass_kg)?;
        pos("mu_x", self.mu_x)?;
        pos("mu_y", self.mu_y)?;
        pos("v_cap_ms", self.v_cap_ms)?;
        pos("rho_ref", self.rho_ref)?;
        nonneg("power_w", self.power_w)?;
        nonneg("cda_m2", self.cda_m2)?;
        nonneg("crr", self.crr)?;
        nonneg("rot_inertia_frac", self.rot_inertia_frac)?;
        nonneg("mu_drop_per_kn", self.mu_drop_per_kn)?;
        nonneg("brake_max_g", self.brake_max_g)?;
        nonneg("f_drive_max_n", self.f_drive_max_n)?;
        if !(self.cla_m2.is_finite() && self.cda_offset_m2.is_finite()) {
            return Err("vehicle: cla_m2 / cda_offset_m2 is not finite".into());
        }
        if self.cda_eff() < 0.0 {
            return Err("vehicle: cda_m2 + cda_offset_m2 is negative".into());
        }
        if self.is_geared() {
            pos("wheel_radius_m", self.wheel_radius_m)?;
            pos("final_drive", self.final_drive)?;
            pos("driveline_eff", self.driveline_eff)?;
            if self.engine_curve.len() < 2 {
                return Err("vehicle: a geared drivetrain needs at least two engine_curve points".into());
            }
            if self.gear_ratios.iter().any(|g| !(g.is_finite() && *g > 0.0)) {
                return Err("vehicle: gear ratios must be positive".into());
            }
            if self.engine_curve.windows(2).any(|w| w[1][0] <= w[0][0]) {
                return Err("vehicle: engine_curve rpm values must increase".into());
            }
        }
        Ok(())
    }

    pub fn is_geared(&self) -> bool {
        !self.gear_ratios.is_empty() && !self.engine_curve.is_empty()
    }

    pub fn weight(&self) -> f64 {
        self.mass_kg * G
    }

    /// Effective mass for acceleration and braking.
    pub fn m_eff(&self) -> f64 {
        self.mass_kg * (1.0 + self.rot_inertia_frac)
    }

    pub fn cda_eff(&self) -> f64 {
        self.cda_m2 + self.cda_offset_m2
    }

    pub fn drag(&self, rho: f64, v: f64) -> f64 {
        0.5 * rho * self.cda_eff() * v * v
    }

    pub fn downforce(&self, rho: f64, v: f64) -> f64 {
        0.5 * rho * self.cla_m2 * v * v
    }

    pub fn normal_load(&self, rho: f64, v: f64) -> f64 {
        (self.weight() + self.downforce(rho, v)).max(0.0)
    }

    pub fn rolling(&self, rho: f64, v: f64) -> f64 {
        if self.rr_includes_downforce {
            self.crr * self.normal_load(rho, v)
        } else {
            self.crr * self.weight()
        }
    }

    fn mu_at(&self, mu0: f64, n: f64) -> f64 {
        if self.mu_drop_per_kn > 0.0 {
            (mu0 - self.mu_drop_per_kn * (n - self.weight()) / 1000.0).max(0.2)
        } else {
            mu0
        }
    }

    pub fn mu_x_at(&self, n: f64) -> f64 {
        self.mu_at(self.mu_x, n)
    }

    pub fn mu_y_at(&self, n: f64) -> f64 {
        self.mu_at(self.mu_y, n)
    }

    /// Engine power at the given density.
    pub fn power_at(&self, rho: f64) -> f64 {
        self.power_w * (rho / self.rho_ref).powf(self.power_density_exponent)
    }

    fn torque_at(&self, rpm: f64) -> f64 {
        let c = &self.engine_curve;
        if rpm <= c[0][0] {
            return c[0][1];
        }
        for w in c.windows(2) {
            if rpm <= w[1][0] {
                let f = (rpm - w[0][0]) / (w[1][0] - w[0][0]);
                return w[0][1] + f * (w[1][1] - w[0][1]);
            }
        }
        c[c.len() - 1][1]
    }

    /// Engine speed for a gear at road speed `v`, rpm.
    pub fn engine_rpm(&self, gear_ratio: f64, v: f64) -> f64 {
        v * gear_ratio * self.final_drive / self.wheel_radius_m * 60.0 / (2.0 * std::f64::consts::PI)
    }

    /// Drive force at the wheels before the traction limit. Speed is floored at 1 m/s.
    pub fn drive(&self, rho: f64, v: f64) -> Drive {
        let v = v.max(1.0);
        let mut best = Drive { force: 0.0, gear: 0, rpm: 0.0 };
        if self.is_geared() {
            let rpm_min = self.engine_curve[0][0];
            let rpm_max = self.engine_curve[self.engine_curve.len() - 1][0];
            let scale = (rho / self.rho_ref).powf(self.power_density_exponent);
            let mut found = false;
            for (i, &ratio) in self.gear_ratios.iter().enumerate() {
                let rpm = self.engine_rpm(ratio, v);
                if rpm > rpm_max {
                    continue;
                }
                let t = self.torque_at(rpm.max(rpm_min)) * scale;
                let f = t * ratio * self.final_drive * self.driveline_eff / self.wheel_radius_m;
                if !found || f > best.force {
                    best = Drive { force: f, gear: i as u32 + 1, rpm };
                    found = true;
                }
            }
            if !found {
                // over-revved in every gear: fuel cut in the tallest gear
                let (gi, &ratio) = self
                    .gear_ratios
                    .iter()
                    .enumerate()
                    .min_by(|a, b| a.1.total_cmp(b.1))
                    .expect("geared vehicle has gears");
                best = Drive { force: 0.0, gear: gi as u32 + 1, rpm: self.engine_rpm(ratio, v) };
            }
        } else {
            best.force = self.power_at(rho) / v;
        }
        if self.f_drive_max_n > 0.0 {
            best.force = best.force.min(self.f_drive_max_n);
        }
        best
    }

    /// Forces and friction-ellipse limits at speed `v` on a path of curvature `kappa` (1/m).
    pub fn grip(&self, rho: f64, v: f64, kappa: f64) -> Grip {
        let down = self.downforce(rho, v);
        let n = (self.weight() + down).max(0.0);
        let mu_y = self.mu_y_at(n);
        let mu_x = self.mu_x_at(n);
        let fy_cap = mu_y * n;
        let fy_req = self.mass_kg * v * v * kappa.abs();
        let r = if fy_cap > 0.0 { fy_req / fy_cap } else { f64::INFINITY };
        let fx_max = if r >= 1.0 { 0.0 } else { mu_x * n * (1.0 - r * r).sqrt() };
        Grip { drag: self.drag(rho, v), down, normal: n, rolling: self.rolling(rho, v), fy_cap, fx_max }
    }

    /// Net forward acceleration `(a, drive force, drive)` at speed `v`, curvature `kappa`.
    pub fn accel(&self, rho: f64, v: f64, kappa: f64) -> (f64, f64, Drive) {
        let g = self.grip(rho, v, kappa);
        let d = self.drive(rho, v);
        let f = d.force.min(g.fx_max);
        ((f - g.drag - g.rolling) / self.m_eff(), f, d)
    }

    /// Deceleration magnitude `(a, brake force)` at speed `v`, curvature `kappa`.
    pub fn decel(&self, rho: f64, v: f64, kappa: f64) -> (f64, f64) {
        let g = self.grip(rho, v, kappa);
        let mut fb = g.fx_max;
        if self.brake_max_g > 0.0 {
            fb = fb.min(self.brake_max_g * self.weight());
        }
        ((fb + g.drag + g.rolling) / self.m_eff(), fb)
    }

    /// Cornering speed limit for curvature `kappa` (1/m), capped at `v_cap_ms`.
    pub fn corner_limit(&self, rho: f64, kappa: f64) -> f64 {
        let k = kappa.abs();
        if k < 1e-12 {
            return self.v_cap_ms;
        }
        let mut mu = self.mu_y;
        let mut v = self.v_cap_ms;
        let passes = if self.mu_drop_per_kn > 0.0 { 3 } else { 1 };
        for _ in 0..passes {
            let denom = k - mu * rho * self.cla_m2 / (2.0 * self.mass_kg);
            if denom <= 0.0 {
                return self.v_cap_ms;
            }
            v = (mu * G / denom).sqrt();
            if self.mu_drop_per_kn > 0.0 {
                mu = self.mu_y_at(self.normal_load(rho, v));
            }
        }
        v.min(self.v_cap_ms)
    }

    /// Speed where wheel force equals drag plus rolling resistance (steady top speed).
    pub fn top_speed(&self, rho: f64) -> TopSpeed {
        let excess = |v: f64| {
            let n = self.normal_load(rho, v);
            let f = self.drive(rho, v).force.min(self.mu_x_at(n) * n);
            f - self.drag(rho, v) - self.rolling(rho, v)
        };
        let cap = self.v_cap_ms;
        let (v, limited_by_cap) = if excess(cap) >= 0.0 {
            (cap, true)
        } else {
            // largest v with excess >= 0: scan down from the cap, then bisect
            let steps = 4000;
            let dv = cap / steps as f64;
            let mut hi = cap;
            let mut lo = 0.0;
            for i in (1..=steps).rev() {
                let x = i as f64 * dv;
                if excess(x) >= 0.0 {
                    lo = x;
                    break;
                }
                hi = x;
            }
            for _ in 0..80 {
                let mid = 0.5 * (lo + hi);
                if excess(mid) >= 0.0 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            (0.5 * (lo + hi), false)
        };
        TopSpeed {
            v_ms: v,
            p_aero_w: aero_power(rho, self.cda_eff(), v),
            p_roll_w: self.rolling(rho, v) * v,
            limited_by_cap,
        }
    }
}

/// Steady top-speed solution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopSpeed {
    pub v_ms: f64,
    pub p_aero_w: f64,
    pub p_roll_w: f64,
    pub limited_by_cap: bool,
}
