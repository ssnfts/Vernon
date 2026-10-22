//! Scan import, phone-scan import, unit setting and marker tables.

use std::path::Path;

use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::{
    inputs, CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
};
use vernon_io::{FileUnit, Format, ReadOptions};

use crate::tables::{parse_marker_list, DuplicatePolicy};
use crate::units::{classify_size, parse_unit, unit_name, UnitVerdict};
use crate::util::{self, base, set_metric_flag};

/// File extensions the importers accept.
pub const SCAN_EXTENSIONS: &[&str] = &["ply", "obj", "stl", "xyz", "pts", "asc", "3mf"];

/// How to interpret the numbers in a file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnitChoice {
    /// Use the size check; fall back to mm and mark non-metric when unsure.
    Auto,
    /// Scale is unknown: keep the numbers, mark non-metric.
    Unknown,
    Fixed(FileUnit),
}

impl UnitChoice {
    pub fn parse(s: &str) -> Option<UnitChoice> {
        match s {
            "auto" => Some(UnitChoice::Auto),
            "unknown" => Some(UnitChoice::Unknown),
            other => parse_unit(other).map(UnitChoice::Fixed),
        }
    }
}

/// Options for [`import`].
pub struct ImportOptions<'a> {
    pub path: &'a Path,
    pub unit: UnitChoice,
    pub weld: bool,
    pub keep_normals: bool,
    pub keep_colors: bool,
    pub unit_check: bool,
    pub title: &'a str,
}

fn verdict_text(v: &UnitVerdict) -> String {
    match v {
        UnitVerdict::Confident(u) => format!("vehicle-sized and car-shaped in {}", unit_name(*u)),
        UnitVerdict::Ambiguous(c) => format!(
            "vehicle-sized only if the unit is {}; a part in another unit is just as plausible",
            c.iter().map(|u| unit_name(*u)).collect::<Vec<_>>().join(" or ")
        ),
        UnitVerdict::NoMatch => "not vehicle-sized (part scale)".to_string(),
    }
}

