//! Primitive generators: box, grid, sphere, tube and the parametric car panel
//! test surface. The generator functions are public so other crates' tests can
//! build synthetic inputs without a graph.

use std::f64::consts::PI;

use vernon_core::{AttribArray, DVec3, Geometry, Prim};
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
    Stage,
};

use crate::topology::fuse_points;
use crate::util::{axis_index, int_clamped, non_negative, SPEC};

/// Append a `cols` x `rows` quad grid spanning `origin + u*s + v*t`
/// (s, t in 0..1). Faces wind counter-clockwise around `u x v`.
fn add_grid(geo: &mut Geometry, origin: DVec3, u: DVec3, v: DVec3, cols: usize, rows: usize) {
    let base = geo.points.len() as u32;
    for j in 0..=rows {
        for i in 0..=cols {
            let s = i as f64 / cols as f64;
            let t = j as f64 / rows as f64;
            geo.points.push(origin + u * s + v * t);
        }
    }
    let w = cols as u32 + 1;
    for j in 0..rows as u32 {
        for i in 0..cols as u32 {
            let a = base + j * w + i;
            geo.prims.push(Prim::polygon(vec![a, a + 1, a + 1 + w, a + w]));
        }
    }
}

/// Closed box of `size` centred at `center`, each face split into
/// `divisions` x `divisions` quads, outward winding, shared seam points.
pub fn make_box(size: DVec3, center: DVec3, divisions: usize) -> Geometry {
    let n = divisions.max(1);
    let s = size.abs();
    let h = s * 0.5;
    let (x, y, z) = (DVec3::X * s.x, DVec3::Y * s.y, DVec3::Z * s.z);
    let lo = center - h;
    let mut g = Geometry::new();
    // (origin, u, v) with u x v pointing outward.
    let faces = [
        (lo + x, y, z), // +X
        (lo, z, y),     // -X
        (lo + y, z, x), // +Y
        (lo, x, z),     // -Y
        (lo + z, x, y), // +Z
        (lo, y, x),     // -Z
    ];
    for (o, u, v) in faces {
        add_grid(&mut g, o, u, v, n, n);
    }
    let tol = 1e-9 * s.max_element().max(1e-9);
    fuse_points(&g, tol, false, false).0
}

/// Which plane a grid lies in. The grid normal is the remaining axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridPlane {
    /// Normal +Z.
    Xy,
    /// Normal +X.
    Yz,
    /// Normal +Y.
    Zx,
}

impl GridPlane {
    pub fn parse(s: &str) -> Self {
        match s {
            "yz" => GridPlane::Yz,
            "zx" => GridPlane::Zx,
            _ => GridPlane::Xy,
        }
    }
}

/// Flat quad grid of `size_u` x `size_v` with `cols` x `rows` cells, centred
/// at `center`, with a point `uv` attribute.
pub fn make_grid(
    size_u: f64,
    size_v: f64,
    cols: usize,
    rows: usize,
    plane: GridPlane,
    center: DVec3,
) -> Geometry {
    let (cols, rows) = (cols.max(1), rows.max(1));
    let (du, dv) = match plane {
        GridPlane::Xy => (DVec3::X, DVec3::Y),
        GridPlane::Yz => (DVec3::Y, DVec3::Z),
        GridPlane::Zx => (DVec3::Z, DVec3::X),
    };
    let u = du * size_u;
    let v = dv * size_v;
    let mut g = Geometry::new();
    add_grid(&mut g, center - (u + v) * 0.5, u, v, cols, rows);
    let uv = (0..=rows)
        .flat_map(|j| {
            (0..=cols).map(move |i| DVec3::new(i as f64 / cols as f64, j as f64 / rows as f64, 0.0))
        })
        .collect();
    g.point_attribs.set("uv", AttribArray::Vec3(uv));
    g
}

