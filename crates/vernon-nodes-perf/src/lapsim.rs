//! Point-mass quasi-steady lap solver: corner limits, forward and backward passes over
//! the combined-grip (friction ellipse) envelope, time integration, channels and events.
//! Spec section 3.5.

use serde_json::{json, Value};

use crate::physics::Vehicle;
use crate::track::{SegKind, Track};

/// Lowest speed the update may produce (avoids a standing-start stall).
pub const MIN_SPEED: f64 = 0.5;

/// How lap time is accumulated from the speed profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRule {
    /// `sum 2 ds / (v_i + v_{i+1})`, exact for constant acceleration per interval.
    Trapezoid,
    /// `sum ds / v_i`.
    SumDsOverV,
}

impl TimeRule {
    pub fn name(self) -> &'static str {
        match self {
            TimeRule::Trapezoid => "trapezoid",
            TimeRule::SumDsOverV => "sum_ds_over_v",
        }
    }
}

/// Solver options.
#[derive(Debug, Clone, Copy)]
pub struct LapOptions {
    pub time_rule: TimeRule,
    /// Start speed of an open track (default standing start).
    pub start_speed: Option<f64>,
    /// End speed of an open track (default free).
    pub end_speed: Option<f64>,
    /// Intervals longer than this are subdivided before solving; the step is also kept below
    /// 1/20 of the tightest corner radius.
    pub ds_max_m: f64,
    /// Hold `v = v_lim` on constant-radius arcs instead of letting drag sag it.
    pub hold_limit_speed: bool,
    /// Fraction of braking energy recovered, 0..1.
    pub regen_frac: f64,
    pub kappa_straight: f64,
    pub min_seg_m: f64,
}

impl Default for LapOptions {
    fn default() -> Self {
        Self {
            time_rule: TimeRule::Trapezoid,
            start_speed: None,
            end_speed: None,
            ds_max_m: 5.0,
            hold_limit_speed: false,
            regen_frac: 0.0,
            kappa_straight: 0.001,
            min_seg_m: 3.0,
        }
    }
}

/// One entry of the event table.
#[derive(Debug, Clone, PartialEq)]
pub struct SegResult {
    pub kind: SegKind,
    pub s_start_m: f64,
    pub length_m: f64,
    pub radius_m: f64,
    pub v_min: f64,
    pub v_max: f64,
    pub time_s: f64,
}

/// Solved lap. Per-node arrays have `track.nodes()` entries.
#[derive(Debug, Clone)]
pub struct LapResult {
    pub track: Track,
    pub rho: f64,
    pub time_rule: TimeRule,
    pub lap_time_s: f64,
    pub s: Vec<f64>,
    pub v: Vec<f64>,
    pub v_lim: Vec<f64>,
    pub t: Vec<f64>,
    pub ax: Vec<f64>,
    pub ay: Vec<f64>,
    /// 0 cruise, 1 accelerate, 2 brake, 3 at the cornering limit.
    pub mode: Vec<u8>,
    pub f_drive: Vec<f64>,
    pub f_brake: Vec<f64>,
    pub drag: Vec<f64>,
    pub down: Vec<f64>,
    pub normal: Vec<f64>,
    pub gear: Vec<u32>,
    pub rpm: Vec<f64>,
    pub p_wheel: Vec<f64>,
    pub energy_wheel_j: f64,
    pub energy_source_j: f64,
    pub regen_j: f64,
    pub segments: Vec<SegResult>,
    /// Intervals where the profile leaves the grip envelope or exceeds `v_lim`.
    pub violations: usize,
}

impl LapResult {
    pub fn v_max(&self) -> f64 {
        self.v.iter().copied().fold(f64::MIN, f64::max)
    }

    pub fn v_min(&self) -> f64 {
        self.v.iter().copied().fold(f64::MAX, f64::min)
    }

    /// Net energy drawn from the source, kWh.
    pub fn energy_kwh(&self) -> f64 {
        (self.energy_source_j - self.regen_j) / 3.6e6
    }

    /// Speed at arc length `s` by linear interpolation of the node speeds.
    pub fn speed_at(&self, s: f64) -> f64 {
        let i = self.s.partition_point(|x| *x <= s).saturating_sub(1).min(self.s.len() - 1);
        if i + 1 >= self.s.len() {
            return self.v[i];
        }
        let f = ((s - self.s[i]) / (self.s[i + 1] - self.s[i])).clamp(0.0, 1.0);
        self.v[i] + f * (self.v[i + 1] - self.v[i])
    }

