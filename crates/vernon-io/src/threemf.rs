//! 3MF: a ZIP package holding one or more XML model parts.
//!
//! Reader: the root model part is found through `_rels/.rels` (falling back to
//! `3D/3dmodel.model`), `unit` is honoured, build items and components are
//! flattened with their row-vector transforms, objects that live in other
//! model parts (`path` on components and items, as slicer projects write them)
//! are followed, and mirrored items keep an outward winding. Every triangle is
//! one closed polygon and carries the Int prim attribute `part`, the index of
//! the build item it came from (items of type `support`/`other` are skipped).
//! Unknown parts and attributes are ignored.
//!
//! Writer: one `object` and one build item per distinct `part` value (a single
//! part when the attribute is missing), vertices welded at 1e-6 mm, polygons
//! triangulated, closed shells made outward-facing, `unit` set from the write
//! options, numbers written with full round-trip precision.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use glam::{DMat3, DMat4, DVec4};
use vernon_core::{AttribArray, DVec3, Geometry, Prim};

use crate::xml::{self, escape_attr, Event, Tag};
use crate::zip::{normalize, Archive, Limits, ZipWriter};
use crate::{parse_err, tri, FileUnit, IoError, IoResult, WriteOptions};

const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const MODEL_REL: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const MODEL_CT: &str = "application/vnd.ms-package.3dmanufacturing-3dmodel+xml";
const RELS_CT: &str = "application/vnd.openxmlformats-package.relationships+xml";
const ROOT_PART: &str = "3D/3dmodel.model";
/// Triangles allowed in the flattened result (stops instancing bombs).
const MAX_TRIANGLES: usize = 100_000_000;
/// Component nesting limit (also catches reference cycles).
const MAX_DEPTH: usize = 32;
/// External model parts followed.
const MAX_PARTS: usize = 4_096;
/// Vertex weld tolerance on write, mm.
const WELD_MM: f64 = 1e-6;

// ---------------------------------------------------------------- reading

#[derive(Default)]
struct Mesh {
    verts: Vec<DVec3>,
    tris: Vec<[u32; 3]>,
}

struct Component {
    object: u32,
    path: Option<String>,
    transform: DMat4,
}

enum Content {
    Mesh(Mesh),
    Components(Vec<Component>),
}

struct Object {
    /// Model and solid-support objects can be printed; `support` and `other`
    /// cannot be build items.
    buildable: bool,
    content: Content,
}

struct Item {
    object: u32,
    path: Option<String>,
    transform: DMat4,
}

struct Model {
    unit_mm: f64,
    objects: HashMap<u32, Object>,
    items: Vec<Item>,
}

fn unit_mm(name: &str) -> Option<f64> {
    Some(match name {
        "micron" => 0.001,
        "millimeter" => 1.0,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => return None,
    })
}

fn unit_name(u: FileUnit) -> &'static str {
    match u {
        FileUnit::Millimetre => "millimeter",
        FileUnit::Centimetre => "centimeter",
        FileUnit::Metre => "meter",
        FileUnit::Inch => "inch",
    }
}

fn need<'t>(t: &'t Tag, name: &str) -> Result<&'t str, String> {
    t.attr(name)
        .ok_or_else(|| format!("<{}> lacks the '{name}' attribute", t.name))
}

fn number(t: &Tag, name: &str) -> Result<f64, String> {
    let s = need(t, name)?;
    match s.trim().parse::<f64>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => Err(format!("<{}> has an invalid {name}='{s}'", t.name)),
    }
}

fn index(t: &Tag, name: &str) -> Result<u32, String> {
    let s = need(t, name)?;
    s.trim()
        .parse::<u32>()
        .map_err(|_| format!("<{}> has an invalid {name}='{s}'", t.name))
}

/// 12 numbers `m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32` for `p' = p * M`,
/// as a column-vector matrix.
fn parse_transform(s: &str) -> Result<DMat4, String> {
    let v: Vec<f64> = s
        .split_ascii_whitespace()
        .map(|t| t.parse::<f64>().ok().filter(|x| x.is_finite()))
        .collect::<Option<_>>()
        .ok_or_else(|| format!("invalid transform '{s}'"))?;
    if v.len() != 12 {
        return Err(format!("transform has {} numbers, expected 12", v.len()));
    }
    Ok(DMat4::from_cols(
        DVec4::new(v[0], v[1], v[2], 0.0),
        DVec4::new(v[3], v[4], v[5], 0.0),
        DVec4::new(v[6], v[7], v[8], 0.0),
        DVec4::new(v[9], v[10], v[11], 1.0),
    ))
}

fn transform_attr(t: &Tag) -> Result<DMat4, String> {
    match t.attr("transform") {
        Some(s) => parse_transform(s),
        None => Ok(DMat4::IDENTITY),
    }
}

