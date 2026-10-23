//! Track model (`track.v1`): a chain of intervals, each with a length and a signed
//! curvature (left positive), open or closed. Builders: hand-entered segment list,
//! polyline or GPS path. Edits: reverse, mirror, scale. See spec sections 2.2 and 3.7.

use serde_json::{json, Value};

/// Kind of a track segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegKind {
    Straight,
    Left,
    Right,
}

impl SegKind {
    pub fn name(self) -> &'static str {
        match self {
            SegKind::Straight => "straight",
            SegKind::Left => "left",
            SegKind::Right => "right",
        }
    }
}

/// A hand-entered segment: type, length and radius (radius ignored for straights).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegSpec {
    pub kind: SegKind,
    pub length_m: f64,
    pub radius_m: f64,
}

/// A segment found in a built track, with the interval range it covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub kind: SegKind,
    pub length_m: f64,
    /// Mean radius; infinite for straights.
    pub radius_m: f64,
    pub i0: usize,
    pub i1: usize,
    /// Signed turning angle, rad (left positive).
    pub angle_rad: f64,
}

/// A track as `m` intervals. Interval `i` joins node `i` and node `i + 1` (wrapping to
/// node 0 when closed) and carries curvature `kappa[i]` over length `ds[i]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub name: String,
    pub closed: bool,
    pub ds: Vec<f64>,
    pub kappa: Vec<f64>,
}

impl Track {
    pub fn new(closed: bool, ds: Vec<f64>, kappa: Vec<f64>) -> Result<Self, String> {
        if ds.len() != kappa.len() {
            return Err(format!("track: {} lengths but {} curvatures", ds.len(), kappa.len()));
        }
        let min = if closed { 3 } else { 1 };
        if ds.len() < min {
            return Err(format!("track: needs at least {min} intervals"));
        }
        if ds.iter().any(|d| !(d.is_finite() && *d > 0.0)) {
            return Err("track: every interval length must be positive".into());
        }
        if kappa.iter().any(|k| !k.is_finite()) {
            return Err("track: curvature is not finite".into());
        }
        Ok(Self { name: String::new(), closed, ds, kappa })
    }

    /// A circle of radius `r` split into `n` equal intervals (left turn), closed.
    pub fn circle(r: f64, n: usize) -> Result<Self, String> {
        let d = 2.0 * std::f64::consts::PI * r / n as f64;
        Self::new(true, vec![d; n], vec![1.0 / r; n])
    }

    /// A straight open track.
    pub fn straight(length: f64, ds: f64) -> Result<Self, String> {
        let n = (length / ds).ceil().max(1.0) as usize;
        Self::new(false, vec![length / n as f64; n], vec![0.0; n])
    }

    pub fn intervals(&self) -> usize {
        self.ds.len()
    }

    pub fn nodes(&self) -> usize {
        if self.closed {
            self.ds.len()
        } else {
            self.ds.len() + 1
        }
    }

    pub fn length(&self) -> f64 {
        self.ds.iter().sum()
    }

    /// Arc length at each node (starting at 0).
    pub fn s_nodes(&self) -> Vec<f64> {
        let mut s = Vec::with_capacity(self.nodes());
        let mut acc = 0.0;
        for i in 0..self.nodes() {
            s.push(acc);
            if i < self.ds.len() {
                acc += self.ds[i];
            }
        }
        s
    }