    /// Time at arc length `s` by linear interpolation of the cumulative time.
    pub fn time_at(&self, s: f64) -> f64 {
        let i = self.s.partition_point(|x| *x <= s).saturating_sub(1).min(self.s.len() - 1);
        if i + 1 >= self.s.len() {
            return self.t[i];
        }
        let f = ((s - self.s[i]) / (self.s[i + 1] - self.s[i])).clamp(0.0, 1.0);
        self.t[i] + f * (self.t[i + 1] - self.t[i])
    }

    /// First arc length where the speed reaches `v` (open tracks, accelerating).
    pub fn time_to_speed(&self, v: f64) -> Option<f64> {
        let i = self.v.iter().position(|x| *x >= v)?;
        if i == 0 {
            return Some(self.t[0]);
        }
        let f = (v - self.v[i - 1]) / (self.v[i] - self.v[i - 1]);
        Some(self.t[i - 1] + f * (self.t[i] - self.t[i - 1]))
    }

    pub fn to_json(&self) -> Value {
        let seg: Vec<Value> = self
            .segments
            .iter()
            .map(|g| {
                json!({
                    "type": g.kind.name(),
                    "s_start_m": g.s_start_m,
                    "length_m": g.length_m,
                    "radius_m": if g.radius_m.is_finite() { json!(g.radius_m) } else { Value::Null },
                    "v_min_ms": g.v_min,
                    "v_max_ms": g.v_max,
                    "v_min_kmh": g.v_min * 3.6,
                    "v_max_kmh": g.v_max * 3.6,
                    "time_s": g.time_s,
                })
            })
            .collect();
        json!({
            "kind": "lap_result.v1",
            "lap_time_s": self.lap_time_s,
            "closed": self.track.closed,
            "length_m": self.track.length(),
            "time_rule": self.time_rule.name(),
            "rho": self.rho,
            "v_max_ms": self.v_max(),
            "v_min_ms": self.v_min(),
            "energy_kwh": self.energy_kwh(),
            "s": self.s,
            "ds": self.track.ds,
            "kappa": self.track.kappa,
            "v_ms": self.v,
            "v_lim_ms": self.v_lim,
            "t_s": self.t,
            "a_x": self.ax,
            "a_y": self.ay,
            "mode": self.mode,
            "f_drive_n": self.f_drive,
            "f_brake_n": self.f_brake,
            "drag_n": self.drag,
            "down_n": self.down,
            "normal_n": self.normal,
            "gear": self.gear,
            "rpm": self.rpm,
            "p_wheel_w": self.p_wheel,
            "segments": seg,
        })
    }
}

