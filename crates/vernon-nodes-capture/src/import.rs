//! Scan import, phone-scan import, unit setting and marker tables.

use std::path::Path;

use vernon_core::{Geometry, Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
};
use vernon_io::{FileUnit, Format, ReadOptions};

use crate::tables::{parse_marker_list, DuplicatePolicy};
use crate::units::{classify_size, parse_unit, unit_name, UnitVerdict};
use crate::util::{self, base, set_metric_flag};

/// File extensions the importers accept.
pub const SCAN_EXTENSIONS: &[&str] = &["ply", "obj", "stl", "xyz", "pts", "asc", "3mf", "e57", "las"];

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

    // 3MF, E57 and LAS declare their own unit and are already in mm after
    // reading. A LAS file that states none was read as metres, a guess, so its
    // scale is unverified and the geometry is not marked metric.
    let las_assumed = format == Format::Las && vernon_io::las_unit_was_assumed(&geo);
    if las_assumed {
        report.note(
            Severity::Warning,
            "LAS: the file does not state its length unit, so metres were assumed; the geometry is marked non-metric. If the unit is right, run Set Units (from mm) to mark it metric; if the file uses another unit, fix the scale with Set Units (factor) or a scale node, or read it with File Read and its 'las_unit' parameter",
        );
    }
    let (factor, metric, declared) = if matches!(format, Format::ThreeMf | Format::E57 | Format::Las) {
        (1.0, !las_assumed, Some(FileUnit::Millimetre))
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

/// Import a scanner export (PLY, OBJ, STL, XYZ/PTS/ASC, 3MF, E57, LAS).
pub struct ImportScan;

impl NodeOp for ImportScan {
    fn info(&self) -> NodeTypeInfo {
        geo_report(
            base("capture.import_scan", "Import Scan")
                .param(path_param())
                .param(
                    ParamSpec::choice("unit", "File unit", "mm", &["mm", "cm", "m", "in", "auto"])
                        .help("Unit of the numbers in the file. 'auto' uses the vehicle-scale size check and falls back to non-metric when unsure. 3MF, E57 and LAS state their own unit and ignore this; a LAS file that states none is read as metres and marked non-metric."),
                )
                .param(ParamSpec::bool("unit_check", "Size check", true).help("Warn when the size does not match the chosen unit."))
                .param(ParamSpec::bool("weld", "Weld STL vertices", true).help("Merge coincident vertices of STL files."))
                .param(ParamSpec::bool("keep_normals", "Keep normals", true).help("Keep per-vertex normals from the file. Off drops them so they can be re-estimated."))
                .param(ParamSpec::bool("keep_colors", "Keep colors", true).help("Keep per-vertex colours from the file. Off drops them.")),
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
                .param(ParamSpec::bool("weld", "Weld STL vertices", true).help("Merge coincident vertices after reading. Mesh files that store every triangle separately need this to be watertight."))
                .param(ParamSpec::bool("keep_colors", "Keep colors", true).help("Keep per-vertex colours from the file. Off drops them.")),
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
                .param(ParamSpec::choice("unit", "Coordinate unit", "mm", &["mm", "cm", "m", "in"]).help("Unit of the coordinates in the file. They are converted to millimetres on import."))
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

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use vernon_graph::ParamValue;

    use super::*;
    use crate::util::metric_flag;

    /// A LAS 1.2 file, point format 0, scale 0.001, with an optional GeoTIFF
    /// key directory holding (key, value) pairs.
    fn las(points: &[[i32; 3]], keys: Option<&[(u16, u16)]>) -> Vec<u8> {
        let mut vlr = Vec::new();
        if let Some(keys) = keys {
            let mut words = vec![1u16, 1, 0, keys.len() as u16];
            for (k, v) in keys {
                words.extend([*k, 0, 1, *v]);
            }
            let body: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            vlr = vec![0u8; 54];
            vlr[2..17].copy_from_slice(b"LASF_Projection");
            vlr[18..20].copy_from_slice(&34735u16.to_le_bytes());
            vlr[20..22].copy_from_slice(&(body.len() as u16).to_le_bytes());
            vlr.extend(body);
        }
        let mut b = vec![0u8; 227];
        b[0..4].copy_from_slice(b"LASF");
        b[24] = 1;
        b[25] = 2;
        b[94..96].copy_from_slice(&227u16.to_le_bytes());
        b[96..100].copy_from_slice(&((227 + vlr.len()) as u32).to_le_bytes());
        b[100..104].copy_from_slice(&u32::from(keys.is_some()).to_le_bytes());
        b[105..107].copy_from_slice(&20u16.to_le_bytes());
        b[107..111].copy_from_slice(&(points.len() as u32).to_le_bytes());
        for a in 0..3 {
            b[131 + 8 * a..139 + 8 * a].copy_from_slice(&0.001f64.to_le_bytes());
        }
        b.extend(vlr);
        for p in points {
            let mut r = vec![0u8; 20];
            for (a, v) in p.iter().enumerate() {
                r[4 * a..4 * a + 4].copy_from_slice(&v.to_le_bytes());
            }
            b.extend(r);
        }
        b
    }

    /// Cook `capture.import_scan` on a file written under a fresh temp folder.
    fn cook_import(name: &str, bytes: &[u8], unit: &str) -> Result<(Geometry, Report), String> {
        let dir = std::env::temp_dir().join(format!("vernon-capture-import-{}-{}", name.replace('.', "_"), std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let node = ImportScan;
        let mut params = Params::from_specs(&node.info().params);
        params.set("path", ParamValue::Str(path.to_str().unwrap().to_string()));
        params.set("unit", ParamValue::Str(unit.to_string()));
        let ctx = CookContext { node_name: "import".into(), work_dir: dir.clone(), frame: 0.0 };
        let out = node.cook(&ctx, &[], &params).map_err(|e| e.to_string());
        std::fs::remove_dir_all(&dir).ok();
        let out = out?;
        let geo = (**out[0].as_geometry().unwrap()).clone();
        Ok((geo, (**out[1].as_report().unwrap()).clone()))
    }

    fn warnings(r: &Report) -> Vec<&str> {
        r.notes.iter().filter(|(s, _)| *s == Severity::Warning).map(|(_, t)| t.as_str()).collect()
    }

    // 1, 2 and 3 file units after the scale.
    const POINTS: [[i32; 3]; 2] = [[0, 0, 0], [1000, 2000, 3000]];

    #[test]
    fn the_scan_extensions_include_e57_and_las() {
        for ext in ["e57", "las"] {
            assert!(SCAN_EXTENSIONS.contains(&ext), "{ext}");
        }
        let info = ImportScan.info();
        let path = info.params.iter().find(|p| p.name == "path").unwrap();
        let text = format!("{:?}", path.kind);
        assert!(text.contains("e57") && text.contains("las"), "{text}");
    }

    #[test]
    fn a_las_file_that_states_its_unit_imports_in_millimetres_and_metric() {
        let keys = [(1024, 1), (3076, 9001)];
        for unit in ["mm", "in", "auto"] {
            // The node's unit does not apply to LAS: the file states metres.
            let (g, r) = cook_import("states_unit.las", &las(&POINTS, Some(&keys)), unit).unwrap();
            assert_eq!(g.point_count(), 2, "{unit}");
            assert!((g.points[1] - DVec3::new(1000.0, 2000.0, 3000.0)).abs().max_element() < 1e-9, "{unit}");
            assert_eq!(metric_flag(&g), Some(true), "{unit}");
            assert_eq!(r.metrics["unit_scale"].value, 1.0);
            assert_eq!(r.metrics["metric"].value, 1.0);
            assert!(!warnings(&r).iter().any(|w| w.contains("LAS")), "{unit}: {:?}", warnings(&r));
            assert_eq!(g.detail_attribs.strs("las_unit_source").unwrap(), &["header".to_string()]);
        }
    }

    #[test]
    fn a_las_file_without_a_stated_unit_is_read_as_metres_but_flagged() {
        // No coordinate-system record, and an EPSG code with no unit key.
        for keys in [None, Some(&[(1024u16, 1u16), (3072, 2263)][..])] {
            let (g, r) = cook_import("assumed.las", &las(&POINTS, keys), "mm").unwrap();
            assert!((g.points[1] - DVec3::new(1000.0, 2000.0, 3000.0)).abs().max_element() < 1e-9);
            assert_eq!(metric_flag(&g), Some(false), "an assumed unit is not verified");
            assert_eq!(r.metrics["metric"].value, 0.0);
            let w = warnings(&r);
            assert!(w.iter().any(|t| t.contains("metres were assumed") && t.contains("non-metric")), "{w:?}");
        }
    }

    #[test]
    fn e57_goes_through_the_reader_and_laz_stays_refused() {
        // Damaged bytes with an .e57 name reach the E57 reader (not the
        // extension check), which names the problem.
        let e = cook_import("broken.e57", b"not an e57 file", "mm").unwrap_err();
        assert!(e.contains("E57") && !e.contains("unsupported file type"), "{e}");
        let e = cook_import("tile.laz", &las(&POINTS, None), "mm").unwrap_err();
        assert!(e.contains("unsupported file type '.laz'"), "{e}");
    }
}
