//! Reader for the documented text and binary layouts of a sparse
//! structure-from-motion model (`cameras`, `images`, `points3D`), plus the
//! pinhole camera maths needed to project and triangulate markers.
//!
//! The parsers never default a bad number to zero: any malformed row is an
//! error naming the line, and truncated binary data is an error, not a panic.

use std::path::Path;

use glam::{DMat3, DQuat, DVec3};
use vernon_core::{AttribArray, Geometry};

use crate::util::set_metric_flag;

/// A camera with its model name and raw parameter list.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    pub id: u32,
    pub model: String,
    pub width: u64,
    pub height: u64,
    pub params: Vec<f64>,
}

/// A registered image: world-to-camera pose and file name.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub id: u32,
    /// Rotation quaternion in `(w, x, y, z)` order.
    pub q: [f64; 4],
    pub t: [f64; 3],
    pub camera_id: u32,
    pub name: String,
}

/// A sparse 3D point with its colour and mean reprojection error.
#[derive(Debug, Clone, PartialEq)]
pub struct Point3D {
    pub id: u64,
    pub xyz: [f64; 3],
    pub rgb: [u8; 3],
    pub error: f64,
    pub track_len: usize,
}

/// All three tables of a model.
#[derive(Debug, Clone, Default)]
pub struct Model {
    pub cameras: Vec<Camera>,
    pub images: Vec<Image>,
    pub points: Vec<Point3D>,
}

fn data_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.starts_with('#'))
}

fn parse<T: std::str::FromStr>(s: Option<&str>, line: usize, what: &str) -> Result<T, String> {
    s.ok_or_else(|| format!("line {line}: missing {what}"))?
        .parse::<T>()
        .map_err(|_| format!("line {line}: '{}' is not a valid {what}", s.unwrap_or("")))
}

fn finite(v: f64, line: usize, what: &str) -> Result<f64, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("line {line}: {what} is not finite"))
    }
}

fn take_field<'a>(remaining: &mut &'a str) -> Option<&'a str> {
    let text = remaining.trim_start();
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    let (field, tail) = text.split_at(end);
    *remaining = tail;
    (!field.is_empty()).then_some(field)
}

/// Parse `cameras.txt`.
pub fn parse_cameras_txt(text: &str) -> Result<Vec<Camera>, String> {
    let mut out = Vec::new();
    for (ln, l) in data_lines(text).filter(|(_, l)| !l.is_empty()) {
        let mut f = l.split_whitespace();
        let id = parse(f.next(), ln, "camera id")?;
        let model = f.next().ok_or_else(|| format!("line {ln}: missing camera model"))?.to_string();
        let width = parse(f.next(), ln, "width")?;
        let height = parse(f.next(), ln, "height")?;
        let params = f
            .map(|s| s.parse::<f64>().map_err(|_| format!("line {ln}: '{s}' is not a number")))
            .collect::<Result<Vec<_>, _>>()?;
        if params.iter().any(|p| !p.is_finite()) {
            return Err(format!("line {ln}: camera parameter is not finite"));
        }
        out.push(Camera {
            id,
            model,
            width,
            height,
            params,
        });
    }
    Ok(out)
}

/// Parse `images.txt`: two lines per image, the second (2D points) possibly empty.
pub fn parse_images_txt(text: &str) -> Result<Vec<Image>, String> {
    let mut out = Vec::new();
    let mut lines = data_lines(text);
    while let Some((ln, l)) = lines.next() {
        if l.is_empty() {
            continue;
        }
        let mut f = l;
        let id = parse(take_field(&mut f), ln, "image id")?;
        let mut q = [0.0; 4];
        for (k, name) in ["QW", "QX", "QY", "QZ"].iter().enumerate() {
            q[k] = finite(parse(take_field(&mut f), ln, name)?, ln, name)?;
        }
        let mut t = [0.0; 3];
        for (k, name) in ["TX", "TY", "TZ"].iter().enumerate() {
            t[k] = finite(parse(take_field(&mut f), ln, name)?, ln, name)?;
        }
        let camera_id = parse(take_field(&mut f), ln, "camera id")?;
        let name = f.trim_start();
        if name.is_empty() {
            return Err(format!("line {ln}: missing image name"));
        }
        // The points line belongs to this image even when it is empty.
        let (point_ln, points) = lines.next().ok_or_else(|| format!("line {ln}: missing image points line"))?;
        let mut points = points.split_whitespace();
        while let Some(x) = points.next() {
            finite(parse(Some(x), point_ln, "point X")?, point_ln, "point X")?;
            finite(parse(points.next(), point_ln, "point Y")?, point_ln, "point Y")?;
            match points.next() {
                Some("-1") => (),
                value => {
                    let _: u64 = parse(value, point_ln, "point id")?;
                }
            }
        }
        out.push(Image { id, q, t, camera_id, name: name.to_string() });
    }
    Ok(out)
}