fn parse_model(text: &str, file: &str, part: &str) -> IoResult<Model> {
    let err = |m: String| parse_err(file, 0, format!("3MF part '{part}': {m}"));
    let mut rd = xml::Reader::new(text);
    let mut model = Model {
        unit_mm: 1.0,
        objects: HashMap::new(),
        items: Vec::new(),
    };
    struct Cur {
        id: u32,
        buildable: bool,
        mesh: Option<Mesh>,
        comps: Vec<Component>,
    }
    let mut cur: Option<Cur> = None;
    let mut saw_model = false;
    while let Some(ev) = rd.next_event().map_err(err)? {
        match ev {
            Event::Start(t) => match t.name {
                "model" => {
                    saw_model = true;
                    if let Some(u) = t.attr("unit") {
                        model.unit_mm =
                            unit_mm(u).ok_or_else(|| err(format!("unknown unit '{u}'")))?;
                    }
                }
                "object" => {
                    let id = index(&t, "id").map_err(err)?;
                    let buildable = !matches!(t.attr("type"), Some("support" | "other"));
                    cur = Some(Cur {
                        id,
                        buildable,
                        mesh: None,
                        comps: Vec::new(),
                    });
                }
                "mesh" => {
                    if let Some(c) = cur.as_mut() {
                        c.mesh = Some(Mesh::default());
                    }
                }
                "vertex" => {
                    if let Some(m) = cur.as_mut().and_then(|c| c.mesh.as_mut()) {
                        let v = DVec3::new(
                            number(&t, "x").map_err(err)?,
                            number(&t, "y").map_err(err)?,
                            number(&t, "z").map_err(err)?,
                        );
                        m.verts.push(v);
                    }
                }
                "triangle" => {
                    if let Some(m) = cur.as_mut().and_then(|c| c.mesh.as_mut()) {
                        let tri = [
                            index(&t, "v1").map_err(err)?,
                            index(&t, "v2").map_err(err)?,
                            index(&t, "v3").map_err(err)?,
                        ];
                        m.tris.push(tri);
                    }
                }
                "component" => {
                    if let Some(c) = cur.as_mut() {
                        c.comps.push(Component {
                            object: index(&t, "objectid").map_err(err)?,
                            path: t.attr_local("path").map(str::to_string),
                            transform: transform_attr(&t).map_err(err)?,
                        });
                    }
                }
                "item" => model.items.push(Item {
                    object: index(&t, "objectid").map_err(err)?,
                    path: t.attr_local("path").map(str::to_string),
                    transform: transform_attr(&t).map_err(err)?,
                }),
                _ => {}
            },
            Event::End("object") => {
                if let Some(c) = cur.take() {
                    let content = match c.mesh {
                        Some(m) => {
                            if let Some(bad) = m
                                .tris
                                .iter()
                                .flatten()
                                .find(|&&i| i as usize >= m.verts.len())
                            {
                                return Err(err(format!(
                                    "object {} has a triangle with vertex {bad}, but only {} vertices",
                                    c.id,
                                    m.verts.len()
                                )));
                            }
                            Content::Mesh(m)
                        }
                        None => Content::Components(c.comps),
                    };
                    model.objects.insert(
                        c.id,
                        Object {
                            buildable: c.buildable,
                            content,
                        },
                    );
                }
            }
            Event::End(_) => {}
        }
    }
    if !saw_model {
        return Err(err("no <model> element".into()));
    }
    Ok(model)
}

/// Name of the root model part inside the package.
fn start_part(zip: &Archive, file: &str) -> IoResult<String> {
    if let Some(rels) = zip.read("_rels/.rels")? {
        let text = String::from_utf8_lossy(&rels);
        let mut rd = xml::Reader::new(&text);
        while let Some(ev) = rd
            .next_event()
            .map_err(|m| parse_err(file, 0, format!("3MF _rels/.rels: {m}")))?
        {
            if let Event::Start(t) = ev {
                if t.name == "Relationship" {
                    let ty = t.attr("Type").unwrap_or("");
                    if ty.eq_ignore_ascii_case(MODEL_REL)
                        || ty.to_ascii_lowercase().ends_with("/3dmodel")
                    {
                        if let Some(target) = t.attr("Target") {
                            let name = normalize(target);
                            if zip.contains(&name) {
                                return Ok(name);
                            }
                        }
                    }
                }
            }
        }
    }
    if zip.contains(ROOT_PART) {
        return Ok(ROOT_PART.to_string());
    }
    zip.names()
        .find(|n| n.to_ascii_lowercase().ends_with(".model"))
        .map(str::to_string)
        .ok_or_else(|| parse_err(file, 0, "3MF package has no model part"))
}

fn key(name: &str) -> String {
    normalize(name).to_ascii_lowercase()
}

/// Paths of other model parts a model refers to.
fn external_paths(m: &Model) -> Vec<String> {
    let mut v: Vec<String> = m.items.iter().filter_map(|i| i.path.clone()).collect();
    for o in m.objects.values() {
        if let Content::Components(cs) = &o.content {
            v.extend(cs.iter().filter_map(|c| c.path.clone()));
        }
    }
    v
}

struct Out {
    unit_mm: f64,
    points: Vec<DVec3>,
    prims: Vec<Prim>,
    part: Vec<i64>,
}

