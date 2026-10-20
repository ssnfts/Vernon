use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{NodeOp, NodeTypeInfo, Stage};

/// All known node types, keyed by `NodeTypeInfo::id`.
#[derive(Clone, Default)]
pub struct Registry {
    ops: BTreeMap<String, (NodeTypeInfo, Arc<dyn NodeOp>)>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a node type. Panics on duplicate ids, which is a programming
    /// error caught by every crate's registration test.
    pub fn add(&mut self, op: impl NodeOp + 'static) {
        let info = op.info();
        assert!(
            !self.ops.contains_key(&info.id),
            "duplicate node type id '{}'",
            info.id
        );
        self.ops.insert(info.id.clone(), (info, Arc::new(op)));
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn NodeOp>> {
        self.ops.get(id).map(|(_, op)| op)
    }

    pub fn info(&self, id: &str) -> Option<&NodeTypeInfo> {
        self.ops.get(id).map(|(info, _)| info)
    }

    pub fn types(&self) -> impl Iterator<Item = &NodeTypeInfo> {
        self.ops.values().map(|(info, _)| info)
    }

    pub fn by_stage(&self, stage: Stage) -> Vec<&NodeTypeInfo> {
        self.types().filter(|i| i.stage == stage).collect()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}