    /// Build from a segment list; each segment is cut into intervals no longer than `ds_target`.
    pub fn from_segments(segs: &[SegSpec], closed: bool, ds_target: f64) -> Result<Self, String> {
        if !(ds_target.is_finite() && ds_target > 0.0) {
            return Err("track: ds must be positive".into());
        }
        let mut ds = Vec::new();
        let mut kappa = Vec::new();
        for (i, s) in segs.iter().enumerate() {
            if !(s.length_m.is_finite() && s.length_m > 0.0) {
                return Err(format!("segment {i}: length must be positive"));
            }
            let k = match s.kind {
                SegKind::Straight => 0.0,
                SegKind::Left | SegKind::Right => {
                    if !(s.radius_m.is_finite() && s.radius_m > 0.0) {
                        return Err(format!("segment {i}: corner radius must be positive"));
                    }
                    let k = 1.0 / s.radius_m;
                    if s.kind == SegKind::Left {
                        k
                    } else {
                        -k
                    }
                }
            };
            let count = (s.length_m / ds_target).ceil().max(1.0) as usize;
            for _ in 0..count {
                ds.push(s.length_m / count as f64);
                kappa.push(k);
            }
        }
        Self::new(closed, ds, kappa)
    }

    /// Split every interval longer than `ds_max` into equal parts with the same curvature.
    pub fn subdivided(&self, ds_max: f64) -> Track {
        if !(ds_max.is_finite() && ds_max > 0.0) || self.ds.iter().all(|d| *d <= ds_max * (1.0 + 1e-12)) {
            return self.clone();
        }
        let mut ds = Vec::new();
        let mut kappa = Vec::new();
        for (d, k) in self.ds.iter().zip(&self.kappa) {
            let c = (d / ds_max).ceil().max(1.0) as usize;
            for _ in 0..c {
                ds.push(d / c as f64);
                kappa.push(*k);
            }
        }
        Track { name: self.name.clone(), closed: self.closed, ds, kappa }
    }

    /// Travel the same path the other way: intervals reversed, turns swap sides.
    pub fn reversed(&self) -> Track {
        Track {
            name: self.name.clone(),
            closed: self.closed,
            ds: self.ds.iter().rev().copied().collect(),
            kappa: self.kappa.iter().rev().map(|k| -k).collect(),
        }
    }

    /// Mirror image: left and right swap.
    pub fn mirrored(&self) -> Track {
        Track {
            name: self.name.clone(),
            closed: self.closed,
            ds: self.ds.clone(),
            kappa: self.kappa.iter().map(|k| -k).collect(),
        }
    }

    /// Uniform scale: lengths times `f`, curvature divided by `f`.
    pub fn scaled(&self, f: f64) -> Track {
        Track {
            name: self.name.clone(),
            closed: self.closed,
            ds: self.ds.iter().map(|d| d * f).collect(),
            kappa: self.kappa.iter().map(|k| k / f).collect(),
        }
    }

    /// Rotate a closed track so interval `k` becomes the first.
    pub fn rotated(&self, k: usize) -> Track {
        let m = self.ds.len();
        let k = k % m;
        let mut t = self.clone();
        t.ds.rotate_left(k);
        t.kappa.rotate_left(k);
        t
    }

    /// Node positions in metres from integrating the curvature (start at the origin,
    /// heading +X). A closed track built from a consistent path returns to the start.
    pub fn path_xy(&self) -> Vec<[f64; 2]> {
        let mut out = Vec::with_capacity(self.nodes());
        let (mut x, mut y, mut psi) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..self.nodes() {
            out.push([x, y]);
            if i < self.ds.len() {
                let mid = psi + 0.5 * self.kappa[i] * self.ds[i];
                x += self.ds[i] * mid.cos();
                y += self.ds[i] * mid.sin();
                psi += self.kappa[i] * self.ds[i];
            }
        }
        out
    }

    /// Closure of the integrated path: `(end-to-start distance in m, heading mismatch in rad)`.
    /// For a closed loop the total turning should be a multiple of a full turn.
    pub fn closure(&self) -> (f64, f64) {
        let (mut x, mut y, mut psi) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..self.ds.len() {
            let mid = psi + 0.5 * self.kappa[i] * self.ds[i];
            x += self.ds[i] * mid.cos();
            y += self.ds[i] * mid.sin();
            psi += self.kappa[i] * self.ds[i];
        }
        let tau = 2.0 * std::f64::consts::PI;
        let mut h = psi.rem_euclid(tau);
        if h > std::f64::consts::PI {
            h -= tau;
        }
        ((x * x + y * y).sqrt(), h)
    }

