//! Track nodes: build from a segment list, from a path or GPS log, and edit.

use glam::DVec3;
use vernon_core::{AttribArray, Geometry, Metric, Prim, Report, Severity};
use vernon_graph::{CookContext, CookError, CookResult, Data, DataKind, NodeTypeInfo, ParamSpec, Params, PortSpec, Stage};

use crate::track::{parse_path_csv, parse_segments, track_from_path, ClosedMode, PathOpts, SegKind, SegSpec, Track};
use crate::util::{json_in, read_text, FnNode, OrFail, SPEC};

fn node(id: &str, label: &str) -> NodeTypeInfo {
    NodeTypeInfo::new(id, label, Stage::Performance).spec(SPEC).implemented()
}

pub fn register(reg: &mut vernon_graph::Registry) {
    reg.add(FnNode::new(segments_info, segments_cook));
    reg.add(FnNode::new(path_info, path_cook));
    reg.add(FnNode::new(edit_info, edit_cook));
}

/// Parse the compact segment text: `S<len>`, `L<len>@<radius>`, `R<len>@<radius>`; a corner
/// length may be written as an angle, `L180deg@40`.
pub fn parse_segment_text(s: &str) -> Result<Vec<SegSpec>, String> {
    let mut out = Vec::new();
    for tok in s.split([',', ';', '\n']).map(str::trim).filter(|t| !t.is_empty()) {
        let kind = match tok.chars().next().map(|c| c.to_ascii_uppercase()) {
            Some('S') => SegKind::Straight,
            Some('L') => SegKind::Left,
            Some('R') => SegKind::Right,
            _ => return Err(format!("'{tok}': a segment starts with S, L or R")),
        };
        let rest = tok[1..].trim();
        let num = |t: &str| t.trim().parse::<f64>().map_err(|_| format!("'{tok}': '{t}' is not a number"));
        if kind == SegKind::Straight {
            out.push(SegSpec { kind, length_m: num(rest)?, radius_m: 0.0 });
            continue;
        }
        let (a, r) = rest.split_once('@').ok_or_else(|| format!("'{tok}': a corner is written L<length>@<radius>"))?;
        let radius = num(r)?;
        let a = a.trim();
        let length = match a.strip_suffix("deg") {
            Some(d) => num(d)?.abs().to_radians() * radius,
            None => num(a)?,
        };
        out.push(SegSpec { kind, length_m: length, radius_m: radius });
    }
    if out.is_empty() {
        return Err("no segments".into());
    }
    Ok(out)
}

/// Report shared by the track builders.
fn track_report(title: &str, t: &Track, kappa_straight: f64, min_seg_m: f64) -> Report {
    let len = t.length();
    let segs = t.segment_list(kappa_straight, min_seg_m);
    let kmax = t.kappa.iter().fold(0.0f64, |m, k| m.max(k.abs()));
    let mut r = Report::new(title);
    r.metric("length", Metric::new(len, "m"))
        .metric("intervals", Metric::new(t.intervals() as f64, "-"))
        .metric("segments", Metric::new(segs.len() as f64, "-"))
        .metric("max_ds", Metric::new(t.ds.iter().fold(0.0f64, |m, d| m.max(*d)), "m"))
        .metric("turning", Metric::new(t.kappa.iter().zip(&t.ds).map(|(k, d)| k * d).sum::<f64>().to_degrees(), "deg"));
    if kmax > 1e-9 {
        r.metric("min_radius", Metric::new(1.0 / kmax, "m"));
    }
    if t.closed {
        let (d, h) = t.closure();
        r.metric("closure_error", Metric::new(d, "m"));
        r.metric("heading_mismatch", Metric::new(h.to_degrees().abs(), "deg"));
        if d > 0.02 * len {
            r.note(Severity::Warning, "the segments do not close into a loop (end is far from the start); lap results still use the curvature only");
        }
    }
    r
}

// ------------------------------------------------------------------ from segments

