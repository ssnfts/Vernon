//! Joint nodes: plug and socket, dowel, dovetail and (placeholder) snap fit.
//! They take capped halves from the cut nodes and add features on the seams.

use glam::DVec2;
use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec};

use crate::common::{info, placeholder};
use crate::joints::{apply_joints, circle_prism, dovetail_free_width, trapezoid_prism, JointOutcome, JointShape, JointSpec, Kind};
use crate::mesh;

fn common_params(info: NodeTypeInfo) -> NodeTypeInfo {
    info.input(PortSpec::new("parts", DataKind::Geometry))
        .param(ParamSpec::float("spacing", "Spacing", 40.0, 10.0, 200.0).unit("mm").help("Distance between joints on the seam."))
        .param(ParamSpec::float("wall_min", "Wall margin", 1.5, 0.5, 10.0).unit("mm").help("Smallest wall left between a feature and the edge of the seam."))
        .param(ParamSpec::float("tolerance", "Tolerance", 0.15, 0.05, 0.4).unit("mm").help("Clearance per side between the male feature and its socket."))
        .param(ParamSpec::int("max_count", "Joints per seam", 0, 0, 200).help("Upper limit per seam; 0 fits as many as the spacing allows."))
        .param(ParamSpec::choice("male_side", "Male side", "lower", &["lower", "upper"]).help("Which half carries the protruding feature: the lower or the higher part id."))
        .param(ParamSpec::int("sides", "Facets per circle", 64, 8, 256).help("Polygon count of round features."))
}

fn joint_outputs(info: NodeTypeInfo, pins: bool) -> NodeTypeInfo {
    let info = info.output(PortSpec::new("parts", DataKind::Geometry));
    let info = if pins { info.output(PortSpec::new("pins", DataKind::Geometry)) } else { info };
    info.output(PortSpec::new("report", DataKind::Report))
}

struct Common {
    spacing: f64,
    wall: f64,
    tol: f64,
    max_count: usize,
    male_lower: bool,
    sides: usize,
}

fn read_common(p: &Params) -> CookResult<Common> {
    Ok(Common {
        spacing: p.float("spacing")?,
        wall: p.float("wall_min")?,
        tol: p.float("tolerance")?,
        max_count: p.int("max_count")?.max(0) as usize,
        male_lower: p.str("male_side")? == "lower",
        sides: p.int("sides")?.clamp(8, 256) as usize,
    })
}

fn finish(ctx: &CookContext, title: &str, input: &Geometry, out: JointOutcome, pins_out: bool) -> CookResult<Vec<Data>> {
    if out.pairs == 0 {
        return Err(ctx.fail("no pair of cap faces found: cut the body with capping on first"));
    }
    let mut rep = Report::new(title);
    rep.metric("joint_count", Metric::new(out.joints as f64, "count"));
    rep.metric("pair_count", Metric::new(out.pairs as f64, "count"));
    rep.metric("skipped_count", Metric::new(out.skipped as f64, "count"));
    if pins_out {
        rep.metric("pin_count", Metric::new(mesh::unique_parts(&out.pins).len() as f64, "count"));
    }
    rep.metric("open_edges", Metric::new(mesh::open_edges_by_part(&out.geo) as f64, "count").with_limit(0.0, true));
    rep.metric("volume_change_mm3", Metric::new(mesh::signed_volume(&out.geo) - mesh::signed_volume(input), "mm3"));
    if out.joints == 0 {
        rep.note(Severity::Warning, "no joint fitted: the seam is too small, too thin behind the cap, or the spacing is too large");
    } else if out.skipped > 0 {
        rep.note(Severity::Info, format!("{} candidate positions were rejected (overlap or not enough wall)", out.skipped));
    }
    let mut v = vec![Data::geometry(out.geo)];
    if pins_out {
        v.push(Data::geometry(out.pins));
    }
    v.push(Data::report(rep));
    Ok(v)
}

fn run(ctx: &CookContext, geo: &Geometry, spec: JointSpec, title: &str, pins_out: bool) -> CookResult<Vec<Data>> {
    let out = apply_joints(geo, &spec);
    finish(ctx, title, geo, out, pins_out)
}

fn plug_shape(r: f64, depth: f64, tol: f64, gap: f64, sides: usize) -> Box<dyn Fn(DVec2) -> JointShape> {
    Box::new(move |c| JointShape {
        a: circle_prism(c, r - tol, depth, sides),
        a_kind: Kind::Protrude,
        b: circle_prism(c, r, depth + gap, sides),
        b_kind: Kind::Pocket,
        pin: None,
    })
}