/// Parse `points3D.txt`.
pub fn parse_points3d_txt(text: &str) -> Result<Vec<Point3D>, String> {
    let mut out = Vec::new();
    for (ln, l) in data_lines(text).filter(|(_, l)| !l.is_empty()) {
        let mut f = l.split_whitespace();
        let id = parse(f.next(), ln, "point id")?;
        let mut xyz = [0.0; 3];
        for (k, name) in ["X", "Y", "Z"].iter().enumerate() {
            xyz[k] = finite(parse(f.next(), ln, name)?, ln, name)?;
        }
        let mut rgb = [0u8; 3];
        for (k, name) in ["R", "G", "B"].iter().enumerate() {
            rgb[k] = parse(f.next(), ln, name)?;
        }
        let error = finite(parse(f.next(), ln, "error")?, ln, "error")?;
        let mut track_len = 0;
        while let Some(image_id) = f.next() {
            let _: u32 = parse(Some(image_id), ln, "track image id")?;
            let _: u32 = parse(f.next(), ln, "track point index")?;
            track_len += 1;
        }
        out.push(Point3D {
            id,
            xyz,
            rgb,
            error,
            track_len,
        });
    }
    Ok(out)
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|e| *e <= self.b.len());
        let end = end.ok_or_else(|| format!("unexpected end of data at byte {}", self.pos))?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| "short read")?))
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().map_err(|_| "short read")?))
    }
}

/// Parse `points3D.bin` (little endian: count, then per point id, xyz, rgb,
/// error and a track of image/point index pairs).
pub fn parse_points3d_bin(bytes: &[u8]) -> Result<Vec<Point3D>, String> {
    let mut c = Cursor { b: bytes, pos: 0 };
    let n = c.u64()?;
    if n > (bytes.len() as u64) / 43 {
        return Err(format!("point count {n} does not fit the file size"));
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let id = c.u64()?;
        let xyz = [c.f64()?, c.f64()?, c.f64()?];
        let rgb_b = c.take(3)?;
        let rgb = [rgb_b[0], rgb_b[1], rgb_b[2]];
        let error = c.f64()?;
        let track = c.u64()?;
        let skip = track.checked_mul(8).ok_or("track length overflow")?;
        c.take(usize::try_from(skip).map_err(|_| "track length overflow")?)?;
        if xyz.iter().any(|v| !v.is_finite()) {
            return Err(format!("point {id} has a non-finite coordinate"));
        }
        if !error.is_finite() {
            return Err(format!("point {id} has a non-finite reprojection error"));
        }
        out.push(Point3D {
            id,
            xyz,
            rgb,
            error,
            track_len: track as usize,
        });
    }
    Ok(out)
}

