//! Selection-driven operators: delete, group, attribute create.

use vernon_core::{AttribArray, AttribTable, DVec3, Geometry};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
    Stage,
};

use crate::util::{filter_points, filter_prims, compact_points, in_box, point_normals, prim_centroid, used_points, SPEC};

fn geo_in() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

fn geo_out() -> PortSpec {
    PortSpec::new("geometry", DataKind::Geometry)
}

/// Element class a selection applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Points,
    Prims,
}

impl Element {
    fn parse(s: &str) -> Self {
        if s == "points" {
            Element::Points
        } else {
            Element::Prims
        }
    }

    fn count(self, geo: &Geometry) -> usize {
        match self {
            Element::Points => geo.points.len(),
            Element::Prims => geo.prims.len(),
        }
    }
}

/// Membership mask of a named group. Empty name selects nothing.
pub fn group_mask(geo: &Geometry, element: Element, name: &str) -> Result<Vec<bool>, String> {
    let mut mask = vec![false; element.count(geo)];
    let name = name.trim();
    if name.is_empty() {
        return Ok(mask);
    }
    let groups = match element {
        Element::Points => &geo.point_groups,
        Element::Prims => &geo.prim_groups,
    };
    let idx = groups.get(name).ok_or_else(|| {
        format!(
            "no {} group named '{name}'",
            if element == Element::Points { "point" } else { "primitive" }
        )
    })?;
    for &i in idx {
        if let Some(m) = mask.get_mut(i as usize) {
            *m = true;
        }
    }
    Ok(mask)
}

/// Elements inside an axis-aligned box (primitives by centroid).
pub fn bbox_mask(geo: &Geometry, element: Element, center: DVec3, size: DVec3) -> Vec<bool> {
    match element {
        Element::Points => geo.points.iter().map(|&p| in_box(p, center, size)).collect(),
        Element::Prims => (0..geo.prims.len())
            .map(|i| !geo.prims[i].vertices.is_empty() && in_box(prim_centroid(geo, i), center, size))
            .collect(),
    }
}

/// Elements whose normal is within `spread_deg` of `direction`.
pub fn normal_mask(geo: &Geometry, element: Element, direction: DVec3, spread_deg: f64) -> Vec<bool> {
    let d = direction.normalize_or_zero();
    if d == DVec3::ZERO {
        return vec![false; element.count(geo)];
    }
    let cos_lim = spread_deg.clamp(0.0, 180.0).to_radians().cos();
    let test = |n: DVec3| n != DVec3::ZERO && n.normalize().dot(d) >= cos_lim - 1e-12;
    match element {
        Element::Points => point_normals(geo).into_iter().map(test).collect(),
        Element::Prims => (0..geo.prims.len())
            .map(|i| {
                let p = &geo.prims[i];
                p.closed && p.vertices.len() >= 3 && test(geo.prim_normal(i))
            })
            .collect(),
    }
}

// --------------------------------------------------------------- delete ---

pub struct DeleteNode;

impl NodeOp for DeleteNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.delete", "Delete", Stage::Geometry)
            .input(geo_in())
            .output(geo_out())
            .param(
                ParamSpec::choice("mode", "Select by", "group", &["group", "bbox"])
                    .help("Choose elements by group name or by an axis-aligned box."),
            )
            .param(
                ParamSpec::choice("element", "Element", "prims", &["prims", "points"])
                    .help("Delete faces/lines or points. Deleting a point deletes the faces using it."),
            )
            .param(ParamSpec::string("group", "Group", "").help("Group to delete in group mode."))
            .param(
                ParamSpec::vec3("bbox_center", "Box center", DVec3::ZERO)
                    .unit("mm")
                    .help("Centre of the selection box in box mode."),
            )
            .param(
                ParamSpec::vec3("bbox_size", "Box size", DVec3::splat(100.0))
                    .unit("mm")
                    .help("Size of the selection box; faces count as inside when their centroid is."),
            )
            .param(
                ParamSpec::bool("invert", "Delete non-selected", false)
                    .help("Keep the selection and delete everything else."),
            )
            .param(
                ParamSpec::bool("remove_unused", "Delete unused points", true)
                    .help("Remove points left without any face after deleting faces."),
            )
            .help("Removes faces or points chosen by group or by a box region.")
            .emulates("Delete by selection as in 3ds Max Delete Mesh / Vol. Select and the Houdini Blast SOP")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let element = Element::parse(params.str("element")?);
        let mut sel = if params.str("mode")? == "bbox" {
            bbox_mask(&g, element, params.vec3("bbox_center")?, params.vec3("bbox_size")?)
        } else {
            group_mask(&g, element, params.str("group")?).map_err(|e| ctx.fail(e))?
        };
        if params.bool("invert")? {
            sel.iter_mut().for_each(|s| *s = !*s);
        }
        let keep: Vec<bool> = sel.iter().map(|s| !s).collect();
        let mut out = match element {
            Element::Points => filter_points(&g, &keep),
            Element::Prims => filter_prims(&g, &keep),
        };
        if params.bool("remove_unused")? {
            // Only drop points that lost their last face: free points (a
            // point cloud) stay.
            let was = used_points(&g);
            let was = match element {
                Element::Points => was
                    .iter()
                    .zip(&keep)
                    .filter(|(_, &k)| k)
                    .map(|(&w, _)| w)
                    .collect(),
                Element::Prims => was,
            };
            let now = used_points(&out);
            let keep_pt: Vec<bool> = was.iter().zip(&now).map(|(&w, &n)| !w || n).collect();
            out = compact_points(&out, &keep_pt);
        }
        Ok(vec![Data::geometry(out)])
    }
}