/// Closed UV sphere: `rows` latitude bands (>= 2) and `cols` longitude
/// segments (>= 3), pole triangles and quads elsewhere, outward winding.
pub fn make_sphere(radius: f64, rows: usize, cols: usize, center: DVec3) -> Geometry {
    let rows = rows.max(2);
    let cols = cols.max(3);
    let mut g = Geometry::new();
    g.points.push(center + DVec3::Z * radius);
    for j in 1..rows {
        let theta = PI * j as f64 / rows as f64;
        for i in 0..cols {
            let phi = 2.0 * PI * i as f64 / cols as f64;
            let d = DVec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), theta.cos());
            g.points.push(center + d * radius);
        }
    }
    g.points.push(center - DVec3::Z * radius);
    let south = (g.points.len() - 1) as u32;
    let ring = |j: usize, i: usize| -> u32 { (1 + (j - 1) * cols + (i % cols)) as u32 };
    for i in 0..cols {
        g.prims.push(Prim::polygon(vec![0, ring(1, i), ring(1, i + 1)]));
    }
    for j in 1..rows - 1 {
        for i in 0..cols {
            g.prims.push(Prim::polygon(vec![
                ring(j, i),
                ring(j + 1, i),
                ring(j + 1, i + 1),
                ring(j, i + 1),
            ]));
        }
    }
    for i in 0..cols {
        g.prims
            .push(Prim::polygon(vec![ring(rows - 1, i), south, ring(rows - 1, i + 1)]));
    }
    g
}

/// Tube / cylinder / cone frustum along `axis` (0 = X, 1 = Y, 2 = Z),
/// `rows` height segments, `cols` sides, optional end caps.
#[allow(clippy::too_many_arguments)]
pub fn make_tube(
    radius_bottom: f64,
    radius_top: f64,
    height: f64,
    rows: usize,
    cols: usize,
    caps: bool,
    axis: usize,
    center: DVec3,
) -> Geometry {
    let rows = rows.max(1);
    let cols = cols.max(3);
    let mut g = Geometry::new();
    for k in 0..=rows {
        let t = k as f64 / rows as f64;
        let z = -0.5 * height + height * t;
        let r = radius_bottom + (radius_top - radius_bottom) * t;
        for i in 0..cols {
            let phi = 2.0 * PI * i as f64 / cols as f64;
            g.points.push(DVec3::new(r * phi.cos(), r * phi.sin(), z));
        }
    }
    let idx = |k: usize, i: usize| -> u32 { (k * cols + i % cols) as u32 };
    for k in 0..rows {
        for i in 0..cols {
            g.prims.push(Prim::polygon(vec![
                idx(k, i),
                idx(k, i + 1),
                idx(k + 1, i + 1),
                idx(k + 1, i),
            ]));
        }
    }
    if caps {
        g.prims
            .push(Prim::polygon((0..cols).rev().map(|i| idx(0, i)).collect()));
        g.prims
            .push(Prim::polygon((0..cols).map(|i| idx(rows, i)).collect()));
    }
    // Cyclic axis permutation keeps the winding orientation.
    for p in &mut g.points {
        *p = match axis {
            0 => DVec3::new(p.z, p.x, p.y),
            1 => DVec3::new(p.y, p.z, p.x),
            _ => *p,
        } + center;
    }
    g
}

/// Parameters of the car panel test surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarPanel {
    /// Extent along X (vehicle length direction), mm.
    pub length: f64,
    /// Extent along Y (across the panel), mm.
    pub width: f64,
    /// Peak height of the central crown bulge, mm.
    pub crown: f64,
    /// Peak outward lift of the flared outboard (+Y) edge at mid-length, mm.
    pub flare: f64,
    /// Segments along X.
    pub cols: usize,
    /// Segments along Y.
    pub rows: usize,
    /// Shell thickness in mm; 0 gives a single open surface.
    pub thickness: f64,
}

impl Default for CarPanel {
    fn default() -> Self {
        Self {
            length: 900.0,
            width: 600.0,
            crown: 40.0,
            flare: 60.0,
            cols: 36,
            rows: 24,
            thickness: 0.0,
        }
    }
}

