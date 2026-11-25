//! glTF 2.0 reader (`.glb` binary container and `.gltf` JSON), geometry only.
//!
//! Written from the public Khronos glTF 2.0 specification
//! (<https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html>).
//!
//! - Container: a GLB file is a 12-byte header (`glTF`, version 2, length)
//!   followed by a JSON chunk and an optional BIN chunk; a `.gltf` file is the
//!   JSON alone. Which one a file is gets sniffed from its first bytes, not
//!   from the extension.
//! - Buffers: the GLB BIN chunk, base64 `data:` URIs, or files next to the
//!   `.gltf` (relative paths only; absolute paths, `..` and URL schemes are
//!   refused).
//! - Meshes: every primitive of every mesh reached from the default scene
//!   (or every root node when no scene is named, or every mesh at identity when
//!   there are no nodes), with each node's matrix or translation/rotation/scale
//!   applied down the hierarchy. Triangles, strips and fans become triangles;
//!   lines, line strips and loops become polylines; points become bare points.
//!   Indexed and non-indexed primitives are both read.
//! - Attributes: `POSITION` (required); `NORMAL` -> point `N`; `TEXCOORD_0` ->
//!   point `uv` with `v` flipped to the bottom-left origin used by the OBJ
//!   reader; `COLOR_0` -> point `Cd` (RGB 0..1 as stored, i.e. linear; alpha
//!   dropped). An attribute is kept only when every primitive read has it.
//! - Units and axes: glTF is metres, Y up, right-handed. Positions are scaled
//!   to millimetres. With [`GltfAxes::ZUp`] (the default) the model is rotated
//!   +90 degrees about X, `(x, y, z) -> (x, -z, y)`, so its up axis is +Z;
//!   [`GltfAxes::AsStored`] keeps the file's axes. The choice is recorded in
//!   the detail attributes `gltf_source_frame` and `gltf_axes`, see
//!   [`crate::gltf_frame_note`].
//! - Refused with a clear error: Draco (`KHR_draco_mesh_compression`) and
//!   meshopt (`EXT_meshopt_compression`, `KHR_meshopt_compression`) compressed
//!   data, sparse accessors, and accessors without a buffer view.
//!
//! Declared counts are never trusted: every accessor is bounds-checked
//! against its buffer view and buffer before anything is allocated.

use std::path::{Component, Path};

use serde_json::Value;
use glam::{DMat3, DQuat};
use vernon_core::{AttribArray, DMat4, DVec3, Geometry, Prim};

use crate::{parse_err, IoResult};

/// How a glTF model's axes are mapped on read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GltfAxes {
    /// Rotate the Y-up file +90 degrees about X so up is +Z (Vernon's
    /// convention): `(x, y, z) -> (x, -z, y)`.
    #[default]
    ZUp,
    /// Keep the axes as stored (up stays +Y).
    AsStored,
}

impl GltfAxes {
    /// Value of the `gltf_axes` detail attribute.
    pub fn name(self) -> &'static str {
        match self {
            GltfAxes::ZUp => "z_up",
            GltfAxes::AsStored => "as_stored_y_up",
        }
    }
}

/// Value of the `gltf_source_frame` detail attribute.
pub(crate) const SOURCE_FRAME: &str = "y_up_right_handed_metres";

const GLB_MAGIC: &[u8; 4] = b"glTF";
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

/// Extensions whose data this reader cannot decode.
const COMPRESSION_EXTENSIONS: [&str; 3] = [
    "KHR_draco_mesh_compression",
    "EXT_meshopt_compression",
    "KHR_meshopt_compression",
];

/// Whether `bytes` starts like a glTF file: the GLB magic, or a JSON object
/// whose first kilobyte names the required `asset` property.
pub(crate) fn sniff(bytes: &[u8]) -> bool {
    if bytes.starts_with(GLB_MAGIC) {
        return true;
    }
    let head = &bytes[..bytes.len().min(1024)];
    let text = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
    text.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{')
        && text.windows(7).any(|w| w == b"\"asset\"")
}

pub(crate) fn read(
    bytes: &[u8],
    file: &str,
    base_dir: Option<&Path>,
    axes: GltfAxes,
) -> IoResult<Geometry> {
    let err = |m: String| parse_err(file, 0, m);
    let (json_bytes, bin) = if bytes.starts_with(GLB_MAGIC) {
        split_glb(bytes).map_err(err)?
    } else {
        (bytes, None)
    };
    let json_bytes = json_bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(json_bytes);
    let doc: Value =
        serde_json::from_slice(json_bytes).map_err(|e| err(format!("invalid glTF JSON: {e}")))?;
    let reader = Reader::new(&doc, bin, base_dir).map_err(err)?;
    reader.build(axes).map_err(err)
}

type R<T> = Result<T, String>;

/// Split a GLB image into its JSON chunk and optional BIN chunk.
fn split_glb(b: &[u8]) -> R<(&[u8], Option<&[u8]>)> {
    let word = |at: usize| crate::bin::u32_at(b, at);
    let version = word(4)?;
    if version != 2 {
        return Err(format!("GLB version {version} is not supported (only glTF 2.0)"));
    }
    let length = word(8)? as usize;
    if length > b.len() {
        return Err(format!(
            "GLB header says {length} bytes but the file has {}; it is truncated",
            b.len()
        ));
    }
    let b = &b[..length];
    let mut at = 12;
    let mut json = None;
    let mut bin = None;
    while at < b.len() {
        let len = crate::bin::u32_at(b, at)? as usize;
        let ty = crate::bin::u32_at(b, at + 4)?;
        let start = at + 8;
        let chunk = start
            .checked_add(len)
            .and_then(|end| b.get(start..end))
            .ok_or_else(|| format!("GLB chunk at byte {at} runs past the end of the file"))?;
        match ty {
            CHUNK_JSON if json.is_none() && at == 12 => json = Some(chunk),
            CHUNK_JSON => return Err("GLB has a misplaced or second JSON chunk".into()),
            CHUNK_BIN if bin.is_none() && json.is_some() => bin = Some(chunk),
            _ => {} // unknown chunk types are skipped, as the specification asks
        }
        // Chunks are 4-byte aligned.
        at = start + len.next_multiple_of(4);
    }
    let json = json.ok_or("GLB has no JSON chunk")?;
    Ok((json, bin))
}

struct Reader<'a> {
    doc: &'a Value,
    buffers: Vec<Vec<u8>>,
}

