//! AI nodes that run installed open-weight models as separate processes.
//!
//! * `ai.partcrafter` turns one image into several separate mesh parts (PartCrafter, MIT).
//! * `ai.select_part` picks one generated part.
//! * `ai.aero_preview` runs the DoMINO DrivAerML surface and volume surrogates (NVIDIA Open Model License).
//! * `ai.aero_compare` compares two previews.
//!
//! Nothing here validates CFD. Every DoMINO output is labelled
//! [`AERO_LABEL`], carries `cfd_validation = DEFERRED`, and uses `ai_`-prefixed
//! metric names that no CFD acceptance gate reads.

use glam::DVec3;
use vernon_graph::Registry;

pub mod aero;
pub mod bridge;
pub mod compare;
pub mod forecast;
pub mod parts;
pub mod prep;
pub mod speak;
pub mod transcribe;
#[cfg(test)]
mod tests;

/// The label every DoMINO result carries.
pub const AERO_LABEL: &str = "AI prediction — CFD validation pending";
/// Status string stored in provenance next to the label.
pub const CFD_VALIDATION: &str = "DEFERRED";

pub fn register(reg: &mut Registry) {
    parts::register(reg);
    aero::register(reg);
    compare::register(reg);
    speak::register(reg);
    transcribe::register(reg);
    forecast::register(reg);
}

/// Stable 64-bit FNV-1a hash, used to name artifact folders by their inputs.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Blue to red ramp, `t` in 0..1.
pub fn colormap(t: f64) -> DVec3 {
    const STOPS: [[f64; 3]; 5] =
        [[0.05, 0.1, 0.6], [0.0, 0.7, 0.9], [0.1, 0.8, 0.2], [0.95, 0.9, 0.1], [0.85, 0.1, 0.1]];
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 } * 4.0;
    let i = (t.floor() as usize).min(3);
    let f = t - i as f64;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    DVec3::new(a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f)
}

/// Value at quantile `q` (0..1) of `v`, ignoring non-finite entries.
pub fn quantile(v: &[f64], q: f64) -> f64 {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return f64::NAN;
    }
    s.sort_by(|a, b| a.total_cmp(b));
    s[((s.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize]
}

/// Read a raw little-endian float32 file written by a bridge.
pub fn read_f32(path: &std::path::Path) -> Result<Vec<f32>, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if b.len() % 4 != 0 {
        return Err(format!("{} is not a float32 array", path.display()));
    }
    Ok(b.chunks(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}
