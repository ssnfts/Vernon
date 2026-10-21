//! Placement operators: transform, mirror, merge.

use vernon_core::{DMat4, DVec3, Geometry};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
    Stage,
};

use crate::util::{axis_index, compact_points, non_negative, reverse_winding, SPEC};

fn geo_in(name: &str) -> PortSpec {
    PortSpec::new(name, DataKind::Geometry)
}

fn geo_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

/// Rotation matrix from Euler angles in degrees, applied one axis at a time
/// in `order` (e.g. "xyz" rotates about X first, then Y, then Z, all about
/// fixed world axes).
pub fn euler_matrix(rot_deg: DVec3, order: &str) -> DMat4 {
    let mut m = DMat4::IDENTITY;
    for c in order.chars() {
        let r = match c {
            'x' => DMat4::from_rotation_x(rot_deg.x.to_radians()),
            'y' => DMat4::from_rotation_y(rot_deg.y.to_radians()),
            'z' => DMat4::from_rotation_z(rot_deg.z.to_radians()),
            _ => DMat4::IDENTITY,
        };
        m = r * m;
    }
    m
}

/// Full transform: scale and rotate about `pivot`, then translate.
pub fn transform_matrix(
    translate: DVec3,
    rotate_deg: DVec3,
    order: &str,
    scale: DVec3,
    pivot: DVec3,
) -> DMat4 {
    DMat4::from_translation(translate + pivot)
        * euler_matrix(rotate_deg, order)
        * DMat4::from_scale(scale)
        * DMat4::from_translation(-pivot)
}

/// Apply `m` to the geometry; with `fix_winding`, a mirroring matrix
/// (negative determinant) also reverses face winding so faces stay
/// outward-facing.
pub fn apply_transform(geo: &mut Geometry, m: DMat4, fix_winding: bool) {
    geo.transform(m);
    if fix_winding && m.determinant() < 0.0 {
        reverse_winding(geo, None);
    }
}

pub struct TransformNode;

const ORDERS: [&str; 6] = ["xyz", "xzy", "yxz", "yzx", "zxy", "zyx"];

impl NodeOp for TransformNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.transform", "Transform", Stage::Geometry)
            .input(geo_in("geometry"))
            .output(geo_out())
            .param(
                ParamSpec::vec3("translate", "Translate", DVec3::ZERO)
                    .unit("mm")
                    .help("Offset applied after rotation and scale."),
            )
            .param(
                ParamSpec::vec3("rotate", "Rotate", DVec3::ZERO)
                    .unit("deg")
                    .help("Rotation about the X, Y and Z axes through the pivot."),
            )
            .param(
                ParamSpec::choice("rotate_order", "Rotation order", "xyz", &ORDERS)
                    .help("Order in which the three axis rotations are applied."),
            )
            .param(
                ParamSpec::vec3("scale", "Scale", DVec3::ONE)
                    .help("Per-axis scale factors about the pivot."),
            )
            .param(
                ParamSpec::float("uniform_scale", "Uniform scale", 1.0, -1000.0, 1000.0)
                    .help("Extra scale factor applied to all three axes."),
            )
            .param(
                ParamSpec::vec3("pivot", "Pivot", DVec3::ZERO)
                    .unit("mm")
                    .help("Point that rotation and scale happen about."),
            )
            .param(
                ParamSpec::bool("fix_winding", "Keep outward winding", true)
                    .help("Reverse faces when the scale mirrors the geometry."),
            )
            .help("Moves, rotates and scales geometry about a pivot; normals follow the transform.")
            .emulates("Object/SOP transform as in 3ds Max XForm and the Houdini Transform SOP")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let order = params.str("rotate_order")?;
        let order = if ORDERS.contains(&order) { order } else { "xyz" };
        let m = transform_matrix(
            params.vec3("translate")?,
            params.vec3("rotate")?,
            order,
            params.vec3("scale")? * params.float("uniform_scale")?,
            params.vec3("pivot")?,
        );
        if !m.is_finite() {
            return Err(vernon_graph::CookError::BadParam("translate/rotate/scale".into()));
        }
        let mut out = (*g).clone();
        apply_transform(&mut out, m, params.bool("fix_winding")?);
        Ok(vec![Data::geometry(out)])
    }
}

// --------------------------------------------------------------- mirror ---