#[allow(clippy::too_many_arguments)]
fn emit(
    parts: &HashMap<String, Model>,
    out: &mut Out,
    file: &str,
    path: &str,
    id: u32,
    acc: DMat4,
    depth: usize,
    part: i64,
) -> IoResult<()> {
    let err = |m: String| parse_err(file, 0, format!("3MF: {m}"));
    if depth > MAX_DEPTH {
        return Err(err(format!(
            "components nested deeper than {MAX_DEPTH} (or a reference cycle)"
        )));
    }
    let model = parts
        .get(path)
        .ok_or_else(|| err(format!("model part '{path}' is not loaded")))?;
    let obj = model
        .objects
        .get(&id)
        .ok_or_else(|| err(format!("object {id} is not defined in '{path}'")))?;
    match &obj.content {
        Content::Mesh(mesh) => {
            let m3 = DMat3::from_cols(
                acc.x_axis.truncate(),
                acc.y_axis.truncate(),
                acc.z_axis.truncate(),
            );
            let det = m3.determinant();
            let size = m3.x_axis.length() * m3.y_axis.length() * m3.z_axis.length();
            let regular = det.abs() > 1e-9 * size; // false for NaN too
            if !regular {
                return Err(err(format!(
                    "object {id} is placed with a singular transform"
                )));
            }
            if out.prims.len() + mesh.tris.len() > MAX_TRIANGLES {
                return Err(err(format!(
                    "more than {MAX_TRIANGLES} triangles after flattening"
                )));
            }
            let base = out.points.len() as u32;
            out.points.extend(
                mesh.verts
                    .iter()
                    .map(|&v| acc.transform_point3(v) * out.unit_mm),
            );
            let flip = det < 0.0;
            for &[a, b, c] in &mesh.tris {
                if a == b || b == c || a == c {
                    continue;
                }
                let tri = if flip { [a, c, b] } else { [a, b, c] };
                out.prims
                    .push(Prim::polygon(tri.iter().map(|&i| base + i).collect()));
                out.part.push(part);
            }
        }
        Content::Components(comps) => {
            for c in comps {
                let p = c.path.as_deref().map(key);
                let p = p.as_deref().unwrap_or(path);
                emit(
                    parts,
                    out,
                    file,
                    p,
                    c.object,
                    acc * c.transform,
                    depth + 1,
                    part,
                )?;
            }
        }
    }
    Ok(())
}

pub(crate) fn read(bytes: &[u8], file: &str) -> IoResult<Geometry> {
    let zip = Archive::open(bytes, file, Limits::default())?;
    let root_name = start_part(&zip, file)?;
    let load = |name: &str| -> IoResult<Model> {
        let data = zip
            .read(name)?
            .ok_or_else(|| parse_err(file, 0, format!("3MF part '{name}' is missing")))?;
        let text = std::str::from_utf8(&data)
            .map_err(|_| parse_err(file, 0, format!("3MF part '{name}' is not valid UTF-8")))?;
        parse_model(text, file, name)
    };
    let root_key = key(&root_name);
    let root = load(&root_name)?;
    let mut pending = external_paths(&root);
    let mut parts: HashMap<String, Model> = HashMap::new();
    parts.insert(root_key.clone(), root);
    while let Some(p) = pending.pop() {
        let k = key(&p);
        if parts.contains_key(&k) {
            continue;
        }
        if parts.len() >= MAX_PARTS {
            return Err(parse_err(
                file,
                0,
                format!("3MF refers to more than {MAX_PARTS} model parts"),
            ));
        }
        let m = load(&p)?;
        pending.extend(external_paths(&m));
        parts.insert(k, m);
    }

    let root = &parts[&root_key];
    let mut out = Out {
        unit_mm: root.unit_mm,
        points: Vec::new(),
        prims: Vec::new(),
        part: Vec::new(),
    };
    let mut part = 0i64;
    for item in &root.items {
        let path = item
            .path
            .as_deref()
            .map(key)
            .unwrap_or_else(|| root_key.clone());
        let model = parts
            .get(&path)
            .ok_or_else(|| parse_err(file, 0, format!("3MF model part '{path}' is not loaded")))?;
        let obj = model.objects.get(&item.object).ok_or_else(|| {
            parse_err(
                file,
                0,
                format!("3MF build item refers to undefined object {}", item.object),
            )
        })?;
        if !obj.buildable {
            continue;
        }
        emit(
            &parts,
            &mut out,
            file,
            &path,
            item.object,
            item.transform,
            0,
            part,
        )?;
        part += 1;
    }
    if out.prims.is_empty() {
        return Err(parse_err(file, 0, "3MF contains no printable triangles"));
    }
    let mut geo = Geometry {
        points: out.points,
        prims: out.prims,
        ..Default::default()
    };
    geo.prim_attribs.set("part", AttribArray::Int(out.part));
    Ok(geo)
}

// ---------------------------------------------------------------- writing

fn num(s: &mut String, v: f64) {
    if v == 0.0 {
        s.push('0');
    } else {
        let _ = write!(s, "{v}");
    }
}