/// glTF accessor element types.
fn type_components(t: &str) -> Option<usize> {
    Some(match t {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        _ => return None,
    })
}

fn component_size(ct: u64) -> Option<usize> {
    Some(match ct {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        _ => return None,
    })
}

fn arr<'v>(v: &'v Value, key: &str) -> &'v [Value] {
    v.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn uint(v: &Value, key: &str) -> R<Option<usize>> {
    match v.get(key) {
        None => Ok(None),
        Some(x) => x
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("'{key}' must be a non-negative integer")),
    }
}

fn numbers<const N: usize>(v: &Value, key: &str) -> R<Option<[f64; N]>> {
    let Some(a) = v.get(key) else { return Ok(None) };
    let a = a.as_array().filter(|a| a.len() == N).ok_or_else(|| {
        format!("'{key}' must be an array of {N} numbers")
    })?;
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(a) {
        *o = x
            .as_f64()
            .filter(|f| f.is_finite())
            .ok_or_else(|| format!("'{key}' must hold finite numbers"))?;
    }
    Ok(Some(out))
}

fn has_extension(v: &Value, name: &str) -> bool {
    v.get("extensions").and_then(|e| e.get(name)).is_some()
}

fn compression_error(what: &str, ext: &str) -> String {
    let kind = if ext.contains("draco") { "Draco" } else { "meshopt" };
    format!(
        "{what} uses {kind} compression ({ext}), which is not supported; re-export the model without mesh compression"
    )
}

impl<'a> Reader<'a> {
    fn new(doc: &'a Value, bin: Option<&[u8]>, base_dir: Option<&Path>) -> Result<Self, String> {
        let version = doc
            .get("asset")
            .and_then(|a| a.get("version"))
            .and_then(Value::as_str)
            .ok_or("not a glTF file: 'asset.version' is missing")?;
        if !version.starts_with("2.") {
            return Err(format!("glTF version {version} is not supported (only 2.x)"));
        }
        for ext in arr(doc, "extensionsRequired").iter().filter_map(Value::as_str) {
            if COMPRESSION_EXTENSIONS.contains(&ext) {
                return Err(compression_error("the file", ext));
            }
        }
        let mut buffers = Vec::new();
        for (i, b) in arr(doc, "buffers").iter().enumerate() {
            let declared = uint(b, "byteLength")?.ok_or(format!("buffer {i} has no byteLength"))?;
            let data = match b.get("uri").and_then(Value::as_str) {
                None if i == 0 => bin
                    .ok_or("buffer 0 has no 'uri' and the file has no GLB binary chunk")?
                    .to_vec(),
                None => return Err(format!("buffer {i} has no 'uri'")),
                Some(uri) => load_uri(uri, base_dir).map_err(|m| format!("buffer {i}: {m}"))?,
            };
            if data.len() < declared {
                return Err(format!(
                    "buffer {i} declares {declared} bytes but only {} are present",
                    data.len()
                ));
            }
            buffers.push(data);
        }
        Ok(Self { doc, buffers })
    }