// ------------------------------------------------------------------- plug

pub struct PlugJoint;

impl NodeOp for PlugJoint {
    fn info(&self) -> NodeTypeInfo {
        joint_outputs(
            common_params(
                info("print.plug_joint", "Plug Joint", "Add round plugs on one half of each seam and matching sockets on the other.")
                    .emulates("plug connectors of slicer cut tools")
                    .implemented(),
            )
            .param(ParamSpec::float("radius", "Radius", 4.0, 1.5, 15.0).unit("mm").help("Socket radius."))
            .param(ParamSpec::float("depth", "Depth", 6.0, 2.0, 20.0).unit("mm").help("Height of the plug."))
            .param(ParamSpec::float("gap_z", "Depth gap", 0.3, 0.0, 2.0).unit("mm").help("Extra socket depth so the faces meet before the plug bottoms out.")),
            false,
        )
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let c = read_common(p)?;
        let (r, depth, gap) = (p.float("radius")?, p.float("depth")?, p.float("gap_z")?);
        if r <= c.tol {
            return Err(ctx.fail("the tolerance must be smaller than the radius"));
        }
        let spec = JointSpec {
            shape: plug_shape(r, depth, c.tol, gap, c.sides),
            spacing: c.spacing,
            wall_min: c.wall,
            male_lower: c.male_lower,
            max_count: c.max_count,
            pin_sides: c.sides,
        };
        run(ctx, &geo, spec, "Plug joint", false)
    }
}

// ------------------------------------------------------------------ dowel

pub struct DowelJoint;

impl NodeOp for DowelJoint {
    fn info(&self) -> NodeTypeInfo {
        joint_outputs(
            common_params(
                info("print.dowel_joint", "Dowel Joint", "Drill matching holes in both halves of each seam and output the loose pins to print.")
                    .emulates("dowel connectors of slicer cut tools")
                    .implemented(),
            )
            .param(ParamSpec::float("radius", "Hole radius", 4.0, 1.5, 15.0).unit("mm").help("Radius of the holes."))
            .param(ParamSpec::float("depth", "Hole depth", 8.0, 2.0, 20.0).unit("mm").help("Depth of the hole in each half."))
            .param(ParamSpec::float("gap_z", "Pin gap", 0.3, 0.0, 2.0).unit("mm").help("Total shortfall of the pin against the two holes.")),
            true,
        )
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let c = read_common(p)?;
        let (r, depth, gap) = (p.float("radius")?, p.float("depth")?, p.float("gap_z")?);
        if r <= c.tol {
            return Err(ctx.fail("the tolerance must be smaller than the radius"));
        }
        let pin = (r - c.tol, 2.0 * depth - gap);
        let sides = c.sides;
        let spec = JointSpec {
            shape: Box::new(move |ctr| JointShape {
                a: circle_prism(ctr, r, depth, sides),
                a_kind: Kind::Pocket,
                b: circle_prism(ctr, r, depth, sides),
                b_kind: Kind::Pocket,
                pin: Some(pin),
            }),
            spacing: c.spacing,
            wall_min: c.wall,
            male_lower: c.male_lower,
            max_count: c.max_count,
            pin_sides: sides,
        };
        run(ctx, &geo, spec, "Dowel joint", true)
    }
}

// --------------------------------------------------------------- dovetail

pub struct DovetailJoint;

