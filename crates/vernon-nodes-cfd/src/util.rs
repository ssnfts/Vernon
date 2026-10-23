//! Shared helpers: function-pointer node wrapper, input readers, report notes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use vernon_core::{Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, NodeOp, NodeTypeInfo, Params};

pub(crate) type CookFn = fn(&CookContext, &[Option<Data>], &Params) -> CookResult<Vec<Data>>;

/// A node type described by two plain functions; keeps every node file short.
pub(crate) struct FnNode {
    info: fn() -> NodeTypeInfo,
    cook: CookFn,
}

impl FnNode {
    pub(crate) fn new(info: fn() -> NodeTypeInfo, cook: CookFn) -> Self {
        Self { info, cook }
    }
}

impl NodeOp for FnNode {
    fn info(&self) -> NodeTypeInfo {
        (self.info)()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        (self.cook)(ctx, inputs, params)
    }
}

/// Optional Json input at `idx`. A wrong kind is an error, a missing input is `None`.
pub(crate) fn json_in(inputs: &[Option<Data>], idx: usize) -> CookResult<Option<Arc<Value>>> {
    match inputs.get(idx).and_then(|d| d.as_ref()) {
        None => Ok(None),
        Some(Data::Json(v)) => Ok(Some(v.clone())),
        Some(other) => Err(CookError::WrongInput {
            index: idx,
            expected: vernon_graph::DataKind::Json,
            found: other.kind(),
        }),
    }
}

/// Optional File input at `idx`.
pub(crate) fn file_in(inputs: &[Option<Data>], idx: usize) -> CookResult<Option<PathBuf>> {
    match inputs.get(idx).and_then(|d| d.as_ref()) {
        None => Ok(None),
        Some(Data::File(p)) => Ok(Some(p.clone())),
        Some(other) => Err(CookError::WrongInput {
            index: idx,
            expected: vernon_graph::DataKind::File,
            found: other.kind(),
        }),
    }
}

/// Number from a Json object field.
pub(crate) fn jf(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|x| x.is_finite())
}

/// Finite metric value or a clean error (serde_json writes NaN as null).
pub(crate) fn fin(ctx: &CookContext, name: &str, v: f64) -> CookResult<f64> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(ctx.fail(format!("{name} is not finite")))
    }
}

pub(crate) fn placeholder(report: &mut Report, what: &str) {
    report.note(Severity::Warning, format!("white-box placeholder: {what}"));
}

pub(crate) fn ensure_dir(p: &Path) -> CookResult<()> {
    std::fs::create_dir_all(p).map_err(|e| CookError::Other(format!("cannot create {}: {e}", p.display())))
}

pub(crate) fn write_file(p: &Path, text: &str) -> CookResult<()> {
    if let Some(parent) = p.parent() {
        ensure_dir(parent)?;
    }
    std::fs::write(p, text).map_err(|e| CookError::Other(format!("cannot write {}: {e}", p.display())))
}

/// Case directory to use: the upstream one when connected, else `work_dir/case`.
pub(crate) fn case_dir(ctx: &CookContext, upstream: Option<PathBuf>) -> CookResult<PathBuf> {
    let dir = upstream.unwrap_or_else(|| ctx.work_dir.join("case"));
    ensure_dir(&dir)?;
    Ok(dir)
}

/// Compact number formatting for OpenFOAM dictionaries (round-trips, no
/// locale, never prints `NaN`).
pub(crate) fn num(x: f64) -> String {
    if x == 0.0 {
        "0".into()
    } else if x.abs() >= 1e6 || x.abs() < 1e-4 {
        format!("{x:e}")
    } else {
        let s = format!("{x:.9}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_string()
    }
}

pub(crate) fn vec3s(v: [f64; 3]) -> String {
    format!("({} {} {})", num(v[0]), num(v[1]), num(v[2]))
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;

    use vernon_core::{DVec3, Geometry};
    use vernon_graph::CookContext;

    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    /// A context with a fresh, empty work directory (unique per call, so
    /// parallel tests never share or wipe each other's files).
    pub fn ctx(name: &str) -> CookContext {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vernon-cfd-{}-{}-{}", std::process::id(), name, n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        CookContext {
            node_name: name.into(),
            work_dir: dir,
            frame: 0.0,
        }
    }

    pub fn tmp(name: &str) -> PathBuf {
        ctx(name).work_dir
    }

    /// Closed triangle-fan cylinder along Y, as polygons.
    pub fn cylinder_y(c: DVec3, radius: f64, width: f64, segs: usize) -> Geometry {
        let mut pts = Vec::new();
        for side in [-0.5, 0.5] {
            for i in 0..segs {
                let a = std::f64::consts::TAU * i as f64 / segs as f64;
                pts.push(c + DVec3::new(radius * a.cos(), side * width, radius * a.sin()));
            }
        }
        let mut faces = Vec::new();
        for i in 0..segs {
            let j = (i + 1) % segs;
            faces.push(vec![i as u32, j as u32, (segs + j) as u32, (segs + i) as u32]);
        }
        faces.push((0..segs as u32).rev().collect());
        faces.push((segs as u32..2 * segs as u32).collect());
        Geometry::from_polygons(pts, faces)
    }

    /// Box with min/max corners.
    pub fn box_between(min: DVec3, max: DVec3) -> Geometry {
        let mut g = Geometry::make_box(max - min);
        g.transform(vernon_core::DMat4::from_translation((min + max) * 0.5));
        g
    }

    /// A wedge-nosed "car": body box with a pointed nose toward `+x`
    /// (asymmetric so the frame test can tell front from back), plus four
    /// wheels. Millimetres, Z up, nose toward +x.
    pub fn synthetic_car() -> (Geometry, Geometry) {
        let mut body = box_between(DVec3::new(0.0, -900.0, 250.0), DVec3::new(4000.0, 900.0, 1300.0));
        // pull the +x face vertices together to make a nose
        for p in &mut body.points {
            if p.x > 3999.0 {
                p.y *= 0.3;
                p.z = 250.0 + (p.z - 250.0) * 0.5;
            }
        }
        let mut wheels = Geometry::new();
        for (x, y) in [(3000.0, -800.0), (3000.0, 800.0), (800.0, -800.0), (800.0, 800.0)] {
            wheels.merge(&cylinder_y(DVec3::new(x, y, 320.0), 320.0, 250.0, 16));
        }
        (body, wheels)
    }
}