    /// Read accessor `index` as `count` elements of `ncomp` components each,
    /// flattened. Normalised integers are mapped to 0..1 or -1..1.
    fn accessor(&self, index: usize) -> R<(Vec<f64>, usize)> {
        let acc = arr(self.doc, "accessors")
            .get(index)
            .ok_or(format!("accessor {index} does not exist"))?;
        let what = format!("accessor {index}");
        if acc.get("sparse").is_some() {
            return Err(format!("{what} is sparse, which is not supported"));
        }
        let ct = acc.get("componentType").and_then(Value::as_u64).unwrap_or(0);
        let csize = component_size(ct).ok_or(format!("{what} has unknown componentType {ct}"))?;
        let ty = acc.get("type").and_then(Value::as_str).unwrap_or("");
        let ncomp = type_components(ty).ok_or(format!("{what} has unsupported type '{ty}'"))?;
        let count = uint(acc, "count")?.ok_or(format!("{what} has no count"))?;
        let normalized = acc.get("normalized").and_then(Value::as_bool).unwrap_or(false);
        let view_index = uint(acc, "bufferView")?
            .ok_or(format!("{what} has no bufferView (empty accessors are not supported)"))?;
        let view = arr(self.doc, "bufferViews")
            .get(view_index)
            .ok_or(format!("{what}: bufferView {view_index} does not exist"))?;
        for ext in COMPRESSION_EXTENSIONS {
            if has_extension(view, ext) {
                return Err(compression_error(&format!("bufferView {view_index}"), ext));
            }
        }
        let buf_index = uint(view, "buffer")?.ok_or(format!("bufferView {view_index} has no buffer"))?;
        let buffer = self
            .buffers
            .get(buf_index)
            .ok_or(format!("bufferView {view_index}: buffer {buf_index} does not exist"))?;
        let view_off = uint(view, "byteOffset")?.unwrap_or(0);
        let view_len = uint(view, "byteLength")?.ok_or(format!("bufferView {view_index} has no byteLength"))?;
        let view_bytes = view_off
            .checked_add(view_len)
            .and_then(|end| buffer.get(view_off..end))
            .ok_or(format!("bufferView {view_index} lies outside buffer {buf_index}"))?;
        let elem = csize * ncomp;
        let stride = uint(view, "byteStride")?.unwrap_or(elem);
        if stride < elem {
            return Err(format!("bufferView {view_index} stride {stride} is smaller than an element ({elem} bytes)"));
        }
        let acc_off = uint(acc, "byteOffset")?.unwrap_or(0);
        if count > 0 {
            let end = (count - 1)
                .checked_mul(stride)
                .and_then(|x| x.checked_add(acc_off))
                .and_then(|x| x.checked_add(elem));
            if end.is_none_or(|e| e > view_bytes.len()) {
                return Err(format!("{what} ({count} elements) runs past the end of bufferView {view_index}"));
            }
        }
        let mut out = Vec::with_capacity(count * ncomp);
        for i in 0..count {
            let base = acc_off + i * stride;
            for c in 0..ncomp {
                let at = base + c * csize;
                let s = &view_bytes[at..at + csize];
                let v = match ct {
                    5120 => {
                        let x = s[0] as i8 as f64;
                        if normalized { (x / 127.0).max(-1.0) } else { x }
                    }
                    5121 => {
                        let x = f64::from(s[0]);
                        if normalized { x / 255.0 } else { x }
                    }
                    5122 => {
                        let x = f64::from(i16::from_le_bytes([s[0], s[1]]));
                        if normalized { (x / 32767.0).max(-1.0) } else { x }
                    }
                    5123 => {
                        let x = f64::from(u16::from_le_bytes([s[0], s[1]]));
                        if normalized { x / 65535.0 } else { x }
                    }
                    5125 => f64::from(u32::from_le_bytes([s[0], s[1], s[2], s[3]])),
                    _ => f64::from(f32::from_le_bytes([s[0], s[1], s[2], s[3]])),
                };
                out.push(v);
            }
        }
        Ok((out, ncomp))
    }

    fn vec3s(&self, index: usize, name: &str) -> R<Vec<DVec3>> {
        let (v, n) = self.accessor(index)?;
        if n != 3 {
            return Err(format!("{name} accessor {index} must be VEC3"));
        }
        Ok(v.as_chunks::<3>().0.iter().map(|c| DVec3::new(c[0], c[1], c[2])).collect())
    }

    fn node_matrix(node: &Value) -> R<DMat4> {
        if let Some(m) = numbers::<16>(node, "matrix")? {
            return Ok(DMat4::from_cols_array(&m));
        }
        let t = numbers::<3>(node, "translation")?.unwrap_or([0.0; 3]);
        let r = numbers::<4>(node, "rotation")?.unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let s = numbers::<3>(node, "scale")?.unwrap_or([1.0; 3]);
        let q = DQuat::from_xyzw(r[0], r[1], r[2], r[3]);
        let q = if q.length_squared() > 0.0 { q.normalize() } else { DQuat::IDENTITY };
        Ok(DMat4::from_scale_rotation_translation(
            DVec3::from(s),
            q,
            DVec3::from(t),
        ))
    }