/// Read a scan file, apply the unit choice, run the size check and build the
/// import report. Pure function so the nodes and tests share it.
pub fn import(opts: &ImportOptions) -> Result<(Geometry, Report), String> {
    let path = opts.path;
    if path.as_os_str().is_empty() {
        return Err("no file path set".into());
    }
    if !path.is_file() {
        return Err(format!("file not found: {}", path.display()));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !SCAN_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "unsupported file type '.{ext}' (supported: {}); export the scan as PLY, OBJ or STL",
            SCAN_EXTENSIONS.join(", ")
        ));
    }
    let format = if ext == "asc" {
        Format::Xyz
    } else {
        Format::from_path(path).ok_or_else(|| format!("unsupported file type '.{ext}'"))?
    };
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let read_opts = ReadOptions {
        unit: FileUnit::Millimetre,
        weld: opts.weld,
        weld_tolerance: 1e-6,
    };
    let mut geo =
        vernon_io::read_bytes(format, &bytes, &read_opts).map_err(|e| format!("{}: {e}", path.display()))?;
    if geo.points.is_empty() {
        return Err(format!("{} contains no points", path.display()));
    }
    let raw_size = geo.bounds().size();
    let verdict = classify_size(raw_size);
    let mut report = Report::new(opts.title);

    // 3MF declares its own unit and is already in mm after reading.
    let (factor, metric, declared) = if format == Format::ThreeMf {
        (1.0, true, Some(FileUnit::Millimetre))
    } else {
        match opts.unit {
            UnitChoice::Fixed(u) => (u.to_mm(), true, Some(u)),
            UnitChoice::Auto => match &verdict {
                UnitVerdict::Confident(u) => {
                    report.note(
                        Severity::Info,
                        format!("unit detected from size: {} ({})", unit_name(*u), verdict_text(&verdict)),
                    );
                    (u.to_mm(), true, Some(*u))
                }
                other => {
                    report.note(
                        Severity::Warning,
                        format!(
                            "unit could not be determined ({}); numbers kept as mm and the geometry is marked non-metric, set the unit or run a scale node",
                            verdict_text(other)
                        ),
                    );
                    (1.0, false, None)
                }
            },
            UnitChoice::Unknown => {
                report.note(
                    Severity::Warning,
                    "scale is unknown; the geometry is marked non-metric until a scale node (Scale From Distance or Solve Scale Bars) has run",
                );
                (1.0, false, None)
            }
        }
    };
    if factor != 1.0 {
        for p in &mut geo.points {
            *p *= factor;
        }
    }
    if opts.unit_check {
        if let Some(declared) = declared.filter(|_| opts.unit != UnitChoice::Auto) {
            let name = unit_name(declared);
            match &verdict {
                UnitVerdict::Confident(u) if *u == declared => report.note(
                    Severity::Info,
                    format!("size check: {}", verdict_text(&verdict)),
                ),
                UnitVerdict::Confident(u) => report.note(
                    Severity::Warning,
                    format!(
                        "size check: file is declared as {name} but looks {}; no rescale applied, change the unit if this is wrong",
                        verdict_text(&UnitVerdict::Confident(*u))
                    ),
                ),
                UnitVerdict::Ambiguous(c) if c.contains(&declared) => report.note(
                    Severity::Info,
                    format!("size check: {}", verdict_text(&verdict)),
                ),
                UnitVerdict::Ambiguous(_) => report.note(
                    Severity::Warning,
                    format!(
                        "size check ambiguous: declared {name}, but {}; no rescale applied",
                        verdict_text(&verdict)
                    ),
                ),
                UnitVerdict::NoMatch => report.note(
                    Severity::Info,
                    "size check: part scale, the unit cannot be verified from the size",
                ),
            };
        }
    }
    if !opts.keep_normals {
        geo.point_attribs.remove("N");
        geo.prim_attribs.remove("N");
    }
    if !opts.keep_colors {
        geo.point_attribs.remove("Cd");
    }
    set_metric_flag(&mut geo, metric);
    report.metric("point_count", Metric::new(geo.point_count() as f64, "points"));
    report.metric("prim_count", Metric::new(geo.prim_count() as f64, "prims"));
    util::bbox_metrics(&mut report, &geo);
    report.metric("unit_scale", Metric::new(factor, "mm/unit"));
    report.metric("metric", Metric::new(f64::from(u8::from(metric)), "flag"));
    Ok((geo, report))
}

fn geo_report(i: NodeTypeInfo) -> NodeTypeInfo {
    i.output(PortSpec::new("geometry", DataKind::Geometry))
        .output(PortSpec::new("report", DataKind::Report))
}

fn path_param() -> ParamSpec {
    ParamSpec::path("path", "File", SCAN_EXTENSIONS, false).help("Scan file to read.")
}

/// Import a scanner export (PLY, OBJ, STL, XYZ/PTS/ASC, 3MF).
pub struct ImportScan;

impl NodeOp for ImportScan {
    fn info(&self) -> NodeTypeInfo {
        geo_report(
            base("capture.import_scan", "Import Scan")
                .param(path_param())
                .param(
                    ParamSpec::choice("unit", "File unit", "mm", &["mm", "cm", "m", "in", "auto"])
                        .help("Unit of the numbers in the file. 'auto' uses the vehicle-scale size check and falls back to non-metric when unsure."),
                )
                .param(ParamSpec::bool("unit_check", "Size check", true).help("Warn when the size does not match the chosen unit."))
                .param(ParamSpec::bool("weld", "Weld STL vertices", true).help("Merge coincident vertices of STL files."))
                .param(ParamSpec::bool("keep_normals", "Keep normals", true))
                .param(ParamSpec::bool("keep_colors", "Keep colors", true)),
        )
        .help("Reads a scanner export into millimetres, runs a vehicle-scale size check and reports point count and bounding box. Geometry is marked metric only when the unit is known.")
        .emulates("handheld-scanner mesh / point-cloud export import with explicit unit handling")
        .implemented()
    }