/// Read a model directory. Binary points take precedence over text points;
/// read or parse failures never fall back to another format. Cameras and
/// images need the text form, which may be exported by `model_converter`.
pub fn read_model_dir(dir: &Path, need_poses: bool) -> Result<Model, String> {
    let read = |name: &str| match std::fs::read_to_string(dir.join(name)) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", dir.join(name).display())),
    };
    let mut model = Model::default();
    if let Some(t) = read("cameras.txt")? {
        model.cameras = parse_cameras_txt(&t).map_err(|e| format!("cameras.txt: {e}"))?;
    }
    if let Some(t) = read("images.txt")? {
        model.images = parse_images_txt(&t).map_err(|e| format!("images.txt: {e}"))?;
    }
    if need_poses && (model.cameras.is_empty() || model.images.is_empty()) {
        return Err(format!(
            "{} has no cameras.txt / images.txt; export the model as text first (model_converter with output type TXT)",
            dir.display()
        ));
    }
    match std::fs::read(dir.join("points3D.bin")) {
        Ok(b) => {
            model.points = parse_points3d_bin(&b).map_err(|e| format!("points3D.bin: {e}"))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(t) = read("points3D.txt")? {
                model.points = parse_points3d_txt(&t).map_err(|e| format!("points3D.txt: {e}"))?;
            }
        }
        Err(e) => return Err(format!("{}: {e}", dir.join("points3D.bin").display())),
    }
    Ok(model)
}

/// Sparse points as a point cloud: colour in `Cd` (0..1), mean reprojection
/// error in `error`, track length in `track`, and the metric flag cleared
/// because monocular reconstruction has an arbitrary scale.
pub fn points_to_geometry(points: &[Point3D]) -> Geometry {
    let mut g = Geometry::from_points(points.iter().map(|p| DVec3::from_array(p.xyz)).collect());
    g.point_attribs.set(
        "Cd",
        AttribArray::Vec3(
            points
                .iter()
                .map(|p| DVec3::new(p.rgb[0] as f64, p.rgb[1] as f64, p.rgb[2] as f64) / 255.0)
                .collect(),
        ),
    );
    g.point_attribs
        .set("error", AttribArray::Float(points.iter().map(|p| p.error).collect()));
    g.point_attribs.set(
        "track",
        AttribArray::Int(points.iter().map(|p| p.track_len as i64).collect()),
    );
    set_metric_flag(&mut g, false);
    g
}

// ---------------------------------------------------------------------------
// Camera maths
// ---------------------------------------------------------------------------

/// Intrinsics of the supported camera models (radial and tangential terms).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Intrinsics {
    pub fx: f64,
    pub fy: f64,
    pub cx: f64,
    pub cy: f64,
    pub k1: f64,
    pub k2: f64,
    pub p1: f64,
    pub p2: f64,
}

impl Camera {
    /// Intrinsics for SIMPLE_PINHOLE, PINHOLE, SIMPLE_RADIAL, RADIAL, OPENCV.
    pub fn intrinsics(&self) -> Result<Intrinsics, String> {
        let p = &self.params;
        let need = |n: usize| {
            if p.len() == n {
                Ok(())
            } else {
                Err(format!("camera {} ({}) needs {n} parameters, has {}", self.id, self.model, p.len()))
            }
        };
        let z = Intrinsics {
            fx: 0.0,
            fy: 0.0,
            cx: 0.0,
            cy: 0.0,
            k1: 0.0,
            k2: 0.0,
            p1: 0.0,
            p2: 0.0,
        };
        match self.model.as_str() {
            "SIMPLE_PINHOLE" => {
                need(3)?;
                Ok(Intrinsics { fx: p[0], fy: p[0], cx: p[1], cy: p[2], ..z })
            }
            "PINHOLE" => {
                need(4)?;
                Ok(Intrinsics { fx: p[0], fy: p[1], cx: p[2], cy: p[3], ..z })
            }
            "SIMPLE_RADIAL" => {
                need(4)?;
                Ok(Intrinsics { fx: p[0], fy: p[0], cx: p[1], cy: p[2], k1: p[3], ..z })
            }
            "RADIAL" => {
                need(5)?;
                Ok(Intrinsics { fx: p[0], fy: p[0], cx: p[1], cy: p[2], k1: p[3], k2: p[4], ..z })
            }
            "OPENCV" => {
                need(8)?;
                Ok(Intrinsics { fx: p[0], fy: p[1], cx: p[2], cy: p[3], k1: p[4], k2: p[5], p1: p[6], p2: p[7] })
            }
            other => Err(format!(
                "camera model {other} is not supported for marker triangulation (use SIMPLE_PINHOLE, PINHOLE, SIMPLE_RADIAL, RADIAL or OPENCV)"
            )),
        }
    }
}