impl CarPanel {
    /// Height (along +Z) of the outer surface at plan position (x, y).
    ///
    /// With `s = 2x/length` in [-1, 1] and `t = y/width + 1/2` in [0, 1]:
    /// `z = crown (1 - s^2)(1 - (2t - 1)^2) + flare * smoothstep(1/2, 1, t) * (1 - s^2)^2`.
    /// So `z = crown` at the centre, `z = flare` at the middle of the +Y
    /// edge, and `z = 0` along the -Y edge and both X ends.
    pub fn height(&self, x: f64, y: f64) -> f64 {
        let s = if self.length.abs() > 0.0 { 2.0 * x / self.length } else { 0.0 };
        let t = if self.width.abs() > 0.0 { y / self.width + 0.5 } else { 0.5 };
        let bell = (1.0 - s * s).max(0.0);
        let across = (1.0 - (2.0 * t - 1.0).powi(2)).max(0.0);
        let e = ((t - 0.5) / 0.5).clamp(0.0, 1.0);
        let smooth = e * e * (3.0 - 2.0 * e);
        self.crown * bell * across + self.flare * smooth * bell * bell
    }

    /// Build the geometry: point `N` and `uv`; when `thickness > 0` a closed
    /// shell with prim groups `outer`, `inner` and `edge`.
    pub fn build(&self) -> Geometry {
        let cols = self.cols.max(1);
        let rows = self.rows.max(1);
        let mut g = Geometry::new();
        let mut uv = Vec::new();
        for j in 0..=rows {
            for i in 0..=cols {
                let u = i as f64 / cols as f64;
                let v = j as f64 / rows as f64;
                let x = (u - 0.5) * self.length;
                let y = (v - 0.5) * self.width;
                g.points.push(DVec3::new(x, y, self.height(x, y)));
                uv.push(DVec3::new(u, v, 0.0));
            }
        }
        let w = cols as u32 + 1;
        for j in 0..rows as u32 {
            for i in 0..cols as u32 {
                let a = j * w + i;
                g.prims.push(Prim::polygon(vec![a, a + 1, a + 1 + w, a + w]));
            }
        }
        g.point_attribs.set("uv", AttribArray::Vec3(uv));
        g.compute_normals();
        g.prim_attribs.remove("N");
        if self.thickness <= 0.0 {
            return g;
        }

        let n_outer = g.points.len() as u32;
        let f_outer = g.prims.len() as u32;
        let normals = g.point_attribs.vec3s("N").expect("normals").to_vec();
        let uvs = g.point_attribs.vec3s("uv").expect("uv").to_vec();
        let inner: Vec<DVec3> = g
            .points
            .iter()
            .zip(&normals)
            .map(|(p, n)| *p - *n * self.thickness)
            .collect();
        g.points.extend(inner);
        let inner_faces: Vec<Prim> = g.prims[..f_outer as usize]
            .iter()
            .map(|p| Prim::polygon(p.vertices.iter().rev().map(|&v| v + n_outer).collect()))
            .collect();
        g.prims.extend(inner_faces);
        let f_inner = g.prims.len() as u32;

        // Boundary loop, counter-clockwise seen from +Z.
        let id = |i: u32, j: u32| j * w + i;
        let (c, r) = (cols as u32, rows as u32);
        let mut lp: Vec<u32> = (0..c).map(|i| id(i, 0)).collect();
        lp.extend((0..r).map(|j| id(c, j)));
        lp.extend((1..=c).rev().map(|i| id(i, r)));
        lp.extend((1..=r).rev().map(|j| id(0, j)));
        for k in 0..lp.len() {
            let a = lp[k];
            let b = lp[(k + 1) % lp.len()];
            g.prims
                .push(Prim::polygon(vec![b, a, a + n_outer, b + n_outer]));
        }
        let f_all = g.prims.len() as u32;

        let mut n_all = normals.clone();
        n_all.extend(normals.iter().map(|n| -*n));
        let mut uv_all = uvs.clone();
        uv_all.extend(uvs);
        g.point_attribs.set("N", AttribArray::Vec3(n_all));
        g.point_attribs.set("uv", AttribArray::Vec3(uv_all));
        g.prim_groups.insert("outer".into(), (0..f_outer).collect());
        g.prim_groups.insert("inner".into(), (f_outer..f_inner).collect());
        g.prim_groups.insert("edge".into(), (f_inner..f_all).collect());
        g
    }
}