/// UTC time as an ISO 8601 string: `fixed` seconds when given, else
/// `SOURCE_DATE_EPOCH`, else the current time. A fixed date makes the
/// written package reproducible.
fn timestamp(fixed: Option<i64>) -> String {
    let secs = fixed.unwrap_or_else(|| {
        std::env::var("SOURCE_DATE_EPOCH")
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64)
            })
    });
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (proleptic Gregorian).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// One welded, triangulated, outward-facing shell ready to write.
struct Shell {
    points: Vec<DVec3>,
    tris: Vec<[u32; 3]>,
}

/// Every undirected edge is used by exactly one triangle in each direction.
fn is_closed_oriented(tris: &[[u32; 3]]) -> bool {
    let mut edges: HashMap<(u32, u32), i32> = HashMap::with_capacity(tris.len() * 3);
    for &[a, b, c] in tris {
        for (u, v) in [(a, b), (b, c), (c, a)] {
            *edges.entry((u, v)).or_insert(0) += 1;
        }
    }
    edges
        .iter()
        .all(|(&(u, v), &n)| n == 1 && edges.get(&(v, u)) == Some(&1))
}

fn signed_volume(points: &[DVec3], tris: &[[u32; 3]]) -> f64 {
    let o = points[tris[0][0] as usize];
    tris.iter()
        .map(|&[a, b, c]| {
            let (a, b, c) = (
                points[a as usize] - o,
                points[b as usize] - o,
                points[c as usize] - o,
            );
            a.dot(b.cross(c))
        })
        .sum::<f64>()
        / 6.0
}

fn build_shell(geo: &Geometry, prim_ids: &[usize]) -> Option<Shell> {
    // Sub-geometry of just this part's polygons.
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut sub = Geometry::new();
    for &pi in prim_ids {
        let verts: Vec<u32> = geo.prims[pi]
            .vertices
            .iter()
            .map(|&v| {
                *remap.entry(v).or_insert_with(|| {
                    sub.points.push(geo.points[v as usize]);
                    (sub.points.len() - 1) as u32
                })
            })
            .collect();
        sub.prims.push(Prim::polygon(verts));
    }
    crate::weld_points(&mut sub, WELD_MM);
    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(sub.prims.len());
    for p in &sub.prims {
        tri::triangulate_ring(&sub.points, &p.vertices, &mut tris);
    }
    if tris.is_empty() {
        return None;
    }
    // Keep only referenced points.
    let mut new_index = vec![u32::MAX; sub.points.len()];
    let mut points = Vec::new();
    for t in &mut tris {
        for i in t.iter_mut() {
            if new_index[*i as usize] == u32::MAX {
                new_index[*i as usize] = points.len() as u32;
                points.push(sub.points[*i as usize]);
            }
            *i = new_index[*i as usize];
        }
    }
    if is_closed_oriented(&tris) && signed_volume(&points, &tris) < 0.0 {
        for t in &mut tris {
            t.swap(1, 2);
        }
    }
    Some(Shell { points, tris })
}