impl Intrinsics {
    fn delta(&self, x: f64, y: f64) -> (f64, f64) {
        let r2 = x * x + y * y;
        let rad = self.k1 * r2 + self.k2 * r2 * r2;
        (
            x * rad + 2.0 * self.p1 * x * y + self.p2 * (r2 + 2.0 * x * x),
            y * rad + self.p1 * (r2 + 2.0 * y * y) + 2.0 * self.p2 * x * y,
        )
    }

    /// Project a camera-space point to pixels (with distortion).
    pub fn project(&self, pc: DVec3) -> (f64, f64) {
        let (x, y) = (pc.x / pc.z, pc.y / pc.z);
        let (dx, dy) = self.delta(x, y);
        (self.fx * (x + dx) + self.cx, self.fy * (y + dy) + self.cy)
    }

    /// Pixel to undistorted normalised image coordinates.
    pub fn unproject(&self, u: f64, v: f64) -> (f64, f64) {
        let xd = (u - self.cx) / self.fx;
        let yd = (v - self.cy) / self.fy;
        let (mut x, mut y) = (xd, yd);
        for _ in 0..30 {
            let (dx, dy) = self.delta(x, y);
            x = xd - dx;
            y = yd - dy;
        }
        (x, y)
    }
}

impl Image {
    /// World-to-camera rotation.
    pub fn rotation(&self) -> DMat3 {
        DMat3::from_quat(DQuat::from_xyzw(self.q[1], self.q[2], self.q[3], self.q[0]).normalize())
    }

    pub fn translation(&self) -> DVec3 {
        DVec3::from_array(self.t)
    }

