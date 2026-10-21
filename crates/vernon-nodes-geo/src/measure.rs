//! Measure: counts, area, volume, bounds and edge topology as a Report.

use vernon_core::{AttribArray, DVec3, Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
    Stage,
};

use crate::util::{edge_counts, SPEC};

/// Numbers describing a geometry.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Measurements {
    pub points: usize,
    pub prims: usize,
    pub triangles: usize,
    /// mm^2
    pub area: f64,
    /// Signed enclosed volume by the divergence theorem, mm^3. Only
    /// meaningful for a closed, consistently wound surface.
    pub volume: f64,
    pub bbox_min: DVec3,
    pub bbox_max: DVec3,
    /// Area-weighted surface centroid (point average for point clouds), mm.
    pub centroid: DVec3,
    /// Edges used by exactly one face.
    pub open_edges: usize,
    /// Edges used by more than two faces.
    pub non_manifold_edges: usize,
}

impl Measurements {
    pub fn size(&self) -> DVec3 {
        if self.points == 0 {
            DVec3::ZERO
        } else {
            self.bbox_max - self.bbox_min
        }
    }

    pub fn watertight(&self) -> bool {
        self.triangles > 0 && self.open_edges == 0 && self.non_manifold_edges == 0
    }
}

pub fn measure(geo: &Geometry) -> Measurements {
    let tris = geo.triangles();
    let mut area = 0.0;
    let mut volume = 0.0;
    let mut weighted = DVec3::ZERO;
    for (_, [a, b, c]) in &tris {
        let (a, b, c) = (geo.points[*a as usize], geo.points[*b as usize], geo.points[*c as usize]);
        let ar = (b - a).cross(c - a).length() * 0.5;
        area += ar;
        weighted += (a + b + c) / 3.0 * ar;
        volume += a.dot(b.cross(c)) / 6.0;
    }
    let bounds = geo.bounds();
    let centroid = if area > 0.0 {
        weighted / area
    } else if !geo.points.is_empty() {
        geo.points.iter().copied().sum::<DVec3>() / geo.points.len() as f64
    } else {
        DVec3::ZERO
    };
    let edges = edge_counts(geo);
    Measurements {
        points: geo.points.len(),
        prims: geo.prims.len(),
        triangles: tris.len(),
        area,
        volume,
        bbox_min: if bounds.is_empty() { DVec3::ZERO } else { bounds.min },
        bbox_max: if bounds.is_empty() { DVec3::ZERO } else { bounds.max },
        centroid,
        open_edges: edges.values().filter(|&&c| c == 1).count(),
        non_manifold_edges: edges.values().filter(|&&c| c > 2).count(),
    }
}

/// Report with the standard measurement metrics.
pub fn measure_report(m: &Measurements, require_watertight: bool) -> Report {
    let mut r = Report::new("Measure");
    let size = m.size();
    r.metric("point_count", Metric::new(m.points as f64, "count"));
    r.metric("prim_count", Metric::new(m.prims as f64, "count"));
    r.metric("triangle_count", Metric::new(m.triangles as f64, "count"));
    r.metric("surface_area", Metric::new(m.area, "mm²"));
    r.metric("surface_area_m2", Metric::new(m.area * 1e-6, "m²"));
    r.metric("volume", Metric::new(m.volume, "mm³"));
    r.metric("size_x", Metric::new(size.x, "mm"));
    r.metric("size_y", Metric::new(size.y, "mm"));
    r.metric("size_z", Metric::new(size.z, "mm"));
    r.metric("bbox_diagonal", Metric::new(size.length(), "mm"));
    r.metric("centroid_x", Metric::new(m.centroid.x, "mm"));
    r.metric("centroid_y", Metric::new(m.centroid.y, "mm"));
    r.metric("centroid_z", Metric::new(m.centroid.z, "mm"));
    let mut open = Metric::new(m.open_edges as f64, "count");
    let mut nm = Metric::new(m.non_manifold_edges as f64, "count");
    if require_watertight {
        open = open.with_limit(0.0, true);
        nm = nm.with_limit(0.0, true);
    }
    r.metric("open_edges", open);
    r.metric("non_manifold_edges", nm);
    if m.triangles == 0 {
        r.note(Severity::Info, "no faces: area and volume are zero");
    } else if !m.watertight() {
        r.note(
            if require_watertight { Severity::Error } else { Severity::Info },
            format!(
                "surface is not closed ({} open, {} non-manifold edges): volume is not meaningful",
                m.open_edges, m.non_manifold_edges
            ),
        );
    } else if m.volume < 0.0 {
        r.note(Severity::Warning, "closed surface has negative volume: faces wind inward");
    }
    r
}

pub struct MeasureNode;

impl NodeOp for MeasureNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.measure", "Measure", Stage::Geometry)
            .input(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(
                ParamSpec::bool("prim_area", "Store face area", true)
                    .help("Write each face's area into the prim attribute 'area' (mm²)."),
            )
            .param(
                ParamSpec::bool("require_watertight", "Require closed surface", false)
                    .help("Fail the report when the surface has open or non-manifold edges."),
            )
            .help("Reports point and face counts, surface area, enclosed volume, bounding box, centroid and open edges.")
            .emulates("Measure / statistics readouts as in 3ds Max Measure utility, Houdini Measure SOP and mesh-statistics panels of RE tools")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let m = measure(&g);
        let report = measure_report(&m, params.bool("require_watertight")?);
        let geo = if params.bool("prim_area")? {
            let mut out = (*g).clone();
            let areas = (0..out.prims.len())
                .map(|i| if out.prims[i].closed { out.prim_area(i) } else { 0.0 })
                .collect();
            out.prim_attribs.set("area", AttribArray::Float(areas));
            Data::geometry(out)
        } else {
            Data::Geometry(g)
        };
        Ok(vec![geo, Data::report(report)])
    }
}
