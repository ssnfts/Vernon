//! Shared helpers for the node implementations.

use std::sync::Arc;

use vernon_core::Geometry;
use vernon_graph::node::inputs;
use vernon_graph::{CookContext, CookError, CookResult, Data, NodeTypeInfo, Stage};

use crate::mesh::element_size;

/// Clean-room spec every node of this crate is built from.
pub const SPEC: &str = "docs/specs/structural-fea.md";

/// Start a node description in stage 6 with the spec path set.
pub fn fea_info(id: &str, label: &str) -> NodeTypeInfo {
    NodeTypeInfo::new(id, label, Stage::Fea).spec(SPEC)
}

/// A solid tetrahedral mesh input (4 or 10 nodes per element).
pub fn solid_mesh(ctx: &CookContext, inputs: &[Option<Data>], index: usize) -> CookResult<Arc<Geometry>> {
    let g = inputs::geometry(inputs, index)?;
    element_size(&g).map_err(|e| ctx.fail(format!("input {index} is not a solid tetrahedral mesh: {e}")))?;
    Ok(g)
}

/// Write a text file into the node's work directory and return its path.
pub fn write_work_file(ctx: &CookContext, name: &str, text: &str) -> CookResult<std::path::PathBuf> {
    std::fs::create_dir_all(&ctx.work_dir)?;
    let p = ctx.work_dir.join(name);
    std::fs::write(&p, text).map_err(|e| CookError::Other(format!("cannot write {}: {e}", p.display())))?;
    Ok(p)
}

/// Make a string safe for use inside a file or deck set name.
pub fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    if t.is_empty() {
        "x".into()
    } else {
        t
    }
}

/// Helpers shared by the node tests.
#[cfg(test)]
pub mod testing {
    use vernon_graph::{CookContext, NodeOp, Params};

    pub fn ctx(tag: &str) -> CookContext {
        let d = std::env::temp_dir().join(format!("vernon_fea_{tag}_{}", std::process::id()));
        CookContext { node_name: tag.into(), work_dir: d, frame: 0.0 }
    }

    pub fn defaults(op: &dyn NodeOp) -> Params {
        Params::from_specs(&op.info().params)
    }
}
