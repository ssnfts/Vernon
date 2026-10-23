use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{CookError, CookResult, Data, DataKind, ParamSpec, Params};

/// The body-kit pipeline stage a node belongs to. Drives grouping in the
/// Create panel and the pipeline-gate dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Stage {
    /// General geometry tools not tied to one stage.
    Geometry,
    /// 1. Scan / photogrammetry capture and import.
    Capture,
    /// 2. Clean, align, measure deviation.
    Clean,
    /// 3. Mesh to CAD surface.
    Surface,
    /// 4. Split, key, slice, print.
    Print,
    /// 5. CFD / airflow.
    Cfd,
    /// 6. Structural FEA.
    Fea,
    /// 7. Solar / UV dose and surface temperature.
    Solar,
    /// 7b. UV unwrap / flatten.
    Flatten,
    /// 8. Performance and validation.
    Performance,
}

impl Stage {
    pub const ALL: [Stage; 10] = [
        Stage::Geometry,
        Stage::Capture,
        Stage::Clean,
        Stage::Surface,
        Stage::Print,
        Stage::Cfd,
        Stage::Fea,
        Stage::Solar,
        Stage::Flatten,
        Stage::Performance,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Geometry => "Geometry",
            Stage::Capture => "1 Capture",
            Stage::Clean => "2 Clean / Align",
            Stage::Surface => "3 Mesh → Surface",
            Stage::Print => "4 Split / Print",
            Stage::Cfd => "5 Airflow (CFD)",
            Stage::Fea => "6 Structure (FEA)",
            Stage::Solar => "7 Solar / Heat",
            Stage::Flatten => "7b Flatten",
            Stage::Performance => "8 Performance",
        }
    }
}

/// One input or output port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortSpec {
    pub name: String,
    pub kind: DataKind,
    /// Inputs only: whether cooking fails when the input is unconnected.
    pub required: bool,
}

impl PortSpec {
    pub fn new(name: &str, kind: DataKind) -> Self {
        Self {
            name: name.into(),
            kind,
            required: true,
        }
    }

    pub fn optional(name: &str, kind: DataKind) -> Self {
        Self {
            name: name.into(),
            kind,
            required: false,
        }
    }
}

/// Static description of a node type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeTypeInfo {
    /// Stable id, `stage.verb_noun`, e.g. `clean.icp_align`.
    pub id: String,
    /// Menu / stack label, e.g. `ICP Align`.
    pub label: String,
    pub stage: Stage,
    pub inputs: Vec<PortSpec>,
    pub outputs: Vec<PortSpec>,
    pub params: Vec<ParamSpec>,
    /// Short description shown as help.
    pub help: String,
    /// Which tool behaviours this node reimplements, described generically,
    /// e.g. `["Rhino ShrinkWrap-style watertight wrap"]`. Names here are for
    /// documentation only and never appear as branding in the UI.
    pub emulates: Vec<String>,
    /// Path of the clean-room spec this node is built from, relative to
    /// the repo root, e.g. `docs/specs/rhino8.md`.
    pub spec: Option<String>,
    /// True while the node is a white-box stub: correct ports and parameters,
    /// placeholder algorithm.
    pub whitebox: bool,
    /// Name of an external open-source program this node drives, if any
    /// (`OpenFOAM`, `CalculiX`, `OrcaSlicer`, `COLMAP`...).
    pub external_tool: Option<String>,
}

impl NodeTypeInfo {
    pub fn new(id: &str, label: &str, stage: Stage) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            stage,
            inputs: vec![],
            outputs: vec![],
            params: vec![],
            help: String::new(),
            emulates: vec![],
            spec: None,
            whitebox: true,
            external_tool: None,
        }
    }

    pub fn input(mut self, port: PortSpec) -> Self {
        self.inputs.push(port);
        self
    }

    pub fn output(mut self, port: PortSpec) -> Self {
        self.outputs.push(port);
        self
    }

    pub fn param(mut self, p: ParamSpec) -> Self {
        self.params.push(p);
        self
    }

    pub fn help(mut self, h: &str) -> Self {
        self.help = h.into();
        self
    }

    pub fn emulates(mut self, what: &str) -> Self {
        self.emulates.push(what.into());
        self
    }

    pub fn spec(mut self, path: &str) -> Self {
        self.spec = Some(path.into());
        self
    }

    pub fn implemented(mut self) -> Self {
        self.whitebox = false;
        self
    }

    pub fn external(mut self, tool: &str) -> Self {
        self.external_tool = Some(tool.into());
        self
    }
}

/// Context handed to `cook`.
#[derive(Debug, Clone)]
pub struct CookContext {
    /// The instance name, for error messages.
    pub node_name: String,
    /// Scratch directory for this node's files (solver cases, exports).
    pub work_dir: PathBuf,
    /// Current study frame (the time slider; see `Graph::frame`), 0 outside a sweep.
    pub frame: f64,
}

impl CookContext {
    pub fn fail(&self, message: impl Into<String>) -> CookError {
        CookError::Failed {
            node: self.node_name.clone(),
            message: message.into(),
        }
    }
}

/// A node type. Implementations must be stateless: all state lives in
/// `Params` and inputs, so cooking is a pure function and can be cached.
pub trait NodeOp: Send + Sync {
    fn info(&self) -> NodeTypeInfo;

    /// `inputs[i]` is `None` when input port `i` is unconnected. Must return
    /// exactly one `Data` per output port, in order.
    fn cook(&self, ctx: &CookContext, inputs: &[Option<Data>], params: &Params) -> CookResult<Vec<Data>>;
}

/// Helpers for reading inputs inside `cook`.
pub mod inputs {
    use std::sync::Arc;

    use vernon_core::{Geometry, Report};

    use crate::{CookError, CookResult, Data, DataKind};

    pub fn geometry(inputs: &[Option<Data>], index: usize) -> CookResult<Arc<Geometry>> {
        match inputs.get(index).and_then(|d| d.as_ref()) {
            Some(Data::Geometry(g)) => Ok(g.clone()),
            Some(other) => Err(CookError::WrongInput {
                index,
                expected: DataKind::Geometry,
                found: other.kind(),
            }),
            None => Err(CookError::MissingInput {
                index,
                name: format!("input {index}"),
            }),
        }
    }

    pub fn opt_geometry(inputs: &[Option<Data>], index: usize) -> Option<Arc<Geometry>> {
        inputs.get(index).and_then(|d| d.as_ref()).and_then(|d| d.as_geometry().cloned())
    }

    pub fn report(inputs: &[Option<Data>], index: usize) -> CookResult<Arc<Report>> {
        match inputs.get(index).and_then(|d| d.as_ref()) {
            Some(Data::Report(r)) => Ok(r.clone()),
            Some(other) => Err(CookError::WrongInput {
                index,
                expected: DataKind::Report,
                found: other.kind(),
            }),
            None => Err(CookError::MissingInput {
                index,
                name: format!("input {index}"),
            }),
        }
    }

    pub fn opt_report(inputs: &[Option<Data>], index: usize) -> Option<Arc<Report>> {
        inputs.get(index).and_then(|d| d.as_ref()).and_then(|d| d.as_report().cloned())
    }
}