    /// Every (mesh index, world matrix) pair to emit, in scene order.
    fn instances(&self) -> R<Vec<(usize, DMat4)>> {
        let nodes = arr(self.doc, "nodes");
        if nodes.is_empty() {
            return Ok((0..arr(self.doc, "meshes").len()).map(|m| (m, DMat4::IDENTITY)).collect());
        }
        let scenes = arr(self.doc, "scenes");
        let roots: Vec<usize> = match uint(self.doc, "scene")?.or(if scenes.is_empty() { None } else { Some(0) }) {
            Some(s) => {
                let scene = scenes.get(s).ok_or(format!("scene {s} does not exist"))?;
                arr(scene, "nodes")
                    .iter()
                    .map(|n| n.as_u64().map(|n| n as usize).ok_or("scene node index must be an integer".to_string()))
                    .collect::<R<_>>()?
            }
            None => {
                let mut is_child = vec![false; nodes.len()];
                for n in nodes {
                    for c in arr(n, "children").iter().filter_map(Value::as_u64) {
                        if let Some(f) = is_child.get_mut(c as usize) {
                            *f = true;
                        }
                    }
                }
                (0..nodes.len()).filter(|&i| !is_child[i]).collect()
            }
        };
        let mut out = Vec::new();
        let mut on_path = vec![false; nodes.len()];
        for r in roots {
            self.walk(r, DMat4::IDENTITY, &mut on_path, &mut out)?;
        }
        Ok(out)
    }

    fn walk(&self, index: usize, parent: DMat4, on_path: &mut [bool], out: &mut Vec<(usize, DMat4)>) -> R<()> {
        let node = arr(self.doc, "nodes").get(index).ok_or(format!("node {index} does not exist"))?;
        if on_path[index] {
            return Err(format!("node {index} is its own ancestor; the node hierarchy has a cycle"));
        }
        on_path[index] = true;
        let world = parent * Self::node_matrix(node).map_err(|m| format!("node {index}: {m}"))?;
        if let Some(m) = uint(node, "mesh")? {
            out.push((m, world));
        }
        for c in arr(node, "children") {
            let c = c.as_u64().ok_or(format!("node {index}: child index must be an integer"))? as usize;
            self.walk(c, world, on_path, out)?;
        }
        on_path[index] = false;
        Ok(())
    }

    fn build(&self, axes: GltfAxes) -> R<Geometry> {
        let axis = match axes {
            // Columns: x -> x, y -> +z, z -> -y.
            GltfAxes::ZUp => DMat4::from_cols(
                [1.0, 0.0, 0.0, 0.0].into(),
                [0.0, 0.0, 1.0, 0.0].into(),
                [0.0, -1.0, 0.0, 0.0].into(),
                [0.0, 0.0, 0.0, 1.0].into(),
            ),
            GltfAxes::AsStored => DMat4::IDENTITY,
        };
        let to_mm = DMat4::from_scale(DVec3::splat(1000.0));
        let meshes = arr(self.doc, "meshes");
        let mut geo = Geometry::new();
        let mut acc = Attrs::default();
        for (mesh_index, world) in self.instances()? {
            let mesh = meshes.get(mesh_index).ok_or(format!("mesh {mesh_index} does not exist"))?;
            let m = to_mm * axis * world;
            let lin = DMat3::from_mat4(axis * world);
            let flip = lin.determinant() < 0.0;
            let normal_m = if lin.determinant().abs() > 1e-300 { lin.inverse().transpose() } else { lin };
            for (pi, prim) in arr(mesh, "primitives").iter().enumerate() {
                let what = format!("mesh {mesh_index} primitive {pi}");
                self.primitive(&what, prim, m, normal_m, flip, &mut geo, &mut acc)
                    .map_err(|e| format!("{what}: {e}"))?;
            }
        }
        acc.finish(&mut geo);
        geo.detail_attribs
            .set("gltf_source_frame", AttribArray::Str(vec![SOURCE_FRAME.to_string()]));
        geo.detail_attribs
            .set("gltf_axes", AttribArray::Str(vec![axes.name().to_string()]));
        Ok(geo)
    }