    /// Camera centre in world coordinates.
    pub fn centre(&self) -> DVec3 {
        -(self.rotation().transpose() * self.translation())
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    /// Write a text model with the given cameras and images, returning its dir.
    pub fn write_model(name: &str, cameras: &[Camera], images: &[Image]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("vernon-capture-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut c = String::from("# Camera list\n");
        for cam in cameras {
            let p: Vec<String> = cam.params.iter().map(|v| format!("{v:.17}")).collect();
            c += &format!("{} {} {} {} {}\n", cam.id, cam.model, cam.width, cam.height, p.join(" "));
        }
        let mut i = String::from("# Image list\n");
        for im in images {
            i += &format!(
                "{} {:.17} {:.17} {:.17} {:.17} {:.17} {:.17} {:.17} {} {}\n\n",
                im.id, im.q[0], im.q[1], im.q[2], im.q[3], im.t[0], im.t[1], im.t[2], im.camera_id, im.name
            );
        }
        std::fs::write(dir.join("cameras.txt"), c).unwrap();
        std::fs::write(dir.join("images.txt"), i).unwrap();
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAMERAS: &str = "# Camera list with one line of data per camera:\n\
        1 SIMPLE_RADIAL 4000 3000 3200.5 2000 1500 -0.02\n\
        2 PINHOLE 1920 1080 1500 1500 960 540\n";
    const IMAGES: &str = "# Image list\n\
        1 1 0 0 0 0 0 5 1 a.jpg\n\
        10.5 20.5 -1 30.5 40.5 7\n\
        2 0.9238795325 0 0.3826834324 0 1 0 5 1 b.jpg\n\
        \n\
        3 1 0 0 0 0 0 9 2 c.jpg\n\
        5 6 -1\n";
    const POINTS: &str = "# 3D point list\n\
        1 1.5 2.5 3.5 255 128 0 0.25 1 0 2 4\n\
        7 -1 0 1 0 0 255 1.5 3 9 4 2\n";

    #[test]
    fn spec_golden_21_three_images_two_cameras() {
        let cams = parse_cameras_txt(CAMERAS).unwrap();
        assert_eq!(cams.len(), 2);
        assert_eq!(cams[0].model, "SIMPLE_RADIAL");
        assert_eq!(cams[0].params.len(), 4);
        let imgs = parse_images_txt(IMAGES).unwrap();
        assert_eq!(imgs.len(), 3, "an empty points line must not break the pairing");
        assert_eq!(imgs[1].name, "b.jpg");
        assert_eq!(imgs[2].camera_id, 2);
        let pts = parse_points3d_txt(POINTS).unwrap();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].track_len, 2);
        assert_eq!(pts[1].rgb, [0, 0, 255]);
        let g = points_to_geometry(&pts);
        g.validate().unwrap();
        assert_eq!(g.point_count(), 2);
        assert_eq!(crate::util::metric_flag(&g), Some(false));
        assert!((g.point_attribs.vec3s("Cd").unwrap()[0].x - 1.0).abs() < 1e-12);
    }

    #[test]
    fn malformed_numbers_are_errors_not_zeros() {
        assert!(parse_points3d_txt("1 1.5 abc 3.5 255 128 0 0.25\n").is_err());
        assert!(parse_points3d_txt("1 1.5 2.5\n").is_err());
        assert!(parse_cameras_txt("1 PINHOLE 10 10 1 x 3 4\n").is_err());
        assert!(parse_images_txt("1 1 0 0 0 0 0 5 1\n").is_err());
        assert!(parse_points3d_txt("1 nan 2 3 0 0 0 0\n").is_err());
    }

    #[test]
    fn images_text_preserves_spaces_in_file_names() {
        let images = parse_images_txt("1 1 0 0 0 0 0 5 1 front  left panel.jpg\n\n").unwrap();
        assert_eq!(images[0].name, "front  left panel.jpg");
    }

    #[test]
    fn images_text_requires_the_second_line() {
        let err = parse_images_txt("# Images\n1 1 0 0 0 0 0 5 1 a.jpg\n").unwrap_err();
        assert!(err.contains("line 2") && err.contains("points"), "{err}");
    }

    #[test]
    fn images_text_rejects_a_missing_points_line_before_the_next_image() {
        let err = parse_images_txt("1 1 0 0 0 0 0 5 1 a.jpg\n2 1 0 0 0 0 0 5 1 b.jpg\n\n").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn images_text_validates_observation_triples() {
        for points in ["1", "1 2", "bad 2 3", "1 NaN 3", "1 2 bad", "1 2 -2"] {
            let err = parse_images_txt(&format!("1 1 0 0 0 0 0 5 1 a.jpg\n{points}\n")).unwrap_err();
            assert!(err.contains("line 2"), "points {points}: {err}");
        }
    }

    #[test]
    fn images_text_accepts_unmatched_and_large_point_ids() {
        let images = parse_images_txt("1 1 0 0 0 0 0 5 1 a.jpg\n1 2 -1 3 4 18446744073709551615\n").unwrap();
        assert_eq!(images.len(), 1);
    }

    #[test]
    fn points_text_requires_numeric_complete_track_pairs() {
        for track in ["1", "1 0 2", "bad 0", "1 bad", "1 -1", "4294967296 0"] {
            let text = format!("# Points\n1 1 2 3 255 128 0 0.25 {track}\n");
            let err = parse_points3d_txt(&text).unwrap_err();
            assert!(err.contains("line 2"), "track {track}: {err}");
        }
    }

    fn bin_fixture() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&2u64.to_le_bytes());
        for (id, xyz, rgb, track) in [(5u64, [1.0f64, 2.0, 3.0], [10u8, 20, 30], 2u64), (9, [-4.0, 0.5, 8.0], [255, 0, 0], 0)] {
            b.extend_from_slice(&id.to_le_bytes());
            for v in xyz {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&rgb);
            b.extend_from_slice(&0.5f64.to_le_bytes());
            b.extend_from_slice(&track.to_le_bytes());
            for k in 0..track {
                b.extend_from_slice(&(k as u32 + 1).to_le_bytes());
                b.extend_from_slice(&(k as u32).to_le_bytes());
            }
        }
        b
    }

