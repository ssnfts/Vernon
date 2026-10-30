//! Reader for Gmsh `.msh` format 4.1, ASCII and binary, written from the
//! format description in the public Gmsh manual. Tetrahedra come back as
//! corner nodes (C3D4); lower-dimensional elements are skipped and other
//! volume elements are refused, since dropping them would leave holes.

use std::collections::{HashMap, HashSet};

use glam::DVec3;

/// The `$MeshFormat` header of a mesh file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MshHeader {
    pub version: String,
    pub binary: bool,
    pub data_size: usize,
}

/// Read the `$MeshFormat` section at the start of a mesh file.
pub fn read_header(bytes: &[u8]) -> Result<MshHeader, String> {
    Cursor::new(bytes, false).header()
}

/// Nodes and corner-node tetrahedra of a format 4.1 file. Only nodes used by a
/// tetrahedron are kept, renumbered from zero in order of first use.
pub fn read_msh4(bytes: &[u8]) -> Result<(Vec<DVec3>, Vec<[u32; 4]>), String> {
    let mut c = Cursor::new(bytes, false);
    let h = c.header()?;
    if h.version != "4.1" {
        return Err(format!("unsupported mesh format version {} (expected 4.1)", h.version));
    }
    if h.data_size != 8 {
        return Err(format!("unsupported data size {} (expected 8)", h.data_size));
    }
    if h.binary {
        let one = c.take(4)?;
        if one != 1i32.to_le_bytes() {
            return Err("binary mesh file is not little-endian".into());
        }
    }
    c.binary = h.binary;
    c.expect_line("$EndMeshFormat")?;

    let mut nodes: Option<HashMap<u64, DVec3>> = None;
    let mut raw: Option<Vec<[u64; 4]>> = None;
    while let Some(section) = c.next_section()? {
        match section.as_str() {
            "$Nodes" if nodes.is_some() => return Err("more than one $Nodes section".into()),
            "$Elements" if raw.is_some() => return Err("more than one $Elements section".into()),
            "$Nodes" => {
                nodes = Some(c.nodes()?);
                c.expect_line("$EndNodes")?;
            }
            "$Elements" => {
                raw = Some(c.elements()?);
                c.expect_line("$EndElements")?;
            }
            other => c.skip_section(other)?,
        }
    }
    let nodes = nodes.ok_or("the mesh file has no $Nodes section")?;
    let raw = raw.ok_or("the mesh file has no $Elements section")?;
    if raw.is_empty() {
        return Err("the mesh file has no tetrahedra".into());
    }

    let mut index: HashMap<u64, u32> = HashMap::new();
    let mut points = Vec::new();
    let mut tets = Vec::with_capacity(raw.len());
    for t in raw {
        let mut e = [0u32; 4];
        for (k, id) in t.iter().enumerate() {
            let p = *nodes.get(id).ok_or_else(|| format!("element references missing node {id}"))?;
            e[k] = *index.entry(*id).or_insert_with(|| {
                points.push(p);
                (points.len() - 1) as u32
            });
        }
        tets.push(e);
    }
    Ok((points, tets))
}

/// Node count of an element type, and whether it is a tetrahedron. Types that
/// are not skippable surface or lower elements are refused.
fn element_kind(ty: i32) -> Result<(usize, bool), String> {
    Ok(match ty {
        4 => (4, true),
        11 => (10, true),
        15 => (1, false),
        1 => (2, false),
        8 => (3, false),
        2 => (3, false),
        9 => (6, false),
        3 => (4, false),
        16 => (8, false),
        10 => (9, false),
        5 | 6 | 7 | 12 | 13 | 14 | 17 | 18 | 19 => return Err(format!("element type {ty} is a volume element that is not a tetrahedron")),
        _ => return Err(format!("unsupported element type {ty}")),
    })
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
    binary: bool,
}