    #[allow(clippy::too_many_arguments)]
    fn primitive(
        &self,
        what: &str,
        prim: &Value,
        m: DMat4,
        normal_m: DMat3,
        flip: bool,
        geo: &mut Geometry,
        acc: &mut Attrs,
    ) -> R<()> {
        for ext in COMPRESSION_EXTENSIONS {
            if has_extension(prim, ext) {
                return Err(compression_error(what, ext));
            }
        }
        let attrs = prim.get("attributes").ok_or("primitive has no attributes")?;
        let pos_index = uint(attrs, "POSITION")?.ok_or("primitive has no POSITION attribute")?;
        let positions = self.vec3s(pos_index, "POSITION")?;
        let n = positions.len();
        let base = geo.points.len();
        let base_u32 = u32::try_from(base + n).map(|_| base as u32).map_err(|_| "too many points".to_string())?;
        geo.points.extend(positions.iter().map(|p| m.transform_point3(*p)));

        let per_vertex = |key: &str| -> R<Option<(Vec<f64>, usize)>> {
            match uint(attrs, key)? {
                None => Ok(None),
                Some(i) => {
                    let (v, c) = self.accessor(i)?;
                    if v.len() != n * c {
                        return Err(format!("{key} has {} elements but POSITION has {n}", v.len() / c.max(1)));
                    }
                    Ok(Some((v, c)))
                }
            }
        };
        let normals = match per_vertex("NORMAL")? {
            Some((v, 3)) => Some(
                v.as_chunks::<3>().0.iter()
                    .map(|c| (normal_m * DVec3::new(c[0], c[1], c[2])).normalize_or_zero())
                    .collect(),
            ),
            Some(_) => return Err("NORMAL must be VEC3".into()),
            None => None,
        };
        let uvs = match per_vertex("TEXCOORD_0")? {
            Some((v, 2)) => Some(v.as_chunks::<2>().0.iter().map(|c| DVec3::new(c[0], 1.0 - c[1], 0.0)).collect()),
            Some(_) => return Err("TEXCOORD_0 must be VEC2".into()),
            None => None,
        };
        let colors = match per_vertex("COLOR_0")? {
            Some((v, c @ (3 | 4))) => Some(v.chunks_exact(c).map(|c| DVec3::new(c[0], c[1], c[2])).collect()),
            Some(_) => return Err("COLOR_0 must be VEC3 or VEC4".into()),
            None => None,
        };
        acc.push(normals, uvs, colors);

        let indices: Vec<u32> = match uint(prim, "indices")? {
            Some(i) => {
                let (v, c) = self.accessor(i)?;
                if c != 1 {
                    return Err("indices accessor must be SCALAR".into());
                }
                v.into_iter()
                    .map(|x| {
                        let x = x as usize;
                        if x < n { Ok(x as u32) } else { Err(format!("index {x} is out of range ({n} vertices)")) }
                    })
                    .collect::<R<_>>()?
            }
            None => (0..n as u32).collect(),
        };
        let mode = prim.get("mode").and_then(Value::as_u64).unwrap_or(4);
        let mut tri = |a: u32, b: u32, c: u32, skip_degenerate: bool| {
            if skip_degenerate && (a == b || b == c || a == c) {
                return;
            }
            let (b, c) = if flip { (c, b) } else { (b, c) };
            geo.prims.push(Prim::polygon(vec![base_u32 + a, base_u32 + b, base_u32 + c]));
        };
        let k = indices.len();
        match mode {
            0 => {} // points: the vertices alone
            4 => {
                if !k.is_multiple_of(3) {
                    return Err(format!("{k} triangle indices is not a multiple of 3"));
                }
                for t in indices.as_chunks::<3>().0 {
                    tri(t[0], t[1], t[2], false);
                }
            }
            5 => {
                for i in 0..k.saturating_sub(2) {
                    let (a, b, c) = if i.is_multiple_of(2) {
                        (indices[i], indices[i + 1], indices[i + 2])
                    } else {
                        (indices[i], indices[i + 2], indices[i + 1])
                    };
                    tri(a, b, c, true);
                }
            }
            6 => {
                for i in 1..k.saturating_sub(1) {
                    tri(indices[i], indices[i + 1], indices[0], true);
                }
            }
            1 => {
                if !k.is_multiple_of(2) {
                    return Err(format!("{k} line indices is not a multiple of 2"));
                }
                for l in indices.as_chunks::<2>().0 {
                    geo.prims.push(Prim::polyline(vec![base_u32 + l[0], base_u32 + l[1]]));
                }
            }
            2 | 3 if k >= 2 => {
                let mut v: Vec<u32> = indices.iter().map(|i| base_u32 + i).collect();
                if mode == 2 {
                    v.push(v[0]);
                }
                geo.prims.push(Prim::polyline(v));
            }
            2 | 3 => {}
            other => return Err(format!("unknown primitive mode {other}")),
        }
        Ok(())
    }
}

/// Per-point optional attributes gathered across primitives. One that some
/// primitive lacks is dropped rather than padded with made-up values.
#[derive(Default)]
struct Attrs {
    n: Option<Vec<DVec3>>,
    uv: Option<Vec<DVec3>>,
    cd: Option<Vec<DVec3>>,
    started: bool,
}

impl Attrs {
    fn push(&mut self, n: Option<Vec<DVec3>>, uv: Option<Vec<DVec3>>, cd: Option<Vec<DVec3>>) {
        let first = !self.started;
        self.started = true;
        let join = |slot: &mut Option<Vec<DVec3>>, v: Option<Vec<DVec3>>| match (first, v) {
            (true, v) => *slot = v,
            (false, Some(v)) => {
                if let Some(s) = slot {
                    s.extend(v);
                }
            }
            (false, None) => *slot = None,
        };
        join(&mut self.n, n);
        join(&mut self.uv, uv);
        join(&mut self.cd, cd);
    }

    fn finish(self, geo: &mut Geometry) {
        for (name, v) in [("N", self.n), ("uv", self.uv), ("Cd", self.cd)] {
            if let Some(v) = v.filter(|v| !v.is_empty() && v.len() == geo.points.len()) {
                geo.point_attribs.set(name, AttribArray::Vec3(v));
            }
        }
    }
}

/// Load a buffer URI: a base64 `data:` URI or a relative file path.
fn load_uri(uri: &str, base_dir: Option<&Path>) -> R<Vec<u8>> {
    if let Some(rest) = uri.strip_prefix("data:") {
        let (_, payload) = rest
            .split_once(";base64,")
            .ok_or("only base64 data URIs are supported")?;
        return base64_decode(payload);
    }
    if uri.contains("://") || uri.starts_with('/') || uri.starts_with('\\') {
        return Err(format!("'{uri}' is not a relative file path; only files next to the .gltf are read"));
    }
    let rel = percent_decode(uri)?;
    let rel_path = Path::new(&rel);
    if rel_path
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!("'{rel}' leaves the folder of the .gltf; only files next to it are read"));
    }
    let dir = base_dir.ok_or_else(|| {
        format!("external buffer '{rel}' needs the file's folder; read the .gltf from a path, or use a .glb")
    })?;
    let path = dir.join(rel_path);
    std::fs::read(&path).map_err(|e| format!("cannot read external buffer '{}': {e}", path.display()))
}