    fn cook(&self, ctx: &CookContext, _inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let path = params.str("path")?;
        let unit = UnitChoice::parse(params.str("unit")?)
            .filter(|u| *u != UnitChoice::Unknown)
            .ok_or_else(|| util::bad_choice(ctx, "unit", params.str("unit").unwrap_or("")))?;
        let (geo, rep) = import(&ImportOptions {
            path: Path::new(path),
            unit,
            weld: params.bool("weld")?,
            keep_normals: params.bool("keep_normals")?,
            keep_colors: params.bool("keep_colors")?,
            unit_check: params.bool("unit_check")?,
            title: "Import Scan",
        })
        .map_err(|m| ctx.fail(m))?;
        Ok(vec![Data::geometry(geo), Data::report(rep)])
    }
}

/// Import a phone or photogrammetry export where the scale is not trusted.
pub struct ImportPhoneScan;

impl NodeOp for ImportPhoneScan {
    fn info(&self) -> NodeTypeInfo {
        geo_report(
            base("capture.import_phone_scan", "Import Phone Scan")
                .param(path_param())
                .param(
                    ParamSpec::choice("unit", "File unit", "unknown", &["unknown", "mm", "cm", "m", "in"])
                        .help("Export units are not recorded by most phone apps; leave 'unknown' and fix the scale with a known distance."),
                )
                .param(
                    ParamSpec::choice("capture_mode", "Capture mode", "photogrammetry", &["photogrammetry", "lidar"])
                        .help("Photogrammetry needs scale correction; depth-sensor capture is too coarse for fitment."),
                )
                .param(ParamSpec::bool("weld", "Weld STL vertices", true))
                .param(ParamSpec::bool("keep_colors", "Keep colors", true)),
        )
        .help("Reads a mock-up grade mesh or cloud from a phone or photogrammetry export. The scale is treated as unknown (non-metric) until a scale node has run.")
        .emulates("phone photogrammetry / depth-sensor app export import with rescale-after-import workflow")
        .implemented()
    }

    fn cook(&self, ctx: &CookContext, _inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let unit = UnitChoice::parse(params.str("unit")?)
            .ok_or_else(|| util::bad_choice(ctx, "unit", params.str("unit").unwrap_or("")))?;
        let mode = params.str("capture_mode")?.to_string();
        let (geo, mut rep) = import(&ImportOptions {
            path: Path::new(params.str("path")?),
            unit,
            weld: params.bool("weld")?,
            keep_normals: true,
            keep_colors: params.bool("keep_colors")?,
            unit_check: true,
            title: "Import Phone Scan",
        })
        .map_err(|m| ctx.fail(m))?;
        if mode == "lidar" {
            rep.note(
                Severity::Warning,
                "depth-sensor capture is mock-up grade; it is too coarse for panel fitment, use photogrammetry or a laser scanner",
            );
        } else {
            rep.note(
                Severity::Info,
                "photogrammetry output has an arbitrary scale until corrected with a known distance",
            );
        }
        Ok(vec![Data::geometry(geo), Data::report(rep)])
    }
}

/// Re-interpret the units of a geometry and mark it metric.
pub struct SetUnits;