    /// Straight/left/right segments with hysteresis; pieces shorter than `min_seg_m`
    /// are merged into the longer neighbour.
    pub fn segment_list(&self, kappa_straight: f64, min_seg_m: f64) -> Vec<Segment> {
        let ks = kappa_straight.max(1e-9);
        let hyst = 0.2;
        let m = self.ds.len();
        let mut class = Vec::with_capacity(m);
        let mut state = SegKind::Straight;
        for &k in &self.kappa {
            let a = k.abs();
            let dir = if k > 0.0 { SegKind::Left } else { SegKind::Right };
            match state {
                SegKind::Straight => {
                    if a > ks * (1.0 + hyst) || (class.is_empty() && a > ks) {
                        state = dir;
                    }
                }
                _ => {
                    if a < ks * (1.0 - hyst) {
                        state = SegKind::Straight;
                    } else if state != dir && a > ks * (1.0 + hyst) {
                        state = dir;
                    }
                }
            }
            class.push(state);
        }
        let mut segs: Vec<Segment> = Vec::new();
        let mut start = 0;
        for i in 1..=m {
            if i == m || class[i] != class[start] {
                segs.push(self.make_segment(class[start], start, i));
                start = i;
            }
        }
        // merge short pieces into the longer neighbour
        loop {
            if segs.len() < 2 {
                break;
            }
            let (idx, len) = segs
                .iter()
                .enumerate()
                .map(|(i, s)| (i, s.length_m))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .expect("non-empty");
            if len >= min_seg_m {
                break;
            }
            let into = if idx == 0 {
                1
            } else if idx == segs.len() - 1 || segs[idx - 1].length_m >= segs[idx + 1].length_m {
                idx - 1
            } else {
                idx + 1
            };
            let (lo, hi) = (idx.min(into), idx.max(into));
            let kind = segs[into].kind;
            let merged = self.make_segment(kind, segs[lo].i0, segs[hi].i1);
            segs[lo] = merged;
            segs.remove(hi);
        }
        // fuse neighbours of the same kind
        let mut out: Vec<Segment> = Vec::new();
        for s in segs {
            match out.last_mut() {
                Some(last) if last.kind == s.kind => {
                    *last = self.make_segment(s.kind, last.i0, s.i1);
                }
                _ => out.push(s),
            }
        }
        out
    }

    fn make_segment(&self, kind: SegKind, i0: usize, i1: usize) -> Segment {
        let length: f64 = self.ds[i0..i1].iter().sum();
        let angle: f64 = (i0..i1).map(|i| self.kappa[i] * self.ds[i]).sum();
        let abs_angle: f64 = (i0..i1).map(|i| self.kappa[i].abs() * self.ds[i]).sum();
        let radius = if kind == SegKind::Straight || abs_angle < 1e-12 { f64::INFINITY } else { length / abs_angle };
        Segment { kind, length_m: length, radius_m: radius, i0, i1, angle_rad: angle }
    }

    pub fn to_json(&self, kappa_straight: f64, min_seg_m: f64) -> Value {
        let mut s = Vec::with_capacity(self.ds.len());
        let mut acc = 0.0;
        for d in &self.ds {
            s.push(acc);
            acc += d;
        }
        let segs: Vec<Value> = self
            .segment_list(kappa_straight, min_seg_m)
            .iter()
            .map(|g| {
                json!({
                    "type": g.kind.name(),
                    "length_m": g.length_m,
                    "radius_m": if g.radius_m.is_finite() { json!(g.radius_m) } else { Value::Null },
                    "angle_deg": g.angle_rad.to_degrees(),
                })
            })
            .collect();
        json!({
            "kind": "track.v1",
            "name": self.name,
            "closed": self.closed,
            "length_m": self.length(),
            "s": s,
            "ds": self.ds,
            "kappa": self.kappa,
            "segments": segs,
        })
    }

