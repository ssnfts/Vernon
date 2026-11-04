use std::collections::HashMap;

use serde_json::{json, Value};
use vernon_core::Geometry;

/// Size and closure of a geometry, in millimetres.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub points: usize,
    pub prims: usize,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub size: [f64; 3],
    /// Surface area, mm².
    pub area_mm2: f64,
    /// Every edge of every polygon is shared by exactly two faces and there
    /// is at least one face.
    pub watertight: bool,
    /// Edges used by one face (holes) …
    pub boundary_edges: usize,
    /// … and by three or more faces.
    pub non_manifold_edges: usize,
}

pub fn measure(g: &Geometry) -> Measurement {
    let b = g.bounds();
    let (min, max, size) = if b.is_empty() {
        ([0.0; 3], [0.0; 3], [0.0; 3])
    } else {
        (b.min.to_array(), b.max.to_array(), b.size().to_array())
    };
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    let mut faces = 0usize;
    let mut open_prims = false;
    for p in &g.prims {
        if !p.closed {
            open_prims = true;
            continue;
        }
        faces += 1;
        let n = p.vertices.len();
        for i in 0..n {
            let (a, c) = (p.vertices[i], p.vertices[(i + 1) % n]);
            *edges.entry((a.min(c), a.max(c))).or_insert(0) += 1;
        }
    }
    let boundary_edges = edges.values().filter(|&&c| c == 1).count();
    let non_manifold_edges = edges.values().filter(|&&c| c > 2).count();
    Measurement {
        points: g.point_count(),
        prims: g.prim_count(),
        min,
        max,
        size,
        area_mm2: g.surface_area(),
        watertight: faces > 0 && !open_prims && boundary_edges == 0 && non_manifold_edges == 0,
        boundary_edges,
        non_manifold_edges,
    }
}

impl Measurement {
    pub fn to_json(&self) -> Value {
        json!({
            "points": self.points,
            "prims": self.prims,
            "bounds_min_mm": self.min,
            "bounds_max_mm": self.max,
            "size_mm": self.size,
            "area_mm2": self.area_mm2,
            "watertight": self.watertight,
            "boundary_edges": self.boundary_edges,
            "non_manifold_edges": self.non_manifold_edges,
        })
    }
}