/// Convenience: the default car panel surface (open, 900 x 600 mm).
pub fn car_panel_default() -> Geometry {
    CarPanel::default().build()
}

fn geo_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

fn center_param() -> ParamSpec {
    ParamSpec::vec3("center", "Center", DVec3::ZERO)
        .unit("mm")
        .help("Position of the shape's centre.")
}

// ---------------------------------------------------------------- nodes ---

pub struct BoxNode;

impl NodeOp for BoxNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.box", "Box", Stage::Geometry)
            .output(geo_out())
            .param(
                ParamSpec::vec3("size", "Size", DVec3::splat(100.0))
                    .unit("mm")
                    .help("Edge lengths along X, Y and Z."),
            )
            .param(center_param())
            .param(
                ParamSpec::int("divisions", "Divisions", 1, 1, 256)
                    .help("Quads per edge on every face."),
            )
            .help("Closed box made of outward-facing quads, optionally subdivided per face.")
            .emulates("Box primitive of general-purpose DCC tools (e.g. 3ds Max Box, Houdini Box SOP)")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, _: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = make_box(
            params.vec3("size")?,
            params.vec3("center")?,
            int_clamped(params, "divisions", 1, 256)?,
        );
        Ok(vec![Data::geometry(g)])
    }
}

pub struct GridNode;

impl NodeOp for GridNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.grid", "Grid", Stage::Geometry)
            .output(geo_out())
            .param(
                ParamSpec::float("size_u", "Width", 1000.0, 0.0, 1.0e6)
                    .unit("mm")
                    .help("Extent along the plane's first axis."),
            )
            .param(
                ParamSpec::float("size_v", "Length", 1000.0, 0.0, 1.0e6)
                    .unit("mm")
                    .help("Extent along the plane's second axis."),
            )
            .param(ParamSpec::int("cols", "Columns", 10, 1, 4096).help("Cells along the first axis."))
            .param(ParamSpec::int("rows", "Rows", 10, 1, 4096).help("Cells along the second axis."))
            .param(
                ParamSpec::choice("plane", "Plane", "xy", &["xy", "yz", "zx"])
                    .help("Plane the grid lies in; its normal is the remaining axis."),
            )
            .param(center_param())
            .help("Flat subdivided plane of quads with a uv attribute.")
            .emulates("Grid / Plane primitive of general-purpose DCC tools (e.g. 3ds Max Plane, Houdini Grid SOP)")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, _: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = make_grid(
            non_negative(params, "size_u")?,
            non_negative(params, "size_v")?,
            int_clamped(params, "cols", 1, 4096)?,
            int_clamped(params, "rows", 1, 4096)?,
            GridPlane::parse(params.str("plane")?),
            params.vec3("center")?,
        );
        Ok(vec![Data::geometry(g)])
    }
}

pub struct SphereNode;

impl NodeOp for SphereNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.sphere", "Sphere", Stage::Geometry)
            .output(geo_out())
            .param(
                ParamSpec::float("radius", "Radius", 50.0, 0.0, 1.0e6)
                    .unit("mm")
                    .help("Sphere radius."),
            )
            .param(
                ParamSpec::int("rows", "Rows", 16, 2, 1024)
                    .help("Latitude bands from pole to pole."),
            )
            .param(
                ParamSpec::int("cols", "Columns", 32, 3, 2048)
                    .help("Longitude segments around the axis."),
            )
            .param(center_param())
            .help("Closed latitude/longitude sphere with triangle fans at the poles.")
            .emulates("UV sphere primitive of general-purpose DCC tools (e.g. 3ds Max Sphere, Houdini Sphere SOP polygon mode)")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, _: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = make_sphere(
            non_negative(params, "radius")?,
            int_clamped(params, "rows", 2, 1024)?,
            int_clamped(params, "cols", 3, 2048)?,
            params.vec3("center")?,
        );
        Ok(vec![Data::geometry(g)])
    }
}

pub struct TubeNode;

