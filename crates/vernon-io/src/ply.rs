//! Stanford PLY: ASCII and binary (little or big endian) on read; binary
//! little endian or ASCII on write.
//!
//! Vertex properties: `x y z` -> points, `nx ny nz` -> point `N`,
//! `red green blue` -> point `Cd` (integer types scaled to 0..1), `s t` (or
//! `u v`, `texture_u texture_v`) -> point `uv`, any other scalar -> a point
//! attribute of the same name (Int for integer types, Float otherwise).
//! Face properties: `vertex_indices` / `vertex_index` list -> polygons,
//! `red green blue` -> prim `Cd`, other scalars -> prim attributes.
//! Other elements and list properties are skipped.

use std::io::Write;

use vernon_core::{AttribArray, AttribTable, DVec3, Geometry, Prim};

use crate::{parse_err, IoError, IoResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(s: &str) -> Option<Scalar> {
        Some(match s {
            "char" | "int8" => Scalar::I8,
            "uchar" | "uint8" => Scalar::U8,
            "short" | "int16" => Scalar::I16,
            "ushort" | "uint16" => Scalar::U16,
            "int" | "int32" => Scalar::I32,
            "uint" | "uint32" => Scalar::U32,
            "float" | "float32" => Scalar::F32,
            "double" | "float64" => Scalar::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }

    fn is_int(self) -> bool {
        !matches!(self, Scalar::F32 | Scalar::F64)
    }

    /// Full-scale value used to bring colour channels to 0..1.
    fn colour_scale(self) -> f64 {
        match self {
            Scalar::U16 | Scalar::I16 => 65535.0,
            Scalar::F32 | Scalar::F64 => 1.0,
            _ => 255.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PropKind {
    Scalar(Scalar),
    List(Scalar, Scalar),
}

#[derive(Debug, Clone)]
struct Property {
    name: String,
    kind: PropKind,
}

#[derive(Debug, Clone)]
struct Element {
    name: String,
    count: usize,
    props: Vec<Property>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Encoding {
    Ascii,
    BinLe,
    BinBe,
}

struct Header {
    encoding: Encoding,
    elements: Vec<Element>,
    /// Byte offset of the first data byte.
    body: usize,
    /// Number of header lines (for ASCII error line numbers).
    lines: usize,
}

fn parse_header(bytes: &[u8], file: &str) -> IoResult<Header> {
    let mut pos = 0;
    let mut line_no = 0;
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    loop {
        let end = bytes[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|i| pos + i)
            .ok_or_else(|| parse_err(file, line_no + 1, "PLY header has no end_header"))?;
        let raw = String::from_utf8_lossy(&bytes[pos..end]);
        let line = raw.trim();
        line_no += 1;
        pos = end + 1;
        let err = |m: String| parse_err(file, line_no, m);
        if line_no == 1 {
            if line != "ply" {
                return Err(err("not a PLY file (missing 'ply' magic)".into()));
            }
            continue;
        }
        let mut t = line.split_ascii_whitespace();
        match t.next() {
            Some("format") => {
                encoding = Some(match t.next() {
                    Some("ascii") => Encoding::Ascii,
                    Some("binary_little_endian") => Encoding::BinLe,
                    Some("binary_big_endian") => Encoding::BinBe,
                    other => return Err(err(format!("unsupported PLY format {other:?}"))),
                })
            }
            Some("element") => {
                let name = t.next().ok_or_else(|| err("element without a name".into()))?;
                let count = t
                    .next()
                    .and_then(|c| c.parse::<usize>().ok())
                    .ok_or_else(|| err(format!("element '{name}' has no valid count")))?;
                elements.push(Element {
                    name: name.to_string(),
                    count,
                    props: Vec::new(),
                });
            }
            Some("property") => {
                let el = elements
                    .last_mut()
                    .ok_or_else(|| err("property before any element".into()))?;
                let ty = t.next().ok_or_else(|| err("property without a type".into()))?;
                let scalar = |s: Option<&str>| {
                    s.and_then(Scalar::parse)
                        .ok_or_else(|| err(format!("unknown property type {s:?}")))
                };
                let kind = if ty == "list" {
                    let ct = scalar(t.next())?;
                    let it = scalar(t.next())?;
                    PropKind::List(ct, it)
                } else {
                    PropKind::Scalar(scalar(Some(ty))?)
                };
                let name = t.next().ok_or_else(|| err("property without a name".into()))?;
                el.props.push(Property {
                    name: name.to_string(),
                    kind,
                });
            }
            Some("end_header") => break,
            Some("comment") | Some("obj_info") | None => {}
            Some(other) => return Err(err(format!("unknown header keyword '{other}'"))),
        }
    }
    let encoding = encoding.ok_or_else(|| parse_err(file, line_no, "PLY header has no format line"))?;
    Ok(Header {
        encoding,
        elements,
        body: pos,
        lines: line_no,
    })
}

/// A stream of scalar values from the PLY body.
trait Source {
    fn scalar(&mut self, t: Scalar) -> IoResult<f64>;
}

struct AsciiSrc<'a> {
    file: &'a str,
    lines: std::str::Lines<'a>,
    cur: std::str::SplitAsciiWhitespace<'a>,
    line: usize,
}

impl Source for AsciiSrc<'_> {
    fn scalar(&mut self, _t: Scalar) -> IoResult<f64> {
        loop {
            if let Some(tok) = self.cur.next() {
                return tok
                    .parse::<f64>()
                    .map_err(|_| parse_err(self.file, self.line, format!("bad number '{tok}'")));
            }
            match self.lines.next() {
                Some(l) => {
                    self.line += 1;
                    self.cur = l.split_ascii_whitespace();
                }
                None => return Err(parse_err(self.file, self.line, "unexpected end of PLY data")),
            }
        }
    }
}

struct BinSrc<'a> {
    file: &'a str,
    data: &'a [u8],
    pos: usize,
    be: bool,
}

impl Source for BinSrc<'_> {
    fn scalar(&mut self, t: Scalar) -> IoResult<f64> {
        let n = t.size();
        let b = self.data.get(self.pos..self.pos + n).ok_or_else(|| {
            parse_err(
                self.file,
                0,
                format!("unexpected end of binary PLY data at body byte {}", self.pos),
            )
        })?;
        self.pos += n;
        let be = self.be;
        macro_rules! num {
            ($ty:ty, $n:literal) => {{
                let mut a = [0u8; $n];
                a.copy_from_slice(b);
                (if be { <$ty>::from_be_bytes(a) } else { <$ty>::from_le_bytes(a) }) as f64
            }};
        }
        Ok(match t {
            Scalar::I8 => b[0] as i8 as f64,
            Scalar::U8 => b[0] as f64,
            Scalar::I16 => num!(i16, 2),
            Scalar::U16 => num!(u16, 2),
            Scalar::I32 => num!(i32, 4),
            Scalar::U32 => num!(u32, 4),
            Scalar::F32 => num!(f32, 4),
            Scalar::F64 => num!(f64, 8),
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum Role {
    Pos(usize),
    Normal(usize),
    Colour(usize, f64),
    Uv(usize),
    Indices,
    Extra(usize),
    Skip,
}

/// Standard vertex property names and the role they map to.
fn vertex_role(name: &str) -> Option<Role> {
    Some(match name {
        "x" => Role::Pos(0),
        "y" => Role::Pos(1),
        "z" => Role::Pos(2),
        "nx" | "normal_x" => Role::Normal(0),
        "ny" | "normal_y" => Role::Normal(1),
        "nz" | "normal_z" => Role::Normal(2),
        "red" | "r" | "diffuse_red" => Role::Colour(0, 1.0),
        "green" | "g" | "diffuse_green" => Role::Colour(1, 1.0),
        "blue" | "b" | "diffuse_blue" => Role::Colour(2, 1.0),
        "s" | "u" | "texture_u" | "texture_s" => Role::Uv(0),
        "t" | "v" | "texture_v" | "texture_t" => Role::Uv(1),
        _ => return None,
    })
}

fn face_role(name: &str) -> Option<Role> {
    Some(match name {
        "vertex_indices" | "vertex_index" => Role::Indices,
        "red" | "diffuse_red" => Role::Colour(0, 1.0),
        "green" | "diffuse_green" => Role::Colour(1, 1.0),
        "blue" | "diffuse_blue" => Role::Colour(2, 1.0),
        _ => return None,
    })
}

struct Extra {
    name: String,
    int: bool,
    values: Vec<f64>,
}

impl Extra {
    fn into_attrib(self, table: &mut AttribTable) {
        let col = if self.int {
            AttribArray::Int(self.values.iter().map(|&v| v as i64).collect())
        } else {
            AttribArray::Float(self.values)
        };
        table.set(self.name, col);
    }
}

/// Roles for one element's properties; unknown scalars become extras.
fn roles(el: &Element, lookup: fn(&str) -> Option<Role>, extras: &mut Vec<Extra>) -> Vec<Role> {
    el.props
        .iter()
        .map(|p| match p.kind {
            PropKind::List(..) => match lookup(&p.name) {
                Some(Role::Indices) => Role::Indices,
                _ => Role::Skip,
            },
            PropKind::Scalar(t) => match lookup(&p.name) {
                Some(Role::Indices) => Role::Skip,
                Some(Role::Colour(i, _)) => Role::Colour(i, t.colour_scale()),
                Some(r) => r,
                None => {
                    extras.push(Extra {
                        name: p.name.clone(),
                        int: t.is_int(),
                        values: Vec::with_capacity(el.count),
                    });
                    Role::Extra(extras.len() - 1)
                }
            },
        })
        .collect()
}

fn skip_element(el: &Element, src: &mut dyn Source) -> IoResult<()> {
    for _ in 0..el.count {
        for p in &el.props {
            skip_prop(p.kind, src)?;
        }
    }
    Ok(())
}

fn skip_prop(kind: PropKind, src: &mut dyn Source) -> IoResult<()> {
    match kind {
        PropKind::Scalar(t) => {
            src.scalar(t)?;
        }
        PropKind::List(ct, it) => {
            let k = src.scalar(ct)?;
            for _ in 0..(k.max(0.0) as usize) {
                src.scalar(it)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn read(bytes: &[u8], file: &str) -> IoResult<Geometry> {
    let h = parse_header(bytes, file)?;
    let body = &bytes[h.body..];
    let text;
    let mut src: Box<dyn Source + '_> = match h.encoding {
        Encoding::Ascii => {
            text = String::from_utf8_lossy(body);
            Box::new(AsciiSrc {
                file,
                lines: text.lines(),
                cur: "".split_ascii_whitespace(),
                line: h.lines,
            })
        }
        Encoding::BinLe | Encoding::BinBe => Box::new(BinSrc {
            file,
            data: body,
            pos: 0,
            be: h.encoding == Encoding::BinBe,
        }),
    };
    let src: &mut dyn Source = src.as_mut();

    let mut geo = Geometry::new();
    let mut seen_vertex = false;
    let mut seen_face = false;
    for el in &h.elements {
        if el.name == "vertex" && !seen_vertex {
            seen_vertex = true;
            read_vertices(el, src, &mut geo, file)?;
        } else if el.name == "face" && !seen_face {
            seen_face = true;
            read_faces(el, src, &mut geo)?;
        } else {
            skip_element(el, src)?;
        }
    }
    let np = geo.points.len();
    for (fi, prim) in geo.prims.iter().enumerate() {
        if let Some(&v) = prim.vertices.iter().find(|&&v| v as usize >= np) {
            return Err(parse_err(
                file,
                0,
                format!("face {fi} references vertex {v}, but there are only {np} vertices"),
            ));
        }
    }
    Ok(geo)
}

fn read_vertices(el: &Element, src: &mut dyn Source, geo: &mut Geometry, file: &str) -> IoResult<()> {
    let mut extras = Vec::new();
    let roles = roles(el, vertex_role, &mut extras);
    let has = |f: fn(&Role) -> bool| roles.iter().any(f);
    for axis in 0..3 {
        if !roles.iter().any(|r| matches!(r, Role::Pos(a) if *a == axis)) {
            return Err(parse_err(file, 0, "PLY vertex element lacks x, y or z"));
        }
    }
    let has_n = has(|r| matches!(r, Role::Normal(_)));
    let has_c = has(|r| matches!(r, Role::Colour(..)));
    let has_uv = has(|r| matches!(r, Role::Uv(_)));
    let n = el.count;
    geo.points.reserve(n);
    let mut normals = Vec::with_capacity(if has_n { n } else { 0 });
    let mut colours = Vec::with_capacity(if has_c { n } else { 0 });
    let mut uvs = Vec::with_capacity(if has_uv { n } else { 0 });
    for _ in 0..n {
        let mut p = [0.0; 3];
        let mut nn = [0.0; 3];
        let mut c = [0.0; 3];
        let mut uv = [0.0; 3];
        for (prop, role) in el.props.iter().zip(&roles) {
            let PropKind::Scalar(t) = prop.kind else {
                skip_prop(prop.kind, src)?;
                continue;
            };
            let v = src.scalar(t)?;
            match *role {
                Role::Pos(i) => p[i] = v,
                Role::Normal(i) => nn[i] = v,
                Role::Colour(i, s) => c[i] = v / s,
                Role::Uv(i) => uv[i] = v,
                Role::Extra(i) => extras[i].values.push(v),
                Role::Indices | Role::Skip => {}
            }
        }
        geo.points.push(DVec3::from_array(p));
        if has_n {
            normals.push(DVec3::from_array(nn));
        }
        if has_c {
            colours.push(DVec3::from_array(c));
        }
        if has_uv {
            uvs.push(DVec3::from_array(uv));
        }
    }
    for e in extras {
        e.into_attrib(&mut geo.point_attribs);
    }
    if has_n {
        geo.point_attribs.set("N", AttribArray::Vec3(normals));
    }
    if has_c {
        geo.point_attribs.set("Cd", AttribArray::Vec3(colours));
    }
    if has_uv {
        geo.point_attribs.set("uv", AttribArray::Vec3(uvs));
    }
    Ok(())
}

fn read_faces(el: &Element, src: &mut dyn Source, geo: &mut Geometry) -> IoResult<()> {
    let mut extras = Vec::new();
    let roles = roles(el, face_role, &mut extras);
    if !roles.iter().any(|r| matches!(r, Role::Indices)) {
        return skip_element(el, src);
    }
    let has_c = roles.iter().any(|r| matches!(r, Role::Colour(..)));
    let mut colours = Vec::new();
    let mut tmp = vec![0.0; extras.len()];
    for _ in 0..el.count {
        let mut verts: Vec<u32> = Vec::new();
        let mut c = [0.0; 3];
        for (prop, role) in el.props.iter().zip(&roles) {
            match (prop.kind, *role) {
                (PropKind::List(ct, it), Role::Indices) => {
                    let k = src.scalar(ct)?.max(0.0) as usize;
                    verts.reserve(k);
                    for _ in 0..k {
                        let v = src.scalar(it)?;
                        // Negative or fractional indices are invalid; map them
                        // out of range so the final check reports them.
                        verts.push(if v >= 0.0 && v.fract() == 0.0 { v as u32 } else { u32::MAX });
                    }
                }
                (PropKind::Scalar(t), role) => {
                    let v = src.scalar(t)?;
                    match role {
                        Role::Colour(i, s) => c[i] = v / s,
                        Role::Extra(i) => tmp[i] = v,
                        _ => {}
                    }
                }
                (kind, _) => skip_prop(kind, src)?,
            }
        }
        if verts.len() >= 3 {
            geo.prims.push(Prim::polygon(verts));
            for (e, &v) in extras.iter_mut().zip(&tmp) {
                e.values.push(v);
            }
            if has_c {
                colours.push(DVec3::from_array(c));
            }
        }
    }
    for e in extras {
        e.into_attrib(&mut geo.prim_attribs);
    }
    if has_c {
        geo.prim_attribs.set("Cd", AttribArray::Vec3(colours));
    }
    Ok(())
}

/// One output record (a vertex or a face), ASCII or binary little endian.
struct Rec {
    buf: Vec<u8>,
    ascii: bool,
}

impl Rec {
    fn sep(&mut self) {
        if self.ascii && !self.buf.is_empty() {
            self.buf.push(b' ');
        }
    }
    fn text(&mut self, s: String) {
        self.sep();
        self.buf.extend_from_slice(s.as_bytes());
    }
    fn f64(&mut self, v: f64) {
        if self.ascii {
            self.text(v.to_string());
        } else {
            self.buf.extend_from_slice(&v.to_le_bytes());
        }
    }
    fn f32(&mut self, v: f64) {
        let v = v as f32;
        if self.ascii {
            self.text(v.to_string());
        } else {
            self.buf.extend_from_slice(&v.to_le_bytes());
        }
    }
    fn u8(&mut self, v: u8) {
        if self.ascii {
            self.text(v.to_string());
        } else {
            self.buf.push(v);
        }
    }
    fn i32(&mut self, v: i32) {
        if self.ascii {
            self.text(v.to_string());
        } else {
            self.buf.extend_from_slice(&v.to_le_bytes());
        }
    }
    fn colour(&mut self, c: DVec3) {
        for x in [c.x, c.y, c.z] {
            self.u8((x.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    fn flush<W: Write>(&mut self, w: &mut W) -> IoResult<()> {
        if self.ascii {
            self.buf.push(b'\n');
        }
        w.write_all(&self.buf)?;
        self.buf.clear();
        Ok(())
    }
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

/// Float/Int attributes that can be written as plain scalar properties.
fn scalar_attribs<'a>(
    table: &'a AttribTable,
    reserved: fn(&str) -> bool,
) -> Vec<(&'a str, &'a AttribArray)> {
    table
        .iter()
        .filter(|(name, col)| {
            matches!(col, AttribArray::Float(_) | AttribArray::Int(_))
                && !name.is_empty()
                && !name.chars().any(|c| c.is_whitespace())
                && !reserved(name)
        })
        .collect()
}

fn header_props<W: Write>(w: &mut W, attribs: &[(&str, &AttribArray)]) -> IoResult<()> {
    for (name, col) in attribs {
        let ty = if matches!(col, AttribArray::Int(_)) { "int" } else { "double" };
        writeln!(w, "property {ty} {name}")?;
    }
    Ok(())
}

fn record_props(rec: &mut Rec, attribs: &[(&str, &AttribArray)], i: usize) {
    for (_, col) in attribs {
        match col {
            AttribArray::Float(v) => rec.f64(v[i]),
            AttribArray::Int(v) => rec.i32(clamp_i32(v[i])),
            _ => {}
        }
    }
}

pub(crate) fn write<W: Write>(w: &mut W, geo: &Geometry, scale: f64, ascii: bool) -> IoResult<()> {
    let nrm = geo.point_attribs.vec3s("N").ok();
    let cd = geo.point_attribs.vec3s("Cd").ok();
    let uv = geo.point_attribs.vec3s("uv").ok();
    let point_extra = scalar_attribs(&geo.point_attribs, |n| vertex_role(n).is_some());
    let faces: Vec<usize> = (0..geo.prims.len())
        .filter(|&i| geo.prims[i].closed && geo.prims[i].vertices.len() >= 3)
        .collect();
    let prim_cd = geo.prim_attribs.vec3s("Cd").ok();
    let prim_extra = scalar_attribs(&geo.prim_attribs, |n| face_role(n).is_some());
    let max_deg = faces.iter().map(|&i| geo.prims[i].vertices.len()).max().unwrap_or(3);
    let wide = max_deg > 255;
    if geo.points.len() > i32::MAX as usize {
        return Err(IoError::Other("too many points for PLY int indices".into()));
    }

    let fmt = if ascii { "ascii" } else { "binary_little_endian" };
    writeln!(w, "ply\nformat {fmt} 1.0\ncomment Vernon export")?;
    writeln!(w, "element vertex {}", geo.points.len())?;
    writeln!(w, "property double x\nproperty double y\nproperty double z")?;
    if nrm.is_some() {
        writeln!(w, "property float nx\nproperty float ny\nproperty float nz")?;
    }
    if cd.is_some() {
        writeln!(w, "property uchar red\nproperty uchar green\nproperty uchar blue")?;
    }
    if uv.is_some() {
        writeln!(w, "property float s\nproperty float t")?;
    }
    header_props(w, &point_extra)?;
    if !faces.is_empty() {
        writeln!(w, "element face {}", faces.len())?;
        let ct = if wide { "int" } else { "uchar" };
        writeln!(w, "property list {ct} int vertex_indices")?;
        if prim_cd.is_some() {
            writeln!(w, "property uchar red\nproperty uchar green\nproperty uchar blue")?;
        }
        header_props(w, &prim_extra)?;
    }
    writeln!(w, "end_header")?;

    let mut rec = Rec {
        buf: Vec::with_capacity(64),
        ascii,
    };
    for (i, p) in geo.points.iter().enumerate() {
        let q = *p * scale;
        rec.f64(q.x);
        rec.f64(q.y);
        rec.f64(q.z);
        if let Some(n) = nrm {
            rec.f32(n[i].x);
            rec.f32(n[i].y);
            rec.f32(n[i].z);
        }
        if let Some(c) = cd {
            rec.colour(c[i]);
        }
        if let Some(t) = uv {
            rec.f32(t[i].x);
            rec.f32(t[i].y);
        }
        record_props(&mut rec, &point_extra, i);
        rec.flush(w)?;
    }
    for &fi in &faces {
        let v = &geo.prims[fi].vertices;
        if wide {
            rec.i32(v.len() as i32);
        } else {
            rec.u8(v.len() as u8);
        }
        for &x in v {
            rec.i32(x as i32);
        }
        if let Some(c) = prim_cd {
            rec.colour(c[fi]);
        }
        record_props(&mut rec, &prim_extra, fi);
        rec.flush(w)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use crate::{read_bytes, write_bytes, Format, ReadOptions, WriteOptions};

    fn rd(b: &[u8]) -> IoResult<Geometry> {
        read_bytes(Format::Ply, b, &ReadOptions::default())
    }

    /// Box plus a pentagon, with point and prim attributes.
    fn sample() -> Geometry {
        let mut g = Geometry::make_box(DVec3::new(10.5, 20.25, 30.125));
        g.compute_normals();
        g.prim_attribs.remove("N");
        let pent = [
            DVec3::new(0.0, 0.0, 50.0),
            DVec3::new(2.0, 0.0, 50.0),
            DVec3::new(3.0, 1.0, 50.0),
            DVec3::new(1.0, 2.0, 50.0),
            DVec3::new(-1.0, 1.0, 50.0),
        ];
        let base = g.points.len() as u32;
        for p in pent {
            g.add_point(p);
        }
        g.add_prim(Prim::polygon((base..base + 5).collect()));
        let np = g.point_count();
        let nf = g.prim_count();
        g.point_attribs.set(
            "Cd",
            AttribArray::Vec3((0..np).map(|i| DVec3::new(i as f64 / 255.0, 1.0, 0.0)).collect()),
        );
        g.point_attribs.set(
            "uv",
            AttribArray::Vec3((0..np).map(|i| DVec3::new(i as f64 * 0.25, 0.5, 0.0)).collect()),
        );
        g.point_attribs.set(
            "dist",
            AttribArray::Float((0..np).map(|i| i as f64 * 0.1 - 0.3).collect()),
        );
        g.prim_attribs
            .set("part", AttribArray::Int((0..nf as i64).map(|i| i % 3).collect()));
        g.prim_attribs.set(
            "Cd",
            AttribArray::Vec3((0..nf).map(|i| DVec3::new(0.0, i as f64 / 255.0, 1.0)).collect()),
        );
        g
    }

    fn check(back: &Geometry, g: &Geometry) {
        back.validate().unwrap();
        assert_eq!(back.points, g.points);
        assert_eq!(back.prims, g.prims);
        let pairs = [("N", 1e-6), ("Cd", 1e-12), ("uv", 1e-6)];
        for (name, tol) in pairs {
            let a = g.point_attribs.vec3s(name).unwrap();
            let b = back.point_attribs.vec3s(name).unwrap();
            for i in 0..a.len() {
                assert!(approx(a[i], b[i], tol), "{name}[{i}]");
            }
        }
        assert_eq!(
            back.point_attribs.floats("dist").unwrap(),
            g.point_attribs.floats("dist").unwrap()
        );
        assert_eq!(
            back.prim_attribs.ints("part").unwrap(),
            g.prim_attribs.ints("part").unwrap()
        );
        let a = g.prim_attribs.vec3s("Cd").unwrap();
        let b = back.prim_attribs.vec3s("Cd").unwrap();
        for i in 0..a.len() {
            assert!(approx(a[i], b[i], 1e-12));
        }
    }

    #[test]
    fn binary_round_trip_with_attributes() {
        let g = sample();
        let bytes = write_bytes(Format::Ply, &g, &WriteOptions::default()).unwrap();
        assert!(bytes.starts_with(b"ply\nformat binary_little_endian 1.0\n"));
        check(&rd(&bytes).unwrap(), &g);
    }

    #[test]
    fn ascii_round_trip_with_attributes() {
        let g = sample();
        let opts = WriteOptions {
            ascii: true,
            ..Default::default()
        };
        let bytes = write_bytes(Format::Ply, &g, &opts).unwrap();
        assert!(bytes.starts_with(b"ply\nformat ascii 1.0\n"));
        check(&rd(&bytes).unwrap(), &g);
    }

    #[test]
    fn handwritten_ascii_with_extras() {
        let src = "ply\r
format ascii 1.0\r
comment made by hand\r
obj_info scanner X\r
element vertex 4\r
property float x\r
property float y\r
property float z\r
property uchar red\r
property uchar green\r
property uchar blue\r
property uchar alpha\r
property float quality\r
element face 2\r
property list uchar int vertex_index\r
property list uchar float texcoord\r
element edge 1\r
property int vertex1\r
property int vertex2\r
end_header\r
0 0 0 255 0 0 255 0.5\r
1 0 0 0 255 0 255 0.25\r
1 1 0 0 0 255 255 1\r
0 1 0 51 102 153 255 2\r
3 0 1 2 6 0 0 1 0 1 1\r
3 0 2 3 0\r
0 1\r
";
        let g = rd(src.as_bytes()).unwrap();
        assert_eq!(g.point_count(), 4);
        assert_eq!(g.prim_count(), 2);
        assert_eq!(g.prims[1].vertices, vec![0, 2, 3]);
        let cd = g.point_attribs.vec3s("Cd").unwrap();
        assert!(approx(cd[3], DVec3::new(0.2, 0.4, 0.6), 1e-12));
        assert_eq!(g.point_attribs.ints("alpha").unwrap(), &[255, 255, 255, 255]);
        assert_eq!(g.point_attribs.floats("quality").unwrap()[3], 2.0);
        assert!((g.surface_area() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn big_endian_binary() {
        let mut b = b"ply\nformat binary_big_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar uint vertex_indices\nend_header\n".to_vec();
        for p in [[0.0f32, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 3.0, 0.0]] {
            for x in p {
                b.extend_from_slice(&x.to_be_bytes());
            }
        }
        b.push(3);
        for i in [0u32, 1, 2] {
            b.extend_from_slice(&i.to_be_bytes());
        }
        let g = rd(&b).unwrap();
        assert_eq!(g.points[1], DVec3::new(4.0, 0.0, 0.0));
        assert!((g.surface_area() - 6.0).abs() < 1e-12);
    }

    #[test]
    fn point_cloud_has_no_face_element() {
        let mut g = Geometry::from_points(vec![DVec3::ZERO, DVec3::ONE]);
        g.point_attribs
            .set("N", AttribArray::Vec3(vec![DVec3::Z, DVec3::X]));
        let bytes = write_bytes(Format::Ply, &g, &WriteOptions::default()).unwrap();
        let header = String::from_utf8_lossy(&bytes[..120]).to_string();
        assert!(!header.contains("element face"));
        let back = rd(&bytes).unwrap();
        assert!(back.is_point_cloud());
        assert_eq!(back.point_attribs.vec3s("N").unwrap()[1], DVec3::X);
    }

    #[test]
    fn errors() {
        assert!(rd(b"PLY\nformat ascii 1.0\nend_header\n").is_err());
        assert!(rd(b"ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\n").is_err());
        let bad_index = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n0 1 0\n3 0 1 7\n";
        assert!(rd(bad_index.as_bytes()).is_err());
        let g = Geometry::make_box(DVec3::ONE);
        let bytes = write_bytes(Format::Ply, &g, &WriteOptions::default()).unwrap();
        assert!(rd(&bytes[..bytes.len() - 3]).is_err());
    }
}