// ---------------------------------------------------------------- group ---

pub struct GroupNode;

impl NodeOp for GroupNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.group", "Group", Stage::Geometry)
            .input(geo_in())
            .output(geo_out())
            .param(ParamSpec::string("name", "Group name", "group1").help("Name of the group to create or edit."))
            .param(
                ParamSpec::choice("element", "Element", "prims", &["prims", "points"])
                    .help("Group faces/lines or points."),
            )
            .param(
                ParamSpec::choice("method", "Select by", "bbox", &["bbox", "normal"])
                    .help("Choose elements inside a box or facing a direction."),
            )
            .param(
                ParamSpec::vec3("bbox_center", "Box center", DVec3::ZERO)
                    .unit("mm")
                    .help("Centre of the selection box."),
            )
            .param(
                ParamSpec::vec3("bbox_size", "Box size", DVec3::splat(100.0))
                    .unit("mm")
                    .help("Size of the selection box; faces count as inside when their centroid is."),
            )
            .param(
                ParamSpec::vec3("direction", "Direction", DVec3::Z)
                    .help("Elements whose normal points this way are selected."),
            )
            .param(
                ParamSpec::float("spread", "Spread angle", 30.0, 0.0, 180.0)
                    .unit("deg")
                    .help("Largest angle between an element normal and the direction."),
            )
            .param(
                ParamSpec::choice(
                    "combine",
                    "Combine",
                    "replace",
                    &["replace", "union", "intersect", "subtract"],
                )
                .help("How the new selection combines with an existing group of the same name."),
            )
            .help("Creates a named point or face group from a box region or a normal direction.")
            .emulates("Named selection sets as in 3ds Max Vol. Select / Select by Normal and the Houdini Group SOP (bounding and normal modes)")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let name = params.str("name")?.trim().to_string();
        if name.is_empty() {
            return Err(ctx.fail("group name is empty"));
        }
        let element = Element::parse(params.str("element")?);
        let sel = if params.str("method")? == "normal" {
            normal_mask(&g, element, params.vec3("direction")?, params.float("spread")?)
        } else {
            bbox_mask(&g, element, params.vec3("bbox_center")?, params.vec3("bbox_size")?)
        };
        let existing = group_mask(&g, element, &name).unwrap_or_else(|_| vec![false; sel.len()]);
        let combined: Vec<bool> = match params.str("combine")? {
            "union" => sel.iter().zip(&existing).map(|(&s, &e)| s || e).collect(),
            "intersect" => sel.iter().zip(&existing).map(|(&s, &e)| s && e).collect(),
            "subtract" => sel.iter().zip(&existing).map(|(&s, &e)| e && !s).collect(),
            _ => sel,
        };
        let idx: Vec<u32> = combined
            .iter()
            .enumerate()
            .filter(|(_, &s)| s)
            .map(|(i, _)| i as u32)
            .collect();
        let mut out = (*g).clone();
        match element {
            Element::Points => out.point_groups.insert(name, idx),
            Element::Prims => out.prim_groups.insert(name, idx),
        };
        Ok(vec![Data::geometry(out)])
    }
}