    pub fn from_json(v: &Value) -> Result<Self, String> {
        let arr = |k: &str| -> Result<Vec<f64>, String> {
            v.get(k)
                .and_then(Value::as_array)
                .ok_or_else(|| format!("track: missing array '{k}'"))?
                .iter()
                .map(|x| x.as_f64().ok_or_else(|| format!("track: '{k}' holds a non-number")))
                .collect()
        };
        let closed = v.get("closed").and_then(Value::as_bool).unwrap_or(false);
        let mut t = Track::new(closed, arr("ds")?, arr("kappa")?)?;
        if let Some(n) = v.get("name").and_then(Value::as_str) {
            t.name = n.to_string();
        }
        Ok(t)
    }
}

/// Parse a hand-entered segment list. Accepts `[type, length, radius]` rows with the
/// type coded 0 straight, 1 left, 2 right, or objects with `type` (name or code),
/// `length_m` or `angle_deg` (corners), and `radius_m`.
pub fn parse_segments(v: &Value) -> Result<Vec<SegSpec>, String> {
    let list = v
        .get("segments")
        .and_then(Value::as_array)
        .or_else(|| v.as_array())
        .ok_or("segments: expected a list")?;
    let kind_of = |x: &Value| -> Result<SegKind, String> {
        if let Some(c) = x.as_f64() {
            return match c.round() as i64 {
                0 => Ok(SegKind::Straight),
                1 => Ok(SegKind::Left),
                2 => Ok(SegKind::Right),
                _ => Err(format!("segment type code {c} is not 0, 1 or 2")),
            };
        }
        match x.as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("straight") | Some("s") => Ok(SegKind::Straight),
            Some("left") | Some("l") => Ok(SegKind::Left),
            Some("right") | Some("r") => Ok(SegKind::Right),
            _ => Err(format!("unknown segment type {x}")),
        }
    };
    let mut out = Vec::new();
    for (i, row) in list.iter().enumerate() {
        let spec = if let Some(a) = row.as_array() {
            if a.len() < 2 {
                return Err(format!("segment {i}: needs [type, length_m, radius_m]"));
            }
            let kind = kind_of(&a[0])?;
            let length = a[1].as_f64().ok_or_else(|| format!("segment {i}: length is not a number"))?;
            let radius = a.get(2).and_then(Value::as_f64).unwrap_or(0.0);
            SegSpec { kind, length_m: length, radius_m: radius }
        } else if row.is_object() {
            let kind = kind_of(row.get("type").ok_or_else(|| format!("segment {i}: missing type"))?)?;
            let radius = row.get("radius_m").and_then(Value::as_f64).unwrap_or(0.0);
            let length = match (row.get("length_m").and_then(Value::as_f64), row.get("angle_deg").and_then(Value::as_f64)) {
                (Some(l), _) => l,
                (None, Some(a)) if kind != SegKind::Straight => radius * a.abs().to_radians(),
                _ => return Err(format!("segment {i}: needs length_m (or angle_deg for a corner)")),
            };
            SegSpec { kind, length_m: length, radius_m: radius }
        } else {
            return Err(format!("segment {i}: expected a row or an object"));
        };
        out.push(spec);
    }
    if out.is_empty() {
        return Err("segments: the list is empty".into());
    }
    Ok(out)
}

/// How to treat the ends of a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosedMode {
    Auto,
    Closed,
    Open,
}

/// Options of [`track_from_path`].
#[derive(Debug, Clone, Copy)]
pub struct PathOpts {
    pub ds_m: f64,
    pub smooth_window_m: f64,
    pub curvature_gain: f64,
    pub closed: ClosedMode,
}

impl Default for PathOpts {
    fn default() -> Self {
        Self { ds_m: 1.0, smooth_window_m: 15.0, curvature_gain: 1.0, closed: ClosedMode::Auto }
    }
}