/// Solve one lap (or one run along an open track).
pub fn solve(veh: &Vehicle, rho: f64, track: &Track, o: &LapOptions) -> Result<LapResult, String> {
    veh.validate()?;
    if !(rho.is_finite() && rho > 0.0) {
        return Err(format!("air density {rho} must be positive"));
    }
    // discretisation rule (spec 3.5): interval no longer than ds_max_m nor 1/20 of the tightest radius
    let kmax = track.kappa.iter().fold(0.0f64, |m, k| m.max(k.abs()));
    let mut ds_cap = o.ds_max_m;
    if kmax > 1e-9 {
        ds_cap = ds_cap.min(1.0 / (20.0 * kmax));
    }
    let ds_cap = ds_cap.max(track.length() / 400_000.0).max(0.01);
    let tr = track.subdivided(ds_cap);
    let m = tr.intervals();
    let n = tr.nodes();
    let closed = tr.closed;
    let next = |i: usize| -> usize {
        if closed {
            (i + 1) % n
        } else {
            i + 1
        }
    };
    let kap = |i: usize| tr.kappa[i.min(m - 1)];
    let v_lim: Vec<f64> = (0..n).map(|i| veh.corner_limit(rho, kap(i)).max(MIN_SPEED)).collect();

    let fwd_step = |v: f64, i: usize, vf_next_lim: f64| -> f64 {
        let (a, _, _) = veh.accel(rho, v, tr.kappa[i]);
        let v2 = v * v + 2.0 * a * tr.ds[i];
        let mut vn = if v2 > 0.0 { v2.sqrt() } else { 0.0 };
        if o.hold_limit_speed && a <= 0.0 && v >= v_lim[i] * (1.0 - 1e-12) {
            vn = vn.max(v);
        }
        vn.max(MIN_SPEED).min(vf_next_lim)
    };
    let bwd_step = |v_end: f64, j: usize, lim: f64| -> f64 {
        let (a, _) = veh.decel(rho, v_end, tr.kappa[j]);
        (v_end * v_end + 2.0 * a * tr.ds[j]).sqrt().min(lim)
    };

    let mut vf = vec![0.0; n];
    let mut vb = vec![0.0; n];
    if closed {
        let i0 = v_lim
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .expect("closed track has nodes");
        vf[i0] = v_lim[i0];
        let mut i = i0;
        for _ in 0..n - 1 {
            let j = next(i);
            vf[j] = fwd_step(vf[i], i, v_lim[j]);
            i = j;
        }
        vb[i0] = v_lim[i0];
        for k in 1..n {
            let j = (i0 + n - k) % n;
            vb[j] = bwd_step(vb[next(j)], j, v_lim[j]);
        }
    } else {
        vf[0] = o.start_speed.unwrap_or(0.0).max(0.0).min(v_lim[0]);
        for i in 0..m {
            vf[i + 1] = fwd_step(vf[i], i, v_lim[i + 1]);
        }
        vb[m] = o.end_speed.map(|e| e.max(MIN_SPEED)).unwrap_or(v_lim[m]).min(v_lim[m]);
        for j in (0..m).rev() {
            vb[j] = bwd_step(vb[j + 1], j, v_lim[j]);
        }
    }
    let v: Vec<f64> = vf.iter().zip(&vb).map(|(a, b)| a.min(*b)).collect();

    // time
    let mut t = vec![0.0; n];
    let mut dt = vec![0.0; m];
    for i in 0..m {
        let d = match o.time_rule {
            TimeRule::Trapezoid => 2.0 * tr.ds[i] / (v[i] + v[next(i)]).max(1e-9),
            TimeRule::SumDsOverV => tr.ds[i] / v[i].max(MIN_SPEED),
        };
        dt[i] = d;
        if i + 1 < n {
            t[i + 1] = t[i] + d;
        }
    }
    let lap_time_s: f64 = dt.iter().sum();
    let s = tr.s_nodes();

    // channels
    let mut ax = vec![0.0; n];
    let mut ay = vec![0.0; n];
    let mut mode = vec![0u8; n];
    let mut f_drive = vec![0.0; n];
    let mut f_brake = vec![0.0; n];
    let mut drag = vec![0.0; n];
    let mut down = vec![0.0; n];
    let mut normal = vec![0.0; n];
    let mut gear = vec![0u32; n];
    let mut rpm = vec![0.0; n];
    let mut p_wheel = vec![0.0; n];
    let (mut e_wheel, mut e_brake, mut violations) = (0.0, 0.0, 0usize);
    let m_eff = veh.m_eff();
    for i in 0..m {
        let vi = v[i];
        let vn = v[next(i)];
        let a = (vn * vn - vi * vi) / (2.0 * tr.ds[i]);
        let vm = 0.5 * (vi + vn);
        let g = veh.grip(rho, vi, tr.kappa[i]);
        let gm = veh.grip(rho, vm, tr.kappa[i]);
        let net = m_eff * a + gm.drag + gm.rolling;
        let fd = net.max(0.0);
        let fb = (-net).max(0.0);
        let drv = veh.drive(rho, vm);
        ax[i] = a;
        ay[i] = vi * vi * tr.kappa[i];
        mode[i] = if a > 1e-3 {
            1
        } else if a < -1e-3 {
            2
        } else if vi >= v_lim[i] * (1.0 - 1e-4) && tr.kappa[i].abs() > 1e-9 {
            3
        } else {
            0
        };
        f_drive[i] = fd;
        f_brake[i] = fb;
        drag[i] = g.drag;
        down[i] = g.down;
        normal[i] = g.normal;
        gear[i] = drv.gear;
        rpm[i] = drv.rpm;
        p_wheel[i] = fd * vm;
        e_wheel += fd * tr.ds[i];
        e_brake += fb * tr.ds[i];
        // envelope check (tolerance covers the discrete step)
        let (a_acc, _, _) = veh.accel(rho, vi, tr.kappa[i]);
        let (a_dec, _) = veh.decel(rho, vn, tr.kappa[i]);
        let tol = 1e-6 * (1.0 + a.abs());
        if a > a_acc + tol || -a > a_dec + tol || vi > v_lim[i] * (1.0 + 1e-9) + 1e-9 {
            violations += 1;
        }
    }
    if !closed {
        let l = m - 1;
        ax[m] = ax[l];
        ay[m] = v[m] * v[m] * tr.kappa[l];
        mode[m] = mode[l];
        f_drive[m] = f_drive[l];
        f_brake[m] = f_brake[l];
        let g = veh.grip(rho, v[m], tr.kappa[l]);
        drag[m] = g.drag;
        down[m] = g.down;
        normal[m] = g.normal;
        let drv = veh.drive(rho, v[m]);
        gear[m] = drv.gear;
        rpm[m] = drv.rpm;
        p_wheel[m] = f_drive[l] * v[m];
    }
    let eta = veh.driveline_eff.clamp(1e-3, 1.0);
    let energy_source_j = e_wheel / eta;
    let regen_j = o.regen_frac.clamp(0.0, 1.0) * e_brake * eta;

    // event table
    let mut segments = Vec::new();
    for sg in tr.segment_list(o.kappa_straight, o.min_seg_m) {
        let mut v_min = f64::MAX;
        let mut v_max = f64::MIN;
        for i in sg.i0..=sg.i1 {
            let vv = v[i % n];
            v_min = v_min.min(vv);
            v_max = v_max.max(vv);
        }
        segments.push(SegResult {
            kind: sg.kind,
            s_start_m: tr.ds[..sg.i0].iter().sum(),
            length_m: sg.length_m,
            radius_m: sg.radius_m,
            v_min,
            v_max,
            time_s: dt[sg.i0..sg.i1].iter().sum(),
        });
    }

    Ok(LapResult {
        track: tr,
        rho,
        time_rule: o.time_rule,
        lap_time_s,
        s,
        v,
        v_lim,
        t,
        ax,
        ay,
        mode,
        f_drive,
        f_brake,
        drag,
        down,
        normal,
        gear,
        rpm,
        p_wheel,
        energy_wheel_j: e_wheel,
        energy_source_j,
        regen_j,
        segments,
        violations,
    })
}

