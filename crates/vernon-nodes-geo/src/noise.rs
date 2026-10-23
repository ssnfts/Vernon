//! Seeded Gaussian point noise, used to make synthetic "scans" of a known
//! reference surface for alignment and deviation tests.

use vernon_core::{DVec3, Geometry};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
    Stage,
};

use crate::util::SPEC;

/// SplitMix64: tiny, fast and good enough for test noise.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1].
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) / (1u64 << 53) as f64
    }

    /// Standard normal (Box-Muller).
    pub fn gauss(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

/// Displace every point by Gaussian noise of standard deviation `sigma`
/// along its normal (`along_normal`, when an `N` attribute exists or can be
/// computed) or in all three axes.
pub fn add_noise(geo: &mut Geometry, sigma: f64, along_normal: bool, seed: u64) {
    let mut rng = Rng::new(seed);
    let normals: Option<Vec<DVec3>> = if along_normal {
        if !geo.point_attribs.contains("N") && !geo.prims.is_empty() {
            geo.compute_normals();
        }
        geo.point_attribs.vec3s("N").ok().map(|n| n.to_vec())
    } else {
        None
    };
    for (i, p) in geo.points.iter_mut().enumerate() {
        match normals.as_ref().and_then(|n| n.get(i)) {
            Some(n) if n.length_squared() > 0.0 => *p += n.normalize() * (sigma * rng.gauss()),
            _ => *p += DVec3::new(rng.gauss(), rng.gauss(), rng.gauss()) * sigma,
        }
    }
}

pub struct NoiseNode;

impl NodeOp for NoiseNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.noise", "Add Noise", Stage::Geometry)
            .input(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("geometry", DataKind::Geometry))
            .param(
                ParamSpec::float("sigma", "Noise level", 0.05, 0.0, 100.0)
                    .unit("mm")
                    .help("Standard deviation of the displacement."),
            )
            .param(
                ParamSpec::choice("direction", "Direction", "normal", &["normal", "isotropic"])
                    .help("Move points along their normals (like scanner range noise) or in all three axes."),
            )
            .param(ParamSpec::int("seed", "Seed", 1, 0, i64::MAX).help("Same seed, same noise."))
            .help("Adds seeded Gaussian noise to point positions, for example to turn a reference surface into a synthetic scan.")
            .emulates("Point jitter / noise deformers of general-purpose DCC tools, used here to fake scanner range noise")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let sigma = p.float("sigma")?;
        if !(sigma.is_finite() && sigma >= 0.0) {
            return Err(ctx.fail("noise level must be zero or positive"));
        }
        let mut geo = (*inputs::geometry(ins, 0)?).clone();
        add_noise(
            &mut geo,
            sigma,
            p.str("direction")? == "normal",
            p.int("seed")? as u64,
        );
        Ok(vec![Data::geometry(geo)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_has_requested_spread_and_is_seeded() {
        let flat = Geometry::from_points(vec![DVec3::ZERO; 20000]);
        let mut a = flat.clone();
        add_noise(&mut a, 0.1, false, 7);
        let mut b = flat.clone();
        add_noise(&mut b, 0.1, false, 7);
        assert_eq!(a.points, b.points);
        let var = a.points.iter().map(|p| p.z * p.z).sum::<f64>() / a.points.len() as f64;
        assert!((var.sqrt() - 0.1).abs() < 0.005, "sd {}", var.sqrt());
    }
}
