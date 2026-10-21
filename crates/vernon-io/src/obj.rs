//! Wavefront OBJ: `v` (optionally with `r g b`), `vt`, `vn`, `f`, `l`, `g`, `o`.
//! Materials and smoothing groups are ignored.

use std::collections::BTreeMap;
use std::io::Write;

use vernon_core::{AttribArray, DVec3, Geometry, Prim};

use crate::{parse_err, IoResult};

pub(crate) fn read(bytes: &[u8], file: &str) -> IoResult<Geometry> {
    let text = String::from_utf8_lossy(bytes);
    let mut points: Vec<DVec3> = Vec::new();
    let mut colors: Vec<DVec3> = Vec::new();
    let mut has_color = false;
    let mut normals: Vec<DVec3> = Vec::new();
    let mut uvs: Vec<DVec3> = Vec::new();
    let mut prims: Vec<Prim> = Vec::new();
    // (point, normal) and (point, uv) pairs from face corners.
    let mut corner_n: Vec<(u32, u32)> = Vec::new();
    let mut corner_t: Vec<(u32, u32)> = Vec::new();
    let mut groups: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut cur_groups: Vec<String> = Vec::new();
    let mut cur_object: Option<String> = None;

    let mut joined = String::new();
    let mut start_line = 0;
    for (i, raw) in text.lines().enumerate() {
        let lineno = i + 1;
        // Backslash line continuation.
        if let Some(head) = raw.strip_suffix('\\') {
            if joined.is_empty() {
                start_line = lineno;
            }
            joined.push_str(head);
            joined.push(' ');
            continue;
        }
        let (line, lineno) = if joined.is_empty() {
            (raw, lineno)
        } else {
            joined.push_str(raw);
            (joined.as_str(), start_line)
        };
        let line = match line.find('#') {
            Some(c) => &line[..c],
            None => line,
        };
        let mut toks = line.split_ascii_whitespace();
        let Some(kw) = toks.next() else {
            joined.clear();
            continue;
        };
        let err = |m: String| parse_err(file, lineno, m);
        let num = |t: Option<&str>, what: &str| -> IoResult<f64> {
            let t = t.ok_or_else(|| err(format!("missing {what}")))?;
            t.parse::<f64>()
                .map_err(|_| err(format!("bad number '{t}' for {what}")))
        };
        match kw {
            "v" => {
                let x = num(toks.next(), "x")?;
                let y = num(toks.next(), "y")?;
                let z = num(toks.next(), "z")?;
                points.push(DVec3::new(x, y, z));
                let rest: Vec<&str> = toks.collect();
                // `v x y z r g b` (vertex colours) or `v x y z w`.
                if rest.len() >= 3 {
                    let r = num(Some(rest[0]), "red")?;
                    let g = num(Some(rest[1]), "green")?;
                    let b = num(Some(rest[2]), "blue")?;
                    colors.push(DVec3::new(r, g, b));
                    has_color = true;
                } else {
                    colors.push(DVec3::ZERO);
                }
            }
            "vn" => {
                let x = num(toks.next(), "nx")?;
                let y = num(toks.next(), "ny")?;
                let z = num(toks.next(), "nz")?;
                normals.push(DVec3::new(x, y, z));
            }
            "vt" => {
                let u = num(toks.next(), "u")?;
                let v = match toks.next() {
                    Some(t) => num(Some(t), "v")?,
                    None => 0.0,
                };
                uvs.push(DVec3::new(u, v, 0.0));
            }
            "f" | "fo" | "l" => {
                let closed = kw != "l";
                let mut verts = Vec::new();
                for tok in toks {
                    let mut parts = tok.split('/');
                    let p = resolve(parts.next(), points.len(), "vertex", &err)?;
                    verts.push(p);
                    if let Some(t) = parts.next().filter(|s| !s.is_empty()) {
                        corner_t.push((p, resolve(Some(t), uvs.len(), "texture", &err)?));
                    }
                    if let Some(n) = parts.next().filter(|s| !s.is_empty()) {
                        corner_n.push((p, resolve(Some(n), normals.len(), "normal", &err)?));
                    }
                }
                let min = if closed { 3 } else { 2 };
                if verts.len() >= min {
                    let pi = prims.len() as u32;
                    for name in cur_groups.iter().chain(cur_object.iter()) {
                        groups.entry(name.clone()).or_default().push(pi);
                    }
                    prims.push(Prim {
                        vertices: verts,
                        closed,
                    });
                }
            }
            "g" => cur_groups = toks.map(str::to_string).collect(),
            "o" => {
                let name: Vec<&str> = toks.collect();
                cur_object = (!name.is_empty()).then(|| name.join(" "));
            }
            // mtllib, usemtl, s, p, vp, curv, surf, ... are ignored.
            _ => {}
        }
        joined.clear();
    }

    let np = points.len();
    let mut geo = Geometry {
        points,
        prims,
        ..Default::default()
    };
    if has_color {
        geo.point_attribs.set("Cd", AttribArray::Vec3(colors));
    }
    if !corner_n.is_empty() {
        let mut acc = vec![DVec3::ZERO; np];
        for &(p, n) in &corner_n {
            acc[p as usize] += normals[n as usize];
        }
        for n in &mut acc {
            *n = n.normalize_or_zero();
        }
        geo.point_attribs.set("N", AttribArray::Vec3(acc));
    } else if !normals.is_empty() && normals.len() == np && geo.prims.is_empty() {
        // Point cloud with one `vn` per `v`.
        geo.point_attribs.set("N", AttribArray::Vec3(normals));
    }
    if !corner_t.is_empty() {
        let mut uv = vec![DVec3::ZERO; np];
        for &(p, t) in &corner_t {
            uv[p as usize] = uvs[t as usize];
        }
        geo.point_attribs.set("uv", AttribArray::Vec3(uv));
    } else if !uvs.is_empty() && uvs.len() == np && geo.prims.is_empty() {
        geo.point_attribs.set("uv", AttribArray::Vec3(uvs));
    }
    geo.prim_groups = groups;
    Ok(geo)
}