/// Reflect across the plane `axis = origin` (axis 0/1/2 = X/Y/Z).
/// With `keep_original` the reflected copy is appended (its faces in prim
/// group `mirrored`); with `weld` the copy shares the original's points that
/// lie within `tol` of the plane, and those points are snapped onto it.
/// Returns the geometry and the number of welded seam points.
pub fn mirror(geo: &Geometry, axis: usize, origin: f64, keep_original: bool, weld: bool, tol: f64) -> (Geometry, usize) {
    let mut n = DVec3::ZERO;
    n[axis] = 1.0;
    // Householder reflection about the plane through origin * n.
    let refl = DMat4::from_cols(
        (DVec3::X - 2.0 * n * n.x).extend(0.0),
        (DVec3::Y - 2.0 * n * n.y).extend(0.0),
        (DVec3::Z - 2.0 * n * n.z).extend(0.0),
        (n * 2.0 * origin).extend(1.0),
    );
    let mut copy = geo.clone();
    apply_transform(&mut copy, refl, true);
    if !keep_original {
        return (copy, 0);
    }
    let mut src = geo.clone();
    let mut seam = vec![false; geo.points.len()];
    if weld {
        for (i, p) in src.points.iter_mut().enumerate() {
            if (p[axis] - origin).abs() <= tol {
                p[axis] = origin;
                seam[i] = true;
            }
        }
        for (i, p) in copy.points.iter_mut().enumerate() {
            if seam[i] {
                p[axis] = origin;
            }
        }
    }
    let n0 = src.points.len() as u32;
    let f0 = src.prims.len() as u32;
    src.merge(&copy);
    let f1 = src.prims.len() as u32;
    src.prim_groups.insert("mirrored".into(), (f0..f1).collect());
    let welded = seam.iter().filter(|&&s| s).count();
    if welded == 0 {
        return (src, 0);
    }
    for p in src.prims.iter_mut().skip(f0 as usize) {
        for v in &mut p.vertices {
            let orig = *v - n0;
            if seam[orig as usize] {
                *v = orig;
            }
        }
    }
    let mut keep = vec![true; src.points.len()];
    for (i, &s) in seam.iter().enumerate() {
        if s {
            keep[n0 as usize + i] = false;
        }
    }
    (compact_points(&src, &keep), welded)
}

pub struct MirrorNode;

impl NodeOp for MirrorNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.mirror", "Mirror", Stage::Geometry)
            .input(geo_in("geometry"))
            .output(geo_out())
            .param(
                ParamSpec::choice("axis", "Mirror axis", "y", &["x", "y", "z"])
                    .help("Axis normal to the mirror plane; Y mirrors across a vehicle centreline lying along X."),
            )
            .param(
                ParamSpec::float("origin", "Plane offset", 0.0, -1.0e6, 1.0e6)
                    .unit("mm")
                    .help("Position of the mirror plane along the mirror axis."),
            )
            .param(
                ParamSpec::bool("keep_original", "Keep original", true)
                    .help("Output the original plus its reflection instead of the reflection only."),
            )
            .param(
                ParamSpec::bool("weld_seam", "Weld seam", true)
                    .help("Join the two halves where points lie on the mirror plane."),
            )
            .param(
                ParamSpec::float("tolerance", "Seam tolerance", 0.01, 0.0, 100.0)
                    .unit("mm")
                    .help("Points this close to the plane are treated as seam points."),
            )
            .help("Reflects geometry across an axis-aligned plane, keeping faces outward and optionally welding the seam.")
            .emulates("Symmetry / Mirror as in the 3ds Max Symmetry modifier, Houdini Mirror SOP and RE-tool mirrored-symmetry workflows")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let (out, _) = mirror(
            &g,
            axis_index(params.str("axis")?),
            params.float("origin")?,
            params.bool("keep_original")?,
            params.bool("weld_seam")?,
            non_negative(params, "tolerance")?,
        );
        Ok(vec![Data::geometry(out)])
    }
}

// ---------------------------------------------------------------- merge ---

pub struct MergeNode;

impl NodeOp for MergeNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.merge", "Merge", Stage::Geometry)
            .input(geo_in("a"))
            .input(geo_in("b"))
            .input(PortSpec::optional("c", DataKind::Geometry))
            .output(geo_out())
            .help("Combines two or three geometries into one, keeping attributes and groups.")
            .emulates("Attach / combine as in 3ds Max Attach and the Houdini Merge SOP")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, _: &CookContext, ins: &[Option<Data>], _: &Params) -> CookResult<Vec<Data>> {
        let a = inputs::geometry(ins, 0)?;
        let b = inputs::geometry(ins, 1)?;
        let mut out = (*a).clone();
        out.merge(&b);
        if let Some(c) = inputs::opt_geometry(ins, 2) {
            out.merge(&c);
        }
        Ok(vec![Data::geometry(out)])
    }
}