impl<'a> Cursor<'a> {
    fn new(b: &'a [u8], binary: bool) -> Self {
        Cursor { b, pos: 0, binary }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.b.len() - self.pos < n {
            return Err("the mesh file is truncated".into());
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn skip_space(&mut self) {
        while self.b.get(self.pos).is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }

    /// The next non-blank line, trimmed; `None` at the end of the file.
    fn line(&mut self) -> Option<String> {
        self.skip_space();
        if self.pos >= self.b.len() {
            return None;
        }
        let rest = &self.b[self.pos..];
        let n = rest.iter().position(|&c| c == b'\n').unwrap_or(rest.len());
        self.pos += n;
        Some(String::from_utf8_lossy(&rest[..n]).trim().to_string())
    }

    fn header(&mut self) -> Result<MshHeader, String> {
        if self.line().as_deref() != Some("$MeshFormat") {
            return Err("not a mesh file: it does not start with $MeshFormat".into());
        }
        let l = self.line().ok_or("the mesh file is truncated")?;
        let t: Vec<&str> = l.split_whitespace().collect();
        if t.len() != 3 {
            return Err(format!("bad $MeshFormat line '{l}'"));
        }
        let binary = match t[1] {
            "0" => false,
            "1" => true,
            x => return Err(format!("bad file type '{x}' in $MeshFormat")),
        };
        let data_size = t[2].parse().map_err(|_| format!("bad data size '{}' in $MeshFormat", t[2]))?;
        // A binary header ends in one newline, followed by raw bytes that may look like blanks.
        if self.b.get(self.pos) == Some(&b'\n') {
            self.pos += 1;
        }
        Ok(MshHeader { version: t[0].to_string(), binary, data_size })
    }

    fn expect_line(&mut self, want: &str) -> Result<(), String> {
        match self.line() {
            Some(l) if l == want => Ok(()),
            Some(l) => Err(format!("expected {want}, found '{}'", l.chars().take(40).collect::<String>())),
            None => Err(format!("the mesh file is truncated: {want} is missing")),
        }
    }

    fn next_section(&mut self) -> Result<Option<String>, String> {
        match self.line() {
            None => Ok(None),
            Some(l) if l.starts_with('$') && !l.starts_with("$End") => {
                // Section data starts right after this line's newline.
                if self.b.get(self.pos) == Some(&b'\n') {
                    self.pos += 1;
                }
                Ok(Some(l))
            }
            Some(l) => Err(format!("expected a section, found '{}'", l.chars().take(40).collect::<String>())),
        }
    }

    /// Move past a section this reader does not use.
    fn skip_section(&mut self, name: &str) -> Result<(), String> {
        let end = format!("$End{}", &name[1..]);
        let rest = &self.b[self.pos..];
        let at = rest.windows(end.len()).position(|w| w == end.as_bytes()).ok_or_else(|| format!("the mesh file is truncated: {end} is missing"))?;
        self.pos += at + end.len();
        Ok(())
    }

    fn token(&mut self) -> Result<&'a str, String> {
        self.skip_space();
        let rest = &self.b[self.pos..];
        let n = rest.iter().position(|c| c.is_ascii_whitespace()).unwrap_or(rest.len());
        if n == 0 {
            return Err("the mesh file is truncated".into());
        }
        self.pos += n;
        std::str::from_utf8(&rest[..n]).map_err(|_| "non-text data in an ASCII mesh file".to_string())
    }

    fn size(&mut self) -> Result<u64, String> {
        if self.binary {
            let s = self.take(8)?;
            return Ok(u64::from_le_bytes(s.try_into().map_err(|_| "the mesh file is truncated")?));
        }
        let t = self.token()?;
        t.parse().map_err(|_| format!("bad count or tag '{t}'"))
    }

    fn int(&mut self) -> Result<i32, String> {
        if self.binary {
            let s = self.take(4)?;
            return Ok(i32::from_le_bytes(s.try_into().map_err(|_| "the mesh file is truncated")?));
        }
        let t = self.token()?;
        t.parse().map_err(|_| format!("bad integer '{t}'"))
    }

    fn real(&mut self) -> Result<f64, String> {
        let v = if self.binary {
            let s = self.take(8)?;
            f64::from_le_bytes(s.try_into().map_err(|_| "the mesh file is truncated")?)
        } else {
            let t = self.token()?;
            t.parse().map_err(|_| format!("bad coordinate '{t}'"))?
        };
        if !v.is_finite() {
            return Err("a node coordinate is not finite".into());
        }
        Ok(v)
    }

    fn nodes(&mut self) -> Result<HashMap<u64, DVec3>, String> {
        let (blocks, total) = (self.size()?, self.size()?);
        let (_min_tag, _max_tag) = (self.size()?, self.size()?);
        let mut nodes = HashMap::new();
        for _ in 0..blocks {
            let (dim, _entity, parametric) = (self.int()?, self.int()?, self.int()?);
            if !(0..=3).contains(&dim) || !(0..=1).contains(&parametric) {
                return Err(format!("bad node block header (dimension {dim}, parametric {parametric})"));
            }
            let n = self.size()?;
            if n > total - (nodes.len() as u64).min(total) {
                return Err(format!("node blocks hold more nodes than the declared {total}"));
            }
            let mut tags = Vec::new();
            for _ in 0..n {
                tags.push(self.size()?);
            }
            for tag in tags {
                let p = DVec3::new(self.real()?, self.real()?, self.real()?);
                for _ in 0..parametric * dim {
                    self.real()?;
                }
                if nodes.insert(tag, p).is_some() {
                    return Err(format!("duplicate node {tag}"));
                }
            }
        }
        if nodes.len() as u64 != total {
            return Err(format!("the file declares {total} nodes but holds {}", nodes.len()));
        }
        Ok(nodes)
    }

    fn elements(&mut self) -> Result<Vec<[u64; 4]>, String> {
        let (blocks, total) = (self.size()?, self.size()?);
        let (_min_tag, _max_tag) = (self.size()?, self.size()?);
        let mut tets = Vec::new();
        let mut seen = 0u64;
        for _ in 0..blocks {
            let (_dim, _entity, ty) = (self.int()?, self.int()?, self.int()?);
            let (count, is_tet) = element_kind(ty)?;
            let n = self.size()?;
            if n > total - seen {
                return Err(format!("element blocks hold more elements than the declared {total}"));
            }
            for _ in 0..n {
                let tag = self.size()?;
                let mut conn = [0u64; 10];
                for slot in conn.iter_mut().take(count) {
                    *slot = self.size()?;
                }
                if is_tet {
                    let t = [conn[0], conn[1], conn[2], conn[3]];
                    if t.iter().collect::<HashSet<_>>().len() != 4 {
                        return Err(format!("element {tag} repeats a corner node"));
                    }
                    tets.push(t);
                }
            }
            seen += n;
        }
        if seen != total {
            return Err(format!("the file declares {total} elements but holds {seen}"));
        }
        Ok(tets)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Two tetrahedra sharing a face, plus a surface triangle and an unused node.
    pub(crate) const ASCII: &str = "$MeshFormat\n4.1 0 8\n$EndMeshFormat\n\
$Entities\n0 0 0 1\n1 0 0 0 1 1 1 0 0\n$EndEntities\n\
$Nodes\n2 6 1 9\n3 1 0 5\n1\n2\n3\n7\n9\n0 0 0\n10 0 0\n0 10 0\n0 0 10\n10 10 10\n0 2 0 1\n8\n99 99 99\n$EndNodes\n\
$Elements\n2 3 1 3\n2 1 2 1\n1 1 2 3\n3 1 4 2\n2 1 2 3 7\n3 2 3 7 9\n$EndElements\n\
$ElementData\n1\n\"min_dihedral\"\n$EndElementData\n";

    fn err(text: &str) -> String {
        read_msh4(text.as_bytes()).unwrap_err()
    }

    /// `ASCII`'s mesh in the binary encoding.
    fn binary() -> Vec<u8> {
        let mut b = b"$MeshFormat\n4.1 1 8\n".to_vec();
        b.extend(1i32.to_le_bytes());
        b.extend(b"\n$EndMeshFormat\n$Nodes\n");
        for v in [1u64, 5, 1, 9] {
            b.extend(v.to_le_bytes());
        }
        for v in [3i32, 1, 0] {
            b.extend(v.to_le_bytes());
        }
        b.extend(5u64.to_le_bytes());
        for v in [1u64, 2, 3, 7, 9] {
            b.extend(v.to_le_bytes());
        }
        for v in [0.0f64, 0., 0., 10., 0., 0., 0., 10., 0., 0., 0., 10., 10., 10., 10.] {
            b.extend(v.to_le_bytes());
        }
        b.extend(b"\n$EndNodes\n$Elements\n");
        for v in [1u64, 2, 1, 2] {
            b.extend(v.to_le_bytes());
        }
        for v in [3i32, 1, 4] {
            b.extend(v.to_le_bytes());
        }
        b.extend(2u64.to_le_bytes());
        for v in [1u64, 1, 2, 3, 7, 2, 2, 3, 7, 9] {
            b.extend(v.to_le_bytes());
        }
        b.extend(b"\n$EndElements\n");
        b
    }

    fn check_two_tets(p: &[DVec3], t: &[[u32; 4]]) {
        assert_eq!(p.len(), 5, "the unused node is dropped");
        assert_eq!(t, &[[0, 1, 2, 3], [1, 2, 3, 4]]);
        assert_eq!(p[3], DVec3::new(0.0, 0.0, 10.0));
        assert_eq!(p[4], DVec3::splat(10.0));
    }

    #[test]
    fn ascii_tets_are_read_and_other_sections_skipped() {
        let (p, t) = read_msh4(ASCII.as_bytes()).unwrap();
        check_two_tets(&p, &t);
    }

    #[test]
    fn crlf_line_ends_are_accepted() {
        let (p, t) = read_msh4(ASCII.replace('\n', "\r\n").as_bytes()).unwrap();
        check_two_tets(&p, &t);
    }

    #[test]
    fn binary_tets_match_ascii() {
        let (p, t) = read_msh4(&binary()).unwrap();
        check_two_tets(&p, &t);
    }

    #[test]
    fn binary_data_bytes_that_look_like_whitespace_survive() {
        // Node tag 10 starts with byte 0x0a, a newline in text.
        let mut b = binary();
        let from = b.windows(7).position(|w| w == b"$Nodes\n").unwrap() + 7;
        let tag_at = from + 32 + 12 + 8;
        b[tag_at..tag_at + 8].copy_from_slice(&10u64.to_le_bytes());
        let conn_at = b.len() - b"\n$EndElements\n".len() - 80 + 8;
        b[conn_at..conn_at + 8].copy_from_slice(&10u64.to_le_bytes());
        let (p, t) = read_msh4(&b).unwrap();
        assert_eq!(p.len(), 5);
        assert_eq!(t[0], [0, 1, 2, 3]);
    }

    #[test]
    fn tet10_keeps_its_corners() {
        let text = "$MeshFormat\n4.1 0 8\n$EndMeshFormat\n$Nodes\n1 4 1 4\n3 1 0 4\n1\n2\n3\n4\n0 0 0\n1 0 0\n0 1 0\n0 0 1\n$EndNodes\n$Elements\n1 1 1 1\n3 1 11 1\n1 1 2 3 4 1 2 3 4 1 2\n$EndElements\n";
        assert_eq!(read_msh4(text.as_bytes()).unwrap().1, vec![[0, 1, 2, 3]]);
    }

    #[test]
    fn malformed_files_are_refused_with_a_reason() {
        assert!(err("hello\n").contains("does not start with"));
        assert!(err("$MeshFormat\n4.1 0\n$EndMeshFormat\n").contains("bad $MeshFormat"));
        assert!(err("$MeshFormat\n4.0 0 8\n$EndMeshFormat\n").contains("version 4.0"));
        assert!(err("$MeshFormat\n4.1 0 4\n$EndMeshFormat\n").contains("data size"));
        assert!(err("$MeshFormat\n4.1 0 8\n$EndMeshFormat\n").contains("no $Nodes"));
        let cut = &ASCII[..ASCII.find("$Elements").unwrap()];
        assert!(err(cut).contains("no $Elements"));
        let cut = &ASCII[..ASCII.find("$EndNodes").unwrap() - 8];
        assert!(err(cut).contains("truncated") || err(cut).contains("expected"), "{}", err(cut));
        assert!(err(&ASCII.replace("10 10 10", "10 x 10")).contains("bad coordinate"));
        assert!(err(&ASCII.replace("10 10 10", "10 inf 10")).contains("not finite"));
        assert!(err(&ASCII.replace("2 6 1 9", "2 7 1 9")).contains("declares 7 nodes"));
        assert!(err(&ASCII.replace("2 6 1 9", "2 2 1 9")).contains("more nodes"));
        assert!(err(&ASCII.replace("2 3 1 3", "2 4 1 3")).contains("declares 4 elements"));
        assert!(err(&ASCII.replace("3 2 3 7 9", "3 2 3 7 44")).contains("missing node 44"));
        assert!(err(&ASCII.replace("3 2 3 7 9", "3 2 3 7 7")).contains("repeats a corner"));
        assert!(err(&ASCII.replace("\n8\n", "\n7\n")).contains("duplicate node 7"));
        assert!(err(&ASCII.replace("3 1 4 2", "3 1 5 2")).contains("not a tetrahedron"));
        assert!(err(&ASCII.replace("3 1 4 2", "3 1 77 2")).contains("unsupported element type 77"));
        assert!(err(&ASCII.replace("$EndElementData\n", "")).contains("$EndElementData is missing"));
        // surface elements only
        let flat = "$MeshFormat\n4.1 0 8\n$EndMeshFormat\n$Nodes\n1 3 1 3\n2 1 0 3\n1\n2\n3\n0 0 0\n1 0 0\n0 1 0\n$EndNodes\n$Elements\n1 1 1 1\n2 1 2 1\n1 1 2 3\n$EndElements\n";
        assert!(err(flat).contains("no tetrahedra"));
        // binary: cut mid-block, and big-endian marker
        let b = binary();
        assert!(read_msh4(&b[..b.len() - 30]).unwrap_err().contains("truncated"));
        let mut be = b.clone();
        let at = be.iter().position(|&c| c == b'8').unwrap() + 2;
        be[at..at + 4].copy_from_slice(&1i32.to_be_bytes());
        assert!(read_msh4(&be).unwrap_err().contains("little-endian"));
    }

    #[test]
    fn huge_declared_counts_fail_without_allocating() {
        let huge = "$MeshFormat\n4.1 0 8\n$EndMeshFormat\n$Nodes\n1 18446744073709551615 1 9\n3 1 0 18446744073709551615\n1\n$EndNodes\n";
        assert!(err(huge).contains("bad count"));
    }

    #[test]
    fn header_reports_version_and_encoding() {
        assert_eq!(read_header(ASCII.as_bytes()).unwrap(), MshHeader { version: "4.1".into(), binary: false, data_size: 8 });
        assert!(read_header(&binary()).unwrap().binary);
    }
}