fn segments_info() -> NodeTypeInfo {
    node("perf.track_from_segments", "Track From Segments")
        .input(PortSpec::optional("segments", DataKind::Json))
        .output(PortSpec::new("track", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::string("segments_text", "Segments", "S400, L180deg@40, S400, L180deg@40").help("Used when no segment list is connected. S<length m>, L<length m>@<radius m> or L<angle>deg@<radius m>; R for right-hand corners"))
        .param(ParamSpec::bool("closed", "Closed lap", true).help("Closed circuit (lap) or open run"))
        .param(ParamSpec::float("ds_m", "Step", 1.0, 0.1, 20.0).unit("m").help("Target interval length; each segment is cut into equal intervals no longer than this"))
        .param(ParamSpec::float("kappa_straight", "Straight curvature", 0.001, 1.0e-5, 0.1).unit("1/m").help("Below this curvature (radius above 1 km) a piece counts as straight"))
        .param(ParamSpec::float("min_seg_m", "Shortest segment", 3.0, 0.5, 100.0).unit("m").help("Shorter pieces merge into their neighbour"))
        .help("Builds a track from a hand-entered list of straights and corners (type, length, radius), cut into short intervals with a signed curvature each.")
        .emulates("track file with straight/left/right segments of a lap simulator")
}

fn segments_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let specs = match json_in(inputs, 0)? {
        Some(v) => parse_segments(&v).or_fail(ctx)?,
        None => parse_segment_text(p.str("segments_text")?).or_fail(ctx)?,
    };
    let t = Track::from_segments(&specs, p.bool("closed")?, p.float("ds_m")?).or_fail(ctx)?;
    let (ks, ms) = (p.float("kappa_straight")?, p.float("min_seg_m")?);
    let rep = track_report("Track", &t, ks, ms);
    Ok(vec![Data::json(t.to_json(ks, ms)), Data::report(rep)])
}

// ------------------------------------------------------------------ from path

fn path_info() -> NodeTypeInfo {
    node("perf.track_from_path", "Track From Path")
        .input(PortSpec::optional("curve", DataKind::Geometry))
        .input(PortSpec::optional("file", DataKind::File))
        .output(PortSpec::new("track", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::path("gps_path", "Path file", &["csv"], false).help("CSV with lat,lon (degrees) or x,y (metres) columns; used when neither input is connected"))
        .param(ParamSpec::float("ds_m", "Step", 1.0, 0.1, 20.0).unit("m").help("Resampling interval along the path"))
        .param(ParamSpec::float("smooth_window_m", "Smoothing window", 15.0, 0.0, 100.0).unit("m").help("Length over which curvature is estimated and averaged"))
        .param(ParamSpec::float("curvature_gain", "Curvature gain", 1.0, 0.5, 3.0).help("Multiplies curvature; above 1 tightens hairpins that a coarse path underestimates"))
        .param(ParamSpec::choice("closed", "Closed", "auto", &["auto", "closed", "open"]).help("Auto closes the loop when the end meets the start with a similar heading"))
        .param(ParamSpec::float("kappa_straight", "Straight curvature", 0.001, 1.0e-5, 0.1).unit("1/m").help("Below this curvature a piece counts as straight"))
        .param(ParamSpec::float("min_seg_m", "Shortest segment", 3.0, 0.5, 100.0).unit("m").help("Shorter pieces merge into their neighbour"))
        .help("Turns a drawn path (polyline curve in the XY plane, in mm) or a logged GPS/XY track into a lap track: uniform resampling, local-circle curvature, smoothing and straight/corner segmentation.")
        .emulates("track building from GPS logs and drawn paths for a lap simulator")
}

fn path_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let mut hint_closed = false;
    let pts: Vec<[f64; 2]> = if let Some(Some(Data::Geometry(g))) = inputs.first() {
        g.validate()?;
        let (idx, closed): (Vec<u32>, bool) = match g.prims.iter().find(|pr| pr.vertices.len() >= 3) {
            Some(pr) => (pr.vertices.clone(), pr.closed),
            None => ((0..g.points.len() as u32).collect(), false),
        };
        hint_closed = closed;
        idx.iter().map(|&i| [g.points[i as usize].x / 1000.0, g.points[i as usize].y / 1000.0]).collect()
    } else if let Some(Some(d)) = inputs.get(1) {
        let f = d.as_file().ok_or(CookError::WrongInput { index: 1, expected: DataKind::File, found: d.kind() })?;
        parse_path_csv(&read_text(ctx, f)?).or_fail(ctx)?
    } else {
        let path = p.str("gps_path")?;
        if path.is_empty() {
            return Err(ctx.fail("connect a curve or a path file, or set the path file parameter"));
        }
        parse_path_csv(&read_text(ctx, std::path::Path::new(path))?).or_fail(ctx)?
    };
    let mode = match p.str("closed")? {
        "closed" => ClosedMode::Closed,
        "open" => ClosedMode::Open,
        _ => ClosedMode::Auto,
    };
    let opts = PathOpts {
        ds_m: p.float("ds_m")?,
        smooth_window_m: p.float("smooth_window_m")?,
        curvature_gain: p.float("curvature_gain")?,
        closed: mode,
    };
    let t = track_from_path(&pts, hint_closed, &opts).or_fail(ctx)?;
    let (ks, ms) = (p.float("kappa_straight")?, p.float("min_seg_m")?);
    let mut rep = track_report("Track from path", &t, ks, ms);
    rep.metric("path_points", Metric::new(pts.len() as f64, "-"));
    rep.note(Severity::Info, if t.closed { "closed loop detected" } else { "open path" });
    Ok(vec![Data::json(t.to_json(ks, ms)), Data::report(rep)])
}