impl NodeOp for DovetailJoint {
    fn info(&self) -> NodeTypeInfo {
        joint_outputs(
            common_params(
                info("print.dovetail_joint", "Dovetail Joint", "Add flared keys on one half of each seam and matching grooves on the other; several keys can be arrayed along a seam.")
                    .emulates("dovetail mode of slicer cut tools, extended to many keys")
                    .implemented(),
            )
            .param(ParamSpec::float("width", "Neck width", 12.0, 4.0, 60.0).unit("mm").help("Width of the key where it leaves the seam face."))
            .param(ParamSpec::float("depth", "Depth", 6.0, 2.0, 20.0).unit("mm").help("Height of the key."))
            .param(ParamSpec::float("flap_angle", "Flap angle", 15.0, 5.0, 30.0).unit("deg").help("Flare of the key sides away from the seam normal."))
            .param(ParamSpec::float("length", "Length", 20.0, 5.0, 200.0).unit("mm").help("Length of the key along the seam."))
            .param(ParamSpec::float("direction", "Direction", 90.0, 0.0, 180.0).unit("deg").help("Direction of the key length in the seam plane."))
            .param(ParamSpec::int("count", "Keys per seam", 1, 1, 20).help("Number of keys per seam; they are spaced by the spacing value.")),
            false,
        )
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let c = read_common(p)?;
        let (w, d, a, len) = (p.float("width")?, p.float("depth")?, p.float("flap_angle")?, p.float("length")?);
        let t = c.tol;
        let ang = p.float("direction")?.to_radians();
        let dir = DVec2::new(ang.cos(), ang.sin());
        let ta = a.to_radians().tan();
        let w_free = dovetail_free_width(w, d, a);
        let spec = JointSpec {
            shape: Box::new(move |ctr| {
                let hw0 = w / 2.0 + t / a.to_radians().cos();
                JointShape {
                    a: trapezoid_prism(ctr, dir, w / 2.0, w_free / 2.0, len / 2.0, d),
                    a_kind: Kind::Protrude,
                    b: trapezoid_prism(ctr, dir, hw0, hw0 + (d + t) * ta, len / 2.0 + t, d + t),
                    b_kind: Kind::Pocket,
                    pin: None,
                }
            }),
            spacing: c.spacing,
            wall_min: c.wall,
            male_lower: c.male_lower,
            max_count: p.int("count")?.max(1) as usize,
            pin_sides: c.sides,
        };
        let mut v = run(ctx, &geo, spec, "Dovetail joint", false)?;
        if let Some(Data::Report(r)) = v.pop() {
            let mut r = (*r).clone();
            r.metric("free_width_mm", Metric::new(w_free, "mm"));
            r.metric("groove_clearance_mm", Metric::new(t, "mm"));
            r.note(Severity::Info, "grooves are closed at both ends; join the halves with a snug fit or glue");
            v.push(Data::report(r));
        }
        Ok(v)
    }
}

// ------------------------------------------------------------------- snap

pub struct SnapJoint;

impl NodeOp for SnapJoint {
    fn info(&self) -> NodeTypeInfo {
        joint_outputs(
            common_params(
                info("print.snap_joint", "Snap Joint", "Add snap-fit arms on one half of each seam and recesses on the other.")
                    .emulates("snap connectors of slicer cut tools"),
            )
            .param(ParamSpec::float("radius", "Radius", 4.0, 1.5, 15.0).unit("mm").help("Recess radius."))
            .param(ParamSpec::float("arm_length", "Arm length", 8.0, 3.0, 30.0).unit("mm").help("Length of the flexing arm."))
            .param(ParamSpec::float("flex_room", "Flex room", 0.6, 0.1, 3.0).unit("mm").help("Space left for the arm to bend."))
            .param(ParamSpec::float("bulge", "Bulge", 0.4, 0.1, 2.0).unit("mm").help("How far the catch overshoots into the recess.")),
            false,
        )
    }