pub(crate) fn write(geo: &Geometry, opts: &WriteOptions) -> IoResult<Vec<u8>> {
    let scale = 1.0 / opts.unit.to_mm();
    let part_attr = geo.prim_attribs.ints("part").ok();
    let mut groups: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (pi, prim) in geo.prims.iter().enumerate() {
        if prim.closed && prim.vertices.len() >= 3 {
            groups
                .entry(part_attr.map_or(0, |a| a[pi]))
                .or_default()
                .push(pi);
        }
    }
    let shells: Vec<(i64, Shell)> = groups
        .iter()
        .filter_map(|(&part, ids)| build_shell(geo, ids).map(|s| (part, s)))
        .collect();
    if shells.is_empty() {
        return Err(IoError::Other(
            "3MF needs closed polygons; the geometry has none".into(),
        ));
    }

    let mut xml = String::new();
    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        xml,
        "<model unit=\"{}\" xml:lang=\"en-US\" xmlns=\"{CORE_NS}\">",
        unit_name(opts.unit)
    );
    let _ = writeln!(xml, " <metadata name=\"Title\">Vernon export</metadata>");
    let _ = writeln!(xml, " <metadata name=\"Application\">Vernon</metadata>");
    let _ = writeln!(
        xml,
        " <metadata name=\"CreationDate\">{}</metadata>",
        timestamp(opts.creation_date)
    );
    xml.push_str(" <resources>\n");
    for (n, (part, shell)) in shells.iter().enumerate() {
        let name = escape_attr(&format!("part_{part}")).into_owned();
        let _ = writeln!(
            xml,
            "  <object id=\"{}\" type=\"model\" name=\"{name}\">",
            n + 1
        );
        xml.push_str("   <mesh>\n    <vertices>\n");
        for p in &shell.points {
            let q = *p * scale;
            xml.push_str("     <vertex x=\"");
            num(&mut xml, q.x);
            xml.push_str("\" y=\"");
            num(&mut xml, q.y);
            xml.push_str("\" z=\"");
            num(&mut xml, q.z);
            xml.push_str("\"/>\n");
        }
        xml.push_str("    </vertices>\n    <triangles>\n");
        for t in &shell.tris {
            let _ = writeln!(
                xml,
                "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>",
                t[0], t[1], t[2]
            );
        }
        xml.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }
    xml.push_str(" </resources>\n <build>\n");
    for n in 0..shells.len() {
        let _ = writeln!(xml, "  <item objectid=\"{}\"/>", n + 1);
    }
    xml.push_str(" </build>\n</model>\n");

    let types = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"{TYPES_NS}\">\n \
         <Default Extension=\"rels\" ContentType=\"{RELS_CT}\"/>\n \
         <Default Extension=\"model\" ContentType=\"{MODEL_CT}\"/>\n</Types>\n"
    );
    let rels = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"{REL_NS}\">\n \
         <Relationship Target=\"/{ROOT_PART}\" Id=\"rel0\" Type=\"{MODEL_REL}\"/>\n</Relationships>\n"
    );
    let mut zip = ZipWriter::new();
    zip.add("[Content_Types].xml", types.as_bytes())?;
    zip.add("_rels/.rels", rels.as_bytes())?;
    zip.add(ROOT_PART, xml.as_bytes())?;
    zip.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{read_bytes, write_bytes, Format, ReadOptions};

    fn package(model: &str) -> Vec<u8> {
        let mut z = ZipWriter::new();
        z.add("[Content_Types].xml", b"<Types/>").unwrap();
        z.add(
            "_rels/.rels",
            format!("<Relationships><Relationship Target=\"/3D/m.model\" Id=\"r\" Type=\"{MODEL_REL}\"/></Relationships>")
                .as_bytes(),
        )
        .unwrap();
        z.add("3D/m.model", model.as_bytes()).unwrap();
        z.finish().unwrap()
    }

    fn cube_xml(id: u32, size: f64) -> String {
        let s = size;
        let v = [
            (0., 0., 0.),
            (s, 0., 0.),
            (s, s, 0.),
            (0., s, 0.),
            (0., 0., s),
            (s, 0., s),
            (s, s, s),
            (0., s, s),
        ];
        let t = [
            (0, 2, 1),
            (0, 3, 2),
            (4, 5, 6),
            (4, 6, 7),
            (0, 1, 5),
            (0, 5, 4),
            (1, 2, 6),
            (1, 6, 5),
            (2, 3, 7),
            (2, 7, 6),
            (3, 0, 4),
            (3, 4, 7),
        ];
        let mut x = format!("<object id=\"{id}\" type=\"model\"><mesh><vertices>");
        for (a, b, c) in v {
            x += &format!("<vertex x=\"{a}\" y=\"{b}\" z=\"{c}\"/>");
        }
        x += "</vertices><triangles>";
        for (a, b, c) in t {
            x += &format!("<triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>");
        }
        x + "</triangles></mesh></object>"
    }

    fn model(unit: &str, resources: &str, build: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?><model unit=\"{unit}\" xmlns=\"{CORE_NS}\"><resources>{resources}</resources><build>{build}</build></model>"
        )
    }

    fn volume(g: &Geometry) -> f64 {
        let tris = g.triangles();
        let idx: Vec<[u32; 3]> = tris.iter().map(|t| t.1).collect();
        signed_volume(&g.points, &idx)
    }

    fn rd(bytes: &[u8]) -> IoResult<Geometry> {
        read_bytes(Format::ThreeMf, bytes, &ReadOptions::default())
    }

    #[test]
    fn written_cube_is_a_valid_package() {
        let g = Geometry::make_box(DVec3::splat(10.0));
        let bytes = write_bytes(Format::ThreeMf, &g, &WriteOptions::default()).unwrap();
        assert_eq!(&bytes[..4], &[0x50, 0x4B, 0x03, 0x04]);
        let zip = Archive::open(&bytes, "t", Limits::default()).unwrap();
        let names: Vec<&str> = zip.names().collect();
        assert_eq!(
            names,
            ["[Content_Types].xml", "_rels/.rels", "3D/3dmodel.model"]
        );
        let xml = String::from_utf8(zip.read("3D/3dmodel.model").unwrap().unwrap()).unwrap();
        assert!(xml.contains("unit=\"millimeter\""));
        assert!(xml.contains("name=\"Application\""));
        assert!(xml.contains("name=\"CreationDate\""));
        assert_eq!(xml.matches("<vertex ").count(), 8);
        assert_eq!(xml.matches("<triangle ").count(), 12);
        assert_eq!(xml.matches("<object ").count(), 1);
        let rels = String::from_utf8(zip.read("_rels/.rels").unwrap().unwrap()).unwrap();
        assert!(rels.contains(MODEL_REL) && rels.contains("/3D/3dmodel.model"));
        let ct = String::from_utf8(zip.read("[Content_Types].xml").unwrap().unwrap()).unwrap();
        assert!(ct.contains(MODEL_CT) && ct.contains(RELS_CT));
    }

    #[test]
    fn cube_round_trip_has_8_points_12_triangles_and_volume_1000() {
        let g = Geometry::make_box(DVec3::splat(10.0));
        let bytes = write_bytes(Format::ThreeMf, &g, &WriteOptions::default()).unwrap();
        let b = rd(&bytes).unwrap();
        assert_eq!(b.point_count(), 8);
        assert_eq!(b.prim_count(), 12);
        assert!((volume(&b) - 1000.0).abs() < 1e-9, "{}", volume(&b));
        assert_eq!(b.prim_attribs.ints("part").unwrap(), &[0; 12]);
        assert!((b.surface_area() - 600.0).abs() < 1e-9);
    }

    #[test]
    fn inch_file_reads_as_25_4_mm() {
        let g = Geometry::make_box(DVec3::splat(25.4));
        let opts = WriteOptions {
            unit: FileUnit::Inch,
            ..Default::default()
        };
        let bytes = write_bytes(Format::ThreeMf, &g, &opts).unwrap();
        let zip = Archive::open(&bytes, "t", Limits::default()).unwrap();
        let xml = String::from_utf8(zip.read(ROOT_PART).unwrap().unwrap()).unwrap();
        assert!(xml.contains("unit=\"inch\""));
        assert!(
            xml.contains("x=\"0.5\"") || xml.contains("x=\"0.49999"),
            "{}",
            &xml[..600]
        );
        let b = rd(&bytes).unwrap();
        assert!((b.bounds().size().x - 25.4).abs() < 1e-9);
        // Hand-written 1-inch cube: 25.4 mm edge, 16387.064 mm3.
        let src = model("inch", &cube_xml(1, 1.0), "<item objectid=\"1\"/>");
        let b = rd(&package(&src)).unwrap();
        assert!((b.bounds().size().x - 25.4).abs() < 1e-9);
        assert!((volume(&b) - 16_387.064).abs() < 1e-6, "{}", volume(&b));
    }

    #[test]
    fn units_scale_to_millimetres() {
        for (unit, k) in [
            ("micron", 0.001),
            ("millimeter", 1.0),
            ("centimeter", 10.0),
            ("meter", 1000.0),
            ("foot", 304.8),
        ] {
            let src = model(unit, &cube_xml(1, 2.0), "<item objectid=\"1\"/>");
            let b = rd(&package(&src)).unwrap();
            assert!((b.bounds().size().z - 2.0 * k).abs() < 1e-9, "{unit}");
        }
        let src = model("furlong", &cube_xml(1, 2.0), "<item objectid=\"1\"/>");
        assert!(rd(&package(&src)).is_err());
        // Missing unit defaults to millimetres.
        let src = format!(
            "<model xmlns=\"{CORE_NS}\"><resources>{}</resources><build><item objectid=\"1\"/></build></model>",
            cube_xml(1, 2.0)
        );
        assert!((rd(&package(&src)).unwrap().bounds().size().x - 2.0).abs() < 1e-12);
    }

    #[test]
    fn item_transform_is_row_vector_and_units_apply_to_translation() {
        // Translation of +50 along X (last three numbers), plus a 90 degree turn
        // about Z: p' = p * M maps +X to +Y with this row layout.
        let src = model(
            "millimeter",
            &cube_xml(1, 10.0),
            "<item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 50 0 0\"/>",
        );
        let b = rd(&package(&src)).unwrap();
        assert!((b.bounds().min.x - 50.0).abs() < 1e-12 && (b.bounds().max.x - 60.0).abs() < 1e-12);
        let src = model(
            "millimeter",
            &cube_xml(1, 10.0),
            "<item objectid=\"1\" transform=\"0 1 0 -1 0 0 0 0 1 0 0 0\"/>",
        );
        let b = rd(&package(&src)).unwrap();
        // Rotating the cube [0,10]^3 by +90 deg about Z gives x in [-10, 0], y in [0, 10].
        assert!((b.bounds().min.x + 10.0).abs() < 1e-12 && b.bounds().max.x.abs() < 1e-12);
        assert!(b.bounds().min.y.abs() < 1e-12 && (b.bounds().max.y - 10.0).abs() < 1e-12);
        assert!((volume(&b) - 1000.0).abs() < 1e-9);
    }

    #[test]
    fn mirrored_items_keep_positive_volume() {
        let src = model(
            "millimeter",
            &cube_xml(1, 10.0),
            "<item objectid=\"1\" transform=\"-1 0 0 0 1 0 0 0 1 0 0 0\"/>",
        );
        let b = rd(&package(&src)).unwrap();
        assert!((volume(&b) - 1000.0).abs() < 1e-9, "{}", volume(&b));
    }

    #[test]
    fn singular_and_malformed_transforms_are_errors() {
        for t in [
            "1 0 0 0 1 0 0 0 0 0 0 0",
            "0 0 0 0 0 0 0 0 0 0 0 0",
            "1 0 0 0 1 0 0 0 1 0 0",
            "1 0 0 0 1 0 0 0 1 0 0 x",
            "1 0 0 0 1 0 0 0 1 0 0 nan",
        ] {
            let src = model(
                "millimeter",
                &cube_xml(1, 1.0),
                &format!("<item objectid=\"1\" transform=\"{t}\"/>"),
            );
            assert!(rd(&package(&src)).is_err(), "{t}");
        }
    }

    #[test]
    fn components_are_flattened_with_nested_transforms_and_parts_are_numbered() {
        let res = format!(
            "{}<object id=\"2\" type=\"model\"><components>\
             <component objectid=\"1\"/>\
             <component objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 20 0 0\"/>\
             </components></object>\
             <object id=\"3\" type=\"support\"><components><component objectid=\"1\"/></components></object>",
            cube_xml(1, 10.0)
        );
        let src = model(
            "millimeter",
            &res,
            "<item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 0 100 0\"/>\
             <item objectid=\"3\"/><item objectid=\"1\"/>",
        );
        let b = rd(&package(&src)).unwrap();
        assert_eq!(b.prim_count(), 36); // 2 cubes + 1 cube, support item skipped
        assert_eq!(b.point_count(), 24);
        let part = b.prim_attribs.ints("part").unwrap();
        assert_eq!(part.iter().filter(|&&p| p == 0).count(), 24);
        assert_eq!(part.iter().filter(|&&p| p == 1).count(), 12);
        assert!(part.iter().all(|&p| p == 0 || p == 1));
        let bb = b.bounds();
        assert!((bb.max.x - 30.0).abs() < 1e-12 && (bb.max.y - 110.0).abs() < 1e-12);
    }

    #[test]
    fn component_cycles_and_dangling_references_are_errors() {
        let cyc = "<object id=\"1\"><components><component objectid=\"2\"/></components></object>\
                   <object id=\"2\"><components><component objectid=\"1\"/></components></object>";
        let src = model("millimeter", cyc, "<item objectid=\"1\"/>");
        assert!(rd(&package(&src)).is_err());
        let src = model("millimeter", &cube_xml(1, 1.0), "<item objectid=\"9\"/>");
        assert!(rd(&package(&src)).is_err());
        let src = model(
            "millimeter",
            "<object id=\"1\"><components><component objectid=\"7\"/></components></object>",
            "<item objectid=\"1\"/>",
        );
        assert!(rd(&package(&src)).is_err());
    }

    #[test]
    fn bad_triangle_indices_and_empty_models_are_errors() {
        let bad = "<object id=\"1\"><mesh><vertices><vertex x=\"0\" y=\"0\" z=\"0\"/></vertices>\
                   <triangles><triangle v1=\"0\" v2=\"1\" v3=\"2\"/></triangles></mesh></object>";
        assert!(rd(&package(&model(
            "millimeter",
            bad,
            "<item objectid=\"1\"/>"
        )))
        .is_err());
        assert!(rd(&package(&model("millimeter", &cube_xml(1, 1.0), ""))).is_err());
        let nan = cube_xml(1, 1.0).replacen("x=\"1\"", "x=\"NaN\"", 1);
        assert!(rd(&package(&model(
            "millimeter",
            &nan,
            "<item objectid=\"1\"/>"
        )))
        .is_err());
    }

    #[test]
    fn external_model_parts_are_followed() {
        let root = format!(
            "<model unit=\"millimeter\" xmlns=\"{CORE_NS}\" xmlns:p=\"urn:p\"><resources>\
             <object id=\"2\"><components><component p:path=\"/3D/Objects/o.model\" objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 5 0 0\"/></components></object>\
             </resources><build><item objectid=\"2\"/></build></model>"
        );
        let other = model("millimeter", &cube_xml(1, 10.0), "");
        let mut z = ZipWriter::new();
        z.add("[Content_Types].xml", b"<Types/>").unwrap();
        z.add("3D/3dmodel.model", root.as_bytes()).unwrap(); // no _rels: fallback name
        z.add("3D/Objects/o.model", other.as_bytes()).unwrap();
        let b = rd(&z.finish().unwrap()).unwrap();
        assert_eq!(b.prim_count(), 12);
        assert!((b.bounds().min.x - 5.0).abs() < 1e-12);
    }

    #[test]
    fn multi_part_geometry_writes_one_object_per_part() {
        let mut g = Geometry::make_box(DVec3::splat(10.0));
        let mut h = Geometry::make_box(DVec3::splat(4.0));
        h.transform(glam::DMat4::from_translation(DVec3::new(30.0, 0.0, 0.0)));
        let n = g.prim_count();
        g.prim_attribs.set("part", AttribArray::Int(vec![7; n]));
        h.prim_attribs
            .set("part", AttribArray::Int(vec![3; h.prim_count()]));
        g.merge(&h);
        let bytes = write_bytes(Format::ThreeMf, &g, &WriteOptions::default()).unwrap();
        let zip = Archive::open(&bytes, "t", Limits::default()).unwrap();
        let xml = String::from_utf8(zip.read(ROOT_PART).unwrap().unwrap()).unwrap();
        assert_eq!(xml.matches("<object ").count(), 2);
        assert_eq!(xml.matches("<item ").count(), 2);
        assert!(xml.find("part_3").unwrap() < xml.find("part_7").unwrap());
        let b = rd(&bytes).unwrap();
        assert_eq!(b.prim_count(), 24);
        let part = b.prim_attribs.ints("part").unwrap();
        // Part values are renumbered in build order: old 3 -> 0, old 7 -> 1.
        let small: f64 = (0..24)
            .filter(|&i| part[i] == 0)
            .map(|i| b.prim_area(i))
            .sum();
        assert!((small - 96.0).abs() < 1e-9, "{small}");
        let big: f64 = (0..24)
            .filter(|&i| part[i] == 1)
            .map(|i| b.prim_area(i))
            .sum();
        assert!((big - 600.0).abs() < 1e-9);
    }

    #[test]
    fn writer_welds_orients_and_triangulates() {
        // Triangle soup of an inside-out cube with duplicated corners and quads.
        let mut g = Geometry::make_box(DVec3::splat(10.0));
        for p in &mut g.prims {
            p.vertices.reverse();
        }
        // Un-weld: give every quad its own four points.
        let mut soup = Geometry::new();
        for p in &g.prims {
            let vs: Vec<u32> = p
                .vertices
                .iter()
                .map(|&v| soup.add_point(g.points[v as usize]))
                .collect();
            soup.add_prim(Prim::polygon(vs));
        }
        assert_eq!(soup.point_count(), 24);
        let bytes = write_bytes(Format::ThreeMf, &soup, &WriteOptions::default()).unwrap();
        let b = rd(&bytes).unwrap();
        assert_eq!(b.point_count(), 8);
        assert_eq!(b.prim_count(), 12);
        assert!(
            (volume(&b) - 1000.0).abs() < 1e-9,
            "inside-out input must come out outward"
        );
    }

    #[test]
    fn point_clouds_and_open_polylines_cannot_be_written() {
        let g = Geometry::from_points(vec![DVec3::ZERO, DVec3::X]);
        assert!(write_bytes(Format::ThreeMf, &g, &WriteOptions::default()).is_err());
        let mut l = g.clone();
        l.add_prim(Prim::polyline(vec![0, 1]));
        assert!(write_bytes(Format::ThreeMf, &l, &WriteOptions::default()).is_err());
    }

    #[test]
    fn foreign_package_layouts_are_accepted() {
        // Upper-case part names, backslashes and a relationship target without a slash.
        let mut z = ZipWriter::new();
        z.add(
            "_rels/.rels",
            format!("<Relationships><Relationship Target=\"3D\\Model.MODEL\" Id=\"r\" Type=\"{MODEL_REL}\"/></Relationships>").as_bytes(),
        )
        .unwrap();
        z.add(
            "3D/MODEL.model",
            model("millimeter", &cube_xml(1, 3.0), "<item objectid=\"1\"/>").as_bytes(),
        )
        .unwrap();
        z.add("Metadata/thumbnail.png", &[0x89, 0x50, 0x4E, 0x47])
            .unwrap();
        assert_eq!(rd(&z.finish().unwrap()).unwrap().prim_count(), 12);
    }

    #[test]
    fn timestamps_follow_source_date_epoch_format() {
        let t = timestamp(None);
        assert_eq!(t.len(), 20);
        assert!(t.ends_with('Z') && t.as_bytes()[10] == b'T');
        assert_eq!(timestamp(Some(0)), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(Some(86_400 + 3_661)), "1970-01-02T01:01:01Z");
    }

    #[test]
    fn a_fixed_creation_date_makes_packages_reproducible() {
        let g = Geometry::make_box(DVec3::splat(10.0));
        let opts = WriteOptions { creation_date: Some(0), ..Default::default() };
        let a = write_bytes(Format::ThreeMf, &g, &opts).unwrap();
        // The wall clock moves on between the two writes.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let b = write_bytes(Format::ThreeMf, &g, &opts).unwrap();
        assert_eq!(a, b);
        let zip = Archive::open(&a, "t", Limits::default()).unwrap();
        let xml = String::from_utf8(zip.read(ROOT_PART).unwrap().unwrap()).unwrap();
        assert!(xml.contains(">1970-01-01T00:00:00Z<"), "{xml}");
    }

    #[test]
    fn corrupt_and_truncated_packages_are_errors_not_panics() {
        let g = Geometry::make_box(DVec3::splat(10.0));
        let bytes = write_bytes(Format::ThreeMf, &g, &WriteOptions::default()).unwrap();
        for n in (0..bytes.len()).step_by(7) {
            assert!(rd(&bytes[..n]).is_err(), "prefix {n}");
        }
        for i in (0..bytes.len()).step_by(11) {
            let mut b = bytes.clone();
            b[i] ^= 0x55;
            let _ = rd(&b);
        }
        assert!(rd(b"PK\x03\x04 and then garbage that is not a zip").is_err());
    }
}