fn percent_decode(s: &str) -> R<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| format!("bad percent escape in URI '{s}'"))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| format!("URI '{s}' is not UTF-8"))
}

fn base64_decode(s: &str) -> R<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        if c.is_ascii_whitespace() {
            continue;
        }
        acc = (acc << 6) | val(c).ok_or("invalid base64 in data URI")?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{read_bytes, Format, ReadOptions};
    use serde_json::json;

    /// A unit cube (1 m) with one node translated by (2, 3, 4) m and
    /// scaled by 0.5, as GLB. 8 shared vertices, 12 indexed triangles, colours.
    pub(crate) fn cube_glb(indexed: bool) -> Vec<u8> {
        let p: [[f32; 3]; 8] = [
            [0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.],
            [0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.],
        ];
        // Outward-facing (counter-clockwise from outside) quads, split in two.
        let quads: [[u16; 4]; 6] = [
            [0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4],
            [2, 3, 7, 6], [1, 2, 6, 5], [0, 4, 7, 3],
        ];
        let mut idx: Vec<u16> = Vec::new();
        for q in quads {
            idx.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
        let (verts, idx): (Vec<[f32; 3]>, Option<Vec<u16>>) = if indexed {
            (p.to_vec(), Some(idx))
        } else {
            (idx.iter().map(|&i| p[i as usize]).collect(), None)
        };
        let mut bin = Vec::new();
        for v in &verts {
            for c in v {
                bin.extend(c.to_le_bytes());
            }
        }
        let pos_len = bin.len();
        // COLOR_0 as normalised u8 RGBA, red channel = vertex x.
        let col_off = bin.len();
        for v in &verts {
            bin.extend([(v[0] * 255.0) as u8, 0, 0, 255]);
        }
        let col_len = bin.len() - col_off;
        let idx_off = bin.len();
        if let Some(idx) = &idx {
            for i in idx {
                bin.extend(i.to_le_bytes());
            }
        }
        let idx_len = bin.len() - idx_off;
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let mut prim = json!({"attributes": {"POSITION": 0, "COLOR_0": 1}, "mode": 4});
        let mut accessors = vec![
            json!({"bufferView": 0, "componentType": 5126, "count": verts.len(), "type": "VEC3",
                   "min": [0,0,0], "max": [1,1,1]}),
            json!({"bufferView": 1, "componentType": 5121, "normalized": true, "count": verts.len(), "type": "VEC4"}),
        ];
        let mut views = vec![
            json!({"buffer": 0, "byteOffset": 0, "byteLength": pos_len}),
            json!({"buffer": 0, "byteOffset": col_off, "byteLength": col_len}),
        ];
        if idx.is_some() {
            prim["indices"] = json!(2);
            accessors.push(json!({"bufferView": 2, "componentType": 5123, "count": 36, "type": "SCALAR"}));
            views.push(json!({"buffer": 0, "byteOffset": idx_off, "byteLength": idx_len}));
        }
        let doc = json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"children": [1], "translation": [2.0, 3.0, 4.0]},
                      {"mesh": 0, "scale": [0.5, 0.5, 0.5]}],
            "meshes": [{"primitives": [prim]}],
            "accessors": accessors,
            "bufferViews": views,
            "buffers": [{"byteLength": bin.len()}],
        });
        glb(&doc, &bin)
    }

    pub(crate) fn glb(doc: &Value, bin: &[u8]) -> Vec<u8> {
        let mut json = serde_json::to_vec(doc).unwrap();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = Vec::new();
        out.extend(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend((total as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(CHUNK_JSON.to_le_bytes());
        out.extend(&json);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(CHUNK_BIN.to_le_bytes());
        out.extend(bin);
        out
    }

    fn opts(axes: GltfAxes) -> ReadOptions {
        ReadOptions { gltf_axes: axes, ..Default::default() }
    }

    #[test]
    fn glb_cube_with_node_transform_reads_in_mm_z_up() {
        let g = read_bytes(Format::Gltf, &cube_glb(true), &ReadOptions::default()).unwrap();
        assert_eq!(g.point_count(), 8);
        assert_eq!(g.prim_count(), 12);
        // Y-up metres: x 2..2.5, y 3..3.5, z 4..4.5. Z-up mm: (x, -z, y).
        let b = g.bounds();
        assert!((b.min - DVec3::new(2000.0, -4500.0, 3000.0)).abs().max_element() < 1e-3, "{b:?}");
        assert!((b.max - DVec3::new(2500.0, -4000.0, 3500.0)).abs().max_element() < 1e-3, "{b:?}");
        assert!((g.surface_area() - 6.0 * 500.0 * 500.0).abs() < 1e-3);
        // Rotation keeps outward winding: the face with the highest Z points +Z.
        let top = (0..g.prim_count())
            .max_by(|&a, &b| {
                let c = |p: usize| g.prims[p].vertices.iter().map(|&v| g.points[v as usize].z).sum::<f64>();
                c(a).total_cmp(&c(b))
            })
            .unwrap();
        assert!(g.prim_normal(top).z > 0.99);
        assert_eq!(g.detail_attribs.strs("gltf_axes").unwrap(), ["z_up"]);
        assert_eq!(g.detail_attribs.strs("gltf_source_frame").unwrap(), [SOURCE_FRAME]);
        // COLOR_0 red = x within the cube (0 or 1).
        let cd = match g.point_attribs.get("Cd") {
            Some(AttribArray::Vec3(v)) => v.clone(),
            other => panic!("Cd missing: {other:?}"),
        };
        for (p, c) in g.points.iter().zip(&cd) {
            let expect = if p.x > 2250.0 { 1.0 } else { 0.0 };
            assert!((c.x - expect).abs() < 1e-9 && c.y == 0.0);
        }
        assert!(crate::gltf_frame_note(&g).unwrap().contains("Z up"));
    }

    #[test]
    fn as_stored_keeps_y_up_and_non_indexed_matches_indexed() {
        let g = read_bytes(Format::Gltf, &cube_glb(false), &opts(GltfAxes::AsStored)).unwrap();
        assert_eq!(g.point_count(), 36);
        assert_eq!(g.prim_count(), 12);
        let b = g.bounds();
        assert!((b.min - DVec3::new(2000.0, 3000.0, 4000.0)).abs().max_element() < 1e-3);
        assert!((b.max - DVec3::new(2500.0, 3500.0, 4500.0)).abs().max_element() < 1e-3);
        assert!((g.surface_area() - 1.5e6).abs() < 1e-3);
        assert_eq!(g.detail_attribs.strs("gltf_axes").unwrap(), ["as_stored_y_up"]);
        // Unit option does not apply: glTF is always metres.
        let in_inch = ReadOptions { unit: crate::FileUnit::Inch, ..Default::default() };
        let g2 = read_bytes(Format::Gltf, &cube_glb(true), &in_inch).unwrap();
        assert!((g2.bounds().size().x - 500.0).abs() < 1e-3);
    }

    #[test]
    fn sniffs_glb_and_gltf_json() {
        assert_eq!(Format::sniff(&cube_glb(true)), Some(Format::Gltf));
        assert_eq!(Format::sniff(br#" {"asset":{"version":"2.0"}}"#), Some(Format::Gltf));
        assert_eq!(Format::sniff(br#"{"other":1}"#), None);
        assert_eq!(Format::from_path(Path::new("a.GLB")), Some(Format::Gltf));
        assert_eq!(Format::from_path(Path::new("a.gltf")), Some(Format::Gltf));
        assert!(!Format::Gltf.can_write());
        // Content wins over a wrong extension for glTF, and only for glTF.
        assert_eq!(Format::Stl.refine(&cube_glb(true)), Format::Gltf);
        assert_eq!(Format::Gltf.refine(b"LASF...."), Format::Las);
        assert_eq!(Format::Obj.refine(b"v 0 0 0"), Format::Obj);
    }

    /// The GLB cube re-expressed as `.gltf` JSON with the binary in a
    /// separate file and, separately, as a base64 data URI.
    fn split(glb_bytes: &[u8]) -> (Value, Vec<u8>) {
        let (json, bin) = split_glb(glb_bytes).unwrap();
        (serde_json::from_slice(json).unwrap(), bin.unwrap().to_vec())
    }

    #[test]
    fn gltf_with_external_and_embedded_buffers() {
        let (mut doc, bin) = split(&cube_glb(true));
        let dir = crate::test_util::temp_dir("gltf-ext");
        std::fs::write(dir.join("cube data.bin"), &bin).unwrap();
        doc["buffers"][0]["uri"] = json!("cube%20data.bin");
        let path = dir.join("cube.gltf");
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
        let g = crate::read(&path, &ReadOptions::default()).unwrap();
        assert_eq!((g.point_count(), g.prim_count()), (8, 12));
        // In memory without a folder, the external buffer cannot be found.
        let e = read_bytes(Format::Gltf, &std::fs::read(&path).unwrap(), &ReadOptions::default()).unwrap_err();
        assert!(e.to_string().contains("needs the file's folder"), "{e}");
        // In memory with the folder given, it can.
        let g = crate::read_bytes_in(Format::Gltf, &std::fs::read(&path).unwrap(), &ReadOptions::default(), &dir).unwrap();
        assert_eq!(g.point_count(), 8);
        // Paths that leave the folder are refused.
        doc["buffers"][0]["uri"] = json!("../cube.bin");
        let e = crate::read_bytes_in(Format::Gltf, &serde_json::to_vec(&doc).unwrap(), &ReadOptions::default(), &dir)
            .unwrap_err();
        assert!(e.to_string().contains("leaves the folder"), "{e}");
        std::fs::remove_dir_all(&dir).ok();

        // Embedded base64.
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut b64 = String::new();
        for c in bin.chunks(3) {
            let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
            for k in 0..=c.len() {
                b64.push(A[((n >> (18 - 6 * k)) & 63) as usize] as char);
            }
        }
        while !b64.len().is_multiple_of(4) {
            b64.push('=');
        }
        doc["buffers"][0]["uri"] = json!(format!("data:application/octet-stream;base64,{b64}"));
        let g = read_bytes(Format::Gltf, &serde_json::to_vec(&doc).unwrap(), &ReadOptions::default()).unwrap();
        assert_eq!((g.point_count(), g.prim_count()), (8, 12));
        assert!(matches!(g.point_attribs.get("Cd"), Some(AttribArray::Vec3(_))));
    }

    #[test]
    fn compressed_primitives_are_refused() {
        let (mut doc, bin) = split(&cube_glb(true));
        doc["meshes"][0]["primitives"][0]["extensions"] =
            json!({"KHR_draco_mesh_compression": {"bufferView": 0, "attributes": {"POSITION": 0}}});
        let e = read_bytes(Format::Gltf, &glb(&doc, &bin), &ReadOptions::default()).unwrap_err();
        assert!(e.to_string().contains("Draco compression"), "{e}");

        let (mut doc, bin) = split(&cube_glb(true));
        doc["extensionsRequired"] = json!(["EXT_meshopt_compression"]);
        let e = read_bytes(Format::Gltf, &glb(&doc, &bin), &ReadOptions::default()).unwrap_err();
        assert!(e.to_string().contains("meshopt compression"), "{e}");

        let (mut doc, bin) = split(&cube_glb(true));
        doc["bufferViews"][0]["extensions"] = json!({"EXT_meshopt_compression": {}});
        let e = read_bytes(Format::Gltf, &glb(&doc, &bin), &ReadOptions::default()).unwrap_err();
        assert!(e.to_string().contains("meshopt compression"), "{e}");
    }

    #[test]
    fn damaged_files_error_instead_of_panicking() {
        let good = cube_glb(true);
        for cut in [4, 12, 20, 40, good.len() - 1] {
            assert!(read_bytes(Format::Gltf, &good[..cut], &ReadOptions::default()).is_err(), "cut {cut}");
        }
        let (mut doc, bin) = split(&good);
        doc["accessors"][0]["count"] = json!(1_000_000_000u64);
        assert!(read_bytes(Format::Gltf, &glb(&doc, &bin), &ReadOptions::default()).is_err());
        let (mut doc, bin) = split(&good);
        doc["nodes"][1]["children"] = json!([0]);
        let e = read_bytes(Format::Gltf, &glb(&doc, &bin), &ReadOptions::default()).unwrap_err();
        assert!(e.to_string().contains("cycle"), "{e}");
        let (mut doc, bin) = split(&good);
        doc["asset"]["version"] = json!("1.0");
        assert!(read_bytes(Format::Gltf, &glb(&doc, &bin), &ReadOptions::default()).is_err());
    }

    #[test]
    fn strips_fans_mirrors_and_normals() {
        // A unit square as a strip and as a fan, with +Z normals, under a
        // mirroring node (scale x by -1), kept Y-up.
        let p: [[f32; 3]; 4] = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [1., 1., 0.]];
        let mut bin = Vec::new();
        for v in p {
            for c in v {
                bin.extend(c.to_le_bytes());
            }
        }
        for _ in 0..4 {
            for c in [0f32, 0., 1.] {
                bin.extend(c.to_le_bytes());
            }
        }
        for i in [0u8, 1, 3, 2] {
            bin.push(i);
        }
        let doc = json!({
            "asset": {"version": "2.0"},
            "nodes": [{"mesh": 0, "scale": [-1.0, 1.0, 1.0]}],
            "meshes": [{"primitives": [
                {"attributes": {"POSITION": 0, "NORMAL": 1}, "mode": 5},
                {"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2, "mode": 6},
            ]}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC3"},
                {"bufferView": 2, "componentType": 5121, "count": 4, "type": "SCALAR"},
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 48},
                {"buffer": 0, "byteOffset": 48, "byteLength": 48},
                {"buffer": 0, "byteOffset": 96, "byteLength": 4},
            ],
            "buffers": [{"byteLength": 100}],
        });
        let g = read_bytes(Format::Gltf, &glb(&doc, &bin), &opts(GltfAxes::AsStored)).unwrap();
        assert_eq!(g.point_count(), 8);
        assert_eq!(g.prim_count(), 4);
        assert!((g.surface_area() - 2.0e6).abs() < 1e-3);
        // Mirroring keeps faces aligned with their normals: winding is flipped
        // and the normal transform keeps +Z.
        let Some(AttribArray::Vec3(n)) = g.point_attribs.get("N") else { panic!("N missing") };
        for (i, prim) in g.prims.iter().enumerate() {
            let face = g.prim_normal(i);
            assert!(face.dot(n[prim.vertices[0] as usize]) > 0.99, "prim {i}: {face:?}");
        }
    }
}