    fn cook(&self, ctx: &CookContext, inp: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
        let geo = inputs::geometry(inp, 0)?;
        let c = read_common(p)?;
        let (r, arm) = (p.float("radius")?, p.float("arm_length")?);
        if r <= c.tol {
            return Err(ctx.fail("the tolerance must be smaller than the radius"));
        }
        let spec = JointSpec {
            shape: plug_shape(r, arm, c.tol, p.float("flex_room")?.min(1.0), c.sides),
            spacing: c.spacing,
            wall_min: c.wall,
            male_lower: c.male_lower,
            max_count: c.max_count,
            pin_sides: c.sides,
        };
        let mut v = run(ctx, &geo, spec, "Snap joint", false)?;
        if let Some(Data::Report(r)) = v.pop() {
            let mut r = (*r).clone();
            placeholder(&mut r, "a plain round plug and socket stands in; the real node cuts a flexing arm with a catch (bulge) and a recess with flex room");
            v.push(Data::report(r));
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::testutil::cook;
    use crate::cut::{cut_all, Keep, Plane};
    use crate::mesh::testutil::tri_box;
    use glam::DVec3;
    use vernon_graph::ParamValue;

    pub fn halves() -> Geometry {
        let cube = tri_box(DVec3::ZERO, DVec3::splat(100.0));
        let plane = Plane::new(DVec3::new(0.0, 0.0, 50.0), DVec3::Z).unwrap();
        cut_all(&cube, &[plane], 1e-6, true, Keep::Both).unwrap().geo
    }

    fn input() -> Vec<Option<Data>> {
        vec![Some(Data::geometry(halves()))]
    }

    fn metric(d: &Data, name: &str) -> f64 {
        d.as_report().unwrap().metrics[name].value
    }

    #[test]
    fn plug_node_counts_and_closed_parts() {
        let out = cook(&PlugJoint, "plug", &input(), &[]).unwrap();
        assert_eq!(out.len(), 2);
        assert!(metric(&out[1], "joint_count") >= 5.0);
        assert_eq!(metric(&out[1], "open_edges"), 0.0);
        assert!(out[1].as_report().unwrap().passes());
    }

    #[test]
    fn dowel_node_outputs_one_pin_per_joint() {
        let out = cook(&DowelJoint, "dowel", &input(), &[]).unwrap();
        assert_eq!(out.len(), 3);
        let n = metric(&out[2], "joint_count") as usize;
        assert!(n >= 5);
        assert_eq!(mesh::unique_parts(out[1].as_geometry().unwrap()).len(), n);
        // pin radius is hole radius minus tolerance, length 2 * depth - gap
        let pins = out[1].as_geometry().unwrap();
        let first = mesh::extract_part(pins, 0).bounds();
        assert!((first.size().z - 15.7).abs() < 1e-9);
        assert!((first.size().x - 2.0 * 3.85).abs() < 0.01);
    }

    #[test]
    fn dovetail_node_array_and_report() {
        let out = cook(&DovetailJoint, "dt", &input(), &[("count", ParamValue::Int(2)), ("spacing", ParamValue::Float(30.0))]).unwrap();
        assert_eq!(metric(&out[1], "joint_count"), 2.0);
        assert!((metric(&out[1], "free_width_mm") - 15.2154).abs() < 1e-3);
        assert_eq!(metric(&out[1], "open_edges"), 0.0);
    }

    #[test]
    fn snap_node_is_a_flagged_placeholder() {
        let out = cook(&SnapJoint, "snap", &input(), &[]).unwrap();
        assert!(out[1].as_report().unwrap().notes.iter().any(|(s, t)| *s == Severity::Warning && t.starts_with("white-box placeholder")));
    }

    #[test]
    fn grid_split_then_dowels_on_every_interface() {
        use crate::nodes_split::GridPartition;
        let slab = tri_box(DVec3::ZERO, DVec3::new(300.0, 200.0, 60.0));
        let set: &[(&str, ParamValue)] = &[("printer", ParamValue::Str("custom".into())), ("bed", ParamValue::Vec3(DVec3::new(160.0, 160.0, 100.0)))];
        let grid = cook(&GridPartition, "grid_for_dowels", &[Some(Data::geometry(slab.clone()))], set).unwrap();
        assert_eq!(metric(&grid[2], "part_count"), 4.0);
        assert_eq!(metric(&grid[2], "interface_count"), 4.0);
        let out = cook(&DowelJoint, "grid_dowels", &[Some(grid[0].clone())], &[]).unwrap();
        let n = metric(&out[2], "joint_count");
        assert!(n >= 4.0, "joints {n}");
        assert_eq!(metric(&out[2], "pin_count"), n);
        assert_eq!(metric(&out[2], "open_edges"), 0.0);
        // every pin lies inside the original slab
        let b = slab.bounds();
        let pins = out[1].as_geometry().unwrap();
        for p in &pins.points {
            assert!(p.cmpge(b.min).all() && p.cmple(b.max).all(), "pin point {p:?} outside");
        }
        // each pocket removes volume from the parts; the total drops by 2 pockets per joint
        let pocket = 0.5 * 64.0 * 16.0 * (std::f64::consts::TAU / 64.0).sin() * 8.0;
        let vol_in = mesh::signed_volume(&slab);
        let vol_out = mesh::signed_volume(out[0].as_geometry().unwrap());
        assert!((vol_in - vol_out - 2.0 * n * pocket).abs() < 1e-6, "{vol_in} {vol_out}");
    }

    #[test]
    fn uncapped_input_is_a_clean_error() {
        let cube = tri_box(DVec3::ZERO, DVec3::splat(100.0));
        assert!(cook(&PlugJoint, "nocap", &[Some(Data::geometry(cube))], &[]).is_err());
    }
}