    #[test]
    fn points_bin_round_trip_and_truncation() {
        let b = bin_fixture();
        let pts = parse_points3d_bin(&b).unwrap();
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].id, 5);
        assert_eq!(pts[0].track_len, 2);
        assert_eq!(pts[1].xyz, [-4.0, 0.5, 8.0]);
        for cut in 0..b.len() {
            assert!(parse_points3d_bin(&b[..cut]).is_err(), "cut at {cut}");
        }
        // A huge declared count cannot allocate.
        let mut bad = Vec::new();
        bad.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(parse_points3d_bin(&bad).is_err());
    }

    #[test]
    fn binary_points_reject_nonfinite_reprojection_error() {
        for error in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut bytes = bin_fixture();
            // Count (8), point id (8), xyz (24), rgb (3), then the error.
            bytes[43..51].copy_from_slice(&error.to_le_bytes());
            let err = parse_points3d_bin(&bytes).unwrap_err();
            assert!(err.contains("point 5") && err.contains("error"), "{err}");
        }
    }

    fn fresh_model_dir(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("vernon-capture-{name}-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn model_dir_prefers_binary_points_over_text() {
        let dir = fresh_model_dir("binary-point-precedence");
        std::fs::write(dir.join("points3D.txt"), POINTS).unwrap();
        std::fs::write(dir.join("points3D.bin"), bin_fixture()).unwrap();
        let model = read_model_dir(&dir, false).unwrap();
        assert_eq!(model.points[0].id, 5, "binary points must take precedence");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn model_dir_reports_unreadable_text_tables() {
        for name in ["cameras.txt", "images.txt", "points3D.txt"] {
            let dir = fresh_model_dir(&format!("invalid-utf8-{name}"));
            std::fs::write(dir.join(name), [0xff, 0xfe]).unwrap();
            let err = read_model_dir(&dir, false).unwrap_err();
            assert!(err.contains(name), "{err}");
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn model_dir_reports_binary_read_errors_without_text_fallback() {
        let dir = fresh_model_dir("unreadable-binary-points");
        std::fs::write(dir.join("points3D.txt"), POINTS).unwrap();
        std::fs::create_dir(dir.join("points3D.bin")).unwrap();
        let err = read_model_dir(&dir, false).unwrap_err();
        assert!(err.contains("points3D.bin"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn model_dir_reports_malformed_binary_without_text_fallback() {
        let dir = fresh_model_dir("malformed-binary-points");
        std::fs::write(dir.join("points3D.txt"), POINTS).unwrap();
        std::fs::write(dir.join("points3D.bin"), [0]).unwrap();
        let err = read_model_dir(&dir, false).unwrap_err();
        assert!(err.contains("points3D.bin"), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn model_dir_reads_text_and_binary_points() {
        let dir = std::env::temp_dir().join(format!("vernon-capture-modeldir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("points3D.bin"), bin_fixture()).unwrap();
        let m = read_model_dir(&dir, false).unwrap();
        assert_eq!(m.points.len(), 2);
        assert!(read_model_dir(&dir, true).is_err());
        std::fs::write(dir.join("cameras.txt"), CAMERAS).unwrap();
        std::fs::write(dir.join("images.txt"), IMAGES).unwrap();
        let m = read_model_dir(&dir, true).unwrap();
        assert_eq!((m.cameras.len(), m.images.len()), (2, 3));
    }

    #[test]
    fn projection_and_unprojection_are_inverse() {
        let cam = Camera {
            id: 1,
            model: "OPENCV".into(),
            width: 4000,
            height: 3000,
            params: vec![3000.0, 3010.0, 2000.0, 1500.0, -0.1, 0.02, 0.001, -0.002],
        };
        let k = cam.intrinsics().unwrap();
        let pc = DVec3::new(0.3, -0.2, 2.0);
        let (u, v) = k.project(pc);
        let (x, y) = k.unproject(u, v);
        assert!((x - pc.x / pc.z).abs() < 1e-10 && (y - pc.y / pc.z).abs() < 1e-10);
        let fisheye = Camera {
            model: "OPENCV_FISHEYE".into(),
            ..cam
        };
        assert!(fisheye.intrinsics().is_err());
    }

    #[test]
    fn image_pose_centre() {
        let imgs = parse_images_txt(IMAGES).unwrap();
        // Identity rotation, t = (0, 0, 5): centre at (0, 0, -5).
        assert!((imgs[0].centre() - DVec3::new(0.0, 0.0, -5.0)).length() < 1e-12);
    }
}