/// Resolve a 1-based (or negative, relative) OBJ index against `count`
/// elements defined so far.
fn resolve(
    tok: Option<&str>,
    count: usize,
    what: &str,
    err: &dyn Fn(String) -> crate::IoError,
) -> IoResult<u32> {
    let tok = tok.filter(|s| !s.is_empty()).ok_or_else(|| err(format!("missing {what} index")))?;
    let i: i64 = tok
        .parse()
        .map_err(|_| err(format!("bad {what} index '{tok}'")))?;
    let r = if i > 0 { i - 1 } else { count as i64 + i };
    if i == 0 || r < 0 || r >= count as i64 {
        return Err(err(format!(
            "{what} index {i} out of range ({count} defined so far)"
        )));
    }
    Ok(r as u32)
}

pub(crate) fn write<W: Write>(w: &mut W, geo: &Geometry, scale: f64) -> IoResult<()> {
    let cd = geo.point_attribs.vec3s("Cd").ok();
    let nrm = geo.point_attribs.vec3s("N").ok();
    let uv = geo.point_attribs.vec3s("uv").ok();
    writeln!(
        w,
        "# Vernon OBJ: {} points, {} primitives",
        geo.point_count(),
        geo.prim_count()
    )?;
    for (i, p) in geo.points.iter().enumerate() {
        let q = *p * scale;
        match cd {
            Some(c) => writeln!(
                w,
                "v {} {} {} {} {} {}",
                q.x, q.y, q.z, c[i].x, c[i].y, c[i].z
            )?,
            None => writeln!(w, "v {} {} {}", q.x, q.y, q.z)?,
        }
    }
    if let Some(uv) = uv {
        for t in uv {
            writeln!(w, "vt {} {}", t.x, t.y)?;
        }
    }
    if let Some(n) = nrm {
        for n in n {
            writeln!(w, "vn {} {} {}", n.x, n.y, n.z)?;
        }
    }
    for prim in &geo.prims {
        let (kw, min) = if prim.closed { ("f", 3) } else { ("l", 2) };
        if prim.vertices.len() < min {
            continue;
        }
        w.write_all(kw.as_bytes())?;
        for &v in &prim.vertices {
            let i = v + 1;
            match (prim.closed, uv.is_some(), nrm.is_some()) {
                (true, true, true) => write!(w, " {i}/{i}/{i}")?,
                (true, true, false) => write!(w, " {i}/{i}")?,
                (true, false, true) => write!(w, " {i}//{i}")?,
                _ => write!(w, " {i}")?,
            }
        }
        writeln!(w)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::test_util::*;
    use crate::{read_bytes, write_bytes, Format, IoError, ReadOptions, WriteOptions};
    use vernon_core::{AttribArray, DVec3, Geometry, Prim};

    fn rd(s: &str) -> Geometry {
        read_bytes(Format::Obj, s.as_bytes(), &ReadOptions::default()).unwrap()
    }

    #[test]
    fn box_round_trip_is_exact() {
        let g = Geometry::make_box(DVec3::new(1.5, 2.25, 0.1));
        let bytes = write_bytes(Format::Obj, &g, &WriteOptions::default()).unwrap();
        let back = rd(std::str::from_utf8(&bytes).unwrap());
        assert_eq!(back.points, g.points);
        assert_eq!(back.prims, g.prims);
    }

    #[test]
    fn ngons_negative_indices_groups_and_continuations() {
        let src = "\
# comment
mtllib x.mtl
o Panel
v 0 0 0
v 2 0 0
v 3 1 0
v 1 2 0
v -1 1 0
vn 0 0 1
vt 0 0
vt 1 1
g left right
usemtl red
s off
f -5/1/1 -4/2/1 -3/1/1 \\
  -2//1 -1//1
g
f 1 2 3
l 1 3 5
";
        let g = rd(src);
        assert_eq!(g.point_count(), 5);
        assert_eq!(g.prim_count(), 3);
        assert_eq!(g.prims[0].vertices, vec![0, 1, 2, 3, 4]);
        assert!(g.prims[0].closed);
        assert_eq!(g.prims[2], Prim::polyline(vec![0, 2, 4]));
        assert_eq!(g.prim_groups["left"], vec![0]);
        assert_eq!(g.prim_groups["right"], vec![0]);
        assert_eq!(g.prim_groups["Panel"], vec![0, 1, 2]);
        let n = g.point_attribs.vec3s("N").unwrap();
        assert!(approx(n[3], DVec3::Z, 1e-12));
        let uv = g.point_attribs.vec3s("uv").unwrap();
        assert!(approx(uv[1], DVec3::new(1.0, 1.0, 0.0), 1e-12));
        assert!((g.prim_area(0) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn attributes_round_trip() {
        let mut g = Geometry::make_box(DVec3::ONE);
        g.compute_normals();
        g.prim_attribs.remove("N");
        let n = g.point_count();
        let cd: Vec<DVec3> = (0..n).map(|i| DVec3::new(i as f64 / 8.0, 0.5, 1.0)).collect();
        let uv: Vec<DVec3> = (0..n).map(|i| DVec3::new(i as f64, 1.0 - i as f64, 0.0)).collect();
        g.point_attribs.set("Cd", AttribArray::Vec3(cd.clone()));
        g.point_attribs.set("uv", AttribArray::Vec3(uv.clone()));
        let bytes = write_bytes(Format::Obj, &g, &WriteOptions::default()).unwrap();
        let back = read_bytes(Format::Obj, &bytes, &ReadOptions::default()).unwrap();
        assert_eq!(back.point_attribs.vec3s("Cd").unwrap(), &cd[..]);
        assert_eq!(back.point_attribs.vec3s("uv").unwrap(), &uv[..]);
        let n0 = g.point_attribs.vec3s("N").unwrap();
        let n1 = back.point_attribs.vec3s("N").unwrap();
        for i in 0..n {
            assert!(approx(n0[i], n1[i], 1e-12));
        }
    }

    #[test]
    fn point_cloud_with_normals() {
        let g = rd("v 0 0 0\nv 1 0 0\nvn 0 0 1\nvn 0 1 0\n");
        assert!(g.is_point_cloud());
        assert_eq!(g.point_attribs.vec3s("N").unwrap()[1], DVec3::Y);
    }

    #[test]
    fn bad_index_reports_line() {
        let r = read_bytes(
            Format::Obj,
            b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 4\n",
            &ReadOptions::default(),
        );
        match r {
            Err(IoError::Parse { line, .. }) => assert_eq!(line, 4),
            other => panic!("expected parse error, got {other:?}"),
        }
        assert!(read_bytes(Format::Obj, b"v 0 0\n", &ReadOptions::default()).is_err());
        assert!(read_bytes(Format::Obj, b"v 0 0 0\nf 0 1 1\n", &ReadOptions::default()).is_err());
    }
}
