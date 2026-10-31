//! File read / write through `vernon-io`.

use std::path::{Path, PathBuf};

use vernon_core::{Metric, Report, Severity};
use vernon_graph::node::inputs;
use vernon_graph::{
    CookContext, CookResult, Data, DataKind, NodeOp, NodeTypeInfo, ParamSpec, Params, PortSpec,
    Stage,
};
use vernon_io::{FileUnit, Format, ReadOptions, WriteOptions};

use crate::util::{non_negative, SPEC};

const UNITS: [&str; 4] = ["mm", "cm", "m", "in"];
const LAS_UNITS: [&str; 5] = ["auto", "mm", "cm", "m", "in"];

/// Map a unit choice string to a file unit.
pub fn file_unit(s: &str) -> FileUnit {
    match s {
        "cm" => FileUnit::Centimetre,
        "m" => FileUnit::Metre,
        "in" => FileUnit::Inch,
        _ => FileUnit::Millimetre,
    }
}

fn unit_param(help: &str) -> ParamSpec {
    ParamSpec::choice("unit", "File unit", "mm", &UNITS).help(help)
}

fn require_path(ctx: &CookContext, params: &Params) -> CookResult<PathBuf> {
    let raw = params.str("path")?.trim();
    if raw.is_empty() {
        return Err(ctx.fail("no file path set"));
    }
    let p = PathBuf::from(raw);
    if Format::from_path(&p).is_none() {
        return Err(ctx.fail(format!(
            "unsupported file extension in '{raw}' (supported: {})",
            Format::extensions().join(", ")
        )));
    }
    Ok(p)
}

/// Resolve a relative path against the node's work directory when it does
/// not exist relative to the current directory.
fn resolve_existing(ctx: &CookContext, p: &Path) -> PathBuf {
    if p.is_relative() && !p.exists() {
        let alt = ctx.work_dir.join(p);
        if alt.exists() {
            return alt;
        }
    }
    p.to_path_buf()
}

pub struct FileReadNode;

impl NodeOp for FileReadNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.file_read", "File Read", Stage::Geometry)
            .output(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("report", DataKind::Report))
            .param(
                ParamSpec::path("path", "File", Format::extensions(), false)
                    .help("Mesh or point-cloud file to load (OBJ, STL, PLY, 3MF, XYZ, E57, LAS)."),
            )
            .param(unit_param(
                "Unit the file's numbers are in; converted to millimetres. 3MF, E57 and LAS state their own unit and ignore this.",
            ))
            .param(
                ParamSpec::choice("las_unit", "LAS unit", "auto", &LAS_UNITS).help(
                    "LAS only: unit of the coordinates. Auto reads it from the file's coordinate-system records and assumes metres, with a warning, when they state no unit.",
                ),
            )
            .param(
                ParamSpec::bool("weld", "Weld vertices", true)
                    .help("Merge coincident vertices, e.g. the per-triangle corners of STL."),
            )
            .param(
                ParamSpec::float("weld_tolerance", "Weld tolerance", 1.0e-6, 0.0, 10.0)
                    .unit("mm")
                    .help("Vertices closer than this are merged when welding."),
            )
            .help("Loads a mesh or point cloud from disk in millimetres and reports its size.")
            .emulates("Import of neutral mesh formats (OBJ/STL/PLY/3MF) as in any DCC or RE tool's File > Import")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, _: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let path = resolve_existing(ctx, &require_path(ctx, params)?);
        let opts = ReadOptions {
            unit: file_unit(params.str("unit")?),
            weld: params.bool("weld")?,
            weld_tolerance: non_negative(params, "weld_tolerance")?,
        };
        let las_unit = params.str("las_unit")?;
        let geo = if las_unit != "auto" && Format::from_path(&path) == Some(Format::Las) {
            std::fs::read(&path)
                .map_err(vernon_io::IoError::from)
                .and_then(|bytes| vernon_io::read_las_bytes(&bytes, Some(file_unit(las_unit))))
        } else {
            vernon_io::read(&path, &opts)
        }
        .map_err(|e| ctx.fail(format!("cannot read '{}': {e}", path.display())))?;
        let size = geo.bounds().size();
        let mut r = Report::new("File read");
        r.metric("point_count", Metric::new(geo.points.len() as f64, "count"));
        r.metric("prim_count", Metric::new(geo.prims.len() as f64, "count"));
        r.metric("size_x", Metric::new(size.x, "mm"));
        r.metric("size_y", Metric::new(size.y, "mm"));
        r.metric("size_z", Metric::new(size.z, "mm"));
        r.note(Severity::Info, format!("read {}", path.display()));
        if vernon_io::las_unit_was_assumed(&geo) {
            r.note(
                Severity::Warning,
                "LAS: the file does not state its length unit, so metres were assumed; if the coordinates are in another unit the scale is wrong. Name the unit with the 'las_unit' parameter (mm, cm, m or in); for feet set it to 'mm' and scale the geometry by 304.8",
            );
        }
        if geo.points.is_empty() {
            r.note(Severity::Warning, "file contains no points");
        }
        Ok(vec![Data::geometry(geo), Data::report(r)])
    }
}

pub struct FileWriteNode;

impl NodeOp for FileWriteNode {
    fn info(&self) -> NodeTypeInfo {
        NodeTypeInfo::new("geo.file_write", "File Write", Stage::Geometry)
            .input(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("geometry", DataKind::Geometry))
            .output(PortSpec::new("file", DataKind::File))
            .param(
                ParamSpec::path("path", "File", Format::write_extensions(), true)
                    .help("Where to save; the extension picks the format. Relative paths go to the node's work folder."),
            )
            .param(unit_param("Unit to write the numbers in."))
            .param(
                ParamSpec::bool("ascii", "ASCII", false)
                    .help("Write a text variant where the format has one."),
            )
            .help("Saves the incoming geometry to disk and passes it through unchanged.")
            .emulates("Export of neutral mesh formats (OBJ/STL/PLY/3MF) as in any DCC tool's File > Export or a ROP output node")
            .spec(SPEC)
            .implemented()
    }

    fn cook(&self, ctx: &CookContext, ins: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>> {
        let g = inputs::geometry(ins, 0)?;
        let mut path = require_path(ctx, params)?;
        if path.is_relative() {
            path = ctx.work_dir.join(path);
        }
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let opts = WriteOptions {
            unit: file_unit(params.str("unit")?),
            ascii: params.bool("ascii")?,
            ..Default::default()
        };
        vernon_io::write(&path, &g, &opts)
            .map_err(|e| ctx.fail(format!("cannot write '{}': {e}", path.display())))?;
        Ok(vec![Data::Geometry(g), Data::File(path)])
    }
}