// ----------------------------------------------------- attribute create ---

/// Set attribute `name` on the elements selected by `mask` (all when `None`),
/// creating or retyping the column as needed. Unselected elements keep their
/// previous value, or zero when the column is new or retyped.
pub fn set_attrib(table: &mut AttribTable, len: usize, name: &str, value: &AttribArray, mask: Option<&[bool]>) {
    let ty = value.ty();
    let fresh = match table.get(name) {
        Some(col) => col.ty() != ty || col.len() != len,
        None => true,
    };
    if fresh {
        table.set(name, AttribArray::new(ty, len));
    }
    let col = table.get_mut(name).expect("column exists");
    let on = |i: usize| mask.is_none_or(|m| m.get(i).copied().unwrap_or(false));
    match (col, value) {
        (AttribArray::Float(c), AttribArray::Float(v)) => {
            c.iter_mut().enumerate().filter(|(i, _)| on(*i)).for_each(|(_, x)| *x = v[0])
        }
        (AttribArray::Int(c), AttribArray::Int(v)) => {
            c.iter_mut().enumerate().filter(|(i, _)| on(*i)).for_each(|(_, x)| *x = v[0])
        }
        (AttribArray::Vec3(c), AttribArray::Vec3(v)) => {
            c.iter_mut().enumerate().filter(|(i, _)| on(*i)).for_each(|(_, x)| *x = v[0])
        }
        (AttribArray::Str(c), AttribArray::Str(v)) => c
            .iter_mut()
            .enumerate()
            .filter(|(i, _)| on(*i))
            .for_each(|(_, x)| *x = v[0].clone()),
        _ => {}
    }
}

fn valid_attrib_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub struct AttribCreateNode;

impl NodeOp for AttribCreateNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.attrib_create", "Attribute Create", Stage::Geometry)
            .input(geo_in())
            .output(geo_out())
            .param(ParamSpec::string("name", "Name", "attr1").help("Attribute name: letters, digits and underscores."))
            .param(
                ParamSpec::choice("class", "Class", "point", &["point", "prim", "detail"])
                    .help("Store one value per point, per face, or once for the whole geometry."),
            )
            .param(
                ParamSpec::choice("type", "Type", "float", &["float", "int", "vector", "string"])
                    .help("Value type of the attribute."),
            )
            .param(
                ParamSpec::vec3("value", "Value", DVec3::ZERO)
                    .help("Vector value; float and int types use the first component."),
            )
            .param(ParamSpec::string("string_value", "String value", "").help("Value for the string type."))
            .param(
                ParamSpec::string("group", "Group", "")
                    .help("Only set elements in this point or face group; empty sets all."),
            )
            .help("Adds or overwrites a named attribute with a constant value, optionally only inside a group.")
            .emulates("User-defined channel / attribute creation as in 3ds Max Channel Info and the Houdini Attribute Create SOP")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let name = params.str("name")?.trim().to_string();
        if !valid_attrib_name(&name) {
            return Err(ctx.fail(format!("invalid attribute name '{name}'")));
        }
        let v = params.vec3("value")?;
        let value = match params.str("type")? {
            "int" => AttribArray::Int(vec![v.x.round() as i64]),
            "vector" => AttribArray::Vec3(vec![v]),
            "string" => AttribArray::Str(vec![params.str("string_value")?.to_string()]),
            _ => AttribArray::Float(vec![v.x]),
        };
        let group = params.str("group")?;
        let mut out = (*g).clone();
        match params.str("class")? {
            "detail" => {
                set_attrib(&mut out.detail_attribs, 1, &name, &value, None);
            }
            "prim" => {
                let mask = if group.trim().is_empty() {
                    None
                } else {
                    Some(group_mask(&g, Element::Prims, group).map_err(|e| ctx.fail(e))?)
                };
                let n = out.prims.len();
                set_attrib(&mut out.prim_attribs, n, &name, &value, mask.as_deref());
            }
            _ => {
                let mask = if group.trim().is_empty() {
                    None
                } else {
                    Some(group_mask(&g, Element::Points, group).map_err(|e| ctx.fail(e))?)
                };
                let n = out.points.len();
                set_attrib(&mut out.point_attribs, n, &name, &value, mask.as_deref());
            }
        }
        Ok(vec![Data::geometry(out)])
    }
}
