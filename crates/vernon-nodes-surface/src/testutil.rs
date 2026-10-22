//! Synthetic meshes and cook helpers shared by this crate's tests.

use std::f64::consts::{FRAC_PI_4, PI, TAU};

use glam::DVec3;
use vernon_core::{Geometry, Prim};
use vernon_graph::{CookContext, CookResult, Data, ParamValue, Params, Registry};

pub fn registry() -> Registry {
    let mut r = Registry::new();
    crate::register(&mut r);
    r
}

pub fn ctx(name: &str) -> CookContext {
    CookContext {
        node_name: name.to_string(),
        work_dir: std::env::temp_dir().join("vernon-nodes-surface-tests").join(name.replace('.', "_")),
        frame: 0.0,
    }
}

/// Cook a registered node with default params overridden by `set`.
pub fn cook(id: &str, inputs: &[Option<Data>], set: &[(&str, ParamValue)]) -> CookResult<Vec<Data>> {
    let reg = registry();
    let info = reg.info(id).unwrap_or_else(|| panic!("unknown node {id}"));
    let mut params = Params::from_specs(&info.params);
    for (k, v) in set {
        assert!(params.get(k).is_some(), "{id} has no param {k}");
        params.set(k, v.clone());
    }
    reg.get(id).expect("registered").cook(&ctx(id), inputs, &params)
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
pub fn v3(x: f64, y: f64, z: f64) -> ParamValue {
    ParamValue::Vec3(DVec3::new(x, y, z))
}

pub fn some(g: &Geometry) -> Option<Data> {
    Some(Data::geometry(g.clone()))
}

pub fn geo_of(d: &Data) -> Geometry {
    (**d.as_geometry().expect("geometry output")).clone()
}

pub fn report_of(d: &Data) -> vernon_core::Report {
    (**d.as_report().expect("report output")).clone()
}

/// Flip all primitives if the closed mesh has negative volume.
pub fn fix_outward(g: &mut Geometry) {
    if crate::topology::geometry_volume(g) < 0.0 {
        for p in &mut g.prims {
            p.vertices.reverse();
        }
    }
}

/// Split every polygon into a fan of triangles.
pub fn triangulated(g: &Geometry) -> Geometry {
    let mut out = g.clone();
    out.prims.clear();
    out.prim_attribs = Default::default();
    for (_, t) in g.triangles() {
        out.prims.push(Prim::polygon(t.to_vec()));
    }
    out
}

/// UV sphere with single pole vertices (z up), `nu` segments, `nv` bands.
pub fn sphere(r: f64, nu: usize, nv: usize) -> Geometry {
    let mut g = Geometry::new();
    g.points.push(DVec3::new(0.0, 0.0, r));
    for j in 1..nv {
        let th = PI * j as f64 / nv as f64;
        for i in 0..nu {
            let ph = TAU * i as f64 / nu as f64;
            g.points.push(DVec3::new(r * th.sin() * ph.cos(), r * th.sin() * ph.sin(), r * th.cos()));
        }
    }
    g.points.push(DVec3::new(0.0, 0.0, -r));
    let ring = |j: usize, i: usize| (1 + (j - 1) * nu + i % nu) as u32;
    let south = (g.points.len() - 1) as u32;
    for i in 0..nu {
        g.prims.push(Prim::polygon(vec![0, ring(1, i), ring(1, i + 1)]));
    }
    for j in 1..nv - 1 {
        for i in 0..nu {
            g.prims.push(Prim::polygon(vec![ring(j, i), ring(j + 1, i), ring(j + 1, i + 1), ring(j, i + 1)]));
        }
    }
    for i in 0..nu {
        g.prims.push(Prim::polygon(vec![south, ring(nv - 1, i + 1), ring(nv - 1, i)]));
    }
    fix_outward(&mut g);
    g
}

/// Icosphere: `subdiv` = 4 gives 2562 vertices and 5120 triangles.
pub fn icosphere(r: f64, subdiv: usize) -> Geometry {
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut pts: Vec<DVec3> = [
        (-1.0, t, 0.0), (1.0, t, 0.0), (-1.0, -t, 0.0), (1.0, -t, 0.0),
        (0.0, -1.0, t), (0.0, 1.0, t), (0.0, -1.0, -t), (0.0, 1.0, -t),
        (t, 0.0, -1.0), (t, 0.0, 1.0), (-t, 0.0, -1.0), (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| DVec3::new(x, y, z).normalize() * r)
    .collect();
    let mut tris: Vec<[u32; 3]> = vec![
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
        [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
        [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];
    for _ in 0..subdiv {
        let mut cache = std::collections::HashMap::new();
        let mut mid = |a: u32, b: u32, pts: &mut Vec<DVec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *cache.entry(key).or_insert_with(|| {
                pts.push(((pts[a as usize] + pts[b as usize]) * 0.5).normalize() * r);
                (pts.len() - 1) as u32
            })
        };
        let mut next = Vec::new();
        for tri in &tris {
            let ab = mid(tri[0], tri[1], &mut pts);
            let bc = mid(tri[1], tri[2], &mut pts);
            let ca = mid(tri[2], tri[0], &mut pts);
            next.push([tri[0], ab, ca]);
            next.push([tri[1], bc, ab]);
            next.push([tri[2], ca, bc]);
            next.push([ab, bc, ca]);
        }
        tris = next;
    }
    let mut g = Geometry::from_polygons(pts, tris.iter().map(|t| t.to_vec()).collect());
    fix_outward(&mut g);
    g
}

fn lateral(r: f64, h: f64, seg: usize, outward: bool, caps_base: &mut Geometry) {
    let base = caps_base.points.len() as u32;
    for k in 0..2 {
        let z = if k == 0 { -h / 2.0 } else { h / 2.0 };
        for i in 0..seg {
            let a = TAU * i as f64 / seg as f64;
            caps_base.points.push(DVec3::new(r * a.cos(), r * a.sin(), z));
        }
    }
    for i in 0..seg {
        let (a, b) = (base + i as u32, base + ((i + 1) % seg) as u32);
        let (c, d) = (b + seg as u32, a + seg as u32);
        let quad = if outward { vec![a, b, c, d] } else { vec![a, d, c, b] };
        caps_base.prims.push(Prim::polygon(quad));
    }
}

/// Closed capped cylinder, axis Z, centred at the origin; triangles only.
pub fn cylinder(r: f64, h: f64, seg: usize) -> Geometry {
    let mut g = Geometry::new();
    lateral(r, h, seg, true, &mut g);
    let bottom = g.points.len() as u32;
    g.points.push(DVec3::new(0.0, 0.0, -h / 2.0));
    let top = bottom + 1;
    g.points.push(DVec3::new(0.0, 0.0, h / 2.0));
    for i in 0..seg as u32 {
        let j = (i + 1) % seg as u32;
        g.prims.push(Prim::polygon(vec![bottom, j, i]));
        g.prims.push(Prim::polygon(vec![top, i + seg as u32, j + seg as u32]));
    }
    let mut t = triangulated(&g);
    fix_outward(&mut t);
    t
}

/// Open hollow tube: outer wall facing out, inner wall facing the axis.
pub fn tube(r_out: f64, r_in: f64, h: f64, seg: usize) -> Geometry {
    let mut g = Geometry::new();
    lateral(r_out, h, seg, true, &mut g);
    lateral(r_in, h, seg, false, &mut g);
    g
}

/// Open height-field grid over `[0,sx] x [0,sy]` with `nx * ny` quads, normals +Z.
pub fn grid(nx: usize, ny: usize, sx: f64, sy: f64, z: impl Fn(f64, f64) -> f64) -> Geometry {
    let mut g = Geometry::new();
    for j in 0..=ny {
        for i in 0..=nx {
            let (x, y) = (sx * i as f64 / nx as f64, sy * j as f64 / ny as f64);
            g.points.push(DVec3::new(x, y, z(x, y)));
        }
    }
    let id = |i: usize, j: usize| (j * (nx + 1) + i) as u32;
    for j in 0..ny {
        for i in 0..nx {
            g.prims.push(Prim::polygon(vec![id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)]));
        }
    }
    g
}

/// Cube of side `a` with edge fillets of radius `r` (`ns` segments per 45 degrees),
/// as a welded triangle mesh. Expected regions: 6 planes, 12 cylinders, 8 spheres.
pub fn filleted_box(a: f64, r: f64, ns: usize) -> Geometry {
    let h = a / 2.0;
    let q = h - r;
    let mut us: Vec<f64> = (0..=ns).rev().map(|k| -(q + r * (FRAC_PI_4 * k as f64 / ns as f64).tan())).collect();
    us.push(-q / 2.0);
    us.push(0.0);
    us.push(q / 2.0);
    us.extend((0..=ns).map(|k| q + r * (FRAC_PI_4 * k as f64 / ns as f64).tan()));
    us.sort_by(|x, y| x.total_cmp(y));
    us.dedup_by(|x, y| (*x - *y).abs() < 1e-12);
    let map = |p: DVec3| -> DVec3 {
        let c = p.clamp(DVec3::splat(-q), DVec3::splat(q));
        let d = p - c;
        if d.length() > 1e-12 {
            c + d.normalize() * r
        } else {
            p
        }
    };
    let mut g = Geometry::new();
    let n = us.len();
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let base = g.points.len() as u32;
            for iu in 0..n {
                for iv in 0..n {
                    let mut p = DVec3::ZERO;
                    p[axis] = sign * h;
                    p[(axis + 1) % 3] = us[iu];
                    p[(axis + 2) % 3] = us[iv];
                    g.points.push(map(p));
                }
            }
            for iu in 0..n - 1 {
                for iv in 0..n - 1 {
                    let id = |a: usize, b: usize| base + (a * n + b) as u32;
                    let quad = [id(iu, iv), id(iu + 1, iv), id(iu + 1, iv + 1), id(iu, iv + 1)];
                    // (axis+1, axis+2) is a right-handed frame with +axis, so sign +1 faces out
                    let q4 = if sign > 0.0 { quad.to_vec() } else { quad.iter().rev().copied().collect() };
                    g.prims.push(Prim::polygon(q4));
                }
            }
        }
    }
    let w = crate::topology::weld(&g, 1e-9);
    let mut tris = Vec::new();
    for f in &w.faces {
        tris.push(vec![f[0], f[1], f[2]]);
        tris.push(vec![f[0], f[2], f[3]]);
    }
    let mut out = Geometry::from_polygons(w.points, tris);
    fix_outward(&mut out);
    out
}
