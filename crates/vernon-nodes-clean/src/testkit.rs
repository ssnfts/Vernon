//! Shared helpers for unit tests (compiled only under `cfg(test)`).

use std::path::PathBuf;
use std::sync::Arc;

use glam::DVec3;
use vernon_core::{AttribArray, Geometry, Prim, Report};
use vernon_graph::{CookContext, CookResult, Data, NodeTypeInfo, Params, ParamValue};

use crate::util::{CookFn, Rng};

/// UV sphere as a welded triangle mesh with outward winding.
pub fn uv_sphere(radius: f64, nu: usize, nv: usize) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut pos = vec![DVec3::new(0.0, 0.0, radius)];
    for j in 1..nv {
        let th = std::f64::consts::PI * j as f64 / nv as f64;
        for i in 0..nu {
            let ph = 2.0 * std::f64::consts::PI * i as f64 / nu as f64;
            pos.push(DVec3::new(
                radius * th.sin() * ph.cos(),
                radius * th.sin() * ph.sin(),
                radius * th.cos(),
            ));
        }
    }
    pos.push(DVec3::new(0.0, 0.0, -radius));
    let south = (pos.len() - 1) as u32;
    let ring = |j: usize, i: usize| (1 + (j - 1) * nu + (i % nu)) as u32;
    let mut tris = vec![];
    for i in 0..nu {
        tris.push([0, ring(1, i), ring(1, i + 1)]);
    }
    for j in 1..nv - 1 {
        for i in 0..nu {
            tris.push([ring(j, i), ring(j + 1, i), ring(j + 1, i + 1)]);
            tris.push([ring(j, i), ring(j + 1, i + 1), ring(j, i + 1)]);
        }
    }
    for i in 0..nu {
        tris.push([south, ring(nv - 1, i + 1), ring(nv - 1, i)]);
    }
    (pos, tris)
}

/// Sphere as a `Geometry` of triangle polygons.
pub fn sphere_geo(radius: f64, nu: usize, nv: usize) -> Geometry {
    let (pos, tris) = uv_sphere(radius, nu, nv);
    Geometry {
        points: pos,
        prims: tris.iter().map(|t| Prim::polygon(t.to_vec())).collect(),
        ..Default::default()
    }
}

/// Point on an ellipsoid with semi-axes (a, b, c) for a random direction;
/// returns (point, outward unit normal).
pub fn ellipsoid_sample(a: f64, b: f64, c: f64, rng: &mut Rng) -> (DVec3, DVec3) {
    loop {
        let d = DVec3::new(rng.next_f64() * 2.0 - 1.0, rng.next_f64() * 2.0 - 1.0, rng.next_f64() * 2.0 - 1.0);
        let l = d.length();
        if l > 0.05 && l <= 1.0 {
            let u = d / l;
            let p = DVec3::new(u.x * a, u.y * b, u.z * c);
            let n = DVec3::new(p.x / (a * a), p.y / (b * b), p.z / (c * c)).normalize();
            return (p, n);
        }
    }
}

/// Ellipsoid point cloud with analytic normals in `N`.
pub fn ellipsoid_cloud(a: f64, b: f64, c: f64, n: usize, seed: u64) -> Geometry {
    let mut rng = Rng::new(seed);
    let mut pts = Vec::with_capacity(n);
    let mut nor = Vec::with_capacity(n);
    for _ in 0..n {
        let (p, nn) = ellipsoid_sample(a, b, c, &mut rng);
        pts.push(p);
        nor.push(nn);
    }
    let mut g = Geometry::from_points(pts);
    g.point_attribs.set("N", AttribArray::Vec3(nor));
    g
}

/// Flat grid of points on z = 0 with `step` spacing over [0, n*step].
pub fn grid_plane(n: usize, step: f64) -> Geometry {
    let mut pts = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            pts.push(DVec3::new(i as f64 * step, j as f64 * step, 0.0));
        }
    }
    Geometry::from_points(pts)
}

/// Fresh context writing into a temp directory unique to the test name.
pub fn ctx(name: &str) -> CookContext {
    let dir: PathBuf = std::env::temp_dir().join("vernon-nodes-clean-tests").join(name);
    CookContext {
        node_name: name.to_string(),
        work_dir: dir,
        frame: 0.0,
    }
}

/// Defaults of `info`, overridden by `over`.
pub fn params(info: &NodeTypeInfo, over: &[(&str, ParamValue)]) -> Params {
    let mut p = Params::from_specs(&info.params);
    for (k, v) in over {
        assert!(p.get(k).is_some(), "unknown parameter '{k}' on {}", info.id);
        p.set(k, v.clone());
    }
    p
}

/// Run a node with defaults plus overrides.
pub fn run(
    info: fn() -> NodeTypeInfo,
    cook: CookFn,
    inputs: Vec<Option<Data>>,
    over: &[(&str, ParamValue)],
) -> CookResult<Vec<Data>> {
    let i = info();
    cook(&ctx(&i.id), &inputs, &params(&i, over))
}

pub fn geo(g: Geometry) -> Option<Data> {
    Some(Data::Geometry(Arc::new(g)))
}

pub fn json(v: serde_json::Value) -> Option<Data> {
    Some(Data::json(v))
}

pub fn out_geo(d: &Data) -> &Geometry {
    d.as_geometry().expect("geometry output")
}

pub fn out_report(d: &Data) -> &Report {
    d.as_report().expect("report output")
}

pub fn out_json(d: &Data) -> &serde_json::Value {
    d.as_json().expect("json output")
}

pub fn f(v: f64) -> ParamValue {
    ParamValue::Float(v)
}

pub fn i(v: i64) -> ParamValue {
    ParamValue::Int(v)
}

pub fn b(v: bool) -> ParamValue {
    ParamValue::Bool(v)
}

pub fn s(v: &str) -> ParamValue {
    ParamValue::Str(v.to_string())
}