/// Build a track from a planar path given in metres. `hint_closed` (a closed polyline
/// primitive) is used by `Auto`.
pub fn track_from_path(pts: &[[f64; 2]], hint_closed: bool, o: &PathOpts) -> Result<Track, String> {
    if !(o.ds_m.is_finite() && o.ds_m > 0.0) {
        return Err("path: ds must be positive".into());
    }
    let mut p: Vec<[f64; 2]> = Vec::with_capacity(pts.len());
    for q in pts {
        if !(q[0].is_finite() && q[1].is_finite()) {
            return Err("path: a point is not finite".into());
        }
        if p.last().map(|l| dist(*l, *q) > 1e-9).unwrap_or(true) {
            p.push(*q);
        }
    }
    if p.len() < 3 {
        return Err("path: needs at least three distinct points".into());
    }
    let first_last = dist(p[0], p[p.len() - 1]);
    let mut closed = match o.closed {
        ClosedMode::Closed => true,
        ClosedMode::Open => false,
        ClosedMode::Auto => {
            hint_closed || (first_last < 2.0 * o.ds_m && heading_diff_deg(&p) < 20.0 && path_length(&p, false) > 8.0 * o.ds_m)
        }
    };
    if closed && first_last < 1e-6 * path_length(&p, false).max(1.0) {
        p.pop();
    }
    if p.len() < 3 {
        closed = false;
    }
    let total = path_length(&p, closed);
    if total <= 0.0 {
        return Err("path: zero length".into());
    }
    // uniform resampling
    let n_int = if closed { (total / o.ds_m).round().max(8.0) as usize } else { (total / o.ds_m).ceil().max(2.0) as usize };
    let ds = total / n_int as f64;
    let n_nodes = if closed { n_int } else { n_int + 1 };
    let mut cum = vec![0.0];
    let segs = if closed { p.len() } else { p.len() - 1 };
    for i in 0..segs {
        cum.push(cum[i] + dist(p[i], p[(i + 1) % p.len()]));
    }
    let mut pos: Vec<[f64; 2]> = Vec::with_capacity(n_nodes);
    let mut seg = 0;
    for k in 0..n_nodes {
        let s = (k as f64 * ds).min(total);
        while seg + 1 < segs && cum[seg + 1] < s {
            seg += 1;
        }
        let (a, b) = (p[seg], p[(seg + 1) % p.len()]);
        let l = (cum[seg + 1] - cum[seg]).max(1e-12);
        let f = ((s - cum[seg]) / l).clamp(0.0, 1.0);
        pos.push([a[0] + f * (b[0] - a[0]), a[1] + f * (b[1] - a[1])]);
    }
    // Menger curvature at each node
    let w1 = ((o.smooth_window_m / (4.0 * ds)).round() as usize).max(1);
    let w2 = (o.smooth_window_m / (4.0 * ds)).round() as usize;
    let mut kn = vec![f64::NAN; n_nodes];
    for (i, slot) in kn.iter_mut().enumerate() {
        let w = if closed { w1.min(n_nodes / 3).max(1) } else { w1.min(i).min(n_nodes - 1 - i) };
        if w == 0 {
            continue;
        }
        let (ia, ic) = if closed { ((i + n_nodes - w) % n_nodes, (i + w) % n_nodes) } else { (i - w, i + w) };
        *slot = menger(pos[ia], pos[i], pos[ic]);
    }
    if !closed {
        // fill the ends from the nearest valid node
        let first = kn.iter().position(|k| k.is_finite()).ok_or("path: too short for the curvature window")?;
        let last = kn.iter().rposition(|k| k.is_finite()).unwrap_or(first);
        for i in 0..first {
            kn[i] = kn[first];
        }
        for i in last + 1..n_nodes {
            kn[i] = kn[last];
        }
    }
    // moving average over the window
    let sm = if w2 == 0 {
        kn.clone()
    } else {
        (0..n_nodes)
            .map(|i| {
                let (mut acc, mut cnt) = (0.0, 0.0);
                for d in -(w2 as isize)..=(w2 as isize) {
                    let j = i as isize + d;
                    let j = if closed {
                        j.rem_euclid(n_nodes as isize) as usize
                    } else if j < 0 || j >= n_nodes as isize {
                        continue;
                    } else {
                        j as usize
                    };
                    acc += kn[j];
                    cnt += 1.0;
                }
                acc / cnt
            })
            .collect()
    };
    let kappa: Vec<f64> = (0..n_int).map(|i| 0.5 * (sm[i] + sm[(i + 1) % n_nodes]) * o.curvature_gain).collect();
    Track::new(closed, vec![ds; n_int], kappa)
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn path_length(p: &[[f64; 2]], closed: bool) -> f64 {
    let n = p.len();
    let segs = if closed { n } else { n - 1 };
    (0..segs).map(|i| dist(p[i], p[(i + 1) % n])).sum()
}

fn heading_diff_deg(p: &[[f64; 2]]) -> f64 {
    let n = p.len();
    let h0 = (p[1][1] - p[0][1]).atan2(p[1][0] - p[0][0]);
    let h1 = (p[n - 1][1] - p[n - 2][1]).atan2(p[n - 1][0] - p[n - 2][0]);
    let mut d = (h1 - h0).rem_euclid(2.0 * std::f64::consts::PI);
    if d > std::f64::consts::PI {
        d = 2.0 * std::f64::consts::PI - d;
    }
    d.to_degrees()
}

/// Signed Menger curvature of three points (left turn positive).
pub fn menger(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    let cross = (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]);
    let d = dist(a, b) * dist(b, c) * dist(c, a);
    if d < 1e-18 {
        0.0
    } else {
        2.0 * cross / d
    }
}