impl NodeOp for TubeNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.tube", "Tube", Stage::Geometry)
            .output(geo_out())
            .param(
                ParamSpec::float("radius_bottom", "Bottom radius", 25.0, 0.0, 1.0e6)
                    .unit("mm")
                    .help("Radius at the low end of the axis."),
            )
            .param(
                ParamSpec::float("radius_top", "Top radius", 25.0, 0.0, 1.0e6)
                    .unit("mm")
                    .help("Radius at the high end; differ from the bottom for a cone frustum."),
            )
            .param(
                ParamSpec::float("height", "Height", 100.0, 0.0, 1.0e6)
                    .unit("mm")
                    .help("Length along the axis."),
            )
            .param(ParamSpec::int("rows", "Height segments", 1, 1, 1024).help("Segments along the axis."))
            .param(ParamSpec::int("cols", "Sides", 24, 3, 2048).help("Segments around the axis."))
            .param(ParamSpec::bool("caps", "End caps", true).help("Close both ends with polygons."))
            .param(
                ParamSpec::choice("axis", "Axis", "z", &["x", "y", "z"])
                    .help("Axis the tube runs along."),
            )
            .param(center_param())
            .help("Cylinder, cone frustum or open pipe built from quads with optional polygon caps.")
            .emulates("Tube / Cylinder primitive of general-purpose DCC tools (e.g. 3ds Max Cylinder and Tube, Houdini Tube SOP)")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, _: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = make_tube(
            non_negative(params, "radius_bottom")?,
            non_negative(params, "radius_top")?,
            params.float("height")?,
            int_clamped(params, "rows", 1, 1024)?,
            int_clamped(params, "cols", 3, 2048)?,
            params.bool("caps")?,
            axis_index(params.str("axis")?),
            params.vec3("center")?,
        );
        Ok(vec![Data::geometry(g)])
    }
}

pub struct CarPanelNode;

impl NodeOp for CarPanelNode {
    fn info(&self) -> NodeTypeInfo {
        let d = CarPanel::default();
        NodeTypeInfo::new("geo.car_panel", "Car Panel", Stage::Geometry)
            .output(geo_out())
            .param(
                ParamSpec::float("length", "Length", d.length, 1.0, 1.0e5)
                    .unit("mm")
                    .help("Extent along X, the vehicle length direction."),
            )
            .param(
                ParamSpec::float("width", "Width", d.width, 1.0, 1.0e5)
                    .unit("mm")
                    .help("Extent along Y, across the panel."),
            )
            .param(
                ParamSpec::float("crown", "Crown", d.crown, -1.0e4, 1.0e4)
                    .unit("mm")
                    .help("Height of the central bulge above the panel edges."),
            )
            .param(
                ParamSpec::float("flare", "Flare", d.flare, -1.0e4, 1.0e4)
                    .unit("mm")
                    .help("Outward lift of the +Y edge at mid-length, like a wheel-arch flare."),
            )
            .param(
                ParamSpec::int("cols", "Length segments", d.cols as i64, 1, 4096)
                    .help("Quads along the length."),
            )
            .param(
                ParamSpec::int("rows", "Width segments", d.rows as i64, 1, 4096)
                    .help("Quads across the width."),
            )
            .param(
                ParamSpec::float("thickness", "Thickness", d.thickness, 0.0, 1000.0)
                    .unit("mm")
                    .help("Shell thickness; 0 makes a single open surface, above 0 a closed solid."),
            )
            .help("Doubly curved fender-like test surface with a crown and a flared edge, optionally thickened into a closed shell.")
            .emulates("Synthetic body-panel stand-in for a scanned fender, used to exercise every pipeline stage without real scan data")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, _: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let p = CarPanel {
            length: params.float("length")?.abs(),
            width: params.float("width")?.abs(),
            crown: params.float("crown")?,
            flare: params.float("flare")?,
            cols: int_clamped(params, "cols", 1, 4096)?,
            rows: int_clamped(params, "rows", 1, 4096)?,
            thickness: non_negative(params, "thickness")?,
        };
        Ok(vec![Data::geometry(p.build())])
    }
}