/// Lap time only.
pub fn lap_time(veh: &Vehicle, rho: f64, track: &Track, o: &LapOptions) -> Result<f64, String> {
    solve(veh, rho, track, o).map(|r| r.lap_time_s)
}

/// Quantities swept by `perf.lap_sweep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepParam {
    CdA,
    ClA,
    Mass,
    Mu,
    Power,
    Rho,
}

impl SweepParam {
    pub const NAMES: [&'static str; 6] = ["cda_m2", "cla_m2", "mass_kg", "mu", "power_w", "rho"];

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "cda_m2" => SweepParam::CdA,
            "cla_m2" => SweepParam::ClA,
            "mass_kg" => SweepParam::Mass,
            "mu" => SweepParam::Mu,
            "power_w" => SweepParam::Power,
            "rho" => SweepParam::Rho,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    pub fn unit(self) -> &'static str {
        match self {
            SweepParam::CdA | SweepParam::ClA => "m2",
            SweepParam::Mass => "kg",
            SweepParam::Mu => "-",
            SweepParam::Power => "W",
            SweepParam::Rho => "kg/m3",
        }
    }

    pub fn value(self, veh: &Vehicle, rho: f64) -> f64 {
        match self {
            SweepParam::CdA => veh.cda_m2,
            SweepParam::ClA => veh.cla_m2,
            SweepParam::Mass => veh.mass_kg,
            SweepParam::Mu => veh.mu_x,
            SweepParam::Power => veh.power_w,
            SweepParam::Rho => rho,
        }
    }

    pub fn apply(self, veh: &mut Vehicle, rho: &mut f64, x: f64) {
        match self {
            SweepParam::CdA => veh.cda_m2 = x,
            SweepParam::ClA => veh.cla_m2 = x,
            SweepParam::Mass => veh.mass_kg = x,
            SweepParam::Mu => {
                veh.mu_x = x;
                veh.mu_y = x;
            }
            SweepParam::Power => veh.power_w = x,
            SweepParam::Rho => *rho = x,
        }
    }
}

/// Lap time with one parameter replaced.
pub fn lap_time_with(
    veh: &Vehicle,
    rho: f64,
    track: &Track,
    o: &LapOptions,
    p: SweepParam,
    x: f64,
) -> Result<f64, String> {
    let mut v = veh.clone();
    let mut r = rho;
    p.apply(&mut v, &mut r, x);
    lap_time(&v, r, track, o)
}

/// Central-difference sensitivity `dt/dp` with a step of `h`.
pub fn sensitivity(
    veh: &Vehicle,
    rho: f64,
    track: &Track,
    o: &LapOptions,
    p: SweepParam,
    h: f64,
) -> Result<f64, String> {
    let x = p.value(veh, rho);
    let lo = lap_time_with(veh, rho, track, o, p, x - h)?;
    let hi = lap_time_with(veh, rho, track, o, p, x + h)?;
    Ok((hi - lo) / (2.0 * h))
}

/// Default sensitivity step: 5 percent of the value, or `fallback` when the value is zero.
pub fn default_step(x: f64, fallback: f64) -> f64 {
    if x.abs() > 1e-12 {
        0.05 * x.abs()
    } else {
        fallback
    }
}