/// Project latitude and longitude (degrees) to a local plane in metres about the centroid.
pub fn project_gps(lat_lon: &[(f64, f64)]) -> Vec<[f64; 2]> {
    let n = lat_lon.len().max(1) as f64;
    let lat0 = lat_lon.iter().map(|p| p.0).sum::<f64>() / n;
    let lon0 = lat_lon.iter().map(|p| p.1).sum::<f64>() / n;
    let r = 6_371_000.0;
    lat_lon
        .iter()
        .map(|&(la, lo)| {
            [
                r * lat0.to_radians().cos() * (lo - lon0).to_radians(),
                r * (la - lat0).to_radians(),
            ]
        })
        .collect()
}

/// Read a path CSV with `lat`/`lon` (degrees) or `x`/`y` (metres) columns.
pub fn parse_path_csv(text: &str) -> Result<Vec<[f64; 2]>, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let header = lines.next().ok_or("path file is empty")?;
    let names: Vec<String> = header.split(',').map(|s| s.trim().to_ascii_lowercase()).collect();
    let find = |keys: &[&str]| names.iter().position(|n| keys.contains(&n.as_str()));
    let gps = find(&["lat", "latitude"]).zip(find(&["lon", "lng", "long", "longitude"]));
    let xy = find(&["x", "x_m"]).zip(find(&["y", "y_m"]));
    let (ca, cb, is_gps) = match (gps, xy) {
        (Some((a, b)), _) => (a, b, true),
        (None, Some((a, b))) => (a, b, false),
        _ => return Err("path file needs lat,lon or x,y columns".into()),
    };
    let mut vals = Vec::new();
    for (ln, l) in lines.enumerate() {
        let f: Vec<&str> = l.split(',').collect();
        let get = |c: usize| -> Result<f64, String> {
            f.get(c)
                .and_then(|s| s.trim().parse::<f64>().ok())
                .ok_or_else(|| format!("path file row {}: bad number", ln + 2))
        };
        vals.push((get(ca)?, get(cb)?));
    }
    Ok(if is_gps { project_gps(&vals) } else { vals.iter().map(|&(x, y)| [x, y]).collect() })
}