impl NodeOp for SetUnits {
    fn info(&self) -> NodeTypeInfo {
        geo_report(
            base("capture.set_units", "Set Units")
                .input(PortSpec::new("geometry", DataKind::Geometry))
                .param(
                    ParamSpec::choice("from_unit", "Current unit", "mm", &["mm", "cm", "m", "in"])
                        .help("Unit the geometry is currently expressed in."),
                )
                .param(
                    ParamSpec::float("factor", "Factor override", 0.0, 0.0, 1e6)
                        .unit("mm/unit")
                        .help("When above zero, multiply by this instead of using the unit."),
                )
                .param(ParamSpec::bool("guess_only", "Guess only", false).help("Report the size-check guess without changing the geometry.")),
        )
        .help("Converts a geometry to millimetres from a stated unit (or an explicit factor) and marks it metric. The size check reports whether the result looks vehicle-sized.")
        .emulates("unit conversion on import in scan-processing software")
        .implemented()
    }

    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let src = inputs::geometry(inputs, 0)?;
        let unit = parse_unit(params.str("from_unit")?)
            .ok_or_else(|| util::bad_choice(ctx, "from_unit", params.str("from_unit").unwrap_or("")))?;
        let override_f = params.float("factor")?;
        util::require_non_negative(ctx, "factor", override_f)?;
        let factor = if override_f > 0.0 { override_f } else { unit.to_mm() };
        let verdict = classify_size(src.bounds().size() * factor);
        let mut rep = Report::new("Set Units");
        let mut geo = (*src).clone();
        if params.bool("guess_only")? {
            // Size check in the *current* unit, applied to the untouched numbers.
            let v = classify_size(src.bounds().size());
            rep.note(Severity::Info, format!("size check on the unchanged geometry: {}", verdict_text(&v)));
        } else {
            for p in &mut geo.points {
                *p *= factor;
            }
            set_metric_flag(&mut geo, true);
            rep.note(
                Severity::Info,
                format!("scaled by {factor} (to mm); size check: {}", verdict_text(&verdict)),
            );
            if let UnitVerdict::Confident(u) = verdict {
                if u != FileUnit::Millimetre {
                    rep.note(
                        Severity::Warning,
                        format!("after conversion the geometry still looks like a vehicle in {}", unit_name(u)),
                    );
                }
            }
        }
        rep.metric("unit_scale", Metric::new(if params.bool("guess_only")? { 1.0 } else { factor }, "mm/unit"));
        rep.metric("point_count", Metric::new(geo.point_count() as f64, "points"));
        util::bbox_metrics(&mut rep, &geo);
        Ok(vec![Data::geometry(geo), Data::report(rep)])
    }
}

/// Read a survey table of marker coordinates.
pub struct MarkerTable;

impl NodeOp for MarkerTable {
    fn info(&self) -> NodeTypeInfo {
        geo_report(
            base("capture.marker_table", "Marker Table")
                .param(ParamSpec::path("path", "Table file", &["csv", "txt"], false).help("Rows of id, x, y, z and optional sigma."))
                .param(ParamSpec::choice("unit", "Coordinate unit", "mm", &["mm", "cm", "m", "in"]))
                .param(
                    ParamSpec::choice("duplicates", "Duplicate ids", "error", &["error", "keep_first", "keep_last"])
                        .help("What to do when a marker id appears twice."),
                ),
        )
        .help("Reads marker coordinates (id, x, y, z, optional sigma) into a point cloud with an 'id' point attribute, in millimetres.")
        .emulates("coded-target / marker coordinate table import for scan alignment")
        .implemented()
    }

    fn cook(&self, ctx: &CookContext, _inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let path = params.str("path")?;
        if path.is_empty() {
            return Err(ctx.fail("no table file set"));
        }
        let text = std::fs::read_to_string(path).map_err(|e| ctx.fail(format!("cannot read {path}: {e}")))?;
        let unit = parse_unit(params.str("unit")?)
            .ok_or_else(|| util::bad_choice(ctx, "unit", params.str("unit").unwrap_or("")))?;
        let policy = match params.str("duplicates")? {
            "error" => DuplicatePolicy::Error,
            "keep_first" => DuplicatePolicy::KeepFirst,
            "keep_last" => DuplicatePolicy::KeepLast,
            other => return Err(util::bad_choice(ctx, "duplicates", other)),
        };
        let list = parse_marker_list(&text, policy).map_err(|m| ctx.fail(format!("{path}: {m}")))?;
        let k = unit.to_mm();
        let mut geo = Geometry::from_points(list.coords.iter().map(|c| glam::DVec3::from_array(*c) * k).collect());
        geo.point_attribs.set("id", vernon_core::AttribArray::Str(list.ids.clone()));
        if list.sigma.iter().any(Option::is_some) {
            geo.point_attribs.set(
                "sigma",
                vernon_core::AttribArray::Float(list.sigma.iter().map(|s| s.map_or(0.0, |v| v * k)).collect()),
            );
        }
        set_metric_flag(&mut geo, true);
        let mut rep = Report::new("Marker Table");
        rep.metric("marker_count", Metric::new(geo.point_count() as f64, "markers"));
        util::bbox_metrics(&mut rep, &geo);
        Ok(vec![Data::geometry(geo), Data::report(rep)])
    }
}