// ------------------------------------------------------------------ edit

fn edit_info() -> NodeTypeInfo {
    node("perf.track_edit", "Track Edit")
        .input(PortSpec::new("track", DataKind::Json))
        .output(PortSpec::new("track", DataKind::Json))
        .output(PortSpec::new("report", DataKind::Report))
        .param(ParamSpec::bool("reverse", "Reverse", false).help("Drive the track the other way round"))
        .param(ParamSpec::bool("mirror", "Mirror", false).help("Swap left and right"))
        .param(ParamSpec::float("scale", "Scale", 1.0, 0.1, 10.0).help("Uniform scale: lengths multiply, curvature divides"))
        .param(ParamSpec::float("kappa_straight", "Straight curvature", 0.001, 1.0e-5, 0.1).unit("1/m").help("Below this curvature a piece counts as straight"))
        .param(ParamSpec::float("min_seg_m", "Shortest segment", 3.0, 0.5, 100.0).unit("m").help("Shorter pieces merge into their neighbour"))
        .help("Reverses, mirrors or scales a track.")
        .emulates("track editing (reverse, mirror, scale) of a lap simulator")
}

fn edit_cook(ctx: &CookContext, inputs: &[Option<Data>], p: &Params) -> CookResult<Vec<Data>> {
    let v = json_in(inputs, 0)?.ok_or(CookError::MissingInput { index: 0, name: "track".into() })?;
    let mut t = Track::from_json(&v).or_fail(ctx)?;
    if p.bool("reverse")? {
        t = t.reversed();
    }
    if p.bool("mirror")? {
        t = t.mirrored();
    }
    let s = p.float("scale")?;
    if s != 1.0 {
        t = t.scaled(s);
    }
    let (ks, ms) = (p.float("kappa_straight")?, p.float("min_seg_m")?);
    let rep = track_report("Track edit", &t, ks, ms);
    Ok(vec![Data::json(t.to_json(ks, ms)), Data::report(rep)])
}

// ------------------------------------------------------------------ shared: path geometry

/// A polyline of the track in mm, coloured by a per-node scalar (blue slow to red fast).
pub fn coloured_path(track: &Track, values: &[f64], name: &str, unit_scale: f64) -> Geometry {
    let xy = track.path_xy();
    let mut g = Geometry::new();
    for p in &xy {
        g.add_point(DVec3::new(p[0] * 1000.0, p[1] * 1000.0, 0.0));
    }
    let n = xy.len() as u32;
    g.add_prim(Prim { vertices: (0..n).collect(), closed: track.closed });
    let (lo, hi) = values.iter().fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
    let span = (hi - lo).max(1e-12);
    let cd: Vec<DVec3> = values
        .iter()
        .map(|v| {
            let t = ((v - lo) / span).clamp(0.0, 1.0);
            let (blue, yellow, red) = (DVec3::new(0.1, 0.2, 1.0), DVec3::new(1.0, 0.9, 0.1), DVec3::new(1.0, 0.1, 0.1));
            if t < 0.5 {
                blue.lerp(yellow, 2.0 * t)
            } else {
                yellow.lerp(red, 2.0 * t - 1.0)
            }
        })
        .collect();
    g.point_attribs.set("Cd", AttribArray::Vec3(cd));
    g.point_attribs.set(name, AttribArray::Float(values.iter().map(|v| v * unit_scale).collect()));
    g
}
